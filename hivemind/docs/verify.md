# Verification

## Skill registry and multi-skill picker (2026-10-03)

`cargo test -p eud-agent --lib` passes the registry parse/summary, legacy
`SkillSelectionChanged` deserialization, per-skill selection, multi-skill
`[active skill]` assembly, map-capacity manual-only activation and per-skill IPC
tri-state tests; existing luck-defense routing tests are unchanged. Panel
InstructionBox, App and IPC tests cover the `+` picker, chips, legacy
localStorage migration and per-skill queue/retry/edit directives. These checks
validate selection and prompt assembly, not a live model's use of the guidance.

## Central summons and closed-loop instructions

The recorded EPS/Map handoff created an external summon platform and explicitly opened the
enemy route into an open path, despite the requested loop. The skill now specifies central direct summons, a closed
perimeter with collision-aware final-to-first verification, and independent ground separation.
The Map Agent baseline preserves that topology and does not treat generic candidate validity
as gameplay evidence. Older briefs and coordinates must be reviewed before reuse.

`cargo test --lib prompt -- --nocapture` passes 24 tests with one environment-backed test ignored;
`cargo test --lib luck_defense -- --nocapture` passes both selection/delivery tests. UTF-8 skill
validation, scoped rustfmt and diff checks pass. These checks validate instruction assembly and
delivery, not a live model's map design or StarCraft gameplay. The installed application and
historical session records were not replaced. A corrected layout spec and index were saved in
the selected defense project's `.eud-agent/workspace/specs`; actual terrain repair remains open.

## Project work checkpoints and worker scheduling (2026-10-01)

The real filesystem cleanup regression and request-batch scheduling regression both failed
before the fixes. Valid compiler results now survive cleanup failure. Successful request
batches wait for active peers, while failure wakes for recovery; cancellation and independent
batches retain their previous boundaries. A production AgentEngine fixture confirms an
automatic worker continuation makes no semantic compiler call.

Native project/session fixtures observe the saved JSON and plan/evidence files, one-time
hash delivery, delivery after reset, and refresh of workers interrupted on restart. Store
fixtures cover moving the project, discovery by a new session, retention of recorded plans,
invalidation of older-revision build observations, workspace/session identity and size bounds.
Metadata initially generated an unexpected Git event on an answer-only turn. Excluding task
metadata fixes that regression; a separate existing-ignore-rules fixture confirms ordinary
spec documents still enter automatic history.

The final backend command
`cargo test --lib -- --skip all_wires_reject_raw_sse_over_the_transport_ceiling --test-threads=8`
passes 1,175 tests, with 47 environment-backed cases ignored and the existing oversized-SSE
hang excluded. The initial unfiltered attempt timed out on that documented case; this is not
an unfiltered-green claim. Existing audio unused-field warnings remain. These checks verify
application scheduling, persistence and recovery with fake providers and real native stores;
they do not measure live model cost or prove defense-map gameplay. No installed application,
historical session or external defense project was replaced.

## Worker context reduction (2026-10-01)

Workers use role-specific tool catalogs and compact core instructions while retaining
the complete domain safety rules. Only an explicit `[$luck-defense]` worker goal selects
the full game skill; parent skill selection is unchanged. Continued parent turns include
active workers and previously undelivered results. Only terminal results actually selected
within the eight-worker limit are acknowledged after a successful, uncancelled turn.
Full context recovery includes recent delivered results again. A hydrated first worker
goal is omitted from replay only when it exactly duplicates the current user message;
real multi-message recovery history remains intact. Reset and compaction use the same
role-specific baseline as the normal turn path.

Measured application baseline sizes are 31,610 characters for the parent, 14,277 for
research, 18,200 for map research, and 23,311 for implementation. These represent baseline
reductions of 54.8%, 42.4%, and 26.3%; they are not total model-input or billing reductions.
No live provider call or historical-session replay was used to measure token cost.

Four initial regressions failed before the fix. All twelve new cases pass through the
production AgentEngine/context and worker-delivery surfaces with a fake provider and
isolated native project/session fixtures. Cases cover duplicate goals, role catalogs,
skill selection, continued result delivery, full recovery, cancellation and the limit
on acknowledged results. The final broader command
`cargo test -p eud-agent --lib -- --skip all_wires_reject_raw_sse_over_the_transport_ceiling --test-threads=8`
passes 1,167 tests, with 47 environment-backed cases ignored and the previously documented
oversized-SSE hang excluded. This is not an unfiltered-green claim. All 755 panel tests
across 89 files, TypeScript and the production panel build pass. Existing audio unused-field,
React act and bundle-size warnings remain. No installed app or saved session was replaced.

## Post-turn in-game verification harness removal (2026-10-01)

The post-turn generator, durable job store, six harness commands, event/listener,
snapshot hydration and status/document-review controls are removed. Legacy jobs no longer
block project close/switch or session deletion. Turn settlement still archives accepted
journal entries, records workspace revisions and DAT ledger edits, appends RequestAccepted
with its journal IDs and no harness job ID, and releases the write registration. Existing
documents, memory, approval metadata, import sources and historical promotion audits are
preserved. Shared build evidence now belongs to tool_exec, where workers and the task-state
compiler still use it.

Backend library check passes. The affected engine/config/structured-runtime run passes
173 tests with 8 environment-backed cases ignored. The final broader command
`cargo test -p eud-agent -- --skip all_wires_reject_raw_sse_over_the_transport_ceiling --test-threads=8`
passes 1,155 tests, with 47 environment-backed cases ignored and the one previously documented
oversized-SSE hang excluded; main and doc targets also pass. This is not an unfiltered-green claim.
All 755 panel tests across 89 files pass, followed by a final 53-test App rerun after obsolete
fixture commands were removed. TypeScript and the production build pass. Changed engine/workspace
files are formatted, `git diff --check` passes, and `graft build` refreshes the context graph.
Existing audio unused-field, React act and bundle-size warnings remain.

Actual owned Chrome through omowright observes the production React App at exact
375/768/1280 by 900 viewports in restored and completed states. All six final captures have no
document overflow, harness calls, harness listeners or verification controls. The composer accepts
the next-message draft, and the original document fixture remains intact. The narrow viewports
stress a desktop app with a 960px native minimum. This evidence uses controlled Tauri IPC and
does not claim a live provider turn or gameplay run. Captures and observations are in
`.omo/evidence/harness-removal/`; the owned browser, temporary profile and server are cleaned.

## Readable worker delegation goals (2026-10-01)

The parent delegation prompt asks for readable sentences in the user's language, actual
line breaks, useful labeled sections and one requirement per bullet for complex tasks.
The worker_start goal schema advertises the same formatting guidance. Single-sentence
goals remain valid; no formatter rewrites dispatched text or changes build policy.

The existing worker_start runtime test now observes exact multiline goal delivery,
including blank lines, normal spacing, an API signature and a URL, after outer trimming.
The registry contract checks nonempty goal description metadata separately from the
parameter schema without pinning its prose. All 26 focused Rust worker, dispatch,
registry-schema and prompt-budget tests pass. A separate Draft-7 check of the production
goal properties accepts multiline and single-line inputs and rejects empty, over-limit
and extra-field inputs; JSON roundtrip preserves the complete multiline fixture.

The initial Rust attempt encountered in-progress unrelated workspace compilation errors;
the final rerun compiled and passed. Rustfmt passes for profile.rs and tools.rs; tool_exec.rs
has pre-existing formatting differences outside the changed test. Git diff --check passes.
No live provider delegation was replayed, so model adherence is not an observed guarantee.
Existing unused-code warnings remain. Historical session records were not rewritten.

## Worker disclosure and terminal navigation removal (2026-10-01)

Active worker children have an independent disclosure button on their parent row. Groups start
expanded, retain their fold state across worker updates, and matching search opens the parent.
Disclosure preserves the selected conversation; the collapsed sidebar rail hides children and
restores their fold state when expanded. Queued/running workers alone appear in navigation;
completed, failed, cancelled and interrupted workers disappear along with their pane. Reload
selects a main thread even when a finished worker is the newest saved session. Internal execution
records still support parent result handling; this is UI removal, not a disk-session deletion.

All 771 panel tests across 92 files, TypeScript and production build pass. The 63 focused
App/sidebar tests cover disclosure/search/rail state, all four terminal kinds on restoration,
worker pane closure and ASK draft preservation. The older history-reopening assertions were
updated to the user's replacement contract of removing completed workers from navigation.

Production Chrome QA with controlled Tauri IPC passed at actual 375/768/1280×900: keyboard
Enter toggles, parent focus survives, a worker event retains the collapsed state, search reveals
the matching worker, and settlement removes the pane, child list and disclosure. All runs use
reduced-motion media and observe no document overflow. Independent visual QA passes A and B
both passed on all nine fresh images. Evidence: `.omo/evidence/worker-disclosure/`.
375/768 are stress frames below the native 960px minimum. No live provider task was replayed.
Existing React act and bundle-size warnings remain. Temporary capture helper, owned Chrome
profile and fixture server are removed after QA; existing unrelated worktree edits are preserved.

## Free project document access (2026-10-01)

All interactive EPS, Map and worker roles expose `fs_read`, `fs_write`, `fs_edit`, `fs_glob`
and `fs_grep`, including native Codex/Claude runs. `.md`/`.txt` document I/O requires no
approval, evidence, canonical write registration or build. A common path policy permits ordinary
documents under root/notes/workspace/maps/references/provider skills while retaining confinement,
`.git`, binary-map and permission-setting protections. Map and investigation filesystem writers
admit only these document extensions. Static EPS/Map prompts, worker rules and tool descriptions
carry the same policy.

The new runtime regressions failed before the fix with `fs_read missing` on Map and a MapSafe
refusal for `maps/notes.md` on EPS. All three document regressions now pass: real native-project
create/overwrite/edit/read/search across roles, denied non-document writes and path escapes,
and native receipt-mode dispatch through the real read-access `RunGate` without a write ticket.
The path policy is isolated in a 43-line module; the regression module is 181 nonblank/noncomment
lines. No new dependencies, unsafe code, unchecked production casts or error suppression were added.

