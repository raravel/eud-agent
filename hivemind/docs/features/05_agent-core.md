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

## Context compaction and 1M opt-in

An exact `/compact` command is session maintenance, not a chat turn. The panel invokes
`compact{sessionId}` without adding `/compact` to model-visible or panel conversation history.
The session worker keeps its normal same-session serialization, sends app-server
`thread/compact/start{threadId}`, and does not resolve the command until the matching
`contextCompaction` item completes. Native Codex compaction retains Codex's instruction and
recent-task semantics; backend-owned plan, review, workspace, and journal state are unchanged.
Codex's normal threshold-driven compaction remains enabled, and its started/completed items become
Korean progress lines rather than raw event names.

## Instruction epochs and active task state

Each saved session owns a defaulted `SessionContextState` and `SessionTaskState`; adding them does
not bump the destructive global session schema. A cold/fresh thread receives one full static
instruction baseline plus current project state, memory, wiki, references, the complete active-task
projection, optional replay transcript, resolved mentions, and the current user message. A normal
follow-up sends only current project state, changed memory/wiki replacement sections, the
undelivered task-state delta, resolved mentions, and the current message. The successful-delivery
cursor advances only after the primary Codex turn completes. Cancellation or transport failure
therefore retries the same epoch/revisions.

The static baseline has a SHA-256 fingerprint. A changed fingerprint resets the provider thread
without deleting panel history or task events, increments `instructionEpoch`, and starts fresh with
the condensed transcript. Rewind, resume fallback, and successful native compaction also reset the
delivery cursor; compaction records a `CompactionBoundary` and the next turn resends the current
full baseline/projection once.

Active task state is an append-only parent-linked event graph with a derived bounded typed
projection. Stable panel-generated `clientTurnId` values anchor user log rows, IPC requests, and
task events. Rewind moves only the branch leaf, replays the projection, and retains abandoned events
for audit; a legacy prefix without an anchor fails closed to an empty projection.

After an answer or plan is visible (and after a changeset is visible for implementation turns), a
separate fresh Codex thread may compile one strict-schema `TaskStateDelta`. That compiler has no
MCP endpoint, tools are forbidden, session persistence is disabled, and input/output sizes are
bounded. Rust validates base revision, state transitions, projection bounds, exact user/approved
plan quotes, confined project-relative artifact paths, current file hashes, and journal
provenance before appending. Failure preserves the foreground result and prior projection, appends
a bounded reason code plus an optional bounded diagnostic detail (including captured app-server
stderr/exit context when available), logs the correlated session/request/turn identifiers, and
emits the generic `task_state_warning`. The projection is background context: it grants no
editor/map/workspace/journal/tool authority.

`config.json.codex_large_context_models` is a sorted set of model slugs selected in
**Settings → Codex**. Before every turn, each production driver reloads the global model pair and
this set. An enabled active model receives thread start/resume config
`model_context_window=1000000` and `model_auto_compact_token_limit=900000`; disabling it omits both
overrides on the next resume. Capability is not hard-coded because the authenticated catalog can
change independently of the app. Codex clamps requested windows to the catalog maximum and reports
95% of that window as effective; the current 1M-capable catalog cap of 872,000 therefore reports
828,400. An effective value below 828,400 logs one fallback warning per model and continues on
the Codex-reported window.

## Read and write execution modes

Every Codex turn has explicit `WorkspaceAccess::Read` or `WorkspaceAccess::Write`.

- Read turns use `eud_workspace_read`: minimal runtime reads, the session workspace read-only,
  and sandboxed command network disabled.
- Implementation turns use `eud_workspace_write`, but project documents and `source/**` remain
  read-only; only `.tmp/**` is writable for Code Mode. Live editor/map mutations use eud-tools.
- Both require the elevated exact-root Windows sandbox. Unsupported or denied setup fails closed.
- Codex hosted web search is explicitly `live` at app-server launch and in every fresh/resumed
  thread. It does not grant sandboxed commands network access and has no app-added usage policy.
- App-server command approvals are automatic in both modes so native commands and Code Mode
  JavaScript run inside the active profile. File-change, patch, and permission-expansion
  approvals remain denied; neither command approval widens filesystem or network access.
- Switching mode respawns the session's app-server when necessary, retains the thread id, and
  resumes the same conversation.

All initial chats and plan-feedback turns start in read mode. A direct edit calls
`request_write_lane(reason)`, which records write intent and parks the turn. `plan_approve`
submits the same request directly in the backend. Mutating tools called without ownership return
`WriteLeaseRequired`; neither foreground mode permits native project-document writes.

## Project write transaction

`ProjectWriteCoordinator` registers concurrent session writes immediately and serializes only
short shared-state transactions. Session-local reasoning, isolated source changes, and review do
not hold the project transaction mutex. Mutating eud-tools, build calls, canonical promotion,
rollback, panel memory save, and wiki save each acquire it only while their operation settles.

