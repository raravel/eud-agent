# Permanent engineering rules

## Agent prompt delivery

- A tool error requires diagnostic-based recovery when a supported authorized path remains. Schema correction must preserve the full requested target set; uncertain writes require status inspection before replay. Cancellation, permission denial and validation boundaries remain binding.
- Keep shared EUD safety/build/manifest contracts in the static baseline. Advertise registry tool names without duplicating their complete descriptions and schemas.
- Worker baselines follow their saved role and retain all domain safety rules. Investigation goals do not implicitly select a full gameplay skill; explicit skill markers remain supported. Reset and compaction use the same role baseline.
- Deliver worker settlements once in a continuing parent context, acknowledging only the bounded records actually shown after successful delivery. Full context recovery may restore recent reported results. Preserve pending reports on failure/cancellation and real recovery history; do not replay a sole first goal alongside itself.
- Write complex worker goals with actual line breaks, useful labeled sections and one requirement per bullet. Preserve normal word spacing, exact identifiers, paths, limits, URLs and all constraints; shorten repetition rather than compressing words together. Simple goals may remain one sentence.
- Deliver detailed Python/audio guides only for relevant request, attachment, resolved-resource, source, accepted-task or replay context. Hash their delivery separately, replace changed selections and resend them on full context recovery. Commit delivery hashes only after successful provider delivery.
- Explicit user requests control plan approval; task size and a completed design interview do not create an extra approval stage.

## Authority and runtime

- The sole schema-v2 `.eap` manifest, `src/**/*.eps`, `src/**/*.py`, and sparse `dat/*.json` are the only authoring authority.
- NEVER make EDS, generated Python/binaries, E3S, SQLite, LocalAppData mirrors, or Codex workspaces canonical.
- NEVER start, install, patch, probe, or communicate with EUD Editor at runtime.
- NEVER add Lua/file-IPC, heartbeat/status polling, Editor path/bootstrap, BindingManager calls, assembly loading, or Editor build initiation.
- The sibling `../euddraft` repository is read-only. Adapters and fixes belong in `eud-agent`.
- euddraft runs only through the bounded native launcher with explicit cwd/arguments and captured diagnostics. Any project with direct Python state MUST use configured frozen `euddraft.exe`; source-repository mode is rejected.

### E2 import rules

- E2 source support is version-gated to the exact `0.17.9.7` text dialect. Reject `TEST` and unknown versions with `unsupported_e2_version`; do not infer support from a file extension or from the older 0.17.8.2 checkout. The pinned release/source and its verified provenance are compatibility oracles, never runtime installation/archive dependencies.
- Keep E2 in the dedicated `src-tauri/src/e2_import/` module. Do not route E2 through `e3s_nrbf.rs`, reuse E3 GUI refusal behavior, export an EAP back to E2, or promise game equivalence from parse/build success.
- Treat `project.eap` as the schema-v2 authority. `e2Import` is optional for backward compatibility. Active resource entries must be normalized project-relative files with exact size/SHA-256, a typed kind, source provenance, and a deterministic `ProjectResourceConsumer`; disabled payload belongs only to inert provenance. Reopen and build must reject missing, changed, stale-hash, absolute/parent/NUL, symlink/reparse, canonical-escape, or Windows case-colliding resources.
- Resolve an input map from stored `InputMap`, then the actual E2P `Map` or E2S same-name sibling/package candidate, and finally an explicit picker relink. Never use `OutputMap` as an input fallback. Zero or multiple candidates remain a typed relink state, and relinks are accepted only when the source fingerprint has not drifted.
- All enabled non-GRP rows require a parser representation, canonical EAP authority, deterministic native consumer, and semantic oracle: DAT; FireGraft/XDAT/TBL/requirements/buttons; BIN; in-memory and file-backed custom tiles; BGM/sound; editable Struct/GUI/lifecycle EPS; plugins; and file-manager TBL/wireframe/iscript/aiscript/non-GRP binaries. Active SCDB and effective GRP replacement/injection/use fail clearly. Flag-false payload and true-but-empty/effectively no-op GRP are preserved as inactive provenance. Enabled `ChatWrite` fails as `active_chatwrite_unverified`; it must never be silently aliased.
- The seven-function custom-text surface is the only adapted helper surface. Preserve the MIT notice and Artanis attribution from `src-tauri/src/e2_import/LICENSE.EUD-Editor-2-customText`; do not vendor a full legacy helper or revive the historical `punitloop` alias.
- Import must preflight and hash all inputs before destination mutation; materialize in a confined stage; validate/reopen before publication; publish with create-new/no-replace receipt writes; and activate exactly once after published reopen. Cancellation, malformed input, relink drift, collision, active unsupported features, and cleanup failure must leave original inputs, active configuration, and unrelated concurrent destination files intact.
- For BGM/sound, inject generated `MPQAddFile` calls from confined owned bytes before euddraft freeze, then verify fresh output MPQ entries read-only by exact alias, bytes, size, and SHA-256. A helper-only `PythonPath.py` line must not become an entrypoint. Existing MapSafe sound-writer compiling guards remain unchanged.
- The five E2 commands are `setup_pick_e2_source`, `setup_pick_e2_relink`, `setup_e2_preflight`, `setup_import_e2`, and `setup_cancel_e2_import`; destination selection reuses `setup_pick_import_destination`. The launcher, setup screen, and Settings → Project each pass an E2 action into the shared dialog. The completed path refreshes read-only `setup_status`, checks the returned project path, then performs the UI-only transition; it does not call `project_open` a second time.

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
- A successful build also copies its output map to the resolved StarCraft install's `Maps/eud-agent/<output file name>` (temporary file + rename). The copy is a convenience, never build authority: without a resolvable install nothing is copied, and a failed copy is a warning that never changes `ok`.

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
- Every DAT wiki picture is read from the INSTALLED StarCraft through
  `isom::game_asset` and parsed in Rust (`grp.rs`). Never bundle GRPs,
  palettes or remap tables, and never cache a decoded copy outside the
  process. Without a resolvable install the schema reports `pictures: false`
  with a recovery action and the view stays text-only.
