# Feature: Concurrent multi-active sessions

eud-agent persists named Codex conversations and runs independent sessions concurrently. Each
session owns its panel store, log, Codex thread/client, cancellation generation, MCP endpoint,
request/preflight state, working workspace, and immutable event route. Commands within one
session are serialized; different sessions may overlap read and write turns.

Write intent creates a concurrent session registration immediately. Only operations that touch
shared editor/map/memory/build state and canonical workspace acceptance enter a short per-project
transaction. A changeset under review does not block another session from editing or building.

## Durable session records

Rust owns `%appdata%\eud-agent\sessions\`:

```text
sessions/
├── index.json
└── <session-id>.json
```

All writes use `memory::write_atomic_bytes` and UTF-8 without BOM. `index.json` contains
`SessionMeta` rows sorted by `lastConversationAt` descending:

```json
{
  "schemaVersion": 2,
  "sessions": [
    {
      "id": "<rust UUID>",
      "name": "유닛 HP 작업",
      "project": "mymap",
      "createdAt": 1718000000,
      "lastConversationAt": 1718009999000
    }
  ]
}
```

`createdAt` is Unix seconds; `lastConversationAt` is Unix milliseconds. Creating the first
conversation and submitting a later `chat` or `plan_feedback` advances
`lastConversationAt` past every indexed row before the turn runs. Rename, panel-log autosave,
context usage, activity transitions, rewind, cancellation, and changeset decisions do not change
conversation recency. Schema-v1 `updatedAt` seconds remain readable and migrate to
`lastConversationAt` milliseconds on the next save.

Each session file contains the flattened metadata plus:

```json
{
  "threadId": "019ece1c-...",
  "pendingRequestIds": ["req-1a2b3c4d"],
  "contextUsage": {
    "last": {
      "inputTokens": 31000,
      "cachedInputTokens": 24000,
      "cacheWriteInputTokens": 0,
      "outputTokens": 1200,
      "reasoningOutputTokens": 800,
      "totalTokens": 32200
    },
    "total": {
      "inputTokens": 52000,
      "cachedInputTokens": 40000,
      "cacheWriteInputTokens": 600,
      "outputTokens": 2100,
      "reasoningOutputTokens": 1300,
      "totalTokens": 54100
    },
    "modelContextWindow": 128000
  },
  "panelLog": { "schemaVersion": 2, "logSeq": 4, "log": [] }
}
```

`pendingRequestIds` names unarchived journals; each session expects at most one pending review.
Startup removes stale ids only when the matching journal is already in the accepted archive.
A missing live journal without that archive remains an explicit error on its owning session, but
it never prevents `session_list`, healthy session hydration, or restoration of other valid pending
journals in the same project. `panelLog` is opaque to Rust.
`contextUsage` is absent until Codex emits `thread/tokenUsage/updated`; `last.totalTokens` is the
active context size, while `total` is cumulative for the thread. The latest snapshot is persisted
outside `panelLog`, so unopened rows retain usage across an app restart.

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

Transient turn, plan, changeset, activity, wiki, and connection state is not persisted in
`panelLog`. `conversation_rewind` replaces the log with the selected prefix, clears the thread
id, pending request ids, and context usage, and stages a condensed replay for the next fresh
thread. Rewind is rejected while the session is running, waiting for write, or has a recoverable
pending review. If every pending marker instead names a missing or empty journal, explicit rewind
clears the unrecoverable markers and repairs the session so conversation can resume.

## Backend activity

The backend is authoritative for each session:

```text
idle
running_read
running_write
review
error
```

Typical transitions:

```text
idle -> running_read -> idle
running_read -> running_write
running_write -> review -> idle
running_write -> idle
```

Plan review uses `review` presentation before approval. Changeset review keeps only that
session's write registration and journal; it does not reserve a project-wide execution lane.
Partial decisions and failed rollback remain `review`.

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

`ProjectWriteCoordinator` registers every active request independently:

```rust
request(project_id, session_id, request_id) -> WriteTicket
release(request_id)
restore_review(project_id, session_id, request_id)
owns(project_id, session_id, request_id) -> bool
transaction(project_id, operation)
```

`request` and `restore_review` grant immediately, including when another request for the same
project is writing or awaiting review. `transaction` is the only serialized boundary. Mutating
MCP calls, `build_run`, direct panel writes, and changeset decisions hold it only while their
shared-state operation settles.

Cancellation interrupts only that worker. An unmutated registration is released immediately;
journaled changes remain reviewable. Startup restores every valid pending journal registration,
so multiple sessions can recover review state for one project without blocking new writers.

## Session tool/runtime isolation

`ToolServices` shares the journal store, RAG, map rails, analyzer, data dirs, and coordinator.
Every `SessionToolRuntime` separately owns the live request id, evidence/mutation/action/search/
build state, pending plan, epScript preflight state, write registration, source baseline, and
tool execution lock.

One ephemeral MCP endpoint is created per worker. No global current-request pointer infers the
caller. Mutating tools require that runtime's exact `(project, session, request)` registration.
Each shared-state dispatch runs inside one short project transaction.

## Workspace isolation

Canonical accepted files remain panel-visible at:

`workspaces/<project-id>/`

Codex uses:

`workspaces/.sessions/<project-id>/<session-id>/`

Before every read turn, canonical documents are delta-synced and a coherent session source
snapshot is refreshed. Read mode makes the whole root read-only. Before write continuation, the
sync and snapshot run again, Codex is told to re-read targets, and a trusted baseline is captured.

Native session document changes remain isolated until review. Acceptance compares their journal
baseline with current canonical bytes: unchanged targets promote directly, non-overlapping line
changes merge automatically, and overlapping changes fail with `ConcurrentWriteConflict` while
leaving canonical bytes untouched.

Editor file tools use the session's coherent `source/` snapshot as their optimistic baseline.
`file_write` and `file_edit` three-way merge non-overlapping live changes; `file_edit` first
applies ordered exact replacements to the request's latest desired content and rejects missing or
ambiguous matches before mutation. Write/delete/rename/move reject stale overlapping targets.
Shared tool calls and builds are serialized only for the duration of that call. Reject
restores/discards only the session workspace copy. Approved plan snapshots remain app-owned,
immutable, and preserved after implementation rejection.

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
`session_delete` do not require write registration. `memory_save` and `wiki_save` use short
project transactions.

Every conversation event has a required immutable `sessionId`:

- `agent_event`, `context_usage`, `answer`, `plan`, `changeset`, `rollback_result`;
- turn `progress` and turn `error`;
- `session_activity`.

Project status, list, memory/wiki snapshots, setup, bootstrap, and RAG warmup remain global.
`session_active` and selected-row event-routing fallbacks do not exist.

## Panel contract

`App.tsx` owns `Map<sessionId, SessionSlot>`, with one `PanelStore` per row. Drafts are persisted
before their first chat, then invoked immediately. The sidebar sorts by `lastConversationAt`
descending and optimistically advances the submitted row before awaiting the backend, so a lower
row moves to the top as soon as its user message is sent. Session subscriptions autosave only when
the conversation-log array changes; project/status/context rerenders do not rewrite every log.
There is no frontend conversation queue, running slot, review owner, or event-owner fallback.
`context_usage` replaces only the addressed store's typed snapshot. The PromptInput footer uses
the AI Elements `Context` hover card: its trigger shows `last.totalTokens / modelContextWindow`,
and its body shows cumulative input, cached-input, output, and reasoning counts. A missing context
window suppresses the trigger rather than guessing. Cost is intentionally omitted because Codex
account billing is not equivalent to direct API model pricing.

`session_activity` drives:

- `running_read` -> `분석 중`;
- `running_write` -> `변경 중 · 격리 워크스페이스`;
- `review` -> `검토 필요`.

The transition into `running_write` appends
`격리 워크스페이스에서 변경을 시작합니다.` to that session log. Two rows may show activity
simultaneously, and review in one row does not disable another row. Selection only changes the
rendered store. Delete/rewind remains disabled for running and review rows.

The left sidebar remains 220–420 px, collapses to a 56 px rail, and ellipsizes long names with a
title. The center and both sidebars keep `min-width: 0`/horizontal clipping so the configured
960 px minimum surface has no horizontal overflow.

## Verification

- Rust barrier tests: different-session overlap, same-session serialization, concurrent write
  registration, and per-project operation serialization.
- Runtime tests: request/evidence/budget/preflight/source-baseline isolation.
- Workspace tests: independent snapshots, non-overlapping three-way merge, explicit overlapping
  conflict, rejection invariance, and approved plans.
- Panel integration: overlapping `chat` invokes, simultaneous write/review activities, immutable
  event routing, and selection independence.
- Session recency: project/status/context fan-out leaves every idle timestamp unchanged; a new
  `chat` or `plan_feedback` advances only its session and moves that row to the top immediately
  and after restart.
- Browser mock-Tauri smoke: concurrent read/write/review and 1280/960 px horizontal-overflow
  checks.
