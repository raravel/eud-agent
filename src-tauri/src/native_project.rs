//! Native EUD project authority.
//!
//! The canonical project is a versioned manifest, a real `src/` EPS tree, and sparse JSON
//! DAT overrides. No EUD Editor process, assembly, heartbeat, or file-IPC state participates in
//! reads or writes. All paths are project-relative, slash-separated, and confined beneath the
//! selected project root.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::memory::write_atomic_bytes;

pub const PROJECT_SCHEMA_VERSION: u32 = 1;
pub const DAT_SCHEMA_VERSION: u32 = 1;
pub const PROJECT_MANIFEST_FILE: &str = "project.json";
pub const MAX_DAT_PATCH_CHANGES: usize = 300;
pub const MAX_TEXT_BYTES: usize = 4 * 1024 * 1024;

const STANDARD_DAT_FILE: &str = "dat/standard.json";
const XDAT_FILE: &str = "dat/xdat.json";
const TBL_FILE: &str = "dat/tbl.json";
const REQUIREMENTS_FILE: &str = "dat/requirements.json";
const BUTTONS_FILE: &str = "dat/buttons.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectManifest {
    pub schema_version: u32,
    pub name: String,
    pub source_map: String,
    pub output_map: String,
    pub main_file: String,
    #[serde(default)]
    pub settings: ProjectSettings,
    #[serde(default)]
    pub plugins: Vec<EdsPlugin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editor_compatibility: Option<EditorCompatibility>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectSettings {
    #[serde(default)]
    pub shuffle_payload: bool,
    #[serde(default)]
    pub debug: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decode_unit_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_field_count: Option<u32>,
    #[serde(default = "default_sector_size")]
    pub sector_size: u32,
    #[serde(default)]
    pub use_custom_tbl: bool,
}

fn default_sector_size() -> u32 {
    15
}

