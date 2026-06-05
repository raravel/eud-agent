---
created: '2026-06-05'
depends_on:
- EUD-054-8c97
id: EUD-055-f4cc
parent: EUD-045-38f9
priority: high
scope:
- server/eud_agent/journal.py
- server/eud_agent/tools.py
- server/tests/test_journal.py
status: pending
title: Change journal + rollback engine
type: task
updated: '2026-06-05'
---

## Description
Change journal + rollback per spec: before-snapshots for every write tool (dat/xdat/tbl/req/btn value + was_default, file old content / created marker / deleted content+position, rename/move old path, set_main old path, settings/plugin old values); JSON persistence per request under the data-dir journal folder; changeset assembly (dat grouped per objId with unit names via tbl, unified diffs for modified files); inverse-op rollback in reverse seq order incl. RESETDAT for was_default fields.

## Spec References
- [[features/05_agent-core|05_agent-core]] `../docs/features/05_agent-core.md` — Change journal and rollback

## Completion Criteria
- [ ] Snapshot-then-rollback round-trip tests per tool kind against a fake bridge (apply, reject, verify inverse .cmd sequence and final state)
- [ ] was_default fields roll back via dat_reset, not value-write
- [ ] Journal survives server restart (file persistence; reload produces an identical changeset)
- [ ] Changeset items match the WS v2 shape consumed by the panel spec
- [ ] ruff + full pytest green