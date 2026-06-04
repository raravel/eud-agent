# eud-agent Tech Stack

> Grounding (2026-06-04, refreshed by the React-panel re-plan): Python deps are grounded in `server/pyproject.toml` + `server/uv.lock` (349 resolved packages) and the working venv. Frontend deps below are TO BE PINNED by the panel scaffold task in `panel/package.json` — re-run Phase 0 grounding against that manifest once it exists.

## Active Dependencies

Pinned in `server/pyproject.toml` (uv-managed venv at `server/.venv`, Python 3.12.x):

- fastapi >=0.115,<1 — HTTP + WebSocket server (panel serving, /ws endpoint)
- uvicorn 0.49.0 — ASGI server (version proven in ECA venv)
- chromadb 1.5.9 — vector DB client for the bge RAG store (matches the DB built by ECA)
- sentence-transformers 5.5.1 — bge-m3 embedding loader (matches ECA venv)
- transformers 5.10.1 — pinned: 5.10 requires torch>=2.8 float8 symbols; older torch fails at import
- torch 2.12.0+cu126 — **must install from the cu126 index** (`--index-url https://download.pytorch.org/whl/cu126`); plain PyPI torch is CPU-only and the cu124 index lacks a compatible build. CPU fallback for machines without CUDA: torch 2.12.0+cpu with reduced seq/batch (switch documented in pyproject comments).
- numpy 2.4.6 — transitive pin proven in ECA venv
- ruff (dev) — lint; pytest (dev) — test runner; pytest-asyncio (dev) — WS/orchestrator tests

### Frontend (panel/) — to be pinned in `panel/package.json` by the scaffold task

- react ^19 / react-dom ^19 — panel UI runtime
- typescript ~5.x — TS sources (`panel/src`)
- vite (current major at scaffold time) + @vitejs/plugin-react — build tool; output `panel/dist/` (gitignored)
- tailwindcss ^4 + @tailwindcss/vite — styling, CSS-variables mode (shadcn requirement)
- shadcn/ui — vendored component SOURCE in `panel/components/ui/` (registry copy, not a runtime dep)
- Vercel AI Elements — vendored component SOURCE in `panel/components/ai-elements/` (`npx ai-elements@latest`; shadcn-style registry copy; no runtime CDN/registry dependency)
- monaco-editor + @monaco-editor/react ^4 — edit tab; MUST be wired to the npm bundle via `loader.config({ monaco })` (default CDN loader forbidden, rules.md)
- npm — package manager (node v24.11.1 system install)

> Decision: see [[decisions/03_react-panel-rebuild]] and [[decisions/05_monaco-editor-adoption]].

## Build Artifacts

- `panel/dist/` — Vite build output; NEVER committed (gitignored). Dev machines build locally (`npm run build`); distribution is packaged into GitHub Releases by the later release phase. See [[decisions/04_dist-release-distribution]].
- WebView2 SDK 1.0.3800.47 DLLs — `Microsoft.Web.WebView2.Core.dll` (649,840 B), `Microsoft.Web.WebView2.Wpf.dll` (82,544 B), `WebView2Loader.dll` (160,880 B, win-x64) — vendored at `vendor/webview2/`
- WebView2 Evergreen runtime — installed (Chromium 148 verified by probe)
- bge-m3 model weights — 4.3 GB in the HF cache (`C:\Users\ifthe\.cache\huggingface\hub\models--BAAI--bge-m3`); setup_env.ps1 checks presence and warns (first query downloads ~4.3 GB otherwise).
- ECA RAG DB — `C:\Users\ifthe\proj\eud\ECA\chromadb_bge` (111 MB, collection `eud_docs_bge`, 1024d cosine, 4,974 docs incl. comments + 54 EPS manual pages). Referenced by path; never imported into this repo.

## Legacy / Vendored

- `bridge/ZZZ_10_agent_bridge.lua` — verified v6 bridge imported from `C:\Users\ifthe\eud-agent-analysis\test-lua\`; extended in place (import-then-extend; LIST added by EUD-011, NEWEPS by EUD-012)
- `server/eud_agent/runner_legacy.py` — verified runner draft imported from ECA (read-only reference); absorbed into `server/eud_agent/runner_cli.py` later
- codex CLI — BYO npm shim; on this machine `C:\Program Files\nodejs\codex.cmd` (also `codex.ps1`). Never spawn bare `codex` without `shutil.which` resolution.
- node v24.11.1 — system install; runs the optional epscript-lsp AND the panel build toolchain
- @eps-server/server 1.2.12 (npm) — epscript-lsp language server (MIT, EDAC community); optional advisory diagnostics only; the system must degrade gracefully when absent
- KopiLua / KopiLuaInterface / luanet — the editor's embedded Lua; not a dependency we install, but the API surface the bridge codes against (editor v0.19.6.0, `C:\Users\ifthe\proj\eud\EUD.Editor.3.0.19.6.0`)
- (retired 2026-06-04) vanilla panel `panel/index.html|app.js|style.css` — verified baseline, replaced by the React panel; retained in git history only. See [[decisions/03_react-panel-rebuild]].

## Project Structure

See architecture.md "Repository layout". Tooling: uv for venv + installs, PowerShell 7 for scripts/, Vite/npm for panel/ (build output `panel/dist/` served by the server; dist and node_modules gitignored).

## Rationale

- **FastAPI over reusing ECA's Flask-less app.py stack**: the panel needs WebSocket push (progress events) and static serving from one origin; FastAPI/uvicorn gives both natively, and uvicorn 0.49.0 is already proven on this machine.
- **uv over pip**: the proven ECA venv is uv-managed (pip is absent there); one toolchain for both projects, and uv handles the cu126 index pin cleanly.
- **Vendored WebView2 DLLs over NuGet restore**: the drop-in install has no build step on the user machine; copying 3 DLLs next to the editor exe is the verified load path (app-base probing).
- **React + Vercel AI Elements over vanilla JS** (supersedes the original no-framework rationale): the user chose an AI-chat-native, visually polished UI; AI Elements ships vendored component source (no runtime CDN), and the no-build-on-user-machine principle moves to the release phase (GitHub Releases packaging + updater). The verified vanilla panel remains in git history as the fallback baseline. See [[decisions/03_react-panel-rebuild]] / [[decisions/04_dist-release-distribution]].
