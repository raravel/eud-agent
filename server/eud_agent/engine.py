"""The v2 WS state machine + system-prompt builder (features/05 "WS protocol v2"
+ "Triage and plan gating").

The v1 ``orchestrator.py`` (instruct -> rag -> codex -> code event -> manual apply)
is RETIRED. The v2 flow is a SMALL deterministic state machine driven by WS events
(no LangChain, per features/05):

    idle -> triage -> answer | apply | plan_review* -> executing
         -> changeset_review -> idle

One :class:`AgentEngine` is created per panel WS connection. It owns the per-session
:class:`AgentRunner` (codex thread continuity) and routes the WS v2 messages:

  client -> server: ``chat`` / ``plan_feedback`` / ``plan_approve`` /
                    ``changeset_decision`` / ``cancel`` / ``reset`` /
                    ``status`` / ``list``
  server -> client: ``agent_event`` / ``answer`` / ``plan`` / ``changeset`` /
                    ``rollback_result`` / ``error`` / ``status`` / ``progress``

A turn runs in the background (the runner streams ``agent_event``s); on turn
completion the engine emits ``answer`` (answer-only), ``plan`` (propose_plan ended
the turn), or ``changeset`` (writes were journaled). ``plan_feedback`` /
``plan_approve`` RESUME the codex thread; ``plan_approve`` also lifts the mutation
gate via ``ToolLayer.approve_plan_for_request``. ``changeset_decision`` routes to
the journal: ``reject`` replays inverse ops -> ``rollback_result``; ``accept``
archives. ``cancel`` interrupts the in-flight turn safely (journal entries persist).

System prompt (first turn)
--------------------------
:func:`build_system_prompt` composes the tool catalog (``ToolLayer.tool_specs``),
the project state (bridge STATUS + LIST, best-effort), the first principles
(never-do crash causes, ahead of the RAG context so they outrank retrieved
examples), the RAG top-k context for
the user request (``rag.search``, degrading to none when unavailable), and the
triage instructions (answer-only -> no write tools; <=2 mutations -> apply
directly; larger -> ``propose_plan`` first). The same prompt drives the real
CodexSDKRunner and the test FakeRunner.
"""

from __future__ import annotations

import asyncio
import uuid
from pathlib import Path

from . import rag

# RAG retrieval depth for the system-prompt context (matches v1 orchestrator).
_RAG_K = 5

# First principles: community-verified crash/EUD-error/drop/freeze causes
# (Naver cafe edac/91492) the agent must NEVER trigger. Curated English text
# ships as package data; injected ahead of the RAG context so the never-do
# rules outrank retrieved examples.
_FIRST_PRINCIPLES_PATH = (
    Path(__file__).resolve().parent / "data" / "first_principles.md"
)

# Triage rules surfaced in the system prompt (features/05, mechanical not advisory).
_TRIAGE_INSTRUCTIONS = (
    "[triage]\n"
    "- Answer-only requests (questions, explanations): reply directly and use "
    "NO write tools.\n"
    "- Small edits (at most 2 mutations): you MAY apply them directly with the "
    "write tools.\n"
    "- Larger work (3+ mutations): you MUST call propose_plan(markdown) FIRST to "
    "outline the change for user review; only after the user approves the plan "
    "will the mutation gate lift. The 3rd mutating call without an approved plan "
    "is rejected."
)


# --------------------------------------------------------------------------- #
# System-prompt builder.
# --------------------------------------------------------------------------- #


def build_system_prompt(
    request_text: str,
    *,
    tool_layer,
    bridge,
    rag_db: str,
) -> str:
    """Compose the first-turn system prompt (tools + state + principles + RAG + triage).

    Best-effort throughout: a bridge/RAG failure degrades that section rather than
    failing the turn (the panel must stay responsive). Called from a worker thread
    (the bridge/RAG calls are synchronous).
    """
    parts: list[str] = [
        "You are the EUD Editor 3 agent. You edit a StarCraft EUD map project by "
        "calling the eud-tools below; the server validates, journals, and can roll "
        "back every change.",
        "",
        _tool_catalog_section(tool_layer),
        "",
        _project_state_section(bridge),
        "",
        _first_principles_section(),
        "",
        _rag_section(request_text, rag_db),
        "",
        _TRIAGE_INSTRUCTIONS,
    ]
    return "\n".join(parts)


