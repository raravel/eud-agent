//! Low-level codex subprocess client and prompt composer.
//!
//! This module mirrors `server/eud_agent/codex_client.py`: it composes the small
//! codex-facing prompt shape, invokes the user's resolved `codex exec` shim, and extracts
//! epScript code from codex stdout.
//!
//! The fenced-output contract is intentional. `generate()` extracts fenced code blocks and
//! returns `CodexError::NoCode` when there are none. Per rules.md "codex invocation", codex
//! stdout is treated as noisy, so the client fails with the raw output snippet rather than
//! applying unfenced banner or usage text to the editor. The `codex exec` CLI wraps model
//! output in fenced blocks, so fenced output is the normal success path; `NoCode` indicates
//! a real failure such as banner-only output or an argument/usage error. The prompt's
//! "코드만" instruction removes explanatory prose, not the code fence.
//!
//! Scope and layering are deliberately narrow here. This composer emits only the
//! low-level `참고자료` / `현재 코드` / `요청` / `epScript 코드` framing. The
//! first-principles, evidence, and message-format system guardrails are assembled upstream
//! by the engine/orchestrator, as described in feature 11's engine section. That matches
//! the Python layering where `engine.py` wraps `codex_client.build_prompt`. Callers wiring
//! generation must go through the engine so those guardrails apply.

use std::{env, io::ErrorKind, path::PathBuf, process::Stdio, time::Duration};

use thiserror::Error;
use tokio::{io::AsyncWriteExt, process::Command, time};

const SYSTEM_PROMPT: &str =
    "너는 스타크래프트 EUD 맵 제작용 epScript(eps) 코드를 작성하는 어시스턴트다. \
아래 [참고자료]는 네이버 카페/공식 매뉴얼에서 검색한 eps/eud3 지식이다. \
사용자 요청을 만족하는 epScript 코드만 출력해라. 설명/마크다운 없이 코드만. \
플레이어 루프·변수 선언 등 eps 관례를 지켜라.";

const RAW_SNIPPET_LIMIT: usize = 500;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CodexError {
    #[error("codex command not found: {0}")]
    NotFound(String),
    #[error("codex exec timed out: {0}")]
    Timeout(String),
    #[error("codex produced no fenced code block: {0}")]
    NoCode(String),
}

/// Resolve the codex `.cmd` shim path.
///
/// Honors a `CODEX_CMD` env override (full path to the shim); otherwise locates `codex`
/// on PATH via the `which` crate. This never returns a bare `"codex"` command.
pub fn resolve_codex_cmd() -> Result<PathBuf, CodexError> {
    if let Some(cmd) = env::var_os("CODEX_CMD").filter(|cmd| !cmd.is_empty()) {
        return Ok(PathBuf::from(cmd));
    }

    which::which("codex").map_err(|err| {
        CodexError::NotFound(format!(
            "could not resolve codex.cmd via PATH: {err}. Install codex or set CODEX_CMD to the codex.cmd shim path."
        ))
    })
}

pub fn extract_code(text: &str) -> Result<String, CodexError> {
    let raw = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut blocks = Vec::new();
    let mut current = Vec::new();
    let mut in_block = false;

    for line in raw.split('\n') {
        if in_block {
            if line.trim() == "```" {
                let block = current.join("\n").trim().to_string();
                if !block.is_empty() {
                    blocks.push(block);
                }
                current.clear();
                in_block = false;
            } else {
                current.push(line);
            }
        } else if line.trim_start().starts_with("```") {
            current.clear();
            in_block = true;
        }
    }

    if blocks.is_empty() {
        return Err(CodexError::NoCode(
            raw.chars().take(RAW_SNIPPET_LIMIT).collect(),
        ));
    }

    Ok(blocks.join("\n\n"))
}

/// Low-level prompt composer; safety and first-principles sections are added by the engine.
pub fn build_prompt(
    instruction: &str,
    context_chunks: &[String],
    current_code: Option<&str>,
) -> String {
    let chunks = context_chunks
        .iter()
        .filter(|chunk| !chunk.trim().is_empty())
        .map(String::as_str)
        .collect::<Vec<_>>();
    let context = if chunks.is_empty() {
        "(없음)".to_string()
    } else {
        chunks.join("\n\n")
    };

    let mut parts = vec![
        SYSTEM_PROMPT.to_string(),
        String::new(),
        "[참고자료]".to_string(),
        context,
    ];

    if let Some(code) = current_code.filter(|code| !code.trim().is_empty()) {
        parts.extend([String::new(), "[현재 코드]".to_string(), code.to_string()]);
    }

    parts.extend([
        String::new(),
        "[요청]".to_string(),
        instruction.to_string(),
        String::new(),
        "[epScript 코드]".to_string(),
    ]);

    parts.join("\n")
}

#[derive(Debug, Clone)]
pub struct CodexClient {
    codex_cmd: PathBuf,
    repo_root: PathBuf,
}

impl CodexClient {
    pub fn new(
        codex_cmd: impl Into<PathBuf>,
        repo_root: impl Into<PathBuf>,
    ) -> Result<Self, CodexError> {
        let codex_cmd = codex_cmd.into();
        if codex_cmd.as_os_str().is_empty() {
            return Err(CodexError::NotFound(
                "codex path is empty; resolve codex.cmd before constructing CodexClient"
                    .to_string(),
            ));
        }
        if !codex_cmd.is_file() {
            return Err(CodexError::NotFound(format!(
                "codex path does not exist: {}",
                codex_cmd.display()
            )));
        }

        Ok(Self {
            codex_cmd,
            repo_root: repo_root.into(),
        })
    }

