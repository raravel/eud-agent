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

pub const PROJECT_SCHEMA_VERSION: u32 = 2;
pub const LEGACY_PROJECT_SCHEMA_VERSION: u32 = 1;
pub const DAT_SCHEMA_VERSION: u32 = 1;
pub const PYTHON_LOCK_DOMAIN: &[u8] = b"eud-agent/python-lock/v1\0";
pub const PYTHON_LOCK_OS: &str = "windows";
pub const PYTHON_LOCK_ARCH: &str = "x86_64";
pub const MANAGED_UV_VERSION: &str = "0.11.3";
/// Canonical EUD Agent Project manifest filename.
pub const PROJECT_MANIFEST_FILE: &str = "project.eap";
/// Legacy manifest filename accepted only by explicit migration.
pub const LEGACY_PROJECT_MANIFEST_FILE: &str = "project.json";
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
    pub python_entrypoints: Vec<String>,
    pub python_dependencies: Vec<String>,
    pub python_lock: Option<PythonLock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editor_compatibility: Option<EditorCompatibility>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProjectManifestV1 {
    schema_version: u32,
    name: String,
    source_map: String,
    output_map: String,
    main_file: String,
    #[serde(default)]
    settings: ProjectSettings,
    #[serde(default)]
    plugins: Vec<EdsPlugin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    editor_compatibility: Option<EditorCompatibility>,
}

impl From<ProjectManifestV1> for ProjectManifest {
    fn from(legacy: ProjectManifestV1) -> Self {
        Self {
            schema_version: PROJECT_SCHEMA_VERSION,
            name: legacy.name,
            source_map: legacy.source_map,
            output_map: legacy.output_map,
            main_file: legacy.main_file,
            settings: legacy.settings,
            plugins: legacy.plugins,
            python_entrypoints: Vec::new(),
            python_dependencies: Vec::new(),
            python_lock: None,
            editor_compatibility: legacy.editor_compatibility,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PythonLock {
    pub digest: String,
    pub target: PythonLockTarget,
    pub packages: Vec<PythonLockedPackage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PythonLockTarget {
    pub os: String,
    pub arch: String,
    pub python_abi: String,
    pub euddraft_fingerprint: String,
    pub uv_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PythonLockedPackage {
    pub name: String,
    pub version: String,
    pub wheel_filename: String,
    pub wheel_tags: Vec<String>,
    pub artifact_url: String,
    pub sha256: String,
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
    /// epScript import prefixes stripped when this project was imported from an E3S
    /// (`import TriggerEditor.leaf` -> `import leaf`). Export restores them; the field
    /// is additive so manifests written before it stay readable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub editor_import_prefixes: Vec<String>,
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

/// One sparse override exactly as the project stores it: which document holds
/// it, how to name it to the user, which target it addresses, and the `before`
/// value it claims is stock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatOverride {
    pub file: &'static str,
    pub label: String,
    pub target: DatTarget,
    pub before: DatScalar,
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
    manifest_path: PathBuf,
    manifest: ProjectManifest,
    dat: NativeDatState,
}

impl NativeProject {
    pub fn create(root: &Path, manifest: ProjectManifest) -> Result<Self, String> {
        validate_manifest(&manifest)?;
        ensure_no_symlink_components(root)?;
        if root_exists_as_symlink(root)? {
            return Err("native project root must not be a symlink".to_string());
        }
        fs::create_dir_all(root).map_err(stringify_io)?;
        for directory in ["src", "dat", "plugins", "maps", "build"] {
            fs::create_dir_all(root.join(directory)).map_err(stringify_io)?;
        }
        let root = canonical_or_absolute(root)?;
        let project = Self {
            manifest_path: root.join(PROJECT_MANIFEST_FILE),
            root,
            manifest,
            dat: NativeDatState::default(),
        };
        project.save_manifest()?;
        project.save_dat_state()?;
        project.create_source(&project.manifest.main_file, "")?;
        validate_manifest_source_paths(&project)?;
        Ok(project)
    }

    /// Open the sole canonical `.eap` manifest in a project root.
    pub fn open(root: &Path) -> Result<Self, String> {
        ensure_no_symlink_components(root)?;
        let root = fs::canonicalize(root).map_err(|error| {
            format!(
                "native project root '{}' is unavailable: {error}",
                root.display()
            )
        })?;
        if !root.is_dir() {
            return Err(format!(
                "native project root is not a directory: {}",
                root.display()
            ));
        }
        let manifest_path = discover_manifest_path(&root)?;
        reject_legacy_authority(&root)?;
        Self::open_manifest(&manifest_path)
    }

    /// Open an explicitly selected canonical `.eap` file without substituting a sibling.
    pub fn open_manifest(path: &Path) -> Result<Self, String> {
        ensure_no_symlink_components(path)?;
        let metadata = fs::symlink_metadata(path).map_err(|error| {
            format!(
                "native project manifest '{}' is unavailable: {error}",
                path.display()
            )
        })?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            return Err("native project manifest must be a regular, non-symlink file".to_string());
        }
        if !path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("eap"))
        {
            return Err("native project manifest must use the .eap extension".to_string());
        }
        let manifest_path = fs::canonicalize(path).map_err(stringify_io)?;
        let root = manifest_path
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| "native project manifest has no parent directory".to_string())?;
        let manifest = read_project_manifest_v2(&manifest_path)?;
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
            manifest_path,
            manifest,
        };
        project.validate_project_state()?;
        Ok(project)
    }

    /// 명시적으로 선택된 스키마 v1 `.eap`를 검증한 뒤 같은 파일에 v2로 원자 변환한다.
    /// 일반 [`Self::open`]과 [`Self::open_manifest`]는 이 함수를 호출하지 않는다.
    pub(crate) fn migrate_v1_manifest(path: &Path) -> Result<Self, String> {
        ensure_regular_manifest_file(path, "eap")?;
        let manifest = read_project_manifest_v1(path)?;
        let manifest_path = fs::canonicalize(path).map_err(stringify_io)?;
        let root = manifest_path
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| "프로젝트 매니페스트의 상위 폴더가 없습니다.".to_string())?;
        let project = Self::from_parts(root, manifest_path, manifest)?;
        project.save_manifest()?;
        Ok(project)
    }