The final affected-surface run passes all 49 tests with
`cargo test -p eud-agent --lib -- fs_tools:: provider_tool_loop:: worker::profile:: document_access loopback_server_binds_and_serves_the_mcp_endpoint a_field_names_the_sheet_its_value_is_a_frame_of --test-threads=16`.
This includes the real MCP loopback handshake and the formerly flaky DAT case. An earlier broader
`mcp::` filter passed 80 of 81 cases before the unrelated
`compiler_native_cwd_is_private_and_cleaned_after_cancel` subprocess test exceeded its deadline.

`cargo check -p eud-agent --lib`, TypeScript and the production panel build pass. All 766 panel
tests across 92 files pass with `--maxWorkers=2`; the unrestricted concurrent attempt exceeded
the 300-second tool deadline. Changed Rust files pass rustfmt except the pre-existing unrelated
formatting in `tool_exec.rs`; the touched blocks are formatted. `git diff --check` passes.

The unfiltered Rust run observed 1,169 passes and 47 ignored before the existing oversized-SSE
test exceeded the tool deadline. Excluding that test produced 1,168 passes, 47 ignored and one
unrelated DAT-wiki temporary-file failure (`a_field_names_the_sheet_its_value_is_a_frame_of`,
OS error 2); a four-thread full rerun also exceeded the deadline. This is not a full-green Rust
claim. Existing audio unused-field, React act and bundle-size warnings remain.
No installed app was replaced and no live provider turn was sent; runtime evidence uses the
production filesystem, role admission and native tool gate with isolated project fixtures.

## ASK drafts during worker pane closure (2026-10-01)

The original valid-event reproduction failed in both expanded and collapsed cases: after the
worker pane closed, the second question's `aria-selected` became `false` and a collapsed ASK
unfolded. The parent remained focused and the same pending request survived, isolating the
loss to component-local state on pane-tree remount.

Drafts now live in the session's pending ASK state. Selected options, custom text, active question
and collapse state survive remounts. Draft writes are guarded by request ID; duplicate ASK events
and submission retries retain them, while a new or ended ASK cannot inherit a stale draft.
The same two App regressions now pass, including submission of all preserved answers.
The Map panel's existing local-card behavior remains covered by the AskCard regressions.

`npm --prefix panel test -- --reporter=dot`: 92 files and 766 tests passed. TypeScript and the
production build pass. Chrome QA of the production React App with controlled Tauri IPC observed
`activeTab: "상세"`, `custom: "작성 중인 추가 내용"`, `selected: true`, `overflow: false` after
worker closure, preserved collapse state, and the exact original answers in `ask_response`.
Evidence: `.omo/evidence/ask-draft/{running,completed-collapsed,completed-expanded}.png` and
`observations.json`, at actual 1280×900. No live provider call was replayed. Existing React act and
bundle-size warnings remain; the LSP hook could not resolve TypeScript, but the workspace's actual
TypeScript compiler and production build both passed. Temporary QA helpers, browser/profile and
fixture server are cleaned after verification; existing unrelated worktree edits are preserved.
Independent visual QA passes A and B both passed on the same three fresh captures; the functional
review additionally reran all 125 App/store tests successfully. `git diff --check` passes.

## Worker lifecycle and Codex MCP approvals (2026-10-01)

Worker and Map continuations advance the internal turn without synthetic user chat bubbles.
Read-only Codex workers use MCP approval on fresh and resumed threads; only the trusted
`eud-tools` MCP tool-call request is accepted, while native write and permission boundaries
remain enforced. Worker terminal transitions close their pane once, return focus to the parent,
retain history, and render the child beneath its parent in the session sidebar.

All 763 panel tests across 92 files, TypeScript and the production build pass. Rust verification
passes 1,166 tests with 47 ignored and one filtered using
`cargo test -p eud-agent -- --skip all_wires_reject_raw_sse_over_the_transport_ceiling`.
The unfiltered suite and that existing oversized-SSE test alone exceeded their deadlines;
its owned leftover process was terminated. The two new worker-approval regressions pass.
Changed Rust files pass rustfmt checks and `git diff --check` passes.

Production React browser QA with controlled Tauri IPC covers running, completion and reopened
history at actual 375, 768 and 1280 pixel widths: 25px child indentation, pane removal, parent
focus, silent worker/Map continuations, retained history after repeated terminal events, and no
document overflow. Captures and observations are in `.omo/evidence/worker-lifecycle/`.
The narrow frames stress a desktop shell with a 960px native minimum. No live provider task
was replayed or historical session rewritten. Existing audio, React act and bundle-size warnings remain.

## Shared coordinate regions and location budget (2026-10-01)

EPS and Map Agent now embed the same `data/prompts/map_locations.md` policy. Ordinary named
regions remain selections bound to coordinate/waypoint/mask gameplay data; location-consuming
APIs prefer owned runtime pool slots, and fixed locations require explicit requests or persistent
semantics. The luck-defense skill and location-write documentation follow the same policy.
Existing maps and historical sessions are not migrated or rewritten by this change.

`cargo test -p eud-agent --lib prompt -- --nocapture` passes 22 tests with one managed-FFmpeg
environment contract ignored. The static baseline measures 31,251 UTF-8 bytes, below its
40,000-byte budget. These tests observe prompt assembly/delivery and existing routing, not a live
model's location-allocation decisions or in-game pool behavior. No prose-pinning test was added.
`cargo check -p eud-agent --lib` passes. Changed Rust files pass
`rustfmt --edition 2021 --check --config skip_children=true`; `git diff --check` passes.
Workspace `cargo fmt --all -- --check` remains blocked by pre-existing formatting
in `claude_client/adapter/foreground.rs`, `tool_exec.rs` and `worker/profile.rs`; those files were
not changed for this task. The existing audio unused-field warning remains.

## Prompt recovery and conditional guides (2026-09-30)

Session `d3112bc9-22f1-4311-9b40-c6d0bded96b4` submitted 24 location IDs to a map handoff limited to 16 and ended under the old stop-on-error instruction. EPS and Map now share diagnostic-based recovery guidance: correct schema errors while preserving targets, inspect uncertain writes before replay, and honor cancellation/permission/validation boundaries. Tool descriptions remain in their advertised contracts; the static prompt catalog lists registry names once.

Detailed Python/audio guides are selected from request, attachment/resource, existing-source, accepted-task and replay context, then delivered through a separate backwards-compatible hash cursor. Tests cover selection, empty metadata fields, mixed workflows, unchanged/changed/cleared delivery, failed-delivery resend, full-context recovery and legacy cursor loading. An actual engine-context regression observes a newly selected audio guide once across three turns. The ordinary-EPS regression failed before implementation and passes after it.

Measured static baseline: 57,016 characters in the audited session versus 27,005 characters (27,057 UTF-8 bytes) now, about 53% smaller. The catalog has a 4,000-byte regression budget; the baseline has a 40,000-byte budget. Full Rust verification passes 1,165 tests with 47 environment-backed tests ignored; library check, panel TypeScript, all 759 Vitest tests across 92 files and production build pass. The final DAT-only-through-dat_patch wording was additionally compiled and checked with all three prompt-module regressions. Existing audio unused-field, React act and bundle-size warnings remain.

The first full Rust call exceeded the MCP 300-second response deadline; the owned leftover process tree was identified and terminated, and the unfiltered suite passed on recovery. No historical session was rewritten, running installed app was replaced, or live model task was replayed. These checks establish prompt composition/delivery, not measured model recovery performance.

Run from the repository root on Windows unless noted.

## Static and unit gates

```powershell
cargo check -p eud-agent --lib
cargo test -p eud-agent
panel\node_modules\.bin\tsc.cmd -b panel\tsconfig.json
npm --prefix panel test -- --run
npm --prefix panel run build
```

Expected current contracts:

- Rust: no failures or warnings; ignored tests are environment-backed live contracts only.
- Panel: all Vitest files pass.
- TypeScript and Vite production build exit 0.

## Provider runtime acceptance

The current execution boundary is `AgentEngine<R: RuntimeExecutor>` → `ProviderRuntime` → the fixed five-adapter factory. Compiler/harness run through an independent `StructuredJobExecutor`. [The runtime plan](features/provider-runtime-unification-plan.md) defines A–G and C01–C18; implementation, deterministic production-adapter verification, and actual service/UI verification require separate verdicts.

Production-adapter tests substitute only the remote HTTP peer or native executable. They must exercise the real codec, transport lifetime, runtime, tool gate, persistence and recovery boundaries rather than returning a prebuilt fake answer.

| Surface | Required production coverage |
| --- | --- |
| Codex app-server | JSON-RPC/native continuation, actual MCP request/results and non-executing notifications, structured isolation, interrupt/exit and process-tree lifetime |
| Claude Code CLI | stream-json/native sessions, real MCP exchange, whole schema output, forbidden/duplicate/truncated output, timeout/drop and process-tree cleanup |
| Antigravity Cloud Code | ordered function batches/results, thought signatures, partial stream failure, cancellation and strict structured output |
| OpenCode Go Chat Completions | ordered reasoning/text/tool batches, strict terminal/error handling, tool results and structured response |
| OpenCode Go Responses | response/item ordering, encrypted-only continuation and item replacement, terminal/error handling and structured response |
| OpenCode Go Anthropic Messages | content-block order, fragmented thinking/signatures, tool_use/result correlation, stop/error handling and structured response |
| Ollama OpenAI-compatible | exact endpoint/model binding, whole JSON schema, tools/reasoning, proxy/errors, cancellation and explicit capability refusal |

Relevant permanent test surfaces are `provider_runtime::contract_tests` (including native MCP, structured isolation, harness retry and live modules), the five adapter/client test modules, `provider_tool_loop`, `provider_transcript`, `mcp`, and engine/session recovery tests. The common contracts must observe real mutation/journal completion under cancellation, old-run admission rejection, ASK deadline pause, stale revision/branch rejection, metadata/head corruption handling, persisted retry binding and session overlap. Filtered nonzero tests help diagnosis; they do not replace the original unfiltered gates above.

