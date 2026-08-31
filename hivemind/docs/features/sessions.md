# Feature: Concurrent multi-active sessions

eud-agent persists named conversations with one immutable `ProviderBinding` per session. Each
session owns its panel log, exact provider driver/conversation state, cancellation generation,
tool/ASK/preflight state, working workspace, and immutable event route. Commands within one
session are serialized; different sessions/providers may overlap read turns.

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
  "schemaVersion": 4,
  "sessions": [
    {
      "id": "<rust UUID>",
      "name": "유닛 HP 작업",
      "project": "mymap",
      "kind": "eps",
      "provider": "claude-code",
      "model": "sonnet",
      "createdAt": 1718000000,
      "lastConversationAt": 1718009999000
    }
  ]
}
```

`createdAt` is Unix seconds; `lastConversationAt` is Unix milliseconds. First request admission
copies the ready global default provider/model/reasoning into the record before worker creation.
Subsequent global changes never mutate the row. Rename, panel-log autosave, context usage,
activity, rewind, cancellation, and review do not change recency or provider. Schema-v3 and older
records migrate losslessly to a Codex binding using the legacy thread id and migrated Codex model.

Each session file contains the flattened metadata plus strict provider authority:

```json
{
  "providerBinding": {
    "provider": "claude-code",
    "model": "sonnet",
    "reasoning": { "level": "high" },
    "conversation": {
      "provider": "claude-code",
      "sessionId": "019ece1c-..."
    }
  },
  "pendingRequestIds": ["req-1a2b3c4d"],
  "contextUsage": {
    "last": { "inputTokens": 31000, "totalTokens": 32200 },
    "total": { "inputTokens": 52000, "totalTokens": 54100 },
    "modelContextWindow": 128000
  },
  "panelLog": { "schemaVersion": 2, "logSeq": 4, "log": [] },
  "contextState": {
    "schemaVersion": 2,
    "instructionEpoch": 3,
    "staticPromptFingerprint": "<sha256>",
    "delivered": {
      "provider": "claude-code",
      "conversationKey": "019ece1c-...",
      "epoch": 3,
      "memorySha256": "<sha256>",
      "wikiSha256": "<sha256>",
      "taskRevision": 8
    }
  },
  "taskState": {
    "schemaVersion": 1,
    "events": [],
    "leafId": null,
    "projection": {
      "revision": 0,
      "topic": null,
      "goals": [],
      "targetSets": [],
      "constraints": [],
      "decisions": [],
      "authoritativeArtifacts": [],
      "blockers": [],
      "acceptanceCriteria": []
    }
  }
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

`contextState` persists the instruction epoch, static-baseline fingerprint, and only the last
successfully delivered model cursor. `taskState.events` is append-only; `leafId` selects the
current branch and `projection` is a checksum-verified cache rebuilt from that branch on load.
Both top-level fields use `serde(default)`. Existing schema-v3 records without them retain their
name, thread, pending review, usage, and panel log and load with empty state; there is no new global
schema cutover.

Attachment bytes remain under `%localappdata%\eud-agent\attachments\objects\` and are bound to
the session on send.

## Panel log

The panel persists only conversation history:

```json
{
  "schemaVersion": 2,
  "logSeq": 4,
  "log": [
    { "id": 1, "kind": "you", "text": "이 화면을 확인해 줘", "clientTurnId": "6aa5d80d-...", "attachments": [], "mentions": [] },
    { "id": 2, "kind": "agent", "text": "..." },
    { "id": 3, "kind": "info", "text": "도구 호출 2건", "tools": [] },
    { "id": 4, "kind": "ok", "text": "적용 유지" }
  ]
}
```

Transient turn, plan, changeset, activity, wiki, and connection state is not persisted in
`panelLog`. Every new `you` row gets a UUID `clientTurnId` before it is appended; `chat` or
`plan_feedback` sends the same value, and a transport retry reuses it. Editing and resending creates
a new id for the new branch. Hydrated legacy rows may omit the field.

`conversation_rewind` replaces the log with the selected prefix, clears the thread id, pending
request ids, context usage, and context delivery cursor, and moves the task-event leaf to the final
retained `clientTurnId` before staging a condensed replay. Abandoned task events remain durable.
A legacy prefix without an id does not guess from text or sequence; it selects an empty task
projection. Rewind is rejected while the session is running, waiting for write, or has a
recoverable pending review. If every pending marker instead names a missing or empty journal,
explicit rewind clears the unrecoverable markers and repairs the session so conversation can
resume.

`PanelLogEntry.mentions` is an optional additive schema-v2 field containing ordered generic
`MentionInstance` records. The panel preserves exact backend-created snapshots through autosave,
hydrate, session reload, historical rendering, edit, and rewind without resolving them. Edited
historical mentions are revalidated on resend and may fail stale. Each selected session owns only
its own durable mention history; unsent chips are scoped to the selected session and become
invalid when the active EUD project changes. A rejected chat or plan-feedback invoke restores the
complete unsent text, attachments, and mentions.

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

Plan review uses `review` presentation before approval. Code changeset review keeps only that
session's write registration and journal; it does not reserve a project-wide execution lane.
Partial decisions and failed rollback remain `review`.

Post-acceptance harness jobs are a separate durable state machine:

```text
waiting_runtime -> pending -> running -> review -> completed
       \-> skipped             \-> failed -> pending (manual retry)
review -> rejected
```

`skipped` is terminal and means the user cancelled runtime verification and all harness
generation while keeping accepted code.

Harness state never changes `session_activity`, never occupies the conversation worker mutex, and
never disables chat. Each attempt owns a fresh tools-disabled driver created from the job's
provider/model/reasoning snapshot and a dedicated document workspace.

## Session workers

`SessionEngineManager` lazily owns `HashMap<SessionId, Arc<SessionWorker>>`. Each worker contains:

- a session-bound `AgentEngine` behind its own Tokio mutex;
- one exact `ProductionProviderDriver` enum variant from the persisted binding;
- a session-bound `SessionToolRuntime` and, for CLI providers, loopback MCP server;
- a per-worker cancellation watch channel;
- `SessionEventSink(app, sessionId)` and immutable provider id for logout busy protection.

The worker mutex is the same-session command sequencer. There is no global `ManagedAgentEngine`
mutex and no mutable session-switching path. `session_open` hydrates only the named worker and is
idempotent. Selecting a sidebar row never calls it.

Resume seeds the binding's typed conversation state. A mismatch fails load; a provider-supported
resume failure resets only that provider conversation and replays a bounded condensed transcript.

Global provider/model settings are owned by `ProviderService` with per-provider locks. They are
new-session defaults only and are never read by an existing worker or harness retry.

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

`mention_search` is a separate read-only main-panel composer command. It searches only the current
saved-project authority, returns opaque snapshots, has no model/MCP exposure, and does not mutate
or select a session. The selected session id is required only when the resulting instances are
sent through `chat` or `plan_feedback`.

`chat` and `plan_feedback` also require the panel-generated `clientTurnId`. The backend validates
it as a UUID and carries it unchanged into task-state lifecycle events.

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
| `harness_jobs` | list/recover durable jobs for one session |
| `harness_runtime_confirm` | release a runtime-sensitive job after user verification |
| `harness_skip` | terminate a runtime-waiting job without durable harness updates |
| `harness_dismiss` | durably hide a terminal job card without deleting its audit record |
| `harness_retry` | retry one failed job with one new model attempt |
| `harness_decision` | accept/reject an atomic harness document changeset |

`session_list`, `session_load`, `session_create`, `session_update_log`, `session_rename`, and
`session_delete` do not require write registration. `memory_save` and `wiki_save` use short
project transactions.

Every conversation event has a required immutable `sessionId`:

- `agent_event`, `context_usage`, `answer`, `plan`, `changeset`, `rollback_result`;
- turn `progress` and turn `error`;
- `session_activity`.

`harness_job` is separately session-scoped and carries durable job status, attempt count,
runtime-verification state, optional failure/summary, optional memory file names, and the
secondary document changeset while under review. Project status, list, memory/wiki snapshots,
setup, bootstrap, and RAG warmup remain global. `session_active` and selected-row routing
fallbacks do not exist.

## Map Agent session history

The Map Agent window lists only `SessionKind::Map` rows for the current project and saved
`OpenMapName` source. `map_agent_session_list`, `map_agent_session_create`,
`map_agent_session_load`, `map_agent_session_rename`, and `map_agent_session_delete` keep this
surface separate from the main EPS sidebar. Loading a row recreates that session's exact provider
worker, candidate revision chain, selections, context usage, and panel conversation.

The history dialog is latest-conversation-first, searchable, and identifies the active row. It
supports creating, renaming, and deleting inactive map work. Switching is disabled while the
visible map session is running; the panel flushes its current conversation log before create/load
and clears draft mentions, prompt text, live stream state, and canvas selection when the session
changes. Window focus/source refresh first reloads the selected session id, so selecting an older
history row does not silently jump back to the newest row. Backend source checks reject rows bound
to another project or saved map.

Candidate state creation and reopening are explicit backend operations. A newly created or
persisted-but-unbound Map session calls `CandidateStore::create_session`; loading, bootstrapping,
or focus-reloading a source-bound row calls `CandidateStore::open_session`. Normal hydration
validates source identity, repairs the visible candidate by replay when necessary, and refreshes
stale-source state without inspecting or sweeping `drafts/`, so an active request keeps its exact
draft path and bytes across reloads. `CandidateStore::cleanup_startup` is the only generic orphan
draft sweep and runs before `MapAgentService` is managed; request finish/cancel and successful
settlement continue to remove only the owning request's draft.

Map image attachments remain session-bound in LocalAppData but each active request receives a new
ordered `image-1..N` map in its `SessionToolRuntime`. The binding includes attachment SHA-256,
decoded source dimensions, candidate revision key, and baseline hash; only the safe ref/name/mime/
dimensions list is shown to the model beside its normal `localImage` inputs. Ending/resetting the
request drops the ref map, so another request or session cannot reuse an `imageRef`. Candidate
replay uses the manifest's stored `TerrainBlit` and image conversion metadata, never the attachment
or cache. Direct image placement keeps one normalized image cache entry per session and releases it
on successful confirm, session/source replacement, or UI cancellation.

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

Each `SessionSlot` also keeps `harnessJobs`. `HarnessStatusCard` shows explicit runtime waiting,
skip, background generation, retryable failure, atomic document review, and the latest terminal
result. Failed/completed/rejected/skipped cards expose a labelled close control. The backend
persists `dismissed`; the panel retains that marker so closing the newest terminal result does not
surface an older one after an event or restart. The main PromptInput remains enabled.
`harness_jobs` snapshot hydration merges by `updatedAt`, so a slower snapshot cannot overwrite a
newer push event.

The left sidebar remains 220–420 px, collapses to a 56 px rail, and ellipsizes long names with a
title. The center and both sidebars keep `min-width: 0`/horizontal clipping so the configured
960 px minimum surface has no horizontal overflow.

## Verification

- Rust barrier tests: different-session overlap, same-session serialization, concurrent write
  registration, and per-project operation serialization.
- Harness tests: runtime/static classification, skip-without-generation, structured delta
  validation, deterministic worklog staging, durable interrupted-job recovery, schema-v3 reset,
  and foreground completion without document repair turns.
- Active-state tests cover append/reload/replay equivalence, projection cache repair, 10-member
  target sets, authority/provenance rejection, anchored rewind with retained abandoned events,
  detached promotion audit, and legacy no-anchor fail-closed behavior.
- Panel tests cover chat/plan-feedback anchors, retry id reuse, new edit-branch ids, hydration,
  legacy rows, and alignment after the 500-entry log cap.
- Panel integration: overlapping conversations, immutable `harness_job` routing, runtime
  confirmation, skip, retry, terminal dismissal, atomic document review, and input availability
  during background work.
- Session recency: project/status/context/harness fan-out leaves every idle timestamp unchanged;
  a new `chat` or `plan_feedback` advances only its session.
- Browser mock-Tauri smoke: a completed/failed terminal card closes durably while the chat input
  stays enabled and horizontal overflow remains zero at 1280 px.
