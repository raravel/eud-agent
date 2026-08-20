# epScript Project Architecture and Main-File Discovery — Authoritative Implementation Plan

Status: authoritative implementation plan; implementation has not started.

This is the single implementation plan for teaching the agent to place epScript code by project architecture while using EUD Editor's configured start file as the only source of truth for the main file. Do not create a competing plan. During implementation, update the current-behavior feature documents named below; those documents describe the resulting system, while this file retains the implementation decisions and sequencing.

## Goal

Make file placement a grounded architectural decision instead of a filename or line-count heuristic:

1. expose the exact EUD Editor start file (`pj.TEData.MainFile`) to the agent through the normal read-tool surface;
2. preserve that file as the composition root regardless of its name;
3. guide the agent to place behavior with the subsystem that owns its state and invariants;
4. create new files only for real cohesive responsibilities;
5. keep imports directional and acyclic;
6. preserve localized-change discipline so a small fix never triggers an unrelated project split;
7. persist concise file-role knowledge after structural changes;
8. retain complete-batch `eps_check` and mandatory `build_run` verification.

## Ground Truth

The implementation MUST build on these existing facts rather than adding a second convention:

- EUD Editor's Settings → start-file value is `pj.TEData.MainFile`. A file named `main`, `main.eps`, or similar has no special meaning by name.
- `bridge/ZZZ_10_agent_bridge.lua` already implements `GETMAIN`. `mainFilePath()` walks `pj.TEData.PFIles` and returns the current main node's complete `/`-separated project path, or an empty string when no start file is configured.
- `SETMAIN` already assigns `pj.TEData.MainFile`; `file_move` preserves object identity and `file_delete` clears a deleted main file.
- `SessionToolRuntime::set_main` already calls `GETMAIN` internally to journal the prior value, proving the bridge contract is available, but the normal read-tool catalog does not expose it to Codex.
- `project_status` currently returns only `{ "status": <raw STATUS reply> }`.
- `list_files` returns file path, type, and settable state, but it does not identify the start file.
- `EPSCRIPT_GUIDE` explains syntax and lifecycle hooks but contains no project-placement policy.
- `EPS_PREFLIGHT_GUIDE` already supports one coherent candidate batch for mutually dependent files.
- Project memory `structure` is already the durable one-line role summary per project file.
- The connected validation project demonstrates the ambiguity: a `ClassicTrigger` node is named `main`, while the configured CUIEps start file is `survivor_mvp`. Correct behavior must select `survivor_mvp` without using either name as a heuristic.

## Scope

### Included

- A typed Rust `BridgeIo` read wrapper for the existing `GETMAIN` command.
- Additive `mainFile` output on the existing `project_status` read tool.
- Migration of the existing `set_main` before-snapshot read to the same wrapper, so empty/error interpretation has one implementation.
- A concise, always-available epScript architecture guide in agent prompt assembly.
- Placement rules for existing owners, new feature modules, shared leaf modules, composition-root responsibilities, imports, and localized fixes.
- Mandatory `structure` memory refresh after file topology or file-role changes.
- Unit/static/live-smoke coverage for start-file identity and prompt behavior.
- Current-behavior documentation updates after implementation.

### Excluded

- Renaming existing project files to `main` or `main.eps`.
- Automatically calling `set_main` when EUD Editor already has a configured start file.
- Automatically refactoring the user's existing 1,300+ line project as part of this eud-agent product change.
- A general-purpose architecture analyzer, dependency-injection framework, or new project-template subsystem.
- Blocking builds based on file length, module count, or architecture style.
- Changing epScript import semantics or the pinned analyzer parser.
- Adding a new bridge command; `GETMAIN` already exists.
- Changing the `LIST` wire format. Existing consumers depend on `path<TAB>ftype`.
- Treating `structure` memory as executable truth over the live editor; live `GETMAIN`, `LIST`, and source reads remain authoritative.

## Decisions

### D1 — `GETMAIN` is the only MainFile authority

