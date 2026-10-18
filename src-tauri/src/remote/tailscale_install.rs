//! Install Tailscale on this PC from Settings → 원격 접속.
//!
//! The app downloads one pinned official MSI from pkgs.tailscale.com, checks
//! its exact size and SHA-256, holds it open against writes, and hands it to
//! `msiexec` through the UAC prompt. Tailscale updates itself afterwards; the
//! pin only decides which build a fresh install starts from.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use sha2::{Digest, Sha256};

/// One pinned official installer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Installer {
    pub file_name: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

pub const INSTALLER_VERSION: &str = "1.104.1";
const DOWNLOAD_BASE: &str = "https://pkgs.tailscale.com/stable/";
/// Both MSIs are Authenticode-signed by Tailscale Inc.; the digests are the
/// ones pkgs.tailscale.com publishes next to them, re-hashed on download.
const MSI_AMD64: Installer = Installer {
    file_name: "tailscale-setup-1.104.1-amd64.msi",
    size: 38_718_976,
    sha256: "6da8bcdb0d45a214cae0e8a88c2987ad48dec134bf04dffb9c05f1354c02b0b4",
};
const MSI_ARM64: Installer = Installer {
    file_name: "tailscale-setup-1.104.1-arm64.msi",
    size: 36_392_960,
    sha256: "cb9688912eb48faf2f752b084ee638d80545832634da5f1885514041071c95d0",
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// A stalled download fails instead of leaving the settings screen waiting.
const CHUNK_TIMEOUT: Duration = Duration::from_secs(60);

pub const INSTALL_PROGRESS_EVENT: &str = "remote_tailscale_install";

/// `remote_tailscale_install` progress, broadcast to every desktop window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "phase", rename_all = "camelCase")]
pub enum InstallProgress {
    Download {
        downloaded: u64,
        total: u64,
    },
    /// Waiting for the UAC prompt and the installer.
    Install,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallOutcome {
    /// Windows Installer asked for a restart (3010/1641).
    pub reboot_required: bool,
}

/// The installer for a native machine architecture (`amd64`/`arm64`).
pub fn installer_for(arch: &str) -> Option<Installer> {
    match arch {
        "amd64" => Some(MSI_AMD64),
        "arm64" => Some(MSI_ARM64),
        _ => None,
    }
}

/// The installer this PC needs; None where the app cannot install Tailscale.
pub fn installer() -> Option<Installer> {
    if !cfg!(windows) {
        return None;
    }
    native_arch().and_then(installer_for)
}

/// The machine's own architecture: an x64 build emulated on ARM64 Windows
/// still needs the arm64 MSI.
#[cfg(windows)]
fn native_arch() -> Option<&'static str> {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, IsWow64Process2};
    const IMAGE_FILE_MACHINE_AMD64: u16 = 0x8664;
    const IMAGE_FILE_MACHINE_ARM64: u16 = 0xAA64;
    let mut process_machine = 0u16;
    let mut native_machine = 0u16;
    let ok = unsafe {
        IsWow64Process2(
            GetCurrentProcess(),
            &mut process_machine,
            &mut native_machine,
        )
    };
    if ok != 0 {
        match native_machine {
            IMAGE_FILE_MACHINE_AMD64 => return Some("amd64"),
            IMAGE_FILE_MACHINE_ARM64 => return Some("arm64"),
            _ => {}
        }
    }
    match std::env::consts::ARCH {
        "x86_64" => Some("amd64"),
        "aarch64" => Some("arm64"),
        _ => None,
    }
}

#[cfg(not(windows))]
fn native_arch() -> Option<&'static str> {
    None
}

