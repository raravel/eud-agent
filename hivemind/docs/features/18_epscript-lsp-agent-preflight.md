# Feature 18: Agent-only epScript preflight (epscript-lsp + project import analysis)

- Status: Proposed
- Scope decision: agent coding only; no user editor language features
- Upstream baseline: `zuhanit/epscript-lsp` commit
  `7f175df06ae57e9da65b8add25d084b5f5df0e1f` (`v1.2.14`, 2026-05-26)

## Purpose

Give Codex a fast, read-only epScript preflight before it mutates EUD Editor 3 project files.
The preflight analyzes a batch of complete-code or exact-edit candidate `.eps` files against a
snapshot of the whole editor project, resolves direct and transitive imports, and returns
structured diagnostics to the same Codex turn. Codex fixes the candidates, checks again, and only
then calls `file_create`, `file_write`, or `file_edit`.

This complements, never replaces, the existing authoritative build loop:

1. `eps_check` catches syntax, import, and analyzer-visible symbol mistakes before mutation.
2. `file_create`, `file_write`, or `file_edit` applies only the corrected candidate.
3. `build_run` remains mandatory after file mutations and catches real euddraft/editor semantics.

The existing rules remain binding: diagnostics are advisory, absence of the analyzer never breaks
the turn, and a diagnostic never mechanically blocks a write.

## Grounded current state

- `hivemind/docs/rules.md` already defines epscript-lsp diagnostics as advisory and optional.
- `src-tauri/src/ipc.rs` reserves `ProgressStage::Lsp`, but the Rust backend never emits it.
- `panel/src/components/DiagnosticsStrip.tsx` exists but is not wired into the v2 changeset flow.
- The retired Python v1 had a no-raise, timeout-bounded `lsp_gate.py`; v2 removed Python and did
  not port the runner.
- `ToolRuntime::execute` already runs under `tokio::task::spawn_blocking`, so a synchronous,
  mutex-serialized analyzer client fits the existing MCP execution boundary.
- The Lua bridge already has a debug-oriented `DUMP` path, but it flattens paths and uses shared
  filenames. It is not collision-safe or request-scoped enough for a production project mirror.
- The published `@epscript-lsp/server@1.2.13` package does not declare its runtime dependency on
  `@epscript-lsp/types`; installing the server package alone fails at startup. Runtime `npm
  install` is therefore forbidden.
- A Windows stdio probe succeeded after explicitly supplying the missing types package: a valid
  fixture returned no diagnostics and malformed epScript returned parser diagnostics. The
  upstream analyzer is usable, but its packaging and failure containment must be owned here.

## Goals

- Add one read-only Codex tool, `eps_check`, that accepts one or more complete-code or exact-edit candidate files.
- Analyze candidates in the context of every readable `.eps` file in the current editor project.
- Resolve `import foo.bar [as alias];` against `foo/bar.eps` under an explicit project root.
- Detect missing imports, case-insensitive path collisions, and import cycles independently of
  upstream's incomplete import diagnostics.
- Return diagnostics grouped by project path, including diagnostics from affected dependencies
  and reverse dependents.
- Keep candidate checking non-mutating: no editor object, journal, map, or project file changes.
- Degrade to a successful `skipped` result on missing Node, analyzer startup failure, crash,
  malformed protocol output, or timeout.
- Keep `build_run` mandatory after every applied eps/file change.
- Ship a commit-pinned, checksum-verified analyzer bundle; perform no network or npm operation at
  application runtime.

## Non-goals

- Monaco integration, editable panel buffers, squiggles, or a diagnostics panel.
- Completion, hover, signature help, go-to-definition, document/workspace symbols, semantic
  tokens, formatting, or rename.
- Modifying EUD Editor 3 binaries or making its editor an LSP client.
- Running the epscript-lsp euddraft Build Manager.
- Treating epscript-lsp as authoritative over EUD Editor/euddraft compilation.
- Blocking `file_create` or `file_write` because diagnostics exist or the analyzer is absent.
- Reintroducing Python, a localhost service, Electron, or a bundled Node runtime.

## Architecture

The integration uses epscript-lsp's parser/analyzer behind a purpose-built agent adapter rather
than implementing a general-purpose editor LSP client. The adapter is built from the pinned
upstream source and exposes only diagnostics plus syntax-aware import extraction. This removes
all client capability negotiation and user-facing language features while retaining the relevant
upstream analysis engine.