    /// 이전 `project.json`을 검증한 뒤 v2 `project.eap`로 명시적으로 변환한다.
    pub(crate) fn migrate_legacy(root: &Path) -> Result<Self, String> {
        ensure_no_symlink_components(root)?;
        let root = fs::canonicalize(root).map_err(stringify_io)?;
        let legacy_path = root.join(LEGACY_PROJECT_MANIFEST_FILE);
        ensure_regular_manifest_file(&legacy_path, "json")?;
        if !discover_manifest_files(&root)?.is_empty() {
            return Err("정식 매니페스트와 이전 매니페스트가 함께 존재합니다.".to_string());
        }
        let manifest = read_project_manifest_v1(&legacy_path)?;
        let manifest_path = root.join(PROJECT_MANIFEST_FILE);
        let project = Self::from_parts(root, manifest_path.clone(), manifest)?;
        project.save_manifest()?;
        if let Err(error) = fs::remove_file(&legacy_path) {
            let cleanup = fs::remove_file(&manifest_path);
            return Err(match cleanup {
                Ok(()) => format!("project.json 변환을 완료하지 못했습니다: {error}"),
                Err(cleanup) => format!(
                    "project.json 변환을 완료하지 못했고 생성된 project.eap 정리도 실패했습니다: {error}; {cleanup}"
                ),
            });
        }
        Ok(project)
    }

