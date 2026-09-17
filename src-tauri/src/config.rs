//! Native runtime configuration and app data-directory resolution.
//!
//! - Roaming `%APPDATA%/eud-agent`: config, journals, sessions, backups, and
//!   explicit legacy-import sources.
//! - Local `%LOCALAPPDATA%/eud-agent`: models, RAG, logs, attachments, analyzer
//!   mirrors, media tools, and native compatibility assets.
//! - Canonical project memory/wiki state stays in `<project>/.eud-agent/memory`.
//!
//! `config.json` is atomic UTF-8 without BOM. Large/regenerable assets never
//! live in Roaming.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const APP_DIR_NAME: &str = "eud-agent";
const CONFIG_FILE_NAME: &str = "config.json";
pub const MAX_RECENT_PROJECTS: usize = 20;

/// Serializes project selection/history read-modify-write operations across managers.
pub(crate) static PROJECT_CONFIG_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// One durable project-launch history entry. Availability is probed from the
/// canonical project root when the launcher asks for its list and is therefore
/// intentionally not persisted here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentProjectRecord {
    pub name: String,
    pub path: String,
    pub last_opened_at: u64,
}

impl RecentProjectRecord {
    pub fn key(path: &str) -> String {
        let normalized = path.replace('\\', "/").to_lowercase();
        let normalized = if let Some(unc) = normalized.strip_prefix("//?/unc/") {
            format!("//{unc}")
        } else {
            normalized
                .strip_prefix("//?/")
                .unwrap_or(&normalized)
                .to_string()
        };
        normalized.trim_end_matches('/').to_string()
    }
}

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

fn notification_enabled_by_default() -> bool {
    true
}

/// Delivery channels for one user-attention event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationChannelSettings {
    #[serde(default = "notification_enabled_by_default")]
    pub sound: bool,
    #[serde(default = "notification_enabled_by_default")]
    pub os_notification: bool,
}

impl Default for NotificationChannelSettings {
    fn default() -> Self {
        Self {
            sound: true,
            os_notification: true,
        }
    }
}

/// Persisted attention-notification preferences.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationSettings {
    #[serde(default)]
    pub plan_approval: NotificationChannelSettings,
    #[serde(default)]
    pub changeset_review: NotificationChannelSettings,
    #[serde(default)]
    pub agent_turn_complete: NotificationChannelSettings,
    #[serde(default)]
    pub ask_response_required: NotificationChannelSettings,
}

pub const CONFIG_SCHEMA_VERSION: u32 = 3;

const fn config_schema_version() -> u32 {
    CONFIG_SCHEMA_VERSION
}

/// Secret-free `config.json` authority. Credentials live in provider-owned stores.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "config_schema_version")]
    pub schema_version: u32,
    /// Absolute root of the active native EUD project (containing its `.eap` manifest).
    #[serde(default)]
    pub project_path: String,
    /// `euddraft.exe`, `euddraft.py`, or an euddraft source-repository root.
    #[serde(default)]
    pub euddraft_path: String,
    /// Optional StarCraft install root used for map rendering/catalog assets.
    #[serde(default)]
    pub starcraft_path: String,
    #[serde(default)]
    pub default_provider: Option<crate::provider::ProviderId>,
    #[serde(default)]
    pub providers: crate::provider::ProviderSettings,
    #[serde(default)]
    pub notifications: NotificationSettings,
    #[serde(default)]
    pub model: AssetSpec,
    #[serde(default)]
    pub rag_index: AssetSpec,
    /// Recently opened native project roots, newest first.
    #[serde(default)]
    pub project_recents: Vec<RecentProjectRecord>,
    /// Distinguishes first-run migration from an intentionally emptied history.
    #[serde(default)]
    pub project_recents_initialized: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: CONFIG_SCHEMA_VERSION,
            project_path: String::new(),
            euddraft_path: String::new(),
            starcraft_path: String::new(),
            default_provider: None,
            providers: crate::provider::ProviderSettings::default(),
            notifications: NotificationSettings::default(),
            model: AssetSpec::default(),
            rag_index: AssetSpec::default(),
            project_recents: Vec::new(),
            project_recents_initialized: false,
        }
    }
}

