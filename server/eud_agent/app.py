"""FastAPI app, WebSocket endpoint, and the boot/shutdown lifecycle.

The resident local server (architecture.md "Boot and lifecycle" + "WebSocket
protocol", rules.md "Server and panel"). It serves the BUILT React panel from
``panel/dist`` (features/02 React re-plan), exposes the panel WebSocket, and runs
the lifecycle threads that make the server a well-behaved child of the editor:

  * **server.ready** is written ATOMICALLY (temp + ``os.replace``, UTF-8 no BOM)
    ONLY after a background thread confirms the server's OWN socket accepts a TCP
    connection (the panel must never be told a port that does not yet listen). It
    is deleted on graceful shutdown AND by the heartbeat watcher on staleness.
  * **WS accept** validates the ``token`` query param AND the ``Origin`` header
    (``http://127.0.0.1:<port>``) BEFORE ``accept``; either mismatch closes the
    socket with code 4403. Two clients (same token) are allowed; events broadcast.
  * **HeartbeatWatcher** self-terminates the server when ``heartbeat.txt`` goes
    stale (>``staleness``) or stays missing past a ``grace`` — the server must
    never outlive the editor. Intervals are injectable for tests.
  * **RAG warmup** is kicked at startup (``rag.start_warmup``) and NEVER gates
    readiness (rules.md / Decision 01); progress is broadcast as ``rag_warmup``.

Port policy (rules.md): bind ``127.0.0.1`` only; the configured port, falling
back to an OS-assigned ephemeral port (0) when taken. ``resolve_bound_socket``
pre-binds the listener so the ACTUAL port is known before uvicorn starts and is
the single value written to ``server.ready`` / used in the Origin check.
"""

from __future__ import annotations

import asyncio
import json
import os
import socket
import sys
import threading
import time
from datetime import UTC, datetime
from pathlib import Path

from fastapi import FastAPI, WebSocket
from fastapi.responses import FileResponse, HTMLResponse, JSONResponse
from fastapi.staticfiles import StaticFiles
from starlette.websockets import WebSocketDisconnect, WebSocketState

from . import rag
from .bridge_io import BridgeIO
from .codex_client import CodexClient, CodexNotFound
from .config import Config
from .orchestrator import Orchestrator

# Lifecycle defaults (rules.md: heartbeat check 15s, staleness 60s; ready poll is
# a fast internal confirm). All are injectable via create_app for tests.
DEFAULT_READY_POLL_INTERVAL = 0.1
DEFAULT_READY_TIMEOUT = 30.0
DEFAULT_HEARTBEAT_CHECK_INTERVAL = 15.0
DEFAULT_HEARTBEAT_STALENESS = 60.0
DEFAULT_HEARTBEAT_GRACE = 60.0

_PANEL_NOT_BUILT = (
    "panel not built — run npm run build in panel/"
)


# --------------------------------------------------------------------------- #
# Port resolution (bind 127.0.0.1 only; fall back to ephemeral).
# --------------------------------------------------------------------------- #


def resolve_bound_socket(port: int, *, host: str = "127.0.0.1") -> socket.socket:
    """Bind ``host:port``; on failure (or port 0) fall back to an ephemeral port.

    Returns the bound, listening socket. The caller reads the actual port from
    ``sock.getsockname()[1]`` — for an OS-assigned port (the fallback) that is
    the single source of truth written into ``server.ready``. The socket is
    handed to uvicorn (``Server.run(sockets=[sock])``) so there is exactly one
    bind and no race between "pick a port" and "tell the panel the port".
    """
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    try:
        sock.bind((host, port))
    except OSError:
        # Configured port taken -> OS-assigned ephemeral port (rules.md policy).
        sock.bind((host, 0))
    sock.listen()
    sock.setblocking(False)
    return sock


# --------------------------------------------------------------------------- #
# server.ready writer (only after the socket actually accepts a connection).
# --------------------------------------------------------------------------- #


