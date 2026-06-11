//! config.json load/save, data-dir resolution, and editor-path validation.
//!
//! Data dirs (feature 10 / Decision 12):
//! - `app_data` -> `%appdata%\eud-agent\` : `config.json`, `memory/`, `map_backups/`,
//!   `journal/`.
//! - `app_local_data` -> `%localappdata%\eud-agent\` : `models/`, `rag/`, `logs/`.
//!   The model (~570MB) and RAG index NEVER live in Roaming.
//! - editor IPC dir: `<editor_path>\Data\agent\`.
//!
//! `config.json` is written UTF-8 **without BOM** (rules.md: a BOM breaks first-line
//! command parsing on the bridge side and we keep every app-written file BOM-free).

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const APP_DIR_NAME: &str = "eud-agent";
const CONFIG_FILE_NAME: &str = "config.json";

/// A downloadable, sha256-verified asset (the bge-m3 model or the RAG index).
///
/// For the model, `name` is the HF model id (e.g. `BAAI/bge-m3`); for the RAG index
/// it is the GitHub Release asset URL. `sha256`/`version` drive bootstrap verification
/// (the download flow itself is a later task).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AssetSpec {
    /// HF model id (model) or release asset URL (rag index).
    #[serde(default)]
    pub name: String,
    /// Expected sha256 of the placed asset.
    #[serde(default)]
    pub sha256: String,
    /// Asset version tag.
    #[serde(default)]
    pub version: String,
}

/// `config.json` contents (feature 10).
///
/// Every field carries a serde default so a partial / first-run file (e.g. `{}`)
/// deserializes cleanly.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Config {
    /// Absolute path to the EUD Editor 3 install root (the folder that contains
    /// `Data\Lua\TriggerEditor`). Empty until captured via the first-run picker.
    #[serde(default)]
    pub editor_path: String,
    /// Optional explicit path to the `codex` `.cmd` shim (overrides PATH resolution).
    #[serde(default)]
    pub codex_cmd: Option<String>,
    /// The embedding model asset.
    #[serde(default)]
    pub model: AssetSpec,
    /// The RAG index asset.
    #[serde(default)]
    pub rag_index: AssetSpec,
}

/// The editor's file-IPC directory: `<editor_path>\Data\agent`.
pub fn editor_ipc_dir(editor_path: &Path) -> PathBuf {
    editor_path.join("Data").join("agent")
}

/// True iff `<p>\Data\Lua\TriggerEditor` exists — the marker that `p` is a valid
/// EUD Editor 3 install root. Pure (filesystem-probe only) so it is unit-testable
/// without a running Tauri app; the folder picker wraps this (pick -> validate -> store).
pub fn validate_editor_path(p: &Path) -> bool {
    p.join("Data").join("Lua").join("TriggerEditor").is_dir()
}

/// Resolved app data directories.
///
/// Constructed either from raw OS base dirs ([`DataDirs::from_bases`], used by tests and
/// any caller that already has the bases) or from the Tauri path API
/// ([`DataDirs::resolve`]). Both append `eud-agent` to the respective base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataDirs {
    app_data: PathBuf,
    app_local_data: PathBuf,
}

impl DataDirs {
    /// Build from OS base dirs: `<roaming_base>\eud-agent` and `<local_base>\eud-agent`.
    ///
    /// `roaming_base` is `%appdata%` (Tauri `data_dir()`); `local_base` is
    /// `%localappdata%` (Tauri `local_data_dir()`).
    pub fn from_bases(roaming_base: &Path, local_base: &Path) -> Self {
        Self {
            app_data: roaming_base.join(APP_DIR_NAME),
            app_local_data: local_base.join(APP_DIR_NAME),
        }
    }

    /// Resolve from the Tauri path API. `data_dir()` is `%appdata%` (Roaming),
    /// `local_data_dir()` is `%localappdata%`; we append `eud-agent` to match the
    /// documented layout exactly (rather than Tauri's bundle-identifier dirs).
    pub fn resolve<R: tauri::Runtime, M: tauri::Manager<R>>(
        manager: &M,
    ) -> Result<Self, tauri::Error> {
        let roaming = manager.path().data_dir()?;
        let local = manager.path().local_data_dir()?;
        Ok(Self::from_bases(&roaming, &local))
    }

