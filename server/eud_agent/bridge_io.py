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

# DAT surface whitelists (features/04 "DAT surface (B1)"; capability-survey rows
# 1-8). Validation rejects unknown args BEFORE a .cmd is written, so a typo never
# round-trips to the editor. The dat-name set bypasses the editor's GetDatFileE
# 8-name whitelist by including portdata/sfxdata.
_DAT_NAMES = (
    "units",
    "weapons",
    "flingy",
    "sprites",
    "images",
    "upgrades",
    "techdata",
    "orders",
    "portdata",
    "sfxdata",
)
# ExtraDat kinds for GET/SETXDAT (the ExtraDatBinding key enums).
_XDAT_KINDS = ("statusinfor", "wireframe", "ButtonSet")
# require.dat subset for GET/SETREQ (techdata vs Stechdata duality).
_REQ_DATS = ("units", "upgrades", "techdata", "Stechdata", "orders")
# RESETDAT routing kinds.
_RESET_KINDS = ("dat", "xdat", "tbl")

# File-tree creatable/settable types (features/04 "File tree (B2)" + "Scope
# decisions": SCA fully defunct; only the CUI text families + RawText). NEWFILE's
# type arg is validated against this whitelist BEFORE send.
_CREATABLE_TYPES = ("CUIEps", "CUIPy", "RawText")


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


# ----------------------------------------------------------- DAT arg validation
# Each helper raises :class:`BridgeError` for an out-of-contract argument BEFORE
# any command is sent (features/04: "Numeric values validated server-side before
# send"). Raising the module's own error type keeps the wrapper surface uniform.


def _require_in(value: str, allowed: tuple[str, ...], label: str) -> str:
    if value not in allowed:
        raise BridgeError(
            f"ERROR: invalid {label} {value!r} (one of {', '.join(allowed)})"
        )
    return value


def _require_nonneg_int(value: object, label: str) -> int:
    try:
        n = int(value)  # type: ignore[arg-type]
    except (TypeError, ValueError) as exc:
        raise BridgeError(f"ERROR: {label} must be an integer, got {value!r}") from exc
    if n < 0:
        raise BridgeError(f"ERROR: {label} must be non-negative, got {n}")
    return n


def _require_numeric_value(value: object, label: str) -> str:
    """A dat value must be numeric (the editor's setters are integer-backed)."""
    try:
        int(str(value), 0) if isinstance(value, str) else int(value)  # type: ignore[arg-type]
    except (TypeError, ValueError) as exc:
        raise BridgeError(
            f"ERROR: {label} must be numeric, got {value!r}"
        ) from exc
    return str(value)


# RequireUse use-mode keyword -> numeric value (CRequireData.vb:15-21). The
# editor's PasteCopyData coerces the first dot-segment String->Enum (number), so
# only a numeric first segment is safe; keywords must be pre-mapped to digits.
_REQ_USE_KEYWORDS = {
    "Default": "0",
    "Dont": "1",
    "Always": "2",
    "AlwaysCurrent": "3",
}


def _normalize_req_payload(payload: str) -> str:
    """Map a use-mode keyword to its numeric value; validate the first segment.

    Accepts a keyword (Default/Dont/Always/AlwaysCurrent), a bare use-mode digit
    (0-4), or a custom copy-string (``4.<op,val>...``). The first dot-segment of
    the result MUST be one of 0-4; anything else raises :class:`BridgeError`
    BEFORE send so a non-numeric first segment never reaches the bridge (where it
    would throw an uncatchable InvalidCastException in the editor).
    """
    if payload in _REQ_USE_KEYWORDS:
        return _REQ_USE_KEYWORDS[payload]
    first = payload.split(".", 1)[0]
    if first not in ("0", "1", "2", "3", "4"):
        raise BridgeError(
            "ERROR: setreq payload must be a use-mode keyword "
            "(Default/Dont/Always/AlwaysCurrent) or a copy-string whose first "
            f"segment is 0-4, got {payload!r}"
        )
    return payload


