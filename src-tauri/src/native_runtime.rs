//! Configured native-project runtime shared by Tauri IPC, MCP tools, map services, and builds.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::Serialize;

use crate::config::DataDirs;
use crate::native_build::{run_native_build, DatCatalog, EuddraftLaunch, NativeBuildResult};
use crate::native_project::{
    DatScalar, DatTarget, NativeDatPatch, NativeProject, NativeProjectStatus, NativeSourceSnapshot,
    ProjectManifest,
};

pub const PROJECT_NOT_CONFIGURED: &str = "native project path not configured";
pub const EUDDRAFT_NOT_CONFIGURED: &str = "euddraft path not configured";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeFileInfo {
    pub path: String,
    pub file_type: String,
    pub settable: bool,
}

#[derive(Clone)]
pub struct NativeProjectManager {
    dirs: DataDirs,
    transaction: Arc<Mutex<()>>,
}

impl NativeProjectManager {
    pub fn new(dirs: DataDirs) -> Self {
        Self {
            dirs,
            transaction: Arc::new(Mutex::new(())),
        }
    }

    pub fn data_dirs(&self) -> &DataDirs {
        &self.dirs
    }

    pub fn project_root(&self) -> Result<PathBuf, String> {
        let config = self.dirs.load_config().map_err(|error| error.to_string())?;
        let configured = config.project_path.trim();
        if configured.is_empty() {
            return Err(PROJECT_NOT_CONFIGURED.to_string());
        }
        Ok(PathBuf::from(configured))
    }

    pub fn open(&self) -> Result<NativeProject, String> {
        NativeProject::open(&self.project_root()?)
    }

    pub fn configure_project(&self, root: &Path) -> Result<NativeProjectStatus, String> {
        let project = NativeProject::open(root)?;
        let mut config = self.dirs.load_config().map_err(|error| error.to_string())?;
        config.project_path = project.root().to_string_lossy().into_owned();
        self.dirs
            .save_config(&config)
            .map_err(|error| error.to_string())?;
        project.status()
    }

    pub fn create_project(
        &self,
        root: &Path,
        manifest: ProjectManifest,
    ) -> Result<NativeProjectStatus, String> {
        let _transaction = self.transaction.lock();
        let project = NativeProject::create(root, manifest)?;
        let mut config = self.dirs.load_config().map_err(|error| error.to_string())?;
        config.project_path = project.root().to_string_lossy().into_owned();
        self.dirs
            .save_config(&config)
            .map_err(|error| error.to_string())?;
        project.status()
    }

    pub fn status(&self) -> Result<NativeProjectStatus, String> {
        self.open()?.status()
    }

    pub fn source_snapshot(&self) -> Result<NativeSourceSnapshot, String> {
        self.open()?.source_snapshot()
    }

    pub fn list_files(&self) -> Result<Vec<NativeFileInfo>, String> {
        Ok(self
            .open()?
            .list_source_files()?
            .into_iter()
            .map(|path| NativeFileInfo {
                settable: path.to_lowercase().ends_with(".eps"),
                file_type: if path.to_lowercase().ends_with(".eps") {
                    "CUIEps".to_string()
                } else {
                    "RawText".to_string()
                },
                path,
            })
            .collect())
    }

    pub fn read_source(&self, path: &str) -> Result<String, String> {
        self.open()?.read_source(path)
    }

    pub fn write_source(&self, path: &str, content: &str) -> Result<(), String> {
        let _transaction = self.transaction.lock();
        self.open()?.write_source(path, content)
    }

    pub fn create_source(&self, path: &str, content: &str) -> Result<(), String> {
        let _transaction = self.transaction.lock();
        self.open()?.create_source(path, content)
    }

    pub fn create_source_dir(&self, path: &str) -> Result<(), String> {
        let _transaction = self.transaction.lock();
        self.open()?.create_source_dir(path)
    }

    pub fn move_source(&self, from: &str, to: &str) -> Result<(), String> {
        let _transaction = self.transaction.lock();
        self.open()?.move_source(from, to)
    }

    pub fn delete_source(&self, path: &str) -> Result<(), String> {
        let _transaction = self.transaction.lock();
        self.open()?.delete_source(path)
    }

    pub fn set_main_file(&self, path: &str) -> Result<(), String> {
        let _transaction = self.transaction.lock();
        self.open()?.set_main_file(path)
    }

    pub fn set_project_setting(&self, key: &str, value: &str) -> Result<(), String> {
        let _transaction = self.transaction.lock();
        self.open()?.set_project_setting(key, value)
    }

    pub fn plugin_add(&self, index: i64, raw_text: &str) -> Result<usize, String> {
        let _transaction = self.transaction.lock();
        self.open()?.plugin_add(index, raw_text)
    }

    pub fn plugin_edit(&self, index: usize, raw_text: &str) -> Result<(), String> {
        let _transaction = self.transaction.lock();
        self.open()?.plugin_edit(index, raw_text)
    }

    pub fn plugin_remove(&self, index: usize) -> Result<crate::native_project::EdsPlugin, String> {
        let _transaction = self.transaction.lock();
        self.open()?.plugin_remove(index)
    }

    pub fn plugin_move(&self, from: usize, to: usize) -> Result<(), String> {
        let _transaction = self.transaction.lock();
        self.open()?.plugin_move(from, to)
    }

    pub fn dat_values(
        &self,
        targets: &[DatTarget],
    ) -> Result<BTreeMap<DatTarget, DatScalar>, String> {
        let project = self.open()?;
        let catalog = DatCatalog::load(&self.dirs.native_assets_dir())?;
        let mut values = BTreeMap::new();
        for target in targets {
            let value = match project.current_dat_value(target) {
                Some(value) => value,
                None => baseline_value(&catalog, target)?,
            };
            values.insert(target.clone(), value);
        }
        Ok(values)
    }

