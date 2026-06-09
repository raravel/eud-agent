---
task_id: EUD-117-015a
completed_at: 2026-06-10T02:00:00
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
  coder_session_id: 019ead39-2738-72e3-becf-a147c54f97ad
  coder_tokens:
    input: 93170
    output: 900
    total: 94070
  reviewer_tracked: false
---

## Summary
Completed the Tauri bundle config (`src-tauri/tauri.conf.json`): added
`bundle.windows.webviewInstallMode = downloadBootstrapper` so a missing WebView2 runtime is
resolved via the Evergreen bootstrapper at install (the "guide the user to the installer" path,
matching rules.md "WebView2 uses the system Evergreen runtime"). Window, identifier, icons,
`build.frontendDist` (panel) left intact. No `bundle.resources` is needed — the `[first principles]`
prompt is compile-time embedded (`include_str!`) and the panel is bundled via `frontendDist`.

## Changes
- `src-tauri/tauri.conf.json` — `bundle.windows.webviewInstallMode: { type: "downloadBootstrapper" }`.

## Verification (orchestrator-run)
- `tauri.conf.json` parses as valid JSON (PowerShell `ConvertFrom-Json`).
- Panel builds clean: `npm install` (main panel node_modules was stale, missing `@tauri-apps/api`)
  then `npm run build` → `panel/dist` (tsc + vite, 1m02s). [criterion 2 build]
- No-CDN: the only `cdn.jsdelivr...monaco-editor` string is the INERT `@monaco-editor/react` default
  that is OVERRIDDEN — `panel/src/editor/monaco.ts` imports the local `monaco-editor` npm bundle,
  bundles the editor/json/css/html workers locally, and calls `loader.config({ monaco })` (rules.md
  mandate). The `[first principles]` prompt is `include_str!`-embedded. So no network/CDN at load.
  [criterion 2]
- Runnable bundled exe: installed `tauri-cli 2.11.2`, copied the built `panel/dist` into the
  worktree, ran `cargo tauri build --no-bundle` → "Built application at:
  …\release\eud-agent.exe" (release, 2m01s, frontendDist embedded). [criterion 1]
- `webviewInstallMode = downloadBootstrapper` provides the WebView2-missing → installer path.
  [criterion 3]

## Review
codex review (`--base main`): no findings ("valid Tauri Windows bundle WebView2 install mode
configuration; does not break existing behavior").

## Harness Sync
- No-op: `src-tauri/tauri.conf.json` is already listed in features/10_tauri-shell-bootstrap.md
  `## Implementation`. No manifest change.

## Notes
- Verified the runnable exe with `--no-bundle` (skips the MSI/NSIS INSTALLER bundling, which needs
  the WiX/NSIS toolchain not present on this dev box). Producing the signed installers is the
  release-machine / CI packaging step (EUD-118 "CI: build … publish Release asset"). The config and
  the runnable exe are verified here.
- Model: profile `gpt-5.2-codex` is rejected on this ChatGPT-account codex; used `gpt-5.5`.
- The task mentioned "capabilities (shell/dialog/fs)": shell+dialog already live in
  `src-tauri/capabilities/default.json` (out of this task's scope); the app uses no fs plugin, so no
  fs capability was added.
