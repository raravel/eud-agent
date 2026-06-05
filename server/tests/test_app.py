"""Verification artifact for EUD-018-bafb: the FastAPI app + lifecycle.

These tests drive ``eud_agent.app`` (the resident local server) WITHOUT codex,
the real RAG model, or the Lua bridge. They assert the HTTP/WS surface and the
boot/shutdown lifecycle the harness fixes (architecture.md "Boot and lifecycle"
+ "WebSocket protocol", rules.md "Server and panel", features/02 "app.py /
__main__.py" — the React re-plan: serve ``panel/dist/``, 503 when not built):

  * ``GET /healthz`` -> 200.
  * ``GET /`` with NO ``panel/dist/`` -> 503 carrying the build hint
    ("panel not built — run npm run build in panel/"); WITH a built
    ``panel/dist/index.html`` -> 200 serving it.
  * ``WS /ws`` validates BOTH the ``token`` query param AND the ``Origin``
    header (``http://127.0.0.1:<port>``) at accept; a wrong token OR a wrong
    Origin closes with code 4403 (never accepted). A correct token + Origin is
    accepted and round-trips ``status`` / ``list`` against a fake bridge dir.
  * ``server.ready`` is written ONLY after a real TCP connect to the server's
    own socket succeeds (integration test: a real uvicorn in a thread on port 0
    with a tmp data dir and injected fast intervals; we poll for the file and
    parse it).
  * The heartbeat watcher is a TESTABLE component: with a tiny check-interval and
    staleness it detects a stale/missing ``heartbeat.txt`` and invokes a shutdown
    callback (and the server deletes ``server.ready``) WITHOUT killing the test
    process.

``eud_agent.app`` does NOT exist during Step A, so this suite is expected to
FAIL on import until app.py is implemented (Step B).
"""

from __future__ import annotations

import json
import socket
import threading
import time
from datetime import UTC, datetime, timedelta

import pytest

# Imported at collection so the failing import is the first signal in Step A.
# (No try/except: the suite MUST error out until the module exists.)
from eud_agent import app as app_mod
from eud_agent.config import Config

# httpx-backed TestClient ships a deprecation warning under starlette 1.x; it is
# orthogonal to what we assert, so silence just that one.
pytestmark = pytest.mark.filterwarnings("ignore::DeprecationWarning")


# --------------------------------------------------------------------------- #
# Helpers: build a Config pointing at a tmp data dir + (optional) panel dist.
# --------------------------------------------------------------------------- #


def make_config(tmp_path, *, port=8765, build_panel=False, token="tok-123"):
    """A Config whose data_dir/repo_root live under tmp_path.

    ``build_panel`` writes a fake ``panel/dist/index.html`` under repo_root so
    ``GET /`` can serve it; otherwise the 503 path is exercised. ``codex_cmd`` /
    ``rag_db`` are stub paths — the HTTP/WS surface never spawns codex or loads
    the model, and RAG warmup is patched off in the unit tests.
    """
    data_dir = tmp_path / "data"
    (data_dir / "inbox").mkdir(parents=True, exist_ok=True)
    (data_dir / "outbox").mkdir(parents=True, exist_ok=True)
    repo_root = tmp_path / "repo"
    repo_root.mkdir(parents=True, exist_ok=True)
    if build_panel:
        dist = repo_root / "panel" / "dist"
        dist.mkdir(parents=True, exist_ok=True)
        (dist / "index.html").write_text(
            "<!doctype html><title>panel</title>", encoding="utf-8"
        )
        # an asset to prove the static mount serves the built dir
        assets = dist / "assets"
        assets.mkdir(exist_ok=True)
        (assets / "app.js").write_text("console.log('panel')", encoding="utf-8")
    return Config(
        data_dir=str(data_dir),
        port=port,
        codex_cmd="",  # not used by the HTTP/WS surface
        rag_db=str(tmp_path / "rag"),
        repo_root=str(repo_root),
        hf_cache_dir=str(tmp_path / "hf"),
        token=token,
    )


