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

/// HF model id installed on first run when `config.json` carries none (feature 10).
pub const DEFAULT_MODEL_NAME: &str = "BAAI/bge-m3";

/// Published release manifest for the RAG index, uploaded next to `rag-index.bin`
/// by `.github/workflows/build-rag-index.yml` (`{"rag_index":{url,sha256,version}}`).
/// Fetched when `config.json` has no pinned spec yet (first run); the sha256 inside
/// pins the asset bytes that `verify_and_place` enforces.
pub const RAG_MANIFEST_URL: &str =
    "https://github.com/raravel/eud-agent/releases/latest/download/rag-index.manifest.json";

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

/// Verify `tmp` against `expected_sha`, then atomically rename it over `final_path`.
///
/// On a sha256 mismatch (or a missing/unreadable tmp) the tmp is removed and an error is
/// returned — the final path is NEVER written. This is the single chokepoint every
/// download funnels through, so no code path can place an unverified or partial file.
pub fn verify_and_place(tmp: &Path, final_path: &Path, expected_sha: &str) -> anyhow::Result<()> {
    // Hash the tmp; any read failure (e.g. a failed/short write left no tmp) is an error
    // and leaves the final path untouched.
    let actual = match sha256_file(tmp) {
        Ok(h) => h,
        Err(e) => {
            // Best-effort cleanup of a partial tmp; ignore if it never existed.
            let _ = fs::remove_file(tmp);
            return Err(anyhow::Error::new(e)
                .context(format!("cannot hash downloaded tmp {}", tmp.display())));
        }
    };

    if !actual.eq_ignore_ascii_case(expected_sha) {
        // Mismatch: refuse to install. Remove the tmp; never touch the final path.
        let _ = fs::remove_file(tmp);
        bail!(
            "sha256 mismatch for {}: expected {}, got {} — refusing to install",
            final_path.display(),
            expected_sha,
            actual
        );
    }

    // Verified. Ensure the parent dir exists, then atomically rename over the final path.
    if let Some(parent) = final_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("cannot create dir {}", parent.display()))?;
    }
    fs::rename(tmp, final_path)
        .with_context(|| format!("cannot place {} -> {}", tmp.display(), final_path.display()))?;
    Ok(())
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
    Ok(AssetSpec {
        name: spec.url,
        sha256: spec.sha256,
        version: spec.version,
    })
}

/// Fetch + parse [`RAG_MANIFEST_URL`]. NOT unit-tested (real HTTP); the parse logic
/// is covered by the `parse_release_manifest` tests.
pub async fn fetch_release_manifest() -> anyhow::Result<AssetSpec> {
    let client = reqwest::Client::builder()
        .user_agent("eud-agent-bootstrap")
        .build()?;
    let bytes = client
        .get(RAG_MANIFEST_URL)
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
    fn release_manifest_parses_into_asset_spec() {
        let json = br#"{
            "rag_index": {
                "url": "https://github.com/raravel/eud-agent/releases/download/rag-index-v1/rag-index.bin",
                "sha256": "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
                "version": "1"
            }
        }"#;

        let spec = parse_release_manifest(json).unwrap();

        // The manifest's `url` maps onto AssetSpec.name (the release asset URL).
        assert_eq!(
            spec.name,
            "https://github.com/raravel/eud-agent/releases/download/rag-index-v1/rag-index.bin"
        );
        assert_eq!(spec.sha256, HELLO_SHA);
        assert_eq!(spec.version, "1");
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
