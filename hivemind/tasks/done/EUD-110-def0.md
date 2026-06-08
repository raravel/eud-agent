---
completed_at: '2026-06-08T17:06:18.490837'
created: '2026-06-08'
depends_on:
- EUD-098-fe34
id: EUD-110-def0
parent: EUD-094-e9ee
priority: high
scope:
- src-tauri/src/ipc.rs
- src-tauri/src/lib.rs
status: done
title: Tauri IPC surface (commands + events)
type: task
updated: '2026-06-08'
---

## Description
Define the Tauri IPC surface: commands instruct/apply/status/list and events
progress/code/agent_event/applied/error (1:1 with the old WS schema). No localhost socket,
token, or Origin check.

## Spec References
- [[features/11_rust-backend-core|11_rust-backend-core]] `../docs/features/11_rust-backend-core.md` - IPC surface
- [[decisions/11_panel-tauri-ipc|11_panel-tauri-ipc]] `../docs/decisions/11_panel-tauri-ipc.md`

## Completion Criteria
- [ ] Commands + event emitters registered with the Tauri builder
- [ ] Payload types match the documented schema (serde)
- [ ] `cargo test ipc` + clippy pass