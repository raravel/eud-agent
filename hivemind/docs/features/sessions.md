# Feature: Multi-active sessions

eud-agent persists named Codex conversations and keeps multiple conversations active in the
panel at once. Every session owns an independent panel store, conversation log, Codex
`threadId`, turn/review state, and autosave stream. The current editor project owns one
execution lane: sessions may be viewed and queued concurrently, but Codex turns and changeset
decisions run serially so rollback can never overwrite a later session's edit.

This file is the shared Rust/panel contract. Do not drift from the command names, JSON shapes,
project-lane rules, or event routing below.

## Confirmed decisions

- **A. Rust owns durable session files.** The panel creates local unsaved draft tabs, but a draft
  becomes a Rust-minted persisted session before its first queued turn. The panel never writes
  session files directly.
- **B. Multiple active panel sessions.** Selecting a left-sidebar row only changes the visible
  conversation. It MUST NOT reset the executing Codex thread, archive a changeset, or steal the
  project lane. Each row retains its own log, streaming buffers, plan, changeset, and status.
- **C. One execution lane per editor project.** Initial chats queue FIFO. The lane remains owned
  through `plan_review` and `changeset_review`; another session starts only after the owner
  reaches `ready` or is cancelled. A new chat MUST NOT default-accept a pending changeset.
- **D. One live changeset per session.** `pendingRequestIds` reconnects at most the latest
  unarchived request for that session. A reopened pending review acquires the project lane before
  any queued turn.
- **E. Session-scoped IPC.** Every conversation mutation includes `sessionId`. Turn events carry
  optional `sessionId`; global status, memory, wiki, setup, and bootstrap events remain
  project/app scoped.
- **F. Current-project sidebar.** The left sidebar lists only sessions whose `project` matches
  the editor's current project. Other-project sessions stay durable but are not executable or
  shown in that project.
