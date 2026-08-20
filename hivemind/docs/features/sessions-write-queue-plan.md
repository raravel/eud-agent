# Concurrent Sessions with a Project Write Queue — Implementation Plan

Status: superseded (2026-08-20). The FIFO transaction lease below is retained as historical
context; `features/sessions.md` now defines concurrent write registration, per-operation project
transactions, and conflict-aware acceptance.

## Goal

Allow independent conversation sessions to run Codex turns concurrently while serializing only project mutations. A long-running or review-blocked session must not prevent another session from reading project state, searching documentation, reasoning, producing a plan, or returning a read-only answer.

The serialized unit is a **project write transaction**, not an individual tool call:

1. write intent is declared;
2. the session acquires the project write lease;
3. all live-editor, map, memory, build, and native workspace mutations execute;
4. the resulting changeset remains reviewable;
5. the lease is released only after all accept/reject work completes or after a mutation-free turn settles.

This preserves rollback and build isolation while making conversation sessions genuinely concurrent.

## Problem Statement

The current multi-session implementation isolates panel stores, persisted logs, Codex thread IDs, and review state, but it does not provide concurrent execution.

The effective serialization was introduced in commit `70fa41f` and is enforced at several layers:

- `panel/src/App.tsx` owns one `queueRef`, `runningSlotRef`, `reviewOwnerRef`, and `drainQueueRef`. A queued chat starts only after the previous chat and its review ownership settle.
- `ManagedAgentEngine` owns one `tokio::sync::Mutex<AgentEngine<ProductionCodexDriver, TauriEventSink>>`. `engine_chat` holds that mutex across the complete asynchronous Codex turn.
- `ProductionCodexDriver` owns one `CodexAppServerClient`, one event receiver, one current thread ID, and one cancellation receiver.
- `ToolRuntime` owns one global `current: Option<String>` request pointer. `begin_request` clears the request registry, so concurrent requests would overwrite each other's evidence, plan, and budget state.
- `TauriEventSink` owns one mutable current session ID, so interleaved events cannot be routed safely.
- All sessions currently use one writable project workspace, including the atomically refreshed `source/` mirror and native Codex document writes.

Commit `34e8411` added the visible multi-session workspace UI and activity presentation, but the central execution queue predates it.

The current behavior therefore differs from a single conversation only in persistence, thread continuity, independent logs, review retention, and navigation. Throughput remains one turn at a time.

## Core Decision

### Queue write transactions, not conversations

A project owns one fair FIFO `ProjectWriteLease`. Conversation sessions do not own a global execution lane.

The write lease starts before the first project mutation and remains owned through build and changeset review. Releasing it after each individual write is unsafe:

- Session A can write a target, session B can overwrite it, and rejecting A can then restore A's old value over B's accepted value.
- A build can observe a mixture of mutations from multiple sessions and falsely validate either session.
- Native workspace writes can bypass MCP tool-call locking unless the workspace permission mode is also tied to the lease.

### Concurrent activity model

Each session has one of these backend-owned activities:

```text
idle
running_read
waiting_write
running_write
review
error
```

`queuePosition` is present only for `waiting_write`. It means a position in the project write queue, never a position in a conversation queue.

State transitions:

```text
idle -> running_read
running_read -> idle                 read-only answer
running_read -> waiting_write        direct edit declares write intent
waiting_write -> running_write       project lease granted
running_write -> review              journaled changes exist
running_write -> idle                no changes remain
review -> idle                       all decisions complete
any running/waiting state -> error   unrecoverable failure
```

A session in `review` blocks only later writers. Other sessions may continue `running_read` work.

## Required Invariants

1. Different sessions may run read-only Codex turns concurrently.
2. Commands within one session remain serialized.
3. A project has at most one write owner.
4. FIFO order is determined when write intent is registered, not when chat text was submitted.
5. Only the write owner may execute mutating MCP tools, `build_run`, project memory writes, map writes, or native writable-workspace operations.
6. The write lease remains held while any journal entry is undecided or any rollback is incomplete.
7. A cancellation releases the lease only when no journaled or unpromoted workspace mutation remains.
8. A pending changeset restored after restart reacquires the project lease before any new writer starts.
9. Every conversation event carries an explicit immutable `sessionId`; global project/application events remain unscoped.
10. One session's cancellation, rewind, error, request state, or Codex event stream cannot affect another session.
11. Selecting a sidebar row never starts, cancels, activates, or transfers backend execution.
12. Session persistence and panel-log autosave do not use the project write queue.
13. All project-scoped mutations, including direct panel IPC mutations, must honor the coordinator; external manual editor changes remain outside the app's control and retain the existing warning semantics.

