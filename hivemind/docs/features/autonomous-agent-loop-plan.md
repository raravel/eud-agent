# Autonomous agent loop implementation plan

## Status

Implemented on 2026-09-14. The clean cutover is active across the common engine/runtime, all five
provider adapters, durable session state, and the Panel. Fixed request-wide build/search/dependency
limits no longer terminate useful work; iteration boundaries and progress identities now govern
continuation. The verification section below remains the acceptance contract.

## Problem

The pre-cutover foreground runtime was optimized for a short interactive request:

- `build_run` was registered as a mutating tool even though it changes generated build outputs, not
  canonical authoring state. An EPS read run therefore stopped with `WriteWorkspaceTransition`
  before a standalone build.
- Every admitted `build_run`, successful or not, consumed one of three request-wide build attempts.
- Most tools shared a request-wide 300-action ceiling.
- `search_docs` had a separate 120-call ceiling.
- `python_dependencies_prepare` had a separate four-call ceiling.
- direct provider execution failed after 64 foreground tool rounds.

These limits stopped valid edit/build/debug cycles. Raising the constants would only have delayed
the failure while permitting a genuine runaway loop to consume more resources. Multi-hour work
therefore required bounded, durable iterations and progress-aware loop detection rather than one
unbounded provider turn.

## Goals

1. Run `build_run` directly from an EPS read foreground without acquiring a write workspace.
2. Continue serializing builds against shared project operations and retain the native build marker.
3. Permit any number of useful tool and build cycles within a configured autonomous run.
4. Stop repeated calls, unchanged build failures, and short revision cycles that make no progress.
5. Convert action/round ceilings into durable iteration boundaries instead of terminal failures.
6. Preserve exactly-once tool completion, semantic journals, review, ASK, cancellation, provider
   continuation, and stale-project protections.
7. Persist enough autonomous-run state to pause safely and resume explicitly after an app restart.
8. Keep ordinary interactive chat bounded and distinct from opt-in autonomous execution.

## Non-goals

- Do not make generated EDS, Python, binary files, or output maps canonical project state.
- Do not auto-approve changesets or Map Apply decisions.
- Do not weaken path, schema, payload-size, subprocess-timeout, cancellation, or stale-authority
  checks.
- Do not infer provider cost or token usage when the provider does not report it.
- Do not replay an unconfirmed native provider continuation or an already completed tool call.
- Do not resume mutations automatically when the application starts after a crash.

## Invariants

- `project.eap`, `src/**/*.eps`, `src/**/*.py`, `dat/*.json`, and the source map remain the only
  canonical authoring authority.
- Canonical mutations require runtime-managed write admission.
- Shared project operations remain serialized only for their actual transaction duration.
- A successful tool result and its journal/receipt remain durable even if a later provider step
  fails or is cancelled.
- An iteration boundary is not a failure and does not finalize the request changeset.
- ASK and review always pause autonomous execution for the user.
- Restart recovery is fail-closed and requires explicit resume.

## Runtime hierarchy

Introduce three explicit scopes:

```text
autonomous run
└── user request / journal authority
    ├── foreground iteration 1
    ├── foreground iteration 2
    └── foreground iteration N
```

- **Autonomous run** owns the original goal, run policy, elapsed time, cumulative observed usage,
  progress fingerprints, and durable status.
- **Request** owns the user authorization, request ID, evidence state, semantic journal, and final
  changeset review.
- **Iteration** owns soft action/round counters and one confirmed provider execution boundary.

A write transition or iteration continuation keeps the same request and journal authority. Each new
foreground execution receives a new run identity.

## Phase 1: Separate build effects from workspace mutation

### Tool metadata

Replace the overloaded `ToolSpec.mutating` flag in `src-tauri/src/tools.rs` with two explicit
properties:

```rust
pub struct ToolSpec {
    pub requires_write_workspace: bool,
    pub requires_project_transaction: bool,
    // existing fields
}
```

Initial classification:

| Tool class | Write workspace | Project transaction |
| --- | ---: | ---: |
| Pure reads and diagnostics | no | no |
| Request-local Map draft operations | no | no |
| `build_run` | no | yes |
| Canonical source/DAT/settings/plugin mutations | yes | yes |
| Connected source-map mutations | yes | yes |

