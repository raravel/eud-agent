//! First-run native-project, euddraft, model, and RAG setup surface.

use std::fs;
use std::path::{Path, PathBuf};
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

/// Preserve installer progress while keeping each caller's UI surface isolated.
struct RetaggedProgressEmitter<'a> {
    inner: &'a (dyn ProgressEmitter + Send + Sync),
    stage: &'static str,
}

impl ProgressEmitter for RetaggedProgressEmitter<'_> {
    fn emit(&self, _stage: &str, pct: u8, detail: &str) {
        self.inner.emit(self.stage, pct, detail);
    }
}

pub const INVALID_PROJECT_FOLDER: &str = "invalid_project_folder";
pub const INVALID_EUDDRAFT_PATH: &str = "invalid_euddraft_path";
pub const PROJECT_CREATE_FAILED: &str = "project_create_failed";
pub const E3S_IMPORT_FAILED: &str = "e3s_import_failed";
pub const E3S_IMPORT_SOURCE_MAP_MISSING: &str = "e3s_import_source_map_missing";
pub const E3S_IMPORT_DESTINATION_NOT_EMPTY: &str = "e3s_import_destination_not_empty";
pub const E3S_IMPORT_UNSUPPORTED: &str = "e3s_import_unsupported";
pub const E3S_IMPORT_HARNESS_FAILED: &str = "e3s_import_harness_failed";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PathSelectionResponse {
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportDestinationSelectionResponse {
    pub path: String,
    pub empty: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct E3sImportRequest {
    pub source_e3s: String,
    pub destination: String,
    #[serde(default)]
    pub excluded_import_items: Vec<String>,
}

/// `setup_status` and native picker command output.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupStatusResponse {
    pub project_path: String,
    pub project_valid: bool,
    pub euddraft_path: String,
    pub euddraft_valid: bool,
    pub assets_ready: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_provider: Option<crate::provider::ProviderId>,
    pub providers: Vec<crate::provider::ProviderStatus>,
    pub setup_required: bool,
    /// True only when this response represents a successful explicit
    /// open/create/import action; ordinary status and cancellation are false.
    pub project_opened: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub import_issues: Vec<crate::harness_import::HarnessImportIssue>,
}

/// euddraft state shown in Settings → Compile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EuddraftSettingsResponse {
    pub path: String,
    pub valid: bool,
    pub managed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_version: Option<String>,
    pub update_available: bool,
}

pub fn euddraft_settings_payload(
    dirs: &DataDirs,
    latest_version: Option<String>,
) -> Result<EuddraftSettingsResponse, String> {
    let config = dirs.load_config().map_err(|error| error.to_string())?;
    let path = config.euddraft_path.trim().to_string();
    let valid =
        !path.is_empty() && crate::native_build::EuddraftLaunch::resolve(Path::new(&path)).is_ok();
    let installed_version = valid
        .then(|| bootstrap::managed_euddraft_version(dirs, Path::new(&path)))
        .flatten();
    let managed = installed_version.is_some();
    let update_available = installed_version
        .as_deref()
        .zip(latest_version.as_deref())
        .is_some_and(|(installed, latest)| installed != latest);
    Ok(EuddraftSettingsResponse {
        path,
        valid,
        managed,
        installed_version,
        latest_version,
        update_available,
    })
}

/// Build the setup/manifest snapshot for the panel (filesystem probe + hash +
/// codex login probe). Probes the ambient codex login state; see
/// [`status_from_config`] for the injectable form used by tests.
pub fn setup_status_payload(
    dirs: &DataDirs,
    providers: Vec<crate::provider::ProviderStatus>,
) -> Result<SetupStatusResponse, String> {
    let config = dirs.load_config().map_err(|error| error.to_string())?;
    Ok(status_from_config(dirs, &config, providers, None, false))
}