/// Lower-case hex SHA-256.
fn hex(digest: &[u8]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Whether `bytes` read so far still fit the pinned size.
fn check_size(installer: &Installer, downloaded: u64) -> Result<(), String> {
    if downloaded > installer.size {
        return Err(format!(
            "Tailscale 설치 파일이 예상보다 큽니다({downloaded} > {} 바이트). 잠시 후 다시 시도해 주세요.",
            installer.size
        ));
    }
    Ok(())
}

/// Compare a finished file's size and digest with the pin.
fn verify(installer: &Installer, size: u64, digest: &str) -> Result<(), String> {
    if size != installer.size || !digest.eq_ignore_ascii_case(installer.sha256) {
        return Err(
            "내려받은 Tailscale 설치 파일이 공식 파일과 다릅니다. 네트워크를 확인한 뒤 다시 시도해 주세요."
                .to_string(),
        );
    }
    Ok(())
}

/// Read and hash an open file from the start.
fn hash_file(file: &mut std::fs::File) -> Result<(u64, String), String> {
    use std::io::{Read, Seek, SeekFrom};
    file.seek(SeekFrom::Start(0))
        .map_err(|error| format!("설치 파일을 읽지 못했습니다: {error}"))?;
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut chunk)
            .map_err(|error| format!("설치 파일을 읽지 못했습니다: {error}"))?;
        if read == 0 {
            break;
        }
        size += read as u64;
        hasher.update(&chunk[..read]);
    }
    Ok((size, hex(&hasher.finalize())))
}

/// Open `path` so nothing can change, rename, or delete it while held; other
/// readers (msiexec) still can.
fn open_locked(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        options.share_mode(FILE_SHARE_READ);
    }
    options.open(path)
}

async fn download(
    installer: &Installer,
    partial: &Path,
    progress: &(dyn Fn(InstallProgress) + Send + Sync),
) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    let client = reqwest::Client::builder()
        .user_agent("eud-agent")
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .map_err(|error| error.to_string())?;
    let url = format!("{DOWNLOAD_BASE}{}", installer.file_name);
    let mut response = client
        .get(&url)
        .send()
        .await
        .and_then(|response| response.error_for_status())
        .map_err(|error| {
            format!(
                "Tailscale 설치 파일을 내려받지 못했습니다. 인터넷 연결을 확인해 주세요. ({error})"
            )
        })?;
    if let Some(length) = response.content_length() {
        check_size(installer, length)?;
    }
    let mut out = tokio::fs::File::create(partial)
        .await
        .map_err(|error| format!("설치 파일을 저장하지 못했습니다: {error}"))?;
    let mut downloaded = 0u64;
    let mut reported = 0u64;
    progress(InstallProgress::Download {
        downloaded,
        total: installer.size,
    });
    loop {
        let chunk = tokio::time::timeout(CHUNK_TIMEOUT, response.chunk())
            .await
            .map_err(|_| {
                "Tailscale 설치 파일 내려받기가 멈췄습니다. 인터넷 연결을 확인하고 다시 시도해 주세요."
                    .to_string()
            })?
            .map_err(|error| format!("Tailscale 설치 파일을 내려받지 못했습니다: {error}"))?;
        let Some(chunk) = chunk else { break };
        downloaded += chunk.len() as u64;
        check_size(installer, downloaded)?;
        out.write_all(&chunk)
            .await
            .map_err(|error| format!("설치 파일을 저장하지 못했습니다: {error}"))?;
        // One event per percent, not one per chunk.
        let percent = downloaded * 100 / installer.size.max(1);
        if percent != reported {
            reported = percent;
            progress(InstallProgress::Download {
                downloaded,
                total: installer.size,
            });
        }
    }
    out.flush()
        .await
        .map_err(|error| format!("설치 파일을 저장하지 못했습니다: {error}"))?;
    Ok(())
}

/// Hash a held file off the async runtime; the handle comes back with it.
async fn hash_held(mut held: std::fs::File) -> Result<(std::fs::File, u64, String), String> {
    tokio::task::spawn_blocking(move || {
        let (size, digest) = hash_file(&mut held)?;
        Ok((held, size, digest))
    })
    .await
    .map_err(|_| "설치 파일을 확인하지 못했습니다. 다시 시도해 주세요.".to_string())?
}

