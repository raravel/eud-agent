# Decision 11: Panel <-> backend transport = Tauri IPC

> **SUPERSEDED by [[decisions/13_ipc-v2-chat-contract]] (2026-06-09).** The transport
> choice (Tauri IPC `invoke` + events) still holds, but the "WS schema maps 1:1 via
> instruct/apply/status/list -> invoke; progress/code/applied/error -> events" premise was
> wrong — the panel speaks a v2 CHAT protocol (chat/plan/changeset...), not instruct/apply.
> The live contract is the v2 chat schema in decision 13. The instruct/apply mapping below
> is retained only as the historical record of why this decision was reopened.

- Date: 2026-06-08
- Status: Superseded (transport choice retained; message mapping replaced by decision 13)
- Context: The React panel currently talks to the Python server over a WebSocket
  on localhost (token + Origin validated). With the backend in-process under
  Tauri, the transport must be re-chosen.
- Considered:
  - Tauri IPC: `invoke` commands + emitted events (Recommended) — Pros: removes
    the localhost socket, token, Origin check, and `server.ready` entirely;
    idiomatic and more secure. Cons: the panel transport layer (ws-client) must be
    rewritten. ★★★.
  - In-process localhost HTTP/WS (axum) — Pros: panel transport code almost
    unchanged. Cons: re-introduces port/token/Origin complexity inside a single
    process for no benefit. ★★☆.
- Chosen: Tauri IPC (`invoke` + `emit`/`listen`). Streaming agent deltas
  (reasoning/answer) ride Tauri events; the request/response calls are commands.
- Rationale: One process has no trust boundary to police; the socket handshake was
  pure overhead.
- Impact: architecture.md, rules.md, feature 15 — now reframed to the v2 chat schema
  by decision 13.
