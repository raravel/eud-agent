"""Verification artifact for EUD-056-5ca7: v2 engine + runner UNIT tests.

The v1 ``orchestrator.py`` (instruct -> rag -> codex -> code event -> manual apply)
is RETIRED; this suite — which used to test it — is migrated to UNIT-test the v2
replacements directly (no WS, no codex):

  * ``engine.parse_status`` — the tolerant STATUS-reply parser (moved verbatim
    from the retired orchestrator).
  * ``engine.build_system_prompt`` — the first-turn prompt carries the tool
    catalog, project state (bridge STATUS + LIST, best-effort), the RAG context
    (degrading to none on RagUnavailable), and the triage instructions.
  * ``engine.AgentEngine`` state machine driven by WS messages against a fake
    runner + fake bridge: chat->answer (idle), chat->plan (plan_review),
    plan_approve resumes + lifts the gate, apply-turn emits changeset, and the
    unknown-type error path.
  * ``agent_runner._classify_event`` — the streamed-SDK event classifier
    (tolerant of model variance; detects propose_plan / mutation / answer text).
"""

from __future__ import annotations

import types

import pytest

from eud_agent import rag as rag_mod
from eud_agent.engine import AgentEngine, build_system_prompt, parse_status
from eud_agent.rag import RagUnavailable

# --------------------------------------------------------------------------- #
# Fakes.
# --------------------------------------------------------------------------- #


class FakeBridge:
    """Instant bridge stand-in (status/list + dat journal seams)."""

    def __init__(self, *, status="compiling=false\nproject=demo\n", files=None):
        self._status = status
        self._files = files if files is not None else [
            {"path": "main.eps", "ftype": "CUIEps", "settable": True},
        ]

    def status(self, **kw):
        return self._status

    def list_files(self, **kw):
        return self._files

    def getdat(self, dat, param, obj_id):
        return f"OK: {dat} {param} {obj_id} = 7"

    def setdat(self, *a):
        return "OK"


class FakeToolLayer:
    """Minimal ToolLayer surface the engine + prompt builder use."""

    def __init__(self):
        self.approved: list[str] = []
        self._journal = None

    def tool_specs(self):
        return [
            {"name": "list_files", "description": "list project files"},
            {"name": "dat_set", "description": "write a dat field"},
            {"name": "propose_plan", "description": "propose a plan; ends turn"},
        ]

    def approve_plan_for_request(self, request_id):
        self.approved.append(request_id)

    def set_journal(self, journal):
        self._journal = journal

    def get_journal(self, request_id):
        return self._journal


class FakeJournal:
    def __init__(self, items=None):
        self._items = items if items is not None else []
        self.rolled_back = False
        self.accepted = False
        self.finalized_note = None

    def changeset(self):
        return {"request_id": "req", "items": self._items}

    def rollback(self, *, ids=None, all=False):
        self.rolled_back = True
        return {"request_id": "req",
                "items": [{"id": "e1", "ok": True}]}

    def accept(self, *, ids=None, all=False):
        self.accepted = True

    def finalize(self, *, note=None):
        self.finalized_note = note


class FakeRunner:
    def __init__(self, *, tool_layer, send, build_system_prompt):
        self.tool_layer = tool_layer
        self._send = send
        self.captured_system_prompt = None
        self.cancelled = False
        self.scripts: list = []

    def queue(self, script):
        self.scripts.append(script)

    async def start_turn(self, text, *, request_id, system_prompt):
        self.captured_system_prompt = system_prompt
        return await self._run(request_id)

    async def resume_turn(self, text, *, request_id):
        return await self._run(request_id)

    async def _run(self, request_id):
        if not self.scripts:
            return {"kind": "answer"}
        return await self.scripts.pop(0)(self._send, self.tool_layer, request_id)

    def cancel(self):
        self.cancelled = True