/// Download (or reuse) the pinned MSI into `dir`, verified and held open.
async fn fetch_verified(
    installer: &Installer,
    dir: &Path,
    progress: &(dyn Fn(InstallProgress) + Send + Sync),
) -> Result<(PathBuf, std::fs::File), String> {
    std::fs::create_dir_all(dir).map_err(|error| {
        format!("설치 파일 폴더를 만들지 못했습니다. 다시 시도해 주세요. ({error})")
    })?;
    let path = dir.join(installer.file_name);
    if let Ok(held) = open_locked(&path) {
        if let Ok((held, size, digest)) = hash_held(held).await {
            if verify(installer, size, &digest).is_ok() {
                return Ok((path, held));
            }
        }
        let _ = std::fs::remove_file(&path);
    }
    let mut partial_name = installer.file_name.to_string();
    partial_name.push_str(".part");
    let partial = dir.join(partial_name);
    let result = download(installer, &partial, progress).await;
    if let Err(error) = result {
        let _ = std::fs::remove_file(&partial);
        return Err(error);
    }
    std::fs::rename(&partial, &path).map_err(|error| {
        let _ = std::fs::remove_file(&partial);
        format!("설치 파일을 저장하지 못했습니다. 다시 시도해 주세요. ({error})")
    })?;
    // Verify through the handle that stays open while msiexec runs, so the
    // bytes checked are the bytes installed.
    let held = open_locked(&path)
        .map_err(|error| format!("설치 파일을 열지 못했습니다. 다시 시도해 주세요. ({error})"))?;
    let (held, size, digest) = hash_held(held).await?;
    if let Err(error) = verify(installer, size, &digest) {
        drop(held);
        let _ = std::fs::remove_file(&path);
        return Err(error);
    }
    Ok((path, held))
}

/// `\\?\C:\x` → `C:\x`, `\\?\UNC\host\share\x` → `\\host\share\x`.
pub fn strip_verbatim(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = path.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        path.to_string()
    }
}

/// The path the held handle actually names, every junction and link
/// resolved. While the file is open none of its directories can be renamed
/// away, so msiexec opening this path opens the verified file.
#[cfg(windows)]
fn final_path(held: &std::fs::File) -> Result<PathBuf, String> {
    use std::os::windows::ffi::OsStringExt as _;
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFinalPathNameByHandleW, FILE_NAME_NORMALIZED, VOLUME_NAME_DOS,
    };
    let mut buffer = vec![0u16; 1024];
    loop {
        let length = unsafe {
            GetFinalPathNameByHandleW(
                held.as_raw_handle() as _,
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                FILE_NAME_NORMALIZED | VOLUME_NAME_DOS,
            )
        } as usize;
        if length == 0 {
            return Err("설치 파일 경로를 확인하지 못했습니다. 다시 시도해 주세요.".to_string());
        }
        if length < buffer.len() {
            let path = std::ffi::OsString::from_wide(&buffer[..length]);
            return Ok(PathBuf::from(strip_verbatim(&path.to_string_lossy())));
        }
        buffer.resize(length + 1, 0);
    }
}

#[cfg(not(windows))]
fn final_path(_held: &std::fs::File) -> Result<PathBuf, String> {
    Err(unsupported())
}

/// What a finished `msiexec` exit code means for the user.
pub fn msiexec_outcome(code: u32, msi: &Path) -> Result<InstallOutcome, String> {
    match code {
        0 => Ok(InstallOutcome {
            reboot_required: false,
        }),
        1641 | 3010 => Ok(InstallOutcome {
            reboot_required: true,
        }),
        1602 => Err(
            "Tailscale 설치를 취소했습니다. 다시 설치하려면 Tailscale 설치를 누르고 관리자 권한 확인 창에서 예를 눌러 주세요."
                .to_string(),
        ),
        1618 => Err(
            "다른 프로그램 설치가 진행 중입니다. 그 설치가 끝난 뒤 다시 시도해 주세요.".to_string(),
        ),
        _ => Err(format!(
            "Tailscale 설치에 실패했습니다(코드 {code}). 설치 파일을 직접 실행해 보세요: {}",
            msi.display()
        )),
    }
}