## Target Architecture

### Session engine manager

Replace the single global engine with a manager of lazily created session workers.

```rust
struct SessionEngineManager {
    workers: Mutex<HashMap<SessionId, Arc<SessionWorker>>>,
    sessions: SessionStore,
    tools: ToolServices,
    writes: ProjectWriteCoordinator,
}

struct SessionWorker {
    engine: tokio::sync::Mutex<AgentEngine<ProductionCodexDriver, SessionEventSink>>,
    cancellation: tokio::sync::watch::Sender<u64>,
    mcp: McpServerHandle,
}
```

Each worker owns:

- one session-bound engine state machine;
- one session-bound Codex driver and event receiver while a turn is active;
- one cancellation generation;
- one session-scoped tool runtime;
- one loopback MCP endpoint;
- one immutable event sink session ID.

Workers are created on first use. An idle or review worker may drop its Codex subprocess after persisting the thread ID; the next turn resumes it through the existing `thread/resume` path. No additional global analysis queue or arbitrary concurrency cap is introduced in this change.

`AgentEngine` becomes bound to one session. Remove the mutable `current_session_id` switching path. `session_open` no longer steals an execution lane or resets another session's driver; it only hydrates the requested worker and reconnects a pending review when necessary.

### Shared and session-scoped tool state

Split the current `ToolRuntime` into:

```rust
struct ToolServices {
    dirs: DataDirs,
    journal: JournalStore,
    rag: Arc<Rag>,
    map_safe: Arc<ProductionMapSafe>,
    analyzer: Arc<dyn EpsAnalyzer>,
    writes: ProjectWriteCoordinator,
}

struct SessionToolRuntime {
    services: ToolServices,
    session_id: String,
    current_request_id: Mutex<Option<String>>,
    request_state: Mutex<Option<RequestState>>,
    pending_plan: Mutex<Option<(String, String)>>,
    write_state: Mutex<SessionWriteState>,
    eps_preflight: EpsPreflight,
}
```

The RAG index, journal store, map rails, analyzer process, and coordinator remain shared. Evidence flags, mutation budgets, approved-plan state, pending plans, request IDs, and preflight request state are session scoped.

Each active worker receives its own ephemeral loopback MCP server. This is intentionally preferred over teaching one endpoint to infer the caller from a mutable global pointer. The MCP server must return a shutdown handle and stop when its worker is discarded.

### Immutable event routing

Replace the mutable `TauriEventSink.session_id` with a sink constructed for one session:

```rust
SessionEventSink::new(app_handle, session_id)
```

Every conversation event is serialized with that fixed ID. Remove `session_active` as an event-routing mechanism and remove the panel's `eventSessionIdRef` compatibility fallback. Project status, setup, bootstrap, RAG warmup, memory snapshots, and wiki snapshots remain global and fan out to all panel stores.

### Project write coordinator

Add `src-tauri/src/write_coordinator.rs` with a project-keyed FIFO queue.

Required operations:

```rust
request(project_id, session_id, request_id) -> WriteTicket
cancel(request_id)
release(request_id)
restore_review(project_id, session_id, request_id)
owner(project_id) -> Option<WriteOwner>
```

A ticket reports `granted` or `waiting(position)`. Queue position changes emit session activity events to every affected waiter.

The coordinator must not hold a blocking MCP request for an unbounded review duration. Waiting is represented as durable engine state for the lifetime of the application, not as a sleeping bridge or HTTP call.

### Read mode and write mode

Codex execution gains an explicit permission mode:

```rust
enum WorkspaceAccess {
    Read,
    Write,
}
```

- Read mode uses a sandbox profile that cannot mutate the session workspace.
- Write mode uses the existing strict writable workspace profile and is available only to the lease owner.
- Both modes retain minimal runtime reads, disabled network access, and exact-root elevated Windows sandbox requirements.
- Transitioning to write mode respawns the app-server when necessary, retains the thread ID, and resumes the same conversation.

