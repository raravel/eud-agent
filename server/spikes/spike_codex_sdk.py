r"""EUD-053 spike: official Codex Python SDK + eud-tools MCP round-trip (Windows).

De-risks the v2 agent core "Engine (single path)" BEFORE any dependent code.
Proves, in order, on THIS Windows machine:

  1. SDK import works            -> openai-codex  (module: openai_codex)
  2. mcp package import works    -> mcp
  3. codex CLI resolvable        -> shutil.which("codex")  (rules.md: never bare)
  4. thread start + run + stream -> consume events, record kinds
  5. ONE real MCP tool round-trip via PER-THREAD config injection (no global
     `codex mcp add`): the dummy echo_marker tool is invoked; proven by a
     sentinel file + the marker in the final response + an mcpToolCall item
  6. thread RESUME continues context (turn 2 references turn 1)
  7. measured cold-start latency (SDK init -> first event) + per-tool-call latency

This is NOT a pytest test (it spends real codex tokens). It lives outside
server/tests/ so pytest never collects it. Run explicitly with the server venv:

    server\.venv\Scripts\python.exe server\spikes\spike_codex_sdk.py

Exits non-zero with a clear "[stepN] ..." message at the first failing step.
"""

from __future__ import annotations

import asyncio
import os
import shutil
import sys
import tempfile
import time
from pathlib import Path

SPIKE_DIR = Path(__file__).resolve().parent
DUMMY_TOOL = SPIKE_DIR / "dummy_mcp_tool.py"
MARKER_TEXT = "EUD53"
EXPECTED_MARKER = f"EUD53-ECHO::{MARKER_TEXT}"


def fail(step: str, msg: str) -> None:
    print(f"FAIL [{step}] {msg}", file=sys.stderr)
    raise SystemExit(1)


def ok(step: str, msg: str) -> None:
    print(f"OK   [{step}] {msg}")


