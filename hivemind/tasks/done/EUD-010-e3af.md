---
completed_at: '2026-06-04T18:46:57.177884'
created: '2026-06-04'
depends_on:
- EUD-008-cddc
- EUD-009-6aed
id: EUD-010-e3af
parent: EUD-002-684a
priority: high
scope:
- scripts/**
- server/tests/test_deploy_scripts.py
status: done
title: 'Deployment scripts: setup_env, install_dropin (agent.cfg), dev_run'
type: task
updated: '2026-06-04'
---

## Description
PowerShell 7 deployment scripts. setup_env.ps1: create server/.venv via uv with pinned deps (torch from cu126 index, CPU fallback flag), check bge-m3 presence in HF cache and warn about the 4.3GB download otherwise. install_dropin.ps1: copy bridge lua to editor Data\Lua\TriggerEditor\, vendor DLLs next to the editor exe, and write Data\agent\agent.cfg with absolute python_exe/repo_root/port - this cfg is the only way the drop-in lua can find the server (critical finding). dev_run.ps1: run the server standalone for browser-based panel work. Also uninstall_dropin.ps1 (remove lua + cfg; leave DLLs optional).

## Spec References
- [[architecture]] `../docs/architecture.md` - Boot and lifecycle (agent.cfg schema)
- [[tech-stack]] `../docs/tech-stack.md` - Active Dependencies (pin list, cu126 index)
- [[verify]] `../docs/verify.md` - e2e step 1

## Completion Criteria
- [ ] setup_env.ps1 produces a working venv: server\.venv\Scripts\python.exe -c "import torch, fastapi, chromadb, sentence_transformers" succeeds
- [ ] install_dropin.ps1 writes agent.cfg with valid absolute paths (JSON parses; paths exist)
- [ ] install_dropin.ps1 is idempotent (re-run overwrites cleanly)
- [ ] Scripts refuse to run with a clear message when the editor path is wrong