- A GRP is drawn with the palette the engine draws it with: `game\ticon.pcx`
  colours the 16-shade command-icon ramp, `game\twire.pcx` is the full-health
  wireframe palette (its remap rows are the damage tints), and a unit graphic
  is a tileset `.wpe` with indices 8-15 remapped through `game\tunit.pcx`.
  A tileset palette alone renders icons and wireframes in the wrong colours.
- A sheet reaches the panel as ONE grid image plus its frame geometry, never
  as a request per frame: `cmdicons` alone holds 390 frames.
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
- Every build loads the generated `ChkSectionGuard.py` first. eudplib ends the CHK with a fake `ISOM` section of random negative length; when that jump lands inside the CHK, StarCraft's section walk cycles and the game hangs listing the map's folder. The guard redraws it until `len + size < 0`, before freeze keys its protection to the CHK. Never repair it by patching the frozen output.
- An output map whose MPQ block table exceeds 2^18 (262,144) entries fails the build: StarCraft reports it as an invalid or corrupt scenario. freeze writes a decoy block table sized by the compressed CHK (≈ 4.18 MB compressed is the boundary; in game 257,169 entries opened and 263,660 did not), so object count is only a proxy. From 90% of the limit a successful build warns. The error names the recovery (fewer objects, or `[freeze]` `freeze : 0`); it never disables freeze itself.
- A release version lives ONLY in the build output: `settings.outputTitle` makes the generated `OutputTitle.py` rewrite the output map's SPRP scenario name; no plugin is emitted without it. Never edit the source map's title or `SPRP` to show a version — that stays the Map properties request's.

## Release (배포)

- Versions are `vMAJOR.MINOR` and strictly increase. Only `release_prepare` sets the release `outputMap` (`build/[EUD]{title}{version}.<ext>`) and `outputTitle` (`{title} {version}`); a user-named title is stored in `settings.releaseTitle`, otherwise the source map's title (minus a trailing version) is used.
- The changelog source is git: every commit after the newest `release/*` tag (all history without one). `CHANGELOG.md` keeps the newest section on top, written in gameplay terms, never as a file list.
- The tag `release/<version>` goes only on a turn commit of the request whose latest `build_run` succeeded for the current revision, inside the `git_scope` transaction. A failed build, no commit, or no git consent creates no tag and is reported in the turn's git warning; an existing tag is never moved.
- `release_history`/`release_prepare` are EPS-only and never offered to workers or Map sessions.

## Source editing and build verification

- Apply mutually dependent source changes coherently through the native file tools.
- After source, plugin, or Python dependency changes, run `build_run` in the same turn.
- euddraft diagnostics and a fresh output map are the only build authority; repair every reported compiler error before completion.
- Prefer localized `file_edit`; use full `file_write` only for intentional replacement.
- Preserve exact import paths and MainFile composition-root policy.

## Request scope

- Do not schedule post-turn verification harness jobs or require an in-game confirmation
  through the application. Completed changes settle directly; retain journal acceptance,
  workspace revisions, DAT ledger updates, write release and the task-state compiler.
- Preserve existing project documents, memory, approvals, historical promotion audits and
  legacy import sources when removing harness runtime behavior.

- An EPS chat request is one ordinary foreground turn that reads the project, changes it, and
  builds. No triage, research, plan, critique, or verification stage runs before or after it, and
  there is no mandatory staged pipeline. The foreground may start isolated role workers through
  `worker_start` for broad investigation or independent implementation; no tool waits for their turn.
- `propose_plan` is the agent's choice: only when the user asks for a plan or the work is large
  enough that the user should agree with its shape first. Approval then executes the plan.
