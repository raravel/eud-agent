# Feature: Session restore

External AI conversations in eud-agent currently live only in (a) the codex in-process
thread and (b) the panel's in-memory `log`. Closing the app loses everything. This feature
persists conversations as **named sessions** that survive a full app restart: opening a
saved session re-renders its conversation, **resumes the codex thread** (so the model keeps
prior context), and reconnects any un-applied changeset so the user can keep deciding.

This file is the shared contract. The Rust backend and the React panel are built to it
independently; do not drift from the command names, JSON shapes, or event names below.

## Confirmed decisions

- **A. Rust owns all session files.** The panel never touches the filesystem; it pushes its
  serialized log through `session_update_log` and Rust writes the whole record. (Matches the
  `panel ↔ core is Tauri IPC only` rule.)
- **B. Auto-save (no save button).** A conversation IS a session, persisted continuously —
  exactly like codex's rollout or a ChatGPT chat. The core auto-creates the active session on
  the conversation's FIRST turn (auto-named from the first user message; rename later from the
  list), and every completed `chat` turn auto-updates the record (thread_id, pending req-ids).
  The panel pushes its serialized log after each change via `session_update_log`. `새 대화`
  (reset) detaches the active session so the next turn starts a fresh one. There is no
  "save" action and no name prompt.
- **C. Single live changeset invariant.** At most ONE pending (un-archived) changeset is
  reconnected per session — the latest. Matches the existing EUD-070 single-live-changeset
  behavior; do NOT replace `current_request_id: Option` with a registry.
- **D. Location: `%appdata%\eud-agent\sessions\`** (Roaming — small, user-owned, must survive
  self-update; the updater preserves `%appdata%`).
- **E. Resume-first, replay-fallback.** Primary path seeds the saved `thread_id` so codex
  issues `thread/resume`. If resume fails OR does not complete within a timeout, fall back to
  a fresh `thread/start` and inject a condensed transcript into the first turn's prompt via
  the existing `resume_turn_text` prepend mechanism. The fallback is defensive-by-construction
  (timeout + error catch) so it is safe regardless of how codex signals a missing rollout.

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

All registered in `lib.rs` `generate_handler!`. Engine-command shape:
`#[tauri::command(rename = "...")] async fn session_*(state: State<'_, ManagedAgentEngine>, ...)
-> Result<T, String>` delegating to an `AgentEngine` method, `.map_err(|e| e.message)`.

| rename (panel `invoke` name) | args | returns | notes |
|---|---|---|---|
| `session_list` | — | `Vec<SessionMeta>` | most-recently-updated first |
| `session_update_log` | `{ panelLog: Value }` | `()` | the panel pushes its serialized log after each turn; updates the active record's `panelLog` (no-op if no active session) |
| `session_open` | `{ id: String }` | `SessionRecord` | resets live thread, seeds saved thread_id, reconnects ≤1 pending changeset, sets active session; panel hydrates from the returned `panelLog` |
| `session_rename` | `{ id: String, name: String }` | `()` | |
| `session_delete` | `{ id: String }` | `()` | also removes from index; if it is the active session, detach (`current_session_id=None`) |

The core auto-creates the active session inside `chat()` on the first turn (there is no
`session_save` command). Two panel events: `session_loaded` (signal after a reconnect on
open) and `session_active` (`{ id, name }` — emitted on auto-create and on open so the panel
highlights the current row).

`SessionMeta` = `{ id, name, project, createdAt, updatedAt }`.
`SessionRecord` = `SessionMeta` (flattened) + `{ threadId, pendingRequestIds, panelLog }`.
Field names in the JSON crossing IPC are **camelCase** (`#[serde(rename_all = "camelCase")]`)
to match panel expectations.

## Rust module: `src-tauri/src/session.rs` (new)

```rust
pub struct SessionStore { sessions_dir: PathBuf }

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SessionMeta { pub id, name, project: String; pub created_at, updated_at: u64 }

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecord {
    #[serde(flatten)] pub meta: SessionMeta,
    pub thread_id: Option<String>,
    pub pending_request_ids: Vec<String>,
    pub panel_log: serde_json::Value,
}

impl SessionStore {
    pub fn new(dirs: &DataDirs) -> Self;
    pub fn list(&self) -> anyhow::Result<Vec<SessionMeta>>;          // read index.json (empty → [])
    pub fn load(&self, id: &str) -> anyhow::Result<SessionRecord>;
    pub fn save(&self, rec: &SessionRecord) -> anyhow::Result<()>;   // write <id>.json + rewrite index.json
    pub fn delete(&self, id: &str) -> anyhow::Result<()>;
    pub fn rename(&self, id: &str, name: &str) -> anyhow::Result<()>;
}
```
- `config.rs`: add `sessions_dir()` and include it in `ensure_dirs()`.
- Reuse `memory::write_atomic_bytes` (same crate, `pub(crate)`); note the coupling, do not add
  a new fs module.
