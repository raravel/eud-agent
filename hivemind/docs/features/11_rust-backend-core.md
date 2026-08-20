# Feature 11: Rust backend core (agentic v2 — IPC, engine, codex app-server, tools, journal, bridge_io, memory)

Ports the Python v2 agent core (features 05/06) into the in-process Rust core, exposed over
Tauri IPC. The backend is the policy layer: an agentic codex turn loop, tool validation, a
change journal with rollback, plan gating, budgets, and file-IPC to the editor. Behavior
matches the v1 features 02/05/06 (kept as behavioral source).

> Decision: see [[decisions/08_tauri-rust-rewrite]] and [[decisions/13_ipc-v2-chat-contract]]
> (the v1 single-shot instruct/apply surface is superseded by the v2 chat schema).

## Tauri IPC surface

Conversation commands always include `sessionId`: `chat`, `plan_feedback`, `plan_approve`,
`changeset_decision`, `cancel`, and `conversation_rewind`. `session_open` hydrates one worker and
reconnects its pending changeset; selection uses read-only `session_load`.

Turn commands for different sessions may remain pending concurrently. The same worker mutex
serializes commands within one session. `cancel` advances only that worker's cancellation
generation or removes only its waiting write ticket.

Conversation events always carry immutable `sessionId`: `agent_event`, `answer`, `plan`,
`changeset`, `rollback_result`, turn `progress`, and turn `error`.
`session_activity{sessionId,activity,queuePosition?,blockingSessionId?}` reports:

```text
idle | running_read | waiting_write | running_write | review | error
```

`queuePosition` and `blockingSessionId` are present only for `waiting_write`, identifying the
FIFO position and current write owner. Project/app events (`status`, `list`, memory/wiki
snapshots, setup, bootstrap, RAG warmup) remain unscoped. `session_active` and selected-row
routing fallbacks are removed.

`memory_save` and `wiki_save` remain direct user-owned writes without an agent changeset, but they
acquire a short project write ticket so they cannot interleave with an agent transaction.
The v1 `instruct`/`apply`/`code`/`applied` surface remains removed.

## Session engine manager

`SessionEngineManager` replaces the single global engine mutex with lazy `SessionWorker`s. Each
worker owns one `AgentEngine`, `ProductionCodexDriver`, Codex app-server client/event receiver,
`SessionToolRuntime`, loopback MCP server, immutable `SessionEventSink`, and cancellation channel.
The worker mutex is only a same-session command sequencer.

Different sessions can run read-only Codex turns concurrently. Initial chat and plan feedback use
`WorkspaceAccess::Read`. Direct edits end the read turn with `request_write_lane(reason)`;
`plan_approve` registers intent in the backend. Grant resumes the same thread in
`WorkspaceAccess::Write`.

Two named elevated sandbox profiles are configured on every app-server spawn:

- `eud_workspace_read`: minimal runtime reads, current session root read-only, network off;
- `eud_workspace_write`: minimal runtime reads, current session root write,
  `source/**` read-only, network off.

Changing access or cwd respawns only that worker's client and retains its thread id. Exact-root
sandbox setup failure aborts the turn; no legacy fallback is permitted.

`ToolServices` shares immutable/app-wide journal, RAG, map, analyzer, data-dir, and coordinator
handles. `SessionToolRuntime` owns the request id, evidence/mutation/action/search/build counters,
pending plan, preflight request state, write ticket, and execution lock. Each worker has its own
ephemeral MCP endpoint and shutdown handle; no global current-request pointer exists.
The endpoint URL is registered at app-server launch and injected again in every
`thread/start` and `thread/resume` config. A restored Codex thread therefore cannot retain an
older tool-less MCP configuration.

Global Codex model settings are handled behind a separate short settings lock and temporary
app-server, not an arbitrary session mutex.

## Project write coordinator

`ProjectWriteCoordinator` serializes complete transactions by project. The FIFO timestamp is
write-intent registration. The lease spans latest-state rebase, mutation, mandatory build,
changeset review, and complete accept/reject work. It remains held for partial decisions,
undecided entries, rollback failure, or a cancelled writer with journaled changes.

A review blocks later writers only. Read-only tools and turns in other workers continue. On full
settlement, release grants the next ticket and the manager automatically resumes that session in
write mode.

Startup recovery scans project sessions before admitting writers. One valid pending journal is
restored as owner. Multiple pending writers, a missing journal, or an empty pending journal is an
explicit recovery error.

## Tools

Read: `project_status`, `list_files`, `read_file`, `eps_check`, `dat_get`, `xdat_get`, `tbl_get`,
`req_get`, `btn_get`, `settings_get`, `plugins_list`, `map_info`, `map_minimap`, `search_docs`.
The DAT/XDAT/TBL/REQ/BTN getters take non-empty `items` arrays and return ordered per-item
success/error results while consuming one action per tool envelope.

Flow: `propose_plan(markdown)`, `request_write_lane(reason)`.

Write: `dat_set`, `xdat_set`, `tbl_set`, `req_set`, `btn_set`, `dat_reset`, `file_create`,
`file_write`, `file_edit`, `file_rename`, `file_delete`, `file_move`, `mkdir`, `set_main`,
`settings_set`, `plugin_add`, `plugin_edit`, `plugin_remove`, `plugin_move`, `build_run`,
`location_write`, `player_setup`, `switch_write`, `memory_write`.

