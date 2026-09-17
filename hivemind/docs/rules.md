# Permanent engineering rules

## Authority and runtime

- The sole schema-v2 `.eap` manifest, `src/**/*.eps`, `src/**/*.py`, and sparse `dat/*.json` are the only authoring authority.
- NEVER make EDS, generated Python/binaries, E3S, SQLite, LocalAppData mirrors, or Codex workspaces canonical.
- NEVER start, install, patch, probe, or communicate with EUD Editor at runtime.
- NEVER add Lua/file-IPC, heartbeat/status polling, Editor path/bootstrap, BindingManager calls, assembly loading, or Editor build initiation.
- The sibling `../euddraft` repository is read-only. Adapters and fixes belong in `eud-agent`.
- euddraft runs only through the bounded native launcher with explicit cwd/arguments and captured diagnostics. Any project with direct Python state MUST use configured frozen `euddraft.exe`; source-repository mode is rejected.

## Native project paths

- All manifest/source/plugin paths are `/`-separated project-relative paths.
- EPS and direct-Python sources MUST stay under `src/` and end in `.eps` or `.py`; MainFile remains EPS-only.
- Source and output maps MUST end in `.scx` or `.scm` and MUST NOT alias.
- Reject absolute paths, `..`, NUL, empty components, case-colliding duplicates, and symlink/canonical-parent escapes before I/O.
- MainFile is exact manifest state, not a filename convention. Moving MainFile updates the manifest; deleting it is rejected.
- Project creation/import destinations MUST be empty. Never delete a nonempty user folder on failed setup.

## Persistence

- App-written text and JSON are UTF-8 without BOM.
- Use same-directory temp files plus atomic replace for canonical state.
- Validate complete state before any write.
- Roaming contains config, conversations, runtime jobs, journals, session workspaces/turn baselines, and preserved legacy harness sources.
- Local contains large/regenerable model, RAG, analyzer, audio, and compatibility assets.
- Accepted harness documents, trusted approval metadata, memory, and wiki MUST live in the selected project's `.eud-agent`, not in display-name or absolute-path-hash AppData stores. Keep the local workspace ID across folder moves.
- Legacy migration MUST preserve source stores and existing local files, require unambiguous ownership, and record a one-time cutover so deleted local files are not resurrected.
- Optional E3S harness omissions MUST return scoped paths/reasons and stable consent IDs. Explicit approval applies only to the current issues; recheck never authorizes exclusions. Core import and cleanup/rollback failures remain errors.
- New import files MUST be published without replacing concurrent destination files. Windows publication MUST work without hard-link support, including exFAT project volumes.
- Generated build outputs stay inside the project `build/` tree.

## Sparse DAT

- Stock values come from the version-matched `DatCatalog`.
- Store only `{before, after}` overrides; `before` MUST equal the current catalog baseline.
- The model mutates DAT only through `dat_patch(changes[])`.
- A patch MUST reject duplicate targets, no-ops, stale before-values, malformed tagged unions, invalid requirements/buttons, and out-of-range values before writing anything.
- Maximum patch size is 300. Prefer one bounded batch over individual calls.
- Standard DAT, XDAT, TBL, requirements, and buttons remain separate sparse documents.
- Reset removes the sparse override and restores catalog behavior.

## E3S compatibility

- E3S is import/export compatibility, never runtime authority.
- Parse NRBF as bounded data; NEVER execute BinaryFormatter payloads or load Editor assemblies in product code.
- Parse→write of an unmodified supported stream MUST be byte-exact.
- New object ids MUST exceed the magnitude of every positive and negative existing id.
- Import/export MUST update CUI sources, MainFile, settings/plugins, and every declared DAT family semantically.
- Unsupported GUIEps/GUIPy/RawText or ClassicTrigger-as-main MUST fail explicitly. Silent projection is prohibited.
- Preserve supported opaque base objects; never claim an extension-only payload makes Editor reflect native edits.
- Native-only projects may refuse E3S export when no compatibility base exists. Refusal is safer than a synthetic lossy graph.
- Export MUST copy referenced source assets, reparse output, and write atomically.

## Build generation

- Generate EDS and plugins deterministically from canonical state.
- Unchanged DAT families emit no generated plugin.
- Preserve source-verified address, delta, byte order, requirement capacity, and TBL encoding rules.
- Hold a project-scoped build marker for the full euddraft process.
- Build success requires exit success and a fresh expected output map.
- Surface structured file/line diagnostics; never suppress or special-case a compiler symptom.
- Map/sound writes refuse while the build marker is held.

