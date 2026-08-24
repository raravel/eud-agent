# Project Memory (post-acceptance harness + panel editing)

Project Memory is the small, fully prompt-injected summary of durable map-project facts that
prevent cross-turn collisions: resource allocations, file roles, stable conventions, and user
corrections. It contains no request history or code-derivable detail.

Long-lived behavior specifications, approved plans, architectural decisions, and detailed worklogs
belong in the canonical project workspace. Memory remains the hot context injected into every
foreground Codex turn.

## Storage

Rust owns `%appdata%\eud-agent\memory\<sanitized-project-name>\`:

| File | Content |
|---|---|
| `resources.md` | Switch/death-counter/location/EUD-address allocations |
| `structure.md` | Current file roles and direct project dependencies |
| `conventions.md` | Stable naming and trigger conventions |
| `lessons.md` | Durable user corrections and their application rule |
| `meta.json` | Store metadata and source-list hash |

Project names come from bridge STATUS. Windows-invalid filename characters are replaced with `_`;
trailing dots/spaces are stripped. An empty project disables memory. Writes are atomic UTF-8
without BOM, each Markdown file has an 8 KiB cap, and absent files read as empty.

## Foreground boundary

Foreground Codex receives the full `[project memory]` section on a fresh instruction epoch. A
normal active-thread follow-up receives a revision-labelled replacement only when the rendered
memory hash changes. The section explicitly says it is accepted context and is read-only during
implementation.

`memory_write` is not an MCP tool. The implementation agent cannot spend post-build foreground
cycles rewriting memory, cannot duplicate spec facts into memory, and cannot place unaccepted
behavior into durable context.

Panel `memory_get` and `memory_save` remain available for direct user inspection and editing.
`memory_save` is a short project transaction and never depends on an agent turn.

## Post-acceptance synchronization

A complete code changeset acceptance creates a durable harness job containing the exact accepted
journal entries, request/plan/answer context, build evidence, runtime-verification state, and a
bounded copy of accepted active-task promotion candidates pinned to the source task revision/event.
Candidates remain optional evidence and cannot directly write a document or memory.

Runtime-sensitive jobs may be skipped before generation; skip leaves current memory unchanged and
creates no worklog, document delta, or promoted fact. Otherwise, one tool-free,
output-schema-constrained Codex turn returns `HarnessDelta`:

```text
summary
specs/decisions exact document patches
optional full replacements for resources|structure|conventions|lessons
promotedFactIds for candidates actually incorporated by this delta
```

The delta validator enforces:

- at most four memory replacements and one replacement per memory file;
- known memory file names and existing per-file size caps;
- no memory update for transient or code-derivable facts;
- no duplicate fact between canonical specs and memory;
- structure replacement only when file topology, MainFile, dependency, or material responsibility
  changed.
- every `promotedFactIds` value must name a pinned accepted candidate, and a promoted id requires
  at least one document or memory update;

Memory replacements appear by file name in the harness review card. They are applied only when the
separate atomic harness changeset is accepted. If a memory write fails, earlier memory writes roll
back. If canonical document promotion fails, all memory replacements roll back. Only after that
transaction succeeds does the session store append a hashed `PromotionAccepted` audit and mark the
named facts promoted when the source event is still on the current branch. A rewound source leaves
the durable audit detached and cannot revive the abandoned projection. Reject and skip record no
promoted authority. Rejecting the harness changeset leaves accepted code and existing memory
unchanged.

## Prompt rendering

The rendered section contains non-empty memory files in this order:

1. `resources.md`
2. `structure.md`
3. `conventions.md`
4. `lessons.md`

The full section is capped at 40,000 characters. Overflow truncates lessons tail-first and appends
`memory section truncated`. Missing/unreadable stores degrade to
`[project memory]\n(no project memory)` and never block a turn.

Live `project_status.mainFile`, `list_files`, DAT reads, map reads, and source snapshots override
stale memory. Memory is descriptive context, not executable authority.

## Conversation boundary

Memory stores no conversation episodes. Durable conversations stay in session records. The schema
v3 cutover preserves session panel logs but clears legacy thread/pending ownership; it does not
delete accepted project memory.

## Panel contract

The project sidebar Memory tab remains the user-owned editor:

- selecting one of the four files loads its full content;
- dirty state is local to the selected file;
- Save calls `memory_save` and reports inline success/error;
- background harness state is shown in `HarnessStatusCard`, not in the memory editor;
- a running or reviewable harness job never disables manual memory viewing or normal chat.

## Edge cases

- No project: `memory_get`/`memory_save` return an explicit disabled-store error; prompt injection
  renders no project memory.
- Project switch: each foreground prompt resolves the current project and refreshes memory.
- Concurrent panel edit and harness acceptance: both use the project transaction. Harness applies
  its captured replacement atomically; canonical document promotion failure restores prior memory.
- Harness generation failure: accepted code and current memory remain unchanged; the job becomes
  retryable without an automatic second model call.
- Harness skip: accepted code remains, current memory remains, no model turn starts, and the job
  records terminal `skipped`.

## Verification

- `memory.rs` tests cover sanitization, atomic UTF-8 writes, caps, rendering order, truncation, and
  no-project degradation.
- `harness.rs` tests cover memory file validation, structured delta persistence, deterministic
  worklog staging, and interrupted-job recovery.
- Promotion tests cover pinned candidate ids, canonical document/memory hashes, reject/skip
  boundaries, and detached-branch audits that do not alter the current projection.
- Engine tests prove foreground approved-plan completion performs no memory/document repair turns.
- Panel tests prove runtime confirmation, skip, retry, atomic harness review, and direct memory
  editing remain independently usable.

## Implementation

- `src-tauri/src/memory.rs` — project memory store and prompt renderer.
- `src-tauri/src/harness.rs` — durable jobs, structured delta, memory rollback, worklog staging.
- `src-tauri/src/engine.rs` — post-accept scheduling, one-turn generator, retry/decision IPC.
- `src-tauri/src/task_state.rs` — bounded promotion input, audit records, and branch-aware reducer.
- `panel/src/components/HarnessStatusCard.tsx` — background status and review controls.
- `panel/src/components/ProjectSidebar.tsx` — direct user memory editor.
