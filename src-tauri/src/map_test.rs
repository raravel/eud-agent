//! Map runtime tests (Windows only).
//!
//! A built map is played headless by `eud-runner`, the runner of the `raravel/sc4j-scr` fork, on the
//! pinned SC4J engine runtime (its bundled JDK and engine jars) with the fork's SC:R mod. Everything is
//! downloaded on demand from Settings, pinned by URL, size and SHA-256, and kept in LocalAppData. While
//! the runtime is enabled and installed, `build_run` adds a short crash check (`runtime`) to a
//! successful build and the model is offered `map_test_run`, which runs the project's tests: JavaScript
//! (`tests/*.test.js`, run in the game's SC:R Enhanced script runtime) and the older `.mtest` scripts.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::bootstrap::process_tree::{run_process_tree, ProcessCancellation, ProcessEnd};
use crate::bootstrap::{sha256_file, ProgressEmitter};
use crate::config::DataDirs;

/// The SC4J engine release whose runtime (JDK and engine jars) runs the runner.
pub const ENGINE_VERSION: &str = "0.1.9";
const ENGINE_URL: &str =
    "https://github.com/fogofwarlabs/sc4j/releases/download/v0.1.9/SC4J-0.1.9.zip";
const ENGINE_SHA256: &str = "b630981225b892014041acd61b52ed2f08557aab3a369c9a889b422dba40578e";
const ENGINE_BYTES: u64 = 117_548_535;
const ENGINE_ARCHIVE: &str = "SC4J-0.1.9.zip";
/// The single top folder of the engine archive.
const ENGINE_ROOT: &str = "SC4J-0.1.9";
/// The parts of the engine distribution the runner needs: the JDK, the engine jars and the licence texts.
const ENGINE_KEPT: [&str; 5] = ["java/", "runtime/", "licenses/", "LICENSE", "NOTICE"];
const MAX_ENGINE_FILES: usize = 4_000;
const MAX_ENGINE_BYTES: u64 = 768 * 1024 * 1024;

/// The fork release holding the SC:R mod and the runner built for [`ENGINE_VERSION`].
pub const MOD_RELEASE: &str = "v0.1.10-eud.1";

/// A jar of [`MOD_RELEASE`], downloaded from the fork's release and put on the class path in this order.
struct ModJar {
    file: &'static str,
    sha256: &'static str,
    bytes: u64,
}

/// The SC:R mod, the runner, the SC:R Enhanced script runtime the JavaScript tests run in, and Rhino (MPL-2.0).
const MOD_JARS: [ModJar; 4] = [
    ModJar {
        file: "scr.jar",
        sha256: "0e820a1a0be7cea5d5dae9d016b0417da5bbefce68f333cde7f23a235f4e616a",
        bytes: 5_111_811,
    },
    ModJar {
        file: "eud-runner.jar",
        sha256: "ead6a44fa1eec5c8e63b697ccc36d75e4ace780e8d8ae0c1851335a74bb5d318",
        bytes: 126_224,
    },
    ModJar {
        file: "scr-enhanced-script.jar",
        sha256: "002ccc73d23d882236d81e373550155e78e3cccf6de75df28baea5e484b92c6a",
        bytes: 523_969,
    },
    ModJar {
        file: "rhino-1.8.1.jar",
        sha256: "98a06d42bdd69da15bf9c2a7812b4cfe648ca9e64fe8baaeb502a32b0ed2c8ac",
        bytes: 1_271_206,
    },
];

/// The install folder name: one immutable bundle per pinned engine and mod release.
pub const BUNDLE: &str = "sc4j-0.1.9+scr-v0.1.10-eud.1";
const MARKER: &str = ".maptest-install.json";
const RUNNER_MAIN: &str = "org.fogofwarlabs.sc4j.scr.eud.runner.RunnerMain";

const fn mod_jar_bytes() -> u64 {
    let mut total = 0;
    let mut index = 0;
    while index < MOD_JARS.len() {
        total += MOD_JARS[index].bytes;
        index += 1;
    }
    total
}

/// Download bytes reported to the user before enabling.
pub const DOWNLOAD_BYTES: u64 = ENGINE_BYTES + mod_jar_bytes();

/// The frames of the crash check after a successful build (about 100 s of game time at the fastest speed).
pub const CRASH_CHECK_FRAMES: u32 = 2400;
/// The wall time one frame may take before the run counts as hung.
pub const FRAME_TIMEOUT: Duration = Duration::from_secs(10);
/// The longest wall time of one run.
pub const RUN_TIME_LIMIT: Duration = Duration::from_secs(120);
/// A run is not started with less than this left of its tool call's budget.
pub const MIN_RUN_TIME: Duration = Duration::from_secs(15);
/// The JVM start, the installed data load and the report write around the simulation.
const RUN_OVERHEAD: Duration = Duration::from_secs(25);

static AVAILABLE: AtomicBool = AtomicBool::new(false);

/// Whether map tests are offered: enabled in Settings and installed (Windows only).
pub fn available() -> bool {
    AVAILABLE.load(Ordering::Relaxed)
}

/// Re-reads the Settings switch and the installed runtime, and returns [`available`].
pub fn refresh_availability(dirs: &DataDirs) -> bool {
    let enabled = dirs
        .load_config()
        .map(|config| config.map_test.enabled)
        .unwrap_or(false);
    let ready = cfg!(windows) && enabled && installed(dirs).is_ok();
    AVAILABLE.store(ready, Ordering::Relaxed);
    ready
}

