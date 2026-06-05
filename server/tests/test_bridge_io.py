"""Verification artifact for EUD-015-c726: server-side bridge_io module.

These tests drive ``eud_agent.bridge_io`` against a FAKE BRIDGE: a tmp_path
``Data\\agent`` directory plus a background thread that imitates the real Lua
bridge's inbox/outbox file-IPC behavior (architecture.md "File IPC protocol",
rules.md "IPC and encoding"). The fake:

  - polls ``inbox/srv-*.cmd``;
  - reads the command (UTF-8, no BOM expected);
  - writes the configured reply to the matching ``outbox/<stem>.result``;
  - DELETES the ``.cmd`` after processing (as the real bridge does).

The module under test is responsible for the server side: writing the ``.cmd``
BOM-free, polling for the ``.result``, deleting the ``.result`` after reading,
extending the timeout while the editor reports ``compiling=true`` (invoking an
``on_busy`` callback once), raising ``BridgeBusy`` on timeout (leaving the
``.cmd`` in place), and parsing helper command results.

Timeouts are INJECTABLE so the suite stays fast (no real 10s/180s waits).

``eud_agent.bridge_io`` does NOT exist during Step A, so this suite is expected
to FAIL on import until bridge_io.py is implemented (Step B).
"""

from __future__ import annotations

import os
import threading
import time
from pathlib import Path

import pytest

# Imported at collection so the failing import is the first signal in Step A.
# (No try/except: the suite MUST error out until the module exists.)
from eud_agent import bridge_io
from eud_agent.bridge_io import BridgeBusy, BridgeError, BridgeIO

# --------------------------------------------------------------------------- #
# Fake bridge: a background thread mimicking the Lua side of the file IPC.
# --------------------------------------------------------------------------- #


class FakeBridge:
    """Background watcher that answers ``inbox/srv-*.cmd`` files.

    ``responder(first_line, body) -> reply_text`` decides each reply. A reply
    of ``None`` means "do not answer" (used for timeout tests). The fake deletes
    the ``.cmd`` only AFTER it has written the ``.result`` and (optionally) waited
    ``answer_delay`` seconds, matching the real bridge ordering closely enough
    for the round-trip and busy-extension tests.
    """

    def __init__(self, data_dir: Path, responder, *, answer_delay: float = 0.0):
        self.inbox = data_dir / "inbox"
        self.outbox = data_dir / "outbox"
        self.inbox.mkdir(parents=True, exist_ok=True)
        self.outbox.mkdir(parents=True, exist_ok=True)
        self._responder = responder
        self._answer_delay = answer_delay
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._run, daemon=True)
        self.seen: list[str] = []  # command first-lines we processed

    def __enter__(self) -> FakeBridge:
        self._thread.start()
        return self

    def __exit__(self, *exc) -> None:
        self._stop.set()
        self._thread.join(timeout=2.0)

    def _run(self) -> None:
        while not self._stop.is_set():
            for cmd in sorted(self.inbox.glob("srv-*.cmd")):
                try:
                    raw = cmd.read_text(encoding="utf-8")
                except (OSError, ValueError):
                    continue
                nl = raw.find("\n")
                if nl >= 0:
                    first_line = raw[:nl].replace("\r", "")
                    body = raw[nl + 1 :]
                else:
                    first_line = raw.replace("\r", "")
                    body = ""
                reply = self._responder(first_line, body)
                if reply is None:
                    # Do not answer (timeout case) and do not delete the .cmd.
                    continue
                if self._answer_delay:
                    time.sleep(self._answer_delay)
                result_path = self.outbox / (cmd.stem + ".result")
                # Atomic write (temp + replace), mirroring the real bridge's
                # File.WriteAllText so the reader never observes a half-written
                # (momentarily empty) .result and mistakes it for an empty reply.
                tmp = result_path.with_suffix(".result.tmp")
                tmp.write_text(reply, encoding="utf-8")
                os.replace(tmp, result_path)
                self.seen.append(first_line)
                cmd.unlink(missing_ok=True)  # real bridge deletes the .cmd
            time.sleep(0.02)


