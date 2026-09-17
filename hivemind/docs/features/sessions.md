# Feature: Concurrent multi-active sessions

eud-agent persists named conversations with one immutable `ProviderBinding` per session. Each
session owns its panel log, ProviderRuntime/conversation state, cancellation generation,
tool/ASK/build state, working workspace, and immutable event route. Commands within one
session are serialized; different sessions/providers may overlap read turns.

Write intent creates a concurrent session registration immediately. Only operations that touch
shared native project/map/memory/build state and canonical workspace acceptance enter a short per-project
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
`contextUsage` is absent until the provider reports usage. Codex's `thread/tokenUsage/updated`
supplies active-context `last` and cumulative thread `total`; other adapters retain their observed
usage meaning without inventing missing values. The latest snapshot is persisted
outside `panelLog`, so unopened rows retain usage across an app restart.

For a Codex native foreground turn, the runtime records the official nonempty `turn/started`
`turn.id` before accepting `thread/tokenUsage/updated`. It persists usage only when the event's
`turnId` equals that active native turn ID; usage from a prior or stale turn is ignored. The normal
`SessionRuntimeEventSink` remains strict, so this correlation happens before session event routing.

`contextState` persists the instruction epoch, static-baseline fingerprint, and only the last
successfully delivered model cursor. `taskState.events` is append-only; `leafId` selects the
current branch and `projection` is a checksum-verified cache rebuilt from that branch on load.
Both top-level fields use `serde(default)`. Existing schema-v3 records without them retain their
name, thread, pending review, usage, and panel log and load with empty state; there is no new global
schema cutover.

Attachment bytes remain under `%localappdata%\eud-agent\attachments\objects\` and are bound to
the session on send.

E3S import preserves locally associated EPS and Map conversation history as new native sessions.
Historical EPS project keys may include surrounding single quotes; matching uses the full path,
never the basename. Map history uses the legacy canonical full-path hash. Copies retain names,
provider/model/reasoning settings, conversation timestamps, and opaque panel logs. Fresh session
IDs separate them from original records; provider connections, pending review IDs, usage/context/
task state, journals, and map candidates are not carried over. Original session records remain
unchanged. Import validates the index and each matching record before publication. Unreadable
records and occupied native histories are scoped omissions, not a veto on healthy independent
records/kinds. Explicit setup consent is required for current omissions; rollback removes only
its own unchanged records/index entries and preserves unrelated concurrent session saves.

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

`conversation_rewind` replaces the log with the selected prefix, resets provider continuation, pending
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
never disables chat. Each attempt owns an independent `StructuredJobExecutor` created from the
job's persisted provider/model/reasoning/base URL snapshot, with isolated input/cwd, no EUD tools
or MCP endpoint, and no main store/workspace/UI authority. Schema and domain validation precede
staging; retry never reads current global defaults.

## Session workers

`SessionEngineManager` lazily owns `HashMap<SessionId, Arc<SessionWorker>>`. Each worker contains:

- a session-bound `AgentEngine<R: RuntimeExecutor>` behind its own Tokio mutex;
- one `ProviderRuntime` with its fixed Codex, Claude Code, Antigravity, OpenCode Go, or Ollama production adapter selected from the persisted binding;
- a session-bound `SessionToolRuntime`; native foreground runs create their own MCP endpoint;
- a per-worker cancellation watch channel;
- `SessionEventSink(app, sessionId)` and immutable provider id for logout busy protection.

The worker mutex is the same-session command sequencer. There is no global `ManagedAgentEngine`
mutex and no mutable session-switching path. `session_open` hydrates only the named worker and is
idempotent. Selecting a sidebar row never calls it.

Resume seeds the binding's typed conversation state through the runtime. A provider mismatch,
corrupt direct head, or unknown native continuation fails closed while preserving pending review;
it does not reset to empty state or automatically replay completed tools. A validated direct head
can repair lagging matching metadata. Native receipts retire only after engine persistence
acknowledgement. Explicit rewind/reset remains a separate idle-session operation.

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

`ToolServices` shares the journal store, RAG, map rails, data dirs, and coordinator.
Every `SessionToolRuntime` separately owns the live request ID, evidence/mutation counters,
iteration and progress identities, pending plan, write registration, source baseline, and tool
execution lock.

Each native foreground run creates an ephemeral unique-URL MCP endpoint and handler bound to its
fixed `RunIdentity`. Old handlers cannot acquire a later run's tool authority. Direct batches and
native MCP use the same gate; native tool notifications never execute tools again. The first
mutating tool call in a read run is not executed: the gate registers the exact
`(project, session, request)`, parks the remaining batch, and resumes the same conversation in
write mode. Each shared-state dispatch then runs inside one short project transaction.

Cancellation closes new admission before transport cleanup. Late model events are discarded, but
already-started blocking tools settle and persist their original run's completion/journal. Failed
final answers neither discard reviewable mutations nor replay/rollback tools automatically. ASK
waiting pauses the provider active deadline and clears once on cancellation or transport exit.

## Workspace isolation

The provider CLI cwd is the native project root — the directory holding `project.eap`.
The agent reads the real project tree (`src/`, `dat/`, `maps/`, `build/`, `compat/`)
and the durable documents directly; there is no session mirror and no per-turn document copy.

```text
<project>/                                  ← CLI cwd
  project.eap
  src/  dat/  maps/  build/  compat/
  .eud-agent/
    workspace/                              ← canonical documents (specs/plans/decisions/worklog)
      .tmp/<session-id>/                    ← the only filesystem write area
    state/workspace.json                    ← trusted acceptance metadata
    memory/
