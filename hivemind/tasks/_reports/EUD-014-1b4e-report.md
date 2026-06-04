---
task_id: EUD-014-1b4e
completed_at: 2026-06-04T19:33:10
duration_minutes: 60
coding_retries: 0
verify_retries: 0
review_rounds: 1
verification_required: true
verification_passed: true
blocking_issues: true
review_scores:
  correctness: 6
  spec_compliance: 7
  safety: 8
  clarity: 8
tokens:
  estimated: true
  input: 4430
  output: 6370
cost_usd: 0.54
profile: quality
models:
  executor: claude-opus-4-7
  reviewer: claude-opus-4-7
---

## Summary
WebView2 panel hosting replaces the v6 WPF control panel (+131/-101, then fix +21): Core/Wpf assemblies loaded (app-base probing of the editor-exe-side DLLs), Window("EUD Agent") + WebView2 control with explicit `UserDataFolder = Data\agent\webview2`, EnsureCoreWebView2Async + CoreWebView2InitializationCompleted, NavigationCompleted with navOk + 3s-backoff re-navigate, URL built from the EUD-013 bare globals (`http://127.0.0.1:<agentSrvPort>/?token=<agentSrvToken>`), handle-tracked re-arm per Tick (win.Closed nils panelWin/panelView/panelCoreReady; recreate while project open AND window dead), PANEL command = show/refocus/create/ERROR-not-ready. After the review round: `lastNavUrl` URL-freshness comparison in maintainPanel guarantees re-navigation to the NEW port/token after a server respawn. WPF panel (mkBtn/StackPanel/panelShown/auto-show) fully removed per spec; non-ASCII dropped 1263→582 (Korean WPF strings gone; additions ASCII).

## Changes
- `bridge/ZZZ_10_agent_bridge.lua` — WPF removal + WebView2 hosting region + Tick maintainPanel
- `server/tests/test_bridge_webview_static.py` — 21 checks (comment-stripped code matching, function-body bounded extraction, URL-freshness check)

## Verification
- Two-phase gate: Step A RED (7/20, exit 1) confirmed by orchestrator before Step B; post-fix GREEN — all 7 static suites pass, re-run independently by orchestrator; key fixes confirmed in source (lastNavUrl at 248/267/363, dot-access CoreWebView2 at 264, Uri import gone).
- Test teeth proven by mutation: pre-fix bridge (stash) fails the new freshness + convention checks.
- Scope-drift gate: 2 paths, both declared. The 3 test_deploy_scripts failures in the worktree are environmental (no venv there; pass on main — proven pre-existing via stash).

## Review
Initial verdict BLOCKED (6/7/8/8): F1 — after a server respawn the loaded panel never re-navigates (navOk stays true; WS disconnect is not a NavigationCompleted failure), leaving it on the dead old-token URL — completion criterion 3 unmet (reviewer traced the exact path). Also: F2 newly-added dead Uri import; F3 get_CoreWebView2() parameterized-property form on a plain property; F4/F5 tests binding on comments/file-wide substrings.
Fix round (4df991a): URL-comparison freshness (`panelUrl() ~= lastNavUrl`) chosen over navOk-reset (covers ANY port/token change and keeps failed-nav retry); Uri removed; convention-correct `panelView.CoreWebView2:Navigate(...)`; tests rebuilt on comment-stripped, body-bounded code matching. Reviewer-verified clean aspects retained: removal completeness, no double-creation race, full teardown on win.Closed, UDF rule, PANEL IPC semantics, pcall isolation.

## Incident
### What broke
- Review traced the respawn path end-to-end and found the loaded panel pinned to the dead old-token URL (blocking); plus convention/dead-import/test-strength advisories.
### Why
- navOk models NAVIGATION success only; server death does not produce a NavigationCompleted failure, so a re-navigate trigger keyed on navOk can never fire for the loaded-then-respawned case.
### What fixed it
- One review round: lastNavUrl freshness comparison in maintainPanel (re-navigates whenever the target URL differs), with a mutation-verified static test.

## Harness Sync
- no-op (skip condition): bridge lua in features/01 ## Implementation; test excluded; no manifest. Contract-drift: the REMOVED WPF showPanel/panelShown identifiers are spec'd removals (features/01 "Removal / unchanged") — not drift.

## Notes
- Pre-existing orphaned `WMenus` import left in place (v6 conservatism; flagged, not deleted per surgical-changes rule).
- USER-ASSISTED E2E DEFERRED (verify.md e2e steps 1/7/9/10): panel auto-appears when ready; UDF lands under Data\agent\webview2; blank-navigate retry until respawned server ready (now incl. the fixed new-token path); project-switch re-arm; PANEL refocus.
- Raw harness-reported subagent tokens ≈ 371,095 (68,383 + 99,158 + 74,420 + 129,134).