```mermaid
flowchart LR
    C[Codex turn] -->|eps_check candidates[]| T[ToolRuntime]
    T --> S[Bridge EPSNAPSHOT]
    S --> E[EUD Editor 3 in-memory project]
    T --> W[Local project mirror]
    T -->|framed JSON over stdio| A[Pinned epscript-lsp agent adapter]
    A -->|diagnostics + import graph| T
    T -->|structured tool result| C
    C -->|corrected file_create/file_write| E
    C -->|mandatory build_run| E
```

Dependency direction remains:

```text
Codex -> eud-tools MCP -> ToolRuntime -> EpsPreflight
                                      -> BridgeIo -> Lua bridge -> editor
EpsPreflight -> app-owned mirror -> Node adapter (pinned epscript-lsp core)
```

The Lua bridge never spawns Node and never receives analyzer output. Node is lazy-started by the
external Tauri process only.

## Agent tool contract

Register `eps_check` as a non-mutating tool in `src-tauri/src/tools.rs`.

### Input

```json
{
  "files": [
    {"path": "main.eps", "code": "import lib.units; ..."},
    {
      "path": "lib/units.eps",
      "edits": [{"old_text": "oldCall();", "new_text": "newCall();"}]
    }
  ]
}
```

Contract:

- `files` is required and contains at least one item.
- Every `path` is a normalized, project-relative `.eps` path using `/` separators.
- Paths must reject an absolute/prefixed path, `.` / `..`, an empty segment, NUL, and duplicate
  case-insensitive keys.
- An extensionless `CUIEps` editor path is exposed to `eps_check` as `<exact-path>.eps`; editor
  reads and mutations continue using the exact extensionless path returned by `list_files`.
- Each item contains exactly one of `code` or `edits`. `code` is the complete candidate content.
  `edits` is a non-empty ordered array of exact `old_text`/`new_text` replacements; every
  `old_text` must be non-empty and match exactly once at its step.
- Edit candidates resolve against the reusable project mirror, then enter the same complete-file
  atomic overlay as code candidates. Neither form mutates the editor or reusable mirror.
- An absent path is valid only for a complete-code candidate; exact edits require an existing
  readable mirror file.
- A batch is required for mutually dependent new files: candidates are overlaid atomically before
  the import graph is built, so `a.eps` may import a new `b.eps` in the same call.

### Successful analyzed result

```json
{
  "status": "diagnosed",
  "project": "ExampleProject",
  "checkedFiles": ["main.eps", "lib/units.eps", "lib/common.eps"],
  "diagnostics": [
    {
      "path": "lib/units.eps",
      "line": 23,
      "character": 12,
      "endLine": 23,
      "endCharacter": 17,
      "severity": "error",
      "source": "epscript-lsp",
      "code": null,
      "message": "mismatched input ..."
    }
  ],
  "imports": [
    {
      "from": "main.eps",
      "module": "lib.units",
      "to": "lib/units.eps",
      "status": "resolved"
    }
  ],
  "truncated": false
}
```

Positions are 1-based in the Codex-facing result even though upstream LSP ranges are 0-based.
Upstream syntax diagnostics that omit severity map to `error`. Synthetic import diagnostics use
stable codes:

- `EUDLSP001`: imported module is missing.
- `EUDLSP002`: import cycle detected; advisory `warning`, because the actual build is authoritative.
- `EUDLSP003`: two project paths collide under Windows case-insensitive comparison.
- `EUDLSP004`: an imported project file exists but could not be snapshotted/read.

The result caps diagnostics and rendered message bytes with explicit `truncated` and omitted-count
fields so an upstream failure cannot flood the MCP result or Codex context. Exact caps are named
constants and covered by boundary tests.

### Successful degraded result

```json
{
  "status": "skipped",
  "reason": "node_not_found",
  "diagnostics": [],
  "imports": []
}
```

Stable skip reasons:

- `node_not_found`
- `adapter_missing`
- `adapter_start_failed`
- `adapter_crashed`
- `adapter_timeout`
- `adapter_protocol_error`
- `snapshot_unavailable`

These are successful tool results, not MCP errors. Invalid arguments and path traversal remain
normal corrective tool errors.

## Project snapshot protocol

A correct multi-file result requires a coherent editor-memory snapshot. Repeated `LIST` + serial
`GET` calls cost one or more editor ticks per file and are not acceptable for a project-wide
preflight. Extend the verified bridge without changing the existing `DUMP` path.

Add `EPSNAPSHOT <token>`:

