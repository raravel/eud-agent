# Changeset Review Panel (panel v2: chat-first, plan + accept/reject)

Replaces the v1 target-picker/apply-bar flow ENTIRELY (user decision: full replacement — the agent chooses files/targets itself). The panel becomes a chat-first surface with three review affordances: streamed agent progress, plan review with feedback iteration, and an apply-then-review changeset with per-item and bulk accept/reject.

**UI foundation (user decision 2026-06-05 — supersedes the earlier "dep pruning / no streamdown" carry-forward)**: the surface is built on vendored **Vercel AI Elements** components — mandatory: `Message`, `PromptInput`, `Plan` (plan approval), `Reasoning`; adopted alongside: `Conversation` (auto-scroll container), `Response` (message body), `Tool` (tool-call rows), `Loader`. Vendored SOURCE under `panel/components/ai-elements/` (fetched at dev time, committed — never a runtime CDN). ALL agent-authored markdown renders through **Streamdown** (streaming-safe markdown, npm-bundled) so text renders live as deltas arrive. See [[decisions/06_ai-elements-streamdown-adoption]].

## UI layout

```text
+-------------------+--------------------------------+----------------------+
| SessionSidebar    | selected session               | ProjectSidebar       |
| + 새 세션          | Header + conversation          | DAT wiki / memory /  |
| current project   | plan + changeset + PromptInput | workspace tabs       |
| running / queued  |                                |                      |
| review / idle     |                                |                      |
+-------------------+--------------------------------+----------------------+
```

The left sidebar is primary conversation navigation; selection is independent from the session
currently running. The center is keyed by selected session id and owns all session decisions.
The right sidebar is project-scoped contextual data. At narrow widths the left collapses to a
rail and the right becomes an overlay, preserving the center chat width.

## State machine

```mermaid
stateDiagram-v2
    [*] --> connecting
    connecting --> ready: transport open
    connecting --> retry: transport error
    retry --> connecting
    ready --> queued: another session owns project lane
    queued --> thinking: queue head
    ready --> thinking: lane free + chat sent
    thinking --> ready: answer (no edits)
    thinking --> plan_review: plan{}
    plan_review --> thinking: plan_feedback / plan_approve
    thinking --> changeset_review: changeset{}
    changeset_review --> ready: all decisions completed
```

The project execution lane remains owned throughout plan and changeset review. New chats queue;
they never default-accept another session's undecided changes. Reconnect during a running turn
cancels that turn; a persisted pending changeset remains reviewable.

## Behaviors

- **Send gating v2**: `connected && hasProject && !busy` — the settable-target requirement is GONE (the agent creates files itself). No-project keeps the v1 placeholder behavior.
- **Attachments**: the PromptInput accepts up to 5 files through the picker,
  drag-and-drop, or pasted clipboard images. PNG/JPEG/WebP/GIF files (≤10 MiB)
  are sent to Codex as `localImage` inputs; UTF-8 text/code files (≤512 KiB
  combined per turn) are delimited into the user text. Unsupported binaries are
  rejected. A message may contain attachments without text. Selected files render
  as removable chips (generated image thumbnail + name + size), and the same
  attachment cards persist in user-message history. Files cross Tauri IPC as a raw
  `Uint8Array`, are stored under LocalAppData by opaque UUID, bound to the active
  session on send, and deleted with that session. `chat` and `plan_feedback` carry
  only the staged attachment ids. The Tauri window sets `dragDropEnabled=false` so
  WebView2 delivers normal HTML5 file-drop events on Windows instead of consuming
  them in Tauri's native handler.
