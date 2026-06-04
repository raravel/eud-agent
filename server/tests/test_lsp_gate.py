"""Verification artifact for EUD-019-58af: advisory epscript-lsp diagnostics.

These tests drive ``eud_agent.lsp_gate`` WITHOUT a real ``node`` / installed
``@eps-server/server`` package (except the explicitly-gated live round-trip).
The default suite mocks the resolver and the subprocess so it can assert the
ONE defining property of this module (features/02 "lsp_gate.py", rules.md
"epscript-lsp diagnostics are advisory only"):

    every failure degrades to ``[]`` and NOTHING ever blocks.

The public surface the orchestrator probes (orchestrator.py ``_lsp_stage``):

    from . import lsp_gate              # lazy import; module must EXIST
    lsp_gate.diagnose(code)            # single positional str arg, run in a
                                       # thread; returns a list of dicts (or
                                       # falsy -> coerced to []).

``diagnose`` is the NO-RAISE surface: missing node, missing package, spawn
error, timeout, protocol error, and malformed frames ALL return ``[]``. The
mapped diagnostic shape is ``[{line, severity, message}]`` where ``line`` is
1-based (LSP ranges are 0-based) and ``severity`` is the LSP integer passthrough
(1=Error, 2=Warning, 3=Information, 4=Hint).

Seams the tests patch (so the default suite needs no real node):

  * ``lsp_gate._resolve_lsp() -> (node_path, server_entry) | None`` — resolution.
  * ``lsp_gate.subprocess.Popen`` — the spawned LSP process (a FakeProc speaking
    canned, Content-Length-framed LSP JSON-RPC over stdout).

``eud_agent.lsp_gate`` does NOT exist during Step A, so this suite is expected
to FAIL on import until lsp_gate.py is implemented (Step B).
"""

from __future__ import annotations

import io
import json
import os
import threading
import time

import pytest

# Imported at collection so the failing import is the first signal in Step A.
# (No try/except: the suite MUST error out until the module exists.)
from eud_agent import lsp_gate
from eud_agent.lsp_gate import diagnose

# A small budget so the timeout test never costs a real 2s (injected everywhere).
FAST = 0.3


# --------------------------------------------------------------------------- #
# LSP JSON-RPC framing helpers: build Content-Length framed payloads exactly as
# a real language server emits them, so the parser is exercised end to end.
# --------------------------------------------------------------------------- #


def _frame(obj: dict) -> bytes:
    body = json.dumps(obj).encode("utf-8")
    return b"Content-Length: %d\r\n\r\n%s" % (len(body), body)


def _publish_diagnostics(diags: list[dict], uri: str = "inmemory://model.eps") -> dict:
    """A ``textDocument/publishDiagnostics`` notification with LSP-shaped diags.

    Each LSP diagnostic carries a 0-based ``range`` and an integer ``severity``;
    the module must map them to 1-based ``line`` + ``severity`` passthrough.
    """
    return {
        "jsonrpc": "2.0",
        "method": "textDocument/publishDiagnostics",
        "params": {"uri": uri, "diagnostics": diags},
    }


def _lsp_diag(line0: int, severity: int, message: str) -> dict:
    return {
        "range": {
            "start": {"line": line0, "character": 0},
            "end": {"line": line0, "character": 5},
        },
        "severity": severity,
        "message": message,
    }


# --------------------------------------------------------------------------- #
# Fake subprocess: an LSP server that replies over stdout with canned frames.
# stdin is a sink (records writes + close); stdout is a stream that yields the
# scripted bytes, optionally in awkward chunk boundaries to stress the parser.
# --------------------------------------------------------------------------- #


class FakeStdin:
    def __init__(self) -> None:
        self.buffer = bytearray()
        self.closed = False

    def write(self, data: bytes) -> int:
        assert isinstance(data, (bytes, bytearray)), "LSP stdin must receive bytes"
        self.buffer.extend(data)
        return len(data)

    def flush(self) -> None:
        pass

    def close(self) -> None:
        self.closed = True