/// The installed runtime: the java launcher and the class path of engine jars, mod and runner.
#[derive(Debug, Clone)]
pub struct RuntimePaths {
    pub java: PathBuf,
    pub classpath: String,
    /// LWJGL's native libraries, which a `shot` loads to draw the screen.
    pub natives: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InstalledFile {
    path: String,
    bytes: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct InstallMarker {
    bundle: String,
    engine_sha256: String,
    /// The SHA-256 of every [`MOD_JARS`] file, in that order.
    jar_sha256: Vec<String>,
    files: Vec<InstalledFile>,
}

fn root_dir(dirs: &DataDirs) -> PathBuf {
    dirs.app_local_data().join("maptest")
}

fn install_dir(dirs: &DataDirs) -> PathBuf {
    root_dir(dirs).join(BUNDLE)
}

fn is_reparse(metadata: &fs::Metadata) -> bool {
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
        .with_context(|| format!("맵 테스트 폴더가 없습니다: {}", path.display()))?;
    if !metadata.file_type().is_dir() || is_reparse(&metadata) {
        bail!("맵 테스트 폴더가 일반 폴더가 아닙니다: {}", path.display());
    }
    Ok(())
}

/// Validates the installed bundle without hashing it (the archive and jars were hashed when installed):
/// the marker names this bundle's pins, and every listed file is a plain file of the listed size.
pub fn installed(dirs: &DataDirs) -> anyhow::Result<RuntimePaths> {
    let dir = install_dir(dirs);
    require_plain_directory(&dir)?;
    let marker_path = dir.join(MARKER);
    let metadata = fs::symlink_metadata(&marker_path).context("맵 테스트 설치 표식이 없습니다.")?;
    if !metadata.file_type().is_file() || is_reparse(&metadata) || metadata.len() > 1024 * 1024 {
        bail!("맵 테스트 설치 표식이 일반 파일이 아닙니다.");
    }
    let marker: InstallMarker = serde_json::from_slice(&fs::read(&marker_path)?)
        .context("맵 테스트 설치 표식이 올바르지 않습니다.")?;
    if marker.bundle != BUNDLE
        || marker.engine_sha256 != ENGINE_SHA256
        || !marker.jar_sha256.iter().map(String::as_str).eq(MOD_JARS.iter().map(|jar| jar.sha256))
    {
        bail!("맵 테스트 설치 표식의 버전이 맞지 않습니다.");
    }
    let mut listed = HashSet::new();
    for file in &marker.files {
        let relative = safe_relative_path(&file.path)?;
        let path = dir.join(&relative);
        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("맵 테스트 설치 파일이 없습니다: {}", file.path))?;
        if !metadata.file_type().is_file() || is_reparse(&metadata) || metadata.len() != file.bytes {
            bail!("맵 테스트 설치 파일이 손상되었습니다: {}", file.path);
        }
        listed.insert(file.path.to_lowercase());
    }
    let java = if cfg!(windows) { "engine/java/bin/java.exe" } else { "engine/java/bin/java" };
    for required in std::iter::once(java).chain(MOD_JARS.iter().map(|jar| jar.file)) {
        if !listed.contains(required) {
            bail!("맵 테스트 설치에 {required}이(가) 없습니다.");
        }
    }
    let separator = if cfg!(windows) { ";" } else { ":" };
    let classpath = std::iter::once(dir.join("engine/runtime/lib/*"))
        .chain(MOD_JARS.iter().map(|jar| dir.join(jar.file)))
        .collect::<Vec<_>>()
        .iter()
    .map(|path| path.to_string_lossy().into_owned())
    .collect::<Vec<_>>()
    .join(separator);
    Ok(RuntimePaths {
        java: dir.join(java),
        classpath,
        natives: dir.join("engine/runtime/lib/natives"),
    })
}

/// A relative archive or marker path: `/`-separated, no root, drive, `..`, `.`, empty or Windows-reserved component.
fn safe_relative_path(name: &str) -> anyhow::Result<PathBuf> {
    if name.is_empty() || name.contains('\\') || name.contains('\0') {
        bail!("맵 테스트 파일 경로가 올바르지 않습니다: {name}");
    }
    let trimmed = name.trim_end_matches('/');
    if trimmed.is_empty() || trimmed.starts_with('/') || trimmed.as_bytes().get(1) == Some(&b':') {
        bail!("맵 테스트 파일 경로가 절대 경로입니다: {name}");
    }
    let mut path = PathBuf::new();
    for component in trimmed.split('/') {
        if component.is_empty()
            || matches!(component, "." | "..")
            || component.ends_with(['.', ' '])
            || component
                .chars()
                .any(|character| character.is_control() || "<>:\"|?*".contains(character))
        {
            bail!("맵 테스트 파일 경로가 설치 폴더를 벗어납니다: {name}");
        }
        path.push(component);
    }
    Ok(path)
}

/// The engine archive entry below [`ENGINE_ROOT`] the runtime keeps, or `None` for a skipped one.
fn kept_engine_entry(name: &str) -> anyhow::Result<Option<String>> {
    let Some(rest) = name.strip_prefix(ENGINE_ROOT).and_then(|rest| rest.strip_prefix('/')) else {
        if name.trim_end_matches('/') == ENGINE_ROOT {
            return Ok(None);
        }
        bail!("엔진 압축 파일에 예상하지 못한 항목이 있습니다: {name}");
    };
    if rest.is_empty() {
        return Ok(None);
    }
    let kept = ENGINE_KEPT
        .iter()
        .any(|prefix| if prefix.ends_with('/') { rest.starts_with(prefix) } else { rest == *prefix });
    Ok(kept.then(|| rest.to_string()))
}

fn extract_engine(archive_path: &Path, destination: &Path) -> anyhow::Result<Vec<InstalledFile>> {
    let mut archive = zip::ZipArchive::new(File::open(archive_path)?)
        .context("엔진 배포 파일이 올바른 ZIP이 아닙니다.")?;
    if archive.is_empty() || archive.len() > MAX_ENGINE_FILES {
        bail!("엔진 압축 파일의 항목 수가 허용 범위를 벗어났습니다.");
    }
    let mut seen = HashSet::new();
    let mut files = Vec::new();
    let mut total = 0_u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = entry.name().to_owned();
        let Some(relative_name) = kept_engine_entry(&name)? else {
            continue;
        };
        let relative = safe_relative_path(&relative_name)?;
        if entry.unix_mode().is_some_and(|mode| mode & 0o170000 == 0o120000) {
            bail!("엔진 압축 파일에 심볼릭 링크가 있습니다: {name}");
        }
        if !seen.insert(relative_name.trim_end_matches('/').to_lowercase()) {
            bail!("엔진 압축 파일에 중복 경로가 있습니다: {name}");
        }
        let output = destination.join(&relative);
        if entry.is_dir() {
            fs::create_dir_all(&output)?;
            continue;
        }
        let bytes = entry.size();
        total = total.checked_add(bytes).context("엔진 압축 해제 크기가 넘쳤습니다.")?;
        if total > MAX_ENGINE_BYTES {
            bail!("엔진 압축 해제 크기가 허용 범위를 벗어났습니다.");
        }
        fs::create_dir_all(output.parent().context("엔진 파일에 상위 폴더가 없습니다.")?)?;
        let mut file = File::create_new(&output)?;
        std::io::copy(&mut entry, &mut file)?;
        file.flush()?;
        if file.metadata()?.len() != bytes {
            bail!("엔진 파일이 완전히 기록되지 않았습니다: {name}");
        }
        files.push(InstalledFile {
            path: format!("engine/{}", relative_name),
            bytes,
        });
    }
    Ok(files)
}

