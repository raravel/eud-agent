---
created: '2026-06-08'
depends_on: []
id: EUD-094-e9ee
parent: EUD-090-b768
priority: high
status: pending
title: S4 Rust backend core
type: story
updated: '2026-06-08'
---

## Description
Port the Python server's request handling into the in-process Rust core exposed over
Tauri IPC: orchestrator, tool layer, codex client, bridge file-IPC, memory.

## Spec References
- [[features/11_rust-backend-core|11_rust-backend-core]] `../docs/features/11_rust-backend-core.md`
- [[decisions/08_tauri-rust-rewrite|08_tauri-rust-rewrite]] `../docs/decisions/08_tauri-rust-rewrite.md`

## Completion Criteria
- [ ] All child tasks done
- [ ] instruct/apply/status/list commands + progress/code/applied/error events work
- [ ] evidence gate + first-principles rejections enforced; `cargo test` passes