---
created: '2026-06-09'
depends_on: []
id: EUD-123-bab1
parent: EUD-094-e9ee
priority: high
scope:
- src-tauri/src/codex_client.rs
status: pending
title: Codex app-server JSON-RPC transport (thread start/resume, streaming, approvals)
type: task
updated: '2026-06-09'
---

## Description
Replace the single-shot `codex exec` path in `codex_client.rs` with a codex app-server JSON-RPC
client over stdio (decision 13): start a thread (system prompt as base_instructions), resume it on
subsequent turns, and forward streamed JSONL events to the engine as a typed event stream. Apply the
measured app-server config (codex app-server quirks): `skills.include_instructions=false`; raw
`approvalPolicy:"on-request"` with a handler that ACCEPTS only the eud-tools MCP server
(`mcpServer/elicitation/request` -> `{"action":"accept"}`) and DECLINES shell/patch/file-change;
`model_supports_reasoning_summaries=true` + `model_reasoning_summary="detailed"`. Keep codex `.cmd`
resolution via `which` (+ `CODEX_CMD`), `--skip-git-repo-check`, explicit piped stdio, stable cwd.

## Spec References
- [[features/11_rust-backend-core|11_rust-backend-core]] `../docs/features/11_rust-backend-core.md` — engine / app-server config / codex_client
- [[features/05_agent-core|05_agent-core]] `../docs/features/05_agent-core.md` — thread lifecycle, approvals, reasoning
- [[decisions/13_ipc-v2-chat-contract|13_ipc-v2-chat-contract]] `../docs/decisions/13_ipc-v2-chat-contract.md`

## Completion Criteria
- [ ] app-server JSON-RPC framing (thread start/resume + event stream) over piped stdio; tested against a stub server
- [ ] approval handler accepts eud-tools only and declines shell/patch; reasoning + skills overrides set
- [ ] `cargo test` + `cargo clippy --workspace --all-targets -- -D warnings` pass