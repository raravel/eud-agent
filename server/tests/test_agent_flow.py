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
        # EUD-064 continuity instrumentation: record the ORDER of start/resume
        # calls (the engine must start the FIRST chat then resume every later one),
        # every system prompt seen (only the first thread gets one), and how many
        # times the engine dropped the thread (reset{}).
        self.turn_calls: list[str] = []
        self.captured_system_prompts: list[str | None] = []
        self.reset_count = 0

    def queue(self, script) -> None:
        self.scripts.append(script)

    async def _emit(self, event: dict) -> None:
        await self._send(event)

    async def start_turn(self, text, *, request_id, system_prompt) -> dict:
        self.thread_id = self.thread_id or "fake-thread"
        self.captured_system_prompt = system_prompt
        self.captured_system_prompts.append(system_prompt)
        self.captured_prompts.append(text)
        self.turn_calls.append("start")
        return await self._run_script(request_id, text)

    async def resume_turn(self, text, *, request_id) -> dict:
        self.captured_prompts.append(text)
        self.captured_system_prompts.append(None)
        self.turn_calls.append("resume")
        return await self._run_script(request_id, text)

    def has_thread(self) -> bool:
        return self.thread_id is not None

    def reset_thread(self) -> None:
        self.thread_id = None
        self.reset_count += 1

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
# 6b. codex-thread isolation (EUD-062): the runner must NOT inherit the user's
#     personal codex environment. It composes LAUNCH-LEVEL config_overrides that
#     whole-table-replace mcp_servers (only eud-tools) and disable plugins, plus a
#     per-thread mcp_servers config carrying the live EUD_REQUEST_ID.
# --------------------------------------------------------------------------- #


def _make_runner(tmp_path):
    """A CodexSDKRunner built codex-free (no spawn) for composition assertions."""
    from eud_agent.agent_runner import CodexSDKRunner

    async def _send(_ev):  # pragma: no cover - never awaited in these tests
        return None

    return CodexSDKRunner(
        tool_layer=object(),
        send=_send,
        build_system_prompt=lambda *a, **k: "",
        codex_bin=str(tmp_path / "codex.cmd"),
        data_dir=str(tmp_path / "data"),
    )


def _overrides_dict(overrides):
    """Parse a tuple of ``key=tomlvalue`` override strings into a dict.

    Each entry is a ``-c`` config override the SDK forwards as ``--config k=v``;
    the value portion is TOML. ``mcp_servers`` is a whole-table override so its
    value parses as an (inline) TOML table -> a dict.
    """
    import tomllib

    parsed: dict = {}
    for entry in overrides:
        key, _, val = entry.partition("=")
        # Parse "key = value" as a one-line TOML document so inline tables /
        # bools / strings all decode through the real TOML grammar. A dotted key
        # (features.plugins) nests in the doc; walk back down to recover the leaf
        # keyed under the original dotted string.
        doc = tomllib.loads(f"{key} = {val}")
        node = doc
        for part in key.split("."):
            node = node[part]
        parsed[key] = node
    return parsed


def test_isolation_config_overrides_replace_mcp_table_and_disable_plugins(tmp_path):
    """The launch-level CodexConfig carries config_overrides that (a) whole-table
    replace mcp_servers with ONLY eud-tools and (b) set features.plugins=false."""
    runner = _make_runner(tmp_path)
    overrides = runner._codex_config().config_overrides
    assert isinstance(overrides, tuple)
    parsed = _overrides_dict(overrides)

    # (a) mcp_servers is a WHOLE-TABLE override (replaces the personal table) and
    # contains ONLY eud-tools — playwright/pencil/node_repl must be gone.
    assert "mcp_servers" in parsed, f"no mcp_servers override; got {overrides}"
    mcp = parsed["mcp_servers"]
    assert isinstance(mcp, dict)
    assert set(mcp.keys()) == {"eud-tools"}, (
        f"the override must replace the whole table with only eud-tools; got {mcp}"
    )
    assert mcp["eud-tools"].get("command")
    assert mcp["eud-tools"].get("args") == ["-m", "eud_agent.mcp_shim"]

    # (b) plugins disabled.
    assert parsed.get("features.plugins") is False, (
        f"features.plugins must be False; got {overrides}"
    )


