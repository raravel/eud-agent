---
task_id: EUD-072-3309
completed_at: 2026-06-05T20:45:00
duration_minutes: 40
coding_retries: 1
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
  input: 70000
  output: 14000
cost_usd: 2.60
profile: quality
models:
  executor: orchestrator-direct
  reviewer: orchestrator-live-probe
---

## Summary

Live regression from EUD-067: `propose_plan` (and every MCP tool call) failed
with **"user rejected MCP tool call"** — `ApprovalMode.deny_all` maps to
approvalPolicy "never", which AUTO-REJECTS MCP tool calls (the guardian's job
under `auto_review` was precisely to auto-approve them). Rollout evidence also
showed the model falling back to `shell_command` when MCP was rejected.

**Probe-driven resolution** (4 live probes):
1. `default_tools_approval_mode="auto"` on the MCP server entry — does NOT
   bypass the "never" rejection (dead end, recorded).
2. approvalPolicy "on-request" reveals the real approval channel:
   `mcpServer/elicitation/request` with
   `_meta.codex_approval_kind == "mcp_tool_call"` (form mode).
3. Replying `{"action": "accept", "content": null}` → tool call
   **status=completed**.
4. Full real-runner path against the live editor: project_status completes,
   args + result text flow to the panel data fields.

**Fix**: the runner builds the low-level `CodexClient` directly (the
high-level `Codex` facade hides `approval_handler`) wrapped in a minimal
`_ClientFacade` (keeps the thread_start/thread_resume seam the retention tests
fake). Thread params are raw camelCase: `approvalPolicy: "on-request"`, no
`approvalsReviewer`, `sandbox: "read-only"`. `_approval_response` accepts ONLY
eud-tools `mcp_tool_call` elicitations and DECLINES everything else —
including commandExecution/fileChange (shell/patch denied; the journaled
eud-tools are the agent's only legitimate effects). Bonus fix discovered by
probe 4: MCP result content blocks arrive as plain dicts live — 
`_tool_result_text` now extracts text from dict and typed shapes (the EUD-068
result field was silently empty otherwise).

## Changes

`server/eud_agent/agent_runner.py` (_ClientFacade, _approval_response,
_thread_start_params, dict content blocks), `server/tests/test_agent_flow.py`
(+3 tests; retention fakes updated to the params-dict contract), features/05
doc.

## Verification

Verify-first: new tests red, green after. Full server suite **532 passed** /
ruff clean. Live probes 3-4 above are the adversarial verification (the real
failure was only reproducible live).

## Harness Sync

features/05 approval paragraph rewritten (EUD-067 → EUD-072 lineage with both
dead ends recorded). No manifest changes.

## Notes

- The read-only sandbox also neuters the observed shell_command fallback
  (non-approval sandboxed commands can read but not write).
- The editor must be restarted (or the server respawned) to pick this up.