- **Codex model controls**: the PromptInput footer contains compact model + reasoning selectors populated from the authenticated app-server `model/list` catalog. Changing the model selects that model's advertised default effort; changing either control persists the coherent pair and applies it to the next turn. Selects lock while loading/saving or while a turn is in flight, and a failed catalog load leaves an inline retry control.
- **Status visibility** (user request 2026-06-05): header shows connection state transitions (연결 중 → 연결됨 → 재연결 중) and RAG model state with elapsed seconds while loading (`rag_warmup` started ts → done), reusing `progressLabel`.
- **Agent stream (EUD-063 contract; EUD-068/069 amendments)**: per-turn `agent_event`s drive three surfaces — (1) `reasoning` deltas accumulate into the **Reasoning** component: dim/secondary, GPT-style, auto-open while streaming, collapses when the answer starts; (2) `delta` answer text streams into a PROMINENT (foreground) agent **Message/Response** via Streamdown; (3) `tool_call`/`tool_result` render as **Tool** rows showing the tool name (도구 호출 n건 summary retained) PLUS the call arguments (요청) and result text (결과) from `agent_event.data` inside the expandable card; a non-"completed" status renders a 실패 badge (EUD-068). Raw internal kind identifiers (`delta`, `answer`, `token_usage`, `turn_done`, `item_started`, `item_completed`, `event`) MUST NEVER appear as literal UI text. All per-turn surfaces reset when a new turn starts.
- **Inline stream placement (EUD-069 — layout-crush fix)**: the live agent stream (Reasoning block + Tool rows + streamed answer bubble) renders INLINE at the END of the Conversation scroll area — NEVER as a fixed band between the log and the input (an unbounded band has no min-height escape and crushes the log/plan card; measured live: log 0px, plan 33px, 승인 button off-viewport). When a turn ends (answer/plan/changeset/error), the tool rows ARCHIVE into the log as a compact entry carrying the rows (`LogEntry.tools` → 도구 호출 n건 — name×k summary + expandable Tool cards) and the live buffer clears.
- **Persistent turn status + stop**: while `phase === "thinking"`, a compact status bar stays
  attached to the bottom PromptInput instead of scrolling away. It derives the current label
  from the live turn (`작업 준비 중` / `추론 중` / `도구 실행 중 · <name>` /
  `도구 결과 확인 중` / `응답 작성 중`), uses the bundled CSS Shimmer with
  `prefers-reduced-motion` respected, and exposes a visible `작업 중단` control.
- **Cancel and edit history**: `cancel{}` sends app-server `turn/interrupt` and does not
  resolve until the interrupted `turn/completed` arrives. Each user bubble exposes `수정`;
  editing cancels an active turn if needed, removes that message and every later panel row,
  restores its text/attachments to PromptInput, and calls `conversation_rewind{panelLog}` so
  the retained session starts a fresh Codex thread replaying only the surviving prefix.
  Already-applied editor/map changes are not rolled back by a chat-history edit.
- **Long-chat virtualization**: `ConversationLog` uses `@tanstack/react-virtual` with measured
  variable-height rows and overscan; only viewport-near message/activity rows stay mounted.
  The virtual height remains inside `use-stick-to-bottom`, preserving stream autoscroll and
  the existing scroll-to-bottom control without retaining all 500 capped rows in the DOM.
- **Decision progress (EUD-070)**: while a `changeset_decision` awaits its `rollback_result`, ChangesetView shows a spinner notice (결정 처리 중…) — a rollback replays inverse ops over the 1s-tick file IPC, so the wait is visible, not just silently-disabled buttons.
- **Answer prominence**: agent answers are the most visible text in the log (foreground Message bubbles, Streamdown-rendered); system/progress/info rows stay muted. (Inverts the original v2 styling where answers were muted.)
- **Plan review (EUD-074 — user decision 2026-06-05)**: ai-elements **Plan** component; plan markdown renders via Streamdown. The embedded feedback textarea and the [수정요청] button are REMOVED: **the MAIN prompt input is the feedback channel** — during plan_review it stays ENABLED with a guidance placeholder, and a send routes to `plan_feedback{text}` (App routes by phase; the panel stays in plan_review until the next `plan{revision+1}` replaces the card). [승인] on the plan card sends `plan_approve`. `plan_review` is therefore NOT a send-gated busy phase (only `thinking` is).
  Clicking [승인] immediately collapses the approved revision while its execution turn runs;
  the accessible Plan trigger can reopen it manually. A later plan revision always opens
  automatically so new review content is never hidden by the previous revision's state.
