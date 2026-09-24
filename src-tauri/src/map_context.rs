use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::Serialize;

use crate::config::DataDirs;
use crate::map_model::{hex_sha256, MapRevision, Tileset};

#[derive(Clone)]
pub struct MapContextService {
    dirs: DataDirs,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapContextSnapshot {
    pub revision: MapRevision,
    pub saved_source_notice: String,
    pub source_file_size: u64,
    pub starcraft_path: PathBuf,
    pub digest: crate::chk::Digest,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapSourceProbe {
    pub project_id: String,
    pub source_path: PathBuf,
    #[serde(with = "crate::map_model::u128_string")]
    pub mtime_ns: u128,
    pub file_size: u64,
}

impl MapContextService {
    pub fn new(dirs: DataDirs) -> Self {
        Self { dirs }
    }

    pub fn current(&self) -> Result<MapContextSnapshot, String> {
        let (project_id, source_path) = self.current_source()?;
        let probe = Self::probe_path(project_id, source_path)?;
        self.snapshot_for_probe(probe)
    }

    pub(crate) fn snapshot_for_probe(
        &self,
        probe: MapSourceProbe,
    ) -> Result<MapContextSnapshot, String> {
        let file = std::fs::read(&probe.source_path)
            .map_err(|error| format!("source map bytes could not be read: {error}"))?;
        let chk = isom::chk_extract(&probe.source_path)
            .map_err(|error| format!("saved source map CHK could not be extracted: {error}"))?;
        let revision = revision_from_parts(
            probe.project_id,
            &probe.source_path,
            probe.mtime_ns,
            &file,
            &chk,
        )?;
        Ok(MapContextSnapshot {
            revision,
            saved_source_notice: "저장된 SCX 기준 · SCMDraft 미저장 상태는 포함되지 않음"
                .to_string(),
            source_file_size: probe.file_size,
            starcraft_path: resolve_starcraft_path(&self.dirs)?,
            digest: crate::chk::digest_chk(&chk),
        })
    }

    pub fn probe_current(&self) -> Result<MapSourceProbe, String> {
        let (project_id, source_path) = self.status_source()?;
        Self::probe_path(project_id, source_path)
    }

    fn probe_path(project_id: String, source_path: PathBuf) -> Result<MapSourceProbe, String> {
        let metadata = std::fs::metadata(&source_path)
            .map_err(|error| format!("saved source map metadata could not be read: {error}"))?;
        if !metadata.is_file() {
            return Err("saved OpenMapName is not a file".to_string());
        }
        let mtime_ns = metadata
            .modified()
            .map_err(|error| format!("saved source map mtime could not be read: {error}"))?
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        Ok(MapSourceProbe {
            project_id,
            source_path,
            mtime_ns,
            file_size: metadata.len(),
        })
    }

    #[cfg(not(test))]
    pub(crate) fn snapshot_binding_is_current(
        &self,
        snapshot: &MapContextSnapshot,
    ) -> Result<bool, String> {
        if self.current_project_id()? != snapshot.revision.project_id {
            return Ok(false);
        }
        let metadata = std::fs::metadata(&snapshot.revision.source_path)
            .map_err(|error| format!("saved source map metadata could not be read: {error}"))?;
        if !metadata.is_file() {
            return Ok(false);
        }
        let mtime_ns = metadata
            .modified()
            .map_err(|error| format!("saved source map mtime could not be read: {error}"))?
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        Ok(mtime_ns == snapshot.revision.mtime_ns && metadata.len() == snapshot.source_file_size)
    }

    fn current_source(&self) -> Result<(String, PathBuf), String> {
        let project = crate::native_runtime::NativeProjectManager::new(self.dirs.clone()).open()?;
        // Keep the historical project identity key (root/project.json) even though the
        // canonical manifest is now `.eap`; changing this hash would orphan map sessions,
        // selections, and imported stamps already persisted for the project.
        let project_id = project_id_for_path(project.root().to_path_buf());
        Ok((project_id, project.source_map_path()?))
    }

    fn status_source(&self) -> Result<(String, PathBuf), String> {
        self.current_source()
    }

    pub fn current_project_root(&self) -> Result<PathBuf, String> {
        let project = crate::native_runtime::NativeProjectManager::new(self.dirs.clone()).open()?;
        Ok(project.root().to_path_buf())
    }

    #[cfg_attr(test, allow(dead_code))]
    fn current_project_id(&self) -> Result<String, String> {
        self.current_source().map(|(project_id, _)| project_id)
    }

    pub fn revision_for_path(
        &self,
        project_id: String,
        source_path: &Path,
    ) -> Result<MapRevision, String> {
        let metadata = std::fs::metadata(source_path)
            .map_err(|error| format!("source map metadata could not be read: {error}"))?;
        if !metadata.is_file() {
            return Err("source map path is not a file".to_string());
        }
        let file = std::fs::read(source_path)
            .map_err(|error| format!("source map bytes could not be read: {error}"))?;
        let chk = isom::chk_extract(source_path)
            .map_err(|error| format!("source map CHK could not be extracted: {error}"))?;
        let modified = metadata
            .modified()
            .map_err(|error| format!("source map mtime could not be read: {error}"))?;
        let mtime_ns = modified
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        revision_from_parts(project_id, source_path, mtime_ns, &file, &chk)
    }

    pub fn starcraft_path(&self) -> Result<PathBuf, String> {
        resolve_starcraft_path(&self.dirs)
    }
}

pub(crate) fn project_id_for_path(project_root: PathBuf) -> String {
    let project_identity = project_root
        .join(crate::native_project::LEGACY_PROJECT_MANIFEST_FILE)
        .canonicalize()
        .unwrap_or_else(|_| project_root.join(crate::native_project::LEGACY_PROJECT_MANIFEST_FILE))
        .to_string_lossy()
        .to_lowercase();
    hex_sha256(project_identity.as_bytes())
}

fn revision_from_parts(
    project_id: String,
    source_path: &Path,
    mtime_ns: u128,
    file: &[u8],
    chk: &[u8],
) -> Result<MapRevision, String> {
    let sections = crate::chk::assemble_sections(&crate::chk::walk_sections(chk));
    let dim = sections
        .get("DIM ")
        .ok_or_else(|| "source map has no DIM section".to_string())?;
    let era = sections
        .get("ERA ")
        .ok_or_else(|| "source map has no ERA section".to_string())?;
    if dim.len() < 4 || era.len() < 2 {
        return Err("source map DIM/ERA section is truncated".to_string());
    }
    let width = u16::from_le_bytes([dim[0], dim[1]]);
    let height = u16::from_le_bytes([dim[2], dim[3]]);
    if width == 0 || height == 0 {
        return Err("source map dimensions are empty".to_string());
    }
    let era = u16::from_le_bytes([era[0], era[1]]);
    Ok(MapRevision {
        project_id,
        source_path: source_path.to_path_buf(),
        file_sha256: hex_sha256(file),
        chk_sha256: hex_sha256(chk),
        mtime_ns,
        tileset: Tileset::from_era(era)?,
        width,
        height,
    })
}

pub(crate) fn resolve_starcraft_path(dirs: &DataDirs) -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("STARCRAFT_PATH").map(PathBuf::from) {
        if path.is_dir() {
            return Ok(path);
        }
        return Err("STARCRAFT_PATH does not name an installed StarCraft directory".to_string());
    }
    // An explicitly configured folder wins over the default install location so
    // the wizard's "StarCraft 폴더 선택" can override a stale default directory.
    let configured = dirs
        .load_config()
        .map_err(|error| format!("app config could not be read: {error}"))?
        .starcraft_path;
    let configured = PathBuf::from(configured);
    if configured.is_dir() {
        return Ok(configured);
    }
    // Default Battle.net install location for this platform.
    let standard = PathBuf::from(if cfg!(windows) {
        r"C:\Program Files (x86)\StarCraft"
    } else {
        "/Applications/StarCraft"
    });
    if standard.is_dir() {
        return Ok(standard);
    }
    Err("StarCraft data directory could not be resolved".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dirs(root: &Path) -> DataDirs {
        DataDirs::from_bases(&root.join("roaming"), &root.join("local"))
    }

    #[test]
    fn rich_fixture_revision_is_stable_and_complete() {
        let root = std::env::temp_dir().join(format!("map-context-{}", uuid::Uuid::new_v4()));
        let service = MapContextService::new(test_dirs(&root));
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("crates")
            .join("isom")
            .join("tests")
            .join("fixtures")
            .join("map_agent_rich.scx");
        let first = service
            .revision_for_path("project".to_string(), &fixture)
            .unwrap();
        let second = service
            .revision_for_path("project".to_string(), &fixture)
            .unwrap();
        assert_eq!(first, second);
        assert!(!first.file_sha256.is_empty());
        assert!(!first.chk_sha256.is_empty());
        assert!(first.width > 0 && first.height > 0);
    }

    #[test]
    fn source_probe_serializes_mtime_losslessly() {
        let mtime_ns = 1_700_000_000_000_000_123_u128;
        let value = serde_json::to_value(MapSourceProbe {
            project_id: "project".to_string(),
            source_path: PathBuf::from(r"C:\maps\demo.scx"),
            mtime_ns,
            file_size: 1024,
        })
        .unwrap();
        assert_eq!(value["mtimeNs"], mtime_ns.to_string());
    }

    #[test]
    #[ignore = "requires configured native project and StarCraft assets"]
    fn live_saved_open_map_loads_and_renders() {
        let roaming = PathBuf::from(std::env::var_os("APPDATA").unwrap());
        let local = PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap());
        let service = MapContextService::new(DataDirs::from_bases(&roaming, &local));
        let context = service.current().unwrap();
        eprintln!(
            "live map: {} {}x{} {}",
            context.revision.source_path.display(),
            context.revision.width,
            context.revision.height,
            context.revision.tileset.era()
        );
        assert!(context.revision.source_path.is_file());
        assert!(!context.revision.file_sha256.is_empty());
        assert!(context.revision.width > 0 && context.revision.height > 0);
        let request = serde_json::json!({
            "schema": "eud-map-render/1",
            "mode": "region",
            "x": 0,
            "y": 0,
            "width": context.revision.width.min(16),
            "height": context.revision.height.min(16),
            "scale": 4,
            "layers": ["terrain", "doodads", "sprites", "units", "buildings"]
        });
        let image = isom::render_region(
            &context.revision.source_path,
            &context.starcraft_path,
            request.to_string().as_bytes(),
        )
        .unwrap();
        assert!(image.width > 0 && image.height > 0);
        assert!(image.rgba.chunks_exact(4).any(|pixel| pixel[3] == 255));
    }
}