The agent MUST consume the live path returned from `pj.TEData.MainFile` through `GETMAIN`. It MUST NOT infer the start file from:

- a filename;
- list order;
- the selected or last-open editor tab;
- the only `.eps` file;
- the file containing lifecycle hooks;
- project memory.

Project memory may describe the known role, but a conflicting live `GETMAIN` value wins.

### D2 — Extend `project_status`; do not add another agent tool

`project_status` is already the natural first read for project-aware work. Extend its result additively:

```json
{
  "status": "compiling=False\nproject='E:\\maps\\demo.e3s'\nversion=0.19.6.0",
  "mainFile": "scripts/survivor_mvp"
}
```

When no start file is configured:

```json
{
  "status": "compiling=False\nproject='E:\\maps\\demo.e3s'\nversion=0.19.6.0",
  "mainFile": null
}
```

Contract details:

- `mainFile` is the canonical project-relative `/` path returned by `GETMAIN`, without inventing or normalizing a display filename.
- An empty successful `GETMAIN` reply maps to JSON `null`.
- `ERROR: no project` maps to `null` only for the expected no-project state; unexpected transport, timeout, or bridge errors remain visible failures.
- The existing raw `status` field remains unchanged for compatibility.
- Do not duplicate file type in `mainFile`; `list_files` remains the authority for path/type/settable metadata.

### D3 — One typed bridge wrapper owns empty-path interpretation

Add a `BridgeIo` method with this logical contract:

```rust
pub fn get_main(
    &self,
    opts: &SendOpts,
    on_busy: Option<&dyn Fn()>,
) -> Result<Option<String>, BridgeError>;
```

It sends `GETMAIN`, trims only protocol whitespace, returns `None` for an empty success, and otherwise returns the exact project path. `SessionToolRuntime::project_status` and `SessionToolRuntime::set_main` MUST both use this method. Remove the raw `self.send("GETMAIN")` interpretation from `set_main`; there must be one code path for this protocol result.

No bridge rewrite is needed. Preserve the verified import-then-extend Lua implementation.

### D4 — The configured main file is the composition root, not necessarily `main.eps`

When `mainFile` is non-null, the agent MUST preserve that exact file as the composition root regardless of its filename. For new architecture introduced into an existing project:

- keep lifecycle entry functions and explicit subsystem call order in the configured main file;
- import feature modules from that file;
- do not rename the main file for style;
- do not call `set_main` merely to normalize naming;
- do not modify a same-named non-main file because its name looks conventional.

For the connected validation project, the intended result is `survivor_mvp` as composition root. The `ClassicTrigger` file named `main` remains unrelated and read-only.

### D5 — `mainFile: null` is explicit ambiguity, never permission to guess

When EUD Editor has no configured start file:

- a new, otherwise empty project may receive a newly created CUIEps composition root and `set_main` as part of the requested initialization;
- a non-empty existing project requires the intended MainFile selection to be explicit in the reviewed plan before `set_main` executes;
- a localized edit that does not require changing the start file proceeds without opportunistically setting one;
- the agent MUST NOT silently choose the only CUIEps file.

### D6 — Place code by state and invariant ownership

The placement decision order is:

1. Inspect `project_status.mainFile`, `list_files`, project memory `structure`, and relevant `source/` files.
2. Identify the subsystem that owns the mutable state and invariant changed by the request.
3. If an existing file owns that responsibility, edit that file.
4. If no existing owner exists and the behavior forms a cohesive subsystem with a narrow API, create a feature module.
5. If behavior coordinates multiple peer subsystems, keep coordination in the composition root rather than reaching into peer internals.
6. Extract a shared leaf module only after at least two real consumers need the same stable contract.

A new feature module is justified when at least two of these are true:

- it has an independently nameable game responsibility;
- it owns meaningful runtime state or resource allocations;
- it has independent initialization or per-tick work;
- it is likely to change independently from neighboring systems;
- it can expose a narrow API without sibling modules mutating its internals.

### D7 — Keep imports directional and acyclic