Use direct predicates with matching names. Remove `is_mutating_tool()` after every caller migrates;
do not retain a compatibility alias.

### Gate and execution

Update `src-tauri/src/provider_tool_loop/run_gate.rs` so only
`requires_write_workspace == true` triggers automatic read-to-write transition.

Update `src-tauri/src/tool_exec.rs` so:

- write-registration admission uses `requires_write_workspace`;
- `project_transaction()` wrapping uses `requires_project_transaction`;
- `build_run` can execute from a read foreground but still serializes against shared project
  transactions.

Keep `NativeProjectManager::build_with_cancellation()` locking and `build/.building`. A build must
continue to block conflicting Map/sound writes for its full native process lifetime.

### Acceptance

- A standalone EPS `build_run` executes in its first read foreground with no
  `WriteWorkspaceTransition` result.
- `file_edit`, `dat_patch`, and other canonical mutations still transition exactly once and are not
  executed in the read foreground.
- Concurrent build and canonical mutation operations serialize at the project transaction.
- Generated build artifacts never enter the semantic workspace changeset.

## Phase 2: Replace the three-build budget with progress detection

### Remove request-wide attempt counting

Remove `RequestState.build_fix_attempts`, its admission-time increment, the `>= 3` refusal, and all
prompt/documentation language requiring the agent to stop after three builds.

A build attempt must be evaluated after execution because admission does not know its diagnostics or
whether it made progress.

### Build progress state

Record the following after each build:

```rust
pub struct BuildProgress {
    pub input_revision: String,
    pub diagnostics_fingerprint: String,
    pub error_count: usize,
    pub success: bool,
    pub consecutive_no_progress: u32,
}
```

Use `NativeProject::revision()` as the canonical build-input revision. Compute the diagnostic
fingerprint from a stable ordering of normalized error kind, logical file, line, and message. Exclude
stdout framing, temporary absolute paths, timestamps, and other nondeterministic text.

### Progress rules

Treat the following as progress:

- the canonical input revision changed;
- the normalized diagnostic fingerprint changed;
- the error count decreased;
- the build succeeded.

Treat the following as no progress:

- the same input revision produces the same diagnostics repeatedly;
- the agent calls `build_run` again without changing input after an unchanged failure;
- recent `(revision, diagnostics)` pairs repeat as a short cycle such as A → B → A → B.

Retain a small bounded ring of recent fingerprints to detect cycles without unbounded memory.
Successful builds reset consecutive build no-progress state. Useful builds have no lifetime count
limit.

On repeated no progress, return a recoverable tool result instructing the agent to inspect or change
the relevant input. Do not force the entire request to end unless the run-level no-progress policy is
also exhausted.

### Acceptance

- More than three builds across changing revisions are admitted.
- More than three successful builds are admitted.
- An unchanged build failure is not executed indefinitely.
- Improving diagnostics remain admissible.
- A short revision/diagnostic oscillation is detected.

## Phase 3: Convert action and round ceilings into iteration boundaries

### Runtime outcome

Add a non-failure outcome in `src-tauri/src/provider_runtime.rs`:

```rust
pub enum IterationBoundaryReason {
    ToolActions,
    ToolRounds,
    ContextPressure,
    ProviderContinuation,
}

pub enum RunOutcome {
    // existing outcomes
    IterationBoundary {
        reason: IterationBoundaryReason,
        conversation: ProviderConversationState,
    },
}
```

The exact representation may follow existing checkpoint types, but the boundary must be typed and
must not be encoded as a tool error string.

### Soft iteration thresholds

Keep 300 admitted actions and 64 direct tool rounds initially as conservative iteration thresholds:

- reaching either threshold checkpoints the confirmed conversation state;
- the runtime returns `IterationBoundary` instead of `ToolRoundLimit` or action-budget failure;
- `AgentEngine` starts the next iteration automatically in both modes: autonomous mode persists
  its run checkpoint first, while an ordinary interactive turn persists the session conversation
  and continues the same request in place with a minimal `[continuation]` prompt. Neither mode
  ends a turn or emits a "paused" answer because a soft threshold was reached.


