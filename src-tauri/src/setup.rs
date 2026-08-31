//! First-run setup surface: manifest check, editor-path picker, bootstrap runner.
//!
//! Feature 10's boot flow gates the panel behind a setup screen when the manifest
//! check fails (editor-path config + model + RAG index). This module is that check
//! plus the commands the setup screen drives: the editor folder picker
//! (pick -> validate -> store) and the bootstrap download. `lib.rs` auto-runs the
//! download on later launches when an asset went missing/corrupt but the editor
//! path is already configured; the very first run stays panel-driven so the user
//! picks the editor folder before anything downloads.

use std::path::Path;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use tauri_plugin_dialog::DialogExt;
use tokio::sync::Mutex;

use crate::bootstrap::{self, ProgressEmitter};
use crate::config::{self, Config, DataDirs};
use crate::ipc::BridgeManaged;

/// Process-wide bootstrap serialization lock.
///
/// Bootstrap has two entry points that can fire concurrently: the startup
/// auto-bootstrap (`lib.rs`, when an asset is missing but the editor path is set) and
/// the panel-driven [`bootstrap_run`] command. Both download each asset to a FIXED
/// `<asset>.tmp` path, so an overlap races on the same tmp — one entrant renames it into
/// place while the other's `verify_and_place` rename then hits `os error 2` (tmp gone).
/// Holding this lock for the whole run serializes them; the second entrant re-checks and
/// finds the asset already `Present`, so it no-ops instead of re-downloading.
fn bootstrap_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Stable error code for a picked folder that is not an EUD Editor 3 install.
/// The panel maps codes to user-facing text (rules.md: raw identifiers are never
/// rendered as-is).
pub const INVALID_EDITOR_FOLDER: &str = "invalid_editor_folder";

/// Typed five-provider setup snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupStatusResponse {
    pub editor_path: String,
    pub editor_valid: bool,
    pub assets_ready: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_provider: Option<crate::provider::ProviderId>,
    pub providers: Vec<crate::provider::ProviderStatus>,
    pub setup_required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub fn setup_status_payload(
    dirs: &DataDirs,
    providers: Vec<crate::provider::ProviderStatus>,
) -> Result<SetupStatusResponse, String> {
    let config = dirs.load_config().map_err(|error| error.to_string())?;
    Ok(status_from_config(dirs, &config, providers, None))
}

fn status_from_config(
    dirs: &DataDirs,
    config: &Config,
    providers: Vec<crate::provider::ProviderStatus>,
    error: Option<String>,
) -> SetupStatusResponse {
    let editor_path = config.editor_path.trim().to_string();
    let editor_valid =
        !editor_path.is_empty() && config::validate_editor_path(Path::new(&editor_path));
    let assets_ready = !bootstrap::needs_bootstrap(dirs, config);
    let selected_ready = config.default_provider.is_some_and(|selected| {
        providers
            .iter()
            .find(|status| status.provider == selected)
            .is_some_and(|status| status.availability.is_ready())
            && config.providers.default_model(selected).is_some()
    });
    SetupStatusResponse {
        editor_path,
        editor_valid,
        assets_ready,
        default_provider: config.default_provider,
        providers,
        setup_required: !editor_valid || !assets_ready || !selected_ready,
        error,
    }
}

