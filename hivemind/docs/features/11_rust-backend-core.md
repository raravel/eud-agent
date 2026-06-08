# Feature 11: Rust backend core (IPC, engine, codex, tools, bridge_io, memory)

Ports the Python server's request handling into the in-process Rust core, exposed over
Tauri IPC. The orchestration, prompt assembly, tool layer, and file-IPC to the editor are
re-implemented; behavior matches the v1 features 02/05 (kept as behavioral source).

> Decision: see [[decisions/08_tauri-rust-rewrite]] and [[decisions/11_panel-tauri-ipc]].

## Tauri IPC surface (replaces the WebSocket protocol)
Commands (panel -> core, `invoke`):
- `instruct { instruction, target, useContext }` -> runs rag -> codex -> diff; result
  streamed via events, returns when the turn completes.
- `apply { mode: "set"|"neweps", target, code }` -> applies via bridge; returns
  applied/error.
- `status {}` -> `{ compiling, project }` (read from editor status.txt).
- `list {}` -> `{ files: [{ path, ftype, settable }] }` (bridge LIST).
Events (core -> panel, `emit`):
- `progress { stage: rag|rag_warmup|codex|lsp|waiting_build|bootstrap, detail, pct? }`
- `code { code, lang: "eps", diff, diagnostics }` — diff is a unified diff (`similar`)
  against current target content for mode set; diagnostics is advisory LSP (empty if absent)
- `agent_event { kind, ... }` — reasoning/answer deltas (rendered via AI Elements; raw
  kind strings never shown)
- `applied { target }` / `error { message }`

## Orchestrator (engine)
Mirrors v1 `engine.py`/`agent_runner.py`: build the v2 system prompt (`[first principles]`
before `[reference context]`, `[evidence]`, `[message format]`), run RAG, call codex, run
advisory LSP, produce diff. The agentic tool loop (propose_plan, search_docs, eps/dat
edits, location/player writes, memory) is ported with the same tool names and the evidence
gate + first-principles rejections (rules.md). codex is the LLM via subprocess.

## codex_client
Port of `codex_client.py` under rules.md "codex invocation (Rust)": `which` to the `.cmd`
shim, prompt via stdin, `--skip-git-repo-check`, explicit piped stdin, fenced-block
extraction. Use `tauri-plugin-shell` or `tokio::process` with piped stdio + timeout.

## bridge_io (file-IPC to editor)
Port of `bridge_io.py`: write `srv-<uuid8>.cmd` to `<editor>\Data\agent\inbox` (UTF-8 no
BOM), poll `outbox\<name>.result` with 10s timeout (180s when status.txt compiling=true,
emit `waiting_build`), delete consumed `.result`, clear stale inbox/outbox at startup.
Commands: PING/STATUS/LIST/GET/SET/NEWEPS/GETDAT/SETDAT/BUILD/LUA.

## memory
Port of `memory.py`: project memory under `%appdata%\eud-agent\memory\`; `memory_write` is
evidence-gate-exempt.

## Edge cases
- codex CLI unresolved -> fast clear error (no bare spawn).
- editor not connected (stale/absent heartbeat) -> instruct/apply return a friendly error;
  panel shows "editor not connected".
- build in progress -> file-IPC extends timeout and emits `waiting_build`.

## Implementation
- `src-tauri/src/ipc.rs` — tauri command handlers + event emit helpers
- `src-tauri/src/engine.rs` — orchestrator, prompt assembly, tool loop
- `src-tauri/src/tools.rs` — tool layer (evidence gate, first_principles, btn/xdat rails)
- `src-tauri/src/codex_client.rs` — codex subprocess
- `src-tauri/src/bridge_io.rs` — file-IPC client
- `src-tauri/src/memory.rs` — project memory
- `src-tauri/src/data/first_principles.md` — bundled prompt asset (ported)
- external: `tokio`, `which`, `similar`, `tauri-plugin-shell`, `serde_json`