def fresh_heartbeat(data_dir, *, age_seconds=0.0):
    """Write a heartbeat.txt with an ISO timestamp aged by ``age_seconds``."""
    ts = datetime.now(UTC) - timedelta(seconds=age_seconds)
    (data_dir / "heartbeat.txt").write_text(
        ts.isoformat(), encoding="utf-8"
    )


@pytest.fixture(autouse=True)
def _no_real_warmup(monkeypatch):
    """Never kick the real bge-m3 warmup thread during app tests."""
    from eud_agent import rag as rag_mod

    monkeypatch.setattr(
        rag_mod, "start_warmup", lambda *a, **k: threading.Thread(target=lambda: None)
    )


def build_test_app(cfg, **kw):
    """Build the app for in-process TestClient use.

    ``create_app(cfg, **lifecycle_kwargs)`` is the contract. We disable the
    background lifecycle threads for the pure HTTP/WS-surface tests (we exercise
    the lifecycle separately in the integration + watcher tests).
    """
    kw.setdefault("start_lifecycle", False)
    return app_mod.create_app(cfg, **kw)


# --------------------------------------------------------------------------- #
# GET /healthz
# --------------------------------------------------------------------------- #


def test_healthz_ok(tmp_path):
    from fastapi.testclient import TestClient

    cfg = make_config(tmp_path)
    with TestClient(build_test_app(cfg)) as client:
        r = client.get("/healthz")
    assert r.status_code == 200


# --------------------------------------------------------------------------- #
# GET / : 503 when not built, 200 when built.
# --------------------------------------------------------------------------- #


def test_root_503_when_panel_not_built(tmp_path):
    from fastapi.testclient import TestClient

    cfg = make_config(tmp_path, build_panel=False)
    with TestClient(build_test_app(cfg)) as client:
        r = client.get("/")
    assert r.status_code == 503
    body = r.text.lower()
    assert "panel not built" in body
    assert "npm run build" in body


def test_root_serves_built_index(tmp_path):
    from fastapi.testclient import TestClient

    cfg = make_config(tmp_path, build_panel=True)
    with TestClient(build_test_app(cfg)) as client:
        r = client.get("/")
    assert r.status_code == 200
    assert "<title>panel</title>" in r.text


# --------------------------------------------------------------------------- #
# WS /ws : token + Origin validation (close 4403 otherwise).
# --------------------------------------------------------------------------- #


def _origin_for(cfg):
    return f"http://127.0.0.1:{cfg.port}"


def test_ws_wrong_token_closed_4403(tmp_path):
    from fastapi.testclient import TestClient
    from starlette.websockets import WebSocketDisconnect

    cfg = make_config(tmp_path, token="right")
    with TestClient(build_test_app(cfg)) as client:
        with pytest.raises(WebSocketDisconnect) as ei:
            with client.websocket_connect(
                "/ws?token=wrong", headers={"origin": _origin_for(cfg)}
            ):
                pass
    assert ei.value.code == 4403


def test_ws_wrong_origin_closed_4403(tmp_path):
    from fastapi.testclient import TestClient
    from starlette.websockets import WebSocketDisconnect

    cfg = make_config(tmp_path, token="right")
    with TestClient(build_test_app(cfg)) as client:
        with pytest.raises(WebSocketDisconnect) as ei:
            with client.websocket_connect(
                "/ws?token=right", headers={"origin": "http://evil.example"}
            ):
                pass
    assert ei.value.code == 4403


def test_ws_correct_token_and_origin_accepted_status_roundtrip(tmp_path, monkeypatch):
    from fastapi.testclient import TestClient

    cfg = make_config(tmp_path, token="right")

    # Patch bridge_io so the WS 'status' handler returns a deterministic reply
    # without a real bridge round-trip. The orchestrator calls bridge.status().
    from eud_agent import bridge_io as bio_mod

    def fake_status(self, **kw):
        return "compiling=false\nproject=myproj\n"

    monkeypatch.setattr(bio_mod.BridgeIO, "status", fake_status)

    with TestClient(build_test_app(cfg)) as client:
        with client.websocket_connect(
            "/ws?token=right", headers={"origin": _origin_for(cfg)}
        ) as ws:
            ws.send_json({"type": "status"})
            msg = ws.receive_json()
    assert msg["type"] == "status"
    assert msg["compiling"] is False
    assert msg["project"] == "myproj"


