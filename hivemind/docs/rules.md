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
- `[` and `]` are rejected in every path that can become an EDS section header (sources, plugins, Python entrypoints, source map). Only `outputMap` may contain them, because it is emitted solely as the `[main]` `output:` value; the canonical default is `build/[EUD]<name>.<ext>`.
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
- A DAT label field (`.def` `Type=11`) stores a ONE-based `stat_txt.tbl` string
  id, so its text is `tbl_strings()[value - 1]`. The sparse TBL document and
  `DatTarget::Tbl` address the same strings ZERO-based. Never resolve one
  convention with the other.
- The DAT wiki (`dat_wiki.rs`) is READ-ONLY. It shows stock values and the
  project's overrides side by side; it never creates, edits or removes an
  override, because only `dat_patch` can keep `before` equal to the catalog
  baseline the build checks.
- Requirement tables reuse the DAT table names (`units`, `orders`, `techdata`,
  `upgrades`). Any surface listing both MUST give them distinct ids, or one
  hides the other.
- images.dat's `Iscript ID` selects an EXISTING script, and which animation slots
  that script carries lives only in the installed `scripts\iscript.bin`. The model
  reads them with `iscript_info`, never by inference from an ID→name list: a
  script's header declares slots `0..=type` padded to an even length, so most
  overlay scripts carry only Init and Death. Never present a slot the file does
  not declare as available, and never repoint an image without checking the slots
  it needs.

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
- One euddraft traceback or `warn_with_traceback` stack is one diagnostic located at its innermost project frame; epScript `[Error N] Module "m" Line n` lines keep their source file/line. Warnings are reported separately and never fail a build; `ok` depends only on errors, exit status, and a fresh output map.
- `build_run` persists the complete stdout/stderr as `build/euddraft/build.log` (removed before each run so a timed-out or cancelled run never leaves a stale log; a stale log that cannot be removed is reported as a warning with `logPath: null`, never presented as this run's) and hands the model only structured diagnostics plus a bounded head/tail excerpt; `build_log_read` pages the log. Both the observation and every page are bounded by the double-escaped byte measure the Claude adapter applies (40 KiB), with omitted diagnostic counts reported. Raw euddraft output never enters a tool observation unbounded. A failed log write is a reported warning, never a discarded build verdict.
- euddraft's own post-failure `input()` under the launcher's closed stdin (an `EOFError` traceback with no project frame ending in `euddraft.py`) is the launcher's artifact and is reported as a warning, not as a project error.
- Map/sound writes refuse while the build marker is held.

## Source editing and build verification

- Apply mutually dependent source changes coherently through the native file tools.
- After source, plugin, or Python dependency changes, run `build_run` in the same turn.
- euddraft diagnostics and a fresh output map are the only build authority; repair every reported compiler error before completion.
- Prefer localized `file_edit`; use full `file_write` only for intentional replacement.
- Preserve exact import paths and MainFile composition-root policy.

## Staged request routing and scope

- Triage picks the lightest route that can finish the request. `scoped` — the foreground turn
  locates the site itself, makes the change, and builds — is the route for work that is small once
  its target or value is known; a request does NOT take `pipeline` merely because a value, address,
  or file has to be looked up first. `pipeline` is for work that needs design or investigation
  before anything can be changed.
- `scoped` runs one ordinary foreground turn with no research, plan, critique, or approval stage.
  Writes still journal, build, and reach changeset review like every other route.
- Acceptance criteria state only the outcomes the user asked for. Hardening, refactors, renames,
  comment or wording sweeps, new modules, and new tests the user did not request are never added by
  triage, research, or the planner; a stage that notices one reports it (research `openQuestions`,
  plan `outOfScope`, or the turn's answer) instead of absorbing it into the work.
- The research stage reads only as far as its acceptance criteria need and stops there; it consults
  the documentation only for facts the project itself cannot answer.
- The critic fails excess as it fails a gap: a step, file, module, test, or criterion the goal did
  not ask for is a major issue naming the step to drop.

## Agent tool admission

- Tool schemas are closed, typed, and validated before dispatch.
- `const`, `enum`, `oneOf`, primitive, array, and object constraints MUST match the advertised schema. A violating call completes with a detailed usage error the model can correct within the run's tool-round budget; it never executes and never fails the run outright. Duplicate call ids, unknown tools, malformed call shape, and stale-run dispatch remain fatal admission errors.
- Read tools never consume write budgets.
- The Map Agent reads tile ids with `map_terrain_read`/`map_draft_terrain_read` (≤ 4096 tiles per call); probing ids through `terrain.set` expected-before conflicts is prohibited.
- Semantic terrain that must blend with its surroundings (hills, plateaus, pools, ground-type
  changes) is painted with the ISOM brushes: `terrain.isom_rect` on a tile rectangle (the model's
  default form) or `terrain.isom_brush` on one ISOM diamond (`isomX` = tile x / 2, `isomY` = tile
  y, `isomX + isomY` even, footprint `[2·isomX−2, 2·isomX+1] × [isomY−1, isomY]`). The transition
  ring is regenerated from the map's ISOM data up to two diamonds (8 × 4 tiles) around the painted
  area. Cliff edges are never assembled from exact tile ids unless the user asks for exact tiles.
  The native engine refuses a missing/mis-sized ISOM section, a brush without an ISOM value, an
  off-grid or off-lattice diamond, and a rectangle holding no whole diamond, each with the exact
  reason; it never reports a no-op placement as success, and the model never concludes from a
  refusal that the map lacks ISOM data. Every `mapedit` operation is deterministic: ISOM subtile
  variation is drawn from a per-operation generator seeded by the operation's own parameters,
  never from process-global `std::rand()`, because Apply replays the candidate's operation
  manifest. The manifests are the candidate's authority and the snapshot is their cached
  product: a snapshot the replay no longer reproduces is rebuilt from the manifests on the
  session's baseline (fresh verification, revisions and object ids refreshed, selections
  cleared) before Apply or revert continues; only a chain a revision of which does not replay
  keeps the refusal.
- No single tool call waits longer than 240 seconds: native CLIs abort a silent MCP call at 300
  seconds. `ask` expires into a plain-text handoff; any future delegation or team wait shares the
  bound and continues as a new user turn.
- Mutations require evidence and a project write registration.
- A `delegate_read` child never writes, asks, builds, delegates again, or lifts the parent's evidence gate; the parent re-reads a target before editing it.
- The EPS session never places terrain, units, buildings, doodads, or sprites itself: it hands that work to its team Map session with `map_task_request`. A team candidate reaches the source map only through the Map window's Apply or the EPS session's `map_task_apply`, which is admitted only after that request inspected the exact announced candidate (`map_task_diff`/`map_task_objects`/`map_task_render`) and runs the Map window's own backup, verification, and rollback; the user can undo either apply from the Map window. A `map_task_request` that revises an earlier task's result (`revisesTaskId` on a candidate_ready or applied task) continues that task's team session, on its candidate while it is ready, and its goal states only the change; every other request runs in a fresh team Map session on the saved source map, superseding a ready candidate, and an earlier team session is retired unless its Apply is still undoable. While a team task is queued, running, or candidate_ready the EPS session's own map writers are refused.
- A team task that settles after its `map_task_request` returned `running` continues the EPS conversation by itself: the engine starts one ordinary interactive turn on the fixed "이어서 진행" text once the session is idle (never over a pending review, plan, or live autonomous run, never for a cancelled task, never across a restart) and announces it through `team_task.continuation` so the panel records the user turn. A settlement the engine could not pick up reaches the model through `[map tasks]` on the next user turn.
- A team task goal relays the user's request, never the EPS session's design: a few sentences as the user would type them in the Map window — the user's wording and intent in the user's language, the reference area to match, and only the constraints the code depends on (keep-clear cells, location ids, walkability). Bounds go in the request's `target`/`protect` rectangles, which the verifier enforces with the task's layers, never in goal prose. One task covers one area or feature; larger work is a sequence of tasks. The EPS session never chooses the medium, counts, palette ids, cluster coordinates, or a layout, never pastes `map_info` tile ids that are not themselves a constraint, and grants every layer the look may need. The team wrapper tells the Map Agent to work the goal exactly like a Map-window request (palette, draft, render, analyze, iterate, one finalize) and that the design is its own; a team session has the same engine, prompt, provider binding, and tools as the Map window.
- A doodad is laid out as StarCraft's CV5 defines it: the DD2 id is the CV5 `ddDataIndex`, row `y` of the footprint is CV5 group `start + y`, cell `(x, y)` is tile `16 * (start + y) + x`, and a row cell whose `megaTileRef` is 0 leaves the terrain untouched. Footprint tiles are never derived from a single group, and `replacementTiles` restore only the cells the doodad owned.
- Journal every accepted semantic mutation with exact before/after state.
- Reject/rollback applies inverse operations in reverse sequence and persists exact canonical state.
- Never advertise obsolete individual DAT setters or bridge commands.

## Continuity across an interruption

- A session's own conversation reaches triage. Triage routes one message, and a message that points
  back at earlier work ("continue", "계속", "그거 해줘") names work only the transcript and the
  unresolved request can identify; routing it as ambiguous is a defect, not a clarification.
- Model-facing transcripts are bounded by dropping the OLDEST rows and marking the cut. A
  head-first cut hands the model the opening of a long session and none of the work it must
  continue.
- A request that a shutdown or a cancellation left unresolved is set aside on the session record
  and projected as `interrupted_request`, with its research, its plan, and whether the user
  approved that plan. A later message never discards it; only the user resolves it, by resuming or
  restarting it.
- A chat turn persists the provider conversation it reached on every exit path, including failure
  and cancellation. A boundary that exists only in memory is lost with the process.
- A native provider publishes its resumable session identity before any output, and an interrupted
  run keeps it as that run's boundary (receipt state `interrupted`). A session the run did not ask
  to resume is a protocol deviation and is never adopted; without an observed session the
  continuation stays unknown and fails closed. An interruption costs the unfinished turn, never
  the conversation.

## Sessions and concurrency

- Conversation events are session-owned.
- Turns may overlap; individual project writes serialize through `ProjectWriteCoordinator`. An EPS chat turn runs with write access from its first call; never reintroduce a refused-mutation read→write restart for it.
- Loading or renaming a session never steals another session's execution lane.
- ASK/plan/changeset state survives the documented reconnect/session restore boundary.
- Tauri listener readiness and native project availability are independent states.
- Project unavailability gates authoring without pretending the in-process transport disconnected.

## Map safety

- The saved source map is the authority every Map session follows; a session never owns it. When
  the source hash differs from a session's baseline, the session re-reads the source and replays its
  candidate revisions onto it with fresh verification (last writer wins). A revision the new source
  cannot reproduce leaves the candidate on its old snapshot with `sourceDiverged`; Apply then
  overwrites the saved bytes after backing them up — but only the user may Apply a diverged
  candidate from the Map window; `map_task_apply` refuses it. A followed source also retires the
  last Apply's undo (`canUndo` requires the source to still be that Apply's output). Only a session
  with an active request defers (`stale`) until that request settles. Never ask the user to discard
  or reopen because the source changed. Every load → follow → save window holds the session lock,
  and `MapSafe::apply` still compares the exact source hash it was handed and refuses a race.