def _require_pathlike(value: object, label: str) -> str:
    """Validate a path/name that rides the pipe-separated ARG line.

    Rejects an empty value and any ``|``/newline/carriage-return character: those
    would corrupt the bridge's first-line ``<CMD> <arg>`` / pipe-split parse (the
    EUD-049 review flagged this gap for the arg-line carriers — closed here for the
    new file-tree wrappers). Multi-line / non-ASCII content travels in the BODY
    instead, never the arg line.
    """
    s = str(value).strip()
    if not s:
        raise BridgeError(f"ERROR: {label} must be non-empty")
    bad = [c for c in ("|", "\n", "\r") if c in str(value)]
    if bad:
        raise BridgeError(
            f"ERROR: {label} must not contain {bad!r} (arg-line delimiters); "
            "got " + repr(value)
        )
    return s


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

    # ----------------------------------------------------------- DAT surface
    # Wrappers per features/04 "DAT surface (B1)". Args are validated (raising
    # BridgeError) BEFORE send so a bad name/index never reaches the editor. The
    # pipe-separated arg line carries identifiers; multi-line / UTF-8 values
    # (SETTBL/SETREQ/SETBTN) travel in the body (2nd line onward).

    def getdat(self, dat: str, param: str, obj_id: int, **kw) -> str:
        """Read a standard dat field (``GETDAT dat|param|objId``)."""
        _require_in(dat, _DAT_NAMES, "dat name")
        obj_id = _require_nonneg_int(obj_id, "objId")
        reply = self.send(f"GETDAT {dat}|{param}|{obj_id}", **kw)
        self._raise_if_error(reply)
        return reply

    def setdat(self, dat: str, param: str, obj_id: int, value, **kw) -> str:
        """Write a standard dat field (``SETDAT dat|param|objId|value``).

        ``value`` is validated numeric (the editor's dat setters are integer-
        backed and clamp out-of-range; a non-numeric value is rejected here).
        """
        _require_in(dat, _DAT_NAMES, "dat name")
        obj_id = _require_nonneg_int(obj_id, "objId")
        value = _require_numeric_value(value, "value")
        reply = self.send(f"SETDAT {dat}|{param}|{obj_id}|{value}", **kw)
        self._raise_if_error(reply)
        return reply

    def getxdat(self, dat: str, name: str, obj_id: int, **kw) -> str:
        """Read an ExtraDat field (``GETXDAT dat|name|objId``).

        ``dat`` ∈ {statusinfor, wireframe, ButtonSet}; ``name`` per survey
        (Status/Display/Joint, wire/grp/tran, ButtonSet).
        """
        _require_in(dat, _XDAT_KINDS, "xdat kind")
        obj_id = _require_nonneg_int(obj_id, "objId")
        reply = self.send(f"GETXDAT {dat}|{name}|{obj_id}", **kw)
        self._raise_if_error(reply)
        return reply

    def setxdat(self, dat: str, name: str, obj_id: int, value, **kw) -> str:
        """Write an ExtraDat field (``SETXDAT dat|name|objId|value``).

        The bridge re-reads ``.Value`` after assignment (Byte setters swallow bad
        values) and returns the read-back so the caller can verify the write.
        """
        _require_in(dat, _XDAT_KINDS, "xdat kind")
        obj_id = _require_nonneg_int(obj_id, "objId")
        value = _require_numeric_value(value, "value")
        reply = self.send(f"SETXDAT {dat}|{name}|{obj_id}|{value}", **kw)
        self._raise_if_error(reply)
        return reply

    def gettbl(self, index: int, **kw) -> str:
        """Read a stat_txt/tbl string (``GETTBL index``)."""
        index = _require_nonneg_int(index, "index")
        reply = self.send(f"GETTBL {index}", **kw)
        self._raise_if_error(reply)
        return reply

    def settbl(self, index: int, value: str, **kw) -> str:
        """Write a stat_txt/tbl string. Value travels in the BODY (UTF-8-safe).

        ``value == "NULLSTRING"`` resets the entry to its default.
        """
        index = _require_nonneg_int(index, "index")
        reply = self.send(f"SETTBL {index}\n{value}", **kw)
        self._raise_if_error(reply)
        return reply

    def resetdat(
        self, kind: str, dat: str, param_or_name: str, obj_id: int, **kw
    ) -> str:
        """Reset a field to its stock value (``RESETDAT kind|dat|param-or-name|objId``).

        ``kind`` ∈ {dat, xdat, tbl}; for ``tbl`` the dat/param args are ignored by
        the bridge (only the index matters) but kept for a uniform arg shape.
        """
        _require_in(kind, _RESET_KINDS, "reset kind")
        obj_id = _require_nonneg_int(obj_id, "objId")
        if kind == "dat":
            _require_in(dat, _DAT_NAMES, "dat name")
        elif kind == "xdat":
            _require_in(dat, _XDAT_KINDS, "xdat kind")
        reply = self.send(f"RESETDAT {kind}|{dat}|{param_or_name}|{obj_id}", **kw)
        self._raise_if_error(reply)
        return reply

    def getreq(self, dat: str, obj_id: int, **kw) -> str:
        """Read a requirement as the editor copy-string (``GETREQ dat|objId``).

        ``dat`` ∈ {units, upgrades, techdata, Stechdata, orders}.
        """
        _require_in(dat, _REQ_DATS, "req dat")
        obj_id = _require_nonneg_int(obj_id, "objId")
        reply = self.send(f"GETREQ {dat}|{obj_id}", **kw)
        self._raise_if_error(reply)
        return reply

    def setreq(self, dat: str, obj_id: int, payload: str, **kw) -> str:
        """Write a requirement (``SETREQ dat|objId`` + payload in BODY).

        ``payload`` is either a use-mode keyword (Default/Dont/Always/
        AlwaysCurrent) or the editor's own custom copy-string (starts ``4.``).
        Keywords are mapped to their NUMERIC ``RequireUse`` value BEFORE send
        (Default→0, Dont→1, Always→2, AlwaysCurrent→3); the bare digit is also
        accepted. The editor's ``PasteCopyData`` does a String=Enum compare that
        coerces the first dot-segment to a number — a non-numeric first segment
        throws ``InvalidCastException`` (uncatchable by lua pcall → editor error
        dialog), so the first segment MUST be one of 0-4. We validate that here
        and reject anything else BEFORE send (rules.md: isolate risk in Python).
        """
        _require_in(dat, _REQ_DATS, "req dat")
        obj_id = _require_nonneg_int(obj_id, "objId")
        payload = _normalize_req_payload(payload)
        reply = self.send(f"SETREQ {dat}|{obj_id}\n{payload}", **kw)
        self._raise_if_error(reply)
        return reply

    def getbtn(self, set_id: int, **kw) -> str:
        """Read a button set as the editor CSV (``GETBTN setId``)."""
        set_id = _require_nonneg_int(set_id, "setId")
        reply = self.send(f"GETBTN {set_id}", **kw)
        self._raise_if_error(reply)
        return reply

    def setbtn(self, set_id: int, csv: str, **kw) -> str:
        """Write a button set (``SETBTN setId`` + CSV in BODY).

        The bridge 8-field-validates each dot-separated button before Paste and
        dirties the project; a malformed CSV returns ``ERROR:``.
        """
        set_id = _require_nonneg_int(set_id, "setId")
        reply = self.send(f"SETBTN {set_id}\n{csv}", **kw)
        self._raise_if_error(reply)
        return reply

    # ----------------------------------------------------------- file tree
    # Wrappers per features/04 "File tree (B2)". Path-like identifiers ride the
    # arg line and are validated (empty / ``|`` / newline rejected) BEFORE send;
    # newname (RENAME) and destFolder (MOVEFILE) and file content (NEWFILE) travel
    # in the BODY (B2: "Multi-line or non-ASCII values travel in the body").

    def newfile(self, path: str, ftype: str, code: str, **kw) -> str:
        """Create a file of ``ftype`` at ``path`` (``NEWFILE path|type`` + body).

        ``ftype`` is validated against the creatable whitelist {CUIEps, CUIPy,
        RawText}; ``path`` may include folders (auto-created by the bridge). A
        duplicate full path returns ``ERROR:`` (Decision 02 generalized).
        """
        path = _require_pathlike(path, "path")
        _require_in(ftype, _CREATABLE_TYPES, "file type")
        reply = self.send(f"NEWFILE {path}|{ftype}\n{code}", **kw)
        self._raise_if_error(reply)
        return reply

    def mkdir(self, path: str, **kw) -> str:
        """Create a folder (``MKDIR path``); nested ok, duplicate -> ``ERROR:``."""
        path = _require_pathlike(path, "path")
        reply = self.send(f"MKDIR {path}", **kw)
        self._raise_if_error(reply)
        return reply

    def rename(self, path: str, newname: str, **kw) -> str:
        """Rename a node (``RENAME path`` + newname in BODY).

        The new name travels in the body (B2). The bridge rejects the top node,
        the Setting node, and a duplicate sibling name.
        """
        path = _require_pathlike(path, "path")
        if not str(newname).strip():
            raise BridgeError("ERROR: newname must be non-empty")
        reply = self.send(f"RENAME {path}\n{newname}", **kw)
        self._raise_if_error(reply)
        return reply

    def delfile(self, path: str, **kw) -> str:
        """Delete a node (``DELFILE path``).

        The bridge rejects top/Setting nodes, clears a dangling ``MainFile`` (the
        result then notes ``main-cleared``), closes any open tab, and dirties the
        project.
        """
        path = _require_pathlike(path, "path")
        reply = self.send(f"DELFILE {path}", **kw)
        self._raise_if_error(reply)
        return reply

    def movefile(self, path: str, dest_folder: str, **kw) -> str:
        """Move a node into ``dest_folder`` (``MOVEFILE path`` + destFolder in BODY).

        The destFolder travels in the body (B2). An empty destFolder moves the
        node to the project root. The bridge re-adds the SAME instance (preserving
        MainFile identity) and rejects moving the top/Setting node or into Setting.
        """
        path = _require_pathlike(path, "path")
        reply = self.send(f"MOVEFILE {path}\n{dest_folder}", **kw)
        self._raise_if_error(reply)
        return reply

    def setmain(self, path: str, **kw) -> str:
        """Point ``MainFile`` at the node at ``path`` (``SETMAIN path``)."""
        path = _require_pathlike(path, "path")
        reply = self.send(f"SETMAIN {path}", **kw)
        self._raise_if_error(reply)
        return reply

    def getmain(self, **kw) -> str:
        """Return the current main file path, or ``""`` when none is set."""
        reply = self.send("GETMAIN", **kw)
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