def _first_principles_section() -> str:
    lines = ["[first principles]"]
    try:
        lines.append(_FIRST_PRINCIPLES_PATH.read_text(encoding="utf-8").strip())
    except OSError:  # noqa: B904 - best-effort: a missing data file degrades
        lines.append("(first principles unavailable)")
    return "\n".join(lines)


def _tool_catalog_section(tool_layer) -> str:
    lines = ["[tools]"]
    try:
        specs = tool_layer.tool_specs()
    except Exception:  # noqa: BLE001 - never fail the prompt on introspection
        specs = []
    for s in specs:
        lines.append(f"- {s['name']}: {s.get('description', '')}")
    return "\n".join(lines)


def _project_state_section(bridge) -> str:
    lines = ["[project state]"]
    try:
        status_reply = bridge.status()
        compiling, project = parse_status(status_reply)
        lines.append(f"project={project} compiling={str(compiling).lower()}")
    except Exception:  # noqa: BLE001 - best-effort state
        lines.append("project=(unknown)")
    try:
        files = bridge.list_files()
        if files:
            lines.append("files:")
            for f in files[:200]:
                settable = " (settable)" if f.get("settable") else ""
                lines.append(f"  {f.get('path', '')} [{f.get('ftype', '')}]{settable}")
        else:
            lines.append("files: (none)")
    except Exception:  # noqa: BLE001 - best-effort state
        lines.append("files: (unavailable)")
    return "\n".join(lines)


def _rag_section(request_text: str, rag_db: str) -> str:
    lines = ["[reference context]"]
    try:
        results = rag.search(request_text, _RAG_K, rag_db=rag_db)
    except rag.RagUnavailable:
        lines.append("(no reference context available)")
        return "\n".join(lines)
    except Exception:  # noqa: BLE001 - any RAG failure degrades to none
        lines.append("(no reference context available)")
        return "\n".join(lines)
    chunks = [r.get("text", "") for r in results if r.get("text")]
    if not chunks:
        lines.append("(no reference context available)")
    else:
        lines.extend(chunks)
    return "\n".join(lines)


def parse_status(reply: str) -> tuple[bool, str]:
    """Parse a bridge STATUS reply (``compiling=.. / project=.. / version=..``).

    Tolerant: unknown/missing keys degrade to ``(False, "")``; ``compiling`` is
    true only for the literal ``true`` (case-insensitive). (Moved verbatim from
    the retired v1 orchestrator — the only piece app/state needs from it.)
    """
    compiling = False
    project = ""
    for line in reply.splitlines():
        key, sep, value = line.partition("=")
        if not sep:
            continue
        key = key.strip().lower()
        value = value.strip()
        if key == "compiling":
            compiling = value.lower() == "true"
        elif key == "project":
            project = value
    return compiling, project


# --------------------------------------------------------------------------- #
# The state machine.
# --------------------------------------------------------------------------- #


