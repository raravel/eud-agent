---
task_id: EUD-100-b403
completed_at: 2026-06-08T12:58:00
duration_minutes: 14
coding_retries: 0
verify_retries: 0
review_rounds: 0
verification_required: true
verification_passed: true
blocking_issues: false
status: done
revived_at: 2026-06-08T14:45:00
review_scores:
  correctness: 9
  spec_compliance: 7
  safety: 9
  clarity: 9
tokens:
  estimated: true
  input: 97533
  output: 17212
cost_usd: 2.75
profile: quality
models:
  executor: claude-opus-4-7
  reviewer: none (blocked before review)
---

## Summary
**BLOCKED on a pre-existing toolchain gap — NOT a code defect.** The bootstrap downloader was
implemented and its pure logic verified, but the verify.md smoke (`cargo test
bootstrap::manifest`) cannot LINK in this environment: the moment any code references
`fastembed` (bootstrap's `ensure_model`), the `ort`/`libort_sys` static lib is pulled into the
link, and that prebuilt references MSVC STL ≥14.41 vectorized symbols the locally installed
MSVC 14.40.33807 does not provide.

## Changes (in the preserved worktree, NOT merged)
- `src-tauri/src/bootstrap.rs` — pure logic (`sha256_hex_bytes`, `sha256_file`,
  `asset_status → Present|Missing|Corrupt`, `verify_and_place` = sha256-check THEN atomic
  `rename`, tmp cleaned on mismatch, final never half-placed) + network wrappers
  (`download_to_tmp` via reqwest `Response::chunk`, `ensure_rag_index` GitHub Release stream,
  `ensure_model` fastembed `Bgem3Model::BGEM3Q` HF cache → `models_dir()`, `TauriEmitter`
  `progress {stage:bootstrap,pct}`, `needs_bootstrap`/`bootstrap_assets`).
- `src-tauri/Cargo.toml` — `reqwest 0.12.28` (default-features=false, rustls-tls + stream),
  `sha2 0.10.9`.
- `src-tauri/src/lib.rs` — `+ pub mod bootstrap;`.

## Verification — what passed, what is blocked
- `cargo check -p eud-agent --lib --tests` → 0 errors/0 warnings (the CODE compiles).
- `cargo clippy --workspace --all-targets -- -D warnings` → clean (clippy does not link).
- `cargo fmt --check` → clean.
- The 9 `bootstrap::manifest` tests, run in isolation (logic copied into an ort-free scratch
  crate by the worker) → **9 passed**. The orchestrator independently reproduced the link
  failure (below).
- `cargo test bootstrap::manifest` / `cargo test config` → **FAIL TO LINK** (exit 101):
  `libort_sys-*.rlib(...) : error LNK2019/LNK2001: unresolved external symbol
  __std_find_end_2 / __std_remove_8 / __std_mismatch_1 / __std_find_last_of_trivial_pos_2 ...`
  → `eud_agent_lib.dll : fatal error LNK1120: 13 unresolved externals`.

## Incident

### What broke
- `cargo test bootstrap::manifest` (and now `cargo test config`, and any `cargo build` of the
  crate) cannot link: building the lib's cdylib pulls `fastembed → ort → libort_sys`, whose
  prebuilt binary needs MSVC STL ≥14.41 symbols absent from the local 14.40.33807 toolset.

### Why
- ROOT CAUSE = environment/toolchain skew, not the task code. `ort 2.0.0-rc.12`'s prebuilt
  `libort_sys` (pulled transitively by `fastembed 5.16`) was compiled against MSVC ≥14.41 and
  references `__std_*` vectorized `<algorithm>` symbols that the 14.41 STL import lib provides
  but 14.40 does not. Only MSVC `14.40.33807` is installed on this machine (no 14.41+).
- This was LATENT until now: EUD-098/099 compiled because no code referenced `fastembed`, so
  the linker pruned `ort`. EUD-100 is the FIRST task to actually call `fastembed`, surfacing
  the skew. It blocks ALL fastembed/ort-dependent work (RAG/feature 12) AND the final
  `src-tauri` / `cargo tauri build`.

### What fixed it
- NOT fixed — requires a project-level decision (see Notes). The task code is complete and
  correct; re-running the smoke after the toolchain is resolved should pass with no code change.

### Preserved artifacts (do NOT delete — for diagnosis / re-verify after the env fix)
- worktree: `C:\Users\ifthe\proj\eud\eud-agent\.claude\worktrees\agent-a514590355803861b`
- branch:   `worktree-agent-a514590355803861b` (commits `4fea4da` test-first, `3f95678` impl)

## Notes — resolution options (project-level, user decision)
1. **Update MSVC Build Tools to ≥14.41 (VS 17.11+)** via the VS Installer — cleanest; keeps the
   static-link/single-binary goal. Then ort links and the smoke passes unchanged.
2. **Pin ort/fastembed to a release whose prebuilt was built with ≤14.40** — fragile; may not
   exist for `ort 2.0.0-rc.x`.
3. **ort `load-dynamic`** (dlopen `onnxruntime.dll` at runtime) — avoids the static link, but
   ships a DLL (conflicts with Decision 09's single-static-binary goal; bootstrap would also
   fetch the DLL).
4. **Switch embedding backend to `candle`** (the rejected alternative in Decision 10) — largest
   change.

## Revival (2026-06-08)

Resolution option 1 happened: the local MSVC toolset is now **14.44.35207** (≥ the 14.41 the ort
prebuilt needs). The blocker is gone — confirmed empirically (a fastembed-referencing
`cargo test -p eud-agent --no-run` links cleanly).

Revival steps (orchestrator):
- Cherry-picked the two preserved bootstrap commits (`4fea4da` test, `3f95678` impl, kept on
  branch `eud-100-bootstrap`) onto current `main` via a fresh worktree — auto-merged with no
  conflicts (lib.rs gained `pub mod bootstrap;` alongside the later `pub mod config;`/`pub mod
  mapsafe;`).
- Provisioned the gitignored `panel/dist` into the fresh worktree (required by
  `tauri::generate_context!()`), then re-verified: **`cargo test -p eud-agent` → 26 passed / 0
  failed** (the previously-unlinkable `bootstrap::manifest` smoke now links + passes), clippy
  `-D warnings` clean, fmt clean.
- First review (the task was originally BLOCKED before review): no blocking findings; rubric
  correctness 9 / spec_compliance 7 / safety 9 / clarity 9. Atomic-place + sha256 +
  tmp-cleanup verified correct (no half-install / corrupt-model risk).

Carried-forward advisories (for the bootstrap-WIRING task, not blocking the merge):
- **Model sha256**: `ensure_model` delegates integrity to hf-hub's etag and ignores
  `config.model.sha256` / `config.model.name` (the model is multi-file, so a single hash does
  not map cleanly). Decide at wiring time: hash `model_quantized.onnx` against
  `config.model.sha256`, OR document the hf-hub delegation in feature 10 so spec and code agree.
- **`block_in_place`** in `bootstrap_assets` requires the multi-threaded tokio runtime (Tauri's
  default) — the init wiring must not call it from a current-thread runtime (or switch to
  `spawn_blocking`).

Merged to `main`; task moved to done.