- **G. Location.** Durable records remain under `%appdata%\eud-agent\sessions\`; attachment bytes
  remain under `%localappdata%\eud-agent\attachments\objects\`.
- **H. Resume-first, replay-fallback.** Activation seeds the saved `threadId` for
  `thread/resume`. A seed/resume error or timeout starts a fresh thread with a condensed
  transcript and the full first-turn guardrails.

## On-disk layout

```
%appdata%\eud-agent\sessions\
├── index.json              # ordered session list (drives the list UI)
└── <session-id>.json       # one full record per session
```

All files written via `memory::write_atomic_bytes` (temp + rename, UTF-8 **without BOM**).
`<session-id>` is a v4-style UUID minted in Rust (reuse the project's existing uuid path;
do NOT use `Math.random`/panel-side ids).

### index.json
```json
{
  "schemaVersion": 1,
  "sessions": [
    { "id": "<uuid>", "name": "유닛 HP 작업", "project": "mymap",
      "createdAt": 1718000000, "updatedAt": 1718009999 }
  ]
}
```
`sessions` is ordered most-recently-updated first. Rewritten atomically on every mutation.

### <session-id>.json
```json
{
  "schemaVersion": 1,
  "id": "<uuid>",
  "name": "유닛 HP 작업",
  "project": "<editor project name captured at session creation>",
  "createdAt": 1718000000,
  "updatedAt": 1718009999,
  "threadId": "019ece1c-...-f86f5b119d40",
  "pendingRequestIds": ["req-1a2b3c4d"],
  "panelLog": { "schemaVersion": 1, "logSeq": 4, "log": [ ... ] }
}
```
- `threadId` is `null` until the first turn emits `ThreadStarted`.
- `pendingRequestIds` ⊆ `journal/<req>.json` files still live (un-archived) at save time.
  Per decision C this list has at most one entry that will be reconnected; older live
  journals are default-accepted/archived on save.
- `panelLog` is **opaque to Rust** — stored and returned verbatim as `serde_json::Value`.
  Its schema is owned solely by the panel (see below).

## panelLog schema (panel-owned)

```json
{
  "schemaVersion": 1,
  "logSeq": 4,
  "log": [
    { "id": 1, "kind": "you",   "text": "유닛 HP 올려줘" },
    { "id": 2, "kind": "agent", "text": "...streamdown markdown..." },
    { "id": 3, "kind": "info",  "text": "도구 호출 2건",
      "tools": [
        { "id": "tool-1", "name": "file_create", "state": "done", "args": "{...}", "detail": "ok" }
      ] },
    { "id": 4, "kind": "ok", "text": "적용 유지 (2건)" }
  ]
}
```
Durable `LogEntry` subset: `id`(number), `kind`(LogKind), `text`(string), optional
`stage`(string), optional `tools[]`. Durable `AgentTool` subset: `id, name, state, args?,
detail?` (archived tools are always terminal `done`/`failed`). Transient state
(`turn`, `plan`, `changeset`, `pendingDecision`, `wiki`, connection flags) is NOT persisted —
it re-arrives from the core on reconnect.

## Tauri IPC commands

All commands are registered in `lib.rs` `generate_handler!`.

| panel `invoke` name | args | returns | notes |
|---|---|---|---|
| `session_list` | — | `Vec<SessionMeta>` | all durable rows, most-recently-updated first; panel filters current project |
| `session_load` | `{ id }` | `SessionRecord` | read-only; selecting a row uses this and never activates Codex |
| `session_create` | `{ firstText }` | `SessionRecord` | Rust id + first-message-derived name; called when a draft reaches the queue head |
| `session_update_log` | `{ id, panelLog }` | `()` | autosaves any active/inactive row without changing the execution owner |
| `session_open` | `{ id }` | `SessionRecord` | internal lane activation/resume; reconnects ≤1 pending changeset |
| `session_rename` | `{ id, name }` | `()` | updates record + index |
| `session_delete` | `{ id }` | `()` | removes record, index row, and bound attachment objects |
| `chat` | `{ sessionId, text, attachments }` | `()` | activates/resumes that session only when it owns the queue head |
| `plan_feedback`, `plan_approve`, `changeset_decision`, `conversation_rewind`, `cancel` | each includes `sessionId` | `()` | rejected when the id does not own the current execution lane |

`session_active {id,name}` establishes the backend event-routing owner; it does not select the
sidebar row. `session_loaded {id}` signals that activation/reconnect completed. `agent_event`,
`answer`, `plan`, `changeset`, `rollback_result`, turn `progress`, and turn `error` are flattened
with `sessionId`. Global project/app events omit it.

`SessionMeta` = `{ id, name, project, createdAt, updatedAt }`.
`SessionRecord` = `SessionMeta` + `{ threadId, pendingRequestIds, panelLog }`.
All crossing fields are camelCase.

Attachment bytes are never embedded in roaming JSON. Draft attachment cleanup and
session-delete ownership rules are unchanged.

## Rust module: `src-tauri/src/session.rs`

`SessionStore` owns the directory plus a clone-shared process mutex. Record + index writes remain
individually atomic; the mutex serializes engine completion, sidebar rename/delete, and inactive
session autosave read-modify-write operations.

```rust
pub struct SessionStore {
    sessions_dir: PathBuf,
    write_lock: Arc<Mutex<()>>,
}

