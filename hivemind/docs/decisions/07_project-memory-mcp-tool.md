# Decision 07: Project memory via journaled MCP write tool

- Date: 2026-06-06
- Status: Accepted
- Context: Planning the per-map-project memory harness ("project memory") — a store codex
  updates autonomously so it remembers map structure, resource allocation, conventions, and
  corrections across chats. The design conversation (2026-06-06) settled storage location
  (`<data-dir>/harness/<project-name>/`), injection strategy (full injection with size caps,
  `[project memory]` between `[first principles]` and `[reference context]`), user visibility
  (panel memory view with read+edit), and apply-gated commits — but those choices were made
  against the v1 single-shot instruct/apply model. The codebase is v2 (codex SDK thread +
  eud-tools MCP + journal/changeset review; v1 messages removed), so two forks re-opened:
  the capture mechanism and the commit/review shape.
- Considered (capture mechanism):
  - MCP tool `memory_write` — Pros: reuses the existing tool registry, server-side validation,
    and journal infrastructure; no output parsing. Cons: one more tool spec to maintain.
    Recommendation: ★★★.
  - `harness-update` fenced block in answer text — Pros: works without a tool. Cons: requires
    parsing streamed SDK answers; fences can be dropped or malformed; v1-era design.
    Recommendation: ★☆☆.
  - Post-turn extraction call — Pros: separates memory quality from generation. Cons: extra
    codex call per turn = user token cost + latency. Recommendation: ★★☆.
- Considered (commit/review shape, mapping the v1 "commit on Apply only" decision):
  - Journaled write tool (changeset integration) — Pros: memory updates appear as changeset
    items with per-item accept/reject (reject = rollback via the existing inverse-op path);
    crash-safe persistence for free. Cons: editor changes and memory changes mix in one
    changeset. Recommendation: ★★★.
  - Separate staging (write files only on accept) — Pros: nothing touches disk before
    approval. Cons: a second staging path parallel to the journal; lost on crash.
    Recommendation: ★★☆.
  - Immediate write, no review — Pros: simplest. Cons: self-reinforcing bad memory defended
    only by panel editing; contradicts the user's explicit apply-gating choice.
    Recommendation: ★☆☆.
- Chosen: MCP tool `memory_write`, implemented as a journaled write tool so memory updates
  ride the existing changeset review (reject rolls the file back).
- Rationale: v2's natural extension point — codex updating its own memory "by itself" is
  exactly a tool call; journaling preserves the spirit of the apply-gating decision (user
  approval gate) with zero new staging machinery. User confirmed both recommendations.
- Impact: features/07_project-memory.md (new feature spec); tools.py (new ToolSpec, gate
  exemption); journal.py (memory snapshot/inverse + changeset item kind); engine.py
  ([project memory] section + episode recording); app.py (memory_get/memory_save WS);
  panel (memory view).
