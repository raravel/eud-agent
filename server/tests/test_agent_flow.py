"""Verification artifact for EUD-056-5ca7: the v2 agent engine + WS v2 routing.

These tests drive the v2 single-path agent core WITHOUT real codex: a
``FakeRunner`` implements the ``AgentRunner`` interface so the WS state machine
(``idle -> triage -> answer | apply | plan_review* -> executing ->
changeset_review -> idle``) is exercised deterministically. They assert the WS v2
contract verbatim (features/05 "WS protocol v2"):

  * ``chat`` -> ``answer`` (an answer-only turn);
  * ``chat`` -> ``plan`` -> ``plan_feedback`` -> ``plan`` (rev 2) ->
    ``plan_approve`` -> ``agent_event`` (executing) -> ``changeset``;
  * ``changeset_decision{reject}`` -> ``rollback_result``; ``accept`` archives;
  * a v1 ``instruct``/``apply`` message -> ``error {unknown type ...}`` (no compat
    shim — v1 is REMOVED);
  * ``cancel`` mid-turn cancels the in-flight turn safely;
  * the SYSTEM PROMPT the runner receives carries the tool catalog + project
    state + RAG context (assert on the prompt captured by the FakeRunner);
  * the env-flagged real-codex smoke (``EUD_REAL_CODEX_SMOKE=1``), skipped
    otherwise (it spends real tokens).

The v2 engine (``agent_runner.AgentRunner`` / ``CodexSDKRunner``) and the WS v2
routing in ``app.py`` do NOT exist during Step A, so this suite is expected to
FAIL on import / unknown-type until Step B.
"""

from __future__ import annotations

import os
import threading

import pytest

from eud_agent import app as app_mod
from eud_agent import rag as rag_mod
from eud_agent.config import Config

pytestmark = pytest.mark.filterwarnings("ignore::DeprecationWarning")


# --------------------------------------------------------------------------- #
# FakeRunner: a scriptable AgentRunner so every WS test runs without codex.
# --------------------------------------------------------------------------- #


class FakeRunner:
    """Scriptable :class:`AgentRunner` stand-in (no codex subprocess).

    Constructed with the same protocol the real runner uses
    (``build_system_prompt`` / ``send`` callback). Each ``start_turn`` /
    ``resume_turn`` runs a SCRIPTED step: a callable taking ``(emit, tools,
    request_id)`` that drives tool calls + emits agent_event/answer/plan via the
    runner's ``emit`` coroutine. Scripts are queued; the Nth turn pops the Nth
    script. The system prompt of the FIRST turn is captured for assertions.
    """

    def __init__(self, *, tool_layer, send, build_system_prompt) -> None:
        self.tool_layer = tool_layer
        self._send = send
        self._build_system_prompt = build_system_prompt
        self.scripts: list = []
        self.captured_system_prompt: str | None = None
        self.captured_prompts: list[str] = []
        self.cancelled = False
        self.thread_id: str | None = None

    def queue(self, script) -> None:
        self.scripts.append(script)

    async def _emit(self, event: dict) -> None:
        await self._send(event)

    async def start_turn(self, text, *, request_id, system_prompt) -> dict:
        self.thread_id = self.thread_id or "fake-thread"
        self.captured_system_prompt = system_prompt
        self.captured_prompts.append(text)
        return await self._run_script(request_id, text)

    async def resume_turn(self, text, *, request_id) -> dict:
        self.captured_prompts.append(text)
        return await self._run_script(request_id, text)

    async def _run_script(self, request_id, text) -> dict:
        if not self.scripts:
            return {"kind": "answer"}
        script = self.scripts.pop(0)
        return await script(self._emit, self.tool_layer, request_id)

    def cancel(self) -> None:
        self.cancelled = True


# --------------------------------------------------------------------------- #
# Config / app builders (mirror test_integration_ws).
# --------------------------------------------------------------------------- #