    fn from_parts(
        root: PathBuf,
        manifest_path: PathBuf,
        manifest: ProjectManifest,
    ) -> Result<Self, String> {
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
            manifest_path,
            manifest,
        };
        project.validate_project_state()?;
        Ok(project)
    }

    fn validate_project_state(&self) -> Result<(), String> {
        self.validate_dat_state()?;
        self.require_path(&self.manifest.source_map, true)?;
        self.require_path(&self.manifest.main_file, true)?;
        validate_manifest_source_paths(self)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn manifest(&self) -> &ProjectManifest {
        &self.manifest
    }

    pub fn manifest_path(&self) -> &Path {
        &self.manifest_path
    }

    pub fn manifest_bytes(&self) -> Result<Vec<u8>, String> {
        fs::read(&self.manifest_path).map_err(stringify_io)
    }

    pub fn manifest_sha256(&self) -> Result<String, String> {
        self.manifest_bytes().map(|bytes| sha256_hex(&bytes))
    }

    pub fn has_direct_python(&self) -> Result<bool, String> {
        Ok(!self.manifest.python_entrypoints.is_empty()
            || !self.manifest.python_dependencies.is_empty()
            || self.manifest.python_lock.is_some()
            || self
                .list_source_files()?
                .iter()
                .any(|path| is_python_source_path(path)))
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
        self.require_path_with(&self.manifest.output_map, false, true)
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
            if !is_editable_source_path(&path) {
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
        require_editable_source_path(path)?;
        read_bounded_text(&self.require_path(path, true)?)
    }

    pub fn write_source(&self, path: &str, content: &str) -> Result<(), String> {
        require_editable_source_path(path)?;
        if content.len() > MAX_TEXT_BYTES {
            return Err(format!("source content exceeds {MAX_TEXT_BYTES} bytes"));
        }
        let target = self.require_path(path, false)?;
        write_atomic_bytes(&target, content.as_bytes()).map_err(|error| error.to_string())
    }

    pub fn create_source(&self, path: &str, content: &str) -> Result<(), String> {
        require_editable_source_path(path)?;
        let normalized = normalize_relative(path)?;
        self.reject_source_case_collision(&normalized, None)?;
        let target = self.require_path(&normalized, false)?;
        if target.exists() {
            return Err(format!("소스 파일이 이미 존재합니다: {normalized}"));
        }
        self.write_source(&normalized, content)
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
        require_editable_source_path(from)?;
        require_editable_source_path(to)?;
        let from = normalize_relative(from)?;
        let to = normalize_relative(to)?;
        let moves_main = self.manifest.main_file.eq_ignore_ascii_case(&from);
        if moves_main {
            require_eps_source_path(&to)?;
        }
        let entrypoint_index = self
            .manifest
            .python_entrypoints
            .iter()
            .position(|path| path.eq_ignore_ascii_case(&from));
        if entrypoint_index.is_some() {
            require_python_source_path(&to)?;
        }
        self.reject_source_case_collision(&to, Some(&from))?;
        let source = self.require_path(&from, true)?;
        let target = self.require_path(&to, false)?;
        if target.exists() && !from.eq_ignore_ascii_case(&to) {
            return Err(format!("이동 대상 소스가 이미 존재합니다: {to}"));
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(stringify_io)?;
        }
        fs::rename(&source, &target).map_err(stringify_io)?;

        if !moves_main && entrypoint_index.is_none() {
            return Ok(());
        }
        let previous = self.manifest.clone();
        if moves_main {
            self.manifest.main_file = to.clone();
        }
        if let Some(index) = entrypoint_index {
            self.manifest.python_entrypoints[index] = to.clone();
        }
        if let Err(error) = validate_manifest(&self.manifest).and_then(|_| self.save_manifest()) {
            self.manifest = previous;
            return match fs::rename(&target, &source) {
                Ok(()) => Err(error),
                Err(rollback) => Err(format!(
                    "소스 이동 후 매니페스트 저장에 실패했고 파일 복원도 실패했습니다: {error}; {rollback}"
                )),
            };
        }
        Ok(())
    }

    pub fn set_project_setting(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            "OpenMapName" => self.manifest.source_map = normalize_relative(value)?,
            "SaveMapName" => self.manifest.output_map = normalize_output_map_relative(value)?,
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
        require_editable_source_path(path)?;
        if self.manifest.main_file.eq_ignore_ascii_case(path) {
            return Err("선택된 mainFile은 삭제할 수 없습니다.".to_string());
        }
        if self
            .manifest
            .python_entrypoints
            .iter()
            .any(|entrypoint| entrypoint.eq_ignore_ascii_case(path))
        {
            return Err("선택된 Python 진입점은 삭제할 수 없습니다.".to_string());
        }
        fs::remove_file(self.require_path(path, true)?).map_err(stringify_io)
    }

    pub fn set_main_file(&mut self, path: &str) -> Result<(), String> {
        require_eps_source_path(path)?;
        let normalized = normalize_relative(path)?;
        if !self
            .list_source_files()?
            .iter()
            .any(|existing| existing == &normalized)
        {
            return Err("mainFile은 기존 소스의 정확한 대소문자 경로여야 합니다.".to_string());
        }
        self.require_path(&normalized, true)?;
        let previous = self.manifest.main_file.clone();
        self.manifest.main_file = normalized;
        if let Err(error) = self.save_manifest() {
            self.manifest.main_file = previous;
            return Err(error);
        }
        Ok(())
    }

    pub fn replace_python_dependencies(
        &mut self,
        dependencies: Vec<String>,
        python_lock: Option<PythonLock>,
    ) -> Result<(String, Vec<u8>), String> {
        let dependencies = validate_python_dependencies(&dependencies)?;
        validate_python_lock(&dependencies, python_lock.as_ref())?;
        let before_bytes = fs::read(&self.manifest_path).map_err(stringify_io)?;
        let previous = self.manifest.clone();
        self.manifest.python_dependencies = dependencies;
        self.manifest.python_lock = python_lock;
        let after_bytes =
            serde_json::to_vec_pretty(&self.manifest).map_err(|error| error.to_string())?;
        if let Err(error) = validate_manifest(&self.manifest).and_then(|_| {
            write_atomic_bytes(&self.manifest_path, &after_bytes).map_err(|error| error.to_string())
        }) {
            self.manifest = previous;
            return Err(error);
        }
        match self.revision() {
            Ok(revision) => Ok((revision, after_bytes)),
            Err(error) => {
                self.manifest = previous;
                write_atomic_bytes(&self.manifest_path, &before_bytes).map_err(|restore_error| {
                    format!(
                        "Python 의존성 변경 후 리비전 계산과 project.eap 복원이 모두 실패했습니다: {error}; {restore_error}"
                    )
                })?;
                Err(format!(
                    "Python 의존성 변경 후 리비전을 계산하지 못해 project.eap을 복원했습니다: {error}"
                ))
            }
        }
    }

    pub fn restore_manifest_bytes(
        &mut self,
        expected_manifest_sha256: &str,
        bytes: &[u8],
    ) -> Result<String, String> {
        let current_manifest = fs::read(&self.manifest_path).map_err(stringify_io)?;
        if sha256_hex(&current_manifest) != expected_manifest_sha256 {
            return Err("project.eap이 바뀌어 매니페스트를 복원할 수 없습니다.".to_string());
        }
        let manifest: ProjectManifest = serde_json::from_slice(bytes)
            .map_err(|error| format!("복원할 project.eap JSON이 올바르지 않습니다: {error}"))?;
        validate_manifest(&manifest)?;
        let previous = self.manifest.clone();
        self.manifest = manifest;
        if let Err(error) = validate_manifest_source_paths(self).and_then(|_| {
            write_atomic_bytes(&self.manifest_path, bytes).map_err(|error| error.to_string())
        }) {
            self.manifest = previous;
            return Err(error);
        }
        Ok(sha256_hex(bytes))
    }

    fn reject_source_case_collision(
        &self,
        candidate: &str,
        ignored: Option<&str>,
    ) -> Result<(), String> {
        let folded = windows_case_fold(candidate);
        if self.list_source_files()?.into_iter().any(|path| {
            ignored.map_or(true, |ignored| !path.eq_ignore_ascii_case(ignored))
                && windows_case_fold(&path) == folded
        }) {
            return Err(format!(
                "대소문자만 다른 소스 경로가 이미 존재합니다: {candidate}"
            ));
        }
        Ok(())
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

    /// Every sparse override this project stores, with the `before` value it
    /// claims is the stock catalog value.
    ///
    /// `dat_patch` checks that claim when it writes. An edit made straight to
    /// `dat/*.json` does not pass through the patch, and a wrong `before` is
    /// not a cosmetic error: the generator emits `SetMemory(offset, Add,
    /// after - before)`, so the map silently runs on the wrong numbers. The
    /// build checks the same rule again against this list.
    pub fn dat_overrides(&self) -> Vec<DatOverride> {
        let mut overrides = Vec::new();
        for (file, document, numeric) in [
            (STANDARD_DAT_FILE, &self.dat.standard, true),
            (XDAT_FILE, &self.dat.xdat, false),
        ] {
            for (dat, objects) in &document.tables {
                for (object_id, fields) in objects {
                    for (field, change) in fields {
                        let target = if numeric {
                            DatTarget::Dat {
                                dat: dat.clone(),
                                object_id: *object_id,
                                field: field.clone(),
                            }
                        } else {
                            DatTarget::Xdat {
                                dat: dat.clone(),
                                object_id: *object_id,
                                field: field.clone(),
                            }
                        };
                        overrides.push(DatOverride {
                            file,
                            label: format!("{dat}[{object_id}].{field}"),
                            target,
                            before: DatScalar::Number(change.before),
                        });
                    }
                }
            }
        }
        for (index, change) in &self.dat.tbl.values {
            overrides.push(DatOverride {
                file: TBL_FILE,
                label: format!("tbl[{index}]"),
                target: DatTarget::Tbl(*index),
                before: DatScalar::Text(change.before.clone()),
            });
        }
        for (dat, objects) in &self.dat.requirements.tables {
            for (object_id, change) in objects {
                overrides.push(DatOverride {
                    file: REQUIREMENTS_FILE,
                    label: format!("{dat}[{object_id}]"),
                    target: DatTarget::Requirement {
                        dat: dat.clone(),
                        object_id: *object_id,
                    },
                    before: DatScalar::Text(change.before.clone()),
                });
            }
        }
        for (set_id, change) in &self.dat.buttons.values {
            overrides.push(DatOverride {
                file: BUTTONS_FILE,
                label: format!("buttons[{set_id}]"),
                target: DatTarget::Button(*set_id),
                before: DatScalar::Text(change.before.clone()),
            });
        }
        overrides
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
            if is_editable_source_path(&path) {
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
        write_json(&self.manifest_path, &self.manifest)
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
        self.require_path_with(relative, must_exist, false)
    }

    fn require_path_with(
        &self,
        relative: &str,
        must_exist: bool,
        allow_brackets: bool,
    ) -> Result<PathBuf, String> {
        let relative = normalize_relative_with(relative, allow_brackets)?;
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectSchemaEnvelope {
    schema_version: u32,
}

fn ensure_regular_manifest_file(path: &Path, extension: &str) -> Result<(), String> {
    ensure_no_symlink_components(path)?;
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        format!(
            "프로젝트 매니페스트 '{}'를 읽을 수 없습니다: {error}",
            path.display()
        )
    })?;
    if metadata.file_type().is_symlink()
        || crate::memory::is_reparse_point(&metadata)
        || !metadata.file_type().is_file()
    {
        return Err("프로젝트 매니페스트는 일반 파일이어야 합니다.".to_string());
    }
    if !path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case(extension))
    {
        return Err(format!(
            "프로젝트 매니페스트 확장자는 .{extension}이어야 합니다."
        ));
    }
    Ok(())
}

pub(crate) fn project_manifest_schema_version(path: &Path) -> Result<u32, String> {
    let text = read_bounded_text(path)?;
    serde_json::from_str::<ProjectSchemaEnvelope>(&text)
        .map(|envelope| envelope.schema_version)
        .map_err(|error| format!("프로젝트 매니페스트 JSON이 올바르지 않습니다: {error}"))
}

fn read_project_manifest_v2(path: &Path) -> Result<ProjectManifest, String> {
    let text = read_bounded_text(path)?;
    let envelope: ProjectSchemaEnvelope = serde_json::from_str(&text)
        .map_err(|error| format!("project.eap JSON이 올바르지 않습니다: {error}"))?;
    match envelope.schema_version {
        PROJECT_SCHEMA_VERSION => serde_json::from_str(&text)
            .map_err(|error| format!("project.eap 스키마 v2가 올바르지 않습니다: {error}")),
        LEGACY_PROJECT_SCHEMA_VERSION => Err(
            "project.eap 스키마 v1은 설정 화면에서 프로젝트를 명시적으로 열어 v2로 변환해야 합니다."
                .to_string(),
        ),
        version => Err(format!(
            "지원하지 않는 project.eap schemaVersion입니다: {version} (필요: {PROJECT_SCHEMA_VERSION})"
        )),
    }
}

fn read_project_manifest_v1(path: &Path) -> Result<ProjectManifest, String> {
    let text = read_bounded_text(path)?;
    let envelope: ProjectSchemaEnvelope = serde_json::from_str(&text)
        .map_err(|error| format!("이전 프로젝트 매니페스트 JSON이 올바르지 않습니다: {error}"))?;
    if envelope.schema_version != LEGACY_PROJECT_SCHEMA_VERSION {
        return Err(format!(
            "schemaVersion {}은 v2로 변환할 수 없습니다.",
            envelope.schema_version
        ));
    }
    let legacy: ProjectManifestV1 = serde_json::from_str(&text)
        .map_err(|error| format!("이전 프로젝트 스키마 v1이 올바르지 않습니다: {error}"))?;
    Ok(legacy.into())
}

pub fn validate_python_dependencies(dependencies: &[String]) -> Result<Vec<String>, String> {
    let mut normalized = Vec::with_capacity(dependencies.len());
    let mut names = BTreeSet::new();
    for (index, dependency) in dependencies.iter().enumerate() {
        let (name, version) = normalize_exact_python_dependency(dependency)
            .map_err(|error| format!("pythonDependencies[{index}]: {error}"))?;
        if !names.insert(name.clone()) {
            return Err(format!("Python 직접 의존성이 중복되었습니다: {name}"));
        }
        normalized.push(format!("{name}=={version}"));
    }
    normalized.sort_by(|left, right| {
        let left_name = left.split_once("==").map_or(left.as_str(), |value| value.0);
        let right_name = right
            .split_once("==")
            .map_or(right.as_str(), |value| value.0);
        left_name.cmp(right_name)
    });
    Ok(normalized)
}

pub fn normalize_exact_python_dependency(value: &str) -> Result<(String, String), String> {
    if value.trim() != value || value.matches("==").count() != 1 {
        return Err("의존성은 공백 없는 정확한 name==version 형식이어야 합니다.".to_string());
    }
    let (name, version) = value
        .split_once("==")
        .ok_or_else(|| "의존성은 정확한 name==version 형식이어야 합니다.".to_string())?;
    Ok((
        normalize_python_name(name)?,
        normalize_python_version(version)?,
    ))
}

fn normalize_python_name(value: &str) -> Result<String, String> {
    if value.is_empty()
        || !value.is_ascii()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        || !value.as_bytes()[0].is_ascii_alphanumeric()
        || !value.as_bytes()[value.len() - 1].is_ascii_alphanumeric()
    {
        return Err("Python 패키지 이름이 PEP 503 형식이 아닙니다.".to_string());
    }
    let mut normalized = String::with_capacity(value.len());
    let mut separator = false;
    for byte in value.bytes() {
        if matches!(byte, b'-' | b'_' | b'.') {
            separator = true;
        } else {
            if separator && !normalized.is_empty() {
                normalized.push('-');
            }
            separator = false;
            normalized.push(char::from(byte.to_ascii_lowercase()));
        }
    }
    Ok(normalized)
}

fn normalize_python_version(value: &str) -> Result<String, String> {
    if value.is_empty() || !value.is_ascii() || value.trim() != value {
        return Err("Python 버전이 비어 있거나 ASCII 정확 버전이 아닙니다.".to_string());
    }
    let lower = value.to_ascii_lowercase();
    let mut public_and_local = lower.split('+');
    let public = public_and_local.next().unwrap_or_default();
    let local = public_and_local.next();
    if public_and_local.next().is_some() {
        return Err("Python 버전에 '+'가 두 번 이상 들어 있습니다.".to_string());
    }
    let (epoch, public) = match public.split_once('!') {
        Some((epoch, rest))
            if !epoch.is_empty() && epoch.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            (Some(normalize_decimal(epoch)?), rest)
        }
        Some(_) => return Err("Python 버전 epoch 형식이 올바르지 않습니다.".to_string()),
        None => (None, public),
    };
    let release_end = public
        .bytes()
        .position(|byte| !byte.is_ascii_digit() && byte != b'.')
        .unwrap_or(public.len());
    let release = &public[..release_end];
    if release.is_empty() || release.split('.').any(|part| part.is_empty()) {
        return Err("Python 버전 release 형식이 올바르지 않습니다.".to_string());
    }
    let release = release
        .split('.')
        .map(normalize_decimal)
        .collect::<Result<Vec<_>, _>>()?
        .join(".");
    let mut rest = &public[release_end..];
    let mut suffix = String::new();
    for label in ["rc", "a", "b"] {
        if let Some(after) = rest.strip_prefix(label) {
            let (number, remaining) = take_decimal(after)?;
            suffix.push_str(label);
            suffix.push_str(&normalize_decimal(number)?);
            rest = remaining;
            break;
        }
    }
    if let Some(after) = rest.strip_prefix(".post") {
        let (number, remaining) = take_decimal(after)?;
        suffix.push_str(".post");
        suffix.push_str(&normalize_decimal(number)?);
        rest = remaining;
    }
    if let Some(after) = rest.strip_prefix(".dev") {
        let (number, remaining) = take_decimal(after)?;
        suffix.push_str(".dev");
        suffix.push_str(&normalize_decimal(number)?);
        rest = remaining;
    }
    if !rest.is_empty() {
        return Err("Python 버전은 정규화 가능한 정확한 PEP 440 버전이어야 합니다.".to_string());
    }
    let mut normalized = String::new();
    if let Some(epoch) = epoch {
        normalized.push_str(&epoch);
        normalized.push('!');
    }
    normalized.push_str(&release);
    normalized.push_str(&suffix);
    if let Some(local) = local {
        let mut local_parts = Vec::new();
        for part in local.split(['-', '_', '.']) {
            if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
                return Err("Python 로컬 버전 형식이 올바르지 않습니다.".to_string());
            }
            local_parts.push(if part.bytes().all(|byte| byte.is_ascii_digit()) {
                normalize_decimal(part)?
            } else {
                part.to_string()
            });
        }
        normalized.push('+');
        normalized.push_str(&local_parts.join("."));
    }
    Ok(normalized)
}