    pub async fn generate(&self, prompt: &str) -> Result<String, CodexError> {
        let mut command = Command::new(&self.codex_cmd);
        command
            .arg("exec")
            .arg("--skip-git-repo-check")
            .current_dir(&self.repo_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = command
            .spawn()
            .map_err(|err| CodexError::NotFound(format!("failed to spawn codex: {err}")))?;

        if let Some(mut stdin) = child.stdin.take() {
            match stdin.write_all(prompt.as_bytes()).await {
                Ok(()) => {
                    if let Err(err) = stdin.shutdown().await {
                        if !matches!(
                            err.kind(),
                            ErrorKind::BrokenPipe | ErrorKind::ConnectionReset
                        ) {
                            return Err(CodexError::NoCode(format!(
                                "failed to close codex stdin: {err}"
                            )));
                        }
                    }
                }
                Err(err)
                    if matches!(
                        err.kind(),
                        ErrorKind::BrokenPipe | ErrorKind::ConnectionReset
                    ) => {}
                Err(err) => {
                    return Err(CodexError::NoCode(format!(
                        "failed to write codex stdin: {err}"
                    )));
                }
            }
        }

        let output = match time::timeout(DEFAULT_TIMEOUT, child.wait_with_output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(err)) => {
                return Err(CodexError::NoCode(format!(
                    "failed to read codex output: {err}"
                )));
            }
            Err(_) => {
                return Err(CodexError::Timeout(format!(
                    "codex exec timed out after {}s",
                    DEFAULT_TIMEOUT.as_secs()
                )));
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        match extract_code(&stdout) {
            Ok(code) => Ok(code),
            Err(CodexError::NoCode(snippet)) => {
                let tail = take_last_chars(stderr.trim(), RAW_SNIPPET_LIMIT);
                if tail.is_empty() {
                    Err(CodexError::NoCode(snippet))
                } else {
                    Err(CodexError::NoCode(format!(
                        "{snippet}\n--- stderr (tail) ---\n{tail}"
                    )))
                }
            }
            Err(err) => Err(err),
        }
    }
}

fn take_last_chars(text: &str, limit: usize) -> String {
    let mut chars = text.chars().rev().take(limit).collect::<Vec<_>>();
    chars.reverse();
    chars.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_code_single_fence_without_language_tag() {
        let output = "banner\n```\nfunction main() {\n    DoActions();\n}\n```\nusage";

        assert_eq!(
            extract_code(output).unwrap(),
            "function main() {\n    DoActions();\n}"
        );
    }

    #[test]
    fn extract_code_single_fence_with_language_tag() {
        let output = "```eps\nconst cp = getcurpl();\nsetcurpl(cp);\n```";

        assert_eq!(
            extract_code(output).unwrap(),
            "const cp = getcurpl();\nsetcurpl(cp);"
        );
    }

    #[test]
    fn extract_code_joins_multiple_blocks_with_blank_line() {
        let output = "```eps\nconst a = 1;\n```\nnoise\n```javascript\nconst b = 2;\n```";

        assert_eq!(
            extract_code(output).unwrap(),
            "const a = 1;\n\nconst b = 2;"
        );
    }

    #[test]
    fn extract_code_normalizes_crlf_to_lf() {
        let output = "```eps\r\nconst a = 1;\r\nconst b = 2;\r\n```";

        assert_eq!(extract_code(output).unwrap(), "const a = 1;\nconst b = 2;");
    }

    #[test]
    fn extract_code_requires_closing_fence_at_line_start() {
        let output =
            "```eps\nconst marker = \"inline ``` is not a close\";\nconst done = true;\n```";

        assert_eq!(
            extract_code(output).unwrap(),
            "const marker = \"inline ``` is not a close\";\nconst done = true;"
        );
    }

    #[test]
    fn extract_code_zero_fences_returns_no_code_with_truncated_raw_output() {
        let raw = format!("prefix {}", "x".repeat(700));
        let err = extract_code(&raw).unwrap_err();

        match err {
            CodexError::NoCode(snippet) => {
                assert!(snippet.contains("prefix "));
                assert_eq!(snippet.len(), 500);
                assert!(!snippet.contains(&"x".repeat(600)));
            }
            other => panic!("expected NoCode, got {other:?}"),
        }
    }

    #[test]
    fn build_prompt_empty_context_marks_none_and_includes_request_and_code_section() {
        let prompt = build_prompt("마린 생성", &[], None);

        assert!(prompt.contains("[참고자료]\n(없음)"));
        assert!(prompt.contains("[요청]\n마린 생성"));
        assert!(prompt.contains("[epScript 코드]"));
        assert!(!prompt.contains("[현재 코드]"));
    }

    #[test]
    fn build_prompt_includes_current_code_section_when_supplied() {
        let context = vec!["source: docs\nUse epScript.".to_string()];
        let prompt = build_prompt("수정", &context, Some("function before() {}"));

        assert!(prompt.contains("[참고자료]\nsource: docs\nUse epScript."));
        assert!(prompt.contains("[현재 코드]\nfunction before() {}"));
        assert!(prompt.contains("[요청]\n수정"));
        assert!(prompt.ends_with("[epScript 코드]"));
    }
}
