# eud-agent Rules

Violations of the EDITOR-* and LUANET-* rules crash or corrupt EUD Editor 3 at runtime. These are measured behaviors (2026-06-04 verification), not style preferences.

## Editor integrity

- **NEVER modify EUD Editor 3 source, binaries, or repo** (third-party, Buizz/EUD-Editor-3). Integration = file copies only: `.lua` into `Data\Lua\TriggerEditor\`, DLLs next to the exe, runtime state under `Data\agent\`.
- **NEVER modify the ECA repo** beyond what its own git-exclude already allows. The RAG DB (`chromadb_bge`) is read-only input; NEVER import it into this repo (chromadb mutates tracked sqlite on every open — proven LFS churn).
- ALWAYS treat `bridge/ZZZ_10_agent_bridge.lua` as import-then-extend: keep verified v6 code paths intact; extend, do not rewrite.

## Lua bridge (KopiLua/luanet) — crash rules

- NEVER use `os.execute` (KopiLua corrupts it into `CMD.exe /C regenresx ...`). External processes only via luanet `System.Diagnostics.Process`.
- NEVER use sockets or `io.popen` from Lua (not available). Server-bridge IPC is file-based only.
- NEVER rely on lua `pcall` to catch .NET exceptions — they pass through to the Dispatcher and show an editor error dialog. Avoid risky calls structurally; isolate complex logic in Python.
- ALWAYS access editor objects on the UI thread (`DispatcherTimer.Tick`). NEVER while `pgData.IsCompilng` (build shares the same lua_State from a BackgroundWorker).
- .NET arrays: ALWAYS use the indexer `arr[i]` (0-based). NEVER `arr:GetValue(i)` (KeyNotFoundException, uncatchable).
- VB parameterized properties: ALWAYS call as `obj:get_PropName(args)` (e.g. `:get_Files(i)`, `:get_DatBinding(...)`). Plain access throws TargetParameterCountException.
- Instance methods use colon (`obj:Method()`), properties use dot (`obj.Prop`).
- `load_assembly` BEFORE `import_type`; `System.dll` and WPF assemblies need full names (`System, Version=4.0.0.0, Culture=neutral, PublicKeyToken=b77a5c561934e089`).
- Enum arguments: pass enum objects from `import_type("Ns.Outer+EnumName")`, never raw numbers.
- Empty `StringText` returns nil: ALWAYS `val or ""` when reading.
- Non-ASCII literals in .lua source are mojibake (KopiLua reads Latin1): ALWAYS restore with the `u8()` helper. Text read via .NET `File.ReadAllText` or typed into WPF controls is fine as-is.
- SET/NEWEPS change memory objects only (user saves to disk). Setter exists only for CUI/SCA/RawText file types — GUI files are read-only; LIST must expose the type so callers can avoid them.
- Auxiliary windows are closed by the editor on project create/switch: ALWAYS re-create the panel via window-handle tracking ("project open AND window not alive" per Tick). NEVER rely on `pjData==nil` re-arm alone.
- ALWAYS write `heartbeat.txt` before the `IsCompilng` early-return in Tick (unconditional heartbeat).

## IPC and encoding

- ALWAYS write IPC files (`.cmd`, `.result`, cfg/ready/heartbeat) as UTF-8 **without BOM**. In Python: `encoding="utf-8"` — `utf-8-sig` is forbidden. (`File.ReadAllText` strips an incoming BOM, so reads are safe; writes from Python must still be BOM-free for first-line command parsing of files Python reads back.)
- The server deletes each `.result` after consuming it and clears stale `inbox/outbox` files at startup. The bridge deletes `.cmd` after processing.
- Server command files are named `srv-<uuid8>.cmd`; the legacy runner keeps `agent_<jobid>.cmd`. A consumer polls only its own basenames.
- NEVER poll `.result` without a timeout. Default 10s; extend to 180s when `status.txt` says `compiling=true` and notify the panel (`waiting_build`).

## codex invocation (Windows)

- NEVER spawn bare `"codex"`. ALWAYS resolve via `shutil.which("codex")` to the `.cmd` shim path (fail fast with a clear error if unresolved). `CODEX_CMD` env var may override with a full path.
- ALWAYS pass `--skip-git-repo-check`; set `cwd` to the repo root.
- ALWAYS pass the prompt via stdin (write, then close). NEVER as argv (32,767-char CreateProcess limit; RAG context exceeds it).
- ALWAYS give every subprocess an explicit stdin (the prompt pipe, or `subprocess.DEVNULL`) — an inherited console-less stdin makes codex hang until timeout.
- Treat codex stdout as noisy: extract fenced code blocks; if none, fail with the raw output in the error message rather than applying noise to the editor.

## Server and panel

- ALWAYS bind `127.0.0.1` explicitly. NEVER `0.0.0.0`.
- ALWAYS require the `server.ready` token on WS connect and validate the `Origin` header. Reject otherwise.
- ALWAYS write `server.ready` atomically (temp + rename) and only after confirming the socket accepts connections. Delete it on graceful shutdown.
- ALWAYS self-terminate when `heartbeat.txt` is stale (>60s). The server must never outlive the editor.
- WebView2: ALWAYS set an explicit user-data-folder (`Data\agent\webview2`). NEVER let it default next to the editor exe. ALWAYS subscribe `NavigationCompleted` and re-Navigate on failure (no auto-retry in WebView2).
- The panel has no code-editor component (no Monaco/CodeMirror; textarea + server-side diff only) and no external CDN dependencies — everything served from the local server.
- RAG model loading must never gate `server.ready` (lazy load + background warmup; report `rag_warmup` progress).
- epscript-lsp diagnostics are advisory only: they annotate, never block apply; absence of node/the package must not break the flow.

## Process

- All spec/task content in English; user-facing conversation in Korean.
- Single editor instance per machine is the supported topology (documented limitation).
- Windows E2E steps that need the editor GUI are user-assisted; everything else must be verifiable headless via verify.md stages.
