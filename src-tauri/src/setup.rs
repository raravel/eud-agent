//! First-run native-project, euddraft, model, and RAG setup surface.

use std::fs;
use std::path::Path;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use tauri_plugin_dialog::DialogExt;
use tokio::sync::Mutex;

use crate::bootstrap::{self, ProgressEmitter};
use crate::config::{Config, DataDirs};
use crate::ipc::AppManaged;

/// Process-wide bootstrap serialization lock.
///
/// Bootstrap has startup and panel-driven entry points. Both download each
/// asset to a fixed `<asset>.tmp` path, so overlap would race on one file.
/// Holding this lock for the whole run serializes them; the second entrant re-checks and
/// finds the asset already `Present`, so it no-ops instead of re-downloading.
fn bootstrap_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

pub const INVALID_PROJECT_FOLDER: &str = "invalid_project_folder";
pub const INVALID_EUDDRAFT_PATH: &str = "invalid_euddraft_path";
pub const PROJECT_CREATE_FAILED: &str = "project_create_failed";
pub const E3S_IMPORT_FAILED: &str = "e3s_import_failed";

/// `setup_status` and native picker command output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetupStatusResponse {
    pub project_path: String,
    pub project_valid: bool,
    pub euddraft_path: String,
    pub euddraft_valid: bool,
    pub assets_ready: bool,
    pub codex_resolved: bool,
    pub codex_authed: bool,
    pub setup_required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Build the setup/manifest snapshot for the panel (filesystem probe + hash +
/// codex login probe). Probes the ambient codex login state; see
/// [`status_from_config`] for the injectable form used by tests.
pub fn setup_status_payload(dirs: &DataDirs) -> Result<SetupStatusResponse, String> {
    let config = dirs.load_config().map_err(|error| error.to_string())?;
    let codex = crate::codex_auth::login_status();
    Ok(status_from_config(dirs, &config, &codex, None))
}

fn status_from_config(
    dirs: &DataDirs,
    config: &Config,
    codex: &crate::codex_auth::CodexAuthState,
    error: Option<String>,
) -> SetupStatusResponse {
    let project_path = config.project_path.trim().to_string();
    let project_valid = !project_path.is_empty()
        && crate::native_project::NativeProject::open(Path::new(&project_path)).is_ok();
    let euddraft_path = config.euddraft_path.trim().to_string();
    let euddraft_valid = !euddraft_path.is_empty()
        && crate::native_build::EuddraftLaunch::resolve(Path::new(&euddraft_path)).is_ok();
    let assets_ready = !bootstrap::needs_bootstrap(dirs, config);
    SetupStatusResponse {
        project_path,
        project_valid,
        euddraft_path,
        euddraft_valid,
        assets_ready,
        codex_resolved: codex.resolved,
        codex_authed: codex.authed,
        setup_required: !project_valid || !euddraft_valid || !assets_ready || !codex.authed,
        error,
    }
}

/// True when configured native prerequisites exist but a downloadable asset is stale.
pub fn should_auto_bootstrap(dirs: &DataDirs) -> bool {
    match dirs.load_config() {
        Ok(config) => {
            let project_valid = !config.project_path.trim().is_empty()
                && crate::native_project::NativeProject::open(Path::new(&config.project_path))
                    .is_ok();
            let euddraft_valid = !config.euddraft_path.trim().is_empty()
                && crate::native_build::EuddraftLaunch::resolve(Path::new(&config.euddraft_path))
                    .is_ok();
            project_valid && euddraft_valid && bootstrap::needs_bootstrap(dirs, &config)
        }
        Err(_) => false,
    }
}

/// Run the full bootstrap: resolve missing specs (release manifest / default model
/// id), download + verify + atomically place both assets, then persist the resolved
/// specs to `config.json`. Emits `progress {stage: bootstrap}` throughout; a failure
/// emits an `error: ...` detail (the setup screen renders it with retry) and is
/// returned to the caller.
pub async fn run_bootstrap<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    dirs: &DataDirs,
) -> anyhow::Result<()> {
    // Serialize concurrent bootstrap runs (auto + panel) so they never race on the
    // shared `<asset>.tmp` download path. Held for the whole run; the waiter then sees
    // the asset already placed and no-ops.
    let _guard = bootstrap_lock().lock().await;
    let emitter = bootstrap::TauriEmitter(app.clone());
    match run_bootstrap_inner(dirs, &emitter).await {
        Ok(()) => {
            emitter.emit("bootstrap", 100, "done");
            Ok(())
        }
        Err(error) => {
            emitter.emit("bootstrap", 0, &format!("error: {error:#}"));
            Err(error)
        }
    }
}