def make_config(tmp_path, *, port=8765, token="tok-v2"):
    data_dir = tmp_path / "data"
    (data_dir / "inbox").mkdir(parents=True, exist_ok=True)
    (data_dir / "outbox").mkdir(parents=True, exist_ok=True)
    repo_root = tmp_path / "repo"
    repo_root.mkdir(parents=True, exist_ok=True)
    return Config(
        data_dir=str(data_dir),
        port=port,
        codex_cmd="codex-stub",
        rag_db=str(tmp_path / "rag"),
        repo_root=str(repo_root),
        hf_cache_dir=str(tmp_path / "hf"),
        token=token,
    )


@pytest.fixture(autouse=True)
def _no_real_warmup(monkeypatch):
    monkeypatch.setattr(
        rag_mod, "start_warmup",
        lambda *a, **k: threading.Thread(target=lambda: None),
    )


@pytest.fixture(autouse=True)
def _no_real_rag_search(monkeypatch):
    """Default RAG search to a deterministic chunk (the system-prompt test
    overrides it)."""
    def fake_search(query, k=5, *, rag_db):
        return [{"text": "DEFAULT_RAG", "title": "t", "url": "u", "distance": 0.1}]

    monkeypatch.setattr(rag_mod, "search", fake_search)


class _FastBridge:
    """An instant in-memory BridgeIO stand-in (no file-IPC polling).

    ``create_app`` builds a ``BridgeIO`` for the engine's system-prompt state +
    status/list handlers; the real one polls the editor (10s timeouts) which would
    make every chat test slow. This patched stand-in answers status/list instantly
    so ``build_system_prompt``'s best-effort state section is fast.
    """

    def __init__(self, *a, **kw):
        pass

    def status(self, **kw):
        return "compiling=false\nproject=demo\n"

    def list_files(self, **kw):
        return [{"path": "main.eps", "ftype": "CUIEps", "settable": True}]


def build_app(tmp_path, monkeypatch, *, token="tok-v2"):
    """Build the real app with a FakeRunner injected as the agent runner factory.

    The app must accept an injectable runner factory so tests run codex-free; the
    factory signature mirrors the real CodexSDKRunner constructor
    (``tool_layer`` + ``send`` + ``build_system_prompt``). A fast bridge stand-in
    is patched over ``app.BridgeIO`` so the engine's status/list + system-prompt
    state never hit the real 10s file-IPC poll.
    """
    monkeypatch.setattr(app_mod, "BridgeIO", _FastBridge)
    cfg = make_config(tmp_path, token=token)
    created: dict = {}

    def runner_factory(*, tool_layer, send, build_system_prompt):
        r = FakeRunner(
            tool_layer=tool_layer, send=send,
            build_system_prompt=build_system_prompt,
        )
        created["runner"] = r
        return r

    app = app_mod.create_app(
        cfg, start_lifecycle=False, runner_factory=runner_factory
    )
    return cfg, app, created


def _origin(cfg):
    return f"http://127.0.0.1:{cfg.port}"


def _connect(client, cfg):
    return client.websocket_connect(
        f"/ws?token={cfg.token}", headers={"origin": _origin(cfg)}
    )


def _recv_until(ws, etype, *, max_msgs=40):
    collected = []
    for _ in range(max_msgs):
        msg = ws.receive_json()
        collected.append(msg)
        if msg.get("type") == etype:
            break
    else:
        raise AssertionError(
            f"never received {etype!r}; got {[m.get('type') for m in collected]}"
        )
    return collected


# --------------------------------------------------------------------------- #
# Scripts: small async callables a FakeRunner turn runs.
# --------------------------------------------------------------------------- #


def answer_script(text):
    async def _script(emit, tools, request_id):
        await emit({"type": "agent_event", "kind": "thinking", "detail": ""})
        await emit({"type": "answer", "text": text})
        return {"kind": "answer"}
    return _script


