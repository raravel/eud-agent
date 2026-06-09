---
created: '2026-06-09'
depends_on:
- EUD-128-daea
id: EUD-130-b4eb
parent: EUD-127-e1a7
priority: medium
scope:
- src-tauri/src/tools.rs
status: pending
title: location_write Rust tool handler (MRGN CRUD via mapsafe + changeset/journal)
type: task
updated: '2026-06-09'
---

## Description
Implement the `location_write` WRITE tool handler in Rust (the schema is already registered in
tools.rs via EUD-124; the handler body is missing). Agent-driven MRGN CRUD on the connected
source map, applied through the mapsafe rails + IsomEngine (EUD-128).

- Actions add/set/rename/delete; validate args (name, locationId, tile rects, invertX/invertY)
  before any write. Encode MRGN locedit ops (MapGenCli encoding) and apply via
  `MapSafe::write(map, OpKind::Locedit, ops)` so backup/lock/compiling/rollback rails run.
- Emit a `file`/map changeset item (kind matching the journal contract) so the edit is
  reviewable/rollbackable like other mutations; obeys the mutation gate + budgets.
- Refuse #64 (Anywhere) edits at the rails; inverted (negative) locations supported (feature 09).

## Spec References
- [[features/09_location-write-tool|09_location-write-tool]] `../docs/features/09_location-write-tool.md` — actions, validation, inverted locations
- [[features/13_isom-ffi|13_isom-ffi]] `../docs/features/13_isom-ffi.md` — locedit ops + mapsafe rails
- [[rules]] `../docs/rules.md` — #64 protected, autoDefragmentLocations=false, backup-before-write

## Completion Criteria
- [ ] `location_write` handler implements add/set/rename/delete with validate-before-write; invalid op aborts before save
- [ ] Writes go through `MapSafe::write(OpKind::Locedit)` (backup/lock/compiling/rollback rails); #64 refused; inverted locations supported
- [ ] Produces a journaled, reviewable changeset item; mutation gate + budget honored
- [ ] `cargo test -p eud-agent` passes; clippy/fmt clean