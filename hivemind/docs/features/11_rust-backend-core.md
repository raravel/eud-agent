# Feature 11: Rust backend core (agentic v2 — IPC, engine, codex app-server, tools, journal, bridge_io, memory)

Ports the Python v2 agent core (features 05/06) into the in-process Rust core, exposed over
Tauri IPC. The backend is the policy layer: an agentic codex turn loop, tool validation, a
change journal with rollback, plan gating, budgets, and file-IPC to the editor. Behavior
matches the v1 features 02/05/06 (kept as behavioral source).

> Decision: see [[decisions/08_tauri-rust-rewrite]] and [[decisions/13_ipc-v2-chat-contract]]
> (the v1 single-shot instruct/apply surface is superseded by the v2 chat schema).

## Tauri IPC surface (v2 chat schema — replaces the WebSocket protocol)
Commands (panel -> core, `invoke`). These start background work and resolve on accept; the
turn result arrives as events:
- `chat { text }` — start/resume an agentic turn (the agent picks files/targets itself).
- `plan_feedback { text }` — iterate the current plan (resumes the codex thread).
- `plan_approve {}` — approve the plan; lifts the per-request mutation gate; resumes.
- `changeset_decision { decision: "accept"|"reject", ids: "all"|string[] }` — accept/reject
  journaled items; runs as a background rollback task (EUD-070).
- `cancel {}` — interrupt the in-flight turn (journal entries persist).
- `reset {}` — drop the retained codex thread; next `chat` starts a fresh conversation (EUD-064).
- `status {}` -> `{ compiling, project }` (read from editor status.txt).
- `list {}` -> `{ files: [{ path, ftype, settable }] }` (bridge LIST).
- `memory_get {}` -> `{ project, files: { resources, structure, conventions, lessons },
  episodes }` (episodes: last 50, newest first). Error when no project is open (STATUS
  project empty). Request/response: the payload is the command return value.
- `memory_save { file, content }` -> `{ file }` — panel edit of one memory file (same file
  enum + 8 KB cap as `memory_write`); writes directly via the store, NO journal entry (a
  user editing their own memory is not an agent mutation). Error: no project open / unknown
  file / oversize content.

Events (core -> panel, `emit`):
- `agent_event { kind, detail, data? }` — streamed turn activity. `reasoning` = reasoning
  delta in `detail`; `delta` = answer-text delta; `tool_call`/`tool_result` carry `data`
  ({args} / {result,status}); thinking/turn_done/item_* are internal (panel shows no raw kind).
- `answer { text }` — answer-only turn (no edits).
- `plan { markdown, revision }` — propose_plan ended the turn; higher revision replaces.
- `changeset { request_id, items[] }` — journaled writes awaiting accept/reject (dat grouped
  per objId with property/old/new; file items kind created|modified|deleted + unified diff).
- `rollback_result { ids, ok }` — outcome of a changeset_decision.
- `progress { stage: rag|rag_warmup|codex|lsp|waiting_build|bootstrap, detail, pct? }`
- `error { message }`

The v1 `instruct`/`apply`/`code`/`applied` command+event surface is REMOVED (decision 13).

## Engine (agentic turn loop — single path)
Mirrors v1 `engine.py`/`agent_runner.py`. A small deterministic state machine driven by IPC
commands: `idle -> triage -> answer | plan_review* -> executing -> changeset_review -> idle`.
- **Codex drive = codex app-server (JSON-RPC over stdio)** (decision 13). The first `chat`
  of a session starts a codex thread (system prompt as base_instructions); EVERY subsequent
  `chat` RESUMES the same thread so codex retains its message + tool-call history (EUD-064).
  `reset{}` / a fresh session drops the thread id. Resumed turns PREPEND refreshed
  `[project state]` + project memory + `[reference context]` (RAG for the new question)
  before a `[user message]` header (EUD-092). System prompt assembly already exists
  (`build_system_prompt`/`resume_turn_text` in engine.rs) and is reused.
- **App-server config (measured quirks, decision 13 / codex app-server quirks memory):**
  `skills.include_instructions=false`; raw `approvalPolicy:"on-request"` with an approval
  handler that ACCEPTS only the eud-tools MCP server (`mcpServer/elicitation/request` with
  `_meta.codex_approval_kind=="mcp_tool_call"` -> `{"action":"accept"}`) and DECLINES shell/
  patch/file-change approvals; `model_supports_reasoning_summaries=true` +
  `model_reasoning_summary="detailed"`. Streamed JSONL events are forwarded as `agent_event`s.