impl Config {
    /// Add an explicitly opened project to bounded, newest-first history.
    pub fn record_recent_project(&mut self, name: String, path: String, now: u64) {
        let key = RecentProjectRecord::key(&path);
        self.project_recents
            .retain(|entry| RecentProjectRecord::key(&entry.path) != key);
        let newest = self
            .project_recents
            .iter()
            .map(|entry| entry.last_opened_at)
            .max()
            .unwrap_or_default();
        self.project_recents.insert(
            0,
            RecentProjectRecord {
                name,
                path,
                last_opened_at: now.max(newest.saturating_add(1)),
            },
        );
        self.project_recents.truncate(MAX_RECENT_PROJECTS);
        self.project_recents_initialized = true;
    }

    /// Canonicalize duplicate aliases while preserving the newest record.
    pub fn normalize_recent_projects(&mut self) -> bool {
        let before = self.project_recents.clone();
        self.project_recents
            .sort_by(|left, right| right.last_opened_at.cmp(&left.last_opened_at));
        let mut seen = BTreeSet::new();
        self.project_recents
            .retain(|entry| seen.insert(RecentProjectRecord::key(&entry.path)));
        self.project_recents.truncate(MAX_RECENT_PROJECTS);
        self.project_recents != before
    }

    pub fn remove_recent_project(&mut self, path: &str) {
        let key = RecentProjectRecord::key(path);
        self.project_recents
            .retain(|entry| RecentProjectRecord::key(&entry.path) != key);
        self.project_recents_initialized = true;
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfigV1 {
    #[serde(default, rename = "editor_path")]
    _legacy_editor_root: String,
    #[serde(default)]
    codex_cmd: Option<String>,
    #[serde(default)]
    codex_model: Option<String>,
    #[serde(default)]
    codex_reasoning_effort: Option<String>,
    #[serde(default)]
    codex_large_context_models: BTreeSet<String>,
    #[serde(default)]
    notifications: NotificationSettings,
    #[serde(default)]
    model: AssetSpec,
    #[serde(default)]
    rag_index: AssetSpec,
}

impl RawConfigV1 {
    fn migrate(self) -> Config {
        Config {
            default_provider: Some(crate::provider::ProviderId::Codex),
            providers: crate::provider::ProviderSettings {
                codex: crate::provider::CodexProviderSettings {
                    executable_override: self.codex_cmd,
                    default_model: self.codex_model,
                    default_reasoning: self
                        .codex_reasoning_effort
                        .map(|level| crate::provider::ReasoningSelection { level }),
                    large_context_models: self.codex_large_context_models,
                },
                ..Default::default()
            },
            notifications: self.notifications,
            model: self.model,
            rag_index: self.rag_index,
            ..Config::default()
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfigV2 {
    schema_version: u32,
    #[serde(default, rename = "editor_path")]
    _legacy_editor_root: String,
    #[serde(default)]
    default_provider: Option<crate::provider::ProviderId>,
    #[serde(default)]
    providers: crate::provider::ProviderSettings,
    #[serde(default)]
    notifications: NotificationSettings,
    #[serde(default)]
    model: AssetSpec,
    #[serde(default)]
    rag_index: AssetSpec,
}

impl RawConfigV2 {
    fn migrate(self) -> Config {
        debug_assert_eq!(self.schema_version, 2);
        Config {
            default_provider: self.default_provider,
            providers: self.providers,
            notifications: self.notifications,
            model: self.model,
            rag_index: self.rag_index,
            ..Config::default()
        }
    }
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
        // Repeatable debug QA must not depend on platform AppData environment handling.
        #[cfg(debug_assertions)]
        if let Some(root) = std::env::var_os("EUD_AGENT_TEST_DATA_ROOT") {
            return Ok(Self::from_test_data_root(Path::new(&root))?);
        }
        let roaming = manager.path().data_dir()?;
        let local = manager.path().local_data_dir()?;
        Ok(Self::from_bases(&roaming, &local))
    }

    /// Debug QA stores app data under `<root>/roaming` and `<root>/local`.
    /// Invalid explicit roots fail closed before resolving any user directories.
    #[cfg(debug_assertions)]
    fn from_test_data_root(root: &Path) -> std::io::Result<Self> {
        if !root.is_absolute() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "EUD_AGENT_TEST_DATA_ROOT must be a nonempty absolute path",
            ));
        }
        Ok(Self::from_bases(&root.join("roaming"), &root.join("local")))
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

    /// `%appdata%\eud-agent\memory` legacy-import source root.
    ///
    /// Live project memory is resolved from the validated native project root;
    /// this path is retained solely for explicit migration/import operations.
    pub fn memory_dir(&self) -> PathBuf {
        self.app_data.join("memory")
    }

    /// `%appdata%\eud-agent\workspaces` — preserved legacy import sources.
    /// Accepted documents live under the selected project's `.eud-agent/workspace`;
    /// the provider CLI cwd is the project root itself, so there are no
    /// machine-local session working roots anymore.
    pub fn workspaces_dir(&self) -> PathBuf {
        self.app_data.join("workspaces")
    }

    /// Parent-owned turn baselines and preserved legacy trusted-state sources.
    /// Live approval metadata is in the project's `.eud-agent/state`, outside
    /// the session Codex cwd and its writable document tree.
    pub fn workspace_state_dir(&self) -> PathBuf {
        self.workspaces_dir().join(".state")
    }

    /// `%appdata%\eud-agent\map_backups`.
    pub fn map_backups_dir(&self) -> PathBuf {
        self.app_data.join("map_backups")
    }
    /// `%appdata%\eud-agent\map_candidates` — isolated candidate documents.
    pub fn map_candidates_dir(&self) -> PathBuf {
        self.app_data.join("map_candidates")
    }

    /// `%localappdata%\eud-agent\map_imports` — content-addressed external map blobs.
    pub fn map_imports_dir(&self) -> PathBuf {
        self.app_local_data.join("map_imports")
    }

    /// `<native-root>\.eud-agent\memory\wiki` for the currently configured
    /// native project. The supplied project name must match the validated
    /// manifest; this prevents a stale caller from selecting another
    /// same-name project. `None` means no validated current project.
    pub fn wiki_dir(&self, project: &str) -> Option<PathBuf> {
        if project.trim().is_empty() {
            return None;
        }
        let configured = self.load_config().ok()?.project_path;
        if configured.trim().is_empty() {
            return None;
        }
        let native =
            crate::native_project::NativeProject::open(Path::new(configured.trim())).ok()?;
        (native.manifest().name == project)
            .then(|| native.root().join(".eud-agent").join("memory").join("wiki"))
    }

    /// `%appdata%\eud-agent\journal`.
    pub fn journal_dir(&self) -> PathBuf {
        self.app_data.join("journal")
    }

    /// `%appdata%\eud-agent\sessions` — named conversation records (Roaming so they
    /// survive a self-update, decision D). Small + user-owned.
    pub fn sessions_dir(&self) -> PathBuf {
        self.app_data.join("sessions")
    }

    /// `%appdata%\eud-agent\harness_jobs` — durable post-acceptance sync jobs.
    pub fn harness_jobs_dir(&self) -> PathBuf {
        self.app_data.join("harness_jobs")
    }
    /// Crash-safe normalized transcripts for direct providers.
    pub fn provider_sessions_dir(&self) -> PathBuf {
        self.app_data.join("provider-sessions")
    }

    /// `%localappdata%\eud-agent\models` — NEVER in Roaming (the model is ~570MB).
    pub fn models_dir(&self) -> PathBuf {
        self.app_local_data.join("models")
    }

    /// `%localappdata%\eud-agent\rag`.
    pub fn rag_dir(&self) -> PathBuf {
        self.app_local_data.join("rag")
    }

    /// `%localappdata%\eud-agent\euddraft` — complete managed release distributions.
    pub fn euddraft_dir(&self) -> PathBuf {
        self.app_local_data.join("euddraft")
    }

    /// `%localappdata%\eud-agent\uv` — versioned, checksum-pinned uv distributions.
    pub fn uv_dir(&self) -> PathBuf {
        self.app_local_data.join("uv")
    }

    /// One immutable managed uv distribution below [`Self::uv_dir`].
    pub fn managed_uv_dir(&self, version: &str) -> PathBuf {
        self.uv_dir().join(version)
    }

    /// `%localappdata%\eud-agent\python-downloads` — content-addressed wheel files.
    pub fn python_downloads_dir(&self) -> PathBuf {
        self.app_local_data.join("python-downloads")
    }

    /// `%localappdata%\eud-agent\python-envs` — immutable, verified project environments.
    pub fn python_envs_dir(&self) -> PathBuf {
        self.app_local_data.join("python-envs")
    }

    /// `%localappdata%\eud-agent\bin` — app-installed executables (the codex
    /// standalone binary). NEVER in Roaming; resolved by [`resolve_codex_cmd`].
    pub fn bin_dir(&self) -> PathBuf {
        self.app_local_data.join("bin")
    }
    pub fn providers_dir(&self) -> PathBuf {
        self.app_local_data.join("providers")
    }

    pub fn provider_dir(&self, provider: crate::provider::ProviderId) -> PathBuf {
        self.providers_dir().join(provider.to_string())
    }

    pub fn codex_bin_dir(&self) -> PathBuf {
        self.provider_dir(crate::provider::ProviderId::Codex)
            .join("bin")
    }

    pub fn codex_home_dir(&self) -> PathBuf {
        self.provider_dir(crate::provider::ProviderId::Codex)
            .join("home")
    }

    pub fn claude_bin_dir(&self) -> PathBuf {
        self.provider_dir(crate::provider::ProviderId::ClaudeCode)
            .join("bin")
    }

    pub fn claude_config_dir(&self) -> PathBuf {
        self.provider_dir(crate::provider::ProviderId::ClaudeCode)
            .join("config")
    }

    pub fn provider_cache_dir(&self, provider: crate::provider::ProviderId) -> PathBuf {
        self.provider_dir(provider).join("cache")
    }

    /// `%localappdata%\eud-agent\logs`.
    pub fn logs_dir(&self) -> PathBuf {
        self.app_local_data.join("logs")
    }

    /// `%localappdata%\eud-agent\attachments` — session-owned images and text/code
    /// files. Attachments can be large and therefore never live in Roaming.
    pub fn attachments_dir(&self) -> PathBuf {
        self.app_local_data.join("attachments")
    }
    /// `%localappdata%\eud-agent\audio_temp` — request-owned normalized audio.
    pub fn audio_temp_dir(&self) -> PathBuf {
        self.app_local_data.join("audio_temp")
    }
    /// `%localappdata%\eud-agent\audio_sources` — immutable original audio blobs
    /// and per-project managed-sound edit records.
    pub fn audio_sources_dir(&self) -> PathBuf {
        self.app_local_data.join("audio_sources")
    }
    /// `%localappdata%\eud-agent\native_assets` — versioned DAT/offset/TBL compatibility data.
    pub fn native_assets_dir(&self) -> PathBuf {
        self.app_local_data.join("native_assets")
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
            self.workspaces_dir(),
            self.workspace_state_dir(),
            self.map_backups_dir(),
            self.map_candidates_dir(),
            self.journal_dir(),
            self.sessions_dir(),
            self.harness_jobs_dir(),
            self.provider_sessions_dir(),
            self.app_local_data.clone(),
            self.models_dir(),
            self.rag_dir(),
            self.euddraft_dir(),
            self.uv_dir(),
            self.python_downloads_dir(),
            self.python_envs_dir(),
            self.bin_dir(),
            self.providers_dir(),
            self.codex_bin_dir(),
            self.codex_home_dir(),
            self.claude_bin_dir(),
            self.claude_config_dir(),
            self.provider_cache_dir(crate::provider::ProviderId::Antigravity),
            self.provider_cache_dir(crate::provider::ProviderId::OpencodeGo),
            self.logs_dir(),
            self.attachments_dir(),
            self.audio_temp_dir(),
            self.audio_sources_dir(),
            self.map_imports_dir(),
            self.native_assets_dir(),
            self.codex_workspace_dir(),
        ] {
            fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    /// Load current config or atomically migrate a legacy Codex-only config.
    pub fn load_config(&self) -> anyhow::Result<Config> {
        let path = self.config_path();
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Config::default())
            }
            Err(error) => return Err(error.into()),
        };
        let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes);
        let value: serde_json::Value = serde_json::from_slice(bytes)?;
        let object = value
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("config root must be an object"))?;

        if object.is_empty() {
            return Ok(Config::default());
        }

        if let Some(raw_version) = object.get("schema_version") {
            let version = raw_version
                .as_u64()
                .ok_or_else(|| anyhow::anyhow!("config schema version must be an integer"))?;
            if version == u64::from(CONFIG_SCHEMA_VERSION) {
                return Ok(serde_json::from_value(value)?);
            }
            if version == 2 {
                let migrated = serde_json::from_value::<RawConfigV2>(value)?.migrate();
                self.save_config(&migrated)?;
                return Ok(migrated);
            }
            anyhow::bail!("unsupported config schema version {version}");
        }

        let legacy: RawConfigV1 = serde_json::from_value(value)?;
        let migrated = legacy.migrate();
        self.save_config(&migrated)?;
        Ok(migrated)
    }

    /// Serialize and atomically write secret-free config as UTF-8 without BOM.
    pub fn save_config(&self, config: &Config) -> anyhow::Result<()> {
        if config.schema_version != CONFIG_SCHEMA_VERSION {
            anyhow::bail!(
                "refusing to save unsupported config schema version {}",
                config.schema_version
            );
        }
        self.ensure_dirs()?;
        let json = serde_json::to_vec_pretty(config)?;
        crate::memory::write_atomic_bytes(&self.config_path(), &json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    #[cfg(debug_assertions)]
    #[test]
    fn test_data_dirs_use_isolated_bases_when_root_is_absolute() {
        // Given an absolute root without creating directories or changing global environment.
        let root = std::env::temp_dir().join("eud-agent-isolated-data-root");

        // When debug QA resolves that root.
        let dirs = DataDirs::from_test_data_root(&root).unwrap();

        // Then both app data bases are confined to the supplied root.
        assert_eq!(dirs.app_data(), root.join("roaming").join("eud-agent"));
        assert_eq!(dirs.app_local_data(), root.join("local").join("eud-agent"));
    }

    #[cfg(debug_assertions)]
    #[test]
    fn test_data_dirs_reject_relative_root_without_falling_back() {
        // Given a relative root.
        let root = Path::new("isolated-data");

        // When debug QA resolves that root.
        let error = DataDirs::from_test_data_root(root).unwrap_err();

        // Then resolution fails instead of returning user data directories.
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn test_data_dirs_reject_empty_root_without_falling_back() {
        // Given an explicitly empty root.
        let root = Path::new("");

        // When debug QA resolves that root.
        let error = DataDirs::from_test_data_root(root).unwrap_err();

        // Then resolution fails instead of returning user data directories.
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }

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
            project_path: "C:\\Maps\\NativeProject".to_string(),
            euddraft_path: "C:\\Tools\\euddraft.exe".to_string(),
            starcraft_path: "C:\\Games\\StarCraft".to_string(),
            default_provider: Some(crate::provider::ProviderId::Codex),
            providers: crate::provider::ProviderSettings {
                codex: crate::provider::CodexProviderSettings {
                    executable_override: Some("C:\\tools\\codex.exe".to_string()),
                    default_model: Some("gpt-5.5-codex".to_string()),
                    default_reasoning: Some(crate::provider::ReasoningSelection {
                        level: "high".to_string(),
                    }),
                    large_context_models: BTreeSet::from(["gpt-5.5-codex".to_string()]),
                },
                ..Default::default()
            },
            notifications: NotificationSettings::default(),
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
            ..Config::default()
        };

        let json = serde_json::to_string(&cfg).unwrap();
        let back: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn partial_config_deserializes_with_fresh_provider_selection() {
        let back: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(back.schema_version, CONFIG_SCHEMA_VERSION);
        assert_eq!(back.project_path, "");
        assert_eq!(back.euddraft_path, "");
        assert_eq!(back.default_provider, None);
        assert!(back.providers.codex.large_context_models.is_empty());
        assert_eq!(back.model, AssetSpec::default());
        assert_eq!(back.notifications, NotificationSettings::default());
    }

    #[test]
    fn existing_v2_provider_settings_gain_the_default_ollama_endpoint() {
        let mut value = serde_json::to_value(Config::default()).unwrap();
        value["providers"].as_object_mut().unwrap().remove("ollama");
        let config: Config = serde_json::from_value(value).unwrap();
        assert_eq!(
            config.providers.ollama.base_url,
            crate::provider::DEFAULT_OLLAMA_BASE_URL
        );
        assert_eq!(config.providers.ollama.default_model, None);
    }

    #[test]
    fn partial_notification_settings_keep_unspecified_channels_enabled() {
        let back: Config =
            serde_json::from_str(r#"{"notifications":{"planApproval":{"sound":false}}}"#).unwrap();
        assert!(!back.notifications.plan_approval.sound);
        assert!(back.notifications.plan_approval.os_notification);
        assert_eq!(
            back.notifications.changeset_review,
            NotificationChannelSettings::default()
        );
        assert_eq!(
            back.notifications.agent_turn_complete,
            NotificationChannelSettings::default()
        );
        assert_eq!(
            back.notifications.ask_response_required,
            NotificationChannelSettings::default()
        );
    }

    #[test]
    fn config_load_save_round_trips_on_disk() {
        let base = unique_temp_dir("loadsave");
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        dirs.ensure_dirs().unwrap();

        let cfg = Config {
            project_path: "C:\\Maps\\NativeProject".to_string(),
            euddraft_path: "C:\\Tools\\euddraft.exe".to_string(),
            ..Default::default()
        };
        dirs.save_config(&cfg).unwrap();

        let loaded = dirs.load_config().unwrap();
        assert_eq!(cfg, loaded);

        fs::remove_dir_all(&base).ok();
    }
    #[test]
    fn legacy_codex_config_migrates_once_without_alias_fields() {
        let base = unique_temp_dir("provider-migration");
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        dirs.ensure_dirs().unwrap();
        fs::write(
            dirs.config_path(),
            br#"{
                "editor_path": "C:\\Editor",
                "codex_cmd": "C:\\tools\\codex.exe",
                "codex_model": "gpt-legacy",
                "codex_reasoning_effort": "high",
                "codex_large_context_models": ["gpt-legacy"]
            }"#,
        )
        .unwrap();

        let migrated = dirs.load_config().unwrap();
        assert_eq!(
            migrated.default_provider,
            Some(crate::provider::ProviderId::Codex)
        );
        assert_eq!(
            migrated.providers.codex.default_model.as_deref(),
            Some("gpt-legacy")
        );
        assert_eq!(
            migrated
                .providers
                .codex
                .default_reasoning
                .as_ref()
                .map(|selection| selection.level.as_str()),
            Some("high")
        );
        let saved: serde_json::Value =
            serde_json::from_slice(&fs::read(dirs.config_path()).unwrap()).unwrap();
        assert_eq!(saved["schema_version"], CONFIG_SCHEMA_VERSION);
        assert!(saved.get("codex_model").is_none());
        assert!(saved.get("codex_cmd").is_none());
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn v2_provider_config_migrates_to_native_paths_without_losing_provider_defaults() {
        let base = unique_temp_dir("native-path-migration");
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        dirs.ensure_dirs().unwrap();
        let mut legacy = serde_json::to_value(Config {
            default_provider: Some(crate::provider::ProviderId::ClaudeCode),
            providers: crate::provider::ProviderSettings {
                claude_code: crate::provider::ClaudeCodeProviderSettings {
                    default_model: Some("claude-test".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Config::default()
        })
        .unwrap();
        let object = legacy.as_object_mut().unwrap();
        object.insert("schema_version".to_string(), serde_json::json!(2));
        object.insert(
            "editor_path".to_string(),
            serde_json::json!("C:\\LegacyEditor"),
        );
        object.remove("project_path");
        object.remove("euddraft_path");
        object.remove("starcraft_path");
        object.remove("project_recents");
        object.remove("project_recents_initialized");
        fs::write(dirs.config_path(), serde_json::to_vec(&legacy).unwrap()).unwrap();

        let migrated = dirs.load_config().unwrap();
        assert_eq!(migrated.schema_version, CONFIG_SCHEMA_VERSION);
        assert!(migrated.project_path.is_empty());
        assert!(migrated.euddraft_path.is_empty());
        assert_eq!(
            migrated.default_provider,
            Some(crate::provider::ProviderId::ClaudeCode)
        );
        assert_eq!(
            migrated.providers.claude_code.default_model.as_deref(),
            Some("claude-test")
        );
        let saved: serde_json::Value =
            serde_json::from_slice(&fs::read(dirs.config_path()).unwrap()).unwrap();
        assert_eq!(saved["schema_version"], CONFIG_SCHEMA_VERSION);
        assert!(saved.get("editor_path").is_none());
        fs::remove_dir_all(base).ok();
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
        assert!(dirs.sessions_dir().is_dir());
        assert!(dirs.harness_jobs_dir().is_dir());
        // Local subtree — model/rag/logs NEVER in Roaming.
        assert!(dirs.app_local_data().is_dir());
        assert!(dirs.models_dir().is_dir());
        assert!(dirs.rag_dir().is_dir());
        assert!(dirs.euddraft_dir().is_dir());
        assert!(dirs.uv_dir().is_dir());
        assert!(dirs.python_downloads_dir().is_dir());
        assert!(dirs.python_envs_dir().is_dir());
        assert!(dirs.logs_dir().is_dir());
        assert!(dirs.attachments_dir().is_dir());
        assert!(dirs.audio_sources_dir().is_dir());
        assert!(dirs.map_imports_dir().is_dir());

        assert!(dirs.workspaces_dir().is_dir());
        assert!(dirs.workspace_state_dir().is_dir());
        // The model dir must live under local, not roaming.
        assert!(dirs.models_dir().starts_with(dirs.app_local_data()));
        assert!(!dirs.models_dir().starts_with(dirs.app_data()));
        assert!(dirs.attachments_dir().starts_with(dirs.app_local_data()));
        assert!(dirs.audio_sources_dir().starts_with(dirs.app_local_data()));
        assert!(dirs.map_imports_dir().starts_with(dirs.app_local_data()));
        assert!(!dirs.map_imports_dir().starts_with(dirs.app_data()));
        assert!(!dirs.attachments_dir().starts_with(dirs.app_data()));
        assert!(dirs.workspaces_dir().starts_with(dirs.app_data()));
        assert!(!dirs.workspaces_dir().starts_with(dirs.app_local_data()));

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn recent_projects_dedupe_aliases_using_newest_timestamp() {
        let mut config = Config {
            project_recents: vec![
                RecentProjectRecord {
                    name: "old".to_string(),
                    path: "C:\\Work\\Demo".to_string(),
                    last_opened_at: 10,
                },
                RecentProjectRecord {
                    name: "new".to_string(),
                    path: "c:/work/demo".to_string(),
                    last_opened_at: 20,
                },
            ],
            ..Config::default()
        };
        assert!(config.normalize_recent_projects());
        assert_eq!(config.project_recents.len(), 1);
        assert_eq!(config.project_recents[0].name, "new");
        config.remove_recent_project("C:\\WORK\\DEMO");
        assert!(config.project_recents.is_empty());
        assert!(config.project_recents_initialized);
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
        assert_eq!(
            dirs.map_imports_dir(),
            PathBuf::from("C:\\loc\\eud-agent\\map_imports")
        );
        assert_eq!(
            dirs.managed_uv_dir("0.11.3"),
            PathBuf::from("C:\\loc\\eud-agent\\uv\\0.11.3")
        );
        assert_eq!(
            dirs.python_downloads_dir(),
            PathBuf::from("C:\\loc\\eud-agent\\python-downloads")
        );
        assert_eq!(
            dirs.python_envs_dir(),
            PathBuf::from("C:\\loc\\eud-agent\\python-envs")
        );
    }

    #[test]
    fn wiki_dir_requires_validated_current_project() {
        let dirs = DataDirs::from_bases(&PathBuf::from("C:\\roam"), &PathBuf::from("C:\\loc"));
        assert_eq!(dirs.wiki_dir("My<Project>"), None);
        assert_eq!(dirs.wiki_dir("   "), None);
        assert_eq!(dirs.wiki_dir(""), None);
    }
}
