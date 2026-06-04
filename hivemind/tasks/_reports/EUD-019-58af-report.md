---
task_id: EUD-019-58af
completed_at: 2026-06-04T22:37:36
duration_minutes: 45
coding_retries: 0
verify_retries: 0
review_rounds: 0
verification_required: true
verification_passed: true
blocking_issues: false
review_scores:
  correctness: 9
  spec_compliance: 8
  safety: 10
  clarity: 9
tokens:
  estimated: true
  input: 4000
  output: 6200
cost_usd: 0.53
profile: quality
models:
  executor: claude-opus-4-7
  reviewer: claude-opus-4-7
---

## Summary
`server/eud_agent/lsp_gate.py` (449 lines): advisory epscript-lsp diagnostics with a NO-RAISE public surface — `diagnose(code, *, timeout=2.0) -> list[dict]` returns [] on EVERY failure (no node, no @eps-server/server package, spawn error, timeout, framing/JSON errors). Resolution: shutil.which("node") + _locate_server_entry (server/node_modules → node-relative global → ProgramFiles → NODE_PATH; package.json bin/main parsing with conventional fallbacks). LSP JSON-RPC over stdio (Content-Length framing, incremental split/merged-chunk reader) — initialize → initialized → didOpen(in-memory eps doc) → publishDiagnostics for our URI within the budget → shutdown/exit; reader in a daemon thread joined against the deadline, kill-to-unblock + always-reap. Maps to [{line(1-based), severity(int, default 3), message}]. Matches the orchestrator's _lsp_stage seam exactly (single positional arg; module presence flips the lazy-import path — one EUD-018 orchestrator test legitimately adjusted to the machine-independent intent with resolution neutralized).

## Changes
- `server/eud_agent/lsp_gate.py` (new), `server/tests/test_lsp_gate.py` (17 tests incl. 1 live-gated), `server/tests/test_orchestrator.py` (1 test adjusted — scope-add approved)

## Verification
- Two-phase gate: Step A RED (collection ImportError) confirmed by orchestrator; GREEN after — 234 passed + 4 skipped + ruff clean (reviewer independently reproduced).
- Live resolution honestly reported: node present; @eps-server/server NOT installed on this machine (local + global checked) → _resolve_lsp None, live test self-skips with an actionable reason. Package deliberately NOT installed (user environment decision).
- Scope-drift gate: 3 paths (1 via approved scope-add).

## Review
Verdict PASS (9/8/10/9), no blocking. Reviewer audited EVERY diagnose() path for the no-raise guarantee (outer+inner excepts, send-closure broken-pipe coverage, daemon-thread isolation, defensive _shutdown with kill+wait on all exits — Windows-sound: kill closes the child stdout so the blocked read returns EOF). Protocol minimalism verified incl. malformed-frame no-hang. Gate integrity: the adjusted orchestrator assertion's INTENT preserved (stage order + diagnostics==[]). Advisories: (1) spec text says in-module failures should emit progress detail:"skipped" but a clean []-return is indistinguishable from ran-and-found-nothing — ZERO user impact (the React panel ignores detail; renders "진단 검사 중…" either way); a None-vs-[] failure signal would be a behavior change with no benefit today; (2) stderr pipe never drained — theoretical 64KB-chatty-server block, bounded by the 2s budget + kill.

## Harness Sync
- no-op (skip condition): lsp_gate.py already in features/02 ## Implementation; tests excluded; no manifest. Contract-drift: the detail:skipped nuance recorded here as a documented spec-vs-impl note (advisory; revisit only if the panel ever surfaces detail).

## Notes
- For a real LSP round-trip: `npm --prefix server install @eps-server/server` (or global) then `EUD_LSP_LIVE=1 pytest -k live` — left to the user (optional advisory feature per rules.md).
- Raw harness-reported subagent tokens ≈ 262,394 (75,198 + 107,163 + 80,033).