- A requirements interview (`[clarify]`) runs INSIDE that same turn, through `ask`, never as a
  stage before it: no classifier run decides whether it happens. It runs when the user turns on
  the composer's one-message "인터뷰" toggle (`ChatRequest.clarify`, delivered as
  `[clarify requested]`), or when the agent itself judges that a guessed decision would change the
  shape of the result and be expensive to undo. It never runs for questions, bug reports, targeted
  edits, continuations (a reply to the interview's own unanswered ask resumes it), plan feedback,
  autonomous runs, background workers, or Map sessions; the toggle is disabled during plan review.
  It uses at most three `ask` calls, then writes the confirmed spec to `.eud-agent/workspace/specs/`
  with a line in `specs/index.md` and does the work in the same turn with no approval step.

- Codex foreground native file-change approvals (`item/fileChange/requestApproval` and legacy `applyPatchApproval`) are accepted like native command approvals. Structured jobs still refuse native writes, and explicit permission escalation remains refused.

## Agent tool admission

- Every interactive agent role (EPS, Map, research, map_research and implementation) may freely read, create, overwrite and edit project `.md`/`.txt` documents through `fs_*`, without approval, evidence, write registration or build verification. Documents are not confined to `.eud-agent/workspace`; project-root notes, `maps/`/`references/` documentation and provider skill documents are allowed. `.git`, binary maps/references, provider permission settings and project-root confinement retain their protections. Investigation/Map filesystem writes are restricted to these document extensions, case-insensitively. Native and direct providers advertise the same role-scoped filesystem tools.
- Tool schemas are closed, typed, and validated before dispatch.
- `const`, `enum`, `oneOf`, primitive, array, and object constraints MUST match the advertised schema. A violating call completes with a detailed usage error the model can correct within the run's tool-round budget; it never executes and never fails the run outright. A tool name this session does not offer (`unknown_tool`, naming the exact offered name when the call only wrapped or re-cased one) and non-object arguments (`invalid_arguments`) are the same kind of usage error. Duplicate call ids, an invalid call id or empty tool name, and stale-run dispatch remain fatal admission errors.
- The Claude Code process boundary ends a run only for a tool the CLI opened without this run authorizing it, or for a tool the CLI never opened whose result it reports as successful. A call to a name the CLI never opened (`fs_edit` for `mcp__eud-tools__fs_edit`) is answered by the CLI's own error result and the turn continues.
- Read tools never consume write budgets.
- The Map Agent reads tile ids with `map_terrain_read`/`map_draft_terrain_read` (≤ 4096 tiles per call); probing ids through `terrain.set` expected-before conflicts is prohibited.
- A "사진 배치" request (`imagePlacement`) is enforced by the runtime, not only by the prompt: `map_candidate_finalize` is refused until every bound image is on the current draft through `map_image_place`, and a turn that ends with an image unplaced fails with that message instead of completing silently. Never redraw a photo with terrain patches, brushes, or doodads to satisfy it.
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
- Canonical authoring mutations require evidence and a project write registration; ordinary document filesystem I/O is exempt.
- `worker_start` is admitted only on an EPS session that is not itself a worker (depth 1) and has no live autonomous run, and at most 3 workers per parent are queued or running; the limit is checked and the record written under one store lock. A worker session is never offered `worker_*`, `ask`, `propose_plan`, or `map_task_*`, and a call to one is a usage error. A worker gets the parent's provider binding with an empty conversation; its user message is the goal alone and its rules are the engine's `[background worker]` section on every turn. `worker_start` returns at once; nothing waits for a worker inside a tool call.
- A settled or failed worker continues its parent by itself: one interactive turn on `WORKER_CONTINUE_TEXT`, only while the parent is idle with no live autonomous run, never for a cancelled worker, and never right after a parent turn the user cancelled. Every parent command exit re-dispatches an unreported settlement, and a settlement is marked reported only after the turn that showed it in `[workers]` succeeded.
- Cancelling ANY turn of a parent cancels EVERY queued or running worker of that parent, whichever turn started it, without waiting for their turns; stopping a worker session cancels its worker. A cancelled worker never continues its parent. An autonomous run cannot start while workers are active, a session with active workers (or a worker session still counted active) cannot be deleted, and startup marks queued/running workers `interrupted`, never resuming them.
- A failed or cancelled worker turn keeps what it committed, so `worker_revert` can undo it: newest commit first, stopping at the first conflict, recorded in `reverted`, at most once per worker. `worker_status` stays within 40 KiB double-escaped.
- The EPS session never places terrain, units, buildings, doodads, or sprites itself: it hands that work to its team Map session with `map_task_request`. A team candidate reaches the source map only through the Map window's Apply or the EPS session's `map_task_apply`, which is admitted only after that request inspected the exact announced candidate (`map_task_diff`/`map_task_objects`/`map_task_render`) and runs the Map window's own backup, verification, and rollback; the user can undo either apply from the Map window. A `map_task_request` that revises an earlier task's result (`revisesTaskId` on a candidate_ready or applied task) continues that task's team session, on its candidate while it is ready, and its goal states only the change; every other request runs in a fresh team Map session on the saved source map, superseding a ready candidate, and an earlier team session is retired unless its Apply is still undoable. While a team task is queued, running, or candidate_ready the EPS session's own map writers are refused.
- A team task that settles after its `map_task_request` returned `running` continues the EPS conversation by itself: the engine starts one ordinary interactive turn on the fixed "이어서 진행" text once the session is idle (never over a pending review, plan, or live autonomous run, never for a cancelled task, never across a restart) and announces it through `team_task.continuation` so the panel marks the internal turn in flight. Worker and Map task continuations never append synthetic user chat messages. A settlement the engine could not pick up reaches the model through `[map tasks]` on the next user turn.
- A team task goal relays the user's request, never the EPS session's design: a few sentences as the user would type them in the Map window — the user's wording and intent in the user's language, the reference area to match, and only the constraints the code depends on (keep-clear cells, location ids, walkability). Bounds go in the request's `target`/`protect` rectangles, which the verifier enforces with the task's layers, never in goal prose. One task covers one area or feature; larger work is a sequence of tasks. The EPS session never chooses the medium, counts, palette ids, cluster coordinates, or a layout, never pastes `map_info` tile ids that are not themselves a constraint, and grants every layer the look may need. The team wrapper tells the Map Agent to work the goal exactly like a Map-window request (palette, draft, render, analyze, iterate, one finalize) and that the design is its own; a team session has the same engine, prompt, provider binding, and tools as the Map window.
- A doodad is laid out as StarCraft's CV5 defines it: the DD2 id is the CV5 `ddDataIndex`, row `y` of the footprint is CV5 group `start + y`, cell `(x, y)` is tile `16 * (start + y) + x`, and a row cell whose `megaTileRef` is 0 leaves the terrain untouched. Footprint tiles are never derived from a single group, and `replacementTiles` restore only the cells the doodad owned.
- Doodad placement preserves the editor ground in TILE and stamps only MTXM, matching SCMDraft 2. Verification admits unequal cells only when MTXM matches the exact DD2/CV5 footprint tile; malformed section lengths, unrelated differences, and changes to protected cells remain errors. Terrain diffs include changes to either MTXM or TILE. Terrain reads expose MTXM as `rows` and TILE as `editorRows` (null when missing or malformed), so restoring original ground can use TILE without guessing.
- An area or region the user asks for is a selection-palette entry saved with
  `map_selection_write`, never a map location: a location is CHK game data the triggers use and
  costs one of 255 slots. Palette labels are unique (the `@` mention finds an area by label), and a
  `team-` selection is the working area of a `map_task_request`, so only that request changes it.
- All skills and both EPS/Map agents follow the shared `data/prompts/map_locations.md` policy:
  bind ordinary saved-region geometry into coordinate/waypoint/mask data; a `map.region` mention
  alone never creates a location. Preserve unions/exclusions and deliberate tile-to-pixel conversion.
  APIs that require locations use a project-owned synchronized runtime pool sized by simultaneous
  live uses, including nested/source-destination uses, with explicit ownership and release lifetimes.
  Dedicated locations require an explicit location request or persistent semantics the pool cannot
  preserve. Inspect existing source/plugin references and actual map capacity before reserving slots;
  never steal existing slots, repurpose Anywhere, or automatically delete locations/expand capacity.
- Journal every accepted semantic mutation with exact before/after state.
- Reject/rollback applies inverse operations in reverse sequence and persists exact canonical state.
- Never advertise obsolete individual DAT setters or bridge commands.
- `audio_ffmpeg` is an ALLOWLIST, never a denylist, and is validated in full before anything runs. Inputs are only request audioRefs or WAV-registered `mpqPath`s of the saved map (read natively and checked against the map inventory sha256); each is opened exactly once as `-i {inN}` with `-protocol_whitelist file` and an audio-demuxer `-format_whitelist` added by the app (every FFmpeg/FFprobe input in the app carries both, so content probing can never pick `dash`/`hls`/`concat` and open another file). The one output is the last argument `{out}/<name>.flac|wav|ogg` (ASCII `[A-Za-z0-9_%.-]`, `%d` only with `-f segment`) inside a fresh per-call request-temp directory. Every other token is an allowlisted audio option with a validated value; every filtergraph name is an allowlisted audio filter, with quotes, backslashes and `/` refused so no `movie`/`amovie`/`sendcmd`/plugin filter and no `/option=file` load is reachable. Sources and whole-stream or unbounded buffers (`anullsrc`, `sine`, `areverse`, `apad`, `aecho`) are not allowlisted, and filters that size a rate, buffer or fan-out (`aresample`, `asetrate`, `asplit`, `concat`, `amix`, `amerge`, `aloop`, `atempo`, `adelay`, `aformat`) accept only listed options within fixed ranges. `-segment_time`/`-segment_times` are floored at 0.05 s. Raw paths, URLs, protocol options, `-/option`, `-filter_script`, report/progress/stats/attachment files and non-audio muxers are rejected. It writes only request temp: no write lane, no project transaction, no write budget; EPS sessions only. Runs are bounded and one job runs at a time: 150 s wall and 160 s CPU for FFmpeg, 200 s total; output entries (128), bytes (512 MiB) and resident memory (2 GiB) re-measured every 5 ms poll; per-file size (`RLIMIT_FSIZE`, Unix), memory (`RLIMIT_AS` on Linux, job `ProcessMemoryLimit` on Windows; macOS relies on the poll) and a new session plus CPU limit (Linux parent-death SIGKILL) so an orphan cannot outlive a crash for long. Outputs get `-vn -sn -dn -map_chapters -1`, never use a Windows device stem, are probed under the normal audio limits, and each becomes a new request audioRef.
- Audio that already sits in the project root reaches a request only through `audio_from_project({paths})` (EPS only, 1..=64 root-confined regular files recognized as audio, copied into request temp before binding). The model never asks the user to attach a file the project already holds. A `project.path` mention carries only a path; it never stages bytes or grants a write.
- `map_sound_import` takes exactly one of `audioRef` or `audioRefs` (1..=128). A batch normalizes every ref and checks free WAV slots for all new sounds before writing, then registers them in ONE MapSafe transaction (one backup, one native write, one post-verify that restores the exact pre-batch bytes on failure); results follow input order.
- `map_sound_remove` takes 1..=512 exact `map_sound_list` mpqPaths (managed or not), each naming exactly one WAV registration, and removes every slot, game string and MPQ asset in ONE MapSafe transaction. The native side refuses the whole call before any mutation when a string is still used by any other CHK user (trigger or briefing Play WAV, location, unit, force, switch, scenario text, another WAV slot); it never deletes such a reference to make room. The EPS session deletes the removed paths' EPS references and builds.

## Mid-turn steering

- Enter while a turn runs steers that turn (`chat_steer`, `map_agent_chat_steer`); there is no panel queue. The model receives the message at its next tool boundary; a steer never interrupts, cancels or restarts a turn.
- The steer inbox is per session and never behind the engine mutex. It opens when a chat command is admitted (before context assembly) and for every foreground run, through a guard that also closes it when the turn future is dropped. A push and the close are decided under one lock. A push is refused once another turn opened since the steer was resolved (`push_for` generation). A steer's refs bind only to the request its turn began (`bind_request`/`request_for`; it waits while the turn assembles its context). After the close a push joins steers the turn left over, which stay in the inbox until the follow-up turn opens it and takes them in one step; with none left it answers `no_turn`, never queues silently. A `no_turn` message is sent as an ordinary turn only after the panel saw the turn end, and goes back to the composer when that turn was cancelled or ended in a plan review.
- A steer is resolved like a chat message (attachments, audio bound to the running request, Map images, mentions) and framed `[user message during this turn]`. Every baseline (EPS, worker, Map) states such a block is the user's own instruction and takes precedence over the work in progress. Map mention snapshots in a steer are reference only; they never widen the running request's authority.
- Direct providers append a steer as a `User` block of the running request in the batch's own resumable checkpoint (after every result, batch boundary and provider continuation kept); never publish it as a separate non-resumable context checkpoint. History builders keep tool results first (Anthropic/Gemini merge it into the tool-result user content). Codex uses `turn/steer` against the active turn id, one at a time, racing the cancel. Claude Code keeps stdin open on a writer task (never await a pipe write in the read loop) with `--replay-user-messages`, applies a steer on its echoed uuid, closes stdin on `result`, and ends the process at the result when a written steer was not echoed, discarding what it prints after. A bad steer (unreadable image) is refused or returned alone and never fails the running turn. Native providers announce `applied` through the adapter event channel so it follows the tool events before it; Codex also sweeps acceptances whose event landed after `turn/completed` at turn end, for that turn only. The panel places the row only once no tool of the turn still runs, places it on every ending path (answer, error, failure, cancel), and never carries a parked row into the next request.
- A steer the turn never received is never dropped: after a user cancel, in a worker session, and while an autonomous run is not over it returns to the composer (`returned`); otherwise it starts the next ordinary turn (`turn`) on the dispatcher, or one more Map request in `map_agent_chat` (a Map steer with mentions returns to the composer). A team Map run returns them to the Map composer. The phone resends a steer whose answer was lost under the same request id and never hands it back on a lost connection.
- While a turn runs the composer sends only text, attachments and mentions: 인터뷰, skill ON/OFF, model selection and 사진 배치 are disabled. A pending question is answered on its card and plan feedback keeps its own path; neither steers.
- Applied steers are ordinary user rows of the panel log at the point the turn received them, so reload and transcript replay keep them; a waiting steer is shown as "전달 대기" and is never persisted. A steered row has no `clientTurnId`; editing it rewinds to the transcript before it like any message.

## Continuity across an interruption

- A message that points back at earlier work ("continue", "계속", "그거 해줘") names work only the
  session's own conversation can identify. When the provider conversation is reset, rewound, or
  cannot be resumed, the next turn receives the session's condensed transcript instead of starting blind.
- Model-facing transcripts are bounded by dropping the OLDEST rows and marking the cut. A
  head-first cut hands the model the opening of a long session and none of the work it must
  continue.
- A chat turn persists the provider conversation it reached on every exit path, including failure
  and cancellation. A boundary that exists only in memory is lost with the process.
- A native provider publishes its resumable session identity before any output, and an interrupted
  run keeps it as that run's boundary (receipt state `interrupted`). A run the runtime cancels or
  times out before the CLI published anything keeps the session it asked to resume; a CLI that
  fails on its own before publishing may have failed on that session and leaves no boundary. A session the run did not ask to resume
  is a protocol deviation and is never adopted, and it leaves no boundary at all; a run with
  neither an observed session nor a resume target stays unknown and fails closed. The runtime seeds
  the adapter onto the boundary it records, because a cancelled or timed-out step never lets the
  adapter settle itself. An interruption costs the unfinished turn, never the conversation.

## Sessions and concurrency

- The runtime project root comes only from a `ProjectHandle` of the `ProjectRegistry`; a command resolves it from the calling window's binding (or a remote connection's bound label) and a windowless path from the session's `projectId`. `config.project_path` only records the last opened project for the launcher; never read it to decide a runtime root.
- A `ProjectId` is derived only from `NativeProject::root()` (`ProjectId::of`); never hash a path string another way.
- Project-owned events go only through `project_events::emit_project` to that project's windows with its `projectId`; never `.emit(` them to every window, and drop one whose project is not open.
- Coordinator lock keys are namespaced `eps:<id>` and `map:<id>`; never merge them (`map_task_apply` runs a Map apply inside an EPS transaction).
- Conversation events are session-owned.
- Turns may overlap; individual project writes serialize through `ProjectWriteCoordinator`. An EPS chat turn runs with write access from its first call; never reintroduce a refused-mutation read→write restart for it.
- A turn commit records only what its request owns: the dirty paths minus those claimed only by other in-flight requests of the same `git_scope`. Every writer claims its paths — argument-path writers before dispatch, journal targets after success, provider-native edits from their observations (Claude Code `Edit`/`Write`/`MultiEdit`/`NotebookEdit`, Codex `fileChange`), including observations whose arguments were omitted for size. A path two in-flight requests both claimed is committed with a `shared-with:` trailer and the warning "다른 세션과 같은 파일을 함께 변경했습니다", never refused. The turn-start external-edit commit never takes an in-flight claim. With no other request's claim, a turn commits everything dirty. Codex shell writes name no path and stay unattributed.
- Every git mutation (turn and external-edit commits, history revert, `worker_revert`) runs inside `ProjectWriteCoordinator::transaction` for `git_scope`, which EPS and Map sessions share; never run a git mutation outside that transaction. `git::revert` refuses only over staged changes or dirty paths the reverted commit touched, never over unrelated work in progress.
- Loading or renaming a session never steals another session's execution lane.
- ASK/plan/changeset state survives the documented reconnect/session restore boundary.
- Tauri listener readiness and native project availability are independent states.
- Project unavailability gates authoring without pretending the in-process transport disconnected.