### Direct providers

In `src-tauri/src/provider_runtime/runtime/foreground.rs`:

- checkpoint the ordered transcript and continuation at the last confirmed response/tool-result
  boundary;
- return `IterationBoundary` when the direct round threshold is reached;
- resume without replaying the completed batch.

### Native providers

Native CLI providers must finish at a confirmed native conversation boundary. Do not kill a native
process merely to manufacture an iteration checkpoint. When the soft action threshold is reached,
stop new admission, deliver a typed checkpoint request through the native tool gate, and accept only
the official completed native continuation before resuming.

If the provider exits without a confirmed boundary, preserve the receipt and fail closed under the
existing recovery rules.

### Acceptance

- A useful workload exceeding 300 actions continues in another iteration.
- A direct workload exceeding 64 tool rounds continues in another iteration.
- Completed tool IDs occur exactly once across the boundary.
- The same request ID and semantic journal survive the boundary.
- Write transition and iteration boundary remain distinct outcomes.
- Native resume adopts only a confirmed provider continuation.

## Phase 4: Make search and dependency preparation progress-aware

### Documentation search

Replace the fixed 120-search lifetime refusal with novelty-based protection using the existing
request evidence fields:

- normalized query and filters;
- returned stable document IDs;
- unique versus repeated hits;
- serialized result bytes.

Allow continued searches while they discover new evidence or materially change the query. Refuse
repeated equivalent searches with no new documents. Preserve the evidence requirement for canonical
mutations.

### Python dependency preparation

Replace the fixed four-call lifetime refusal with desired-state identity:

- fingerprint the complete normalized dependency set;
- refuse repeated preparation of the same set while a valid candidate already exists;
- allow a new preparation when the desired dependency set or a material failure condition changes;
- retain token expiry, single use, checksum verification, cache integrity, timeouts, and bounded
  downloads.

### Acceptance

- New evidence can be gathered beyond 120 useful searches.
- Equivalent searches returning the same IDs are stopped.
- More than four distinct dependency sets can be prepared during a long run.
- Repeated preparation of an unchanged dependency set does not redownload or recompute needlessly.

## Phase 5: Add opt-in autonomous orchestration

### Execution mode and policy

Add an explicit mode instead of changing every chat request:

```rust
pub enum ExecutionMode {
    Interactive,
    Autonomous(AutonomousRunPolicy),
}

pub struct AutonomousRunPolicy {
    pub max_wall_time: Option<Duration>,
    pub max_total_tokens: Option<u64>,
}
```

Both budgets are explicit and default to unset: an unbudgeted run continues until completion,
review, ASK, user pause/stop, cancellation, or failure. Apply token limits only to usage actually
reported by the selected provider. Do not invent missing cost or token values. Consecutive
no-progress fingerprints are recorded as visible progress state only; they never stop a run, because
a legitimate long investigation or repeated read-only iteration must not be mistaken for a stall.

### Engine controller

Keep orchestration in the common `AgentEngine`/`SessionEngineManager` layer, not in provider
adapters. The controller handles:

```text
Completed          -> validate completion and settle the request
IterationBoundary  -> persist and start the next iteration
WriteTransition    -> resume in write access
WaitingInput       -> pause for ASK
Review             -> pause for user review
SafetyStopped      -> persist the reason and pause
Cancelled          -> stop promptly
Failed             -> preserve recoverable state and stop
```

An iteration continuation prompt contains only stable control context:

- original goal;
- current task-state projection;
- completed and remaining acceptance criteria;
- unresolved blockers;
- latest build status;
- boundary reason;
- an instruction not to replay completed calls.

Fresh project/file state must be read through tools rather than copied from a stale iteration.

### Completion contract

Autonomous completion requires an explicit typed completion outcome. If the request changed runtime
source, plugins, DAT, map state, or Python dependencies, the latest canonical revision must have a
successful required build before completion is accepted.

Ordinary assistant prose is not by itself an autonomous completion signal.

### ASK and review

