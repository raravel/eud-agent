# Feature: Concurrent multi-active sessions

eud-agent persists named Codex conversations and runs independent sessions concurrently. Each
session owns its panel store, log, Codex thread/client, cancellation generation, MCP endpoint,
request/preflight state, working workspace, and immutable event route. Commands within one
session are serialized; different sessions may overlap read-only turns.

Only project write transactions are serialized. The FIFO lease begins at declared write intent
and remains owned through mutation, build, changeset review, and complete accept/reject rollback.
Review blocks later writers, not read-only conversations.

## Durable session records

Rust owns `%appdata%\eud-agent\sessions\`:

```text
sessions/
├── index.json
└── <session-id>.json
```

All writes use `memory::write_atomic_bytes` and UTF-8 without BOM. `index.json` contains
most-recently-updated `SessionMeta` rows:

```json
{
  "schemaVersion": 1,
  "sessions": [
    {
      "id": "<rust UUID>",
      "name": "유닛 HP 작업",
      "project": "mymap",
      "createdAt": 1718000000,
      "updatedAt": 1718009999
    }
  ]
}
```

Each session file contains the flattened metadata plus:

```json
{
  "threadId": "019ece1c-...",
  "pendingRequestIds": ["req-1a2b3c4d"],
  "panelLog": { "schemaVersion": 2, "logSeq": 4, "log": [] }
}
```

`threadId` is null before the first Codex thread starts. `pendingRequestIds` names unarchived
journals; recovery requires zero or one project writer. Startup removes stale ids only when the
matching journal is already in the accepted archive. A missing live journal without that archive
and multiple pending writers remain explicit errors. `panelLog` is opaque to Rust.

Attachment bytes remain under `%localappdata%\eud-agent\attachments\objects\` and are bound to
the session on send.

## Panel log

The panel persists only conversation history:

```json
{
  "schemaVersion": 2,
  "logSeq": 4,
  "log": [
    { "id": 1, "kind": "you", "text": "이 화면을 확인해 줘", "attachments": [] },
    { "id": 2, "kind": "agent", "text": "..." },
    { "id": 3, "kind": "info", "text": "도구 호출 2건", "tools": [] },
    { "id": 4, "kind": "ok", "text": "적용 유지" }
  ]
}
```

Transient turn, plan, changeset, activity, wiki, and connection state is not persisted.
`conversation_rewind` replaces the log with the selected prefix, clears the thread id and pending
request ids, and stages a condensed replay for the next fresh thread. Rewind is rejected while
the session is running, waiting for write, or in review.

## Backend activity

The backend is authoritative for each session:

```text
idle
running_read
waiting_write(queuePosition)
running_write
review
error
```

`queuePosition` means only position in the project write queue. A waiting activity also carries
`blockingSessionId`, the current project write owner. Neither field describes conversation order.

Typical transitions:

```text
idle -> running_read -> idle
running_read -> waiting_write -> running_write
running_write -> review -> idle
running_write -> idle
```

Plan review uses `review` presentation but does not own the write lease until approval. A
changeset review owns the lease. Partial decisions and failed rollback remain `review`.

## Session workers

`SessionEngineManager` lazily owns `HashMap<SessionId, Arc<SessionWorker>>`. Each worker contains:

- a session-bound `AgentEngine` behind its own Tokio mutex;
- `ProductionCodexDriver` with its own app-server client and event receiver;
- a session-bound loopback MCP server and `SessionToolRuntime`;
- a per-worker cancellation watch channel;
- `SessionEventSink(app, sessionId)`.

The worker mutex is the same-session command sequencer. There is no global `ManagedAgentEngine`
mutex and no mutable session-switching path. `session_open` hydrates only the named worker and is
idempotent. Selecting a sidebar row never calls it.

Resume seeds the saved thread id. A seed error or bounded first-resume failure resets only that
worker and starts a fresh thread with a condensed transcript plus the full safety prompt.

Global model settings use a separate settings lock and temporary app-server; they are not stored
inside any conversation worker.

## Project write coordinator

`ProjectWriteCoordinator` provides:

```rust
request(project_id, session_id, request_id) -> WriteTicket
cancel(request_id)
release(request_id)
restore_review(project_id, session_id, request_id)
owner(project_id) -> Option<WriteOwner>
```

One project has at most one owner. Tickets are ordered when `request_write_lane` or
`plan_approve` registers intent. Waiting is process state, not a sleeping MCP request. A granted
ticket automatically resumes the same thread in write mode.

The coordinator releases only after:

- no live journal entry remains;
- every workspace promotion or rollback completed;
- build and changeset decision processing settled.

Cancellation removes only a waiting ticket or interrupts only that worker. Journaled active
writes become review and retain ownership.

If a read turn fails after declaring write intent but before write continuation, the manager
cancels a waiting ticket or releases an unmutated granted ticket before allowing another request.
`SessionToolRuntime::begin_request` refuses to overwrite any active ticket. This prevents a
single session from queueing behind its own stale owner as `쓰기 대기 1`.

At startup, current-project session records are scanned before admitting any writer. A valid
pending journal is restored as owner before new tickets. Read turns and unfinished tickets are
not persisted; journaled writes are.

## Session tool/runtime isolation

`ToolServices` shares the journal store, RAG, map rails, analyzer, data dirs, and coordinator.
Every `SessionToolRuntime` separately owns the live request id, evidence/mutation/action/search/
build state, pending plan, epScript preflight state, write ticket, and tool execution lock.

One ephemeral MCP endpoint is created per worker. No global current-request pointer infers the
caller. Mutating tools and `build_run` require that runtime's exact `(project, session, request)`
ownership.

## Workspace isolation

Canonical accepted files remain panel-visible at:

`workspaces/<project-id>/`

Codex uses:

`workspaces/.sessions/<project-id>/<session-id>/`

Before every read turn, canonical documents are delta-synced and a coherent session source
snapshot is refreshed. Read mode makes the whole root read-only. Before a granted write
continuation, the sync and snapshot run again, Codex is told to re-read targets, and a trusted
baseline is captured.

Workspace accept promotes selected bytes to canonical storage under the lease. Promotion and
trusted metadata update roll back together on failure. Reject restores/discards only the session
copy. Approved plan snapshots are app-owned canonical files, synced before execution, immutable,
and preserved after implementation rejection.

## Tauri IPC

All conversation commands include `sessionId`:

| command | purpose |
|---|---|
| `chat` | start/resume a read turn immediately |
| `plan_feedback` | revise that session's plan in read mode |
| `plan_approve` | register write intent and execute after grant |
| `changeset_decision` | accept/reject that session's journal |
| `cancel` | interrupt that turn or remove that write ticket |
| `conversation_rewind` | reset that idle session to a log prefix |
| `session_open` | hydrate/reconnect one persisted worker |

`session_list`, `session_load`, `session_create`, `session_update_log`, `session_rename`, and
`session_delete` remain outside the project write queue. `memory_save` and `wiki_save` use short
coordinated project writes.

Every conversation event has a required immutable `sessionId`:

- `agent_event`, `answer`, `plan`, `changeset`, `rollback_result`;
- turn `progress` and turn `error`;
- `session_activity`.

Project status, list, memory/wiki snapshots, setup, bootstrap, and RAG warmup remain global.
`session_active` and selected-row event-routing fallbacks do not exist.

## Panel contract

`App.tsx` owns `Map<sessionId, SessionSlot>`, with one `PanelStore` per row. Drafts are persisted
before their first chat, then invoked immediately. There is no frontend conversation queue,
running slot, review owner, or event-owner fallback.

`session_activity` drives explicit ownership labels:

- `running_read` -> `분석 중`;
- `waiting_write` -> `쓰기 대기 N · <blocker> 검토 결정/변경 완료 대기 · 경과 초`;
- `running_write` -> `변경 중 · 쓰기 권한 획득`;
- `review` -> `검토 필요`.

The transition into `running_write` also appends
`쓰기 권한을 획득했습니다. 변경을 시작합니다.` to that session log. A user can therefore
distinguish a legitimate review wait, elapsed waiting, and an actual granted write turn.

Two rows may show activity simultaneously. Review in one row does not disable a ready row.
Selection only changes the rendered store. Delete/rewind remains disabled for running, waiting,
and review rows. Waiting cancellation invokes backend `cancel`.

The left sidebar remains 220–420 px, collapses to a 56 px rail, and ellipsizes long names with a
title. The center and both sidebars keep `min-width: 0`/horizontal clipping so the configured
960 px minimum surface has no horizontal overflow.

## Verification

- Rust barrier tests: different-session overlap and same-session serialization.
- Runtime tests: request/evidence/budget/preflight isolation.
- Coordinator tests: FIFO, one owner, waiting cancellation, queue-position updates, review
  retention, and restored-review priority.
- Workspace tests: stable source snapshots, promotion, rejection invariance, approved plans.
- Panel integration: overlapping `chat` invokes, simultaneous activities, immutable event
  routing, and selection independence.
- Browser mock-Tauri smoke: concurrent read, waiting write, active write, review, and 1280/960 px
  horizontal-overflow checks.
