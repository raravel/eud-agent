# Map Agent Candidate Lifecycle and MCP Operation Schema — Authoritative Implementation Plan

Status: implemented; historical authority. Strict create/open, startup-only orphan cleanup, distinct draft diagnostics, and exhaustive twenty-operation MCP schema are current runtime behavior.

This is the single implementation authority for separating Map Agent candidate-session creation from reopening, preventing normal session hydration from deleting an active request draft, reporting missing drafts accurately, and advertising the complete `map_draft_patch` operation contract through MCP. Do not create a competing plan for this change. If another document conflicts with this plan on candidate draft cleanup or the `map_draft_patch` input schema, this plan wins until implementation is complete. After implementation, current-behavior documents become the runtime authority and this file remains the historical implementation record.

## 1. Goal

Make Map Agent session reload and model-driven map editing reliable without adding a second locking or validation system:

1. split `CandidateStore::open_or_create` into explicit create and open operations;
2. make session open, bootstrap, focus reload, status, render, and source refresh incapable of deleting an active request draft;
3. retain startup recovery of orphan drafts after a process restart;
4. distinguish a missing or unreadable draft from a real zero-byte draft;
5. advertise every accepted `MapOperation` variant, field, nested state, required property, and primitive type in the MCP schema;
6. preserve the existing candidate authority, atomic map-edit, verification, finalize, commit, Apply, and undo semantics;
7. prove the original failure path with deterministic regression tests.

## 2. Incident and Problem Statement

A Map Agent request successfully created a request-owned draft and later intermittently failed with:

```text
candidate map bytes could not be read: 지정된 파일을 찾을 수 없습니다. (os error 2)
```

The same patch succeeded when `map_draft_reset` and `map_draft_patch` ran back-to-back. A later render or analysis could again fail to open the draft. The visible candidate remained at revision 0 because the agent correctly did not call `map_candidate_finalize`.

Two independent product defects were exposed.

### 2.1 Normal session open performs destructive recovery

`CandidateStore::open_or_create` currently performs all of these responsibilities:

- create candidate directories;
- remove every file under `drafts/`;
- create baseline/current candidate state when absent;
- load existing candidate state;
- validate session/source identity;
- repair a mismatched visible candidate by replay;
- refresh stale-source state.

The unconditional call is unsafe:

```rust
cleanup_drafts(&root.join("drafts"))?;
```

`map_agent_bootstrap`, `map_agent_session_create`, and `map_agent_session_load` all reach `open_or_create`. The panel also reloads the selected session when the Map Agent window gains focus. A normal reload can therefore delete the file owned by an active `MapRequestAuthority`, even though `draft_patch`, `draft_analyze`, `finalize`, and `finish_request` otherwise coordinate through `CandidateStoreInner::active`.

### 2.2 MCP advertises only the operation discriminator

The current `map_draft_patch` schema advertises each operation as an object containing only:

```json
{
  "properties": {
    "op": { "type": "string" }
  },
  "required": ["op"]
}
```

`MapOperation` actually has twenty tagged variants with variant-specific fields and nested state. Because those fields are absent from MCP discovery, the model has to infer the contract from correctable runtime errors such as `missing field isomX`, `missing field brush`, and `expected u16`.

## 3. Ground Truth

The implementation MUST build on the following existing behavior.

- `src-tauri/src/lib.rs` constructs one `CandidateStore`, calls `cleanup_startup()`, and only then installs the `MapAgentService` as managed application state.
- `cleanup_startup()` already removes draft files left by a previous process, incomplete candidate directories, old unused revision-0 caches, and root temporary files.
- A process restart clears `CandidateStoreInner::active`; drafts found during startup are therefore orphaned by definition.
- `draft_begin`, `draft_patch`, `draft_reset`, `draft_analyze`, `finalize`, `commit_request`, and `finish_request` already use the exact active `(session_id, request_id)` ownership record.
- `draft_patch` writes native output to a distinct `*.next.scx`, native code verifies MPQ asset preservation and reopens the saved map, and Rust promotes the bytes atomically before returning a hash.
- `map_candidate_finalize` cannot modify the original SCX. Original Apply remains a trusted Map Agent window action.
- `MapOperation` is the runtime deserialization authority and uses `#[serde(tag = "op", rename_all_fields = "camelCase", deny_unknown_fields)]`.
- `map_mcp_tool_descriptors()` copies each registry `input_schema` verbatim into MCP `inputSchema`.
- Existing schema helpers in `src-tauri/src/tools.rs` use JSON Schema objects with `additionalProperties: false`; `oneOf` is already used by `eps_candidates_schema()`.
- No new JSON Schema dependency is required.