async fn run_bootstrap_inner(
    dirs: &DataDirs,
    emitter: &(dyn ProgressEmitter + Send + Sync),
) -> anyhow::Result<()> {
    let mut config = dirs.load_config()?;
    if config.model.name.trim().is_empty() {
        config.model.name = bootstrap::DEFAULT_MODEL_NAME.to_string();
    }
    // Re-fetch the release manifest whenever we are about to (re)download the RAG
    // index, not only when the pin is empty/wrong-version. A pinned sha256 goes
    // stale if the `rag-index.bin` under the same version tag was republished with
    // new content (the version field did not change), which would otherwise leave
    // existing installs verifying a freshly-downloaded binary against a dead hash
    // forever. When the asset is already Present on disk this branch is skipped, so
    // a healthy install pays no extra network cost.
    let rag_needs_download = bootstrap::asset_status(
        &dirs.rag_dir(),
        bootstrap::RAG_INDEX_FILENAME,
        &config.rag_index,
    )
    .needs_download();
    if config.rag_index.sha256.trim().is_empty()
        || config.rag_index.version != bootstrap::REQUIRED_RAG_INDEX_VERSION
        || rag_needs_download
    {
        emitter.emit("bootstrap", 0, "fetching release manifest");
        config.rag_index = bootstrap::fetch_release_manifest().await?;
    }
    bootstrap::bootstrap_assets(dirs, &config, emitter).await?;
    // Persist only after every asset is verified and placed, so an interrupted
    // install re-runs the manifest check from scratch on the next launch.
    dirs.save_config(&config)?;
    Ok(())
}

/// Report first-run native setup state.
#[tauri::command]
pub async fn setup_status(
    state: tauri::State<'_, AppManaged>,
) -> Result<SetupStatusResponse, String> {
    let dirs = state.dirs().clone();
    tauri::async_runtime::spawn_blocking(move || setup_status_payload(&dirs))
        .await
        .map_err(|error| error.to_string())?
}

/// Pick and configure an existing native project root.
#[tauri::command]
pub async fn setup_pick_project_path(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppManaged>,
) -> Result<SetupStatusResponse, String> {
    let dirs = state.dirs().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let Some(picked) = app.dialog().file().blocking_pick_folder() else {
            return setup_status_payload(&dirs);
        };
        let picked = picked.into_path().map_err(|error| error.to_string())?;
        let mut config = dirs.load_config().map_err(|error| error.to_string())?;
        let codex = crate::codex_auth::login_status();
        if crate::native_project::NativeProject::open(&picked).is_err() {
            return Ok(status_from_config(
                &dirs,
                &config,
                &codex,
                Some(INVALID_PROJECT_FOLDER.to_string()),
            ));
        }
        config.project_path = picked.to_string_lossy().into_owned();
        dirs.save_config(&config)
            .map_err(|error| error.to_string())?;
        Ok(status_from_config(&dirs, &config, &codex, None))
    })
    .await
    .map_err(|error| error.to_string())?
}
/// Create a canonical native project from a selected source map.
#[tauri::command]
pub async fn setup_create_project(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppManaged>,
) -> Result<SetupStatusResponse, String> {
    let dirs = state.dirs().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let Some(source) = app
            .dialog()
            .file()
            .add_filter("StarCraft maps", &["scx", "scm"])
            .blocking_pick_file()
        else {
            return setup_status_payload(&dirs);
        };
        let source = source.into_path().map_err(|error| error.to_string())?;
        let Some(destination) = app.dialog().file().blocking_pick_folder() else {
            return setup_status_payload(&dirs);
        };
        let destination = destination.into_path().map_err(|error| error.to_string())?;
        configure_created_project(&dirs, &source, &destination)
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Import a semantic legacy E3S projection into a new canonical native project.
#[tauri::command]
pub async fn setup_import_e3s(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppManaged>,
) -> Result<SetupStatusResponse, String> {
    let dirs = state.dirs().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let Some(source) = app
            .dialog()
            .file()
            .add_filter("E3S compatibility projects", &["e3s"])
            .blocking_pick_file()
        else {
            return setup_status_payload(&dirs);
        };
        let source = source.into_path().map_err(|error| error.to_string())?;
        let Some(destination) = app.dialog().file().blocking_pick_folder() else {
            return setup_status_payload(&dirs);
        };
        let destination = destination.into_path().map_err(|error| error.to_string())?;
        configure_imported_project(&dirs, &source, &destination)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct E3sExportResponse {
    pub path: String,
}

/// Export the configured imported project to an Editor-readable E3S.
#[tauri::command]
pub async fn project_export_e3s(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppManaged>,
) -> Result<Option<E3sExportResponse>, String> {
    let dirs = state.dirs().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let config = dirs.load_config().map_err(|error| error.to_string())?;
        let project = crate::native_project::NativeProject::open(Path::new(&config.project_path))?;
        let default_name = format!("{}.e3s", project.manifest().name);
        let Some(destination) = app
            .dialog()
            .file()
            .add_filter("E3S compatibility projects", &["e3s"])
            .set_file_name(default_name)
            .blocking_save_file()
        else {
            return Ok(None);
        };
        let destination = destination.into_path().map_err(|error| error.to_string())?;
        crate::e3s_nrbf::export_e3s(&project, &destination, &dirs.native_assets_dir())?;
        Ok(Some(E3sExportResponse {
            path: destination.to_string_lossy().into_owned(),
        }))
    })
    .await
    .map_err(|error| error.to_string())?
}

