"""Server side of the file-IPC bridge to EUD Editor 3.

The Python server is the only writer on its half of the file IPC (architecture.md
"File IPC protocol"). It writes a command file into ``<data_dir>\\inbox`` and polls
``<data_dir>\\outbox`` for the matching reply the Lua bridge produces on its 1s
UI-thread ``Tick``. KopiLua has no sockets/io.popen, so files are the only
transport (rules.md "Lua bridge ... crash rules").

Protocol invariants enforced here (rules.md "IPC and encoding"):

  * Command files are written UTF-8 **without** a BOM (``encoding="utf-8"``;
    ``utf-8-sig`` is forbidden — a BOM would corrupt the bridge's first-line
    command parsing). The write is atomic (temp file + ``os.replace``) so the
    bridge never reads a half-written ``.cmd``.
  * Server command files are named ``srv-<uuid8>.cmd``; the reply lands at
    ``srv-<uuid8>.result``. The legacy headless runner owns the ``agent_*``
    namespace — this module never touches those files.
  * **The reader (server) deletes the ``.result`` after consuming it.** The
    bridge deletes the ``.cmd`` after processing.
  * Polling always has a timeout (default 10s). While ``status.txt`` reports
    ``compiling=true`` the deadline extends to ``busy_timeout`` (default 180s)
    and an ``on_busy`` callback fires once (the orchestrator forwards
    ``waiting_build`` to the panel). On timeout the ``.cmd`` is **left in place**
    (it will apply once the build finishes) and ``BridgeBusy`` is raised.

This is a synchronous implementation (stdlib only: ``pathlib``/``uuid``/``time``/
``typing``); the orchestrator runs ``send`` in a thread executor. Helper command
texts follow the architecture.md IPC table — ``SET``/``NEWEPS`` put ``<CMD> <arg>``
on the first line and the body on the second line, joined with ``"\n"``.

``ERROR:``-prefixed replies raise :class:`BridgeError`; a poll timeout raises
:class:`BridgeBusy`.
"""

from __future__ import annotations

import os
import time
import uuid
from collections.abc import Callable
from pathlib import Path

# Real defaults (rules.md "IPC and encoding": default 10s, extend to 180s while
# compiling; architecture.md busy-editor handling uses the same 10s/180s).
DEFAULT_TIMEOUT = 10.0
DEFAULT_BUSY_TIMEOUT = 180.0
DEFAULT_POLL_INTERVAL = 0.2

# settable file-type families: SET exists only for CUI/RawText file types;
# GUI files have no setter (architecture.md SET row; rules.md "SET/NEWEPS").
# SCA is fully defunct (capability-survey, 2026-06-05) and is NOT settable.
# The bridge's LIST emits the EFileType enum NAME (e.g. CUIEps, CUI, CUIPy,
# CUITrg, RawText, GUI); membership is by case-insensitive substring so the whole
# CUI family (CUIEps/CUIPy/CUITrg/...) is covered without enumerating every member.
_SETTABLE_FAMILIES = ("CUI", "RAWTEXT")


class BridgeError(Exception):
    """The bridge returned an ``ERROR:``-prefixed result for a command."""


class BridgeBusy(Exception):
    """The bridge did not answer before the (possibly extended) timeout.

    The ``.cmd`` is left in place so it still applies once a build finishes.
    """


def _settable_for(ftype: str) -> bool:
    """Whether a file of this EFileType name accepts SET (memory-only edit)."""
    upper = ftype.upper()
    return any(fam in upper for fam in _SETTABLE_FAMILIES)


