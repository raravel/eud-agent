# eud-agent Tech Stack

> Grounding (2026-06-04, refreshed after the EUD-031 scaffold; re-checked 2026-06-05 for the v2 plan — manifests unchanged): Python deps grounded in `server/pyproject.toml` + `server/uv.lock`; frontend deps grounded in `panel/package.json` + `panel/package-lock.json` (exact resolved versions below).

## Active Dependencies

Pinned in `server/pyproject.toml` (uv-managed venv at `server/.venv`, Python 3.12.x):

- fastapi >=0.115,<1 — HTTP + WebSocket server (panel serving, /ws endpoint)
- uvicorn 0.49.0 — ASGI server (version proven in ECA venv)
- chromadb 1.5.9 — vector DB client for the bge RAG store (matches the DB built by ECA)
- sentence-transformers 5.5.1 — bge-m3 embedding loader (matches ECA venv)
- transformers 5.10.1 — pinned: 5.10 requires torch>=2.8 float8 symbols; older torch fails at import
- torch 2.12.0+cu126 — **must install from the cu126 index** (`--index-url https://download.pytorch.org/whl/cu126`); plain PyPI torch is CPU-only and the cu124 index lacks a compatible build. CPU fallback for machines without CUDA: torch 2.12.0+cpu with reduced seq/batch (switch documented in pyproject comments).
- numpy 2.4.6 — transitive pin proven in ECA venv
- openai-codex 0.1.0b3 — official Codex Python SDK (openai/codex `sdk/python`, module `openai_codex`); codex thread lifecycle + streaming JSONL events for the v2 agent core. Pre-release; pulls in the pre-release `openai-codex-cli-bin` (bundled binary) but pointed at the BYO authenticated CLI via `CodexConfig(codex_bin=...)` at runtime. `[tool.uv] prerelease = "allow"` set so `uv sync` resolves it (EUD-053 spike).
- mcp 1.27.2 — Model Context Protocol Python SDK; server side of the eud-tools stdio shim codex attaches to (FastMCP, stdio transport) (EUD-053 spike)
- ruff (dev) — lint; pytest (dev) — test runner; pytest-asyncio (dev) — WS/orchestrator tests

### Frontend (panel/) — pinned in `panel/package.json` (resolved via package-lock.json, EUD-031)

- react / react-dom 19.2.7 — panel UI runtime
- typescript 5.9.3 — TS sources (`panel/src`)
- vite 7.3.5 + @vitejs/plugin-react 5.2.0 — build tool; output `panel/dist/` (gitignored)
- tailwindcss / @tailwindcss/vite 4.3.0 — styling, CSS-variables mode (shadcn requirement)
- shadcn/ui — vendored component SOURCE in `panel/components/ui/` (registry copy, not a runtime dep; `components.json` aliases point future `npx shadcn add` at the same dirs)
- (retired) Vercel AI Elements — vendored in EUD-031, REPLACED by custom lightweight components during EUD-034 pruning (streamdown/shiki inflated the eager entry); the vendored source was deleted in EUD-035 (`panel/components/ai-elements/` guarded ABSENT by the contract test)
- monaco-editor 0.55.1 + @monaco-editor/react 4.7.0 (+ @monaco-editor/loader 1.7.0) — edit tab; bound to the npm bundle via `loader.config({ monaco })` in `panel/src/editor/monaco.ts` with 5 `?worker` Vite imports (CDN injection path verified unreachable — EUD-031 review)
- Final runtime deps (post EUD-034/035 pruning, package-lock-grounded): @monaco-editor/react ^4.7.0, class-variance-authority ^0.7.1, clsx ^2.1.1, lucide-react ^1.17.0, monaco-editor ^0.55.1, radix-ui ^1.4.3, react/react-dom ^19.2.0, tailwind-merge ^3.6.0 (9 deps; 17 registry/streamdown deps removed in EUD-034, ai + use-stick-to-bottom removed in EUD-035)
- Test devDeps: vitest ^3.2.6, happy-dom ^16.8.1, @testing-library/react|dom|user-event|jest-dom (panel unit/component suites, 120 tests)
- npm — package manager (node v24.11.1 system install)

> Decision: see [[decisions/03_react-panel-rebuild]] and [[decisions/05_monaco-editor-adoption]].

## Build Artifacts

