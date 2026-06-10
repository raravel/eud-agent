---
task_id: EUD-140-c28a
completed_at: 2026-06-10T15:18:12
duration_minutes: 18
coding_retries: 1
verify_retries: 0
review_rounds: 0
verification_required: true
verification_passed: true
blocking_issues: false
providers:
  coder: codex
  reviewer: codex
review_scores: {}
tokens:
  estimated: true
  input: 0
  output: 0
cost_usd: 0.00
profile: mixed
models:
  executor: gpt-5.5
  reviewer: gpt-5.5
codex_usage:
  coder_session_id: null
  coder_tokens:
    input: 323982
    output: 5151
    total: 329133
  reviewer_tracked: false
---

## Summary
Dropped the ECA dependency from `.github/workflows/build-rag-index.yml`. The CI job now
embeds the in-repo corpus (`ci/corpus`) via the `build_rag_index --corpus ci/corpus`
interface added in EUD-136, instead of checking out a separate private ECA repo.

## Changes
- `.github/workflows/build-rag-index.yml`:
  - Removed the "Checkout ECA corpus" step and its explanatory comment (all
    `vars.ECA_REPO` / `secrets.ECA_TOKEN` usage gone).
  - Changed the "Build RAG index" run command from `--eca eca --out rag-index.bin` to
    `--corpus ci/corpus --out rag-index.bin` (the builder's `--eca` flag no longer exists;
    confirmed against `ci/build_rag_index.rs`).
  - Added a `push` trigger scoped to `branches: [main]` + `paths: [ci/corpus/**]` so a
    corpus commit on main rebuilds the index. Retained the `rag-index-v*` tag trigger and
    `workflow_dispatch` (with the `version` input).
  - Manifest generation and the `softprops/action-gh-release@v2` publish step are unchanged.

## Verification
- Completion criteria (orchestrator-checked against the worktree):
  - [PASS] "Checkout ECA corpus" step + every `vars.ECA_REPO`/`secrets.ECA_TOKEN`
    reference removed — `Select-String "ECA|eca|--eca"` returns nothing.
  - [PASS] Build step invokes `build_rag_index.exe --corpus ci/corpus --out rag-index.bin`.
  - [PASS] `workflow_dispatch` + `rag-index-v*` tag retained; `push` on `ci/corpus/**` added.
  - [PASS] Manifest + Release publish steps unchanged; YAML parses (`yaml.safe_load` → OK,
    `on` keys = `push`, `workflow_dispatch`).
- verify.md stages (lint/type/test/build) target `src-tauri/`+`panel/`; this change touches
  only `.github/workflows/`, which no stage covers. Applicable verification is YAML parse
  (PASS) + criteria (all PASS). Rust/panel stages were not run — they do not consume the file.
- GitHub Actions semantics verified: `paths` filters are NOT applied to tag pushes
  (documented + community-confirmed), so the `rag-index-v*` tag trigger still fires
  regardless of the `paths: [ci/corpus/**]` filter sharing the same `push:` block.

## Review
Codex review (`codex review --base df93274`): "the ECA checkout and secret usage are
removed, and the builder is invoked with the supported --corpus path. I did not identify a
blocking regression in the modified lines." No `[P1]`/`[P2]` findings. (A `[P1]` string
appears in the worktree's `EUD-136-ea37-report.md` that codex read during review — it is the
prior task's note that EUD-140 must update the workflow, not a finding against this diff.)

## Harness Sync
- harness sync: no-op — `.github/workflows/build-rag-index.yml` is already listed under
  `features/16_rag-corpus-pipeline.md ## Implementation`, and no manifest file changed.
  Contract-drift guard: clean (the change adopts the spec-promised `--corpus` interface).

## Notes
- Marking EUD-140 done auto-completed its parent story EUD-135 and epic EUD-134 (all
  children done).
- The Codex coding session ran with `-s workspace-write`; it could not create the commit
  because the worktree's `.git` metadata lives in the main repo
  (`.git/worktrees/...`), outside the sandbox-writable root (`index.lock` → permission
  denied). The orchestrator created the commit instead.

## Incident

### What broke
- The first coding pass added `branches: ["**"]` to the new `push` trigger. Combined with
  the Release-publishing job, that would build and publish/clobber the `rag-index-v1`
  GitHub Release on a corpus push to ANY branch (including feature branches), not just the
  default branch.

### Why
- The task said "optionally add a push trigger on `ci/corpus/**`" without naming a branch;
  the worker defaulted to all-branches, missing the footgun that the job's final step
  publishes a Release.

### What fixed it
- On retry 1 the branch filter was narrowed to `branches: [main]`, keeping the
  `tags: [rag-index-v*]` and `paths: [ci/corpus/**]` entries. The tag trigger remains
  unaffected because GitHub does not apply `paths` filters to tag pushes.
