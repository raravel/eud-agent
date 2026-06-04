"""Verification artifact for EUD-022-5763: headless WS integration round-trips.

End-to-end tests of the PANEL protocol against the REAL FastAPI app
(``eud_agent.app.create_app``) wired to:

  * a FAKE BRIDGE thread (the same file-IPC imitation used by
    ``test_bridge_io.FakeBridge`` — answers ``inbox/srv-*.cmd`` with real
    ``outbox/*.result`` files for STATUS / LIST / GET / SET / NEWEPS); and
  * a MOCK codex (a tiny stand-in patched over ``app.CodexClient`` so no real
    ``codex exec`` subprocess is spawned).

The WS client is a python ``fastapi.testclient.TestClient`` websocket (NOT a
browser). These tests exercise the full ``instruct -> code -> apply`` happy path,
the NEWEPS duplicate path, the busy/timeout paths (via the BridgeIO timeout
seam), token/Origin rejection (4403), and the ``useContext=True`` rag path. A
final COVERAGE GATE test asserts every server->client message type documented in
architecture.md (``progress``, ``code``, ``applied``, ``error``, ``status``,
``list``) was observed at least once across the suite.

Design notes / seams used (so the suite stays fast and deterministic):

  * ``app.create_app`` constructs ``BridgeIO`` and ``CodexClient`` internally
    from the module globals. We monkeypatch ``app.BridgeIO`` with a thin
    subclass whose ``send`` binds SMALL default timeouts (the orchestrator calls
    ``bridge.get/set/neweps/status/list_files`` WITHOUT timeout kwargs, so the
    real 10s/180s defaults would otherwise govern). This is the documented
    "inject small timeouts via the BridgeIO seam".
  * ``app.CodexClient`` is monkeypatched with ``FakeCodex`` so ``create_app``
    builds a working (non-None) codex without resolving a real shim. The fake
    captures the prompt so the ``useContext`` test can prove the RAG context
    reached it.
  * ``rag.start_warmup`` is patched to a no-op thread (never load bge-m3) and
    ``rag.search`` is monkeypatched per-test for the context path.

The whole stack already exists (this is an integration-test task, not a TDD red
phase), so these tests are expected to PASS on first contact; any failure is a
real integration divergence to report (verify-first protocol, test-only variant).
"""

from __future__ import annotations

import threading
import time

import pytest

# Reuse the FakeBridge thread from the bridge_io suite rather than duplicating
# the file-IPC imitation (task: "a shared FakeBridge can be imported from
# test_bridge_io if importable"). It is a plain class with no import side
# effects, so importing it here is safe.
from test_bridge_io import FakeBridge

# Imported at collection: the real app + collaborators under test.
from eud_agent import app as app_mod
from eud_agent import rag as rag_mod
from eud_agent.config import Config

# httpx-backed TestClient ships a deprecation warning under starlette 1.x; it is
# orthogonal to what we assert (mirrors test_app.py).
pytestmark = pytest.mark.filterwarnings("ignore::DeprecationWarning")


# --------------------------------------------------------------------------- #
# COVERAGE GATE accumulator: every server->client message type seen across the
# whole suite is recorded here; the final test asserts the full set.
# --------------------------------------------------------------------------- #

OBSERVED: set[str] = set()

# The server->client message types architecture.md documents ("Server to
# client" bullet list). The gate proves the suite exercised each at least once.
EXPECTED_SERVER_TYPES = {"progress", "code", "applied", "error", "status", "list"}


def _record(events) -> None:
    """Fold a list of received events into the module-level OBSERVED set."""
    for ev in events:
        t = ev.get("type")
        if t is not None:
            OBSERVED.add(t)


# --------------------------------------------------------------------------- #
# Mock codex: a stand-in for codex_client.CodexClient (no subprocess).
# --------------------------------------------------------------------------- #


class FakeCodex:
    """Minimal CodexClient stand-in patched over ``app.CodexClient``.

    ``create_app`` calls ``CodexClient(cfg.codex_cmd, cfg.repo_root)``; this
    accepts that signature, never validates a shim, and returns canned code from
    an async ``generate``. The last prompt is captured so a test can prove the
    RAG context flowed into it.
    """

    last_prompt: str | None = None

    def __init__(self, codex_cmd, repo_root, *, code: str = "puts(1234);") -> None:
        self.codex_cmd = codex_cmd
        self.repo_root = repo_root
        self.code = code

    async def generate(self, prompt: str, *, timeout: float | None = None) -> str:
        type(self).last_prompt = prompt
        return self.code


# --------------------------------------------------------------------------- #
# Fast BridgeIO: a subclass binding small default timeouts at the documented
# "BridgeIO seam" so the busy/timeout paths resolve in well under a second.
# --------------------------------------------------------------------------- #