/// True when a later launch should auto-run the bootstrap: the editor path is
/// already configured and valid, but an asset is missing/corrupt. The first run
/// (no editor path yet) is panel-driven instead, so nothing downloads before the
/// user has been asked anything.
pub fn should_auto_bootstrap(dirs: &DataDirs) -> bool {
    match dirs.load_config() {
        Ok(config) => {
            let editor_path = config.editor_path.trim();
            !editor_path.is_empty()
                && config::validate_editor_path(Path::new(editor_path))
                && bootstrap::needs_bootstrap(dirs, &config)
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

/// Report the first-run setup state (editor path + asset manifest check).
#[tauri::command]
pub async fn setup_status(
    state: tauri::State<'_, BridgeManaged>,
    providers: tauri::State<'_, crate::provider_service::ProviderService>,
) -> Result<SetupStatusResponse, String> {
    let dirs = state.dirs().clone();
    let statuses = providers.status_list().await?;
    tauri::async_runtime::spawn_blocking(move || setup_status_payload(&dirs, statuses))
        .await
        .map_err(|error| error.to_string())?
}

/// Open the native folder picker, validate the selection as an EUD Editor 3 install,
/// and persist it to `config.json`. A cancelled pick returns the unchanged state; an
/// invalid folder returns the state with the `invalid_editor_folder` error code.
#[tauri::command]
pub async fn setup_pick_editor_path(
    app: tauri::AppHandle,
    state: tauri::State<'_, BridgeManaged>,
    providers: tauri::State<'_, crate::provider_service::ProviderService>,
) -> Result<SetupStatusResponse, String> {
    let dirs = state.dirs().clone();
    let statuses = providers.status_list().await?;
    tauri::async_runtime::spawn_blocking(move || {
        let Some(picked) = app.dialog().file().blocking_pick_folder() else {
            return setup_status_payload(&dirs, statuses);
        };
        let picked = picked.into_path().map_err(|error| error.to_string())?;
        let mut config = dirs.load_config().map_err(|error| error.to_string())?;
        if !config::validate_editor_path(&picked) {
            return Ok(status_from_config(
                &dirs,
                &config,
                statuses,
                Some(INVALID_EDITOR_FOLDER.to_string()),
            ));
        }
        config.editor_path = picked.to_string_lossy().into_owned();
        dirs.save_config(&config)
            .map_err(|error| error.to_string())?;
        Ok(status_from_config(&dirs, &config, statuses, None))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn setup_provider_select(
    state: tauri::State<'_, BridgeManaged>,
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
    let core_setup_incomplete = config.editor_path.trim().is_empty()
        || !config::validate_editor_path(Path::new(config.editor_path.trim()))
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
    state: tauri::State<'_, BridgeManaged>,
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
    use crate::config::AssetSpec;
    use std::fs;
    use std::path::PathBuf;

    // sha256("hello") — matches the bootstrap manifest test vector.
    const HELLO_SHA: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

    fn unique_temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("eud-agent-setup-test-{tag}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn make_dirs(base: &Path) -> DataDirs {
        DataDirs::from_bases(&base.join("roaming"), &base.join("local"))
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

    /// A fake EUD Editor 3 install root (`Data\Lua\TriggerEditor` marker present).
    fn make_editor_root(base: &Path) -> PathBuf {
        let editor = base.join("EUDEditor3");
        fs::create_dir_all(editor.join("Data").join("Lua").join("TriggerEditor")).unwrap();
        editor
    }

    /// Place a verified RAG index asset matching `spec_sha` under `dirs.rag_dir()`.
    fn place_rag_asset(dirs: &DataDirs) -> AssetSpec {
        fs::create_dir_all(dirs.rag_dir()).unwrap();
        fs::write(dirs.rag_dir().join(RAG_INDEX_FILENAME), b"hello").unwrap();
        AssetSpec {
            name: "https://example.com/rag-index.bin".to_string(),
            sha256: HELLO_SHA.to_string(),
            version: bootstrap::REQUIRED_RAG_INDEX_VERSION.to_string(),
        }
    }

    #[test]
    fn setup_required_on_first_run_without_config() {
        let base = unique_temp_dir("first-run");
        let dirs = make_dirs(&base);

        let status = setup_status_payload(
            &dirs,
            provider_statuses(None, crate::provider::ProviderAvailability::Unavailable),
        )
        .unwrap();

        assert_eq!(status.editor_path, "");
        assert!(!status.editor_valid);
        assert!(!status.assets_ready);
        assert!(status.setup_required);
        assert_eq!(status.error, None);

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn setup_json_omits_absent_optional_fields_for_the_panel_guard() {
        let base = unique_temp_dir("setup-json");
        let dirs = make_dirs(&base);
        let status = setup_status_payload(
            &dirs,
            provider_statuses(None, crate::provider::ProviderAvailability::Unavailable),
        )
        .unwrap();
        let json = serde_json::to_value(status).unwrap();
        let object = json.as_object().unwrap();
        assert!(!object.contains_key("defaultProvider"));
        assert!(!object.contains_key("error"));
        for provider in object["providers"].as_array().unwrap() {
            assert!(!provider.as_object().unwrap().contains_key("detailCode"));
        }
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn setup_required_when_editor_path_is_stale() {
        // A configured-but-vanished editor folder must route back to the picker.
        let base = unique_temp_dir("stale-editor");
        let dirs = make_dirs(&base);
        let config = Config {
            editor_path: base.join("missing-editor").to_string_lossy().into_owned(),
            ..Default::default()
        };
        dirs.save_config(&config).unwrap();

        let status = setup_status_payload(
            &dirs,
            provider_statuses(None, crate::provider::ProviderAvailability::Unavailable),
        )
        .unwrap();

        assert!(!status.editor_valid);
        assert!(status.setup_required);

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn setup_not_required_when_editor_valid_and_assets_verified() {
        let base = unique_temp_dir("ready");
        let dirs = make_dirs(&base);
        let editor = make_editor_root(&base);
        let rag_spec = place_rag_asset(&dirs);
        let config = Config {
            default_provider: Some(crate::provider::ProviderId::Codex),
            providers: crate::provider::ProviderSettings {
                codex: crate::provider::CodexProviderSettings {
                    default_model: Some("gpt-test".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            },
            editor_path: editor.to_string_lossy().into_owned(),
            model: AssetSpec {
                name: "BAAI/bge-m3".to_string(),
                ..Default::default()
            },
            rag_index: rag_spec,
            ..Default::default()
        };
        dirs.save_config(&config).unwrap();

        let status = status_from_config(
            &dirs,
            &config,
            provider_statuses(
                Some(crate::provider::ProviderId::Codex),
                crate::provider::ProviderAvailability::Ready,
            ),
            None,
        );

        assert!(status.editor_valid);
        assert!(status.assets_ready);
        assert!(!status.setup_required);

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn setup_required_when_codex_not_logged_in() {
        // Editor + assets ready, but codex is unauthenticated: still gated, since
        // every agent turn would otherwise fail on a codex auth error.
        let base = unique_temp_dir("codex-unauthed");
        let dirs = make_dirs(&base);
        let editor = make_editor_root(&base);
        let rag_spec = place_rag_asset(&dirs);
        let config = Config {
            default_provider: Some(crate::provider::ProviderId::Codex),
            providers: crate::provider::ProviderSettings {
                codex: crate::provider::CodexProviderSettings {
                    default_model: Some("gpt-test".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            },
            editor_path: editor.to_string_lossy().into_owned(),
            model: AssetSpec {
                name: "BAAI/bge-m3".to_string(),
                ..Default::default()
            },
            rag_index: rag_spec,
            ..Default::default()
        };

        let status = status_from_config(
            &dirs,
            &config,
            provider_statuses(
                Some(crate::provider::ProviderId::Codex),
                crate::provider::ProviderAvailability::NeedsAuthentication,
            ),
            None,
        );

        assert!(status.editor_valid);
        assert!(status.assets_ready);
        assert!(status.setup_required);

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn setup_required_when_editor_valid_but_assets_missing() {
        // The download step of the setup flow: path picked, assets still absent.
        let base = unique_temp_dir("assets-missing");
        let dirs = make_dirs(&base);
        let editor = make_editor_root(&base);
        let config = Config {
            editor_path: editor.to_string_lossy().into_owned(),
            ..Default::default()
        };
        dirs.save_config(&config).unwrap();

        let status = setup_status_payload(
            &dirs,
            provider_statuses(None, crate::provider::ProviderAvailability::Unavailable),
        )
        .unwrap();

        assert!(status.editor_valid);
        assert!(!status.assets_ready);
        assert!(status.setup_required);

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn auto_bootstrap_only_when_editor_configured_and_assets_needed() {
        // First run (no editor path): panel-driven, never auto-download.
        let base = unique_temp_dir("auto");
        let dirs = make_dirs(&base);
        assert!(!should_auto_bootstrap(&dirs));

        // Editor configured + assets missing: auto-run on launch.
        let editor = make_editor_root(&base);
        let mut config = Config {
            editor_path: editor.to_string_lossy().into_owned(),
            ..Default::default()
        };
        dirs.save_config(&config).unwrap();
        assert!(should_auto_bootstrap(&dirs));

        // Everything installed and verified: nothing to do.
        config.rag_index = place_rag_asset(&dirs);
        config.model.name = "BAAI/bge-m3".to_string();
        dirs.save_config(&config).unwrap();
        assert!(!should_auto_bootstrap(&dirs));

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn pick_error_code_is_carried_on_the_status_payload() {
        let base = unique_temp_dir("pick-error");
        let dirs = make_dirs(&base);
        let config = Config::default();

        let status = status_from_config(
            &dirs,
            &config,
            provider_statuses(None, crate::provider::ProviderAvailability::Unavailable),
            Some(INVALID_EDITOR_FOLDER.to_string()),
        );

        assert_eq!(status.error.as_deref(), Some(INVALID_EDITOR_FOLDER));
        assert!(status.setup_required);

        fs::remove_dir_all(&base).ok();
    }

    /// Regression (concurrent-bootstrap race): `run_bootstrap`'s `bootstrap_lock` must
    /// serialize overlapping runs so the auto + panel entry points never download to the
    /// shared `<asset>.tmp` at the same time (the overlap raced one rename into `os error
    /// 2`). Each task increments an in-flight counter inside the lock and yields, giving
    /// any unguarded peer a chance to overlap; the peak concurrency must stay 1. Remove
    /// the lock and the counter climbs to 8 at the yield, failing this assertion.
    #[tokio::test]
    async fn bootstrap_lock_serializes_concurrent_runs() {
        use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
        use std::sync::Arc;

        let inflight = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for _ in 0..8 {
            let inflight = inflight.clone();
            let peak = peak.clone();
            handles.push(tokio::spawn(async move {
                let _guard = bootstrap_lock().lock().await;
                let now = inflight.fetch_add(1, SeqCst) + 1;
                peak.fetch_max(now, SeqCst);
                tokio::task::yield_now().await;
                inflight.fetch_sub(1, SeqCst);
            }));
        }
        for handle in handles {
            handle.await.unwrap();
        }

        assert_eq!(
            peak.load(SeqCst),
            1,
            "bootstrap runs must be serialized by bootstrap_lock"
        );
    }
}