fn take_decimal(value: &str) -> Result<(&str, &str), String> {
    let length = value
        .bytes()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if length == 0 {
        return Err("Python 버전 접미사에는 숫자가 필요합니다.".to_string());
    }
    Ok(value.split_at(length))
}

fn normalize_decimal(value: &str) -> Result<String, String> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("Python 버전 숫자가 올바르지 않습니다.".to_string());
    }
    Ok(value
        .trim_start_matches('0')
        .to_string()
        .chars()
        .next()
        .map_or_else(
            || "0".to_string(),
            |_| value.trim_start_matches('0').to_string(),
        ))
}

pub fn validate_python_lock(
    dependencies: &[String],
    python_lock: Option<&PythonLock>,
) -> Result<(), String> {
    let normalized_dependencies = validate_python_dependencies(dependencies)?;
    if normalized_dependencies != dependencies {
        return Err("pythonDependencies는 PEP 503 이름 순서의 정규형이어야 합니다.".to_string());
    }
    let Some(python_lock) = python_lock else {
        return if dependencies.is_empty() {
            Ok(())
        } else {
            Err("비어 있지 않은 pythonDependencies에는 pythonLock이 필요합니다.".to_string())
        };
    };
    if dependencies.is_empty() {
        return Err("pythonDependencies가 비어 있으면 pythonLock도 null이어야 합니다.".to_string());
    }
    validate_lock_target(&python_lock.target)?;
    if python_lock.packages.is_empty() {
        return Err("pythonLock.packages는 비어 있을 수 없습니다.".to_string());
    }
    let mut previous: Option<(&str, &str, &str)> = None;
    let mut package_versions = BTreeMap::new();
    for (index, package) in python_lock.packages.iter().enumerate() {
        let canonical_name = normalize_python_name(&package.name)?;
        let canonical_version = normalize_python_version(&package.version)?;
        if canonical_name != package.name || canonical_version != package.version {
            return Err(format!(
                "pythonLock.packages[{index}] 이름 또는 버전이 정규형이 아닙니다."
            ));
        }
        let key = (
            package.name.as_str(),
            package.version.as_str(),
            package.wheel_filename.as_str(),
        );
        if previous.is_some_and(|previous| previous >= key) {
            return Err(
                "pythonLock.packages 정렬 순서가 올바르지 않거나 중복되었습니다.".to_string(),
            );
        }
        previous = Some(key);
        if package_versions
            .insert(package.name.clone(), package.version.clone())
            .is_some()
        {
            return Err(format!(
                "pythonLock 패키지 이름이 중복되었습니다: {}",
                package.name
            ));
        }
        validate_locked_wheel(package)
            .map_err(|error| format!("pythonLock.packages[{index}]: {error}"))?;
    }
    for dependency in dependencies {
        let (name, version) = dependency.split_once("==").expect("validated dependency");
        if package_versions.get(name).map(String::as_str) != Some(version) {
            return Err(format!(
                "직접 의존성 {dependency}의 정확한 wheel 기록이 없습니다."
            ));
        }
    }
    let expected =
        compute_python_lock_digest(dependencies, &python_lock.target, &python_lock.packages)?;
    if python_lock.digest != expected {
        return Err("pythonLock.digest가 선언 및 wheel 기록과 일치하지 않습니다.".to_string());
    }
    Ok(())
}