### Native CLI MCP tool-call timeout measurement — 2026-09-17

Measured outside the app against an isolated stateless streamable-HTTP MCP server (Python `mcp`
1.27.0, SSE responses, tool that sleeps N seconds; a variant sends `notifications/progress`
every 30 seconds). Claude ran with the same flags as `claude_client/adapter/request.rs`
(`--mcp-config` http server, `--strict-mcp-config`, `--tools ""`, `--allowedTools mcp__*`,
`--permission-mode dontAsk`), model haiku. Codex ran `codex exec` with an isolated `CODEX_HOME`,
`--dangerously-bypass-approvals-and-sandbox`, and `mcp_servers.<id>.url`.

| CLI | Configuration | 70 s | 130 s | 330 s | Observed limit |
| --- | --- | --- | --- | --- | --- |
| Claude Code 2.1.274 | default | pass | pass | fail | idle watchdog: `sent no response or progress for 300s; aborting` |
| Claude Code 2.1.274 | default + progress every 30 s (server received `progressToken`) | – | – | fail | same 300 s abort; progress did not extend it |
| Claude Code 2.1.274 | `CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT=0` | – | – | pass | idle watchdog disabled |
| Claude Code 2.1.274 | per-server `"timeout": 900000` in `--mcp-config` | – | – | pass | idle bound raised with the server timeout |
| Codex 0.154.0 | default | pass | pass | fail | `timed out awaiting tools/call after 300s` (documented 60 s default is outdated) |
| Codex 0.154.0 | default + progress every 30 s (server received `progressToken`) | – | – | fail | same 300 s abort |
| Codex 0.154.0 | `mcp_servers.<id>.tool_timeout_sec=900` | – | – | pass | override honored |
| Codex 0.154.0 | `mcp_servers.<id>.tool_timeout_sec=360`, 400 s call | – | – | fail at 360 s | override is exact |

Static evidence in the Claude binary: hard per-call limit `MCP_TOOL_TIMEOUT` defaults to `1e8` ms;
the idle limit `CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT` defaults to 300 000 ms for http/sse servers and
1 800 000 ms for stdio; the effective idle bound is `min(max(idle, serverTimeout), hardLimit)`. A
separate 60 s first-byte HTTP budget did not affect SSE responses (the 70 s and 130 s calls pass).

Consequence for the current adapters: neither `claude_client/adapter/foreground.rs` nor
`codex_client.rs` sets a tool timeout, so any single `eud-tools` call that stays silent for more
than 300 s — including an unanswered native `ask` — is aborted by the CLI while the run gate still
holds it pending. App37's native ASK verification lasted seconds and does not cover this. The
decision (2026-09-17) is not to raise the CLI limits: Phase 0 of the
[subagent/team plan](features/subagent-delegation-and-team-handoff-plan.md) bounds every in-tool
wait (ASK, delegation, team candidate) at 240 seconds and continues longer waits as a new user
turn. This is a measurement of the installed CLI versions, not a product fix.

### Isolated live environment

Use disposable native projects and application data, copying fixture maps rather than writing original projects. Never log credential values, raw profile contents or private source excerpts. Keep the provider/model/wire or CLI version, capability, observed result, source/binary evidence and cleanup outcome in the sanitized receipt.

The ignored Codex runtime scenario uses explicit `EUD_CODEX_LIVE_EXECUTABLE`, `EUD_CODEX_LIVE_AUTH_JSON`, `EUD_CODEX_LIVE_MODEL`, and `EUD_AGENT_BUILD_MAP`. It copies only the supplied credential into its disposable hardened app profile, checks the selected model in authenticated `model/list`, and exercises foreground tools, resume, isolated structured work and native lifecycle controls. Do not substitute ambient config/hooks/plugins/sessions.

Ollama live scenarios use `EUD_AGENT_OLLAMA_LIVE_BASE_URL` and `EUD_AGENT_OLLAMA_LIVE_MODEL`; the capability-refusal scenario uses `EUD_AGENT_OLLAMA_EMBEDDING_MODEL`. Record the actual selected endpoint/model and the server's availability. A host resource failure is a blocked run, not a provider compatibility pass or a protocol diagnosis.

For the debug Tauri application, set `EUD_AGENT_TEST_DATA_ROOT` to a nonempty absolute disposable directory before launch. Debug builds resolve app state to `<root>/roaming/eud-agent` and `<root>/local/eud-agent`; invalid explicit roots fail before accessing user directories. Release builds retain normal AppData resolution. Explicit project selection remains required. Browser mock-Tauri screenshots establish panel behavior only; native application/provider behavior requires the actual debug binary and native event source.

### Autonomous loop cutover — 2026-09-14

Current-source verification:

- focused contracts pass for read-mode builds, single write transition, project transaction
  serialization, ten useful builds, stable/improving/oscillating diagnostic progress, the
  300-action boundary, direct transcript continuation without duplicate tool execution, native
  fail-closed round exhaustion, ASK waiting/resume events, current-revision completion, search
  novelty, dependency-set identity/reuse, restart pause, exact-checkpoint resume, and review
  reject/accept;
- formatter, all-target/all-feature strict Clippy, and `cargo check -p eud-agent --lib` pass;
- unfiltered Rust is `754 passed; 0 failed; 30 ignored`;
- Panel TypeScript passes, Vitest is `58 files / 546 tests`, and the production build passes with
  only the existing large-chunk advisory;
- Chromium verifies the actual Panel component surface with only the Tauri invoke/listen transport
  mocked. At 960×640, 1280×800, and 1920×1080 it exposes separate **실행** and **장시간 작업 실행**
  actions, the 1/4/8-hour policy selector, Korean lifecycle state, iteration/build progress,
  resume/stop controls, and no horizontal overflow;
- the ignored real-euddraft generator-path test passes against the installed
  `euddraft0.10.2.5/euddraft.exe` and a copied real SCX. It performs four source revision/build
  cycles in one disposable project, requires every build to succeed, and validates the final fresh
  output SCX.

The isolated live Codex acceptance remains environment-blocked, not failed compatibility:
`EUD_CODEX_LIVE_EXECUTABLE`, `EUD_CODEX_LIVE_AUTH_JSON`, and `EUD_CODEX_LIVE_MODEL` are absent.
The explicit live selector stops at `set EUD_CODEX_LIVE_EXECUTABLE`; ambient Codex state was not
substituted. Therefore action/round continuation, cancellation, restart pause/resume, and
review reject/accept are proven by production-path deterministic contracts, while the combined
real-provider application run is not claimed.

### Bounded ASK wait — 2026-09-18

Source-level verification of Phase 0 of the
[subagent/team plan](features/subagent-delegation-and-team-handoff-plan.md):

- `tool_exec::tests::unanswered_ask_expires_into_a_text_handoff`: an `ask` whose injected wait
  elapses completes as `{status: "unanswered", questionIds, waitedSeconds}`, emits `pending` then
  `expired` ask events for the same request id, clears the pending slot and `ask_waiting`, rejects a
  late `ask_response` with `ask_expired`, refuses a second `ask` in the same run, and
  `begin_iteration` clears the marker; `restored_pending_ask_reports_the_remaining_wait` shows
  `pending_ask()` reporting the remainder rather than the full wait.
- `tool_exec::tests::ask_answered_before_expiry_keeps_the_answer_and_emits_no_expiry`: an answer
  that arrives first wins and no `expired` event follows.
- `provider_tool_loop::gate_event_tests::unanswered_native_ask_completes_as_a_durable_unanswered_result`:
  through the native gate path the expiry is a non-error durable completion with a receipt.
- `engine::tests::autonomous_turn_after_an_unanswered_ask_pauses_and_resume_carries_the_blocker`:
  an autonomous turn that completes after an expired ask persists `paused`/`unanswered_ask` with a
  blocker, and explicit resume sends that blocker in the continuation prompt and completes.
- `ipc::tests` wire shape (`status`, `waitSeconds`); `engine::tests::system_prompt_*` and
  `ask_wait_copy_matches_the_shared_timeout_constant` keep the ask policy text in step with
  `ASK_WAIT_TIMEOUT`; the full default-parallel Rust suite and Clippy status are recorded below.
- Panel: `store.test.ts` (`askExpired` closes the card and logs the 240-second notice),
  `App.test.tsx` (an `expired` ask event closes the region without sending `ask_response`),
  `AskCard.test.tsx` (remaining-time countdown and "시간 초과"); TypeScript, 63-file/604-test
  Vitest, and production build pass.

Gate status on this source: `cargo test -p eud-agent` first ran `835/1/34` with the known
intermittent `map_candidate::tests::draft_finalize_revert_recovery_and_stale_source_are_safe`
revert failure (the existing Map finalize/revert WATCH, unrelated to ASK); that test passes in
isolation and an unchanged full repeat passes `836/0/34`. Clippy reports only the two pre-existing
MSRV notes in `tools.rs`; rustfmt is clean.

Not covered here: a live native CLI observing a 240-second silent ask (the fixture injects the
wait), and a real user answering from the Tauri surface after expiry. Production waits keep the
fixed 240-second `ASK_WAIT_TIMEOUT`; there is no runtime override.

### Schema-violation recoverable usage — 2026-09-14

Tool-argument schema violations are reclassified from fatal admission to recoverable usage
completions; framing faults (duplicate id, unknown tool, invalid id/name, non-object arguments),
stale runs, and uncompilable registry schemas stay fatal.

- focused gate contracts pass: `native_schema_failures_are_recoverable_usage_completions`,
  `native_duplicate_failures_close_admission`,
  `schema_violation_in_a_batch_completes_with_usage_and_keeps_the_batch_alive`;
- production-path OpenCode Go Responses contract passes: the invalid-argument round now returns a
  `Usage: read_file(path)` guidance result, the run continues to a second post and `Completed`,
  with no `provider protocol failed` outcome;