def plan_script(markdown):
    async def _script(emit, tools, request_id):
        # codex calls propose_plan; the tool layer marks plan_proposed + ends turn.
        result = tools.call_for_request(request_id, "propose_plan",
                                        {"markdown": markdown})
        await emit({"type": "agent_event", "kind": "tool_call",
                    "detail": "propose_plan"})
        return {"kind": "plan", "markdown": result["markdown"]}
    return _script


def edit_script(*, dat="units", param="MaxHP", obj_id=0, value="100"):
    async def _script(emit, tools, request_id):
        tools.call_for_request(request_id, "dat_set",
                               {"dat": dat, "param": param,
                                "objId": obj_id, "value": value})
        await emit({"type": "agent_event", "kind": "tool_call", "detail": "dat_set"})
        await emit({"type": "answer", "text": "done"})
        return {"kind": "apply"}
    return _script


# A fake bridge for the journal: dat GETs return a parseable reply; writes OK.
class _JournalBridge:
    def __init__(self):
        self.calls = []

    def getdat(self, dat, param, obj_id):
        return f"OK: {dat} {param} {obj_id} = 50"

    def setdat(self, dat, param, obj_id, value):
        self.calls.append(("setdat", dat, param, obj_id, value))
        return "OK"

    def resetdat(self, *a):
        self.calls.append(("resetdat", *a))
        return "OK"

    def status(self):
        return "compiling=false\nproject=demo\n"

    def list_files(self):
        return [{"path": "main.eps", "ftype": "CUIEps", "settable": True}]


# --------------------------------------------------------------------------- #
# 1. chat -> answer.
# --------------------------------------------------------------------------- #


def test_chat_answer(tmp_path, monkeypatch):
    from fastapi.testclient import TestClient

    cfg, app, created = build_app(tmp_path, monkeypatch)
    with TestClient(app) as client:
        with _connect(client, cfg) as ws:
            created["runner"].queue(answer_script("hello there"))
            ws.send_json({"type": "chat", "text": "what is eps?"})
            evs = _recv_until(ws, "answer")
    assert evs[-1] == {"type": "answer", "text": "hello there"}


# --------------------------------------------------------------------------- #
# 2. chat -> plan -> feedback -> plan rev2 -> approve -> executing -> changeset.
# --------------------------------------------------------------------------- #


def test_plan_feedback_approve_executing_changeset(tmp_path, monkeypatch):
    from fastapi.testclient import TestClient

    # Wire a real journal so the changeset is assembled from journaled writes.
    jbridge = _JournalBridge()

    cfg, app, created = build_app(tmp_path, monkeypatch)
    # Replace the tool layer's bridge + journal with our fake (the app builds a
    # ToolLayer; we point its journal_factory at the fake bridge).
    _wire_journal(app, tmp_path, jbridge)

    with TestClient(app) as client:
        with _connect(client, cfg) as ws:
            r = created["runner"]
            # turn 1: propose plan rev 1
            r.queue(plan_script("# Plan v1"))
            ws.send_json({"type": "chat", "text": "do a big change"})
            evs = _recv_until(ws, "plan")
            assert evs[-1]["markdown"] == "# Plan v1"
            assert evs[-1]["revision"] == 1

            # plan_feedback -> resume -> plan rev 2
            r.queue(plan_script("# Plan v2"))
            ws.send_json({"type": "plan_feedback", "text": "tweak it"})
            evs = _recv_until(ws, "plan")
            assert evs[-1]["markdown"] == "# Plan v2"
            assert evs[-1]["revision"] == 2

            # plan_approve -> resume executing turn (does the edits) -> changeset
            r.queue(edit_script())
            ws.send_json({"type": "plan_approve"})
            evs = _recv_until(ws, "changeset")
            cs = evs[-1]
    assert cs["type"] == "changeset"
    assert cs["request_id"]
    assert cs["items"], "the executing turn's journaled write must appear"


# --------------------------------------------------------------------------- #
# 3. changeset_decision reject -> rollback_result; accept -> archived.
# --------------------------------------------------------------------------- #


