# Decision 17: Self-update via tauri-plugin-updater, local manual release

- Date: 2026-06-11
- Status: Implemented
- Context: [[decisions/04_dist-release-distribution]] chose "GitHub Releases packaging +
  updater" but deferred the implementation. This decision records how that updater phase
  was built. The repo is a public GitHub repo (`raravel/eud-agent`); the user opted out of
  an Authenticode code-signing certificate for now (SmartScreen warning accepted) and asked
  for full self-replacement updates with a local manual release pipeline first (GitHub
  Actions CI is a later phase).
- Considered:
  - **No updater, ship a bare binary** — Pros: simplest. Cons: users never learn a new
    version exists and must re-download manually. Rejected: Decision 04 already committed to
    an updater, and the single-binary design makes self-update cheap (the model/RAG assets
    live in `%localappdata%` and are not re-downloaded on app update).
  - **In-app "new version" notice only (no self-replace)** — Pros: no signing, no manifest.
    Cons: still a manual install. Rejected for the same reason.
  - **tauri-plugin-updater self-replace (chosen)** — Pros: real auto-update; the updater
    downloads + verifies (minisign) + installs the signed NSIS bundle and relaunches. Cons:
    needs a signing key + a `latest.json` manifest + an NSIS bundle target.
- Chosen: `tauri-plugin-updater` + `tauri-plugin-process` (relaunch). Bundle target narrowed
  to `["nsis"]` with `createUpdaterArtifacts: true`; the updater endpoint is the static
  `releases/latest/download/latest.json`. Updates are minisign-signed (free; separate from
  the absent Authenticode signing). A local PowerShell script (`scripts/release.ps1`) builds,
  signs, **synthesizes `latest.json` from the `.sig`** (a local `tauri build` does not emit
  it — only tauri-action does), and publishes to GitHub Releases via `gh`.
- bridge re-install: the editor's `Data\Lua\TriggerEditor\ZZZ_10_agent_bridge.lua` is a copy
  bundled into the app as a Tauri resource. `bridge_install::sync_bridge` (the Rust port of
  `scripts/install_bridge.ps1`) runs on every app start and overwrites the editor's copy when
  the bytes differ, so a self-update that ships a newer bridge re-installs it on the next
  launch — no manual `install_bridge.ps1` re-run. Bytes are copied verbatim (KopiLua reads
  the `.lua` as Latin1; never re-encode). Best-effort: a downed/moved editor never blocks
  startup.
- UX: a non-blocking banner (`UpdateNotice`) shows the available version + notes after
  first-run setup is satisfied; the user consents ([지금 업데이트]) or defers ([나중에]);
  on consent it streams download progress and relaunches. The check runs once per session
  and never gates the panel (offline / no release → no banner).
- Rationale: user decisions (public repo, no Authenticode, full self-replace, local release
  first). Self-update stays decoupled from `bootstrap`: the updater replaces only the app
  binary; `%localappdata%` assets (model/RAG) and `%appdata%` config are preserved and not
  re-downloaded. RAG/model asset *versioning* remains the existing bootstrap manifest's job.
- Impact: `src-tauri/Cargo.toml` (updater + process plugins), `tauri.conf.json` (nsis target,
  createUpdaterArtifacts, resources, `plugins.updater`), `capabilities/default.json`
  (`updater:default` + `process:default`), new `src-tauri/src/bridge_install.rs` + lib.rs
  wiring, panel `setup/update.ts` + `components/UpdateNotice.tsx` + App.tsx, new
  `scripts/release.ps1`, rules.md updater/signing rules. The minisign **public** key is
  committed in `tauri.conf.json`; the **private** key is NEVER committed (kept under
  `%USERPROFILE%\.tauri\`, injected via `TAURI_SIGNING_PRIVATE_KEY`).
