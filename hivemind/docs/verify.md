# Verification

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
review restore, strip, interrupted controls, verdict card, and the deep-planning switch.

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

## Native project and batch DAT

Rust suite MUST cover:

- manifest/path confinement and canonical open;
- EPS CRUD and MainFile move/delete behavior;
- atomic sparse persistence;
- duplicate/no-op/stale/malformed patch refusal;
- semantic journal reject/rollback and restart persistence;
- exact persisted state for 1, 50, and 200 changes.
- native source-backed `trace_test_run`/`trace_suite_run` without Editor bridge access.

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
8. validate the final generated EDS ordering.

A hand-written EDS smoke test is supplementary only; it does not replace this generator-path acceptance.

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

## Direct eudplib Python

The unconditional static/unit commands at the top MUST cover strict schema-v2 parsing and explicit v1 migration, Python CRUD/revision/snapshot/search/review/rollback, entrypoint move/delete invariants, dependency-token replay/expiry/stale rejection, deterministic lock digests, managed uv/archive/cache integrity, EDS ordering, Windows process-tree cleanup, and E3S refusal.

Real acceptance additionally uses `EUD_AGENT_EUDDRAFT` and `EUD_AGENT_BUILD_MAP` to prove local helper imports and lifecycle hooks, entrypoints before exact EPS MainFile in one frozen process, fresh SCX generation, normalized Python traceback file/line, raw access-violation status, pure/native wheel compatibility where supported, source-repository rejection, online preparation followed by offline verified-cache build, and fail-closed corruption without changing `project.eap`. Python-bearing E3S export must fail explicitly while the existing EPS-only real fixture remains stable.

The panel TypeScript build, complete Vitest suite, and production build are unconditional even when panel files are unchanged.
Blank-map wizard (2026-09-20): the launcher's **빈 맵으로 새 프로젝트** opens the ordered
기본 → 지형 → 플레이어 → 완료 dialog. Without a resolvable StarCraft folder the first step shows the
Korean reason and an in-place **StarCraft 폴더 선택** that must unblock 다음 without reopening the
dialog; a brush-load failure on the terrain step must disable 다음 and offer **StarCraft 폴더 다시
선택**. Verify name validation copy, the `maps/<name>.scx` / `build/[EUD]<name>.scx` hint, size
presets plus free 64..256 input with non-numeric rejection, tileset change re-picking the first
graphics-valid brush, per-slot type/race/force, the 1..4 force count with per-force name, flags
and member summary (shrinking moves orphaned slots to the last force), the three quick presets
(개별 disabled above four players), the start-location preview line (all packed top-left, four per row), busy disabling during creation,
the preview image/summary on 완료, and **프로젝트 열기** leaving the launcher for environment setup.
At 960×640 and 1280×800 the dialog must scroll internally with no horizontal overflow.

Current-source evidence: real-engine `isom` tests create and re-verify all eight tilesets and
reject five invalid specs without writing; `blank_project::tests::creates_a_native_project_around_a_generated_map`
creates a project under a non-ASCII folder with a Korean name and, with `EUDDRAFT_PATH` and
`NATIVE_ASSETS_DIR` set, builds `build/[EUD]새 맵.scx` through the real frozen euddraft. Chromium
with only the Tauri invoke/listen transport mocked walked every wizard step at 1280×800 and
960×640 with no console errors from app code. The wizard has not been exercised in the actual
Tauri binary, and the generated map has not been reopened in SCMDraft 2; those remain open.

