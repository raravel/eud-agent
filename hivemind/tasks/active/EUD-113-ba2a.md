---
created: '2026-06-08'
depends_on:
- EUD-110-def0
id: EUD-113-ba2a
parent: EUD-094-e9ee
priority: high
scope:
- src-tauri/src/engine.rs
status: pending
title: engine/orchestrator + prompt assembly + tool loop
type: task
updated: '2026-06-08'
---

## Description
Implement `src-tauri/src/engine.rs`: assemble the v2 system prompt ([first principles]
before [reference context], [evidence], [message format]), run RAG -> codex -> advisory
LSP -> unified diff (`similar`), and drive the agentic tool loop. Depends on IPC, codex,
rag. (Also depends on tools layer EUD-114 for the loop.)

## Spec References
- [[features/11_rust-backend-core|11_rust-backend-core]] `../docs/features/11_rust-backend-core.md` - orchestrator
- [[features/05_agent-core|05_agent-core]] `../docs/features/05_agent-core.md` - tool loop (behavioral source)
- [[rules]] `../docs/rules.md` - system prompt / message format / evidence

## Completion Criteria
- [ ] instruct produces code+diff; [first principles] precedes [reference context]
- [ ] [user message] header applied on resumed turns (EUD-092 behavior)
- [ ] `cargo test engine` passes