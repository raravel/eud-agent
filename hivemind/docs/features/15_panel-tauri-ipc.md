# Panel ↔ Tauri IPC

## Transport

The panel uses Tauri 2 directly:

- panel → core: `invoke(command, camelCaseArgs)`
- core → panel: `listen(event, handler)`
- no WebSocket, HTTP server, origin/token handshake, or reconnect socket

`IpcClient.connect()` registers every push listener and then marks transport open. Native project validation is separate and performed by `refresh()`.

## Refresh

`refresh()` invokes native `status`, then invokes `list` only after recovery or a project identity change. It reports project availability through `onProjectAvailabilityChange` without changing transport state. Periodic repeats are edge-suppressed.

A missing/invalid project:

- keeps listeners active;
- keeps bootstrap/session push events flowing;
- sets `projectAvailable=false`;
- gates authoring and shows the native project notice;
- recovers on a later successful refresh.

## Setup/project commands

Setup response commands are normalized to the `setup` server message after runtime guarding:

- `setup_status`
- `setup_pick_project_path`
- `setup_create_project`
- `setup_import_e3s`
- `setup_pick_euddraft_path`

`project_export_e3s` is a direct typed helper returning `{path}` or `null` on dialog cancellation.

## Message ownership

Conversation events carry `sessionId`. The panel dispatches them only to the owning session store. Global bootstrap/setup/settings events remain unscoped. Listener registration precedes pending ASK synchronization.

## Runtime guards

`protocol.ts` defines closed discriminated unions and structural guards. Unknown/malformed payloads become diagnostic log entries and never crash rendering.

Setup snapshots require:

- `project_path`, `project_valid`
- `euddraft_path`, `euddraft_valid`
- `assets_ready`
- `codex_resolved`, `codex_authed`
- `setup_required`
- optional string `error`

## Verification

`ipc.test.ts` covers listener registration, request/response normalization, no-project vs genuine failure, native project disappearance/recovery, setup create/import dispatch, workspace commands, and malformed response rejection.