def main() -> None:
    # Windows: the codex .cmd shim is spawned by the SDK over a subprocess; the
    # asyncio default on Windows must be the ProactorEventLoop (rules.md / EUD-016)
    # so subprocess pipes work. Python 3.8+ already defaults to Proactor on
    # Windows, but make it explicit for the spike record.
    if sys.platform == "win32":
        asyncio.set_event_loop_policy(asyncio.WindowsProactorEventLoopPolicy())

    # --- step 1: SDK import -------------------------------------------------
    try:
        import openai_codex  # noqa: F401
        from openai_codex import Codex, CodexConfig
    except Exception as e:  # noqa: BLE001
        fail("step1", f"cannot import openai_codex (the official Codex SDK): {e!r}")
    ok("step1", f"openai_codex imported (version {openai_codex.__version__})")

    # --- step 2: mcp import -------------------------------------------------
    try:
        import importlib.metadata as _md

        import mcp  # noqa: F401
        from mcp.server.fastmcp import FastMCP  # noqa: F401
    except Exception as e:  # noqa: BLE001
        fail("step2", f"cannot import mcp (Model Context Protocol SDK): {e!r}")
    try:
        _mcp_ver = _md.version("mcp")
    except Exception:  # noqa: BLE001
        _mcp_ver = getattr(mcp, "__version__", "?")
    ok("step2", f"mcp imported (version {_mcp_ver})")

    # --- step 3: codex CLI resolvable --------------------------------------
    # rules.md: NEVER spawn bare 'codex'. Resolve to the .cmd shim via which;
    # CODEX_CMD may override with a full path. We then hand this resolved path to
    # the SDK via CodexConfig.codex_bin so it uses the BYO authenticated CLI
    # (not the bundled openai-codex-cli-bin).
    codex_bin = os.environ.get("CODEX_CMD") or shutil.which("codex")
    if not codex_bin or not Path(codex_bin).exists():
        fail("step3", "codex CLI not resolvable via shutil.which / CODEX_CMD")
    ok("step3", f"codex resolved: {codex_bin}")

    if not DUMMY_TOOL.exists():
        fail("step5", f"dummy MCP tool missing at {DUMMY_TOOL}")

    # Sentinel file: ground truth that the tool ran in-process.
    sentinel = Path(tempfile.gettempdir()) / "eud53_mcp_sentinel.txt"
    sentinel.unlink(missing_ok=True)

    # Per-thread MCP attachment via config injection (NO global `codex mcp add`).
    # The SDK passes this `config` dict straight to the codex app-server, which
    # reads mcp_servers exactly like ~/.codex/config.toml's [mcp_servers.<name>].
    # command/args/env mirror the stdio MCP server spec.
    thread_config = {
        "mcp_servers": {
            "eud53dummy": {
                "command": sys.executable,
                "args": ["-u", str(DUMMY_TOOL)],
                "env": {"EUD53_SENTINEL": str(sentinel)},
            }
        }
    }

    # codex_bin -> use the machine's authenticated CLI instead of the bundled bin.
    cfg = CodexConfig(codex_bin=codex_bin)

    event_kinds: dict[str, int] = {}
    cold_start_s: float | None = None
    tool_call_s: float | None = None

    t_init = time.perf_counter()
    try:
        with Codex(config=cfg) as codex:
            # --- step 4: thread start + run + stream events ----------------
            thread = codex.thread_start(config=thread_config)
            ok("step4", f"thread started: id={thread.id}")

            # --- step 5: real MCP tool round-trip (streamed) --------------
            t_tool0 = time.perf_counter()
            turn = thread.turn(
                f"Call the echo_marker tool with text '{MARKER_TEXT}'. "
                "Then reply with exactly the string the tool returned."
            )
            saw_mcp_item = False
            first_event = True
            final_text_parts: list[str] = []
            for event in turn.stream():
                if first_event:
                    cold_start_s = time.perf_counter() - t_init
                    first_event = False
                method = getattr(event, "method", "<no-method>")
                event_kinds[method] = event_kinds.get(method, 0) + 1
                if method == "item/completed":
                    root = event.payload.item.root
                    rtype = getattr(root, "type", None)
                    if rtype == "mcpToolCall":
                        saw_mcp_item = True
                        if tool_call_s is None:
                            tool_call_s = time.perf_counter() - t_tool0
                        print(
                            f"     mcpToolCall: server={getattr(root, 'server', '?')} "
                            f"tool={getattr(root, 'tool', '?')} "
                            f"status={getattr(root, 'status', '?')}"
                        )
                    elif rtype == "agentMessage":
                        final_text_parts.append(getattr(root, "text", "") or "")

            final_text = " ".join(p for p in final_text_parts if p)
            ok("step4", f"streamed events: {event_kinds}")

            # Triple proof the tool was REALLY invoked.
            sentinel_ok = sentinel.exists() and sentinel.read_text(
                encoding="utf-8"
            ).startswith("called:")
            marker_ok = EXPECTED_MARKER in final_text
            if not (sentinel_ok or saw_mcp_item or marker_ok):
                fail(
                    "step5",
                    "no evidence the echo_marker tool was invoked "
                    f"(sentinel={sentinel_ok}, mcp_item={saw_mcp_item}, "
                    f"marker_in_reply={marker_ok}); final_text={final_text!r}",
                )
            ok(
                "step5",
                f"MCP round-trip via config injection: sentinel={sentinel_ok}, "
                f"mcp_item={saw_mcp_item}, marker_in_reply={marker_ok}",
            )

            # --- step 6: thread RESUME continues context ------------------
            resumed = codex.thread_resume(thread.id)
            follow = resumed.turn(
                "What text did you echo before? Reply with just that text."
            ).run()
            follow_text = follow.final_response or ""
            if MARKER_TEXT not in follow_text:
                fail(
                    "step6",
                    "resumed thread did not retain context "
                    f"(expected '{MARKER_TEXT}' in reply); got {follow_text!r}",
                )
            ok("step6", f"resume retained context: reply mentions '{MARKER_TEXT}'")
    except SystemExit:
        raise
    except Exception as e:  # noqa: BLE001
        fail("step4-6", f"SDK/MCP runtime error: {e!r}")

    # --- step 7: measurements ---------------------------------------------
    cs = f"{cold_start_s:.3f}s" if cold_start_s is not None else "n/a"
    tc = f"{tool_call_s:.3f}s" if tool_call_s is not None else "n/a"
    ok("step7", f"cold_start (init->first event)={cs}; first tool-call latency={tc}")

    print("\nSPIKE PASSED: codex SDK + eud-tools MCP round-trip works on Windows.")
    print(f"  event kinds: {event_kinds}")
    print(f"  cold_start={cs}  tool_call_latency={tc}")


if __name__ == "__main__":
    main()