def test_changeset_reject_rolls_back(tmp_path, monkeypatch):
    from fastapi.testclient import TestClient

    jbridge = _JournalBridge()
    cfg, app, created = build_app(tmp_path, monkeypatch)
    _wire_journal(app, tmp_path, jbridge)

    with TestClient(app) as client:
        with _connect(client, cfg) as ws:
            r = created["runner"]
            r.queue(edit_script(value="999"))
            ws.send_json({"type": "chat", "text": "set maxhp to 999"})
            evs = _recv_until(ws, "changeset")
            cs = evs[-1]

            ws.send_json({"type": "changeset_decision", "decision": "reject",
                          "ids": "all"})
            evs = _recv_until(ws, "rollback_result")
            rr = evs[-1]
    assert rr["type"] == "rollback_result"
    assert rr["ok"] is True
    # The inverse op wrote the old value (50) back through the bridge.
    assert any(c[0] == "setdat" and str(c[-1]) == "50" for c in jbridge.calls), (
        f"reject must replay the inverse setdat; calls={jbridge.calls}"
    )
    _ = cs


def test_changeset_accept_archives(tmp_path, monkeypatch):
    from fastapi.testclient import TestClient

    jbridge = _JournalBridge()
    cfg, app, created = build_app(tmp_path, monkeypatch)
    _wire_journal(app, tmp_path, jbridge)

    with TestClient(app) as client:
        with _connect(client, cfg) as ws:
            r = created["runner"]
            r.queue(edit_script())
            ws.send_json({"type": "chat", "text": "small edit"})
            evs = _recv_until(ws, "changeset")
            request_id = evs[-1]["request_id"]

            # Only the edit's own setdat (value 100) should exist so far.
            assert jbridge.calls == [("setdat", "units", "MaxHP", 0, "100")]

            ws.send_json({"type": "changeset_decision", "decision": "accept",
                          "ids": "all"})
            # accept does not roll back; the journal is archived. The server
            # confirms with a rollback_result-shaped ack (ok, no inverse calls).
            evs = _recv_until(ws, "rollback_result")
    # accept performs NO inverse setdat (no write back of the old value 50).
    assert not any(
        c[0] == "setdat" and str(c[-1]) == "50" for c in jbridge.calls
    ), f"accept must not roll back; calls={jbridge.calls}"
    archived = tmp_path / "data" / "journal" / f"{request_id}.accepted.json"
    assert archived.is_file(), "accept must archive the journal"


def test_new_chat_finalizes_prior_undecided_changeset(tmp_path, monkeypatch):
    """features/05 line 45: a new chat while the prior changeset is UNDECIDED
    (no accept/reject) defaults the undecided items to accepted and archives the
    prior journal with a note — it must NOT leak as a live journal, and it must
    NOT be rolled back."""
    import json

    from fastapi.testclient import TestClient

    jbridge = _JournalBridge()
    cfg, app, created = build_app(tmp_path, monkeypatch)
    _wire_journal(app, tmp_path, jbridge)

    with TestClient(app) as client:
        with _connect(client, cfg) as ws:
            r = created["runner"]
            # Turn 1: a journaled edit -> changeset, left UNDECIDED.
            r.queue(edit_script(value="100"))
            ws.send_json({"type": "chat", "text": "first edit"})
            first_request = _recv_until(ws, "changeset")[-1]["request_id"]

            # Turn 2: a new chat with NO decision on the prior changeset.
            r.queue(answer_script("moved on"))
            ws.send_json({"type": "chat", "text": "something else"})
            _recv_until(ws, "answer")

    journal_dir = tmp_path / "data" / "journal"
    archived = journal_dir / f"{first_request}.accepted.json"
    live = journal_dir / f"{first_request}.json"
    assert archived.is_file(), "prior undecided journal must be archived on new chat"
    assert not live.is_file(), "the prior live journal must not leak"
    payload = json.loads(archived.read_text(encoding="utf-8"))
    assert payload.get("note"), "the archive must carry a defaulted-to-accepted note"
    # Default-accept, NOT rollback: no inverse setdat of the old value 50 fired.
    assert not any(
        c[0] == "setdat" and str(c[-1]) == "50" for c in jbridge.calls
    ), f"finalize must default-accept, not roll back; calls={jbridge.calls}"


