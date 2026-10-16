//! Read-only Tailscale probes and the one `tailscale serve` command the
//! settings screen can run for the user.
//!
//! Every call is bounded and windowless; a missing or stopped Tailscale is a
//! status, never an error.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use tokio::io::AsyncReadExt;

const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
/// `tailscale serve` may stop to ask for HTTPS to be enabled on the tailnet;
/// it is cut off rather than left waiting.
const SERVE_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_OUTPUT_BYTES: usize = 256 * 1024;
const MAX_ERROR_CHARS: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TailscaleStatus {
    pub installed: bool,
    pub running: bool,
    /// MagicDNS name without the trailing dot.
    pub dns_name: Option<String>,
    /// `tailscale serve` already proxies to this app's port.
    pub serve_configured: bool,
    pub serve_command: String,
}

pub fn serve_target(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

pub fn serve_command(port: u16) -> String {
    format!("tailscale serve --bg --https=443 {}", serve_target(port))
}

fn executable() -> Option<PathBuf> {
    if let Ok(path) = which::which("tailscale") {
        return Some(path);
    }
    let fallbacks: &[&str] = if cfg!(windows) {
        &[r"C:\Program Files\Tailscale\tailscale.exe"]
    } else if cfg!(target_os = "macos") {
        &["/Applications/Tailscale.app/Contents/MacOS/Tailscale"]
    } else {
        &[]
    };
    fallbacks
        .iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
}

struct RunOutput {
    success: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    timed_out: bool,
}

async fn run(exe: &PathBuf, args: &[&str], timeout: Duration) -> Result<RunOutput, String> {
    let mut command = tokio::process::Command::new(exe);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        // The app is a windowless GUI process; without this every probe
        // flashes a console window.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("Tailscale을 실행하지 못했습니다: {error}"))?;
    let stdout = child.stdout.take().map(read_bounded);
    let stderr = child.stderr.take().map(read_bounded);
    let (success, timed_out) = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => (status.success(), false),
        Ok(Err(error)) => return Err(format!("Tailscale 실행 상태를 읽지 못했습니다: {error}")),
        Err(_) => {
            let _ = child.kill().await;
            (false, true)
        }
    };
    let collect = |task: Option<tokio::task::JoinHandle<Vec<u8>>>| async move {
        match task {
            Some(task) => task.await.unwrap_or_default(),
            None => Vec::new(),
        }
    };
    Ok(RunOutput {
        success,
        stdout: collect(stdout).await,
        stderr: collect(stderr).await,
        timed_out,
    })
}

fn read_bounded<R>(mut reader: R) -> tokio::task::JoinHandle<Vec<u8>>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 8192];
        while let Ok(read) = reader.read(&mut chunk).await {
            if read == 0 {
                break;
            }
            let room = MAX_OUTPUT_BYTES.saturating_sub(buffer.len());
            buffer.extend_from_slice(&chunk[..read.min(room)]);
        }
        buffer
    })
}

/// `(running, dnsName)` from `tailscale status --json`.
pub fn parse_status(json: &[u8]) -> (bool, Option<String>) {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(json) else {
        return (false, None);
    };
    let running = value.get("BackendState").and_then(|state| state.as_str()) == Some("Running");
    let dns_name = value
        .get("Self")
        .and_then(|node| node.get("DNSName"))
        .and_then(|name| name.as_str())
        .map(|name| name.trim_end_matches('.').to_string())
        .filter(|name| !name.is_empty());
    (running, dns_name)
}

/// Whether `tailscale serve status --json` proxies anything to this port.
pub fn serve_proxies_port(json: &[u8], port: u16) -> bool {
    fn walk(value: &serde_json::Value, target: &str) -> bool {
        match value {
            serde_json::Value::Object(map) => map.iter().any(|(key, child)| {
                (key == "Proxy"
                    && child
                        .as_str()
                        .is_some_and(|proxy| proxy.trim_end_matches('/') == target))
                    || walk(child, target)
            }),
            serde_json::Value::Array(items) => items.iter().any(|item| walk(item, target)),
            _ => false,
        }
    }
    serde_json::from_slice::<serde_json::Value>(json)
        .is_ok_and(|value| walk(&value, &serve_target(port)))
}

pub async fn status(port: u16) -> TailscaleStatus {
    let mut status = TailscaleStatus {
        installed: false,
        running: false,
        dns_name: None,
        serve_configured: false,
        serve_command: serve_command(port),
    };
    let Some(exe) = executable() else {
        return status;
    };
    status.installed = true;
    if let Ok(output) = run(&exe, &["status", "--json"], PROBE_TIMEOUT).await {
        let (running, dns_name) = parse_status(&output.stdout);
        status.running = running;
        status.dns_name = dns_name;
    }
    if status.running {
        if let Ok(output) = run(&exe, &["serve", "status", "--json"], PROBE_TIMEOUT).await {
            status.serve_configured = output.success && serve_proxies_port(&output.stdout, port);
        }
    }
    status
}

/// Run `tailscale serve --bg --https=443 http://127.0.0.1:<port>`.
pub async fn serve(port: u16) -> Result<(), String> {
    let exe = executable().ok_or_else(|| {
        "Tailscale이 설치되어 있지 않습니다. Tailscale을 설치하고 로그인한 뒤 다시 시도해 주세요."
            .to_string()
    })?;
    let target = serve_target(port);
    let output = run(
        &exe,
        &["serve", "--bg", "--https=443", target.as_str()],
        SERVE_TIMEOUT,
    )
    .await?;
    if output.success {
        return Ok(());
    }
    let mut text = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.trim().is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(stdout.trim());
    }
    let text: String = text.chars().take(MAX_ERROR_CHARS).collect();
    let lead = if output.timed_out {
        "Tailscale이 응답을 마치지 않았습니다. 아래 안내(예: 테일넷에서 HTTPS 사용 허용)를 따른 뒤 다시 시도해 주세요."
    } else {
        "tailscale serve 실행에 실패했습니다. 아래 출력을 확인하고 명령을 직접 실행해 보세요."
    };
    Err(if text.is_empty() {
        lead.to_string()
    } else {
        format!("{lead}\n{text}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_reads_backend_state_and_magic_dns_name() {
        let json = br#"{"BackendState":"Running","Self":{"DNSName":"my-pc.tail1234.ts.net."}}"#;
        assert_eq!(
            parse_status(json),
            (true, Some("my-pc.tail1234.ts.net".to_string()))
        );
        assert_eq!(
            parse_status(br#"{"BackendState":"Stopped","Self":{"DNSName":""}}"#),
            (false, None)
        );
        assert_eq!(parse_status(b"not json"), (false, None));
    }

    #[test]
    fn serve_status_matches_only_this_port() {
        let json = br#"{"TCP":{"443":{"HTTPS":true}},"Web":{"my-pc.tail1234.ts.net:443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:47821"}}}}}"#;
        assert!(serve_proxies_port(json, 47821));
        assert!(!serve_proxies_port(json, 47822));
        assert!(!serve_proxies_port(b"{}", 47821));
        assert!(!serve_proxies_port(b"", 47821));
    }

    #[test]
    fn serve_command_names_the_loopback_port() {
        assert_eq!(
            serve_command(47821),
            "tailscale serve --bg --https=443 http://127.0.0.1:47821"
        );
    }
}
