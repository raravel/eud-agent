# Native project surface

## Canonical layout

```text
project-root/
  project.eap
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

The sole `.eap` manifest contains schema version, name, source/output maps, exact MainFile, settings, ordered EDS plugins, and optional E3S compatibility metadata.

New/imported projects write `project.eap` directly; no pointer file is generated. Opening accepts a project directory or its sole `.eap` file, including a renamed manifest; subsequent saves target that exact file. Multiple `.eap` files, nonregular/symlink manifests, and mixed canonical/legacy authorities are rejected. Reads retain the 4 MiB manifest bound. Explicitly opening a legacy folder, `project.json`, or bounded `.eudproj` descriptor validates the project and every legacy descriptor before atomically renaming the manifest and removing descriptors. Descriptor targets remain restricted to sibling `project.json`. Failed descriptor cleanup attempts rollback without deleting the surviving manifest; errors report incomplete rollback. Passive status probes never migrate.

All paths are normalized `/`-separated project-relative paths. Source files must be under `src/` and end in `.eps`. Traversal, absolute paths, NUL, case-colliding duplicates, invalid map extensions, and source/output aliasing are rejected.

## Project operations

`NativeProject` provides:

- create/open/status/revision;
- source list/read/create/write/edit/move/rename/delete;
- MainFile selection;
- settings and ordered plugin CRUD;
- complete source snapshots for turn baselines and revision hashes;
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

Explicit successful open/create/import updates a canonical-root MRU entry only after validation. Failed or canceled actions preserve the configured project and recency. Switching is serialized against session workers and refused during agent/review/harness work, builds, or open map-work windows; a successful root change discards idle worker caches and panel project/session views, not persisted sessions.

There is no Editor heartbeat, process launch, install path, BindingManager, command inbox/outbox, or runtime bridge.

## Verification

- project confinement/source/MainFile unit tests;
- duplicate/stale/no-op/invalid atomicity tests;
- restart persistence tests;
- exact-state batch contract for 1, 50, and 200 changes;
- tool runtime/journal/rollback integration in the complete Rust suite.