- **eud-tools as an MCP server**: codex attaches an MCP server that exposes the tool registry.
  codex's MCP transport accepts only `command` (stdio) or `url` (HTTP), so attaching an
  in-process Rust server directly is NOT possible (decision A2). The app instead hosts a
  **127.0.0.1-only streamable-HTTP MCP server** (rmcp, ephemeral port) and registers codex
  with `-c mcp_servers.eud-tools.url="http://127.0.0.1:<port>/mcp"`. The server still runs IN
  the app process, so the MCP handler shares the live tool runtime (request state, journal,
  bridge, RAG, mapsafe) directly — the loopback HTTP is only codex's required transport, not
  an out-of-process shim. rules.md's "panel ↔ core is Tauri IPC only — NO localhost socket"
  bounds the PANEL boundary and does not apply to this codex ↔ core channel; the bind is
  loopback-only and no bearer token is layered on. MCP result content blocks are plain dicts.
- **Triage + plan gating (mechanical):** answer-only requests use no write tools; <=2
  mutations may apply directly; the 3rd mutating call WITHOUT an approved plan returns a tool
  error directing codex to `propose_plan`. `plan_approve` lifts the gate for that request.
  `propose_plan(markdown)` ends the turn -> `plan` event.
- **Request scoping (EUD-064):** each `chat` mints a fresh `request_id` (journal/changeset
  scope, mutation gate, budget are PER-REQUEST); only the codex thread persists. The live
  request id is resolved at tool-call time.
- **Budgets:** 30 tool actions per request; 3 build self-fix attempts.

## Tools (registry, MCP-exposed)
Read: `project_status`, `list_files`, `read_file`, `dat_get`, `xdat_get`, `tbl_get`,
`req_get`, `btn_get`, `settings_get`, `plugins_list`, `build_errors`, `search_docs` (RAG
top-k; Korean query; k clamped to 10; evidence-gate trigger).
Write (journaled): `dat_set`, `xdat_set`, `tbl_set`, `req_set`, `btn_set`, `dat_reset`,
`file_create`, `file_write`, `file_rename`, `file_delete`, `file_move`, `mkdir`, `set_main`,
`settings_set`, `plugin_add`, `plugin_edit`, `plugin_remove`, `plugin_move`, `build_run`,
`location_write` (features/09), `player_setup` (EUD-089).
Flow: `propose_plan(markdown)`.
Every tool validates args server-side (numeric ranges, index bounds, type whitelists,
FileType guards) BEFORE the bridge call. The evidence gate (EUD-090) rejects mutating calls
with `EvidenceRequired` until one `search_docs` has run in the request (zero hits lift it;
items marked 근거 없음 — never a fabricated source). Exempt: `memory_write`, `build_run`.
The btn/xdat first-principles rails (rules.md) are enforced here. The EUD-114 tool layer
(evidence gate, first_principles, btn/xdat rails) is the foundation; this feature extends it
with MCP exposure, the full registry, the mutation gate, budgets, and journal integration.

## Change journal and rollback
- Every write tool snapshots BEFORE mutating (dat/xdat/tbl/req/btn old value + was_default;
  file_write old content; file_create/mkdir created marker; file_delete full content+position;
  rename/move old path; set_main old path; settings/plugins old value/Texts/index).
- Entries `{id, seq, tool, target, before, after, ts}` accumulate per request; persisted as
  JSON to `%appdata%\eud-agent\journal\<request-id>.json` (UTF-8 no BOM) so a crash cannot
  strand un-reviewable changes.
- On turn completion emit `changeset{request_id, items[]}` (dat grouped per objId; file items
  with kind + server-side unified diff for modified).
- `changeset_decision{reject, ids|all}` -> inverse ops via the bridge in reverse seq order;
  `accept` -> journal archived. Mixed per-item decisions; undecided items default-accept on
  the next request. Decisions run as a BACKGROUND task (EUD-070); one at a time.

