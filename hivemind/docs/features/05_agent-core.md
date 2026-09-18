# Agent core

## Runtime

The agent core is in-process Rust. Tauri commands feed `SessionEngineManager`; a session-bound `AgentEngine<R: RuntimeExecutor>` uses `ProviderRuntime`. The fixed production factory selects exactly five transport adapters: Codex, Claude Code, Antigravity, OpenCode Go, and Ollama.

Turn flow:

```text
chat/feedback
  -> project + memory + RAG context
  -> immutable session binding + run identity + execution policy
  -> ProviderRuntime -> production ProviderAdapter
  -> run-scoped tool gate -> SessionToolRuntime
  -> read/evidence -> automatic mutation admission -> semantic journal
  -> validated checkpoint and explicit answer/write-transition/review outcome
```

The runtime owns the direct HTTP model-step/tool-result loop, ordered text/reasoning/tool/signature blocks, deadlines, cancellation, model-event validation, foreground stream routing, workspace preparation, and application checkpoint publication. Adapters own authentication/transport encoding, decoding, capability constraints, and native process/session controls. They receive no application store writer, UI sink, workspace manager, or unrestricted tool runtime. Codex app-server and Claude Code keep their official authentication and native internal loops.

A run fixes session/request/job identity, target kind, cancellation generation, binding, and policy before execution. Direct tool batches and native MCP requests enter the same run-scoped gate; each native MCP endpoint and call identity is unique to its run. Native notifications observe tool execution; they never dispatch it again. Cancellation first closes admission, then stops transport and settles already-started operations. Completed tool results and journaled changes persist independently of answer success, without automatic replay or rollback. ASK waiting pauses the provider active deadline and ends once on cancellation/transport exit.

Ordered text, reasoning, tool batches/results, signatures and encrypted/native continuation data retain their protocol meaning. Partial output followed by an error, incomplete response boundary, malformed tool-call framing (invalid id/name/non-object arguments, duplicate ids, unknown tools), or truncated structured result fails explicitly. Model arguments that violate an advertised schema are not framing failures: they complete as usage errors the model can correct. No model-name exceptions, reasoning-as-answer substitution, prose JSON extraction, or cross-provider/model fallback repairs such a failure.

`RunPolicy.max_output_bytes` measures serialized normalized blocks and final semantic output in the common runtime. It does not shrink native/raw transport limits: OpenCode Go, Ollama, and Antigravity each retain a separate 16 MiB HTTP response ceiling; Claude Code retains a separate 32 MiB stdout ceiling and 1 MiB JSONL-line ceiling; Codex retains its existing common checks. Therefore a valid large wire envelope with small normalized content may continue, while normalized content over policy fails with the common byte-limit error.

Direct conversations retain validated generation/pointer storage. A committed head can repair lagging matching metadata; missing, corrupt, mismatched, or ahead-of-head state fails closed. Native continuation candidates are adopted only at confirmed boundaries, and durable execution receipts retire only after application metadata persistence is acknowledged. Late events must still match their captured identity. Unknown native state blocks resume while preserving review; explicit reset discards that unconfirmed continuation and starts a fresh one.

Typed model usage belongs to the `context_usage` event and persisted session usage, rather than an unconsumed generic event. A Codex native usage event is publishable only after the official nonempty `turn/started` ID is recorded for this run and its `turnId` matches that active ID; prior and stale turn usage is discarded. The production `SessionRuntimeEventSink` remains strict. The gate is the authority for tool completion publication and fatal native admission errors.

OpenCode Go sends ordinary optional-field tool descriptors with explicit `strict: false` on its Responses and Chat Completions wrappers, while the structured result descriptor remains `strict: true`. If stream parsing classifies a failure as protocol or incomplete, the adapter returns that classification instead of inserting a payloadless `TransportClosed` event that could mask it. Checkpoint37's full suite passes this path and the permanent Map descriptor regressions. Actual Responses/`gpt-5.6-luna` and Chat/`kimi-k3` retain their completed tools/resume/structured outcomes; Anthropic Messages/`qwen3.8-flash` still fails structured work with `provider_transport_closed`. The same-shape HTTP 500/no-SSE diagnostic remains non-retroactive. See [verification status](../verify.md).

