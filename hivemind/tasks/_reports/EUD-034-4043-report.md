---
task_id: EUD-034-4043
completed_at: 2026-06-04T21:15:56
duration_minutes: 110
coding_retries: 0
verify_retries: 1
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
  input: 5400
  output: 8200
cost_usd: 0.70
profile: quality
models:
  executor: claude-opus-4-7
  reviewer: claude-opus-4-7
---

## Summary
Full React panel UI on the EUD-033 core + EUD-031 toolchain: Header (Korean conn-state pill, project), ConversationLog (capped store log, spinner on active progress incl. waiting_build — plain rows, NO streamdown pipeline), TargetPicker (accessible custom listbox; GUI disabled + "읽기 전용 파일 형식"; no-project placeholder; new-file toggle + inline NEWEPS validation), InstructionBox (useContext default ON; instruct ALWAYS targets the picker selection — post-fix), ReviewTabs (escaped preview + lang label + UTF-16-consistent 1 MiB truncation; server-diff coloring via src/lib/diff classification, hidden in new-file mode; lazy Monaco as the apply source of truth), DiagnosticsStrip (dismissible, structurally cannot block Apply), ApplyBar (SET=Monaco buffer; NEWEPS=validated name; Cancel). DEP PRUNING: entry 4,060 kB → 265.5 kB (gzip 84.6 kB), Monaco lazy-split 3.79 MB, dist 28→14 MB, 17 deps removed, 4 dead vendored components deleted.

## Changes
- panel/src/lib/{truncate,diff}.ts; panel/src/components/* (8 components + MonacoEditor lazy wrapper); App.tsx/main.tsx rewired; vendored heavy components removed; package.json pruned (+@testing-library devDeps); 9 test files (120 tests total)

## Verification
- Two-phase gate: Step A RED (9 import-failing test files) confirmed by orchestrator before Step B; GREEN after — 120 vitest, build exit 0, contract 13/13, all re-run independently.
- FULL RUNTIME VERIFICATION by orchestrator against the REAL stack (real `python -m eud_agent` + fake-bridge file-IPC responder + fake codex; real RAG = bge-m3 + ECA DB): connect/status/list (real IPC), GUI-disabled, instruct→rag→codex→diagnostics→code, diff tab rendering the real difflib output, Monaco lazy load (local workers), Apply SET body verified ARRIVED at the fake bridge, NEWEPS duplicate rejection + path-separator gating + fresh-name applied round-trip, exactly ONE disconnect log across a server-death outage (22 handshake retries, no crash, no close-code branching), heartbeat self-termination observed live (2x, rig-induced — proving the watcher end-to-end). Screenshot: hivemind/tasks/_reports/EUD-034-4043-panel.png
- Verification retry (1): the live run caught instruct sending the NEWEPS name as target in new-file mode → server "no such file" (the Step-A test had pinned the WRONG payload). Fixed in e4a56ce: instruct always sends the picker target; gating canSendSet in both modes; the wrong assertion corrected + an empty-project gating test added. Re-verified live end-to-end after the fix.
- Scope-drift gate: all changes within panel/** (+ approved vitest.config/tsconfig adds).

## Incident
### What broke
- Live runtime verification surfaced a contract bug invisible to the unit suite: new-file-mode instruct targeted the not-yet-existing NEWEPS filename.
### Why
- The Step-A test itself encoded the wrong payload contract (plausible-but-wrong reading of new-file mode); unit tests can't catch a wrong contract they share with the implementation.
### What fixed it
- Verification retry: e4a56ce per features/02's instruct-target note (added during EUD-018 from the server review) — the cross-document contract note caught exactly this class of bug.

## Review
Verdict PASS (9/9/9/9), no blocking. Reviewer verified the fix against the actual server code (unconditional bridge.get(target)), gate integrity (only the genuinely-wrong assertion swapped; tests increased), XSS sinks all text-escaped (real payload test), truncation UTF-16-consistent, Monaco confined to the lazy chunk (CDN string inert, eager entry clean), no dangling imports after the vendored deletions. Advisories: A1 dead conversation.tsx keeps ai/use-stick-to-bottom alive (→ EUD-035); A2 store.compiling unrendered (harmless); A3 project-name quote-strip belongs in orchestrator._parse_status (server-side, out of panel scope); A4 listbox lacks keyboard nav (acceptable for the WebView2 mouse-driven host, documented trade); A5 no App.test (covered by the live runtime pass).

## Harness Sync
- features/03 ## Implementation already covers the path families (src/components/, src/lib via src/**); no new manifest FILE (package.json edits recorded for the next tech-stack touch — see Notes). Contract-drift clean (the fix ALIGNED code to the documented contract).

## Notes
- tech-stack.md Frontend section pending re-ground (batched into EUD-035): 17 deps removed (incl. @streamdown/*, shiki, streamdown, cmdk, motion, embla, xyflow, rive, media-chrome), devDeps added (vitest, happy-dom, @testing-library/react|dom|user-event|jest-dom).
- Monaco 0.55 EditContext ignores Playwright synthetic keys — manual Monaco typing deferred to the user-assisted editor e2e (buffer-as-apply-source is unit-tested with a Monaco double; the apply path was live-verified with the seeded buffer).
- Raw harness-reported subagent tokens ≈ 715,398 (140,049 + 218,225 + 238,376 + 118,748).
