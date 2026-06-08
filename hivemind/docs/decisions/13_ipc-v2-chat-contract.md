# Decision 13: IPC contract = v2 chat schema; Rust backend driven via codex app-server

- Date: 2026-06-09
- Status: Accepted
- Supersedes: [[decisions/11_panel-tauri-ipc]] (and the v1 instruct/apply IPC surface in feature 11/15, realized by EUD-110/113/114).
- Context: Decision 11 framed the Tauri migration as "the existing WS schema maps 1:1
  (instruct/apply/status/list -> invoke; progress/code/applied/error -> events)". That
  premise was wrong: the panel had ALREADY been migrated (Python-server era, features
  05/06; EUD-063/064/065/068/069/074) to a v2 chat protocol and NEVER used
  instruct/apply. The contradiction was found while planning EUD-119: the panel
  (panel/src/ws/protocol.ts, state/store.ts, App.tsx) speaks a chat-first plan/changeset
  protocol, while the completed Rust backend (src-tauri/src/ipc.rs + engine.rs,
  EUD-110/113/114) implements the v1 single-shot instruct->code->apply flow. The two
  cannot interoperate.
- Considered:
  - Re-plan to v2; panel chat schema is the source of truth; rebuild the Rust backend
    (IPC + agentic engine + journal/rollback) to expose the v2 chat surface — Pros: the
    panel's plan/changeset/review UX (already built + tested) works for real; matches
    features 05/06 behavioral source. Cons: supersedes three "done" tasks; large port.
    Recommendation: ★★★ — the panel is the user-facing truth and is more expensive to
    rebuild than the placeholder backend.
  - Revert the panel to v1 instruct/apply — Pros: smallest backend change. Cons: discards
    the entire chat/plan/changeset UI and features 05/06; contradicts decision 11's
    "rendering untouched". Recommendation: ★☆☆.
- Chosen: Re-plan to v2 (panel is source of truth); full agentic Rust backend rebuild.
- Codex drive mechanism: the agentic turn loop requires a PERSISTENT codex thread
  (start/resume), streamed JSONL events, in-process eud-tools as an MCP server, and an
  approval handler — none of which the single-shot `codex exec` subprocess supports. The
  Rust backend therefore drives codex via the **codex app-server JSON-RPC protocol over
  stdio** (the same protocol the Python `openai-codex` SDK wrapped in v2). All the
  measured app-server quirks carry over (codex app-server quirks memory, features 05):
  `skills.include_instructions=false`; raw `approvalPolicy:"on-request"` with a handler
  that ACCEPTS only the eud-tools MCP server (`mcpServer/elicitation/request` ->
  `{"action":"accept"}`) and declines shell/patch; `model_supports_reasoning_summaries=
  true` + `model_reasoning_summary="detailed"`. The per-turn `codex exec` path in
  codex_client.rs is retired for the agentic flow.
- Rationale: the panel v2 protocol is the contract; the backend was the side written
  against a stale premise. The app-server mechanism is the only viable way to run the
  agentic plan/changeset loop and is already validated by the v2 Python E2E.
- Impact: feature 11 (rust backend core — IPC surface + engine + codex_client + new
  journal), feature 15 (panel transport — v2 chat mapping), decision 11 (superseded),
  tasks: new backend rebuild tasks supersede EUD-110/113/114; EUD-119 redefined to the v2
  contract with expanded scope (App.tsx + ws/ removal); EUD-120 unchanged (depends on 119).