Checkpoint 23/24 isolates the raw-overhead defect from normalized-output policy: all three OpenCode wires and both direct HTTP adapters pass their post-RED production-path fixtures. The initial Claude size observations were not attributed because a PowerShell common parameter consumed `-p`; corrected numeric witnesses produced four qualified Claude REDs before the Claude product fix. All four corrected Claude selectors pass on checkpoint 25's immutable binary. These are deterministic CLI/fixture results, not a live Claude service verdict.

Actual app33 replaces the earlier EPS write failure with scoped native evidence: a Codex read persists its matching semantic delta; one isolated edit passes `eps_check` and `build_run`, then exercises both rollback and accepted-review paths; and cancellation occurs after `response_started`, before text/tool output, with no pending request or late work during the 8-second observation. The historical SQLite startup failure remains unexplained: probe non-reproduction does not authorize reset, retry, or a global provider lock.

Actual app37 passes a native ASK answer after its bounded wait, a separate ASK cancellation with no later event in the 8.08-second observation, and normal restart persistence without active-request replay. The generic Map admission path validates the verbatim registry Draft7 schema, preserving strict rejection for malformed values and original Apply. Actual39 then found a separate 271-character Windows temporary-draft path failure after numeric rendering and draft begin; source41 corrects the shared path boundary and its permanent long-path Map selector passes. Actual Map41 completes a candidate, trusted UI Apply, and exact undo to baseline, while stale-fork validation remains not run. This is a generic native-path correction, not a model-specific workaround.

Actual39 also verifies read-only C18 isolation: a Map status/analyze request overlaps a separately bound OpenCode Go/`glm-5.3` `project_status` request, with session-bound results and usage. The foreground read completes, while the persisted compiler warning is `runtime_error`; neither its underlying cause nor its exact OpenCode wire is inferred.

Independent QA41 observes the post-Undo Map surface at r0 with no candidate and disabled Apply/Undo while retaining the completed answer and tool history. The app closes normally without remaining owned processes. Ollama service verification was rejected before `ollama serve` launched; no catalog, generation, or model compatibility conclusion follows.

Historical checkpoints28/33 retain an OpenCode Go/`glm-5.3` foreground read while its separate task-state compiler fails at the local 16 KiB serialized-normalized-output policy and keeps the foreground result. The provider/model binding is exact, but the wire remains unverified; no Chat Completions compatibility conclusion follows from its call-ID convention.

## Structured jobs

Task-state compilation and harness generation use a fresh `StructuredJobExecutor`, with their own immutable `StructuredJobRequest`. It owns only an adapter and cancellation receiver: empty main history/continuation, no EUD tools or MCP endpoint, and no main store/workspace/UI authority. The runtime supplies an isolated cwd where a native CLI requires one.

Each wire returns exactly one whole schema result; a required result-submission function is an encoding channel, never an EUD tool. Rust schema validation precedes the existing domain validator. Compiler results must match both returned and current revision/branch/provenance before application. Harness retry/restart uses its persisted provider/model/reasoning/base URL and stages only a validated delta through the existing review path.

The compiler retains its 60-second deadline and 8192-token requested cap, bounded by a known model limit where the wire supports a cap. Harness retains its 300-second deadline and separate output contract. Unsupported CLI token controls are not invented. Job failure may produce an explicit task-state warning or harness failure; model content and usage never enter the foreground stream.

## Native project context

Every turn receives current native project identity, MainFile, source snapshot/revision, settings/plugins, map context, and relevant memory. `project_status` and `list_files` read `NativeProjectManager`; no raw status wire reply exists.

Project state changes invalidate stale source/mention/workspace authority. A missing project is normal gated state, not an IPC disconnect.

## Tool model

- Read-workspace tools: project/source/DAT/map/memory/workspace diagnostics and `build_run`.
- Canonical mutation tools: native source/settings/plugins/map operations and `dat_patch`.
- `build_run` requires the project transaction but not write-workspace admission; generated EDS,
  Python, output maps, and other build artifacts never enter the semantic changeset.
