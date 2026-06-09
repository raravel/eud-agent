---
task_id: EUD-124-9090
completed_at: 2026-06-09T12:10:00
duration_minutes: 28
coding_retries: 0
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
  executor: gpt-5.2-codex
  reviewer: gpt-5.2-codex
codex_usage:
  coder_session_id: 019eaa45-0b69-7782-80f5-47461606b91f
  coder_tokens:
    input: 4619950
    output: 39789
    total: 4659739
  reviewer_tracked: false
---

## Summary
Extended the EUD-114 tool-layer foundation in `src-tauri/src/tools.rs` with the full eud-tools
read/write/flow registry, MCP-style exposure, per-request admission gating, and budgets — all
self-contained in `tools.rs` (codex-thread attachment is a later task, out of scope).

Implemented:
- `tool_registry()` enumerating every feature-11 tool (12 read, 21 write/journaled, `propose_plan`,
  `memory_write`) with name, one-line description, `mutating` flag, and a verbatim MCP `inputSchema`
  using the real parameter names (EUD-087: e.g. `xdat_set{dat,name,objId,value}`, never a generic
  derived wrapper). Settable file types restricted to `CUIEps`/`CUIPy`/`RawText` — no SCA.
- `mcp_tool_descriptors()` advertising `{name, description, inputSchema}` verbatim from the registry.
- `RequestState` extended with `request_id`, `plan_approved`, `action_count`, `mutation_count`,
  `build_fix_attempts`; `for_request()`, `start_request()` (per-request reset — EUD-064 live
  request-id scoping), `approve_plan()`.
- `admit_tool_call()` single admission check: arg validation (required-arg presence with a
  self-correcting `Usage: tool(args)` line; bounds/whitelist/FileType guards) → action budget (30) →
  evidence gate (EUD-090, reuses `check_evidence_gate`) → mutation gate (3rd write w/o plan →
  `propose_plan`) → build self-fix budget (3) → btn/xdat first-principles rails. Rejected calls do
  not consume budget counters.

## Changes
- `src-tauri/src/tools.rs` (+1410/-3): registry, MCP descriptors, expanded RequestState, admission
  gates, budgets, arg validation, schema helpers. Existing EUD-114 evidence/btn/xdat code and tests
  kept intact (import-then-extend).

## Verification
Run by the orchestrator in the worker worktree against the warm shared cargo cache
(`CARGO_TARGET_DIR=.cargo-shared-target`; `panel/dist` copied in to satisfy the Tauri context macro;
`ORT_LIB_PATH` resolved the ONNX Runtime link):
- `cargo test --manifest-path src-tauri/Cargo.toml` → `test result: ok. 82 passed; 0 failed; 1 ignored`
- `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings` → clean (exit 0)
- `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` → clean (exit 0)
- Completion criteria: [PASS] registry MCP verbatim schemas + pre-bridge arg validation;
  [PASS] mutation gate + action/build budgets + evidence gate; [PASS] cargo test + clippy.

## Review
Codex review (`codex review --base 820f545`) raised 3 blocking findings, all judged valid and fixed
in one review round:
- [P1] `docs_searched` was set at admission time → a later failed RAG search would still leave the
  evidence gate lifted. Fixed: admission no longer records `search_docs`; the (future) execution
  layer records a successful search via `record_search_docs()`.
- [P2] `memory_write` was subject to the mutation gate/counter despite being non-journaled and
  plan-gate-exempt. Fixed: `counts_against_mutation_gate()` excludes `memory_write`; it consumes only
  the action budget.
- [P2] `plugin_add`'s `-1` append sentinel was rejected by blanket non-negative integer validation.
  Fixed: per-field minimum bound allows `plugin_add.index == -1` while other integer fields stay
  non-negative.
Regression tests added for all three; re-verification (82 passed / clippy / fmt) confirmed.

## Harness Sync
- no-op: the only touched file (`src-tauri/src/tools.rs`) is already documented in
  `features/11_rust-backend-core.md ## Implementation`; no manifest file changed. Contract-drift
  guard skip-condition satisfied.

## Notes
- Codex `workspace-write` sandbox on Windows cannot write outside the worktree: it could not write
  the warm shared cargo cache, the parent `.git/worktrees/.../index.lock` (commits), and recompiled
  ort-sys when forced to a worktree-local target. The orchestrator handled all commits and ran the
  authoritative verification itself. Granting `sandbox_workspace_write.writable_roots` for the shared
  cache let the worker self-compile (clippy/fmt) but `.git` writes still failed, so commits stayed
  orchestrator-side.
- The Tauri lib's `generate_context!()` macro panics unless `panel/dist` exists; it is gitignored and
  absent in fresh worktrees. Orchestrator copied it from the main repo for verification only.

## Incident

### What broke
- Code review found 3 blocking findings (1×P1, 2×P2): evidence-gate could be bypassed by a failed
  search (docs_searched marked at admission), `memory_write` was incorrectly plan-gated, and
  `plugin_add`'s documented `-1` append sentinel was rejected by blanket non-negative validation.

### Why
- The first cut recorded `search_docs` success at the admission layer rather than after execution,
  and applied a single generic mutating-tool gate + a single non-negative integer rule uniformly,
  ignoring tool-specific exemptions (memory_write is non-journaled) and sentinels (plugin append -1).

### What fixed it
- One review round: moved successful-search recording out of admission (doc-commented for the
  execution layer), added `counts_against_mutation_gate()` to exempt `memory_write`, and introduced a
  per-field integer minimum so `plugin_add.index` accepts `-1`. Three regression tests added; all 82
  tests + clippy + fmt pass.
