# Feature 14: Slim Lua bridge (file-IPC tool layer only)

Reduce `bridge/ZZZ_10_agent_bridge.lua` to a thin file-IPC tool layer. Remove everything
tied to in-editor WebView2 hosting and the server-spawn lifecycle; the standalone app is
now the UI and is launched independently.

> Decision: see [[decisions/08_tauri-rust-rewrite]] and
> [[decisions/12_bootstrap-download-distribution]].

## Removed from the bridge
- WebView2 window creation/navigation/recreation and panel re-arm (window-handle tracking).
- Server spawning (`System.Diagnostics.Process`), `server.ready` write/validate, and the
  `webview2/` user-data folder.
- The `PANEL` command (no panel to show).
- The `agent.cfg` reads of `python_exe`/`repo_root` (no python, no repo at runtime).

## Retained (import-then-extend; keep verified v6 paths)
- Inbox/outbox processing on the `DispatcherTimer.Tick` (UI thread).
- Commands: PING, STATUS, LIST, GET, SET, NEWEPS, GETDAT/SETDAT, BUILD, LUA.
- All crash rules (rules.md "Lua bridge"): UI-thread access, `arr[i]` indexer,
  `:get_Prop(args)`, gsub truncation, `u8()`, `val or ""`, SETBTN IsDefault clear,
  SET/NEWEPS memory-only + CUI/RawText-only + NEWEPS duplicate ERROR.

## Reversed lifecycle signals
- `heartbeat.txt` and `status.txt` are STILL written every Tick before the `IsCompilng`
  early-return (unconditional). Their consumer is now the APP (reads them to know the editor
  is alive + whether a build is running), not a self-spawned server. The busy `status.txt`
  reports `compiling=True` with the project line CACHED from the last idle Tick (never touch
  `pjData` while compiling).
- The editor never kills the app; the app never kills the editor. No self-terminate path.

## Path coordination (no baked Lua literal)
- The bridge locates `Data\agent\` editor-relative (existing mechanism), so no absolute path
  is needed in the .lua — avoids the KopiLua Latin1 mojibake trap for non-ASCII usernames.
- The app->editor direction: the installer/app records `editor_path` in
  `%appdata%\eud-agent\config.json` (UTF-8). The bridge does NOT read this.

## Install
`scripts/install_bridge.ps1` copies the slim `.lua` into `<editor>\Data\Lua\TriggerEditor\`
(no cfg generation beyond what the bridge needs). Idempotent.

## Edge cases
- Stale inbox/outbox from a previous run -> cleared at bridge init (and at app startup).
- Build in progress -> inbox processing skipped; `.cmd` left in place to apply after build;
  status.txt still reports compiling=true so the app extends its timeout.

## Implementation
- `bridge/ZZZ_10_agent_bridge.lua` — slimmed bridge (remove WebView2/spawn; keep IPC)
- `scripts/install_bridge.ps1` — drop-in copy
- (verify) `server/tests` analogues become Rust bridge_io round-trip tests (feature 11)
