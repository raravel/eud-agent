---
task_id: EUD-053-f3ac
completed_at: 2026-06-05T13:20:00
duration_minutes: 25
coding_retries: 0
verify_retries: 0
review_rounds: 0
verification_required: true
verification_passed: true
blocking_issues: false
review_scores:
  correctness: 9
  spec_compliance: 9
  safety: 9
  clarity: 9
tokens:
  estimated: true
  input: 123738
  output: 30935
cost_usd: 4.18
profile: quality
models:
  executor: claude-opus-4-7
  reviewer: claude-opus-4-7
---

## Summary

De-risk spike PASSED: the official Codex Python SDK + MCP tool attachment works on this Windows machine. All four unknowns from features/05 "Engine (single path)" are resolved:

1. **Package**: `openai-codex==0.1.0b3` (module `openai_codex`) — verified official (PyPI author OpenAI, homepage github.com/openai/codex `sdk/python`; reviewer re-verified from installed dist-info; NOT the third-party `codex-sdk-python`). Pre-release; pulls transitive `openai-codex-cli-bin==0.137.0a4` (same publisher, Apache-2.0) whose bundled binary is NOT used — `CodexConfig(codex_bin=<shutil.which("codex")>)` points the SDK at the BYO authenticated CLI. Plus `mcp==1.27.2` (official Anthropic MCP Python SDK).
2. **Thread lifecycle**: `Codex(config=CodexConfig(...))` context manager; `codex.thread_start(model=..., config={...})` → `Thread` (`.id` retained per panel session); `thread.turn(prompt).run()` → `TurnResult(.final_response, .status, .items)`; `.stream()` yields events; `codex.thread_resume(thread_id)` continues context (proven: turn 2 recalled turn 1's marker).
3. **MCP attachment**: **per-thread config injection** — `thread_start(config={"mcp_servers": {name: {command, args, env}}})`. ZERO global state: `~/.codex/config.toml` untouched (verified), nothing to revert. `codex mcp add` is unnecessary.
4. **Real tool round-trip**: dummy stdio FastMCP tool `echo_marker` invoked by codex — proven by sentinel file (`called:EUD53`), an `mcpToolCall` thread item (`server=eud53dummy tool=echo_marker status=completed`), and the wrapper marker `EUD53-ECHO::EUD53` in the final response (the wrapper string is not in the prompt, so it cannot appear without the tool running).

**Measurements** (1 end-to-end run, default model): cold start (SDK init → first stream event) **12.195s**; tool-call latency (turn submit → mcpToolCall completed) **30.792s**.

**Event kinds observed** (`event.method`): `turn/started`, `item/started`, `item/completed` (item types incl. `agentMessage`, `mcpToolCall`), `item/agentMessage/delta`, `item/autoApprovalReview/started|completed`, `item/mcpToolCall/progress`, `thread/tokenUsage/updated`, `turn/completed`.

**Windows quirks**: codex resolves to the `.CMD` shim via `shutil.which` (rules.md); SDK spawns `codex app-server --listen stdio://` via subprocess; ProactorEventLoop set explicitly; `mcp` has no `__version__` (use `importlib.metadata`).

Verify-first gate: the spike script was committed BEFORE any install (commit a0933e6) and failed at step 1 (`ModuleNotFoundError: openai_codex`) — orchestrator reproduced the failure against the SDK-less main venv.

## Changes

- `server/spikes/spike_codex_sdk.py` (new) — 7-step assertive spike; exits non-zero at first failure; NOT pytest-collected (spends real codex tokens; run manually with the venv python).
- `server/spikes/dummy_mcp_tool.py` (new) — one-tool FastMCP stdio server (`echo_marker`), sentinel-file ground truth, no sockets.
- `server/pyproject.toml` — + `openai-codex==0.1.0b3`, `mcp==1.27.2`; `[tool.uv] prerelease = "allow"` (documented: required for the pre-release SDK; global scope noted as advisory risk).
- `server/uv.lock` — re-locked (`prerelease-mode = "allow"` recorded; reviewer scanned transitive additions — all expected: httpx-sse, sse-starlette, python-multipart, pyjwt/cryptography chain, pywin32).
- `hivemind/docs/tech-stack.md` — the two "Planned — v2 agent core" entries moved to Active Dependencies with pins and roles; Planned section removed.

## Verification

- Step A gate: committed pre-install; fails `FAIL [step1] cannot import openai_codex ... ModuleNotFoundError` (orchestrator-reproduced on the main venv).
- Step B: spike passes 7/7 end-to-end (worker-run once; transcript in worker log; orchestrator did NOT re-run to avoid spending more codex tokens — artifacts verified instead).
- ruff: All checks passed (spikes covered). pytest: 311 passed/4 skipped (worktree); merged main tree after `uv sync`: **312 passed, 3 skipped**; `openai-codex 0.1.0b3 / mcp 1.27.2` importable from the main venv.

## Review

Verdict: approve. Rubric: correctness 9, spec_compliance 9, safety 9, clarity 9. No blocking findings.

Advisories (recorded):
- `[tool.uv] prerelease = "allow"` is GLOBAL — future `uv lock` refreshes could silently resolve pre-releases of other deps (uv has no per-package scope; documented in pyproject).
- The spike's step-5 assertion is an OR of the three proofs (any one passes), while the comments say "triple proof" — in the actual run all three signals were present; a stricter spike would AND the sentinel + mcpToolCall.
- Cosmetic: duplicate `OK [step4]` log line.

## Harness Sync

- features/05_agent-core.md `## Implementation` += `server/spikes/*` binding (commit 6d9e0ed, isolated lesson-commit per policy).
- tech-stack.md dep entries: recorded by the task itself (criteria-mandated Planned→Active move) — binding append would be a harness-duplicate; in sync.
- Contract-drift guard: nothing removed/renamed that specs promise (the Planned-section removal IS the task's criteria). Pass.

## Notes

- The main venv now carries the v2 agent-core deps; `scripts/setup_env.ps1` (plain `uv sync`) keeps working — `prerelease` policy lives in pyproject so no command-line flag is needed.
- `hv feedback save` quality-gate dedup is misfiring this session (every non-binding lesson matched unrelated L2 docs, scores 6-8) — two lessons from EUD-049 were skipped per the no-workaround rule; the binding path (auto-bypass) worked. The hv CLI owner should look at the BM25 dedup threshold/corpus.