struct Download {
    url: String,
    sha256: &'static str,
    bytes: u64,
    file: &'static str,
    label: &'static str,
}

/// The engine archive, then every [`MOD_JARS`] file.
fn downloads() -> Vec<Download> {
    std::iter::once(Download {
        url: ENGINE_URL.to_string(),
        sha256: ENGINE_SHA256,
        bytes: ENGINE_BYTES,
        file: ENGINE_ARCHIVE,
        label: "SC4J 0.1.9",
    })
    .chain(MOD_JARS.iter().map(|jar| Download {
        url: format!("https://github.com/raravel/sc4j-scr/releases/download/{MOD_RELEASE}/{}", jar.file),
        sha256: jar.sha256,
        bytes: jar.bytes,
        file: jar.file,
        label: jar.file,
    }))
    .collect()
}

/// Downloads, verifies and publishes the pinned bundle; returns the installed runtime. Older bundles are removed.
pub async fn ensure_installed(
    dirs: &DataDirs,
    emitter: &(dyn ProgressEmitter + Send + Sync),
) -> anyhow::Result<RuntimePaths> {
    if !cfg!(windows) {
        bail!("맵 실행 테스트는 Windows에서만 쓸 수 있습니다.");
    }
    if let Ok(runtime) = installed(dirs) {
        return Ok(runtime);
    }
    let root = root_dir(dirs);
    fs::create_dir_all(&root)?;
    require_plain_directory(&root)?;
    let staging = root.join(format!(".staging-{}", uuid::Uuid::new_v4().simple()));
    fs::create_dir(&staging)?;
    let result = async {
        for (index, download) in downloads().iter().enumerate() {
            let target = staging.join(download.file);
            // The engine archive takes 90% of the bar, each jar 2%.
            let stage = Stage {
                inner: emitter,
                first: if index == 0 { 0 } else { 90 + 2 * (index as u8 - 1) },
                span: if index == 0 { 90 } else { 2 },
            };
            crate::bootstrap::download_to_tmp(&download.url, &target, download.label, &stage).await?;
            let bytes = fs::metadata(&target)?.len();
            if bytes != download.bytes {
                bail!(
                    "{}의 크기가 고정값과 다릅니다 ({bytes} != {}).",
                    download.label,
                    download.bytes
                );
            }
            if !sha256_file(&target)?.eq_ignore_ascii_case(download.sha256) {
                bail!("{}의 SHA-256이 고정값과 다릅니다.", download.label);
            }
        }
        emitter.emit("map_test_install", 98, "extract");
        let payload = staging.join("payload");
        let staged = staging.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            fs::create_dir(&payload)?;
            let mut files = extract_engine(&staged.join(ENGINE_ARCHIVE), &payload.join("engine"))?;
            for jar in &MOD_JARS {
                fs::rename(staged.join(jar.file), payload.join(jar.file))?;
                files.push(InstalledFile {
                    path: jar.file.to_string(),
                    bytes: jar.bytes,
                });
            }
            files.sort_by(|left, right| left.path.cmp(&right.path));
            let marker = InstallMarker {
                bundle: BUNDLE.to_string(),
                engine_sha256: ENGINE_SHA256.to_string(),
                jar_sha256: MOD_JARS.iter().map(|jar| jar.sha256.to_string()).collect(),
                files,
            };
            fs::write(payload.join(MARKER), serde_json::to_vec(&marker)?)?;
            Ok(())
        })
        .await
        .context("맵 테스트 압축 해제 작업이 중단되었습니다.")??;
        let target = install_dir(dirs);
        if target.exists() {
            fs::remove_dir_all(&target).context("손상된 맵 테스트 설치를 지우지 못했습니다.")?;
        }
        fs::rename(staging.join("payload"), &target).context("맵 테스트 설치를 게시하지 못했습니다.")?;
        installed(dirs)
    }
    .await;
    let _ = fs::remove_dir_all(&staging);
    if result.is_ok() {
        remove_other_bundles(&root);
        emitter.emit("map_test_install", 100, "done");
    }
    result
}

