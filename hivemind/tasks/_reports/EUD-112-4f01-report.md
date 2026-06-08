---
task_id: EUD-112-4f01
completed_at: 2026-06-08T18:16:42Z
duration_minutes: 95
coding_retries: 1
verify_retries: 0
review_rounds: 1
verification_required: true
verification_passed: true
blocking_issues: true
providers:
  coder: codex
  reviewer: codex
review_scores: {}
tokens:
  estimated: true
  input: 0
  output: 0
cost_usd: 0.00
profile: mixed
models:
  executor: gpt-5.5
  reviewer: gpt-5.5
codex_usage:
  coder_session_id: 019ea664-fd1a-7450-b265-d78cf6f607be
  coder_tokens:
    input: 3328050
    output: 22209
    total: 3350259
  reviewer_tracked: false
---

## Summary
Ported `server/eud_agent/codex_client.py` to `src-tauri/src/codex_client.rs`. The module
provides: `resolve_codex_cmd()` (CODEX_CMD env override else `which::which("codex")`,
fail-fast NotFound — never a bare `codex`), `extract_code()` (line-based fenced-block
extraction, CRLF→LF normalization, blank-line join, 500-char NoCode snippet on zero
fences), `build_prompt()` (verbatim Python `[참고자료]/[현재 코드]/[요청]/[epScript 코드]`
layout + the verified SYSTEM_PROMPT), and `CodexClient::{new, generate}` (tokio subprocess:
`exec --skip-git-repo-check`, cwd=repo_root, prompt written to stdin then closed,
BrokenPipe/ConnectionReset tolerated, 600s timeout, stderr tail appended on NoCode). All
behavior mirrors the verified Python source (feature 11: "behavior matches v1").

## Changes
- `src-tauri/src/codex_client.rs` (new, 335 lines) — full port + 8 unit tests + module
  rationale docs.
- `src-tauri/src/lib.rs` — `pub mod codex_client;` (alphabetically ordered).
- `src-tauri/Cargo.toml` — `which = "8"`.
- `Cargo.lock` — `which 8.0.3` (+ transitive `windows-sys` bump pulled by which).

## Verification
Run by the orchestrator in the worktree against the warmed shared CARGO_TARGET_DIR:
- `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` → clean (exit 0).
- `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings` → no
  warnings (exit 0).
- `cargo test --manifest-path src-tauri/Cargo.toml codex` → 8 passed; 0 failed.
- Verify-first gate: the 8 tests were confirmed FAILING (todo!() stubs) before
  implementation, then PASSING after.

Completion criteria:
- [PASS] Never spawns bare `codex`; unresolved shim fails fast — `resolve_codex_cmd` returns
  `CodexError::NotFound`, `CodexClient::new` rejects empty/missing paths, `generate` spawns
  the resolved `PathBuf`.
- [PASS] Prompt passed via stdin; fenced-block extraction unit-tested — `generate` writes
  prompt to piped stdin then closes; 6 extract_code + 2 build_prompt tests.
- [PASS] `cargo test codex` passes — 8/8.

## Review
Codex review (`codex review --base main`) raised two findings, both `[P2]` (blocking under
the Codex priority mapping). Orchestrator judgment:

- **[P2] "Request fenced output before extracting it"** — JUDGED NOT A DEFECT. The
  prompt/extractor tension is a verbatim port of the verified Python `codex_client.py`: the
  `codex exec` CLI wraps output in fenced blocks, and NoCode-on-unfenced is the intended
  safety behavior (rules.md: fail with raw output rather than apply noise). Changing it
  would violate "behavior matches v1".
- **[P2] "Include the required safety prompt sections"** — JUDGED OUT OF SCOPE. The
  `[first principles]`/evidence/message-format guardrails are assembled by the
  engine/orchestrator (feature 11 engine section), not this low-level composer — mirroring
  Python's `engine.py` wrapping `codex_client.build_prompt`. That work belongs to the
  engine.rs task, not EUD-112.

Both findings stemmed from the Rust port dropping the Python module's explanatory
docstrings. Resolved (1 review round) by porting the rationale back as module + function
doc comments — ZERO behavior change — making the fenced-output contract and the
engine-layer boundary explicit. fmt/clippy/test re-confirmed clean after the doc edit.

## Harness Sync
- features/11_rust-backend-core.md — `src-tauri/src/codex_client.rs` already listed under
  ## Implementation (no-op).
- tech-stack.md ## Active Dependencies += `which 8.0.3` (BOUND, commit f0e1cf9).
- Note: tech-stack ## Target Rust Stack still lists the floor `which 7`; the actual pin is
  `which 8` (latest; satisfies the floor as a newer major). Left for user review — not
  auto-edited.
- `src-tauri/src/lib.rs` not bound to a feature: it is the shared crate-root module
  registry touched by every module task, not a feature-specific implementation file.

## Notes
- Profile `mixed` specifies executor/reviewer `gpt-5.2-codex`, but that model is rejected by
  this Codex install ("not supported when using Codex with a ChatGPT account"). Fell back to
  the configured default `gpt-5.5` for both coder and reviewer.
- The Codex `workspace-write` sandbox could not write the worktree git index
  (`.git/worktrees/...` lives outside the worktree root) nor reach the network/shared cargo
  cache. Consequence: the orchestrator (unsandboxed) performed all git commits and ran all
  cargo verification against the warmed shared target on the worker's behalf. The code is
  the worker's; only landing + verification were done by the orchestrator.
- `codex exec resume` has no `-C`/`-s` flags; resumes were run from the worktree cwd with
  `-c sandbox_mode=workspace-write -c approval_policy=never`.
- Scope was extended (orchestrator-authorized, no in-flight peers) to Cargo.toml, lib.rs,
  Cargo.lock — all mechanical consequences of the codex_client port.

## Incident

### What broke
- After the initial implementation (Step B), the `which` crate was added to Cargo.toml but
  never used (dead dependency) and the task deliverable "resolve the .cmd shim via the
  `which` crate" was unmet. Separately, `cargo fmt --check` failed: `pub mod codex_client;`
  was inserted after `pub mod config;` (rustfmt sorts module declarations).
- The Codex review then raised two `[P2]` findings (see Review).

### Why
- The worker treated which-resolution as the caller's concern (as in Python, where config.py
  resolves) and left it out of codex_client, leaving the added dep unused. The fmt failure
  was a simple mis-ordered insertion. The review findings stemmed from the port dropping the
  Python module's explanatory docstrings, so the intentional fenced contract and the
  engine-layer boundary were not self-evident in the Rust source.

### What fixed it
- Coding round (Step C): added `resolve_codex_cmd()` using `which::which` + CODEX_CMD
  override, and ran `cargo fmt` to reorder the module list. Re-verified fmt/clippy/test green.
- Review round (Step D): ported the Python rationale back as doc comments (no behavior
  change), making the fenced-output contract and engine-layer split explicit.