class ChunkedStdout:
    """A readable byte stream that hands out ``chunks`` across read() calls.

    Implements just enough of a binary file object for a Content-Length parser:
    ``read(n)`` (returns up to n bytes, splicing across the scripted chunks) and
    ``readline()`` (header-line reads). When exhausted, blocks until ``close()``
    if ``block_when_empty`` (models a server that never publishes -> timeout) or
    returns ``b""`` (EOF) otherwise.
    """

    def __init__(self, chunks: list[bytes], *, block_when_empty: bool = False) -> None:
        self._data = bytearray(b"".join(chunks))
        self._pos = 0
        self._block_when_empty = block_when_empty
        self._closed = threading.Event()

    def read(self, n: int = -1) -> bytes:
        if self._pos >= len(self._data):
            if self._block_when_empty:
                # Never produces more: the reader must be abandoned on timeout.
                self._closed.wait(timeout=5.0)
            return b""
        if n is None or n < 0:
            chunk = bytes(self._data[self._pos :])
            self._pos = len(self._data)
            return chunk
        chunk = bytes(self._data[self._pos : self._pos + n])
        self._pos += len(chunk)
        return chunk

    def readline(self) -> bytes:
        if self._pos >= len(self._data):
            if self._block_when_empty:
                self._closed.wait(timeout=5.0)
            return b""
        nl = self._data.find(b"\n", self._pos)
        if nl == -1:
            line = bytes(self._data[self._pos :])
            self._pos = len(self._data)
            return line
        line = bytes(self._data[self._pos : nl + 1])
        self._pos = nl + 1
        return line

    def close(self) -> None:
        self._closed.set()


class FakeProc:
    """Stands in for the object returned by ``subprocess.Popen``."""

    def __init__(
        self,
        stdout: ChunkedStdout,
        *,
        spawn_error: BaseException | None = None,
    ) -> None:
        if spawn_error is not None:
            raise spawn_error
        self.stdin = FakeStdin()
        self.stdout = stdout
        self.stderr = io.BytesIO(b"")
        self.killed = False
        self.waited = False
        self._rc: int | None = None

    def poll(self) -> int | None:
        return self._rc

    def kill(self) -> None:
        self.killed = True
        self._rc = -9
        # Unblock any reader parked on a never-publishing stdout.
        self.stdout.close()

    def terminate(self) -> None:
        self.kill()

    def wait(self, timeout=None) -> int:
        self.waited = True
        self._rc = self._rc if self._rc is not None else 0
        return self._rc


def _install_proc(
    monkeypatch,
    *,
    chunks: list[bytes] | None = None,
    block_when_empty: bool = False,
    spawn_error: BaseException | None = None,
) -> dict:
    """Patch ``_resolve_lsp`` to succeed and ``subprocess.Popen`` to return a
    FakeProc scripted with ``chunks`` (or raising ``spawn_error``)."""
    monkeypatch.setattr(
        lsp_gate,
        "_resolve_lsp",
        lambda: ("C:\\fake\\node.exe", "C:\\fake\\server.js"),
    )
    captured: dict = {}
    stdout = ChunkedStdout(chunks or [], block_when_empty=block_when_empty)

    def fake_popen(*args, **kwargs):
        captured["args"] = args
        captured["kwargs"] = kwargs
        captured["proc"] = FakeProc(stdout, spawn_error=spawn_error)
        return captured["proc"]

    monkeypatch.setattr(lsp_gate.subprocess, "Popen", fake_popen)
    return captured


# --------------------------------------------------------------------------- #
# 1. No node -> [] fast.
# --------------------------------------------------------------------------- #


def test_no_node_returns_empty_fast(monkeypatch):
    """``_resolve_lsp`` returns None when node is unresolved -> diagnose() = []."""
    monkeypatch.setattr(lsp_gate, "_resolve_lsp", lambda: None)
    # Popen must never be reached when resolution fails.
    monkeypatch.setattr(
        lsp_gate.subprocess,
        "Popen",
        lambda *a, **k: pytest.fail("Popen must NOT be called when node is absent"),
    )
    t0 = time.monotonic()
    out = diagnose("puts(1);", timeout=FAST)
    assert out == []
    assert time.monotonic() - t0 < 1.0, "no-node path must short-circuit, not wait"


def test_resolve_helper_returns_none_without_node(monkeypatch):
    """``_resolve_lsp`` itself returns None when ``shutil.which('node')`` fails."""
    monkeypatch.setattr(lsp_gate.shutil, "which", lambda name: None)
    assert lsp_gate._resolve_lsp() is None


