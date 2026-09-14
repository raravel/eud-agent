//! First-run bootstrap: manifest check + sha256-verified, atomic asset download.
//!
//! Two assets are installed on first run (Decision 12, feature 10):
//! - the bge-m3 ONNX embedding model, fetched via fastembed's HuggingFace cache
//!   (cache dir pointed at `DataDirs::models_dir()`),
//! - the RAG index, a direct `reqwest` GET of a versioned GitHub Release asset placed
//!   under `DataDirs::rag_dir()`.
//!
//! Both live under `%localappdata%\eud-agent\` — NEVER Roaming (the model is ~570MB).
//!
//! Every asset is sha256-verified against its [`AssetSpec`] BEFORE it is placed, and
//! placement is atomic: download to `<final>.tmp`, verify, then `std::fs::rename` over the
//! final path. A sha256 mismatch refuses to install (the tmp is deleted, the final path is
//! never touched). A missing or corrupt asset triggers a re-download.
//!
//! The network-free verify/place/status logic is split from the actual download so it is
//! unit-testable with local fixtures (no real network). Progress is emitted through an
//! injected [`ProgressEmitter`]: prod uses Tauri's `AppHandle::emit`; tests use a recording
//! double.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use anyhow::{bail, Context};
use sha2::{Digest, Sha256};

use crate::config::{AssetSpec, DataDirs};

#[path = "process_tree.rs"]
pub mod process_tree;

/// The RAG index is stored under `rag/` with this fixed basename (the GitHub Release asset
/// is downloaded to it after sha256 verification).
pub const RAG_INDEX_FILENAME: &str = "rag-index.bin";

/// The RAG release generation the runtime requires. The persisted binary layout remains
/// v2 (`rag.rs::INDEX_VERSION`); release v4 carries the refreshed nine-source corpus.
/// Bumping this generation forces healthy v3 installations to fetch the v4 manifest and
/// atomically replace their otherwise-valid old index.
pub const REQUIRED_RAG_INDEX_VERSION: &str = "4";

/// HF model id installed on first run when `config.json` carries none (feature 10).
pub const DEFAULT_MODEL_NAME: &str = "BAAI/bge-m3";

/// Serialize every in-process initialization of the shared fastembed model cache.
///
/// First-run bootstrap and background RAG warmup can otherwise call `hf-hub` for the
/// same blob concurrently. Its Windows file lock waits only five seconds, so the later
/// caller fails while the first is still downloading the ~570 MB model.
pub(crate) fn lock_model_initialization() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Published release manifest for the RAG index, uploaded next to `rag-index.bin`
/// by `.github/workflows/build-rag-index.yml` (`{"rag_index":{url,sha256,version}}`).
/// Fetched when `config.json` has no pinned spec yet (first run); the sha256 inside
/// pins the asset bytes that `verify_and_place` enforces.
///
/// Resolved against the **dedicated RAG release tag** `rag-index-v<version>`, NOT
/// `releases/latest`. The RAG index ships on its own tag, decoupled from the app-binary
/// release (`v*` / the updater's `releases/latest`): `releases/latest` tracks the newest
/// release overall, so once an app-binary release is published it shadows the RAG release
/// and the manifest 404s. Pinning the URL to the required index version keeps the two
/// distributions independent and the URL in lock-step with [`REQUIRED_RAG_INDEX_VERSION`].
pub fn rag_manifest_url() -> String {
    format!(
        "https://github.com/raravel/eud-agent/releases/download/rag-index-v{REQUIRED_RAG_INDEX_VERSION}/rag-index.manifest.json"
    )
}

/// On-disk state of an asset relative to its expected [`AssetSpec`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetStatus {
    /// Present and the sha256 matches the spec — no download needed.
    Present,
    /// The file is absent — needs download.
    Missing,
    /// The file exists but its sha256 does not match — needs re-download.
    Corrupt,
}

impl AssetStatus {
    /// True when the asset must be (re)downloaded (`Missing` or `Corrupt`).
    pub fn needs_download(self) -> bool {
        !matches!(self, AssetStatus::Present)
    }
}

/// Sink for `progress {stage: bootstrap, pct, detail}` events.
///
/// Injected so the download flow is testable without a running Tauri app: prod wraps
/// `AppHandle::emit` ([`TauriEmitter`]); tests use a recording double.
pub trait ProgressEmitter {
    /// Report progress for `stage` at `pct` (0..=100) with a human-readable `detail`.
    fn emit(&self, stage: &str, pct: u8, detail: &str);
}

/// Lowercase-hex sha256 of `bytes`.
pub fn sha256_hex_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex_lower(&hasher.finalize())
}

/// Lowercase-hex sha256 of the file at `path`, hashed in chunks (the model is hundreds of
/// MB; never read the whole file into memory).
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex_lower(&hasher.finalize()))
}

/// Render a digest as lowercase hex.
fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Classify the asset stored at `dir/filename` against `spec`.
///
/// `Missing` when the file is absent (also when it cannot be read), `Present` when its
/// sha256 matches `spec.sha256` (case-insensitively), `Corrupt` otherwise. A `Corrupt`
/// or `Missing` asset is re-downloaded by the ensure-* wrappers.
pub fn asset_status(dir: &Path, filename: &str, spec: &AssetSpec) -> AssetStatus {
    let path = dir.join(filename);
    if !path.is_file() {
        return AssetStatus::Missing;
    }
    match sha256_file(&path) {
        Ok(actual) if actual.eq_ignore_ascii_case(&spec.sha256) => AssetStatus::Present,
        // Unreadable file -> treat as Missing so the caller re-downloads.
        Err(_) => AssetStatus::Missing,
        _ => AssetStatus::Corrupt,
    }
}

/// Verify `tmp` against `expected_sha` without changing the final path.
///
/// A failure removes the staged file so an unverified download can never be placed later.
fn verify_downloaded_tmp(tmp: &Path, final_path: &Path, expected_sha: &str) -> anyhow::Result<()> {
    let actual = match sha256_file(tmp) {
        Ok(hash) => hash,
        Err(error) => {
            let _ = fs::remove_file(tmp);
            return Err(anyhow::Error::new(error)
                .context(format!("cannot hash downloaded tmp {}", tmp.display())));
        }
    };

    if !actual.eq_ignore_ascii_case(expected_sha) {
        let _ = fs::remove_file(tmp);
        bail!(
            "sha256 mismatch for {}: expected {}, got {} — refusing to install",
            final_path.display(),
            expected_sha,
            actual
        );
    }

    Ok(())
}

/// Atomically place a file already accepted by [`verify_downloaded_tmp`].
fn place_verified_tmp(tmp: &Path, final_path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = final_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("cannot create dir {}", parent.display()))?;
    }
    fs::rename(tmp, final_path)
        .with_context(|| format!("cannot place {} -> {}", tmp.display(), final_path.display()))
}

/// Verify `tmp` against `expected_sha`, then atomically rename it over `final_path`.
///
/// On a sha256 mismatch (or a missing/unreadable tmp) the tmp is removed and an error is
/// returned — the final path is NEVER written. This is the single chokepoint every
/// single-asset download funnels through.
pub fn verify_and_place(tmp: &Path, final_path: &Path, expected_sha: &str) -> anyhow::Result<()> {
    verify_downloaded_tmp(tmp, final_path, expected_sha)?;
    place_verified_tmp(tmp, final_path)
}

// ---------------------------------------------------------------------------------------
// Network wrappers (thin, NOT unit-tested — they hit HuggingFace / GitHub Releases).
// Each one funnels its bytes through `verify_and_place` so the verify/atomic-place logic
// stays covered by the unit tests above.
// ---------------------------------------------------------------------------------------

/// A [`ProgressEmitter`] backed by a Tauri `AppHandle`. Emits a `progress` event whose
/// payload is `{ stage, pct, detail }` (rules.md: panel↔core is Tauri IPC only).
pub struct TauriEmitter<R: tauri::Runtime>(pub tauri::AppHandle<R>);

impl<R: tauri::Runtime> ProgressEmitter for TauriEmitter<R> {
    fn emit(&self, stage: &str, pct: u8, detail: &str) {
        use tauri::Emitter;
        // A dropped event must never break the install; log-and-continue.
        let _ = self.0.emit(
            "progress",
            serde_json::json!({ "stage": stage, "pct": pct, "detail": detail }),
        );
    }
}

/// Ensure the RAG index is present and verified under `dirs.rag_dir()`.
///
/// No-op when already `Present`. Otherwise streams the GitHub Release asset
/// (`spec.name` = the asset URL) to `<final>.tmp`, emits byte-progress, then
/// `verify_and_place`s it. Returns the placed path.
pub async fn ensure_rag_index(
    dirs: &DataDirs,
    spec: &AssetSpec,
    emitter: &(dyn ProgressEmitter + Send + Sync),
) -> anyhow::Result<PathBuf> {
    let rag_dir = dirs.rag_dir();
    fs::create_dir_all(&rag_dir)
        .with_context(|| format!("cannot create rag dir {}", rag_dir.display()))?;
    let final_path = rag_dir.join(RAG_INDEX_FILENAME);

    if asset_status(&rag_dir, RAG_INDEX_FILENAME, spec) == AssetStatus::Present {
        emitter.emit("bootstrap", 100, "rag index already installed");
        return Ok(final_path);
    }

    let tmp = with_tmp_suffix(&final_path);
    // Clean any stale tmp from a previous aborted run before re-downloading.
    let _ = fs::remove_file(&tmp);

    emitter.emit("bootstrap", 0, "downloading rag index");
    download_to_tmp(&spec.name, &tmp, "rag index", emitter)
        .await
        .inspect_err(|_| {
            // Never leave a half-written tmp on a download failure.
            let _ = fs::remove_file(&tmp);
        })?;

    verify_and_place(&tmp, &final_path, &spec.sha256)?;
    emitter.emit("bootstrap", 100, "rag index installed");
    Ok(final_path)
}