Preferred dependency direction:

```text
configured MainFile -> feature modules -> stable leaf modules
```

Rules:

- feature modules own their internal mutable state;
- sibling feature modules do not reach into each other's internal state;
- cross-feature sequencing belongs in the configured main file or an explicit narrow API;
- stable constants/resources may live in leaf modules;
- leaf modules do not import feature modules;
- a new `EUDLSP002` import-cycle warning introduced by the candidate change must be removed;
- unrelated pre-existing cycles are reported but not opportunistically refactored.

### D8 — Avoid both monolith bias and over-splitting

The agent MUST NOT create empty scaffolding or generic dumping grounds such as `utils`, `common`, `helpers`, or `state` without a concrete stable responsibility.

File length is an advisory review signal only:

- re-evaluate cohesion when the configured main file contains feature implementation;
- re-evaluate cohesion when a handwritten epScript file exceeds 800 nonblank lines;
- do not split generated, declarative, or table-heavy code solely by size;
- do not split code when the resulting modules would continuously mutate each other's internal state.

No runtime or tool gate rejects a file because of line count.

### D9 — Localized changes stay localized

Architecture guidance MUST NOT turn every request into a refactor. For a bug fix or small feature that clearly belongs to an existing module:

- modify the owning module only;
- do not move unrelated code;
- do not split a monolith unless the requested/planned work includes that split;
- treat broad file moves, renames, and responsibility changes as reviewed planned work.

### D10 — Persist the resulting file-role map

After `file_create`, `file_move`, `file_rename`, `file_delete`, a MainFile change, or a material file-responsibility change, the agent MUST rewrite project memory `structure` as a complete replacement. Each line records:

```text
<path> — <single responsibility>; imports <direct project dependencies>
```

Example:

```text
survivor_mvp — composition root and lifecycle ordering; imports systems/player, systems/combat
systems/player — player initialization and per-player state; imports config/resources
systems/combat — combat and boss rules; imports config/units
config/resources — stable resource allocations; no project imports
```

Do not update `structure` for a localized implementation change that leaves file roles and dependencies unchanged.

### D11 — Multi-file correctness remains one batch plus one authoritative build

For creates, full rewrites, or exact edits that depend on each other:

1. pass all candidates to one `eps_check` call;
2. correct errors and candidate-introduced import cycles;
3. apply the complete intended file changes;
4. run the mandatory `build_run` in the same turn;
5. repair within the existing three-build-attempt budget;
6. update `structure` memory when topology or roles changed;
7. present one coherent changeset for review.

Architecture guidance never weakens the existing build contract.

## Canonical Prompt Block

Add one concise constant in `src-tauri/src/engine.rs`. Its normative content is:

```text
[eps project architecture]
- Optimize for change locality, clear ownership, and explicit dependencies — not for the fewest or smallest files.
- Before placing code, inspect project_status.mainFile, list_files, project memory structure, and relevant source files. Never guess the MainFile from a filename, list order, open tab, lifecycle hooks, or file count.
- Preserve a configured MainFile as the composition root regardless of its name. Keep lifecycle entry functions and explicit subsystem call order there; never call set_main merely to normalize naming.
- Put behavior in the module that owns the mutable state and invariant being changed. Edit an existing owner; create a module only for a distinct cohesive responsibility with a narrow API.
- Keep imports directional and acyclic: configured MainFile -> feature modules -> stable leaf modules. Sibling modules must not mutate each other's internal state.
- Do not create empty scaffolding or generic utils/common/helpers/state dumping grounds. Extract shared code only after two real consumers need the same stable contract.
- Preserve the established layout for localized fixes. Broad splitting, moving, or renaming is planned work, not incidental cleanup.
- File length is only a review signal: re-evaluate handwritten files above 800 nonblank lines and any MainFile containing feature implementation; never split generated/table-heavy or tightly coupled code solely by size.
- If mainFile is null, never infer one. A new empty project may create and set a composition root; a non-empty project requires the selection in the reviewed plan.
- After file topology, MainFile, dependency, or responsibility changes, rewrite memory structure with every file's current role and direct dependencies.
- Preflight every mutually dependent candidate in one eps_check batch, then run the mandatory complete-project build.
```

