# Native project surface

## Canonical layout

```text
project-root/
  project.json
  src/**/*.eps
  dat/standard.json
  dat/xdat.json
  dat/tbl.json
  dat/requirements.json
  dat/buttons.json
  maps/
  plugins/
  build/
  compat/editor-project.e3s   # imported projects only
```

`project.json` contains schema version, name, source/output maps, exact MainFile, settings, ordered EDS plugins, and optional E3S compatibility metadata.

All paths are normalized `/`-separated project-relative paths. Source files must be under `src/` and end in `.eps`. Traversal, absolute paths, NUL, case-colliding duplicates, invalid map extensions, and source/output aliasing are rejected.

## Project operations

`NativeProject` provides:

- create/open/status/revision;
- source list/read/create/write/edit/move/rename/delete;
- MainFile selection;
- settings and ordered plugin CRUD;
- complete source snapshots for Codex workspaces and epScript preflight;
- sparse DAT reads and atomic batch patching;
- E3S compatibility metadata;
- atomic JSON/manifest persistence.

Project creation always creates an empty configured MainFile. Deleting MainFile is rejected; moving it updates the manifest atomically.

## Agent tools

Read tools:

- `project_status`, `list_files`, `read_file`, `source_search`
- `dat_get`, `xdat_get`, `tbl_get`, `req_get`, `btn_get`
- settings/plugin/map/source context tools

Write tools:

- source CRUD and MainFile operations
- settings/plugin operations
- one schema-rich `dat_patch` batch mutation
- `build_run`

Individual model-facing DAT setter tools were removed. `dat_patch` is the single admission, evidence, journaling, rollback, and persistence boundary.

## Atomic DAT patch

A patch accepts at most 300 changes. Before any write it rejects:

- duplicate targets;
- no-op changes;
- stale `before` values;
- invalid table/field/object ids;
- invalid requirement/button payloads;
- out-of-range numeric values.

The five sparse documents are written atomically only after the complete patch validates. Journal entries remain semantic per target so reject/rollback restores the exact prior sparse state.

## Runtime integration

`NativeProjectManager` reloads configured state for each operation. Tauri `status` and `list` are local native-project probes. A periodic panel refresh detects external project removal/recovery without affecting the in-process Tauri transport.

There is no Editor heartbeat, process launch, install path, BindingManager, command inbox/outbox, or runtime bridge.

## Verification

- project confinement/source/MainFile unit tests;
- duplicate/stale/no-op/invalid atomicity tests;
- restart persistence tests;
- exact-state batch contract for 1, 50, and 200 changes;
- tool runtime/journal/rollback integration in the complete Rust suite.