A direct small edit requests the lease through a new flow tool:

```text
request_write_lane(reason)
```

This tool:

- is not a mutation and consumes no mutation budget;
- ends/parks the current read turn like `propose_plan` ends a planning turn;
- records write intent without holding the MCP request open;
- causes the manager to resume the same thread automatically in write mode after grant;
- is mechanically required before any mutating tool in read mode.

If Codex attempts a mutating tool without a lease, the runtime returns a stable `WriteLeaseRequired` error directing it to `request_write_lane`. Native writes are independently denied by the read-only sandbox.

`plan_approve` does not require a model tool round trip. The backend submits a write ticket directly, persists the exact approved plan only after grant and before the execution baseline, switches the worker to write mode, and resumes approved-plan execution.

### Session workspaces

Concurrent turns require stable source snapshots and isolated native filesystem changes. Keep the existing canonical project workspace as the accepted, panel-visible state and add session working roots outside it:

```text
%appdata%\eud-agent\workspaces\<project-id>\
    specs\
    plans\
    decisions\
    worklog\
    source\

%appdata%\eud-agent\workspaces\.sessions\<project-id>\<session-id>\
    specs\
    plans\
    decisions\
    worklog\
    source\

%appdata%\eud-agent\workspaces\.state\
```

The canonical root remains compatible with existing data and continues to drive the panel workspace explorer.

Before a read turn:

1. delta-sync accepted canonical documents into the session root;
2. refresh a coherent session-owned `source/` snapshot;
3. run Codex with the session root read-only.

Before a granted write continuation:

1. rebase the session root from the latest accepted canonical documents;
2. refresh its coherent source snapshot;
3. instruct Codex to re-read mutation targets because project state may have changed while it waited;
4. capture the trusted baseline;
5. enable write mode.

At review:

- live editor/map mutations have already been journaled and remain protected by the held lease;
- native workspace changes remain in the session root;
- workspace changes are rendered against the trusted baseline;
- accepting a workspace item promotes it to the canonical root under the lease;
- rejecting it restores/discards the session copy without modifying accepted canonical content;
- the exact app-owned approved plan remains canonical and survives implementation rejection.

Synchronization must be delta-based using existing baseline/hash information; do not blindly recopy unchanged documents. Session roots with no pending review may be refreshed or reclaimed after the thread ID and panel log are safely persisted.

## Implementation Phases

### Phase 1 — Pin the new contract with failing tests

Update the session feature contract and add focused tests before changing behavior.

Backend tests must prove:

- two different session workers enter fake `run_turn` concurrently;
- same-session commands remain serialized;
- interleaved events retain the correct session ID;
- one session's request initialization does not clear another session's evidence, budget, plan, or preflight state;
- cancellation targets exactly one worker.

Panel tests must prove that sending in session B invokes `chat` before session A's unresolved invocation completes.

Target files:

- `hivemind/docs/features/sessions.md`
- `hivemind/docs/features/05_agent-core.md`
- `hivemind/docs/features/06_changeset-review-panel.md`
- `hivemind/docs/features/11_rust-backend-core.md`
- `src-tauri/src/engine.rs`
- `panel/src/App.test.tsx` or the existing closest App integration test location

### Phase 2 — Split shared services from session runtimes

Refactor `ToolRuntime` without changing write behavior yet:

- extract `ToolServices`;
- create one `SessionToolRuntime` per worker;
- make pending plan, current request, gates, budgets, and preflight state session local;
- return an MCP server handle with explicit shutdown;
- construct a session-bound event sink;
- move cancellation to each worker.

Target files:

- `src-tauri/src/tool_exec.rs`
- `src-tauri/src/mcp.rs`
- `src-tauri/src/tools.rs`
- `src-tauri/src/eps_preflight.rs`
- `src-tauri/src/engine.rs`
- `src-tauri/src/lib.rs`

### Phase 3 — Introduce session workers

Replace `ManagedAgentEngine` with `SessionEngineManager`:

- lazily hydrate workers from `SessionStore`;
- bind one engine and driver to one session;
- remove global session switching and mutable event ownership;
- route chat, plan feedback, approval, changeset decisions, rewind, delete, and cancel to the target worker;
- keep list/load/rename/autosave on the existing shared `SessionStore` path;
- separate global model settings from any individual worker mutex and apply saved settings to subsequent turns.