class Recorder:
    def __init__(self):
        self.events: list[dict] = []

    async def __call__(self, ev):
        self.events.append(ev)

    def first(self, etype):
        for e in self.events:
            if e.get("type") == etype:
                return e
        raise AssertionError(f"no {etype!r} in {[e.get('type') for e in self.events]}")

    def has(self, etype):
        return any(e.get("type") == etype for e in self.events)


def make_engine(*, bridge=None, tool_layer=None, recorder=None, rag_db="C:\\rag"):
    bridge = bridge or FakeBridge()
    tool_layer = tool_layer or FakeToolLayer()
    recorder = recorder or Recorder()
    created = {}

    def make_runner(*, tool_layer, send, build_system_prompt):
        r = FakeRunner(tool_layer=tool_layer, send=send,
                       build_system_prompt=build_system_prompt)
        created["runner"] = r
        return r

    engine = AgentEngine(
        send=recorder, make_runner=make_runner,
        get_tool_layer=lambda: tool_layer, bridge=bridge, rag_db=rag_db,
    )
    return engine, recorder, created, tool_layer


async def _drain(engine):
    """Await the in-flight turn task so the test sees the terminal event."""
    task = engine._turn_task
    if task is not None:
        await task


# --------------------------------------------------------------------------- #
# parse_status.
# --------------------------------------------------------------------------- #


def test_parse_status_compiling_and_project():
    assert parse_status("compiling=true\nproject=demoproj\n") == (True, "demoproj")
    assert parse_status("compiling=false\nproject=p\n") == (False, "p")
    # tolerant: unknown/missing keys degrade.
    assert parse_status("garbage\nversion=1.0\n") == (False, "")
    # compiling is true only for literal true (case-insensitive).
    assert parse_status("compiling=TRUE\n")[0] is True
    assert parse_status("compiling=yes\n")[0] is False


# --------------------------------------------------------------------------- #
# build_system_prompt.
# --------------------------------------------------------------------------- #


def test_system_prompt_has_catalog_state_triage(monkeypatch):
    monkeypatch.setattr(
        rag_mod, "search",
        lambda q, k=5, *, rag_db: [{"text": "RAGCHUNK"}],
    )
    sp = build_system_prompt(
        "make a thing", tool_layer=FakeToolLayer(), bridge=FakeBridge(),
        rag_db="C:\\rag",
    )
    assert "list_files" in sp and "dat_set" in sp and "propose_plan" in sp
    assert "demo" in sp  # project state
    assert "main.eps" in sp  # file listing
    assert "RAGCHUNK" in sp  # RAG context
    assert "triage" in sp.lower()


def test_system_prompt_rag_unavailable_degrades(monkeypatch):
    def boom(q, k=5, *, rag_db):
        raise RagUnavailable("db gone")

    monkeypatch.setattr(rag_mod, "search", boom)
    sp = build_system_prompt(
        "x", tool_layer=FakeToolLayer(), bridge=FakeBridge(), rag_db="C:\\rag",
    )
    # Degrades to a no-context section, never raises.
    assert "no reference context" in sp.lower()
    assert "list_files" in sp  # the rest of the prompt still built


def test_system_prompt_bridge_failure_degrades(monkeypatch):
    monkeypatch.setattr(rag_mod, "search", lambda *a, **k: [])

    class BadBridge:
        def status(self):
            raise RuntimeError("bridge down")

        def list_files(self):
            raise RuntimeError("bridge down")

    sp = build_system_prompt(
        "x", tool_layer=FakeToolLayer(), bridge=BadBridge(), rag_db="C:\\rag",
    )
    assert "unknown" in sp.lower() or "unavailable" in sp.lower()


# --------------------------------------------------------------------------- #
# AgentEngine state machine.
# --------------------------------------------------------------------------- #


