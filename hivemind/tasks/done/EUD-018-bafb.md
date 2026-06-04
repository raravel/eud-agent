---
completed_at: '2026-06-04T20:21:54.652556'
created: '2026-06-04'
depends_on:
- EUD-015-c726
- EUD-016-af75
- EUD-017-e827
id: EUD-018-bafb
parent: EUD-004-16b7
priority: high
scope:
- server/eud_agent/orchestrator.py
- server/eud_agent/app.py
- server/eud_agent/__main__.py
- server/tests/test_orchestrator.py
- server/tests/test_app.py
- server/tests/test_config.py
- server/tests/test_deploy_scripts.py
status: done
title: 'Server: orchestrator + FastAPI app + WS'
type: task
updated: '2026-06-04'
---

## Description
Orchestrator + FastAPI app. Orchestrator: async per-instruct state machine rag -> codex -> lsp -> diff -> done emitting WS events per transition; unified diff via difflib for SET targets (current content via bridge_io.get); apply routes to set/neweps translating BridgeBusy into waiting_build progress and editor-busy errors; one in-flight instruct (second gets error busy). App: GET / serves panel/index.html, /static mount, /healthz, WS /ws validating token query param + Origin header at accept (close 4403). Startup: bind 127.0.0.1 cfg-port falling back to port 0; background thread confirms own socket accepts then atomically writes server.ready {port,pid,token,started_at}; heartbeat watcher every 15s self-terminates after 60s staleness deleting server.ready; RAG warmup thread kicked off.

## Spec References
- [[features/02_python-server|02_python-server]] `../docs/features/02_python-server.md` - orchestrator.py / app.py
- [[architecture]] `../docs/architecture.md` - WebSocket protocol, boot and lifecycle
- [[rules]] `../docs/rules.md` - Server and panel

## Completion Criteria
- [ ] WS integration tests (httpx/websockets against the test app): token rejection, origin rejection, instruct happy path with mock codex + stub rag, apply with fake bridge, busy translation
- [ ] server.ready written only after a real TCP connect succeeds (test polls)
- [ ] Heartbeat staleness shuts the server down in a test (short thresholds injected)
- [ ] ruff clean; pytest green