1. Rust generates a lowercase ASCII UUID token and validates it before sending.
2. The bridge walks the project tree once on the idle UI-thread Tick.
3. Every settable text object is selected regardless of its stored filename suffix. Readable
   legacy objects whose exact path already ends in `.eps` remain selected. Each selected file is
   written as an ordinal file under `Data\agent\outbox\epsnapshot-<token>\`.
4. `manifest.tsv` maps ordinal -> file type -> exact UTF-8/base64 editor path -> byte length ->
   read status. Ordinals avoid path flattening and collisions. Rust preserves the exact path in
   the session `source/` baseline and maps an extensionless `CUIEps` to a virtual `<path>.eps`
   only inside the analyzer mirror.
5. File content and the manifest are UTF-8 without BOM. `manifest.tsv` is written last.
6. `handleCommand` returns only after the snapshot is complete; the normal request `.result`
   therefore remains the completion barrier.
7. Project metadata uses `Filename` when available, falls back to `OpenMapName` when only the
   project filename is empty, and uses a bridge-session UUID for a fully untitled project so it
   cannot alias a previous unsaved project's workspace.

   During bridge hot replacement, Rust also accepts an already-running legacy bridge's empty
   untitled identity by scoping it to the canonical editor data directory plus the bounded,
   non-empty `bridge_loaded.txt` session marker. A missing or invalid marker still fails closed.
8. Rust validates token ownership, manifest shape, declared lengths, duplicate/case-colliding
   paths, and path containment before copying into the app-owned mirror.
9. Rust removes the request snapshot directory after consumption. Startup cleanup removes stale
   `epsnapshot-*` directories without touching normal `.result` files.

The command reports unreadable selected files in the manifest instead of aborting the entire
snapshot. The analyzer emits `EUDLSP004` only when an affected import needs one of those files.
Heartbeat/status writes and the compiling early-return remain untouched and before inbox work.
No project objects are accessed while compiling.

## App-owned mirror

Add `DataDirs::lsp_workspaces_dir()` at:

```text
%localappdata%\eud-agent\lsp_workspaces\<sha256-project-id>\
```

Rules:

- Roaming storage is forbidden; mirrors are regenerable cache data.
- The project id comes from the full snapshot project identity and is hashed, never used as a raw
  path component.
- The first `eps_check` in each request refreshes the base mirror from `EPSNAPSHOT`.
- Later checks in the same request reuse the mirror, resolve exact-edit candidates against it, and
  atomically overlay the resulting complete candidate batch in a temporary analysis directory.
- Successful `file_create`, `file_write`, `file_edit`, `file_rename`, `file_move`, and
  `file_delete` operations update/invalidate the cached mirror so a later check in the same turn
  sees the applied state.
- A project identity change terminates the old adapter session and switches mirrors.
- Candidate overlays use temp files plus rename. They are never copied back to the editor.
- Mirror reads and adapter filesystem access are confined to the normalized mirror root.

## Import and affected-file analysis

The adapter uses the pinned upstream generated ANTLR lexer/parser to extract
`ImportStatementContext`; regex import parsing is prohibited. For each file it records
`import dotted.name [as alias];` with source range and resolves it as:

```text
<mirror-root>/<dotted>/<name>.eps
```

Resolution is case-insensitive for collision detection, matching Windows, but requires one unique
preserved-casing project path.

Build one project graph after all candidates are overlaid:

- Forward edges: candidate/importer -> imported dependency.
- Reverse edges: dependency -> every importer.
- Missing edge targets produce `EUDLSP001` at the import statement.
- Strongly connected components with more than one node, or a self-edge, produce `EUDLSP002`.
- Analyze candidate files, their transitive dependencies, and their transitive reverse dependents.
  Reverse dependents matter when a changed module removes or changes a symbol used elsewhere.
- Parse every affected file explicitly and aggregate its own diagnostics; never assume imported
  diagnostics are republished on the parent document.
- Preload dependencies in stable topological order where possible. Cyclic components use stable
  normalized-path order and remain advisory until `build_run`.

This closes known upstream gaps: missing imports are not reliably diagnosed, imported-file
errors are not automatically folded into the parent, and upstream's `ModuleListener` skips nested
imports to avoid recursion.

## Adapter process and protocol

Create a small Node adapter bundle built from the pinned epscript-lsp analyzer core. It is not the
published npm CLI and does not expose editor capabilities.

Runtime behavior:

- Lazy start on the first `eps_check`; application readiness never waits for it.
- Resolve `node.exe` with `which`, spawn directly with an argv array, piped stdin/stdout/stderr,
  app-owned cwd, and no shell/window.
- Use Content-Length framed UTF-8 JSON over stdio. stdout is protocol-only; bounded stderr goes to
  `%localappdata%\eud-agent\logs`.
- Serialize requests through one mutex; the application supports one editor and one active Codex
  turn, so concurrent analyzer requests are unnecessary.
- A cold-start deadline and a shorter warm-request deadline are separate named constants. Timeout
  kills the entire child tree and returns `skipped`.
- On crash/protocol failure, restart at most once for the current `eps_check`; a second failure
  returns `skipped` and suppresses respawn for the rest of that request.
- Reset/restart on project identity change. Terminate on application shutdown.
- Parse source only. Never execute epScript, euddraft config, project plugins, or arbitrary project
  JavaScript/Python.

Use dependency injection (`EpsAnalyzer` trait) in Rust so tool tests use a deterministic fake and
do not require Node.

## Agent workflow

The `[eps preflight]` system-prompt section appears before `[build]`:

```text
- Before file_create/file_write/file_edit for .eps, call eps_check with every candidate in one
  batch. Pass complete code for creates/full rewrites or the same ordered exact edits used by
  file_edit.