    /// `%appdata%\eud-agent\`.
    pub fn app_data(&self) -> &Path {
        &self.app_data
    }

    /// `%localappdata%\eud-agent\`.
    pub fn app_local_data(&self) -> &Path {
        &self.app_local_data
    }

    /// `%appdata%\eud-agent\config.json`.
    pub fn config_path(&self) -> PathBuf {
        self.app_data.join(CONFIG_FILE_NAME)
    }

    /// `%appdata%\eud-agent\memory`.
    pub fn memory_dir(&self) -> PathBuf {
        self.app_data.join("memory")
    }

    /// `%appdata%\eud-agent\map_backups`.
    pub fn map_backups_dir(&self) -> PathBuf {
        self.app_data.join("map_backups")
    }

    /// `%appdata%\eud-agent\journal`.
    pub fn journal_dir(&self) -> PathBuf {
        self.app_data.join("journal")
    }

    /// `%localappdata%\eud-agent\models` — NEVER in Roaming (the model is ~570MB).
    pub fn models_dir(&self) -> PathBuf {
        self.app_local_data.join("models")
    }

    /// `%localappdata%\eud-agent\rag`.
    pub fn rag_dir(&self) -> PathBuf {
        self.app_local_data.join("rag")
    }

    /// `%localappdata%\eud-agent\bin` — app-installed executables (the codex
    /// standalone binary). NEVER in Roaming; resolved by [`resolve_codex_cmd`].
    pub fn bin_dir(&self) -> PathBuf {
        self.app_local_data.join("bin")
    }

    /// `%localappdata%\eud-agent\logs`.
    pub fn logs_dir(&self) -> PathBuf {
        self.app_local_data.join("logs")
    }

    /// `%localappdata%\eud-agent\codex_workspace` — the STABLE, app-owned cwd
    /// for spawned codex processes (rules.md: never the launch dir). Kept empty
    /// so codex finds no AGENTS.md/repo there: launching from the dev repo
    /// otherwise injected the repo's hivemind instructions and made codex
    /// analyze the Rust repo instead of the map project (measured 2026-06-11).
    pub fn codex_workspace_dir(&self) -> PathBuf {
        self.app_local_data.join("codex_workspace")
    }