/// How long the installer (and the UAC prompt it waits on) may take.
const MSIEXEC_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Start `msiexec /i <msi> /passive /norestart` unelevated.
/// Windows Installer raises the UAC prompt itself for a per-machine package,
/// and that prompt names the package's signer (Tailscale Inc.); elevating
/// msiexec.exe directly would show only "Windows Installer".
fn spawn_msiexec(msi: &Path) -> Result<tokio::process::Child, String> {
    if !cfg!(windows) {
        return Err(unsupported());
    }
    let system_root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    let msiexec = PathBuf::from(system_root)
        .join("System32")
        .join("msiexec.exe");
    tokio::process::Command::new(msiexec)
        .arg("/i")
        .arg(msi)
        .args(["/passive", "/norestart"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| {
            format!("Tailscale 설치 프로그램을 시작하지 못했습니다. 다시 시도해 주세요. ({error})")
        })
}

fn exit_code(status: std::io::Result<std::process::ExitStatus>) -> Result<u32, String> {
    let unreadable =
        "Tailscale 설치 결과를 읽지 못했습니다. 새로고침을 눌러 설치 여부를 확인해 주세요.";
    status
        .map_err(|error| format!("{unreadable} ({error})"))?
        .code()
        .map(|code| code as u32)
        .ok_or_else(|| unreadable.to_string())
}

fn unsupported() -> String {
    "이 PC에서는 앱이 Tailscale을 설치할 수 없습니다. tailscale.com/download 에서 설치해 주세요."
        .to_string()
}

static INSTALLING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Download, verify, and install the pinned MSI; `dir` holds the download.
pub async fn install(
    dir: &Path,
    progress: impl Fn(InstallProgress) + Send + Sync,
) -> Result<InstallOutcome, String> {
    let installer = installer().ok_or_else(unsupported)?;
    let guard = INSTALLING.try_lock().map_err(|_| {
        "Tailscale을 이미 설치하고 있습니다. 끝날 때까지 기다려 주세요.".to_string()
    })?;
    let (msi, held) = fetch_verified(&installer, dir, &progress).await?;
    let resolved = final_path(&held)?;
    progress(InstallProgress::Install);
    let mut child = spawn_msiexec(&resolved)?;
    let code = match tokio::time::timeout(MSIEXEC_TIMEOUT, child.wait()).await {
        Ok(status) => exit_code(status),
        Err(_) => {
            // Still running: keep the verified bytes locked and a second
            // install refused until it is done.
            tokio::spawn(async move {
                let _ = child.wait().await;
                drop(held);
                drop(guard);
            });
            return Err(
                "Tailscale 설치가 30분 넘게 끝나지 않았습니다. 설치 창을 확인하고, 끝나면 새로고침을 눌러 주세요."
                    .to_string(),
            );
        }
    };
    // Keep the verified bytes locked until msiexec is done with them.
    drop(held);
    drop(guard);
    let outcome = msiexec_outcome(code?, &msi)?;
    let _ = std::fs::remove_file(&msi);
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_one_installer_per_windows_architecture() {
        assert_eq!(installer_for("amd64"), Some(MSI_AMD64));
        assert_eq!(installer_for("arm64"), Some(MSI_ARM64));
        assert_eq!(installer_for("x86"), None);
        for installer in [MSI_AMD64, MSI_ARM64] {
            assert!(installer.file_name.contains(INSTALLER_VERSION));
            assert_eq!(installer.sha256.len(), 64);
        }
    }

    #[test]
    fn a_download_must_match_the_pin_exactly() {
        let pin = Installer {
            file_name: "x.msi",
            size: 3,
            sha256: "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD",
        };
        let mut hasher = Sha256::new();
        hasher.update(b"abc");
        let digest = hex(&hasher.finalize());
        assert!(verify(&pin, 3, &digest).is_ok());
        assert!(verify(&pin, 4, &digest).is_err());
        assert!(verify(&pin, 3, &"0".repeat(64)).is_err());
        assert!(check_size(&pin, 3).is_ok());
        assert!(check_size(&pin, 4).is_err());
    }

    #[test]
    fn a_verified_download_is_reused_and_held_locked() {
        let dir = std::env::temp_dir().join(format!(
            "eud-agent-tailscale-install-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut hasher = Sha256::new();
        hasher.update(b"abc");
        let digest: &'static str = Box::leak(hex(&hasher.finalize()).into_boxed_str());
        let pin = Installer {
            file_name: "x.msi",
            size: 3,
            sha256: digest,
        };
        std::fs::write(dir.join("x.msi"), b"abc").unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (path, held) = runtime
            .block_on(fetch_verified(&pin, &dir, &|_| {}))
            .unwrap();
        assert_eq!(path, dir.join("x.msi"));
        #[cfg(windows)]
        {
            let resolved = final_path(&held).unwrap();
            assert_eq!(
                std::fs::canonicalize(&resolved).unwrap(),
                std::fs::canonicalize(&path).unwrap(),
                "msiexec gets the held file's own path"
            );
            assert!(!resolved.to_string_lossy().starts_with(r"\\?\"));
            assert!(
                std::fs::OpenOptions::new().write(true).open(&path).is_err(),
                "the verified file is locked against writes while held"
            );
            assert!(std::fs::remove_file(&path).is_err(), "and against deletion");
        }
        drop(held);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Real download from pkgs.tailscale.com (no install); run after a pin
    /// bump: `cargo test --lib tailscale_install -- --ignored`.
    #[test]
    #[ignore = "network"]
    fn the_pinned_installer_downloads_and_verifies() {
        let dir = std::env::temp_dir().join(format!(
            "eud-agent-tailscale-download-{}",
            std::process::id()
        ));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        for installer in [MSI_AMD64, MSI_ARM64] {
            let events = std::sync::Mutex::new(Vec::new());
            let (path, held) = runtime
                .block_on(fetch_verified(&installer, &dir, &|progress| {
                    events.lock().unwrap().push(progress)
                }))
                .unwrap();
            assert_eq!(std::fs::metadata(&path).unwrap().len(), installer.size);
            let events = events.into_inner().unwrap();
            assert!(
                events.len() <= 102,
                "one event per percent: {}",
                events.len()
            );
            assert_eq!(
                events.last(),
                Some(&InstallProgress::Download {
                    downloaded: installer.size,
                    total: installer.size
                })
            );
            drop(held);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn verbatim_prefixes_are_dropped_for_msiexec() {
        assert_eq!(
            strip_verbatim(r"\\?\C:\Users\me\x.msi"),
            r"C:\Users\me\x.msi"
        );
        assert_eq!(
            strip_verbatim(r"\\?\UNC\host\share\x.msi"),
            r"\\host\share\x.msi"
        );
        assert_eq!(strip_verbatim(r"C:\x.msi"), r"C:\x.msi");
    }

    #[test]
    fn msiexec_codes_name_the_recovery() {
        let msi = Path::new("C:/x.msi");
        assert_eq!(
            msiexec_outcome(0, msi),
            Ok(InstallOutcome {
                reboot_required: false
            })
        );
        assert_eq!(
            msiexec_outcome(3010, msi),
            Ok(InstallOutcome {
                reboot_required: true
            })
        );
        assert!(msiexec_outcome(1602, msi).unwrap_err().contains("취소"));
        assert!(msiexec_outcome(1618, msi).unwrap_err().contains("진행 중"));
        assert!(msiexec_outcome(1603, msi).unwrap_err().contains("1603"));
    }

    #[test]
    fn progress_serializes_with_its_phase() {
        assert_eq!(
            serde_json::to_value(InstallProgress::Download {
                downloaded: 1,
                total: 2
            })
            .unwrap(),
            serde_json::json!({"phase": "download", "downloaded": 1, "total": 2})
        );
        assert_eq!(
            serde_json::to_value(InstallProgress::Install).unwrap(),
            serde_json::json!({"phase": "install"})
        );
    }
}