## codex_client / app-server transport
The single-shot `codex exec` fenced-extraction path (codex_client.rs) is RETIRED for the
agentic flow (decision 13). codex resolution rules from rules.md still apply: resolve the
`.cmd` shim via `which` (honor `CODEX_CMD`), `--skip-git-repo-check`, explicit piped stdio,
stable cwd. The app-server is driven over stdin/stdout JSON-RPC (tokio piped stdio).

## bridge_io (file-IPC to editor)
Port of `bridge_io.py`: write `srv-<uuid8>.cmd` to `<editor>\Data\agent\inbox` (UTF-8 no
BOM), poll `outbox\<name>.result` with 10s timeout (180s when status.txt compiling=true, emit
`waiting_build`), delete consumed `.result`, clear stale inbox/outbox at startup.
Commands: PING/STATUS/LIST/GET/SET/NEWEPS/GETDAT/SETDAT/BUILD/LUA.
The `status`/`list` Tauri commands are served BY this client (status.txt read / LIST
round-trip) — never placeholder constants. The editor install path comes from `config.json`
(config.rs DataDirs); an unset path or absent/stale editor heartbeat returns the friendly
"editor not connected" error, never a panic.

## memory
Port of `memory.py` (semantics: [[features/07_project-memory|07_project-memory]] — files,
caps, staleness, episodes; that spec's WS/server-path sections are superseded by THIS
feature's Tauri surface): project memory under `%appdata%\eud-agent\memory\<sanitized>\`;
`memory_write` is evidence-gate-exempt. v2 wiring contract:
- The project name is resolved per turn from bridge STATUS (the same fetch that feeds
  `[project state]`); an empty project disables the store for that turn.
- `build_system_prompt` / `resume_turn_text` receive a freshly rendered
  `ProjectMemory::render_section(list_reply)` EVERY turn (first + resumed) — never a
  construction-time constant.
- The engine appends one episode line at each request finalization point (changeset decided /
  default-accept on next chat / answer-only turn end), per feature 07.
- The `memory_get`/`memory_save` commands (IPC surface above) are served by the same store.

## Edge cases
- codex CLI unresolved -> fast clear error (no bare spawn).
- editor not connected (stale/absent heartbeat) -> commands return a friendly error; panel
  shows "editor not connected".
- build in progress -> file-IPC extends timeout and emits `waiting_build`.

## Verification contract
- Unit: IPC v2 payload (de)serialization (serde round-trip for every command/event); tool
  validation (bounds/whitelists/guards); journal inverse-op correctness per tool kind
  (snapshot->rollback round-trips against a fake bridge); mutation gate (3rd write without
  plan -> error); evidence gate; budgets.
- Integration: fake-bridge IPC responder driving chat -> changeset -> reject-single -> verify
  inverse .cmd sequence. App-server JSON-RPC framing test with a stub server.
- `cargo test --workspace` + `cargo clippy --workspace --all-targets -- -D warnings`.

## Implementation
- `src-tauri/src/ipc.rs` — Tauri command handlers + event emit helpers (v2 chat surface;
  the v1 instruct/apply handlers in EUD-110 are replaced)
- `src-tauri/src/engine.rs` — agentic turn-loop state machine + prompt assembly (extends the
  EUD-113 prompt seam; the single-shot run_instruct/InstructOutput path is retired)
- `src-tauri/src/codex_client.rs` — codex app-server JSON-RPC transport (the EUD-113
  single-shot `codex exec`/extract_code path is retired)
- `src-tauri/src/tools.rs` — tool registry + MCP exposure (extends EUD-114 evidence gate /
  first_principles / btn-xdat rails)
- `src-tauri/src/journal.rs` — journal snapshots, persistence, inverse ops (NEW)
- `src-tauri/src/bridge_io.rs` — file-IPC client
- `src-tauri/src/memory.rs` — project memory
- `src-tauri/src/data/first_principles.md` — bundled prompt asset (ported)
- external: `tokio`, `which`, `similar`, `tauri-plugin-shell`, `serde_json`
- [BOUND 2026-06-08 from EUD-113-ba2a] src-tauri/src/lib.rs — Tauri app shell; registers core modules (wires pub mod engine; for the orchestrator)
- [BOUND 2026-06-08 from EUD-113-ba2a] src-tauri/src/engine.rs — prompt assembly (build_system_prompt, resume_turn_text) REUSED; the single-shot run_instruct/unified_diff seam is SUPERSEDED by the agentic loop (decision 13)
