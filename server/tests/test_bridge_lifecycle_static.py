"""Verification artifact for EUD-013-6466: bridge server lifecycle (static).

Static source checks against ``bridge/ZZZ_10_agent_bridge.lua`` for the server
lifecycle extension (hivemind/docs/features/01_lua-bridge.md "Server lifecycle"
+ "Edge cases", architecture.md "Boot and lifecycle", rules.md Lua crash rules):

  1. agent.cfg read: ``File.ReadAllText`` on an ``agent.cfg`` path, and Lua
     string-match extraction of the three flat keys python_exe / repo_root /
     port (no JSON lib in KopiLua).
  2. Spawn block: a ``ProcessStartInfo`` with ``UseShellExecute=false``,
     ``CreateNoWindow=true``, ``WorkingDirectory``, the ``-m eud_agent``
     argument, and the spawned Process object retained in a GLOBAL (GC guard +
     pid source) -- a bare assignment (no ``local``) or a documented global
     table.
  3. Heartbeat ORDER (rules.md hard rule): inside the Tick handler the
     ``heartbeat.txt`` write must occur BEFORE the ``IsCompilng`` early-return
     (positional: heartbeat index < IsCompilng index).
  4. Ready validation: a ``pid`` string-compare marker AND a write-time / mtime
     marker against the bridge start time; and NEVER ``Process.GetProcessById``
     anywhere in the file (uncatchable .NET exception for dead pids).
  5. Respawn throttle: a ``HasExited`` marker (safe on an owned handle) AND a
     30(-second) throttle constant.
  6. Marker version: the ``bridge_loaded`` marker write contains ``v7`` and no
     longer the stale ``v5``.
  7. Degrade path: a ``bridge_error.log`` marker (agent.cfg missing/unparseable
     -> log + skip spawn, keep serving file IPC).
  8. Regression: ALL v6 + LIST + NEWEPS command branches remain; no
     ``os.execute`` / ``io.popen`` / ``:GetValue(`` anywhere; non-ASCII byte
     count must not grow over the v6 baseline (lifecycle additions are
     ASCII-only -- English log/marker strings per spec).

This file is pytest-compatible (plain ``test_*`` functions with asserts)
AND standalone-runnable with system Python::

    python server/tests/test_bridge_lifecycle_static.py

The project venv does not exist yet, so only the stdlib is used.

Checks 1-7 FAIL before the lifecycle extension is implemented; check 8 passes
throughout (regression guard).
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

# repo_root: server/tests/test_bridge_lifecycle_static.py -> parents[2] == repo root
REPO_ROOT = Path(__file__).resolve().parents[2]

BRIDGE = REPO_ROOT / "bridge" / "ZZZ_10_agent_bridge.lua"

# v6 command markers + LIST (EUD-011) + NEWEPS (EUD-012) that must survive
# import-then-extend. Each is matched as a dispatcher branch ``cmd == "<NAME>"``
# so a stray substring elsewhere cannot satisfy the check.
ALL_COMMANDS = (
    "PING",
    "STATUS",
    "LIST",
    "DUMP",
    "GET",
    "SET",
    "NEWEPS",
    "GETDAT",
    "SETDAT",
    "BUILD",
    "LUA",
    "PANEL",
)

# Known non-ASCII byte count in the verified v6 import + LIST + NEWEPS (Korean
# mojibake in comments + WPF panel UI strings + Korean error messages). The
# lifecycle extension is ASCII-only (English log + marker + JSON-key strings per
# spec), so this count must not increase. Computed from the source on disk at
# task start: 1263 bytes.
BASELINE_NONASCII_BYTES = 1_263


def _read_text() -> str:
    # latin-1 round-trips every byte 1:1, matching how KopiLua reads the source.
    return BRIDGE.read_bytes().decode("latin-1")


def _branch_re(name: str) -> re.Pattern[str]:
    """Match a dispatcher branch comparing ``cmd`` to a command name."""
    return re.compile(r'cmd\s*==\s*"' + re.escape(name) + r'"')


def _tick_region(text: str) -> str:
    """Source region of the DispatcherTimer Tick handler.

    From ``timer.Tick:Add`` to the following ``timer:Start()`` -- the span where
    the per-Tick lifecycle logic (heartbeat, ready validation, respawn) lives.
    """
    start = re.search(r"timer\.Tick:Add", text)
    assert start, "no `timer.Tick:Add` handler found"
    end = re.search(r"timer:Start\(\)", text[start.start():])
    assert end, "no `timer:Start()` after the Tick handler"
    return text[start.start(): start.start() + end.start()]


def _validate_ready_body(text: str) -> str:
    """Source body of the ``validateReady`` local function.

    From ``local function validateReady()`` to the next ``local function``
    declaration (``maybeRespawn``) -- the span that owns the server.ready
    ownership check (pid/ppid compare + the stale-drop ``File.Delete``).
    """
    start = re.search(r"local\s+function\s+validateReady\s*\(", text)
    assert start, "no `local function validateReady(` definition found"
    rest = text[start.end():]
    end = re.search(r"local\s+function\s+\w+\s*\(", rest)
    assert end, "no following `local function` to bound validateReady"
    return text[start.start(): start.end() + end.start()]


# --------------------------------------------------------------------------
# baseline
# --------------------------------------------------------------------------
def test_bridge_file_present_and_nonempty():
    assert BRIDGE.is_file(), f"missing file: {BRIDGE}"
    assert BRIDGE.stat().st_size > 0, f"empty file: {BRIDGE}"


# --------------------------------------------------------------------------
# 1. agent.cfg read + 3-key extraction
# --------------------------------------------------------------------------
def test_agent_cfg_read_and_key_extraction():
    """Init reads agent.cfg via File.ReadAllText and extracts the 3 flat keys."""
    text = _read_text()
    assert "agent.cfg" in text, "no reference to agent.cfg"
    # File.ReadAllText applied to an agent.cfg path. Allow any expression in
    # between (path var concatenation), but the read call must exist and the cfg
    # must be the thing read.
    assert re.search(r"File\.ReadAllText", text), (
        "agent.cfg not read via File.ReadAllText"
    )
    assert re.search(
        r"agent\.cfg.*File\.ReadAllText|File\.ReadAllText.*agent\.cfg", text, re.S
    ), "File.ReadAllText is not associated with the agent.cfg path"
    # The three flat JSON keys must be extracted (string-matched) from the cfg.
    for key in ("python_exe", "repo_root", "port"):
        assert key in text, f"agent.cfg key not extracted: {key!r}"
    # A Lua string-match against the cfg text (no JSON lib): string.match / find
    # / gmatch on a quoted key. Heuristic: a string.match|find|gmatch call that
    # mentions one of the keys.
    assert re.search(
        r'string\.(match|find|gmatch)\s*\([^)]*(python_exe|repo_root|port)', text
    ), "no Lua string-match extraction of the cfg keys (string.match/find/gmatch)"


# --------------------------------------------------------------------------
# 2. spawn block
# --------------------------------------------------------------------------
def test_spawn_processstartinfo_flags():
    """Spawn uses ProcessStartInfo with the required no-window flags."""
    text = _read_text()
    assert "ProcessStartInfo" in text, (
        "no ProcessStartInfo (spawn must use luanet Process)"
    )
    # UseShellExecute=false and CreateNoWindow=true (Lua `false`/`true`),
    # tolerant of spacing around `=`.
    assert re.search(r"UseShellExecute\s*=\s*false", text), (
        "ProcessStartInfo.UseShellExecute must be set to false"
    )
    assert re.search(r"CreateNoWindow\s*=\s*true", text), (
        "ProcessStartInfo.CreateNoWindow must be set to true"
    )
    assert "WorkingDirectory" in text, "ProcessStartInfo.WorkingDirectory not set"


def test_spawn_module_argument():
    """The server is launched as `-m eud_agent`."""
    text = _read_text()
    assert "-m eud_agent" in text, "spawn must pass `-m eud_agent`"


def test_spawn_passes_data_dir_argument():
    """The spawn Arguments pass ``--data-dir "<agent dir>"`` (EUD-036-9163).

    The server keys server.ready / heartbeat.txt / inbox/outbox off
    ``cfg.data_dir``; without a CLI signal it resolves empty and server.ready
    lands relative to the server cwd, so the bridge never validates readiness.
    ``__main__.py`` accepts ``--data-dir`` (top precedence via Config.resolve),
    so the spawn Arguments must carry it.

    Match the ``psi.Arguments`` assignment specifically (not a stray substring),
    tolerant of Lua string concatenation: ``-m eud_agent --data-dir`` followed by
    an opening quote and a ``..``-concatenated path expression. The trailing
    backslash on ``agentDir`` must be stripped before quoting so ``"...\\"`` does
    not escape the closing quote on the CreateProcess command line.
    """
    text = _read_text()
    arg_assign = re.search(r"\.Arguments\s*=\s*([^\n]*)", text)
    assert arg_assign, "no `psi.Arguments =` assignment found in the spawn block"
    rhs = arg_assign.group(1)
    assert "--data-dir" in rhs, (
        "spawn Arguments must include `--data-dir` so the server keys "
        "server.ready/heartbeat/inbox off the editor Data\\agent dir; got: "
        f"{rhs!r}"
    )
    # The command-line literal opens `-m eud_agent --data-dir "` then the Lua
    # string literal closes (`'` or `"`) and the path is `..`-concatenated in.
    # Tolerant of the literal quote style: `... --data-dir "' .. <path> .. '"'`
    # (single-quoted Lua literal) or the double-quoted equivalent. Requires a
    # concatenation (`..`) so a bare empty `--data-dir ""` cannot satisfy it.
    assert re.search(
        r"""-m eud_agent --data-dir "['"]\s*\.\.\s*.+\.\.""", rhs
    ), (
        "spawn Arguments must be `-m eud_agent --data-dir \"` concatenated (`..`) "
        f"with the quoted (trailing-backslash-stripped) agent dir path; got: {rhs!r}"
    )
    # The trailing backslash on agentDir must be stripped before quoting (so the
    # closing quote is not escaped on the CreateProcess command line): the RHS
    # must NOT concatenate the raw `agentDir` straight into the quotes.
    assert "agentDir" not in rhs or re.search(r"string\.sub\s*\(\s*agentDir", rhs), (
        "the trailing backslash on agentDir must be stripped (e.g. "
        "string.sub(agentDir, 1, -2)) before quoting, else `...\\\"` escapes the "
        f"closing quote on the CreateProcess command line; got: {rhs!r}"
    )


def test_spawn_process_stored_in_global():
    """The spawned Process object is retained in a GLOBAL (GC guard + pid source).

    Heuristic: an assignment to a name that is NOT declared ``local`` on that
    line, where the right-hand side starts a Process (``Process(`` /
    ``Process.Start`` / ``:Start()`` on a ProcessStartInfo). A ``local``-scoped
    handle would be collected and the pid lost.
    """
    text = _read_text()
    # Find an assignment whose RHS produces a Process and whose LHS is global.
    # Accept `Name = Process(...)`, `Name = Process.Start(...)`, or a global
    # table field `agentProc = ...` / `_G.x = ...`.
    global_assign = re.search(
        r'(?m)^(?!\s*local\b)\s*[A-Za-z_][\w.]*\s*=\s*'
        r'(Process(\.Start)?\s*\(|[A-Za-z_][\w]*:Start\s*\()',
        text,
    )
    # Also accept an explicit documented global table field assignment that is
    # later used as the pid/HasExited source even if RHS is indirected.
    documented_global = re.search(
        r'(?m)^(?!\s*local\b)\s*[A-Za-z_][\w]*\s*=\s*[A-Za-z_][\w]*\s*--\s*global', text
    )
    assert global_assign or documented_global, (
        "spawned Process is not retained in a global (no non-local assignment "
        "of a Process(...) / :Start() result; needed as GC guard + pid source)"
    )


# --------------------------------------------------------------------------
# 3. heartbeat ORDER (before IsCompilng early-return)
# --------------------------------------------------------------------------
def test_heartbeat_written_before_iscompiling_check():
    """Within the Tick handler, the heartbeat write precedes the IsCompilng check."""
    text = _read_text()
    region = _tick_region(text)
    hb = re.search(r"heartbeat\.txt", region)
    assert hb, "no heartbeat.txt write inside the Tick handler"
    isc = re.search(r"IsCompilng", region)
    assert isc, "no IsCompilng check inside the Tick handler"
    assert hb.start() < isc.start(), (
        "heartbeat.txt must be written BEFORE the IsCompilng early-return "
        f"(heartbeat@{hb.start()} vs IsCompilng@{isc.start()}) -- rules.md hard rule"
    )


# --------------------------------------------------------------------------
# 4. ready validation (pid string-compare + mtime), never GetProcessById
# --------------------------------------------------------------------------
def test_ready_validation_pid_and_mtime():
    """server.ready is validated by pid string-compare AND a write-time/mtime check."""
    text = _read_text()
    assert "server.ready" in text, "no server.ready reference"
    # pid extracted from the ready JSON text and compared (string compare on the
    # JSON value). Require both a 'pid' mention near server.ready and a compare.
    assert re.search(r'"?pid"?', text) and "pid" in text, (
        "no pid handling for server.ready"
    )
    # A string match for the pid value out of the ready JSON.
    assert re.search(r'string\.(match|find)\s*\([^)]*pid', text, re.I), (
        "pid is not string-matched out of the server.ready JSON"
    )
    # A write-time / mtime check against the bridge start time. Accept
    # GetLastWriteTime (.NET) or a tracked start-time comparison.
    assert re.search(r"GetLastWriteTime|LastWriteTime|WriteTime|mtime", text), (
        "no server.ready write-time / mtime check against bridge start"
    )


def test_never_getprocessbyid():
    """GetProcessById must never appear (uncatchable .NET exception for dead pids)."""
    text = _read_text()
    assert "GetProcessById" not in text, (
        "GetProcessById is forbidden (uncatchable for dead pids); use the owned "
        "handle's HasExited / pid string-compare instead"
    )


# --------------------------------------------------------------------------
# 4b. ppid ownership acceptance (EUD-037-897c)
# --------------------------------------------------------------------------
# The bridge spawns the server through the venv launcher
# (server\.venv\Scripts\python.exe), which on Windows re-execs the base
# interpreter as a CHILD process. The bridge owns the LAUNCHER pid; the server
# writes its OWN (child) pid into server.ready and now also its parent pid
# (ppid == launcher). validateReady() must accept ownership when ownPid matches
# EITHER the ready pid OR the ready ppid, and delete server.ready ONLY when
# NEITHER matches -- otherwise the launcher-vs-child pid mismatch takes the
# stale branch, server.ready is deleted, agentSrvReady never flips, and the
# panel never navigates (the live E2E failure on 2026-06-04).


def test_validate_ready_extracts_ppid_from_ready_json():
    """validateReady() string-matches a ``"ppid"`` value out of the ready JSON.

    Bound to the validateReady body (not a stray substring elsewhere): a
    ``string.match`` on the ready text whose key literal is ``"ppid"``. The
    existing ``'"pid"%s*:%s*(%d+)'`` pattern can NEVER match inside ``"ppid"``
    (its leading quote would have to precede the ``pid`` run, but ``"ppid"``
    puts a ``p`` there) -- so the ppid is a SEPARATE value that needs its own
    explicit ``"ppid"``-keyed capture inside a ``string.match`` call.
    """
    body = _validate_ready_body(_read_text())
    assert "ppid" in body, (
        "validateReady() does not reference a ppid (the bridge owns the venv "
        "LAUNCHER pid; the ready ppid is the only field that matches it)"
    )
    assert re.search(r'string\.match\s*\([^)]*"ppid"', body), (
        "validateReady() must string-match a `\"ppid\"`-keyed value out of the "
        "ready JSON (a distinct capture -- the `\"pid\"` pattern cannot reach "
        "the ppid field, so a second `\"ppid\"`-keyed match is required)"
    )


def test_validate_ready_or_matches_pid_or_ppid_against_own_pid():
    """Ownership accepts when ownPid matches EITHER the ready pid OR the ready ppid.

    The current pid-only body has a single ``== ownPid`` comparison; the fix
    adds a second (the ppid var) joined by an ``or``. Require BOTH: two distinct
    ``== ownPid`` comparisons in the validateReady body AND an ``or`` operator
    combining the ownership condition. The pid-only body has exactly one
    ``== ownPid`` and no ownership ``or``, so this fails today.
    """
    body = _validate_ready_body(_read_text())
    own_pid_cmps = re.findall(r"==\s*ownPid\b", body)
    assert len(own_pid_cmps) >= 2, (
        "validateReady() must compare ownPid against BOTH the ready pid AND the "
        "ready ppid (the bridge spawns via the venv launcher: ownPid == ready "
        f"ppid, not ready pid); found {len(own_pid_cmps)} `== ownPid` compare(s)"
    )
    # An `or` joining the two ownership comparisons (accept on either match).
    assert re.search(r"==\s*ownPid\b[^\n]*\bor\b|\bor\b[^\n]*==\s*ownPid\b", body), (
        "the pid/ppid ownership comparisons must be joined by `or` (accept when "
        "ownPid matches EITHER the ready pid OR the ready ppid)"
    )


def test_validate_ready_deletes_only_when_neither_pid_nor_ppid_matches():
    """server.ready is dropped ONLY when NEITHER pid NOR ppid matches ownPid.

    The stale-drop ``File.Delete(readyPath)`` must be reached only after both
    the pid AND the ppid comparison fail -- i.e. it sits AFTER both
    ``== ownPid`` comparisons in the validateReady body (it is the else of the
    or-combined accept condition). The pid-only body deletes whenever the sole
    pid compare fails, which wrongly drops a ready owned by the launcher's child.
    """
    body = _validate_ready_body(_read_text())
    delete = re.search(r"File\.Delete\s*\(\s*readyPath\s*\)", body)
    assert delete, (
        "validateReady() must keep the stale-drop File.Delete(readyPath) (the "
        "neither-matches branch); none found in the validateReady body"
    )
    own_pid_cmps = list(re.finditer(r"==\s*ownPid\b", body))
    assert len(own_pid_cmps) >= 2, (
        "expected two `== ownPid` comparisons (pid AND ppid) before the "
        f"stale-drop delete; found {len(own_pid_cmps)}"
    )
    # The delete must follow BOTH ownership comparisons (it is the else of the
    # or-combined accept condition -- only when NEITHER pid nor ppid matched).
    assert delete.start() > own_pid_cmps[1].start(), (
        "File.Delete(readyPath) must be reached only when NEITHER the pid NOR "
        "the ppid matches ownPid (it must follow both `== ownPid` comparisons); "
        "the pid-only body deletes on the sole pid mismatch, dropping a ready "
        "legitimately owned by the launcher's child process"
    )


# --------------------------------------------------------------------------
# 5. respawn throttle
# --------------------------------------------------------------------------
def test_respawn_hasexited_and_throttle():
    """Respawn keys off the owned handle's HasExited, throttled to once per 30s."""
    text = _read_text()
    assert "HasExited" in text, (
        "no HasExited check (respawn must test the owned process handle)"
    )
    # A 30-second throttle constant. Accept TimeSpan.FromSeconds(30), a bare 30
    # used as a seconds throttle, or 30000 ms. Match a standalone 30.
    assert re.search(r"\b30\b", text), "no 30-second respawn throttle constant"


# --------------------------------------------------------------------------
# 6. marker version v7 (not v5)
# --------------------------------------------------------------------------
def test_bridge_loaded_marker_is_v7():
    """The bridge_loaded marker write says v7 and no longer the stale v5."""
    text = _read_text()
    m = re.search(r'bridge_loaded\.txt[^\n]*', text)
    assert m, "no bridge_loaded.txt marker write"
    line = m.group(0)
    assert "v7" in line, f"bridge_loaded marker must say v7; got: {line!r}"
    assert "v5" not in line, f"stale 'v5' still in bridge_loaded marker: {line!r}"


# --------------------------------------------------------------------------
# 7. degrade path -> bridge_error.log
# --------------------------------------------------------------------------
def test_degrade_logs_to_bridge_error_log():
    """agent.cfg missing/unparseable logs to bridge_error.log (then skips spawn).

    The verified v6 source already writes ``bridge_error.log`` once -- from the
    Tick exception handler (after ``timer:Start()``-adjacent dispatch). The init
    degrade path is a SEPARATE write in the init body, BEFORE the Tick handler
    is wired (``timer.Tick:Add``). Require a ``bridge_error.log`` write in that
    init region, AND a ``logError(`` call inside the cfg-parse degrade region
    (the cfg ``do`` block, where the error/else branch logs) -- so deleting the
    degrade log call while keeping the ``bridge_error.log`` constant fails here.
    """
    text = _read_text()
    assert "bridge_error.log" in text, (
        "no bridge_error.log marker (degrade path: log + skip spawn, keep file IPC)"
    )
    tick = re.search(r"timer\.Tick:Add", text)
    assert tick, "no `timer.Tick:Add` handler found"
    init_region = text[: tick.start()]
    assert "bridge_error.log" in init_region, (
        "the degrade path must log to bridge_error.log in the init body (before "
        "`timer.Tick:Add`); the only v6 occurrence is the Tick exception handler"
    )
    # The bridge_error.log write must be wrapped in a reusable logger that the
    # degrade path calls. Require its definition in the init region.
    assert re.search(r"function\s+logError\b|logError\s*=\s*function", init_region), (
        "no logError() helper defined in the init body (degrade path should log "
        "via a reusable logger, not an inline write)"
    )
    # The cfg-parse degrade region: from the cfg read (File.ReadAllText(cfgPath))
    # to the spawn function that consumes the parsed cfg. The degrade branch must
    # call logError() within this region.
    cfg_read = re.search(r"File\.ReadAllText\s*\(\s*cfgPath\s*\)", init_region)
    assert cfg_read, "agent.cfg is not read via File.ReadAllText(cfgPath)"
    spawn_def = re.search(r"function\s+spawnServer\b", init_region)
    assert spawn_def and spawn_def.start() > cfg_read.start(), (
        "spawnServer() not found after the cfg read; cannot bound the degrade region"
    )
    degrade_region = init_region[cfg_read.start(): spawn_def.start()]
    assert re.search(r"logError\s*\(", degrade_region), (
        "the cfg-parse degrade branch must CALL logError(...) (missing/unparseable "
        "cfg -> log + skip spawn); a bare bridge_error.log constant is insufficient"
    )


# --------------------------------------------------------------------------
# 8. regression (passes throughout)
# --------------------------------------------------------------------------
def test_all_command_markers_present():
    """All v6 + LIST + NEWEPS dispatcher commands survive (import-then-extend)."""
    text = _read_text()
    missing = [c for c in ALL_COMMANDS if not _branch_re(c).search(text)]
    assert not missing, f"missing command branches: {missing}"


def test_no_forbidden_lua_calls():
    """Crash-rule lint: no os.execute / io.popen / :GetValue( anywhere."""
    text = _read_text()
    forbidden = [tok for tok in ("os.execute", "io.popen", ":GetValue(") if tok in text]
    assert not forbidden, f"forbidden lua calls present: {forbidden}"


def test_extension_adds_no_raw_nonascii_bytes():
    """The lifecycle extension is ASCII-only: total non-ASCII bytes must not grow.

    The v6 + LIST + NEWEPS source already carries Korean mojibake
    (1,263 non-ASCII bytes). Lifecycle log/marker/JSON-key strings are English,
    so the count must stay <= the baseline.
    """
    raw = BRIDGE.read_bytes()
    nonascii = sum(1 for b in raw if b > 0x7F)
    assert nonascii <= BASELINE_NONASCII_BYTES, (
        f"non-ASCII byte count grew to {nonascii} (baseline "
        f"{BASELINE_NONASCII_BYTES}); the lifecycle extension must be ASCII-only"
    )


def _all_test_functions():
    module = sys.modules[__name__]
    return [
        (name, obj)
        for name, obj in sorted(vars(module).items())
        if name.startswith("test_") and callable(obj)
    ]


def main() -> int:
    failures = 0
    for name, fn in _all_test_functions():
        try:
            fn()
        except AssertionError as exc:
            failures += 1
            print(f"FAIL {name}: {exc}")
        except Exception as exc:  # unexpected (e.g. missing file)
            failures += 1
            print(f"ERROR {name}: {type(exc).__name__}: {exc}")
        else:
            print(f"PASS {name}")
    total = len(_all_test_functions())
    print(f"\n{total - failures}/{total} checks passed")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