/// Ensure the bge-m3 ONNX model is present in fastembed's HF cache under
/// `dirs.models_dir()`.
///
/// fastembed (via `hf-hub`) downloads atomically into its own cache layout and verifies
/// each file against the HF etag, so we delegate placement to it rather than re-implement
/// the multi-file fetch. This is a blocking call (ONNX runtime init + download); callers
/// run it on a blocking task. NOT unit-tested — it performs the real HF download.
pub fn ensure_model(dirs: &DataDirs, emitter: &dyn ProgressEmitter) -> anyhow::Result<()> {
    use fastembed::{Bgem3Embedding, Bgem3InitOptions, Bgem3Model};

    let models_dir = dirs.models_dir();
    fs::create_dir_all(&models_dir)
        .with_context(|| format!("cannot create models dir {}", models_dir.display()))?;

    emitter.emit("bootstrap", 0, "downloading bge-m3 model");
    let _model_guard = lock_model_initialization();
    // Point the HF cache at our Local data dir (never Roaming) and trigger the fetch.
    Bgem3Embedding::try_new(
        Bgem3InitOptions::new(Bgem3Model::BGEM3Q)
            .with_cache_dir(models_dir)
            .with_show_download_progress(true),
    )
    .context("fastembed bge-m3 model download/init failed")?;
    emitter.emit("bootstrap", 100, "bge-m3 model installed");
    Ok(())
}