Backend activities are `idle`, `running_read`, `waiting_input`, `running_write`, `review`, and
`error`. Background harness state is orthogonal and never changes the session activity or disables
chat.

## Tool state and MCP

`ToolServices` shares immutable/app-wide services: data directories, journal store, RAG, map
rails, analyzer, and write coordinator. Each `SessionToolRuntime` separately owns:

- current request/project id;
- evidence, mutation, action, search, and build budgets;
- pending plan;
- epScript preflight snapshot/suppression state;
- write ticket and execution lock;
- one pending structured ASK request and its same-turn response channel.

Each worker hosts its own `127.0.0.1` streamable-HTTP MCP endpoint and shuts it down when the
worker is discarded. No mutable global request pointer identifies MCP callers.

Read tools: `project_status`, `list_files`, `read_file`, `eps_check`, `dat_get`, `xdat_get`,
`tbl_get`, `req_get`, `btn_get`, `settings_get`, `plugins_list`, `map_info`, `map_minimap`,
`search_docs`. The DAT/XDAT/TBL/REQ/BTN getters require a non-empty `items` array, execute the
items sequentially inside one runtime call, preserve input order, and return a per-item
`ok`/value-or-error result with the identifying coordinates echoed.

`project_status` returns `{status, mainFile}`. `status` remains the trimmed raw `STATUS` reply;
`mainFile` is the exact project-relative path from the typed `BridgeIo::get_main` wrapper, or JSON
`null` for an empty/unset result and the expected no-project state. Unexpected bridge,
transport, and timeout failures remain visible. The tool stays read-only and requires no write
registration; `list_files` separately owns path/type/settable metadata. `set_main` uses the same
wrapper to journal its prior value.

Flow tools: `ask(questions)`, `propose_plan(markdown)`, `request_write_lane(reason)`.

Write tools: `dat_set`, `xdat_set`, `tbl_set`, `req_set`, `btn_set`, `dat_reset`, `file_create`,
`file_write`, `file_edit`, `file_rename`, `file_delete`, `file_move`, `mkdir`, `set_main`,
`settings_set`, `plugin_add`, `plugin_edit`, `plugin_remove`, `plugin_move`, `build_run`,
`location_write`, `player_setup`, and `switch_write`. Project memory is synchronized only by the
post-acceptance harness; `memory_write` is not exposed to foreground Codex.


`ask` accepts one to four related questions. Each question has a stable id, optional header,
optional 2-5 choices, and a `multi` flag; the panel always exposes direct input. The tool call
uses a standard MCP form elicitation so Codex pauses its MCP active-time deadline while the user
is answering. The app-server callback registers one owner-request-scoped pending ASK, emits a
session-scoped `ask` event, sets activity to `waiting_input`, and awaits
`ask_response{sessionId,requestId,answers}` without holding the session engine mutex. The
session-restore fallback deadline is paused by the same pending-ASK state. ASK therefore has no
wall-clock response timeout: only a valid response, explicit turn cancellation, or teardown of
the owning turn can end it. A valid response resolves the original MCP call, restores read/write
activity, and lets Codex continue the same turn. A dropped ASK future removes its pending slot
through an RAII lease; turn cancellation resolves it as cancelled. Because Tauri events are
ephemeral notifications rather than delivery acknowledgments, `ask_pending{sessionId}` returns the
backend-authoritative pending snapshot. The main and Map panels query it only after installing
listeners and restore the matching ASK idempotently. Missing/duplicate/oversized questions and
incomplete or cardinality-invalid answers fail validation.
The live `ask` event also triggers the owning main or Map panel's configured sound/OS attention
notification once per request; restoring an existing `ask_pending` snapshot does not replay it.
`file_edit` applies a non-empty ordered list of exact, uniquely matching `old_text`/`new_text`
replacements to the session baseline, then uses the same non-overlapping live-change merge and
full before/after journal snapshots as `file_write`.

## epScript project placement policy

Cold-start and resumed turns both receive the canonical `[eps project architecture]` guide. The
configured `project_status.mainFile` is the composition root regardless of filename; the agent
must not infer one or use `set_main` for name normalization. Behavior stays with the module that
owns its mutable state and invariants, while a new module requires a cohesive responsibility and
narrow API. Imports flow `configured MainFile -> feature modules -> stable leaf modules` without
new cycles. Local fixes do not trigger unrelated moves or splits, and 800 nonblank lines is only a
cohesion review signal. Structural role/dependency changes replace memory `structure` completely;
mutually dependent candidates use one `eps_check` batch and every applied epScript changeset still
requires `build_run`.

Evidence, first-principles, mutation-count, action-count, search, and three-build-attempt rails
remain request scoped. The non-search action hard ceiling is 300 calls; each batched getter
envelope consumes one action. `request_write_lane` is non-mutating and consumes no mutation budget.

## Main resource mention transport