Prompt placement:

- in `build_system_prompt`, place it after `EPSCRIPT_GUIDE` and before `EPS_PREFLIGHT_GUIDE`;
- in `resume_turn_text`, include it after `EPS_IDIOMS` so existing saved threads receive the current architecture contract instead of retaining only their original system prompt;
- preserve `[first principles]`, RAG/reference ordering, and message-boundary invariants.

## Implementation Sequence

### Phase 1 — Expose the live configured MainFile

Target files:

- `src-tauri/src/bridge_io.rs`
- `src-tauri/src/tool_exec.rs`
- `src-tauri/src/tools.rs`

Work:

1. Add `BridgeIo::get_main` over existing `GETMAIN`.
2. Extend `project_status` output with additive `mainFile: string | null`.
3. Update the tool description to state that it returns the exact configured EUD Editor start-file path.
4. Migrate `set_main`'s prior-value capture to `BridgeIo::get_main`.
5. Preserve read-tool classification and action-budget behavior; this remains non-mutating.

Exit condition: a normal agent turn can distinguish a file named `main` from the actual configured MainFile without using the debug `LUA` channel.

### Phase 2 — Install the architecture decision policy

Target file:

- `src-tauri/src/engine.rs`

Work:

1. Add the canonical prompt block.
2. Include it in cold-start and resume prompt assembly at the specified positions.
3. Keep the prompt concise and avoid restating epScript syntax, preflight, build, or workspace rules already owned by adjacent guides.

Exit condition: both new and resumed conversations receive the same MainFile and file-placement policy.

### Phase 3 — Verification coverage

Target files:

- existing unit-test modules in `src-tauri/src/bridge_io.rs`
- existing unit-test modules in `src-tauri/src/tool_exec.rs`
- existing prompt tests in `src-tauri/src/engine.rs`
- bridge static-contract tests colocated with current bridge checks

Required tests:

1. `GETMAIN` root path maps to `Some(path)`.
2. A nested MainFile path is preserved exactly with `/` separators.
3. An empty successful reply maps to `None` and JSON `null`.
4. `project_status` preserves its existing raw `status` value and adds `mainFile`.
5. A file named `main` does not override a different configured MainFile in the returned contract.
6. `set_main` journals the old path through the shared wrapper, including the unset case.
7. `project_status` remains a read tool and does not require a write lease.
8. The architecture guide appears in both cold-start and resume prompts.
9. Cold-start ordering is `EPSCRIPT_GUIDE` → architecture guide → `EPS_PREFLIGHT_GUIDE` → `BUILD_GUIDE`.
10. Prompt tests pin: no filename inference, configured-main preservation, null-main behavior, localized-change discipline, acyclic direction, `structure` refresh, complete-batch preflight, and mandatory build.
11. Existing first-principles/header and reference-context boundary tests remain unchanged.

### Phase 4 — Current-behavior documentation

After code and tests pass, update these existing sources of truth; do not create another plan:

- `hivemind/docs/architecture.md` — read-tool flow exposes live MainFile; project architecture policy consumes it.
- `hivemind/docs/rules.md` — MainFile authority is `GETMAIN`; filename inference and incidental `set_main` are forbidden.
- `hivemind/docs/features/04_bridge-v2-surface.md` — retain existing `GETMAIN`; document its normal read-tool consumer.
- `hivemind/docs/features/05_agent-core.md` — `project_status` result includes `mainFile`; architecture guide behavior.
- `hivemind/docs/features/07_project-memory.md` — structural changes require complete `structure` role/dependency refresh.
- `hivemind/docs/features/18_epscript-lsp-agent-preflight.md` — candidate-introduced cycles are repaired under architecture policy while analyzer severity remains advisory.
- `hivemind/docs/verify.md` — focused Rust test commands and connected-editor smoke.

