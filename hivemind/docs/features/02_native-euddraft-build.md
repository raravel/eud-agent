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

Requirement and TBL output follow the Editor's `WriteReqFile`/`tblReader`/`tblWriter` byte contract, verified against a real Editor build folder of the same project:

- an `orders` requirement pointer addresses the word after the leading order-id word, matching stock `require.dat`; a pointer at the id word makes the game read that id as a must-own-unit opcode, which drops MSQC queue commands and other orders;
- a TBL entry ends at the first NUL after at least two bytes, so hotkey strings such as `o<00>Tank Mode` keep their text;
- `custom_txt.tbl` always dumps the complete 1547-entry table because `dataDumper` copies it over the in-game header in place.

## euddraft process

`EuddraftLaunch` accepts `euddraft.exe` or `euddraft.py`, normalizes argv, and runs with:

- explicit EDS argument and working directory;
- bounded timeout;
- captured stdout/stderr;
- project-scoped build marker;
- required fresh output map;
- structured file/line diagnostics.

The sibling `../euddraft` repository is read-only. Missing private source modules are not patched; installed euddraft is a supported configured executable.

The Settings **Compile** category reads the configured path and, for managed
installs, the persisted release tag. An explicit check compares that tag with
GitHub's latest official release; an available update reuses the same
SHA-256-verified staging and atomic publication path before replacing the
configured executable path. A manually selected distribution is never probed
by launching it and is replaced only by an explicit managed-install action.

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