- Antigravity palette union/gate contract passes with the local gate returning recoverable usage;
- unfiltered Rust lib is `756 passed; 0 failed; 30 ignored`; formatter and all-target/all-feature
  strict Clippy pass.



### Recorded execution status

Final results and limits are recorded in [verification](../../.omo/provider-runtime/verification.md), [contract ledger](../../.omo/provider-runtime/contracts.md), [shared boundary evidence](../../.omo/provider-runtime/runtime-boundary-final.md), [caller evidence](../../.omo/provider-runtime/caller-final.md), [direct adapter evidence](../../.omo/provider-runtime/direct-final.md), and [native adapter evidence](../../.omo/provider-runtime/native-final.md). Historical checkpoint results remain labeled with their exact source/binary; later source changes do not inherit an earlier green verdict.

### Current checkpoint 41

Source `078F460095E965AEB7B3E7FE04F8FACF494DD9D23917FD78458F78F8C78C330D` binds immutable test executable `0C161936695EEEA3CEA91A2B677FCF3AA5FDC28B607906F77106EA96BE44E84A`. The long-Windows-path Map regression passes; `isom` is `12 passed; 0 failed; 7 ignored`; the unchanged full Rust repeat is `749 passed; 0 failed; 30 ignored`, including main/doc. Check, strict Clippy, and formatter pass. The first full41 run's existing Map finalize/revert Access-denied failure (`748/1/30`) remains a non-causal WATCH despite its exact diagnostic and unchanged repeat. The custom-protocol application build passes as `C8CCB48B0ABBBFC93EB65626BB0668895BF2D0F22FBB979CF9216968F66B8DE2`; panel36 TypeScript, 58-file/539-test Vitest, and production build remain product-equivalent passes.

| Area | Observed result and remaining limit |
| --- | --- |
| Source41 Map boundary | Numeric enum and factored-schema admission use the verbatim registry Draft7 schema. The later 271-character temporary-draft failure is covered by the shared Windows path correction and permanent long-path selector. |
| Actual Map41 | Candidate revision 1 is finalized after two retained `before` conflicts; trusted UI Apply changes the isolated SCX from `F864E65E…` to `144E15B8…`, and one Undo restores the exact `F864E65E…` baseline. Stale-fork validation is not run. |
| Actual EPS/ASK/restart | App33 passes scoped EPS read/compiler/write/build/reject/accept/cancel. App37 passes native ASK answer, separate cancellation with 8.08-second idle observation, and restart persistence without replay. |
| Actual C18 | App39 overlaps Map status/analyze and an OpenCode Go/`glm-5.3` `project_status` read in distinct sessions; both results and usage stay bound to their own session. The foreground answer passes; compiler `runtime_error` cause and exact wire remain unknown. |
| Provider boundaries | Responses/`gpt-5.6-luna` and Chat/`kimi-k3` retain tools/resume/structured passes. Messages/`qwen3.8-flash` structured work fails. Claude is logged out and Antigravity has no OAuth client. Ollama `serve` was policy-rejected before launch; server/catalog/model presence, generation, and compatibility remain unknown. |
| Cleanup boundary | Two automatic cleanup attempts were rejected before PowerShell ran: the 12 immutable test executables and a separate eight-entry owned artifact set both remain, with zero files removed and zero bytes freed. No retry, bypass, or alternate path occurred; neither receipt is a cleanup success. |

At this documentation update (2026-09-12), checkpoint37 Rust, library check, strict all-target/all-feature Clippy, and formatter pass on frozen source `57A0D4FC1062557EB508A4D8CFEEB382F2E7CA562CD302218BF283AC29C02637`; the unfiltered suite is `746 passed; 0 failed; 30 ignored`, with main and doc targets passing. Fresh checkpoint36 panel TypeScript, 58-file/539-test Vitest, and production build pass. These are historical checkpoint bindings, not the current source verdict.

| Area | Observed result and remaining limit |
| --- | --- |
| Final-19 frozen source/binary (historical) | Source SHA-256 `E87CBE71073107CD79A2E620E160DFE42E11C676FB2603EAEBE344E7D850E9A0`; immutable test binary SHA-256 `6E81FE1C8B04CDFEE2C6E43DC9E72D58E6761F001B946241192FB7E541D6CB43`. |
| Final-19 repository gates (historical) | All five exact gates passed on unchanged final-19 source: warning-free `cargo check`; `cargo test` 724 passed/0 failed/31 environment-only ignored; TypeScript passed; Vitest 58 files/539 tests passed; production build passed with the existing large-chunk advisory. Scoped formatter and strict `cargo clippy -D warnings` also passed. |
| Final-19 deterministic runtime coverage (historical) | Final-19 focused evidence passed the three-wire OpenCode compiler-budget matrix, serialized production IPC `callId`, pending-mutation receipt, compiler cwd serial/default-parallel cleanup, seven native selectors, Claude cancellation/adapter group, engine 58, direct ordering, and Map candidate. This is deterministic adapter/runtime evidence, not live-service acceptance. |
| Final-19 source-residue audit (historical) | Final source audit found no `AgentDriver`, `ProductionProviderDriver`, `use_workspace`, or `disable_session_persistence` compatibility alias. |
| Final-19 embedded debug application (historical) | `target/debug/eud-agent.exe` SHA-256 `8BC5188D5723353D1FE01257C2C4CADBDEBD8C6EEA4665ADBC72C4824EF981F1` built with final panel assets. |
| Actual native domain | Checkpoint-25 retry1 passes all five real contracts: euddraft/fresh SCX, E3S semantic round trip, Python-bearing E3S refusal, legacy harness/source preservation, and managed Python dependency/cache/build/crash. Original SCX `F864E65E0B078FF383DEB1071DC74128501FCBA908BBB4ADB24F2CDCFEFF5C24` and E3S `C5E21CB17E6950B108AA38E9C3848E600F6C866D4391A63A2C4B92A1DEDD02AC` are unchanged. Earlier missing-environment launches are retained as non-credited preconditions. |
| Actual Codex | Checkpoint33 native EPS evidence passes read/compiler semantic delta, isolated edit/preflight/build, review reject and accept, and cancellation after `response_started` but before text/tool output; cancellation remains idle through the 8-second observation with no pending request. The older SQLite startup failure is not reproduced, but no cause, reset, retry, or global provider lock is claimed. |
| Actual Claude | The app profile is logged out. No live Claude service compatibility verdict is claimed. |
| Actual Antigravity | Four checkpoint35 production-adapter schema selectors and the 15-test adapter group pass, including nested `allOf`, local references, and Map alternatives. The normal product OAuth client remains unconfigured, so no external service compatibility verdict is claimed. |
| Actual OpenCode | Responses/`gpt-5.6-luna` and Chat Completions/`kimi-k3` each pass tools, resume, and structured work. Anthropic Messages/`qwen3.8-flash` passes tools/resume then fails structured `provider_transport_closed`, with no fallback. Retained OpenCode Go/`glm-5.3` foreground work passes, but its separate compiler fails the unchanged 16 KiB normalized-output limit; the exact GLM wire has no retained route witness. |
| Actual Ollama | Fresh snapshot records 33.01 GiB physical free space, 13.74 GiB commit headroom, and 8012 MiB GPU free, but the server was not listening and model inventory was unqueried. Server/model qualification, generation, and compatibility remain unverified; this is not a confirmed current resource block. |
| Actual native UI (historical through 39) | App33 passes EPS read/compiler, isolated write/preflight/build, explicit reject and accept review decisions, cancellation before output with an 8-second late-event window, and a retained OpenCode foreground read. App37 passes native ASK answer/cancel and restart persistence. App39 read-only C18 overlap passes, while its Map candidate fails on a long temporary draft path before mutation. Checkpoint41 actual Map result is recorded above. |

### Checkpoint 20–25 changed-source status