- **New session**: the permanent left `SessionSidebar` creates an unsaved draft tab. Its first
  queued message calls `session_create`; there is no duplicate reset control in `PromptInput`.
- **Multi-active state**: every row owns a `PanelStore` and an
  `idle|queued|running|review|error` activity. Turn events route by `sessionId`; project events
  fan out to all rows.
- **Changeset review**: grouped DAT/file/settings/plugin/main entries retain the project lane
  until all decisions complete; incomplete/failed decisions keep it owned by that session.
- **Workspace explorer / project wiki**: the right project sidebar's Files tab opens the
  viewer-only workspace explorer. `workspace_list` refreshes the EPS source mirror and
  returns durable documents plus `source/`; selecting a file calls confined
  `workspace_read`. `specs/` sorts first, `specs/index.md` is the default wiki home, and
  Markdown renders in Streamdown static mode. Existing relative Markdown links that resolve
  to listed workspace files are rewritten to a reserved safe HTTPS target and intercepted
  as in-explorer navigation; external links retain the normal safe browser path.
  Source/non-Markdown text uses a read-only code block. Documents carry “검토 대상 문서”,
  accepted revisions carry “확정됨 · rN”, approved plan snapshots carry
  “승인된 계획 · rN”, and generated EPS files carry “읽기 전용 소스”.
- **Workspace changeset items**: turn-end filesystem create/modify/delete entries render as
  category `workspace`, with a distinct Workspace title badge and server unified diff. They
  use the existing per-item/bulk accept/reject controls; reject restores the trusted
  pre-turn snapshot without an editor connection.
- **Diff/preview limits**: reuse v1 truncation (1 MiB UTF-16-consistent) for previews/diffs.
- **Diagnostics**: epscript-lsp advisory strip retained for files the agent wrote (server includes diagnostics per modified/created eps in the changeset item).
- **Removed**: TargetPicker, ApplyBar, ReviewTabs as apply-source, NEWEPS filename input, `canSendSet/canSendNewEps` gating, Monaco edit-buffer-as-apply-source. Monaco remains only as a lazy read-only viewer for file previews/diffs if needed by ChangesetView.
- Korean labels throughout; log cap 500 retained.

## Verification contract

- vitest unit suites: state machine transitions (incl. reconnect mid-thinking and changeset persistence), changeset grouping/rendering logic, plan revision replacement, decision dispatch payloads, status header (elapsed-time formatting), reasoning/answer-delta accumulation + reset-per-turn, and a no-raw-kind-leak test (a noise-kind event sequence must not surface literal kind strings).
- `npm --prefix panel run build` exits 0; static contract test updated (target-picker/apply-bar components ABSENT guards replacing the v1 presence checks); the built-dist external-origin scan stays green with Streamdown (all highlighter/math assets npm-bundled — no runtime CDN).
- Live E2E: the three v2 acceptance scenarios (see plan) drive this UI in the editor.

## Implementation

- `panel/src/ws/protocol.ts` / `client.ts` — WS v2 message types (v1 instruct/apply removed; + `reset{}`, `reasoning`/`delta` agent_event kinds)
- `panel/src/state/store.ts` — state machine v2 + changeset/plan state
- `panel/src/components/` — InstructionBox→PromptInput wrapper, AgentStream, PlanView,
  ChangesetView (including Workspace diffs), WorkspaceView (file explorer + Markdown/source
  viewer), Header, and ConversationLog.
- `panel/components/ai-elements/` — vendored AI Elements source (Conversation, Message, Response, PromptInput, Plan, Reasoning, Tool, Loader)
- `panel/src/lib/progress.ts` — extended labels (elapsed time)
- `panel/src/lib/changeset.ts` — dat-group id-shape helpers (itemKey/itemIds) shared by view + store
- `panel/src/lib/attachments.ts` — raw binary Tauri upload, client-side image thumbnails, limits/size labels
- removed: `TargetPicker.tsx`, `ApplyBar.tsx`, ReviewTabs apply wiring
- external: `streamdown` (npm, bundled — real-time markdown); served by `server/eud_agent/app.py`; protocol per [[features/05_agent-core|05_agent-core]] `05_agent-core.md`