## 4. Scope

### 4.1 Included

- Replace `CandidateStore::open_or_create` with explicit candidate-session create and open APIs.
- Migrate every production caller and test; remove `open_or_create` completely.
- Keep orphan cleanup exclusively in the startup recovery path.
- Preserve opening-time identity checks, candidate replay repair, and stale-source refresh.
- Add a regression test proving a candidate session can be reopened while an active draft exists without changing or deleting that draft.
- Move the existing orphan-draft expectation from normal open to explicit `cleanup_startup()`.
- Replace verifier `read(...).unwrap_or_default()` behavior with distinct unreadable and empty-draft diagnostics.
- Define the complete MCP JSON Schema for all `MapOperation` variants and nested state objects.
- Add schema tests against the advertised MCP descriptor.
- Update current-behavior and verification documentation after implementation.

### 4.2 Excluded

- A new candidate-session lock map or a replacement for `CandidateStoreInner::active`.
- Suppressing or delaying focus reload as the primary safety mechanism.
- Additional native map saves, duplicate CHK extraction, or duplicate hash passes after every patch.
- Changes to map authority, target/protect masks, supported writable layers, or Apply permissions.
- Changes to `MapOperation` JSON names, defaults, runtime validation, or native operation behavior.
- Automatic recovery or continuation of an in-flight model request after process restart.
- Persisting active request state across restart.
- A generated-schema framework or a new dependency such as `schemars`.
- General Map Agent prompt tuning beyond making the existing tool contract discoverable.
- UI layout or panel behavior changes.

## 5. Required Invariants

1. A normal candidate-session open MUST NOT enumerate, remove, truncate, replace, or repair files under `drafts/`.
2. Only `draft_begin`, `draft_patch`, `draft_reset`, successful commit/finalize settlement, request finish/cancel, and startup orphan recovery may mutate request draft storage.
3. `cleanup_startup()` MUST run before the managed `MapAgentService` becomes callable.
4. A candidate session with no `state.json` is created explicitly; an existing candidate session is opened explicitly.
5. Create MUST refuse to overwrite an existing candidate state.
6. Open MUST refuse a missing state instead of silently initializing it.
7. Open MUST retain the existing project/source identity validation, baseline/current presence checks, replay repair, and stale-source refresh.
8. Reopening a session during an active request MUST preserve the draft path, bytes, and SHA-256.
9. Finishing or cancelling the owning request MUST still remove its draft.
10. Startup cleanup MUST remove orphan drafts left by the previous process.
11. A missing draft and an empty draft MUST produce different verification errors.
12. MCP MUST advertise exactly the camelCase JSON accepted by `MapOperation` deserialization.
13. Every operation alternative MUST reject unknown fields through `additionalProperties: false`.
14. Schema improvement MUST NOT weaken runtime deserialization or native validation; schema is discovery and early validation, not a replacement authority.
15. The original SCX MUST remain untouched until trusted user Apply.

## 6. CandidateStore API Decision

### D1 — Replace `open_or_create` with strict create/open APIs

Use two public methods with non-overlapping contracts:

```rust
pub fn create_session(
    &self,
    session_id: &str,
    context: &MapContextSnapshot,
) -> Result<CandidateStateView, String>;

pub fn open_session(
    &self,
    session_id: &str,
    context: &MapContextSnapshot,
) -> Result<CandidateStateView, String>;
```

`open_or_create` MUST be removed rather than retained as an alias or compatibility wrapper.

#### `create_session` contract

`create_session` MUST:

1. validate `session_id` and `project_id` path components;
2. derive the session root;
3. fail if `state.json` already exists;
4. create `revisions/` and `drafts/` directories;
5. atomically copy the saved source SCX to `baseline.scx` and `current.scx`;
6. construct revision-0 `CandidateSession` state;
7. atomically persist `state.json`;
8. return `CandidateStateView`.

It MUST NOT call `cleanup_drafts()` and MUST NOT overwrite or reinterpret an existing candidate session.

If initialization fails before `state.json` is committed, the directory is an incomplete cache. `cleanup_startup()` owns its later removal. Do not add best-effort recursive deletion to the normal create error path unless a test demonstrates that it is required for correctness.

#### `open_session` contract

`open_session` MUST:

1. validate the same path components;
2. require an existing `state.json`;
3. deserialize candidate state;
4. require matching session id, project id, and source path;
5. require existing baseline and current map files;
6. compare the current map hash with the current revision authority;
7. replay the current revision when durable current bytes are inconsistent;
8. refresh the stale-source flag against the saved source bytes;
9. persist repaired or stale-state changes using the existing atomic state writer;
10. return `CandidateStateView`.

It MUST NOT create candidate directories and MUST NOT inspect or delete `drafts/`.

### D2 — Make the caller choose intentionally

`MapAgentService` already knows whether a session is bound to candidate state through `CandidateStore::session_source`:

- `Some(source)` matching the current saved map means open the candidate session;
- `None` means the persisted Map session is not yet bound and must create candidate state;
- a different source remains an error and MUST NOT create over the old binding.

Refactor bootstrap flow so that the create/open choice is explicit before `bootstrap_map_session` returns a response. Do not introduce another generically named `ensure`, `load_or_create`, or private `open_or_create` helper that restores the same ambiguous contract.

Expected command behavior:

| Command | Candidate action |
|---|---|
| `map_agent_session_create` | `create_session` |
| `map_agent_session_load` with bound state | `open_session` |
| `map_agent_session_load` for an unbound persisted Map session | `create_session` |
| initial `map_agent_bootstrap` selecting a bound session | `open_session` |
| initial `map_agent_bootstrap` selecting a new/unbound session | `create_session` |
| focus reload of selected bound session | `open_session` |

Corrupt or unreadable existing `state.json` is an explicit error. It MUST NOT be treated as absence and overwritten.

### D3 — Startup cleanup remains the only generic draft sweep

Keep `cleanup_drafts()` private and call it only from `cleanup_startup()`.

This is safe because application startup invokes cleanup before `MapAgentService` is managed and before a model request can populate `active`. No normal IPC path may call the generic sweep.

The existing recovery test that creates `orphan.tmp.scx` and then expects `open_or_create` to remove it MUST be rewritten to call `cleanup_startup()` before `open_session()`.

### D4 — Keep focus reload functional

Do not make panel `busy` state the safety boundary. Focus reload and source checks may continue to hydrate the selected session. Their backend path must be safe because it selects `open_session`, which has no draft cleanup side effect.

This avoids delaying legitimate stale-source detection and ensures safety does not depend on frontend timing.

## 7. Verification Diagnostic Decision

### D5 — Do not convert read failure into empty bytes

Replace:

```rust
let candidate_bytes = std::fs::read(candidate_path).unwrap_or_default();
```

with explicit handling in `MapVerificationService::verify_inner`.

Required diagnostics:

- read failure: `candidate draft could not be read: {error}`;
- successful read with zero bytes: `candidate draft is empty`;
- bytes present but CHK extraction fails: retain `candidate SCX is not parseable: {error}`.

A read failure may not have an authoritative candidate SHA-256. The failure response MUST use an empty diagnostic hash string rather than the SHA-256 of an invented empty byte sequence. A real zero-byte file may use the actual empty-file SHA-256 if the existing response shape requires a hash.

This change is diagnostic only. It MUST NOT add retries, recreate the draft, or mutate visible candidate state.

## 8. MCP Schema Decision

### D6 — Hand-author the schema from the runtime enum

Add focused schema builders in `src-tauri/src/tools.rs`. Do not add `schemars` or generate schema at build time.

Required primitive helpers:

```rust
u8_schema()   // integer, minimum 0, maximum 255
u16_schema()  // integer, minimum 0, maximum 65535
u32_schema()  // integer, minimum 0, maximum 4294967295
i32_schema()  // integer, minimum -2147483648, maximum 2147483647
```

Required composite helpers:

```rust
tile_rows_schema()
unit_state_schema()
unit_patch_schema()
doodad_state_schema()
sprite_state_schema()
location_state_schema()
map_operation_schema()
map_operations_schema()
```

Every nested object MUST use `additionalProperties: false`. The outer operations array retains:

```json
{
  "type": "array",
  "minItems": 1,
  "maxItems": 4096
}
```

`map_draft_patch` continues to wrap this under the required `operations` property.

### D7 — Use a discriminated `oneOf`

`map_operation_schema()` MUST return:

```json
{
  "oneOf": [
    {
      "type": "object",
      "properties": {
        "op": { "const": "terrain.set" },
        "x": { "type": "integer", "minimum": 0, "maximum": 65535 }
      },
      "required": ["op", "x", "y", "before", "after"],
      "additionalProperties": false
    }
  ]
}
```

