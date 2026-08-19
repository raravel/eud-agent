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

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use sha2::{Digest, Sha256};

use crate::config::{AssetSpec, DataDirs};

/// The RAG index is stored under `rag/` with this fixed basename (the GitHub Release asset
/// is downloaded to it after sha256 verification).
pub const RAG_INDEX_FILENAME: &str = "rag-index.bin";

/// The RAG release generation the runtime requires. The persisted binary layout remains
/// v2 (`rag.rs::INDEX_VERSION`); release v3 carries the refreshed seven-source corpus.
/// Bumping this generation forces healthy v2 installations to fetch the v3 manifest and
/// atomically replace their otherwise-valid old index.
pub const REQUIRED_RAG_INDEX_VERSION: &str = "3";

/// HF model id installed on first run when `config.json` carries none (feature 10).
pub const DEFAULT_MODEL_NAME: &str = "BAAI/bge-m3";

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
    let bin_dir = dirs.bin_dir();
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
        // A stale v1-pinned config re-downloads even when the asset is present and its
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
    Ok(())
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
    fn verify_and_place_refuses_on_bad_hash() {
        let base = unique_temp_dir("place-bad");
        let tmp = base.join("rag.bin.tmp");
        let final_path = base.join("rag.bin");
        fs::write(&tmp, b"not hello").unwrap();

        let err = verify_and_place(&tmp, &final_path, HELLO_SHA);
        assert!(err.is_err(), "sha256 mismatch must refuse to install");

        // No half-install: the final path is never written and the tmp is cleaned up.
        assert!(!final_path.exists(), "final must not be placed on mismatch");
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
                "url": "https://github.com/raravel/eud-agent/releases/download/rag-index-v3/rag-index.bin",
                "sha256": "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
                "version": "3"
            }
        }"#;

        let spec = parse_release_manifest(json).unwrap();

        // The manifest's `url` maps onto AssetSpec.name (the release asset URL).
        assert_eq!(
            spec.name,
            "https://github.com/raravel/eud-agent/releases/download/rag-index-v3/rag-index.bin"
        );
        assert_eq!(spec.sha256, HELLO_SHA);
        assert_eq!(spec.version, REQUIRED_RAG_INDEX_VERSION);
    }

    // ---- RAG release generation rollover ----
    //
    // Release v3 keeps the v2 binary layout and replaces its corpus. These tests pin the
    // distribution contract: only a v3 manifest is adopted, a stale v2-pinned healthy
    // asset must re-download, and a present v3 asset remains ready.

    /// A config whose rag_index asset is Present on disk (sha256 matches) but pinned
    /// to v2 must still report `needs_bootstrap == true` so the refreshed v3 corpus
    /// replaces the otherwise-valid old index.
    #[test]
    fn needs_bootstrap_true_for_stale_v2_present_asset() {
        let base = unique_temp_dir("needs-v2-stale");
        let dirs = crate::config::DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        dirs.ensure_dirs().unwrap();
        // Place the asset so its sha256 matches the spec -> AssetStatus::Present.
        fs::write(dirs.rag_dir().join(RAG_INDEX_FILENAME), b"hello").unwrap();

        let config = crate::config::Config {
            model: AssetSpec {
                name: DEFAULT_MODEL_NAME.to_string(),
                ..Default::default()
            },
            rag_index: AssetSpec {
                name: "https://example.com/rag.bin".to_string(),
                sha256: HELLO_SHA.to_string(),
                version: "2".to_string(),
            },
            ..Default::default()
        };

        assert!(
            needs_bootstrap(&dirs, &config),
            "a v2-pinned config must re-download even when the asset is present + sha256 matches"
        );
        fs::remove_dir_all(&base).ok();
    }

    /// Everything present AND pinned to the required v3 version (model name set) ->
    /// no bootstrap needed.
    #[test]
    fn needs_bootstrap_false_when_present_and_v3() {
        let base = unique_temp_dir("needs-v3-ok");
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
                version: "3".to_string(),
            },
            ..Default::default()
        };

        assert!(
            !needs_bootstrap(&dirs, &config),
            "present asset pinned to v3 with a model name must not need bootstrap"
        );
        fs::remove_dir_all(&base).ok();
    }

    /// Only a v3 manifest is acceptable: a v2 manifest is rejected, while v3 is
    /// adopted and its release generation flows into the AssetSpec.
    #[test]
    fn release_manifest_requires_v3() {
        let v2 = br#"{
            "rag_index": {
                "url": "https://x/y.bin",
                "sha256": "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
                "version": "2"
            }
        }"#;
        assert!(
            parse_release_manifest(v2).is_err(),
            "a v2 manifest must be rejected once v3 ships"
        );

        let v3 = br#"{
            "rag_index": {
                "url": "https://x/y.bin",
                "sha256": "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
                "version": "3"
            }
        }"#;
        let spec = parse_release_manifest(v3).expect("a v3 manifest must be accepted");
        assert_eq!(spec.version, "3");
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