def _atomic_write_json(path: Path, payload: dict) -> None:
    """Write JSON to ``path`` atomically, UTF-8 without BOM (rules.md).

    temp file + ``os.replace`` so a reader (the bridge) never sees a half file;
    ``encoding="utf-8"`` only (``utf-8-sig`` is forbidden).
    """
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_bytes(json.dumps(payload).encode("utf-8"))
    os.replace(tmp, path)


def wait_for_socket_then_write_ready(
    *,
    host: str,
    port: int,
    ready_path: Path,
    token: str,
    poll_interval: float,
    timeout: float,
    stop_event: threading.Event | None = None,
) -> bool:
    """Connect to the server's OWN socket until it accepts, then write ready.

    rules.md: ``server.ready`` is written only AFTER confirming the socket
    accepts connections (the panel availability contract). Returns True when the
    file was written, False on timeout / stop. The write is atomic and carries
    ``{port, pid, token, started_at}``.
    """
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if stop_event is not None and stop_event.is_set():
            return False
        try:
            with socket.create_connection((host, port), timeout=poll_interval):
                pass
        except OSError:
            time.sleep(poll_interval)
            continue
        # The socket accepts -> safe to advertise it.
        _atomic_write_json(
            ready_path,
            {
                "port": port,
                "pid": os.getpid(),
                "token": token,
                "started_at": datetime.now(UTC).isoformat(),
            },
        )
        return True
    return False


# --------------------------------------------------------------------------- #
# Heartbeat watcher: a standalone, testable component.
# --------------------------------------------------------------------------- #


def _read_heartbeat_age(path: Path) -> float | None:
    """Seconds since the heartbeat timestamp, or None when absent/unparsable."""
    try:
        text = path.read_text(encoding="utf-8").strip()
    except OSError:
        return None
    try:
        ts = datetime.fromisoformat(text)
    except ValueError:
        return None
    if ts.tzinfo is None:
        ts = ts.replace(tzinfo=UTC)
    return (datetime.now(UTC) - ts).total_seconds()


class HeartbeatWatcher:
    """Background thread that fires ``on_stale`` when the heartbeat goes stale.

    The bridge writes ``heartbeat.txt`` every Tick (architecture.md). The server
    self-terminates when that file is older than ``staleness`` (or stays missing
    past ``grace`` after the watcher starts) — it must never outlive the editor.
    The thread invokes ``on_stale`` exactly once, then exits.

    Designed as a testable unit: ``check_interval``/``staleness``/``grace`` are
    parameters (so tests use tiny values, no real 15s/60s waits) and ``on_stale``
    is a plain callback (so tests assert firing without killing the process).
    """

    def __init__(
        self,
        *,
        data_dir: str | os.PathLike,
        check_interval: float,
        staleness: float,
        on_stale,
        grace: float = DEFAULT_HEARTBEAT_GRACE,
    ) -> None:
        self.data_dir = Path(data_dir)
        self.heartbeat_path = self.data_dir / "heartbeat.txt"
        self.ready_path = self.data_dir / "server.ready"
        self.check_interval = check_interval
        self.staleness = staleness
        self.grace = grace
        self._on_stale = on_stale
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None
        self._started_at = 0.0

    def start(self) -> None:
        self._started_at = time.monotonic()
        self._thread = threading.Thread(
            target=self._run, name="heartbeat-watcher", daemon=True
        )
        self._thread.start()

    def stop(self) -> None:
        self._stop.set()
        if self._thread is not None:
            self._thread.join(timeout=2.0)

    def _is_stale(self) -> bool:
        age = _read_heartbeat_age(self.heartbeat_path)
        if age is None:
            # Missing/unparsable heartbeat: tolerate until the grace elapses
            # (the bridge may not have written the first one yet at boot).
            return (time.monotonic() - self._started_at) >= self.grace
        return age >= self.staleness

    def _run(self) -> None:
        while not self._stop.wait(self.check_interval):
            # The watcher must be UNKILLABLE: an unforeseen exception in
            # _is_stale()/_on_stale() must NEVER silently kill this daemon thread
            # (that would let the server outlive the editor forever — the exact
            # rules.md violation, undiagnosable in the field). Swallow + continue
            # so the next tick re-evaluates staleness.
            try:
                if not self._is_stale():
                    continue
                # The server must never outlive the editor: delete server.ready
                # ourselves (so the bridge sees the server gone) BEFORE invoking
                # the shutdown callback. Deletion is owned by the watcher so the
                # contract holds regardless of what on_stale does.
                try:
                    self.ready_path.unlink(missing_ok=True)
                except OSError:
                    pass
                self._on_stale()
                return
            except Exception as exc:  # noqa: BLE001 - watcher must not die
                print(
                    f"[heartbeat-watcher] check failed, continuing: {exc}",
                    file=sys.stderr,
                )