Main EPS `ChatRequest` and `PlanFeedbackRequest` carry an optional ordered
`Vec<MentionInstance>` beside text and attachments. `MentionService` is shared through
`ToolServices`; `mention_search` is trusted panel IPC and is not present in the model-facing MCP
registry. Its closed versioned union currently implements only `map.region` and `map.location`.

Before request state begins or any Codex driver call, the engine validates the complete batch
against a fresh saved `OpenMapName` context. Regions come from the exact project's persistent
selection library through the narrow `CandidateStore::persistent_selections` boundary and bind
source hash, dimensions, id, and complete persistent-selection hash. Locations come only from the
saved CHK `MRGN` digest and bind source hash, exact id, and complete decoded-record fingerprint.
Any stale/invalid/unsupported instance, duplicate instance id, or count above 16 rejects the
complete turn. Text/attachment-only turns do not resolve a map context and retain their prior
prompt behavior.

Valid context is projected deterministically as compact `eud-resolved-mentions/1` JSON in a
`[resolved mentions]` section before `[user message]` on cold, resumed, resume-fallback, and
plan-feedback turns. Mention-only chat and feedback use stable backend fallback instructions.
Visible labels are presentation only. Mentions do not authorize tools or change any existing
evidence, plan, write, action, build, journal, changeset, or rollback state.

## Session workspaces

The canonical accepted project workspace remains:

`%appdata%\eud-agent\workspaces\<project-id>\`

Codex runs from:

`%appdata%\eud-agent\workspaces\.sessions\<project-id>\<session-id>\`

Before every turn, accepted canonical documents are delta-synced and a coherent session-owned
`source/` snapshot is refreshed. Foreground documents remain read-only. The app-owned approved
plan is written canonical before implementation, synced into the session root, remains immutable,
and survives implementation rejection.

After a code changeset settles with accepted entries, `HarnessJobStore` persists one job under
`%appdata%\eud-agent\harness_jobs\`. Runtime-sensitive jobs wait for user confirmation. The user
may skip them into a terminal state that produces no model call or durable updates. Other jobs run
immediately on a dedicated document workspace
`workspaces/.sessions/<project-id>/<harness-job-id>/`. One tool-free, output-schema-constrained
Codex turn returns ordered exact document edits and optional durable-memory replacements. Each
`old_text` must match exactly once after earlier edits in the same patch have been applied. Rust
validates and applies the batch, writes the worklog from accepted journal/build evidence, journals
the document workspace, and emits a separate atomic review. Accept promotes documents and memory;
reject discards the job workspace. A failed attempt retains accepted code, its validation error,
and the rejected structured delta when parsing succeeded. Retry feeds that context into a fresh
generation instead of repeating the original prompt.
Terminal failed/completed/rejected/skipped jobs may be durably dismissed without deleting their
audit record or changing accepted state.

Every project mutation is journaled before review. Partial code decisions retain only undecided
entries; accepted entry snapshots accumulate in the pending harness job context. Harness reviews
are independent of `AgentEngine.current_request_id`, so new conversations and code reviews may
continue.

Schema v3 performs a clean legacy cutover: it preserves session names and panel logs, clears Codex
thread/context/pending ownership, removes unaccepted journals and session workspaces, preserves
accepted journals/canonical documents, and starts the new harness state machine empty.

## Event and cancellation isolation

`SessionEventSink` is constructed with one immutable session id. `agent_event`, `context_usage`,
`ask`, `answer`, `plan`, `changeset`, `rollback_result`, turn `progress`, and turn `error` always
carry that id. Global project/setup/bootstrap/RAG events remain unscoped. There is no
`session_active` routing event or fallback to the selected panel row.

`cancel {sessionId}` advances only that worker's cancellation generation, cancels that session's
pending ASK, or removes only its waiting write ticket. A writer with journal entries transitions
to review and retains ownership.

## Verification

- Barrier-based Rust tests prove overlapping different-session reads and same-session
  serialization.
- Coordinator tests prove concurrent registrations and short per-project transactions.
- Runtime tests prove request/evidence/budget/preflight isolation.
- Harness tests prove runtime classification, structured-delta validation, deterministic worklog
  staging, interrupted-job recovery, foreground document-repair removal, and schema-v3 cutover.
- Context/task-state tests cover full-versus-delta delivery, fingerprint/compaction resets,
  deterministic replay and cache repair, stable rewind anchors, tools-disabled compiler
  success/failure/timeout, provenance confinement, and cross-store atomic updates.
- ASK tests prove one blocking MCP call emits one session-scoped related-question request,
  rejects incomplete answers without consuming it, returns mixed choice/direct answers to the
  same call, releases the session slot when the MCP future is dropped, and restores a pending
  snapshot when the panel missed the push event.
- Panel integration tests prove harness event routing, runtime confirmation, retry, atomic
  document review, and chat availability while a harness job runs.
