# Feature 10: Tauri shell + first-run bootstrap

The standalone Tauri 2 app shell: window, data-dir resolution, first-run download of the
model + RAG index, and the editor-path config. Replaces the POC's editor-hosted WebView2
and server-spawn lifecycle.

> Decision: see [[decisions/08_tauri-rust-rewrite]] and
> [[decisions/12_bootstrap-download-distribution]].

## Data directories
Resolve via Tauri path API:
- `app_data_dir()` -> `%appdata%\eud-agent\` : `config.json`, `memory/`, `map_backups/`,
  `journal/`.
- `app_local_data_dir()` -> `%localappdata%\eud-agent\` : `models/`, `rag/`, `logs/`.
- editor IPC dir: `<editor_path>\Data\agent\` (from `config.json`).
Create missing dirs at startup. Never put the model in Roaming.

## config.json
```json
{
  "editor_path": "C:\\...\\EUDEditor3",
  "codex_cmd": null,
  "model": { "name": "BAAI/bge-m3", "sha256": "<onnx hash>", "version": "1" },
  "rag_index": { "url": "<github release asset>", "sha256": "<hash>", "version": "1" }
}
```
Written UTF-8 (no BOM). `editor_path` is captured on first run via a `tauri-plugin-dialog`
folder picker (validated: `Data\Lua\TriggerEditor` must exist under it).

## First-run flow
```mermaid
flowchart TD
    A[launch] --> B[read/create config.json]
    B --> C{editor_path set & valid?}
    C -- no --> P[picker: choose EUDEditor3 folder] --> B
    C -- yes --> D{model + rag_index present & sha256 OK?}
    D -- no --> E[setup screen: download with progress]
    E --> F[bge-m3 ONNX via fastembed HF cache to models/]
    E --> G[RAG index from GitHub Release to rag/ tmp]
    F & G --> H[sha256 verify]
    H -- ok --> I[atomic rename into place] --> D
    H -- fail --> E2[show error + retry]
    D -- yes --> J[init core, lazy RAG warmup] --> K[show panel]
```

## Bootstrap rules
- Every asset sha256-verified against `config.json`/a bundled manifest before use.
- Atomic placement: download to `*.tmp`, verify, then `os::rename` over the final path.
- Missing/corrupt -> re-download; NEVER leave a half-written asset in place.
- Download progress emitted to the panel as `progress {stage: bootstrap, detail, pct}`.
- The model is fetched through fastembed's HF cache (cache dir = `models/`); the RAG index
  is a direct `reqwest` GET of the Release asset.

## Edge cases
- Offline on first run: setup screen shows a clear "network required for first-run
  install" message; retains partial-but-verified assets, resumes on next launch.
- WebView2 runtime missing: detect and link the user to the Evergreen installer.
- Disk full mid-download: surfaced as a bootstrap error; tmp file cleaned.

## Implementation
- `src-tauri/src/config.rs` — config.json load/save, editor-path validation
- `src-tauri/src/bootstrap.rs` — manifest check, downloads, sha256, atomic place, progress
- `src-tauri/src/main.rs` — Tauri builder, window, dir resolution, init ordering
- `src-tauri/tauri.conf.json` — bundle, capabilities (shell/dialog/fs), window config
- `panel/src/setup/` — first-run setup + download-progress UI
- external: `tauri-plugin-dialog`, `reqwest`, `sha2`, `fastembed` (HF cache dir)
- [BOUND 2026-06-08 from EUD-098-fe34] `src-tauri/src/lib.rs` — Tauri 2 builder + shell/dialog plugin registration; app entry (`run()`), reused by main.rs shim
- [BOUND 2026-06-08 from EUD-098-fe34] `src-tauri/build.rs` — runs `tauri_build::build()` (codegen + config validation at compile time)
- [BOUND 2026-06-08 from EUD-098-fe34] `src-tauri/capabilities/default.json` — main-window capability granting core/shell/dialog plugin permissions
- [BOUND 2026-06-10 from EUD-120-ecca] `panel/src/setup/bootstrap.ts` — pure bootstrapView(pct, detail) mapping the {stage,pct,detail} bootstrap progress payload to setup-screen view state (phase/label/pct)
- [BOUND 2026-06-10 from EUD-120-ecca] `panel/src/setup/SetupScreen.tsx` — first-run setup overlay (determinate/indeterminate progressbar + error mode with reload retry); rendered by App while bootstrap active
- [BOUND 2026-06-11 from EUD-132-0829] `src-tauri/src/setup.rs` — manifest check (`setup_status`: editor-path + assets), `setup_pick_editor_path` (native picker -> validate -> save_config), `bootstrap_run` (panel-driven download/retry; resolves the RAG spec from the published `rag-index.manifest.json`), `should_auto_bootstrap`/`run_bootstrap` (auto re-download on later launches; readiness never gated)
- [BOUND 2026-06-11 from EUD-132-0829] `panel/src/setup/SetupScreen.tsx` — gained the editor-folder pick step (shown while editor path missing/invalid; maps the `invalid_editor_folder` code to Korean text); retry now re-invokes `bootstrap_run` instead of reloading
- [BOUND 2026-06-11 from EUD-132-0829] `src-tauri/src/ipc.rs` — empty `config.json` editor path fails commands with "editor path not configured" (setup signal), distinct from the stale-heartbeat "editor not connected"