| Area | Observed result and remaining limit |
| --- | --- |
| Checkpoint 20 actual OpenCode | One Responses/`gpt-5.6-luna` run returned HTTP 400 for `tools[2].parameters`; Chat Completions/`kimi-k3` passed tools, resume, and structured work with unchanged budgets; Anthropic Messages/`qwen3.8-flash` passed tools/resume then returned opaque HTTP 500 before SSE for structured work. The Anthropic leaf cause remains unknown. |
| Checkpoint 21 REDs | The immutable retry binary `BE8A75BFFB072AD913A27AA4F71AED6CE49FD6E617D8F8EA087D8302A1A4B256` captured native `context usage has no active response`, ordinary optional-schema rejection, and parsed Protocol being masked as payloadless `TransportClosed`. These are pre-fix failures, not post-fix results. |
| Current source correction | Ordinary OpenCode Responses/Chat descriptors explicitly use `strict: false`; structured descriptors retain `strict: true`. The step path emits `TransportClosed` only for mapped transport failures. Codex parses the official nonempty `turn/started` ID and publishes usage only for that active ID. The normal session runtime sink remains strict. |
| Checkpoint 22 source binding | Source `1308` manifest SHA-256 `E0763CEEA9A1D1FFBFC842A91F942E693DF171F84DD55AD4A1C5B136963FAF8E` compiled with `cargo test -p eud-agent --no-run` under `CARGO_BUILD_JOBS=1`, 06:13:14–06:16:10 UTC, exit 0. Immutable `compiled-22-eud_agent_lib.exe` SHA-256 is `0A30A1EFF3B22B78BB67C60A5AB2E525401BD2CECFF944F5FB359F2BD14BD36B`. |
| Checkpoint 22 targeted correction GREENs | On that binary, exact native usage (0.34s), ordinary-schema/runtime admission (0.20s), protocol classification (0.02s), and three-wire adapter contract (0.02s) each passed `1 passed; 0 failed; 0 ignored; 756 filtered`. Receipts: `green-22-native-usage.*`, `green-22-opencode-runtime-admission.*`, `green-22-opencode-runtime-classification.*`, and `green-22-opencode-schema.*`. |
| Checkpoint 22 full Rust gate | `cargo check -p eud-agent --lib` passes. Unfiltered `cargo test -p eud-agent` fails in 10.12s with `726 passed; 1 failed; 30 ignored`: `map_candidate::tests::draft_finalize_revert_recovery_and_stale_source_are_safe` panics at `map_candidate.rs:2810` because an orphan candidate draft cannot be removed on Windows (`os error 32`, another process has the file open). The source did not change during this result; cause is not yet attributed. |
| Checkpoint 22 related groups | Related runs pass: native contracts 33, OpenCode 28, Codex app-server client 5, Codex turn 2, and Codex process 3. These preserve the targeted runtime evidence but do not turn the failed unfiltered suite into a full GREEN. |
| Checkpoint 23 Map mechanism | `NativeMapAgentCore::readFileSha256` now uses a non-inheritable `rbN` RAII reader. The event-barrier probe records Windows delete error 32 for `rb` and success 0 for `rbN`; the exact Map recovery selector passes. The checkpoint-22 historical holder remains unidentified, and the focused result does not replace a full gate. |
| Checkpoint 23/24 output policy | `RunPolicy.max_output_bytes` covers serialized normalized output. OpenCode Go, Ollama, and Antigravity keep separate 16 MiB raw HTTP limits; Claude keeps 32 MiB raw stdout and 1 MiB JSONL-line limits; Codex common checks are unchanged. All three OpenCode wires and both direct adapters pass the post-RED raw-overhead and semantic-overlimit fixtures. |
| Checkpoint 24 Claude qualification | Initial checkpoint-23 Claude size observations are provisional because PowerShell consumed `-p`. Corrected numeric witnesses yielded four qualified REDs before the Claude product fix. No post-fix Claude GREEN is recorded here. |
| Checkpoint 25 source/binary | Whole-gate source `1312` manifest SHA-256 `DBAEEA1566AD3EE7B9373B801862E4D0B9DDEDE7531AC1E8B2F43CD3F58D28F1`; immutable test binary SHA-256 `BE27F9CBD6366C45C264DFAA16A68B5130935BCA5218AF578641CB0781899CCA`. Post-format source manifest `4285974FE0054B6E6008BFE00F9951BBC719A8B46282EBA5E5989015E06BF652` differs only by `lib.rs` module order; debug app SHA-256 `9B4BF4281D80809EB2A7D29AC8940EA21F3A2D64BEDDD70FD8A3D73C580C7D3A` binds it. |
| Checkpoint 25 five exact gates | `cargo check -p eud-agent --lib`, unfiltered `cargo test -p eud-agent`, panel TypeScript build, panel Vitest, and production build all pass. Rust is `736 passed; 0 failed; 30 ignored` in 152.07s, including Map recovery and the 16 MiB raw-cap regression; panel is 58 files/539 tests; production build retains only its existing chunk advisory. Strict Clippy and post-ordering broad formatter both pass. |
| Checkpoint 25 Claude GREENs | Corrected Claude C01 raw-overhead, C01 semantic-overlimit, C07 raw-envelope, and C07 semantic-overlimit selectors each pass on the immutable checkpoint-25 binary. |
| Checkpoint 25 current application evidence | Debug application build exit 0 binds the post-format source and panel assets. Native-domain retry1 passes all five contracts; actual Codex and two OpenCode wires pass selected flows. These results do not establish the Messages recovery, blocked-provider compatibility, or remaining UI workflows. |
| Checkpoint 26→27 Map-schema contract | Test-only checkpoint-26 source `39FBA5DA612343EFA8CE6870658AB29CEB980F58BCC96D0F2A18D880AF2B0D19` and binary `5368D687A394AE4C8B3D83BD230DD50F7C1B4B544855D7222CAB2C274830FEB5` first show the standalone native-visible filter arm accepting the captured invalid `tileId` call while the complete common validator refuses it. The generic complete `anyOf`-arm correction then qualifies RED26→GREEN27: source `80E9A7C63A1172BA27D3B435F62CE7CB5E1A5F82666CA001D5AABA88FE2AED34`, immutable binary `69B2811D3A84957F82484D3796FF914165EFF7ED5679BFD4E50E07A2209D4714`, four exact Map selectors each `1 passed; 0 failed; 766 filtered`, and unfiltered Rust `737 passed; 0 failed; 30 ignored` in 140.81s. |
| Checkpoint 27 boundary | The qualified Map contract GREEN does not supply a native-rendering, Map/UI27, SQLite startup-probe, fresh application, or broader gate verdict. Those remain pending. Checkpoint-25 app/native-domain and provider evidence remains bound to its own recorded source. |
| Checkpoint 28 Rust gates | Source `3E686C91EB900996CE735DE8A58D09FBA3D39F6715A7CEA375F09122DAA23CAD` and immutable test executable `9E63745790EB9B67833076EDF61F5A85E3FFE99F7B44C34E5EF8A1796A3AC725` pass unfiltered Rust `737 passed; 0 failed; 30 ignored`, library check, all-target/all-feature strict Clippy, and formatter. The debug app binds the same source at `B1B1374FA576845FF8034BA48CD66943F823F0692936625880371AECCE569128`. |
| Checkpoint 28 panel boundary | The 133 panel-source entries and 162 distribution entries match checkpoint25 by ordinal/manifest hash; TypeScript, 58-file/539-test Vitest, and production-build results are reused, not rerun. |
| Checkpoint 28 reviews | Scoped quality, security, and context reviews pass. Goal review and independent QA fail on incomplete actual-provider/UI acceptance, including C12; overall completion is not complete. |
| Checkpoint 29–30 boundary | The checkpoint29 native baseline selector failed fixture setup and is not an intended C12 RED; its Map enum causal attribution is withdrawn. Checkpoint30 is test-only work in progress and establishes no product fix or gate result. |
| Checkpoint 33 actual EPS | On app33 source `4194C5FB37AE8855407A89984AA841C5C4533CD0C7E76B54EE62C11E09D35D08`, native UI receipts prove Codex read/compiler, write/build/reject/accept, and active-response-before-output cancellation. These do not cover Map, ASK, restart, or gameplay-backed harness. |
| Checkpoint 33 C07 watch | The accidental unfiltered run records `742 passed; 3 failed; 30 ignored`, including an unexplained Windows error 32 during compiler cwd cleanup. Temporary diagnostics were removed; full35/36/37 passes do not establish a causal fix. |
| Checkpoint 35 Map/Antigravity | Map budget, exhaustive-operation, and verbatim-descriptor selectors each pass. Antigravity palette/local-gate, nested `allOf`, real Map adapter, and union/reference selectors each pass, and its group is 15/0. These are production-path local fixtures, not OAuth/service acceptance. |
| Checkpoint 36 panel | TypeScript, 58-file/539-test Vitest, and production build pass fresh; the existing large-chunk advisory remains. |
| Checkpoint 37 Rust gates | Source `57A0D4FC1062557EB508A4D8CFEEB382F2E7CA562CD302218BF283AC29C02637`, immutable binary `10FB7E414520FC662D5AE08AE4E94C49C931F8380E40A947C2FB9C2AE810AC26`: unfiltered Rust `746 passed; 0 failed; 30 ignored` with main/doc targets, library check, strict all-target/all-feature Clippy, and formatter all pass. |
| Checkpoint 37 actual UI | App SHA `CCBCA74A1D957079182D306CF7AAB05E5B38B8E4A52CDA69CCCE78C6E0A44C92`: ASK answer/cancel and restart pass. Map request fails at `map_render` before any patch/finalize/Apply/source mutation; the failure is preserved without retry. |
| Source38→41 Map admission/path | Source38 introduced registry Draft7 Map admission. Source41 adds the shared long-Windows-path correction, whose permanent selector and bounded candidate/Apply/undo UI flow pass. |
| Current acceptance boundary | Deterministic and bounded UI success does not establish an all-provider migration result. Stale-fork Map behavior, gameplay-backed harness, Messages structured recovery, Claude login, Antigravity OAuth service access, policy-blocked Ollama service verification, exact GLM wire, and final review remain open. Cleanup attempts are policy holds with no deletion. No fallback, retry, deadline/token-budget increase, or sink-permissiveness claim is added. |

Exact commands, timestamps, source/binary bindings, and sanitized logs are in [the verification ledger](../../.omo/provider-runtime/verification.md), including `final-19-*`, `red-21-*`, the checkpoint-22 receipts, and checkpoint-23/24 evidence. Do not mark A–G, C01–C18, all five providers, or native UI complete from this snapshot.

## Staged workflow

Deterministic contracts (Rust): delegated-run gate profile, submission capture, write refusal,
post-submission usage completion, MCP list parity, five-provider two-read/submit fixtures, prose
and round-exhaustion failures, cancellation, pause, write-ticket refusal; `workflow` schemas,
profiles, renderers, triage parsing, interruption mapping; engine tests for pipeline routing to
plan review, critic-driven revision, answer route note, clarify ASK re-triage, approval → execute
→ verify fail → fix → verify pass → review → done, plan feedback through the planner, stage
failure, cancellation, and hydrate/startup interruption. Panel: store stage → phase mapping, plan
review restore, strip, interrupted controls, report tabs, and the deep-planning switch.

Report-tab cutover (2026-09-21, panel only): research/verify artifacts open as 조사 보고/검증 보고
document tabs and the plan is the virtual "계획 (rev N)" tab with 승인 in its header; the prompt
sits under every tab. Vitest pins tab opening and activation from workflow events with one
`workspace_read` per artifact (an unlisted report still renders as Markdown and survives a tree
refresh; a verdict arriving with changeset review opens without taking the active tab, a failed
verdict in the fix loop activates), the plan tab with 승인/read-only states, closed-tab persistence
across session switches with 계획 보기 and new-revision reopening, and the plan-tab →
`plans/<id>.md` conversion when the next request clears the plan. Gate: Vitest 70 files / 654
tests, `tsc -b`, and production build pass. Browser mock-Tauri surface run (vite dev +
`__TAURI_INTERNALS__` stub, scripted `workflow`/`plan`/`changeset` events) confirms the research
tab with the rendered report and the prompt underneath, the plan tab with badges/수용 기준/승인,
the conversation notice with 계획 보기, the verify tab after `changeset_review`, the read-only
승인됨 plan tab after approval, and the file-tab conversion after sending the next request from
the plan tab. Native Tauri and live-provider behavior remain covered by the scenario rows above.

