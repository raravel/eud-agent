---
task_id: EUD-033-a00f
completed_at: 2026-06-04T20:25:36
duration_minutes: 60
coding_retries: 0
verify_retries: 0
review_rounds: 1
verification_required: true
verification_passed: true
blocking_issues: false
review_scores:
  correctness: 9
  spec_compliance: 9
  safety: 9
  clarity: 10
tokens:
  estimated: true
  input: 4600
  output: 6800
cost_usd: 0.58
profile: quality
models:
  executor: claude-opus-4-7
  reviewer: claude-opus-4-7
---

## Summary
Framework-agnostic typed core for the React panel: `panel/src/ws/protocol.ts` (discriminated unions + runtime guards for the full WS protocol both directions; ftype as STRING enum name after the reconciliation round), `panel/src/ws/client.ts` (injectable WebSocket-factory/location seams; 2s backoff without timer stacking; status+list re-request on every open; was-open single-disconnect-log fix; unknown-type/bad-JSON tolerance via onLog, never throws; WS_OPEN=1 spec constant for the happy-dom limitation), `panel/src/state/store.ts` (plain TS subscribe store; all spec mermaid transitions; 500-entry log cap dropping oldest; Send gating truth table incl. empty-but-open-project SET gating with new-file mode preserved; applying/waiting reconnect → ready; NEWEPS validation; no-project signal via the contractual bridge literal; compiling preserved). vitest 3.2 + happy-dom plumbing (devDeps only), 55 tests.

## Changes
- `panel/src/ws/protocol.ts`, `panel/src/ws/client.ts`, `panel/src/state/store.ts` (new)
- `panel/src/ws/*.test.ts`, `panel/src/state/store.test.ts` (55 tests), `panel/vitest.config.ts`, `panel/package.json`/`package-lock.json` (vitest+happy-dom devDeps, test scripts), `panel/tsconfig.app.json` (test exclude — scope-add approved)

## Verification
- Two-phase gate: Step A RED (3 import-error test files, npm test exit 1) confirmed by orchestrator before Step B; post-fix GREEN — 55/55 vitest, build exit 0 (strict tsc), contract test 13/13, all re-run independently by orchestrator.
- Scope-drift gate: 8 paths, all declared (tsconfig.app.json via approved scope-add).
- No new runtime dependencies (verified in package.json diff).

## Review
Verdict PASS (9/9/9/10), no blocking. The CROSS-CHECK against the in-flight EUD-018 server caught two real pre-merge contract divergences, fixed in one round (65bed2e):
1. ftype was typed number; the server sends the EFileType enum NAME as a string — fixed to string with provenance JSDoc.
2. The server has no list{error} path (bridge "ERROR: no project" → error{message}) — the spec'd no-project placeholder could never fire; the store now keys on the contractual "no project" literal (case-insensitive) to clear hasProject/files/target. Unrelated errors (e.g. duplicate) leave the project intact (tested).
3. status.compiling was dropped — now stored in the snapshot.
Reviewer also verified: timer lifecycle leak-free, was-open re-arm semantics, WS_OPEN constant soundness (WHATWG-fixed), type honesty (no any/unguarded casts), fake-timer determinism.

## Incident
### What broke
- Cross-review found the client typed ftype as number vs the server's string, and modeled a list{error} signal the server never emits (dead no-project path).
### Why
- The client was authored from architecture.md alone; the concurrently-built server's concrete emissions (enum-name ftype, BridgeError→error{message}) diverged in details the doc didn't pin.
### What fixed it
- One reconciliation round, client-side only (no protocol change): string ftype, contractual-literal no-project detection, compiling preservation — 6 new tests.

## Harness Sync
- features/03 ## Implementation already lists panel/src/ws/client.ts + src/state/ — no-op for paths. Manifest changed (vitest/happy-dom devDeps) — recorded here for tech-stack: vitest ^3.2.6 + happy-dom ^16.8.1 (dev-only, panel test runner); next tech-stack touch should add them to the Frontend list.
- Contract-drift guard: clean (additive; the type corrections ALIGN code to the documented contract).

## Notes
- CARRY-FORWARD → EUD-034: the client treats any WS close as retry (correct — browsers surface the pre-accept 4403 as a 1006 handshake failure; never branch on close codes).
- Raw harness-reported subagent tokens ≈ 305,568 (73,683 + 103,044 + 128,841) + review 93,599 ≈ 399,167.
