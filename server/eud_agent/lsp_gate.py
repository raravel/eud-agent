"""Advisory epscript-lsp diagnostics (features/02 "lsp_gate.py (advisory, optional)").

THE defining property of this module (features/02, rules.md "epscript-lsp
diagnostics are advisory only: they annotate, never block apply; absence of
node/the package must not break the flow"):

    every failure degrades to ``[]`` and NOTHING ever blocks.

The orchestrator imports this lazily inside ``try/except ImportError`` and calls
``diagnose(code)`` via ``asyncio.to_thread`` (single positional ``str`` arg; no
keyword forwarded — so ``timeout`` is keyword-only with a 2.0s default). The
returned list is surfaced to the panel as the ``code`` event's ``diagnostics``.
Because the orchestrator coerces a falsy result to ``[]``, and because diagnostics
are advisory, ``diagnose`` is the NO-RAISE surface: missing node, missing package,
spawn error, timeout, protocol error, and malformed frames ALL return ``[]``.

When ``node`` resolves AND ``@eps-server/server`` is installed (under
``server/node_modules`` or the global npm root), the LSP is spawned over stdio
and driven through the minimal JSON-RPC handshake
``initialize -> initialized -> didOpen -> (publishDiagnostics) -> shutdown/exit``
with Content-Length framing, all inside a small (default 2s) budget. The process
is always reaped (kill + wait) on timeout.

Mapped diagnostic shape: ``{line, severity, message}`` where ``line`` is the
LSP 0-based ``range.start.line`` plus 1 (1-based), ``severity`` is the LSP
integer passthrough (1=Error, 2=Warning, 3=Information, 4=Hint; defaults to 3 /
Information when the server omits it), and ``message`` is the diagnostic text.

Resolution (``_resolve_lsp``): ``shutil.which("node")`` gives the node binary;
``_locate_server_entry`` looks for the ``@eps-server/server`` package first under
``server/node_modules`` (repo-local install) then under the global npm root
(``%ProgramFiles%\\nodejs\\node_modules`` / ``$NODE_PATH``), reading its
``package.json`` ``bin``/``main`` to find the launchable entry (falling back to a
conventional ``out/server.js`` / ``index.js`` existence check). Either piece
missing -> ``None`` overall -> ``diagnose`` returns ``[]``.

``shutil`` and ``subprocess`` are referenced as module attributes so tests can
monkeypatch ``lsp_gate.shutil.which`` and ``lsp_gate.subprocess.Popen``.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import threading
import time
from pathlib import Path

# LSP DiagnosticSeverity default when a server omits the field (3 = Information).
_DEFAULT_SEVERITY = 3
# The in-memory document URI we open the generated code under.
_DOC_URI = "inmemory://model.eps"
# epScript language id advertised in didOpen.
_LANGUAGE_ID = "epscript"

# The npm package that ships the language server (tech-stack.md: @eps-server/server
# 1.2.12, optional advisory diagnostics only).
_PACKAGE = os.path.join("@eps-server", "server")

# Conventional launchable entry filenames to probe when package.json gives no
# usable bin/main (kept permissive: this is best-effort advisory resolution).
_ENTRY_CANDIDATES = (
    os.path.join("out", "server.js"),
    os.path.join("dist", "server.js"),
    os.path.join("bin", "server.js"),
    "server.js",
    "index.js",
)


# --------------------------------------------------------------------------- #
# Resolution
# --------------------------------------------------------------------------- #


def _candidate_package_dirs() -> list[Path]:
    """Directories that might hold the ``@eps-server/server`` package.

    Order: repo-local ``server/node_modules`` first (an explicit dev install),
    then the global npm root (``npm root -g`` location on this machine is
    ``%ProgramFiles%\\nodejs\\node_modules``), then any ``NODE_PATH`` entries.
    """
    dirs: list[Path] = []

    # Repo-local: server/node_modules/@eps-server/server. This module lives at
    # server/eud_agent/lsp_gate.py, so the server dir is its parent's parent.
    server_dir = Path(__file__).resolve().parent.parent
    dirs.append(server_dir / "node_modules" / _PACKAGE)

    # Global npm root. We avoid shelling out to ``npm root -g`` (slow, and npm
    # may be absent); the conventional Windows location is next to node, and
    # NODE_PATH may add more.
    node_path = shutil.which("node")
    if node_path:
        node_root = Path(node_path).resolve().parent / "node_modules" / _PACKAGE
        dirs.append(node_root)
    program_files = os.environ.get("ProgramFiles")
    if program_files:
        dirs.append(
            Path(program_files) / "nodejs" / "node_modules" / _PACKAGE
        )
    for entry in (os.environ.get("NODE_PATH") or "").split(os.pathsep):
        if entry.strip():
            dirs.append(Path(entry.strip()) / _PACKAGE)

    return dirs


def _entry_from_package_json(pkg_dir: Path) -> Path | None:
    """Read ``package.json`` ``bin``/``main`` to find the launchable JS entry.

    ``bin`` may be a string or a mapping of command -> path; ``main`` is a single
    path. The first one that resolves to an existing file under ``pkg_dir`` wins.
    Any read/parse error returns None (the caller falls back to file probing).
    """
    pkg_json = pkg_dir / "package.json"
    try:
        data = json.loads(pkg_json.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None

    candidates: list[str] = []
    bin_field = data.get("bin")
    if isinstance(bin_field, str):
        candidates.append(bin_field)
    elif isinstance(bin_field, dict):
        candidates.extend(v for v in bin_field.values() if isinstance(v, str))
    main_field = data.get("main")
    if isinstance(main_field, str):
        candidates.append(main_field)

    for rel in candidates:
        entry = (pkg_dir / rel).resolve()
        if entry.is_file():
            return entry
    return None


def _locate_server_entry() -> str | None:
    """Locate the ``@eps-server/server`` launchable JS entry, or None.

    Checks each candidate package dir: a present ``package.json`` ``bin``/``main``
    is preferred; otherwise conventional entry filenames are probed. The first
    existing entry wins. None means the package is not installed (a normal,
    fully-supported state: diagnostics simply degrade to ``[]``).
    """
    for pkg_dir in _candidate_package_dirs():
        try:
            if not pkg_dir.is_dir():
                continue
        except OSError:
            continue
        entry = _entry_from_package_json(pkg_dir)
        if entry is not None:
            return str(entry)
        for rel in _ENTRY_CANDIDATES:
            entry = pkg_dir / rel
            try:
                if entry.is_file():
                    return str(entry.resolve())
            except OSError:
                continue
    return None


def _resolve_lsp() -> tuple[str, str] | None:
    """Resolve ``(node_path, server_entry)`` or None.

    None whenever ``node`` is unresolved OR ``@eps-server/server`` is not
    installed. The caller (``diagnose``) treats None as "skip, return []".
    """
    node_path = shutil.which("node")
    if not node_path:
        return None
    entry = _locate_server_entry()
    if not entry:
        return None
    return node_path, entry


# --------------------------------------------------------------------------- #
# LSP JSON-RPC framing
# --------------------------------------------------------------------------- #


def _frame(obj: dict) -> bytes:
    """Encode a JSON-RPC message with Content-Length framing (LSP base protocol)."""
    body = json.dumps(obj).encode("utf-8")
    return b"Content-Length: %d\r\n\r\n%s" % (len(body), body)


class _FrameReader:
    """Incremental Content-Length frame parser over a binary stdout stream.

    Handles SPLIT frames (a frame torn across reads) and MERGED frames (several
    frames in one read) by maintaining an internal byte buffer and pulling more
    bytes via ``read(n)`` only when the buffer cannot yet satisfy a parse step.
    Returns one decoded JSON object per ``next_message`` call, or None on EOF /
    unrecoverable framing error (the caller bails to ``[]`` on None).
    """

    def __init__(self, stream) -> None:
        self._stream = stream
        self._buf = bytearray()

    def _fill(self, want: int) -> bool:
        """Read until the buffer holds >= ``want`` bytes. False on EOF."""
        while len(self._buf) < want:
            chunk = self._stream.read(want - len(self._buf))
            if not chunk:
                return False
            self._buf.extend(chunk)
        return True

    def _read_headers(self) -> dict | None:
        """Accumulate bytes until the CRLFCRLF header terminator. None on EOF."""
        sep = b"\r\n\r\n"
        while True:
            idx = self._buf.find(sep)
            if idx != -1:
                raw = bytes(self._buf[:idx])
                del self._buf[: idx + len(sep)]
                headers: dict[str, str] = {}
                for line in raw.split(b"\r\n"):
                    if not line:
                        continue
                    key, _, value = line.partition(b":")
                    headers[key.strip().decode("ascii", "replace").lower()] = (
                        value.strip().decode("ascii", "replace")
                    )
                return headers
            chunk = self._stream.read(4096)
            if not chunk:
                return None
            self._buf.extend(chunk)

    def next_message(self) -> dict | None:
        """Parse and return the next JSON-RPC message, or None on EOF / bad frame."""
        headers = self._read_headers()
        if headers is None:
            return None
        length_str = headers.get("content-length")
        if length_str is None:
            # No Content-Length: we cannot frame the body. Bail (advisory).
            return None
        try:
            length = int(length_str)
        except ValueError:
            return None
        if length < 0:
            return None
        if not self._fill(length):
            return None
        body = bytes(self._buf[:length])
        del self._buf[:length]
        try:
            return json.loads(body.decode("utf-8"))
        except (ValueError, UnicodeDecodeError):
            # Malformed / non-JSON body -> bail to [] (no raise).
            return None


# --------------------------------------------------------------------------- #
# Diagnostic mapping
# --------------------------------------------------------------------------- #


def _map_diagnostics(lsp_diags: list) -> list[dict]:
    """Map LSP diagnostics to ``[{line, severity, message}]``.

    LSP ``range.start.line`` is 0-based -> 1-based ``line``. ``severity`` is the
    LSP integer passthrough, defaulting to 3 (Information) when omitted. Entries
    that are not dicts are skipped (defensive).
    """
    out: list[dict] = []
    for diag in lsp_diags or []:
        if not isinstance(diag, dict):
            continue
        rng = diag.get("range") or {}
        start = rng.get("start") or {}
        line0 = start.get("line", 0)
        try:
            line = int(line0) + 1
        except (TypeError, ValueError):
            line = 1
        severity = diag.get("severity", _DEFAULT_SEVERITY)
        if not isinstance(severity, int):
            severity = _DEFAULT_SEVERITY
        message = diag.get("message", "")
        out.append(
            {"line": line, "severity": severity, "message": str(message)}
        )
    return out


# --------------------------------------------------------------------------- #
# Public surface
# --------------------------------------------------------------------------- #


def diagnose(code: str, *, timeout: float = 2.0) -> list[dict]:
    """Advisory diagnostics for ``code`` via epscript-lsp; NEVER raises.

    Returns ``[{line, severity, message}]`` (1-based lines, LSP severity ints) or
    ``[]`` on ANY failure: unresolved node / package, spawn error, timeout,
    protocol error, or malformed frames (features/02, rules.md "advisory only").
    The spawned process is always reaped (kill + wait) on the timeout path.
    """
    try:
        resolved = _resolve_lsp()
        if resolved is None:
            return []
        node_path, entry = resolved
        return _run_lsp(node_path, entry, code, timeout)
    except Exception:  # noqa: BLE001 - advisory: swallow EVERYTHING, never block.
        return []


def _run_lsp(node_path: str, entry: str, code: str, timeout: float) -> list[dict]:
    """Spawn the LSP and drive the handshake; return mapped diagnostics or [].

    Defensive throughout: a spawn failure, a closed stream, a missing
    publishDiagnostics within the budget, or any framing/JSON error all return
    ``[]``. The process is killed and reaped before returning on every path.
    """
    deadline = time.monotonic() + max(0.0, timeout)
    proc = None
    try:
        proc = subprocess.Popen(  # noqa: S603 - resolved node + resolved entry
            [node_path, entry, "--stdio"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
    except OSError:
        return []

    try:
        stdin = proc.stdin
        if stdin is None or proc.stdout is None:
            return []

        # --- handshake: initialize -> initialized -> didOpen ---
        def send(msg: dict) -> None:
            stdin.write(_frame(msg))
            stdin.flush()

        send(
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "processId": os.getpid(),
                    "rootUri": None,
                    "capabilities": {},
                },
            }
        )
        send({"jsonrpc": "2.0", "method": "initialized", "params": {}})
        send(
            {
                "jsonrpc": "2.0",
                "method": "textDocument/didOpen",
                "params": {
                    "textDocument": {
                        "uri": _DOC_URI,
                        "languageId": _LANGUAGE_ID,
                        "version": 1,
                        "text": code,
                    }
                },
            }
        )

        # --- read frames until our publishDiagnostics or the deadline ---
        # The synchronous pipe ``read`` blocks; a server that never publishes
        # would hang past the budget. We run the read loop in a daemon thread
        # and wait on it with the remaining time; on timeout we kill the process
        # (in ``_shutdown``), which closes the pipe and unblocks the reader.
        result: dict[str, list[dict]] = {"diags": []}

        def reader_loop() -> None:
            try:
                reader = _FrameReader(proc.stdout)
                while True:
                    msg = reader.next_message()
                    if msg is None:
                        # EOF or malformed frame -> bail to [] (advisory).
                        return
                    if not isinstance(msg, dict):
                        continue
                    if msg.get("method") == "textDocument/publishDiagnostics":
                        params = msg.get("params") or {}
                        if params.get("uri") == _DOC_URI:
                            result["diags"] = _map_diagnostics(
                                params.get("diagnostics")
                            )
                            return
            except Exception:  # noqa: BLE001 - advisory: never propagate.
                return

        worker = threading.Thread(target=reader_loop, daemon=True)
        worker.start()
        worker.join(timeout=max(0.0, deadline - time.monotonic()))
        # Whether the worker finished (publishDiagnostics / EOF) or timed out,
        # ``_shutdown`` (finally) kills + reaps the process; if it timed out the
        # kill unblocks the still-parked read so the daemon thread can exit.
        return result["diags"]
    except Exception:  # noqa: BLE001 - advisory: never propagate.
        return []
    finally:
        _shutdown(proc)


def _shutdown(proc) -> None:
    """Best-effort graceful shutdown, then ALWAYS kill + reap (no zombies)."""
    if proc is None:
        return
    try:
        if proc.stdin is not None and not getattr(proc.stdin, "closed", False):
            try:
                proc.stdin.write(
                    _frame({"jsonrpc": "2.0", "id": 2, "method": "shutdown"})
                )
                proc.stdin.write(
                    _frame({"jsonrpc": "2.0", "method": "exit"})
                )
                proc.stdin.flush()
            except (OSError, ValueError):
                pass
            try:
                proc.stdin.close()
            except OSError:
                pass
    except Exception:  # noqa: BLE001
        pass
    # Kill + reap unconditionally: an advisory probe must never leak a process.
    try:
        if proc.poll() is None:
            proc.kill()
    except Exception:  # noqa: BLE001
        pass
    try:
        proc.wait(timeout=1.0)
    except Exception:  # noqa: BLE001
        pass
