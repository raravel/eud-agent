# Feature 15: Panel transport migration (WebSocket -> Tauri IPC)

Swap the panel's transport from the localhost WebSocket client to Tauri IPC (`invoke` +
event listeners). Rendering (Monaco, diff tab, AI Elements/Streamdown) is untouched — only
the wire changes.

> Decision: see [[decisions/11_panel-tauri-ipc]].

## Transport mapping (1:1 with the old WS schema)
| Old WS (client->server) | New Tauri command |
|---|---|
| `instruct {...}` | `invoke("instruct", {...})` |
| `apply {...}` | `invoke("apply", {...})` |
| `status {}` | `invoke("status")` |
| `list {}` | `invoke("list")` |

| Old WS (server->client) | New Tauri event |
|---|---|
| `progress {...}` | `listen("progress", ...)` |
| `code {...}` | `listen("code", ...)` |
| `agent_event {...}` | `listen("agent_event", ...)` |
| `applied` / `error` | `listen("applied"/"error", ...)` |

## Removed from the panel
- WebSocket connect/reconnect logic, `?token=` handshake, Origin assumptions, and the
  `server.ready`/port discovery. The single in-process app has no socket.

## Streaming
Agent reasoning/answer deltas arrive as `agent_event` events; the existing AI Elements +
Streamdown pipeline renders them (reasoning dim/collapsible, answer prominent). NEVER render
raw `agent_event` kind identifiers as user-facing text.

## Monaco / diff (unchanged constraints)
- Monaco loads from the `monaco-editor` npm bundle via `loader.config({ monaco })` — no CDN.
- The diff tab renders the Rust-supplied unified diff (`code.diff`) with +/- coloring — not
  Monaco DiffEditor.

## Setup/connection states
- First-run setup + download progress (feature 10) rendered as a panel screen driven by
  `progress {stage: bootstrap}`.
- "editor not connected" state when the bridge heartbeat is stale/absent; instruct/apply
  disabled with a hint.

## Edge cases
- Backend not ready (RAG warming) -> commands still accepted; `rag_warmup` progress shown.
- Event listener cleanup on unmount to avoid duplicate handlers.

## Implementation
- `panel/src/lib/ipc.ts` — new Tauri IPC client (replaces `ws-client`)
- `panel/src/state/*` — wire store to invoke/events instead of WS
- `panel/src/components/*` — setup screen + connection-state UI (PlanView etc. unchanged)
- external: `@tauri-apps/api` (core invoke + event)
