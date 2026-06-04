---
created: '2026-06-04'
depends_on:
- EUD-007-761d
id: EUD-009-6aed
parent: EUD-004-16b7
priority: high
scope:
- server/pyproject.toml
- server/eud_agent/config.py
- server/eud_agent/__main__.py
- server/tests/test_config.py
- server/eud_agent/__init__.py
- server/uv.lock
status: in_progress
title: 'Server: pyproject, config, selfcheck entrypoint'
type: task
updated: '2026-06-04'
---

## Description
Server package skeleton. server/pyproject.toml with the exact pins from tech-stack.md (uv-managed; torch cu126 index via tool.uv sources, CPU fallback extra). config.py: resolution order CLI args, env vars, agent.cfg, defaults; keys data_dir/port/codex_cmd/rag_db/repo_root; session token generation (uuid4); codex resolution via shutil.which at startup. __main__.py: python -m eud_agent runs the app; --selfcheck validates config, codex shim, RAG DB path, bge-m3 HF cache presence, panel files - per verify.md smoke - without loading the model, exiting non-zero with a specific message per missing prerequisite.

## Spec References
- [[features/02_python-server|02_python-server]] `../docs/features/02_python-server.md` - config.py
- [[tech-stack]] `../docs/tech-stack.md` - Active Dependencies
- [[verify]] `../docs/verify.md` - smoke stage

## Completion Criteria
- [ ] uv sync creates the venv; import smoke passes (torch, fastapi, chromadb, sentence_transformers)
- [ ] --selfcheck exits 0 on this machine; each induced failure (bad path, no codex) yields its own message
- [ ] config precedence covered by unit tests (tmp agent.cfg fixtures)
- [ ] ruff clean