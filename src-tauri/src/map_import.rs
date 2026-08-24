use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};
use tauri::{Emitter, Manager};
use tauri_plugin_dialog::DialogExt;

use crate::config::DataDirs;
use crate::map_context::{MapContextService, MapContextSnapshot};
use crate::map_model::{
    canonical_rows, hex_sha256, MapLayer, MaskGrid, RowSpan, SelectionMask, SelectionRole,
    TileRect, Tileset,
};
use crate::map_stamp::{compile_stamp_placement, StampDestination};
use crate::map_verify::MapRequestAuthority;

pub const MAX_IMPORT_MAP_BYTES: u64 = 256 * 1024 * 1024;
const IMPORT_PALETTE_SCHEMA: &str = "eud-map-import-palette/1";
const MAP_IMPORT_WINDOW_LABEL: &str = "map-import";
const MAP_AGENT_WINDOW_LABEL: &str = "map-agent";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapImportDestination {
    pub project_id: String,
    pub display_name: String,
    pub file_sha256: String,
    pub tileset: Tileset,
    pub width: u16,
    pub height: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapImportBootstrap {
    pub destination: MapImportDestination,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapImportSource {
    pub source_id: String,
    pub display_name: String,
    pub file_sha256: String,
    pub chk_sha256: String,
    pub tileset: Tileset,
    pub width: u16,
    pub height: u16,
    pub file_size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportedStamp {
    pub id: String,
    pub label: String,
    pub snapshot_hash: String,
    pub source_display_name: String,
    pub source_file_sha256: String,
    pub source_chk_sha256: String,
    pub source_extension: String,
    pub source_tileset: Tileset,
    pub source_width: u16,
    pub source_height: u16,
    pub bounds: TileRect,
    pub selected_cells: u32,
    pub rows: Vec<RowSpan>,
    pub layers: BTreeSet<MapLayer>,
    pub created_at: String,
}

impl ImportedStamp {
    pub fn selection(&self) -> Result<SelectionMask, String> {
        let selection = SelectionMask::canonical(
            self.id.clone(),
            self.label.clone(),
            format!("imported:{}", self.snapshot_hash),
            SelectionRole::Reference,
            self.layers.clone(),
            MaskGrid {
                width: self.source_width,
                height: self.source_height,
                rows: self.rows.clone(),
            },
        )?;
        if selection.bounds != self.bounds || selection.selected_cells != self.selected_cells {
            return Err("imported stamp geometry does not match its canonical rows".to_string());
        }
        Ok(selection)
    }

    fn expected_snapshot_hash(&self) -> Result<String, String> {
        snapshot_hash(&SnapshotHashInput {
            schema: IMPORT_PALETTE_SCHEMA,
            source_file_sha256: &self.source_file_sha256,
            source_chk_sha256: &self.source_chk_sha256,
            source_tileset: self.source_tileset,
            source_width: self.source_width,
            source_height: self.source_height,
            bounds: self.bounds,
            selected_cells: self.selected_cells,
            rows: &self.rows,
            layers: &self.layers,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ImportedStampLibrary {
    schema: String,
    entries: BTreeMap<String, ImportedStamp>,
}

impl Default for ImportedStampLibrary {
    fn default() -> Self {
        Self {
            schema: IMPORT_PALETTE_SCHEMA.to_string(),
            entries: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedStampView {
    #[serde(flatten)]
    pub stamp: ImportedStamp,
    pub available: bool,
    pub compatible: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum MapStampSourceRef {
    CandidateSelection {
        selection_id: String,
        snapshot_hash: String,
    },
    Imported {
        import_id: String,
        snapshot_hash: String,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapImportRenderCommand {
    pub source_id: String,
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    pub scale: u8,
    pub layers: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapImportObjectsCommand {
    pub source_id: String,
    pub layer: String,
    #[serde(default)]
    pub offset: u32,
    #[serde(default = "default_object_limit")]
    pub limit: u16,
}

const fn default_object_limit() -> u16 {
    100
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapImportSaveCommand {
    pub source_id: String,
    pub label: String,
    pub rows: Vec<RowSpan>,
    pub layers: BTreeSet<MapLayer>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapImportThumbnailCommand {
    pub import_id: String,
    #[serde(default = "thumbnail_scale")]
    pub scale: u8,
}

const fn thumbnail_scale() -> u8 {
    2
}

#[derive(Clone)]
pub struct MapImportStore {
    inner: Arc<MapImportStoreInner>,
}

struct MapImportStoreInner {
    dirs: DataDirs,
    context: MapContextService,
    staged: Mutex<HashMap<String, StagedSource>>,
    library_lock: Mutex<()>,
    active_blob_refs: Mutex<HashMap<String, usize>>,
}

#[derive(Clone)]
struct StagedSource {
    source: MapImportSource,
    source_extension: String,
    blob_path: PathBuf,
    destination: DestinationBinding,
}

#[derive(Clone)]
struct DestinationBinding {
    project_id: String,
    source_path: PathBuf,
    file_sha256: String,
    tileset: Tileset,
    width: u16,
    height: u16,
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedImportedStamp {
    pub stamp: ImportedStamp,
    pub blob_path: PathBuf,
}

#[derive(Serialize)]
struct SnapshotHashInput<'a> {
    schema: &'static str,
    source_file_sha256: &'a str,
    source_chk_sha256: &'a str,
    source_tileset: Tileset,
    source_width: u16,
    source_height: u16,
    bounds: TileRect,
    selected_cells: u32,
    rows: &'a [RowSpan],
    layers: &'a BTreeSet<MapLayer>,
}

impl MapImportStore {
    pub fn new(dirs: DataDirs) -> Self {
        Self {
            inner: Arc::new(MapImportStoreInner {
                context: MapContextService::new(dirs.clone()),
                dirs,
                staged: Mutex::new(HashMap::new()),
                library_lock: Mutex::new(()),
                active_blob_refs: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub fn bootstrap(&self) -> Result<MapImportBootstrap, String> {
        let current = self.inner.context.current()?;
        Ok(MapImportBootstrap {
            destination: destination_view(&current),
        })
    }

    pub fn cleanup_startup(&self) -> Result<usize, String> {
        let blobs = self.blobs_dir();
        std::fs::create_dir_all(&blobs)
            .map_err(|error| format!("map import blob directory could not be created: {error}"))?;
        let referenced = self.referenced_blob_hashes()?;
        let mut removed = 0;
        for item in std::fs::read_dir(&blobs)
            .map_err(|error| format!("map import blobs could not be inspected: {error}"))?
        {
            let item = item.map_err(|error| error.to_string())?;
            if !item
                .file_type()
                .map_err(|error| error.to_string())?
                .is_file()
            {
                continue;
            }
            let path = item.path();
            let extension = path
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            let stem = path
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            if extension.eq_ignore_ascii_case("tmp")
                || (extension.eq_ignore_ascii_case("map") && !referenced.contains(stem))
            {
                std::fs::remove_file(&path).map_err(|error| {
                    format!("unreferenced map import blob could not be removed: {error}")
                })?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    pub fn stage_source_path(&self, source_path: &Path) -> Result<MapImportSource, String> {
        let destination = self.inner.context.current()?;
        self.stage_source_for_destination(source_path, &destination, true)
    }

    fn stage_source_for_destination(
        &self,
        source_path: &Path,
        destination: &MapContextSnapshot,
        validate_render_surface: bool,
    ) -> Result<MapImportSource, String> {
        let canonical = source_path
            .canonicalize()
            .map_err(|error| format!("selected import source is unreadable: {error}"))?;
        let extension = allowed_extension(&canonical)?;
        let metadata = std::fs::metadata(&canonical)
            .map_err(|error| format!("selected import source metadata is unreadable: {error}"))?;
        if !metadata.is_file() {
            return Err("selected import source is not a regular file".to_string());
        }
        if metadata.len() > MAX_IMPORT_MAP_BYTES {
            return Err(format!(
                "selected import source exceeds the {} byte limit",
                MAX_IMPORT_MAP_BYTES
            ));
        }

        let source_id = uuid::Uuid::new_v4().to_string();
        let blobs = self.blobs_dir();
        std::fs::create_dir_all(&blobs)
            .map_err(|error| format!("map import blob directory could not be created: {error}"))?;
        let temporary = blobs.join(format!("{source_id}.tmp"));
        let copied = match stream_copy_and_hash(&canonical, &temporary) {
            Ok(value) => value,
            Err(error) => {
                let _ = std::fs::remove_file(&temporary);
                return Err(error);
            }
        };
        let result = (|| {
            if copied.length != metadata.len() || copied.length > MAX_IMPORT_MAP_BYTES {
                return Err(
                    "selected import source changed or exceeded the size limit while copying"
                        .to_string(),
                );
            }
            let parsed = validate_pinned_map(&temporary)?;
            if validate_render_surface {
                validate_render_and_objects(
                    &temporary,
                    &destination.starcraft_path,
                    parsed.width,
                    parsed.height,
                    &copied.sha256,
                )?;
            }
            let blob_path = self.blob_path(&copied.sha256);
            if blob_path.exists() {
                let existing = stream_hash(&blob_path)?;
                if existing.length != copied.length || existing.sha256 != copied.sha256 {
                    return Err(
                        "existing content-addressed import blob failed hash verification"
                            .to_string(),
                    );
                }
                std::fs::remove_file(&temporary).map_err(|error| {
                    format!("deduplicated import staging file could not be removed: {error}")
                })?;
            } else {
                std::fs::rename(&temporary, &blob_path).map_err(|error| {
                    format!("map import blob could not be promoted atomically: {error}")
                })?;
            }
            let display_name = canonical
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| "selected import source filename is not valid Unicode".to_string())?
                .to_string();
            let source = MapImportSource {
                source_id: source_id.clone(),
                display_name,
                file_sha256: copied.sha256.clone(),
                chk_sha256: parsed.chk_sha256,
                tileset: parsed.tileset,
                width: parsed.width,
                height: parsed.height,
                file_size: copied.length,
            };
            self.inner.staged.lock().insert(
                source_id,
                StagedSource {
                    source: source.clone(),
                    source_extension: extension,
                    blob_path,
                    destination: DestinationBinding {
                        project_id: destination.revision.project_id.clone(),
                        source_path: destination.revision.source_path.clone(),
                        file_sha256: destination.revision.file_sha256.clone(),
                        tileset: destination.revision.tileset,
                        width: destination.revision.width,
                        height: destination.revision.height,
                    },
                },
            );
            Ok(source)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result
    }

    pub fn render_source(&self, command: &MapImportRenderCommand) -> Result<Vec<u8>, String> {
        let staged = self.staged_source(&command.source_id)?;
        validate_crop(
            command.x,
            command.y,
            command.width,
            command.height,
            staged.source.width,
            staged.source.height,
        )?;
        let request = json!({
            "schema": "eud-map-render/1",
            "mode": "region",
            "x": command.x,
            "y": command.y,
            "width": command.width,
            "height": command.height,
            "scale": command.scale,
            "layers": command.layers,
        });
        let image = isom::render_region(
            &staged.blob_path,
            &self.inner.context.starcraft_path()?,
            request.to_string().as_bytes(),
        )
        .map_err(|error| format!("import source render failed: {error}"))?;
        crate::map_agent::encode_rgba_png(&image)
    }

    pub fn source_objects(&self, command: &MapImportObjectsCommand) -> Result<Value, String> {
        if command.limit == 0 || command.limit > 500 {
            return Err("object page limit must be 1..500".to_string());
        }
        if !matches!(
            command.layer.as_str(),
            "units" | "buildings" | "doodads" | "sprites" | "locations"
        ) {
            return Err(format!("unsupported map object layer '{}'", command.layer));
        }
        let staged = self.staged_source(&command.source_id)?;
        crate::tool_exec::map_objects_page(
            &staged.blob_path,
            &self.inner.context.starcraft_path()?,
            &format!("import-source:{}", staged.source.source_id),
            &staged.source.file_sha256,
            &command.layer,
            command.offset as usize,
            command.limit as usize,
        )
    }

    pub fn save_stamp(&self, command: MapImportSaveCommand) -> Result<ImportedStampView, String> {
        let staged = self.staged_source(&command.source_id)?;
        self.require_current_destination(&staged.destination)?;
        if staged.source.tileset != staged.destination.tileset {
            return Err("stamp source and destination tilesets do not match".to_string());
        }
        let label = command.label.trim();
        if label.is_empty() || label.chars().count() > 80 {
            return Err("imported stamp label must contain 1..80 characters".to_string());
        }
        let rows = canonical_rows(staged.source.width, staged.source.height, command.rows)?;
        if rows.is_empty() {
            return Err("imported stamp selection must not be empty".to_string());
        }
        let selection = SelectionMask::canonical(
            "staged-import",
            label,
            format!("import-source:{}", staged.source.source_id),
            SelectionRole::Reference,
            command.layers.clone(),
            MaskGrid {
                width: staged.source.width,
                height: staged.source.height,
                rows,
            },
        )?;
        let authority = MapRequestAuthority::calculate(
            "map-import".to_string(),
            "map-import-save".to_string(),
            0,
            staged.source.width,
            staged.source.height,
            Vec::new(),
            Vec::new(),
        )?;
        let _preview = compile_stamp_placement(
            &staged.blob_path,
            &staged.blob_path,
            &self.inner.context.starcraft_path()?,
            &selection,
            &[StampDestination {
                x: selection.bounds.left,
                y: selection.bounds.top,
            }],
            None,
            &authority,
        )?;
        let id = uuid::Uuid::new_v4().to_string();
        let created_at = crate::session::now_unix_seconds().to_string();
        let mut stamp = ImportedStamp {
            id: id.clone(),
            label: label.to_string(),
            snapshot_hash: String::new(),
            source_display_name: staged.source.display_name.clone(),
            source_file_sha256: staged.source.file_sha256.clone(),
            source_chk_sha256: staged.source.chk_sha256.clone(),
            source_extension: staged.source_extension,
            source_tileset: staged.source.tileset,
            source_width: staged.source.width,
            source_height: staged.source.height,
            bounds: selection.bounds,
            selected_cells: selection.selected_cells,
            rows: selection.rows,
            layers: selection.layers,
            created_at,
        };
        stamp.snapshot_hash = stamp.expected_snapshot_hash()?;
        let _library = self.inner.library_lock.lock();
        let mut library = self.read_library(&staged.destination.project_id)?;
        library.entries.insert(id, stamp.clone());
        self.write_library(&staged.destination.project_id, &library)?;
        Ok(ImportedStampView {
            stamp,
            available: true,
            compatible: true,
            unavailable_reason: None,
        })
    }

    pub fn list_current(&self) -> Result<Vec<ImportedStampView>, String> {
        let current = self.inner.context.current()?;
        self.list_for_destination(&current)
    }

    fn list_for_destination(
        &self,
        destination: &MapContextSnapshot,
    ) -> Result<Vec<ImportedStampView>, String> {
        let _library = self.inner.library_lock.lock();
        let library = self.read_library(&destination.revision.project_id)?;
        let mut values = Vec::with_capacity(library.entries.len());
        for stamp in library.entries.into_values() {
            let compatible = stamp.source_tileset == destination.revision.tileset
                && stamp.bounds.right - stamp.bounds.left <= destination.revision.width
                && stamp.bounds.bottom - stamp.bounds.top <= destination.revision.height;
            let blob_error = self.validate_entry_blob(&stamp).err();
            let available = blob_error.is_none();
            let unavailable_reason = blob_error.or_else(|| {
                (!compatible).then(|| {
                    if stamp.source_tileset != destination.revision.tileset {
                        "stamp source and destination tilesets do not match".to_string()
                    } else {
                        "imported stamp bounds are larger than the destination map".to_string()
                    }
                })
            });
            values.push(ImportedStampView {
                available,
                compatible,
                unavailable_reason,
                stamp,
            });
        }
        Ok(values)
    }

    pub fn delete_current(&self, import_id: &str) -> Result<(), String> {
        validate_component(import_id, "import id")?;
        let current = self.inner.context.current()?;
        let source_hash = {
            let _library = self.inner.library_lock.lock();
            let mut library = self.read_library(&current.revision.project_id)?;
            let removed = library
                .entries
                .remove(import_id)
                .ok_or_else(|| "imported stamp does not exist".to_string())?;
            self.write_library(&current.revision.project_id, &library)?;
            removed.source_file_sha256
        };
        self.gc_blob_if_unreferenced(&source_hash)
    }

    pub(crate) fn resolve_imported(
        &self,
        project_id: &str,
        import_id: &str,
        snapshot_hash: &str,
        destination_tileset: Tileset,
    ) -> Result<ResolvedImportedStamp, String> {
        validate_component(project_id, "project id")?;
        validate_component(import_id, "import id")?;
        let stamp = {
            let _library = self.inner.library_lock.lock();
            self.read_library(project_id)?
                .entries
                .get(import_id)
                .cloned()
                .ok_or_else(|| format!("imported stamp '{import_id}' no longer exists"))?
        };
        if stamp.snapshot_hash != snapshot_hash {
            return Err(format!("imported stamp '{import_id}' snapshot is stale"));
        }
        if stamp.source_tileset != destination_tileset {
            return Err("stamp source and destination tilesets do not match".to_string());
        }
        let blob_path = self.validate_entry_blob(&stamp)?;
        Ok(ResolvedImportedStamp { stamp, blob_path })
    }

    pub(crate) fn bind_blob(&self, file_sha256: &str) {
        let mut refs = self.inner.active_blob_refs.lock();
        *refs.entry(file_sha256.to_string()).or_default() += 1;
    }

    pub(crate) fn release_blob(&self, file_sha256: &str) {
        let remove = {
            let mut refs = self.inner.active_blob_refs.lock();
            match refs.get_mut(file_sha256) {
                Some(count) if *count > 1 => {
                    *count -= 1;
                    false
                }
                Some(_) => {
                    refs.remove(file_sha256);
                    true
                }
                None => false,
            }
        };
        if remove {
            let _ = self.gc_blob_if_unreferenced(file_sha256);
        }
    }

    pub(crate) fn compact_projection(
        &self,
        project_id: &str,
        import_id: &str,
        snapshot_hash: &str,
        destination_tileset: Tileset,
    ) -> Result<Value, String> {
        let resolved =
            self.resolve_imported(project_id, import_id, snapshot_hash, destination_tileset)?;
        let stamp = resolved.stamp;
        Ok(json!({
            "kind": "importedStamp",
            "importId": stamp.id,
            "label": stamp.label,
            "sourceMap": stamp.source_display_name,
            "tileset": stamp.source_tileset,
            "width": stamp.bounds.right - stamp.bounds.left,
            "height": stamp.bounds.bottom - stamp.bounds.top,
            "selectedCells": stamp.selected_cells,
            "layers": stamp.layers,
        }))
    }

    pub fn thumbnail(&self, command: &MapImportThumbnailCommand) -> Result<Vec<u8>, String> {
        if !matches!(command.scale, 1 | 2 | 4) {
            return Err("import thumbnail scale must be 1, 2, or 4".to_string());
        }
        let current = self.inner.context.current()?;
        let stamp = {
            let _library = self.inner.library_lock.lock();
            self.read_library(&current.revision.project_id)?
                .entries
                .get(&command.import_id)
                .cloned()
                .ok_or_else(|| "imported stamp does not exist".to_string())?
        };
        let blob = self.validate_entry_blob(&stamp)?;
        let request = json!({
            "schema": "eud-map-render/1",
            "mode": "region",
            "x": stamp.bounds.left,
            "y": stamp.bounds.top,
            "width": stamp.bounds.right - stamp.bounds.left,
            "height": stamp.bounds.bottom - stamp.bounds.top,
            "scale": command.scale,
            "layers": stamp.layers,
        });
        let image = isom::render_region(
            &blob,
            &current.starcraft_path,
            request.to_string().as_bytes(),
        )
        .map_err(|error| format!("imported stamp thumbnail failed: {error}"))?;
        crate::map_agent::encode_rgba_png(&image)
    }

    #[cfg(test)]
    pub(crate) fn insert_test_stamp(
        &self,
        project_id: &str,
        source_path: &Path,
        selection: &SelectionMask,
    ) -> Result<ImportedStamp, String> {
        let extension = allowed_extension(source_path)?;
        let parsed = validate_pinned_map(source_path)?;
        let canonical = SelectionMask::canonical(
            selection.id.clone(),
            selection.label.clone(),
            selection.source_revision.clone(),
            selection.role,
            selection.layers.clone(),
            MaskGrid {
                width: parsed.width,
                height: parsed.height,
                rows: selection.rows.clone(),
            },
        )?;
        if &canonical != selection {
            return Err("test imported selection is not canonical for the source map".to_string());
        }
        let digest = stream_hash(source_path)?;
        std::fs::create_dir_all(self.blobs_dir()).map_err(|error| error.to_string())?;
        let blob = self.blob_path(&digest.sha256);
        if !blob.exists() {
            std::fs::copy(source_path, &blob).map_err(|error| error.to_string())?;
        }
        let mut stamp = ImportedStamp {
            id: uuid::Uuid::new_v4().to_string(),
            label: selection.label.clone(),
            snapshot_hash: String::new(),
            source_display_name: source_path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("source.scx")
                .to_string(),
            source_file_sha256: digest.sha256,
            source_chk_sha256: parsed.chk_sha256,
            source_extension: extension,
            source_tileset: parsed.tileset,
            source_width: parsed.width,
            source_height: parsed.height,
            bounds: selection.bounds,
            selected_cells: selection.selected_cells,
            rows: selection.rows.clone(),
            layers: selection.layers.clone(),
            created_at: "test".to_string(),
        };
        stamp.snapshot_hash = stamp.expected_snapshot_hash()?;
        let _library = self.inner.library_lock.lock();
        let mut library = self.read_library(project_id)?;
        library.entries.insert(stamp.id.clone(), stamp.clone());
        self.write_library(project_id, &library)?;
        Ok(stamp)
    }

    #[cfg(test)]
    pub(crate) fn remove_test_stamp(
        &self,
        project_id: &str,
        import_id: &str,
    ) -> Result<(), String> {
        let source_hash = {
            let _library = self.inner.library_lock.lock();
            let mut library = self.read_library(project_id)?;
            let stamp = library
                .entries
                .remove(import_id)
                .ok_or_else(|| "test imported stamp does not exist".to_string())?;
            self.write_library(project_id, &library)?;
            stamp.source_file_sha256
        };
        self.gc_blob_if_unreferenced(&source_hash)
    }

    fn staged_source(&self, source_id: &str) -> Result<StagedSource, String> {
        validate_component(source_id, "source id")?;
        self.inner
            .staged
            .lock()
            .get(source_id)
            .cloned()
            .ok_or_else(|| "import source binding is stale; select the source again".to_string())
    }

    fn require_current_destination(&self, expected: &DestinationBinding) -> Result<(), String> {
        let current = self.inner.context.current()?;
        if current.revision.project_id != expected.project_id
            || current.revision.source_path != expected.source_path
            || current.revision.file_sha256 != expected.file_sha256
            || current.revision.tileset != expected.tileset
            || current.revision.width != expected.width
            || current.revision.height != expected.height
        {
            return Err(
                "map import destination changed; reopen the importer for the current saved map"
                    .to_string(),
            );
        }
        Ok(())
    }

    pub(crate) fn validate_resolved(&self, resolved: &ResolvedImportedStamp) -> Result<(), String> {
        let blob = self.validate_entry_blob(&resolved.stamp)?;
        if blob != resolved.blob_path {
            return Err("imported source blob binding changed".to_string());
        }
        Ok(())
    }

    fn validate_entry_blob(&self, stamp: &ImportedStamp) -> Result<PathBuf, String> {
        if stamp.expected_snapshot_hash()? != stamp.snapshot_hash {
            return Err("imported stamp snapshot hash does not match its content".to_string());
        }
        stamp.selection()?;
        let blob = self.blob_path(&stamp.source_file_sha256);
        if !blob.is_file() {
            return Err("imported source blob is missing".to_string());
        }
        let file = stream_hash(&blob)?;
        if file.sha256 != stamp.source_file_sha256 {
            return Err("imported source blob hash does not match its pinned hash".to_string());
        }
        let parsed = validate_pinned_map(&blob)?;
        if parsed.chk_sha256 != stamp.source_chk_sha256 {
            return Err("imported source CHK hash does not match its pinned hash".to_string());
        }
        if parsed.tileset != stamp.source_tileset
            || parsed.width != stamp.source_width
            || parsed.height != stamp.source_height
        {
            return Err("imported source metadata does not match its pinned snapshot".to_string());
        }
        Ok(blob)
    }

    fn library_path(&self, project_id: &str) -> Result<PathBuf, String> {
        validate_component(project_id, "project id")?;
        Ok(self
            .inner
            .dirs
            .map_candidates_dir()
            .join(project_id)
            .join("import-palette.json"))
    }

    fn read_library(&self, project_id: &str) -> Result<ImportedStampLibrary, String> {
        let path = self.library_path(project_id)?;
        let library = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<ImportedStampLibrary>(&bytes)
                .map_err(|error| format!("map import palette is invalid: {error}"))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ImportedStampLibrary::default())
            }
            Err(error) => return Err(format!("map import palette could not be read: {error}")),
        };
        validate_library(&library)?;
        Ok(library)
    }

    fn write_library(
        &self,
        project_id: &str,
        library: &ImportedStampLibrary,
    ) -> Result<(), String> {
        validate_library(library)?;
        let path = self.library_path(project_id)?;
        let bytes = serde_json::to_vec_pretty(library)
            .map_err(|error| format!("map import palette could not be serialized: {error}"))?;
        crate::memory::write_atomic_bytes(&path, &bytes)
            .map_err(|error| format!("map import palette could not be written atomically: {error}"))
    }

    fn referenced_blob_hashes(&self) -> Result<BTreeSet<String>, String> {
        let root = self.inner.dirs.map_candidates_dir();
        if !root.exists() {
            return Ok(BTreeSet::new());
        }
        let mut hashes = BTreeSet::new();
        for item in std::fs::read_dir(&root)
            .map_err(|error| format!("map import projects could not be inspected: {error}"))?
        {
            let item = item.map_err(|error| error.to_string())?;
            if !item
                .file_type()
                .map_err(|error| error.to_string())?
                .is_dir()
            {
                continue;
            }
            let project_id = item.file_name().to_string_lossy().to_string();
            let library = self.read_library(&project_id)?;
            hashes.extend(
                library
                    .entries
                    .into_values()
                    .map(|stamp| stamp.source_file_sha256),
            );
        }
        Ok(hashes)
    }

    fn gc_blob_if_unreferenced(&self, file_sha256: &str) -> Result<(), String> {
        if self
            .inner
            .active_blob_refs
            .lock()
            .get(file_sha256)
            .copied()
            .unwrap_or_default()
            > 0
            || self.referenced_blob_hashes()?.contains(file_sha256)
        {
            return Ok(());
        }
        match std::fs::remove_file(self.blob_path(file_sha256)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => {
                eprintln!("eud-agent: unreferenced map import blob GC deferred: {error}");
                Ok(())
            }
        }
    }

    fn blobs_dir(&self) -> PathBuf {
        self.inner.dirs.map_imports_dir().join("blobs")
    }

    fn blob_path(&self, file_sha256: &str) -> PathBuf {
        self.blobs_dir().join(format!("{file_sha256}.map"))
    }
}

fn destination_view(context: &MapContextSnapshot) -> MapImportDestination {
    MapImportDestination {
        project_id: context.revision.project_id.clone(),
        display_name: context
            .revision
            .source_path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("saved map")
            .to_string(),
        file_sha256: context.revision.file_sha256.clone(),
        tileset: context.revision.tileset,
        width: context.revision.width,
        height: context.revision.height,
    }
}

fn allowed_extension(path: &Path) -> Result<String, String> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if matches!(extension.as_str(), "scx" | "scm") {
        Ok(extension)
    } else {
        Err("map import accepts only .scx or .scm files".to_string())
    }
}

struct StreamDigest {
    length: u64,
    sha256: String,
}

fn stream_copy_and_hash(source: &Path, destination: &Path) -> Result<StreamDigest, String> {
    let input = File::open(source)
        .map_err(|error| format!("selected import source could not be opened: {error}"))?;
    let output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| format!("map import staging file could not be created: {error}"))?;
    let mut reader = BufReader::with_capacity(1024 * 1024, input);
    let mut writer = BufWriter::with_capacity(1024 * 1024, output);
    let mut buffer = vec![0u8; 1024 * 1024];
    let mut digest = Sha256::new();
    let mut length = 0u64;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("selected import source could not be copied: {error}"))?;
        if read == 0 {
            break;
        }
        length = length
            .checked_add(read as u64)
            .ok_or_else(|| "selected import source length overflow".to_string())?;
        if length > MAX_IMPORT_MAP_BYTES {
            return Err(format!(
                "selected import source exceeds the {} byte limit",
                MAX_IMPORT_MAP_BYTES
            ));
        }
        digest.update(&buffer[..read]);
        writer
            .write_all(&buffer[..read])
            .map_err(|error| format!("map import staging file could not be written: {error}"))?;
    }
    writer
        .flush()
        .map_err(|error| format!("map import staging file could not be flushed: {error}"))?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|error| format!("map import staging file could not be synced: {error}"))?;
    Ok(StreamDigest {
        length,
        sha256: format!("{:x}", digest.finalize()),
    })
}

fn stream_hash(path: &Path) -> Result<StreamDigest, String> {
    let input = File::open(path)
        .map_err(|error| format!("map import blob could not be opened: {error}"))?;
    let mut reader = BufReader::with_capacity(1024 * 1024, input);
    let mut buffer = vec![0u8; 1024 * 1024];
    let mut digest = Sha256::new();
    let mut length = 0u64;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("map import blob could not be hashed: {error}"))?;
        if read == 0 {
            break;
        }
        length = length
            .checked_add(read as u64)
            .ok_or_else(|| "map import blob length overflow".to_string())?;
        if length > MAX_IMPORT_MAP_BYTES {
            return Err("map import blob exceeds the size limit".to_string());
        }
        digest.update(&buffer[..read]);
    }
    Ok(StreamDigest {
        length,
        sha256: format!("{:x}", digest.finalize()),
    })
}

#[derive(Debug)]
struct ParsedPinnedMap {
    chk_sha256: String,
    tileset: Tileset,
    width: u16,
    height: u16,
}

fn validate_pinned_map(path: &Path) -> Result<ParsedPinnedMap, String> {
    let chk = isom::chk_extract(path).map_err(|error| {
        format!("selected SCX/SCM scenario.chk could not be extracted: {error}")
    })?;
    validate_chk_snapshot(&chk)
}

fn validate_chk_snapshot(chk: &[u8]) -> Result<ParsedPinnedMap, String> {
    let sections = crate::chk::assemble_sections(&crate::chk::walk_sections(chk));
    let dim = sections
        .get("DIM ")
        .ok_or_else(|| "selected map has no DIM section".to_string())?;
    let era = sections
        .get("ERA ")
        .ok_or_else(|| "selected map has no ERA section".to_string())?;
    let mtxm = sections
        .get("MTXM")
        .or_else(|| sections.get("TILE"))
        .ok_or_else(|| "selected map has no MTXM/TILE section".to_string())?;
    if dim.len() < 4 || era.len() < 2 {
        return Err("selected map DIM/ERA section is truncated".to_string());
    }
    let width = u16::from_le_bytes([dim[0], dim[1]]);
    let height = u16::from_le_bytes([dim[2], dim[3]]);
    if width == 0 || height == 0 {
        return Err("selected map dimensions are empty".to_string());
    }
    let required_tiles = usize::from(width)
        .checked_mul(usize::from(height))
        .and_then(|cells| cells.checked_mul(2))
        .ok_or_else(|| "selected map terrain dimensions overflow".to_string())?;
    if mtxm.len() < required_tiles {
        return Err("selected map MTXM/TILE section is truncated".to_string());
    }
    let era = u16::from_le_bytes([era[0], era[1]]);
    Ok(ParsedPinnedMap {
        chk_sha256: hex_sha256(chk),
        tileset: Tileset::from_era(era)?,
        width,
        height,
    })
}

fn validate_render_and_objects(
    path: &Path,
    starcraft_path: &Path,
    width: u16,
    height: u16,
    file_hash: &str,
) -> Result<(), String> {
    let request = json!({
        "schema": "eud-map-render/1",
        "mode": "region",
        "x": 0,
        "y": 0,
        "width": width.min(1),
        "height": height.min(1),
        "scale": 1,
        "layers": ["terrain"],
    });
    isom::render_region(path, starcraft_path, request.to_string().as_bytes())
        .map_err(|error| format!("selected map render snapshot is invalid: {error}"))?;
    crate::tool_exec::map_object_snapshot(path, starcraft_path, "import-validation", file_hash)
        .map_err(|error| format!("selected map object snapshot is invalid: {error}"))?;
    Ok(())
}

fn validate_crop(
    x: u16,
    y: u16,
    width: u16,
    height: u16,
    map_width: u16,
    map_height: u16,
) -> Result<(), String> {
    if width == 0
        || height == 0
        || x.saturating_add(width) > map_width
        || y.saturating_add(height) > map_height
    {
        return Err("render crop is outside import source dimensions".to_string());
    }
    Ok(())
}

fn snapshot_hash(input: &SnapshotHashInput<'_>) -> Result<String, String> {
    let bytes = serde_json::to_vec(input)
        .map_err(|error| format!("imported stamp snapshot could not be serialized: {error}"))?;
    Ok(hex_sha256(&bytes))
}

fn validate_library(library: &ImportedStampLibrary) -> Result<(), String> {
    if library.schema != IMPORT_PALETTE_SCHEMA {
        return Err("map import palette schema is unsupported".to_string());
    }
    for (key, stamp) in &library.entries {
        if key != &stamp.id {
            return Err("map import palette key does not match its imported stamp id".to_string());
        }
        validate_component(key, "import id")?;
        stamp.selection()?;
        if stamp.expected_snapshot_hash()? != stamp.snapshot_hash {
            return Err("map import palette contains a stale snapshot hash".to_string());
        }
        if !matches!(stamp.source_extension.as_str(), "scx" | "scm") {
            return Err("map import palette contains an unsupported source extension".to_string());
        }
    }
    Ok(())
}

fn validate_component(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(format!("invalid {label}"));
    }
    Ok(())
}

#[tauri::command]
pub async fn map_agent_import_open(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
) -> Result<(), String> {
    require_window(&window, &[MAP_AGENT_WINDOW_LABEL])?;
    if let Some(importer) = app.get_webview_window(MAP_IMPORT_WINDOW_LABEL) {
        importer.show().map_err(|error| error.to_string())?;
        importer.set_focus().map_err(|error| error.to_string())?;
        return Ok(());
    }
    tauri::WebviewWindowBuilder::new(
        &app,
        MAP_IMPORT_WINDOW_LABEL,
        tauri::WebviewUrl::App("map-import.html".into()),
    )
    .title("Map Importer")
    .inner_size(1500.0, 900.0)
    .min_inner_size(1100.0, 700.0)
    .resizable(true)
    .drag_and_drop(false)
    .build()
    .map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn map_import_bootstrap(
    window: tauri::WebviewWindow,
    store: tauri::State<'_, MapImportStore>,
) -> Result<MapImportBootstrap, String> {
    require_window(&window, &[MAP_IMPORT_WINDOW_LABEL])?;
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || store.bootstrap())
        .await
        .map_err(|error| format!("map import bootstrap task failed: {error}"))?
}

#[tauri::command]
pub async fn map_import_source_pick(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    store: tauri::State<'_, MapImportStore>,
) -> Result<Option<MapImportSource>, String> {
    require_window(&window, &[MAP_IMPORT_WINDOW_LABEL])?;
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let picked = app
            .dialog()
            .file()
            .add_filter("StarCraft maps", &["scx", "scm"])
            .blocking_pick_file();
        let Some(picked) = picked else {
            return Ok(None);
        };
        let path = picked.into_path().map_err(|error| error.to_string())?;
        store.stage_source_path(&path).map(Some)
    })
    .await
    .map_err(|error| format!("map import source task failed: {error}"))?
}

#[tauri::command]
pub async fn map_import_source_render(
    window: tauri::WebviewWindow,
    store: tauri::State<'_, MapImportStore>,
    command: MapImportRenderCommand,
) -> Result<tauri::ipc::Response, String> {
    require_window(&window, &[MAP_IMPORT_WINDOW_LABEL])?;
    let store = store.inner().clone();
    let bytes = tauri::async_runtime::spawn_blocking(move || store.render_source(&command))
        .await
        .map_err(|error| format!("map import render task failed: {error}"))??;
    Ok(tauri::ipc::Response::new(bytes))
}

#[tauri::command]
pub async fn map_import_source_objects(
    window: tauri::WebviewWindow,
    store: tauri::State<'_, MapImportStore>,
    command: MapImportObjectsCommand,
) -> Result<Value, String> {
    require_window(&window, &[MAP_IMPORT_WINDOW_LABEL])?;
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || store.source_objects(&command))
        .await
        .map_err(|error| format!("map import objects task failed: {error}"))?
}

#[tauri::command]
pub async fn map_import_stamp_save(
    window: tauri::WebviewWindow,
    store: tauri::State<'_, MapImportStore>,
    command: MapImportSaveCommand,
) -> Result<ImportedStampView, String> {
    require_window(&window, &[MAP_IMPORT_WINDOW_LABEL])?;
    let store = store.inner().clone();
    let view = tauri::async_runtime::spawn_blocking(move || store.save_stamp(command))
        .await
        .map_err(|error| format!("map import save task failed: {error}"))??;
    window
        .app_handle()
        .emit("map-import-palette-changed", &view.stamp.id)
        .map_err(|error| error.to_string())?;
    Ok(view)
}

#[tauri::command]
pub async fn map_import_stamp_list(
    window: tauri::WebviewWindow,
    store: tauri::State<'_, MapImportStore>,
) -> Result<Vec<ImportedStampView>, String> {
    require_window(&window, &[MAP_AGENT_WINDOW_LABEL, MAP_IMPORT_WINDOW_LABEL])?;
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || store.list_current())
        .await
        .map_err(|error| format!("map import list task failed: {error}"))?
}

#[tauri::command]
pub async fn map_import_stamp_thumbnail(
    window: tauri::WebviewWindow,
    store: tauri::State<'_, MapImportStore>,
    command: MapImportThumbnailCommand,
) -> Result<tauri::ipc::Response, String> {
    require_window(&window, &[MAP_AGENT_WINDOW_LABEL, MAP_IMPORT_WINDOW_LABEL])?;
    let store = store.inner().clone();
    let bytes = tauri::async_runtime::spawn_blocking(move || store.thumbnail(&command))
        .await
        .map_err(|error| format!("map import thumbnail task failed: {error}"))??;
    Ok(tauri::ipc::Response::new(bytes))
}

#[tauri::command]
pub async fn map_import_stamp_delete(
    window: tauri::WebviewWindow,
    store: tauri::State<'_, MapImportStore>,
    import_id: String,
) -> Result<(), String> {
    require_window(&window, &[MAP_AGENT_WINDOW_LABEL, MAP_IMPORT_WINDOW_LABEL])?;
    let store = store.inner().clone();
    let event_id = import_id.clone();
    tauri::async_runtime::spawn_blocking(move || store.delete_current(&import_id))
        .await
        .map_err(|error| format!("map import delete task failed: {error}"))??;
    window
        .app_handle()
        .emit("map-import-palette-changed", &event_id)
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn require_window(window: &tauri::WebviewWindow, allowed: &[&str]) -> Result<(), String> {
    if allowed.contains(&window.label()) {
        Ok(())
    } else {
        Err("trusted map import command rejected outside its authorized window".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs(name: &str) -> DataDirs {
        let root =
            std::env::temp_dir().join(format!("eud-map-import-{name}-{}", uuid::Uuid::new_v4()));
        DataDirs::from_bases(&root.join("roaming"), &root.join("local"))
    }

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("crates")
            .join("isom")
            .join("tests")
            .join("fixtures")
            .join("sample.scx")
    }

    fn chk_section(chk: &mut Vec<u8>, name: &[u8; 4], data: &[u8]) {
        chk.extend_from_slice(name);
        chk.extend_from_slice(&(data.len() as u32).to_le_bytes());
        chk.extend_from_slice(data);
    }

    #[test]
    fn extension_allowlist_is_case_insensitive_and_rejects_raw_chk() {
        assert_eq!(allowed_extension(Path::new("map.SCX")).unwrap(), "scx");
        assert_eq!(allowed_extension(Path::new("map.sCm")).unwrap(), "scm");
        assert!(allowed_extension(Path::new("scenario.chk")).is_err());
        assert!(allowed_extension(Path::new("map.zip")).is_err());
    }

    #[test]
    fn direct_stamp_source_ref_uses_strict_camel_case_shapes() {
        assert_eq!(
            serde_json::from_value::<MapStampSourceRef>(json!({
                "kind": "candidateSelection",
                "selectionId": "selection",
                "snapshotHash": "snapshot"
            }))
            .unwrap(),
            MapStampSourceRef::CandidateSelection {
                selection_id: "selection".to_string(),
                snapshot_hash: "snapshot".to_string(),
            }
        );
        assert_eq!(
            serde_json::from_value::<MapStampSourceRef>(json!({
                "kind": "imported",
                "importId": "import",
                "snapshotHash": "snapshot"
            }))
            .unwrap(),
            MapStampSourceRef::Imported {
                import_id: "import".to_string(),
                snapshot_hash: "snapshot".to_string(),
            }
        );
        assert!(serde_json::from_value::<MapStampSourceRef>(json!({
            "kind": "imported",
            "importId": "import",
            "snapshotHash": "snapshot",
            "path": "forbidden.scx"
        }))
        .is_err());
    }

    #[test]
    fn valid_scx_and_scm_are_pinned_with_persisted_hashes_and_deduplicated() {
        let dirs = dirs("pin");
        dirs.ensure_dirs().unwrap();
        let first = dirs.app_data().join("source.SCX");
        let second = dirs.app_data().join("copy.sCm");
        std::fs::copy(fixture(), &first).unwrap();
        std::fs::copy(fixture(), &second).unwrap();
        let bytes = std::fs::read(&first).unwrap();
        let store = MapImportStore::new(dirs.clone());
        let first_source = store
            .stage_source_for_destination(&first, &fake_destination(&dirs), false)
            .unwrap();
        let second_source = store
            .stage_source_for_destination(&second, &fake_destination(&dirs), false)
            .unwrap();
        assert_eq!(first_source.file_sha256, hex_sha256(&bytes));
        assert_eq!(first_source.file_sha256, second_source.file_sha256);
        assert_eq!(first_source.chk_sha256, second_source.chk_sha256);
        assert!(store.blob_path(&first_source.file_sha256).is_file());
        let blobs = std::fs::read_dir(store.blobs_dir())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|value| value == "map"))
            .count();
        assert_eq!(blobs, 1);
        std::fs::remove_dir_all(dirs.app_data().parent().unwrap()).ok();
    }

    #[test]
    fn corrupt_container_is_refused_and_staging_temp_is_removed() {
        let dirs = dirs("corrupt");
        dirs.ensure_dirs().unwrap();
        let source = dirs.app_data().join("corrupt.scx");
        std::fs::write(&source, b"not an MPQ container").unwrap();
        let store = MapImportStore::new(dirs.clone());
        assert!(store
            .stage_source_for_destination(&source, &fake_destination(&dirs), false)
            .unwrap_err()
            .contains("scenario.chk"));
        let temporary = std::fs::read_dir(store.blobs_dir())
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry.path().extension().is_some_and(|value| value == "tmp"));
        assert!(!temporary);
        std::fs::remove_dir_all(dirs.app_data().parent().unwrap()).ok();
    }

    #[test]
    fn missing_and_truncated_dim_era_mtxm_are_refused_before_storage() {
        assert!(validate_chk_snapshot(&[]).unwrap_err().contains("DIM"));
        let mut truncated_dim = Vec::new();
        chk_section(&mut truncated_dim, b"DIM ", &[2, 0]);
        chk_section(&mut truncated_dim, b"ERA ", &[4, 0]);
        chk_section(&mut truncated_dim, b"MTXM", &[0; 8]);
        assert!(validate_chk_snapshot(&truncated_dim)
            .unwrap_err()
            .contains("truncated"));

        let mut truncated_terrain = Vec::new();
        chk_section(&mut truncated_terrain, b"DIM ", &[2, 0, 2, 0]);
        chk_section(&mut truncated_terrain, b"ERA ", &[4, 0]);
        chk_section(&mut truncated_terrain, b"MTXM", &[0; 6]);
        assert!(validate_chk_snapshot(&truncated_terrain)
            .unwrap_err()
            .contains("MTXM/TILE"));

        let mut valid = Vec::new();
        chk_section(&mut valid, b"DIM ", &[2, 0, 2, 0]);
        chk_section(&mut valid, b"ERA ", &[4, 0]);
        chk_section(&mut valid, b"MTXM", &[0; 8]);
        let parsed = validate_chk_snapshot(&valid).unwrap();
        assert_eq!((parsed.width, parsed.height), (2, 2));
        assert_eq!(parsed.chk_sha256, hex_sha256(&valid));
    }

    #[test]
    fn streaming_copy_hashes_exact_bytes_and_enforces_pre_copy_cap() {
        let dirs = dirs("stream");
        dirs.ensure_dirs().unwrap();
        let source = dirs.app_data().join("source.scx");
        let destination = dirs.map_imports_dir().join("copy.tmp");
        let bytes = (0..=255).cycle().take(2_500_000).collect::<Vec<_>>();
        std::fs::write(&source, &bytes).unwrap();
        let copied = stream_copy_and_hash(&source, &destination).unwrap();
        assert_eq!(copied.length, bytes.len() as u64);
        assert_eq!(copied.sha256, hex_sha256(&bytes));
        assert_eq!(std::fs::read(destination).unwrap(), bytes);
        let oversized = dirs.app_data().join("oversized.scx");
        let file = File::create(&oversized).unwrap();
        file.set_len(MAX_IMPORT_MAP_BYTES + 1).unwrap();
        let store = MapImportStore::new(dirs.clone());
        let metadata = std::fs::metadata(&oversized).unwrap();
        assert!(metadata.len() > MAX_IMPORT_MAP_BYTES);
        assert!(store
            .stage_source_for_destination(&oversized, &fake_destination(&dirs), false,)
            .unwrap_err()
            .contains("limit"));
        std::fs::remove_dir_all(dirs.app_data().parent().unwrap()).ok();
    }

    #[test]
    fn strict_palette_rejects_unknown_fields_and_key_id_mismatch() {
        let dirs = dirs("schema");
        dirs.ensure_dirs().unwrap();
        let store = MapImportStore::new(dirs.clone());
        let project = "a".repeat(64);
        let path = store.library_path(&project).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            br#"{"schema":"eud-map-import-palette/1","entries":{},"extra":true}"#,
        )
        .unwrap();
        assert!(store
            .read_library(&project)
            .unwrap_err()
            .contains("unknown field"));
        let mut library = ImportedStampLibrary::default();
        let stamp = test_stamp("id-a", "hash");
        library.entries.insert("id-b".to_string(), stamp);
        assert!(validate_library(&library).unwrap_err().contains("key"));
        std::fs::remove_dir_all(dirs.app_data().parent().unwrap()).ok();
    }

    #[test]
    fn imported_resolution_rejects_stale_snapshot_project_and_tileset_before_blob_use() {
        let dirs = dirs("resolution");
        dirs.ensure_dirs().unwrap();
        let store = MapImportStore::new(dirs.clone());
        let project = "a".repeat(64);
        let mut stamp = test_stamp("id-a", &"b".repeat(64));
        stamp.snapshot_hash = stamp.expected_snapshot_hash().unwrap();
        let mut library = ImportedStampLibrary::default();
        library.entries.insert(stamp.id.clone(), stamp.clone());
        store.write_library(&project, &library).unwrap();
        assert!(store
            .resolve_imported(&project, &stamp.id, "stale", Tileset::Jungle)
            .unwrap_err()
            .contains("snapshot is stale"));
        assert!(store
            .resolve_imported(
                &"c".repeat(64),
                &stamp.id,
                &stamp.snapshot_hash,
                Tileset::Jungle
            )
            .unwrap_err()
            .contains("no longer exists"));
        assert_eq!(
            store
                .resolve_imported(&project, &stamp.id, &stamp.snapshot_hash, Tileset::Desert,)
                .unwrap_err(),
            "stamp source and destination tilesets do not match"
        );
        std::fs::remove_dir_all(dirs.app_data().parent().unwrap()).ok();
    }

    #[test]
    fn canonical_import_selection_rejects_empty_and_out_of_bounds_rows() {
        assert!(canonical_rows(8, 8, Vec::new()).unwrap().is_empty());
        assert!(canonical_rows(
            8,
            8,
            vec![RowSpan {
                y: 8,
                spans: vec![(0, 1)],
            }],
        )
        .is_err());
    }

    #[test]
    fn identical_content_uses_one_blob_and_missing_or_corrupt_blob_is_unavailable() {
        let dirs = dirs("blob");
        dirs.ensure_dirs().unwrap();
        let store = MapImportStore::new(dirs.clone());
        std::fs::create_dir_all(store.blobs_dir()).unwrap();
        let bytes = b"same source bytes";
        let hash = hex_sha256(bytes);
        let blob = store.blob_path(&hash);
        std::fs::write(&blob, bytes).unwrap();
        assert_eq!(stream_hash(&blob).unwrap().sha256, hash);
        std::fs::remove_file(&blob).unwrap();
        let mut stamp = test_stamp("id-a", &hash);
        stamp.snapshot_hash = stamp.expected_snapshot_hash().unwrap();
        assert!(store
            .validate_entry_blob(&stamp)
            .unwrap_err()
            .contains("missing"));
        std::fs::write(&blob, b"corrupt").unwrap();
        assert!(store
            .validate_entry_blob(&stamp)
            .unwrap_err()
            .contains("hash"));
        std::fs::remove_dir_all(dirs.app_data().parent().unwrap()).ok();
    }

    #[test]
    fn project_libraries_are_isolated_and_delete_is_atomic() {
        let dirs = dirs("projects");
        dirs.ensure_dirs().unwrap();
        let store = MapImportStore::new(dirs.clone());
        let project_a = "a".repeat(64);
        let project_b = "b".repeat(64);
        let mut library = ImportedStampLibrary::default();
        let mut stamp = test_stamp("id-a", &"c".repeat(64));
        stamp.snapshot_hash = stamp.expected_snapshot_hash().unwrap();
        library.entries.insert(stamp.id.clone(), stamp);
        store.write_library(&project_a, &library).unwrap();
        assert_eq!(store.read_library(&project_a).unwrap().entries.len(), 1);
        assert!(store.read_library(&project_b).unwrap().entries.is_empty());
        library.entries.clear();
        store.write_library(&project_a, &library).unwrap();
        assert!(store.read_library(&project_a).unwrap().entries.is_empty());
        std::fs::remove_dir_all(dirs.app_data().parent().unwrap()).ok();
    }

    #[test]
    fn referenced_blob_survives_cleanup_and_unreferenced_temp_and_blob_are_removed() {
        let dirs = dirs("cleanup");
        dirs.ensure_dirs().unwrap();
        let store = MapImportStore::new(dirs.clone());
        std::fs::create_dir_all(store.blobs_dir()).unwrap();
        let project = "a".repeat(64);
        let referenced_hash = "b".repeat(64);
        let mut stamp = test_stamp("id-a", &referenced_hash);
        stamp.snapshot_hash = stamp.expected_snapshot_hash().unwrap();
        let mut library = ImportedStampLibrary::default();
        library.entries.insert(stamp.id.clone(), stamp);
        store.write_library(&project, &library).unwrap();
        std::fs::write(store.blob_path(&referenced_hash), b"referenced").unwrap();
        let unused_hash = "c".repeat(64);
        std::fs::write(store.blob_path(&unused_hash), b"unused").unwrap();
        std::fs::write(store.blobs_dir().join("stale.tmp"), b"temp").unwrap();
        assert_eq!(store.cleanup_startup().unwrap(), 2);
        assert!(store.blob_path(&referenced_hash).exists());
        std::fs::remove_dir_all(dirs.app_data().parent().unwrap()).ok();
    }

    #[test]
    fn model_projection_contains_no_paths_or_raw_content() {
        let mut stamp = test_stamp("id-a", &"d".repeat(64));
        stamp.snapshot_hash = stamp.expected_snapshot_hash().unwrap();
        let value = json!({
            "kind": "importedStamp",
            "importId": stamp.id,
            "label": stamp.label,
            "sourceMap": stamp.source_display_name,
            "tileset": stamp.source_tileset,
            "width": stamp.bounds.right - stamp.bounds.left,
            "height": stamp.bounds.bottom - stamp.bounds.top,
            "selectedCells": stamp.selected_cells,
            "layers": stamp.layers,
        });
        let text = value.to_string();
        for forbidden in ["path", "blob", "chk", "mtxm", "\"tiles\""] {
            assert!(!text.to_ascii_lowercase().contains(forbidden));
        }
    }

    fn test_stamp(id: &str, file_hash: &str) -> ImportedStamp {
        ImportedStamp {
            id: id.to_string(),
            label: "언덕 입구".to_string(),
            snapshot_hash: String::new(),
            source_display_name: "source.scx".to_string(),
            source_file_sha256: file_hash.to_string(),
            source_chk_sha256: "e".repeat(64),
            source_extension: "scx".to_string(),
            source_tileset: Tileset::Jungle,
            source_width: 8,
            source_height: 8,
            bounds: TileRect {
                left: 1,
                top: 1,
                right: 3,
                bottom: 3,
            },
            selected_cells: 4,
            rows: vec![
                RowSpan {
                    y: 1,
                    spans: vec![(1, 3)],
                },
                RowSpan {
                    y: 2,
                    spans: vec![(1, 3)],
                },
            ],
            layers: BTreeSet::from([MapLayer::Terrain]),
            created_at: "1".to_string(),
        }
    }

    fn fake_destination(dirs: &DataDirs) -> MapContextSnapshot {
        MapContextSnapshot {
            revision: crate::map_model::MapRevision {
                project_id: "a".repeat(64),
                source_path: dirs.app_data().join("destination.scx"),
                file_sha256: "f".repeat(64),
                chk_sha256: "e".repeat(64),
                mtime_ns: 0,
                tileset: Tileset::Jungle,
                width: 8,
                height: 8,
            },
            saved_source_notice: String::new(),
            source_file_size: 0,
            starcraft_path: PathBuf::new(),

            digest: crate::chk::digest_chk(&[]),
        }
    }
    #[test]
    #[ignore = "requires a live saved OpenMapName, installed StarCraft assets, and MAP_IMPORT_SMOKE_SOURCE"]
    fn real_cross_dimension_source_stages_saves_and_preserves_both_originals() {
        let source_path = PathBuf::from(
            std::env::var("MAP_IMPORT_SMOKE_SOURCE")
                .expect("MAP_IMPORT_SMOKE_SOURCE must name a real SCX/SCM"),
        );
        let roaming = PathBuf::from(std::env::var("APPDATA").expect("APPDATA is required"));
        let local = PathBuf::from(std::env::var("LOCALAPPDATA").expect("LOCALAPPDATA is required"));
        let dirs = DataDirs::from_bases(&roaming, &local);
        let store = MapImportStore::new(dirs);
        let destination = store.inner.context.current().unwrap();
        let source_before = std::fs::read(&source_path).unwrap();
        let destination_before = std::fs::read(&destination.revision.source_path).unwrap();
        let source = store.stage_source_path(&source_path).unwrap();
        assert_eq!(source.tileset, destination.revision.tileset);
        assert_ne!(
            (source.width, source.height),
            (destination.revision.width, destination.revision.height)
        );
        let saved = store
            .save_stamp(MapImportSaveCommand {
                source_id: source.source_id,
                label: "real smoke import".to_string(),
                rows: vec![
                    RowSpan {
                        y: 0,
                        spans: vec![(0, 2)],
                    },
                    RowSpan {
                        y: 1,
                        spans: vec![(0, 2)],
                    },
                ],
                layers: [
                    MapLayer::Terrain,
                    MapLayer::Units,
                    MapLayer::Buildings,
                    MapLayer::Doodads,
                    MapLayer::Sprites,
                    MapLayer::Locations,
                ]
                .into_iter()
                .collect(),
            })
            .unwrap();
        assert!(store
            .list_current()
            .unwrap()
            .iter()
            .any(|entry| entry.stamp.id == saved.stamp.id && entry.compatible));
        store.delete_current(&saved.stamp.id).unwrap();
        assert_eq!(std::fs::read(&source_path).unwrap(), source_before);
        assert_eq!(
            std::fs::read(&destination.revision.source_path).unwrap(),
            destination_before
        );
    }

    #[test]
    #[ignore = "requires a live saved OpenMapName, installed StarCraft assets, and MAP_IMPORT_MISMATCH_SOURCE"]
    fn real_cross_tileset_source_is_rejected_without_palette_or_original_mutation() {
        let source_path = PathBuf::from(
            std::env::var("MAP_IMPORT_MISMATCH_SOURCE")
                .expect("MAP_IMPORT_MISMATCH_SOURCE must name a real SCX/SCM"),
        );
        let roaming = PathBuf::from(std::env::var("APPDATA").expect("APPDATA is required"));
        let local = PathBuf::from(std::env::var("LOCALAPPDATA").expect("LOCALAPPDATA is required"));
        let dirs = DataDirs::from_bases(&roaming, &local);
        let store = MapImportStore::new(dirs);
        let destination = store.inner.context.current().unwrap();
        let source_before = std::fs::read(&source_path).unwrap();
        let destination_before = std::fs::read(&destination.revision.source_path).unwrap();
        let entries_before = store.list_current().unwrap().len();
        let source = store.stage_source_path(&source_path).unwrap();
        assert_ne!(source.tileset, destination.revision.tileset);
        let error = store
            .save_stamp(MapImportSaveCommand {
                source_id: source.source_id,
                label: "mismatch smoke".to_string(),
                rows: vec![RowSpan {
                    y: 0,
                    spans: vec![(0, 1)],
                }],
                layers: [MapLayer::Terrain].into_iter().collect(),
            })
            .unwrap_err();
        assert_eq!(error, "stamp source and destination tilesets do not match");
        assert_eq!(store.list_current().unwrap().len(), entries_before);
        assert_eq!(std::fs::read(&source_path).unwrap(), source_before);
        assert_eq!(
            std::fs::read(&destination.revision.source_path).unwrap(),
            destination_before
        );
        store.cleanup_startup().unwrap();
    }
}