def make_fast_bridge_io(*, timeout: float, busy_timeout: float, poll: float = 0.02):
    """Build a BridgeIO subclass whose send() uses small default timeouts.

    The orchestrator invokes get/set/neweps/status/list_files with NO timeout
    kwargs, so it would otherwise inherit BridgeIO's real 10s/180s defaults.
    Binding small defaults here is how the WS-level busy/timeout tests stay fast
    without touching production code.
    """

    class FastBridgeIO(app_mod.BridgeIO):
        def send(self, command_text, **kw):
            kw.setdefault("timeout", timeout)
            kw.setdefault("busy_timeout", busy_timeout)
            kw.setdefault("poll_interval", poll)
            return super().send(command_text, **kw)

    return FastBridgeIO


# --------------------------------------------------------------------------- #
# Config + app builder (mirrors test_app.make_config; lifecycle threads off).
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
        codex_cmd="codex-stub",  # FakeCodex ignores it (never validates a shim)
        rag_db=str(tmp_path / "rag"),
        repo_root=str(repo_root),
        hf_cache_dir=str(tmp_path / "hf"),
        token=token,
    )


@pytest.fixture(autouse=True)
def _no_real_warmup(monkeypatch):
    """Never kick the real bge-m3 warmup thread (mirrors test_app)."""
    monkeypatch.setattr(
        rag_mod, "start_warmup",
        lambda *a, **k: threading.Thread(target=lambda: None),
    )


@pytest.fixture(autouse=True)
def _patch_codex(monkeypatch):
    """Replace CodexClient with FakeCodex so create_app builds a working codex.

    Reset the captured prompt before each test so the useContext assertion reads
    only its own run.
    """
    FakeCodex.last_prompt = None
    monkeypatch.setattr(app_mod, "CodexClient", FakeCodex)


def build_app(tmp_path, monkeypatch, *, token="tok-int", timeout=0.3,
              busy_timeout=2.5):
    """Build the real app with the fast-timeout BridgeIO + a tmp data dir.

    Lifecycle threads are OFF (``start_lifecycle=False``): the boot/shutdown
    machinery (ready writer, heartbeat watcher) is covered by test_app; here we
    drive only the HTTP/WS surface + the real orchestrator -> fake bridge path.
    """
    cfg = make_config(tmp_path, token=token)
    monkeypatch.setattr(
        app_mod, "BridgeIO",
        make_fast_bridge_io(timeout=timeout, busy_timeout=busy_timeout),
    )
    app = app_mod.create_app(cfg, start_lifecycle=False)
    return cfg, app


def _origin(cfg) -> str:
    return f"http://127.0.0.1:{cfg.port}"


def _connect(client, cfg):
    return client.websocket_connect(
        f"/ws?token={cfg.token}", headers={"origin": _origin(cfg)}
    )


def _recv_until(ws, etype, *, max_msgs=20):
    """Receive events until one of type ``etype`` arrives; return all collected.

    Records everything into OBSERVED along the way so progress events emitted
    before the terminal event still count toward the coverage gate.
    """
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


# --------------------------------------------------------------------------- #
# 1. Full happy path: status -> list -> instruct -> code (+ real diff) -> apply.
# --------------------------------------------------------------------------- #