Each alternative owns its exact properties. Do not use a broad shared object that permits fields from another variant.

### D8 — Mirror serde-required fields and defaults

The schema MUST match the existing Rust contract. Fields with serde defaults remain optional. This change MUST NOT invent stricter semantic requirements that are absent from `MapOperation` and its nested state types.

#### Terrain operations

| `op` | Required fields | Optional fields |
|---|---|---|
| `terrain.set` | `x`, `y`, `before`, `after` | none |
| `terrain.rect` | `x`, `y`, `width`, `height`, `after` | none |
| `terrain.blit` | `x`, `y`, `tiles` | none |
| `terrain.isom_brush` | `isomX`, `isomY`, `brush` | `extent` (default 1) |

`tiles` is a non-empty array of non-empty arrays of `u16`. Rectangular row width and graphics validity remain runtime/native validations.

#### Unit operations

| `op` | Required fields | Optional fields |
|---|---|---|
| `unit.add` | `state` | none |
| `unit.set` | `ordinal`, `beforeFingerprint`, `state` | none |
| `unit.delete` | `ordinal`, `beforeFingerprint` | none |
| `unit.move` | `ordinal`, `beforeFingerprint`, `x`, `y` | none |

`UnitState`:

- required: `typeId: u16`, `owner: u8`, `x: u16`, `y: u16`;
- optional/defaulted: `classId: u32`, `relationFlags: u16`, `validStateFlags: u16`, `validFieldFlags: u16`, `hpPercent: u8`, `shieldPercent: u8`, `energyPercent: u8`, `resourceAmount: u32`, `hangarAmount: u16`, `stateFlags: u16`, `unused: u32`, `relationClassId: u32`.

`UnitPatch` exposes the same fields as optional properties. An empty patch remains representable because the runtime type currently permits it; native/runtime validation remains authoritative for semantic no-ops.

#### Doodad operations

| `op` | Required fields | Optional fields |
|---|---|---|
| `doodad.add` | `state` | none |
| `doodad.set` | `ordinal`, `beforeFingerprint`, `state`, `replacementTiles` | none |
| `doodad.delete` | `ordinal`, `beforeFingerprint`, `replacementTiles` | none |
| `doodad.move` | `ordinal`, `beforeFingerprint`, `x`, `y`, `replacementTiles` | none |

`DoodadState`:

- required: `doodadId: u16`, `x: u16`, `y: u16`;
- optional/defaulted: `owner: u8`, `disabled: boolean`.

`replacementTiles` uses the same tile-row schema as `terrain.blit`.

#### Sprite operations

| `op` | Required fields | Optional fields |
|---|---|---|
| `sprite.add` | `state` | none |
| `sprite.set` | `ordinal`, `beforeFingerprint`, `state` | none |
| `sprite.delete` | `ordinal`, `beforeFingerprint` | none |
| `sprite.move` | `ordinal`, `beforeFingerprint`, `x`, `y` | none |

`SpriteState`:

- required: `spriteId: u16`, `x: u16`, `y: u16`;
- optional/defaulted: `owner: u8`, `flags: u16`.

#### Location operations

| `op` | Required fields | Optional fields |
|---|---|---|
| `location.add` | `state` | none |
| `location.set` | `state` | none |
| `location.rename` | `locationId`, `nameBytesHex` | none |
| `location.delete` | `locationId` | none |

`LocationState`:

- required: `locationId: u16`, `left: i32`, `top: i32`, `right: i32`, `bottom: i32`;
- optional/defaulted: `elevationFlags: u16`, `nameBytesHex: string`.

Do not add a hex regex or non-empty constraint in this change. Empty raw name bytes may be meaningful, and runtime/native code remains the semantic authority.

### D9 — Keep operation names in one auditable schema function

The schema alternatives MUST appear together in `map_operation_schema()` in the same order as `MapOperation`. This deliberate adjacency makes review against the enum mechanical. Do not scatter variants across registry construction or duplicate raw operation schemas in tests.

Tests may maintain an expected operation-name/required-field table because drift detection is the purpose of the test.

## 9. File-by-File Changes

### `src-tauri/src/map_candidate.rs`

- Remove `open_or_create`.
- Add strict `create_session` and `open_session` methods.
- Move current initialization-only code into `create_session`.
- Keep identity validation, current-map replay repair, stale calculation, and view generation in `open_session`.
- Keep `cleanup_drafts` private and startup-only.
- Update every unit test to call create or open intentionally.
- Rewrite the orphan recovery test to invoke `cleanup_startup()`.
- Add the active-draft reopen regression test.

