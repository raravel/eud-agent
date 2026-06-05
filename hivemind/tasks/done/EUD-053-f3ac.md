---
completed_at: '2026-06-05T13:03:53.539309'
created: '2026-06-05'
depends_on: []
id: EUD-053-f3ac
parent: EUD-045-38f9
priority: high
scope:
- '*'
status: done
title: 'Spike: codex Python SDK + eud-tools MCP round-trip (Windows)'
type: task
updated: '2026-06-05'
---

## Description
De-risk spike: prove the official Codex Python SDK + eud-tools MCP attachment works on this Windows machine BEFORE any dependent code. Determine the exact PyPI package name/pin, thread lifecycle (thread_start/run/resume), streaming event consumption, and the per-thread MCP server attachment method (config injection vs codex mcp add); run one real tool round-trip against a dummy stdio MCP tool.

## Spec References
- [[features/05_agent-core|05_agent-core]] `../docs/features/05_agent-core.md` — Engine (single path)
- [[tech-stack]] `../docs/tech-stack.md` — Planned v2 section

## Completion Criteria
- [ ] SDK + mcp packages pinned in server/pyproject.toml (uv lock updated); tech-stack.md Planned entries moved to Active with pins
- [ ] Spike script committed under server/spikes/ proving: thread run with streamed events; dummy MCP tool called by codex; thread resume continues context
- [ ] MCP attachment method documented in the spike report (works without mutating the global codex config, or the mutation is documented and reversible)
- [ ] Report at hivemind/tasks/_reports with measured cold-start and per-tool-call latency