/// Maps a download's own 0..=100 onto its share of the install progress.
struct Stage<'a> {
    inner: &'a (dyn ProgressEmitter + Send + Sync),
    first: u8,
    span: u8,
}

impl ProgressEmitter for Stage<'_> {
    fn emit(&self, _stage: &str, pct: u8, detail: &str) {
        let scaled = self.first as u32 + self.span as u32 * pct.min(100) as u32 / 100;
        self.inner.emit("map_test_install", scaled as u8, detail);
    }
}

/// Whether `root` holds a bundle folder of another pin, as an update leaves it until the new one is installed.
fn has_other_bundle(root: &Path) -> bool {
    let Ok(entries) = fs::read_dir(root) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let name = entry.file_name().to_string_lossy().into_owned();
        name.starts_with("sc4j-")
            && name != BUNDLE
            && entry.path().join(MARKER).is_file()
    })
}

fn remove_other_bundles(root: &Path) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let plain = entry
            .path()
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.is_dir() && !is_reparse(&metadata));
        if name != BUNDLE && plain {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// Deletes every downloaded runtime file.
pub fn remove_installed(dirs: &DataDirs) -> anyhow::Result<()> {
    let root = root_dir(dirs);
    match fs::symlink_metadata(&root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
        Ok(_) => require_plain_directory(&root)?,
    }
    fs::remove_dir_all(&root).context("맵 테스트 파일을 지우지 못했습니다. 실행 중인 테스트가 끝난 뒤 다시 시도하세요.")?;
    Ok(())
}

/// One runner invocation.
#[derive(Debug, Clone)]
pub struct RunRequest<'a> {
    pub map: &'a Path,
    pub script: Option<&'a Path>,
    pub frames: Option<u32>,
    pub epmap: Option<&'a Path>,
    pub profile: bool,
    pub time_limit: Duration,
    /// Where a script's `shot` lines write their PNGs.
    pub shots_dir: Option<&'a Path>,
}

/// Runs the runner and returns its report. `Err` only when the run could not happen at all
/// (not installed, no StarCraft data, a usage error, a cancelled call); a map failure is a report.
pub fn run(
    dirs: &DataDirs,
    request: &RunRequest<'_>,
    cancellation: Option<&ProcessCancellation>,
) -> Result<Value, String> {
    let runtime = installed(dirs).map_err(|error| {
        format!("맵 실행 테스트가 설치되어 있지 않습니다: {error}. 사용자가 설정 → 컴파일에서 맵 실행 테스트를 켜야 합니다.")
    })?;
    let data = crate::map_context::resolve_starcraft_path(dirs)?;
    let scratch = std::env::temp_dir().join(format!("eud-agent-maptest-{}", uuid::Uuid::new_v4().simple()));
    fs::create_dir_all(&scratch).map_err(|error| format!("맵 테스트 임시 폴더를 만들지 못했습니다: {error}"))?;
    let out = scratch.join("report.json");
    let mut command = Command::new(&runtime.java);
    command
        .arg("-XX:-UsePerfData")
        .arg("--enable-native-access=ALL-UNNAMED")
        .arg(format!("-Dorg.lwjgl.librarypath={}", runtime.natives.display()))
        .arg(format!("-Djava.io.tmpdir={}", scratch.display()))
        .arg("-cp")
        .arg(&runtime.classpath)
        .arg(RUNNER_MAIN)
        .arg("--data")
        .arg(&data)
        .arg("--map")
        .arg(request.map)
        .arg("--out")
        .arg(&out)
        .arg("--frame-timeout-ms")
        .arg(FRAME_TIMEOUT.as_millis().to_string())
        .arg("--time-limit-ms")
        .arg(request.time_limit.as_millis().to_string());
    if let Some(script) = request.script {
        command.arg("--script").arg(script);
    }
    if let Some(frames) = request.frames {
        command.arg("--frames").arg(frames.to_string());
    }
    if let Some(epmap) = request.epmap {
        command.arg("--epmap").arg(epmap);
    }
    if request.profile {
        command.arg("--profile");
    }
    if let Some(shots) = request.shots_dir {
        command.arg("--shots-dir").arg(shots);
    }
    command.current_dir(&scratch);
    let timeout = request.time_limit + FRAME_TIMEOUT + RUN_OVERHEAD;
    let output = run_process_tree(command, timeout, cancellation);
    let report = match output {
        Err(error) => Err(format!("맵 테스트 러너를 실행하지 못했습니다: {error}")),
        Ok(output) => match output.end {
            ProcessEnd::Cancelled => Err("맵 테스트가 취소되었습니다.".to_string()),
            ProcessEnd::TimedOut => Ok(json!({
                "runtime": {"status": "timeout", "error": format!("러너가 {}초 안에 끝나지 않았습니다.", timeout.as_secs())}
            })),
            ProcessEnd::Exited(code) => match fs::read(&out) {
                Ok(bytes) => serde_json::from_slice::<Value>(&bytes)
                    .map_err(|error| format!("맵 테스트 결과를 읽지 못했습니다: {error}")),
                Err(_) => {
                    let stderr = output.stderr_lossy();
                    let tail: String = stderr.chars().rev().take(1500).collect::<Vec<_>>().into_iter().rev().collect();
                    Err(format!("맵 테스트 러너가 결과 없이 끝났습니다 (종료 코드 {code}): {tail}"))
                }
            },
        },
    };
    let _ = fs::remove_dir_all(&scratch);
    report
}

