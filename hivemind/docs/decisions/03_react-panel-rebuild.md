# Decision 03: React + Vercel AI Elements panel replaces the vanilla panel

- Date: 2026-06-04
- Status: Accepted
- Context: After the vanilla panel (EUD-021) was implemented and runtime-verified, the user requested an AI-chat-native, visually polished UI. The original harness committed to "no framework, no build step" to protect the drop-in deployment model; that commitment was re-evaluated with the user via /hv:clarify.
- Considered:
  - Replace vanilla with React + AI Elements — Pros: AI-chat-native components (Conversation, Message, PromptInput, CodeBlock), single UI to maintain, modern UX. Cons: introduces a Node build step on dev machines; rewrite cost. Recommendation: ★★★.
  - Keep vanilla as fallback alongside React — Pros: working fallback if React path fails. Cons: two UIs to keep in sync forever. Recommendation: ★★☆.
  - Stay vanilla — Pros: zero new tooling. Cons: rejects the user's UX goal. Recommendation: ★☆☆.
- Chosen: Replace vanilla with React + AI Elements (full replacement; vanilla files deleted after the React panel passes verification — git history retains them)
- Rationale: user decision; the merged vanilla panel remains in git history as the verified baseline, so a separate live fallback adds maintenance without safety.
- Impact: features/03_agent-panel.md (rewritten), rules.md (Server and panel section), tech-stack.md (frontend stack + rationale), architecture.md (panel description, repository layout), server config panel-files check (panel/dist), test_panel_static.py contract.

Layout commitment recorded here (no separate decision): `panel/` becomes the Vite app root (package.json, src/, index.html template); build output `panel/dist/` is gitignored; the server serves `panel/dist/` only.
