"""Dummy stdio MCP server for the EUD-053 codex SDK + MCP round-trip spike.

Exposes ONE tool, ``echo_marker(text)``, that:
  * returns a distinctive marker string (``EUD53-ECHO::<text>``), AND
  * writes a sentinel file whose path is given by the ``EUD53_SENTINEL``
    environment variable.

The sentinel file is the ground-truth proof that codex actually *invoked* the
tool (vs. merely echoing the marker text from the prompt). The spike asserts on
all three signals: sentinel file written, marker in final response, and an
``mcpToolCall`` thread item.

Run standalone for a smoke check::

    python -m mcp dev server/spikes/dummy_mcp_tool.py   # (optional, manual)

Codex spawns this via per-thread config injection (see spike_codex_sdk.py):
    command = <venv python>, args = ["-u", "<abs path to this file>"]
"""

from __future__ import annotations

import os
from pathlib import Path

from mcp.server.fastmcp import FastMCP

mcp = FastMCP("eud53-dummy")


@mcp.tool()
def echo_marker(text: str) -> str:
    """Echo the given text back wrapped in a distinctive EUD53 marker.

    Also writes a sentinel file (path from the EUD53_SENTINEL env var) recording
    the call, so the spike can prove the tool was really invoked.
    """
    sentinel = os.environ.get("EUD53_SENTINEL")
    if sentinel:
        Path(sentinel).write_text(f"called:{text}", encoding="utf-8")
    return f"EUD53-ECHO::{text}"


if __name__ == "__main__":
    # stdio transport: codex talks to this process over stdin/stdout.
    mcp.run(transport="stdio")