def test_ws_list_roundtrip(tmp_path, monkeypatch):
    from fastapi.testclient import TestClient

    cfg = make_config(tmp_path, token="right")
    from eud_agent import bridge_io as bio_mod

    def fake_list(self, **kw):
        return [{"path": "a.eps", "ftype": "CUIEps", "settable": True}]

    monkeypatch.setattr(bio_mod.BridgeIO, "list_files", fake_list)

    with TestClient(build_test_app(cfg)) as client:
        with client.websocket_connect(
            "/ws?token=right", headers={"origin": _origin_for(cfg)}
        ) as ws:
            ws.send_json({"type": "list"})
            msg = ws.receive_json()
    assert msg["type"] == "list"
    assert msg["files"] == [{"path": "a.eps", "ftype": "CUIEps", "settable": True}]


# --------------------------------------------------------------------------- #
# server.ready written ONLY after a real TCP connect succeeds (integration).
# --------------------------------------------------------------------------- #


def _read_ready(path):
    return json.loads(path.read_text(encoding="utf-8"))


def test_server_ready_written_after_socket_accepts(tmp_path):
    """Start a real uvicorn in a thread on port 0; server.ready appears only
    after the socket actually accepts, and carries port/pid/token/started_at."""
    import uvicorn

    # Pre-bind the listener (port 0 -> OS-assigned) exactly as the real entry
    # point does: the resolved port is the single source of truth shared by cfg
    # (Origin check + ready-writer) AND uvicorn (sockets=[sock]).
    sock = app_mod.resolve_bound_socket(0)
    port = sock.getsockname()[1]

    cfg = make_config(tmp_path, port=port, token="ready-tok", build_panel=True)
    data_dir = tmp_path / "data"
    ready_path = data_dir / "server.ready"
    # Keep the heartbeat fresh so the watcher does not shut us down mid-test.
    fresh_heartbeat(data_dir, age_seconds=0.0)

    # Inject fast intervals so the lifecycle threads spin quickly (no real
    # 15s/60s waits) but generous staleness so the watcher does not fire.
    app = app_mod.create_app(
        cfg,
        start_lifecycle=True,
        ready_poll_interval=0.05,
        ready_timeout=10.0,
        heartbeat_check_interval=0.1,
        heartbeat_staleness=600.0,
    )

    config = uvicorn.Config(app, log_level="warning")
    server = uvicorn.Server(config)
    # Hand uvicorn our existing event-loop server callback + the pre-bound socket.
    app.state.shutdown_state["server"] = server
    thread = threading.Thread(
        target=lambda: server.run(sockets=[sock]), daemon=True
    )
    thread.start()
    try:
        # Wait for uvicorn to actually bind + for server.ready to appear.
        deadline = time.monotonic() + 10.0
        while time.monotonic() < deadline:
            if ready_path.is_file() and getattr(server, "started", False):
                break
            time.sleep(0.05)
        assert ready_path.is_file(), "server.ready was never written"

        ready = _read_ready(ready_path)
        assert set(ready) >= {"port", "pid", "token", "started_at"}
        assert ready["token"] == "ready-tok"
        # EUD-037: the bridge spawns the server through the venv launcher, which
        # re-execs the base interpreter as a CHILD. The bridge owns the LAUNCHER
        # pid, so ownership validation must be able to match the ready PPID too.
        # The writer runs in THIS process (uvicorn-in-thread), so the advertised
        # ppid is os.getppid() as seen here.
        import os

        assert ready["ppid"] == os.getppid()
        # server.ready is the single source of truth for the actual (resolved)
        # port — it must equal the OS-assigned port we pre-bound.
        assert ready["port"] == port
        assert isinstance(ready["port"], int) and ready["port"] > 0

        # The advertised port really accepts connections (the write happened
        # only after the self-connect confirmation).
        with socket.create_connection(("127.0.0.1", ready["port"]), timeout=5.0):
            pass
    finally:
        server.should_exit = True
        thread.join(timeout=10.0)

    # Graceful shutdown deletes server.ready (the server must not outlive boot
    # artifacts).
    assert not ready_path.is_file(), "server.ready not removed on shutdown"