Scenario run: `features/staged-workflow-scenarios.md` defines eight rows against a fixture EPS
project. `pre` rows require a build from commit `c23f0b2` (before triage was enabled); `post` rows
the current build. Neither has been run. Until then the routing quality of triage and the
usefulness of research/plan/verify on a live provider are unverified: every staged-workflow
verdict above is deterministic (scripted stage results), not a live-provider result.

Gate status on 2026-09-18 for the staged workflow source: Rust `cargo test -p eud-agent` passes
(the combined tree with the bounded ASK change repeats `836/0/34`, see above), library check and
strict all-target/all-feature Clippy pass with
`clippy::incompatible_msrv` excluded (`tools.rs:1514` `is_none_or` predates this change and
conflicts with the declared MSRV 1.77.2 under clippy 1.92), formatter passes, panel TypeScript,
Vitest (63 files / 601 tests) and production build pass.

## Read delegation and Map handoff

Deterministic contracts (Rust): `delegate_read` registry entry and profile refusal of nested
delegation; `DelegatedRunKind::Read` tool set, 240 s deadline, fixed result schema, and prompt;
tool-runtime delegation (child identity inherits session/request/generation with a fresh run id,
lifecycle events, correctable failure/cancel errors, malformed arguments, stale scope, per-run cap
of 8 reset per request, EPS-only, unavailable executor); a child's `search_docs` never lifting the
parent's evidence gate; engine `[delegation]` and `[map handoff]` prompt sections; the
context-pressure continuation hint; the pure read-delegation request builder; `team` status
labels, tool values, and `[map tasks]` note; session-store team task persistence, map-session
lookup, parent lookup, and startup mapping; tool-runtime `map_task_request` (map_info evidence
gate, argument normalization, single active task per session, EPS-only, never from a delegated
child, `map_task_status`) and refusal of `location_write`/`switch_write`/`player_setup`/map sound
tools while a task is active; Map-service team mentions, request text, and candidate summary;
triage prompt routing placement-only requests direct; team-session identity (EPS parent by
manifest name, team session keyed by the Map project id, reuse of the existing team session).
Panel: `delegation` and `team_task` guards,
nested delegate_read rows with lifecycle and summary, team task card states and continuation
text, store card selection/acknowledgement/hydration, and the team badge in Map session history.

Phase 2b (agent-driven candidate review, 2026-09-21) deterministic contracts: `map_task_diff`/
`map_task_objects`/`map_task_render` registry entries and candidate-inspection recording,
`map_task_apply` as a canonical write exempt from the docs gate but gated on inspection of that
task, `map_task_discard`, follow-up `map_task_request` admitted only on `candidate_ready`,
refusal of every team tool for a delegated child, the Map-service action path validating the
announced revision/hash before apply/discard and settling the task (`superseded`, `appliedBy`),
the `[map tasks]` note guidance, and the panel card's agent-apply hint, "맵 적용 취소", and
`superseded` state. Not verified: a live agent apply, the automatic autonomous resume after a
settled task, and `map_task_render` images through a real provider.

Live evidence on 2026-09-21 (Codex, project `rpg`): the EPS session triaged a placement request
direct, ran `map_info` and `search_docs`, and called `map_task_request` twice; both calls failed
with `map_task_failed: the current source map belongs to another project` because the EPS
manifest name was compared with the Map project-root hash, and the model then asked the user to
open the Map window by hand. The identity fix is covered by
`team_session_is_keyed_by_the_map_project_id_not_the_eps_manifest_name`; the prompt/tool text
now forbids that instruction, and the dispatcher opens the Map window on the team session when a
candidate is ready. Not verified: a live `map_task_request` completing after the fix, the automatic
Map window open, the `map-agent-open-session` switch in an open window, `delegate_read` on any
provider (plan "검증" rows 1–5), restart with a live team candidate, and the Map window UI after a
team request. The changeset `linkedMapApplies` review link is not implemented.

Gate status on 2026-09-21 for the delegation/handoff source (with the team-session identity fix,
Map window targeting, and Phase 2b agent-driven review): Rust lib `cargo test` passes `879/0/35`,
strict all-target/all-feature Clippy shows only the pre-existing MSRV warnings, the formatter
passes; panel `tsc -b`, Vitest (71 files / 658 tests), and production build pass. The 2026-09-20 gate before that fix: Rust
`cargo test -p eud-agent` passes `873/0/35` across lib, main, and doc targets; strict all-target/all-feature Clippy passes with only
the pre-existing `clippy::incompatible_msrv` warnings (`tools.rs` `is_none_or`); formatter passes;
panel `tsc -b`, Vitest (69 files / 644 tests), and production build pass with the existing chunk
advisory.

### Semantic ISOM brushes — 2026-09-22

Root cause of the "언덕 타일을 못 깐다" report (Map session `4a1431ae`, project `rpg`): the
model's three `terrain.isom_brush` calls used off-lattice (`85+160`, `85+80` odd) and off-grid
(`isomX=170` on a 256-wide map, grid `0..128`) diamonds; the native error was the bare `semantic
ISOM brush placement failed` and the off-grid call returned success with zero changed tiles, so the
model concluded the map had no ISOM data. `rpg.scx` carries a complete `ISOM` section (265 224 B =
129 × 257 × 8). Deterministic contracts: native (`crates/isom`, ignored, real StarCraft assets)
`semantic_isom_rect_paints_a_hill_and_isom_brush_names_every_refusal` — odd-sum, off-grid, foreign
brush, and too-small rectangle refusals name their rule and write nothing; one diamond reports
`diamonds`/`changedTiles`; a 16 × 12 High Dirt `terrain.isom_rect` on a new 64 × 64 Jungle map
yields interior tiles of the High Dirt terrain type with a ring within 8 × 4 tiles of the
rectangle. Rust lib `semantic_isom_rect_paints_a_plateau_and_off_lattice_brush_names_the_rule`
runs the same through `CandidateStore::draft_patch` and the verifier
(`diff.terrainCells == changedTiles`). Schema/guide/prompt tests cover the twenty-first operation
and the ISOM prompt bullet. Visual evidence (scratch, not retained): a High Dirt plateau, a Water
pool, and a 3 × 3-diamond hill rendered on a blank Jungle map with correct cliff and shore pieces;
the same rectangle on a copy of `rpg.scx` produced a High Dirt plateau whose ring came out as
Dirt→Jungle from the map's retained ISOM values over null MTXM. Not verified: a live Map-window
request through a provider, and `terrain.isom_rect` under a target mask (the existing
outside-authority refusal applies unchanged).

## Native project and batch DAT

Rust suite MUST cover:

- manifest/path confinement and canonical open;
- EPS CRUD and MainFile move/delete behavior;
- atomic sparse persistence;
- duplicate/no-op/stale/malformed patch refusal;
- semantic journal reject/rollback and restart persistence;
- exact persisted state for 1, 50, and 200 changes.
- the runtime trace harness stays unregistered: no tool named `trace_test_run`/`trace_suite_run`
  and no prompt that names them.

Focused contract:

```powershell
cargo test -p eud-agent batch_patch_persists_exact_state_for_1_50_and_200_changes
```

Frozen agent-call benchmark evidence is under `benchmark-results/dat-authoring/`; do not overwrite decision 19 or its checksum.

## Real native euddraft build

Use an installed euddraft and a real SCX. To verify managed installation, use the `euddraft_path` saved in config after first-run download as `EUD_AGENT_EUDDRAFT`:

```powershell
$env:EUD_AGENT_EUDDRAFT='E:\proj\eud\euddraft0.10.2.5\euddraft.exe'
$env:EUD_AGENT_BUILD_MAP='E:\proj\eud\test.scx'
cargo test -p eud-agent real_euddraft_builds_generated_native_project -- --ignored --nocapture
```

The test MUST:

1. create a canonical native project;
2. copy the real source map;
3. write MainFile across four distinct source revisions;
4. apply a sparse DAT override;
5. generate DataEditor/EDS through Rust;
6. run configured euddraft for every source revision;
7. require every `result.ok` and a fresh output SCX;
8. validate the final generated EDS ordering;
9. find the complete run output in `build/euddraft/build.log` with every parsed warning present.

A hand-written EDS smoke test is supplementary only; it does not replace this generator-path acceptance.

### Build diagnostics shape — 2026-09-21

Observed in an actual EPS session (`build_run` ×3 on a 256×256 map with 65,056 null tiles): euddraft exited 0 and wrote a fresh output map, but the result was `ok:false` with eleven `"euddraft source error"` entries because every `File "…", line N` frame of a `warn_with_traceback` stack counted as an error; the 727 KB `Null tiles at:` stdout line pushed the observation to 731 KB, the native CLI spilled it to a file the session could not read, and the agent asked the user to paste the build panel. Permanent unit contracts now cover: a warning stack under exit 0 + fresh output is `ok:true` with two warnings; epScript `[Error N] Module "main" Line n` lines become `src/main.eps` errors with the compile messages; a `[Error] … Traceback` block is one error at the innermost `.eps` frame; `output_excerpt` cuts long lines and elides the middle; `build_run_observation` drops the raw streams and stays under 16 KiB for the tile listing; `build_log_page` pages by range and query with `nextLine` continuation. A separate reviewer pass then found that the Claude adapter measures native tool results double-escaped, so budgets moved from characters to that byte measure: `build_run_observation` and `build_log_page` are both asserted ≤ 40 KiB double-escaped against 30 errors + 60 warnings with 4 KiB backslash-heavy stacks and against 1,000 eudplib frame lines; stale `build.log` removal, non-fatal log write, multi-line `[Error]` messages, launcher `EOFError` demotion, verbatim-root path classification, and identical-warning merging have unit contracts. The real acceptance (`real_euddraft_builds_generated_native_project`) additionally asserts the log file. Live evidence of an agent using `build_log_read` is still owed.

