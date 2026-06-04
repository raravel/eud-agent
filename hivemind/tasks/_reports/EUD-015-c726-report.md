---
task_id: EUD-015-c726
completed_at: 2026-06-04T19:12:24
duration_minutes: 50
coding_retries: 0
verify_retries: 0
review_rounds: 1
verification_required: true
verification_passed: true
blocking_issues: false
review_scores:
  correctness: 8
  spec_compliance: 10
  safety: 9
  clarity: 9
tokens:
  estimated: true
  input: 4000
  output: 5600
cost_usd: 0.48
profile: quality
models:
  executor: claude-opus-4-7
  reviewer: claude-opus-4-7
---

## Summary
`server/eud_agent/bridge_io.py`: BridgeIO file-IPC client — atomic BOM-free `srv-<uuid8>.cmd` writes (encode+write_bytes via temp+os.replace, exact `\n`), 0.2s result polling with 10s/180s busy extension (per-poll status.txt check + latched busy window; on_busy exactly once), `.cmd` left on timeout (BridgeBusy), `.result` deleted after consumption, helpers (ping/status/list_files/get/set/neweps), `cleanup_stale` (srv-* only, never agent_*), and — after the review round — a mid-write `.result` visibility guard: a zero-length read is consumed only after staying zero on a second consecutive poll, defeating the real bridge's non-atomic `File.WriteAllText` create→flush race while still terminating for legitimately-empty LIST results.

## Changes
- `server/eud_agent/bridge_io.py` — new (+268, then +fix)
- `server/tests/test_bridge_io.py` — 19 tests incl. FakeBridge thread, busy-extension with injected timings, namespace-safety, and two race tests (genuinely-empty terminates; mid-write empty-then-content yields full content — mutation-verified: reverting the guard fails it 4/4)

## Verification
- Two-phase gate: Step A RED confirmed by orchestrator (collection ImportError) before Step B; GREEN after — 94 passed; fix round → 96 passed + ruff clean, re-run independently by orchestrator from repo root. Worker re-ran the race test 8x for stability.
- Scope-drift gate: 2 paths, both declared.
- Gate integrity (reviewer): zero assertions/raises/test signatures changed between Step A and B; only hygiene edits + a fake-bridge atomicity fix.

## Review
Verdict PASS (8/10/9/9). One advisory-driven fix round (no blocking findings):
- A1 (fixed): the real bridge writes `.result` non-atomically (v6 `File.WriteAllText`, untouchable v6 code); the reader could consume a created-but-unflushed empty file — and an empty LIST result is VALID (zero files), so the race could silently report a populated project as empty. Fixed reader-side with the two-poll empty-stability check.
- A2 (fixed): stale deadline comment relocated to the actual per-poll window-selection site.
- A3 (accepted): settable derivation by case-insensitive substring (CUI/SCA/RawText families) — a hypothetical future "GUICustom" enum name would false-positive, but settable is advisory metadata and a real SET on GUI still gets the bridge ERROR; revisit only if the enum surface grows.

## Incident
### What broke
- Review found the mid-write `.result` race (silent empty-LIST failure mode) that the Step-B fake-bridge atomicity fix had inadvertently masked.
### Why
- The real bridge's `File.WriteAllText` is not atomic; the test fake was made atomic to fix test flakiness, which made the suite blind to the production race.
### What fixed it
- Review round (commit 3ff68d3): two-consecutive-poll zero-length stability check in `_consume_result`, plus a deterministic race test (event-synchronized) and a genuinely-empty terminator test.

## Harness Sync
- no-op (skip condition): bridge_io.py already in features/02 ## Implementation; test excluded; no manifest. Contract-drift clean.

## Notes
- LATENT BEHAVIOR for the orchestrator/diff task (carry-forward): result bodies are read with universal-newline mode — `\r\n` normalizes to `\n`. Fine for LIST/STATUS/OK/ERROR (line-parsed), but GET file content loses CRLF fidelity; if the diff/SET round-trip ever needs byte-exact newlines, switch the result read to `newline=""` semantics or normalize deliberately on both sides.
- Raw harness-reported subagent tokens ≈ 375,255 (76,385 + 105,271 + 53,733 + 139,866).