fn validate_lock_target(target: &PythonLockTarget) -> Result<(), String> {
    if target.os != PYTHON_LOCK_OS
        || target.arch != PYTHON_LOCK_ARCH
        || target.uv_version != MANAGED_UV_VERSION
    {
        return Err(format!(
            "pythonLock.target은 {PYTHON_LOCK_OS}/{PYTHON_LOCK_ARCH}/uv {MANAGED_UV_VERSION}이어야 합니다."
        ));
    }
    if !target.python_abi.starts_with("cp")
        || !(4..=6).contains(&target.python_abi.len())
        || !target.python_abi[2..]
            .bytes()
            .all(|byte| byte.is_ascii_digit())
    {
        return Err("pythonLock.target.pythonAbi가 올바른 CPython ABI가 아닙니다.".to_string());
    }
    validate_lower_sha256(&target.euddraft_fingerprint, "euddraftFingerprint")
}

fn validate_locked_wheel(package: &PythonLockedPackage) -> Result<(), String> {
    validate_lower_sha256(&package.sha256, "sha256")?;
    if package.wheel_filename.contains(['/', '\\']) || !package.wheel_filename.ends_with(".whl") {
        return Err("wheelFilename은 경로가 없는 .whl 파일명이어야 합니다.".to_string());
    }
    let stem = package.wheel_filename.trim_end_matches(".whl");
    let parts: Vec<_> = stem.split('-').collect();
    if parts.len() < 5
        || normalize_python_name(parts[0]).as_deref() != Ok(package.name.as_str())
        || normalize_python_version(parts[1]).as_deref() != Ok(package.version.as_str())
    {
        return Err("wheelFilename의 배포 이름 또는 버전이 패키지 기록과 다릅니다.".to_string());
    }
    let python_tags = parts[parts.len() - 3].split('.').collect::<Vec<_>>();
    let abi_tags = parts[parts.len() - 2].split('.').collect::<Vec<_>>();
    let platform_tags = parts[parts.len() - 1].split('.').collect::<Vec<_>>();
    if python_tags
        .iter()
        .chain(&abi_tags)
        .chain(&platform_tags)
        .any(|tag| {
            tag.is_empty()
                || !tag
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        })
    {
        return Err("wheelFilename 태그 형식이 올바르지 않습니다.".to_string());
    }
    let mut parsed_tags = Vec::new();
    for python in python_tags {
        for abi in &abi_tags {
            for platform in &platform_tags {
                parsed_tags.push(format!("{python}-{abi}-{platform}"));
            }
        }
    }
    parsed_tags.sort();
    parsed_tags.dedup();
    if parsed_tags != package.wheel_tags {
        return Err("wheelTags가 wheelFilename에서 파싱한 정렬 태그와 다릅니다.".to_string());
    }
    let Some(path) = package
        .artifact_url
        .strip_prefix("https://files.pythonhosted.org/packages/")
    else {
        return Err(
            "artifactUrl은 files.pythonhosted.org의 HTTPS PyPI URL이어야 합니다.".to_string(),
        );
    };
    if path.is_empty()
        || path.contains(['?', '#', '\\'])
        || !path.ends_with(&format!("/{}", package.wheel_filename))
    {
        return Err("artifactUrl이 정확한 PyPI wheel 파일을 가리키지 않습니다.".to_string());
    }
    Ok(())
}

fn validate_lower_sha256(value: &str, label: &str) -> Result<(), String> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(format!("{label}은 소문자 SHA-256이어야 합니다."));
    }
    Ok(())
}