# --------------------------------------------------------------------------- #
# Heartbeat watcher: stale / missing -> shutdown callback + ready deleted.
# --------------------------------------------------------------------------- #


def test_heartbeat_watcher_fires_on_stale(tmp_path):
    """The watcher is a standalone testable component: a stale heartbeat invokes
    the shutdown callback. We test the logic directly (no process kill)."""
    data_dir = tmp_path / "data"
    data_dir.mkdir(parents=True, exist_ok=True)
    ready_path = data_dir / "server.ready"
    ready_path.write_text(json.dumps({"port": 1}), encoding="utf-8")

    # A heartbeat older than the staleness threshold.
    fresh_heartbeat(data_dir, age_seconds=120.0)

    fired = threading.Event()

    watcher = app_mod.HeartbeatWatcher(
        data_dir=str(data_dir),
        check_interval=0.05,
        staleness=1.0,
        on_stale=fired.set,
    )
    watcher.start()
    try:
        assert fired.wait(timeout=3.0), "watcher never fired on a stale heartbeat"
    finally:
        watcher.stop()

    # The watcher (or its on_stale) removes server.ready so the server does not
    # outlive the editor.
    assert not ready_path.is_file(), "server.ready not deleted on stale heartbeat"


def test_heartbeat_watcher_does_not_fire_when_fresh(tmp_path):
    data_dir = tmp_path / "data"
    data_dir.mkdir(parents=True, exist_ok=True)
    fresh_heartbeat(data_dir, age_seconds=0.0)

    fired = threading.Event()
    # Keep refreshing the heartbeat so it never goes stale while we watch.
    stop_refresh = threading.Event()

    def refresher():
        while not stop_refresh.is_set():
            fresh_heartbeat(data_dir, age_seconds=0.0)
            time.sleep(0.05)

    rt = threading.Thread(target=refresher, daemon=True)
    rt.start()

    watcher = app_mod.HeartbeatWatcher(
        data_dir=str(data_dir),
        check_interval=0.05,
        staleness=2.0,
        on_stale=fired.set,
    )
    watcher.start()
    try:
        # Watch for a while; a fresh heartbeat must NOT trip the watcher.
        time.sleep(0.6)
        assert not fired.is_set(), "watcher fired despite a fresh heartbeat"
    finally:
        watcher.stop()
        stop_refresh.set()
        rt.join(timeout=2.0)


def test_heartbeat_watcher_fires_on_missing_after_grace(tmp_path):
    """A missing heartbeat (never written) trips the watcher after a grace."""
    data_dir = tmp_path / "data"
    data_dir.mkdir(parents=True, exist_ok=True)
    # No heartbeat.txt at all.

    fired = threading.Event()
    watcher = app_mod.HeartbeatWatcher(
        data_dir=str(data_dir),
        check_interval=0.05,
        staleness=1.0,
        grace=0.3,
        on_stale=fired.set,
    )
    watcher.start()
    try:
        assert fired.wait(timeout=3.0), (
            "watcher never fired on a missing heartbeat after the grace period"
        )
    finally:
        watcher.stop()


def test_heartbeat_watcher_survives_on_stale_raising(tmp_path):
    """The watcher is UNKILLABLE: an on_stale that raises must NOT kill the
    daemon thread — the server must never end up outliving the editor because a
    single shutdown attempt threw (rules.md "never outlive the editor")."""
    data_dir = tmp_path / "data"
    data_dir.mkdir(parents=True, exist_ok=True)
    # A heartbeat older than the staleness threshold (stays stale every tick).
    fresh_heartbeat(data_dir, age_seconds=120.0)

    calls = []
    succeeded = threading.Event()

    def flaky_on_stale():
        calls.append(1)
        if len(calls) == 1:
            raise RuntimeError("boom: shutdown attempt failed once")
        succeeded.set()

    watcher = app_mod.HeartbeatWatcher(
        data_dir=str(data_dir),
        check_interval=0.05,
        staleness=1.0,
        on_stale=flaky_on_stale,
    )
    watcher.start()
    try:
        # The first stale-fire raises; the loop must swallow it and re-fire on the
        # next tick (proving the thread survived the exception).
        assert succeeded.wait(timeout=3.0), (
            "watcher thread died after on_stale raised — it must keep checking"
        )
        assert len(calls) >= 2
    finally:
        watcher.stop()