impl SessionStore {
    pub fn list(&self) -> anyhow::Result<Vec<SessionMeta>>;
    pub fn load(&self, id: &str) -> anyhow::Result<SessionRecord>;
    pub fn save(&self, record: &SessionRecord) -> anyhow::Result<()>;
    pub fn update_panel_log(&self, id: &str, panel_log: Value) -> anyhow::Result<()>;
    pub fn rename(&self, id: &str, name: &str) -> anyhow::Result<()>;
    pub fn delete(&self, id: &str) -> anyhow::Result<()>;
}
```

A missing/corrupt index yields `[]`; a missing/corrupt record is a surfaced error. Every file is
UTF-8 without BOM through `memory::write_atomic_bytes`.

## Engine + driver contract

- `AgentEngine` still has one live `ProductionCodexDriver`, `current_session_id`, and
  `current_request_id`: the project queue intentionally serializes turns.
- `chat_in_session(sessionId, req)` activates the requested record when the lane is free, then
  calls the existing turn loop. `PlanReview`, `ChangesetReview`, or any live journal blocks a
  session switch with a user-facing review-first error.
- `open_session(id)` is lane activation, not sidebar selection. Switching saves the prior
  record, resets the driver thread, seeds the target `threadId`, stages replay fallback, and
  reconnects its pending changeset. Reopening the current owner is idempotent.
- Successful chat, plan continuation, approval, and changeset decisions refresh the owning
  record's thread id and pending request ids.
- `ManagedAgentEngine` holds a clone of `SessionStore`, allowing list/load/log autosave without
  waiting for the long-running engine mutex. The store's shared mutex preserves file ordering.
- `TauriEventSink` holds a clone-shared current session id and wraps conversation events in a
  flattened `{sessionId,...payload}` envelope. Test sinks remain payload-only.
- There is no reset endpoint. “새 세션” creates a separate draft tab; lane activation performs
  the required driver reset only when switching between persisted sessions.

### Changeset reconnect
The changeset is derived from the journal (`journal::changeset_from_journal`) and
`JournalStore::load(data_dir, request_id)` rehydrates one journal by id. In `open_session`,
for the single reconnect target req-id: `journal_store.load(...)`, set
`self.current_request_id = Some(req)` (so `changeset_decision`'s guard passes), then re-emit the
existing `changeset` event to the panel. Resume failure / missing journal must degrade
gracefully (skip reconnect, log), never panic.

### Resume fallback (decision E)
Wrap the seed+first-resume so that (a) a `seed_thread_id` error, or (b) the first post-open
`run_turn` not reaching `thread/started`/completion within a bounded timeout, drops to a fresh
`thread/start`. A non-resumable open (no `threadId`, or seed failed) goes straight to the fresh
start. When falling back, fold a **condensed** transcript (you/agent text + decisions; drop
tool-arg dumps; cap well under prompt limits) into the first turn — and that turn MUST use the
full `build_system_prompt` (a brand-new thread has never seen the `[first principles]`
guardrails), NOT `resume_turn_text`.

## Panel contract

- `App.tsx` owns a dynamic `Map<sessionId, SessionSlot>`. Each slot has its own `PanelStore`,
  metadata, persisted/draft flag, and `idle|queued|running|review|error` activity.
- Global project events fan out to every slot. Conversation events route by payload
  `sessionId`, falling back to the current backend owner only for compatibility.
- The FIFO queue persists the project owner through plan/changeset review. Queued rows expose
  their position and can be cancelled before execution.
- `SessionSidebar` is permanent left primary navigation. It is drag/keyboard resizable from
  220–420px, persists width in `eud.session-sidebar.width`, and collapses to a 56px rail.
  Selected/running/queued/review states use icon + text. Every horizontal container clips
  overflow; long session names use single-line ellipsis with the full name in `title`.
- The center conversation is keyed by selected session id so sticky-scroll internals never
  retain the prior row's rendered log.
- `ProjectSidebar` is the right contextual inspector with DAT wiki, project memory, and
  workspace tabs. It is resizable; below 1140px it overlays the center, and below 1040px the
  session sidebar collapses to a rail. The center never introduces horizontal scrolling at the
  configured 960px minimum window width.
- Autosave subscriptions debounce each persisted slot independently through
  `session_update_log {id,panelLog}`. Toast high-water marks are also session scoped.
- The prompt no longer duplicates “새 대화”; new-session creation lives in the left sidebar.

## Verification

- Rust: full `cargo test`; dedicated coverage pins project-lane review blocking and flattened
  session event serialization.
- Panel: full Vitest suite + `npm run build`; component coverage pins session activity labels,
  selection, queued cancellation, splitter keyboard sizing, and long-name clipping.
- Browser-driven mock-Tauri smoke: two current-project sessions retain isolated logs; the
  second chat invocation occurs only after the first resolves. Splitter drag persisted
  `272px → 368px`; long-name ellipsis and 1280px/960px layouts produced no horizontal scroll.

## Constraints (rules.md)

- Editor/third-party never modified. IPC/config/session files UTF-8 **no BOM**, atomic writes.
- codex invocation rules unchanged (stdin, `.cmd` shim, no persistence-disable flag).
- Existing `memory/`, `journal/`, `wiki/` contracts unchanged — sessions are a layer above project.
- Panel ↔ core is Tauri IPC only; never render raw `agent_event` kind identifiers as user text.