- `panel/dist/` — Vite build output; NEVER committed (gitignored). Dev machines build locally (`npm --prefix panel run build`); distribution is packaged into GitHub Releases by the later release phase. See [[decisions/04_dist-release-distribution]]. Current dist (post EUD-034 pruning): ~14 MB total, eager entry 265.5 kB (84.6 kB gzip); Monaco lazy-split 3.79 MB + 5 local workers.
- WebView2 SDK 1.0.3800.47 DLLs — `Microsoft.Web.WebView2.Core.dll` (649,840 B), `Microsoft.Web.WebView2.Wpf.dll` (82,544 B), `WebView2Loader.dll` (160,880 B, win-x64) — vendored at `vendor/webview2/`
- WebView2 Evergreen runtime — installed (Chromium 148 verified by probe)
- bge-m3 model weights — 4.3 GB in the HF cache (`C:\Users\ifthe\.cache\huggingface\hub\models--BAAI--bge-m3`); setup_env.ps1 checks presence and warns (first query downloads ~4.3 GB otherwise). Measured: load+first-search 12.8s (CUDA), warm search 0.015s (EUD-017).
- ECA RAG DB — `C:\Users\ifthe\proj\eud\ECA\chromadb_bge` (111 MB, collection `eud_docs_bge`, 1024d cosine, 4,974 docs incl. comments + 54 EPS manual pages). Referenced by path; never imported into this repo.

## Legacy / Vendored

- `bridge/ZZZ_10_agent_bridge.lua` — verified v6 bridge imported from `C:\Users\ifthe\eud-agent-analysis\test-lua\`; extended in place (import-then-extend; LIST EUD-011, NEWEPS EUD-012, server lifecycle EUD-013, WebView2 hosting EUD-014 — WPF panel removed per spec)
- `server/eud_agent/runner_legacy.py` — verified runner draft imported from ECA (read-only reference); absorbed into `server/eud_agent/runner_cli.py` later
- codex CLI — BYO npm shim; on this machine `C:\Program Files\nodejs\codex.cmd` (also `codex.ps1`). Never spawn bare `codex` without `shutil.which` resolution. Direct asyncio exec of the .CMD shim works on Windows ProactorEventLoop (proven live, EUD-016).
- node v24.11.1 — system install; runs the optional epscript-lsp AND the panel build toolchain
- @eps-server/server 1.2.12 (npm) — epscript-lsp language server (MIT, EDAC community); optional advisory diagnostics only; the system must degrade gracefully when absent
- KopiLua / KopiLuaInterface / luanet — the editor's embedded Lua; not a dependency we install, but the API surface the bridge codes against (editor v0.19.6.0, `C:\Users\ifthe\proj\eud\EUD.Editor.3.0.19.6.0`)
- (retired 2026-06-04) vanilla panel — verified baseline, replaced by the React panel; `panel/app.js`/`panel/style.css` DELETED in EUD-035 (deletion guarded by the contract test); retrievable from git history. `panel/index.html` is the Vite template. Selfcheck now requires the BUILT `panel/dist/`. See [[decisions/03_react-panel-rebuild]].

## Project Structure

See architecture.md "Repository layout". Tooling: uv for venv + installs, PowerShell 7 for scripts/, Vite/npm for panel/ (build output `panel/dist/` served by the server; dist and node_modules gitignored).

## Rationale

- **FastAPI over reusing ECA's Flask-less app.py stack**: the panel needs WebSocket push (progress events) and static serving from one origin; FastAPI/uvicorn gives both natively, and uvicorn 0.49.0 is already proven on this machine.
- **uv over pip**: the proven ECA venv is uv-managed (pip is absent there); one toolchain for both projects, and uv handles the cu126 index pin cleanly.
- **Vendored WebView2 DLLs over NuGet restore**: the drop-in install has no build step on the user machine; copying 3 DLLs next to the editor exe is the verified load path (app-base probing).
- **React + Vercel AI Elements over vanilla JS** (supersedes the original no-framework rationale): the user chose an AI-chat-native, visually polished UI; AI Elements ships vendored component source (no runtime CDN), and the no-build-on-user-machine principle moves to the release phase (GitHub Releases packaging + updater). The verified vanilla panel remains in git history as the fallback baseline. See [[decisions/03_react-panel-rebuild]] / [[decisions/04_dist-release-distribution]].
