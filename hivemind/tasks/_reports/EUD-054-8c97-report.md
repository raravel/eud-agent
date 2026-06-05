---
task_id: EUD-054-8c97
completed_at: 2026-06-05T14:02:16
duration_minutes: 30
coding_retries: 1
verify_retries: 0
review_rounds: 0
verification_required: true
verification_passed: true
blocking_issues: false
review_scores:
  correctness: 8
  spec_compliance: 10
  safety: 9
  clarity: 10
tokens:
  estimated: true
  input: 324486
  output: 81121
cost_usd: 10.95
profile: quality
models:
  executor: claude-opus-4-7
  reviewer: claude-opus-4-7
---

## Summary

The v2 agent-core policy layer (features/05 "Tools (registry)" + "Triage and plan gating"):

- **`tools.py`** — registry of 32 tools (12 read / 19 write / 1 flow `propose_plan`), exactly the spec set. Every handler validates args BEFORE the bridge call, REUSING the bridge_io helpers/whitelists (no duplication); validation failures, gate blocks, budget exhaustion, and bridge ERRORs all surface as ONE error family (`ToolError`/`PlanRequired`/`BudgetExceeded`). `MutationGate`: writes 1-2 pass without a plan, the 3rd is blocked with a propose_plan directive, lifted by `approve_plan`. `RequestState` (keyed by request_id on `ToolLayer`): 30-action budget (31st → wrap-up message; rejections do NOT consume the budget — only calls that reach the bridge count), mutation counter, plan flags, 3-attempt build-fix counter, `budget_snapshot()` for panel display. `search_docs` exists + validates but returns `[]` with an explicit TODO (RAG wiring deferred per task guidance).
- **`mcp_shim.py`** — stdio FastMCP server codex spawns (dumb transport): reads `server.ready` (port+token, UTF-8 no BOM) from `EUD_DATA_DIR`, fetches `GET /tools/list`, registers one forwarder per advertised spec, forwards each call to `POST /tools/call` with the token. Zero tool logic/validation/whitelists. ok=false re-raised so codex sees a correctable tool error.
- **`app.py`** — token-authenticated `GET /tools/list` + `POST /tools/call` (401 on missing/bad token; ToolError → `{ok:false}` never 5xx; blocking file-IPC via `asyncio.to_thread`); `ToolLayer` on `app.state.tool_layer`. Purely additive — WS/lifecycle (EUD-042 supersede, heartbeat watcher, ready-writer) byte-for-byte untouched.

Verify-first gate: Step A committed the failing test suite first (test-only commit; ImportError — tools.py absent, orchestrator-confirmed from git history).

## Changes

- `server/eud_agent/tools.py` (new, ~850 lines)
- `server/eud_agent/mcp_shim.py` (new, 169 lines)
- `server/eud_agent/app.py` (+48: 2 endpoints + ToolLayer wiring)
- `server/tests/test_tools.py` (new, ~780 lines, 44 tests)

## Verification

- Step A gate: test-only commit b5644ec fails with ImportError (tools.py absent) — confirmed via `git ls-tree`.
- Worker worktree (local uv venv): ruff clean; pytest 391 passed / 4 skipped (after retry; 387 before).
- Merged main tree: ruff clean; **414 passed / 3 skipped**.
- Real codex round-trip intentionally NOT re-run (spends BYO tokens) — transport seam unit-tested; the end-to-end path was proven by the EUD-053 spike with the identical FastMCP/stdio/config-injection shape.

## Review

Verdict: approve. Rubric: correctness 8, spec_compliance 10, safety 9, clarity 10. The reviewer traced the gate/budget/counter-ordering semantics and the security posture as correct, and found one real defect (fixed in retry 1, below) plus two advisories.

Advisories (recorded):
- `propose_plan` is exempt from the 30-action budget — unspec'd but sound (it ends the turn and is how codex satisfies the gate); noted for spec traceability.
- bridge_io's `plugadd` also does an unguarded `int(index)` (out of this task's scope) — the tool layer now guards it upstream; candidate one-liner when bridge_io is next touched.

## Incident

### What broke
- `_h_plugin_add` parsed its index with a bare `int(...)` — the only handler bypassing the validator pattern. A non-integer index (`"abc"`) raised an untranslated `ValueError` that escaped `_dispatch` (translates only ToolError/BridgeError) and the endpoint (catches only ToolError) → unhandled HTTP 500, violating the module's own invariant ("a validation failure is a tool RESULT (ok=false), never a 5xx").

### Why
- The `-1` append sentinel made `_require_nonneg_int` unusable for this one handler, and the workaround skipped the error-translation pattern. Found by the review worker probing handlers against the never-5xx contract.

### What fixed it
- Retry 1 (commit ad73222): local `_require_int_min(value, minimum, label)` helper (tolerates -1, raises ToolError on non-int or < -1); `_h_settings_set`'s redundant Language-only guard simplified to cover ALL non-writable program keys; 4 new tests incl. an endpoint-level regression guard (`{ok:false}`, HTTP 200, never 500 — TestClient would re-raise an unhandled exception, so the test catches a regression structurally).

## Harness Sync

- File-path bindings: `tools.py`, `mcp_shim.py`, `app.py` are all already listed in features/05 `## Implementation`; test file is a test; no manifest changes → no-op.
- Contract-drift guard: purely additive; no spec-promised identifiers removed/renamed. Pass.

## Notes

- The agent-core story (EUD-045) continues: next ready work is expected to be the change journal/rollback (EUD-055) and the CodexSDKRunner (agent_runner.py) per features/05.
- `request_id` is currently supplied by the shim (one per shim process = one codex thread session) or defaults to "default" at the endpoint; the v2 agent runner will own real request ids when it lands.