`file_edit` resolves ordered exact replacements against the request's latest desired content,
three-way merges that candidate with the trusted source baseline and live editor content, and
journals the resulting full before/after bytes as a normal file modification.

The runtime rejects every mutating tool, including build/map/memory writes, unless its exact
project/session/request ticket owns the lease. `request_write_lane` is non-mutating. Existing
validation, evidence, first-principles, plan-mutation, action/search, and build budgets remain
session-request scoped; the non-search action hard ceiling is 300.

`build_run` is the single public build-result tool. It reads `EDSPATH`, snapshots output-map
freshness, invokes the editor `BUILD`, waits up to 300 seconds, and consumes `BUILDERR`
internally for macro errors. A failed editor build with no macro errors triggers one direct
`euddraft.exe <eds>` re-run with captured stdout/stderr. The returned
`{ok, errors[{source,file,line,message,raw}]}` payload is complete; there is no separate
`build_errors` MCP tool.

## Change journal and rollback

All editor/map/memory/native-workspace mutations remain journaled with UTF-8 no-BOM persistence.
Workspace journal targets additionally carry the session id so reject restores the isolated
working root rather than accepted canonical bytes.

Partial accept promotes selected workspace bytes and removes only accepted journal entries.
Partial reject applies inverse operations and removes only rejected entries. Remaining entries
stay live and keep the project lease. Accept/reject-all archives only after promotion/rollback
completes. Failed rollback emits `rollback_result{ok:false}`, retains the journal, and leaves the
session in review.

Canonical workspace promotion and trusted metadata update are one parent-owned transaction:
promotion bytes are restored if metadata persistence fails.

## codex_client / app-server transport
The single-shot `codex exec` fenced-extraction path (codex_client.rs) is RETIRED for the
agentic flow (decision 13). codex resolution rules from rules.md still apply: resolve the
`.cmd` shim via `which` (honor `CODEX_CMD`), `--skip-git-repo-check`, explicit piped stdio,
stable cwd. The app-server is driven over stdin/stdout JSON-RPC (tokio piped stdio).
Unexpected stdout closure marks the client dead, reports bounded stderr and child exit status,
and interrupts the in-flight turn without replaying side effects. The next command respawns the
same worker endpoint and resumes its retained thread id automatically.

## bridge_io (file-IPC to editor)
Port of `bridge_io.py`: write `srv-<uuid8>.cmd` to `<editor>\Data\agent\inbox` (UTF-8 no
BOM), poll `outbox\<name>.result` with 10s timeout (180s when status.txt compiling=true, emit
`waiting_build`), delete consumed `.result`, clear stale inbox/outbox at startup.
Commands used by the build path include BUILD, BUILDERR, EDSPATH, and GETSET in addition to the normal project read/write command set.
The `status`/`list` Tauri commands are served BY this client (status.txt read / LIST
round-trip) — never placeholder constants. The editor install path comes from `config.json`
(config.rs DataDirs); an unset path or absent/stale editor heartbeat returns the friendly
"editor not connected" error, never a panic.

## Memory and wiki

Project memory and the DAT wiki remain under Roaming app data and retain their existing caps,
sanitization, prompt rendering, and acceptance-ledger semantics. Agent `memory_write` requires
the write lease. Panel `memory_save` and `wiki_save` execute as short synthetic coordinator
transactions and remain outside agent changeset review.

Every session reads fresh project memory/wiki context for its turn. Accepted DAT edits update the
global project wiki only after the changeset decision succeeds; rejection never records them.

## Edge cases
- codex CLI unresolved -> fast clear error (no bare spawn).
- editor not connected (stale/absent heartbeat) -> commands return a friendly error; panel
  shows "editor not connected".
- build in progress -> file-IPC extends timeout and emits `waiting_build`.

## Verification contract

- Barrier tests prove different session engines enter `run_turn` concurrently and same-session
  commands serialize.
- Session runtime tests prove request/evidence/budget/preflight isolation.
- Coordinator tests prove one owner, write-intent FIFO, waiting cancellation, queue updates,
  review retention, and pending-review priority.
- Workspace tests prove stable session snapshots, read/write roots, accepted promotion,
  canonical rejection invariance, and approved-plan preservation.
- Journal tests prove partial/failed decisions keep ownership.
- Full `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`.

## Implementation

- `src-tauri/src/engine.rs` — `SessionEngineManager`, session workers, read/write continuations,
  immutable sinks, recovery.
- `src-tauri/src/write_coordinator.rs` — project FIFO tickets and backend activity.
- `src-tauri/src/tool_exec.rs` — `ToolServices` and `SessionToolRuntime`.
- `src-tauri/src/mcp.rs` — per-worker loopback server and shutdown handle.
- `src-tauri/src/codex_client.rs` — session clients and named read/write sandbox profiles.
- `src-tauri/src/workspace.rs` — canonical/session roots, delta sync, baseline, promotion.
- `src-tauri/src/journal.rs` — session-owned workspace targets and partial decision settlement.
- `src-tauri/src/ipc.rs` / `lib.rs` — Tauri commands, direct-write coordination, manager wiring.
- [BOUND 2026-06-08 from EUD-113-ba2a] src-tauri/src/lib.rs — Tauri app shell; registers core modules (wires pub mod engine; for the orchestrator)
- [BOUND 2026-06-08 from EUD-113-ba2a] src-tauri/src/engine.rs — prompt assembly (build_system_prompt, resume_turn_text) REUSED; the single-shot run_instruct/unified_diff seam is SUPERSEDED by the agentic loop (decision 13)
