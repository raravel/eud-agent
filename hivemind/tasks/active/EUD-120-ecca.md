---
created: '2026-06-08'
depends_on:
- EUD-119-bc27
id: EUD-120-ecca
parent: EUD-096-4eb3
priority: medium
scope:
- panel/src/setup
- panel/src/components
status: pending
title: Panel setup + connection-state UI
type: task
updated: '2026-06-08'
---

## Description
Add the first-run setup screen (download progress from `progress {stage: bootstrap}`) and
the editor-connection-state UI ("editor not connected" when the bridge heartbeat is
stale; instruct/apply disabled with a hint). Monaco/diff rendering unchanged.

## Spec References
- [[features/15_panel-tauri-ipc|15_panel-tauri-ipc]] `../docs/features/15_panel-tauri-ipc.md` - setup/connection states
- [[features/10_tauri-shell-bootstrap|10_tauri-shell-bootstrap]] `../docs/features/10_tauri-shell-bootstrap.md` - first-run flow

## Completion Criteria
- [ ] Setup screen reflects download progress and errors with retry
- [ ] Editor-not-connected state disables instruct/apply with a clear hint
- [ ] `npx vitest run` + `tsc -b` pass