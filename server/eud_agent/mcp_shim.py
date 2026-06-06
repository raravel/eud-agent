r"""eud-tools MCP shim: the stdio MCP server codex spawns.

Launched as ``python -m eud_agent.mcp_shim``.

DUMB TRANSPORT ONLY (features/05 "Engine" + architecture.md). codex attaches this
stdio server via per-thread config injection (proven by the EUD-053 spike); it
forwards every tool call as an HTTP POST to the running FastAPI server over
``127.0.0.1`` carrying the ``server.ready`` token. ALL tool logic, validation,
the mutation gate, budgets, and (later) journaling live in the FastAPI process —
this file holds NO tool logic, NO validation, NO whitelist. It only:

  1. reads ``server.ready`` (port + token) from the editor's ``Data\agent``;
  2. fetches the tool specs from ``GET /tools/list?token=...`` and advertises one
     MCP tool per spec — name + description + the server's params JSON schema
     VERBATIM as ``inputSchema`` (lowlevel server, EUD-087: FastMCP derived the
     schema from the wrapper signature, hiding the real parameter names from
     codex);
  3. forwards each invocation to ``POST /tools/call`` with the token and a stable
     ``request_id`` (one per shim process = one codex thread/turn session), and
     returns the server's ``result`` (or raises the server's ``error`` so codex
     sees a correctable tool error).

Authentication: the token is required by the server; an unauthenticated request
is rejected server-side (rules.md "Server and panel": token-validated, 127.0.0.1
only). The shim never binds a socket and never talks to anything but loopback.

The ``server.ready`` location comes from the ``EUD_DATA_DIR`` env var (the editor's
``Data\agent``) — codex's per-thread config injection sets it in the shim's env
(mirroring the spike's ``EUD53_SENTINEL`` pattern). ``EUD_REQUEST_ID`` may pin the
request id; otherwise a uuid is generated at process start.
"""

from __future__ import annotations

import json
import os
import sys
import uuid
from pathlib import Path

# httpx is a dependency of the SDK/fastapi stack (transitively present); used here
# for the synchronous loopback forward. The shim is a separate, short-lived
# process spawned by codex, so a sync client is simplest and correct.
import httpx

HOST = "127.0.0.1"
DEFAULT_TIMEOUT = 300.0  # build_run can run long; the server owns the real budget


def read_ready(ready_path: str | os.PathLike) -> tuple[int, str]:
    """Return ``(port, token)`` from a ``server.ready`` file (UTF-8, no BOM).

    Raises ``RuntimeError`` with a clear message when the file is missing or
    malformed — the shim cannot forward anything without the port + token.
    """
    p = Path(ready_path)
    try:
        data = json.loads(p.read_text(encoding="utf-8"))
    except OSError as exc:
        raise RuntimeError(f"server.ready not readable at {p}: {exc}") from exc
    except ValueError as exc:
        raise RuntimeError(f"server.ready malformed at {p}: {exc}") from exc
    if not isinstance(data, dict) or "port" not in data or "token" not in data:
        raise RuntimeError(f"server.ready missing port/token at {p}: {data!r}")
    return int(data["port"]), str(data["token"])


def tools_call_url(port: int) -> str:
    """The loopback tool-call endpoint URL (127.0.0.1 only)."""
    return f"http://{HOST}:{port}/tools/call"


def tools_list_url(port: int) -> str:
    """The loopback tool-spec endpoint URL (127.0.0.1 only)."""
    return f"http://{HOST}:{port}/tools/list"


def _ready_path_from_env() -> Path:
    """Locate ``server.ready`` from ``EUD_DATA_DIR`` (the editor's Data\\agent)."""
    data_dir = os.environ.get("EUD_DATA_DIR")
    if not data_dir:
        raise RuntimeError(
            "EUD_DATA_DIR not set; the shim needs the editor's Data\\agent dir "
            "to locate server.ready (set via codex per-thread config injection)."
        )
    return Path(data_dir) / "server.ready"


