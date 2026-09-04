# Tauri shell and bootstrap

## Shell

The Tauri 2 application hosts the built React panel and registers typed native commands/events. Managed state is `ipc::AppManaged`, which owns resolved data directories only; each project operation reloads `config.json`.

## Data directories

- `%APPDATA%/eud-agent`: config, sessions, memory, journals, accepted workspaces.
- `%LOCALAPPDATA%/eud-agent`: models, RAG index, logs, analyzer mirrors, audio tools, native compatibility assets.
- selected project root: canonical manifest/sources/DAT/maps/build.

All app-written text/JSON is UTF-8 without BOM.

## config.json

Relevant configured paths:

```json
{
  "project_path": "C:\\Projects\\my-map",
  "euddraft_path": "C:\\Tools\\euddraft.exe",
  "starcraft_path": "C:\\Program Files (x86)\\StarCraft"
}
```

Model/RAG/Codex settings retain their versioned/checksummed fields. There is no Editor path.

## First-run flow

1. Native project
   - open a folder containing valid `project.json`;
   - create from a selected SCX/SCM into an empty destination;
   - import a selected E3S and referenced map into an empty destination.
2. euddraft
   - select `euddraft.exe` or `euddraft.py`;
   - validate through `EuddraftLaunch::resolve`.
3. Assets
   - verify model, RAG index, native compatibility assets, and managed media tools;
   - download missing checksummed assets atomically.
4. Codex
   - resolve/install CLI;
   - OAuth or API-key login.

`setup_status` returns project/euddraft/assets/Codex booleans and one stable optional error. Setup completion arms native project refresh and model settings loading.

## Project commands

- `setup_pick_project_path`
- `setup_create_project`
- `setup_import_e3s`
- `setup_pick_euddraft_path`
- `project_export_e3s`
- `bootstrap_run`

Create/import destinations must be empty. Cancellation returns the unchanged setup snapshot. A failed action does not replace the configured valid project.

## Resource sync

Tauri resource `native/eud-editor-compat` is copied to LocalAppData on startup. It contains data-format metadata only. EUD Editor binaries, DLLs, Lua, and install scripts are not resources.

## Verification

- setup status/config tests;
- create-from-map copy/open and nonempty-destination refusal;
- protocol/type-guard tests for every setup command;
- SetupScreen busy/action/accessibility tests;
- production Tauri/browser interaction acceptance.
