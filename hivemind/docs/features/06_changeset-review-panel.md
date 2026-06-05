# Changeset Review Panel (panel v2: chat-first, plan + accept/reject)

Replaces the v1 target-picker/apply-bar flow ENTIRELY (user decision: full replacement — the agent chooses files/targets itself). The panel becomes a chat-first surface with three review affordances: streamed agent progress, plan review with feedback iteration, and an apply-then-review changeset with per-item and bulk accept/reject.

## UI layout

```
+----------------------------------------------------+
| EUD 에이전트   [project]  [conn: 연결중|연결됨|재연결] |
|                [RAG: 로드중 nn초|준비됨|불가]          |
+----------------------------------------------------+
| ConversationLog (chat):                             |
|   user messages · answer{} bubbles                  |
|   agent_event stream (thinking/tool calls, live)    |
|   plan cards · changeset cards (inline, history)    |
+----------------------------------------------------+
| [PlanView - when plan{} active]                     |
|   markdown render · 피드백 입력 · [수정요청] [승인]    |
+----------------------------------------------------+
| [ChangesetView - when changeset{} active]           |
|   Data: unit [76] 마린                               |
|     · HP  40 → 80          [✓ accept] [✗ reject]    |
|     · Gas 0 → 25           [✓] [✗]                  |
|   Files:                                            |
|     · +created teleport.eps (preview)   [✓] [✗]     |
|     · ~modified main.eps (unified diff) [✓] [✗]     |
|     · -deleted old.eps                  [✓] [✗]     |
|   Settings/Plugins/Main: old → new rows  [✓] [✗]    |
|   [전체 적용 유지] [전체 되돌리기]                      |
+----------------------------------------------------+
| InstructionBox: textarea + [전송]                    |
+----------------------------------------------------+
```

## State machine

```mermaid
stateDiagram-v2
    [*] --> connecting
    connecting --> ready: WS open
    connecting --> retry: 2s backoff
    retry --> connecting
    ready --> thinking: chat sent
    thinking --> ready: answer (no edits)
    thinking --> plan_review: plan{}
    plan_review --> thinking: plan_feedback / plan_approve
    thinking --> changeset_review: changeset{}
    changeset_review --> ready: decisions done (accept/reject applied)
    changeset_review --> thinking: follow-up chat (undecided items auto-accept)
```

Reconnect during thinking resets to ready with a notice (server cancels the thread turn); the last changeset stays reviewable (journal is server-persisted).

## Behaviors

- **Send gating v2**: `connected && hasProject && !busy` — the settable-target requirement is GONE (the agent creates files itself). No-project keeps the v1 placeholder behavior.
- **Status visibility** (user request 2026-06-05): header shows connection state transitions (연결 중 → 연결됨 → 재연결 중) and RAG model state with elapsed seconds while loading (`rag_warmup` started ts → done), reusing `progressLabel`.
- **Agent stream**: `agent_event`s render as a live activity line under the latest user message (도구 호출 n건 · 현재: dat_set units hp …); collapsed into a summary row when the turn ends.
- **Plan review**: markdown plan card; 피드백 textarea sends `plan_feedback` (stays in plan_review, next `plan{revision+1}` replaces card); 승인 sends `plan_approve`.
- **Changeset review**: renders `changeset.items[]` grouped: dat per objId (unit name resolved server-side) with property/old→new; files by kind (created → content preview, modified → server unified diff with +/- coloring (unchanged rule: no Monaco DiffEditor), deleted → name); settings/plugins/main as old→new rows. Each item has accept/reject; bulk buttons map to `changeset_decision{all}`. Reject responses (`rollback_result`) flip the row to 되돌림 state; failures surface inline.
- **Diff/preview limits**: reuse v1 truncation (1 MiB UTF-16-consistent) for previews/diffs.
- **Diagnostics**: epscript-lsp advisory strip retained for files the agent wrote (server includes diagnostics per modified/created eps in the changeset item).
- **Removed**: TargetPicker, ApplyBar, ReviewTabs as apply-source, NEWEPS filename input, `canSendSet/canSendNewEps` gating, Monaco edit-buffer-as-apply-source. Monaco remains only as a lazy read-only viewer for file previews/diffs if needed by ChangesetView.
- Korean labels throughout; log cap 500 retained.

## Verification contract

- vitest unit suites: state machine transitions (incl. reconnect mid-thinking and changeset persistence), changeset grouping/rendering logic, plan revision replacement, decision dispatch payloads, status header (elapsed-time formatting).
- `npm --prefix panel run build` exits 0; static contract test updated (target-picker/apply-bar components ABSENT guards replacing the v1 presence checks).
- Live E2E: the three v2 acceptance scenarios (see plan) drive this UI in the editor.

## Implementation

- `panel/src/ws/protocol.ts` / `client.ts` — WS v2 message types (v1 instruct/apply removed)
- `panel/src/state/store.ts` — state machine v2 + changeset/plan state
- `panel/src/components/` — InstructionBox (regated), AgentStream (new), PlanView (new), ChangesetView (new), Header (status visibility), ConversationLog (cards)
- `panel/src/lib/progress.ts` — extended labels (elapsed time)
- removed: `TargetPicker.tsx`, `ApplyBar.tsx`, ReviewTabs apply wiring
- external: served by `server/eud_agent/app.py`; protocol per [[features/05_agent-core|05_agent-core]] `05_agent-core.md`