At the end of this phase, read-only turns for separate sessions must be capable of overlapping in tests, but mutation concurrency remains mechanically disabled until the coordinator is complete.

Target files:

- `src-tauri/src/engine.rs`
- `src-tauri/src/session.rs`
- `src-tauri/src/codex_client.rs`
- `src-tauri/src/lib.rs`
- `src-tauri/src/ipc.rs`

### Phase 4 — Add the write lease and execution modes

Implement the coordinator and wire every project-scoped mutation through it:

- register direct write intent through `request_write_lane`;
- submit approved-plan execution directly from `plan_approve`;
- reject all mutating MCP tools without ownership;
- require ownership for `build_run`, project memory writes, map writes, and writable workspace mode;
- retain ownership through complete changeset review;
- keep ownership after cancellation/error when journaled changes remain;
- release only after full settlement;
- emit activity and queue-position events.

Correct the partial-decision state machine so a journal with undecided entries remains in `review` and retains the lease. A failed or incomplete rollback must never advance the queue.

Target files:

- `src-tauri/src/write_coordinator.rs`
- `src-tauri/src/engine.rs`
- `src-tauri/src/tool_exec.rs`
- `src-tauri/src/tools.rs`
- `src-tauri/src/journal.rs`
- `src-tauri/src/ipc.rs`

### Phase 5 — Isolate session workspaces

Add session working roots and read/write sandbox modes:

- prepare stable per-session source snapshots;
- delta-sync canonical documents;
- run initial/read turns in read mode;
- refresh and baseline only after write grant;
- promote accepted workspace changes under the lease;
- discard rejected session changes;
- preserve approved-plan immutability and current completion checks;
- keep the project sidebar on canonical accepted files.

Target files:

- `src-tauri/src/workspace.rs`
- `src-tauri/src/codex_client.rs`
- `src-tauri/src/engine.rs`
- `src-tauri/src/journal.rs`

### Phase 6 — Remove the panel conversation queue

Delete the frontend-owned execution lane:

- remove `queueRef`, `runningSlotRef`, `reviewOwnerRef`, `eventSessionIdRef`, and `drainQueueRef`;
- persist a draft session and invoke its chat immediately;
- treat backend session activity events as authoritative;
- display `분석 중`, `쓰기 대기 N`, `변경 중`, and `검토 필요` distinctly;
- allow new read turns while another session is in review;
- cancel only the selected session's turn or write ticket;
- remove compatibility event routing fallbacks;
- retain the prohibition on deleting or rewinding a running/waiting/review session.

Target files:

- `panel/src/App.tsx`
- `panel/src/lib/protocol.ts`
- `panel/src/lib/ipc.ts`
- `panel/src/state/store.ts`
- `panel/src/components/SessionSidebar.tsx`
- corresponding panel tests

### Phase 7 — Recovery and migration

No destructive session or canonical-workspace migration is required.

- Existing session IDs, thread IDs, panel logs, and pending request IDs remain valid.
- Existing canonical workspace directories remain in place.
- Session working roots are created lazily.
- Before starting any writer for a project, scan current-project sessions for a pending journal and restore its review owner first.
- The single-writer invariant means a valid project can restore at most one pending writer; conflicting legacy records must surface an explicit recovery error instead of choosing silently.
- In-flight read turns and write-queue tickets are process-local and cancel on application exit.
- A journaled write survives exit and restores as review.
- A session that was interrupted before any mutation returns to idle with an interruption log entry.

Current unrelated staged and unstaged repository changes must be preserved. Implementation must use surgical edits and must not reset, overwrite, or clean the worktree.

## Verification Plan

### Rust behavioral tests

Add deterministic barrier-based tests for:

