---
task_id: EUD-113-ba2a
completed_at: 2026-06-08T22:35:08
duration_minutes: 95
coding_retries: 0
verify_retries: 0
review_rounds: 1
verification_required: true
verification_passed: true
blocking_issues: true
providers:
  coder: codex
  reviewer: codex
tokens:
  estimated: false
  input: 9013409
  output: 48572
cost_usd: 0.00
profile: mixed
models:
  executor: gpt-5.2-codex
  reviewer: gpt-5.2-codex
codex_usage:
  coder_session_id: 019ea74c-a1bc-7411-b0de-a4e78be15061
  coder_tokens:
    input: 9013409
    output: 48572
    total: 9061981
  reviewer_tracked: false
---

## Summary
Implemented `src-tauri/src/engine.rs`: the v2 system-prompt assembly and the
single-shot instruct seam for the eud-agent Tauri/Rust core. Ported the prompt
section constants and ordering from the retired `server/eud_agent/engine.py`
`build_system_prompt`: `[first principles]` is assembled before `[reference
context]`, with `[epscript]`/`[build]`/`[map locations]`/`[evidence]` between
them and `[message format]`/`[triage]` after. `resume_turn_text` prepends
refreshed `[project state]`/optional `[project memory]`/`[reference context]`
then a literal `[user message]` header before the user's text (EUD-092).
`run_instruct` is a unit-testable instruct seam: it composes the full guardrail
system prompt ahead of the low-level `codex_client::build_prompt` request
framing, calls an injected code generator, and returns the proposed code plus a
`similar`-based unified diff against the current target content.

The agentic tool loop is intentionally deferred — it depends on the tools layer
(EUD-114), which is not yet done. `run_instruct` is the single-shot seam the IPC
`instruct` path will later drive; the full propose_plan/search_docs/edit loop
lands with EUD-114.

## Changes
- `src-tauri/src/engine.rs` (new, 394 lines) — prompt-section constants,
  `build_system_prompt`, `resume_turn_text`, `run_instruct`, `unified_diff`, 6 unit tests.
- `src-tauri/src/data/first_principles.md` (new) — bundled prompt asset, SHA256-identical
  to `server/eud_agent/data/first_principles.md`, loaded via `include_str!`.
- `src-tauri/src/lib.rs` — `pub mod engine;` wiring.
- `src-tauri/Cargo.toml` — `similar = "2"` (unified diff).
- `Cargo.lock` — lock entries for `similar` (was already a transitive entry).

## Verification
Run by the orchestrator in the worktree (RUSTFLAGS cleared — no `/FORCE:UNRESOLVED`;
the binary linked cleanly, confirming the ORT link gap is absent in the toolchain):
- `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` → exit 0.
- `cargo test --manifest-path src-tauri/Cargo.toml engine` → `6 passed; 0 failed; 54 filtered out`.
- `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings` → exit 0.

Completion criteria:
- [PASS] instruct produces code+diff; `[first principles]` precedes `[reference context]`
  (`run_instruct` → `InstructOutput{code, diff}`; ordering test passes).
- [PASS] `[user message]` header applied on resumed turns (EUD-092) — `resume_turn_text` + test.
- [PASS] `cargo test engine` passes (6/6).

## Review
Codex review (`codex review --commit <sha>`) raised one blocking finding:
- **[P1]** `run_instruct` fed only the low-level `[참고자료]/[요청]` prompt to the
  generator, so the engine-level `[first principles]`/`[evidence]`/`[message format]`
  guardrails never reached the model — the instruct path bypassed the crash/desync
  refusal rules the task exists to enforce.

Fix (1 review round): `run_instruct` now prepends the full `build_system_prompt(...)`
output ahead of the request framing and calls `build_prompt` with an empty context
slice so the RAG chunks render once (in `[reference context]`). The `run_instruct`
test asserts the generator receives `[first principles]`, `[evidence]`,
`[message format]`, `[reference context]`, plus `[요청]`/`[현재 코드]`. Re-verified
green by the orchestrator. No P1/P2 remaining.

## Harness Sync
- tech-stack.md ## Active Dependencies += `similar 2` (BOUND)
- features/11_rust-backend-core.md ## Implementation += `src-tauri/src/lib.rs` (BOUND)
- features/11_rust-backend-core.md ## Implementation += `src-tauri/src/engine.rs` (BOUND)
- No contract drift (new files + additions only; no removed/renamed spec identifiers).

## Notes
- coder token counts are the SUM of the 3 codex coder turns (Step A / Step B / review-fix)
  on the same resumed thread; `input` is dominated by cached re-feeds
  (cached_input ≈ 8.2M of 9.0M). `cost_usd = 0.00`: codex runs on a BYO ChatGPT
  subscription, and the claude pricing map carries no codex model rates.
- Reviewer tokens not tracked (`codex review` does not emit a `--json` usage stream here).
- Scope was extended by the orchestrator (sequential mode, no in-flight peers, so
  disjointness is trivial) to cover the module wiring + asset + manifest:
  `src-tauri/src/lib.rs`, `src-tauri/Cargo.toml`, `src-tauri/src/data/first_principles.md`,
  `Cargo.lock`. The sandboxed codex worker could not run `hv task scope-add` itself
  (workspace-write sandbox blocks main-repo writes), so the orchestrator authorized them.

## Incident

### What broke
1. The first coding-worker launch hung for 3+ hours with no completion notification.
2. Codex review found a [P1]: the instruct seam bypassed the guardrail prompt sections.

### Why
1. The worker was spawned as `codex exec ... --json -o <file> <giant-argv-prompt> | Select-Object -Last 5`.
   Two faults compounded: (a) PATH `codex` resolves to `codex.ps1`, which the harness's
   non-interactive shell never actually executed (worktree stayed empty); (b) `Select-Object
   -Last 5` buffers the entire `--json` stream until the process exits, so the backgrounded
   task could never surface output or "complete" — it just sat. codex itself and auth were
   fine (`codex.cmd --version` = 0.137.0, logged in via ChatGPT).
2. `run_instruct` reused only `codex_client::build_prompt` (the low-level request framing),
   not `build_system_prompt`, so the assembled guardrails were never sent to the generator.

### What fixed it
1. Re-ran via the `codex.cmd` shim with the prompt piped on **stdin**, no `Select-Object`
   buffering, and the event stream redirected to a plain file (`*> out.txt`); the `-o` file
   holds only the final message. Confirmed alive by rising process CPU. scope-add was moved
   to the orchestrator since the worker's sandbox blocks main-repo writes.
2. Resumed the same codex thread; `run_instruct` now composes `build_system_prompt(...)`
   ahead of the request framing (Step B fix round). Re-verified green.