- Before mutation: native build guard, Windows no-share probe, full backup, disk-capacity check, and candidate authority validation.
- After mutation: re-extract CHK, verify the requested delta and invariants, then atomically replace.
- If verification fails, restore exact backup; retain backup path when restoration also fails.
- SCMDraft lock recovery text names the required save/close action.
- Never write a Korean/non-ASCII managed MPQ sound path; managed paths are ASCII content-addressed.
- Every CHK string the app writes (locations, wizard title/description/force names, scenario
  properties) goes through `chk::encode_chk_text`: ASCII verbatim, `STRx` maps UTF-8, legacy
  `STR ` maps CP949, UTF-8 only when CP949 cannot represent the text. Never write raw UTF-8 into
  a legacy string table.
- `SPRP/OWNR/IOWN/SIDE/FORC` change only through the Map window's properties request, verified
  under a properties authority; every other Map request treats them as fixed sections.

## Panel UX

- All user-facing text is Korean.
- Use semantic controls, visible focus, keyboard operation, and descriptive aria labels.
- Long operations disable their trigger and show progress within the same surface.
- Project setup order is project → euddraft → assets → provider selection → provider connection.
- Project actions expose open/create/import; settings also exposes export. Create offers both "existing map" and the blank-map wizard; the wizard is launcher-only and never an agent tool.
- The blank-map wizard fails closed without a resolvable StarCraft data folder and offers the folder picker in place; it never generates terrain from bundled or synthetic tileset data.
- Every ordinary launch requires explicit project selection; a file launch opens only its requested project. Stored config alone must not activate project polling, session restoration, or project-dependent bootstrap.
- Recents are newest-first by successful explicit open; failed/canceled actions do not change selection or recency. Removing history must never delete project files.
- File association is `.eap` only, never all `.json` files. The `.eap` file contains the canonical manifest directly. Legacy `project.json`/`.eudproj` may be migrated only on explicit open after validation; never generate descriptors or maintain two manifest authorities.
- Refuse project switches during active work. Preserve forwarded requests for visible retry rather than dropping them or switching under a running agent/build.
- Error text states a recovery action and never leaks raw protocol identifiers as the only explanation.
- Use Lucide/vector icons, semantic theme tokens, and reduced-motion classes; no emoji structural icons.
- Every panel control is a shadcn/ui primitive from `panel/components/ui` (Button, Input, Textarea, Select, RadioGroup, Checkbox, Switch, Dialog, Tabs, ...). Native `<select>`, `<input type="radio">`, `<input type="checkbox">`, or hand-styled equivalents are prohibited; a missing primitive is added with the shadcn CLI (`npx shadcn add <name>` in `panel/`), never hand-rolled.

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
