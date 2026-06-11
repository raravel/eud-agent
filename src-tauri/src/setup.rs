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

use serde::{Deserialize, Serialize};
use tauri_plugin_dialog::DialogExt;

use crate::bootstrap::{self, ProgressEmitter};
use crate::config::{self, Config, DataDirs};
use crate::ipc::BridgeManaged;

/// Stable error code for a picked folder that is not an EUD Editor 3 install.
/// The panel maps codes to user-facing text (rules.md: raw identifiers are never
/// rendered as-is).
pub const INVALID_EDITOR_FOLDER: &str = "invalid_editor_folder";

/// `setup_status` / `setup_pick_editor_path` command output (panel `setup` message).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetupStatusResponse {
    /// Configured editor install root (empty until picked).
    pub editor_path: String,
    /// True when `editor_path` points at a real EUD Editor 3 install.
    pub editor_valid: bool,
    /// True when the model + RAG index pass the manifest check (no download needed).
    pub assets_ready: bool,
    /// True when the codex CLI was found (PATH / `CODEX_CMD`).
    pub codex_resolved: bool,
    /// True when `codex login status` reports a logged-in session.
    pub codex_authed: bool,
    /// True when the panel must show the setup screen before normal operation.
    pub setup_required: bool,
    /// Optional stable error code (e.g. a rejected folder pick).
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
    let editor_path = config.editor_path.trim().to_string();
    let editor_valid =
        !editor_path.is_empty() && config::validate_editor_path(Path::new(&editor_path));
    let assets_ready = !bootstrap::needs_bootstrap(dirs, config);
    // codex must be installed AND logged in before any turn can run; an
    // unauthenticated codex fails every turn, so it gates setup like the editor
    // path and the assets do.
    SetupStatusResponse {
        editor_path,
        editor_valid,
        assets_ready,
        codex_resolved: codex.resolved,
        codex_authed: codex.authed,
        setup_required: !editor_valid || !assets_ready || !codex.authed,
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
    if config.rag_index.sha256.trim().is_empty() {
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
) -> Result<SetupStatusResponse, String> {
    let dirs = state.dirs().clone();
    // The manifest check hashes the RAG index; keep it off the IPC thread.
    tauri::async_runtime::spawn_blocking(move || setup_status_payload(&dirs))
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
) -> Result<SetupStatusResponse, String> {
    let dirs = state.dirs().clone();
    // blocking_pick_folder must not run on the main thread (it pumps its own loop).
    tauri::async_runtime::spawn_blocking(move || {
        let Some(picked) = app.dialog().file().blocking_pick_folder() else {
            return setup_status_payload(&dirs);
        };
        let picked = picked.into_path().map_err(|error| error.to_string())?;
        let mut config = dirs.load_config().map_err(|error| error.to_string())?;
        let codex = crate::codex_auth::login_status();
        if !config::validate_editor_path(&picked) {
            return Ok(status_from_config(
                &dirs,
                &config,
                &codex,
                Some(INVALID_EDITOR_FOLDER.to_string()),
            ));
        }
        config.editor_path = picked.to_string_lossy().into_owned();
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

    fn codex_authed() -> crate::codex_auth::CodexAuthState {
        crate::codex_auth::CodexAuthState {
            resolved: true,
            authed: true,
            detail: "logged in".to_string(),
        }
    }

    fn codex_unauthed() -> crate::codex_auth::CodexAuthState {
        crate::codex_auth::CodexAuthState {
            resolved: true,
            authed: false,
            detail: "not logged in".to_string(),
        }
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
            version: "1".to_string(),
        }
    }

    #[test]
    fn setup_required_on_first_run_without_config() {
        let base = unique_temp_dir("first-run");
        let dirs = make_dirs(&base);

        let status = setup_status_payload(&dirs).unwrap();

        assert_eq!(status.editor_path, "");
        assert!(!status.editor_valid);
        assert!(!status.assets_ready);
        assert!(status.setup_required);
        assert_eq!(status.error, None);

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

        let status = setup_status_payload(&dirs).unwrap();

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
            editor_path: editor.to_string_lossy().into_owned(),
            model: AssetSpec {
                name: "BAAI/bge-m3".to_string(),
                ..Default::default()
            },
            rag_index: rag_spec,
            ..Default::default()
        };
        dirs.save_config(&config).unwrap();

        // Inject an authed codex so the assertion isolates editor + assets from
        // the ambient codex login state of the test host.
        let status = status_from_config(&dirs, &config, &codex_authed(), None);

        assert!(status.editor_valid);
        assert!(status.assets_ready);
        assert!(status.codex_authed);
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
            editor_path: editor.to_string_lossy().into_owned(),
            model: AssetSpec {
                name: "BAAI/bge-m3".to_string(),
                ..Default::default()
            },
            rag_index: rag_spec,
            ..Default::default()
        };

        let status = status_from_config(&dirs, &config, &codex_unauthed(), None);

        assert!(status.editor_valid);
        assert!(status.assets_ready);
        assert!(!status.codex_authed);
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

        let status = setup_status_payload(&dirs).unwrap();

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
            &codex_authed(),
            Some(INVALID_EDITOR_FOLDER.to_string()),
        );

        assert_eq!(status.error.as_deref(), Some(INVALID_EDITOR_FOLDER));
        assert!(status.setup_required);

        fs::remove_dir_all(&base).ok();
    }
}