fn configure_created_project(
    dirs: &DataDirs,
    source_map: &Path,
    destination: &Path,
) -> Result<SetupStatusResponse, String> {
    let cleanable = destination_is_empty(destination)?;
    let result = create_project_from_map(source_map, destination);
    restore_empty_destination_after_failure(destination, cleanable, &result)?;
    finish_project_configuration(dirs, destination, result, PROJECT_CREATE_FAILED)
}

fn configure_imported_project(
    dirs: &DataDirs,
    source_e3s: &Path,
    destination: &Path,
) -> Result<SetupStatusResponse, String> {
    let cleanable = destination_is_empty(destination)?;
    let result =
        crate::e3s_nrbf::import_e3s(source_e3s, destination, &dirs.native_assets_dir()).map(|_| ());
    restore_empty_destination_after_failure(destination, cleanable, &result)?;
    finish_project_configuration(dirs, destination, result, E3S_IMPORT_FAILED)
}

fn finish_project_configuration(
    dirs: &DataDirs,
    destination: &Path,
    result: Result<(), String>,
    error_code: &str,
) -> Result<SetupStatusResponse, String> {
    let mut config = dirs.load_config().map_err(|error| error.to_string())?;
    let codex = crate::codex_auth::login_status();
    match result {
        Ok(()) => {
            config.project_path = destination.to_string_lossy().into_owned();
            dirs.save_config(&config)
                .map_err(|error| error.to_string())?;
            Ok(status_from_config(dirs, &config, &codex, None))
        }
        Err(error) => Ok(status_from_config(
            dirs,
            &config,
            &codex,
            Some(format!("{error_code}: {error}")),
        )),
    }
}

fn destination_is_empty(destination: &Path) -> Result<bool, String> {
    if !destination.exists() {
        return Ok(true);
    }
    Ok(fs::read_dir(destination)
        .map_err(|error| error.to_string())?
        .next()
        .is_none())
}

fn restore_empty_destination_after_failure(
    destination: &Path,
    cleanable: bool,
    result: &Result<(), String>,
) -> Result<(), String> {
    if result.is_ok() || !cleanable || !destination.exists() {
        return Ok(());
    }
    fs::remove_dir_all(destination).map_err(|error| error.to_string())?;
    fs::create_dir_all(destination).map_err(|error| error.to_string())
}