def test_happy_path_status_list_instruct_apply(tmp_path, monkeypatch):
    from fastapi.testclient import TestClient

    current_code = "puts(1);\n"  # the fake bridge's GET content for the target
    target = "main.eps"
    generated = "puts(1234);"
    # Patch CodexClient to emit the chosen code BEFORE create_app builds it (the
    # app captures the codex singleton at construction, so the override must
    # precede build_app, not follow it).
    monkeypatch.setattr(app_mod, "CodexClient",
                        lambda c, r: FakeCodex(c, r, code=generated))
    cfg, app = build_app(tmp_path, monkeypatch)

    set_bodies: list[tuple[str, str]] = []

    def responder(first_line, body):
        if first_line == "STATUS":
            return "compiling=false\r\nproject='demo'\r\nversion=0.19.6.0"
        if first_line == "LIST":
            return "main.eps\tCUIEps\nui/layout.gui\tGUI"
        if first_line == f"GET {target}":
            return current_code
        if first_line == f"SET {target}":
            set_bodies.append((first_line, body))
            return "OK: set 'main.eps'"
        return "ERROR: unexpected " + first_line

    data_dir = tmp_path / "data"
    with FakeBridge(data_dir, responder), TestClient(app) as client:
        with _connect(client, cfg) as ws:
            # --- status ---
            ws.send_json({"type": "status"})
            status_evs = _recv_until(ws, "status")
            status = status_evs[-1]
            assert status["type"] == "status"
            assert status["compiling"] is False
            assert status["project"] == "'demo'"

            # --- list ---
            ws.send_json({"type": "list"})
            list_evs = _recv_until(ws, "list")
            files = list_evs[-1]["files"]
            by_path = {f["path"]: f for f in files}
            assert by_path["main.eps"]["ftype"] == "CUIEps"
            assert by_path["main.eps"]["settable"] is True
            assert by_path["ui/layout.gui"]["settable"] is False

            # --- instruct (useContext=False -> NO rag stage) ---
            ws.send_json({
                "type": "instruct",
                "instruction": "1234를 출력",
                "target": target,
                "useContext": False,
            })
            instruct_evs = _recv_until(ws, "code")
            stages = [e["stage"] for e in instruct_evs if e.get("type") == "progress"]
            # rag MUST be skipped; codex MUST run; lsp present (real or skipped).
            assert "rag" not in stages
            assert "codex" in stages
            assert "lsp" in stages

            code_ev = instruct_evs[-1]
            assert code_ev["type"] == "code"
            assert code_ev["code"] == generated
            assert code_ev["lang"] == "eps"
            assert isinstance(code_ev["diagnostics"], list)

            # The diff is a REAL difflib unified diff of current -> generated.
            diff = code_ev["diff"]
            assert f"a/{target}" in diff and f"b/{target}" in diff
            assert "-puts(1);" in diff
            assert "+puts(1234);" in diff

            # --- apply (mode: set) ---
            ws.send_json({
                "type": "apply",
                "mode": "set",
                "target": target,
                "code": generated,
            })
            applied_evs = _recv_until(ws, "applied")
            assert applied_evs[-1] == {"type": "applied", "target": target}

    # The fake bridge actually received the SET body the panel applied.
    assert set_bodies == [(f"SET {target}", generated)]


# --------------------------------------------------------------------------- #
# 2. NEWEPS path: duplicate -> error; fresh name -> applied.
# --------------------------------------------------------------------------- #


def test_neweps_duplicate_then_fresh(tmp_path, monkeypatch):
    from fastapi.testclient import TestClient

    cfg, app = build_app(tmp_path, monkeypatch)

    existing = {"dup.eps"}
    created: list[str] = []

    def responder(first_line, body):
        if first_line.startswith("NEWEPS "):
            name = first_line[len("NEWEPS "):]
            if name in existing:
                return f"ERROR: duplicate '{name}'"
            existing.add(name)
            created.append(name)
            return f"OK: neweps '{name}'"
        return "ERROR: unexpected " + first_line

    data_dir = tmp_path / "data"
    with FakeBridge(data_dir, responder), TestClient(app) as client:
        with _connect(client, cfg) as ws:
            # Duplicate name -> error.
            ws.send_json({
                "type": "apply",
                "mode": "neweps",
                "target": "dup.eps",
                "code": "x = 1;",
            })
            err_evs = _recv_until(ws, "error")
            assert "duplicate" in err_evs[-1]["message"].lower()

            # Fresh name -> applied.
            ws.send_json({
                "type": "apply",
                "mode": "neweps",
                "target": "fresh.eps",
                "code": "y = 2;",
            })
            ok_evs = _recv_until(ws, "applied")
            assert ok_evs[-1] == {"type": "applied", "target": "fresh.eps"}

    assert created == ["fresh.eps"]


# --------------------------------------------------------------------------- #
# 3a. Busy path: result withheld + compiling=true -> waiting_build -> applied
#     once the responder answers within the extended window.
# --------------------------------------------------------------------------- #


def _write_status(data_dir, *, compiling: bool) -> None:
    data_dir.mkdir(parents=True, exist_ok=True)
    text = "compiling={}\r\nproject='X'\r\n".format(
        "true" if compiling else "false"
    )
    (data_dir / "status.txt").write_text(text, encoding="utf-8")


def test_busy_waiting_build_then_applied(tmp_path, monkeypatch):
    from fastapi.testclient import TestClient

    # base timeout < answer delay < busy_timeout: the base window alone would
    # expire, but compiling=true extends it so the late answer still lands.
    cfg, app = build_app(tmp_path, monkeypatch, timeout=0.2, busy_timeout=3.0)
    data_dir = tmp_path / "data"
    _write_status(data_dir, compiling=True)

    target = "busy.eps"
    answered = threading.Event()

    def responder(first_line, body):
        if first_line == f"SET {target}":
            if not answered.is_set():
                time.sleep(0.5)  # past the 0.2s base window, inside the 3.0s busy
                answered.set()
            return "OK: set"
        return "ERROR: unexpected " + first_line

    with FakeBridge(data_dir, responder), TestClient(app) as client:
        with _connect(client, cfg) as ws:
            ws.send_json({
                "type": "apply",
                "mode": "set",
                "target": target,
                "code": "z = 3;",
            })
            evs = _recv_until(ws, "applied")
    stages = [e["stage"] for e in evs if e.get("type") == "progress"]
    assert "waiting_build" in stages, (
        "compiling=true must surface a waiting_build progress note"
    )
    assert evs[-1] == {"type": "applied", "target": target}