impl Default for ProjectSettings {
    fn default() -> Self {
        Self {
            shuffle_payload: false,
            debug: false,
            decode_unit_name: None,
            object_field_count: None,
            sector_size: default_sector_size(),
            use_custom_tbl: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EdsPlugin {
    pub section: String,
    #[serde(default)]
    pub entries: Vec<EdsPluginEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EdsPluginEntry {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EditorCompatibility {
    pub source_e3s: String,
    pub source_sha256: String,
    #[serde(default)]
    pub opaque_records: Vec<OpaqueEditorRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpaqueEditorRecord {
    pub record_id: u32,
    pub record_type: String,
    pub encoded: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NumericOverride {
    pub before: i64,
    pub after: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TextOverride {
    pub before: String,
    pub after: String,
}

pub type NumericFields = BTreeMap<String, NumericOverride>;
pub type NumericObjects = BTreeMap<u32, NumericFields>;
pub type NumericTables = BTreeMap<String, NumericObjects>;
pub type TextObjects = BTreeMap<u32, TextOverride>;
pub type TextTables = BTreeMap<String, TextObjects>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NumericDatDocument {
    pub schema_version: u32,
    #[serde(default)]
    pub tables: NumericTables,
}

impl Default for NumericDatDocument {
    fn default() -> Self {
        Self {
            schema_version: DAT_SCHEMA_VERSION,
            tables: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TextDatDocument {
    pub schema_version: u32,
    #[serde(default)]
    pub values: TextObjects,
}

impl Default for TextDatDocument {
    fn default() -> Self {
        Self {
            schema_version: DAT_SCHEMA_VERSION,
            values: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequirementDocument {
    pub schema_version: u32,
    #[serde(default)]
    pub tables: TextTables,
}

impl Default for RequirementDocument {
    fn default() -> Self {
        Self {
            schema_version: DAT_SCHEMA_VERSION,
            tables: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NativeDatState {
    pub standard: NumericDatDocument,
    pub xdat: NumericDatDocument,
    pub tbl: TextDatDocument,
    pub requirements: RequirementDocument,
    pub buttons: TextDatDocument,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum NativeDatChange {
    Dat {
        dat: String,
        object_id: u32,
        field: String,
        before: i64,
        after: i64,
    },
    Xdat {
        dat: String,
        object_id: u32,
        field: String,
        before: i64,
        after: i64,
    },
    Tbl {
        index: u32,
        before: String,
        after: String,
    },
    Requirement {
        dat: String,
        object_id: u32,
        before: String,
        after: String,
    },
    Button {
        set_id: u32,
        before: String,
        after: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeDatPatch {
    pub changes: Vec<NativeDatChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppliedDatPatch {
    pub change_count: usize,
    pub revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum DatTarget {
    Dat {
        dat: String,
        object_id: u32,
        field: String,
    },
    Xdat {
        dat: String,
        object_id: u32,
        field: String,
    },
    Tbl(u32),
    Requirement {
        dat: String,
        object_id: u32,
    },
    Button(u32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DatScalar {
    Number(i64),
    Text(String),
}

impl NativeDatChange {
    pub fn target(&self) -> DatTarget {
        match self {
            Self::Dat {
                dat,
                object_id,
                field,
                ..
            } => DatTarget::Dat {
                dat: dat.clone(),
                object_id: *object_id,
                field: field.clone(),
            },
            Self::Xdat {
                dat,
                object_id,
                field,
                ..
            } => DatTarget::Xdat {
                dat: dat.clone(),
                object_id: *object_id,
                field: field.clone(),
            },
            Self::Tbl { index, .. } => DatTarget::Tbl(*index),
            Self::Requirement { dat, object_id, .. } => DatTarget::Requirement {
                dat: dat.clone(),
                object_id: *object_id,
            },
            Self::Button { set_id, .. } => DatTarget::Button(*set_id),
        }
    }

    pub fn before(&self) -> DatScalar {
        match self {
            Self::Dat { before, .. } | Self::Xdat { before, .. } => DatScalar::Number(*before),
            Self::Tbl { before, .. }
            | Self::Requirement { before, .. }
            | Self::Button { before, .. } => DatScalar::Text(before.clone()),
        }
    }

    pub fn after(&self) -> DatScalar {
        match self {
            Self::Dat { after, .. } | Self::Xdat { after, .. } => DatScalar::Number(*after),
            Self::Tbl { after, .. }
            | Self::Requirement { after, .. }
            | Self::Button { after, .. } => DatScalar::Text(after.clone()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeProjectStatus {
    pub name: String,
    pub root: String,
    pub source_map: String,
    pub output_map: String,
    pub main_file: String,
    pub revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeSourceFile {
    pub path: String,
    pub content: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeSourceSnapshot {
    pub project: String,
    pub identity: String,
    pub main_file: String,
    pub files: Vec<NativeSourceFile>,
    pub revision: String,
}

#[derive(Debug, Clone)]
pub struct NativeProject {
    root: PathBuf,
    manifest: ProjectManifest,
    dat: NativeDatState,
}

impl NativeProject {
    pub fn create(root: &Path, manifest: ProjectManifest) -> Result<Self, String> {
        validate_manifest(&manifest)?;
        fs::create_dir_all(root).map_err(stringify_io)?;
        for directory in ["src", "dat", "plugins", "maps", "build"] {
            fs::create_dir_all(root.join(directory)).map_err(stringify_io)?;
        }
        let project = Self {
            root: canonical_or_absolute(root)?,
            manifest,
            dat: NativeDatState::default(),
        };
        project.save_manifest()?;
        project.save_dat_state()?;
        project.create_source(&project.manifest.main_file, "")?;
        Ok(project)
    }

    pub fn open(root: &Path) -> Result<Self, String> {
        let root = fs::canonicalize(root).map_err(|error| {
            format!(
                "native project root '{}' is unavailable: {error}",
                root.display()
            )
        })?;
        let manifest: ProjectManifest = read_json(&root.join(PROJECT_MANIFEST_FILE))?;
        validate_manifest(&manifest)?;
        let project = Self {
            dat: NativeDatState {
                standard: read_json_or_default(&root.join(STANDARD_DAT_FILE))?,
                xdat: read_json_or_default(&root.join(XDAT_FILE))?,
                tbl: read_json_or_default(&root.join(TBL_FILE))?,
                requirements: read_json_or_default(&root.join(REQUIREMENTS_FILE))?,
                buttons: read_json_or_default(&root.join(BUTTONS_FILE))?,
            },
            root,
            manifest,
        };
        project.validate_dat_state()?;
        project.require_path(&project.manifest.source_map, false)?;
        project.require_path(&project.manifest.main_file, true)?;
        Ok(project)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn manifest(&self) -> &ProjectManifest {
        &self.manifest
    }

    pub fn dat(&self) -> &NativeDatState {
        &self.dat
    }

    pub fn status(&self) -> Result<NativeProjectStatus, String> {
        Ok(NativeProjectStatus {
            name: self.manifest.name.clone(),
            root: self.root.to_string_lossy().into_owned(),
            source_map: self.manifest.source_map.clone(),
            output_map: self.manifest.output_map.clone(),
            main_file: self.manifest.main_file.clone(),
            revision: self.revision()?,
        })
    }

    pub fn source_map_path(&self) -> Result<PathBuf, String> {
        self.require_path(&self.manifest.source_map, true)
    }

    pub fn output_map_path(&self) -> Result<PathBuf, String> {
        self.require_path(&self.manifest.output_map, false)
    }

    pub fn list_source_files(&self) -> Result<Vec<String>, String> {
        let mut files = Vec::new();
        collect_text_files(&self.root.join("src"), &self.root, &mut files)?;
        files.sort_by_key(|value| value.to_lowercase());
        Ok(files)
    }

    pub fn replace_dat_state(&mut self, state: NativeDatState) -> Result<(), String> {
        self.dat = state;
        self.validate_dat_state()?;
        self.save_dat_state()
    }

    pub fn source_snapshot(&self) -> Result<NativeSourceSnapshot, String> {
        let mut files = Vec::new();
        for path in self.list_source_files()? {
            if !path.to_lowercase().ends_with(".eps") {
                continue;
            }
            let absolute = self.require_path(&path, true)?;
            let content = read_bounded_text(&absolute)?;
            files.push(NativeSourceFile {
                path,
                sha256: sha256_hex(content.as_bytes()),
                content,
            });
        }
        Ok(NativeSourceSnapshot {
            project: self.manifest.name.clone(),
            identity: self.root.to_string_lossy().into_owned(),
            main_file: self.manifest.main_file.clone(),
            revision: self.revision()?,
            files,
        })
    }

    pub fn read_source(&self, path: &str) -> Result<String, String> {
        require_source_path(path)?;
        read_bounded_text(&self.require_path(path, true)?)
    }

    pub fn write_source(&self, path: &str, content: &str) -> Result<(), String> {
        require_source_path(path)?;
        if content.len() > MAX_TEXT_BYTES {
            return Err(format!("source content exceeds {MAX_TEXT_BYTES} bytes"));
        }
        let target = self.require_path(path, false)?;
        write_atomic_bytes(&target, content.as_bytes()).map_err(|error| error.to_string())
    }

    pub fn create_source(&self, path: &str, content: &str) -> Result<(), String> {
        let target = self.require_path(path, false)?;
        if target.exists() {
            return Err(format!("source file already exists: {path}"));
        }
        self.write_source(path, content)
    }

    pub fn create_source_dir(&self, path: &str) -> Result<(), String> {
        let normalized = normalize_relative(path)?;
        if normalized != "src" && !normalized.starts_with("src/") {
            return Err("source directory must stay under src/".to_string());
        }
        let target = self.require_path(&normalized, false)?;
        fs::create_dir_all(target).map_err(stringify_io)
    }

    pub fn move_source(&mut self, from: &str, to: &str) -> Result<(), String> {
        require_source_path(from)?;
        require_source_path(to)?;
        let source = self.require_path(from, true)?;
        let target = self.require_path(to, false)?;
        if target.exists() {
            return Err(format!("source target already exists: {to}"));
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(stringify_io)?;
        }
        fs::rename(&source, &target).map_err(stringify_io)?;
        if self.manifest.main_file.eq_ignore_ascii_case(from) {
            self.manifest.main_file = normalize_relative(to)?;
            self.save_manifest()?;
        }
        Ok(())
    }

    pub fn set_project_setting(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            "OpenMapName" => self.manifest.source_map = normalize_relative(value)?,
            "SaveMapName" => self.manifest.output_map = normalize_relative(value)?,
            "UseCustomtbl" => {
                self.manifest.settings.use_custom_tbl = value
                    .parse::<bool>()
                    .map_err(|_| "UseCustomtbl must be true or false".to_string())?
            }
            "sectorSize" => {
                self.manifest.settings.sector_size = value
                    .parse::<u32>()
                    .map_err(|_| "sectorSize must be an integer".to_string())?
            }
            other => return Err(format!("unsupported native project setting: {other}")),
        }
        validate_manifest(&self.manifest)?;
        self.save_manifest()
    }

    pub fn plugin_add(&mut self, index: i64, raw_text: &str) -> Result<usize, String> {
        let plugin = parse_raw_plugin(raw_text)?;
        let position = if index < 0 {
            self.manifest.plugins.len()
        } else {
            usize::try_from(index)
                .map_err(|_| "plugin index must be -1 or non-negative".to_string())?
        };
        if position > self.manifest.plugins.len() {
            return Err(format!("plugin index {position} is out of range"));
        }
        self.manifest.plugins.insert(position, plugin);
        validate_manifest(&self.manifest)?;
        self.save_manifest()?;
        Ok(position)
    }

    pub fn plugin_edit(&mut self, index: usize, raw_text: &str) -> Result<(), String> {
        let plugin = parse_raw_plugin(raw_text)?;
        let target = self
            .manifest
            .plugins
            .get_mut(index)
            .ok_or_else(|| format!("plugin index {index} is out of range"))?;
        *target = plugin;
        validate_manifest(&self.manifest)?;
        self.save_manifest()
    }

    pub fn plugin_remove(&mut self, index: usize) -> Result<EdsPlugin, String> {
        if index >= self.manifest.plugins.len() {
            return Err(format!("plugin index {index} is out of range"));
        }
        let removed = self.manifest.plugins.remove(index);
        self.save_manifest()?;
        Ok(removed)
    }

    pub fn plugin_move(&mut self, from: usize, to: usize) -> Result<(), String> {
        if from >= self.manifest.plugins.len() || to >= self.manifest.plugins.len() {
            return Err("plugin move index is out of range".to_string());
        }
        let plugin = self.manifest.plugins.remove(from);
        self.manifest.plugins.insert(to, plugin);
        self.save_manifest()
    }

    pub fn set_editor_compatibility(
        &mut self,
        compatibility: Option<EditorCompatibility>,
    ) -> Result<(), String> {
        self.manifest.editor_compatibility = compatibility;
        self.save_manifest()
    }
    pub fn delete_source(&mut self, path: &str) -> Result<(), String> {
        require_source_path(path)?;
        if self.manifest.main_file.eq_ignore_ascii_case(path) {
            return Err("cannot delete the configured mainFile".to_string());
        }
        fs::remove_file(self.require_path(path, true)?).map_err(stringify_io)
    }

    pub fn set_main_file(&mut self, path: &str) -> Result<(), String> {
        require_source_path(path)?;
        self.require_path(path, true)?;
        self.manifest.main_file = normalize_relative(path)?;
        self.save_manifest()
    }

    pub fn original_dat_value(&self, target: &DatTarget) -> Option<DatScalar> {
        match target {
            DatTarget::Dat {
                dat,
                object_id,
                field,
            } => self
                .dat
                .standard
                .tables
                .get(dat)?
                .get(object_id)?
                .get(field)
                .map(|value| DatScalar::Number(value.before)),
            DatTarget::Xdat {
                dat,
                object_id,
                field,
            } => self
                .dat
                .xdat
                .tables
                .get(dat)?
                .get(object_id)?
                .get(field)
                .map(|value| DatScalar::Number(value.before)),
            DatTarget::Tbl(index) => self
                .dat
                .tbl
                .values
                .get(index)
                .map(|value| DatScalar::Text(value.before.clone())),
            DatTarget::Requirement { dat, object_id } => self
                .dat
                .requirements
                .tables
                .get(dat)?
                .get(object_id)
                .map(|value| DatScalar::Text(value.before.clone())),
            DatTarget::Button(set_id) => self
                .dat
                .buttons
                .values
                .get(set_id)
                .map(|value| DatScalar::Text(value.before.clone())),
        }
    }

    pub fn current_dat_value(&self, target: &DatTarget) -> Option<DatScalar> {
        match target {
            DatTarget::Dat {
                dat,
                object_id,
                field,
            } => self
                .dat
                .standard
                .tables
                .get(dat)?
                .get(object_id)?
                .get(field)
                .map(|value| DatScalar::Number(value.after)),
            DatTarget::Xdat {
                dat,
                object_id,
                field,
            } => self
                .dat
                .xdat
                .tables
                .get(dat)?
                .get(object_id)?
                .get(field)
                .map(|value| DatScalar::Number(value.after)),
            DatTarget::Tbl(index) => self
                .dat
                .tbl
                .values
                .get(index)
                .map(|value| DatScalar::Text(value.after.clone())),
            DatTarget::Requirement { dat, object_id } => self
                .dat
                .requirements
                .tables
                .get(dat)?
                .get(object_id)
                .map(|value| DatScalar::Text(value.after.clone())),
            DatTarget::Button(set_id) => self
                .dat
                .buttons
                .values
                .get(set_id)
                .map(|value| DatScalar::Text(value.after.clone())),
        }
    }

    /// Apply one complete, pre-read patch. `baseline` contains the authoritative effective value
    /// for targets that do not yet have a sparse override. No file is written until every target,
    /// duplicate, no-op, and stale-before check succeeds.
    pub fn apply_dat_patch(
        &mut self,
        patch: &NativeDatPatch,
        baseline: &BTreeMap<DatTarget, DatScalar>,
    ) -> Result<AppliedDatPatch, String> {
        if patch.changes.is_empty() {
            return Err("dat_patch changes must not be empty".to_string());
        }
        if patch.changes.len() > MAX_DAT_PATCH_CHANGES {
            return Err(format!(
                "dat_patch exceeds the {MAX_DAT_PATCH_CHANGES}-change limit"
            ));
        }

        let mut next = self.dat.clone();
        let mut seen = BTreeSet::new();
        for (index, change) in patch.changes.iter().enumerate() {
            validate_change(change).map_err(|error| format!("changes[{index}]: {error}"))?;
            let target = change.target();
            if !seen.insert(target.clone()) {
                return Err(format!("changes[{index}] duplicates target {target:?}"));
            }
            let current = self
                .current_dat_value(&target)
                .or_else(|| baseline.get(&target).cloned())
                .ok_or_else(|| format!("changes[{index}] has no authoritative baseline"))?;
            if current != change.before() {
                return Err(format!(
                    "changes[{index}] stale before value for {target:?}"
                ));
            }
            apply_change(&mut next, change);
        }

        compact_defaults(&mut next);
        let previous = std::mem::replace(&mut self.dat, next);
        if let Err(error) = self.save_dat_state() {
            self.dat = previous;
            return Err(error);
        }
        Ok(AppliedDatPatch {
            change_count: patch.changes.len(),
            revision: self.revision()?,
        })
    }

    pub fn revision(&self) -> Result<String, String> {
        let mut hasher = Sha256::new();
        hasher.update(serde_json::to_vec(&self.manifest).map_err(|error| error.to_string())?);
        hasher.update(serde_json::to_vec(&self.dat.standard).map_err(|error| error.to_string())?);
        hasher.update(serde_json::to_vec(&self.dat.xdat).map_err(|error| error.to_string())?);
        hasher.update(serde_json::to_vec(&self.dat.tbl).map_err(|error| error.to_string())?);
        hasher
            .update(serde_json::to_vec(&self.dat.requirements).map_err(|error| error.to_string())?);
        hasher.update(serde_json::to_vec(&self.dat.buttons).map_err(|error| error.to_string())?);
        for file in self.source_snapshot_without_revision()? {
            hasher.update(file.path.as_bytes());
            hasher.update(file.sha256.as_bytes());
        }
        Ok(format!("{:x}", hasher.finalize()))
    }

    fn source_snapshot_without_revision(&self) -> Result<Vec<NativeSourceFile>, String> {
        let mut files = Vec::new();
        for path in self.list_source_files()? {
            if path.to_lowercase().ends_with(".eps") {
                let content = read_bounded_text(&self.require_path(&path, true)?)?;
                files.push(NativeSourceFile {
                    path,
                    sha256: sha256_hex(content.as_bytes()),
                    content,
                });
            }
        }
        Ok(files)
    }

    fn save_manifest(&self) -> Result<(), String> {
        write_json(&self.root.join(PROJECT_MANIFEST_FILE), &self.manifest)
    }

    fn save_dat_state(&self) -> Result<(), String> {
        let documents = [
            (
                self.root.join(STANDARD_DAT_FILE),
                serde_json::to_vec_pretty(&self.dat.standard).map_err(|error| error.to_string())?,
            ),
            (
                self.root.join(XDAT_FILE),
                serde_json::to_vec_pretty(&self.dat.xdat).map_err(|error| error.to_string())?,
            ),
            (
                self.root.join(TBL_FILE),
                serde_json::to_vec_pretty(&self.dat.tbl).map_err(|error| error.to_string())?,
            ),
            (
                self.root.join(REQUIREMENTS_FILE),
                serde_json::to_vec_pretty(&self.dat.requirements)
                    .map_err(|error| error.to_string())?,
            ),
            (
                self.root.join(BUTTONS_FILE),
                serde_json::to_vec_pretty(&self.dat.buttons).map_err(|error| error.to_string())?,
            ),
        ];
        let originals: Vec<_> = documents
            .iter()
            .map(|(path, _)| fs::read(path).ok())
            .collect();
        for (index, (path, bytes)) in documents.iter().enumerate() {
            if let Err(error) = write_atomic_bytes(path, bytes) {
                for restore_index in 0..index {
                    let restore_path = &documents[restore_index].0;
                    match &originals[restore_index] {
                        Some(original) => {
                            let _ = write_atomic_bytes(restore_path, original);
                        }
                        None => {
                            let _ = fs::remove_file(restore_path);
                        }
                    }
                }
                return Err(format!("failed to commit native DAT state: {error}"));
            }
        }
        Ok(())
    }

    fn validate_dat_state(&self) -> Result<(), String> {
        for (name, version) in [
            (STANDARD_DAT_FILE, self.dat.standard.schema_version),
            (XDAT_FILE, self.dat.xdat.schema_version),
            (TBL_FILE, self.dat.tbl.schema_version),
            (REQUIREMENTS_FILE, self.dat.requirements.schema_version),
            (BUTTONS_FILE, self.dat.buttons.schema_version),
        ] {
            if version != DAT_SCHEMA_VERSION {
                return Err(format!(
                    "{name} schemaVersion {version} is unsupported (expected {DAT_SCHEMA_VERSION})"
                ));
            }
        }
        Ok(())
    }

    fn require_path(&self, relative: &str, must_exist: bool) -> Result<PathBuf, String> {
        let relative = normalize_relative(relative)?;
        let path = self
            .root
            .join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
        if must_exist && !path.exists() {
            return Err(format!("native project path does not exist: {relative}"));
        }
        if path.exists() {
            let canonical = fs::canonicalize(&path).map_err(stringify_io)?;
            if !canonical.starts_with(&self.root) {
                return Err(format!("native project path escapes root: {relative}"));
            }
            return Ok(canonical);
        }
        let parent = path
            .parent()
            .ok_or_else(|| format!("native project path has no parent: {relative}"))?;
        let existing_parent = nearest_existing_parent(parent)?;
        let canonical_parent = fs::canonicalize(existing_parent).map_err(stringify_io)?;
        if !canonical_parent.starts_with(&self.root) {
            return Err(format!(
                "native project path parent escapes root: {relative}"
            ));
        }
        Ok(path)
    }
}

fn parse_raw_plugin(raw_text: &str) -> Result<EdsPlugin, String> {
    if raw_text.len() > MAX_TEXT_BYTES {
        return Err(format!("plugin text exceeds {MAX_TEXT_BYTES} bytes"));
    }
    if raw_text.contains('\0') {
        return Err("plugin text contains NUL".to_string());
    }
    let normalized = raw_text.replace("\r\n", "\n");
    let header = normalized
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .ok_or_else(|| "plugin text is empty".to_string())?;
    let section = header
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .ok_or_else(|| "plugin text must begin with [section]".to_string())?
        .trim()
        .to_string();
    validate_component(&section, "plugin section")?;
    if section.eq_ignore_ascii_case("main") {
        return Err("plugins must not define the reserved [main] section".to_string());
    }
    Ok(EdsPlugin {
        section,
        entries: Vec::new(),
        raw_text: Some(normalized),
    })
}

fn validate_manifest(manifest: &ProjectManifest) -> Result<(), String> {
    if manifest.schema_version != PROJECT_SCHEMA_VERSION {
        return Err(format!(
            "project schemaVersion {} is unsupported (expected {PROJECT_SCHEMA_VERSION})",
            manifest.schema_version
        ));
    }
    if manifest.name.trim().is_empty() || manifest.name.len() > 128 {
        return Err("project name must contain 1..128 characters".to_string());
    }
    let source = normalize_relative(&manifest.source_map)?;
    let output = normalize_relative(&manifest.output_map)?;
    let main = normalize_relative(&manifest.main_file)?;
    if !matches_extension(&source, &["scx", "scm"]) {
        return Err("sourceMap must end in .scx or .scm".to_string());
    }
    if !matches_extension(&output, &["scx", "scm"]) {
        return Err("outputMap must end in .scx or .scm".to_string());
    }
    if source.eq_ignore_ascii_case(&output) {
        return Err("sourceMap and outputMap must differ".to_string());
    }
    require_source_path(&main)?;
    if manifest.settings.sector_size == 0 || manifest.settings.sector_size > 255 {
        return Err("settings.sectorSize must be in 1..255".to_string());
    }
    let mut sections = BTreeSet::new();
    for plugin in &manifest.plugins {
        validate_component(&plugin.section, "plugin section")?;
        if plugin.section.eq_ignore_ascii_case("main") {
            return Err("plugins must not define the reserved [main] section".to_string());
        }
        if !sections.insert(plugin.section.to_lowercase()) {
            return Err(format!("duplicate plugin section: {}", plugin.section));
        }
        if let Some(raw_text) = &plugin.raw_text {
            if !plugin.entries.is_empty() {
                return Err(format!(
                    "plugin [{}] cannot contain both rawText and entries",
                    plugin.section
                ));
            }
            let parsed = parse_raw_plugin(raw_text)?;
            if !parsed.section.eq_ignore_ascii_case(&plugin.section) {
                return Err(format!(
                    "plugin [{}] rawText begins with [{}]",
                    plugin.section, parsed.section
                ));
            }
        }
        let mut keys = BTreeSet::new();
        for entry in &plugin.entries {
            validate_component(&entry.key, "plugin key")?;
            if !keys.insert(entry.key.to_lowercase()) {
                return Err(format!(
                    "duplicate key '{}' in plugin [{}]",
                    entry.key, plugin.section
                ));
            }
            if entry
                .value
                .as_ref()
                .is_some_and(|value| value.contains(['\r', '\n']))
            {
                return Err(format!(
                    "plugin value '{}' in [{}] contains a newline",
                    entry.key, plugin.section
                ));
            }
        }
    }
    Ok(())
}

fn validate_change(change: &NativeDatChange) -> Result<(), String> {
    if change.before() == change.after() {
        return Err("change is a no-op".to_string());
    }
    match change {
        NativeDatChange::Dat { dat, field, .. } | NativeDatChange::Xdat { dat, field, .. } => {
            validate_component(dat, "dat")?;
            validate_component(field, "field")?;
        }
        NativeDatChange::Requirement { dat, after, .. } => {
            validate_component(dat, "requirements dat")?;
            validate_requirement_payload(after)?;
        }
        NativeDatChange::Button { after, .. } => validate_button_csv(after)?,
        NativeDatChange::Tbl { .. } => {}
    }
    Ok(())
}

fn apply_change(state: &mut NativeDatState, change: &NativeDatChange) {
    match change {
        NativeDatChange::Dat {
            dat,
            object_id,
            field,
            before,
            after,
        } => {
            state
                .standard
                .tables
                .entry(dat.clone())
                .or_default()
                .entry(*object_id)
                .or_default()
                .insert(
                    field.clone(),
                    NumericOverride {
                        before: *before,
                        after: *after,
                    },
                );
        }
        NativeDatChange::Xdat {
            dat,
            object_id,
            field,
            before,
            after,
        } => {
            state
                .xdat
                .tables
                .entry(dat.clone())
                .or_default()
                .entry(*object_id)
                .or_default()
                .insert(
                    field.clone(),
                    NumericOverride {
                        before: *before,
                        after: *after,
                    },
                );
        }
        NativeDatChange::Tbl {
            index,
            before,
            after,
        } => {
            state.tbl.values.insert(
                *index,
                TextOverride {
                    before: before.clone(),
                    after: after.clone(),
                },
            );
        }
        NativeDatChange::Requirement {
            dat,
            object_id,
            before,
            after,
        } => {
            state
                .requirements
                .tables
                .entry(dat.clone())
                .or_default()
                .insert(
                    *object_id,
                    TextOverride {
                        before: before.clone(),
                        after: after.clone(),
                    },
                );
        }
        NativeDatChange::Button {
            set_id,
            before,
            after,
        } => {
            state.buttons.values.insert(
                *set_id,
                TextOverride {
                    before: before.clone(),
                    after: after.clone(),
                },
            );
        }
    }
}

fn compact_defaults(state: &mut NativeDatState) {
    for tables in [&mut state.standard.tables, &mut state.xdat.tables] {
        for objects in tables.values_mut() {
            for fields in objects.values_mut() {
                fields.retain(|_, value| value.before != value.after);
            }
            objects.retain(|_, fields| !fields.is_empty());
        }
        tables.retain(|_, objects| !objects.is_empty());
    }
    state
        .tbl
        .values
        .retain(|_, value| value.before != value.after);
    for objects in state.requirements.tables.values_mut() {
        objects.retain(|_, value| value.before != value.after);
    }
    state
        .requirements
        .tables
        .retain(|_, objects| !objects.is_empty());
    state
        .buttons
        .values
        .retain(|_, value| value.before != value.after);
}

fn validate_requirement_payload(payload: &str) -> Result<(), String> {
    let mode = payload
        .split('.')
        .next()
        .unwrap_or_default()
        .parse::<u8>()
        .map_err(|_| "requirements payload first segment must be numeric (0-4)".to_string())?;
    if mode > 4 {
        return Err("requirements payload first segment must be numeric (0-4)".to_string());
    }
    Ok(())
}

fn validate_button_csv(csv: &str) -> Result<(), String> {
    if csv.is_empty() {
        return Err("button CSV must not be empty".to_string());
    }
    for (group_index, group) in csv.split('.').enumerate() {
        let fields: Vec<_> = group.split(',').collect();
        if fields.len() < 8 {
            return Err(format!(
                "button {} needs at least 8 numeric fields",
                group_index + 1
            ));
        }
        for (field_index, field) in fields.iter().take(8).enumerate() {
            field.parse::<i64>().map_err(|_| {
                format!(
                    "button {} field {} is not an integer",
                    group_index + 1,
                    field_index + 1
                )
            })?;
        }
    }
    Ok(())
}

fn validate_component(value: &str, label: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{label} must not be empty"));
    }
    if value.contains(['\0', '\r', '\n', '[', ']']) {
        return Err(format!("{label} contains a reserved character"));
    }
    Ok(())
}

fn require_source_path(path: &str) -> Result<(), String> {
    let normalized = normalize_relative(path)?;
    if !normalized.starts_with("src/") || !normalized.to_lowercase().ends_with(".eps") {
        return Err("EPS source path must be under src/ and end in .eps".to_string());
    }
    Ok(())
}

pub fn normalize_relative(value: &str) -> Result<String, String> {
    if value.is_empty() || value.contains(['\0', '\\']) {
        return Err("path must be non-empty and use '/' separators".to_string());
    }
    let path = Path::new(value);
    if path.is_absolute() {
        return Err("path must be project-relative".to_string());
    }
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let part = part
                    .to_str()
                    .ok_or_else(|| "path must be UTF-8".to_string())?;
                if part.is_empty() || part == "." || part == ".." {
                    return Err("path contains an invalid component".to_string());
                }
                parts.push(part);
            }
            _ => return Err("path contains traversal or a prefix".to_string()),
        }
    }
    if parts.is_empty() {
        return Err("path must contain at least one component".to_string());
    }
    Ok(parts.join("/"))
}

fn matches_extension(path: &str, extensions: &[&str]) -> bool {
    Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| extensions.iter().any(|ext| value.eq_ignore_ascii_case(ext)))
}

fn collect_text_files(
    root: &Path,
    project_root: &Path,
    files: &mut Vec<String>,
) -> Result<(), String> {
    if !root.exists() {
        return Ok(());
    }
    let mut entries: Vec<_> = fs::read_dir(root)
        .map_err(stringify_io)?
        .collect::<Result<_, _>>()
        .map_err(stringify_io)?;
    entries.sort_by_key(|entry| entry.file_name().to_string_lossy().to_lowercase());
    for entry in entries {
        let file_type = entry.file_type().map_err(stringify_io)?;
        if file_type.is_symlink() {
            return Err(format!(
                "symlinks are forbidden in native source trees: {}",
                entry.path().display()
            ));
        }
        if file_type.is_dir() {
            collect_text_files(&entry.path(), project_root, files)?;
        } else if file_type.is_file() {
            let relative = entry
                .path()
                .strip_prefix(project_root)
                .map_err(|error| error.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            files.push(relative);
        }
    }
    Ok(())
}

fn nearest_existing_parent(mut path: &Path) -> Result<&Path, String> {
    while !path.exists() {
        path = path
            .parent()
            .ok_or_else(|| "path has no existing parent".to_string())?;
    }
    Ok(path)
}

fn canonical_or_absolute(path: &Path) -> Result<PathBuf, String> {
    if path.exists() {
        return fs::canonicalize(path).map_err(stringify_io);
    }
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    std::env::current_dir()
        .map_err(stringify_io)
        .map(|cwd| cwd.join(path))
}

fn read_bounded_text(path: &Path) -> Result<String, String> {
    let metadata = fs::metadata(path).map_err(stringify_io)?;
    if metadata.len() > MAX_TEXT_BYTES as u64 {
        return Err(format!(
            "text file exceeds {MAX_TEXT_BYTES} bytes: {}",
            path.display()
        ));
    }
    let bytes = fs::read(path).map_err(stringify_io)?;
    if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        return String::from_utf8(bytes[3..].to_vec()).map_err(|error| error.to_string());
    }
    String::from_utf8(bytes).map_err(|error| error.to_string())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    let text = read_bounded_text(path)?;
    serde_json::from_str(&text)
        .map_err(|error| format!("invalid JSON '{}': {error}", path.display()))
}

fn read_json_or_default<T>(path: &Path) -> Result<T, String>
where
    T: for<'de> Deserialize<'de> + Default,
{
    match read_json(path) {
        Ok(value) => Ok(value),
        Err(error) if !path.exists() => Ok(T::default()),
        Err(error) => Err(error),
    }
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    write_atomic_bytes(path, &bytes).map_err(|error| error.to_string())
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn stringify_io(error: std::io::Error) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "eud-agent-native-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("maps")).unwrap();
        fs::write(root.join("maps/source.scx"), b"map").unwrap();
        root
    }

    fn manifest() -> ProjectManifest {
        ProjectManifest {
            schema_version: 1,
            name: "Native Demo".to_string(),
            source_map: "maps/source.scx".to_string(),
            output_map: "build/output.scx".to_string(),
            main_file: "src/main.eps".to_string(),
            settings: ProjectSettings::default(),
            plugins: vec![EdsPlugin {
                section: "eudTurbo".to_string(),
                entries: Vec::new(),
                raw_text: None,
            }],
            editor_compatibility: None,
        }
    }

    fn project(tag: &str) -> (PathBuf, NativeProject) {
        let root = unique_root(tag);
        let project = NativeProject::create(&root, manifest()).unwrap();
        project
            .write_source("src/main.eps", "function onPluginStart() {}\n")
            .unwrap();
        let reopened = NativeProject::open(&root).unwrap();
        (root, reopened)
    }

    #[test]
    fn creates_opens_and_snapshots_native_project() {
        let (root, project) = project("open");
        let status = project.status().unwrap();
        assert_eq!(status.name, "Native Demo");
        assert_eq!(project.list_source_files().unwrap(), vec!["src/main.eps"]);
        let snapshot = project.source_snapshot().unwrap();
        assert_eq!(snapshot.files.len(), 1);
        assert_eq!(snapshot.main_file, "src/main.eps");
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn source_mutations_stay_confined_and_track_main() {
        let (root, mut project) = project("source");
        assert!(project.write_source("../escape.eps", "x").is_err());
        project
            .move_source("src/main.eps", "src/core/main.eps")
            .unwrap();
        assert_eq!(project.manifest().main_file, "src/core/main.eps");
        assert!(project.delete_source("src/core/main.eps").is_err());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn dat_patch_is_atomic_deduplicated_and_persistent() {
        let (root, mut project) = project("patch");
        let target = DatTarget::Dat {
            dat: "units".to_string(),
            object_id: 0,
            field: "Hit Points".to_string(),
        };
        let baseline = BTreeMap::from([(target.clone(), DatScalar::Number(10240))]);
        let patch = NativeDatPatch {
            changes: vec![NativeDatChange::Dat {
                dat: "units".to_string(),
                object_id: 0,
                field: "Hit Points".to_string(),
                before: 10240,
                after: 20480,
            }],
        };
        assert_eq!(
            project
                .apply_dat_patch(&patch, &baseline)
                .unwrap()
                .change_count,
            1
        );
        assert_eq!(
            project.current_dat_value(&target),
            Some(DatScalar::Number(20480))
        );
        let reopened = NativeProject::open(&root).unwrap();
        assert_eq!(
            reopened.current_dat_value(&target),
            Some(DatScalar::Number(20480))
        );

        let duplicate_change = NativeDatChange::Dat {
            dat: "units".to_string(),
            object_id: 0,
            field: "Hit Points".to_string(),
            before: 20480,
            after: 30000,
        };
        let duplicate = NativeDatPatch {
            changes: vec![duplicate_change.clone(), duplicate_change],
        };
        assert!(project
            .apply_dat_patch(&duplicate, &baseline)
            .unwrap_err()
            .contains("duplicates"));
        assert_eq!(
            project.current_dat_value(&target),
            Some(DatScalar::Number(20480))
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn stale_or_invalid_complex_patch_changes_nothing() {
        let (root, mut project) = project("stale");
        let baseline = BTreeMap::from([(
            DatTarget::Requirement {
                dat: "units".to_string(),
                object_id: 0,
            },
            DatScalar::Text("0".to_string()),
        )]);
        let patch = NativeDatPatch {
            changes: vec![NativeDatChange::Requirement {
                dat: "units".to_string(),
                object_id: 0,
                before: "0".to_string(),
                after: "Default".to_string(),
            }],
        };
        assert!(project
            .apply_dat_patch(&patch, &baseline)
            .unwrap_err()
            .contains("numeric"));
        assert!(project.dat().requirements.tables.is_empty());
        fs::remove_dir_all(root).ok();
    }
    #[test]
    fn batch_patch_persists_exact_state_for_1_50_and_200_changes() {
        for count in [1_u32, 50, 200] {
            let (root, mut project) = project(&format!("batch-{count}"));
            let mut baselines = BTreeMap::new();
            let mut changes = Vec::with_capacity(count as usize);
            for object_id in 0..count {
                let target = DatTarget::Dat {
                    dat: "units".to_string(),
                    object_id,
                    field: "Hit Points".to_string(),
                };
                baselines.insert(target, DatScalar::Number(0));
                changes.push(NativeDatChange::Dat {
                    dat: "units".to_string(),
                    object_id,
                    field: "Hit Points".to_string(),
                    before: 0,
                    after: i64::from(object_id) + 1,
                });
            }

            let applied = project
                .apply_dat_patch(&NativeDatPatch { changes }, &baselines)
                .unwrap();
            let reopened = NativeProject::open(&root).unwrap();

            assert_eq!(applied.change_count, count as usize);
            assert_eq!(
                reopened.dat().standard.tables.get("units").unwrap().len(),
                count as usize
            );
            for object_id in 0..count {
                let value = &reopened.dat().standard.tables["units"][&object_id]["Hit Points"];
                assert_eq!(value.before, 0);
                assert_eq!(value.after, i64::from(object_id) + 1);
            }
            fs::remove_dir_all(root).ok();
        }
    }
}
