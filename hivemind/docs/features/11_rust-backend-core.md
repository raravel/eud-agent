# Rust backend core

## Scope

The Rust backend is the complete product policy/runtime layer:

- typed Tauri commands/events;
- sessions and Codex orchestration;
- MCP schemas/admission/dispatch;
- native project and build frontend;
- RAG and epScript preflight;
- semantic journal/review/rollback;
- map/image/audio tooling;
- memory/wiki/workspaces;
- E3S compatibility.

No Python server or Editor runtime path remains.

## Service graph

`ToolServices` is cloneable shared state. Each `SessionToolRuntime` owns request-local evidence, action budget, pending plan, audio/image bindings, source snapshot, and write ticket.

`NativeProjectManager` is the only project facade used by IPC, engine context, tools, preflight, and map-context resolution. It opens canonical state from configured `project_path` and holds a project-scoped build marker around euddraft.

## Tool execution

1. Registry lookup and closed schema validation.
2. Request/session ownership check.
3. Evidence/first-principles admission.
4. Read or write classification.
5. Project write registration when needed.
6. Native operation.
7. Semantic journal append and changeset emission.

Correctable tool errors return MCP tool errors so Codex can fix its request. Protocol/infrastructure failures remain distinct.

## Native rollback target

`SessionToolRuntime` implements the journal inverse target directly against `NativeProject` and MapSafe. Inverses cover DAT reset/set, source CRUD/move/MainFile, settings/plugins, workspace files, locations/player setup/map backups, and sound operations.

The journal abstraction name is historical internal terminology only; it does not imply a transport. No inverse sends a bridge command.

## Build path

`build_run` resolves configured euddraft, generates deterministic artifacts, runs the process, requires fresh output, and maps diagnostics. A build marker supplies MapSafe's compiling guard.

## Compatibility path

`nrbf` parses/writes supported MS-NRBF records. `e3s_nrbf` owns semantic projection and native payload preservation. Product code never calls `BinaryFormatter` or loads Editor assemblies.

## IPC state

`ipc::AppManaged` contains `DataDirs`. `status`/`list` are native project probes. Memory/wiki/workspace commands derive project identity from native status.

## Verification

The full Rust suite covers tool schemas, session concurrency, project persistence, journal inverses, MapSafe rails, generators, NRBF, setup, and diagnostics. Environment-backed ignored tests cover real E3S and real euddraft only.