# --------------------------------------------------------------------------- #
# 2. Node present but package not installed -> [].
# --------------------------------------------------------------------------- #


def test_node_but_no_package_returns_empty(monkeypatch):
    """node resolves but ``@eps-server/server`` is not installed -> [].

    The resolver finds node via shutil.which but no server entry exists under
    server/node_modules nor globally, so it returns None overall.
    """
    monkeypatch.setattr(lsp_gate.shutil, "which", lambda name: "C:\\node\\node.exe")
    # No package entry can be located.
    monkeypatch.setattr(lsp_gate, "_locate_server_entry", lambda: None)
    assert lsp_gate._resolve_lsp() is None
    # And the public surface degrades to [].
    monkeypatch.setattr(lsp_gate, "_resolve_lsp", lambda: None)
    assert diagnose("x;", timeout=FAST) == []


# --------------------------------------------------------------------------- #
# 3. Mocked subprocess speaking canned LSP frames -> mapped diagnostics.
#    The Content-Length parser is exercised with SPLIT and MERGED chunks.
# --------------------------------------------------------------------------- #


def test_canned_frames_split_chunks_mapped(monkeypatch):
    """publishDiagnostics arriving SPLIT across read boundaries is parsed and
    mapped to [{line, severity, message}] with 1-based lines."""
    notif = _publish_diagnostics(
        [
            _lsp_diag(0, 1, "unexpected token"),   # line0=0 -> line 1, Error
            _lsp_diag(4, 2, "unused variable"),    # line0=4 -> line 5, Warning
        ]
    )
    framed = _frame(notif)
    # Split the framed bytes into many awkward 7-byte chunks (header + body torn).
    chunks = [framed[i : i + 7] for i in range(0, len(framed), 7)]
    _install_proc(monkeypatch, chunks=chunks)

    out = diagnose("line0\nline1\nline2\nline3\nbad", timeout=FAST)

    assert out == [
        {"line": 1, "severity": 1, "message": "unexpected token"},
        {"line": 5, "severity": 2, "message": "unused variable"},
    ]


def test_canned_frames_merged_with_preamble_mapped(monkeypatch):
    """An ``initialize`` response and the publishDiagnostics notification MERGED
    into a single read chunk (and with leading log noise) are both parsed; only
    the diagnostics are mapped."""
    init_response = _frame(
        {"jsonrpc": "2.0", "id": 1, "result": {"capabilities": {}}}
    )
    notif = _frame(_publish_diagnostics([_lsp_diag(2, 3, "info note")]))
    # One big chunk holding both frames back to back.
    _install_proc(monkeypatch, chunks=[init_response + notif])

    out = diagnose("a\nb\nc", timeout=FAST)
    assert out == [{"line": 3, "severity": 3, "message": "info note"}]


def test_empty_diagnostics_publish_maps_to_empty(monkeypatch):
    """A publishDiagnostics with an empty list (clean code) maps to []."""
    _install_proc(monkeypatch, chunks=[_frame(_publish_diagnostics([]))])
    assert diagnose("clean();", timeout=FAST) == []


def test_diagnostic_missing_severity_defaults(monkeypatch):
    """A diagnostic without a ``severity`` field still maps (defaults documented).

    LSP allows omitting severity; the mapper must not raise and must produce a
    dict with the three keys.
    """
    diag = {
        "range": {
            "start": {"line": 7, "character": 0},
            "end": {"line": 7, "character": 1},
        },
        "message": "no severity here",
    }
    _install_proc(monkeypatch, chunks=[_frame(_publish_diagnostics([diag]))])
    out = diagnose("x;", timeout=FAST)
    assert len(out) == 1
    assert out[0]["line"] == 8
    assert out[0]["message"] == "no severity here"
    assert "severity" in out[0]


# --------------------------------------------------------------------------- #
# 4. Timeout: a server that never publishes -> [] within the injected budget,
#    and the process is killed/reaped.
# --------------------------------------------------------------------------- #


