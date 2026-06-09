---
task_id: EUD-118-1729
completed_at: 2026-06-10T02:20:00
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
  coder_session_id: 019ead55-ced0-7860-9d83-f6d6f309b939
  coder_tokens:
    input: 296444
    output: 5397
    total: 301841
  reviewer_tracked: false
---

## Summary
Added `.github/workflows/build-rag-index.yml`: a tag/manual-triggered GitHub Actions workflow that
builds the EUD-108 RAG index builder, runs it against the checked-out ECA corpus, and publishes
`rag-index.bin` + its sha256 sidecar + a `rag_index{url,sha256,version}` manifest as a versioned
GitHub Release asset that the app bootstrap (feature 10) consumes + sha256-verifies.

## Changes
- `.github/workflows/build-rag-index.yml` — triggers `push` tag `rag-index-v*` + `workflow_dispatch`
  (version input); `permissions: contents: write`; `windows-latest`; checkout this repo + the ECA
  corpus repo (`vars.ECA_REPO` + `secrets.ECA_TOKEN`, read-only, `path: eca`); Rust toolchain +
  cargo cache; `cargo build --release --manifest-path ci/Cargo.toml --bin build_rag_index`; run
  `ci/target/release/build_rag_index.exe --eca eca --out rag-index.bin`; generate
  `rag-index.manifest.json` (`{rag_index:{url,sha256,version}}`, UTF-8 no BOM, asset URL derived from
  the same `rag-index-v<version>` tag); publish the Release with all three assets via
  `softprops/action-gh-release@v2`.

## Verification (orchestrator-run; GitHub Actions cannot run here)
- YAML parses cleanly (`yaml.safe_load`). `actionlint` not installed.
- Logic review against criteria: builds + runs the builder; uploads a VERSIONED Release asset +
  sha256 + manifest [criterion 1]; the manifest shape EXACTLY matches feature 10's bootstrap
  `rag_index { url, sha256, version }`, and the manifest's asset URL uses the SAME tag the Release is
  published under, so bootstrap's sha256-verified download resolves [criterion 2]; triggers from a
  tag (`rag-index-v*`) and is documented [criterion 3].
- The `ci/target/release/build_rag_index.exe` path is correct (the `ci` crate has its own
  `[workspace]`, so its artifacts land under `ci/target`).

## Review
codex review (`--base main`): no findings ("consistent with the existing CI builder; publishes the
expected binary, checksum, and manifest assets").

## Harness Sync
- features/12_rust-rag-fastembed.md += `.github/workflows/build-rag-index.yml` (BOUND).

## Notes / user action required
- The workflow's actual execution is GitHub-side and needs two repo settings configured before the
  first tag: `vars.ECA_REPO` (e.g. `owner/ECA`) and `secrets.ECA_TOKEN` (a PAT with read access to
  the private read-only ECA corpus repo). decision 12 specifies CI builds + publishes the index but
  did not pin the CI corpus-access mechanism; the read-only ECA checkout via a configured repo+token
  is the chosen approach — ratify via /hv:plan if a different mechanism is preferred.
- bootstrap.rs (feature 10) already sha256-verifies the downloaded asset; it was not modified
  (out of scope). Model: profile `gpt-5.2-codex` is rejected on this ChatGPT-account codex; used
  `gpt-5.5`.
