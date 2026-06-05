# Decision 06: AI Elements + Streamdown for the panel chat surface

- **Date**: 2026-06-05
- **Status**: accepted (user decision)
- **Supersedes**: the "dep pruning" carry-forward (ConversationLog/AgentStream/PlanView composed as plain styled rows with hand-rolled rendering, explicitly avoiding the streamdown/shiki pipeline).

## Context

Live panel use surfaced three rendering defects (EUD-063): codex reasoning invisible, the final answer styled as the least-visible text, raw internal event kinds (`delta`/`token_usage`/`turn_done`) leaking into the activity line. The hand-rolled rendering layer also re-implemented (poorly) what a chat UI library already solves: streaming markdown, reasoning collapse, tool-call rows, plan cards.

## Decision

The panel chat surface is built on **vendored Vercel AI Elements** (https://elements.ai-sdk.dev/) — mandatory components per the user: `Message`, `PromptInput`, `Plan` (plan approval), `Reasoning`; adopted alongside: `Conversation`, `Response`, `Tool`, `Loader`. Components are vendored as SOURCE under `panel/components/ai-elements/` (fetched at dev time via the shadcn registry, committed to the repo). ALL agent-authored markdown renders via **Streamdown** (https://streamdown.ai/) for real-time streaming markdown.

## Constraints preserved

- **No runtime CDN** (rules.md): AI Elements is vendored source; Streamdown and every transitive asset (highlighter, math, fonts) bundle from npm into `panel/dist/`. The built-dist external-origin scan remains the guard.
- **Diff rendering unchanged**: changeset file diffs stay SERVER-supplied unified-diff text with +/- coloring — no Monaco DiffEditor (Decision 05 scope unchanged).
- Korean labels, 500-entry log cap, send gating v2 — all retained.

## Alternatives rejected

- **Keep hand-rolled rendering, patch the three defects**: re-implements streaming markdown + reasoning collapse forever; the defect class (raw kinds, styling inversions) keeps recurring.
- **Markdown via dangerouslySetInnerHTML + a sanitizer**: rejected on safety; Streamdown renders untrusted markdown safely by design.

## Consequences

- Bundle size grows (streamdown + shiki); acceptable — assets are local, the panel is a desktop WebView2 surface.
- The EUD-060 hand-rolled PlanView markdown renderer is replaced by the Plan component + Streamdown.
- vitest suites move from asserting hand-rolled DOM to asserting component composition + the no-raw-kind-leak contract.
