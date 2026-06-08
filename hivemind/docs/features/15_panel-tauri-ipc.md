# Feature 15: Panel transport migration (WebSocket -> Tauri IPC, v2 chat schema)

Swap the panel's transport from the localhost WebSocket client to Tauri IPC (`invoke` +
event listeners). The PANEL's v2 chat protocol is preserved 1:1 — only the wire changes.
Rendering (Monaco, diff tab, AI Elements/Streamdown, plan/changeset views) is untouched.

> Decision: see [[decisions/13_ipc-v2-chat-contract]] (supersedes [[decisions/11_panel-tauri-ipc]]).

## Source of truth
The panel already speaks the v2 chat protocol (features 05/06: chat-first, plan review,
changeset accept/reject). That schema is the contract; the Rust backend (feature 11) is
rebuilt to match it. There is NO instruct/apply/code/applied surface — those v1 messages
were removed in the panel and must not reappear.

## Transport mapping (panel v2 chat schema, 1:1 with the old WS messages)
Commands (panel -> core, `invoke`):
| v2 WS message (client->server) | New Tauri command |
|---|---|
| `chat {text}` | `invoke("chat", { text })` |
| `plan_feedback {text}` | `invoke("plan_feedback", { text })` |
| `plan_approve {}` | `invoke("plan_approve")` |
| `changeset_decision {decision, ids}` | `invoke("changeset_decision", { decision, ids })` |
| `cancel {}` | `invoke("cancel")` |
| `reset {}` | `invoke("reset")` |
| `status {}` | `invoke("status")` |
| `list {}` | `invoke("list")` |

Events (core -> panel, `listen`):
| v2 WS message (server->client) | New Tauri event |
|---|---|
| `agent_event {kind, detail, data?}` | `listen("agent_event", ...)` |
| `answer {text}` | `listen("answer", ...)` |
| `plan {markdown, revision}` | `listen("plan", ...)` |
| `changeset {request_id, items}` | `listen("changeset", ...)` |
| `rollback_result {ids, ok}` | `listen("rollback_result", ...)` |
| `progress {stage, detail?}` | `listen("progress", ...)` |
| `error {message}` | `listen("error", ...)` |
| `status {compiling, project}` | `listen("status", ...)` (push) or command return |
| `list {files?, error?}` | command return value of `invoke("list")` |

`status`/`list` are request/response commands (return the payload from `invoke`); the
remaining server messages are push events delivered via `listen`. `chat`/`plan_feedback`/
`plan_approve`/`changeset_decision`/`cancel`/`reset` start background work and resolve when
accepted — the turn result arrives later as events.

## Removed from the panel
- WebSocket connect/reconnect logic, `?token=` handshake, Origin assumptions, and the
  `server.ready`/port discovery. The single in-process app has no socket.
- `panel/src/ws/client.ts` (+ test). The shared protocol TYPES in `panel/src/ws/protocol.ts`
  (ClientMessage/ServerMessage discriminated unions, type guards, and the shared shapes
  `FileEntry`/`ChangesetItem`/`Diagnostic`/`ProgressStage`) MOVE into the new IPC module
  (`panel/src/lib/ipc.ts`) or a sibling types module so the store and components keep their
  imports. After the move, `panel/src/ws/` is deleted.

## Connection lifecycle (no socket)
There is no reconnect loop. The store's connection-lifecycle hooks
(`wsConnecting`/`wsOpen`/`wsError`) are retained as transport-neutral phase drivers but are
now driven by IPC readiness, not a socket: the client marks connected once the Tauri event
listeners are registered and an initial `status`+`list` resolve. "editor not connected" is a
backend-reported state (feature 11: stale/absent bridge heartbeat) surfaced via an `error`
event / status, NOT a transport disconnect. Editor-connection UI lands in EUD-120.

## Streaming
Agent reasoning/answer deltas arrive as `agent_event` events; the existing AI Elements +
Streamdown pipeline renders them (reasoning dim/collapsible, answer prominent). NEVER render
raw `agent_event` kind identifiers as user-facing text.

## Monaco / diff (unchanged constraints)
- Monaco loads from the `monaco-editor` npm bundle via `loader.config({ monaco })` — no CDN.
- The diff tab / changeset modified-file rows render the Rust-supplied unified diff with +/-
  coloring — not Monaco DiffEditor.

## Edge cases
- Backend not ready (RAG warming) -> commands still accepted; `rag_warmup` progress shown.
- Event listener cleanup on unmount to avoid duplicate handlers.
- One in-flight `changeset_decision` at a time (matches backend EUD-070 background-decision).

## Implementation
- `panel/src/lib/ipc.ts` — new Tauri IPC client (replaces `ws/client.ts`); maps the v2
  chat commands to `invoke` and the v2 server messages to `listen`; re-exports / re-homes
  the shared protocol types previously in `ws/protocol.ts`.
- `panel/src/state/*` — store wired to invoke/events instead of WS; lifecycle hooks driven
  by IPC readiness.
- `panel/src/App.tsx` — swap `WsClient` for the IPC client; remove WS lifecycle wiring.
- removed: `panel/src/ws/client.ts`, `panel/src/ws/protocol.ts` (+ tests) once types move.
- external: `@tauri-apps/api` ^2 (core `invoke` + `event`).