- Prefer file_edit for localized changes to existing files; use file_write only for intentional
  complete replacement.
- For mutually dependent files, include every candidate in one eps_check call.
- Fix error diagnostics and re-check before writing.
- Warnings are advisory; explain any warning left unresolved.
- If eps_check returns skipped, continue with the normal write and mandatory build_run flow.
- eps_check never replaces build_run. After applying eps/file changes, build and repair using the
  existing three-attempt build budget.
```

Do not add a mechanical admission gate requiring `eps_check`; diagnostics must remain advisory.
The generic Tool row may show the call/result like every MCP tool, but there is no dedicated panel
state, event, control, or diagnostics renderer.

The adjacent `[eps project architecture]` policy keeps imports
`configured MainFile -> feature modules -> stable leaf modules`. A candidate-introduced
`EUDLSP002` cycle is corrected rather than accepted; unrelated pre-existing cycles are reported
without incidental refactoring. Mutually dependent candidates remain one `eps_check` batch, and
the authoritative `build_run` remains mandatory after applying the complete change.

## Upstream pin and distribution

Do not depend on a mutable global npm installation.

- Pin the upstream repository and commit in a machine-readable provenance file.
- Keep an exact npm lock for the adapter build, explicitly including the upstream types package
  omitted by the published server dependency metadata.
- Build one self-contained CommonJS adapter bundle; Node core modules may remain external.
- Commit the generated bundle, its SHA-256, upstream MIT license, and provenance under
  `vendor/epscript-lsp-agent/` so normal Rust/Tauri builds require no network.
- Add the bundle, checksum, license, and provenance to `tauri.conf.json` resources.
- At runtime, verify the bundled bytes against the committed checksum before spawning. A mismatch
  returns `adapter_missing`/`adapter_start_failed`; it never falls back to downloading code.
- Add a regeneration script that fetches only the pinned archive, verifies its archive hash,
  performs `npm ci`, builds the adapter, and rewrites the committed bundle/checksum. Upstream
  upgrades are explicit review tasks with fixture parity, never automatic updates.
- The existing installer/update signature covers the resource. No separate first-run bootstrap
  asset or network dependency is added.

Upstream issue #51 mentions a future Rust rewrite without a delivered contract. Do not design
against it. A future replacement may implement the `EpsAnalyzer` trait after it exists and passes
the same fixtures.

## Failure semantics and invariants

- `eps_check` never mutates editor memory, journal state, map files, project settings, or project
  memory.
- Analyzer diagnostics never block a write tool in `tools::admit_tool_call`.
- Missing Node/analyzer is local to preflight; `search_docs`, write tools, and `build_run` remain
  usable.
- Analyzer crashes and stack overflows are process-isolated. Rust never trusts the child to stay
  alive or return valid JSON.
- A stale/mismatched project snapshot is rejected rather than analyzed under the wrong project.
- Snapshot and analyzer timeouts are always finite.
- Project paths remain editor-relative and UTF-8-safe; no absolute path is baked into Lua source.
- Candidate content is data, never command-line text or executable input.
- Existing bridge `DUMP`, `LIST`, `GET`, `SET`, `NEWFILE`, heartbeat, status, and build behavior is
  unchanged.
- Final correctness comes from `build_run`; a zero-diagnostic preflight is never reported as a
  successful build.

## Implementation plan

### 1. Pin and build the analyzer adapter

- Add `tools/epscript-lsp-agent/` with the adapter entry, exact lockfile, upstream provenance, and
  focused Node tests.
- Build against the pinned upstream ANTLR parser/analyzer, adding only the agent request surface,
  import graph extraction, affected-closure calculation, diagnostic normalization, and output
  caps.
- Add the deterministic regeneration script and committed resource under
  `vendor/epscript-lsp-agent/`.
- Verify the bundle starts with Node on Windows and contains no runtime package lookup for
  `@epscript-lsp/types`.

### 2. Add one-shot project snapshot support

- Extend `bridge/ZZZ_10_agent_bridge.lua` with `EPSNAPSHOT` while preserving every existing command
  branch and the heartbeat/status ordering.
- Add manifest decoding, containment checks, stale cleanup, and `snapshot_eps` to
  `src-tauri/src/bridge_io.rs`.
- Test nested folders, Korean paths/content, empty files, duplicate casing, unreadable files,
  malformed manifests, stale directories, compiling wait, and request-token isolation.

### 3. Add the Rust preflight service

- Add `src-tauri/src/eps_preflight.rs` with `EpsAnalyzer`, production Node adapter client, framed
  protocol codec, process lifecycle, deadlines, mirror/overlay ownership, and normalized result
  types.
- Add `DataDirs::lsp_workspaces_dir()` and ensure/cleanup behavior in `config.rs`.
- Resolve the adapter Tauri resource in `lib.rs`, verify its checksum, and inject an
  `Arc<dyn EpsAnalyzer>` into `ToolRuntime`; construction must remain non-fatal.
- Keep process and filesystem work inside the existing blocking MCP execution boundary.

### 4. Expose and teach `eps_check`

- Add the non-mutating complete-code/exact-edit schema to `tools.rs` and dispatch it from
  `tool_exec.rs`.
- Resolve edits against the mirror, overlay the resulting complete candidate batch, call the
  analyzer, and return normalized JSON without journaling.
- Update successful file mutations to keep/invalidate the request-local mirror.
- Add the `[eps preflight]` prompt section in `engine.rs` before the existing mandatory `[build]`
  guide.
- Do not alter mutation counts, evidence gating, plan approval, changeset composition, or panel
  state.

### 5. Verify, document, and package

- Add focused Rust and Node tests below, then run the project verification commands once.
- Update `architecture.md`, `rules.md`, `tech-stack.md`, and `verify.md` to describe the optional
  agent-only analyzer, pinned resource, snapshot command, and verification command.
- Keep README and panel documentation unchanged: there is no new user workflow or UI.
- Build the NSIS bundle and confirm the adapter resource and license are present without bundling a
  Node runtime.

## File impact

Expected additions:

- `hivemind/docs/features/18_epscript-lsp-agent-preflight.md` — this single source plan.
- `src-tauri/src/eps_preflight.rs` — Rust service, protocol, mirror, and result normalization.
- `tools/epscript-lsp-agent/**` — pinned adapter build source, lock, and tests.
- `vendor/epscript-lsp-agent/**` — generated bundle, checksum, license, provenance.
- `scripts/build_epscript_lsp_agent.ps1` — deterministic regeneration.

Expected modifications:

- `bridge/ZZZ_10_agent_bridge.lua` — additive `EPSNAPSHOT` command only.
- `src-tauri/src/bridge_io.rs` — snapshot client/manifest validation/cleanup.
- `src-tauri/src/config.rs` — local analyzer workspace directory.
- `src-tauri/src/lib.rs` — module/resource resolution and optional analyzer injection.
- `src-tauri/src/tool_exec.rs` — read-only dispatch and mirror invalidation after file mutations.
- `src-tauri/src/tools.rs` — `eps_check` registry/schema.
- `src-tauri/src/engine.rs` — `[eps preflight]` agent instruction.
- `src-tauri/tauri.conf.json` — bundled adapter resources.
- `hivemind/docs/{architecture,rules,tech-stack,verify}.md` — internal contracts and commands.

Explicitly unchanged:

- `panel/**`
- EUD Editor 3 binaries/source
- journal and changeset wire shapes
- map-write safety rails
- RAG index/model bootstrap
- `ProgressStage::Lsp` event behavior

## Verification contract

### Node adapter tests

Run with the adapter package's exact lockfile:

- valid single file -> zero diagnostics.
- malformed single file -> normalized syntax range/message.
- direct import and alias -> resolved edge and cross-file symbols.
- nested import -> complete transitive graph and imported-file diagnostics.
- mutually dependent new candidates in one batch -> both overlaid before resolution.
- missing import -> `EUDLSP001` at the import statement.
- cycle/self-import -> deterministic `EUDLSP002`, no recursion/crash.
- changed dependency -> reverse importer included in `checkedFiles`.
- import-looking text in strings/comments -> no graph edge.
- Korean/Unicode identifiers and paths -> preserved and resolved.
- diagnostic/output cap -> stable truncation counts.
- known upstream recursive-symbol fixture -> bounded failure or diagnostic, never an unhandled
  parent-process crash.

### Rust tests

- Content-Length codec handles fragmented/multiple frames and rejects invalid/oversized lengths.
- path normalization rejects traversal, prefixes, empty segments, and case-insensitive duplicates.
- manifest decoder validates base64 paths, lengths, ordinals, token, and root containment.
- complete-code and exact-edit candidate batches overlay atomically and never alter the base
  mirror/editor; edit matching rejects empty, missing, and ambiguous `old_text`.
- fake analyzer result is returned as a non-mutating tool success with no journal entry.
- every adapter/snapshot failure maps to a stable `skipped` result.
- timeout/crash kills and reaps the child; one retry maximum per check.
- project switch replaces process/mirror state; same-request calls reuse the snapshot.
- successful create/write/edit/rename/move/delete updates or invalidates mirror state.
- `eps_check` remains read-only in MCP descriptors and does not consume mutation/action budget.
- tool catalog and system prompt include preflight before build guidance.
- existing evidence, plan, build-attempt, and changeset tests remain unchanged and green.

### Bridge tests

Use the existing fake bridge harness plus Lua contract checks:

- one `EPSNAPSHOT` produces nested-path-safe ordinal files and a last-written manifest.
- UTF-8 without BOM for manifest/content, including Korean source.
- read failures are per-file entries, not whole-snapshot failure.
- compiling state processes no project objects and follows the existing extended timeout.
- stale/request-foreign snapshot paths are ignored and safely cleaned.
- heartbeat and status writes still occur before the compiling early-return.

### Commands

- `npm --prefix tools/epscript-lsp-agent ci`
- `npm --prefix tools/epscript-lsp-agent test`
- adapter regeneration followed by a clean checksum comparison against committed resources
- `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cd panel && npx tsc -b --noEmit`
- `cd panel && npx vitest run`
- `cd panel && npm run build`
- `cargo build --manifest-path src-tauri/Cargo.toml`
- packaging acceptance: `cargo tauri build` contains the adapter bundle/checksum/license

### Behavioral smoke

With EUD Editor 3 and the app:

1. Open a project containing `main.eps -> lib/units.eps -> lib/common.eps`.
2. Ask the agent for a coordinated change to `main.eps` and `lib/units.eps`.
3. Observe one batch `eps_check` before writes; inject a syntax error and confirm Codex receives the
   file/range diagnostic, fixes it, and checks again without an intermediate editor write.
4. Inject a missing nested import and confirm `EUDLSP001` names the importer and resolved target.
5. Apply the corrected files and confirm the existing `build_run` succeeds in the same turn.
6. Repeat with Node unavailable and confirm `eps_check` returns `skipped`, writes/build still run,
   and no startup/panel failure occurs.

## Acceptance criteria

- Codex can preflight one or multiple complete-code or exact-edit `.eps` candidates before any
  editor mutation.
- Mutually dependent candidates, direct imports, nested imports, and reverse dependents are checked
  against one coherent project snapshot.
- Missing imports and cycles receive stable synthetic diagnostics; imported-file syntax errors are
  attributed to their own project paths.
- Diagnostics return to Codex in the same turn and are not routed through a dedicated user UI.
- Preflight creates no journal entry and cannot block a write through admission policy.
- Missing/broken/timed-out analyzer returns `skipped`; application startup and all existing tools
  continue normally.
- Every applied eps/file change still invokes the authoritative `build_run` repair loop.
- Runtime performs no npm install, network download, project-code execution, or shell command.
- The analyzer source commit, dependency lock, bundle checksum, and MIT license are reviewable and
  shipped in the installer.
- Existing panel, bridge, Rust, and packaging verification remains green.