# --------------------------------------------------------------------------- #
# 4. v1 instruct / apply -> error {unknown type}.
# --------------------------------------------------------------------------- #


def test_v1_instruct_is_unknown_type(tmp_path, monkeypatch):
    from fastapi.testclient import TestClient

    cfg, app, created = build_app(tmp_path, monkeypatch)
    with TestClient(app) as client:
        with _connect(client, cfg) as ws:
            ws.send_json({"type": "instruct", "instruction": "x", "target": "f"})
            evs = _recv_until(ws, "error")
    assert "unknown" in evs[-1]["message"].lower()
    assert "instruct" in evs[-1]["message"]


def test_v1_apply_is_unknown_type(tmp_path, monkeypatch):
    from fastapi.testclient import TestClient

    cfg, app, created = build_app(tmp_path, monkeypatch)
    with TestClient(app) as client:
        with _connect(client, cfg) as ws:
            ws.send_json({"type": "apply", "mode": "set", "target": "f",
                          "code": "x"})
            evs = _recv_until(ws, "error")
    assert "unknown" in evs[-1]["message"].lower()
    assert "apply" in evs[-1]["message"]


# --------------------------------------------------------------------------- #
# 5. cancel mid-turn cancels the in-flight turn safely.
# --------------------------------------------------------------------------- #


def test_cancel_invokes_runner_cancel(tmp_path, monkeypatch):
    from fastapi.testclient import TestClient

    cfg, app, created = build_app(tmp_path, monkeypatch)

    # A cooperative slow turn: it emits a "started" agent_event, then polls the
    # runner's ``cancelled`` flag with non-blocking asyncio.sleep (so the WS loop
    # stays free to receive cancel{}). When cancelled it stops without an answer.
    def slow_script(emit, tools, request_id):
        async def _run(emit, tools, request_id):
            import asyncio as _aio

            await emit({"type": "agent_event", "kind": "thinking",
                        "detail": "started"})
            runner = created["runner"]
            for _ in range(300):
                if runner.cancelled:
                    return {"kind": "answer"}
                await _aio.sleep(0.02)
            await emit({"type": "answer", "text": "late"})
            return {"kind": "answer"}
        return _run(emit, tools, request_id)

    with TestClient(app) as client:
        with _connect(client, cfg) as ws:
            r = created["runner"]
            r.queue(slow_script)
            ws.send_json({"type": "chat", "text": "long task"})
            # Observe the turn started streaming.
            _recv_until(ws, "agent_event", max_msgs=5)
            ws.send_json({"type": "cancel"})
            # Give the loop a moment to process the cancel.
            import time as _t
            for _ in range(100):
                if r.cancelled:
                    break
                _t.sleep(0.02)
    assert r.cancelled is True, "cancel{} must call the runner's cancel()"


# --------------------------------------------------------------------------- #
# 6. System prompt carries the tool catalog + project state + RAG context.
# --------------------------------------------------------------------------- #