def _make_io(data_dir: Path) -> BridgeIO:
    """Construct the BridgeIO under test bound to ``data_dir``.

    The constructor signature is part of the contract: a data dir (the editor's
    ``Data\\agent``) is the single required argument. Default timeouts may be
    overridden per-call via ``send`` kwargs (see below).
    """
    return BridgeIO(str(data_dir))


def _write_status(data_dir: Path, *, compiling: bool) -> None:
    """Write ``status.txt`` the way the bridge does (compiling flag line)."""
    data_dir.mkdir(parents=True, exist_ok=True)
    text = "compiling={}\r\nproject='X'\r\n".format(
        "true" if compiling else "false"
    )
    (data_dir / "status.txt").write_text(text, encoding="utf-8")


# --------------------------------------------------------------------------- #
# 1. Happy-path round-trip
# --------------------------------------------------------------------------- #


def test_send_round_trip_ping(tmp_path):
    """send() writes a .cmd, the fake answers, send() returns the reply.

    Also asserts: the .result is consumed+deleted, and the .cmd is gone (the
    fake bridge deletes it, as the real one does).
    """
    data_dir = tmp_path / "agent"

    def responder(first_line, body):
        assert first_line == "PING"
        return "PONG 2026-06-04"

    bio = _make_io(data_dir)
    with FakeBridge(data_dir, responder):
        result = bio.send("PING", timeout=3.0, poll_interval=0.02)

    assert result.strip() == "PONG 2026-06-04"
    # .result deleted by the reader (server) after consumption.
    assert list((data_dir / "outbox").glob("srv-*.result")) == []
    # .cmd deleted by the (fake) bridge after processing.
    assert list((data_dir / "inbox").glob("srv-*.cmd")) == []


# --------------------------------------------------------------------------- #
# 2. The written .cmd is UTF-8 WITHOUT a BOM
# --------------------------------------------------------------------------- #


def test_cmd_written_without_bom(tmp_path):
    """The .cmd file must start with the command bytes, never the UTF-8 BOM.

    A BOM (EF BB BF) would break the bridge's first-line command parsing
    (rules.md "IPC and encoding": utf-8-sig forbidden). We capture the .cmd
    bytes by NOT answering (responder returns None), then time out quickly.
    """
    data_dir = tmp_path / "agent"

    def responder(first_line, body):
        return None  # never answer -> we inspect the leftover .cmd

    bio = _make_io(data_dir)
    with FakeBridge(data_dir, responder):
        with pytest.raises(BridgeBusy):
            bio.send("PING", timeout=0.4, poll_interval=0.02)

    cmds = list((data_dir / "inbox").glob("srv-*.cmd"))
    assert len(cmds) == 1, "the .cmd must be LEFT in place on timeout"
    raw = cmds[0].read_bytes()
    assert raw[:3] != b"\xef\xbb\xbf", "BOM present: utf-8-sig is forbidden"
    assert raw.startswith(b"PING"), "command bytes must lead the file"


# --------------------------------------------------------------------------- #
# 3. Timeout with no responder -> BridgeBusy; the .cmd is LEFT in place
# --------------------------------------------------------------------------- #


def test_timeout_raises_bridge_busy_and_leaves_cmd(tmp_path):
    data_dir = tmp_path / "agent"
    (data_dir).mkdir(parents=True, exist_ok=True)

    bio = _make_io(data_dir)
    # No FakeBridge running at all: nothing answers, nothing deletes the .cmd.
    t0 = time.monotonic()
    with pytest.raises(BridgeBusy):
        bio.send("STATUS", timeout=0.3, poll_interval=0.02)
    elapsed = time.monotonic() - t0

    # Bounded by the short injected timeout (no real 10s wait).
    assert elapsed < 2.0
    leftover = list((data_dir / "inbox").glob("srv-*.cmd"))
    assert len(leftover) == 1, "timeout must LEAVE the .cmd in place"


