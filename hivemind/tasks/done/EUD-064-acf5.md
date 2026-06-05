---
completed_at: '2026-06-05T17:12:11.625799'
created: '2026-06-05'
depends_on:
- EUD-062-644f
id: EUD-064-acf5
parent: EUD-047-fd09
priority: high
scope:
- server/eud_agent/engine.py
- server/eud_agent/agent_runner.py
- server/eud_agent/app.py
- server/eud_agent/tools.py
- server/eud_agent/mcp_shim.py
- server/tests/test_agent_flow.py
- server/tests/test_app.py
- server/tests/test_integration_ws.py
status: done
title: 'Conversation continuity: resume the codex thread across chats; reset{}; live
  request-id stamping'
type: bug
updated: '2026-06-05'
---

## Bug report (user, live panel observation)

"각 메시지마다 이전 메시지 히스토리를 안 가지고 있어서 내가 방금 전에 무엇을 얘기했는데도 까먹는다" — the agent forgets the previous message. Root cause (confirmed in source):

- `engine.py` `_on_chat` (line ~250) calls `runner.start_turn(...)` for EVERY chat; `CodexSDKRunner._run_turn_blocking` with `resume=False` unconditionally calls `codex.thread_start()` (agent_runner.py ~line 219, the `resume and self._thread_id` guard only triggers from `resume_turn`). Every user message gets a brand-new codex thread: message AND tool-call history are lost.
- Only `plan_feedback`/`plan_approve` use `resume_turn` — which is why plan iteration is the single flow that "remembers".
- features/05 always PROMISED continuity ("thread id retained per panel session for follow-up turns"); the spec was amended 2026-06-05 to make the semantics binding (see Spec References).

## Fix design (user-approved Option A, 2026-06-05)

1. **Thread continuity** — engine: the FIRST `chat` of a session starts the thread (system prompt as `base_instructions`); every subsequent `chat` resumes it. Runner: `start_turn` may stay as-is; the engine decides start-vs-resume (e.g. the runner exposes `has_thread` or the engine tracks first-chat). Resumed chats PREPEND refreshed `[project state]` + `[reference context]` (RAG for the new text) to the turn text — `base_instructions` exist only on the first thread (reuse the section builders from `build_system_prompt`).
2. **`reset{}` WS message** — drops the retained thread id (next chat starts fresh). Arriving in `changeset_review`, it finalizes the prior request first (undecided default-accept + archive) exactly like `_on_chat` does; rejected with an error while a turn is in flight (`executing`). WS reconnect keeps its existing drop-everything behavior.
3. **Live request-id stamping** — each `chat` still mints a fresh `request_id` (journal/changeset scope, mutation gate, 30-action budget all stay per-request). The shim env `EUD_REQUEST_ID` is pinned at thread creation and goes stale from the second chat on. Fix: the tool endpoint (app.py/tools.py) stamps the engine's CURRENT request id onto every tool call from an active panel session, ignoring the shim-supplied id; the shim id remains a fallback only when no session is active (legacy headless runner).

## Completion criteria

- [ ] Second (and later) `chat` resumes the SAME codex thread — FakeRunner asserts resume_turn called with the prior thread retained, not start_turn (and the real runner path is covered by a thread-id retention test).
- [ ] Resumed chats carry refreshed project-state + RAG context prepended to the turn text (test asserts both section markers present and the original user text intact).
- [ ] `reset{}` drops the thread (next chat → start_turn with a NEW system prompt); reset in `changeset_review` finalizes the prior journal (default-accept + archive note); reset during `executing` → error{}; reset is otherwise idempotent.
- [ ] Tool calls during the Nth chat journal under the Nth `request_id` regardless of the stale shim env id (integration-style test through the tool endpoint with an active session).
- [ ] Mutation gate + action budget reset per request as before (regression asserts).
- [ ] Full server suite (pytest) + ruff green.

## Spec References

- [[features/05_agent-core|05_agent-core]] — "Conversation continuity" engine bullet + "Request scoping across a continuous thread" section + `reset{}` in WS protocol v2 (amended 2026-06-05)

## Notes

- The EUD-062 isolation layers are untouched; the agent_runner docstring premise "a new chat starts a fresh thread, so the shim re-spawns and re-reads the env" must be UPDATED to the new continuity semantics (the per-thread env request id is exactly what goes stale — that is why stamping moves server-side).
- No codex token consumption: FakeRunner / fake-SDK objects only.
- Panel-side `reset` button + protocol.ts type are EUD-065 (do not touch panel/ here).