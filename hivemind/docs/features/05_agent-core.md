# Agent Core (Rust session workers + eud-tools MCP + project write transactions)

The in-process Rust backend runs one persistent Codex conversation per saved session. Sessions
have independent drivers, loopback MCP endpoints, cancellation generations, request gates,
preflight state, and immutable event sinks. There is no conversation-wide or application-wide
turn mutex: different sessions may read, search, reason, plan, and answer concurrently.

## Session execution

`SessionEngineManager` lazily creates one `SessionWorker` for each session id. A worker owns:

- `AgentEngine<ProductionCodexDriver, SessionEventSink>`;
- one `CodexAppServerClient` and event receiver while active;
- one `SessionToolRuntime` and ephemeral loopback eud-tools MCP server;
- one cancellation generation;
- the fixed session id used by every conversation event.

The worker mutex serializes commands in that session only. Selecting a panel row only changes
the visible `PanelStore`; it never opens, resets, cancels, or transfers backend execution.
`session_open` hydrates one worker's saved thread and pending review without touching any other
worker. Resume failure starts a fresh thread with a condensed transcript and the full first-turn
safety prompt.

## Read and write execution modes

Every Codex turn has explicit `WorkspaceAccess::Read` or `WorkspaceAccess::Write`.

- Read turns use `eud_workspace_read`: minimal runtime reads, the session workspace read-only,
  and network disabled.
- Write turns use `eud_workspace_write`: minimal runtime reads, the session workspace writable,
  `source/**` read-only, and network disabled.
- Both require the elevated exact-root Windows sandbox. Unsupported or denied setup fails closed.
- App-server command approvals are automatic in both modes so native commands and Code Mode
  JavaScript can run inside the active profile. File-change, patch, and permission-expansion
  approvals remain denied; neither command approval widens filesystem or network access.
- Switching mode respawns the session's app-server when necessary, retains the thread id, and
  resumes the same conversation.

All initial chats and plan-feedback turns start in read mode. A direct edit calls
`request_write_lane(reason)`, which records write intent and parks the turn. `plan_approve`
submits the same request directly in the backend. Mutating tools called without ownership return
`WriteLeaseRequired`; the read sandbox independently denies native filesystem writes.

## Project write transaction

`ProjectWriteCoordinator` owns a fair FIFO queue per project. FIFO order is the time write intent
is registered, not chat submission time. The lease covers the complete transaction:

1. write intent;
2. latest accepted state rebase and trusted baseline;
3. editor/map/memory/native-workspace mutations;
4. mandatory build and any repair;
5. changeset review;
6. complete accept or rollback.

The lease is not released for partial decisions, an undecided journal, rollback failure, or a
cancelled writer that has journaled changes. A review blocks later writers only; read turns in
other sessions continue. On release the next ticket is granted automatically and its saved thread
resumes in write mode. Panel `memory_save` and `wiki_save` use the same coordinator through short
synthetic transactions; session persistence and autosave do not.

Backend activities are `idle`, `running_read`, `waiting_write`, `running_write`, `review`, and
`error`. `queuePosition` exists only for `waiting_write`.

## Tool state and MCP

`ToolServices` shares immutable/app-wide services: data directories, journal store, RAG, map
rails, analyzer, and write coordinator. Each `SessionToolRuntime` separately owns:

- current request/project id;
- evidence, mutation, action, search, and build budgets;
- pending plan;
- epScript preflight snapshot/suppression state;
- write ticket and execution lock.

Each worker hosts its own `127.0.0.1` streamable-HTTP MCP endpoint and shuts it down when the
worker is discarded. No mutable global request pointer identifies MCP callers.

Read tools: `project_status`, `list_files`, `read_file`, `eps_check`, `dat_get`, `xdat_get`,
`tbl_get`, `req_get`, `btn_get`, `settings_get`, `plugins_list`, `map_info`, `map_minimap`,
`search_docs`. The DAT/XDAT/TBL/REQ/BTN getters require a non-empty `items` array, execute the
items sequentially inside one runtime call, preserve input order, and return a per-item
`ok`/value-or-error result with the identifying coordinates echoed.

Flow tools: `propose_plan(markdown)`, `request_write_lane(reason)`.

Write tools: `dat_set`, `xdat_set`, `tbl_set`, `req_set`, `btn_set`, `dat_reset`, `file_create`,
`file_write`, `file_edit`, `file_rename`, `file_delete`, `file_move`, `mkdir`, `set_main`,
`settings_set`, `plugin_add`, `plugin_edit`, `plugin_remove`, `plugin_move`, `build_run`,
`location_write`, `player_setup`, `switch_write`, `memory_write`.

`file_edit` applies a non-empty ordered list of exact, uniquely matching `old_text`/`new_text`
replacements to the session baseline, then uses the same non-overlapping live-change merge and
full before/after journal snapshots as `file_write`.

Evidence, first-principles, mutation-count, action-count, search, and three-build-attempt rails
remain request scoped. The non-search action hard ceiling is 300 calls; each batched getter
envelope consumes one action. `request_write_lane` is non-mutating and consumes no mutation budget.

## Session workspaces

The canonical accepted project workspace remains:

`%appdata%\eud-agent\workspaces\<project-id>\`

Codex runs from:

`%appdata%\eud-agent\workspaces\.sessions\<project-id>\<session-id>\`

Before every read turn, accepted canonical documents are delta-synced and a coherent session-owned
`source/` snapshot is refreshed. Before write mode, the same sync runs again, then the trusted
baseline is captured. Workspace changes are journaled with both project and session ownership.
Accept promotes selected session bytes to canonical storage under the lease; promotion and trusted
metadata update roll back together on failure. Reject restores only the session root, leaving
canonical bytes unchanged. The app-owned approved plan is written canonical before the execution
baseline, synced into the session root, remains immutable, and survives implementation rejection.

## Journal and recovery

Every project mutation is journaled before review. Partial accept removes only accepted entries;
partial reject rolls back and removes only rejected entries. Remaining entries stay live and keep
the lease. Accept/reject-all archives only after all required work succeeds.

On startup, session records for a project are scanned before admitting a new writer. One pending
journal is restored as the project review owner. Multiple legacy pending writers, a missing
journal, or an empty pending journal is an explicit recovery error; the backend never chooses
silently. Read turns and unfinished queue tickets are process-local, while journaled writes
survive restart as review.

## Event and cancellation isolation

`SessionEventSink` is constructed with one immutable session id. `agent_event`, `answer`, `plan`,
`changeset`, `rollback_result`, turn `progress`, and turn `error` always carry that id. Global
project/setup/bootstrap/RAG events remain unscoped. There is no `session_active` routing event or
fallback to the selected panel row.

`cancel {sessionId}` advances only that worker's cancellation generation or removes only its
waiting write ticket. A writer with journal entries transitions to review and retains ownership.

## Verification

- Barrier-based Rust tests prove overlapping different-session reads and same-session
  serialization.
- Coordinator tests prove one writer, write-intent FIFO, queue-position updates, scoped queued
  cancellation, review retention, and pending-review restoration priority.
- Runtime tests prove request/evidence/budget/preflight isolation.
- Workspace tests prove stable session snapshots, accepted promotion, canonical rejection
  invariance, and approved-plan preservation.
- Panel integration tests prove overlapping `chat` invokes and strict event-to-`PanelStore`
  routing.