# --------------------------------------------------------------------------- #
# 4. Busy extension: compiling=true extends the timeout and fires on_busy once
# --------------------------------------------------------------------------- #


def test_busy_extends_timeout_and_invokes_on_busy(tmp_path):
    """status.txt compiling=true -> timeout extends to busy_timeout and the
    on_busy callback is invoked (exactly once). A responder answering within
    the EXTENDED window succeeds; the base window alone would have failed.
    """
    data_dir = tmp_path / "agent"
    _write_status(data_dir, compiling=True)

    answered = threading.Event()

    def responder(first_line, body):
        # Answer only after the base timeout would have elapsed, but inside the
        # extended window. base=0.5s, delay=0.8s, extended=2.5s.
        if not answered.is_set():
            time.sleep(0.8)
            answered.set()
        return "compiling=true\r\nproject='X'\r\nversion=0.19.6.0"

    calls: list[str] = []

    bio = _make_io(data_dir)
    with FakeBridge(data_dir, responder):
        result = bio.send(
            "STATUS",
            timeout=0.5,          # base window (would expire before the answer)
            busy_timeout=2.5,     # extended window while compiling=true
            poll_interval=0.02,
            on_busy=lambda: calls.append("busy"),
        )

    assert "compiling=true" in result
    assert calls == ["busy"], "on_busy must fire exactly once during a busy wait"


def test_on_busy_not_called_when_not_compiling(tmp_path):
    """When status.txt does not report compiling=true, on_busy is never fired
    and the base timeout governs (a prompt answer still succeeds).
    """
    data_dir = tmp_path / "agent"
    _write_status(data_dir, compiling=False)

    def responder(first_line, body):
        return "PONG now"

    calls: list[str] = []
    bio = _make_io(data_dir)
    with FakeBridge(data_dir, responder):
        result = bio.send(
            "PING",
            timeout=2.0,
            busy_timeout=10.0,
            poll_interval=0.02,
            on_busy=lambda: calls.append("busy"),
        )
    assert result.strip() == "PONG now"
    assert calls == [], "on_busy must NOT fire when compiling is not true"


# --------------------------------------------------------------------------- #
# 5. list_files parsing
# --------------------------------------------------------------------------- #


def test_list_files_parses_tab_lines_with_settable(tmp_path):
    """Tab-separated path\\tEFileType lines parse into {path, ftype, settable}.

    A CUI-family type is settable; GUI and SCA types are not (architecture.md
    SET row: CUI/RawText only, GUI has no setter, SCA defunct; rules.md
    "SET/NEWEPS"). The SCA line is kept as a NEGATIVE case so a future change
    cannot silently re-admit the dead family.
    """
    data_dir = tmp_path / "agent"

    list_reply = "\r\n".join(
        [
            "main.eps\tCUIEps",
            "folder/util.eps\tCUI",
            "raw.txt\tRawText",
            "sca/mod.sca\tSCA",
            "ui/layout.gui\tGUI",
        ]
    )

    def responder(first_line, body):
        assert first_line == "LIST"
        return list_reply

    bio = _make_io(data_dir)
    with FakeBridge(data_dir, responder):
        files = bio.list_files(timeout=3.0, poll_interval=0.02)

    by_path = {f["path"]: f for f in files}
    assert by_path["main.eps"]["ftype"] == "CUIEps"
    assert by_path["main.eps"]["settable"] is True
    assert by_path["folder/util.eps"]["settable"] is True
    assert by_path["raw.txt"]["settable"] is True
    assert by_path["sca/mod.sca"]["ftype"] == "SCA"
    assert by_path["sca/mod.sca"]["settable"] is False  # SCA defunct
    assert by_path["ui/layout.gui"]["ftype"] == "GUI"
    assert by_path["ui/layout.gui"]["settable"] is False