1. Session A remains in `run_turn` while session B enters and completes a read-only turn.
2. Interleaved A/B progress, tool, answer, plan, and error events carry the correct immutable session IDs.
3. A and B keep independent request IDs, evidence flags, mutation counts, build budgets, pending plans, and preflight suppression state.
4. A owns the write lease while B continues read tools.
5. B reaches `waiting_write` and cannot invoke any mutation while A owns or reviews.
6. FIFO queue order is based on write-intent arrival.
7. Accepting or fully rejecting A grants B and automatically resumes B's thread in write mode.
8. Partial decisions, rollback failure, or undecided items keep A as owner.
9. Cancelling a read-only A does not affect B.
10. Cancelling a queued writer removes only its ticket.
11. Cancelling an active writer with journal entries retains review ownership.
12. Restart recovery restores a pending review before admitting a new writer.
13. Read-mode sandbox configuration denies native workspace writes.
14. Only a lease owner receives the write-mode sandbox configuration.
15. Session source snapshots remain stable while another session refreshes or writes.
16. Workspace accept promotes the selected content; reject leaves canonical content byte-identical.
17. Approved plans survive implementation rejection and remain immutable.

### Panel tests

Add tests proving:

1. A and B `chat` invocations overlap; B does not wait for A's promise.
2. Two rows may simultaneously show read activity.
3. A review row does not disable another session's read request.
4. Only a writer displays `쓰기 대기 N`.
5. Queue-position events update all affected rows.
6. Cancelling a queued writer leaves the owner and other waiters unchanged.
7. Interleaved events update only their addressed `PanelStore`.
8. Sidebar selection does not alter backend activity.
9. Long names, collapsed rail behavior, splitter behavior, and horizontal clipping remain unchanged.

### Smoke scenario

Run a mock-Tauri browser scenario with five sessions:

1. A starts a long read-only analysis.
2. B starts a short read-only query and completes before A.
3. C starts a change, obtains the lease, writes, builds, and reaches review.
4. D completes a read-only query while C remains in review.
5. E requests a change and displays `쓰기 대기 1`.
6. Reject C and verify live-editor and canonical workspace state return to the exact pre-C state.
7. Verify E automatically enters write mode only after C's rollback completes.
8. Cancel A and verify E remains unaffected.
9. Restart with E's changeset pending and verify it reacquires review ownership before another writer can start.

### Required commands

Run the task-specific tests first, then the repository verification contract:

```text
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cd panel && npx tsc -b --noEmit
cd panel && npx vitest run
cd panel && npm run build
```

The UI change is complete only after browser-driving the actual mock-Tauri surface and observing the concurrent/read, waiting-write, running-write, and review states.

## Acceptance Criteria

The change is complete only when all of the following are observable:

- A session no longer waits for another session's entire Codex turn to finish before starting.
- Read-only sessions can overlap and complete independently.
- Exactly one writer exists per project.
- A writer's build and rollback cannot observe or overwrite another session's mutations.
- Review blocks later writes but does not block unrelated read-only conversations.
- Queue labels describe write contention, not conversation scheduling.
- Cancellation and events are strictly session scoped.
- Existing saved sessions and pending changesets restore without destructive migration.
- Canonical workspace content changes only through accepted workspace decisions, except for the separately trusted approved-plan snapshot.
- All targeted concurrency tests, full Rust/panel verification, and the browser smoke scenario pass.

## Implemented-result notes

- The recovery/coordinator project key reuses durable `SessionMeta.project`, because schema v1
  persists the editor project name and Phase 7 forbids destructive session migration. Canonical
  and session workspace roots still use the EPSNAPSHOT identity hash. This matches the supported
  single-current-editor-project topology and preserves every existing session record.
- Partial accept now promotes and removes only the accepted journal entries immediately. The old
  “undecided defaults to accepted on the next request” behavior was removed: it could not express
  a lease-safe partial review. Remaining entries retain review ownership.
- Panel-owned `memory_save` and `wiki_save` use short synthetic write tickets. They do not create
  agent changesets, but they cannot interleave with a live agent transaction.
- Canonical workspace promotion and trusted metadata persistence are rollback-coupled; a metadata
  failure restores the pre-promotion canonical bytes and retains the lease.

## Non-Goals

- Parallel writes to different files within one project.
- Cross-project execution while the editor exposes only one current project.
- Persisting unfinished read turns or write-queue tickets across application exit.
- Coordinating manual edits performed directly in EUD Editor 3 outside the app.
- Adding a user-configurable concurrency limit before measured resource pressure demonstrates a need.
- Multiplexing multiple session event streams through one `CodexAppServerClient`; isolated per-session clients are the safer initial implementation.