/// The `runtime` summary of a crash check for the `build_run` observation.
pub fn crash_check_summary(report: &Value) -> Value {
    let mut runtime = report
        .get("runtime")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    copy_field(report, &mut runtime, "framesRun");
    copy_field(report, &mut runtime, "framesPlanned");
    add_status_note(&mut runtime);
    Value::Object(runtime)
}

/// The `runtime` of a crash check that did not run.
pub fn skipped(reason: &str, detail: Option<String>) -> Value {
    let mut runtime = json!({"status": "skipped", "reason": reason});
    if let Some(detail) = detail {
        runtime["detail"] = Value::String(detail);
    }
    runtime
}

fn copy_field(from: &Value, to: &mut Map<String, Value>, key: &str) {
    if let Some(value) = from.get(key) {
        to.insert(key.to_string(), value.clone());
    }
}

/// Adds the model-facing reading of a runtime status.
fn add_status_note(runtime: &mut Map<String, Value>) {
    let note = match runtime.get("status").and_then(Value::as_str) {
        Some("unsupported") => Some(
            "에뮬레이터가 지원하지 않는 기법입니다. 맵 버그가 아니므로 이 결과 때문에 코드를 바꾸지 마세요. 실제 게임 확인을 사용자에게 맡기세요.",
        ),
        Some("failed") => Some(
            "맵이 에뮬레이터에서 실행 중 멈췄습니다. 같은 턴 안에서 원인을 고치세요. 위치가 필요하면 map_test_run {trace: true}로 실행 중이던 EPS 줄을 확인하세요.",
        ),
        Some("hang") => Some(
            "프레임 하나가 끝나지 않았습니다(트리거 무한 루프 의심). map_test_run {trace: true}로 멈춘 EPS 줄을 확인하고 고치세요.",
        ),
        _ => None,
    };
    if let Some(note) = note {
        runtime.insert("note".to_string(), Value::String(note.to_string()));
    }
    if runtime.get("stoppedBy").and_then(Value::as_str) == Some("time_limit") {
        runtime.insert(
            "note".to_string(),
            Value::String("시간 한도 안에 다 돌지 못했습니다. 실패가 아니며 framesRun까지만 확인되었습니다.".to_string()),
        );
    }
}

/// Bounds one script report for the model: passed text checks drop the screen they read, and long lists are cut.
pub fn test_report_for_model(path: &str, report: &Value) -> Value {
    let mut out = Map::new();
    out.insert("path".to_string(), Value::String(path.to_string()));
    let script = report.get("script").cloned().unwrap_or(Value::Null);
    let status = script
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("error")
        .to_string();
    let result = match status.as_str() {
        "passed" => "passed",
        "failed" | "incomplete" => "failed",
        "inconclusive" => "inconclusive",
        _ => "error",
    };
    out.insert("result".to_string(), Value::String(result.to_string()));
    out.insert("scriptStatus".to_string(), Value::String(status));
    let mut runtime = report
        .get("runtime")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    add_status_note(&mut runtime);
    out.insert("runtime".to_string(), Value::Object(runtime));
    if let Some(problems) = script.get("problems").and_then(Value::as_array) {
        out.insert(
            "problems".to_string(),
            Value::Array(problems.iter().take(50).cloned().collect()),
        );
    }
    if let Some(expects) = script.get("expects").and_then(Value::as_array) {
        let bounded: Vec<Value> = expects
            .iter()
            .take(100)
            .map(|expect| {
                let mut expect = expect.clone();
                if expect.get("ok") == Some(&Value::Bool(true)) && expect.get("actual").is_some_and(Value::is_array) {
                    if let Some(object) = expect.as_object_mut() {
                        object.remove("actual");
                    }
                }
                expect
            })
            .collect();
        out.insert("expects".to_string(), Value::Array(bounded));
        if expects.len() > 100 {
            out.insert("omittedExpects".to_string(), json!(expects.len() - 100));
        }
    }
    for key in ["error", "scriptErrors", "stoppedAt", "noTest"] {
        copy_field(&script, &mut out, key);
    }
    for key in ["framesRun", "framesPlanned", "screenText", "units", "profile", "readError"] {
        copy_field(report, &mut out, key);
    }
    if let Some(shots) = script.get("shots").and_then(Value::as_array) {
        out.insert("shots".to_string(), Value::Array(shots.iter().take(16).cloned().collect()));
    }
    if let Some(log) = js_log_for_model(report) {
        out.insert("jsLog".to_string(), log);
    }
    Value::Object(out)
}