- A corrupt/missing `index.json` yields `[]` (never crash startup); a corrupt `<id>.json` on
  open is a graceful `Err` surfaced to the panel.

## Engine + driver changes (`engine.rs`, `codex_client.rs`)

1. `CodexDriver` trait gains:
   ```rust
   async fn current_thread_id(&self) -> Option<String>;
   async fn seed_thread_id(&mut self, id: String) -> Result<(), AgentEngineError>;
   ```
   Implement on `ProductionCodexDriver` AND the in-file mock/test driver. On the production
   driver, `seed_thread_id` must `ensure_client().await` first (the client is lazily spawned),
   then set the client's thread_id mutex.
2. `CodexAppServerClient`: make `current_thread_id` `pub`; add
   `pub async fn set_thread_id(&self, id: String)`.
3. `AgentEngine` gains `current_session_id: Option<String>` and a `SessionStore` (injected in
   `lib.rs` construction next to the existing providers).
4. `chat()`: `ensure_active_session(&req.text)` at the start auto-creates the session on the
   first turn (auto-named from the first message), emitting `SessionActive`. After a successful
   turn, `update_active_session()` refreshes the record (thread_id from
   `driver.current_thread_id().await`, pending req-ids). The `panelLog` is pushed separately by
   the panel via `session_update_log`. thread_id capture relies on `ThreadStarted` having arrived
   by turn completion — verified in a unit test.
5. `reset()`: set `current_session_id = None` (a `새 대화` detaches from the active session;
   the next turn auto-creates a fresh one).
6. New `open_session(id) -> SessionRecord`:
   - `self.reset()` first (drop live thread),
   - if `threadId` present: `driver.seed_thread_id(tid)`; on Ok set `thread_active = true` so the
     next `chat()` resumes; on Err leave `thread_active=false` (fresh + replay). When there is no
     `threadId` but a non-empty log, stage the transcript for a fresh-start replay too,
   - `current_session_id = Some(id)`,
   - reconnect ≤1 pending changeset (below),
   - emit `SessionActive` then `SessionLoaded`; return the record.
7. `changeset_decision()`: after a decision archives a journal, drop that req-id from the active
   session record's `pendingRequestIds`.
8. `EngineEvent::SessionLoaded` → panel `session_loaded` (signal after reconnect) and
   `EngineEvent::SessionActive { id, name }` → panel `session_active` (current-session highlight).
   Neither is rendered raw — rules.md forbids raw kind identifiers as user text.

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

## Panel changes (`panel/src`)

- `state/store.ts`: add a `hydrate(panelLog)` action (or `createPanelStore(initial?)`). It must
  run at/just-after store creation so `useSyncExternalStore` sees it, and must advance the
  closure-private `logSeq` (and `toolSeq`/`blockSeq`) past the restored ids, or React keys
  collide. Set `App.tsx` `lastToastedLogId` to the restored max id to avoid a toast storm.
  Leave `phase=connecting`; transient state stays empty and re-arrives from the core.
- `state/protocol.ts`: add `SessionMeta`, `SessionRecord`, `PanelLog` types matching the camelCase
  JSON above.
- `App.tsx`: add a serializer that maps `state.log` → `PanelLog` (durable subset only); a
  debounced effect on `state.log` pushes it via `invoke('session_update_log', { panelLog })`
  after each change (no save button). On `session_open`, take the returned record, call
  `store.hydrate(record.panelLog)` BEFORE the normal connect flow repopulates live state. Track
  the active session id from the `session_active` event (cleared on `reset`).
- `components/SessionList.tsx` (new): modal listing `session_list` results (name, project,
  updatedAt) with open / rename / delete actions; the active session (`currentId`) is
  highlighted. No save button. Do NOT re-open a review surface from the log — review surfaces
  gate on live `state.changeset`/`state.plan`; a reconnected changeset arrives as a live
  `changeset` event.

## Verification

- Rust: `cargo fmt --check`, `cargo clippy`, `cargo test` (unit tests for SessionStore
  round-trip, driver seed/current thread_id on the mock, open_session seeding + thread_active,
  changeset reconnect via a written journal file). Use the shared `CARGO_TARGET_DIR` to avoid
  cold ort/tauri compiles.
- Panel: `npm run build` (tsc + vite) clean; hydrate advances counters with no key collision.
- E2E (user-assisted, editor GUI): create conversation → 저장 → restart app → open → conversation
  re-renders, follow-up instruction keeps prior context, pending changeset still decidable.

## Constraints (rules.md)

- Editor/third-party never modified. IPC/config/session files UTF-8 **no BOM**, atomic writes.
- codex invocation rules unchanged (stdin, `.cmd` shim, no persistence-disable flag).
- Existing `memory/`, `journal/`, `wiki/` contracts unchanged — sessions are a layer above project.
- Panel ↔ core is Tauri IPC only; never render raw `agent_event` kind identifiers as user text.