def test_reasoning_visibility_overrides_present(tmp_path):
    """EUD-067: the launch-level overrides MUST force reasoning summaries on.

    codex requests ``reasoning.summary`` from the API only when the MODEL-FAMILY
    metadata says summaries are supported; gpt-5.5's family ships with it OFF, so
    without ``model_supports_reasoning_summaries=true`` the panel NEVER receives
    ``item/reasoning/summaryTextDelta`` (probed live 2026-06-05: forcing the flag
    produced 79 summaryTextDelta notifications; without it, zero)."""
    runner = _make_runner(tmp_path)
    parsed = _overrides_dict(runner._codex_config().config_overrides)
    assert parsed.get("model_supports_reasoning_summaries") is True, (
        f"model_supports_reasoning_summaries=true missing; got {parsed.keys()}"
    )
    assert parsed.get("model_reasoning_summary") == "detailed", (
        f"model_reasoning_summary must be 'detailed'; got {parsed!r}"
    )


def test_thread_start_kwargs_disable_guardian_reviewer(tmp_path):
    """EUD-067: thread_start must pass ApprovalMode.deny_all.

    The SDK default (``auto_review``) spawns a HIDDEN guardian reviewer thread
    that runs a full model review turn per MCP tool call (21 review turns in the
    live E2E) — 10-25s silent gaps between tool calls and ~2x token burn. The
    eud-agent server is already the policy layer (validation/journal/gate), so
    the guardian is redundant: deny_all = never ask, no reviewer."""
    from openai_codex import ApprovalMode

    runner = _make_runner(tmp_path)
    kwargs = runner._thread_start_kwargs("req-x", "system prompt here")
    assert kwargs.get("approval_mode") is ApprovalMode.deny_all
    # The existing composition must be preserved alongside.
    assert kwargs["base_instructions"] == "system prompt here"
    assert "eud-tools" in kwargs["config"]["mcp_servers"]


def test_isolation_no_ignore_user_config_launch_arg(tmp_path):
    """app-server does NOT accept --ignore-user-config (probed live: exec-only).

    The runner must therefore NOT inject it as a launch arg (it would make codex
    reject the argument and never start). launch_args_override must stay None so
    the SDK builds the normal ``codex --config ... app-server`` invocation.
    """
    runner = _make_runner(tmp_path)
    cfg = runner._codex_config()
    assert cfg.launch_args_override is None
    assert not any(
        "ignore-user-config" in str(o) for o in cfg.config_overrides
    ), "ignore-user-config is exec-only; must not appear as an app-server override"


def test_thread_config_still_carries_live_request_id(tmp_path):
    """Per-thread config keeps the eud-tools mcp_servers entry WITH the live
    EUD_REQUEST_ID (it changes per chat-session; the launch-level override is
    fixed per process, so the request id MUST live in the thread layer)."""
    runner = _make_runner(tmp_path)
    tc = runner._thread_config("req-abc123")
    eud = tc["mcp_servers"]["eud-tools"]
    assert eud["env"]["EUD_REQUEST_ID"] == "req-abc123"
    assert eud["env"]["EUD_DATA_DIR"] == str(tmp_path / "data")
    assert eud["args"] == ["-m", "eud_agent.mcp_shim"]


def test_isolation_is_injectable(tmp_path):
    """Isolation settings are injectable so the live E2E can flip them (e.g. add a
    skills mechanism later). Passing isolation=... overrides the defaults."""
    from eud_agent.agent_runner import CodexIsolation, CodexSDKRunner

    async def _send(_ev):  # pragma: no cover
        return None

    custom = CodexIsolation(disable_plugins=False, extra_overrides=("foo.bar=true",))
    runner = CodexSDKRunner(
        tool_layer=object(),
        send=_send,
        build_system_prompt=lambda *a, **k: "",
        codex_bin=str(tmp_path / "codex.cmd"),
        data_dir=str(tmp_path / "data"),
        isolation=custom,
    )
    parsed = _overrides_dict(runner._codex_config().config_overrides)
    # plugins NOT disabled (flipped off) and the extra override is present.
    assert "features.plugins" not in parsed
    assert parsed.get("foo.bar") is True
    # mcp_servers whole-table replacement still applies regardless.
    assert set(parsed["mcp_servers"].keys()) == {"eud-tools"}