def fetch_tool_specs(port: int, token: str, *, timeout: float = 30.0) -> list[dict]:
    """GET the tool specs from the server (token-authenticated)."""
    resp = httpx.get(tools_list_url(port), params={"token": token}, timeout=timeout)
    resp.raise_for_status()
    return resp.json()["tools"]


def forward_call(
    port: int,
    token: str,
    request_id: str,
    tool: str,
    args: dict,
    *,
    timeout: float = DEFAULT_TIMEOUT,
) -> object:
    """Forward one tool call to the server; return its ``result`` or raise on error.

    The server returns ``{"ok": true, "result": ...}`` or
    ``{"ok": false, "error": "..."}``. On ``ok=false`` we raise so FastMCP reports
    a tool error to codex (a correctable result, not a transport failure).
    """
    resp = httpx.post(
        tools_call_url(port),
        json={"token": token, "request_id": request_id, "tool": tool, "args": args},
        timeout=timeout,
    )
    if resp.status_code in (401, 403):
        raise RuntimeError(f"unauthenticated tool call rejected by server: {tool}")
    resp.raise_for_status()
    body = resp.json()
    if not body.get("ok"):
        raise RuntimeError(body.get("error") or f"tool {tool} failed")
    return body.get("result")


def build_server():
    """Construct the lowlevel MCP stdio server advertising the REAL tool schemas.

    EUD-087: the previous FastMCP registration wrapped every tool as
    ``_tool(args: dict | None)`` — FastMCP derives the inputSchema from the
    function SIGNATURE, so codex only ever saw a single untyped ``args`` object
    and invented its own parameter names (``{"table", "field", "id"}`` instead
    of ``{"dat", "name", "objId"}``), failing server-side validation over and
    over. The lowlevel :class:`mcp.server.lowlevel.Server` lets the shim
    advertise the server's ``parameters`` JSON schema VERBATIM (dumb transport,
    now for the schema too); ``call_tool`` validates incoming args against that
    schema before forwarding (``validate_input`` defaults to True).

    The loopback HTTP forward runs in a worker thread (``anyio.to_thread``) so
    a long tool call (build_run up to 300s) never blocks the MCP session loop.
    """
    import anyio
    import mcp.types as types
    from mcp.server.lowlevel import Server

    ready_path = _ready_path_from_env()
    port, token = read_ready(ready_path)
    request_id = os.environ.get("EUD_REQUEST_ID") or f"shim-{uuid.uuid4().hex[:8]}"

    server = Server("eud-tools")
    specs = fetch_tool_specs(port, token)

    @server.list_tools()
    async def _list_tools() -> list[types.Tool]:
        return [
            types.Tool(
                name=s["name"],
                description=s.get("description", ""),
                inputSchema=s.get("parameters")
                or {"type": "object", "properties": {}},
            )
            for s in specs
        ]

    @server.call_tool()
    async def _call_tool(name: str, arguments: dict | None):
        # DUMB TRANSPORT: forward verbatim; the server validates + gates. An
        # exception (the server's ok=false error) is converted by the lowlevel
        # server into an isError tool result codex can read and correct.
        result = await anyio.to_thread.run_sync(
            lambda: forward_call(port, token, request_id, name, arguments or {})
        )
        return [
            types.TextContent(
                type="text", text=json.dumps(result, ensure_ascii=False)
            )
        ]

    return server


def main() -> None:  # pragma: no cover - real stdio runtime only
    try:
        server = build_server()
    except Exception as exc:  # noqa: BLE001 - surface a clear startup error
        print(f"[eud-tools shim] startup failed: {exc}", file=sys.stderr)
        raise SystemExit(1) from exc

    import anyio
    from mcp.server.stdio import stdio_server

    async def _run() -> None:
        async with stdio_server() as (read, write):
            await server.run(
                read, write, server.create_initialization_options()
            )

    anyio.run(_run)


if __name__ == "__main__":  # pragma: no cover
    main()