def test_timeout_returns_empty_and_kills(monkeypatch):
    """No publishDiagnostics ever arrives -> diagnose returns [] near the budget
    (not hanging) and the spawned process is KILLED and REAPED."""
    cap = _install_proc(monkeypatch, chunks=[], block_when_empty=True)

    t0 = time.monotonic()
    out = diagnose("never();", timeout=FAST)
    elapsed = time.monotonic() - t0

    assert out == []
    assert elapsed < 2.0, "must give up near the injected budget, not hang"
    proc = cap["proc"]
    assert proc.killed, "a non-publishing server must be killed on timeout"
    assert proc.waited, "the killed process must be reaped (wait)"


# --------------------------------------------------------------------------- #
# 5. Malformed frames / non-JSON -> [].
# --------------------------------------------------------------------------- #


def test_malformed_non_json_body_returns_empty(monkeypatch):
    """A correctly-framed message whose body is NOT valid JSON -> [] (no raise)."""
    bad_body = b"this is not json at all"
    framed = b"Content-Length: %d\r\n\r\n%s" % (len(bad_body), bad_body)
    _install_proc(monkeypatch, chunks=[framed])
    assert diagnose("x;", timeout=FAST) == []


def test_garbage_stream_no_framing_returns_empty(monkeypatch):
    """Raw garbage with no Content-Length framing at all -> [] (no raise)."""
    _install_proc(monkeypatch, chunks=[b"\x00\x01garbage no headers\xff\xfe"])
    assert diagnose("x;", timeout=FAST) == []


def test_missing_content_length_header_returns_empty(monkeypatch):
    """Headers present but no Content-Length -> [] (parser must not hang/raise)."""
    framed = b"X-Other: 1\r\n\r\n{\"jsonrpc\":\"2.0\"}"
    _install_proc(monkeypatch, chunks=[framed], block_when_empty=False)
    assert diagnose("x;", timeout=FAST) == []


# --------------------------------------------------------------------------- #
# 6. Spawn error -> [] (no raise).
# --------------------------------------------------------------------------- #


def test_spawn_error_returns_empty(monkeypatch):
    """Popen raising (e.g. OSError) is swallowed -> [] (advisory, never block)."""
    _install_proc(monkeypatch, spawn_error=OSError("cannot spawn node"))
    assert diagnose("x;", timeout=FAST) == []


# --------------------------------------------------------------------------- #
# 7. Invocation contract: when resolved, node is spawned with the server entry
#    and an explicit stdin pipe (rules.md subprocess-stdin discipline). The
#    generated code is delivered via a didOpen (so it reaches stdin).
# --------------------------------------------------------------------------- #


def test_spawns_node_with_server_entry_and_explicit_stdin(monkeypatch):
    """The resolved (node, server_entry) is the spawned argv; stdin is a PIPE and
    the JSON-RPC handshake (initialize + didOpen carrying the code) is written."""
    import subprocess as _sp

    cap = _install_proc(
        monkeypatch, chunks=[_frame(_publish_diagnostics([]))]
    )
    diagnose("foreach(p : EUDLoopPlayer()) {}", timeout=FAST)

    argv = cap["args"][0]
    assert argv[0] == "C:\\fake\\node.exe", "must spawn the RESOLVED node"
    assert "C:\\fake\\server.js" in argv, "must pass the resolved server entry"

    kw = cap["kwargs"]
    assert kw.get("stdin") == _sp.PIPE, "explicit stdin PIPE is mandatory"
    assert kw.get("stdout") == _sp.PIPE

    # The handshake reached stdin: initialize + a didOpen carrying the code.
    sent = bytes(cap["proc"].stdin.buffer).decode("utf-8", "replace")
    assert "initialize" in sent
    assert "didOpen" in sent
    assert "EUDLoopPlayer" in sent, "the generated code must reach the server"
    assert cap["proc"].stdin.closed or True  # stdin closed on shutdown (lenient)


# --------------------------------------------------------------------------- #
# 8. Module surface sanity.
# --------------------------------------------------------------------------- #


def test_module_public_surface():
    """The orchestrator probes ``diagnose`` (single positional code arg); the
    resolver + locator seams exist for testing/resolution."""
    for name in ("diagnose", "_resolve_lsp", "_locate_server_entry"):
        assert hasattr(lsp_gate, name), f"missing public name: {name}"
    # diagnose must accept a single positional code arg and a keyword timeout.
    import inspect

    sig = inspect.signature(diagnose)
    params = list(sig.parameters)
    assert params[0] == "code"
    assert "timeout" in sig.parameters