## Scoped workflow state and recovery (2026-10-02)

- Treat the session task projection and its replayed events as the authority for scoped workflow state. `SkillSelectionChanged` owns explicit ON/OFF per skill id (a missing id in older events means `luck-defense`); `WorkflowProgressReplaced` owns backend-validated mode, phase, action, prerequisites, pending playtests, evidence and provenance. A caller cannot author completed-evidence identifiers or hashes.
- Preserve the manual selection tri-state per skill id at the IPC boundary (`skillSelections`): visible persisted ON sends `true`, an explicit OFF sends `false`, and an untouched or settled OFF omits the id. Unknown ids are rejected before the turn starts. Retry/edit preserves that directive and `clientTurnId`; the captured retry directive wins. Synthetic continuations use `None` and retain durable state.
- Register skills only through `skills/<id>/SKILL.md` plus the static registry list; the panel reads the list from `skill_list`. Only luck-defense has legacy marker/keyword activation and mode slices; other skills activate only by explicit selection.
- An enabled unclassified branch must use the compact router and record progress before scoped mutation. `new_build` and `continue_existing` retain the final acceptance contract; `local_edit` stays within shared safety, the requested local scope and pending acceptance. A legacy marker or natural-language activation is router-only when no explicit selection exists, and explicit OFF suppresses both.
- Seed EPS-to-Map provenance once from the parent. Map-local replacements preserve that origin. Stage Map progress by session, request, client turn and expected task revision; publish only after successful candidate commit, binding the exact committed candidate revision/hash. Text-only completion may be advisory with no evidence. Errors and cancellation discard staged progress and checkpoint writes while preserving prior committed state.
- Keep accepted Map updates in an ordered batch of at most eight against the original durable CAS base. Persist the application-owned intent before candidate commit, retain the precommit receipt, and publish with one session save. Never rebase a stale receipt onto the current revision or replay candidate mutation during recovery. Checkpoint failure after authoritative publication is advisory.
- Read task-state, full `SessionRecord` and checkpoint schema 1, then write schema 2 on the next ordinary save; reject future versions. A revision change makes prior evidence stale. Checkpoints, prior answers, worker reports and `last_completed` cannot grant permission or prove gameplay.
- Worker implementation units verify their owned units; the parent validates the integrated current revision. Investigators own their assigned result documents; the parent owns shared accepted specifications. Worker status never advances the parent workflow by itself.

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