pub const FFMPEG_MANIFEST_JSON: &str = include_str!("../../vendor/ffmpeg/manifest.json");

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedFfmpegArchive {
    pub url: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedFfmpegMember {
    pub name: String,
    pub archive_path: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedFfmpegManifest {
    pub schema: String,
    pub version: String,
    pub archive: ManagedFfmpegArchive,
    pub members: Vec<ManagedFfmpegMember>,
    pub configuration: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedFfmpegPaths {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    pub version: String,
}

pub fn managed_ffmpeg_manifest() -> anyhow::Result<ManagedFfmpegManifest> {
    let manifest: ManagedFfmpegManifest =
        serde_json::from_str(FFMPEG_MANIFEST_JSON).context("bundled FFmpeg manifest is invalid")?;
    if manifest.schema != "eud-managed-ffmpeg/1"
        || manifest.members.len() != 2
        || manifest.archive.bytes == 0
        || manifest.archive.sha256.len() != 64
        || !manifest
            .configuration
            .iter()
            .any(|item| item == "--enable-libvorbis")
    {
        bail!("bundled FFmpeg manifest violates the managed audio contract");
    }
    for name in ["ffmpeg.exe", "ffprobe.exe"] {
        let member = manifest
            .members
            .iter()
            .find(|member| member.name == name)
            .with_context(|| format!("bundled FFmpeg manifest is missing {name}"))?;
        if member.bytes == 0
            || member.sha256.len() != 64
            || member.archive_path.contains("..")
            || !member.archive_path.is_ascii()
        {
            bail!("bundled FFmpeg member metadata is invalid for {name}");
        }
    }
    Ok(manifest)
}

pub fn resolve_managed_ffmpeg(dirs: &DataDirs) -> anyhow::Result<ManagedFfmpegPaths> {
    let manifest = managed_ffmpeg_manifest()?;
    let member = |name: &str| {
        manifest
            .members
            .iter()
            .find(|member| member.name == name)
            .with_context(|| format!("managed FFmpeg manifest is missing {name}"))
    };
    let ffmpeg_member = member("ffmpeg.exe")?;
    let ffprobe_member = member("ffprobe.exe")?;
    let ffmpeg = dirs.bin_dir().join(&ffmpeg_member.name);
    let ffprobe = dirs.bin_dir().join(&ffprobe_member.name);
    for (path, expected) in [
        (&ffmpeg, ffmpeg_member.sha256.as_str()),
        (&ffprobe, ffprobe_member.sha256.as_str()),
    ] {
        let actual = sha256_file(path).with_context(|| {
            format!(
                "managed audio converter asset is unavailable: {}",
                path.display()
            )
        })?;
        if actual != expected {
            bail!("managed audio converter checksum mismatch");
        }
    }
    Ok(ManagedFfmpegPaths {
        ffmpeg,
        ffprobe,
        version: manifest.version,
    })
}

pub fn managed_ffmpeg_ready(dirs: &DataDirs) -> bool {
    resolve_managed_ffmpeg(dirs).is_ok()
}

pub async fn ensure_ffmpeg(
    dirs: &DataDirs,
    emitter: &(dyn ProgressEmitter + Send + Sync),
) -> anyhow::Result<ManagedFfmpegPaths> {
    if let Ok(paths) = resolve_managed_ffmpeg(dirs) {
        return Ok(paths);
    }
    let manifest = managed_ffmpeg_manifest()?;
    fs::create_dir_all(dirs.bin_dir())?;
    let archive_tmp = dirs.bin_dir().join("ffmpeg-distribution.zip.tmp");
    let _ = fs::remove_file(&archive_tmp);
    if let Err(error) = download_to_tmp(
        &manifest.archive.url,
        &archive_tmp,
        "FFmpeg/FFprobe",
        emitter,
    )
    .await
    {
        let _ = fs::remove_file(&archive_tmp);
        return Err(error);
    }
    verify_downloaded_tmp(&archive_tmp, &archive_tmp, &manifest.archive.sha256)?;
    let bin_dir = dirs.bin_dir();
    let extraction = tokio::task::spawn_blocking(move || {
        extract_managed_ffmpeg_archive(&archive_tmp, &bin_dir, &manifest)
    })
    .await
    .context("managed FFmpeg extraction task failed")?;
    extraction?;
    emitter.emit("bootstrap", 100, "audio converter ready");
    resolve_managed_ffmpeg(dirs)
}

fn extract_managed_ffmpeg_archive(
    archive_path: &Path,
    bin_dir: &Path,
    manifest: &ManagedFfmpegManifest,
) -> anyhow::Result<()> {
    let result = (|| {
        let archive_file = File::open(archive_path)?;
        let mut archive = zip::ZipArchive::new(archive_file)
            .context("managed FFmpeg archive is not a valid ZIP")?;
        let mut staged = Vec::with_capacity(manifest.members.len());
        for member in &manifest.members {
            let mut source = archive
                .by_name(&member.archive_path)
                .with_context(|| format!("managed FFmpeg archive is missing {}", member.name))?;
            if source.size() != member.bytes {
                bail!(
                    "managed FFmpeg archive member size mismatch for {}",
                    member.name
                );
            }
            let tmp = bin_dir.join(format!("{}.audio.tmp", member.name));
            let _ = fs::remove_file(&tmp);
            let mut output = File::create(&tmp)?;
            std::io::copy(&mut source, &mut output)?;
            output.flush()?;
            output.sync_all()?;
            verify_downloaded_tmp(&tmp, &bin_dir.join(&member.name), &member.sha256)?;
            staged.push((tmp, bin_dir.join(&member.name)));
        }

        let mut placed = Vec::new();
        for (tmp, final_path) in &staged {
            if final_path.exists() {
                fs::remove_file(final_path)?;
            }
            if let Err(error) = place_verified_tmp(tmp, final_path) {
                for path in placed {
                    let _ = fs::remove_file(path);
                }
                return Err(error);
            }
            placed.push(final_path.clone());
        }
        Ok(())
    })();
    let _ = fs::remove_file(archive_path);
    for member in &manifest.members {
        let _ = fs::remove_file(bin_dir.join(format!("{}.audio.tmp", member.name)));
    }
    result
}

/// The app-installed codex binary filename under [`DataDirs::bin_dir`].
pub const CODEX_BIN_FILENAME: &str = "codex.exe";

/// The Code Mode process host shipped alongside [`CODEX_BIN_FILENAME`].
///
/// Codex resolves this fixed sibling name when an `exec` tool call needs the V8-backed
/// Code Mode runtime. An app-managed Codex install is incomplete without it.
pub const CODEX_CODE_MODE_HOST_FILENAME: &str = "codex-code-mode-host.exe";

/// The elevated Windows sandbox installer shipped alongside [`CODEX_BIN_FILENAME`].
///
/// Codex launches this fixed sibling when the sandbox setup marker is absent or stale.
pub const CODEX_SANDBOX_SETUP_FILENAME: &str = "codex-windows-sandbox-setup.exe";

/// GitHub's metadata for the newest official Codex release. We resolve this once per
/// install so the CLI and both runtime helpers always come from the same concrete tag.
const CODEX_RELEASE_API_URL: &str = "https://api.github.com/repos/openai/codex/releases/latest";
const CODEX_RELEASE_EXE_ASSET_NAME: &str = "codex-x86_64-pc-windows-msvc.exe";
const CODEX_RELEASE_HOST_ASSET_NAME: &str = "codex-code-mode-host-x86_64-pc-windows-msvc.exe";
const CODEX_RELEASE_SANDBOX_SETUP_ASSET_NAME: &str =
    "codex-windows-sandbox-setup-x86_64-pc-windows-msvc.exe";

/// Sanity floor for each executable. The release digest is authoritative; this catches
/// malformed metadata before any large download starts.
const CODEX_MIN_BYTES: u64 = 1_000_000;

#[derive(Debug)]
struct CodexReleaseSpec {
    version: String,
    codex: AssetSpec,
    code_mode_host: AssetSpec,
    sandbox_setup: AssetSpec,
}

#[derive(serde::Deserialize)]
struct GitHubRelease {
    tag_name: String,
    assets: Vec<GitHubReleaseAsset>,
}

#[derive(serde::Deserialize)]
struct GitHubReleaseAsset {
    name: String,
    browser_download_url: String,
    digest: Option<String>,
    size: u64,
}

fn codex_release_asset(
    release: &GitHubRelease,
    asset_name: &str,
    version: &str,
) -> anyhow::Result<AssetSpec> {
    let asset = release
        .assets
        .iter()
        .find(|asset| asset.name == asset_name)
        .with_context(|| format!("Codex release {version} is missing asset {asset_name}"))?;
    if asset.browser_download_url.trim().is_empty() {
        bail!("Codex release {version} asset {asset_name} has no download URL");
    }
    if asset.size < CODEX_MIN_BYTES {
        bail!(
            "Codex release {version} asset {asset_name} is implausibly small ({} bytes)",
            asset.size
        );
    }

    let digest = asset
        .digest
        .as_deref()
        .and_then(|digest| digest.strip_prefix("sha256:"))
        .filter(|digest| digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .with_context(|| {
            format!("Codex release {version} asset {asset_name} has no valid sha256 digest")
        })?;

    Ok(AssetSpec {
        name: asset.browser_download_url.clone(),
        sha256: digest.to_ascii_lowercase(),
        version: version.to_string(),
    })
}

/// Parse the official GitHub release metadata into a version-locked CLI + runtime helpers.
fn parse_codex_release(bytes: &[u8]) -> anyhow::Result<CodexReleaseSpec> {
    let release: GitHubRelease =
        serde_json::from_slice(bytes).context("invalid Codex release metadata")?;
    let version = release.tag_name.trim();
    if version.is_empty() {
        bail!("Codex release metadata has no tag_name");
    }

    Ok(CodexReleaseSpec {
        version: version.to_string(),
        codex: codex_release_asset(&release, CODEX_RELEASE_EXE_ASSET_NAME, version)?,
        code_mode_host: codex_release_asset(&release, CODEX_RELEASE_HOST_ASSET_NAME, version)?,
        sandbox_setup: codex_release_asset(
            &release,
            CODEX_RELEASE_SANDBOX_SETUP_ASSET_NAME,
            version,
        )?,
    })
}

async fn fetch_codex_release() -> anyhow::Result<CodexReleaseSpec> {
    let bytes = reqwest::Client::builder()
        .user_agent("eud-agent-bootstrap")
        .build()?
        .get(CODEX_RELEASE_API_URL)
        .send()
        .await
        .context("failed to fetch latest Codex release metadata")?
        .error_for_status()
        .context("latest Codex release metadata returned an error status")?
        .bytes()
        .await
        .context("failed to read latest Codex release metadata")?;
    parse_codex_release(&bytes)
}

/// Download and install the latest version-matched Codex CLI and runtime helpers.
///
/// GitHub release metadata is resolved once, and all three official sha256 digests are
/// verified before any staged file is placed. Files already matching that concrete release
/// are retained, so an existing app-managed `codex.exe` downloads only missing siblings.
/// A placement failure removes newly placed distribution files so the setup gate cannot
/// mistake a mixed-version installation for ready.
pub async fn ensure_codex(
    dirs: &DataDirs,
    emitter: &(dyn ProgressEmitter + Send + Sync),
) -> anyhow::Result<PathBuf> {
    let bin_dir = dirs.codex_bin_dir();
    fs::create_dir_all(&bin_dir)
        .with_context(|| format!("cannot create bin dir {}", bin_dir.display()))?;
    let codex_path = bin_dir.join(CODEX_BIN_FILENAME);
    let host_path = bin_dir.join(CODEX_CODE_MODE_HOST_FILENAME);
    let sandbox_setup_path = bin_dir.join(CODEX_SANDBOX_SETUP_FILENAME);

    emitter.emit("codex_install", 0, "checking latest codex release");
    let release = fetch_codex_release().await?;
    let codex_needed =
        asset_status(&bin_dir, CODEX_BIN_FILENAME, &release.codex) != AssetStatus::Present;
    let host_needed = asset_status(
        &bin_dir,
        CODEX_CODE_MODE_HOST_FILENAME,
        &release.code_mode_host,
    ) != AssetStatus::Present;
    let sandbox_setup_needed = asset_status(
        &bin_dir,
        CODEX_SANDBOX_SETUP_FILENAME,
        &release.sandbox_setup,
    ) != AssetStatus::Present;

    if !codex_needed && !host_needed && !sandbox_setup_needed {
        emitter.emit("codex_install", 100, "codex already installed");
        return Ok(codex_path);
    }

    let codex_tmp = with_tmp_suffix(&codex_path);
    let host_tmp = with_tmp_suffix(&host_path);
    let sandbox_setup_tmp = with_tmp_suffix(&sandbox_setup_path);
    let cleanup_staged = || {
        let _ = fs::remove_file(&codex_tmp);
        let _ = fs::remove_file(&host_tmp);
        let _ = fs::remove_file(&sandbox_setup_tmp);
    };
    cleanup_staged();

    if codex_needed {
        emitter.emit(
            "codex_install",
            0,
            &format!("downloading codex {}", release.version),
        );
        if let Err(error) = download_to_tmp(&release.codex.name, &codex_tmp, "codex", emitter).await
        {
            cleanup_staged();
            return Err(error);
        }
    }
    if host_needed {
        emitter.emit(
            "codex_install",
            0,
            &format!("downloading codex code mode host {}", release.version),
        );
        if let Err(error) = download_to_tmp(
            &release.code_mode_host.name,
            &host_tmp,
            "codex code mode host",
            emitter,
        )
        .await
        {
            cleanup_staged();
            return Err(error);
        }
    }
    if sandbox_setup_needed {
        emitter.emit(
            "codex_install",
            0,
            &format!("downloading codex sandbox setup helper {}", release.version),
        );
        if let Err(error) = download_to_tmp(
            &release.sandbox_setup.name,
            &sandbox_setup_tmp,
            "codex sandbox setup helper",
            emitter,
        )
        .await
        {
            cleanup_staged();
            return Err(error);
        }
    }

    if codex_needed {
        if let Err(error) = verify_downloaded_tmp(&codex_tmp, &codex_path, &release.codex.sha256) {
            cleanup_staged();
            return Err(error);
        }
    }
    if host_needed {
        if let Err(error) =
            verify_downloaded_tmp(&host_tmp, &host_path, &release.code_mode_host.sha256)
        {
            cleanup_staged();
            return Err(error);
        }
    }
    if sandbox_setup_needed {
        if let Err(error) = verify_downloaded_tmp(
            &sandbox_setup_tmp,
            &sandbox_setup_path,
            &release.sandbox_setup.sha256,
        ) {
            cleanup_staged();
            return Err(error);
        }
    }

    if codex_needed {
        if let Err(error) = place_verified_tmp(&codex_tmp, &codex_path) {
            let _ = fs::remove_file(&codex_path);
            cleanup_staged();
            return Err(error);
        }
    }
    if host_needed {
        if let Err(error) = place_verified_tmp(&host_tmp, &host_path) {
            let _ = fs::remove_file(&host_path);
            if codex_needed {
                let _ = fs::remove_file(&codex_path);
            }
            cleanup_staged();
            return Err(error);
        }
    }
    if sandbox_setup_needed {
        if let Err(error) = place_verified_tmp(&sandbox_setup_tmp, &sandbox_setup_path) {
            let _ = fs::remove_file(&sandbox_setup_path);
            if host_needed {
                let _ = fs::remove_file(&host_path);
            }
            if codex_needed {
                let _ = fs::remove_file(&codex_path);
            }
            cleanup_staged();
            return Err(error);
        }
    }

    emitter.emit("codex_install", 100, "done");
    Ok(codex_path)
}

pub const MANAGED_UV_VERSION: &str = "0.11.3";
pub const MANAGED_UV_URL: &str =
    "https://github.com/astral-sh/uv/releases/download/0.11.3/uv-x86_64-pc-windows-msvc.zip";
pub const MANAGED_UV_SHA256: &str =
    "ae681c0aaec7cc96af184648cb88d73f8393ed60fa5880abdd6bdb910f9b227c";
const MANAGED_UV_ARCHIVE_NAME: &str = "uv-x86_64-pc-windows-msvc.zip";
const MANAGED_UV_EXE_NAME: &str = "uv.exe";
const MANAGED_UV_MARKER: &str = ".uv-install.json";
const MAX_UV_ARCHIVE_FILES: usize = 32;
const MAX_UV_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ManagedUvInstalledFile {
    path: String,
    sha256: String,
    bytes: u64,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ManagedUvInstallMarker {
    version: String,
    archive_sha256: String,
    executable: String,
    files: Vec<ManagedUvInstalledFile>,
}

/// Bundled managed-uv identity. No ambient executable or mutable release metadata
/// participates in dependency resolution.
pub fn managed_uv_spec() -> AssetSpec {
    AssetSpec {
        name: MANAGED_UV_URL.to_string(),
        sha256: MANAGED_UV_SHA256.to_string(),
        version: MANAGED_UV_VERSION.to_string(),
    }
}

fn safe_uv_zip_path(name: &str) -> anyhow::Result<PathBuf> {
    if name.is_empty() || name.contains('\\') || name.contains('\0') {
        bail!("uv 압축 파일에 잘못된 경로가 있습니다.");
    }
    let directoryless = name.trim_end_matches('/');
    if directoryless.is_empty()
        || directoryless.starts_with('/')
        || directoryless.as_bytes().get(1) == Some(&b':')
    {
        bail!("uv 압축 파일에 절대 경로가 있습니다.");
    }
    let mut path = PathBuf::new();
    for component in directoryless.split('/') {
        if component.is_empty()
            || matches!(component, "." | "..")
            || component.ends_with(['.', ' '])
            || component
                .chars()
                .any(|character| character.is_control() || "<>:\"|?*".contains(character))
        {
            bail!("uv 압축 파일에 경로 이탈 항목이 있습니다.");
        }
        path.push(component);
    }
    Ok(path)
}

fn metadata_is_reparse(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    false
}

fn require_plain_directory(path: &Path) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("관리형 도구 디렉터리가 없습니다: {}", path.display()))?;
    if !metadata.file_type().is_dir() || metadata_is_reparse(&metadata) {
        bail!(
            "관리형 도구 경로가 일반 디렉터리가 아닙니다: {}",
            path.display()
        );
    }
    Ok(())
}

fn ensure_plain_directory(base: &Path, path: &Path) -> anyhow::Result<()> {
    if !path.starts_with(base) {
        bail!("관리형 도구 경로가 설치 루트를 벗어났습니다.");
    }
    require_plain_directory(base)?;
    let mut current = base.to_path_buf();
    for component in path.strip_prefix(base)?.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if !metadata.file_type().is_dir() || metadata_is_reparse(&metadata) {
                    bail!(
                        "관리형 도구 하위 경로가 일반 디렉터리가 아닙니다: {}",
                        current.display()
                    );
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current)?;
                require_plain_directory(&current)?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn managed_uv_tree(
    root: &Path,
    current: &Path,
    files: &mut HashSet<String>,
    directories: &mut HashSet<String>,
) -> anyhow::Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata_is_reparse(&metadata) {
            bail!(
                "uv 설치 트리에 reparse point가 있습니다: {}",
                path.display()
            );
        }
        let relative = path
            .strip_prefix(root)?
            .to_string_lossy()
            .replace('\\', "/")
            .to_lowercase();
        if metadata.file_type().is_dir() {
            directories.insert(relative);
            managed_uv_tree(root, &path, files, directories)?;
        } else if metadata.file_type().is_file() {
            files.insert(relative);
        } else {
            bail!(
                "uv 설치 트리에 일반 파일이 아닌 항목이 있습니다: {}",
                path.display()
            );
        }
    }
    Ok(())
}

fn extract_managed_uv_archive(archive_path: &Path, staging_dir: &Path) -> anyhow::Result<PathBuf> {
    require_plain_directory(staging_dir)?;
    let archive_file = File::open(archive_path)?;
    let mut archive =
        zip::ZipArchive::new(archive_file).context("uv 배포 파일이 올바른 ZIP이 아닙니다.")?;
    if archive.is_empty() || archive.len() > MAX_UV_ARCHIVE_FILES {
        bail!("uv 압축 파일의 항목 수가 허용 범위를 벗어났습니다.");
    }
    let mut seen = HashSet::new();
    let mut files = Vec::new();
    let mut executable = None;
    let mut total_bytes = 0_u64;
    for index in 0..archive.len() {
        let mut source = archive.by_index(index)?;
        let raw_name = source.name().to_owned();
        let relative = safe_uv_zip_path(&raw_name)?;
        if relative == Path::new(MANAGED_UV_MARKER) {
            bail!("uv 압축 파일이 예약된 설치 표식을 포함합니다.");
        }
        if source
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            bail!("uv 압축 파일에 심볼릭 링크가 있습니다.");
        }
        let duplicate_key = relative.to_string_lossy().replace('\\', "/").to_lowercase();
        if !seen.insert(duplicate_key) {
            bail!("uv 압축 파일에 중복 경로가 있습니다.");
        }
        let output = staging_dir.join(&relative);
        if source.is_dir() {
            ensure_plain_directory(staging_dir, &output)?;
            continue;
        }
        if raw_name.ends_with('/') {
            bail!("uv 압축 파일의 파일/디렉터리 표시가 일치하지 않습니다.");
        }
        let bytes = source.size();
        total_bytes = total_bytes
            .checked_add(bytes)
            .context("uv 압축 해제 크기가 넘쳤습니다.")?;
        if bytes > MAX_UV_ARCHIVE_BYTES || total_bytes > MAX_UV_ARCHIVE_BYTES {
            bail!("uv 압축 해제 크기가 허용 범위를 벗어났습니다.");
        }
        let parent = output
            .parent()
            .context("uv 압축 파일 항목에 상위 경로가 없습니다.")?;
        ensure_plain_directory(staging_dir, parent)?;
        let mut destination = File::create_new(&output)?;
        std::io::copy(&mut source, &mut destination)?;
        destination.flush()?;
        destination.sync_all()?;
        let installed_bytes = destination.metadata()?.len();
        if installed_bytes != bytes {
            bail!("uv 압축 파일 항목이 완전히 기록되지 않았습니다.");
        }
        let sha256 = sha256_file(&output)?;
        if relative.file_name().is_some_and(|name| {
            name.to_string_lossy()
                .eq_ignore_ascii_case(MANAGED_UV_EXE_NAME)
        }) {
            if executable.is_some() {
                bail!("uv 압축 파일에 uv.exe가 여러 개 있습니다.");
            }
            executable = Some(relative.clone());
        }
        files.push(ManagedUvInstalledFile {
            path: relative.to_string_lossy().replace('\\', "/"),
            sha256,
            bytes,
        });
    }
    let executable = executable.context("uv 압축 파일에 uv.exe가 없습니다.")?;
    files.sort_by(|left, right| left.path.to_lowercase().cmp(&right.path.to_lowercase()));
    let marker = ManagedUvInstallMarker {
        version: MANAGED_UV_VERSION.to_string(),
        archive_sha256: MANAGED_UV_SHA256.to_string(),
        executable: executable.to_string_lossy().replace('\\', "/"),
        files,
    };
    let marker_tmp = staging_dir.join(format!("{MANAGED_UV_MARKER}.tmp"));
    let marker_path = staging_dir.join(MANAGED_UV_MARKER);
    let mut marker_file = File::create_new(&marker_tmp)?;
    marker_file.write_all(&serde_json::to_vec(&marker)?)?;
    marker_file.flush()?;
    marker_file.sync_all()?;
    fs::rename(marker_tmp, marker_path)?;
    validate_managed_uv_dir(staging_dir)
}

fn validate_managed_uv_dir(install_dir: &Path) -> anyhow::Result<PathBuf> {
    require_plain_directory(install_dir)?;
    let marker_path = install_dir.join(MANAGED_UV_MARKER);
    let marker_metadata =
        fs::symlink_metadata(&marker_path).context("uv 설치 완료 표식이 없습니다.")?;
    if !marker_metadata.file_type().is_file()
        || metadata_is_reparse(&marker_metadata)
        || marker_metadata.len() > 64 * 1024
    {
        bail!("uv 설치 완료 표식이 일반 파일이 아닙니다.");
    }
    let marker: ManagedUvInstallMarker = serde_json::from_slice(&fs::read(&marker_path)?)
        .context("uv 설치 완료 표식이 올바르지 않습니다.")?;
    if marker.version != MANAGED_UV_VERSION
        || marker.archive_sha256 != MANAGED_UV_SHA256
        || marker.files.is_empty()
    {
        bail!("uv 설치 완료 표식의 버전 또는 체크섬이 일치하지 않습니다.");
    }
    let root = fs::canonicalize(install_dir)?;
    let declared_executable = safe_uv_zip_path(&marker.executable)?;
    let mut seen = HashSet::with_capacity(marker.files.len());
    let mut executable = None;
    for file in &marker.files {
        let relative = safe_uv_zip_path(&file.path)?;
        let key = relative.to_string_lossy().replace('\\', "/").to_lowercase();
        if !seen.insert(key) {
            bail!("uv 설치 완료 표식에 중복 파일이 있습니다.");
        }
        let path = install_dir.join(&relative);
        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("uv 설치 파일이 없습니다: {}", file.path))?;
        if !metadata.file_type().is_file()
            || metadata_is_reparse(&metadata)
            || metadata.len() != file.bytes
        {
            bail!("uv 설치 파일이 손상되었습니다: {}", file.path);
        }
        let canonical = fs::canonicalize(&path)?;
        if !canonical.starts_with(&root)
            || sha256_file(&path)? != file.sha256
            || file.sha256.len() != 64
            || !file.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            bail!("uv 설치 파일 검증에 실패했습니다: {}", file.path);
        }
        if relative == declared_executable {
            executable = Some(path);
        }
    }
    let mut expected_files = seen;
    expected_files.insert(MANAGED_UV_MARKER.to_lowercase());
    let mut expected_directories = HashSet::new();
    for file in &marker.files {
        let relative = safe_uv_zip_path(&file.path)?;
        let mut parent = relative.parent();
        while let Some(directory) = parent {
            if directory.as_os_str().is_empty() {
                break;
            }
            expected_directories.insert(
                directory
                    .to_string_lossy()
                    .replace('\\', "/")
                    .to_lowercase(),
            );
            parent = directory.parent();
        }
    }
    let mut actual_files = HashSet::new();
    let mut actual_directories = HashSet::new();
    managed_uv_tree(
        install_dir,
        install_dir,
        &mut actual_files,
        &mut actual_directories,
    )?;
    if actual_files != expected_files || actual_directories != expected_directories {
        bail!("uv 설치 트리가 완료 표식의 정확한 파일 목록과 다릅니다.");
    }
    let executable = executable.context("uv 설치 완료 표식의 실행 파일이 없습니다.")?;
    if !executable.file_name().is_some_and(|name| {
        name.to_string_lossy()
            .eq_ignore_ascii_case(MANAGED_UV_EXE_NAME)
    }) {
        bail!("uv 설치 완료 표식의 실행 파일 경로가 올바르지 않습니다.");
    }
    Ok(executable)
}