# --------------------------------------------------------------------------- #
# 6c. Conversation continuity across chats (EUD-064): the FIRST chat starts the
#     codex thread (system prompt as base_instructions); EVERY later chat RESUMES
#     it so codex keeps its message + tool-call history. The agent-forgets bug
#     (chat-per-thread_start) is the defect this asserts against.
# --------------------------------------------------------------------------- #


def test_second_chat_resumes_same_thread(tmp_path, monkeypatch):
    """Two chats in one session -> exactly one start_turn then one resume_turn
    (NOT two start_turns). The retained codex thread carries the history."""
    from fastapi.testclient import TestClient

    cfg, app, created = build_app(tmp_path, monkeypatch)
    _wire_journal(app, tmp_path, _JournalBridge())

    with TestClient(app) as client:
        with _connect(client, cfg) as ws:
            r = created["runner"]
            r.queue(answer_script("first"))
            ws.send_json({"type": "chat", "text": "first message"})
            _recv_until(ws, "answer")

            r.queue(answer_script("second"))
            ws.send_json({"type": "chat", "text": "do you remember?"})
            _recv_until(ws, "answer")

    assert r.turn_calls == ["start", "resume"], (
        f"second chat must RESUME the thread, not start a new one; "
        f"got {r.turn_calls}"
    )
    # Only the FIRST thread carries base_instructions; the resume gets none.
    assert r.captured_system_prompts[0] is not None
    assert r.captured_system_prompts[1] is None


def test_resumed_chat_carries_refreshed_state_and_rag_plus_user_text(
    tmp_path, monkeypatch
):
    """The resumed turn text PREPENDS a refreshed [project state] + [reference
    context] (RAG for the new question) ahead of the original user text."""
    from fastapi.testclient import TestClient

    rag_marker = "RESUME_RAG_MARKER"

    def fake_search(query, k=5, *, rag_db):
        return [{"text": rag_marker, "title": "t", "url": "u", "distance": 0.1}]

    monkeypatch.setattr(rag_mod, "search", fake_search)

    cfg, app, created = build_app(tmp_path, monkeypatch)
    _wire_journal(app, tmp_path, _JournalBridge())

    with TestClient(app) as client:
        with _connect(client, cfg) as ws:
            r = created["runner"]
            r.queue(answer_script("first"))
            ws.send_json({"type": "chat", "text": "first message"})
            _recv_until(ws, "answer")

            r.queue(answer_script("second"))
            ws.send_json({"type": "chat", "text": "SECOND_USER_TEXT please"})
            _recv_until(ws, "answer")

    # captured_prompts[1] is the SECOND chat's resume text.
    resume_text = r.captured_prompts[1]
    assert "SECOND_USER_TEXT please" in resume_text, (
        f"original user text must be intact; got {resume_text!r}"
    )
    assert "[project state]" in resume_text, (
        f"resumed turn must carry refreshed project state; got {resume_text!r}"
    )
    assert "[reference context]" in resume_text, (
        f"resumed turn must carry refreshed reference context; got {resume_text!r}"
    )
    assert rag_marker in resume_text, (
        f"RAG must run against the NEW question for the resume; got {resume_text!r}"
    )


def test_reset_drops_thread_next_chat_starts_fresh(tmp_path, monkeypatch):
    """reset{} drops the retained thread; the NEXT chat starts a fresh thread
    (start_turn with a new system prompt), not a resume."""
    from fastapi.testclient import TestClient

    cfg, app, created = build_app(tmp_path, monkeypatch)
    _wire_journal(app, tmp_path, _JournalBridge())

    with TestClient(app) as client:
        with _connect(client, cfg) as ws:
            r = created["runner"]
            r.queue(answer_script("first"))
            ws.send_json({"type": "chat", "text": "first message"})
            _recv_until(ws, "answer")

            ws.send_json({"type": "reset"})

            r.queue(answer_script("fresh start"))
            ws.send_json({"type": "chat", "text": "brand new conversation"})
            _recv_until(ws, "answer")

    assert r.reset_count >= 1, "reset{} must drop the retained thread"
    # First chat started; after reset the next chat STARTS again (not resume).
    assert r.turn_calls == ["start", "start"], (
        f"chat after reset must start a fresh thread; got {r.turn_calls}"
    )
    assert r.captured_system_prompts[0] is not None
    assert r.captured_system_prompts[1] is not None, (
        "the post-reset chat must carry a fresh system prompt"
    )


