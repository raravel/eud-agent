# Lua Bridge (drop-in, v6 import + extension)

Thin tool-call layer inside the editor process. Imported verified v6 (`ZZZ_10_agent_bridge.lua`) and extended with: LIST, NEWEPS, server lifecycle, heartbeat, WebView2 panel hosting. All extensions follow rules.md LUANET-* constraints (everything on Tick, no .NET-exception-prone calls).

## Inputs / outputs

- In: `Data\agent\inbox\*.cmd` (UTF-8 no BOM), `Data\agent\agent.cfg` (paths), editor object state via luanet.
- Out: `Data\agent\outbox\*.result`, `status.txt` (time/compiling/project, skipped during builds), `heartbeat.txt` (every Tick, unconditional), `bridge_loaded.txt` (version marker — fix the stale "v5" string to "v7"), spawned server process, WebView2 panel window.

## New command: LIST

Walk `pj.TEData.PFIles` with the existing `walk()` helper; return one line per file: `<path>\t<EFileType name>`. Obtain the type via the file's type property on the TEFile object (verify the exact member on Windows with the LUA command before wiring; expected `f.Filetype` enum — read with `safestr`). No file contents, no disk writes (unlike DUMP). Project not loaded returns `ERROR: no project`.

## New command: NEWEPS

`NEWEPS <name>` + body from the 2nd line. Chain (verified in v6 PANEL button 2): `TEFile(name, EFileType.CUIEps)` -> `nf.Scripter.StringText = body` -> `pj.TEData.PFIles:FileAdd(nf)` -> `WindowControl.TEOpenFile(nf, 0)`. Pre-check with `findFile(name)`: if the path already exists return `ERROR: duplicate '<name>'` (no auto-suffix). Root folder only; type fixed to CUIEps; memory-only (user saves).

> Decision: see [[decisions/02_neweps-duplicate-error]] — alternatives evaluated, not pursued.

## Server lifecycle

At init: read `agent.cfg` via `File.ReadAllText` (JSON parsed with simple string matching in Lua — only 3 flat keys; no JSON lib in KopiLua); delete any pre-existing `server.ready`; spawn `"<python_exe>" -m eud_agent` with `ProcessStartInfo` (`UseShellExecute=false`, `CreateNoWindow=true`, `WorkingDirectory=<repo_root>\server`), keep the Process object in a global (GC guard + pid source). Per Tick: if `server.ready` exists, validate `pid` equals the spawned process Id (string compare on the JSON value; never `Process.GetProcessById` — throws uncatchable .NET exception for dead pids) and file write time is after bridge start; on first valid ready, mark navigable. If the spawned process has exited (`proc.HasExited` is safe on an owned handle) and a project is open, respawn (max once per 30s).

## WebView2 panel

Replaces the v6 WPF 4-button panel (delete `showPanel` WPF body; `PANEL` command now shows/refocuses the WebView2 window). Creation per the verified probe11 path: load Core+Wpf assemblies from the editor exe folder (DLLs deployed by install script), create `Window` + `WebView2` control; set `CreationProperties` with `UserDataFolder = Data\agent\webview2`; subscribe `CoreWebView2InitializationCompleted`; `EnsureCoreWebView2Async(nil)`; on success `Navigate("http://127.0.0.1:<port>/?token=<token>")` (port+token from validated `server.ready`). Subscribe `NavigationCompleted`: on `IsSuccess==false` set navOk=false and re-Navigate on a later Tick (3s backoff). Re-arm: track the window handle; while project open AND window not alive (closed by project switch), recreate window + control next Tick. Korean labels via `u8()`.

## Removal / unchanged

- v6 commands PING/STATUS/DUMP/GET/SET/GETDAT/SETDAT/BUILD/LUA stay byte-compatible (regression-tested in verify.md e2e step 6).
- The WPF control panel and its auto-show logic are removed; `panelShown` boolean replaced by window-handle tracking.

## Edge cases

- agent.cfg missing/unparseable: write `bridge_error.log` entry, skip server spawn, still serve file IPC commands (bridge degrades to v6 behavior).
- Server exits immediately (bad venv): respawn throttle (30s) prevents a spawn loop; error logged.
- Ready file present but pid mismatch (stale from crash): delete it, respawn.
- Build running: heartbeat still written; inbox processing skipped (existing v6 guard).

## Implementation

- `bridge/ZZZ_10_agent_bridge.lua` — all bridge logic (single drop-in file)
- `scripts/install_dropin.ps1` — deploys the lua + DLLs + agent.cfg
- external: `vendor/webview2/*.dll` (loaded from editor exe folder), `Data\agent\agent.cfg` (read at init)
- [BOUND 2026-06-04 from EUD-010-e3af] `scripts/uninstall_dropin.ps1` — removes lua + Data\agent; -RemoveDlls opt-in (may remove editor-owned WebView2 DLLs — off by default)