fn status_from_config(
    dirs: &DataDirs,
    config: &Config,
    providers: Vec<crate::provider::ProviderStatus>,
    error: Option<String>,
    project_opened: bool,
) -> SetupStatusResponse {
    let project_path = config.project_path.trim().to_string();
    let project_valid = !project_path.is_empty()
        && crate::native_project::NativeProject::open(Path::new(&project_path)).is_ok();
    let euddraft_path = config.euddraft_path.trim().to_string();
    let euddraft_valid = !euddraft_path.is_empty()
        && crate::native_build::EuddraftLaunch::resolve(Path::new(&euddraft_path)).is_ok();
    let assets_ready = !bootstrap::needs_bootstrap(dirs, config);
    let selected_ready = config.default_provider.is_some_and(|selected| {
        providers
            .iter()
            .find(|status| status.provider == selected)
            .is_some_and(|status| status.availability.is_ready())
            && config.providers.default_model(selected).is_some()
    });
    SetupStatusResponse {
        project_path,
        project_valid,
        euddraft_path,
        euddraft_valid,
        assets_ready,
        default_provider: config.default_provider,
        providers,
        setup_required: !project_valid || !euddraft_valid || !assets_ready || !selected_ready,
        project_opened,
        error,
        import_issues: Vec::new(),
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
    if config.euddraft_path.trim().is_empty() {
        let installed = bootstrap::ensure_euddraft(dirs, emitter).await?;
        // A folder/file picked during the download takes precedence over the default.
        persist_euddraft_path(dirs, &installed, false)?;
        emitter.emit("bootstrap", 100, "euddraft configured");
        config = dirs.load_config()?;
    }
    crate::native_build::EuddraftLaunch::resolve(Path::new(config.euddraft_path.trim()))
        .map_err(anyhow::Error::msg)?;
    bootstrap::ensure_managed_uv(dirs, emitter).await?;
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
    // Preserve project/provider/path changes made while the assets were downloading.
    let mut current = dirs.load_config()?;
    current.model = config.model;
    current.rag_index = config.rag_index;
    dirs.save_config(&current)?;
    Ok(())
}

fn persist_euddraft_path(dirs: &DataDirs, path: &Path, replace: bool) -> anyhow::Result<()> {
    crate::native_build::EuddraftLaunch::resolve(path).map_err(anyhow::Error::msg)?;
    let mut config = dirs.load_config()?;
    if replace || config.euddraft_path.trim().is_empty() {
        config.euddraft_path = path.to_string_lossy().into_owned();
        dirs.save_config(&config)?;
    }
    Ok(())
}

/// Report first-run native setup state.
#[tauri::command]
pub async fn setup_status(
    state: tauri::State<'_, AppManaged>,
    providers: tauri::State<'_, crate::provider_service::ProviderService>,
) -> Result<SetupStatusResponse, String> {
    let dirs = state.dirs().clone();
    let statuses = providers.status_list().await?;
    tauri::async_runtime::spawn_blocking(move || setup_status_payload(&dirs, statuses))
        .await
        .map_err(|error| error.to_string())?
}

/// Read configured euddraft path and managed-install version without network access.
#[tauri::command]
pub async fn euddraft_settings(
    state: tauri::State<'_, AppManaged>,
) -> Result<EuddraftSettingsResponse, String> {
    let dirs = state.dirs().clone();
    tauri::async_runtime::spawn_blocking(move || euddraft_settings_payload(&dirs, None))
        .await
        .map_err(|error| error.to_string())?
}

/// Compare the configured managed euddraft against GitHub's latest official release.
#[tauri::command]
pub async fn euddraft_check_update(
    state: tauri::State<'_, AppManaged>,
) -> Result<EuddraftSettingsResponse, String> {
    let latest = bootstrap::latest_euddraft_version()
        .await
        .map_err(|error| format!("{error:#}"))?;
    let dirs = state.dirs().clone();
    tauri::async_runtime::spawn_blocking(move || euddraft_settings_payload(&dirs, Some(latest)))
        .await
        .map_err(|error| error.to_string())?
}

/// Pick and configure an existing native project root, canonical `.eap` manifest, or legacy import.
#[tauri::command]
pub(crate) async fn setup_pick_project_path(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppManaged>,
    providers: tauri::State<'_, crate::provider_service::ProviderService>,
    engine: tauri::State<'_, crate::engine::SessionEngineManager>,
    directory: Option<bool>,
) -> Result<SetupStatusResponse, String> {
    let dirs = state.dirs().clone();
    let statuses = providers.status_list().await?;
    let manager = crate::native_runtime::NativeProjectManager::new(dirs.clone());
    let switch_guard = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let dialog = app.dialog().file();
        let picked = if directory.unwrap_or(false) {
            dialog.blocking_pick_folder()
        } else {
            dialog
                .add_filter("EUD Agent 프로젝트 (.eap)", &["eap"])
                .add_filter(
                    "이전 프로젝트 변환 (project.json / .eudproj)",
                    &["json", "eudproj"],
                )
                .blocking_pick_file()
        };
        let Some(picked) = picked else {
            return setup_status_payload(&dirs, statuses);
        };
        let picked = picked.into_path().map_err(|error| error.to_string())?;
        let result = switch_guard.with_project_switch(|| manager.configure_project(&picked));
        match result {
            Ok(import_issues) => {
                let config = dirs.load_config().map_err(|error| error.to_string())?;
                let mut status = status_from_config(&dirs, &config, statuses, None, true);
                status.import_issues = import_issues;
                Ok(status)
            }
            Err(error) => {
                let config = dirs.load_config().map_err(|error| error.to_string())?;
                Ok(status_from_config(
                    &dirs,
                    &config,
                    statuses,
                    Some(format!("{INVALID_PROJECT_FOLDER}: {error}")),
                    false,
                ))
            }
        }
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Create a canonical native project from a selected source map.
#[tauri::command]
pub(crate) async fn setup_create_project(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppManaged>,
    providers: tauri::State<'_, crate::provider_service::ProviderService>,
    engine: tauri::State<'_, crate::engine::SessionEngineManager>,
) -> Result<SetupStatusResponse, String> {
    let dirs = state.dirs().clone();
    let statuses = providers.status_list().await?;
    let switch_guard = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let Some(source) = app
            .dialog()
            .file()
            .add_filter("StarCraft maps", &["scx", "scm"])
            .blocking_pick_file()
        else {
            return setup_status_payload(&dirs, statuses);
        };
        let source = source.into_path().map_err(|error| error.to_string())?;
        let Some(destination) = app.dialog().file().blocking_pick_folder() else {
            return setup_status_payload(&dirs, statuses);
        };
        let destination = destination.into_path().map_err(|error| error.to_string())?;
        configure_created_project(&dirs, &source, &destination, statuses, &switch_guard)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn setup_pick_e3s_source(
    app: tauri::AppHandle,
) -> Result<Option<PathSelectionResponse>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let Some(source) = app
            .dialog()
            .file()
            .add_filter("E3S compatibility projects", &["e3s"])
            .blocking_pick_file()
        else {
            return Ok(None);
        };
        let source = source.into_path().map_err(|error| error.to_string())?;
        Ok(Some(PathSelectionResponse {
            path: source.to_string_lossy().into_owned(),
        }))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn setup_pick_import_destination(
    app: tauri::AppHandle,
) -> Result<Option<ImportDestinationSelectionResponse>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let Some(destination) = app.dialog().file().blocking_pick_folder() else {
            return Ok(None);
        };
        let destination = destination.into_path().map_err(|error| error.to_string())?;
        Ok(Some(ImportDestinationSelectionResponse {
            empty: destination_is_empty(&destination)?,
            path: destination.to_string_lossy().into_owned(),
        }))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn setup_import_e3s(
    state: tauri::State<'_, AppManaged>,
    providers: tauri::State<'_, crate::provider_service::ProviderService>,
    engine: tauri::State<'_, crate::engine::SessionEngineManager>,
    request: E3sImportRequest,
) -> Result<SetupStatusResponse, String> {
    let dirs = state.dirs().clone();
    let statuses = providers.status_list().await?;
    let switch_guard = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let source = PathBuf::from(request.source_e3s);
        let destination = PathBuf::from(request.destination);
        configure_imported_project(
            &dirs,
            &source,
            &destination,
            &request.excluded_import_items,
            statuses,
            &switch_guard,
        )
    })
    .await
    .map_err(|error| error.to_string())?
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentProject {
    pub name: String,
    pub path: String,
    pub last_opened_at: u64,
    pub available: bool,
}

fn now_unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().try_into().unwrap_or(u64::MAX)
        })
}

fn recent_projects_payload(dirs: &DataDirs) -> Result<Vec<RecentProject>, String> {
    let config_guard = crate::config::PROJECT_CONFIG_LOCK.lock();
    let mut config = dirs.load_config().map_err(|error| error.to_string())?;
    let mut changed = config.normalize_recent_projects();
    if !config.project_recents_initialized {
        config.project_recents_initialized = true;
        if config.project_recents.is_empty() && !config.project_path.trim().is_empty() {
            let configured = PathBuf::from(config.project_path.trim());
            let canonical = fs::canonicalize(&configured).unwrap_or(configured);
            let name = crate::native_project::NativeProject::open(&canonical)
                .map(|project| project.manifest().name.clone())
                .or_else(|_| {
                    canonical
                        .file_name()
                        .and_then(|value| value.to_str())
                        .map(str::to_string)
                        .ok_or_else(|| {
                            "configured project path filename is not Unicode".to_string()
                        })
                })?;
            config.record_recent_project(
                name,
                canonical.to_string_lossy().into_owned(),
                now_unix_millis(),
            );
        }
        changed = true;
    }
    if changed {
        dirs.save_config(&config)
            .map_err(|error| error.to_string())?;
    }
    drop(config_guard);
    Ok(config
        .project_recents
        .iter()
        .map(|entry| RecentProject {
            name: entry.name.clone(),
            path: entry.path.clone(),
            last_opened_at: entry.last_opened_at,
            available: crate::native_project::NativeProject::open(Path::new(&entry.path)).is_ok(),
        })
        .collect())
}

