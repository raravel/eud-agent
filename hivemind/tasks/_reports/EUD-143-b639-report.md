---
task_id: EUD-143-b639
completed_at: 2026-06-10T17:40:00
duration_minutes: 10
coding_retries: 0
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
    input: 154648
    output: 2713
    total: 157361
  reviewer_tracked: false
---

## Summary
Migrated the `build-rag-index` CI job from `windows-latest` to `ubuntu-latest` and added an
`actions/cache` step for the fastembed bge-m3 model (~570MB), so the job bills at Linux 1x
(vs Windows 2x) and reuses the model across runs instead of re-downloading it every time.
The PowerShell manifest step was ported to bash. See decision 16.

## Changes
- `.github/workflows/build-rag-index.yml`:
  - `runs-on: windows-latest` -> `ubuntu-latest`.
  - Added a `Cache fastembed model` step (`actions/cache@v4`, path `${{ runner.temp }}/fastembed`,
    key `fastembed-bgem3-${{ runner.os }}`) placed before the build step.
  - Builder invocation now `./ci/target/release/build_rag_index --corpus ci/corpus
    --cache "$RUNNER_TEMP/fastembed" --out rag-index.bin` (no `.exe`, Linux path, `--cache`
    pointed at the cached dir).
  - "Generate release manifest" ported from PowerShell to `shell: bash`: version resolution
    (`DISPATCH_VERSION` -> strip `rag-index-v` from `github.ref_name` -> default `1`), sha256
    read via `awk '{print $1}'`, manifest written with `jq -n` (no BOM), `tag` to `$GITHUB_OUTPUT`.
  - Triggers (`workflow_dispatch` + `rag-index-v*` tag + push on `main`/`ci/corpus/**`),
    `Swatinem/rust-cache` cargo cache, and `softprops/action-gh-release@v2` publish retained.

## Verification
- verify.md stages (lint/type/test/build) target `src-tauri/`+`panel/`; this change touches
  only `.github/workflows/`, so the applicable verification is YAML parse + structural assertions.
- Forbidden-token scan (`windows-latest|.exe|$env:|[System|ConvertTo-Json|Get-Content`): none.
- `yaml.safe_load` OK. Structural assertions PASS: `runs-on == ubuntu-latest`; `on` keys =
  `push`,`workflow_dispatch`; push triggers preserved (`branches: [main]`, `tags: [rag-index-v*]`,
  `paths: [ci/corpus/**]`); `Cache fastembed model` step precedes `Build RAG index`;
  `Publish RAG index release` present.
- Completion criteria: all [PASS] (ubuntu-latest + no win/exe/PS; cache step before build;
  builder `--cache`; bash manifest with tag output; triggers/cache/publish retained; YAML valid).

## Review
Codex review (`codex review --base 7ed06f1`): "No discrete, actionable bugs were found... The
Linux runner conversion, command path updates, cache addition, and bash manifest generation
appear consistent." No `[P1]`/`[P2]`/`[P3]` findings.

## Harness Sync
- harness sync: no-op — `.github/workflows/build-rag-index.yml` is already listed under
  `features/16_rag-corpus-pipeline.md ## Implementation`, and no manifest file changed.
  Contract-drift guard: clean (manifest JSON shape `rag_index.{url,sha256,version}` preserved).

## Notes
- The Codex worker (`-s workspace-write`) edited only the workflow file and did not commit
  (sandbox cannot write the worktree's external `.git` metadata); the orchestrator committed.
- A stray `src-tauri/Cargo.toml` showed as modified in the main repo before merge, but
  `git diff` shows no content change — it is a CRLF normalization artifact (core.autocrlf),
  unrelated to this task and left unstaged (not committed).
- Real CI timing improvement is unverified here (not run); EUD-144 (embedding batch/thread
  tuning) is the complementary follow-up under the same story.
