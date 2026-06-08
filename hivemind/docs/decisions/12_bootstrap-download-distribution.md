# Decision 12: Distribution = first-run bootstrap download + data-dir layout

- Date: 2026-06-08
- Status: Accepted
- Context: The bge-m3 ONNX model (~570MB int8; 4.3GB fp32) and the RAG index are
  too large to embed in a small exe. The app needs a first-run install step, and
  the runtime state currently under the editor's `Data\agent\` must find new homes.
- Considered:
  - Small bootstrapper that downloads on first run (Recommended) — Pros: keeps the
    distributable small, matches the single-file-sharing goal. Cons: first-run
    needs network + integrity checks. ★★★.
  - Embed-and-extract — Pros: works offline. Cons: a ~570MB exe kills the
    single-small-file goal. ★★☆.
- Chosen: First-run bootstrap. The bge-m3 ONNX is fetched via fastembed's HF cache
  (cache dir pointed at `%localappdata%\eud-agent\models`); the RAG index is
  downloaded from a versioned **GitHub Release** asset (built in CI per Decision
  10). Every download is sha256-verified and placed atomically (temp + rename).
  The C++ engine is static-linked (no DLL to install — Decision 09).
- Data-dir layout:
  - **IPC rendezvous** (`inbox/`, `outbox/`, `status.txt`, `heartbeat.txt`) stays
    in the editor's `Data\agent\`. The Lua bridge finds it editor-relative (no
    path baked into .lua — KopiLua reads .lua as Latin1, so a literal path with a
    non-ASCII username would be mojibake). The app learns the editor path from a
    config it writes at install time (the app is UTF-8-safe).
  - **App user data** (`memory/`, `map_backups/`, `journal/`, `config.json`) ->
    `%appdata%\eud-agent\`.
  - **Large / regenerable** (bge-m3 model, RAG index, logs) ->
    `%localappdata%\eud-agent\` (Roaming must not carry 570MB).
- Rationale: Model size dominates, so downloading is the only way to stay small;
  the .NET-read config avoids the KopiLua Latin1 path trap; Local vs Roaming
  follows Windows convention.
- Impact: architecture.md, rules.md, feature 10, feature 14.