async def test_chat_answer_returns_to_idle(monkeypatch):
    monkeypatch.setattr(rag_mod, "search", lambda *a, **k: [])
    engine, rec, created, _ = make_engine()

    async def script(send, tools, request_id):
        await send({"type": "answer", "text": "hi"})
        return {"kind": "answer"}

    # The engine builds the runner eagerly, so queue the script THEN chat.
    created["runner"].queue(script)
    await engine.handle({"type": "chat", "text": "hello"})
    await _drain(engine)
    assert rec.first("answer")["text"] == "hi"
    assert engine.state == "idle"


async def test_chat_plan_enters_plan_review(monkeypatch):
    monkeypatch.setattr(rag_mod, "search", lambda *a, **k: [])
    engine, rec, created, _ = make_engine()

    async def plan(send, tools, request_id):
        return {"kind": "plan", "markdown": "# P"}

    created["runner"].queue(plan)
    await engine.handle({"type": "chat", "text": "big"})
    await _drain(engine)
    plan_ev = rec.first("plan")
    assert plan_ev["markdown"] == "# P"
    assert plan_ev["revision"] == 1
    assert engine.state == "plan_review"


async def test_plan_approve_lifts_gate_and_resumes(monkeypatch):
    monkeypatch.setattr(rag_mod, "search", lambda *a, **k: [])
    tl = FakeToolLayer()
    tl.set_journal(FakeJournal(items=[{"category": "dat", "id": "e1"}]))
    engine, rec, created, _ = make_engine(tool_layer=tl)

    async def plan(send, tools, request_id):
        return {"kind": "plan", "markdown": "# P"}

    async def apply(send, tools, request_id):
        return {"kind": "apply"}

    created["runner"].queue(plan)
    await engine.handle({"type": "chat", "text": "big"})
    await _drain(engine)
    assert engine.state == "plan_review"

    created["runner"].queue(apply)
    await engine.handle({"type": "plan_approve"})
    await _drain(engine)
    # The mutation gate was lifted for this request.
    assert tl.approved, "plan_approve must lift the mutation gate"
    # The apply turn produced a changeset (journal had items).
    assert rec.has("changeset")
    assert engine.state == "changeset_review"


async def test_changeset_reject_rolls_back(monkeypatch):
    monkeypatch.setattr(rag_mod, "search", lambda *a, **k: [])
    journal = FakeJournal(items=[{"category": "dat", "id": "e1"}])
    tl = FakeToolLayer()
    tl.set_journal(journal)
    engine, rec, created, _ = make_engine(tool_layer=tl)

    async def apply(send, tools, request_id):
        return {"kind": "apply"}

    created["runner"].queue(apply)
    await engine.handle({"type": "chat", "text": "edit"})
    await _drain(engine)
    assert engine.state == "changeset_review"

    await engine.handle({"type": "changeset_decision", "decision": "reject",
                         "ids": "all"})
    assert journal.rolled_back is True
    rr = rec.first("rollback_result")
    assert rr["ok"] is True
    assert engine.state == "idle"


async def test_changeset_accept_archives(monkeypatch):
    monkeypatch.setattr(rag_mod, "search", lambda *a, **k: [])
    journal = FakeJournal(items=[{"category": "dat", "id": "e1"}])
    tl = FakeToolLayer()
    tl.set_journal(journal)
    engine, rec, created, _ = make_engine(tool_layer=tl)

    async def apply(send, tools, request_id):
        return {"kind": "apply"}

    created["runner"].queue(apply)
    await engine.handle({"type": "chat", "text": "edit"})
    await _drain(engine)

    await engine.handle({"type": "changeset_decision", "decision": "accept",
                         "ids": "all"})
    assert journal.accepted is True
    assert journal.rolled_back is False
    assert engine.state == "idle"


