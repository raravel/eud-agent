use std::path::Path;

use base64::Engine as _;
use serde_json::{json, Value};

use crate::provider::ReasoningLevel;

pub(super) const MAX_STDOUT_BYTES: usize = 32 * 1024 * 1024;
/// One stream-json line may carry a whole echoed tool result, including a rendered
/// map image, so it shares the total stdout ceiling instead of a smaller one.
pub(super) const MAX_JSONL_LINE_BYTES: usize = MAX_STDOUT_BYTES;
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

/// Project-root cwd boundary.
///
/// `CLAUDE.md`/`CLAUDE.local.md` are user-authored prose rules; prose cannot
/// add a tool, so the project root may carry them. Everything that can change
/// tools, permissions, or hooks is still refused: `.claude/settings*.json`,
/// any other `.claude/` entry (agents, commands, plugins), and `.mcp.json`
/// registration attempts. That refusal is what keeps `--tools` and
/// `--disallowedTools` the only word on what this session may do.
pub(super) fn validate_workspace_boundary(root: &Path) -> Result<(), String> {
    if root.join(".mcp.json").exists() {
        return Err(
            "provider workspace contains `.mcp.json`; remove it so no extra MCP server can be registered"
                .to_string(),
        );
    }
    let claude_dir = root.join(".claude");
    if claude_dir.exists() {
        let refusal = if ["settings.json", "settings.local.json"]
            .iter()
            .any(|name| claude_dir.join(name).exists())
        {
            "`.claude/settings*.json` can change Claude tool permissions and hooks"
        } else {
            "`.claude/` can add agents, commands, or plugins"
        };
        return Err(format!(
            "provider workspace contains a `.claude` directory: {refusal}; remove it before starting a turn"
        ));
    }
    Ok(())
}

/// `provider-default` delegates model and effort selection to the CLI and passes neither flag.
/// A catalog model id is passed verbatim on `--model`; its reasoning level must be one of the
/// CLI's closed `--effort` set, so persisted state cannot place an arbitrary string on argv.
pub(super) fn model_args(model: &str, effort: Option<&str>) -> Result<Vec<String>, String> {
    if model == super::state::CLAUDE_PROVIDER_DEFAULT {
        return Ok(Vec::new());
    }
    let mut args = vec!["--model".to_string(), model.to_string()];
    if let Some(effort) = effort {
        let level = match effort {
            "low" => ReasoningLevel::Low,
            "medium" => ReasoningLevel::Medium,
            "high" => ReasoningLevel::High,
            "xhigh" => ReasoningLevel::Xhigh,
            "max" => ReasoningLevel::Max,
            _ => return Err("provider_capability_unsupported".to_string()),
        };
        args.extend(["--effort".to_string(), level.as_str().to_string()]);
    }
    Ok(args)
}

/// The built-in tools an interactive turn may use. Reading and editing the
/// project directly is the point of the cutover; `Bash` and every other
/// built-in stay out, so running anything is still `build_run` alone.
pub(super) const NATIVE_FILE_TOOL_NAMES: &[&str] = &["Read", "Edit", "Write", "Glob", "Grep"];

/// A tool name this run authorized: an eud-tools MCP tool, or one of the
/// native file tools above. The process boundary checks every tool the CLI
/// announces and every observation it reports against this, so a CLI that ran
/// something nobody allowed — `Bash` above all — ends the run instead of
/// being trusted. It is the same list `--tools` sends, so opening a tool and
/// authorizing it cannot drift apart.
pub(super) fn tool_is_authorized(name: &str) -> bool {
    name.starts_with("mcp__eud-tools__") || NATIVE_FILE_TOOL_NAMES.contains(&name)
}

fn native_file_tools() -> String {
    NATIVE_FILE_TOOL_NAMES.join(",")
}

/// Paths no turn writes, whatever tool it reaches for. `maps/` and
/// `references/` are binary and belong to MapSafe's backup, verification and
/// rollback path; `.git/` is the rollback authority itself; `.claude/` and
/// `.mcp.json` decide what this session is allowed to do, which is not a
/// decision the session gets to make about itself.
/// [`validate_workspace_boundary`] already refuses those two at turn start —
/// these rules are what stops a turn from writing one in the first place, so
/// the refusal never has to strand the project.
///
/// The rules name `Edit`, which covers the whole write family: Claude Code
/// accepts a path rule on `Write` but never consults it, and warns at startup.
const PROTECTED_PATH_RULES: &str = concat!(
    "Edit(maps/**) Edit(references/**) Edit(.git/**) ",
    "Edit(.claude/**) Edit(.mcp.json)"
);

