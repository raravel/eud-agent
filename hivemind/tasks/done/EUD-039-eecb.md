---
completed_at: '2026-06-05T00:13:29.399037'
created: '2026-06-05'
depends_on: []
id: EUD-039-eecb
parent: EUD-006-50d8
priority: high
scope:
- bridge/ZZZ_10_agent_bridge.lua
- server/tests/test_bridge_lifecycle_static.py
status: done
title: Bridge Tick DispatcherTimer at Normal priority (WebView2 starves Background
  heartbeat/IPC)
type: bug
updated: '2026-06-05'
---

## Description
Third live editor E2E defect (EUD-024, found 2026-06-05 00:00 with the panel running and RAG warming): the bridge's heartbeat/IPC `DispatcherTimer` is constructed with the parameterless `DispatcherTimer()` constructor, which defaults to `DispatcherPriority.Background` (enum value 4). The live WebView2 panel (React app + animation while RAG warms) generates a continuous stream of `DispatcherPriority.Render` (7) work on the editor UI thread, which PREEMPTS the Background-priority tick. Result: the tick — which writes `heartbeat.txt` AND processes the inbox `.cmd` files — fires only every ~9-10s instead of 1s, and intermittently stalls entirely (heartbeat observed 54s stale). Two failures cascade: (1) `.result` latency exceeds the server's 10s default poll timeout → the panel shows "editor busy" on status/list/instruct; (2) heartbeat staleness > 60s → the server self-terminates, dropping the panel.

PROVEN by orchestrator live A/B on the running editor (LUA hot-reload channel): a SECOND DispatcherTimer created at `DispatcherPriority.Normal` (9), under the identical WebView2 load, fired every 0.2-0.3s and kept heartbeat.txt fresh, while the original Background timer (writing status.txt) stayed at ~9-10s. Same process, same panel load, priority the only variable.

This is a regression introduced by adding the WebView2 panel (EUD-021/026): v6 file-IPC at Background priority was reliable only because nothing was animating the UI thread. The fix restores reliability for the panel-present topology.

Fix (surgical, import-then-extend): construct the timer at a priority above Render so it cannot be starved by the panel —
- import the enum: `local DispatcherPriority = luanet.import_type("System.Windows.Threading.DispatcherPriority")` (DispatcherPriority lives in WindowsBase, already load_assembly'd; place the import with the other import_type lines).
- change `local timer = DispatcherTimer()` to `local timer = DispatcherTimer(DispatcherPriority.Normal)` (pass the enum OBJECT, not a raw number — rules.md).

Normal (9) > Render (7) is empirically sufficient (the A/B proof); Send (10) is unnecessarily aggressive. The heartbeat-first / IsCompilng-early-return tick structure is unchanged.

## Spec References
- [[features/01_lua-bridge|01_lua-bridge]] `../docs/features/01_lua-bridge.md` - Server lifecycle / heartbeat tick
- [[rules]] `../docs/rules.md` - Lua bridge crash rules (enum objects via import_type; unconditional heartbeat); Heartbeat / server shutdown
- [[architecture]] `../docs/architecture.md` - Boot and lifecycle (heartbeat 60s self-terminate)

## Completion Criteria
- [ ] Bridge constructs the Tick DispatcherTimer with an explicit `DispatcherPriority` enum arg at a priority above Render (Normal); `DispatcherPriority` imported via import_type
- [ ] test_bridge_lifecycle_static binds the priority-arg construction (fails against the current `DispatcherTimer()` — verify-first Step A RED) and the enum import; the existing _tick_region / heartbeat-first checks still pass
- [ ] ruff clean; full pytest green incl. all bridge static suites; no non-ASCII bytes added; BOM-free