- `ask` pauses the run as `waiting_input`.
- A changeset pauses the run as `review`.
- User input or a review decision resumes the same autonomous run when valid.
- The runtime never auto-accepts semantic changes or exposes original Map Apply to the model.

## Phase 6: Persist autonomous state and recover safely

Persist a minimal run record with the owning session:

```rust
pub struct AutonomousRunState {
    pub id: String,
    pub status: AutonomousRunStatus,
    pub started_at: u64,
    pub iteration: u64,
    pub original_goal: String,
    pub request_id: String,
    pub policy: AutonomousRunPolicy,
    pub progress: ProgressState,
    pub last_checkpoint: Option<ProviderConversationState>,
}
```

`ProgressState` contains bounded recent tool/build fingerprints, observed usage, elapsed active time,
and consecutive no-progress counts. Do not persist unbounded raw outputs.

On application restart, map an interrupted active run to `paused_after_restart`; never restart
mutation automatically. Explicit resume must verify:

1. the selected project identity;
2. current canonical revision and stale authority;
3. pending semantic review;
4. the last confirmed provider checkpoint or native receipt;
5. that completed tool calls will not replay.

This preserves the existing no-active-request-replay safety boundary while allowing deliberate
continuation.

## Phase 7: Add panel controls and status

Update the panel protocol, IPC client, session state, and active-turn UI with:

- ordinary **실행**;
- **일시 중지**;
- **계속**;
- **중단**.

The composer exposes only ordinary **실행**. The earlier opt-in **장시간 작업 실행** action and
its 1/4/8-hour selector were removed: ordinary runs already continue across every soft boundary,
so a separate long-running entry point and a wall-time choice have no purpose until an explicit
budget (cost/token) policy exists. The backend `executionMode: "autonomous"` path and lifecycle
controls remain for that later budgeted loop.

Expose observed status:

- elapsed active time;
- current iteration;
- current read/write activity;
- latest build result;
- no-progress or safety-stop reason;
- ASK/review/restart pause state;
- configured run limits.

Keep `running_read` and `running_write` as current activity. Store autonomous lifecycle separately so
one enum does not conflate resource activity with run state.

All user-facing copy remains Korean, keyboard accessible, and visible without relying on color.

## Code map

Expected primary files:

- `src-tauri/src/tools.rs`: tool effect metadata, admission counters, search/dependency progress.
- `src-tauri/src/tool_exec.rs`: write registration, project transaction routing, post-build progress.
- `src-tauri/src/provider_tool_loop/run_gate.rs`: transition predicate and iteration-boundary gate.
- `src-tauri/src/provider_tool_loop/gate_state.rs`: typed boundary/progress state.
- `src-tauri/src/provider_runtime.rs`: run policy and typed outcomes.
- `src-tauri/src/provider_runtime/runtime/foreground.rs`: soft round boundary and checkpoint return.
- `src-tauri/src/engine.rs`: autonomous controller, completion validation, continuation prompts.
- `src-tauri/src/session.rs`: durable autonomous run state and migration defaults.
- `src-tauri/src/ipc.rs`: start/pause/resume/cancel/status commands.
- `panel/src/lib/protocol.ts`: typed autonomous commands/events.
- `panel/src/lib/ipc.ts`: command bindings and state decoding.
- `panel/src/App.tsx`: session-owned autonomous state routing.
- `panel/src/components/AgentTurnStatus.tsx`: controls and status surface.
- `hivemind/docs/features/05_agent-core.md`: final runtime contract.
- `hivemind/docs/features/sessions.md`: persistence, concurrency, and recovery contract.
- `hivemind/docs/architecture.md`: implemented runtime ownership after completion.
- `hivemind/docs/verify.md`: verified source/binary and actual-run evidence.

Keep the implementation local where existing ownership already fits. Create a new Rust module only if
the autonomous controller or progress state would otherwise make `engine.rs` or `tools.rs` less
cohesive.

## Verification plan

### Focused behavioral contracts