# --------------------------------------------------------------------------- #
# Supersede check (EUD-042): a quick editor restart spawns a NEW server that
# rewrites server.ready with a NEW token. The OLD server keeps seeing the SAME
# (now restart-refreshed) heartbeat, so staleness alone never fires and the old
# server leaks (zombie + bge-m3 GPU memory + races the new server for srv-* IPC
# files). The watcher MUST also self-terminate when server.ready carries a token
# that differs from THIS process's own token — and on THAT exit path it must NOT
# delete server.ready (it now belongs to the new server). Token (not pid) is
# authoritative (EUD-037: launcher vs child pid is ambiguous).
# --------------------------------------------------------------------------- #


def _write_ready(data_dir, *, token, port=1):
    """Write a server.ready carrying ``token`` (UTF-8 no BOM)."""
    (data_dir / "server.ready").write_text(
        json.dumps({"port": port, "token": token}), encoding="utf-8"
    )


def test_supersede_fires_and_keeps_ready_when_token_differs(tmp_path):
    """A server.ready owned by a NEWER server (different token) self-terminates
    THIS server even with a perfectly fresh heartbeat — AND the dying server must
    NOT delete server.ready (it belongs to the new server)."""
    data_dir = tmp_path / "data"
    data_dir.mkdir(parents=True, exist_ok=True)
    # Fresh heartbeat: staleness must NOT be the reason this fires.
    fresh_heartbeat(data_dir, age_seconds=0.0)
    _write_ready(data_dir, token="NEW-server-token")

    fired = threading.Event()

    watcher = app_mod.HeartbeatWatcher(
        data_dir=str(data_dir),
        check_interval=0.05,
        staleness=600.0,  # generous: staleness can't be what trips it
        own_token="OLD-server-token",
        on_stale=fired.set,
    )
    watcher.start()
    try:
        assert fired.wait(timeout=3.0), (
            "watcher never fired when a newer server owned server.ready"
        )
    finally:
        watcher.stop()

    # Watcher-level: the supersede path never deletes ready itself and records
    # the decision so the lifespan shutdown hook can honor it.
    ready_path = data_dir / "server.ready"
    assert ready_path.is_file(), (
        "superseded server deleted server.ready — it belongs to the new server"
    )
    assert watcher.superseded is True, (
        "watcher did not record the supersede decision for the shutdown hook"
    )

    # Full production exit path: once uvicorn exits, FastAPI fires the lifespan
    # shutdown hook, which calls _shutdown_cleanup(watcher, delete_ready). That
    # MUST stop the watcher but SKIP deleting the new server's ready file when
    # superseded — exercise the exact conditional the hook runs.
    deleted = []

    def _delete_ready():
        deleted.append(True)
        ready_path.unlink(missing_ok=True)

    app_mod._shutdown_cleanup(watcher, _delete_ready)

    assert not deleted, (
        "shutdown hook deleted ready on the superseded exit path "
        "(must be skipped — the file belongs to the new server)"
    )
    assert ready_path.is_file(), (
        "server.ready clobbered by the lifespan shutdown on a superseded exit"
    )
    assert json.loads(ready_path.read_text(encoding="utf-8"))["token"] == (
        "NEW-server-token"
    )


