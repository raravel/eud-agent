# Native euddraft build

## Authority

`src-tauri/src/native_build.rs` is the sole build frontend. EUD Editor build initiation, generated Editor temp files, assembly calls, and Lua/file-IPC are absent.

Input authority:

- `project.json`
- `src/**/*.eps`
- sparse `dat/*.json`
- bundled version-matched compatibility metadata under `native/eud-editor-compat`

## Compatibility catalog

`DatCatalog` loads the vendored EUD Editor data definitions strictly as a documented data-format contract:

- DAT `.def` and `.dat` baselines;
- `Offset.txt` addresses;
- button defaults;
- status function defaults;
- requirement defaults and capacities;
- `stat_txt.tbl` and related TBL files;
- `WireFrameDataEditor.eps`.

These assets are copied into `%LOCALAPPDATA%/eud-agent/native_assets` at startup. EUD Editor binaries are not bundled or loaded.

## Generated artifacts

`generate_native_build` deterministically materializes `build/euddraft/`:

- `DataEditor.py` for standard DAT deltas;
- `ExtraDataEditor.py` for XDAT/buttons/requirements;
- `RequireData` with table-specific capacities and pointer tables;
- `custom_txt.tbl` when TBL overrides exist;
- `WireFrameDataEditor.eps` when required;
- `eud-agent.eds` with canonical `[main]`, settings, generated plugins, source plugins, and user plugins.

Unchanged sparse families emit no plugin. Standard DAT address/delta math follows the source-verified Editor generator contract.

## euddraft process

`EuddraftLaunch` accepts `euddraft.exe` or `euddraft.py`, normalizes argv, and runs with:

- explicit EDS argument and working directory;
- bounded timeout;
- captured stdout/stderr;
- project-scoped build marker;
- required fresh output map;
- structured file/line diagnostics.

The sibling `../euddraft` repository is read-only. Missing private source modules are not patched; installed euddraft is a supported configured executable.

## Safety

- Source/output paths are confined by `NativeProject`.
- A source map and output map may not alias.
- Build success requires a newly created or strictly newer output.
- Numeric values, requirements, buttons, and table bounds are validated before generation.
- Map writes and sound edits refuse while the native build marker is held.

## Verification

- generator unit tests cover field metadata, address math, button byte order, requirement capacity, TBL encoding, EDS ordering, diagnostics, and freshness;
- ignored real-runtime contract `real_euddraft_builds_generated_native_project` copies a real SCX, applies a sparse DAT edit, generates every artifact through Rust, runs configured euddraft, and requires a fresh output SCX;
- verified executable: `E:/proj/eud/euddraft0.10.2.5/euddraft.exe`.