pub fn compute_python_lock_digest(
    dependencies: &[String],
    target: &PythonLockTarget,
    packages: &[PythonLockedPackage],
) -> Result<String, String> {
    let mut declarations = BTreeMap::new();
    declarations.insert(
        "pythonDependencies",
        serde_json::to_value(dependencies).map_err(|error| error.to_string())?,
    );
    let mut target_value = BTreeMap::new();
    target_value.insert("arch", serde_json::Value::String(target.arch.clone()));
    target_value.insert(
        "euddraftFingerprint",
        serde_json::Value::String(target.euddraft_fingerprint.clone()),
    );
    target_value.insert("os", serde_json::Value::String(target.os.clone()));
    target_value.insert(
        "pythonAbi",
        serde_json::Value::String(target.python_abi.clone()),
    );
    target_value.insert(
        "uvVersion",
        serde_json::Value::String(target.uv_version.clone()),
    );
    let package_values = packages
        .iter()
        .map(|package| {
            let mut value = BTreeMap::new();
            value.insert(
                "artifactUrl",
                serde_json::Value::String(package.artifact_url.clone()),
            );
            value.insert("name", serde_json::Value::String(package.name.clone()));
            value.insert("sha256", serde_json::Value::String(package.sha256.clone()));
            value.insert(
                "version",
                serde_json::Value::String(package.version.clone()),
            );
            value.insert(
                "wheelFilename",
                serde_json::Value::String(package.wheel_filename.clone()),
            );
            value.insert(
                "wheelTags",
                serde_json::to_value(&package.wheel_tags).map_err(|error| error.to_string())?,
            );
            Ok(value)
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut value = BTreeMap::new();
    value.insert(
        "declarations",
        serde_json::to_value(declarations).map_err(|error| error.to_string())?,
    );
    value.insert(
        "packages",
        serde_json::to_value(package_values).map_err(|error| error.to_string())?,
    );
    value.insert(
        "target",
        serde_json::to_value(target_value).map_err(|error| error.to_string())?,
    );
    let canonical = serde_json::to_vec(&value).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    hasher.update(PYTHON_LOCK_DOMAIN);
    hasher.update(canonical);
    Ok(format!("{:x}", hasher.finalize()))
}

fn validate_manifest_source_paths(project: &NativeProject) -> Result<(), String> {
    let files = project.list_source_files()?;
    let mut exact = BTreeSet::new();
    let mut folded = BTreeSet::new();
    for path in files.iter().filter(|path| is_editable_source_path(path)) {
        if !folded.insert(windows_case_fold(path)) {
            return Err(format!(
                "대소문자를 무시하면 중복되는 소스 경로가 있습니다: {path}"
            ));
        }
        exact.insert(path.as_str());
        project.require_path(path, true)?;
    }
    if !exact.contains(project.manifest.main_file.as_str()) {
        return Err(format!(
            "mainFile이 정확한 대소문자의 기존 EPS 소스를 가리키지 않습니다: {}",
            project.manifest.main_file
        ));
    }
    for entrypoint in &project.manifest.python_entrypoints {
        if !exact.contains(entrypoint.as_str()) {
            return Err(format!(
                "Python 진입점이 정확한 대소문자의 기존 소스를 가리키지 않습니다: {entrypoint}"
            ));
        }
    }
    Ok(())
}

/// Canonical build output name for a project: `build/[EUD]<name>.<ext>`.
/// `[`/`]` are safe here because the output map only ever appears as the
/// `[main]` `output:` value in the generated EDS, never as a section header.
pub fn default_output_map(name: &str, extension: &str) -> String {
    format!("build/[EUD]{name}.{extension}")
}

pub fn validate_manifest(manifest: &ProjectManifest) -> Result<(), String> {
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
    let output = normalize_output_map_relative(&manifest.output_map)?;
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
    require_eps_source_path(&main)?;
    let mut entrypoints = BTreeSet::new();
    for entrypoint in &manifest.python_entrypoints {
        require_python_source_path(entrypoint)?;
        if normalize_relative(entrypoint)? != *entrypoint {
            return Err(format!(
                "Python 진입점 경로가 정규형이 아닙니다: {entrypoint}"
            ));
        }
        if !entrypoints.insert(windows_case_fold(entrypoint)) {
            return Err(format!("Python 진입점이 중복되었습니다: {entrypoint}"));
        }
    }
    validate_python_lock(&manifest.python_dependencies, manifest.python_lock.as_ref())?;
    if let Some(value) = manifest.settings.decode_unit_name.as_deref() {
        if value.is_empty()
            || value.len() > 128
            || value
                .chars()
                .any(|character| matches!(character, '\r' | '\n' | '\0'))
        {
            return Err(
                "settings.decodeUnitName은 1..128자의 단일 행 값이어야 합니다.".to_string(),
            );
        }
    }
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

pub fn require_eps_source_path(path: &str) -> Result<(), String> {
    require_source_extension(path, "eps", "EPS")
}

pub fn require_python_source_path(path: &str) -> Result<(), String> {
    require_source_extension(path, "py", "Python")
}

pub fn require_editable_source_path(path: &str) -> Result<(), String> {
    let normalized = normalize_relative(path)?;
    if !normalized.starts_with("src/") || !matches_extension(&normalized, &["eps", "py"]) {
        return Err("편집 가능한 소스는 src/ 아래의 .eps 또는 .py 파일이어야 합니다.".to_string());
    }
    Ok(())
}

fn require_source_extension(path: &str, extension: &str, label: &str) -> Result<(), String> {
    let normalized = normalize_relative(path)?;
    if !normalized.starts_with("src/") || !matches_extension(&normalized, &[extension]) {
        return Err(format!(
            "{label} 소스는 src/ 아래의 .{extension} 파일이어야 합니다."
        ));
    }
    Ok(())
}

fn is_python_source_path(path: &str) -> bool {
    require_python_source_path(path).is_ok()
}

fn is_editable_source_path(path: &str) -> bool {
    require_editable_source_path(path).is_ok()
}

fn windows_case_fold(value: &str) -> String {
    value.to_lowercase()
}

/// Normalize a project-relative path that may appear as an EDS section header
/// (`[src/plugin.eps]`), so `[` and `]` are rejected outright.
pub fn normalize_relative(value: &str) -> Result<String, String> {
    normalize_relative_with(value, false)
}

/// Normalize the output map path. It is only ever emitted as the `[main]`
/// `output:` value, never as a section header, so `[EUD]name.scx` is allowed.
pub fn normalize_output_map_relative(value: &str) -> Result<String, String> {
    normalize_relative_with(value, true)
}

/// Normalize an arbitrary path under the project root for the `fs_*` tools.
/// They address the whole root, where `build/[EUD]name.scx` is a real file
/// name, so brackets are allowed; everything `rules.md` rejects still is.
pub(crate) fn normalize_relative_path(value: &str) -> Result<String, String> {
    normalize_relative_with(value, true)
}

fn normalize_relative_with(value: &str, allow_brackets: bool) -> Result<String, String> {
    if value.is_empty()
        || value.contains(['\0', '\\', '\r', '\n'])
        || (!allow_brackets && value.contains(['[', ']']))
    {
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

/// Build-time generated shadows of the canonical source tree. They are
/// outputs, never canonical state, so every source list/snapshot/revision/
/// search/export surface skips them (plan D8).
pub(crate) fn is_generated_artifact_dir(name: &str) -> bool {
    name.eq_ignore_ascii_case("__epspy__") || name.eq_ignore_ascii_case("__pycache__")
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
        if file_type.is_dir()
            && is_generated_artifact_dir(entry.file_name().to_string_lossy().as_ref())
        {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path()).map_err(stringify_io)?;
        if file_type.is_symlink() || crate::memory::is_reparse_point(&metadata) {
            return Err(format!(
                "네이티브 소스 트리에는 심볼릭 링크나 재분석 지점을 둘 수 없습니다: {}",
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

fn root_exists_as_symlink(root: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(root) {
        Ok(metadata) => Ok(metadata.file_type().is_symlink()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(stringify_io(error)),
    }
}

pub(crate) fn discover_manifest_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut manifests = Vec::new();
    for entry in fs::read_dir(root).map_err(stringify_io)? {
        let entry = entry.map_err(stringify_io)?;
        let file_name = entry.file_name();
        let is_eap = file_name
            .to_str()
            .and_then(|name| Path::new(name).extension())
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("eap"));
        if !is_eap {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path()).map_err(stringify_io)?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "canonical .eap manifest must be a regular, non-symlink file: {}",
                entry.path().display()
            ));
        }
        if !metadata.file_type().is_file() {
            return Err(format!(
                "canonical .eap manifest must be a regular file: {}",
                entry.path().display()
            ));
        }
        manifests.push(fs::canonicalize(entry.path()).map_err(stringify_io)?);
    }
    Ok(manifests)
}

fn discover_manifest_path(root: &Path) -> Result<PathBuf, String> {
    let mut manifests = discover_manifest_files(root)?;
    match manifests.len() {
        0 => Err(format!(
            "native project has no canonical .eap manifest in {}",
            root.display()
        )),
        1 => Ok(manifests.remove(0)),
        count => Err(format!(
            "native project has ambiguous canonical manifests ({count} .eap files)"
        )),
    }
}

fn reject_legacy_authority(root: &Path) -> Result<(), String> {
    for entry in fs::read_dir(root).map_err(stringify_io)? {
        let entry = entry.map_err(stringify_io)?;
        let is_descriptor = entry
            .path()
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("eudproj"));
        let is_legacy_manifest = entry
            .file_name()
            .to_str()
            .is_some_and(|value| value.eq_ignore_ascii_case(LEGACY_PROJECT_MANIFEST_FILE));
        if is_descriptor || is_legacy_manifest {
            return Err("project has both canonical and legacy manifest authorities".to_string());
        }
    }
    Ok(())
}

fn ensure_no_symlink_components(path: &Path) -> Result<(), String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map_err(stringify_io)?.join(path)
    };
    for ancestor in absolute.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if crate::memory::is_untrusted_link(&metadata) => {
                return Err(format!(
                    "프로젝트 경로에 심볼릭 링크 또는 재분석 지점이 있습니다: {}",
                    ancestor.display()
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(stringify_io(error)),
        }
    }
    Ok(())
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
            schema_version: PROJECT_SCHEMA_VERSION,
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
            python_entrypoints: Vec::new(),
            python_dependencies: Vec::new(),
            python_lock: None,
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
    fn generated_artifact_dirs_stay_out_of_the_canonical_source_tree() {
        let (root, project) = project("artifacts");
        fs::create_dir_all(root.join("src/__epspy__")).unwrap();
        fs::write(root.join("src/__epspy__/main.py"), b"# generated shadow\n").unwrap();
        fs::create_dir_all(root.join("src/sub/__pycache__")).unwrap();
        fs::write(root.join("src/sub/__pycache__/x.pyc"), b"\x00\x01").unwrap();

        let reopened = NativeProject::open(&root).unwrap();
        assert_eq!(
            reopened.list_source_files().unwrap(),
            vec!["src/main.eps".to_string()]
        );
        assert!(!reopened.has_direct_python().unwrap());
        // Build artifacts must not perturb the project revision.
        assert_eq!(reopened.revision().unwrap(), project.revision().unwrap());
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
    #[cfg(windows)]
    #[test]
    fn dependency_replace_restores_manifest_when_revision_fails_after_write() {
        use std::os::windows::fs::OpenOptionsExt;

        let (root, mut project) = project("dependency-revision-rollback");
        let before = fs::read(root.join(PROJECT_MANIFEST_FILE)).unwrap();
        let dependencies = vec!["demo==1.0".to_string()];
        let target = PythonLockTarget {
            os: PYTHON_LOCK_OS.to_string(),
            arch: PYTHON_LOCK_ARCH.to_string(),
            python_abi: "cp313".to_string(),
            euddraft_fingerprint: "a".repeat(64),
            uv_version: MANAGED_UV_VERSION.to_string(),
        };
        let packages = vec![PythonLockedPackage {
            name: "demo".to_string(),
            version: "1.0".to_string(),
            wheel_filename: "demo-1.0-py3-none-any.whl".to_string(),
            wheel_tags: vec!["py3-none-any".to_string()],
            artifact_url: "https://files.pythonhosted.org/packages/aa/demo-1.0-py3-none-any.whl"
                .to_string(),
            sha256: "b".repeat(64),
        }];
        let lock = PythonLock {
            digest: compute_python_lock_digest(&dependencies, &target, &packages).unwrap(),
            target,
            packages,
        };
        let _source_lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(root.join("src/main.eps"))
            .unwrap();
        assert!(project
            .replace_python_dependencies(dependencies, Some(lock))
            .is_err());
        assert_eq!(fs::read(root.join(PROJECT_MANIFEST_FILE)).unwrap(), before);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn main_file_requires_exact_existing_source_case() {
        let (root, mut project) = project("main-case");
        assert!(project.set_main_file("src/MAIN.eps").is_err());
        assert_eq!(project.manifest().main_file, "src/main.eps");
        assert_eq!(
            NativeProject::open(&root).unwrap().manifest().main_file,
            "src/main.eps"
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn renamed_manifest_remains_the_persistence_target() {
        let (root, project) = project("renamed-manifest");
        let original = root.join(PROJECT_MANIFEST_FILE);
        let renamed = root.join("custom-name.eap");
        fs::rename(&original, &renamed).unwrap();
        let mut reopened = NativeProject::open(&root).unwrap();
        reopened
            .create_source("src/alternate.eps", "function alternate() {}\n")
            .unwrap();
        reopened.set_main_file("src/alternate.eps").unwrap();
        assert!(!original.exists());
        let persisted: ProjectManifest =
            serde_json::from_slice(&fs::read(&renamed).unwrap()).unwrap();
        assert_eq!(persisted.main_file, "src/alternate.eps");
        drop(project);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn brackets_are_allowed_only_in_the_output_map() {
        assert_eq!(default_output_map("Arena", "scx"), "build/[EUD]Arena.scx");
        assert_eq!(
            normalize_output_map_relative("build/[EUD]Arena.scx").unwrap(),
            "build/[EUD]Arena.scx"
        );
        assert!(normalize_relative("build/[EUD]Arena.scx").is_err());

        let mut bracketed = manifest();
        bracketed.output_map = "build/[EUD]Arena.scx".to_string();
        validate_manifest(&bracketed).unwrap();

        let mut source = manifest();
        source.source_map = "maps/[EUD]source.scx".to_string();
        assert!(validate_manifest(&source).is_err());
        let mut main = manifest();
        main.main_file = "src/[main].eps".to_string();
        assert!(validate_manifest(&main).is_err());

        let (root, mut project) = project("bracketed-output");
        project
            .set_project_setting("SaveMapName", "build/[EUD]Arena.scx")
            .unwrap();
        assert!(project
            .set_project_setting("OpenMapName", "maps/[EUD]source.scx")
            .is_err());
        let output = project.output_map_path().unwrap();
        assert!(output.starts_with(project.root()));
        assert!(output.ends_with(Path::new("build").join("[EUD]Arena.scx")));
        // The EDS `[main]` value keeps the brackets verbatim and stays a value line.
        let eds = format!(
            "[main]\ninput: maps/source.scx\noutput: {}\n",
            project.manifest().output_map
        );
        assert!(eds
            .lines()
            .all(|line| !(line.starts_with('[') && line.ends_with(']')) || line == "[main]"));
        drop(project);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn manifest_rejects_eds_scalar_injection_and_invalid_entrypoint_matrix() {
        let mut injected = manifest();
        injected.settings.decode_unit_name = Some("True\n[src/hidden.py]".to_string());
        assert!(validate_manifest(&injected).is_err());

        let mut path_injected = manifest();
        path_injected.output_map = "build/output.scx\n[src/hidden.py]\n#keep.scx".to_string();
        assert!(validate_manifest(&path_injected).is_err());

        let (root, project) = project("invalid-python-entrypoints");
        project.create_source("src/bootstrap.py", "pass\n").unwrap();
        let manifest_path = root.join(PROJECT_MANIFEST_FILE);
        let base: ProjectManifest =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        for entrypoints in [
            vec![
                "src/bootstrap.py".to_string(),
                "src/BOOTSTRAP.py".to_string(),
            ],
            vec!["../outside.py".to_string()],
            vec!["src/missing.py".to_string()],
            vec!["src/BOOTSTRAP.py".to_string()],
            vec!["src/main.eps".to_string()],
        ] {
            let mut invalid = base.clone();
            invalid.python_entrypoints = entrypoints;
            write_json(&manifest_path, &invalid).unwrap();
            assert!(NativeProject::open(&root).is_err());
        }
        write_json(&manifest_path, &base).unwrap();
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn helper_only_python_change_updates_project_revision() {
        let (root, project) = project("python-helper-revision");
        project
            .create_source("src/helper.py", "VALUE = 1\n")
            .unwrap();
        let before = project.revision().unwrap();
        project
            .write_source("src/helper.py", "VALUE = 2\n")
            .unwrap();
        let after = project.revision().unwrap();
        assert_ne!(before, after);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn python_lock_identity_rejects_every_tampered_field() {
        let dependencies = vec!["demo==1.0".to_string()];
        let target = PythonLockTarget {
            os: PYTHON_LOCK_OS.to_string(),
            arch: PYTHON_LOCK_ARCH.to_string(),
            python_abi: "cp313".to_string(),
            euddraft_fingerprint: "a".repeat(64),
            uv_version: MANAGED_UV_VERSION.to_string(),
        };
        let packages = vec![PythonLockedPackage {
            name: "demo".to_string(),
            version: "1.0".to_string(),
            wheel_filename: "demo-1.0-py3-none-any.whl".to_string(),
            wheel_tags: vec!["py3-none-any".to_string()],
            artifact_url: "https://files.pythonhosted.org/packages/aa/demo-1.0-py3-none-any.whl"
                .to_string(),
            sha256: "b".repeat(64),
        }];
        let lock = PythonLock {
            digest: compute_python_lock_digest(&dependencies, &target, &packages).unwrap(),
            target,
            packages,
        };
        validate_python_lock(&dependencies, Some(&lock)).unwrap();
        let mut variants = Vec::new();
        let mut value = lock.clone();
        value.packages[0].wheel_filename = "demo-1.0-cp313-cp313-win_amd64.whl".to_string();
        variants.push(value);
        let mut value = lock.clone();
        value.packages[0].sha256 = "c".repeat(64);
        variants.push(value);
        let mut value = lock.clone();
        value.packages[0].artifact_url = "https://example.invalid/demo.whl".to_string();
        variants.push(value);
        let mut value = lock.clone();
        value.target.euddraft_fingerprint = "d".repeat(64);
        variants.push(value);
        let mut value = lock.clone();
        value.target.python_abi = "cp312".to_string();
        variants.push(value);
        let mut value = lock;
        value.digest = "e".repeat(64);
        variants.push(value);
        for tampered in variants {
            assert!(validate_python_lock(&dependencies, Some(&tampered)).is_err());
        }
    }

    #[test]
    fn python_sources_are_first_class_and_selected_entrypoints_are_guarded() {
        let (root, mut project) = project("python-source");
        project
            .create_source("src/bootstrap.py", "from eudplib import *\n")
            .unwrap();
        assert!(project.create_source("src/bad.txt", "x").is_err());
        assert!(project.set_main_file("src/bootstrap.py").is_err());

        let mut manifest: ProjectManifest =
            serde_json::from_slice(&fs::read(root.join(PROJECT_MANIFEST_FILE)).unwrap()).unwrap();
        manifest.python_entrypoints = vec!["src/bootstrap.py".to_string()];
        write_json(&root.join(PROJECT_MANIFEST_FILE), &manifest).unwrap();

        let mut reopened = NativeProject::open(&root).unwrap();
        assert!(reopened.has_direct_python().unwrap());
        assert!(reopened
            .source_snapshot()
            .unwrap()
            .files
            .iter()
            .any(|file| file.path == "src/bootstrap.py"));
        reopened
            .move_source("src/bootstrap.py", "src/runtime/bootstrap.py")
            .unwrap();
        assert_eq!(
            reopened.manifest().python_entrypoints,
            vec!["src/runtime/bootstrap.py"]
        );
        assert!(reopened.delete_source("src/runtime/bootstrap.py").is_err());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn exact_python_dependencies_normalize_and_reject_unsupported_forms() {
        assert_eq!(
            validate_python_dependencies(&[
                "Requests==2.032.0".to_string(),
                "typing_extensions==4.12.2".to_string(),
            ])
            .unwrap(),
            vec!["requests==2.32.0", "typing-extensions==4.12.2"]
        );
        for invalid in [
            "requests>=2",
            "requests",
            "requests[security]==2.32.0",
            "git+https://example.test/x",
        ] {
            assert!(
                validate_python_dependencies(&[invalid.to_string()]).is_err(),
                "accepted {invalid}"
            );
        }
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