def test_list_files_empty_means_zero_files(tmp_path):
    """An EMPTY (non-ERROR) LIST reply means zero files, NOT a failure
    (verified bridge behavior, EUD-011).
    """
    data_dir = tmp_path / "agent"

    def responder(first_line, body):
        return ""  # open project, but no files

    bio = _make_io(data_dir)
    with FakeBridge(data_dir, responder):
        files = bio.list_files(timeout=3.0, poll_interval=0.02)
    assert files == []


def test_list_files_error_raises(tmp_path):
    """An ``ERROR:``-prefixed LIST reply raises a clear exception type."""
    data_dir = tmp_path / "agent"

    def responder(first_line, body):
        return "ERROR: no project"

    bio = _make_io(data_dir)
    with FakeBridge(data_dir, responder):
        with pytest.raises(BridgeError):
            bio.list_files(timeout=3.0, poll_interval=0.02)


# --------------------------------------------------------------------------- #
# 5b. Mid-write .result visibility: empty-read stability check
#
# The REAL bridge writes .result non-atomically (File.WriteAllText to the final
# path). A poll landing between create and flush sees "" — which is ALSO a valid
# (zero-files) reply. The reader must accept "" only after a SECOND consecutive
# zero-length poll, so a genuinely-empty result still returns promptly while a
# create->flush race never silently truncates a populated reply.
# --------------------------------------------------------------------------- #


def test_send_genuinely_empty_result_returns_empty(tmp_path):
    """A truly empty result is returned as "" within the timeout (not BridgeBusy).

    The empty-read stability check must still TERMINATE for legitimately-empty
    replies (it costs one extra poll, well under the timeout).
    """
    data_dir = tmp_path / "agent"

    def responder(first_line, body):
        return ""  # genuinely empty reply (e.g. open project, zero files)

    bio = _make_io(data_dir)
    t0 = time.monotonic()
    with FakeBridge(data_dir, responder):
        result = bio.send("LIST", timeout=3.0, poll_interval=0.02)
    assert result == ""
    assert time.monotonic() - t0 < 2.0  # bounded; returns after the 2nd empty poll


def test_send_tolerates_mid_write_empty_then_content(tmp_path):
    """A non-atomically written result (empty file first, content later) must
    yield the FULL content, never the momentarily-empty "".

    This reproduces the real bridge's ``File.WriteAllText`` visibility (write to
    the final path, not an atomic rename): a poll can land on the just-created,
    not-yet-flushed empty file. The reader must NOT consume that "" — it must
    wait for a second read, which then sees the content.

    The ordering is made DETERMINISTIC (no timing flakiness) by instrumenting the
    reader: a ``BridgeIO`` subclass signals an event the first time
    ``_consume_result`` observes the empty file (the guard's ``empty_seen=True``
    branch). A watcher waits on that event, then writes the real content IN PLACE
    (no rename) before the reader's next poll. So the empty state is observed
    exactly once, the content is present for the second read, and the test
    exercises the guard path on purpose. Against the OLD reader (consume on any
    read) the watcher's event would fire on a path that returned "" to the
    caller, truncating the reply — which this assertion catches.
    """
    data_dir = tmp_path / "agent"
    inbox = data_dir / "inbox"
    outbox = data_dir / "outbox"
    inbox.mkdir(parents=True)
    outbox.mkdir(parents=True)

    # Plain "\n" newlines: the reader's read_text universal-newline mode collapses
    # "\r\n" to "\n", which is irrelevant here (this test targets the race, not
    # newline fidelity; list_files parses via splitlines() either way).
    full_reply = "main.eps\tCUIEps\nutil.eps\tCUI"

    saw_empty = threading.Event()
    content_written = threading.Event()

    class InstrumentedIO(BridgeIO):
        def _consume_result(self, result_path, empty_seen):
            reply, new_empty_seen = super()._consume_result(result_path, empty_seen)
            # The guard reports a first zero-length read via (None, True).
            if reply is None and new_empty_seen and not empty_seen:
                saw_empty.set()
                # Block this poll's continuation until the watcher has written
                # the content, so the reader's NEXT read observes it.
                content_written.wait(timeout=2.0)
            return reply, new_empty_seen

    stop = threading.Event()

    def watcher():
        while not stop.is_set():
            for cmd in sorted(inbox.glob("srv-*.cmd")):
                result_path = outbox / (cmd.stem + ".result")
                # Phase 1: create the file EMPTY (mid-write visibility window).
                result_path.write_text("", encoding="utf-8")
                # Wait until the reader has observed the empty file once.
                assert saw_empty.wait(timeout=2.0), "reader never saw the empty file"
                # Phase 2: write the real content in place (no atomic rename) as
                # bytes (no newline translation), then release the reader.
                result_path.write_bytes(full_reply.encode("utf-8"))
                content_written.set()
                cmd.unlink(missing_ok=True)
                return
            time.sleep(0.005)

    th = threading.Thread(target=watcher, daemon=True)
    th.start()
    try:
        bio = InstrumentedIO(str(data_dir))
        result = bio.send("LIST", timeout=5.0, poll_interval=0.05)
    finally:
        stop.set()
        th.join(timeout=2.0)

    # The full content must survive the mid-write empty window.
    assert saw_empty.is_set(), "the empty-read path was not exercised"
    assert result == full_reply
    # And it parses to the populated project, not an empty one.
    files = [
        {"path": ln.split("\t")[0]}
        for ln in result.replace("\r", "").splitlines()
        if ln
    ]
    assert [f["path"] for f in files] == ["main.eps", "util.eps"]