pub(super) fn stream_args(
    mcp_config: Option<&str>,
    conversation_id: Option<&str>,
    compaction: bool,
    model: &str,
    effort: Option<&str>,
) -> Result<Vec<String>, String> {
    let mut args = vec![
        "-p".to_string(),
        "--input-format".to_string(),
        "stream-json".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
        "--include-partial-messages".to_string(),
    ];
    args.extend(model_args(model, effort)?);
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
            native_file_tools(),
            "--allowedTools".to_string(),
            format!("{},mcp__eud-tools__*", native_file_tools()),
            "--disallowedTools".to_string(),
            PROTECTED_PATH_RULES.to_string(),
            "--permission-mode".to_string(),
            "dontAsk".to_string(),
            "--disable-slash-commands".to_string(),
            "--no-chrome".to_string(),
        ]);
    }
    if let Some(conversation_id) = conversation_id {
        args.extend(["--resume".to_string(), conversation_id.to_string()]);
    }
    Ok(args)
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

#[cfg(test)]
mod tests {
    use super::{model_args, stream_args, validate_workspace_boundary};

    #[test]
    fn authorization_covers_exactly_the_tools_this_run_opens() {
        for opened in super::NATIVE_FILE_TOOL_NAMES {
            assert!(super::tool_is_authorized(opened), "{opened}");
        }
        assert!(super::tool_is_authorized("mcp__eud-tools__build_run"));
        // Running anything is `build_run` alone, and another server's tools
        // are not this app's to admit.
        assert!(!super::tool_is_authorized("Bash"));
        assert!(!super::tool_is_authorized("Task"));
        assert!(!super::tool_is_authorized("mcp__other__read_file"));
    }

    #[test]
    fn the_opened_tools_and_the_advertised_tools_are_one_list() {
        let args = stream_args(Some("{}"), Some("s1"), false, "provider-default", None).unwrap();
        let index = args.iter().position(|arg| arg == "--tools").unwrap();
        for tool in args[index + 1].split(',') {
            assert!(
                super::tool_is_authorized(tool),
                "{tool} is opened but not authorized"
            );
        }
    }

    #[test]
    fn provider_default_delegates_model_and_effort_to_the_cli() {
        assert!(model_args("provider-default", None).unwrap().is_empty());
        // Stale persisted reasoning never reaches argv for the CLI-selected default.
        assert!(model_args("provider-default", Some("high"))
            .unwrap()
            .is_empty());
        let args = stream_args(Some("{}"), Some("s1"), false, "provider-default", None).unwrap();
        assert!(!args.iter().any(|arg| arg == "--model"));
        assert!(!args.iter().any(|arg| arg == "--effort"));
    }

    #[test]
    fn catalog_model_and_effort_are_passed_verbatim() {
        assert_eq!(
            model_args("claude-opus-5", Some("xhigh")).unwrap(),
            vec!["--model", "claude-opus-5", "--effort", "xhigh"]
        );
        assert_eq!(
            model_args("claude-opus-5", None).unwrap(),
            vec!["--model", "claude-opus-5"]
        );
        assert_eq!(
            model_args("claude-opus-5", Some("ultra")),
            Err("provider_capability_unsupported".to_string())
        );
        assert_eq!(
            model_args("claude-opus-5", Some("-p")),
            Err("provider_capability_unsupported".to_string())
        );
        let args = stream_args(None, Some("s1"), true, "claude-opus-5", Some("high")).unwrap();
        let model = args.iter().position(|arg| arg == "--model").unwrap();
        assert_eq!(args[model + 1], "claude-opus-5");
        let effort = args.iter().position(|arg| arg == "--effort").unwrap();
        assert_eq!(args[effort + 1], "high");
        assert!(args.iter().any(|arg| arg == "--resume"));
    }

    fn root(tag: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "eud-agent-claude-boundary-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn prose_rules_pass_and_tool_granting_state_is_refused_by_name() {
        let root = root("split");
        std::fs::write(root.join("CLAUDE.md"), b"# project rules").unwrap();
        std::fs::write(root.join("CLAUDE.local.md"), b"# local rules").unwrap();
        validate_workspace_boundary(&root).unwrap();

        std::fs::write(root.join(".mcp.json"), b"{}").unwrap();
        let error = validate_workspace_boundary(&root).unwrap_err();
        assert!(error.contains(".mcp.json"), "got: {error}");
        std::fs::remove_file(root.join(".mcp.json")).unwrap();

        std::fs::create_dir_all(root.join(".claude")).unwrap();
        std::fs::write(root.join(".claude/settings.json"), b"{}").unwrap();
        let error = validate_workspace_boundary(&root).unwrap_err();
        assert!(error.contains("settings"), "got: {error}");
        std::fs::remove_file(root.join(".claude/settings.json")).unwrap();

        std::fs::create_dir_all(root.join(".claude/commands")).unwrap();
        let error = validate_workspace_boundary(&root).unwrap_err();
        assert!(
            error.contains("agents, commands, or plugins"),
            "got: {error}"
        );

        std::fs::remove_dir_all(&root).ok();
    }
}
