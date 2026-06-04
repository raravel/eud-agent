# eud-agent Tech Stack

> Greenfield grounding (2026-06-04): this repo has no manifests yet. Versions below are grounded in (a) the working ECA venv measured via dist-info (`C:\Users\ifthe\proj\eud\ECA\.venv`), (b) binaries found on this machine, and (c) the verified WebView2 probe. The server manifest task (`server/pyproject.toml`) must pin exactly these versions; re-run Phase 0 grounding against the manifest once it exists.

## Active Dependencies

To be pinned in `server/pyproject.toml` (uv-managed venv at `server/.venv`, Python 3.12.x):

- fastapi >=0.115,<1 — HTTP + WebSocket server (panel serving, /ws endpoint)
- uvicorn 0.49.0 — ASGI server (version proven in ECA venv)
- chromadb 1.5.9 — vector DB client for the bge RAG store (matches the DB built by ECA)
- sentence-transformers 5.5.1 — bge-m3 embedding loader (matches ECA venv)
- transformers 5.10.1 — pinned: 5.10 requires torch>=2.8 float8 symbols; older torch fails at import
- torch 2.12.0+cu126 — **must install from the cu126 index** (`--index-url https://download.pytorch.org/whl/cu126`); plain PyPI torch is CPU-only and the cu124 index lacks a compatible build. CPU fallback for machines without CUDA: torch 2.12.0+cpu with reduced seq/batch.
- numpy 2.4.6 — transitive pin proven in ECA venv
- ruff (dev) — lint; pytest (dev) — test runner; pytest-asyncio (dev) — WS/orchestrator tests

## Build Artifacts

None yet (no compiled output in this repo). External runtime artifacts treated as ground truth:

- WebView2 SDK 1.0.3800.47 DLLs — `Microsoft.Web.WebView2.Core.dll` (649,840 B), `Microsoft.Web.WebView2.Wpf.dll` (82,544 B), `WebView2Loader.dll` (160,880 B, win-x64) — currently installed in the editor folder; to be vendored at `vendor/webview2/`
- WebView2 Evergreen runtime — installed (Chromium 148 verified by probe)
- bge-m3 model weights — 4.3 GB already present in the HF cache (`C:\Users\ifthe\.cache\huggingface\hub\models--BAAI--bge-m3`); no download needed on this machine. setup_env.ps1 must check presence and warn (first query downloads ~4.3 GB otherwise).
- ECA RAG DB — `C:\Users\ifthe\proj\eud\ECA\chromadb_bge` (111 MB, collection `eud_docs_bge`, 1024d cosine, 4,974 docs incl. comments + 54 EPS manual pages). Referenced by path; never imported into this repo.

## Legacy / Vendored

- `bridge/ZZZ_10_agent_bridge.lua` — verified v6 bridge imported from `C:\Users\ifthe\eud-agent-analysis\test-lua\`; extended in place (import-then-extend, never rewrite)
- `eud_agent_runner.py` — verified runner draft imported from ECA; absorbed into `server/eud_agent/runner_cli.py`
- codex CLI — BYO npm shim; on this machine `C:\Program Files\nodejs\codex.cmd` (also `codex.ps1`). Never spawn bare `codex` without `shutil.which` resolution.
- node v24.11.1 — system install; runs the optional epscript-lsp
- @eps-server/server 1.2.12 (npm) — epscript-lsp language server (MIT, EDAC community); optional advisory diagnostics only; the system must degrade gracefully when absent
- KopiLua / KopiLuaInterface / luanet — the editor's embedded Lua; not a dependency we install, but the API surface the bridge codes against (editor v0.19.6.0, `C:\Users\ifthe\proj\eud\EUD.Editor.3.0.19.6.0`)

## Project Structure

See architecture.md "Repository layout". Tooling: uv for venv + installs (the ECA venv has no pip; same convention here), PowerShell 7 for scripts/, plain HTML/JS/CSS for panel/ (no bundler, no node build step).

## Rationale

- **FastAPI over reusing ECA's Flask-less app.py stack**: the panel needs WebSocket push (progress events) and static serving from one origin; FastAPI/uvicorn gives both natively, and uvicorn 0.49.0 is already proven on this machine.
- **uv over pip**: the proven ECA venv is uv-managed (pip is absent there); one toolchain for both projects, and uv handles the cu126 index pin cleanly.
- **Vendored WebView2 DLLs over NuGet restore**: the drop-in install has no build step on the user machine; copying 3 DLLs next to the editor exe is the verified load path (app-base probing).
- **No panel framework (vanilla JS) over React/Vue**: one screen, WS client, diff rendering — a bundler would add a build step the drop-in deployment model avoids.