# --------------------------------------------------------------------------- #
# App factory.
# --------------------------------------------------------------------------- #


def create_app(
    cfg: Config,
    *,
    start_lifecycle: bool = True,
    ready_poll_interval: float = DEFAULT_READY_POLL_INTERVAL,
    ready_timeout: float = DEFAULT_READY_TIMEOUT,
    heartbeat_check_interval: float = DEFAULT_HEARTBEAT_CHECK_INTERVAL,
    heartbeat_staleness: float = DEFAULT_HEARTBEAT_STALENESS,
    heartbeat_grace: float = DEFAULT_HEARTBEAT_GRACE,
) -> FastAPI:
    """Build the FastAPI app for ``cfg``.

    ``start_lifecycle`` controls the boot/shutdown threads (the ready-writer,
    RAG warmup, heartbeat watcher): True for a real run, False for in-process
    TestClient HTTP/WS-surface tests. All intervals/thresholds are injectable.
    """
    data_dir = Path(cfg.data_dir)
    dist_dir = Path(cfg.repo_root) / "panel" / "dist"
    origin_ok = f"http://127.0.0.1:{cfg.port}"

    app = FastAPI(title="eud-agent")

    # ----- shared connection registry (broadcast to all WS clients) -----
    clients: set[WebSocket] = set()
    clients_lock = threading.Lock()

    async def broadcast(event: dict) -> None:
        """Deliver an event dict to every connected, open WS client."""
        with clients_lock:
            targets = list(clients)
        for ws in targets:
            if ws.application_state != WebSocketState.CONNECTED:
                continue
            try:
                await ws.send_json(event)
            except Exception:  # noqa: BLE001 - a dead client must not break others
                pass

    # The bridge + codex are shared singletons (one editor instance per machine).
    bridge = BridgeIO(cfg.data_dir)
    try:
        codex = CodexClient(cfg.codex_cmd, cfg.repo_root)
    except CodexNotFound:
        # codex unresolved is a real possibility (selfcheck reports it); keep the
        # server up so the panel still loads and instruct surfaces a clean error.
        codex = None

    orchestrator = Orchestrator(
        bridge, codex, rag_db=cfg.rag_db, send=broadcast
    )

    # ----------------------------------------------------------------- routes
    @app.get("/healthz")
    async def healthz() -> JSONResponse:
        return JSONResponse({"status": "ok"})

    @app.get("/")
    async def root():
        index = dist_dir / "index.html"
        if not index.is_file():
            # React panel not built yet (features/02 503 path).
            return HTMLResponse(content=_PANEL_NOT_BUILT, status_code=503)
        return FileResponse(str(index))

    # Static mount for the built assets — only when the dir exists (a missing
    # dist must not raise at app build; the 503 path covers "not built").
    if dist_dir.is_dir():
        app.mount(
            "/assets",
            StaticFiles(directory=str(dist_dir / "assets"), check_dir=False),
            name="assets",
        )

    @app.websocket("/ws")
    async def ws_endpoint(websocket: WebSocket) -> None:
        # Validate token query param AND Origin header BEFORE accept (rules.md).
        token = websocket.query_params.get("token")
        origin = websocket.headers.get("origin")
        if token != cfg.token or origin != origin_ok:
            await websocket.close(code=4403)
            return
        await websocket.accept()
        with clients_lock:
            clients.add(websocket)
        try:
            await _serve_ws(websocket, orchestrator)
        except WebSocketDisconnect:
            pass
        finally:
            with clients_lock:
                clients.discard(websocket)

    # -------------------------------------------------------------- lifecycle
    if start_lifecycle:
        ready_path = data_dir / "server.ready"
        shutdown_state = {"server": None}  # uvicorn Server set in __main__ path

        def _delete_ready() -> None:
            try:
                ready_path.unlink(missing_ok=True)
            except OSError:
                pass

        def _on_stale() -> None:
            # Self-terminate: delete the ready file and ask uvicorn to exit.
            _delete_ready()
            srv = shutdown_state.get("server")
            if srv is not None:
                srv.should_exit = True

        watcher = HeartbeatWatcher(
            data_dir=cfg.data_dir,
            check_interval=heartbeat_check_interval,
            staleness=heartbeat_staleness,
            grace=heartbeat_grace,
            on_stale=_on_stale,
        )
        # Exposed so the entry point can hand uvicorn's Server in for shutdown
        # and so the lifespan can stop the watcher.
        app.state.shutdown_state = shutdown_state
        app.state.delete_ready = _delete_ready

        @app.on_event("startup")
        async def _startup() -> None:  # noqa: D401 - lifecycle hook
            # Capture the running loop so off-loop threads (RAG warmup) can
            # schedule broadcasts back onto it (best-effort; never gates ready).
            loop = asyncio.get_running_loop()

            # Clear any stale server-side IPC files (rules.md startup cleanup).
            try:
                bridge.cleanup_stale()
            except OSError:
                pass
            # Delete any pre-existing ready file (a clean boot owns this).
            _delete_ready()

            # ready-writer: confirm our own socket accepts, THEN write ready.
            def _ready_run() -> None:
                wait_for_socket_then_write_ready(
                    host="127.0.0.1",
                    port=cfg.port,
                    ready_path=ready_path,
                    token=cfg.token,
                    poll_interval=ready_poll_interval,
                    timeout=ready_timeout,
                )

            threading.Thread(
                target=_ready_run, name="ready-writer", daemon=True
            ).start()

            # RAG warmup (never gates readiness) — broadcast progress. The rag
            # callback fires from the warmup thread, so hop back to the loop.
            def _warmup_progress(stage: str, state: str, detail: str | None) -> None:
                ev: dict = {"type": "progress", "stage": stage, "detail": state}
                if detail:
                    ev["detail"] = f"{state}: {detail}"
                try:
                    asyncio.run_coroutine_threadsafe(broadcast(ev), loop)
                except Exception:  # noqa: BLE001 - progress is best-effort
                    pass

            rag.start_warmup(cfg.rag_db, on_progress=_warmup_progress)

            # Heartbeat watcher: self-terminate when the editor stops ticking.
            watcher.start()

        @app.on_event("shutdown")
        def _shutdown() -> None:  # noqa: D401 - lifecycle hook
            watcher.stop()
            _delete_ready()

    return app


# --------------------------------------------------------------------------- #
# WS message loop.
# --------------------------------------------------------------------------- #


async def _serve_ws(websocket: WebSocket, orchestrator: Orchestrator) -> None:
    """Dispatch client messages to the orchestrator (architecture.md protocol).

    Client -> server: ``instruct`` / ``apply`` / ``status`` / ``list``. Each
    handler emits its events through the orchestrator's broadcaster, so both
    connected clients see them. Unknown types get a clean error (never a crash).
    """
    while True:
        msg = await websocket.receive_json()
        mtype = msg.get("type")
        if mtype == "instruct":
            await orchestrator.instruct(
                msg.get("instruction", ""),
                target=msg.get("target", ""),
                use_context=bool(msg.get("useContext", False)),
            )
        elif mtype == "apply":
            await orchestrator.apply(
                mode=msg.get("mode", "set"),
                target=msg.get("target", ""),
                code=msg.get("code", ""),
            )
        elif mtype == "status":
            await orchestrator.status()
        elif mtype == "list":
            await orchestrator.list_files()
        else:
            await websocket.send_json(
                {"type": "error", "message": f"unknown message type: {mtype!r}"}
            )
