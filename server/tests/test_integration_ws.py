"""Verification artifact for EUD-056-5ca7: headless WS v2 integration round-trips.

End-to-end tests of the v2 PANEL protocol against the REAL FastAPI app
(``eud_agent.app.create_app``) wired to:

  * a FAKE BRIDGE thread (the same file-IPC imitation used by
    ``test_bridge_io.FakeBridge`` — answers ``inbox/srv-*.cmd`` with real
    ``outbox/*.result`` files); the REAL ``BridgeIO`` + REAL ``ToolLayer`` +
    REAL ``Journal`` run on top of it, so a tool call writes a true ``SETDAT``
    ``.cmd`` and a rollback writes the true INVERSE ``.cmd`` (the EUD-034
    file-IPC verification pattern, migrated to v2); and
  * a ``FakeRunner`` (codex-free) that drives the tool calls a real codex turn
    would, so the WS v2 state machine + journal + bridge are exercised without a
    codex subprocess.

The WS client is a python ``fastapi.testclient.TestClient`` websocket. These tests
cover: the full v2 ``chat -> changeset -> changeset_decision{reject} -> verify
inverse SETDAT .cmd`` happy path; ``accept`` archiving without an inverse op;
token/Origin rejection (4403); and a COVERAGE GATE asserting every documented
v2 server->client message type was observed.

The v1 ``instruct``/``apply``/``code``/``applied`` protocol is REMOVED (no compat
shim); an incoming v1 message gets ``error{unknown type ...}`` (asserted here).
"""

from __future__ import annotations

import threading

import pytest

# Reuse the file-IPC FakeBridge thread from the bridge_io suite (no import side
# effects), so a tool call really round-trips inbox/.cmd -> outbox/.result.
from test_bridge_io import FakeBridge

from eud_agent import app as app_mod
from eud_agent import rag as rag_mod
from eud_agent.config import Config

pytestmark = pytest.mark.filterwarnings("ignore::DeprecationWarning")


# --------------------------------------------------------------------------- #
# COVERAGE GATE accumulator (v2 server->client message types).
# --------------------------------------------------------------------------- #

OBSERVED: set[str] = set()
EXPECTED_SERVER_TYPES = {
    "agent_event", "changeset", "rollback_result", "error", "status", "list",
    "memory", "memory_saved",
}


def _record(events) -> None:
    for ev in events:
        t = ev.get("type")
        if t is not None:
            OBSERVED.add(t)


# --------------------------------------------------------------------------- #
# FakeRunner: drives the tool calls a real codex turn would (codex-free).
# --------------------------------------------------------------------------- #


class FakeRunner:
    """Scriptable AgentRunner over the REAL tool layer (no codex subprocess).

    Each turn pops a scripted async step ``script(emit, tools, request_id)``; the
    step calls real tools via ``tools.call_for_request`` (which journals through the
    real bridge), streams agent_events, and returns the turn-end dict. The runner is
    handed the app's REAL ToolLayer so writes hit the real journal + file IPC.
    """

    def __init__(self, *, tool_layer, send, build_system_prompt) -> None:
        self.tool_layer = tool_layer
        self._send = send
        self.scripts: list = []
        self.cancelled = False

    def queue(self, script) -> None:
        self.scripts.append(script)

    async def start_turn(self, text, *, request_id, system_prompt) -> dict:
        return await self._run(request_id)

    async def resume_turn(self, text, *, request_id) -> dict:
        return await self._run(request_id)

    async def _run(self, request_id) -> dict:
        if not self.scripts:
            return {"kind": "answer"}
        return await self.scripts.pop(0)(self._send, self.tool_layer, request_id)

    def cancel(self) -> None:
        self.cancelled = True


def dat_edit_script(*, value="999"):
    async def _script(emit, tools, request_id):
        # A real journaled write through the real bridge (SETDAT round-trip).
        tools.call_for_request(
            request_id, "dat_set",
            {"dat": "units", "param": "MaxHP", "objId": 0, "value": value},
        )
        await emit({"type": "agent_event", "kind": "tool_call", "detail": "dat_set"})
        return {"kind": "apply"}
    return _script


def memory_write_script(*, file="resources", content="switch 7 = boss flag"):
    async def _script(emit, tools, request_id):
        # A real journaled memory_write through the app's wired ProjectMemory store
        # (features/07) — kind="write", so it journals + appears in the changeset
        # as a `memory` item, and a reject rolls the file back to its pre-write
        # content (the same inverse-op discipline as a dat/file write).
        tools.call_for_request(
            request_id, "memory_write", {"file": file, "content": content},
        )
        await emit({"type": "agent_event", "kind": "tool_call",
                    "detail": "memory_write"})
        return {"kind": "apply"}
    return _script