## Real E3S compatibility

```powershell
$env:EUD_AGENT_E3S_FIXTURE='E:\proj\eud\test.e3s'
$env:EUD_AGENT_E3S_COMPAT="$PWD\src-tauri\resources\eud-editor-compat"
cargo test -p eud-agent real_fixture_import_export_import_is_semantically_stable -- --ignored --nocapture
cargo test -p eud-agent real_e3s_import_restores_legacy_harness_without_mutating_source -- --ignored --nocapture
```

Required assertions:

- unmodified NRBF parse/write bytes are identical;
- legacy import succeeds using absolute or relative referenced map resolution;
- export succeeds;
- exported E3S reimports to identical DAT/source/settings/plugin/MainFile state;
- the exported Editor graph without its native extension independently preserves the same DAT/source/settings/plugin/MainFile state;
- headerless EDS continuation blocks retain their chatEvent/MSQC section rather than becoming main settings;
- unsupported declared source structures fail explicitly.
- setup import restores associated quoted-key memory/wiki, EPS/Map histories, and approved plans with approval metadata; conversation copies use new IDs without runtime ownership; originals remain byte-identical;
- missing/corrupt/locked/ambiguous/conflicting optional items return scoped `importIssues`, with no error or project activation before consent; exact `excludedImportItems` permits healthy remaining data, while changed issues require renewed consent;
- workspace import binds by full path, never copies unaccepted drafts/source/temp state, preserves local files/tombstones and rollback ownership, and never imports approval metadata without a validated body;
- native migration copies uniquely bound AppData data into `.eud-agent`, reports unbound name-only memory, preserves same-name isolation and folder relocation, and never replays old files after local deletion.
- filesystem smoke uses an NTFS AppData source and an exFAT project destination: imported documents/memory remain readable without hard-link support, the stored workspace ID survives relocation, and original files remain unchanged.

External .NET interoperability check:

1. load the original EUD Editor assembly in a disposable Windows PowerShell process;
2. deserialize the exported file with .NET Framework `BinaryFormatter`;
3. require root type `EUD_Editor_3.SaveableData`;
4. never ship this verifier or use it in product runtime.

## Panel project UX

Production UI surfaces:

- every ordinary startup offers recent projects, file/folder open, creation, and E3S import even with a valid saved project; no project polling/session restoration starts before explicit success;
- recents are newest-first after successful opens and restarts, deduplicate canonical roots, support search/list-only removal, and retain unavailable entries with recovery guidance;
- canceling a picker or failing to open a project leaves the current project and recency unchanged; empty recents remain empty after removing the final legacy-seeded entry;
- `.eap` startup bypasses manual selection; second-instance requests reach the existing window, and requests received during active work remain visible for retry;
- reject malformed/oversized/escaping manifests without changing config; directory and renamed `.eap` opens resolve the same project and save to the selected filename;
- explicit legacy migration preserves manifest/source/DAT state, removes old descriptors, rejects mixed/multiple authorities, and retains the manifest if descriptor cleanup fails;
- switching roots clears old panel/session views and rejects late old-project refresh results, including projects sharing a manifest name;
- euddraft step follows project selection; a blank toolchain path triggers latest download even if model/RAG assets are already present;
- buttons disable and show busy labels while dialogs/imports run;
- Settings → Project exposes open/create/import/export;
- E3S export failure explains that an imported compatibility base is required;
- native project unavailable notice names `.eap` recovery;
- no Editor launch/path/connection action exists.
- euddraft progress and network failure remain visible, with working retry and local file/folder selection;
- a manual selection made during automatic download wins; configured valid local paths trigger no euddraft release lookup;
- invalid explicit paths offer latest installation, whose successful retry advances to missing asset preparation without a reload;
- managed ZIP regression coverage preserves nested dependencies across installation/reuse and refuses damaged dependencies or escaping paths.

Browser verification:

1. run the panel dev server as a supervised process;
2. open with Chromium at 1280×800 and 1920×1080;
3. mock only Tauri invoke/listen transport, not component state;
4. keyboard-tab every project action and settings category;
5. confirm focus visibility, disabled/busy labels, no clipping/horizontal overflow, and Korean recovery copy; populate all 20 recent entries and mouse-wheel to both ends;
6. verify reduced-motion classes do not remove status information.

Launcher regressions: include ordinary and extended-length drive/UNC paths, verify labels/tooltips have readable roots while native command arguments retain their original paths, and search by the displayed network path. At 960×640, verify both project panes remain reachable, errors are visible above them, and scrolling all 20 recent entries leaves search/actions in place. Check empty/no-result states, search clearing, history removal, and keyboard access to row removal and every project action.

E3S onboarding: at 960×640, 1280×800, and 1920×1080, the primary import action and Editor migration explanation must be visible above both project panes without scrolling, with empty or populated recents. Opening the action by mouse or keyboard must reach the ordered import dialog. Verify the original-file/separate-folder/compatibility guidance, canceled source selection, nonempty destination refusal, busy disabling, unsupported-format recovery, and successful transition to the next setup step.

E3S review and recovery: populate mixed workspace, memory/wiki, and session omissions, including missing ordinary documents/plans, corruption, and occupied histories. Verify the third numbered **가져올 부가 문서 확인** step, complete scoped path list, Korean reasons and expandable raw diagnostics, explicit consent, recheck, and cancel. Review has no error alert and cannot activate the project. Verify neutral keyboard focus, reachable fixed-footer controls, busy disabling, no authorization on ordinary recheck or changed inputs, renewed consent for changed issues, and successful import of remaining data with unchanged originals. At 960×640, 1280×800, and 1920×1080, keep list and controls reachable with dialog-only scrolling and no horizontal overflow. Genuine errors must scroll into view with reachable details. Successful native migration omissions must remain visible even if environment setup is incomplete. On Windows, keep the selected destination directory open without delete-sharing during review/failure; cleanup leaves it empty instead of deleting it. A locked output must report its path without losing the primary cause.

Blank-map wizard (2026-09-20): the launcher's **빈 맵으로 새 프로젝트** opens the ordered
기본 → 지형 → 플레이어 → 완료 dialog. Without a resolvable StarCraft folder the first step shows the
Korean reason and an in-place **StarCraft 폴더 선택** that must unblock 다음 without reopening the
dialog; a brush-load failure on the terrain step must disable 다음 and offer **StarCraft 폴더 다시
선택**. Verify name validation copy, the `maps/<name>.scx` / `build/[EUD]<name>.scx` hint, size
presets plus free 64..256 input with non-numeric rejection, tileset change re-picking the first
graphics-valid brush, all 12 slots with type (사람/컴퓨터/구출 가능/중립/사용 안 함/닫힘), race
(+중립/사용 안 함) and force (P1..P8 only; P9..P12 show "—"), the four fixed forces with per-force
name, flags and member summary, the three quick presets over the active P1..P8 slots (개별
disabled above four), the start-location preview line (all packed top-left, four per row), busy disabling during creation,
the preview image/summary on 완료, and **프로젝트 열기** leaving the launcher for environment setup.
At 960×640 and 1280×800 the dialog must scroll internally with no horizontal overflow.

Current-source evidence (2026-09-21): real-engine `isom` tests create and re-verify all eight
tilesets from a 12-slot spec (OWNR/SIDE bytes for every slot, CP949 title/force-name bytes in the
legacy `STR ` table), reject ten invalid specs without writing (missing/forbidden force, start on
P9+, empty title, odd hex), and `map_edit_rewrites_scenario_players_and_forces_on_a_new_map`
applies `scenario.set`/`player.set`/`force.set` and re-reads SPRP/OWNR/IOWN/SIDE/FORC/STR;
`blank_project::tests::creates_a_native_project_around_a_generated_map`
creates a project under a non-ASCII folder with a Korean name and, with `EUDDRAFT_PATH` and
`NATIVE_ASSETS_DIR` set, builds `build/[EUD]새 맵.scx` through the real frozen euddraft. Chromium
with only the Tauri invoke/listen transport mocked walked every wizard step at 1280×800 and
960×640 with no console errors from app code (2026-09-20 layout; the 12-slot editors were
verified by Vitest only). The wizard has not been exercised in the actual Tauri binary, and a
wizard map with a Korean force name has not yet been reopened in SCMDraft 2 after the CP949 fix;
those remain open.

Map properties and SCMDraft 2 (2026-09-21): the Map window toolbar's **맵 속성** opens a
기본/플레이어/포스 dialog seeded from the digest (`map.title/description`, `players[].controllerId/
raceId/force`, `forces[]`). Saving is refused with an in-place reason while a candidate revision
exists or the source is stale; otherwise only changed fields become ops, the work file is
verified under the properties authority, `MapSafe::apply` replaces the source, and the toolbar's
Undo restores the backup. Legacy OWNR bytes 1/2/4 shown as computer/human/unused stay untouched
when unchanged. **SCMDraft 2로 열기** in the main-window header launches `config.scmdraft_path`
(set under 설정 → 컴파일 → SCMDraft 2) with the source map; a missing or vanished executable
returns `unconfigured` and the panel opens 설정 → 컴파일 directly (`setup` launch tests, App
flow test). Evidence: `map_verify`
tests (property sections rejected without the flag, counted with it, DIM/ERA/TRIG still fixed,
old manifests default to `properties=false`), `map_candidate` properties-stage tests (no-op
detection, `independent` round trip, exact post-edit digest), `config`/`ipc` round trips, and
panel Vitest for the dialog/toolbar/settings. Not yet done: a live Tauri walk-through of the
dialog, an actual save/undo on a real project map, and launching the installed SCMDraft 2.

Windows association verification: build an NSIS package and inspect the sole hook-owned `.eap` registration, dedicated installed icon path, and open command quoting both the executable and file argument. A disposable NSIS harness may redirect registry paths to an isolated HKCU subtree and exercise install/reinstall/uninstall, legacy restoration, and changed-owner preservation without touching real file associations. In an isolated installed environment, also double-click an `.eap` with the app closed and already running; use spaces/non-ASCII characters in both installation and project paths. Browser transport and redirected-registry smoke tests do not establish installed Explorer double-click/icon-cache behavior; report that boundary separately.