### `src-tauri/src/map_agent.rs`

- Change session resolution/bootstrap data so it explicitly selects candidate create or open.
- `map_agent_session_create` must create candidate state.
- `map_agent_session_load` and focus bootstrap must open bound candidate state.
- Unbound persisted Map sessions must create candidate state once.
- Preserve project/source binding rejection and engine hydration order.
- Do not add a hidden create-or-open wrapper.

### `src-tauri/src/map_verify.rs`

- Replace unreadable-as-empty fallback with explicit read and zero-length branches.
- Add tests for missing and real empty candidate diagnostics.
- Preserve the existing report shape and parse/authority checks.

### `src-tauri/src/tools.rs`

- Add bounded integer, nested state, tile-row, operation, and operations-array schema helpers.
- Replace the generic `{op}` item schema in `map_tool_registry()`.
- Add exhaustive schema tests for operation names, required fields, defaults, numeric bounds, nested object closure, and MCP descriptor passthrough.

### `src-tauri/src/mcp.rs`

- No production change expected.
- Extend tests only if the existing descriptor test in `tools.rs` cannot prove the final advertised `inputSchema` verbatim. Avoid duplicate assertions across both modules.

### Current-behavior docs after implementation

- Update `hivemind/docs/features/sessions.md` to state that Map session hydration never sweeps active drafts and startup recovery owns orphan cleanup.
- Update `hivemind/docs/verify.md` with the focused candidate reopen and map operation schema checks if the existing Map Agent command list cannot name them precisely.
- Do not create another implementation plan.

## 10. Implementation Sequence

1. Add failing candidate lifecycle tests:
   - active draft survives reopen;
   - startup cleanup removes orphan draft;
   - strict open fails when state is absent;
   - strict create fails when state already exists.
2. Split `CandidateStore::open_or_create` and migrate candidate-store tests.
3. Migrate `MapAgentService` and Tauri command paths to explicit create/open selection.
4. Run candidate lifecycle tests before touching MCP schema.
5. Add verifier diagnostic tests and remove unreadable-as-empty behavior.
6. Add failing MCP schema tests derived from the current `MapOperation` contract.
7. Implement schema helpers and replace the generic operation item.
8. Run focused lifecycle, verifier, tools, MCP, and map-model tests.
9. Run the behavioral smoke scenario using the saved source map and actual Map Agent request flow.
10. Update current-behavior and verification docs only after observed behavior matches this plan.

## 11. Test Contract

### 11.1 Candidate lifecycle regression

Add a deterministic test with this observable sequence:

```text
create_session
→ save target selection
→ prepare_request
→ draft_begin
→ record draft path, bytes, and hash
→ open_session for the same session/context
→ require identical draft path, bytes, and hash
→ draft_analyze or a valid draft_patch succeeds
→ finish_request
→ require draft removed
```

This test MUST fail against the current `open_or_create` implementation because reopening sweeps `drafts/`.

### 11.2 Startup orphan recovery

Simulate a new process with a new `CandidateStore` over the same `DataDirs`:

```text
create persisted candidate state
→ write orphan draft file without active in-memory request
→ construct new CandidateStore
→ cleanup_startup
→ open_session
→ require orphan removed and committed revision/object ids preserved
```

Normal `open_session` alone MUST NOT remove the orphan; only the explicit startup phase owns that effect.

### 11.3 Strict API behavior

Tests MUST require:

- `open_session` on absent `state.json` returns a stable missing-session error;
- `create_session` on existing `state.json` refuses overwrite;
- project/source mismatch remains rejected;
- incomplete baseline/current state remains rejected;
- replay recovery and stale detection remain unchanged.

### 11.4 Diagnostic behavior

Tests MUST distinguish:

- missing path → `candidate draft could not be read` with the OS error;
- present zero-byte path → `candidate draft is empty`;
- present non-empty invalid SCX → `candidate SCX is not parseable`.

### 11.5 MCP schema exhaustiveness

The advertised `map_draft_patch.inputSchema.properties.operations.items.oneOf` MUST contain exactly these twenty discriminator constants:

```text
terrain.set
terrain.rect
terrain.blit
terrain.isom_brush
unit.add
unit.set
unit.delete
unit.move
doodad.add
doodad.set
doodad.delete
doodad.move
sprite.add
sprite.set
sprite.delete
sprite.move
location.add
location.set
location.rename
location.delete
```