- `dat_patch` is the only model-facing DAT mutation boundary.
- `ask` waits at most `ASK_WAIT_TIMEOUT` (240 s) for `ask_response`. Native Codex/Claude CLIs abort
  a silent MCP call after 300 s and progress notifications do not extend it, so the wait is bounded
  below that. An unanswered ask completes as `{status: "unanswered", questionIds, waitedSeconds}`,
  the panel receives an `expired` ask event for the same request id, a late `ask_response` fails
  with `ask_expired`, and the same foreground run cannot ask again; the model restates the
  questions as its final text and the user's next message is the answer. A restored pending ask
  reports its remaining wait. An autonomous turn that ends this way pauses as `unanswered_ask`
  instead of completing, and explicit resume carries that blocker so the model asks again.
- Tool schemas are validated before execution, including `const` tags and exactly-one `oneOf` alternatives. A schema-violating call is completed with a field-level usage error the model can self-correct within the run's tool-round budget; duplicate ids, unknown tools, malformed call shape, and stale-run dispatch stay fatal. For `map_draft_patch`, a rejected `operations[i]` names the documented keys of the attempted `op` (or the documented op list), plus its missing and unexpected keys; the Map system prompt's `[draft patch operations]` section is generated from the same advertised schema.
- The first canonical mutation in a read run automatically registers write intent and parks without
  executing; the resumed write run re-reads the target and must satisfy evidence and
  first-principles rails before execution.
- Writes are journaled with semantic before/after snapshots.

## Source architecture

The manifest MainFile is the composition root regardless of filename. The active provider should:

- read MainFile and relevant imports before placement;
- keep lifecycle hooks in the composition root;
- place cohesive subsystem state/logic in focused modules;
- avoid circular imports and needless files;
- batch mutually dependent source edits;
- build after accepted runtime-affecting changes.

## Sessions and review

- Session records persist conversation log, typed provider conversation, model/reasoning selection, pending requests, project, and kind.
- Existing sessions never switch provider; settings defaults apply only to new sessions.
- Read work may overlap; project writes serialize.
- Plan review and changeset review are explicit turn outcomes.
- Reject applies inverse operations in reverse sequence; accept archives durable state.
- Restart/reopen restores accepted canonical files and review metadata.

Opt-in autonomous execution is session-owned and distinct from ordinary interactive chat. The
common engine—not any adapter—continues typed action, direct-round, context-pressure, and provider
boundaries while retaining the same request ID and journal. Each boundary atomically persists the
confirmed provider checkpoint and bounded progress state. ASK, semantic review, user pause,
safety-stop, cancellation, failure, completion, and restart pause remain distinct lifecycle
states. Restart never replays active work; explicit resume validates project identity/revision,
pending review, and the exact provider checkpoint first.

Soft action/round thresholds are checkpoints, not stop triggers. An ordinary interactive turn that
reaches one persists the session conversation and continues the same request in place; it never
ends with a "paused at boundary" answer. Safety stop applies only to explicit run budgets (wall
time, provider-reported tokens) and to failed resume validation; the default policy sets no budget,
and repeated identical progress fingerprints are reported but never stop a run.

## Build and diagnostics

`build_run` invokes the native generator and euddraft runner as a serialized project transaction.
It returns `{ok, errors, stdout, stderr, outputMap}` with file/line diagnostics. Analyzer diagnostics
are advisory; a fresh euddraft output is final success authority. Stable normalized
`(project revision, diagnostics)` fingerprints stop unchanged failures and short oscillation cycles;
successful, changed-revision, and improving builds have no lifetime count. An autonomous
runtime-affecting request cannot complete until the latest canonical revision has a successful
required build.

Tool progress is identity-based rather than lifetime-counted. Documentation search requires a
materially changed normalized query/filter or new stable document IDs. Python dependency
preparation fingerprints the complete normalized desired set and reuses a valid matching candidate
without repeating environment probes, resolution, or downloads; expiry, ownership, single-use,
hash, cache-integrity, timeout, and bounded-download checks remain mandatory.

After a successful build, `trace_test_run` and `trace_suite_run` use the native source snapshot and
generated EDS to run isolated runtime diagnostics. They never query an Editor project or bridge.

## Removed runtime

The following are not compatibility paths and must not return:

- Python server/orchestrator;
- WebSocket/localhost panel transport;
- BridgeIo or Lua command dispatcher;
- Editor status/list/get/set/build;
- Editor launch/bootstrap/heartbeat;
- individual DAT setter tools.
