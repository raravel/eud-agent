use std::path::Path;

use base64::Engine as _;
use serde_json::{json, Value};

pub(super) const MAX_JSONL_LINE_BYTES: usize = 1024 * 1024;
pub(super) const MAX_STDOUT_BYTES: usize = 32 * 1024 * 1024;
pub(super) const MAX_STDERR_BYTES: u64 = 16 * 1024;
const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;

pub(super) fn claude_user_message(
    input: &crate::provider_runtime::AgentTurnInput,
) -> Result<Value, String> {
    let mut content = vec![json!({"type":"text","text":input.text})];
    for path in &input.image_paths {
        let (mime, data) = encoded_image(path)?;
        content
            .push(json!({"type":"image","source":{"type":"base64","media_type":mime,"data":data}}));
    }
    Ok(
        json!({"type":"user","message":{"role":"user","content":content},"parent_tool_use_id":Value::Null}),
    )
}

fn encoded_image(path: &Path) -> Result<(&'static str, String), String> {
    let bytes = std::fs::read(path).map_err(|_| "provider image cannot be read".to_string())?;
    if bytes.is_empty() || bytes.len() > MAX_IMAGE_BYTES {
        return Err("provider image size is unsupported".to_string());
    }
    let mime = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        "image/jpeg"
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        "image/webp"
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        "image/gif"
    } else {
        return Err("provider image format is unsupported".to_string());
    };
    Ok((
        mime,
        base64::engine::general_purpose::STANDARD.encode(bytes),
    ))
}

pub(super) fn validate_workspace_boundary(root: &Path) -> Result<(), String> {
    for relative in [".claude", ".mcp.json", "CLAUDE.md", "CLAUDE.local.md"] {
        if root.join(relative).exists() {
            return Err("provider workspace contains ambient Claude configuration".to_string());
        }
    }
    Ok(())
}

pub(super) fn stream_args(
    mcp_config: Option<&str>,
    conversation_id: Option<&str>,
    compaction: bool,
) -> Vec<String> {
    let mut args = vec![
        "-p".to_string(),
        "--input-format".to_string(),
        "stream-json".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
        "--include-partial-messages".to_string(),
    ];
    if compaction {
        args.extend([
            "--strict-mcp-config".to_string(),
            "--tools".to_string(),
            String::new(),
            "--permission-mode".to_string(),
            "dontAsk".to_string(),
            "--no-chrome".to_string(),
        ]);
    } else if let Some(mcp_config) = mcp_config {
        args.extend([
            "--mcp-config".to_string(),
            mcp_config.to_string(),
            "--strict-mcp-config".to_string(),
            "--tools".to_string(),
            String::new(),
            "--allowedTools".to_string(),
            "mcp__eud-tools__*".to_string(),
            "--permission-mode".to_string(),
            "dontAsk".to_string(),
            "--disable-slash-commands".to_string(),
            "--no-chrome".to_string(),
        ]);
    }
    if let Some(conversation_id) = conversation_id {
        args.extend(["--resume".to_string(), conversation_id.to_string()]);
    }
    args
}

pub(super) fn configure_tokio_command(command: &mut tokio::process::Command, profile_dir: &Path) {
    command.env("CLAUDE_CONFIG_DIR", profile_dir);
    for name in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_PROFILE",
        "ANTHROPIC_FEDERATION_RULE_ID",
        "ANTHROPIC_ORGANIZATION_ID",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
        "CLAUDE_CODE_USE_FOUNDRY",
        "CLAUDE_CODE_SIMPLE",
    ] {
        command.env_remove(name);
    }
}

#[cfg(windows)]
pub(super) fn hide_console(command: &mut tokio::process::Command) {
    command.creation_flags(0x0800_0000);
}
#[cfg(not(windows))]
pub(super) fn hide_console(_command: &mut tokio::process::Command) {}

pub(super) async fn terminate_child(
    child: &mut tokio::process::Child,
    job: crate::provider_process::WindowsJob,
) {
    let _ = child.start_kill();
    if tokio::time::timeout(std::time::Duration::from_secs(2), child.wait())
        .await
        .is_err()
    {
        job.terminate();
    }
}