def test_reset_in_changeset_review_finalizes_prior_journal(tmp_path, monkeypatch):
    """reset{} arriving while a prior changeset is UNDECIDED finalizes it
    (default-accept + archive note), exactly like a new chat does."""
    import json

    from fastapi.testclient import TestClient

    jbridge = _JournalBridge()
    cfg, app, created = build_app(tmp_path, monkeypatch)
    _wire_journal(app, tmp_path, jbridge)

    with TestClient(app) as client:
        with _connect(client, cfg) as ws:
            r = created["runner"]
            r.queue(edit_script(value="100"))
            ws.send_json({"type": "chat", "text": "an edit"})
            request_id = _recv_until(ws, "changeset")[-1]["request_id"]

            ws.send_json({"type": "reset"})
            # Give the reset a beat to finalize the journal before teardown.
            import time as _t
            _t.sleep(0.1)

    journal_dir = tmp_path / "data" / "journal"
    archived = journal_dir / f"{request_id}.accepted.json"
    live = journal_dir / f"{request_id}.json"
    assert archived.is_file(), "reset in changeset_review must archive the journal"
    assert not live.is_file(), "the prior live journal must not leak"
    payload = json.loads(archived.read_text(encoding="utf-8"))
    assert payload.get("note"), "the archive must carry a defaulted-to-accepted note"
    # Default-accept, NOT rollback: no inverse setdat of the old value 50.
    assert not any(
        c[0] == "setdat" and str(c[-1]) == "50" for c in jbridge.calls
    ), f"reset must default-accept, not roll back; calls={jbridge.calls}"


def test_reset_during_executing_is_error(tmp_path, monkeypatch):
    """reset{} while a turn is in flight (executing) is rejected with error{}."""
    from fastapi.testclient import TestClient

    cfg, app, created = build_app(tmp_path, monkeypatch)
    _wire_journal(app, tmp_path, _JournalBridge())

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
            _recv_until(ws, "agent_event", max_msgs=5)
            ws.send_json({"type": "reset"})
            err = _recv_until(ws, "error", max_msgs=5)[-1]
            ws.send_json({"type": "cancel"})
            import time as _t
            for _ in range(100):
                if r.cancelled:
                    break
                _t.sleep(0.02)
    assert "error" == err["type"]
    assert r.reset_count == 0, "reset during executing must NOT drop the thread"


def test_reset_when_idle_is_idempotent(tmp_path, monkeypatch):
    """reset{} when idle is a no-op-safe drop: it never errors, and a repeated
    reset stays safe (idempotent)."""
    from fastapi.testclient import TestClient

    cfg, app, created = build_app(tmp_path, monkeypatch)
    _wire_journal(app, tmp_path, _JournalBridge())

    with TestClient(app) as client:
        with _connect(client, cfg) as ws:
            r = created["runner"]
            # reset with no thread yet -> safe.
            ws.send_json({"type": "reset"})
            # then a chat + two resets back-to-back -> still safe.
            r.queue(answer_script("ok"))
            ws.send_json({"type": "chat", "text": "hello"})
            _recv_until(ws, "answer")
            ws.send_json({"type": "reset"})
            ws.send_json({"type": "reset"})
            # a following chat still works (starts fresh).
            r.queue(answer_script("again"))
            ws.send_json({"type": "chat", "text": "hi again"})
            _recv_until(ws, "answer")

    # No error{} surfaced for idle resets; the post-reset chat started fresh.
    assert r.turn_calls == ["start", "start"], (
        f"idle resets must not break the next chat's fresh start; got {r.turn_calls}"
    )


