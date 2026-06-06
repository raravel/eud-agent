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

from fastapi import FastAPI, Request, WebSocket
from fastapi.responses import FileResponse, HTMLResponse, JSONResponse
from fastapi.staticfiles import StaticFiles
from starlette.websockets import WebSocketDisconnect, WebSocketState

from . import rag
from .agent_runner import CodexSDKRunner
from .bridge_io import BridgeIO
from .chk_info import MapInfoService
from .config import Config
from .debuglog import DebugLog
from .engine import AgentEngine
from .tools import ToolError, ToolLayer

#: Outbound event types the debug trail records (turn ends). Streaming
#: `agent_event` deltas and progress chatter are deliberately excluded.
_DEBUG_TURN_END_TYPES = frozenset({"answer", "plan", "changeset", "error"})

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
    ``{port, pid, ppid, token, started_at}`` (``ppid`` lets the bridge accept
    ownership when it spawned the server through the venv launcher, which
    re-execs the base interpreter as a child -- the bridge owns the launcher pid).
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
                "ppid": os.getppid(),
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
    """Background thread that self-terminates the server on two conditions.

    The bridge writes ``heartbeat.txt`` every Tick (architecture.md). The server
    self-terminates when EITHER:

      * **stale heartbeat** — ``heartbeat.txt`` is older than ``staleness`` (or
        stays missing past ``grace`` after the watcher starts); the editor has
        stopped ticking and the server must never outlive it. On this path the
        watcher deletes ``server.ready`` (the file belongs to THIS, departing,
        server) before invoking ``on_stale``.
      * **superseded** (EUD-042) — ``server.ready`` exists, parses, and carries a
        ``token`` that differs from this process's ``own_token``: a NEWER server
        owns the data dir (a quick editor restart re-spawned us). The OLD server
        keeps seeing the restart-refreshed heartbeat, so staleness alone never
        fires and zombies leak (bge-m3 GPU memory + racing srv-* IPC files). Here
        the watcher MUST NOT delete ``server.ready`` — it now belongs to the new
        server. Token (not pid) is authoritative (EUD-037: launcher vs child pid
        is ambiguous). A missing/unparsable ready file is NO decision (transient:
        the bridge deletes a stale ready before the new server writes one;
        corrupt = a mid-write read) — staleness remains the fallback.

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
        own_token: str | None = None,
    ) -> None:
        self.data_dir = Path(data_dir)
        self.heartbeat_path = self.data_dir / "heartbeat.txt"
        self.ready_path = self.data_dir / "server.ready"
        self.check_interval = check_interval
        self.staleness = staleness
        self.grace = grace
        self.own_token = own_token
        # Set True exactly once, on the superseded exit path, BEFORE on_stale is
        # invoked (the watcher thread is the only writer). The lifespan shutdown
        # hook reads it to SKIP deleting server.ready — that file now belongs to
        # the new server (EUD-042). A true graceful shutdown leaves this False.
        self.superseded = False
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

    def _is_superseded(self) -> bool:
        """True only when server.ready confidently belongs to ANOTHER server.

        Reads ``server.ready``, parses it as JSON, and compares its ``token`` to
        ``own_token``: a differing token means a NEWER server owns the data dir
        (EUD-042). Token (not pid) is authoritative. ANY ambiguity is NO decision
        (return False, staleness stays the fallback): no ``own_token`` set, a
        missing file (transient — the bridge deletes a stale ready before the new
        server writes one), an unreadable/unparsable file (a mid-write read), or
        a payload without a ``token``.
        """
        if self.own_token is None:
            return False
        try:
            text = self.ready_path.read_text(encoding="utf-8")
        except OSError:
            return False  # missing/unreadable -> no decision
        try:
            data = json.loads(text)
        except ValueError:
            return False  # corrupt/partial mid-write -> no decision
        if not isinstance(data, dict):
            return False
        other = data.get("token")
        if other is None:
            return False
        return other != self.own_token

    def _run(self) -> None:
        while not self._stop.wait(self.check_interval):
            # The watcher must be UNKILLABLE: an unforeseen exception in
            # _is_stale()/_is_superseded()/_on_stale() must NEVER silently kill
            # this daemon thread (that would let the server outlive the editor
            # forever — the exact rules.md violation, undiagnosable in the field).
            # Swallow + continue so the next tick re-evaluates.
            try:
                # Supersede check FIRST (EUD-042): on a quick editor restart the
                # new server refreshes the SAME heartbeat, so staleness alone
                # never fires for the old server. A differing server.ready token
                # means a newer server owns the data dir -> self-terminate, but
                # do NOT delete server.ready (it belongs to the new server).
                if self._is_superseded():
                    print(
                        "[heartbeat-watcher] superseded by a newer server "
                        "(server.ready token differs); self-terminating "
                        "WITHOUT deleting server.ready.",
                        file=sys.stderr,
                    )
                    # Mark BEFORE on_stale so the lifespan shutdown hook (which
                    # fires once uvicorn exits) skips deleting the new server's
                    # ready file.
                    self.superseded = True
                    self._on_stale()
                    return
                if not self._is_stale():
                    continue
                # The server must never outlive the editor: delete server.ready
                # ourselves (so the bridge sees the server gone) BEFORE invoking
                # the shutdown callback. Deletion is owned by the watcher so the
                # contract holds regardless of what on_stale does. (Skipped for
                # the superseded path above — that ready file is not ours.)
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
    runner_factory=None,
) -> FastAPI:
    """Build the FastAPI app for ``cfg``.

    ``start_lifecycle`` controls the boot/shutdown threads (the ready-writer,
    RAG warmup, heartbeat watcher): True for a real run, False for in-process
    TestClient HTTP/WS-surface tests. All intervals/thresholds are injectable.

    ``runner_factory`` (v2, EUD-056) builds the per-session ``AgentRunner``; it is
    injectable so WS tests run with a ``FakeRunner`` (no codex). The default builds
    a real :class:`CodexSDKRunner`. Its signature is
    ``factory(*, tool_layer, send, build_system_prompt) -> AgentRunner``.
    """
    data_dir = Path(cfg.data_dir)
    dist_dir = Path(cfg.repo_root) / "panel" / "dist"
    origin_ok = f"http://127.0.0.1:{cfg.port}"

    app = FastAPI(title="eud-agent")

    # Persistent debug trail (chat inputs / tool calls / turn ends) under
    # <data_dir>/logs/agent-YYYYMMDD.jsonl. Construction runs the retention
    # cleanup; logging is best-effort and never breaks the serving flow.
    debug_log = DebugLog(cfg.data_dir)
    app.state.debug_log = debug_log

    # ----- shared connection registry (broadcast to all WS clients) -----
    clients: set[WebSocket] = set()
    clients_lock = threading.Lock()

    async def broadcast(event: dict) -> None:
        """Deliver an event dict to every connected, open WS client."""
        # Debug trail: record turn-end events only (no streaming deltas).
        if event.get("type") in _DEBUG_TURN_END_TYPES:
            debug_log.log("server", event)
        with clients_lock:
            targets = list(clients)
        for ws in targets:
            if ws.application_state != WebSocketState.CONNECTED:
                continue
            try:
                await ws.send_json(event)
            except Exception:  # noqa: BLE001 - a dead client must not break others
                pass

    # The bridge is a shared singleton (one editor instance per machine).
    bridge = BridgeIO(cfg.data_dir)

    # Per-map-project memory (features/07). The project name comes from the LIVE
    # bridge STATUS and can change mid-session (no project is open at boot), so the
    # store must be re-resolved on every access rather than constructed once.
    # ``_resolve_memory`` parses the current STATUS and builds a fresh
    # :class:`ProjectMemory` (an empty project name -> a DISABLED store that reads
    # ``""`` / rejects writes / renders "(no project memory)"). It is best-effort:
    # a STATUS failure resolves to a disabled store so memory never breaks a turn.
    def _resolve_memory():
        from .engine import parse_status
        from .memory import ProjectMemory

        try:
            _, project = parse_status(bridge.status())
        except Exception:  # noqa: BLE001 - no project resolvable -> disabled store
            project = ""
        return ProjectMemory(data_dir=cfg.data_dir, project_name=project)

    # ToolLayer (tools.py) and Journal (journal.py) take a STATIC ``memory``
    # reference, but the project resolves PER USE — so they get this thin proxy
    # that forwards every attribute/method access (``enabled`` / ``write`` /
    # ``read`` / ``store_dir`` / ``update_list_hash`` — the surface those consumers
    # touch) to a store resolved AT THAT MOMENT from the live STATUS. This is the
    # only app.py-local seam that keeps the project name live without editing
    # tools.py / journal.py (EUD-081 scope: app.py only).
    class _LiveProjectMemory:
        """Delegates the ProjectMemory surface to a freshly resolved store.

        Each attribute access constructs a new :class:`ProjectMemory` for the
        CURRENT project (live STATUS), so a mid-session project switch is followed
        without any cached state. Covers both the codex ``memory_write`` path
        (``enabled`` / ``write`` / ``update_list_hash``) and the journal
        snapshot/rollback path (``enabled`` / ``read`` / ``write`` / ``store_dir``).
        """

        def __getattr__(self, name):
            return getattr(_resolve_memory(), name)

    live_memory = _LiveProjectMemory()

    # The eud-tools tool layer (features/05): the policy layer for codex's tool
    # calls. The MCP shim is dumb transport that forwards to /tools/call below;
    # ALL validation / gate / budget / journaling live here in the FastAPI
    # process. The journal (EUD-055) snapshots every write so a turn's changeset
    # can be assembled + rolled back. Exposed on app.state so the WS engine and
    # the v2 runner read the same per-request state.
    def _journal_factory(request_id: str):
        from .journal import Journal

        return Journal(
            data_dir=cfg.data_dir, request_id=request_id, bridge=bridge,
            memory=live_memory,
        )

    # map_info service (features/08): digests the connected map's CHK via the
    # IsomTerrain.exe extractor. Always constructed — an unresolvable exe path
    # degrades ONLY the map_info tool (clear ToolError at call time, the same
    # advisory shape as epscript-lsp), never the boot.
    # data_dir hosts the location_write map backups (features/09 rollback).
    map_info_service = MapInfoService(
        bridge, isomterrain_cmd=cfg.isomterrain_cmd, data_dir=cfg.data_dir
    )

    tool_layer = ToolLayer(
        bridge, journal_factory=_journal_factory, memory=live_memory,
        map_info=map_info_service,
    )
    app.state.tool_layer = tool_layer

    # The WS tests swap app.state.tool_layer (a fake-bridge ToolLayer) AFTER
    # build; expose a rebind hook so the engine picks up the swapped layer.
    def _rebind_tool_layer(new_layer) -> None:
        app.state.tool_layer = new_layer

    app.state.rebind_tool_layer = _rebind_tool_layer

    # Live request-id stamping (EUD-064): the ACTIVE panel session publishes its
    # CURRENT request id here. The tool endpoint stamps it onto every tool call,
    # overriding the (stale) shim-supplied id; None means no active session, so the
    # shim id is the fallback (legacy headless runner). One editor instance per
    # machine (a single supported session at a time), so a plain holder suffices.
    app.state.active_request_id = None

    def _register_request_id(request_id: str | None) -> None:
        app.state.active_request_id = request_id

    # RAG warmup state holder. Warmup progress is broadcast-only, but "started"
    # usually fires BEFORE the panel connects (server boot) and a reloaded panel
    # misses "done" entirely — the warmup callback maintains the CURRENT state
    # here and the WS endpoint replays it to every newly accepted client (the
    # panel gates its send on it). None = no warmup ran (tests / in-process):
    # nothing is replayed, the panel fails open.
    app.state.rag_warmup_state = None

    # v2 agent-runner factory (EUD-056). Injectable for tests (FakeRunner); the
    # default builds a real CodexSDKRunner over the resolved codex binary.
    def _default_runner_factory(*, tool_layer, send, build_system_prompt):
        return CodexSDKRunner(
            tool_layer=tool_layer,
            send=send,
            build_system_prompt=build_system_prompt,
            codex_bin=cfg.codex_cmd,
            data_dir=cfg.data_dir,
        )

    make_runner = runner_factory or _default_runner_factory

    # ----------------------------------------------------------------- routes
    @app.get("/healthz")
    async def healthz() -> JSONResponse:
        return JSONResponse({"status": "ok"})

    # ----- eud-tools endpoints (token-authenticated, 127.0.0.1 only) -----
    # The MCP shim forwards here with the server.ready token. rules.md "Server and
    # panel": token-validated; never 0.0.0.0 (the server binds 127.0.0.1 only, so
    # these are loopback-only by construction). A wrong/missing token -> 401; a
    # tool/validation/gate/budget failure is a tool RESULT (ok=false), not an HTTP
    # 5xx, so codex sees a correctable error rather than a transport crash.
    @app.get("/tools/list")
    async def tools_list(token: str | None = None) -> JSONResponse:
        if token != cfg.token:
            return JSONResponse({"error": "unauthorized"}, status_code=401)
        return JSONResponse({"tools": tool_layer.tool_specs()})

    @app.post("/tools/call")
    async def tools_call(request: Request) -> JSONResponse:
        try:
            payload = await request.json()
        except Exception:  # noqa: BLE001 - malformed body -> clean 400
            return JSONResponse({"error": "invalid JSON body"}, status_code=400)
        if not isinstance(payload, dict) or payload.get("token") != cfg.token:
            return JSONResponse({"error": "unauthorized"}, status_code=401)
        # Live request-id stamping (EUD-064): for an ACTIVE panel session the
        # engine's CURRENT request id wins, ignoring the shim-supplied id (pinned
        # at thread creation, stale from the second resumed chat). With NO active
        # session (active_request_id is None) the shim id is the fallback (legacy
        # headless runner / no-session calls).
        active_request_id = app.state.active_request_id
        if active_request_id is not None:
            request_id = active_request_id
        else:
            request_id = str(payload.get("request_id") or "default")
        tool = payload.get("tool")
        args = payload.get("args") or {}
        if not isinstance(tool, str):
            return JSONResponse(
                {"ok": False, "error": "missing 'tool' name"}
            )
        # Debug trail: the full (untruncated) call — the panel's agent_event
        # payloads are truncated, this is the authoritative record.
        debug_log.log(
            "tool_call",
            {"request_id": request_id, "tool": tool, "args": args},
        )
        # Read the (possibly test-swapped) tool layer from app.state so the same
        # per-request journal/gate the engine uses backs these forwarded calls.
        active_tool_layer = app.state.tool_layer
        # The handlers do blocking file-IPC (bridge); run off the event loop. A
        # ToolError (bad args / gate / budget / bridge ERROR) is returned as a
        # tool result, never raised to a 5xx.
        try:
            result = await asyncio.to_thread(
                active_tool_layer.call_for_request, request_id, tool, args
            )
        except ToolError as exc:
            debug_log.log(
                "tool_result",
                {
                    "request_id": request_id,
                    "tool": tool,
                    "ok": False,
                    "error": str(exc),
                },
            )
            return JSONResponse({"ok": False, "error": str(exc)})
        debug_log.log(
            "tool_result",
            {"request_id": request_id, "tool": tool, "ok": True,
             "result": result},
        )
        return JSONResponse({"ok": True, "result": result})

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
        # Replay the current RAG warmup state to the new client (broadcasts
        # alone miss late joiners — the panel send-gate needs the snapshot).
        warm = getattr(app.state, "rag_warmup_state", None)
        if warm is not None:
            try:
                await websocket.send_json(
                    {"type": "progress", "stage": "rag_warmup", "detail": warm}
                )
            except Exception:  # noqa: BLE001 - snapshot is best-effort
                pass
        # Per-connection v2 engine: the WS state machine (idle -> triage ->
        # answer | apply | plan_review* -> executing -> changeset_review -> idle)
        # driven by client messages. It owns the per-session AgentRunner and reads
        # the (possibly test-swapped) tool layer + bridge for project state / RAG.
        engine = AgentEngine(
            send=broadcast,
            make_runner=make_runner,
            get_tool_layer=lambda: app.state.tool_layer,
            bridge=bridge,
            rag_db=cfg.rag_db,
            register_request_id=_register_request_id,
            # features/07: enables the [project memory] prompt section (resumed
            # turns) and episode recording at finalization. The project name is
            # re-resolved per turn from the bridge STATUS inside the engine.
            data_dir=cfg.data_dir,
        )
        try:
            await _serve_ws(
                websocket, engine, bridge=bridge, data_dir=cfg.data_dir,
                debug_log=debug_log,
            )
        except WebSocketDisconnect:
            pass
        finally:
            await engine.aclose()
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
            # Self-terminate: ask uvicorn to exit. Deletion of server.ready is
            # OWNED BY THE WATCHER (it deletes on the staleness path but MUST
            # skip it on the superseded path — EUD-042 — where the ready file
            # belongs to the new server). The callback must therefore NOT delete
            # it here; doing so would clobber the new server's ready file.
            srv = shutdown_state.get("server")
            if srv is not None:
                srv.should_exit = True

        watcher = HeartbeatWatcher(
            data_dir=cfg.data_dir,
            check_interval=heartbeat_check_interval,
            staleness=heartbeat_staleness,
            grace=heartbeat_grace,
            on_stale=_on_stale,
            own_token=cfg.token,
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
                # Keep the holder in sync so the WS endpoint can replay the
                # CURRENT state to clients that connect after this broadcast.
                app.state.rag_warmup_state = ev["detail"]
                try:
                    asyncio.run_coroutine_threadsafe(broadcast(ev), loop)
                except Exception:  # noqa: BLE001 - progress is best-effort
                    pass

            rag.start_warmup(cfg.rag_db, on_progress=_warmup_progress)

            # Heartbeat watcher: self-terminate when the editor stops ticking.
            watcher.start()

        @app.on_event("shutdown")
        def _shutdown() -> None:  # noqa: D401 - lifecycle hook
            _shutdown_cleanup(watcher, _delete_ready)

    return app


def _shutdown_cleanup(watcher: HeartbeatWatcher, delete_ready) -> None:
    """Run the lifespan-shutdown cleanup, honoring the supersede contract.

    ALWAYS stop the watcher thread. Delete ``server.ready`` ONLY when this exit
    is NOT a supersede (EUD-042): a true graceful shutdown (editor exit ->
    staleness, or a normal SIGTERM) leaves ``watcher.superseded`` False and the
    departing server removes its own ready file; a SUPERSEDED exit leaves it True
    and we MUST NOT delete it — that file belongs to the NEW server that
    re-spawned us. Factored out so it is testable without driving the FastAPI
    shutdown event end-to-end.
    """
    watcher.stop()
    if not watcher.superseded:
        delete_ready()


# --------------------------------------------------------------------------- #
# WS message loop.
# --------------------------------------------------------------------------- #


async def _serve_ws(
    websocket: WebSocket,
    engine: AgentEngine,
    *,
    bridge=None,
    data_dir: str | os.PathLike | None = None,
    debug_log: DebugLog | None = None,
) -> None:
    """Dispatch WS v2 client messages to the per-connection engine.

    Client -> server (features/05 "WS protocol v2"): ``chat`` / ``plan_feedback``
    / ``plan_approve`` / ``changeset_decision`` / ``cancel`` / ``status`` /
    ``list``. The v1 ``instruct``/``apply`` messages are REMOVED — they fall
    through to the unknown-type error (no compat shim). Every event the engine
    emits flows through the broadcaster so both connected clients see it.

    The project-memory surface (features/07 "WS protocol additions") —
    ``memory_get`` / ``memory_save`` — is handled HERE, before delegating to the
    engine. Deviation note: ``status``/``list`` route through the engine's
    ``handle()`` dispatch, but engine.py is out of EUD-081 scope, so these two are
    handled in this loop instead; the spec's "route like status/list" is satisfied
    at the protocol level (same WS message family), not the dispatch site. The
    reply is sent directly on this socket (memory replies are not turn-end events,
    so they skip the broadcast/debug-trail turn-end path).
    """
    while True:
        msg = await websocket.receive_json()
        # Debug trail: every inbound client message, verbatim (chat text,
        # plan feedback/approve, decisions, status/list, memory_get/save).
        if debug_log is not None:
            debug_log.log("client", msg)
        mtype = msg.get("type")
        if mtype == "memory_get":
            await _handle_memory_get(websocket, bridge, data_dir)
            continue
        if mtype == "memory_save":
            await _handle_memory_save(websocket, bridge, data_dir, msg)
            continue
        await engine.handle(msg)


#: Episodes returned in a ``memory_get`` reply (the prompt section uses its own
#: smaller limit). features/07 "WS protocol additions": last 50, newest FIRST.
_MEMORY_WS_EPISODE_LIMIT = 50


def _resolve_ws_memory(bridge, data_dir):
    """Build a :class:`ProjectMemory` for the CURRENT project (live STATUS).

    Mirrors ``create_app``'s in-process resolver for the WS handlers: parse the
    bridge STATUS for the project name and construct a fresh store (an empty name
    -> a DISABLED store). Best-effort: a STATUS failure yields a disabled store.
    Runs the blocking bridge IO off the event loop at the call site.
    """
    from .engine import parse_status
    from .memory import ProjectMemory

    try:
        _, project = parse_status(bridge.status())
    except Exception:  # noqa: BLE001 - no project resolvable -> disabled store
        project = ""
    return ProjectMemory(data_dir=data_dir, project_name=project)


async def _handle_memory_get(websocket: WebSocket, bridge, data_dir) -> None:
    """``memory_get {}`` -> ``memory {project, files, episodes}`` (features/07).

    ``files`` is the four markdown files (absent -> ``""``); ``episodes`` is the
    last 50, NEWEST FIRST (the store returns newest LAST, so reverse). No project
    open (disabled store) -> ``error``. File IO runs off the event loop.
    """
    from .memory import MEMORY_FILES

    def _read():
        store = _resolve_ws_memory(bridge, data_dir)
        if not store.enabled:
            return None
        files = {name: store.read(name) for name in MEMORY_FILES}
        # read_episodes returns newest LAST; the WS payload is newest FIRST.
        episodes = list(reversed(store.read_episodes(_MEMORY_WS_EPISODE_LIMIT)))
        return store.project_name, files, episodes

    try:
        result = await asyncio.to_thread(_read)
    except Exception as exc:  # noqa: BLE001 - surface as a clean WS error
        await websocket.send_json({"type": "error", "message": str(exc)})
        return
    if result is None:
        await websocket.send_json(
            {"type": "error", "message": "no project is open; memory is disabled"}
        )
        return
    project, files, episodes = result
    await websocket.send_json(
        {"type": "memory", "project": project, "files": files,
         "episodes": episodes}
    )


async def _handle_memory_save(
    websocket: WebSocket, bridge, data_dir, msg: dict
) -> None:
    """``memory_save {file, content}`` -> ``memory_saved {file}`` (features/07).

    A DIRECT store write (NOT journaled — a user editing their own memory is not
    an agent mutation), same file enum + 8 KB cap as ``memory_write``. A disabled
    store / unknown file / oversize content -> ``error`` carrying the store's
    ``WriteResult.reason``. File IO runs off the event loop.
    """
    file = str(msg.get("file", ""))
    content = str(msg.get("content", ""))

    def _write():
        store = _resolve_ws_memory(bridge, data_dir)
        return store.write(file, content)

    try:
        result = await asyncio.to_thread(_write)
    except Exception as exc:  # noqa: BLE001 - surface as a clean WS error
        await websocket.send_json({"type": "error", "message": str(exc)})
        return
    if not result.ok:
        await websocket.send_json({"type": "error", "message": result.reason})
        return
    await websocket.send_json({"type": "memory_saved", "file": file})