/// Validate the pinned managed uv distribution without downloading or consulting PATH.
pub fn managed_uv_path(dirs: &DataDirs) -> anyhow::Result<PathBuf> {
    validate_managed_uv_dir(&dirs.managed_uv_dir(MANAGED_UV_VERSION))
}

fn publish_managed_uv(staging_dir: &Path, install_dir: &Path) -> anyhow::Result<PathBuf> {
    match fs::rename(staging_dir, install_dir) {
        Ok(()) => validate_managed_uv_dir(install_dir),
        Err(_error) if install_dir.exists() => {
            if let Ok(executable) = validate_managed_uv_dir(install_dir) {
                return Ok(executable);
            }
            let parent = install_dir
                .parent()
                .context("uv 설치 경로에 상위 디렉터리가 없습니다.")?;
            let quarantine = parent.join(format!(".invalid-{}", uuid::Uuid::new_v4().simple()));
            fs::rename(install_dir, &quarantine)
                .context("손상된 uv 설치를 격리하지 못했습니다.")?;
            if let Err(publish_error) = fs::rename(staging_dir, install_dir) {
                let _ = fs::rename(&quarantine, install_dir);
                return Err(publish_error).context("검증된 uv 설치를 게시하지 못했습니다.");
            }
            let _ = fs::remove_dir_all(quarantine);
            validate_managed_uv_dir(install_dir)
        }
        Err(error) => Err(error).context("검증된 uv 설치를 게시하지 못했습니다."),
    }
}