# --------------------------------------------------------------------------- #
# 3b. Timeout variant: result never arrives -> error "editor busy".
# --------------------------------------------------------------------------- #


def test_busy_timeout_error_editor_busy(tmp_path, monkeypatch):
    from fastapi.testclient import TestClient

    cfg, app = build_app(tmp_path, monkeypatch, timeout=0.2, busy_timeout=0.6)
    data_dir = tmp_path / "data"
    _write_status(data_dir, compiling=True)

    target = "never.eps"

    def responder(first_line, body):
        return None  # never answer -> the poll times out (BridgeBusy)

    with FakeBridge(data_dir, responder), TestClient(app) as client:
        with _connect(client, cfg) as ws:
            ws.send_json({
                "type": "apply",
                "mode": "set",
                "target": target,
                "code": "w = 4;",
            })
            evs = _recv_until(ws, "error")
    assert evs[-1] == {"type": "error", "message": "editor busy"}
    # The .cmd is LEFT in place on timeout (it applies once the build finishes).
    leftover = list((data_dir / "inbox").glob("srv-*.cmd"))
    assert len(leftover) == 1, "timeout must leave the .cmd in place"


# --------------------------------------------------------------------------- #
# 4. Token / Origin rejection (4403 close observed via the test client).
# --------------------------------------------------------------------------- #


def test_ws_wrong_token_closed_4403(tmp_path, monkeypatch):
    from fastapi.testclient import TestClient
    from starlette.websockets import WebSocketDisconnect

    cfg, app = build_app(tmp_path, monkeypatch, token="right")
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

    cfg, app = build_app(tmp_path, monkeypatch, token="right")
    with TestClient(app) as client:
        with pytest.raises(WebSocketDisconnect) as ei:
            with client.websocket_connect(
                "/ws?token=right", headers={"origin": "http://evil.example"}
            ):
                pass
    assert ei.value.code == 4403


# --------------------------------------------------------------------------- #
# 5. useContext=True with rag stubbed -> rag progress + context in the prompt.
# --------------------------------------------------------------------------- #


def test_instruct_use_context_threads_rag_into_prompt(tmp_path, monkeypatch):
    from fastapi.testclient import TestClient

    cfg, app = build_app(tmp_path, monkeypatch)

    target = "ctx.eps"
    current_code = "old();\n"
    rag_marker = "RAG_CONTEXT_CHUNK_ALPHA"

    # Stub rag.search to return a deterministic chunk; assert it reaches codex.
    def fake_search(query, k, *, rag_db):
        assert query == "컨텍스트로 생성"
        return [{"text": rag_marker, "title": "t", "url": "u", "distance": 0.1}]

    monkeypatch.setattr(rag_mod, "search", fake_search)

    def responder(first_line, body):
        if first_line == f"GET {target}":
            return current_code
        return "ERROR: unexpected " + first_line

    data_dir = tmp_path / "data"
    with FakeBridge(data_dir, responder), TestClient(app) as client:
        with _connect(client, cfg) as ws:
            ws.send_json({
                "type": "instruct",
                "instruction": "컨텍스트로 생성",
                "target": target,
                "useContext": True,
            })
            evs = _recv_until(ws, "code")

    stages = [e["stage"] for e in evs if e.get("type") == "progress"]
    assert stages[0] == "rag", "useContext=True must emit the rag stage first"
    assert "codex" in stages
    # The RAG context chunk reached the codex prompt (FakeCodex captured it).
    assert FakeCodex.last_prompt is not None
    assert rag_marker in FakeCodex.last_prompt
    # And the [참고자료] section carries it (build_prompt framing).
    assert "[참고자료]" in FakeCodex.last_prompt


# --------------------------------------------------------------------------- #
# 6. COVERAGE GATE (runs last by name): every documented server->client message
#    type was observed at least once across the suite.
# --------------------------------------------------------------------------- #


def test_zzz_coverage_gate_all_server_message_types_observed():
    """Meta-test: assert the suite exercised every architecture.md server->client
    message type. Named ``zzz`` so default file-order collection runs it last.
    """
    missing = EXPECTED_SERVER_TYPES - OBSERVED
    assert not missing, (
        f"server->client message types never observed by the suite: {missing}; "
        f"observed={sorted(OBSERVED)}"
    )


if __name__ == "__main__":
    raise SystemExit(pytest.main([__file__, "-q"]))