# --------------------------------------------------------------------------- #
# Config + app builder.
# --------------------------------------------------------------------------- #


def make_config(tmp_path, *, port=8765, token="tok-int"):
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
def _no_real_rag(monkeypatch):
    monkeypatch.setattr(rag_mod, "search", lambda *a, **k: [])


def make_fast_bridge_io(*, timeout=0.5, poll=0.02):
    """A BridgeIO subclass binding small default timeouts at the documented seam.

    The tool layer / journal call get/set with NO timeout kwargs, so they would
    otherwise inherit the real 10s default; small defaults keep the suite fast.
    """

    class FastBridgeIO(app_mod.BridgeIO):
        def send(self, command_text, **kw):
            kw.setdefault("timeout", timeout)
            kw.setdefault("poll_interval", poll)
            return super().send(command_text, **kw)

    return FastBridgeIO


def build_app(tmp_path, monkeypatch, *, token="tok-int"):
    cfg = make_config(tmp_path, token=token)
    monkeypatch.setattr(app_mod, "BridgeIO", make_fast_bridge_io())
    created: dict = {}

    def runner_factory(*, tool_layer, send, build_system_prompt):
        r = FakeRunner(tool_layer=tool_layer, send=send,
                       build_system_prompt=build_system_prompt)
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


def _recv_until(ws, etype, *, max_msgs=30):
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
    _record(collected)
    return collected


def _recv_expecting(ws, etype):
    """Receive EXACTLY ONE message and assert it is ``etype``.

    Hang-proof by construction: the server replies to every WS message with at
    least one frame (the engine answers an unhandled type with ``error``), so a
    SINGLE bounded ``receive_json`` always returns. An ``error`` (or any other
    type) when ``etype`` was expected fails the assertion IMMEDIATELY rather than
    looping into a second ``receive_json`` that would block forever (starlette's
    TestClient has no receive timeout and pytest-timeout is not installed). This
    is the Step-A failing-state path: today ``memory_get``/``memory_save`` are
    unhandled, so the reply is ``error`` and these tests fail fast.
    """
    msg = ws.receive_json()
    _record([msg])
    assert msg.get("type") == etype, (
        f"expected {etype!r}, got {msg.get('type')!r}: {msg}"
    )
    return msg


#: Streaming frames a turn may emit BEFORE its terminal result — skipped while
#: waiting for a turn-end type, but bounded so a hang is impossible.
_STREAM_TYPES = {"agent_event", "progress"}


def _recv_turn_end(ws, etype, *, max_msgs=10):
    """Wait for ``etype``, skipping streaming frames, treating ``error`` as fatal.

    Bounded by ``max_msgs`` AND short-circuited on ``error`` (a turn that failed
    — e.g. memory_write rejected because no memory store is wired today — emits
    ``error`` and NO further frames; continuing to receive would block forever).
    Any non-streaming, non-``etype`` message fails immediately. This keeps Step-A
    runs finishing fast while still passing in Step B (skips the one ``agent_event``
    the FakeRunner streams before the changeset).
    """
    for _ in range(max_msgs):
        msg = ws.receive_json()
        _record([msg])
        t = msg.get("type")
        if t == etype:
            return msg
        if t in _STREAM_TYPES:
            continue
        raise AssertionError(
            f"expected {etype!r}, got terminal {t!r}: {msg}"
        )
    raise AssertionError(f"never received {etype!r} within {max_msgs} messages")


# --------------------------------------------------------------------------- #
# 1. Full v2 happy path: chat -> changeset -> reject -> inverse SETDAT .cmd.
# --------------------------------------------------------------------------- #


def test_chat_changeset_reject_inverse_setdat(tmp_path, monkeypatch):
    from fastapi.testclient import TestClient

    cfg, app, created = build_app(tmp_path, monkeypatch)
    data_dir = tmp_path / "data"

    # The fake bridge records every command first-line so we can assert the
    # forward SETDAT then the INVERSE SETDAT (old value 50) round-tripped.
    cmds: list[str] = []

    def responder(first_line, body):
        cmds.append(first_line)
        if first_line.startswith("GETDAT "):
            return "OK: units MaxHP 0 = 50"
        if first_line.startswith("SETDAT "):
            return "OK: set"
        return "ERROR: unexpected " + first_line

    with FakeBridge(data_dir, responder), TestClient(app) as client:
        with _connect(client, cfg) as ws:
            created["runner"].queue(dat_edit_script(value="999"))
            ws.send_json({"type": "chat", "text": "set units MaxHP to 999"})
            cs = _recv_until(ws, "changeset")[-1]
            assert cs["items"], "the journaled dat_set must appear in the changeset"

            ws.send_json({"type": "changeset_decision", "decision": "reject",
                          "ids": "all"})
            rr = _recv_until(ws, "rollback_result")[-1]
            assert rr["ok"] is True

    # The forward write set 999; the rollback set the snapshotted old value 50.
    setdats = [c for c in cmds if c.startswith("SETDAT ")]
    assert any("|999" in c for c in setdats), f"no forward SETDAT 999: {setdats}"
    assert any("|50" in c for c in setdats), f"no inverse SETDAT 50: {setdats}"