## Source editing and build verification

- Apply mutually dependent source changes coherently through the native file tools.
- After source, plugin, or Python dependency changes, run `build_run` in the same turn.
- euddraft diagnostics and a fresh output map are the only build authority; repair every reported compiler error before completion.
- Prefer localized `file_edit`; use full `file_write` only for intentional replacement.
- Preserve exact import paths and MainFile composition-root policy.

## Agent tool admission

- Tool schemas are closed, typed, and validated before dispatch.
- `const`, `enum`, `oneOf`, primitive, array, and object constraints MUST match the advertised schema. A violating call completes with a detailed usage error the model can correct within the run's tool-round budget; it never executes and never fails the run outright. Duplicate call ids, unknown tools, malformed call shape, and stale-run dispatch remain fatal admission errors.
- Read tools never consume write budgets.
- Mutations require evidence and a project write registration.
- Journal every accepted semantic mutation with exact before/after state.
- Reject/rollback applies inverse operations in reverse sequence and persists exact canonical state.
- Never advertise obsolete individual DAT setters or bridge commands.

## Sessions and concurrency

- Conversation events are session-owned.
- Read turns may overlap; writes serialize through `ProjectWriteCoordinator`.
- Loading or renaming a session never steals another session's execution lane.
- ASK/plan/changeset state survives the documented reconnect/session restore boundary.
- Tauri listener readiness and native project availability are independent states.
- Project unavailability gates authoring without pretending the in-process transport disconnected.

## Map safety

- The selected source-map hash is an authority token. Reject stale maps.
- Before mutation: native build guard, Windows no-share probe, full backup, disk-capacity check, and candidate authority validation.
- After mutation: re-extract CHK, verify the requested delta and invariants, then atomically replace.
- If verification fails, restore exact backup; retain backup path when restoration also fails.
- SCMDraft lock recovery text names the required save/close action.
- Never write a Korean/non-ASCII managed MPQ sound path; managed paths are ASCII content-addressed.

## Panel UX

- All user-facing text is Korean.
- Use semantic controls, visible focus, keyboard operation, and descriptive aria labels.
- Long operations disable their trigger and show progress within the same surface.
- Project setup order is project → euddraft → assets → provider selection → provider connection.
- Project actions expose open/create/import; settings also exposes export.
- Every ordinary launch requires explicit project selection; a file launch opens only its requested project. Stored config alone must not activate project polling, session restoration, or project-dependent bootstrap.
- Recents are newest-first by successful explicit open; failed/canceled actions do not change selection or recency. Removing history must never delete project files.
- File association is `.eap` only, never all `.json` files. The `.eap` file contains the canonical manifest directly. Legacy `project.json`/`.eudproj` may be migrated only on explicit open after validation; never generate descriptors or maintain two manifest authorities.
- Refuse project switches during active work. Preserve forwarded requests for visible retry rather than dropping them or switching under a running agent/build.
- Error text states a recovery action and never leaks raw protocol identifiers as the only explanation.
- Use Lucide/vector icons, semantic theme tokens, and reduced-motion classes; no emoji structural icons.

## Verification

- Bug fixes reproduce and then remove the observed failure.
- Permanent contracts get behavior tests, not source-text tests.
- Rust full suite, panel full suite, TypeScript build, and panel production build MUST pass before release.
- Native build acceptance MUST run the Rust generator and a real configured euddraft against a real SCX.
- E3S acceptance MUST include real import→export→import equality and original .NET BinaryFormatter deserialization.
- UI changes MUST be browser/Tauri-surface verified, not inferred from unit tests.
- Remove temporary fixtures, diagnostic scripts, obsolete tests/docs, and generated smoke files after verification.

## Direct Python dependencies

- Python is always active and full-trust; never describe an import allowlist as a sandbox.
- Only ordered manifest entrypoints execute before the EPS MainFile. All `src/**/*.py` remain part of revision, snapshots, search, review, and rollback.
- Exact direct PyPI pins and the complete wheel lock live only in `project.eap`; caches and preparation tokens are derived.
- Preparation holds no project write lease. Token commit rechecks identity/revision/manifest, journals exact before/after bytes, and retains normal review ownership.
- Managed uv/wheels/environments are checksummed, reparse-safe, atomically published, frozen-runtime probed, and fail closed offline.
- E3S export refuses any project containing direct Python state; EPS-only compatibility is unchanged.