1. Read-mode standalone build executes without a write transition.
2. Canonical mutation still transitions and executes only after write admission.
3. Build and source mutation serialize without holding a session-wide lease.
4. Ten useful builds across changing revisions succeed in one autonomous run.
5. Repeated identical build failures stop through no-progress detection.
6. A → B → A → B revision/diagnostic oscillation stops.
7. More than 300 actions continue across an iteration boundary.
8. More than 64 direct tool rounds continue across an iteration boundary.
9. Direct and native provider continuations never duplicate a completed mutation.
10. ASK, review, cancellation, stale project state, and restart pause at the correct boundary.
11. A runtime-affecting change cannot complete without a successful build of its latest revision.
12. Repeated searches and dependency preparations are blocked by input identity, while useful new
    work remains allowed.

### Project gates

Run the repository gates after focused contracts pass:

```powershell
cargo check -p eud-agent --lib
cargo test -p eud-agent
panel\node_modules\.bin\tsc.cmd -b panel\tsconfig.json
npm --prefix panel test -- --run
npm --prefix panel run build
```

Run scoped Rust formatting and strict Clippy according to the current repository verification
contract.

### Actual runtime acceptance

Using a disposable native project and configured real euddraft:

1. start an autonomous source task;
2. perform at least four edit/build cycles;
3. require a fresh output SCX for the latest revision;
4. cross both an action and tool-round iteration boundary;
5. cancel during a later iteration and observe no late tool admission;
6. restart the app and observe `paused_after_restart`, not automatic replay;
7. explicitly resume and finish;
8. inspect the final changeset, reject it, and verify exact rollback;
9. repeat and accept it, verifying durable canonical state.

Qualify direct-step and native-session providers separately. Fixture-backed provider tests prove
protocol/runtime behavior; one actual configured provider plus real euddraft proves the integrated
application path without claiming untested provider compatibility.

## Implementation order

1. Split build effect metadata and remove unnecessary write transition.
2. Replace the three-build counter with post-execution progress tracking.
3. Convert action and round hard failures into typed iteration boundaries.
4. Replace search and dependency-preparation fixed counts with input/progress guards.
5. Add the common autonomous controller and explicit completion contract.
6. Persist pause/resume state and enforce restart safety.
7. Add panel controls and status.
8. Run focused, full-suite, provider-path, UI, and real-euddraft verification.
9. Remove obsolete counters, error strings, tests, prompt text, and documentation in one clean
   cutover.

## Implemented result

- `build_run` is a read-workspace project transaction; canonical mutations alone request write
  admission.
- Stable revision/diagnostic fingerprints reject unchanged and A → B → A → B build failures while
  successful or improving builds remain unbounded.
- Action and direct-round thresholds return typed, persisted iteration boundaries. Direct
  transcripts resume after completed results; native sessions accept only official confirmed
  continuations.
- Search novelty uses normalized query/filter identity and stable result IDs. Python dependency
  preparation uses the complete normalized dependency-set digest and reuses a still-valid matching
  candidate without probing, resolving, or downloading again.
- Opt-in autonomous runs retain one request ID and journal across iterations, enforce current
  revision build completion, pause for ASK/review/user/restart, and resume only after project,
  revision, review, and provider-checkpoint validation.
- The Panel keeps autonomous lifecycle separate from read/write activity and exposes Korean
  run-limit, progress, blocker, pause, resume, and stop controls.

## Verification result

- All 22 focused behavior criteria pass, including the 300-action and direct-round continuation
  boundaries, no duplicate durable tool execution, native fail-closed continuation, ASK/review,
  current-revision build completion, search/dependency novelty, restart pause, and exact resume.
- The current unfiltered Rust suite is `754 passed; 0 failed; 30 ignored`; formatter,
  all-target/all-feature strict Clippy, and library check pass.
- Panel TypeScript, 58-file/546-test Vitest, and production build pass. Browser verification covers
  the Korean autonomous controls and lifecycle at 960×640, 1280×800, and 1920×1080 with only the
  Tauri transport mocked.
- The real-euddraft acceptance passes four edit/build cycles against a copied real SCX in one
  disposable native project.
- The combined live-provider application scenario is not claimed: the required explicit Codex
  executable, isolated auth JSON, and model variables are absent. The selector fails before provider
  startup rather than substituting ambient credentials. Deterministic production-path contracts
  cover action/round boundaries, cancel, restart/resume, and review reject/accept on this source.