def test_budgets_and_gate_reset_per_request_across_continuous_thread(
    tmp_path, monkeypatch
):
    """Regression (EUD-064): the codex THREAD persists across chats, but each chat
    still mints a FRESH request_id, so the mutation gate + 30-action budget reset
    PER REQUEST. Two chats -> two distinct request ids, each with a fresh count."""
    from fastapi.testclient import TestClient

    jbridge = _JournalBridge()
    cfg, app, created = build_app(tmp_path, monkeypatch)
    _wire_journal(app, tmp_path, jbridge)

    seen_request_ids: list[str] = []

    def capture_edit_script():
        async def _script(emit, tools, request_id):
            seen_request_ids.append(request_id)
            tools.call_for_request(request_id, "dat_set",
                                   {"dat": "units", "param": "MaxHP",
                                    "objId": 0, "value": "100"})
            await emit({"type": "agent_event", "kind": "tool_call",
                        "detail": "dat_set"})
            return {"kind": "apply"}
        return _script

    with TestClient(app) as client:
        with _connect(client, cfg) as ws:
            r = created["runner"]
            r.queue(capture_edit_script())
            ws.send_json({"type": "chat", "text": "edit one"})
            _recv_until(ws, "changeset")

            r.queue(capture_edit_script())
            ws.send_json({"type": "chat", "text": "edit two"})
            _recv_until(ws, "changeset")

            tl = app.state.tool_layer
            assert len(seen_request_ids) == 2
            assert seen_request_ids[0] != seen_request_ids[1], (
                "each chat must mint a fresh request_id"
            )
            # Each per-request state has exactly ONE mutation + ONE action — the
            # gate/budget did NOT accumulate across the continuous thread.
            for rid in seen_request_ids:
                st = tl.get_request_state(rid)
                assert st.mutation_count == 1, (
                    f"per-request mutation count must reset; {rid} -> "
                    f"{st.mutation_count}"
                )
                assert st.action_count == 1, (
                    f"per-request action budget must reset; {rid} -> "
                    f"{st.action_count}"
                )


# --------------------------------------------------------------------------- #
# 6d. CodexSDKRunner-level thread retention (EUD-064): a non-resume turn must NOT
#     discard a retained thread (the engine routes start-vs-resume, but a stray
#     start_turn after a thread exists must reuse it, not nuke history). reset_thread
#     is the ONLY thing that drops it.
# --------------------------------------------------------------------------- #


class _FakeThread:
    def __init__(self, thread_id):
        self.id = thread_id


class _FakeHandle:
    def stream(self):
        return iter(())


class _FakeCodex:
    """A fake SDK ``Codex`` recording thread_start vs thread_resume calls."""

    def __init__(self):
        self.starts = 0
        self.resumes: list[str] = []
        self._next = 0

    def thread_start(self, **kwargs):
        self.starts += 1
        self._next += 1
        t = _FakeThread(f"thread-{self._next}")
        t.turn = lambda text: _FakeHandle()
        return t

    def thread_resume(self, thread_id):
        self.resumes.append(thread_id)
        t = _FakeThread(thread_id)
        t.turn = lambda text: _FakeHandle()
        return t


def _runner_with_fake_codex(tmp_path):
    from eud_agent.agent_runner import CodexSDKRunner

    async def _send(_ev):  # pragma: no cover - never awaited (empty stream)
        return None

    runner = CodexSDKRunner(
        tool_layer=object(),
        send=_send,
        build_system_prompt=lambda *a, **k: "",
        codex_bin=str(tmp_path / "codex.cmd"),
        data_dir=str(tmp_path / "data"),
    )
    fake = _FakeCodex()
    runner._codex = fake  # inject (skips the real SDK spawn)
    return runner, fake


def test_runner_second_nonresume_turn_keeps_retained_thread(tmp_path):
    """A second start_turn after a thread is retained must REUSE it (resume), not
    discard it with a fresh thread_start — history must survive a stray start."""
    import asyncio

    runner, fake = _runner_with_fake_codex(tmp_path)

    asyncio.run(
        runner.start_turn("first", request_id="r1", system_prompt="SYS")
    )
    assert fake.starts == 1
    assert runner.has_thread() is True
    first_id = runner._thread_id

    # A SECOND non-resume turn must NOT start a new thread (the retained one wins).
    asyncio.run(
        runner.start_turn("second", request_id="r2", system_prompt="SYS2")
    )
    assert fake.starts == 1, (
        f"a retained thread must not be discarded by a second start_turn; "
        f"thread_start called {fake.starts} times"
    )
    assert fake.resumes == [first_id], (
        f"the second turn must resume the retained thread; resumes={fake.resumes}"
    )