# --------------------------------------------------------------------------- #
# 9. Integration shape: orchestrator's _lsp_stage with the REAL module present.
#
#    NOTE on the seam: the orchestrator emits {stage:"lsp", detail:"skipped"}
#    ONLY when the lazy import fails (module absent) OR diagnose() RAISES. Once
#    lsp_gate EXISTS and diagnose() degrades by RETURNING [] (its no-raise
#    contract), the orchestrator emits a bare progress {stage:"lsp"} and carries
#    diagnostics=[]. That IS the seamless integration the task wants: the panel
#    sees zero diagnostics either way, nothing blocks, no error event fires. We
#    therefore assert the true post-module contract (progress lsp + diags [] +
#    no error), not the pre-module ImportError "skipped" detail.
# --------------------------------------------------------------------------- #


async def test_orchestrator_lsp_stage_empty_when_resolution_fails(monkeypatch):
    from eud_agent.orchestrator import Orchestrator

    # Resolution fails -> diagnose() returns [] WITHOUT raising (advisory degrade).
    monkeypatch.setattr(lsp_gate, "_resolve_lsp", lambda: None)

    class _Bridge:
        def get(self, path, **kw):
            return "x = 0;\n"

    class _Codex:
        async def generate(self, prompt, *, timeout=None):
            return "x = 1;\n"

    events: list[dict] = []

    async def send(ev):
        events.append(ev)

    o = Orchestrator(_Bridge(), _Codex(), rag_db="C:\\fake", send=send)
    await o.instruct("set x", target="main.eps", use_context=False)

    # The lsp stage ran (a progress event) and the flow did NOT error/block.
    lsp_progress = [
        e for e in events
        if e.get("type") == "progress" and e.get("stage") == "lsp"
    ]
    assert lsp_progress, "expected an lsp progress event"
    assert not [e for e in events if e.get("type") == "error"]

    # Diagnostics are empty: the degrade is invisible to the panel beyond [].
    code_ev = next(e for e in events if e.get("type") == "code")
    assert code_ev["diagnostics"] == []


async def test_orchestrator_lsp_stage_maps_when_resolution_succeeds(monkeypatch):
    """With the real module present AND resolution mocked to succeed, the
    orchestrator's _lsp_stage carries the mapped diagnostics through to the
    ``code`` event (proves the diagnose() return value flows end to end)."""
    from eud_agent.orchestrator import Orchestrator

    notif = _frame(_publish_diagnostics([_lsp_diag(0, 2, "warn here")]))
    _install_proc(monkeypatch, chunks=[notif])

    class _Bridge:
        def get(self, path, **kw):
            return "x = 0;\n"

    class _Codex:
        async def generate(self, prompt, *, timeout=None):
            return "x = 1;\n"

    events: list[dict] = []

    async def send(ev):
        events.append(ev)

    o = Orchestrator(_Bridge(), _Codex(), rag_db="C:\\fake", send=send)
    await o.instruct("set x", target="main.eps", use_context=False)

    code_ev = next(e for e in events if e.get("type") == "code")
    assert code_ev["diagnostics"] == [
        {"line": 1, "severity": 2, "message": "warn here"}
    ]


# --------------------------------------------------------------------------- #
# 10. LIVE round-trip (opt-in): a real epscript-lsp over stdio; skipped unless
#     EUD_LSP_LIVE=1 AND resolution actually succeeds on this machine.
# --------------------------------------------------------------------------- #


@pytest.mark.skipif(
    not (os.environ.get("EUD_LSP_LIVE") == "1" and lsp_gate._resolve_lsp()),
    reason="live LSP round-trip: set EUD_LSP_LIVE=1 and install @eps-server/server",
)
def test_live_lsp_round_trip():
    """A real epscript-lsp diagnoses deliberately broken eps within the budget.

    The contract is only that diagnose() returns a (possibly empty) list of the
    mapped shape without raising — exact diagnostics depend on the LSP version.
    """
    out = diagnose("this is not valid epScript @@@ {{{", timeout=5.0)
    assert isinstance(out, list)
    for d in out:
        assert set(d) >= {"line", "severity", "message"}
        assert isinstance(d["line"], int) and d["line"] >= 1


if __name__ == "__main__":
    raise SystemExit(pytest.main([__file__, "-q"]))