/// Download, verify, safely extract, and atomically publish the pinned uv 0.11.3 build.
pub async fn ensure_managed_uv(
    dirs: &DataDirs,
    emitter: &(dyn ProgressEmitter + Send + Sync),
) -> anyhow::Result<PathBuf> {
    let root = dirs.uv_dir();
    fs::create_dir_all(&root)?;
    require_plain_directory(&root)?;
    if let Ok(executable) = managed_uv_path(dirs) {
        return Ok(executable);
    }

    let staging_root = root.join(format!(".staging-{}", uuid::Uuid::new_v4().simple()));
    fs::create_dir(&staging_root)?;
    require_plain_directory(&staging_root)?;
    let payload = staging_root.join("payload");
    fs::create_dir(&payload)?;
    let archive_tmp = staging_root.join(MANAGED_UV_ARCHIVE_NAME);
    let result = async {
        emitter.emit("bootstrap", 0, "관리형 uv를 다운로드하고 있습니다.");
        download_to_tmp(MANAGED_UV_URL, &archive_tmp, "uv 0.11.3", emitter).await?;
        verify_downloaded_tmp(&archive_tmp, &archive_tmp, MANAGED_UV_SHA256)?;
        emitter.emit("bootstrap", 90, "관리형 uv를 검증하고 있습니다.");
        let extracted = tokio::task::spawn_blocking({
            let archive_tmp = archive_tmp.clone();
            let payload = payload.clone();
            move || {
                let executable = extract_managed_uv_archive(&archive_tmp, &payload)?;
                fs::remove_file(&archive_tmp)?;
                Ok::<PathBuf, anyhow::Error>(executable)
            }
        })
        .await
        .context("uv 압축 해제 작업이 중단되었습니다.")??;
        debug_assert!(extracted.starts_with(&payload));
        let executable = publish_managed_uv(&payload, &dirs.managed_uv_dir(MANAGED_UV_VERSION))?;
        emitter.emit("bootstrap", 100, "관리형 uv가 준비되었습니다.");
        Ok(executable)
    }
    .await;
    let _ = fs::remove_dir_all(staging_root);
    result
}

const EUDDRAFT_RELEASE_API_URL: &str =
    "https://api.github.com/repos/armoha/euddraft/releases/latest";
const EUDDRAFT_INSTALL_MARKER: &str = ".euddraft-install.json";
const EUDDRAFT_EXE_FILENAME: &str = "euddraft.exe";