# --------------------------------------------------------------------------- #
# 6. set / neweps command formatting (exact bytes: first line + body)
# --------------------------------------------------------------------------- #


def test_set_command_formatting(tmp_path):
    """set(path, code) -> first line 'SET <path>', body from the 2nd line."""
    data_dir = tmp_path / "agent"
    captured: dict[str, str] = {}

    def responder(first_line, body):
        captured["first_line"] = first_line
        captured["body"] = body
        return "OK: set 'main.eps' (12B)"

    bio = _make_io(data_dir)
    with FakeBridge(data_dir, responder):
        result = bio.set("main.eps", "puts(1234);\n", timeout=3.0, poll_interval=0.02)

    assert captured["first_line"] == "SET main.eps"
    assert captured["body"] == "puts(1234);\n"
    assert result.startswith("OK")


def test_neweps_command_formatting(tmp_path):
    """neweps(name, code) -> first line 'NEWEPS <name>', body from the 2nd line."""
    data_dir = tmp_path / "agent"
    captured: dict[str, str] = {}

    def responder(first_line, body):
        captured["first_line"] = first_line
        captured["body"] = body
        return "OK: neweps 'feature.eps' (20B)"

    bio = _make_io(data_dir)
    with FakeBridge(data_dir, responder):
        result = bio.neweps(
            "feature.eps", "function f() {}\n", timeout=3.0, poll_interval=0.02
        )

    assert captured["first_line"] == "NEWEPS feature.eps"
    assert captured["body"] == "function f() {}\n"
    assert result.startswith("OK")


def test_neweps_duplicate_error_raises(tmp_path):
    """A duplicate NEWEPS (ERROR: duplicate ...) surfaces as BridgeError."""
    data_dir = tmp_path / "agent"

    def responder(first_line, body):
        return "ERROR: duplicate 'feature.eps'"

    bio = _make_io(data_dir)
    with FakeBridge(data_dir, responder):
        with pytest.raises(BridgeError):
            bio.neweps("feature.eps", "x", timeout=3.0, poll_interval=0.02)


