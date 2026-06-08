# Decision 08: Tauri + Rust standalone rewrite (v2 architecture)

- Date: 2026-06-08
- Status: Accepted
- Context: The POC shipped as a drop-in Lua bridge that spawned an external Python
  FastAPI server and hosted the panel via a WebView2 window *inside* the editor.
  For real distribution the project is being refactored into a single standalone
  desktop app: the server collapses in-process and is rewritten in the shell's
  native language, and the UI moves out of the editor into its own window. The
  fork is the desktop framework, which (because the backend moves in-process) is
  effectively a backend-language choice.
- Considered:
  - Tauri 2 (Rust) — Pros: ~5MB shell, single-exe distribution, uses the system
    WebView2 which the project already depends on, thin-shell philosophy matches
    "heavy work stays native". Cons: Rust learning curve; ML ecosystem less
    mature than Node. Recommendation: ★★★.
  - Electron (Node/TS) — Pros: bundled Node makes epscript-lsp trivial, mature ML
    (transformers.js), single language with the existing TS panel. Cons: ~150MB
    bundle conflicts with the small-distributable goal. Recommendation: ★★☆.
  - Electrobun (Bun) — Pros: small + TS + native Bun runtime. Cons: too immature
    for a production distribution target; onnxruntime on Bun unproven.
    Recommendation: ★☆☆.
- Chosen: Tauri 2 (Rust). The backend is rewritten in Rust and runs in-process
  (no separate server, no spawned child); Python is removed entirely.
- Rationale: The user prioritizes a tiny single-file distributable. Electron's
  only advantage (in-process Node for epscript-lsp) is weak here — the LSP is
  advisory/optional by design (rules.md), so it cannot justify the bundle size.
  Going standalone also deletes the fragile in-editor WebView2 lifecycle (a whole
  class of measured crash/freeze bugs: EUD-039 dispatcher priority, project-switch
  re-arm traps).
- Impact: architecture.md, tech-stack.md, rules.md, features 10-15, all tasks.