## Remote access

- The remote server binds `127.0.0.1` only and is off by default. Reach from a phone is `tailscale serve`'s job; never bind another interface.
- Every `/remote/api` path except `pair` requires a paired device token, inside the tailnet too. Store only the token's SHA-256; pairing codes are single use, expire, and lock after repeated wrong guesses. A token unused for 30 days stops working (401 `code: "expired"`, Settings shows 만료됨); the phone says to pair again.
- Through `tailscale serve` (a `*.ts.net` Host) a device is bound to the `Tailscale-User-Login` that paired it: pairing there requires the header, and every later request must carry the same login (401 `code: "identity"`). Serve sets the header for tailnet users and strips a client's (tailscale.com/kb/1312/serve, "Identity headers"). Only a request from the PC itself stays token-only: a loopback Host AND none of the headers serve always sets (`X-Forwarded-For`/`-Host`/`-Proto`, any `Tailscale-*`). Serve forwards the client's Host unchanged and routes by SNI, so a Host alone never makes a request local; use that one classification (`is_local_request`) for identity, pairing, the Funnel refusal and the Origin comparison (a proxied request's Origin is compared with `X-Forwarded-Host`).
- Never serve Funnel traffic: a request carrying `Tailscale-Funnel-Request` is refused, and while `tailscale serve status` shows `AllowFunnel` for this port every request that is not the PC's own is refused and Settings warns.
- Refuse a Host that is not loopback or `*.ts.net` and a write whose Origin does not match. Every `/remote` answer carries the remote CSP (`default-src 'self'`, `connect-src 'self' wss:`, `frame-ancestors 'none'`, `base-uri 'none'`), `X-Frame-Options: DENY` and `Referrer-Policy: no-referrer`; every `/remote/api` answer is `Cache-Control: no-store`. Serve only `remote.html`, its PWA files and icons, and `assets/*`; never the desktop pages.
- Bind with `SO_EXCLUSIVEADDRUSE` on Windows. The event socket accepts messages of at most 64 KiB; at most two attachment uploads stage at once, more are refused.
- Remote commands are an explicit allowlist in `remote/dispatch.rs`, each calling the Tauri command's own `*_inner`. Never forward the whole `generate_handler!` list. Native pickers, path settings, updater, shell open, E2/E3S import, SCMDraft launch, provider defaults, Tailscale install/sign-in/serve and Map canvas/selection/properties/export stay on the PC.
- The app installs Tailscale only from the pinned official MSI (`tailscale_install.rs`: version, exact size and SHA-256 per architecture). The hash is checked through a read-share-only handle that stays open until `msiexec` exits, and msiexec gets the path that handle resolves to (never the requested path, which a junction could redirect), so the verified bytes are the installed bytes; msiexec runs unelevated so Windows Installer's own UAC prompt shows the package signer; a mismatch deletes the file and installs nothing. Never install from a URL or digest fetched at runtime. A pin bump re-hashes the downloads and runs the ignored network test. Sign-in is a bare `tailscale up`, never `tailscale login` (it switches to a new empty profile) and never an `up` with flags (Tailscale refuses flags that would revert current settings).
- A dispatched remote command runs on its own task: a closed phone request never cancels a turn. A request id the server already ran (`x-eud-request-id`, keyed by device + command + id) joins that command or returns its answer; never run a resent request twice. The ledger never evicts a running command; it remembers the answer of a state-changing or long command (`REMEMBERED_COMMANDS`) for 10 minutes after it FINISHED, within 64 per device, 512 overall and 8 MiB of answers (oldest finished first), and forgets a read-only one once it finished. A full ledger of running commands refuses the new one ("요청이 너무 많습니다…"), never runs it unremembered.
- Removing a device (Settings, or logging out on the phone) closes its event sockets at once; turning remote access off or restarting the listener closes every socket. Each closed socket unbinds its label. A project closing on the desktop tells its sockets `detached` at once.
- A `remote-<device>-<conn>` label is a connection, never a project window: it never receives the per-window fanout, never counts as the project's desktop window, and its session/Map commands are admitted only for the attached project.
- A project has at most one desktop window plus any number of remote observers. A phone opens only a recent project and closes one only through the close gate; a dropped socket never closes a project. A remote close refusal carries a code (`busy`, `building`, `companions`, `switching`); the phone offers 작업 중단 후 닫기 only for `busy`/`building`, never by matching the wording.
- A window leaves a project without admission only through `ProjectRegistry::leave_if_observed`, which checks for a remote observer and unbinds in one step. `leave_project` never closes the project: an observer that detached between a check and the leave leaves it to `close_admitted` (a busy project still asks) or to the close the user already confirmed.
- Closing the PC window while its Map window runs a turn with a phone attached keeps the turn running for the phone: the Map window closes with the main window, its work does not.
- Map commands resolve a `MapCaller`; a phone never consumes the desktop Map window's pending open or properties request.
- Long remote turns recover on the client: a lost request (`CONNECTION_LOST_MESSAGE`) is sent again with the SAME request id once the socket is back (at most 5 times), so the ledger joins the running turn or returns its result and a request that never arrived runs now; the session is then reloaded. Never make a remote command return early or change its desktop semantics to dodge the wait.
- After a reconnect, commands wait for the re-attach (`attached`/`attach_failed`) before they are sent.
- A phone saves an EPS or Map panel log only after it took part in that conversation or while no PC window shows the project, and never just for opening it. A PC main window saves every conversation of its project, shown in a pane or not. The phone flushes a pending save before a resync replaces its store, saves a streaming turn at least every 3 s, and checks any save of a conversation loaded before the latest reconnect (still in its timer or already on its way) against the saved log right before it is sent, dropping it when the saved `logSeq` is newer.
- A session's `session_activity` reaches only sockets attached to that session's project.
- The tray keeps the process alive only while remote access is enabled; 종료 still asks over active work.

## Panel UX

- Only queued/running workers appear beneath their parent sessions in the sidebar. Groups have independent keyboard-accessible disclosure controls that preserve conversation focus; search reveals matching workers. A worker leaving queued/running closes its pane and disappears from navigation; if focused, its parent becomes selected, and a sole worker pane switches to the parent. Reload selects a main thread and keeps completed workers out of navigation.
- Codex investigation workers retain native-tool restrictions but use `on-request` approval for the app MCP endpoint, including resumed threads. Only `eud-tools` MCP tool-call approvals are accepted; profile allowlists and write admission still govern execution.

- All user-facing text is Korean.
- Use semantic controls, visible focus, keyboard operation, and descriptive aria labels.
- Long operations disable their trigger and show progress within the same surface.
- Project setup order is project → euddraft → assets → provider selection → provider connection.
- Project actions expose open/create/import; settings also exposes export. Create offers both "existing map" and the blank-map wizard; the wizard is launcher-only and never an agent tool.
- The header's "프로젝트 빌드" runs the SAME generator, build marker, and diagnostics as `build_run`; it never becomes a second build path. It hands the panel only structured diagnostics plus the bounded output excerpt and the log path — never the raw streams — and reports a refused or failed build with its recovery action in the 빌드 결과 dialog instead of a toast.
- The blank-map wizard fails closed without a resolvable StarCraft data folder and offers the folder picker in place; it never generates terrain from bundled or synthetic tileset data.
- Every ordinary launch requires explicit project selection; a file launch opens only its requested project. Stored config alone must not activate project polling, session restoration, or project-dependent bootstrap.
- Recents are newest-first by successful explicit open; failed/canceled actions do not change selection or recency. Removing history must never delete project files.
- File association is `.eap` only, never all `.json` files. The `.eap` file contains the canonical manifest directly. Legacy `project.json`/`.eudproj` may be migrated only on explicit open after validation; never generate descriptors or maintain two manifest authorities.
- A switch closes a project only while that project has no active work; work on other projects never blocks it. A window close is NEVER refused: over active work it asks "창을 닫을까요?" with 닫기/취소, and 닫기 cancels that project's turns, pending questions and background workers without waiting, then closes the window and its companions. Durable sessions and pending plans survive for the next open. While a remote client is attached, a closing or switching window only leaves the project: nothing is cancelled and no confirmation is asked. Preserve forwarded requests for visible retry rather than dropping them or switching under a running agent/build.
- Reopening a project that is already open is refused with "이미 다른 창에서 열려 있습니다" and focuses its window; for a native project the refusal happens before any migration or recents change (a legacy project is identified only after its conversion), and a window never shares a project with another desktop window. Remote observers are not windows: a desktop open of a project only remote clients hold attaches to it.
- "새 창에서 열기" and file launches open another project in its own `main-<suffix>` window; the launcher window takes a request only while it has no project and nothing queued.
- Error text states a recovery action and never leaks raw protocol identifiers as the only explanation.
- The main window's chat tab holds at most 4 split panes (`paneLayout.ts`), each showing a different session; the last pane never closes, and the layout persists per project in localStorage. There is ONE composer, and it sends to the focused pane's session. Interactive surfaces (question cards) render only in the focused pane; any other pane that waits for an answer or plan review shows "응답 필요". A worker session's pane and sidebar row carry the "작업자" badge, and a newly started worker opens its own pane while fewer than 4 are open.
- Use Lucide/vector icons, semantic theme tokens, and reduced-motion classes; no emoji structural icons.
- Every panel control is a shadcn/ui primitive from `panel/components/ui` (Button, Input, Textarea, Select, RadioGroup, Checkbox, Switch, Dialog, Tabs, ...). Native `<select>`, `<input type="radio">`, `<input type="checkbox">`, or hand-styled equivalents are prohibited; a missing primitive is added with the shadcn CLI (`npx shadcn add <name>` in `panel/`), never hand-rolled.

## Verification

- Bug fixes reproduce and then remove the observed failure.
- Permanent contracts get behavior tests, not source-text tests.
- Rust full suite, panel full suite, TypeScript build, and panel production build MUST pass before release.
- Native build acceptance MUST run the Rust generator and a real configured euddraft against a real SCX.
- E3S acceptance MUST include real import→export→import equality and original .NET BinaryFormatter deserialization.
- UI changes MUST be browser/Tauri-surface verified, not inferred from unit tests.
- Panel event subscriptions MUST use `listenToCurrentWindow` (`panel/src/lib/windowEvents.ts`). Tauri's global `listen` receives every `emit_to` in the project's per-window fanout, duplicating streams and final answers when companion windows are open. Current-window targets still receive process-wide broadcasts.
- Remove temporary fixtures, diagnostic scripts, obsolete tests/docs, and generated smoke files after verification.

## Direct Python dependencies

- Python is always active and full-trust; never describe an import allowlist as a sandbox.
- Only ordered manifest entrypoints execute before the EPS MainFile. All `src/**/*.py` remain part of revision, snapshots, search, review, and rollback.
- Exact direct PyPI pins and the complete wheel lock live only in `project.eap`; caches and preparation tokens are derived.
- Preparation holds no project write lease. Token commit rechecks identity/revision/manifest, journals exact before/after bytes, and retains normal review ownership.
- Managed uv/wheels/environments are checksummed, reparse-safe, atomically published, frozen-runtime probed, and fail closed offline.
- E3S export refuses any project containing direct Python state; EPS-only compatibility is unchanged.