/// The most script log lines one test result shows the model: the newest, which hold the error that ended it.
const JS_LOG_LINES: usize = 30;
/// The longest log message shown, in characters.
const JS_LOG_MESSAGE_CHARS: usize = 300;

/// The newest lines of a JavaScript test's log (`log.*` and the script errors), or `None` without any.
fn js_log_for_model(report: &Value) -> Option<Value> {
    let lines = report.get("js")?.get("log")?.as_array()?;
    if lines.is_empty() {
        return None;
    }
    let skip = lines.len().saturating_sub(JS_LOG_LINES);
    let shown: Vec<Value> = lines[skip..]
        .iter()
        .map(|line| {
            let mut line = line.clone();
            if let Some(message) = line.get("message").and_then(Value::as_str) {
                if message.chars().count() > JS_LOG_MESSAGE_CHARS {
                    let cut: String = message.chars().take(JS_LOG_MESSAGE_CHARS).collect();
                    line["message"] = Value::String(format!("{cut}…"));
                }
            }
            line
        })
        .collect();
    let mut log = json!({"lines": shown});
    let omitted = skip as u64
        + report
            .get("js")
            .and_then(|js| js.get("omittedLines"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
    if omitted > 0 {
        log["omittedLines"] = json!(omitted);
    }
    Some(log)
}

/// The most shots one `map_test_run` shows the model as pictures; later ones are listed by path.
pub const SHOTS_SHOWN: usize = 4;

/// Puts up to [`SHOTS_SHOWN`] screenshots side by side, two to a row, as one PNG image envelope
/// (`{mimeType, width, height, data}`): a tool result carries one image.
pub fn shot_sheet(files: &[PathBuf]) -> Result<Value, String> {
    use base64::Engine as _;
    let mut pictures = Vec::new();
    for file in files.iter().take(SHOTS_SHOWN) {
        let picture = image::open(file)
            .map_err(|error| format!("스크린샷 {}을(를) 읽지 못했습니다: {error}", file.display()))?
            .to_rgb8();
        pictures.push(picture);
    }
    if pictures.is_empty() {
        return Err("스크린샷이 없습니다.".to_string());
    }
    let cell_width = pictures.iter().map(|picture| picture.width()).max().unwrap_or(0);
    let cell_height = pictures.iter().map(|picture| picture.height()).max().unwrap_or(0);
    let columns = pictures.len().min(2) as u32;
    let rows = (pictures.len() as u32).div_ceil(2);
    let mut sheet = image::RgbImage::new(cell_width * columns, cell_height * rows);
    for (index, picture) in pictures.iter().enumerate() {
        let index = index as u32;
        image::imageops::replace(
            &mut sheet,
            picture,
            i64::from((index % 2) * cell_width),
            i64::from((index / 2) * cell_height),
        );
    }
    let (width, height) = sheet.dimensions();
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(sheet)
        .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
        .map_err(|error| format!("스크린샷을 묶지 못했습니다: {error}"))?;
    Ok(json!({
        "mimeType": "image/png",
        "width": width,
        "height": height,
        "data": base64::engine::general_purpose::STANDARD.encode(bytes),
    }))
}

static INSTALLING: AtomicBool = AtomicBool::new(false);

/// Settings → 컴파일 → 맵 실행 테스트.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapTestSettingsView {
    /// Windows only.
    pub supported: bool,
    pub enabled: bool,
    pub installed: bool,
    pub installing: bool,
    /// An older bundle is installed and this version's is not: the switch stays off until the user downloads again.
    pub outdated: bool,
    /// The pinned engine and mod release.
    pub bundle: &'static str,
    pub download_bytes: u64,
}

fn settings_view(dirs: &DataDirs) -> MapTestSettingsView {
    let enabled = dirs
        .load_config()
        .map(|config| config.map_test.enabled)
        .unwrap_or(false);
    let installed = installed(dirs).is_ok();
    MapTestSettingsView {
        supported: cfg!(windows),
        enabled,
        installed,
        installing: INSTALLING.load(Ordering::Relaxed),
        outdated: !installed && has_other_bundle(&root_dir(dirs)),
        bundle: BUNDLE,
        download_bytes: DOWNLOAD_BYTES,
    }
}

fn save_enabled(dirs: &DataDirs, enabled: bool) -> Result<(), String> {
    let mut config = dirs.load_config().map_err(|error| error.to_string())?;
    if config.map_test.enabled != enabled {
        config.map_test.enabled = enabled;
        dirs.save_config(&config).map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Reads the map test switch and install state.
#[tauri::command]
pub async fn map_test_settings(
    state: tauri::State<'_, crate::ipc::AppManaged>,
) -> Result<MapTestSettingsView, String> {
    let dirs = state.dirs().clone();
    tauri::async_runtime::spawn_blocking(move || settings_view(&dirs))
        .await
        .map_err(|error| error.to_string())
}

/// Turns map tests on (downloading the pinned runtime when it is missing, with `progress` events of stage
/// `map_test_install`) or off. A failed download leaves them off.
#[tauri::command]
pub async fn map_test_settings_save(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::ipc::AppManaged>,
    enabled: bool,
) -> Result<MapTestSettingsView, String> {
    let dirs = state.dirs().clone();
    if enabled && !cfg!(windows) {
        return Err(crate::tr!("map_test.unsupported").to_string());
    }
    if enabled && installed(&dirs).is_err() {
        if INSTALLING.swap(true, Ordering::AcqRel) {
            return Err(crate::tr!("map_test.install_running").to_string());
        }
        let emitter = crate::bootstrap::TauriEmitter(app);
        let result = ensure_installed(&dirs, &emitter).await;
        INSTALLING.store(false, Ordering::Release);
        if let Err(error) = result {
            let _ = save_enabled(&dirs, false);
            refresh_availability(&dirs);
            return Err(crate::tr!("map_test.install_failed", error = format!("{error:#}")).to_string());
        }
    }
    tauri::async_runtime::spawn_blocking(move || {
        save_enabled(&dirs, enabled)?;
        refresh_availability(&dirs);
        Ok(settings_view(&dirs))
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Turns map tests off and deletes the downloaded runtime.
#[tauri::command]
pub async fn map_test_remove(
    state: tauri::State<'_, crate::ipc::AppManaged>,
) -> Result<MapTestSettingsView, String> {
    if INSTALLING.load(Ordering::Acquire) {
        return Err(crate::tr!("map_test.install_running").to_string());
    }
    let dirs = state.dirs().clone();
    tauri::async_runtime::spawn_blocking(move || {
        save_enabled(&dirs, false)?;
        refresh_availability(&dirs);
        remove_installed(&dirs)
            .map_err(|error| crate::tr!("map_test.remove_failed", error = format!("{error:#}")).to_string())?;
        Ok(settings_view(&dirs))
    })
    .await
    .map_err(|error| error.to_string())?
}

/// The report of a run without a script: `passed` when the map ran (to the end or the time limit), `inconclusive`
/// on an unsupported technique, `failed` otherwise.
pub fn plain_report_for_model(report: &Value) -> Value {
    let mut runtime = report
        .get("runtime")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let result = match runtime.get("status").and_then(Value::as_str) {
        Some("ok") => "passed",
        Some("unsupported") => "inconclusive",
        _ => "failed",
    };
    add_status_note(&mut runtime);
    let mut out = Map::new();
    out.insert("path".to_string(), Value::Null);
    out.insert("result".to_string(), Value::String(result.to_string()));
    out.insert("runtime".to_string(), Value::Object(runtime));
    for key in ["framesRun", "framesPlanned", "screenText", "units", "profile", "readError"] {
        copy_field(report, &mut out, key);
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Silent;

    impl ProgressEmitter for Silent {
        fn emit(&self, _stage: &str, _pct: u8, _detail: &str) {}
    }

    /// Downloads the pinned bundle (about 117 MB) and validates it. Rerun after bumping a pin:
    /// `cargo test --lib -- --ignored map_test::tests::the_pinned_bundle_installs`. It installs below a
    /// temporary folder, or below `EUD_MAPTEST_LOCAL_BASE` (a `%LOCALAPPDATA%`-like base) when set.
    #[test]
    #[ignore = "downloads the pinned runtime from GitHub"]
    fn the_pinned_bundle_installs() {
        let temporary = std::env::temp_dir().join(format!("eud-maptest-{}", uuid::Uuid::new_v4().simple()));
        let local = std::env::var_os("EUD_MAPTEST_LOCAL_BASE")
            .map(PathBuf::from)
            .unwrap_or_else(|| temporary.join("local"));
        let dirs = DataDirs::from_bases(&temporary.join("roaming"), &local);
        let runtime = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(ensure_installed(&dirs, &Silent))
            .unwrap();
        assert!(runtime.java.is_file());
        assert!(installed(&dirs).is_ok());
        let _ = fs::remove_dir_all(&temporary);
    }

    #[test]
    fn engine_entries_keep_only_the_runtime_parts() {
        assert_eq!(kept_engine_entry("SC4J-0.1.9/").unwrap(), None);
        assert_eq!(
            kept_engine_entry("SC4J-0.1.9/java/bin/java.exe").unwrap().as_deref(),
            Some("java/bin/java.exe")
        );
        assert_eq!(
            kept_engine_entry("SC4J-0.1.9/runtime/lib/sc4j-engine-0.1.9.jar").unwrap().as_deref(),
            Some("runtime/lib/sc4j-engine-0.1.9.jar")
        );
        assert_eq!(kept_engine_entry("SC4J-0.1.9/LICENSE").unwrap().as_deref(), Some("LICENSE"));
        assert_eq!(kept_engine_entry("SC4J-0.1.9/launcher/lib/a.jar").unwrap(), None);
        assert_eq!(kept_engine_entry("SC4J-0.1.9/SC4J.exe").unwrap(), None);
        assert!(kept_engine_entry("other/java/bin/java.exe").is_err());
    }

    #[test]
    fn relative_paths_refuse_escapes_and_reserved_names() {
        assert!(safe_relative_path("engine/java/bin/java.exe").is_ok());
        for bad in ["../x", "/x", "C:/x", "a\\b", "a/./b", "a/b.", "a/b?", ""] {
            assert!(safe_relative_path(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn crash_summary_adds_frames_and_the_unsupported_note() {
        let report = json!({
            "runtime": {"status": "unsupported", "frame": 1, "feature": "UnsupportedOperationException: x"},
            "framesRun": 1, "framesPlanned": 2400, "screenText": ["a"]
        });
        let summary = crash_check_summary(&report);
        assert_eq!(summary["status"], "unsupported");
        assert_eq!(summary["framesRun"], 1);
        assert!(summary["note"].as_str().unwrap().contains("바꾸지 마세요"));
        assert!(summary.get("screenText").is_none());
    }

    #[test]
    fn a_time_limited_run_is_not_a_failure() {
        let report = json!({"runtime": {"status": "ok", "stoppedBy": "time_limit", "frame": 735}, "framesRun": 735});
        let summary = crash_check_summary(&report);
        assert!(summary["note"].as_str().unwrap().contains("실패가 아니며"));
    }

    #[test]
    fn test_reports_drop_the_screen_of_passed_text_checks() {
        let report = json!({
            "runtime": {"status": "ok"},
            "script": {"status": "failed", "expects": [
                {"line": 3, "expect": "expect text contains \"a\"", "ok": true, "actual": ["a"], "frame": 10},
                {"line": 4, "expect": "expect text contains \"b\"", "ok": false, "actual": ["a"], "frame": 10}
            ]},
            "framesRun": 10
        });
        let bounded = test_report_for_model("tests/a.mtest", &report);
        assert_eq!(bounded["result"], "failed");
        assert!(bounded["expects"][0].get("actual").is_none());
        assert_eq!(bounded["expects"][1]["actual"], json!(["a"]));
    }

    #[test]
    fn script_problems_make_an_error_result() {
        let report = json!({
            "runtime": {"status": "skipped", "reason": "script_error"},
            "script": {"status": "error", "problems": [{"line": 2, "code": "unknown_key", "detail": "X", "usage": "key NAME [FRAMES]"}]}
        });
        let bounded = test_report_for_model("tests/a.mtest", &report);
        assert_eq!(bounded["result"], "error");
        assert_eq!(bounded["problems"][0]["code"], "unknown_key");
    }

    #[test]
    fn javascript_test_reports_carry_the_error_the_stop_and_the_newest_log() {
        let lines: Vec<Value> = (0..40)
            .map(|index| json!({"frame": index, "level": "info", "module": "a.test.js", "message": format!("line {index}")}))
            .collect();
        let report = json!({
            "runtime": {"status": "ok", "stoppedBy": "time_limit"},
            "script": {"status": "incomplete", "expects": [], "scriptErrors": 1,
                       "stoppedAt": {"line": 7, "step": "yield t.wait(100000);"}},
            "js": {"status": "errors", "errors": 1, "log": lines, "omittedLines": 2}
        });
        let bounded = test_report_for_model("tests/a.test.js", &report);
        assert_eq!(bounded["result"], "failed");
        assert_eq!(bounded["stoppedAt"]["line"], 7);
        assert_eq!(bounded["scriptErrors"], 1);
        let log = bounded["jsLog"]["lines"].as_array().unwrap();
        assert_eq!(log.len(), JS_LOG_LINES);
        assert_eq!(log.last().unwrap()["message"], "line 39");
        assert_eq!(bounded["jsLog"]["omittedLines"], 12);

        let failed = json!({"runtime": {"status": "ok"},
                            "script": {"status": "error", "expects": [], "error": "test failed: boom (a.test.js:3)"}});
        let bounded = test_report_for_model("tests/a.test.js", &failed);
        assert_eq!(bounded["result"], "error");
        assert_eq!(bounded["error"], "test failed: boom (a.test.js:3)");
        assert!(bounded.get("jsLog").is_none());
    }

    #[test]
    fn test_reports_keep_the_shots() {
        let report = json!({
            "runtime": {"status": "ok"},
            "script": {"status": "passed", "expects": [], "shots": [
                {"line": 2, "name": "start", "frame": 60, "file": "C:/p/build/test/shots/a/start.png", "width": 640, "height": 480},
                {"line": 5, "name": "after", "frame": 90, "error": "unavailable: Window creation failed."}
            ]}
        });
        let bounded = test_report_for_model("tests/a.mtest", &report);
        assert_eq!(bounded["shots"][0]["name"], "start");
        assert_eq!(bounded["shots"][1]["error"], "unavailable: Window creation failed.");
    }

    #[test]
    fn shot_sheets_put_two_shots_per_row() {
        let dir = std::env::temp_dir().join(format!("eud-agent-shot-sheet-{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir_all(&dir).unwrap();
        let files: Vec<PathBuf> = (0..5)
            .map(|index| {
                let file = dir.join(format!("{index}.png"));
                image::RgbImage::from_pixel(4, 3, image::Rgb([index * 40, 0, 0])).save(&file).unwrap();
                file
            })
            .collect();
        let size = |count: usize| {
            let sheet = shot_sheet(&files[..count]).unwrap();
            (sheet["width"].as_u64().unwrap(), sheet["height"].as_u64().unwrap())
        };
        assert_eq!(size(1), (4, 3));
        assert_eq!(size(2), (8, 3));
        assert_eq!(size(3), (8, 6));
        assert_eq!(size(5), (8, 6), "only the first four are drawn");
        let sheet = shot_sheet(&files[..3]).unwrap();
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(sheet["data"].as_str().unwrap())
            .unwrap();
        let picture = image::load_from_memory(&bytes).unwrap().to_rgb8();
        assert_eq!(picture.get_pixel(5, 1), &image::Rgb([40, 0, 0]));
        assert_eq!(picture.get_pixel(1, 4), &image::Rgb([80, 0, 0]));
        assert!(shot_sheet(&[]).is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