def test_raw_cmd_bytes_have_body_on_second_line(tmp_path):
    """Independently of the fake, the written .cmd bytes are exactly
    '<CMD> <arg>\\n<body>' (first line + body from the 2nd line), BOM-free.
    """
    data_dir = tmp_path / "agent"

    def responder(first_line, body):
        return None  # leave the .cmd so we can read its raw bytes

    bio = _make_io(data_dir)
    with FakeBridge(data_dir, responder):
        with pytest.raises(BridgeBusy):
            bio.send("SET main.eps\nputs(1);\n", timeout=0.4, poll_interval=0.02)

    cmds = list((data_dir / "inbox").glob("srv-*.cmd"))
    assert len(cmds) == 1
    raw = cmds[0].read_bytes()
    assert raw[:3] != b"\xef\xbb\xbf"
    text = raw.decode("utf-8")
    first_line, _, body = text.partition("\n")
    assert first_line == "SET main.eps"
    assert body == "puts(1);\n"


# --------------------------------------------------------------------------- #
# Helper command coverage: ping / status / get
# --------------------------------------------------------------------------- #


def test_ping_status_get_helpers(tmp_path):
    data_dir = tmp_path / "agent"

    def responder(first_line, body):
        if first_line == "PING":
            return "PONG t"
        if first_line == "STATUS":
            return "compiling=false\r\nproject='P'\r\nversion=1"
        if first_line == "GET main.eps":
            return "puts(7);"
        return "ERROR: unknown"

    bio = _make_io(data_dir)
    with FakeBridge(data_dir, responder):
        assert bio.ping(timeout=3.0, poll_interval=0.02).strip() == "PONG t"
        assert "project='P'" in bio.status(timeout=3.0, poll_interval=0.02)
        assert bio.get("main.eps", timeout=3.0, poll_interval=0.02) == "puts(7);"


def test_get_error_raises(tmp_path):
    data_dir = tmp_path / "agent"

    def responder(first_line, body):
        return "ERROR: file not found: 'nope.eps'"

    bio = _make_io(data_dir)
    with FakeBridge(data_dir, responder):
        with pytest.raises(BridgeError):
            bio.get("nope.eps", timeout=3.0, poll_interval=0.02)


# --------------------------------------------------------------------------- #
# 7. cleanup_stale removes ONLY srv-* files (never agent_* legacy namespace)
# --------------------------------------------------------------------------- #


def test_cleanup_stale_removes_only_srv_files(tmp_path):
    data_dir = tmp_path / "agent"
    inbox = data_dir / "inbox"
    outbox = data_dir / "outbox"
    inbox.mkdir(parents=True)
    outbox.mkdir(parents=True)

    # Server-namespace leftovers (must be removed).
    (inbox / "srv-deadbeef.cmd").write_text("PING", encoding="utf-8")
    (outbox / "srv-deadbeef.result").write_text("PONG", encoding="utf-8")
    # Legacy runner namespace (must be UNTOUCHED).
    (inbox / "agent_42.cmd").write_text("PING", encoding="utf-8")
    (outbox / "agent_42.result").write_text("PONG", encoding="utf-8")

    bio = _make_io(data_dir)
    bio.cleanup_stale()

    assert not (inbox / "srv-deadbeef.cmd").exists()
    assert not (outbox / "srv-deadbeef.result").exists()
    assert (inbox / "agent_42.cmd").exists(), "legacy agent_* must be untouched"
    assert (outbox / "agent_42.result").exists(), "legacy agent_* must be untouched"


def test_cleanup_stale_tolerates_missing_dirs(tmp_path):
    """cleanup_stale on a fresh data dir (no inbox/outbox yet) does not raise."""
    data_dir = tmp_path / "agent"
    data_dir.mkdir(parents=True)
    bio = _make_io(data_dir)
    bio.cleanup_stale()  # must not raise


# --------------------------------------------------------------------------- #
# Module-surface sanity (exception types are distinct, importable names exist)
# --------------------------------------------------------------------------- #


def test_exception_hierarchy_distinct():
    assert issubclass(BridgeBusy, Exception)
    assert issubclass(BridgeError, Exception)
    assert BridgeBusy is not BridgeError
    # The module exposes the public surface the orchestrator imports.
    for name in ("BridgeIO", "BridgeBusy", "BridgeError"):
        assert hasattr(bridge_io, name)


if __name__ == "__main__":
    raise SystemExit(pytest.main([__file__, "-q"]))