async def test_new_chat_finalizes_prior_undecided_changeset(monkeypatch):
    """features/05 line 45: a new chat while a prior changeset is UNDECIDED
    finalizes it (undecided -> accepted, archived with a note) before the new
    turn, instead of leaking the prior live journal."""
    monkeypatch.setattr(rag_mod, "search", lambda *a, **k: [])
    journal = FakeJournal(items=[{"category": "dat", "id": "e1"}])
    tl = FakeToolLayer()
    tl.set_journal(journal)
    engine, rec, created, _ = make_engine(tool_layer=tl)

    async def apply(send, tools, request_id):
        return {"kind": "apply"}

    async def answer(send, tools, request_id):
        await send({"type": "answer", "text": "next"})
        return {"kind": "answer"}

    created["runner"].queue(apply)
    await engine.handle({"type": "chat", "text": "edit"})
    await _drain(engine)
    assert engine.state == "changeset_review"
    first_request = engine._request_id

    # A new chat with NO accept/reject on the prior changeset.
    created["runner"].queue(answer)
    await engine.handle({"type": "chat", "text": "do something else"})
    await _drain(engine)

    # The prior journal was finalized (default-accepted) with a note; NOT rolled
    # back; and the new turn proceeded under a fresh request id.
    assert journal.finalized_note is not None
    assert journal.rolled_back is False
    assert engine._request_id != first_request
    assert rec.first("answer")["text"] == "next"
    assert engine.state == "idle"


async def test_unknown_type_errors(monkeypatch):
    engine, rec, created, _ = make_engine()
    await engine.handle({"type": "instruct", "instruction": "x"})
    assert "unknown" in rec.first("error")["message"].lower()


async def test_status_and_list_handlers(monkeypatch):
    engine, rec, created, _ = make_engine(
        bridge=FakeBridge(status="compiling=true\nproject=pp\n",
                          files=[{"path": "a.eps", "ftype": "CUIEps",
                                  "settable": True}]),
    )
    await engine.handle({"type": "status"})
    st = rec.first("status")
    assert st["compiling"] is True and st["project"] == "pp"

    await engine.handle({"type": "list"})
    assert rec.first("list")["files"][0]["path"] == "a.eps"


async def test_cancel_calls_runner_cancel(monkeypatch):
    engine, rec, created, _ = make_engine()
    await engine.handle({"type": "cancel"})
    assert created["runner"].cancelled is True


# --------------------------------------------------------------------------- #
# agent_runner._classify_event.
# --------------------------------------------------------------------------- #


def _evt(method, item=None):
    payload = types.SimpleNamespace(item=types.SimpleNamespace(root=item))
    return types.SimpleNamespace(method=method, payload=payload)


def test_classify_turn_lifecycle():
    from eud_agent.agent_runner import _classify_event

    assert _classify_event(_evt("turn/started"))[0] == "thinking"
    assert _classify_event(_evt("turn/completed"))[0] == "turn_done"
    assert _classify_event(_evt("thread/tokenUsage/updated"))[0] == "token_usage"


def test_classify_propose_plan_extracts_markdown():
    from eud_agent.agent_runner import _classify_event

    item = types.SimpleNamespace(type="mcpToolCall", tool="propose_plan",
                                 result={"markdown": "# PLAN"})
    kind, detail, info = _classify_event(_evt("item/completed", item))
    assert kind == "tool_result"
    assert info["plan_markdown"] == "# PLAN"


def test_classify_mutation_tool_flags_mutation():
    from eud_agent.agent_runner import _classify_event

    item = types.SimpleNamespace(type="mcpToolCall", tool="dat_set", result=None)
    kind, detail, info = _classify_event(_evt("item/completed", item))
    assert info.get("mutation") is True


def test_classify_agent_message_carries_text():
    from eud_agent.agent_runner import _classify_event

    item = types.SimpleNamespace(type="agentMessage", text="the answer")
    kind, detail, info = _classify_event(_evt("item/completed", item))
    assert kind == "answer"
    assert info["answer_text"] == "the answer"


def test_classify_unknown_method_is_generic():
    from eud_agent.agent_runner import _classify_event

    kind, detail, info = _classify_event(_evt("something/weird"))
    assert kind == "event"
    assert info == {}


if __name__ == "__main__":
    raise SystemExit(pytest.main([__file__, "-q"]))
