---
task_id: EUD-121-8ff9
completed_at: 2026-06-10T13:40:00Z
duration_minutes: 20
coding_retries: 0
verify_retries: 0
review_rounds: 0
verification_required: false
verification_passed: true
blocking_issues: false
providers:
  coder: claude
  reviewer: claude
review_scores: {}
tokens:
  estimated: true
  input: 0
  output: 0
cost_usd: 0.00
profile: mixed
models:
  executor: orchestrator-direct
  reviewer: orchestrator-direct
---

## Summary
Chore: removed the retired v1 Python `server/` stack now that the Rust core has parity, and did
the "full cleanup" of the Python-era deployment scripts (user-confirmed approach). The v2 app is a
single Tauri 2 binary, so the standalone Python server, its uv venv, and the v1 drop-in
install/release tooling are all obsolete (tech-stack.md ## Removed / Superseded; architecture.md
"v2 has no Python server"). Done directly by the orchestrator (a deletion chore with a few small,
clearly-directed script rewrites — no codex worker).

## Changes
Removed:
- `server/` — the entire Python package (`eud_agent`, tests, spikes, `pyproject.toml`, `uv.lock`)
  plus the gitignored `.venv`/caches. 0 tracked `server/` files remain; the physical dir was also
  deleted (after reaping 5 lingering `python.exe` processes holding `.venv` locks).
- `scripts/setup_env.ps1`, `scripts/install_dropin.ps1`, `scripts/uninstall_dropin.ps1`,
  `scripts/package_release.ps1`, `scripts/README.release.md`, `scripts/install.bat`,
  `scripts/uninstall.bat` — the v1 Python-venv + drop-in install/release flow, superseded by the
  Tauri bundle (EUD-117/118).
Rewritten:
- `scripts/check_prereqs.ps1` — dropped the `uv` and `venv-python` checks; keeps only the `codex`
  CLI prerequisite (the Rust core still spawns codex; rules.md).
- `scripts/dev_run.ps1` — now runs `cargo tauri dev` (Rust core + panel hot-reload) instead of
  `python -m eud_agent`; checks codex + cargo prereqs first.
- `.gitignore` — removed the `server/.venv/` rule and the stale `package_release.ps1` comment.
Kept:
- `scripts/install_bridge.ps1` — installs the slim Lua bridge; still required in v2 (no Python refs).

## Verification (verify.md: cargo + panel)
- `cd panel && npx tsc -b --noEmit` → clean; `npx vitest run` → 237 passed.
- `cargo build --manifest-path src-tauri/Cargo.toml` → Finished (links the isom static lib). `server/`
  is not a cargo workspace member, so the Rust build is structurally independent of its removal;
  the build confirms nothing dangled.
- `git grep` for `server\.venv` / `python -m eud_agent` / `uv sync` / the deleted script names in
  tracked files (excl. hivemind history) → no live references remain. (`eud_agent_lib` in
  src-tauri/Cargo.toml + ci/README.md is the RUST library crate name, not the Python package;
  StormLib's `CPACK_RPM_PACKAGE_RELEASE` is vendored third-party, unrelated.)
- `verification_required: false` (type chore) — no failing-artifact gate.

## Notes
- Removed deps not auto-pruned from tech-stack.md (per harness-sync policy — the user prunes on
  confirmation): the v1 `server/pyproject.toml` stack (fastapi, uvicorn, chromadb, sentence-
  transformers, transformers, torch, numpy, openai-codex, mcp, uv) — already listed under
  tech-stack.md ## Removed / Superseded as "deleted in v2", now actually deleted.
- Harness sync: no-op — no new source files; no manifest dependency ADDED; the removal aligns with
  the already-documented `## Removed / Superseded` intent (no contract drift).
- Docs: architecture.md's mention of the POC Python FastAPI server is intentional historical
  context ("the POC was…/v2 supersedes"), kept as-is; tech-stack.md already documents the removal.
- The v2 install/release flow (MSI/NSIS) lives in the Tauri bundle + CI (EUD-117/118), which is why
  the v1 `install_dropin`/`package_release` scripts were deleted rather than repurposed.