    /// Create every data subdir if missing. Idempotent (`create_dir_all`).
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        for dir in [
            self.app_data.clone(),
            self.memory_dir(),
            self.map_backups_dir(),
            self.journal_dir(),
            self.app_local_data.clone(),
            self.models_dir(),
            self.rag_dir(),
            self.bin_dir(),
            self.logs_dir(),
            self.codex_workspace_dir(),
        ] {
            fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    /// Load `config.json` from `app_data`. A missing file yields [`Config::default`]
    /// (first run); a present file is parsed (partial files fill via serde defaults).
    pub fn load_config(&self) -> anyhow::Result<Config> {
        let path = self.config_path();
        match fs::read(&path) {
            Ok(bytes) => {
                // `File.ReadAllText` strips a BOM; serde_json does not. Strip a UTF-8
                // BOM defensively so a hand-edited file still parses.
                let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes);
                Ok(serde_json::from_slice(bytes)?)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Serialize and write `config.json` to `app_data` as pretty UTF-8 **without BOM**.
    /// Creates the data dirs first so a fresh install can save before anything else runs.
    pub fn save_config(&self, config: &Config) -> anyhow::Result<()> {
        self.ensure_dirs()?;
        let json = serde_json::to_string_pretty(config)?;
        // `fs::write` of a `String` writes its raw UTF-8 bytes — no BOM is ever prepended.
        fs::write(self.config_path(), json)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    /// Unique temp base dir for a test, avoiding a `tempfile` dev-dependency
    /// (Cargo.toml is out of scope for this task).
    fn unique_temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("eud-agent-test-{tag}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn config_round_trips() {
        let cfg = Config {
            editor_path: "C:\\Games\\EUDEditor3".to_string(),
            codex_cmd: Some("C:\\tools\\codex.cmd".to_string()),
            model: AssetSpec {
                name: "BAAI/bge-m3".to_string(),
                sha256: "deadbeef".to_string(),
                version: "1".to_string(),
            },
            rag_index: AssetSpec {
                name: "https://example.com/rag.bin".to_string(),
                sha256: "cafef00d".to_string(),
                version: "1".to_string(),
            },
        };

        let json = serde_json::to_string(&cfg).unwrap();
        let back: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn partial_config_deserializes_with_defaults() {
        // A first-run / partial file must deserialize via serde defaults.
        let back: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(back.editor_path, "");
        assert_eq!(back.codex_cmd, None);
        assert_eq!(back.model, AssetSpec::default());
    }

    #[test]
    fn config_load_save_round_trips_on_disk() {
        let base = unique_temp_dir("loadsave");
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        dirs.ensure_dirs().unwrap();

        let cfg = Config {
            editor_path: "C:\\Games\\EUDEditor3".to_string(),
            ..Default::default()
        };
        dirs.save_config(&cfg).unwrap();

        let loaded = dirs.load_config().unwrap();
        assert_eq!(cfg, loaded);

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn config_file_has_no_bom() {
        let base = unique_temp_dir("nobom");
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        dirs.ensure_dirs().unwrap();
        dirs.save_config(&Config::default()).unwrap();

        let bytes = fs::read(dirs.config_path()).unwrap();
        // UTF-8 BOM is EF BB BF — never written.
        assert!(
            !bytes.starts_with(&[0xEF, 0xBB, 0xBF]),
            "config.json must not have a BOM"
        );

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn ensure_dirs_creates_missing_dirs() {
        let base = unique_temp_dir("ensure");
        let roaming = base.join("roaming");
        let local = base.join("local");
        let dirs = DataDirs::from_bases(&roaming, &local);

        assert!(!dirs.app_data().exists());
        assert!(!dirs.app_local_data().exists());

        dirs.ensure_dirs().unwrap();

        // Roaming subtree.
        assert!(dirs.app_data().is_dir());
        assert!(dirs.memory_dir().is_dir());
        assert!(dirs.map_backups_dir().is_dir());
        assert!(dirs.journal_dir().is_dir());
        // Local subtree — model/rag/logs NEVER in Roaming.
        assert!(dirs.app_local_data().is_dir());
        assert!(dirs.models_dir().is_dir());
        assert!(dirs.rag_dir().is_dir());
        assert!(dirs.logs_dir().is_dir());

        // The model dir must live under local, not roaming.
        assert!(dirs.models_dir().starts_with(dirs.app_local_data()));
        assert!(!dirs.models_dir().starts_with(dirs.app_data()));

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn data_dirs_append_eud_agent_to_bases() {
        let dirs = DataDirs::from_bases(&PathBuf::from("C:\\roam"), &PathBuf::from("C:\\loc"));
        assert_eq!(dirs.app_data(), &PathBuf::from("C:\\roam\\eud-agent"));
        assert_eq!(dirs.app_local_data(), &PathBuf::from("C:\\loc\\eud-agent"));
        assert_eq!(
            dirs.config_path(),
            PathBuf::from("C:\\roam\\eud-agent\\config.json")
        );
    }

    #[test]
    fn editor_ipc_dir_is_under_editor_path() {
        let editor = PathBuf::from("C:\\Games\\EUDEditor3");
        let ipc = editor_ipc_dir(&editor);
        assert_eq!(ipc, PathBuf::from("C:\\Games\\EUDEditor3\\Data\\agent"));
    }

    #[test]
    fn validate_editor_path_true_when_subfolder_exists() {
        let base = unique_temp_dir("valid-ok");
        let editor = base.join("EUDEditor3");
        fs::create_dir_all(editor.join("Data").join("Lua").join("TriggerEditor")).unwrap();

        assert!(validate_editor_path(&editor));

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn validate_editor_path_false_when_subfolder_missing() {
        let base = unique_temp_dir("valid-bad");
        let editor = base.join("NotTheEditor");
        fs::create_dir_all(&editor).unwrap();
        // No Data\Lua\TriggerEditor under it.
        assert!(!validate_editor_path(&editor));
        // A path that does not exist at all is also invalid.
        assert!(!validate_editor_path(&base.join("missing")));

        fs::remove_dir_all(&base).ok();
    }
}