class BridgeIO:
    """File-IPC client bound to the editor's ``Data\\agent`` directory.

    A single instance is shared by the orchestrator. The only required argument
    is ``data_dir`` (the editor's ``Data\\agent`` folder); the drop-in bridge
    cannot know it any other way and the server receives it from config.
    """

    def __init__(self, data_dir: str | os.PathLike) -> None:
        self.data_dir = Path(data_dir)
        self.inbox = self.data_dir / "inbox"
        self.outbox = self.data_dir / "outbox"
        self.status_file = self.data_dir / "status.txt"

    # ------------------------------------------------------------------ send
    def send(
        self,
        command_text: str,
        *,
        timeout: float = DEFAULT_TIMEOUT,
        busy_timeout: float = DEFAULT_BUSY_TIMEOUT,
        poll_interval: float = DEFAULT_POLL_INTERVAL,
        on_busy: Callable[[], None] | None = None,
    ) -> str:
        """Write a ``.cmd``, poll for its ``.result``, return the reply text.

        ``command_text`` is the full command (first line ``<CMD> <arg>`` plus an
        optional body from the second line). Timeouts are parameters so the
        orchestrator (and tests) can tune them; the real defaults match
        rules.md. While ``status.txt`` reports ``compiling=true`` the effective
        deadline extends from ``timeout`` to ``busy_timeout`` and ``on_busy``
        fires exactly once.

        Reading the ``.result``: consume it, then DELETE it (the server is the
        reader). On timeout the ``.cmd`` is LEFT in place and :class:`BridgeBusy`
        is raised.
        """
        name = "srv-" + uuid.uuid4().hex[:8]
        cmd_path = self.inbox / (name + ".cmd")
        result_path = self.outbox / (name + ".result")

        self._write_cmd(cmd_path, command_text)

        start = time.monotonic()
        busy_notified = False
        empty_seen = False  # tracks a prior zero-length read (mid-write guard)
        while True:
            reply, empty_seen = self._consume_result(result_path, empty_seen)
            if reply is not None:
                return reply

            now = time.monotonic()
            compiling = self._is_compiling()
            if compiling and not busy_notified:
                busy_notified = True
                if on_busy is not None:
                    on_busy()
            # Per-poll window selection: extend to busy_timeout once a build is
            # (or has been) seen compiling, so a build that STARTS mid-wait still
            # extends (architecture.md busy-editor handling).
            window = busy_timeout if (compiling or busy_notified) else timeout
            deadline = start + window

            if now >= deadline:
                # Leave the .cmd in place (it applies after the build); signal busy.
                raise BridgeBusy(
                    f"bridge did not answer {name} within "
                    f"{deadline - start:.1f}s (compiling={compiling})"
                )
            time.sleep(poll_interval)

    # --------------------------------------------------------------- helpers
    def ping(self, **kw) -> str:
        """Liveness check; the bridge replies ``PONG <time>``."""
        return self.send("PING", **kw)

    def status(self, **kw) -> str:
        """Editor state: ``compiling`` / ``project`` / ``version`` lines."""
        return self.send("STATUS", **kw)

    def list_files(self, **kw) -> list[dict]:
        """Project file tree as ``[{path, ftype, settable}]``.

        Parses the bridge's ``path\\t<EFileType>`` lines. An EMPTY (non-``ERROR:``)
        reply means ZERO files — an open project with no files, not a failure
        (verified bridge behavior, EUD-011). ``settable`` is derived from the
        file-type family (CUI/RawText settable; GUI read-only).
        """
        reply = self.send("LIST", **kw)
        self._raise_if_error(reply)
        files: list[dict] = []
        for line in reply.splitlines():
            line = line.rstrip("\r")
            if not line:
                continue
            path, tab, ftype = line.partition("\t")
            ftype = ftype if tab else ""
            files.append(
                {
                    "path": path,
                    "ftype": ftype,
                    "settable": _settable_for(ftype),
                }
            )
        return files

    def get(self, path: str, **kw) -> str:
        """Read a file's text by project path (``GET <path>``)."""
        reply = self.send(f"GET {path}", **kw)
        self._raise_if_error(reply)
        return reply

    def set(self, path: str, code: str, **kw) -> str:
        """Replace a file's text (memory-only). ``SET <path>`` + body on line 2.

        Only CUI/RawText files accept SET; the bridge returns an ``ERROR:``
        for GUI files, surfaced here as :class:`BridgeError`.
        """
        reply = self.send(f"SET {path}\n{code}", **kw)
        self._raise_if_error(reply)
        return reply

    def neweps(self, name: str, code: str, **kw) -> str:
        """Create a new root-folder eps file. ``NEWEPS <name>`` + body on line 2.

        A duplicate name returns ``ERROR: duplicate '<name>'`` (Decision 02 — no
        auto-suffix), surfaced as :class:`BridgeError`.
        """
        reply = self.send(f"NEWEPS {name}\n{code}", **kw)
        self._raise_if_error(reply)
        return reply

    # --------------------------------------------------------------- cleanup
    def cleanup_stale(self) -> None:
        """Remove leftover ``srv-*`` IPC files at startup.

        Only the server namespace (``srv-*.cmd`` in inbox, ``srv-*.result`` in
        outbox) is cleared — the legacy runner's ``agent_*`` files are NEVER
        touched (rules.md). Missing inbox/outbox dirs are tolerated.
        """
        for path in self.inbox.glob("srv-*.cmd"):
            path.unlink(missing_ok=True)
        for path in self.outbox.glob("srv-*.result"):
            path.unlink(missing_ok=True)

    # --------------------------------------------------------------- internals
    def _write_cmd(self, cmd_path: Path, command_text: str) -> None:
        """Write the ``.cmd`` UTF-8 without BOM, atomically (temp + replace).

        The atomic rename ensures the bridge's Tick never reads a partial file.
        Bytes are written directly (UTF-8, never ``utf-8-sig``) so that no BOM is
        emitted AND newline translation cannot turn ``\\n`` into ``\\r\\n`` on
        Windows — the command text is delivered exactly as given (the bridge's
        first-line parser keys off ``\\n``).
        """
        self.inbox.mkdir(parents=True, exist_ok=True)
        tmp = cmd_path.with_suffix(cmd_path.suffix + ".tmp")
        tmp.write_bytes(command_text.encode("utf-8"))
        os.replace(tmp, cmd_path)

    def _consume_result(
        self, result_path: Path, empty_seen: bool
    ) -> tuple[str | None, bool]:
        """Try to consume the ``.result``; return ``(reply_or_None, empty_seen)``.

        The REAL bridge writes the ``.result`` non-atomically (v6
        ``File.WriteAllText`` straight to the final path — we must not touch that
        code). A poll landing between file creation and the content flush would
        read ``""``; because an EMPTY LIST result is a VALID reply (zero files),
        consuming that immediately could silently report a populated project as
        empty. To defeat the create -> flush race without breaking the legitimate
        empty case, a zero-length read is accepted only after it stays zero on a
        SECOND consecutive poll (``poll_interval`` apart):

          * non-empty read -> consume + delete, return ``(text, False)``;
          * first zero-length read -> keep the file, return ``(None, True)``;
          * zero-length read with ``empty_seen`` already set -> the result is
            genuinely empty (the write would have flushed within microseconds,
            far under the ~200ms poll) -> consume + delete, return ``("", False)``;
          * file absent / transient read error -> ``(None, False)`` and reset.
        """
        if not result_path.is_file():
            return None, False
        try:
            text = result_path.read_text(encoding="utf-8")
        except OSError:
            # Bridge may be mid-write; treat as not-yet-ready and retry.
            return None, False
        if text == "" and not empty_seen:
            # Possibly mid-write: leave the file, require one more zero read.
            return None, True
        result_path.unlink(missing_ok=True)
        return text, False

    def _is_compiling(self) -> bool:
        """True when ``status.txt`` reports ``compiling=true`` (read per-poll)."""
        try:
            text = self.status_file.read_text(encoding="utf-8")
        except OSError:
            return False
        for line in text.splitlines():
            key, sep, value = line.partition("=")
            if sep and key.strip().lower() == "compiling":
                return value.strip().lower() == "true"
        return False

    @staticmethod
    def _raise_if_error(reply: str) -> None:
        """Raise :class:`BridgeError` for an ``ERROR:``-prefixed reply."""
        if reply.startswith("ERROR:"):
            raise BridgeError(reply.strip())
