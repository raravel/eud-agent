# Decision 11: Panel <-> backend transport = Tauri IPC

- Date: 2026-06-08
- Status: Accepted
- Context: The React panel currently talks to the Python server over a WebSocket
  on localhost (token + Origin validated). With the backend in-process under
  Tauri, the transport must be re-chosen.
- Considered:
  - Tauri IPC: `invoke` commands + emitted events (Recommended) — Pros: removes
    the localhost socket, token, Origin check, and `server.ready` entirely;
    idiomatic and more secure; the existing WS message schema maps 1:1
    (instruct/apply/status/list -> invoke; progress/code/applied/error -> events).
    Cons: the panel transport layer (ws-client) must be rewritten. ★★★.
  - In-process localhost HTTP/WS (axum) — Pros: panel transport code almost
    unchanged. Cons: re-introduces port/token/Origin complexity inside a single
    process for no benefit. ★★☆.
- Chosen: Tauri IPC (`invoke` + `emit`/`listen`). Streaming agent deltas
  (reasoning/answer) ride Tauri events; the request/response calls are commands.
- Rationale: One process has no trust boundary to police; the socket handshake was
  pure overhead. The message schema is preserved so panel rendering (Monaco, diff
  tab, AI Elements/Streamdown) is untouched — only the wire is swapped.
- Impact: architecture.md, rules.md, feature 15.