def test_runner_reset_thread_drops_retention(tmp_path):
    """reset_thread() drops the retained id so the NEXT turn starts fresh."""
    import asyncio

    runner, fake = _runner_with_fake_codex(tmp_path)
    asyncio.run(
        runner.start_turn("first", request_id="r1", system_prompt="SYS")
    )
    assert runner.has_thread() is True

    runner.reset_thread()
    assert runner.has_thread() is False

    asyncio.run(
        runner.start_turn("after reset", request_id="r2", system_prompt="SYS2")
    )
    assert fake.starts == 2, "after reset_thread a fresh thread_start must fire"


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


# --------------------------------------------------------------------------- #
# EUD-063: _classify_event forwards reasoning + answer delta TEXT.
#
# The reasoning delta notifications (ReasoningSummaryTextDeltaNotification /
# ReasoningTextDeltaNotification) and the answer delta notification
# (AgentMessageDeltaNotification) all carry the text in payload field
# ``delta: str`` (pinned openai_codex 0.1.0b3 — v2_all.py:92-99, 2804-2823;
# notification_registry maps item/agentMessage/delta, item/reasoning/textDelta,
# item/reasoning/summaryTextDelta). The classifier must surface that text:
# reasoning -> kind "reasoning", answer chunk -> kind "delta". A missing delta
# field degrades to empty detail, never raising.
# --------------------------------------------------------------------------- #


def _delta_evt(method, *, delta=None):
    """A streamed notification whose payload carries (or omits) ``delta``.

    Mirrors the real Notification wrapper: ``event.method`` + ``event.payload``,
    where the delta notifications expose ``delta`` directly on the payload (NOT
    under ``payload.item.root`` — that shape is for item/* lifecycle events).
    Omitting ``delta`` produces a payload with NO ``delta`` attribute, exercising
    the defensive degrade-to-empty path.
    """
    import types

    if delta is None:
        payload = types.SimpleNamespace()
    else:
        payload = types.SimpleNamespace(delta=delta)
    return types.SimpleNamespace(method=method, payload=payload)


def _item_evt(method, item=None):
    """An item/* lifecycle notification (payload.item.root) — regression helper."""
    import types

    payload = types.SimpleNamespace(item=types.SimpleNamespace(root=item))
    return types.SimpleNamespace(method=method, payload=payload)


def test_classify_reasoning_summary_text_delta():
    from eud_agent.agent_runner import _classify_event

    evt = _delta_evt("item/reasoning/summaryTextDelta", delta="먼저 유닛을")
    assert _classify_event(evt) == ("reasoning", "먼저 유닛을", {})


def test_classify_reasoning_text_delta():
    from eud_agent.agent_runner import _classify_event

    evt = _delta_evt("item/reasoning/textDelta", delta="thinking about marines")
    assert _classify_event(evt) == ("reasoning", "thinking about marines", {})


def test_classify_reasoning_delta_missing_field_is_empty():
    from eud_agent.agent_runner import _classify_event

    evt = _delta_evt("item/reasoning/textDelta")  # no delta field at all
    assert _classify_event(evt) == ("reasoning", "", {})


def test_classify_agent_message_delta_carries_text():
    from eud_agent.agent_runner import _classify_event

    evt = _delta_evt("item/agentMessage/delta", delta="마린의 HP는")
    assert _classify_event(evt) == ("delta", "마린의 HP는", {})


def test_classify_agent_message_delta_missing_field_is_empty():
    from eud_agent.agent_runner import _classify_event

    evt = _delta_evt("item/agentMessage/delta")  # no delta field
    assert _classify_event(evt) == ("delta", "", {})


def test_classify_existing_kinds_unchanged_regression():
    """Every other mapping the turn loop relies on stays exactly as it was."""
    from eud_agent.agent_runner import _classify_event

    # turn lifecycle / token usage
    assert _classify_event(_item_evt("turn/started")) == ("thinking", "", {})
    assert _classify_event(_item_evt("turn/completed")) == ("turn_done", "", {})
    assert (
        _classify_event(_item_evt("thread/tokenUsage/updated"))[0] == "token_usage"
    )

    # tool_call (item/started, mcpToolCall)
    import types

    started = types.SimpleNamespace(type="mcpToolCall", tool="dat_set")
    kind, detail, info = _classify_event(_item_evt("item/started", started))
    assert (kind, detail) == ("tool_call", "dat_set")

    # tool_result + mutation flag
    done = types.SimpleNamespace(type="mcpToolCall", tool="dat_set", result=None)
    kind, detail, info = _classify_event(_item_evt("item/completed", done))
    assert kind == "tool_result"
    assert info.get("mutation") is True

    # answer (full agentMessage item)
    msg = types.SimpleNamespace(type="agentMessage", text="the answer")
    kind, detail, info = _classify_event(_item_evt("item/completed", msg))
    assert kind == "answer"
    assert info["answer_text"] == "the answer"

    # unknown method -> generic event, empty info
    assert _classify_event(_item_evt("something/weird")) == (
        "event",
        "something/weird",
        {},
    )


