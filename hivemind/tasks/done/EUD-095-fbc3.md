---
completed_at: '2026-06-10T01:27:26.226390'
created: '2026-06-08'
depends_on: []
id: EUD-095-fbc3
parent: EUD-090-b768
priority: medium
status: done
title: S5 Slim Lua bridge
type: story
updated: '2026-06-10'
---

## Description
Reduce the Lua bridge to a file-IPC tool layer; remove WebView2 hosting and server spawn;
reverse heartbeat/status to editor->app signals.

## Spec References
- [[features/14_lua-bridge-slim|14_lua-bridge-slim]] `../docs/features/14_lua-bridge-slim.md`
- [[rules]] `../docs/rules.md` - Lua bridge crash rules (retained)

## Completion Criteria
- [ ] Child task done
- [ ] Bridge processes PING/STATUS/LIST/GET/SET/NEWEPS/BUILD; no WebView2/spawn code remains
- [ ] heartbeat.txt + status.txt written unconditionally before IsCompilng early-return