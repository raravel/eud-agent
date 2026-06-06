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
examples), the per-map-project memory (features/07, between the first principles
and the RAG context so project-specific truth outranks generic examples but the
never-do rules outrank both), the RAG top-k context for
the user request (``rag.search``, degrading to none when unavailable), and the
triage instructions (answer-only -> no write tools; <=2 mutations -> apply
directly; larger -> ``propose_plan`` first). The same prompt drives the real
CodexSDKRunner and the test FakeRunner.
"""

from __future__ import annotations

import asyncio
import logging
import uuid
from datetime import datetime
from pathlib import Path

from . import rag
from .memory import ProjectMemory

_log = logging.getLogger(__name__)

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
    data_dir=None,
) -> str:
    """Compose the first-turn system prompt (tools + state + principles + memory
    + RAG + triage).

    Best-effort throughout: a bridge/RAG failure degrades that section rather than
    failing the turn (the panel must stay responsive). Called from a worker thread
    (the bridge/RAG calls are synchronous).

    ``data_dir`` (features/07) enables the ``[project memory]`` section, placed
    BETWEEN ``[first principles]`` and ``[reference context]`` (project-specific
    truth outranks generic RAG; never-do rules outrank both). The project name is
    REUSED from the same STATUS fetch that builds ``[project state]`` (no second
    round-trip). ``None`` (the default) preserves callers that predate the memory
    seam (app.py wires it in EUD-081).
    """
    state_section, project = _fetch_project_state(bridge)
    parts: list[str] = [
        "You are the EUD Editor 3 agent. You edit a StarCraft EUD map project by "
        "calling the eud-tools below; the server validates, journals, and can roll "
        "back every change.",
        "",
        _tool_catalog_section(tool_layer),
        "",
        state_section,
        "",
        _first_principles_section(),
    ]
    if data_dir is not None:
        parts += ["", _project_memory_section(data_dir, project)]
    parts += [
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


def _fetch_project_state(bridge) -> tuple[str, str]:
    """Build the ``[project state]`` section AND resolve the project name in one
    STATUS+LIST fetch.

    Returns ``(section_text, project_name)``. The project name (parsed from the
    SAME STATUS reply, features/07) is reused by the ``[project memory]`` section
    so memory never costs a second bridge round-trip. Best-effort: a STATUS/LIST
    failure degrades that line and yields an EMPTY project name (-> the memory
    store degrades to ``(no project memory)``).
    """
    lines = ["[project state]"]
    project = ""
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
    return "\n".join(lines), project


def _project_state_section(bridge) -> str:
    return _fetch_project_state(bridge)[0]


def _project_memory_section(data_dir, project_name: str) -> str:
    """Build the ``[project memory]`` section for ``project_name`` (features/07).

    Rendered via :meth:`ProjectMemory.render_section`. The store self-degrades to
    ``[project memory]\\n(no project memory)`` for an empty/unknown project; any
    unexpected error here is swallowed to the same degraded body so memory NEVER
    blocks the turn (the same best-effort contract as RAG). ``list_reply`` is
    omitted: the prompt assembly only has the PARSED file list, not the raw LIST
    reply the staleness hash is computed from, and reconstructing it would risk a
    spurious "outdated" suffix — staleness is refreshed via ``memory_write`` on
    ``structure`` instead (features/07 "Prompt injection").
    """
    try:
        return ProjectMemory(
            data_dir=data_dir, project_name=project_name
        ).render_section()
    except Exception:  # noqa: BLE001 - best-effort: never block the turn
        _log.warning("project memory section render failed", exc_info=True)
        return "[project memory]\n(no project memory)"


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


def _journal_tools_and_files(journal) -> tuple[list[str], list[str]]:
    """Distinct journaled tool names + entry targets for an episode (features/07).

    ``journal`` is a :class:`eud_agent.journal.Journal` (or ``None`` for an
    answer-only turn). Both lists preserve first-seen order and drop duplicates;
    a journal without entries (or ``None``) yields two empty lists. Best-effort:
    any introspection failure degrades to empties rather than breaking finalization.
    """
    if journal is None:
        return [], []
    tools: list[str] = []
    files: list[str] = []
    try:
        entries = list(journal.entries)
    except Exception:  # noqa: BLE001 - never break finalization on introspection
        return [], []
    for e in entries:
        tool = getattr(e, "tool", None)
        if tool and tool not in tools:
            tools.append(tool)
        target = getattr(e, "target", None)
        if target and target not in files:
            files.append(target)
    return tools, files


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
        data_dir=None,
    ):
        self._send = send
        self._get_tool_layer = get_tool_layer
        self._bridge = bridge
        self._rag_db = rag_db
        # Per-map-project memory root (features/07). ``None`` disables both the
        # refreshed [project memory] in resumed turns and episode recording — the
        # engine stays usable for unit tests that predate the memory seam (app.py
        # wires data_dir in EUD-081). Project name is re-resolved per turn from the
        # bridge STATUS so a mid-session project switch follows on the next chat.
        self._data_dir = data_dir
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
        # The current request's chat text, retained for the episode ``instruction``
        # (features/07 Episodes — first 200 chars). Spans the whole request: a
        # default-accept of the PRIOR request fires before the new chat overwrites
        # it, so each finalization sees its own request's text.
        self._chat_text: str = ""
        # The in-flight turn runs as a BACKGROUND task so the WS receive loop stays
        # free to accept cancel{} (and reconnect) while codex streams (a turn can
        # run for minutes). None when no turn is running.
        self._turn_task: asyncio.Task | None = None
        # A changeset decision ALSO runs as a background task (EUD-070): a
        # rollback replays inverse ops over the 1s-tick file IPC (2-4s for a
        # 3-property dat group in the live E2E), and awaiting it inline blocked
        # the WS receive loop — every other click queued behind it.
        self._decision_task: asyncio.Task | None = None

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
        # EUD-070: a chat arriving during an in-flight changeset decision must
        # not race the journal — drain the decision first (it finishes with its
        # rollback_result), then proceed with the new turn.
        if self._decision_task is not None and not self._decision_task.done():
            await self._decision_task
        text = str(msg.get("text", ""))
        # A new chat opens a fresh changeset scope. Any prior request whose
        # changeset was left UNDECIDED (the panel moved on without accept/reject)
        # is finalized first: undecided items DEFAULT to accepted and the journal
        # is archived with a note (features/05 line 45). Done before minting the
        # new request_id so the prior live journal never leaks.
        await self._finalize_prior_request()
        self._request_id = f"req-{uuid.uuid4().hex[:8]}"
        self._plan_revision = 0
        # Retain AFTER finalizing the prior request (its episode used the OLD
        # text) so this request's episode carries its own instruction (features/07).
        self._chat_text = text
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
        """Refreshed [project state] + [project memory] + [reference context]
        prepended to ``text``.

        A resumed chat gets no ``base_instructions`` (those exist only on the first
        thread), so the current project state, the per-map-project memory, and the
        RAG context for the NEW question are prepended ahead of the original user
        text. Memory changes between chats, so it is refreshed every turn; the
        project name is re-resolved from the SAME STATUS fetch (features/07 — a
        mid-session project switch follows on this turn). Reuses the same section
        builders ``build_system_prompt`` uses. Called from a worker thread (the
        bridge/RAG calls are synchronous, best-effort).
        """
        state_section, project = _fetch_project_state(self._bridge)
        parts = [state_section, ""]
        if self._data_dir is not None:
            parts += [_project_memory_section(self._data_dir, project), ""]
        parts += [
            _rag_section(text, self._rag_db),
            "",
            text,
        ]
        return "\n".join(parts)

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
        # answer-only: nothing journaled, no changeset; back to idle. Record the
        # turn as an episode (features/07 — answer-only finalization).
        # State -> idle BEFORE recording the episode: the episode write is a
        # best-effort, zero-cost finalization that must NEVER gate the state
        # machine (features/07 "memory must never break the request flow"). It
        # runs via asyncio.to_thread, which yields the event loop; if state were
        # still "executing" across that yield, the client's next chat (already
        # holding the runner-emitted answer) would race in and hit the busy guard.
        self.state = "idle"
        await self._record_episode(kind="answer", decision="answer", journal=None)

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
        # features/07 Episodes: a default-accept of undecided items records a
        # ``defaulted`` episode (the journal entries are still readable post-archive).
        # No state reorder here (unlike _finish_turn / _on_changeset_decision): this
        # runs INLINE inside _on_chat (and _on_reset) BEFORE the new request id is
        # minted, with no state transition coupled to the episode write — the caller
        # owns the subsequent transition. The episode `to_thread` yield is harmless
        # because the caller has already passed its busy gate and has not yet started
        # the new turn. (Audited EUD-081: no race window at this site.)
        await self._record_episode(
            kind="changeset", decision="defaulted", journal=journal
        )

    # ------------------------------------------------------------- episodes
    async def _record_episode(self, *, kind: str, decision: str, journal) -> None:
        """Append ONE request-history episode to the project's ``episodes.jsonl``.

        features/07 "Episodes" — server-written, zero token cost. Called at every
        finalization point (answer-only end, changeset accept/reject/partial,
        default-accept of a prior undecided changeset). The episode is recorded
        ONLY when a project name is known (the store self-disables otherwise) and
        when a memory root is configured (``data_dir``). ``journal`` supplies the
        distinct journaled tool names + file targets (``None`` for an answer-only
        turn -> empty lists).

        Best-effort throughout: a missing data_dir, an empty project, or ANY error
        (STATUS fetch, journal read, append) is logged and SWALLOWED — memory must
        NEVER break the request flow (RAG-style degradation). Runs the synchronous
        bridge/disk IO off the event loop via ``asyncio.to_thread`` like the
        surrounding finalization code.
        """
        if self._data_dir is None:
            return
        try:
            await asyncio.to_thread(
                self._append_episode, kind, decision, journal,
                self._request_id, self._chat_text,
            )
        except Exception:  # noqa: BLE001 - best-effort: never break the request
            _log.warning("episode recording failed", exc_info=True)

    def _append_episode(
        self, kind: str, decision: str, journal, request_id, chat_text: str
    ) -> None:
        """The synchronous body of :meth:`_record_episode` (runs in a worker).

        Resolves the project name from a fresh bridge STATUS (a mid-session
        project switch is honored), builds the episode dict, and appends it. The
        store's :meth:`ProjectMemory.append_episode` already swallows IO failures;
        an empty/unknown project disables the store so no file is written.
        """
        try:
            _, project = parse_status(self._bridge.status())
        except Exception:  # noqa: BLE001 - no project resolvable -> skip episode
            return
        store = ProjectMemory(data_dir=self._data_dir, project_name=project)
        if not store.enabled:
            return
        tools, files = _journal_tools_and_files(journal)
        episode = {
            "ts": datetime.now().isoformat(timespec="seconds"),
            "request_id": request_id,
            "instruction": (chat_text or "")[:200],
            "kind": kind,
            "tools": tools,
            "files": files,
            "decision": decision,
        }
        store.append_episode(episode)

    # ------------------------------------------------------------- changeset
    async def _on_changeset_decision(self, msg: dict) -> None:
        if self.state != "changeset_review":
            await self._error("no changeset awaiting a decision")
            return
        # EUD-070: one decision at a time — the panel locks its controls until
        # the rollback_result lands, so a second decision here is a protocol
        # violation (e.g. a second client), not a queueing request.
        if self._decision_task is not None and not self._decision_task.done():
            await self._error("busy: a decision is already in flight")
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
        if decision not in ("reject", "accept"):
            await self._error(f"unknown changeset decision: {decision!r}")
            return

        # EUD-070: the journal work is blocking file IPC (a rollback waits on
        # the 1s bridge tick PER inverse op — 2-4s for a 3-property dat group in
        # the live E2E). Run it as a BACKGROUND task so the WS receive loop
        # stays free; the state leaves changeset_review only when it completes.
        async def _decide() -> None:
            try:
                if decision == "reject":
                    result = await asyncio.to_thread(
                        journal.rollback, ids=ids, all=want_all
                    )
                    ok = all(
                        item.get("ok") for item in result.get("items", [])
                    )
                    await self._send({
                        "type": "rollback_result",
                        "ids": [
                            item["id"] for item in result.get("items", [])
                        ],
                        "ok": ok,
                    })
                else:
                    await asyncio.to_thread(
                        journal.accept, ids=ids, all=want_all
                    )
                    await self._send({
                        "type": "rollback_result",
                        "ids": ids or [],
                        "ok": True,
                    })
                # features/07 Episodes: record the decision. A whole-changeset
                # accept/reject is ``accepted``/``rejected``; a partial (subset of
                # ids) decision is ``partial``.
                if not want_all:
                    ep_decision = "partial"
                else:
                    ep_decision = "accepted" if decision == "accept" else "rejected"
                # State -> idle BEFORE recording the episode: same rule as the
                # answer-only finalization above — the episode write is a
                # best-effort, zero-cost step that must NEVER gate the state
                # machine (features/07 "memory must never break the request
                # flow"). It runs via asyncio.to_thread (yields the loop); leaving
                # state at "changeset_review" across that yield would let the next
                # client message race the decided changeset. The rollback_result
                # was already sent, so the panel can proceed once idle.
                self.state = "idle"
                await self._record_episode(
                    kind="changeset", decision=ep_decision, journal=journal
                )
            except Exception as exc:  # noqa: BLE001 - surface, never crash the WS
                await self._error(f"changeset decision failed: {exc}")
                self.state = "idle"

        self._decision_task = asyncio.create_task(_decide())

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
        for attr in ("_turn_task", "_decision_task"):
            task = getattr(self, attr)
            setattr(self, attr, None)
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