class AgentEngine:
    """Per-connection WS v2 state machine (features/05).

    States: ``idle`` (no turn), ``executing`` (a codex turn streams), ``plan_review``
    (a plan was proposed; awaiting feedback/approval), ``changeset_review`` (a turn
    finished with journaled writes; awaiting accept/reject). The machine is
    deterministic and small — message handlers check the state and route.

    ``send`` is the async broadcaster; ``make_runner`` builds the per-session runner
    at construction (the real CodexSDKRunner is internally lazy — it spawns codex
    only on the first turn — so eager construction costs nothing and lets the panel
    cancel before any chat). ``get_tool_layer`` returns the (possibly test-swapped)
    ToolLayer; ``bridge`` + ``rag_db`` feed the system prompt's state / RAG sections.
    """

    def __init__(
        self,
        *,
        send,
        make_runner,
        get_tool_layer,
        bridge,
        rag_db,
        register_request_id=None,
    ):
        self._send = send
        self._get_tool_layer = get_tool_layer
        self._bridge = bridge
        self._rag_db = rag_db
        # Live request-id stamping (EUD-064): the engine publishes its CURRENT
        # request id through this callback so the tool endpoint can stamp it onto
        # tool calls from this active session (overriding the stale shim env id).
        # ``None`` (no active session) restores the shim id as the fallback. A
        # no-op default keeps the engine usable without the registry (unit tests).
        self._register_request_id = register_request_id or (lambda _rid: None)

        self.state = "idle"
        self._runner = make_runner(
            tool_layer=get_tool_layer(),
            send=send,
            build_system_prompt=build_system_prompt,
        )
        self._request_id: str | None = None
        self._plan_revision = 0
        # The in-flight turn runs as a BACKGROUND task so the WS receive loop stays
        # free to accept cancel{} (and reconnect) while codex streams (a turn can
        # run for minutes). None when no turn is running.
        self._turn_task: asyncio.Task | None = None

    # ------------------------------------------------------------- dispatch
    async def handle(self, msg: dict) -> None:
        mtype = msg.get("type")
        handler = {
            "chat": self._on_chat,
            "plan_feedback": self._on_plan_feedback,
            "plan_approve": self._on_plan_approve,
            "changeset_decision": self._on_changeset_decision,
            "cancel": self._on_cancel,
            "reset": self._on_reset,
            "status": self._on_status,
            "list": self._on_list,
        }.get(mtype)
        if handler is None:
            await self._error(f"unknown message type: {mtype!r}")
            return
        await handler(msg)

    # ------------------------------------------------------------- emit
    async def _error(self, message: str) -> None:
        await self._send({"type": "error", "message": message})

    # ------------------------------------------------------------- chat
    async def _on_chat(self, msg: dict) -> None:
        if self.state in ("executing", "plan_review"):
            await self._error("busy: a turn is already in flight")
            return
        text = str(msg.get("text", ""))
        # A new chat opens a fresh changeset scope. Any prior request whose
        # changeset was left UNDECIDED (the panel moved on without accept/reject)
        # is finalized first: undecided items DEFAULT to accepted and the journal
        # is archived with a note (features/05 line 45). Done before minting the
        # new request_id so the prior live journal never leaks.
        await self._finalize_prior_request()
        self._request_id = f"req-{uuid.uuid4().hex[:8]}"
        self._plan_revision = 0
        # Publish the live request id so the tool endpoint stamps it onto this
        # session's tool calls (the shim env id is pinned at thread creation and
        # goes stale from the second chat — EUD-064).
        self._register_request_id(self._request_id)
        runner = self._runner
        # Conversation continuity (EUD-064): the FIRST chat STARTS the codex thread
        # (system prompt as base_instructions); every later chat RESUMES the
        # retained thread so codex keeps its message + tool-call history. A resumed
        # chat carries no base_instructions, so the refreshed [project state] +
        # [reference context] (RAG for the NEW question) are PREPENDED to the turn
        # text instead (reusing build_system_prompt's section builders).
        # Defensive access: a runner predating the continuity interface (some unit
        # test fakes) has no has_thread -> treat as "no thread" so it keeps the old
        # always-start behavior. The real CodexSDKRunner + the v2 FakeRunner expose
        # has_thread/reset_thread (AgentRunner ABC, EUD-064).
        if getattr(self._runner, "has_thread", lambda: False)():
            turn_text = await asyncio.to_thread(self._resume_turn_text, text)
            self._launch_turn(
                runner.resume_turn(turn_text, request_id=self._request_id)
            )
            return
        system_prompt = await asyncio.to_thread(
            build_system_prompt,
            text,
            tool_layer=self._get_tool_layer(),
            bridge=self._bridge,
            rag_db=self._rag_db,
        )
        self._launch_turn(
            runner.start_turn(
                text, request_id=self._request_id, system_prompt=system_prompt
            )
        )

    def _resume_turn_text(self, text: str) -> str:
        """Refreshed [project state] + [reference context] prepended to ``text``.

        A resumed chat gets no ``base_instructions`` (those exist only on the first
        thread), so the current project state and the RAG context for the NEW
        question are prepended ahead of the original user text. Reuses the same
        section builders ``build_system_prompt`` uses. Called from a worker thread
        (the bridge/RAG calls are synchronous, best-effort).
        """
        return "\n".join([
            _project_state_section(self._bridge),
            "",
            _rag_section(text, self._rag_db),
            "",
            text,
        ])

    # ------------------------------------------------------------- plan flow
    async def _on_plan_feedback(self, msg: dict) -> None:
        if self.state != "plan_review":
            await self._error("no plan awaiting feedback")
            return
        text = str(msg.get("text", ""))
        self._launch_turn(
            self._runner.resume_turn(
                f"[plan feedback] {text}", request_id=self._request_id
            )
        )

    async def _on_plan_approve(self, msg: dict) -> None:
        if self.state != "plan_review":
            await self._error("no plan awaiting approval")
            return
        # Lift the mutation gate for this request, THEN resume the thread so codex
        # proceeds to apply the approved plan (features/05).
        self._get_tool_layer().approve_plan_for_request(self._request_id)
        self._launch_turn(
            self._runner.resume_turn(
                "[plan approved] The plan is approved; apply it now.",
                request_id=self._request_id,
            )
        )

    # ------------------------------------------------------------- turn launch
    def _launch_turn(self, turn_coro) -> None:
        """Run ``turn_coro`` (a runner.start_turn/resume_turn awaitable) in the
        background so the WS receive loop stays free for cancel/reconnect.

        Moves to ``executing`` immediately; on completion the task calls
        ``_finish_turn`` (or surfaces an error). A cancelled/failed turn returns to
        ``idle`` without stranding the connection (the journal entries persist).
        """
        self.state = "executing"

        async def _runner_wrapper() -> None:
            try:
                result = await turn_coro
            except asyncio.CancelledError:
                self.state = "idle"
                raise
            except Exception as exc:  # noqa: BLE001 - surface, never crash the WS
                self.state = "idle"
                await self._error(f"agent turn failed: {exc}")
                return
            await self._finish_turn(result)

        self._turn_task = asyncio.create_task(_runner_wrapper())

    # ------------------------------------------------------------- turn end
    async def _finish_turn(self, result: dict) -> None:
        kind = (result or {}).get("kind", "answer")
        if kind == "plan":
            self._plan_revision += 1
            self.state = "plan_review"
            await self._send({
                "type": "plan",
                "markdown": result.get("markdown", ""),
                "revision": self._plan_revision,
            })
            return
        if kind == "apply":
            await self._emit_changeset()
            return
        # answer-only: nothing journaled, no changeset; back to idle.
        self.state = "idle"

    async def _emit_changeset(self) -> None:
        journal = self._get_tool_layer().get_journal(self._request_id)
        if journal is None:
            self.state = "idle"
            return
        changeset = journal.changeset()
        if not changeset.get("items"):
            # A turn that journaled nothing reviewable -> no changeset, idle.
            self.state = "idle"
            return
        self.state = "changeset_review"
        await self._send({"type": "changeset", **changeset})

    async def _finalize_prior_request(self) -> None:
        """Default-accept + archive a prior request's UNDECIDED changeset.

        Only the ``changeset_review`` state can hold a live journal the panel never
        decided on (accept/reject both transition to ``idle`` and archive/decide).
        When a new ``chat`` arrives in that state, features/05 line 45 says the
        undecided items default to accepted and the journal is archived with a
        note. The runner's live turn (if any) is already done — ``chat`` is gated
        out of ``executing``/``plan_review`` — so this is safe to run inline.
        """
        if self.state != "changeset_review" or self._request_id is None:
            return
        journal = self._get_tool_layer().get_journal(self._request_id)
        if journal is None:
            return
        await asyncio.to_thread(
            journal.finalize,
            note="superseded by a new request; undecided items default to accepted",
        )

    # ------------------------------------------------------------- changeset
    async def _on_changeset_decision(self, msg: dict) -> None:
        if self.state != "changeset_review":
            await self._error("no changeset awaiting a decision")
            return
        decision = str(msg.get("decision", ""))
        ids_arg = msg.get("ids")
        want_all = ids_arg == "all" or ids_arg is None
        ids = None if want_all else list(ids_arg)
        journal = self._get_tool_layer().get_journal(self._request_id)
        if journal is None:
            await self._error("no journal for this request")
            self.state = "idle"
            return
        if decision == "reject":
            result = await asyncio.to_thread(
                journal.rollback, ids=ids, all=want_all
            )
            ok = all(item.get("ok") for item in result.get("items", []))
            await self._send({
                "type": "rollback_result",
                "ids": [item["id"] for item in result.get("items", [])],
                "ok": ok,
            })
        elif decision == "accept":
            await asyncio.to_thread(journal.accept, ids=ids, all=want_all)
            await self._send({
                "type": "rollback_result",
                "ids": ids or [],
                "ok": True,
            })
        else:
            await self._error(f"unknown changeset decision: {decision!r}")
            return
        self.state = "idle"

    # ------------------------------------------------------------- cancel
    async def _on_cancel(self, msg: dict) -> None:
        self._runner.cancel()
        # The runner's cancel interrupts the live turn; the journal entries
        # already written PERSIST by design (the user still reviews them).

    # ------------------------------------------------------------- reset
    async def _on_reset(self, msg: dict) -> None:
        """Drop the retained codex thread so the next chat starts fresh (EUD-064).

        Semantics (features/05 "Request scoping across a continuous thread"):

          * ``executing`` / ``plan_review`` (a turn is in flight) -> error; the
            user must ``cancel`` first (resetting mid-turn would strand the runner).
          * ``changeset_review`` (a prior changeset is UNDECIDED) -> finalize it
            first (undecided items default to accepted, journal archived with a
            note) exactly like a new ``chat`` does, THEN drop the thread.
          * ``idle`` -> just drop the thread (idempotent: a no-thread reset is a
            harmless no-op).

        Dropping the thread also clears the published live request id (no active
        request until the next chat) and returns the engine to ``idle``.
        """
        if self.state in ("executing", "plan_review"):
            await self._error("busy: cancel the in-flight turn before reset")
            return
        await self._finalize_prior_request()
        reset_thread = getattr(self._runner, "reset_thread", None)
        if callable(reset_thread):
            reset_thread()
        self._request_id = None
        self._plan_revision = 0
        self._register_request_id(None)
        self.state = "idle"

    async def aclose(self) -> None:
        """A WS reconnect/close cancels the in-flight turn and reaps its task.

        Calls the runner's cancel (so the real CodexSDKRunner interrupts the live
        SDK turn) THEN cancels + awaits the background turn task so it does not
        linger past the connection (rules.md: the server must stay tidy; an
        orphaned task otherwise stalls the WS teardown). Journal entries already
        written persist by design.
        """
        self._runner.cancel()
        # The session is gone: stop stamping its request id onto tool calls so a
        # later headless call falls back to the shim id (EUD-064).
        self._register_request_id(None)
        task = self._turn_task
        self._turn_task = None
        if task is not None and not task.done():
            task.cancel()
            try:
                await task
            except (asyncio.CancelledError, Exception):  # noqa: BLE001
                pass

    # ------------------------------------------------------------- status/list
    async def _on_status(self, msg: dict) -> None:
        try:
            reply = await asyncio.to_thread(self._bridge.status)
        except Exception as exc:  # noqa: BLE001 - surface as a clean error
            await self._error(_bridge_error_message(exc))
            return
        compiling, project = parse_status(reply)
        await self._send(
            {"type": "status", "compiling": compiling, "project": project}
        )

    async def _on_list(self, msg: dict) -> None:
        try:
            files = await asyncio.to_thread(self._bridge.list_files)
        except Exception as exc:  # noqa: BLE001 - surface as a clean error
            await self._error(_bridge_error_message(exc))
            return
        await self._send({"type": "list", "files": files})


def _bridge_error_message(exc: Exception) -> str:
    """A panel-facing message for a bridge failure (busy vs other)."""
    from .bridge_io import BridgeBusy

    if isinstance(exc, BridgeBusy):
        return "editor busy"
    return str(exc)
