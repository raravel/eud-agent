---
created: '2026-06-08'
depends_on: []
id: EUD-101-8efe
parent: EUD-092-121b
priority: high
scope:
- native/isom/**
status: pending
title: Vendor isom-poc C++ + MSBuild static-lib target
type: task
updated: '2026-06-08'
---

## Description
Vendor the needed isom-poc projects into `native/isom/` (IsomTerrain lib + CrossCutLib +
IcuLib + CascLib) and add an MSBuild static-lib target that compiles the shim and links
those libs. Our repo becomes the source of truth.

## Spec References
- [[features/13_isom-ffi|13_isom-ffi]] `../docs/features/13_isom-ffi.md` - vendoring, build
- [[decisions/09_cpp-static-lib-ffi|09_cpp-static-lib-ffi]] `../docs/decisions/09_cpp-static-lib-ffi.md`
- [[rules]] `../docs/rules.md` - vendored C++ edited only in native/isom

## Completion Criteria
- [ ] isom-poc sources vendored under `native/isom/`
- [ ] MSBuild static-lib target builds to a `.lib` with MSVC
- [ ] Existing verified code paths kept intact (import-then-extend)