# --------------------------------------------------------------------------- #
# EUD-068: _classify_event surfaces tool-call ARGUMENTS + RESULT/STATUS.
#
# The pinned SDK delivers McpToolCallThreadItem with ``arguments: Any`` on
# item/started and ``result: McpToolCallResult | None`` (content list of text
# blocks + structured_content) / ``status`` / ``error`` on item/completed —
# the official app-server protocol documents the same fields. The classifier
# must surface them through info["event_data"] so the panel Tool cards can show
# what was requested and what came back (live-E2E defect 2: the model retried
# dat_get arg shapes 4 times and the panel showed only bare names).
# --------------------------------------------------------------------------- #


def test_classify_tool_call_carries_arguments():
    import types

    from eud_agent.agent_runner import _classify_event

    item = types.SimpleNamespace(
        type="mcpToolCall",
        tool="dat_set",
        arguments={"dat": "units", "objId": 0, "param": "Hit Points",
                   "value": 20480},
    )
    kind, detail, info = _classify_event(_item_evt("item/started", item))
    assert (kind, detail) == ("tool_call", "dat_set")
    data = info.get("event_data")
    assert data is not None, "tool_call must carry event_data"
    # args serialized as compact JSON text for display.
    assert "units" in data["args"] and "Hit Points" in data["args"]


def test_classify_tool_call_arguments_json_string_passthrough():
    import types

    from eud_agent.agent_runner import _classify_event

    item = types.SimpleNamespace(
        type="mcpToolCall", tool="dat_get",
        arguments='{"dat": "units", "objId": 0}',
    )
    _, _, info = _classify_event(_item_evt("item/started", item))
    assert info["event_data"]["args"] == '{"dat": "units", "objId": 0}'


def test_classify_tool_result_carries_text_and_status():
    import types

    from eud_agent.agent_runner import _classify_event

    result = types.SimpleNamespace(
        content=[types.SimpleNamespace(type="text",
                                       text="OK: units|Hit Points|0 = 20480")],
        structured_content=None,
    )
    item = types.SimpleNamespace(
        type="mcpToolCall", tool="dat_get", arguments=None, result=result,
        status=types.SimpleNamespace(value="completed"), error=None,
    )
    kind, detail, info = _classify_event(_item_evt("item/completed", item))
    assert (kind, detail) == ("tool_result", "dat_get")
    data = info["event_data"]
    assert "20480" in data["result"]
    assert data["status"] == "completed"


def test_classify_tool_result_failed_carries_error():
    import types

    from eud_agent.agent_runner import _classify_event

    item = types.SimpleNamespace(
        type="mcpToolCall", tool="dat_set", arguments=None, result=None,
        status=types.SimpleNamespace(value="failed"),
        error=types.SimpleNamespace(message="ERROR: invalid dat name"),
    )
    _, _, info = _classify_event(_item_evt("item/completed", item))
    data = info["event_data"]
    assert data["status"] == "failed"
    assert "invalid dat name" in data["result"]


def test_classify_tool_event_data_truncated():
    """Huge args/results are truncated server-side (panel render safety)."""
    import types

    from eud_agent.agent_runner import TOOL_DATA_MAX_CHARS, _classify_event

    big = "x" * (TOOL_DATA_MAX_CHARS + 500)
    item = types.SimpleNamespace(type="mcpToolCall", tool="file_write",
                                 arguments={"code": big})
    _, _, info = _classify_event(_item_evt("item/started", item))
    args = info["event_data"]["args"]
    assert len(args) <= TOOL_DATA_MAX_CHARS + 16  # marker allowance
    assert args.endswith("…(잘림)")