Tests MUST compare each alternative's required-field set with the table in section 8, require `additionalProperties: false`, and inspect all nested state property types and numeric bounds. They MUST also verify that `map_mcp_tool_descriptors()` advertises the same schema without a generic `parameters` wrapper.

### 11.6 Focused commands

Run from the repository root:

```text
cargo test -p eud-agent map_candidate::tests --lib
cargo test -p eud-agent map_verify::tests --lib
cargo test -p eud-agent tools::tests --lib
cargo test -p eud-agent map_tool_list_excludes_original_apply_and_eps_mutations --lib
cargo test -p eud-agent map_model::tests --lib
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy -p eud-agent --all-targets -- -D warnings
```

Run the broader native suite only if the Rust changes alter the native batch or render call boundary. This plan does not require such a change:

```text
cargo test -p isom --test map_agent_native -- --ignored --nocapture
```

## 12. Behavioral Smoke

The final smoke MUST exercise the actual failure boundary rather than only calling unit helpers:

1. open a saved `OpenMapName` in Map Agent;
2. create a Map session and a terrain-authorized target selection;
3. start a map request and create a draft;
4. trigger the same session hydration path used by focus reload before patching;
5. apply one valid terrain patch;
6. hydrate the same session again;
7. render and analyze the draft;
8. require a valid, non-empty draft hash and no read/parse error;
9. finalize exactly one candidate revision;
10. require the original SCX hash unchanged and the visible candidate revision advanced;
11. finish the request and require its temporary draft removed.

If live Codex execution is not deterministic enough for automation, expose this sequence through existing Rust service/candidate APIs in an integration test and separately smoke the real Tauri session-load IPC while idle. Do not weaken the core regression to a source-text assertion.

## 13. Acceptance Criteria

### Truths

- Normal Map session create and open are separate explicit operations.
- No `open_or_create` symbol or compatibility wrapper remains.
- Reopening or focus-reloading a bound Map session cannot delete an active request draft.
- Startup cleanup still removes orphan drafts from a previous process.
- Candidate request finish/cancel still removes the owned draft.
- Missing, empty, and non-parseable drafts report distinct failures.
- The model can discover every `MapOperation` field and primitive type without probing runtime missing-field errors.
- Runtime map authority and Apply trust boundaries are unchanged.

### Artifacts

- Updated `src-tauri/src/map_candidate.rs` with strict create/open APIs and lifecycle tests.
- Updated `src-tauri/src/map_agent.rs` with explicit caller selection.
- Updated `src-tauri/src/map_verify.rs` with accurate diagnostics and tests.
- Updated `src-tauri/src/tools.rs` with complete operation schemas and exhaustive descriptor tests.
- Updated current-behavior docs after implementation.

### Key links

- Every Map session command selects exactly one candidate create/open API.
- `cleanup_startup()` remains invoked before `MapAgentService` registration.
- MCP descriptor generation consumes the exact registry schema tested in `tools.rs`.
- Schema operation names and camelCase fields remain identical to `MapOperation` serde representation.
- Candidate lifecycle tests cross the same reopen boundary that previously deleted the draft.

## 14. Risks and Controls

### Create/open call-site misclassification

Risk: an unbound persisted Map session is opened and fails, or a bound session is created and overwritten.

Control: use `session_source` presence and source identity already established by `map_session`/`session_for_context`; strict create/open methods fail closed.

### Orphans no longer cleaned

Risk: removing cleanup from normal open leaves stale draft files indefinitely.

Control: startup already calls `cleanup_startup()` before service registration; move the existing recovery test to that exact path.

### Schema drifts from Rust enum

Risk: a later `MapOperation` change updates serde but not MCP discovery.

Control: one exhaustive test compares the exact discriminator set and required-field table; keep schema alternatives adjacent and ordered like the enum.

### Schema becomes stricter than runtime

Risk: MCP rejects a request that serde/native code accepts.

Control: required properties follow serde defaults exactly; semantic constraints such as rectangular tile rows, fingerprint validity, graphics validity, and non-no-op patches remain runtime checks.

### Unnecessary I/O or locking

Risk: defensive changes duplicate native reopen/hash verification or serialize unrelated Map sessions.

Control: add no new lock and no new post-patch native pass. The fix removes a destructive side effect and reuses existing request ownership.

## 15. Completion Boundary

Implementation is complete only when all acceptance truths are observed, focused tests pass, the behavioral reopen path no longer destroys the draft, and current-behavior docs reflect the implemented contract.

Implementation is complete; current-behavior documents and tests now own the runtime contract.