/// Open a project from a directory, a canonical `.eap` manifest, or an explicit legacy import.
#[tauri::command]
pub(crate) async fn project_open(
    state: tauri::State<'_, AppManaged>,
    providers: tauri::State<'_, crate::provider_service::ProviderService>,
    engine: tauri::State<'_, crate::engine::SessionEngineManager>,
    path: String,
) -> Result<SetupStatusResponse, String> {
    let dirs = state.dirs().clone();
    let statuses = providers.status_list().await?;
    let manager = crate::native_runtime::NativeProjectManager::new(dirs.clone());
    let switch_guard = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let result =
            switch_guard.with_project_switch(|| manager.configure_project(Path::new(&path)));
        match result {
            Ok(import_issues) => {
                let config = dirs.load_config().map_err(|error| error.to_string())?;
                let mut status = status_from_config(&dirs, &config, statuses, None, true);
                status.import_issues = import_issues;
                Ok(status)
            }
            Err(error) => {
                let config = dirs.load_config().map_err(|error| error.to_string())?;
                Ok(status_from_config(
                    &dirs,
                    &config,
                    statuses,
                    Some(format!("{INVALID_PROJECT_FOLDER}: {error}")),
                    false,
                ))
            }
        }
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn project_recent_list(
    state: tauri::State<'_, AppManaged>,
) -> Result<Vec<RecentProject>, String> {
    let dirs = state.dirs().clone();
    tauri::async_runtime::spawn_blocking(move || recent_projects_payload(&dirs))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn project_recent_remove(
    state: tauri::State<'_, AppManaged>,
    path: String,
) -> Result<Vec<RecentProject>, String> {
    let dirs = state.dirs().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let config_guard = crate::config::PROJECT_CONFIG_LOCK.lock();
        let mut config = dirs.load_config().map_err(|error| error.to_string())?;
        config.remove_recent_project(&path);
        dirs.save_config(&config)
            .map_err(|error| error.to_string())?;
        drop(config_guard);
        recent_projects_payload(&dirs)
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
    providers: Vec<crate::provider::ProviderStatus>,
    switch_guard: &crate::engine::SessionEngineManager,
) -> Result<SetupStatusResponse, String> {
    let result = switch_guard.with_project_switch(|| {
        let cleanable = destination_is_empty(destination)?;
        let result = create_project_from_map(source_map, destination);
        let result = if result.is_ok() {
            crate::native_project::NativeProject::open(destination).and_then(|project| {
                crate::native_runtime::NativeProjectManager::new(dirs.clone())
                    .activate_project(&project)
            })
        } else {
            result
        };
        restore_empty_destination_after_failure(destination, cleanable, &result)?;
        result
    });
    finish_project_configuration(dirs, result, PROJECT_CREATE_FAILED, providers)
}

fn configure_imported_project(
    dirs: &DataDirs,
    source_e3s: &Path,
    destination: &Path,
    excluded_import_items: &[String],
    providers: Vec<crate::provider::ProviderStatus>,
    switch_guard: &crate::engine::SessionEngineManager,
) -> Result<SetupStatusResponse, String> {
    let result = switch_guard.with_project_switch(|| {
        import_project_from_e3s(dirs, source_e3s, destination, excluded_import_items)
    });
    match result {
        Ok(ImportProjectOutcome::NeedsApproval { import_issues }) => {
            let config = dirs.load_config().map_err(|error| error.to_string())?;
            let mut status = status_from_config(dirs, &config, providers, None, false);
            status.import_issues = import_issues;
            Ok(status)
        }
        Ok(ImportProjectOutcome::Completed) => {
            finish_project_configuration(dirs, Ok(()), E3S_IMPORT_FAILED, providers)
        }
        Err(error) => {
            let error_code = e3s_import_error_code(&error);
            finish_project_configuration(dirs, Err(error), error_code, providers)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ImportProjectOutcome {
    Completed,
    NeedsApproval {
        import_issues: Vec<crate::harness_import::HarnessImportIssue>,
    },
}

fn import_project_from_e3s(
    dirs: &DataDirs,
    source_e3s: &Path,
    destination: &Path,
    excluded_import_items: &[String],
) -> Result<ImportProjectOutcome, String> {
    let cleanable = destination_is_empty(destination)?;
    let result = crate::e3s_nrbf::import_e3s(source_e3s, destination, &dirs.native_assets_dir())
        .map_err(|error| {
            format!(
                "cannot import E3S '{}' into '{}': {error}",
                source_e3s.display(),
                destination.display()
            )
        })
        .and_then(|project| {
            configure_project_with_legacy_harness(dirs, source_e3s, &project, excluded_import_items)
        });
    let cleanup_result = match &result {
        Ok(ImportProjectOutcome::Completed) => Ok(()),
        Ok(ImportProjectOutcome::NeedsApproval { .. }) => {
            Err("부가 자료 제외 확인을 위해 가져오기 대상 정리가 필요합니다.".to_string())
        }
        Err(error) => Err(error.clone()),
    };
    restore_empty_destination_after_failure(destination, cleanable, &cleanup_result)?;
    result
}

fn configure_project_with_legacy_harness(
    dirs: &DataDirs,
    source_e3s: &Path,
    project: &crate::native_project::NativeProject,
    excluded_import_items: &[String],
) -> Result<ImportProjectOutcome, String> {
    let source_path = fs::canonicalize(source_e3s).map_err(|error| error.to_string())?;
    #[cfg(windows)]
    let source_path = dunce::simplified(&source_path);
    let source_name = source_path.to_string_lossy();
    let mut import_issues = Vec::new();
    let workspace = crate::workspace::WorkspaceManager::new(dirs.clone())
        .import_legacy_harness(source_e3s, project, &mut import_issues)
        .map_err(|error| format!("legacy harness import failed: {error}"))?;
    let memory = match crate::memory::ProjectMemory::import_legacy_harness(
        &dirs.memory_dir(),
        workspace.source_project().unwrap_or(&source_name),
        project,
        &mut import_issues,
    ) {
        Ok(memory) => memory,
        Err(error) => {
            let mut message = format!("legacy harness import failed: {error}");
            if let Err(rollback) = workspace.rollback() {
                message.push_str(&format!("; workspace rollback failed: {rollback}"));
            }
            return Err(message);
        }
    };
    let sessions = match crate::session::SessionStore::import_legacy_harness(
        dirs,
        source_e3s,
        workspace.source_project().unwrap_or(&source_name),
        project,
        &mut import_issues,
    ) {
        Ok(sessions) => sessions,
        Err(error) => {
            let mut message = format!("legacy harness import failed: {error}");
            if let Err(rollback) = memory.rollback() {
                message.push_str(&format!("; memory rollback failed: {rollback}"));
            }
            if let Err(rollback) = workspace.rollback() {
                message.push_str(&format!("; workspace rollback failed: {rollback}"));
            }
            return Err(message);
        }
    };
    import_issues.sort_by(|left, right| {
        (&left.scope, &left.path, &left.id).cmp(&(&right.scope, &right.path, &right.id))
    });
    import_issues.dedup_by(|left, right| left.id == right.id);
    let needs_review = import_issues
        .iter()
        .any(|issue| !excluded_import_items.contains(&issue.id));
    let result = if needs_review {
        Ok(ImportProjectOutcome::NeedsApproval { import_issues })
    } else {
        crate::native_runtime::NativeProjectManager::new(dirs.clone())
            .activate_project(project)
            .map(|_| ImportProjectOutcome::Completed)
    };
    if matches!(&result, Ok(ImportProjectOutcome::Completed)) {
        sessions.commit();
        memory.commit();
        workspace.commit();
        return result;
    }
    let mut rollback_errors = Vec::new();
    if let Err(error) = sessions.rollback() {
        rollback_errors.push(format!("session rollback failed: {error}"));
    }
    if let Err(error) = memory.rollback() {
        rollback_errors.push(format!("memory rollback failed: {error}"));
    }
    if let Err(error) = workspace.rollback() {
        rollback_errors.push(format!("workspace rollback failed: {error}"));
    }
    if !rollback_errors.is_empty() {
        return Err(format!(
            "legacy harness import failed: {}; {}",
            result
                .err()
                .unwrap_or_else(|| "부가 자료 제외 확인 준비".to_string()),
            rollback_errors.join("; ")
        ));
    }
    result
}

fn e3s_import_error_code(error: &str) -> &'static str {
    if error.starts_with("legacy harness import failed:") {
        E3S_IMPORT_HARNESS_FAILED
    } else if error.contains("referenced source map is missing") {
        E3S_IMPORT_SOURCE_MAP_MISSING
    } else if error.contains("destination must be empty") {
        E3S_IMPORT_DESTINATION_NOT_EMPTY
    } else if error.contains("unsupported")
        || error.contains("GUIEps")
        || error.contains("GUIPy")
        || error.contains("RawText")
        || error.contains("ClassicTrigger")
    {
        E3S_IMPORT_UNSUPPORTED
    } else {
        E3S_IMPORT_FAILED
    }
}

fn finish_project_configuration(
    dirs: &DataDirs,
    result: Result<(), String>,
    error_code: &str,
    providers: Vec<crate::provider::ProviderStatus>,
) -> Result<SetupStatusResponse, String> {
    let config = dirs.load_config().map_err(|error| error.to_string())?;
    match result {
        Ok(()) => Ok(status_from_config(dirs, &config, providers, None, true)),
        Err(error) => Ok(status_from_config(
            dirs,
            &config,
            providers,
            Some(format!("{error_code}: {error}")),
            false,
        )),
    }
}

fn destination_is_empty(destination: &Path) -> Result<bool, String> {
    if !destination.exists() {
        return Ok(true);
    }
    Ok(fs::read_dir(destination)
        .map_err(|error| {
            format!(
                "cannot inspect destination '{}': {error}",
                destination.display()
            )
        })?
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
    // File pickers and other processes may retain a handle to the chosen folder
    // without FILE_SHARE_DELETE. Keep that folder; only remove import output.
    let cleanup = (|| -> Result<(), String> {
        let entries = fs::read_dir(destination).map_err(|error| {
            format!(
                "cannot list import destination '{}': {error}",
                destination.display()
            )
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                format!(
                    "cannot read import destination '{}': {error}",
                    destination.display()
                )
            })?;
            let path = entry.path();
            let cleanup_error =
                |error| format!("cannot remove imported path '{}': {error}", path.display());
            if entry.file_type().map_err(cleanup_error)?.is_dir() {
                fs::remove_dir_all(&path).map_err(cleanup_error)?;
            } else {
                fs::remove_file(&path).map_err(cleanup_error)?;
            }
        }
        Ok(())
    })();
    cleanup.map_err(|error| {
        format!(
            "{}; import rollback failed: {error}",
            result.as_ref().unwrap_err()
        )
    })
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
        python_entrypoints: Vec::new(),
        python_dependencies: Vec::new(),
        python_lock: None,
        editor_compatibility: None,
    };
    let project = crate::native_project::NativeProject::create(destination, manifest)?;
    fs::copy(source_map, project.root().join("maps").join(source_name))
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Pick an existing euddraft distribution folder or entrypoint.
#[tauri::command]
pub async fn setup_pick_euddraft_path(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppManaged>,
    providers: tauri::State<'_, crate::provider_service::ProviderService>,
    directory: Option<bool>,
) -> Result<SetupStatusResponse, String> {
    let dirs = state.dirs().clone();
    let statuses = providers.status_list().await?;
    tauri::async_runtime::spawn_blocking(move || {
        let dialog = app.dialog().file();
        let picked = if directory.unwrap_or(false) {
            dialog
                .set_title("euddraft 폴더 선택")
                .blocking_pick_folder()
        } else {
            dialog
                .set_title("euddraft 실행 파일 선택")
                .add_filter("euddraft", &["exe", "py"])
                .blocking_pick_file()
        };
        let Some(picked) = picked else {
            return setup_status_payload(&dirs, statuses);
        };
        let picked = picked.into_path().map_err(|error| error.to_string())?;
        let mut config = dirs.load_config().map_err(|error| error.to_string())?;
        if crate::native_build::EuddraftLaunch::resolve(&picked).is_err() {
            return Ok(status_from_config(
                &dirs,
                &config,
                statuses,
                Some(INVALID_EUDDRAFT_PATH.to_string()),
                false,
            ));
        }
        config.euddraft_path = picked.to_string_lossy().into_owned();
        dirs.save_config(&config)
            .map_err(|error| error.to_string())?;
        Ok(status_from_config(&dirs, &config, statuses, None, false))
    })
    .await
    .map_err(|error| error.to_string())?
}

async fn install_latest_euddraft(
    dirs: &DataDirs,
    emitter: &(dyn ProgressEmitter + Send + Sync),
) -> Result<(), String> {
    let _guard = bootstrap_lock().lock().await;
    let result = async {
        let installed = bootstrap::ensure_euddraft(dirs, emitter).await?;
        persist_euddraft_path(dirs, &installed, true)?;
        emitter.emit("bootstrap", 100, "euddraft configured");
        anyhow::Ok(())
    }
    .await;
    if let Err(error) = result {
        emitter.emit("bootstrap", 0, &format!("error: {error:#}"));
        return Err(format!("{error:#}"));
    }
    Ok(())
}

/// Install GitHub's latest official release and select the managed executable.
#[tauri::command]
pub async fn euddraft_update(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppManaged>,
) -> Result<EuddraftSettingsResponse, String> {
    let dirs = state.dirs().clone();
    let native_emitter = bootstrap::TauriEmitter(app);
    let update_emitter = RetaggedProgressEmitter {
        inner: &native_emitter,
        stage: "euddraft_update",
    };
    install_latest_euddraft(&dirs, &update_emitter).await?;
    tauri::async_runtime::spawn_blocking(move || {
        let mut response = euddraft_settings_payload(&dirs, None)?;
        response.latest_version = response.installed_version.clone();
        response.update_available = false;
        Ok(response)
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Explicitly select the latest managed release instead of a configured local path.
#[tauri::command]
pub async fn setup_install_euddraft(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppManaged>,
    providers: tauri::State<'_, crate::provider_service::ProviderService>,
) -> Result<SetupStatusResponse, String> {
    let dirs = state.dirs().clone();
    let emitter = bootstrap::TauriEmitter(app);
    install_latest_euddraft(&dirs, &emitter).await?;
    let statuses = providers.status_list().await?;
    tauri::async_runtime::spawn_blocking(move || setup_status_payload(&dirs, statuses))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn setup_provider_select(
    state: tauri::State<'_, AppManaged>,
    providers: tauri::State<'_, crate::provider_service::ProviderService>,
    provider: crate::provider::ProviderId,
) -> Result<SetupStatusResponse, String> {
    let dirs = state.dirs().clone();
    let mut config = dirs.load_config().map_err(|error| error.to_string())?;
    let mut statuses = providers.status_list().await?;
    let current_ready = config.default_provider.is_some_and(|current| {
        statuses
            .iter()
            .find(|status| status.provider == current)
            .is_some_and(|status| status.availability.is_ready())
    });
    let target_ready = statuses
        .iter()
        .find(|status| status.provider == provider)
        .is_some_and(|status| status.availability.is_ready());
    let core_setup_incomplete = config.project_path.trim().is_empty()
        || crate::native_project::NativeProject::open(Path::new(config.project_path.trim()))
            .is_err()
        || config.euddraft_path.trim().is_empty()
        || crate::native_build::EuddraftLaunch::resolve(Path::new(config.euddraft_path.trim()))
            .is_err()
        || bootstrap::needs_bootstrap(&dirs, &config);
    if current_ready && !core_setup_incomplete && !target_ready {
        return Err("provider_not_authenticated".to_string());
    }
    config.default_provider = Some(provider);
    dirs.save_config(&config)
        .map_err(|error| error.to_string())?;
    for status in &mut statuses {
        status.selected_as_default = status.provider == provider;
    }
    tauri::async_runtime::spawn_blocking(move || setup_status_payload(&dirs, statuses))
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
    use crate::bootstrap::RAG_INDEX_FILENAME;
    use crate::native_project::{ProjectManifest, ProjectSettings};
    use std::fs;
    use std::path::PathBuf;

    const HELLO_SHA: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

    #[test]
    fn settings_update_progress_is_not_reported_as_bootstrap() {
        #[derive(Default)]
        struct RecordingEmitter(parking_lot::Mutex<Vec<(String, u8, String)>>);

        impl ProgressEmitter for RecordingEmitter {
            fn emit(&self, stage: &str, pct: u8, detail: &str) {
                self.0
                    .lock()
                    .push((stage.to_string(), pct, detail.to_string()));
            }
        }

        let recording = RecordingEmitter::default();
        let update = RetaggedProgressEmitter {
            inner: &recording,
            stage: "euddraft_update",
        };
        update.emit("bootstrap", 42, "downloading euddraft");

        assert_eq!(
            *recording.0.lock(),
            vec![(
                "euddraft_update".to_string(),
                42,
                "downloading euddraft".to_string(),
            )]
        );
    }

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

    fn provider_statuses(
        selected: Option<crate::provider::ProviderId>,
        availability: crate::provider::ProviderAvailability,
    ) -> Vec<crate::provider::ProviderStatus> {
        crate::provider::ProviderId::ALL
            .into_iter()
            .map(|provider| crate::provider::ProviderStatus {
                provider,
                availability: if Some(provider) == selected {
                    availability
                } else {
                    crate::provider::ProviderAvailability::Unavailable
                },
                selected_as_default: Some(provider) == selected,
                can_install: matches!(
                    provider,
                    crate::provider::ProviderId::Codex | crate::provider::ProviderId::ClaudeCode
                ),
                can_import: false,
                experimental: provider == crate::provider::ProviderId::Antigravity,
                detail_code: None,
            })
            .collect()
    }

    fn place_rag_asset(dirs: &DataDirs) -> crate::config::AssetSpec {
        fs::create_dir_all(dirs.rag_dir()).unwrap();
        fs::write(dirs.rag_dir().join(RAG_INDEX_FILENAME), b"hello").unwrap();
        let uv_dir = dirs.managed_uv_dir(crate::bootstrap::MANAGED_UV_VERSION);
        fs::create_dir_all(&uv_dir).unwrap();
        fs::write(uv_dir.join("uv.exe"), b"hello").unwrap();
        fs::write(
            uv_dir.join(".uv-install.json"),
            serde_json::to_vec(&serde_json::json!({
                "version": crate::bootstrap::MANAGED_UV_VERSION,
                "archive_sha256": crate::bootstrap::MANAGED_UV_SHA256,
                "executable": "uv.exe",
                "files": [{
                    "path": "uv.exe",
                    "sha256": HELLO_SHA,
                    "bytes": 5
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        crate::config::AssetSpec {
            name: "https://example.com/rag-index.bin".to_string(),
            sha256: HELLO_SHA.to_string(),
            version: bootstrap::REQUIRED_RAG_INDEX_VERSION.to_string(),
        }
    }

    #[test]
    fn first_run_requires_native_project_and_euddraft() {
        let base = unique_temp_dir("first");
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        let status = status_from_config(
            &dirs,
            &Config::default(),
            provider_statuses(None, crate::provider::ProviderAvailability::Unavailable),
            None,
            false,
        );
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
                schema_version: crate::native_project::PROJECT_SCHEMA_VERSION,
                name: "Demo".to_string(),
                source_map: "maps/source.scx".to_string(),
                output_map: "build/output.scx".to_string(),
                main_file: "src/main.eps".to_string(),
                settings: ProjectSettings::default(),
                plugins: Vec::new(),
                python_entrypoints: Vec::new(),
                python_dependencies: Vec::new(),
                python_lock: None,
                editor_compatibility: None,
            },
        )
        .unwrap();
        let euddraft = base.join("euddraft.exe");
        fs::write(&euddraft, b"stub").unwrap();
        let config = Config {
            project_path: project_root.to_string_lossy().into_owned(),
            euddraft_path: euddraft.to_string_lossy().into_owned(),
            default_provider: Some(crate::provider::ProviderId::Codex),
            providers: crate::provider::ProviderSettings {
                codex: crate::provider::CodexProviderSettings {
                    default_model: Some("gpt-test".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            },
            model: crate::config::AssetSpec {
                name: bootstrap::DEFAULT_MODEL_NAME.to_string(),
                ..Default::default()
            },
            rag_index: place_rag_asset(&dirs),
            ..Default::default()
        };
        let status = status_from_config(
            &dirs,
            &config,
            provider_statuses(
                Some(crate::provider::ProviderId::Codex),
                crate::provider::ProviderAvailability::Ready,
            ),
            None,
            false,
        );
        assert!(status.project_valid);
        assert!(status.euddraft_valid);
        assert!(!status.setup_required);
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn compile_settings_reports_managed_version_and_available_update() {
        let base = unique_temp_dir("euddraft-settings");
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        dirs.ensure_dirs().unwrap();
        let install = dirs.euddraft_dir().join("sha256-old");
        fs::create_dir_all(&install).unwrap();
        let executable = install.join("euddraft.exe");
        fs::write(&executable, b"managed").unwrap();
        fs::write(
            install.join(".euddraft-install.json"),
            serde_json::to_vec(&serde_json::json!({
                "version": "v0.10.2.5",
                "archive_sha256": HELLO_SHA,
                "executable": "euddraft.exe",
                "files": [{
                    "path": "euddraft.exe",
                    "sha256": "7fdfda5f50a433ae127a784fc143105fb6d93fedec7601ddeb3d1d584f83de05",
                    "bytes": 7
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        dirs.save_config(&Config {
            euddraft_path: executable.to_string_lossy().into_owned(),
            ..Default::default()
        })
        .unwrap();

        let status = euddraft_settings_payload(&dirs, Some("v0.11.0.1".to_string())).unwrap();
        assert!(status.valid);
        assert!(status.managed);
        assert_eq!(status.installed_version.as_deref(), Some("v0.10.2.5"));
        assert_eq!(status.latest_version.as_deref(), Some("v0.11.0.1"));
        assert!(status.update_available);
        fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn cached_stale_rag_config_rolls_forward_to_current_readiness() {
        let base = unique_temp_dir("rag-rollover");
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        dirs.ensure_dirs().unwrap();

        let mut stale_rag = place_rag_asset(&dirs);
        stale_rag.version = "2".to_string();
        dirs.save_config(&Config {
            model: crate::config::AssetSpec {
                name: bootstrap::DEFAULT_MODEL_NAME.to_string(),
                ..Default::default()
            },
            rag_index: stale_rag,
            ..Default::default()
        })
        .unwrap();

        let stale = setup_status_payload(&dirs, Vec::new()).unwrap();
        assert!(
            !stale.assets_ready,
            "a cached stale pin must keep setup in the asset-refresh state"
        );

        let current = Config {
            model: crate::config::AssetSpec {
                name: bootstrap::DEFAULT_MODEL_NAME.to_string(),
                ..Default::default()
            },
            rag_index: place_rag_asset(&dirs),
            ..Default::default()
        };
        dirs.save_config(&current).unwrap();
        let refreshed = setup_status_payload(&dirs, Vec::new()).unwrap();
        assert!(
            refreshed.assets_ready,
            "a cached current pin with verified assets must be ready"
        );
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn default_install_keeps_a_manual_selection_made_during_download() {
        let base = unique_temp_dir("euddraft-precedence");
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        dirs.ensure_dirs().unwrap();
        let installed = base.join("euddraft.exe");
        fs::write(&installed, b"managed").unwrap();
        let local = base.join("existing");
        fs::create_dir_all(&local).unwrap();
        fs::write(local.join("euddraft.exe"), b"local").unwrap();
        let selected = Config {
            euddraft_path: local.to_string_lossy().into_owned(),
            project_path: "project-picked-while-downloading".to_string(),
            ..Default::default()
        };
        dirs.save_config(&selected).unwrap();
        persist_euddraft_path(&dirs, &installed, false).unwrap();
        assert_eq!(dirs.load_config().unwrap(), selected);
        // Explicit latest selection replaces only the toolchain path.
        persist_euddraft_path(&dirs, &installed, true).unwrap();
        let current = dirs.load_config().unwrap();
        assert_eq!(current.euddraft_path, installed.to_string_lossy());
        assert_eq!(current.project_path, selected.project_path);
        fs::remove_dir_all(base).unwrap();
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
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(destination.join("project.eap")).unwrap()).unwrap();
        assert_eq!(
            manifest["schemaVersion"],
            crate::native_project::PROJECT_SCHEMA_VERSION
        );
        assert_eq!(manifest["name"], "input");
        assert!(!destination.join("project.json").exists());
        assert!(!destination.join("project.eudproj").exists());
        fs::remove_dir_all(base).ok();
    }
    #[test]
    fn recent_listing_seeds_once_and_remove_does_not_reseed() {
        let base = unique_temp_dir("recent-seed");
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        dirs.ensure_dirs().unwrap();
        let source = base.join("input.scx");
        let destination = base.join("project");
        fs::write(&source, b"map").unwrap();
        create_project_from_map(&source, &destination).unwrap();
        dirs.save_config(&Config {
            project_path: destination.to_string_lossy().into_owned(),
            ..Config::default()
        })
        .unwrap();

        assert_eq!(recent_projects_payload(&dirs).unwrap().len(), 1);
        let mut config = dirs.load_config().unwrap();
        config.remove_recent_project(&destination.to_string_lossy());
        dirs.save_config(&config).unwrap();
        assert!(recent_projects_payload(&dirs).unwrap().is_empty());
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

    #[cfg(windows)]
    #[test]
    fn failed_import_clears_output_without_deleting_the_open_destination() {
        use std::os::windows::fs::OpenOptionsExt;

        let destination = unique_temp_dir("open-import-destination");
        fs::create_dir(destination.join("maps")).unwrap();
        fs::write(destination.join("maps/source.scx"), b"imported map").unwrap();
        fs::write(destination.join("project.eap"), b"imported manifest").unwrap();
        let folder_handle = fs::OpenOptions::new()
            .read(true)
            .share_mode(3) // FILE_SHARE_READ | FILE_SHARE_WRITE, not DELETE.
            .custom_flags(0x02000000) // FILE_FLAG_BACKUP_SEMANTICS for a directory.
            .open(&destination)
            .unwrap();

        restore_empty_destination_after_failure(
            &destination,
            true,
            &Err("legacy approved plan is missing".to_string()),
        )
        .unwrap();

        assert!(destination.is_dir());
        assert!(destination_is_empty(&destination).unwrap());
        drop(folder_handle);
        fs::remove_dir(destination).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn rollback_reports_the_locked_output_without_losing_the_import_error() {
        use std::os::windows::fs::OpenOptionsExt;

        let destination = unique_temp_dir("locked-import-output");
        let locked_path = destination.join("project.eap");
        fs::write(&locked_path, b"imported manifest").unwrap();
        let file_handle = fs::OpenOptions::new()
            .read(true)
            .share_mode(3)
            .open(&locked_path)
            .unwrap();
        let original = "legacy approved plan is missing".to_string();

        let error =
            restore_empty_destination_after_failure(&destination, true, &Err(original.clone()))
                .unwrap_err();

        assert!(error.contains(&original));
        assert!(error.contains(&locked_path.to_string_lossy().to_string()));
        assert_eq!(fs::read(&locked_path).unwrap(), b"imported manifest");
        drop(file_handle);
        fs::remove_dir_all(destination).unwrap();
    }

    #[test]
    #[ignore = "requires EUD_AGENT_E3S_FIXTURE and EUD_AGENT_E3S_COMPAT"]
    fn real_e3s_import_restores_legacy_harness_without_mutating_source() {
        use crate::memory::ProjectMemory;
        use crate::workspace::WorkspaceManager;
        use sha2::{Digest, Sha256};

        let fixture = PathBuf::from(std::env::var("EUD_AGENT_E3S_FIXTURE").unwrap());
        let compat = PathBuf::from(std::env::var("EUD_AGENT_E3S_COMPAT").unwrap());
        let base = unique_temp_dir("import-harness");
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        dirs.ensure_dirs().unwrap();
        crate::native_build::sync_compat_assets(&compat, &dirs.native_assets_dir()).unwrap();
        let source_path = fs::canonicalize(&fixture).unwrap();
        #[cfg(windows)]
        let source_path = dunce::simplified(&source_path);
        let source_project = source_path.to_string_lossy().into_owned();
        let original = fs::read(&fixture).unwrap();
        let workspaces = WorkspaceManager::new(dirs.clone());
        let legacy_id = format!(
            "{:x}",
            Sha256::digest(format!("{source_project}\nlegacy-source.scx"))
        );
        let legacy_root = dirs.workspaces_dir().join(&legacy_id);
        fs::create_dir_all(legacy_root.join("plans")).unwrap();
        fs::create_dir_all(legacy_root.join("specs")).unwrap();
        fs::write(legacy_root.join("specs/healthy.md"), "# 정상 문서").unwrap();
        let approved_body = "# 기존 승인 계획";
        fs::write(
            legacy_root.join("plans/approved-before-import.md"),
            approved_body,
        )
        .unwrap();
        let mut plans = serde_json::Map::new();
        for request in ["approved-before-import", "missing-one", "missing-two"] {
            plans.insert(
                request.to_string(),
                serde_json::json!({
                    "revision": 2, "approvedAt": 1,
                    "markdownSha256": format!("{:x}", Sha256::digest(approved_body)),
                }),
            );
        }
        let original_state = serde_json::to_vec(&serde_json::json!({
            "version": 1, "id": legacy_id, "project": source_project,
            "identityHash": legacy_id,
            "documents": {
                "specs/healthy.md": {"revision": 1, "state": "accepted", "acceptedAt": 1, "requestId": "accepted-doc"},
                "specs/missing.md": {"revision": 1, "state": "accepted", "acceptedAt": 1, "requestId": "missing-doc"}
            },
            "approvedPlans": plans,
        })).unwrap();
        let original_state_path = dirs
            .workspace_state_dir()
            .join("projects")
            .join(format!("{legacy_id}.json"));
        fs::create_dir_all(original_state_path.parent().unwrap()).unwrap();
        fs::write(&original_state_path, &original_state).unwrap();
        // The legacy prompt parser persisted surrounding quotes in memory/session
        // keys, unlike the unquoted workspace identity.
        let quoted_project = format!("'{source_project}'");
        let memory = ProjectMemory::new(dirs.memory_dir(), &quoted_project);
        assert!(memory.write("conventions", "스위치 21은 보스 단계 전용").ok);
        memory.update_list_hash("legacy source list").unwrap();
        let wiki_dir = memory.store_dir().unwrap().join("wiki");
        fs::write(memory.store_dir().unwrap().join("lessons.md"), [0xff, 0xfe]).unwrap();
        fs::create_dir_all(&wiki_dir).unwrap();
        let ledger = serde_json::json!({
            "version": 1,
            "entries": {
                "dat:units:7:HP": {
                    "table": "dat", "dat": "units", "objId": 7,
                    "property": "HP", "value": 80, "itemName": "Marine",
                    "appliedAt": 10, "editedByUser": true
                }
            }
        });
        let ledger_bytes = serde_json::to_vec(&ledger).unwrap();
        fs::write(wiki_dir.join("ledger.json"), &ledger_bytes).unwrap();
        let sessions = crate::session::SessionStore::new(&dirs);
        let session = crate::session::SessionRecord {
            meta: crate::session::SessionMeta {
                id: crate::session::new_session_id(),
                name: "기존 대화".to_string(),
                project: quoted_project,
                kind: crate::session::SessionKind::Eps,
                provider: crate::provider::ProviderId::Codex,
                model: "gpt-test".to_string(),
                created_at: 1_718_000_000,
                last_conversation_at: 1_718_000_000_000,
            },
            provider_binding: crate::provider::ProviderBinding {
                provider: crate::provider::ProviderId::Codex,
                model: "gpt-test".to_string(),
                reasoning: None,
                base_url: None,
                conversation: crate::provider::ProviderConversationState::Codex {
                    thread_id: Some("old-project-thread".to_string()),
                },
            },
            pending_request_ids: vec!["old-pending-review".to_string()],
            context_usage: None,
            panel_log: serde_json::json!({
                "schemaVersion": 2, "logSeq": 2,
                "log": [
                    {"id": 1, "kind": "you", "text": "보스 단계 스위치를 정해 줘"},
                    {"id": 2, "kind": "agent", "text": "스위치 21을 사용합니다."}
                ]
            }),
            context_state: Default::default(),
            task_state: Default::default(),
            autonomous_run: None,
        };
        sessions.save(&session).unwrap();
        let original_session_path = dirs
            .sessions_dir()
            .join(format!("{}.json", session.meta.id));
        let original_session = fs::read(&original_session_path).unwrap();
        let original_index = fs::read(dirs.sessions_dir().join("index.json")).unwrap();
        let destination = base.join("native");

        fs::create_dir(&destination).unwrap();
        // This occupied AppData target used to reject the entire E3S import.
        let old_target_id = format!(
            "{:x}",
            Sha256::digest(
                fs::canonicalize(&destination)
                    .unwrap()
                    .to_string_lossy()
                    .as_bytes()
            )
        );
        let occupied = dirs
            .workspaces_dir()
            .join(old_target_id)
            .join("specs/user.md");
        fs::create_dir_all(occupied.parent().unwrap()).unwrap();
        fs::write(&occupied, "사용자 기존 자료").unwrap();
        let previous_config = dirs.load_config().unwrap();
        #[cfg(windows)]
        let folder_handle = {
            use std::os::windows::fs::OpenOptionsExt;
            fs::OpenOptions::new()
                .read(true)
                .share_mode(3)
                .custom_flags(0x02000000)
                .open(&destination)
                .unwrap()
        };
        let import_issues =
            match import_project_from_e3s(&dirs, &fixture, &destination, &[]).unwrap() {
                ImportProjectOutcome::NeedsApproval { import_issues } => import_issues,
                ImportProjectOutcome::Completed => panic!("import unexpectedly completed"),
            };
        for path in [
            "plans/missing-one.md",
            "plans/missing-two.md",
            "specs/missing.md",
            "lessons.md",
        ] {
            assert!(
                import_issues
                    .iter()
                    .any(|issue| issue.path.replace('\\', "/").ends_with(path)),
                "missing review item {path}: {import_issues:?}"
            );
        }
        assert!(destination_is_empty(&destination).unwrap());
        assert_eq!(dirs.load_config().unwrap(), previous_config);
        assert_eq!(
            fs::read(dirs.sessions_dir().join("index.json")).unwrap(),
            original_index
        );
        #[cfg(windows)]
        drop(folder_handle);
        let excluded_import_items = import_issues
            .iter()
            .map(|issue| issue.id.clone())
            .collect::<Vec<_>>();
        // A newly corrupt conversation requires renewed review, not an error.
        fs::write(&original_session_path, b"{broken").unwrap();
        let renewed =
            match import_project_from_e3s(&dirs, &fixture, &destination, &excluded_import_items)
                .unwrap()
            {
                ImportProjectOutcome::NeedsApproval { import_issues } => import_issues,
                ImportProjectOutcome::Completed => panic!("new issue was not reviewed"),
            };
        assert!(renewed.iter().any(|issue| issue.scope == "sessions"));
        assert!(destination_is_empty(&destination).unwrap());
        assert_eq!(dirs.load_config().unwrap(), previous_config);
        assert_eq!(
            fs::read(dirs.sessions_dir().join("index.json")).unwrap(),
            original_index
        );
        fs::write(&original_session_path, &original_session).unwrap();

        assert_eq!(
            import_project_from_e3s(&dirs, &fixture, &destination, &excluded_import_items).unwrap(),
            ImportProjectOutcome::Completed
        );
        assert_eq!(fs::read(&original_state_path).unwrap(), original_state);

        let project = crate::native_runtime::NativeProjectManager::new(dirs.clone())
            .open()
            .unwrap();
        let imported_memory = ProjectMemory::for_project(&project);
        assert!(imported_memory
            .store_dir()
            .unwrap()
            .starts_with(project.root()));
        assert!(!imported_memory
            .store_dir()
            .unwrap()
            .join("lessons.md")
            .exists());
        assert_eq!(fs::read_to_string(&occupied).unwrap(), "사용자 기존 자료");
        assert_eq!(
            imported_memory.read("conventions"),
            memory.read("conventions")
        );
        assert!(imported_memory.is_stale("native source list"));
        let imported_wiki = crate::wiki::WikiStore::load(dirs.wiki_dir(&project.manifest().name));
        assert_eq!(
            serde_json::to_value(imported_wiki.ledger()).unwrap(),
            ledger
        );
        let copied_sessions = sessions
            .list_kind(crate::session::SessionKind::Eps)
            .unwrap()
            .into_iter()
            .filter(|meta| meta.project == project.manifest().name)
            .collect::<Vec<_>>();
        assert_eq!(copied_sessions.len(), 1);
        let copied = sessions.load(&copied_sessions[0].id).unwrap();
        assert_ne!(copied.meta.id, session.meta.id);
        assert_eq!(copied.meta.name, session.meta.name);
        assert_eq!(copied.panel_log, session.panel_log);
        assert_eq!(
            copied.meta.last_conversation_at,
            session.meta.last_conversation_at
        );
        assert!(copied.pending_request_ids.is_empty());
        assert_eq!(
            copied.provider_binding.conversation,
            crate::provider::ProviderConversationState::Codex { thread_id: None }
        );
        assert_eq!(fs::read(&original_session_path).unwrap(), original_session);
        assert_eq!(
            fs::read(wiki_dir.join("ledger.json")).unwrap(),
            ledger_bytes
        );
        let imported_workspace = workspaces.prepare_current().unwrap();
        assert_ne!(imported_workspace.id, legacy_id);
        assert!(imported_workspace.root.starts_with(project.root()));
        assert_eq!(
            workspaces
                .read_file(&imported_workspace.id, "specs/healthy.md")
                .unwrap(),
            "# 정상 문서"
        );
        assert!(!imported_workspace
            .workspace_root
            .join("specs/missing.md")
            .exists());
        assert_eq!(
            workspaces
                .read_file(&imported_workspace.id, "plans/approved-before-import.md")
                .unwrap(),
            "# 기존 승인 계획"
        );
        let approved = workspaces
            .list_files(&imported_workspace)
            .unwrap()
            .into_iter()
            .find(|entry| entry.path == "plans/approved-before-import.md")
            .unwrap();
        assert_eq!(approved.state.as_deref(), Some("approved"));
        assert_eq!(approved.revision, Some(2));
        assert_eq!(memory.read("conventions"), "스위치 21은 보스 단계 전용");
        assert_eq!(
            fs::read_to_string(legacy_root.join("plans/approved-before-import.md")).unwrap(),
            approved_body
        );
        assert_eq!(fs::read(&fixture).unwrap(), original);

        // Same-name projects now own independent local memory/workspaces.
        // Machine-local conversation ownership may require an omission review.
        let second_destination = base.join("same-name");
        let outcome =
            import_project_from_e3s(&dirs, &fixture, &second_destination, &excluded_import_items)
                .unwrap();
        if let ImportProjectOutcome::NeedsApproval { import_issues } = outcome {
            let approved = import_issues
                .into_iter()
                .map(|issue| issue.id)
                .collect::<Vec<_>>();
            assert_eq!(
                import_project_from_e3s(&dirs, &fixture, &second_destination, &approved).unwrap(),
                ImportProjectOutcome::Completed
            );
        }
        let second = crate::native_project::NativeProject::open(&second_destination).unwrap();
        let second_memory = ProjectMemory::for_project(&second);
        assert!(second_memory.write("conventions", "두 번째 프로젝트").ok);
        assert_eq!(
            imported_memory.read("conventions"),
            memory.read("conventions")
        );
        assert_eq!(fs::read(&original_state_path).unwrap(), original_state);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn e3s_import_errors_keep_their_recovery_category() {
        assert_eq!(
            e3s_import_error_code(
                "legacy E3S referenced source map is missing: C:\\Old\\sample.scx"
            ),
            E3S_IMPORT_SOURCE_MAP_MISSING
        );
        assert_eq!(
            e3s_import_error_code("native import destination must be empty: C:\\Work\\Used"),
            E3S_IMPORT_DESTINATION_NOT_EMPTY
        );
        assert_eq!(
            e3s_import_error_code("unsupported main source kind GUIEps"),
            E3S_IMPORT_UNSUPPORTED
        );
        assert_eq!(
            e3s_import_error_code("invalid NRBF stream header"),
            E3S_IMPORT_FAILED
        );
        assert_eq!(
            e3s_import_error_code("legacy harness import failed: unsupported document metadata"),
            E3S_IMPORT_HARNESS_FAILED
        );
    }
}