```

Trusted accepted-document/plan metadata lives in `<project>/.eud-agent/state/workspace.json`,
outside the editable document tree. Its workspace ID survives moving the project. Journals,
jobs, and turn baselines remain machine-local under `%APPDATA%/eud-agent`; durable harness
documents and memory/wiki move with the project.

Sandbox scope: the read profile exposes the whole project root read-only; the write profile
adds exactly `.eud-agent/workspace/.tmp/**` as writable. Every other filesystem path stays
read-only — source and document mutations go through eud-tools into the journal/changeset
review path. `TEMP`/`TMP` point at the session's `.tmp/<session-id>/` directory.

Turn baselines live outside the cwd at
`%APPDATA%/eud-agent/workspaces/.state/baselines/<request-id>/<workspace-id>/` with
`documents/` (the canonical document tree) and `source/` (canonical `src/` bytes without the
`src/` prefix) subtrees. Read turns capture no baseline. A write turn captures both subtrees
before the first mutation, and the agent is told to re-read targets and retry rather than
replaying stale arguments.

Document changes stage directly against the canonical tree and are journaled with exact
before/after bytes. Acceptance compares the journal baseline with current canonical bytes:
unchanged targets promote directly, non-overlapping line changes merge automatically, and
overlapping changes fail with `ConcurrentWriteConflict` while leaving canonical bytes
untouched. Reject restores the exact journaled canonical bytes.

Native source tools use the captured `source/` baseline subtree as their optimistic baseline.
`file_write` and `file_edit` three-way merge non-overlapping live changes; `file_edit` first
applies ordered exact replacements to the request's latest desired content and rejects missing or
ambiguous matches before mutation. Write/delete/rename/move reject stale overlapping targets.
Shared tool calls and builds are serialized only for the duration of that call.
Approved plan snapshots remain app-owned, immutable, and preserved after implementation rejection.

## Autonomous run lifecycle

`SessionRecord.autonomousRun` persists an opt-in run independently from `session_activity`.
The record owns the original goal, request/client-turn identity, selected project and revision,
policy, iteration number, bounded progress fingerprints, observed provider usage, last build,
blocker, and last confirmed provider checkpoint. Checkpoint persistence updates the run and typed
provider conversation atomically, then retires the runtime receipt only after acknowledgement.

`running`, `pausing`, `paused`, `paused_after_restart`, `waiting_input`, `review`,
`safety_stopped`, `cancelled`, `failed`, and `completed` are distinct durable states. Active elapsed
time excludes user/review/restart waits. Startup converts interrupted running, pausing, or ASK work
to `paused_after_restart`; it never launches provider or mutation work. Explicit resume requires a
resumable status, no pending semantic review, the same project identity and canonical revision,
and byte-equivalent persisted/runtime provider checkpoints. A mismatch becomes `safety_stopped`.

The request ID, semantic journal, and cumulative progress survive iteration boundaries in both
interactive and autonomous turns. Only the soft iteration action count resets. Direct providers resume from the ordered transcript after all
durable call results; native providers resume only from an official completed native continuation.
Pause closes new tool admission at the next confirmed boundary. Stop cancels ASK/provider
generation, preserves reviewable journal entries, and terminal state cannot be overwritten by a
late completion.

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
| `chat` | start an explicit interactive or opt-in autonomous read turn |
| `plan_feedback` | revise that session's plan in read mode |
| `plan_approve` | register write intent and execute after grant |
| `changeset_decision` | accept/reject that session's journal |
| `cancel` | interrupt that turn or remove that write ticket |
| `autonomous_pause` | request pause at the next confirmed tool/provider boundary |
| `autonomous_resume` | validate and continue a paused run from its exact checkpoint |
| `autonomous_stop` | cancel the run, ASK, and new tool admission while preserving review |
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
- `session_activity` for current read/write/wait/review resource activity;
- `autonomous_run` for the separate durable autonomous lifecycle and progress projection.

Tool call/result events additionally preserve the gate-assigned UUID `callId`. The panel matches
EPS and Map rows by `(sessionId, callId)` only. An ID-less start is informational and an ID-less
terminal is a standalone terminal row; neither may mutate a last-running row. A late event whose
session or captured run identity no longer matches the addressed row is discarded. There is no
selected-row, most-recent-running, or other identity fallback.

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

Native map SHA-256 reads use a non-inheritable `rbN` file handle with RAII ownership before any
child process can inherit it. The checkpoint-22 full-suite cleanup failure remains historical with
its exact holder unidentified; the checkpoint-23 event-barrier probe observed Windows deletion
error 32 for inheritable `rb` and success for `rbN`, and the exact Map recovery selector passed.
Checkpoint 25's full Rust suite passed the Map recovery regression; actual Map UI validation
remains separate.

The permanent Map descriptor now uses the production registry schema. Checkpoint35's exact budget,
exhaustive-operation, and verbatim-descriptor selectors pass; Antigravity's production-adapter
regressions also preserve all Map alternatives, inherited fields, local references, and non-first
operations. These are deterministic codec/schema results, with strict local admission still
authoritative where the remote wire cannot express every closure rule.

App37's numeric render refusal established the obsolete string-enum admission defect. The registry
now validates Map calls with Draft7 and its public numeric-scale and complete-draft-operation tests.
Actual39 subsequently completes status/analyze/palette/render/draft-begin but fails its patch on a
271-character temporary draft path before candidate mutation. Source41 corrects that shared Windows
path boundary; the permanent long-path Map selector passes. Actual Map41 preserves two preceding
`before` conflicts, then completes a legal same-request patch, render/analyze, and finalize to
candidate revision 1. The trusted UI Apply changes the isolated source from baseline hash
`F864E65E0B078FF383DEB1071DC74128501FCBA908BBB4ADB24F2CDCFEFF5C24` to
`144E15B8B4AA0A15B60CCE2BD27079869CF71F4680E71DE7C9C487376E49C1FA`; one Undo restores the
exact baseline bytes and hash. Stale-fork validation did not run, so no stale acceptance claim is
made.

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

App37 also supplies bounded continuation evidence: native ASK answer settled after about 91.8
seconds of wait, a separate cancelled ASK stayed idle through 8.08 seconds of observation, and a
normal restart restored the 88-byte EPS source/hash and persisted sessions without replaying either
the settled ASK or failed Map request. Actual39 separately overlaps a Map read with an OpenCode
Go/`glm-5.3` read; its results and usage remain in their own session rows. Independent QA41 then
observes r0/base, no candidate, disabled Apply/Undo, and retained final/history without issuing a
request. The normal app close leaves no owned processes and preserves baseline hashes. This does
not establish stale-fork behavior or gameplay-backed harness recovery.

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
