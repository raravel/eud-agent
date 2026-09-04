# Agent core

## Runtime

The agent core is in-process Rust. Tauri commands feed `SessionEngineManager`; exactly five typed provider drivers (Codex, Claude Code, Antigravity, OpenCode Go, and Ollama) route tool calls through one `SessionToolRuntime` backed by `ToolServices`.

Turn flow:

```text
chat/feedback
  -> project + memory + RAG context
  -> immutable session provider/model binding
  -> provider stream and typed tool admission
  -> read/evidence/preflight
  -> native write registration
  -> semantic journal
  -> plan or changeset review
```

## Native project context

Every turn receives current native project identity, MainFile, source snapshot/revision, settings/plugins, map context, and relevant memory. `project_status` and `list_files` read `NativeProjectManager`; no raw status wire reply exists.

Project state changes invalidate stale source/mention/workspace authority. A missing project is normal gated state, not an IPC disconnect.

## Tool model

- Read tools: project/source/DAT/map/memory/workspace/preflight.
- Mutation tools: native source/settings/plugins/map operations, `dat_patch`, and `build_run`.
- `dat_patch` is the only model-facing DAT mutation boundary.
- Tool schemas are validated before execution, including `const` tags and exactly-one `oneOf` alternatives.
- Evidence requirements and first-principles rails run before write registration.
- Writes are journaled with semantic before/after snapshots.

## Source architecture

The manifest MainFile is the composition root regardless of filename. The active provider should:

- read MainFile and relevant imports before placement;
- keep lifecycle hooks in the composition root;
- place cohesive subsystem state/logic in focused modules;
- avoid circular imports and needless files;
- batch preflight mutually dependent candidates;
- build after accepted runtime-affecting changes.

## Sessions and review

- Session records persist conversation log, typed provider conversation, model/reasoning selection, pending requests, project, and kind.
- Existing sessions never switch provider; settings defaults apply only to new sessions.
- Read work may overlap; project writes serialize.
- Plan review and changeset review are explicit turn outcomes.
- Reject applies inverse operations in reverse sequence; accept archives durable state.
- Restart/reopen restores accepted canonical files and review metadata.

## Build and diagnostics

`build_run` invokes the native generator and euddraft runner. It returns `{ok, errors, stdout, stderr, outputMap}` with file/line diagnostics. Analyzer diagnostics are advisory; a fresh euddraft output is final success authority.

After a successful build, `trace_test_run` and `trace_suite_run` use the native source snapshot and generated EDS to run isolated runtime diagnostics. They never query an Editor project or bridge.

## Removed runtime

The following are not compatibility paths and must not return:

- Python server/orchestrator;
- WebSocket/localhost panel transport;
- BridgeIo or Lua command dispatcher;
- Editor status/list/get/set/build;
- Editor launch/bootstrap/heartbeat;
- individual DAT setter tools.