def test_shutdown_cleanup_deletes_ready_on_graceful_exit(tmp_path):
    """A TRUE graceful shutdown (NOT superseded) still deletes server.ready: the
    departing server must not leave a stale ready file behind. This guards the
    EUD-042 conditional from over-firing (skipping deletion only when superseded,
    never on a normal editor-exit/SIGTERM shutdown)."""
    data_dir = tmp_path / "data"
    data_dir.mkdir(parents=True, exist_ok=True)
    ready_path = data_dir / "server.ready"
    _write_ready(data_dir, token="MY-token")

    watcher = app_mod.HeartbeatWatcher(
        data_dir=str(data_dir),
        check_interval=0.05,
        staleness=600.0,
        own_token="MY-token",
        on_stale=lambda: None,
    )
    # Never started/fired: superseded stays False (a graceful exit).
    assert watcher.superseded is False

    app_mod._shutdown_cleanup(watcher, lambda: ready_path.unlink(missing_ok=True))

    assert not ready_path.is_file(), (
        "graceful shutdown did not delete server.ready (the supersede skip "
        "must not apply to a normal exit)"
    )


def test_no_supersede_when_token_is_own(tmp_path):
    """server.ready carrying THIS server's OWN token must NOT trip the watcher
    (a fresh heartbeat keeps us alive; we own the data dir)."""
    data_dir = tmp_path / "data"
    data_dir.mkdir(parents=True, exist_ok=True)
    _write_ready(data_dir, token="MY-token")

    fired = threading.Event()
    stop_refresh = threading.Event()

    def refresher():
        while not stop_refresh.is_set():
            fresh_heartbeat(data_dir, age_seconds=0.0)
            time.sleep(0.05)

    rt = threading.Thread(target=refresher, daemon=True)
    rt.start()

    watcher = app_mod.HeartbeatWatcher(
        data_dir=str(data_dir),
        check_interval=0.05,
        staleness=2.0,
        own_token="MY-token",
        on_stale=fired.set,
    )
    watcher.start()
    try:
        time.sleep(0.6)
        assert not fired.is_set(), (
            "watcher fired on a server.ready carrying our OWN token"
        )
    finally:
        watcher.stop()
        stop_refresh.set()
        rt.join(timeout=2.0)


def test_no_supersede_when_ready_missing(tmp_path):
    """A missing server.ready is NO decision (transient: the bridge deletes the
    stale ready at init before the new server writes one). With a fresh
    heartbeat, staleness stays the only rule and the watcher does NOT fire."""
    data_dir = tmp_path / "data"
    data_dir.mkdir(parents=True, exist_ok=True)
    # No server.ready at all.

    fired = threading.Event()
    stop_refresh = threading.Event()

    def refresher():
        while not stop_refresh.is_set():
            fresh_heartbeat(data_dir, age_seconds=0.0)
            time.sleep(0.05)

    rt = threading.Thread(target=refresher, daemon=True)
    rt.start()

    watcher = app_mod.HeartbeatWatcher(
        data_dir=str(data_dir),
        check_interval=0.05,
        staleness=2.0,
        own_token="MY-token",
        on_stale=fired.set,
    )
    watcher.start()
    try:
        time.sleep(0.6)
        assert not fired.is_set(), (
            "watcher fired on a MISSING server.ready (must be no-decision)"
        )
    finally:
        watcher.stop()
        stop_refresh.set()
        rt.join(timeout=2.0)


def test_no_supersede_when_ready_unparsable(tmp_path):
    """A corrupt/partial server.ready (mid-write by the new server) is NO
    decision — staleness remains the fallback. With a fresh heartbeat the watcher
    does NOT fire."""
    data_dir = tmp_path / "data"
    data_dir.mkdir(parents=True, exist_ok=True)
    (data_dir / "server.ready").write_text("{not valid json", encoding="utf-8")

    fired = threading.Event()
    stop_refresh = threading.Event()

    def refresher():
        while not stop_refresh.is_set():
            fresh_heartbeat(data_dir, age_seconds=0.0)
            time.sleep(0.05)

    rt = threading.Thread(target=refresher, daemon=True)
    rt.start()

    watcher = app_mod.HeartbeatWatcher(
        data_dir=str(data_dir),
        check_interval=0.05,
        staleness=2.0,
        own_token="MY-token",
        on_stale=fired.set,
    )
    watcher.start()
    try:
        time.sleep(0.6)
        assert not fired.is_set(), (
            "watcher fired on an UNPARSABLE server.ready (must be no-decision)"
        )
    finally:
        watcher.stop()
        stop_refresh.set()
        rt.join(timeout=2.0)