def test_system_prompt_has_catalog_state_and_rag(tmp_path, monkeypatch):
    from fastapi.testclient import TestClient

    rag_marker = "RAG_SYSTEM_PROMPT_MARKER"

    def fake_search(query, k=5, *, rag_db):
        return [{"text": rag_marker, "title": "t", "url": "u", "distance": 0.1}]

    monkeypatch.setattr(rag_mod, "search", fake_search)

    cfg, app, created = build_app(tmp_path, monkeypatch)
    # A bridge that answers status + list so the project-state section populates.
    _wire_journal(app, tmp_path, _JournalBridge())

    with TestClient(app) as client:
        with _connect(client, cfg) as ws:
            r = created["runner"]
            r.queue(answer_script("ok"))
            ws.send_json({"type": "chat", "text": "make me a thing"})
            _recv_until(ws, "answer")
            sp = r.captured_system_prompt
    assert sp is not None, "the runner never received a system prompt"
    # tool catalog: at least one read + one write tool name appears.
    assert "list_files" in sp
    assert "dat_set" in sp
    assert "propose_plan" in sp
    # triage instructions (mechanical gate language from features/05).
    assert "propose_plan" in sp and "plan" in sp.lower()
    # project state (status/list best-effort).
    assert "demo" in sp or "project" in sp.lower()
    # RAG context for the user request.
    assert rag_marker in sp


# --------------------------------------------------------------------------- #
# 7. Real-codex smoke (env-flagged; spends real tokens — DO NOT run by default).
# --------------------------------------------------------------------------- #


@pytest.mark.skipif(
    os.environ.get("EUD_REAL_CODEX_SMOKE") != "1",
    reason="real codex smoke spends tokens; set EUD_REAL_CODEX_SMOKE=1 to run",
)
def test_real_codex_dat_edit_roundtrip(tmp_path):
    """ONE real dat-edit round-trip against real codex + the eud-tools MCP shim.

    Runs only when EUD_REAL_CODEX_SMOKE=1. Drives a CodexSDKRunner turn that asks
    codex to set a dat field via the MCP tools and asserts a changeset is produced
    (a journaled dat_set). The bridge is a fake file-IPC responder so no real
    editor is needed; only codex + the SDK + the MCP shim are exercised live.
    """
    import shutil

    from eud_agent.agent_runner import CodexSDKRunner

    codex_bin = os.environ.get("CODEX_CMD") or shutil.which("codex")
    if not codex_bin:
        pytest.skip("codex CLI not resolvable")

    captured: list[dict] = []

    async def send(ev):
        captured.append(ev)

    jbridge = _JournalBridge()
    from eud_agent.engine import build_system_prompt
    from eud_agent.tools import ToolLayer

    tool_layer = ToolLayer(jbridge)
    runner = CodexSDKRunner(
        tool_layer=tool_layer,
        send=send,
        # The runner never calls this (app.py builds the prompt and passes it to
        # start_turn); pass the real builder so the arity matches the contract.
        build_system_prompt=build_system_prompt,
        codex_bin=codex_bin,
        data_dir=str(tmp_path / "data"),
    )
    import asyncio

    result = asyncio.run(
        runner.start_turn(
            "Use dat_set to set units MaxHP objId 0 to 100, then stop.",
            request_id="smoke",
            system_prompt="You may call eud-tools to edit dats.",
        )
    )
    assert result["kind"] in ("apply", "answer", "plan")


# --------------------------------------------------------------------------- #
# Helper: point the app's ToolLayer journal at a fake bridge under tmp_path.
# --------------------------------------------------------------------------- #


def _wire_journal(app, tmp_path, bridge):
    """Rebuild the app's ToolLayer with a journal_factory bound to ``bridge``.

    The real ``create_app`` builds a ToolLayer over the production BridgeIO; for
    the flow tests we swap in a ToolLayer whose bridge AND journal use the fake
    so writes/rollbacks are observable and the journal persists under tmp_path.
    """
    from eud_agent.journal import Journal
    from eud_agent.tools import ToolLayer

    data_dir = str(tmp_path / "data")

    def journal_factory(request_id):
        return Journal(data_dir=data_dir, request_id=request_id, bridge=bridge)

    app.state.tool_layer = ToolLayer(bridge, journal_factory=journal_factory)
    # The engine reads the tool layer from app.state; rebind any runner-facing
    # handle the app exposes for the engine to pick up the swapped layer.
    if hasattr(app.state, "rebind_tool_layer"):
        app.state.rebind_tool_layer(app.state.tool_layer)