fn create_project_from_map(source_map: &Path, destination: &Path) -> Result<(), String> {
    if !source_map.is_file() {
        return Err(format!(
            "source map is unavailable: {}",
            source_map.display()
        ));
    }
    if destination.exists()
        && fs::read_dir(destination)
            .map_err(|error| error.to_string())?
            .next()
            .is_some()
    {
        return Err(format!(
            "project destination must be empty: {}",
            destination.display()
        ));
    }
    let source_name = source_map
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "source map filename is not Unicode".to_string())?;
    let name = source_map
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("Native EUD Project");
    let extension = source_map
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("scx");
    let manifest = crate::native_project::ProjectManifest {
        schema_version: crate::native_project::PROJECT_SCHEMA_VERSION,
        name: name.to_string(),
        source_map: format!("maps/{source_name}"),
        output_map: format!("build/{name}_EUD.{extension}"),
        main_file: "src/main.eps".to_string(),
        settings: crate::native_project::ProjectSettings::default(),
        plugins: Vec::new(),
        editor_compatibility: None,
    };
    let project = crate::native_project::NativeProject::create(destination, manifest)?;
    fs::copy(source_map, project.root().join("maps").join(source_name))
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Pick `euddraft.exe` or `euddraft.py`.
#[tauri::command]
pub async fn setup_pick_euddraft_path(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppManaged>,
) -> Result<SetupStatusResponse, String> {
    let dirs = state.dirs().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let Some(picked) = app.dialog().file().blocking_pick_file() else {
            return setup_status_payload(&dirs);
        };
        let picked = picked.into_path().map_err(|error| error.to_string())?;
        let mut config = dirs.load_config().map_err(|error| error.to_string())?;
        let codex = crate::codex_auth::login_status();
        if crate::native_build::EuddraftLaunch::resolve(&picked).is_err() {
            return Ok(status_from_config(
                &dirs,
                &config,
                &codex,
                Some(INVALID_EUDDRAFT_PATH.to_string()),
            ));
        }
        config.euddraft_path = picked.to_string_lossy().into_owned();
        dirs.save_config(&config)
            .map_err(|error| error.to_string())?;
        Ok(status_from_config(&dirs, &config, &codex, None))
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Run the first-run asset download (also the setup screen's retry action).
#[tauri::command]
pub async fn bootstrap_run(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppManaged>,
) -> Result<(), String> {
    let dirs = state.dirs().clone();
    run_bootstrap(&app, &dirs)
        .await
        .map_err(|error| format!("{error:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_project::{ProjectManifest, ProjectSettings};
    use std::fs;
    use std::path::PathBuf;

    fn unique_temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "eud-agent-setup-native-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn codex_authed() -> crate::codex_auth::CodexAuthState {
        crate::codex_auth::CodexAuthState {
            resolved: true,
            authed: true,
            detail: "logged in".to_string(),
        }
    }

    #[test]
    fn first_run_requires_native_project_and_euddraft() {
        let base = unique_temp_dir("first");
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        let status = status_from_config(&dirs, &Config::default(), &codex_authed(), None);
        assert!(!status.project_valid);
        assert!(!status.euddraft_valid);
        assert!(status.setup_required);
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn valid_native_paths_advance_past_path_setup() {
        let base = unique_temp_dir("valid");
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        dirs.ensure_dirs().unwrap();
        let project_root = base.join("project");
        fs::create_dir_all(project_root.join("maps")).unwrap();
        fs::write(project_root.join("maps/source.scx"), b"map").unwrap();
        crate::native_project::NativeProject::create(
            &project_root,
            ProjectManifest {
                schema_version: 1,
                name: "Demo".to_string(),
                source_map: "maps/source.scx".to_string(),
                output_map: "build/output.scx".to_string(),
                main_file: "src/main.eps".to_string(),
                settings: ProjectSettings::default(),
                plugins: Vec::new(),
                editor_compatibility: None,
            },
        )
        .unwrap();
        let euddraft = base.join("euddraft.exe");
        fs::write(&euddraft, b"stub").unwrap();
        let config = Config {
            project_path: project_root.to_string_lossy().into_owned(),
            euddraft_path: euddraft.to_string_lossy().into_owned(),
            ..Default::default()
        };
        let status = status_from_config(&dirs, &config, &codex_authed(), None);
        assert!(status.project_valid);
        assert!(status.euddraft_valid);
        assert!(
            status.setup_required,
            "downloadable assets are still absent"
        );
        fs::remove_dir_all(base).ok();
    }
    #[test]
    fn project_creation_copies_the_source_map_and_opens_canonical_state() {
        let base = unique_temp_dir("create");
        let source = base.join("input.scx");
        let destination = base.join("project");
        fs::write(&source, b"map").unwrap();

        create_project_from_map(&source, &destination).unwrap();

        let project = crate::native_project::NativeProject::open(&destination).unwrap();
        assert_eq!(project.manifest().source_map, "maps/input.scx");
        assert_eq!(project.manifest().main_file, "src/main.eps");
        assert_eq!(
            fs::read(project.source_map_path().unwrap()).unwrap(),
            b"map"
        );
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn project_creation_refuses_a_nonempty_destination() {
        let base = unique_temp_dir("create-nonempty");
        let source = base.join("input.scx");
        let destination = base.join("project");
        fs::write(&source, b"map").unwrap();
        fs::create_dir_all(&destination).unwrap();
        fs::write(destination.join("keep.txt"), b"user").unwrap();

        let error = create_project_from_map(&source, &destination).unwrap_err();

        assert!(error.contains("must be empty"));
        assert_eq!(fs::read(destination.join("keep.txt")).unwrap(), b"user");
        fs::remove_dir_all(base).ok();
    }
}