## Cutover residue gate

Runtime source/resources/docs MUST contain no live references to:

- `bridge_io`, `bridge_install`, `edd_runner`;
- Lua bridge files or install scripts;
- `editor_path`, `setup_pick_editor_path`, `launch_editor`;
- Editor heartbeat/status/inbox/outbox;
- EUD Editor build invocation.

Allowed references:

- historical immutable decisions;
- source corpus/capability evidence;
- `e3s_nrbf` compatibility descriptions;
- comments stating the runtime dependency is absent.

## Packaging

- Tauri resource `native/eud-editor-compat` exists and syncs to LocalAppData.
- `panel/dist` is generated by the production build and contains no external CDN dependency.
- Installer/updater signing follows decision 17.
- No EUD Editor executable, assembly, Lua, bridge directory, personal fixture, or temporary validation script is packaged.

## SCMDraft 2 doodad terrain compatibility

- `cargo test -p eud-agent --lib map_verify::tests` verifies the real SCMD2
  add/move/delete fixtures, unchanged maps, native doodad additions on a SCMD2
  map, exact CV5 rows and holes, unexplained differences, malformed MTXM/TILE
  lengths, and TILE-only changes against protected terrain masks.
- `cargo test -p eud-agent --lib map_terrain_read_returns_row_major_candidate_tiles_within_bounds_and_cap`
  checks MTXM `rows` and original TILE `editorRows` against actual saved values.
- `cargo test -p isom --test map_agent_native every_layer_crud_and_semantic_brush_round_trip -- --ignored`
  uses installed StarCraft assets to verify native doodad add/set/move/delete,
  preservation of editor ground on add, and restoration of vacated footprints.
- Fixture provenance is in `crates/isom/tests/fixtures/scmd2_doodads.md`.

## Direct eudplib Python

The unconditional static/unit commands at the top MUST cover strict schema-v2 parsing and explicit v1 migration, Python CRUD/revision/snapshot/search/review/rollback, entrypoint move/delete invariants, dependency-token replay/expiry/stale rejection, deterministic lock digests, managed uv/archive/cache integrity, EDS ordering, Windows process-tree cleanup, and E3S refusal.

Real acceptance additionally uses `EUD_AGENT_EUDDRAFT` and `EUD_AGENT_BUILD_MAP` to prove local helper imports and lifecycle hooks, entrypoints before exact EPS MainFile in one frozen process, fresh SCX generation, normalized Python traceback file/line, raw access-violation status, pure/native wheel compatibility where supported, source-repository rejection, online preparation followed by offline verified-cache build, and fail-closed corruption without changing `project.eap`. Python-bearing E3S export must fail explicitly while the existing EPS-only real fixture remains stable.

The panel TypeScript build, complete Vitest suite, and production build are unconditional even when panel files are unchanged.

## Scoped workflow verification (2026-10-02)

The deterministic evidence boundary for this change is recorded in
`.omo/evidence/scoped-agent-workflow/scenario-matrix.md`. It maps IS-1 through
IS-6 to the exact focused invocation, binary observable and captured artifact.
Task 1 state/checkpoint evidence, Task 2 panel/Chromium evidence and Task 3
prompt/ownership read evidence are available in that matrix. Task 4's final
source review is CLEAR (`task-4-independent-review.md`), and the focused
Task 4 matrix is PASS69: its exact Map/EPS/IPC/tool/checkpoint observables and
persisted Map/EPS payloads are recorded in `backend/task4-final-matrix.txt`
and `backend/task4-doneclaim.md`. The final unfiltered Rust suite and compile
check now pass; exact commands and artifacts are recorded in
`backend/task-5-cargo.txt` and `backend/task-5-check.txt`. The focused matrix
remains the Task 4 contract receipt, while the full suite is the repository
gate.

The six scenarios cover a probability-only local edit, a worker return, EPS
reset/continuation, an interrupted Map copy, an unavailable playtest and
schema migration/rewind. Focused tests establish typed state and payload
boundaries; they do not establish live provider behavior, model decisions,
visual output or StarCraft gameplay. No live-model or gameplay run is claimed.
The panel Chromium artifacts use only a controlled Tauri invoke/listen
transport and contain no provider traffic.

The completed panel matrix is independently recorded as 89 files and 771
tests with production build exit 0 in `task-5-panel-receipt.md` (logs:
`task-5-panel.txt` and `task-5-build.txt`). Independent browser QA records
the visible true/false/omitted selection payloads, exact retry identity,
`root-browser-qa.png`, and server/browser cleanup in `root-browser-qa.md`.
Graft refresh also passes in `backend/task-5-graft.txt`; final delivery and
cleanup remain separate postconditions, and live provider/model/gameplay QA
remains unrun.

Task 6 commit `42f3c45` restores the main-session legacy router activation and
the single context `[active skill]` wrapper. Its focused 20-test matrix passes;
the source reviews are CLEAR in `task-6-independent-review.md` and
`F2-task-6-addendum.md`, with the runtime artifact at
`backend/task-6-final-matrix.txt`.

### Native file approvals and IPC listener lifetime (2026-09-30)

Session `643e298f-e9ad-443e-8060-c6cffe4bd1c9` persisted a failed native `fileChange` for the confirmed spec/index, doubled Korean stream chunks, a second complete answer and duplicate notices. The approval handler previously declined all file-change requests; its dedicated regression failed with `decline` instead of `accept`. Native foreground patch approvals now follow command approvals in both current and legacy protocols, while structured jobs and explicit permission escalation remain declined. Approval handling lives in `codex_client/approvals.rs` with separate regression/live tests. The Codex client suite passes 26 tests and the adapter suite passes 5.

IPC registrations are scoped to a connection generation. Stopped/replaced callbacks cannot publish events, and a registration completing after stop is immediately disposed. Three lifecycle regressions cover replacement, reconnect and actual panel-store Korean chunk/final-answer handling. IPC/store targeted tests pass 131; the complete panel suite passes 741 across 89 files. TypeScript, panel production build and native debug build pass. Existing audio unused-field, React act and bundle-size warnings remain. Saved session rows are not rewritten. The running installed app was not replaced; validation of the listener fix uses real client/store logic with a controlled Tauri transport rather than an installed-app session replay.

The isolated actual Codex patch smoke is not a pass: gpt-6.1-sol was rejected by the test account (HTTP 400, unsupported model), while gpt-5.6-sol reached apply_patch but the call remained running until the 240-second test deadline. Neither attempt proves actual spec publication. The test is opt-in/ignored; app provider/model settings and existing projects were unchanged.

### Project window event fanout (2026-09-30)

The listener lifecycle fix above did not address active global subscriptions. `emit_project` sends one `emit_to` per bound window, and the Tauri JavaScript event API defaults to an `Any` target that receives every such emission. A real-API IPC reproduction with four window targets produced `확확확확정정정정했습니다했습니다했습니다했습니다....`; Map subscriptions received three callbacks instead of one. All panel subscriptions now use `listenToCurrentWindow`, including IPC, main-window direct listeners, Map turn scopes and other Map listeners.

The same regression passes with one archived answer and preserved intentional repetition; Map broadcast and disposal checks pass. The complete panel suite passes 743 tests across 90 files. TypeScript, production panel build, Rust library check and native debug build with `tauri/custom-protocol` pass. Rust's full suite reports 1150 passed, 47 ignored and one unrelated `drains_large_stdout_and_stderr_without_deadlock` timeout during concurrent verification; its exact isolated rerun passes in 0.65s. Existing audio unused-field, React act and bundle-size warnings remain.

Headless Chromium exercised the real `IpcClient`, store and `ConversationLog` with only the Tauri IPC boundary controlled. Four targeted emissions per Korean chunk yielded one rendered answer, `의도된 반복: 하하` remained intact, and stopping the client left zero listeners. Screenshot: `.omo/evidence/window-events/1440x900.png`. The installed app was not replaced and historical panel logs were not rewritten; no new live provider turn was sent.

### Role workers and split panels (2026-10-01)

`worker_start` accepts `research`, `map_research`, and `implementation`; omitted legacy profiles default to implementation. Main interactive EPS guidance delegates broad source/document/map investigation into fresh worker contexts and consumes bounded summaries. Investigation profiles advertise only registered reads, refuse writes before dispatch and write-ticket admission, retain read workspace access across turns, and disable native provider tools. Worker restoration preserves the saved role. Existing split panels display role badges, keep parent focus, arrange parent plus three workers in a balanced 2×2, and reuse only an unfocused terminal worker panel from the same parent.

The complete panel suite passes 759 tests across 92 files; TypeScript and production build pass. Rust library check passes. The earlier complete Rust run reports 1156 passed, 47 ignored, and one unrelated `ordinary_eps_prompt_omits_unrelated_workflow_guides` failure; all new role, permission, persistence, and repeated-turn regressions passed in that run. A later targeted rerun is blocked at test compilation by concurrent prompt changes whose tests still reference removed `WORKSPACE_GUIDE` and `INTERACTION_GUIDE` constants. No full-green Rust claim is made for that later tree. Existing audio unused-field, React act, and bundle-size warnings remain.

Fresh production Chrome evidence covers two panels, four panels, and terminal-panel reuse at 960×900, 1280×900, and 1920×900 (all nine PNGs under `.omo/evidence/role-workers/`). The real App, IPC/store, and layout run with only the Tauri transport controlled. Assertions establish equal-width panes, retained parent focus, preservation of active workers, terminal-only replacement, no document horizontal overflow, and zero page errors. Both independent read-only reviews pass all nine captures and source wiring with no blockers. The capture harness closes the project sidebar and supplies a valid Git-state response; earlier overlay/toast captures were replaced. Native installed-app/provider behavior, model adoption frequency, and measured context savings have not been exercised. `graft build` refreshed the graph.