Do not describe this intended behavior as implemented until the implementation and verification are complete.

## Verification Plan

### Focused automated checks

Run the narrow tests covering the changed contracts first:

```powershell
cargo test -p eud-agent bridge_io --lib
cargo test -p eud-agent tool_exec --lib
cargo test -p eud-agent engine --lib
```

Then run the repository Rust contract:

```powershell
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

No panel command is required unless implementation changes a Tauri IPC type consumed by the panel. The selected design changes only the Codex MCP tool result, so panel work is out of scope.

### Connected-editor smoke

Use a project containing both:

- a non-main file named `main`;
- a differently named configured CUIEps start file.

The current connected project is a valid fixture:

```text
main             ClassicTrigger
survivor_mvp     CUIEps, configured MainFile
```

Required observations:

1. `project_status.mainFile` returns `survivor_mvp`.
2. `list_files` still reports both files with their existing types.
3. The agent identifies `survivor_mvp` as the composition root.
4. A read-only architecture question performs no mutation and never calls `set_main`.
5. A planned modular change proposes imports from `survivor_mvp`, not from the ClassicTrigger named `main`.
6. With the start-file setting cleared, `project_status.mainFile` is `null` and the agent does not infer a replacement.

The smoke is observational unless the user separately approves an actual project refactor.

## Acceptance Criteria

The work is complete only when all are true:

- The normal `project_status` tool returns the exact live configured MainFile path or `null`.
- The implementation uses existing `GETMAIN`; no duplicate bridge command or changed `LIST` format exists.
- `project_status.status` remains backward compatible.
- `set_main` and `project_status` share one typed MainFile-read implementation.
- A file named `main` cannot influence MainFile selection unless EUD Editor's start-file setting actually points to it.
- A configured nonstandard name such as `survivor_mvp` is preserved as composition root without rename or incidental `set_main`.
- The agent has deterministic placement rules based on state/invariant ownership and dependency direction.
- The guide resists both monolith bias and automatic file-count/line-count splitting.
- Small fixes remain localized; broad splits require planned scope.
- Structural changes refresh `structure` memory, while ordinary internal edits do not create memory churn.
- Mutually dependent files use one `eps_check` candidate batch and the resulting project passes mandatory `build_run`.
- Focused tests, workspace tests, formatting, and clippy pass.
- The connected-editor smoke distinguishes `main` ClassicTrigger from `survivor_mvp` MainFile.
- Current-behavior docs are updated only after the behavior exists.

## Risks and Controls

### Extra editor Tick for `project_status`

Reading `STATUS` and `GETMAIN` requires two bridge commands. This is acceptable for an explicit project-inspection tool and avoids destabilizing the existing `STATUS` or `LIST` wire formats. Do not poll `project_status` as a heartbeat substitute.

### Project switch between `STATUS` and `GETMAIN`

The two reads are not atomic. The existing request/project identity and write-rebase rails remain authoritative before mutations. If the project changes, the next turn/rebase refreshes state; no write may rely solely on an earlier `project_status` result.

### Over-splitting caused by prompt pressure

The prompt explicitly makes length advisory, requires real ownership/API boundaries, forbids empty scaffolding and generic dumping grounds, and preserves localized fixes.

### Stale `structure` memory

Live `GETMAIN`, `LIST`, and source remain authoritative. Existing list-hash staleness annotation continues to warn when file topology changed. Mandatory structural refresh reduces, but never replaces, live inspection.

### Existing conversations retain old instructions

Including the concise guide in `resume_turn_text` applies the contract to saved threads without requiring users to discard conversations.

## Implementation Result Shape

For the current connected project, a future separately approved modular refactor would preserve:

```text
survivor_mvp              CUIEps, configured MainFile/composition root
main                      ClassicTrigger, unrelated read-only file
```

Feature modules may then be created and imported by `survivor_mvp`. This plan does not authorize or perform that project refactor; it only makes the product capable of planning and executing such work from the correct live start-file identity.