# --------------------------------------------------------------------------- #
# 2. accept archives the journal without an inverse op.
# --------------------------------------------------------------------------- #


def test_chat_changeset_accept_archives(tmp_path, monkeypatch):
    from fastapi.testclient import TestClient

    cfg, app, created = build_app(tmp_path, monkeypatch)
    data_dir = tmp_path / "data"

    cmds: list[str] = []

    def responder(first_line, body):
        cmds.append(first_line)
        if first_line.startswith("GETDAT "):
            return "OK: units MaxHP 0 = 50"
        if first_line.startswith("SETDAT "):
            return "OK: set"
        return "ERROR: unexpected " + first_line

    with FakeBridge(data_dir, responder), TestClient(app) as client:
        with _connect(client, cfg) as ws:
            created["runner"].queue(dat_edit_script(value="123"))
            ws.send_json({"type": "chat", "text": "set units MaxHP to 123"})
            cs = _recv_until(ws, "changeset")[-1]
            request_id = cs["request_id"]

            ws.send_json({"type": "changeset_decision", "decision": "accept",
                          "ids": "all"})
            _recv_until(ws, "rollback_result")

    # accept performs NO inverse SETDAT (no write-back of the old value 50).
    assert not any("|50" in c for c in cmds if c.startswith("SETDAT ")), (
        f"accept must not roll back; cmds={cmds}"
    )
    archived = data_dir / "journal" / f"{request_id}.accepted.json"
    assert archived.is_file(), "accept must archive the journal"


# --------------------------------------------------------------------------- #
# 2b. Project memory (features/07): chat -> memory_write -> changeset carries a
# `memory` item -> reject -> the file is rolled back to its PRE-write content
# (the journal inverse for memory_write restores the snapshotted old content).
# The project name is resolved from the LIVE bridge STATUS, so the fake bridge
# reports one; the memory itself lives on disk under <data_dir>/harness/<proj>/.
# Then a direct memory_save round-trip (save -> get).
# --------------------------------------------------------------------------- #


def _memory_store(tmp_path, project="demo"):
    from eud_agent.memory import ProjectMemory

    return ProjectMemory(data_dir=str(tmp_path / "data"), project_name=project)


def _status_responder(first_line, body):
    # Only STATUS is needed for project resolution; memory IO never hits the bridge.
    if first_line == "STATUS":
        return "compiling=false\nproject=demo\nversion=0.19.6.0"
    return "ERROR: unexpected " + first_line


def test_chat_memory_write_changeset_reject_restores(tmp_path, monkeypatch):
    from fastapi.testclient import TestClient

    cfg, app, created = build_app(tmp_path, monkeypatch)
    data_dir = tmp_path / "data"

    # Seed a PRE-write value so reject is observable as a restore (not a delete).
    _memory_store(tmp_path, "demo").write("resources", "switch 1 = original")

    with FakeBridge(data_dir, _status_responder), TestClient(app) as client:
        with _connect(client, cfg) as ws:
            created["runner"].queue(
                memory_write_script(file="resources", content="switch 7 = boss flag")
            )
            ws.send_json({"type": "chat", "text": "remember boss switch"})
            # Bounded + error-terminal: today no memory store is wired, so the
            # memory_write turn errors and never reaches a changeset (fast fail).
            cs = _recv_turn_end(ws, "changeset")
            mem_items = [
                it for it in cs["items"] if it.get("kind") == "memory"
            ]
            assert mem_items, (
                f"the journaled memory_write must appear as a memory item; "
                f"items={cs['items']}"
            )
            assert mem_items[0]["target"] == "memory/resources"

            ws.send_json({"type": "changeset_decision", "decision": "reject",
                          "ids": "all"})
            rr = _recv_turn_end(ws, "rollback_result")
            assert rr["ok"] is True

            # memory_get now shows the RESTORED (pre-write) content.
            ws.send_json({"type": "memory_get"})
            mem = _recv_expecting(ws, "memory")
    assert mem["files"]["resources"] == "switch 1 = original", (
        f"reject must restore the pre-write memory content; got "
        f"{mem['files']['resources']!r}"
    )
    # On disk too (the inverse op wrote the old content back atomically).
    assert _memory_store(tmp_path, "demo").read("resources") == "switch 1 = original"


