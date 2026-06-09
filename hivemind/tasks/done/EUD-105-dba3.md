---
completed_at: '2026-06-09T16:46:22.369346'
created: '2026-06-08'
depends_on:
- EUD-104-0ae7
id: EUD-105-dba3
parent: EUD-092-121b
priority: medium
scope:
- src-tauri/src/chk.rs
- src-tauri/src/lib.rs
- src-tauri/Cargo.toml
- src-tauri/Cargo.lock
status: done
title: CHK parse port (chk_info.py -> chk.rs)
type: task
updated: '2026-06-09'
---

## Description
Port `chk_info.py` to `src-tauri/src/chk.rs`: parse the raw CHK returned by chk_extract
into the structured digest (locations/units/forces/players) used by map_info.

## Spec References
- [[features/13_isom-ffi|13_isom-ffi]] `../docs/features/13_isom-ffi.md` - chk parsing
- [[features/08_map-info-tool|08_map-info-tool]] `../docs/features/08_map-info-tool.md` - digest shape (behavioral source)

## Completion Criteria
- [ ] CHK parse yields the same digest fields as Python chk_info on sample maps
- [ ] `cargo test chk` passes