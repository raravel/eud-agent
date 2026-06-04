# eud-agent Verification

All commands run from the repo root on Windows (PowerShell 7). Stages lint/test/smoke/panel are headless (the orchestrator runs them to confirm task completion). Stage e2e needs the editor GUI and is user-assisted.

## Stage: lint

```
server\.venv\Scripts\python.exe -m ruff check server
```

Proves: no syntax/static errors or banned patterns in server code.

## Stage: test

```
server\.venv\Scripts\python.exe -m pytest server/tests -q
```

Proves: unit + integration behavior of config, bridge_io (tmp-dir fake bridge), codex_client (mock subprocess), rag (stubbed model), orchestrator state machine, WS protocol incl. token/Origin rejection, bridge/panel/scripts static contracts — all without the editor or real codex.

## Stage: smoke

```
server\.venv\Scripts\python.exe -m eud_agent --selfcheck
```

Proves: config resolution (agent.cfg schema), codex shim resolution via shutil.which, RAG DB path exists and opens read-only, bge-m3 weights present in HF cache, built panel present (`panel/dist/index.html`). Exits non-zero with a specific message per missing prerequisite. Must NOT load the embedding model (fast).

## Stage: panel

```
npm --prefix panel run build
```

Proves: the React panel typechecks (tsc via the build) and bundles to `panel/dist/` with zero runtime CDN references. Requires `npm --prefix panel install` once per machine (node v24, npm). `panel/dist/` is gitignored — built locally during development; release-packaged later (Decision 04).

## Stage: e2e (manual, Windows, editor v0.19.6.0 — user-assisted)

Run `scripts\install_dropin.ps1`, build the panel (`npm --prefix panel run build`), start the editor, then walk this checklist:

1. Boot handshake: bridge spawns server (no console window), `server.ready` appears, WebView2 panel shows the UI (token accepted).
2. `PING`/`STATUS`/`LIST` round-trip via inbox; LIST shows paths + file types (confirm the `f.Filetype` member yields enum NAMES — EUD-011 deferred check).
3. Instruct flow: natural-language request produces progress (rag, codex), then code preview with diff (for SET target) and advisory diagnostics.
4. Apply SET on an open CUI file: content and open-tab editor update together; Korean text round-trips intact.
5. Apply NEWEPS: file created at root, tab opens; duplicate name returns ERROR shown in panel; Korean body intact (EUD-012 deferred check); memory-only until user saves.
6. Regression (v6 features): GET/DUMP, GETDAT/SETDAT on units, LUA command, BUILD guard.
7. Re-arm: create/switch project — panel window is recreated automatically; server survives.
8. Busy editor: trigger a build, send apply — panel shows waiting_build, apply lands after build.
9. Stale-ready recovery: kill the server process, wait — bridge respawns it (or next editor start cleans the stale ready and respawns); kill the editor — server self-terminates within ~60s (heartbeat).
10. UDF check: WebView2 profile data lands under `Data\agent\webview2`, not next to the exe.
