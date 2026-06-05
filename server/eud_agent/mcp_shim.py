r"""eud-tools MCP shim: the stdio MCP server codex spawns.

Launched as ``python -m eud_agent.mcp_shim``.

DUMB TRANSPORT ONLY (features/05 "Engine" + architecture.md). codex attaches this
stdio server via per-thread config injection (proven by the EUD-053 spike); it
forwards every tool call as an HTTP POST to the running FastAPI server over
``127.0.0.1`` carrying the ``server.ready`` token. ALL tool logic, validation,
the mutation gate, budgets, and (later) journaling live in the FastAPI process —
this file holds NO tool logic, NO validation, NO whitelist. It only:

  1. reads ``server.ready`` (port + token) from the editor's ``Data\agent``;
  2. fetches the tool specs from ``GET /tools/list?token=...`` and registers one
     FastMCP tool per spec (name + description + params schema as advertised);
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


def build_server():  # pragma: no cover - exercised only at real shim runtime
    """Construct the FastMCP stdio server with one tool per server-advertised spec.

    Kept out of unit-test coverage (it spawns a real loopback HTTP client against
    the running server); the unit tests exercise ``read_ready`` / the URL builders
    / ``forward_call`` (the transport seam) directly, and the round-trip is proven
    by the EUD-053 spike.
    """
    from mcp.server.fastmcp import FastMCP

    ready_path = _ready_path_from_env()
    port, token = read_ready(ready_path)
    request_id = os.environ.get("EUD_REQUEST_ID") or f"shim-{uuid.uuid4().hex[:8]}"

    server = FastMCP("eud-tools")

    specs = fetch_tool_specs(port, token)
    for spec in specs:
        name = spec["name"]
        description = spec.get("description", "")

        def _make(tool_name: str, tool_doc: str):
            def _tool(args: dict | None = None) -> object:
                # DUMB TRANSPORT: forward verbatim; the server validates + gates.
                return forward_call(port, token, request_id, tool_name, args or {})

            _tool.__name__ = tool_name
            _tool.__doc__ = tool_doc
            return _tool

        # Register the forwarder under the advertised tool name. The server's
        # params schema is the source of truth; the shim passes args through.
        server.add_tool(_make(name, description), name=name, description=description)

    return server


def main() -> None:  # pragma: no cover - real stdio runtime only
    try:
        server = build_server()
    except Exception as exc:  # noqa: BLE001 - surface a clear startup error
        print(f"[eud-tools shim] startup failed: {exc}", file=sys.stderr)
        raise SystemExit(1) from exc
    server.run(transport="stdio")


if __name__ == "__main__":  # pragma: no cover
    main()