#[derive(Debug)]
struct EuddraftReleaseSpec {
    version: String,
    archive: AssetSpec,
    archive_name: String,
    archive_bytes: u64,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct EuddraftInstalledFile {
    path: String,
    sha256: String,
    bytes: u64,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct EuddraftInstallMarker {
    version: String,
    archive_sha256: String,
    executable: String,
    files: Vec<EuddraftInstalledFile>,
}

fn parse_euddraft_release(bytes: &[u8]) -> anyhow::Result<EuddraftReleaseSpec> {
    let release: GitHubRelease =
        serde_json::from_slice(bytes).context("invalid euddraft release metadata")?;
    let version = release.tag_name.trim();
    if version.is_empty()
        || version.contains('/')
        || version.contains('\\')
        || version.contains('?')
        || version.contains('#')
    {
        bail!("euddraft release metadata has an invalid tag_name");
    }
    let version_number = version.strip_prefix('v').unwrap_or(version);
    let expected_name = format!("euddraft{version_number}.zip");
    let asset = release
        .assets
        .iter()
        .find(|asset| asset.name == expected_name)
        .with_context(|| format!("euddraft release {version} is missing {expected_name}"))?;
    let url_prefix = "https://github.com/armoha/euddraft/releases/download/";
    let url_suffix = asset
        .browser_download_url
        .strip_prefix(url_prefix)
        .with_context(|| {
            format!(
                "euddraft release {version} asset {expected_name} has an unofficial download URL"
            )
        })?;
    let (url_tag, url_name) = url_suffix.split_once('/').with_context(|| {
        format!("euddraft release {version} asset {expected_name} has an invalid download URL")
    })?;
    if url_tag != version || url_name != asset.name || url_name.contains('/') {
        bail!("euddraft release {version} asset {expected_name} has an invalid download URL");
    }
    if asset.size == 0 {
        bail!("euddraft release {version} asset {expected_name} has an invalid size");
    }
    let digest = asset
        .digest
        .as_deref()
        .and_then(|digest| digest.strip_prefix("sha256:"))
        .filter(|digest| digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .with_context(|| {
            format!("euddraft release {version} asset {expected_name} has no valid sha256 digest")
        })?;
    Ok(EuddraftReleaseSpec {
        version: version.to_string(),
        archive: AssetSpec {
            name: asset.browser_download_url.clone(),
            sha256: digest.to_ascii_lowercase(),
            version: version.to_string(),
        },
        archive_name: asset.name.clone(),
        archive_bytes: asset.size,
    })
}

async fn fetch_euddraft_release() -> anyhow::Result<EuddraftReleaseSpec> {
    let bytes = reqwest::Client::builder()
        .user_agent("eud-agent-bootstrap")
        .build()?
        .get(EUDDRAFT_RELEASE_API_URL)
        .send()
        .await
        .context("failed to fetch latest euddraft release metadata")?
        .error_for_status()
        .context("latest euddraft release metadata returned an error status")?
        .bytes()
        .await
        .context("failed to read latest euddraft release metadata")?;
    parse_euddraft_release(&bytes)
}

fn safe_euddraft_zip_path(name: &str) -> anyhow::Result<PathBuf> {
    if name.is_empty() || name.contains('\\') || name.contains('\0') {
        bail!("euddraft archive contains an invalid member path");
    }
    let directoryless = name.trim_end_matches('/');
    if directoryless.is_empty()
        || directoryless.starts_with('/')
        || directoryless.as_bytes().get(1) == Some(&b':')
    {
        bail!("euddraft archive contains an absolute member path");
    }
    let mut path = PathBuf::new();
    for component in directoryless.split('/') {
        if component.is_empty()
            || component.ends_with(['.', ' '])
            || component
                .chars()
                .any(|c| c.is_control() || "<>:\"|?*".contains(c))
        {
            bail!("euddraft archive contains a path traversal member");
        }
        path.push(component);
    }
    Ok(path)
}

fn euddraft_install_path(
    install_dir: &Path,
    marker: &EuddraftInstallMarker,
) -> anyhow::Result<PathBuf> {
    let install_meta =
        fs::symlink_metadata(install_dir).context("euddraft install directory is missing")?;
    if !install_meta.file_type().is_dir() || install_meta.file_type().is_symlink() {
        bail!("euddraft install directory is not a regular directory");
    }
    let marker_path = install_dir.join(EUDDRAFT_INSTALL_MARKER);
    let marker_meta =
        fs::symlink_metadata(&marker_path).context("euddraft install marker is missing")?;
    if !marker_meta.file_type().is_file() {
        bail!("euddraft install marker is not a regular file");
    }
    let root = fs::canonicalize(install_dir)?;
    let mut seen = HashSet::with_capacity(marker.files.len());
    let mut executable = None;
    for file in &marker.files {
        let relative = safe_euddraft_zip_path(&file.path)?;
        if !seen.insert(file.path.to_lowercase()) {
            bail!("euddraft install marker contains duplicate files");
        }
        let path = install_dir.join(&relative);
        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("euddraft install is missing {}", file.path))?;
        if !metadata.file_type().is_file() || metadata.len() != file.bytes {
            bail!("euddraft install file is incomplete: {}", file.path);
        }
        let canonical = fs::canonicalize(&path)?;
        if !canonical.starts_with(&root) || sha256_file(&path)? != file.sha256 {
            bail!("euddraft install file failed validation: {}", file.path);
        }
        if relative.file_name().is_some_and(|name| {
            name.to_string_lossy()
                .eq_ignore_ascii_case(EUDDRAFT_EXE_FILENAME)
        }) {
            if executable.is_some() {
                bail!("euddraft install marker contains multiple euddraft.exe files");
            }
            executable = Some(path);
        }
    }
    let executable = executable.context("euddraft archive has no euddraft.exe")?;
    let declared = safe_euddraft_zip_path(&marker.executable)?;
    if declared != executable.strip_prefix(install_dir)? {
        bail!("euddraft install marker executable is invalid");
    }
    Ok(executable)
}

fn extract_euddraft_archive(
    archive_path: &Path,
    staging_dir: &Path,
    version: &str,
    archive_sha256: &str,
) -> anyhow::Result<PathBuf> {
    let archive_file = File::open(archive_path)?;
    let mut archive =
        zip::ZipArchive::new(archive_file).context("euddraft release is not a valid ZIP")?;
    let mut seen = HashSet::new();
    let mut files = Vec::new();
    let mut executable = None;
    for index in 0..archive.len() {
        let mut source = archive.by_index(index)?;
        let raw_name = source.name().to_owned();
        let relative = safe_euddraft_zip_path(&raw_name)?;
        if raw_name.eq_ignore_ascii_case(EUDDRAFT_INSTALL_MARKER) {
            bail!("euddraft archive contains a reserved install marker");
        }
        if source
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            bail!("euddraft archive contains a symlink");
        }
        if !seen.insert(raw_name.trim_end_matches('/').to_lowercase()) {
            bail!("euddraft archive contains duplicate member paths");
        }
        let output = staging_dir.join(&relative);
        if output == archive_path {
            bail!("euddraft archive member collides with its staging file");
        }
        if source.is_dir() {
            fs::create_dir_all(&output)?;
            continue;
        }
        if raw_name.ends_with('/') {
            bail!("euddraft archive contains a file with a directory path");
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut destination = File::create_new(&output)?;
        std::io::copy(&mut source, &mut destination)?;
        destination.flush()?;
        destination.sync_all()?;
        let bytes = destination.metadata()?.len();
        let sha256 = sha256_file(&output)?;
        if relative.file_name().is_some_and(|name| {
            name.to_string_lossy()
                .eq_ignore_ascii_case(EUDDRAFT_EXE_FILENAME)
        }) {
            if executable.is_some() {
                bail!("euddraft archive contains multiple euddraft.exe files");
            }
            executable = Some(relative.clone());
        }
        files.push(EuddraftInstalledFile {
            path: raw_name,
            sha256,
            bytes,
        });
    }
    let executable = executable.context("euddraft archive has no euddraft.exe")?;
    let marker = EuddraftInstallMarker {
        version: version.to_string(),
        archive_sha256: archive_sha256.to_string(),
        executable: executable.to_string_lossy().replace('\\', "/"),
        files,
    };
    fs::remove_file(archive_path)?;
    let marker_tmp = staging_dir.join(format!("{EUDDRAFT_INSTALL_MARKER}.tmp"));
    let marker_path = staging_dir.join(EUDDRAFT_INSTALL_MARKER);
    let mut marker_file = File::create(&marker_tmp)?;
    marker_file.write_all(&serde_json::to_vec(&marker)?)?;
    marker_file.flush()?;
    marker_file.sync_all()?;
    fs::rename(marker_tmp, marker_path)?;
    euddraft_install_path(staging_dir, &marker)
}

fn validate_euddraft_install(
    install_dir: &Path,
    version: &str,
    archive_sha256: &str,
) -> anyhow::Result<PathBuf> {
    let marker_path = install_dir.join(EUDDRAFT_INSTALL_MARKER);
    let marker: EuddraftInstallMarker = serde_json::from_slice(&fs::read(&marker_path)?)?;
    if marker.version != version || marker.archive_sha256 != archive_sha256 {
        bail!("euddraft install version or digest does not match latest release");
    }
    euddraft_install_path(install_dir, &marker)
}

/// Download and atomically install the latest complete euddraft distribution.
///
/// The release archive is verified before extraction. All members are extracted into a
/// fresh version/digest-addressed directory, validated with a persisted file manifest, and
/// only then published by rename, so a failed download or extraction cannot become runnable.
pub async fn ensure_euddraft(
    dirs: &DataDirs,
    emitter: &(dyn ProgressEmitter + Send + Sync),
) -> anyhow::Result<PathBuf> {
    let root = dirs.euddraft_dir();
    fs::create_dir_all(&root)
        .with_context(|| format!("cannot create euddraft dir {}", root.display()))?;
    if fs::symlink_metadata(&root)?.file_type().is_symlink() {
        bail!("euddraft dir is a symlink");
    }
    emitter.emit("bootstrap", 0, "checking latest euddraft release");
    let release = fetch_euddraft_release().await?;
    let install_dir = root.join(format!("sha256-{}", release.archive.sha256));
    match fs::symlink_metadata(&install_dir) {
        Ok(_) => {
            return validate_euddraft_install(
                &install_dir,
                &release.version,
                &release.archive.sha256,
            )
            .inspect(|_| {
                emitter.emit("bootstrap", 100, "euddraft ready");
            });
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    let staging_dir = root.join(format!(".staging-{}", uuid::Uuid::new_v4().simple()));
    fs::create_dir(&staging_dir)?;
    let archive_tmp = staging_dir.join(&release.archive_name);
    let result = async {
        emitter.emit(
            "bootstrap",
            0,
            &format!("downloading euddraft {}", release.version),
        );
        download_to_tmp(&release.archive.name, &archive_tmp, "euddraft", emitter).await?;
        let actual_size = fs::metadata(&archive_tmp)?.len();
        if actual_size != release.archive_bytes {
            bail!(
                "euddraft archive size mismatch: expected {}, got {}",
                release.archive_bytes,
                actual_size
            );
        }
        verify_downloaded_tmp(&archive_tmp, &archive_tmp, &release.archive.sha256)?;
        emitter.emit("bootstrap", 90, "extracting euddraft");
        let executable = tokio::task::spawn_blocking({
            let archive_tmp = archive_tmp.clone();
            let staging_dir = staging_dir.clone();
            let version = release.version.clone();
            let digest = release.archive.sha256.clone();
            move || extract_euddraft_archive(&archive_tmp, &staging_dir, &version, &digest)
        })
        .await
        .context("euddraft extraction task failed")??;
        let installed_executable = install_dir.join(executable.strip_prefix(&staging_dir)?);
        match fs::rename(&staging_dir, &install_dir) {
            Ok(()) => {}
            Err(_) if fs::symlink_metadata(&install_dir).is_ok() => {
                let executable = validate_euddraft_install(
                    &install_dir,
                    &release.version,
                    &release.archive.sha256,
                )
                .with_context(|| {
                    format!(
                        "euddraft install appeared concurrently but is invalid: {}",
                        install_dir.display()
                    )
                })?;
                emitter.emit("bootstrap", 100, "euddraft ready");
                return Ok(executable);
            }
            Err(error) => return Err(error.into()),
        }
        emitter.emit("bootstrap", 100, "euddraft ready");
        Ok(installed_executable)
    }
    .await;
    // Also remove our staging tree when another process published the same release.
    let _ = fs::remove_dir_all(&staging_dir);
    result
}

/// Stream `url` to `tmp`, emitting `bootstrap` byte-progress. Caller owns tmp cleanup on
/// error (we only write; verify+place happens after). NOT unit-tested (real HTTP).
async fn download_to_tmp(
    url: &str,
    tmp: &Path,
    label: &str,
    emitter: &(dyn ProgressEmitter + Send + Sync),
) -> anyhow::Result<()> {
    let client = reqwest::Client::builder()
        .user_agent("eud-agent-bootstrap")
        .build()?;
    let mut resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url} failed"))?
        .error_for_status()
        .with_context(|| format!("{label} download returned an error status"))?;

    let total = resp.content_length();
    let mut downloaded: u64 = 0;
    let mut out =
        File::create(tmp).with_context(|| format!("cannot create tmp {}", tmp.display()))?;
    // `Response::chunk` (reqwest `stream` feature) avoids a `futures_util` dep edge.
    while let Some(chunk) = resp
        .chunk()
        .await
        .with_context(|| format!("{label} stream error"))?
    {
        out.write_all(&chunk)
            .with_context(|| format!("cannot write tmp {}", tmp.display()))?;
        downloaded += chunk.len() as u64;
        if let Some(total) = total.filter(|t| *t > 0) {
            let pct = ((downloaded.min(total) * 100) / total) as u8;
            emitter.emit("bootstrap", pct, &format!("downloading {label}"));
        }
    }
    out.flush()?;
    Ok(())
}

/// `<path>` with a `.tmp` suffix appended (so `rag-index.bin` -> `rag-index.bin.tmp`).
fn with_tmp_suffix(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

/// Parse the CI release manifest into the config [`AssetSpec`] (`url` -> `name`).
///
/// Pure so it is unit-testable without network; [`fetch_release_manifest`] is the
/// thin HTTP wrapper around it.
pub fn parse_release_manifest(bytes: &[u8]) -> anyhow::Result<AssetSpec> {
    #[derive(serde::Deserialize)]
    struct Manifest {
        rag_index: ManifestSpec,
    }
    #[derive(serde::Deserialize)]
    struct ManifestSpec {
        url: String,
        sha256: String,
        #[serde(default)]
        version: String,
    }

    let manifest: Manifest =
        serde_json::from_slice(bytes).context("invalid rag-index release manifest")?;
    let spec = manifest.rag_index;
    if spec.url.trim().is_empty() || spec.sha256.trim().is_empty() {
        bail!("rag-index release manifest is missing url/sha256");
    }
    // Only the required index format is adopted; an older (or unexpected) version would
    // pin the loader to an index the runtime cannot read (v1 has no `tier_level`).
    if spec.version != REQUIRED_RAG_INDEX_VERSION {
        bail!(
            "rag-index release manifest version {:?} is not the required {:?}",
            spec.version,
            REQUIRED_RAG_INDEX_VERSION
        );
    }
    Ok(AssetSpec {
        name: spec.url,
        sha256: spec.sha256,
        version: spec.version,
    })
}

/// Fetch + parse [`rag_manifest_url`]. NOT unit-tested (real HTTP); the parse logic
/// is covered by the `parse_release_manifest` tests.
pub async fn fetch_release_manifest() -> anyhow::Result<AssetSpec> {
    let client = reqwest::Client::builder()
        .user_agent("eud-agent-bootstrap")
        .build()?;
    let bytes = client
        .get(rag_manifest_url())
        .send()
        .await
        .context("GET rag-index release manifest failed")?
        .error_for_status()
        .context("rag-index release manifest returned an error status")?
        .bytes()
        .await
        .context("rag-index release manifest read failed")?;
    parse_release_manifest(&bytes)
}

/// True when either asset is missing/corrupt and a first-run install is required.
///
/// Pure (filesystem-probe + hash only) so the setup screen can branch on it without any
/// network. Empty specs (a first-run `config.json` with no manifest) report `true`.
pub fn needs_bootstrap(dirs: &DataDirs, config: &crate::config::Config) -> bool {
    asset_status(&dirs.rag_dir(), RAG_INDEX_FILENAME, &config.rag_index).needs_download()
        || config.rag_index.sha256.is_empty()
        // A stale-generation config re-downloads even when the asset is present and its
        // sha256 matches — the runtime loader requires the v2 format (feature 17).
        || config.rag_index.version != REQUIRED_RAG_INDEX_VERSION
        || config.model.name.is_empty()
}

/// Run the full first-run install: fetch + verify + atomically place the bge-m3 model
/// (fastembed HF cache) and the RAG index (GitHub Release), reporting progress.
///
/// Each asset is skipped when already `Present`. The model fetch is blocking (ONNX init),
/// so it runs on a blocking task; the RAG index streams over async HTTP. NOT unit-tested —
/// it performs the real downloads; its testable pieces are covered above.
pub async fn bootstrap_assets(
    dirs: &DataDirs,
    config: &crate::config::Config,
    emitter: &(dyn ProgressEmitter + Send + Sync),
) -> anyhow::Result<()> {
    ensure_rag_index(dirs, &config.rag_index, emitter).await?;
    // fastembed is synchronous/CPU-bound; keep the async runtime free.
    let dirs2 = dirs.clone();
    tokio::task::block_in_place(|| ensure_model(&dirs2, emitter))?;
    if let Err(error) = ensure_ffmpeg(dirs, emitter).await {
        eprintln!("eud-agent: managed audio converter bootstrap failed: {error}");
        emitter.emit(
            "bootstrap",
            100,
            "audio converter unavailable; chat and existing attachments remain available",
        );
    }
    Ok(())
}

#[cfg(test)]
mod model_initialization {
    use super::lock_model_initialization;
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
    use std::sync::{Arc, Barrier};

    #[test]
    fn model_initialization_lock_serializes_bootstrap_and_rag() {
        let workers = 8;
        let start = Arc::new(Barrier::new(workers));
        let inflight = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for _ in 0..workers {
            let start = start.clone();
            let inflight = inflight.clone();
            let peak = peak.clone();
            handles.push(std::thread::spawn(move || {
                start.wait();
                let _guard = lock_model_initialization();
                let now = inflight.fetch_add(1, SeqCst) + 1;
                peak.fetch_max(now, SeqCst);
                std::thread::yield_now();
                inflight.fetch_sub(1, SeqCst);
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }

        assert_eq!(
            peak.load(SeqCst),
            1,
            "bootstrap and RAG warmup must serialize model initialization"
        );
    }
}

#[cfg(test)]
mod manifest {
    use super::*;
    use crate::config::AssetSpec;
    use std::fs;
    use std::path::PathBuf;

    /// Unique temp base dir for a test (no `tempfile` dev-dep; Cargo.toml is scoped).
    fn unique_temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("eud-agent-boot-test-{tag}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    // sha256("hello") — the canonical test vector.
    const HELLO_SHA: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

    #[test]
    fn managed_uv_metadata_is_fully_pinned() {
        let spec = managed_uv_spec();
        assert_eq!(spec.version, "0.11.3");
        assert_eq!(
            spec.name,
            "https://github.com/astral-sh/uv/releases/download/0.11.3/uv-x86_64-pc-windows-msvc.zip"
        );
        assert_eq!(
            spec.sha256,
            "ae681c0aaec7cc96af184648cb88d73f8393ed60fa5880abdd6bdb910f9b227c"
        );
    }

    #[test]
    fn managed_uv_staging_is_marker_last_and_detects_corruption() {
        let base = unique_temp_dir("managed-uv");
        let archive_path = base.join("uv.zip");
        let mut archive = zip::ZipWriter::new(File::create(&archive_path).unwrap());
        for (name, bytes) in [("uv.exe", b"uv".as_slice()), ("uvx.exe", b"uvx".as_slice())] {
            archive
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(bytes).unwrap();
        }
        archive.finish().unwrap();
        let staging = base.join("staging");
        fs::create_dir(&staging).unwrap();
        let executable = extract_managed_uv_archive(&archive_path, &staging).unwrap();
        assert_eq!(executable, staging.join("uv.exe"));
        assert!(staging.join(MANAGED_UV_MARKER).is_file());
        assert_eq!(validate_managed_uv_dir(&staging).unwrap(), executable);
        fs::write(staging.join("unexpected.txt"), b"unexpected").unwrap();
        assert!(validate_managed_uv_dir(&staging).is_err());
        fs::remove_file(staging.join("unexpected.txt")).unwrap();
        fs::write(staging.join("uvx.exe"), b"damaged").unwrap();
        assert!(validate_managed_uv_dir(&staging).is_err());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn managed_uv_zip_paths_reject_escape_and_windows_aliases() {
        for path in [
            "../uv.exe",
            "/uv.exe",
            "C:/uv.exe",
            "dir\\uv.exe",
            "dir./uv.exe",
        ] {
            assert!(
                safe_uv_zip_path(path).is_err(),
                "unsafe path accepted: {path}"
            );
        }
        assert_eq!(
            safe_uv_zip_path("tools/uv.exe").unwrap(),
            PathBuf::from("tools/uv.exe")
        );
    }

    #[test]
    fn euddraft_archive_preserves_nested_distribution_and_detects_damaged_dependency() {
        let base = unique_temp_dir("euddraft-extract");
        let archive_path = base.join("release.zip");
        let mut archive = zip::ZipWriter::new(File::create(&archive_path).unwrap());
        for (name, contents) in [
            ("euddraft/euddraft.exe", "executable"),
            ("euddraft/lib/python.dll", "runtime"),
            ("euddraft/plugins/example.py", "plugin"),
        ] {
            archive
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(contents.as_bytes()).unwrap();
        }
        archive.finish().unwrap();
        let staging = base.join("staging");
        fs::create_dir(&staging).unwrap();
        let executable =
            extract_euddraft_archive(&archive_path, &staging, "v1", HELLO_SHA).unwrap();
        assert_eq!(fs::read(executable).unwrap(), b"executable");
        assert_eq!(
            fs::read(staging.join("euddraft/plugins/example.py")).unwrap(),
            b"plugin"
        );
        let installed = base.join("installed");
        fs::rename(staging, &installed).unwrap();
        assert_eq!(
            validate_euddraft_install(&installed, "v1", HELLO_SHA).unwrap(),
            installed.join("euddraft/euddraft.exe")
        );
        fs::write(installed.join("euddraft/lib/python.dll"), b"damaged").unwrap();
        assert!(validate_euddraft_install(&installed, "v1", HELLO_SHA).is_err());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn sha256_hex_bytes_matches_known_vector() {
        assert_eq!(sha256_hex_bytes(b"hello"), HELLO_SHA);
    }

    #[test]
    fn sha256_file_matches_bytes() {
        let base = unique_temp_dir("shafile");
        let p = base.join("f.bin");
        fs::write(&p, b"hello").unwrap();
        assert_eq!(sha256_file(&p).unwrap(), HELLO_SHA);
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn asset_status_missing_when_absent() {
        let base = unique_temp_dir("status-missing");
        let spec = AssetSpec {
            name: "rag.bin".to_string(),
            sha256: HELLO_SHA.to_string(),
            version: "1".to_string(),
        };
        // No file written -> Missing -> needs download.
        assert_eq!(asset_status(&base, "rag.bin", &spec), AssetStatus::Missing);
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn asset_status_present_when_hash_matches() {
        let base = unique_temp_dir("status-present");
        fs::write(base.join("rag.bin"), b"hello").unwrap();
        let spec = AssetSpec {
            name: "rag.bin".to_string(),
            sha256: HELLO_SHA.to_string(),
            version: "1".to_string(),
        };
        assert_eq!(asset_status(&base, "rag.bin", &spec), AssetStatus::Present);
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn asset_status_corrupt_when_hash_mismatches() {
        let base = unique_temp_dir("status-corrupt");
        fs::write(base.join("rag.bin"), b"not hello").unwrap();
        let spec = AssetSpec {
            name: "rag.bin".to_string(),
            sha256: HELLO_SHA.to_string(),
            version: "1".to_string(),
        };
        assert_eq!(asset_status(&base, "rag.bin", &spec), AssetStatus::Corrupt);
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn verify_and_place_renames_on_good_hash() {
        let base = unique_temp_dir("place-ok");
        let tmp = base.join("rag.bin.tmp");
        let final_path = base.join("rag.bin");
        fs::write(&tmp, b"hello").unwrap();

        verify_and_place(&tmp, &final_path, HELLO_SHA).unwrap();

        // Atomic place succeeded: final exists with the right bytes, tmp is gone.
        assert!(final_path.is_file());
        assert!(!tmp.exists());
        assert_eq!(fs::read(&final_path).unwrap(), b"hello");
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn verify_and_place_refuses_on_bad_hash_without_replacing_existing_asset() {
        let base = unique_temp_dir("place-bad-existing");
        let tmp = base.join("rag.bin.tmp");
        let final_path = base.join("rag.bin");
        fs::write(&final_path, b"old release").unwrap();
        fs::write(&tmp, b"not hello").unwrap();

        let err = verify_and_place(&tmp, &final_path, HELLO_SHA);
        assert!(err.is_err(), "sha256 mismatch must refuse to install");

        // A failed download must roll back completely: retain the last known-good
        // release and remove the unverified staging file.
        assert_eq!(fs::read(&final_path).unwrap(), b"old release");
        assert!(!tmp.exists(), "tmp must be removed on mismatch");
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn verify_and_place_no_final_when_tmp_missing() {
        // A failed/short write means no tmp at all: placement errors, leaves no final.
        let base = unique_temp_dir("place-notmp");
        let tmp = base.join("rag.bin.tmp");
        let final_path = base.join("rag.bin");
        // tmp intentionally not created.

        assert!(verify_and_place(&tmp, &final_path, HELLO_SHA).is_err());
        assert!(
            !final_path.exists(),
            "no final file from a failed/short write"
        );
        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn rag_manifest_url_targets_dedicated_rag_tag() {
        // The manifest must resolve against the RAG index's own `rag-index-v<version>`
        // release tag, NOT `releases/latest` (which the app-binary release shadows -> 404).
        let url = rag_manifest_url();
        assert_eq!(
            url,
            format!(
                "https://github.com/raravel/eud-agent/releases/download/rag-index-v{REQUIRED_RAG_INDEX_VERSION}/rag-index.manifest.json"
            )
        );
        assert!(
            !url.contains("releases/latest"),
            "RAG manifest must not resolve via releases/latest (app-binary release shadows it)"
        );
    }

    #[test]
    fn release_manifest_parses_into_asset_spec() {
        let json = br#"{
            "rag_index": {
                "url": "https://github.com/raravel/eud-agent/releases/download/rag-index-v4/rag-index.bin",
                "sha256": "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
                "version": "4"
            }
        }"#;

        let spec = parse_release_manifest(json).unwrap();

        // The manifest's `url` maps onto AssetSpec.name (the release asset URL).
        assert_eq!(
            spec.name,
            "https://github.com/raravel/eud-agent/releases/download/rag-index-v4/rag-index.bin"
        );
        assert_eq!(spec.sha256, HELLO_SHA);
        assert_eq!(spec.version, REQUIRED_RAG_INDEX_VERSION);
    }

    // ---- RAG release generation rollover ----
    //
    // Release v4 keeps the v2 binary layout and replaces its corpus. These tests pin the
    // distribution contract: only a v4 manifest is adopted, a stale v3-pinned healthy
    // asset must re-download, and a present v4 asset remains ready.

    /// A config whose rag_index asset is Present on disk (sha256 matches) but pinned
    /// to v3 must still report `needs_bootstrap == true` so the refreshed v4 corpus
    /// replaces the otherwise-valid old index.
    #[test]
    fn needs_bootstrap_true_for_stale_v3_cached_config() {
        let base = unique_temp_dir("needs-v3-stale");
        let dirs = crate::config::DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        dirs.ensure_dirs().unwrap();
        // Place the asset so its sha256 matches the cached config -> AssetStatus::Present.
        fs::write(dirs.rag_dir().join(RAG_INDEX_FILENAME), b"hello").unwrap();

        let config = crate::config::Config {
            model: AssetSpec {
                name: DEFAULT_MODEL_NAME.to_string(),
                ..Default::default()
            },
            rag_index: AssetSpec {
                name: "https://example.com/rag.bin".to_string(),
                sha256: HELLO_SHA.to_string(),
                version: "3".to_string(),
            },
            ..Default::default()
        };

        assert!(
            needs_bootstrap(&dirs, &config),
            "a cached v3 config must re-download even when the asset is present + sha256 matches"
        );
        fs::remove_dir_all(&base).ok();
    }

    /// Everything present AND pinned to the required v4 version (model name set) ->
    /// no bootstrap needed.
    #[test]
    fn needs_bootstrap_false_when_present_and_v4() {
        let base = unique_temp_dir("needs-v4-ok");
        let dirs = crate::config::DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        dirs.ensure_dirs().unwrap();
        fs::write(dirs.rag_dir().join(RAG_INDEX_FILENAME), b"hello").unwrap();

        let config = crate::config::Config {
            model: AssetSpec {
                name: DEFAULT_MODEL_NAME.to_string(),
                ..Default::default()
            },
            rag_index: AssetSpec {
                name: "https://example.com/rag.bin".to_string(),
                sha256: HELLO_SHA.to_string(),
                version: "4".to_string(),
            },
            ..Default::default()
        };

        assert!(
            !needs_bootstrap(&dirs, &config),
            "present asset pinned to v4 with a model name must not need bootstrap"
        );
        fs::remove_dir_all(&base).ok();
    }

    /// Only a v4 manifest is acceptable: a v3 manifest is rejected, while v4 is
    /// adopted and its release generation flows into the AssetSpec.

    #[test]
    fn release_manifest_requires_v4() {
        let v3 = br#"{
            "rag_index": {
                "url": "https://x/y.bin",
                "sha256": "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
                "version": "3"
            }
        }"#;
        assert!(
            parse_release_manifest(v3).is_err(),
            "a v3 manifest must be rejected once v4 ships"
        );

        let v4 = br#"{
            "rag_index": {
                "url": "https://x/y.bin",
                "sha256": "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
                "version": "4"
            }
        }"#;
        let spec = parse_release_manifest(v4).expect("a v4 manifest must be accepted");
        assert_eq!(spec.version, "4");
    }

    #[test]
    fn release_manifest_rejects_missing_or_empty_fields() {
        assert!(parse_release_manifest(b"not json").is_err());
        assert!(parse_release_manifest(b"{}").is_err());
        assert!(
            parse_release_manifest(br#"{ "rag_index": { "url": "", "sha256": "abc" } }"#).is_err(),
            "empty url must refuse (nothing to download)"
        );
        assert!(
            parse_release_manifest(
                br#"{ "rag_index": { "url": "https://x/y.bin", "sha256": "" } }"#
            )
            .is_err(),
            "empty sha256 must refuse (nothing to verify against)"
        );
    }

    #[test]
    fn euddraft_release_requires_pinned_official_zip_and_digest() {
        let json = format!(
            r#"{{
                "tag_name": "v0.10.2.5",
                "assets": [{{
                    "name": "euddraft0.10.2.5.zip",
                    "browser_download_url": "https://github.com/armoha/euddraft/releases/download/v0.10.2.5/euddraft0.10.2.5.zip",
                    "digest": "sha256:{HELLO_SHA}",
                    "size": 15108674
                }}]
            }}"#
        );
        let release = parse_euddraft_release(json.as_bytes()).unwrap();
        assert_eq!(release.version, "v0.10.2.5");
        assert_eq!(release.archive_name, "euddraft0.10.2.5.zip");
        assert_eq!(release.archive.sha256, HELLO_SHA);
        assert!(parse_euddraft_release(
            json.replace(
                "https://github.com/armoha/euddraft/releases/download",
                "https://github.com/armoha/euddraft/archive"
            )
            .as_bytes()
        )
        .is_err());
        assert!(parse_euddraft_release(
            json.replace("euddraft0.10.2.5.zip", "euddraft0.10.2.5-source.zip")
                .as_bytes()
        )
        .is_err());
    }

    #[test]
    fn euddraft_zip_paths_reject_escape_and_drive_names() {
        for path in [
            "../escape.exe",
            "/absolute.exe",
            "C:/drive.exe",
            "nested\\escape.exe",
        ] {
            assert!(
                safe_euddraft_zip_path(path).is_err(),
                "unsafe ZIP member path accepted: {path}"
            );
        }
        assert_eq!(
            safe_euddraft_zip_path("release/bin/euddraft.exe").unwrap(),
            PathBuf::from("release/bin/euddraft.exe")
        );
    }

    #[test]
    fn codex_release_parses_version_locked_distribution() {
        let json = format!(
            r#"{{
                "tag_name": "rust-v0.147.0",
                "assets": [
                    {{
                        "name": "{CODEX_RELEASE_HOST_ASSET_NAME}",
                        "browser_download_url": "https://example.com/host.exe",
                        "digest": "sha256:{HELLO_SHA}",
                        "size": 57450288
                    }},
                    {{
                        "name": "{CODEX_RELEASE_SANDBOX_SETUP_ASSET_NAME}",
                        "browser_download_url": "https://example.com/sandbox-setup.exe",
                        "digest": "sha256:{HELLO_SHA}",
                        "size": 8852272
                    }},
                    {{
                        "name": "{CODEX_RELEASE_EXE_ASSET_NAME}",
                        "browser_download_url": "https://example.com/codex.exe",
                        "digest": "sha256:{HELLO_SHA}",
                        "size": 298668336
                    }}
                ]
            }}"#
        );

        let release = parse_codex_release(json.as_bytes()).unwrap();
        assert_eq!(release.version, "rust-v0.147.0");
        assert_eq!(release.codex.name, "https://example.com/codex.exe");
        assert_eq!(release.code_mode_host.name, "https://example.com/host.exe");
        assert_eq!(
            release.sandbox_setup.name,
            "https://example.com/sandbox-setup.exe"
        );
        assert_eq!(release.codex.sha256, HELLO_SHA);
        assert_eq!(release.code_mode_host.sha256, HELLO_SHA);
        assert_eq!(release.sandbox_setup.sha256, HELLO_SHA);
        assert_eq!(release.codex.version, release.code_mode_host.version);
        assert_eq!(
            release.code_mode_host.version,
            release.sandbox_setup.version
        );
    }

    #[test]
    fn codex_release_rejects_missing_runtime_sibling_or_digest() {
        let missing_sandbox_setup = format!(
            r#"{{
                "tag_name": "rust-v0.147.0",
                "assets": [
                    {{
                        "name": "{CODEX_RELEASE_EXE_ASSET_NAME}",
                        "browser_download_url": "https://example.com/codex.exe",
                        "digest": "sha256:{HELLO_SHA}",
                        "size": 298668336
                    }},
                    {{
                        "name": "{CODEX_RELEASE_HOST_ASSET_NAME}",
                        "browser_download_url": "https://example.com/host.exe",
                        "digest": "sha256:{HELLO_SHA}",
                        "size": 57450288
                    }}
                ]
            }}"#
        );
        assert!(parse_codex_release(missing_sandbox_setup.as_bytes()).is_err());

        let missing_digest = format!(
            r#"{{
                "tag_name": "rust-v0.147.0",
                "assets": [
                    {{
                        "name": "{CODEX_RELEASE_EXE_ASSET_NAME}",
                        "browser_download_url": "https://example.com/codex.exe",
                        "digest": null,
                        "size": 298668336
                    }},
                    {{
                        "name": "{CODEX_RELEASE_HOST_ASSET_NAME}",
                        "browser_download_url": "https://example.com/host.exe",
                        "digest": "sha256:{HELLO_SHA}",
                        "size": 57450288
                    }},
                    {{
                        "name": "{CODEX_RELEASE_SANDBOX_SETUP_ASSET_NAME}",
                        "browser_download_url": "https://example.com/sandbox-setup.exe",
                        "digest": "sha256:{HELLO_SHA}",
                        "size": 8852272
                    }}
                ]
            }}"#
        );
        assert!(parse_codex_release(missing_digest.as_bytes()).is_err());
    }

    #[tokio::test]
    #[ignore = "downloads the current official Codex CLI and runtime helpers"]
    async fn codex_distribution_download_smoke() {
        struct Silent;
        impl ProgressEmitter for Silent {
            fn emit(&self, _stage: &str, _pct: u8, _detail: &str) {}
        }

        let base = unique_temp_dir("codex-download-smoke");
        let dirs = crate::config::DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        dirs.ensure_dirs().unwrap();

        let codex_path = ensure_codex(&dirs, &Silent).await.unwrap();
        assert!(codex_path.is_file());
        assert!(dirs.bin_dir().join(CODEX_CODE_MODE_HOST_FILENAME).is_file());
        assert!(dirs.bin_dir().join(CODEX_SANDBOX_SETUP_FILENAME).is_file());

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    #[ignore = "requires FFMPEG_TEST_ARCHIVE pointing to the pinned distribution ZIP"]
    fn managed_ffmpeg_archive_extracts_and_verifies_both_exact_members() {
        let source = PathBuf::from(
            std::env::var_os("FFMPEG_TEST_ARCHIVE")
                .expect("FFMPEG_TEST_ARCHIVE must name the pinned ZIP"),
        );
        let base = unique_temp_dir("ffmpeg-extract");
        let dirs = crate::config::DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        dirs.ensure_dirs().unwrap();
        let staged = dirs.bin_dir().join("ffmpeg-distribution.zip.tmp");
        fs::copy(source, &staged).unwrap();
        let manifest = managed_ffmpeg_manifest().unwrap();
        verify_downloaded_tmp(&staged, &staged, &manifest.archive.sha256).unwrap();
        extract_managed_ffmpeg_archive(&staged, &dirs.bin_dir(), &manifest).unwrap();
        let resolved = resolve_managed_ffmpeg(&dirs).unwrap();
        assert!(resolved.ffmpeg.is_file());
        assert!(resolved.ffprobe.is_file());
        assert_eq!(resolved.version, manifest.version);
        assert!(!staged.exists());
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn progress_emitter_double_records() {
        // The emitter is injectable so the download flow is testable without a Tauri app.
        struct Rec(std::cell::RefCell<Vec<(String, u8)>>);
        impl ProgressEmitter for Rec {
            fn emit(&self, stage: &str, pct: u8, _detail: &str) {
                self.0.borrow_mut().push((stage.to_string(), pct));
            }
        }
        let rec = Rec(std::cell::RefCell::new(Vec::new()));
        rec.emit("bootstrap", 50, "halfway");
        assert_eq!(rec.0.borrow().as_slice(), &[("bootstrap".to_string(), 50)]);
    }
}