def test_memory_save_then_get_roundtrip(tmp_path, monkeypatch):
    from fastapi.testclient import TestClient

    cfg, app, created = build_app(tmp_path, monkeypatch)
    data_dir = tmp_path / "data"

    with FakeBridge(data_dir, _status_responder), TestClient(app) as client:
        with _connect(client, cfg) as ws:
            ws.send_json({"type": "memory_save", "file": "lessons",
                          "content": "never reuse switch 12"})
            # Single bounded receive: today this is an `error` (unhandled type),
            # which fails the assertion fast instead of blocking.
            saved = _recv_expecting(ws, "memory_saved")
            assert saved["file"] == "lessons"

            ws.send_json({"type": "memory_get"})
            mem = _recv_expecting(ws, "memory")
    assert mem["files"]["lessons"] == "never reuse switch 12"
    assert _memory_store(tmp_path, "demo").read("lessons") == "never reuse switch 12"


# --------------------------------------------------------------------------- #
# 3. v1 instruct/apply are REMOVED -> unknown type error.
# --------------------------------------------------------------------------- #


def test_v1_instruct_apply_rejected(tmp_path, monkeypatch):
    from fastapi.testclient import TestClient

    cfg, app, created = build_app(tmp_path, monkeypatch)
    with TestClient(app) as client:
        with _connect(client, cfg) as ws:
            ws.send_json({"type": "instruct", "instruction": "x", "target": "f"})
            assert "unknown" in _recv_until(ws, "error")[-1]["message"].lower()
            ws.send_json({"type": "apply", "mode": "set", "target": "f",
                          "code": "y"})
            assert "unknown" in _recv_until(ws, "error")[-1]["message"].lower()


# --------------------------------------------------------------------------- #
# 4. status / list round-trips against the fake bridge (v2 kept these).
# --------------------------------------------------------------------------- #


def test_status_and_list_roundtrip(tmp_path, monkeypatch):
    from fastapi.testclient import TestClient

    cfg, app, created = build_app(tmp_path, monkeypatch)
    data_dir = tmp_path / "data"

    def responder(first_line, body):
        if first_line == "STATUS":
            return "compiling=false\r\nproject='demo'\r\nversion=0.19.6.0"
        if first_line == "LIST":
            return "main.eps\tCUIEps\nui/layout.gui\tGUI"
        return "ERROR: unexpected " + first_line

    with FakeBridge(data_dir, responder), TestClient(app) as client:
        with _connect(client, cfg) as ws:
            ws.send_json({"type": "status"})
            status = _recv_until(ws, "status")[-1]
            assert status["compiling"] is False
            assert status["project"] == "'demo'"

            ws.send_json({"type": "list"})
            files = _recv_until(ws, "list")[-1]["files"]
            by_path = {f["path"]: f for f in files}
            assert by_path["main.eps"]["ftype"] == "CUIEps"
            assert by_path["main.eps"]["settable"] is True
            assert by_path["ui/layout.gui"]["settable"] is False


# --------------------------------------------------------------------------- #
# 5. Token / Origin rejection (4403).
# --------------------------------------------------------------------------- #


def test_ws_wrong_token_closed_4403(tmp_path, monkeypatch):
    from fastapi.testclient import TestClient
    from starlette.websockets import WebSocketDisconnect

    cfg, app, created = build_app(tmp_path, monkeypatch, token="right")
    with TestClient(app) as client:
        with pytest.raises(WebSocketDisconnect) as ei:
            with client.websocket_connect(
                "/ws?token=wrong", headers={"origin": _origin(cfg)}
            ):
                pass
    assert ei.value.code == 4403


def test_ws_wrong_origin_closed_4403(tmp_path, monkeypatch):
    from fastapi.testclient import TestClient
    from starlette.websockets import WebSocketDisconnect

    cfg, app, created = build_app(tmp_path, monkeypatch, token="right")
    with TestClient(app) as client:
        with pytest.raises(WebSocketDisconnect) as ei:
            with client.websocket_connect(
                "/ws?token=right", headers={"origin": "http://evil.example"}
            ):
                pass
    assert ei.value.code == 4403


# --------------------------------------------------------------------------- #
# 6. COVERAGE GATE (runs last): every documented v2 server->client type observed.
# --------------------------------------------------------------------------- #


def test_zzz_coverage_gate_all_server_message_types_observed():
    missing = EXPECTED_SERVER_TYPES - OBSERVED
    assert not missing, (
        f"v2 server->client message types never observed: {missing}; "
        f"observed={sorted(OBSERVED)}"
    )


if __name__ == "__main__":
    raise SystemExit(pytest.main([__file__, "-q"]))