    pub fn apply_dat_patch(&self, patch: &NativeDatPatch) -> Result<String, String> {
        let _transaction = self.transaction.lock();
        let mut project = self.open()?;
        let targets: Vec<_> = patch.changes.iter().map(|change| change.target()).collect();
        let catalog = DatCatalog::load(&self.dirs.native_assets_dir())?;
        let mut baseline = BTreeMap::new();
        for target in targets {
            if project.current_dat_value(&target).is_none() {
                baseline.insert(target.clone(), baseline_value(&catalog, &target)?);
            }
        }
        project
            .apply_dat_patch(patch, &baseline)
            .map(|result| result.revision)
    }

    pub fn build(&self) -> Result<NativeBuildResult, String> {
        let _transaction = self.transaction.lock();
        let project = self.open()?;
        let marker = project.root().join("build/.building");
        let _marker = BuildMarker::create(&marker)?;
        let config = self.dirs.load_config().map_err(|error| error.to_string())?;
        let configured = config.euddraft_path.trim();
        if configured.is_empty() {
            return Err(EUDDRAFT_NOT_CONFIGURED.to_string());
        }
        let launch = EuddraftLaunch::resolve(Path::new(configured))?;
        run_native_build(&project, &self.dirs.native_assets_dir(), &launch)
    }

    pub fn is_building(&self) -> bool {
        self.project_root()
            .ok()
            .is_some_and(|root| root.join("build/.building").is_file())
    }

    pub fn source_map_path(&self) -> Result<PathBuf, String> {
        self.open()?.source_map_path()
    }

    pub fn output_map_path(&self) -> Result<PathBuf, String> {
        self.open()?.output_map_path()
    }
}

struct BuildMarker {
    path: PathBuf,
}

impl BuildMarker {
    fn create(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|error| format!("another native build is active: {error}"))?;
        Ok(Self {
            path: path.to_path_buf(),
        })
    }
}

impl Drop for BuildMarker {
    fn drop(&mut self) {
        std::fs::remove_file(&self.path).ok();
    }
}

fn baseline_value(catalog: &DatCatalog, target: &DatTarget) -> Result<DatScalar, String> {
    match target {
        DatTarget::Dat {
            dat,
            object_id,
            field,
        } => catalog
            .numeric_value(dat, *object_id, field)
            .map(DatScalar::Number),
        DatTarget::Xdat {
            dat,
            object_id,
            field,
        } => catalog
            .xdat_value(dat, *object_id, field)
            .map(DatScalar::Number),
        DatTarget::Tbl(index) => catalog
            .tbl_value(*index)
            .map(|value| DatScalar::Text(value.to_string())),
        DatTarget::Requirement { dat, object_id } => catalog
            .requirement_payload(dat, *object_id)
            .map(DatScalar::Text),
        DatTarget::Button(set_id) => catalog.button_csv(*set_id).map(DatScalar::Text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::native_build::sync_compat_assets;
    use crate::native_project::{NativeDatChange, ProjectSettings};
    use std::fs;

    fn roots(tag: &str) -> (PathBuf, DataDirs) {
        let base = std::env::temp_dir().join(format!(
            "eud-agent-native-runtime-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        dirs.ensure_dirs().unwrap();
        sync_compat_assets(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/eud-editor-compat"),
            &dirs.native_assets_dir(),
        )
        .unwrap();
        (base, dirs)
    }

    fn manager(tag: &str) -> (PathBuf, NativeProjectManager) {
        let (base, dirs) = roots(tag);
        let project_root = base.join("project");
        fs::create_dir_all(project_root.join("maps")).unwrap();
        fs::write(project_root.join("maps/source.scx"), b"map").unwrap();
        let manager = NativeProjectManager::new(dirs.clone());
        manager
            .create_project(
                &project_root,
                ProjectManifest {
                    schema_version: 1,
                    name: "Runtime Demo".to_string(),
                    source_map: "maps/source.scx".to_string(),
                    output_map: "build/output.scx".to_string(),
                    main_file: "src/main.eps".to_string(),
                    settings: ProjectSettings::default(),
                    plugins: Vec::new(),
                    editor_compatibility: None,
                },
            )
            .unwrap();
        manager
            .write_source("src/main.eps", "function onPluginStart() {}\n")
            .unwrap();
        let mut config = dirs.load_config().unwrap_or_else(|_| Config::default());
        config.project_path = project_root.to_string_lossy().into_owned();
        dirs.save_config(&config).unwrap();
        (base, manager)
    }

    #[test]
    fn reads_stock_baseline_and_applies_one_batch_patch() {
        let (base, manager) = manager("dat");
        let target = DatTarget::Dat {
            dat: "units".to_string(),
            object_id: 15,
            field: "Hit Points".to_string(),
        };
        let values = manager.dat_values(std::slice::from_ref(&target)).unwrap();
        assert_eq!(values[&target], DatScalar::Number(10240));
        let revision = manager
            .apply_dat_patch(&NativeDatPatch {
                changes: vec![NativeDatChange::Dat {
                    dat: "units".to_string(),
                    object_id: 15,
                    field: "Hit Points".to_string(),
                    before: 10240,
                    after: 20480,
                }],
            })
            .unwrap();
        assert_eq!(revision.len(), 64);
        assert_eq!(
            manager.dat_values(std::slice::from_ref(&target)).unwrap()[&target],
            DatScalar::Number(20480)
        );
        fs::remove_dir_all(base).ok();
    }
}
