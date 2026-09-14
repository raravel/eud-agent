use std::{ffi::OsString, path::PathBuf, sync::Arc, time::Duration};

use crate::{
    codex_client::CodexLaunchConfig,
    provider::{ProviderConversationState, ProviderId},
    provider_runtime::{
        AdapterRequestKind, AdapterStepRequest, AgentTurnInput, BindingSnapshot, RunId,
        RunIdentity, RunPolicy,
    },
    session::SessionKind,
};

pub(super) fn launch_config() -> (CodexLaunchConfig, PathBuf) {
    let root = std::env::temp_dir().join(format!("eud-codex-fixture-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let system_root = std::env::var_os("SystemRoot").unwrap();
    let executable = PathBuf::from(system_root)
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe");
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("codex_adapter")
        .join("app_server_fixture.ps1");
    (
        CodexLaunchConfig {
            executable,
            executable_args: vec![
                OsString::from("-NoProfile"),
                OsString::from("-ExecutionPolicy"),
                OsString::from("Bypass"),
                OsString::from("-File"),
                script.into_os_string(),
            ],
            profile_dir: root.join("profile"),
            large_context_models: Default::default(),
        },
        root,
    )
}

pub(super) fn step_request(
    text: &str,
    endpoint: &str,
    cancel: tokio::sync::watch::Receiver<u64>,
) -> AdapterStepRequest {
    AdapterStepRequest {
        identity: RunIdentity {
            session_id: "fixture-session".to_string(),
            run_id: RunId::new(1),
            request_id: format!("request-{text}"),
            session_kind: SessionKind::Eps,
            cancellation_generation: 0,
        },
        binding: BindingSnapshot {
            provider: ProviderId::Codex,
            model: "gpt-test".to_string(),
            reasoning: None,
            base_url: None,
            capabilities: None,
            conversation: ProviderConversationState::Codex { thread_id: None },
        },
        kind: AdapterRequestKind::Foreground(AgentTurnInput::text(text)),
        policy: RunPolicy {
            active_deadline: Some(Duration::from_secs(5)),
            shutdown_grace: Duration::from_secs(1),
            max_output_bytes: 4096,
            max_output_tokens: None,
            max_tool_rounds: 64,
            allow_resume: true,
        },
        continuation: None,
        history: Arc::from([]),
        prior_tool_results: Arc::from([]),
        tool_descriptors: Arc::from([]),
        native_mcp_endpoint: Some(endpoint.to_string()),
        cancellation: cancel,
    }
}

pub(super) fn fixture_pids(profile: &std::path::Path) -> Vec<u32> {
    let mut pids = std::fs::read_dir(profile)
        .unwrap()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .strip_prefix("process-")?
                .strip_suffix(".started")?
                .parse()
                .ok()
        })
        .collect::<Vec<_>>();
    pids.sort_unstable();
    pids
}

pub(super) fn process_is_running(pid: u32) -> bool {
    let output = std::process::Command::new("tasklist.exe")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
}

pub(super) fn descendant_pid(profile: &std::path::Path) -> Option<u32> {
    std::fs::read_dir(profile)
        .ok()?
        .filter_map(Result::ok)
        .find_map(|entry| {
            if !entry
                .file_name()
                .to_string_lossy()
                .starts_with("descendant-")
            {
                return None;
            }
            std::fs::read_to_string(entry.path())
                .ok()?
                .trim()
                .parse()
                .ok()
        })
}

pub(super) async fn assert_process_stopped(pid: u32) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while process_is_running(pid) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("native process tree must terminate");
}

pub(super) async fn cleanup_fixture(root: PathBuf) {
    for pid in fixture_pids(&root.join("profile")) {
        assert_process_stopped(pid).await;
    }
    std::fs::remove_dir_all(root).unwrap();
}
