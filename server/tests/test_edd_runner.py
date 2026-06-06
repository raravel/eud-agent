"""Verification artifact for EUD-057-e71d: the euddraft build runner + self-fix.

These tests drive ``eud_agent.edd_runner`` (the build_run pipeline + the
error-retrieval ladder + the euddraft direct-run parser) and the tool-layer
wiring of the 3-attempt self-fix budget, per features/05 "Build error retrieval
and self-fix".

Grounding for the two euddraft error formats replicated by the parser is the
editor source ``EUD-Editor-3\\EUD Editor 3\\Module\\Tools\\BuildErrorHandling.vb``:

  * the MODULE/LINE form (``BuildErrorHandling.vb:23``):
        ``\\[Error.*\\] Module ""(.*)"" Line (\\d+) : (.+)``
    groups -> (module, line, message).
  * the PYTHON-TRACEBACK form: the ``[Error]`` description regex (line 42)
        ``\\[Error\\](.*)Traceback \\(most recent call last\\):``
    plus the file/line regex (line 49)
        ``File ""(.*)"", line (\\d+), in ([\\w_]+)``
    where the file path is reduced to its basename without extension
    (line 57: ``FilePath.Split("\\").Last.Split(".").First``).

The pipeline:

  1. bridge BUILD (already hardened: returns ``OK: started``) -> poll
     ``status.txt`` ``compiling=`` until false (timeout 300s, injectable);
     the flag is parsed with ``engine.parse_status`` (the SAME parse the engine
     uses -- it lowercases the value, so VB's ``compiling=True`` is handled).
  2. ladder: bridge ``BUILDERR`` (macro errors) SHORT-CIRCUITS. Only when the
     build FAILED with NO macro errors does the runner re-run ``euddraft.exe``
     directly (eds + save-map paths from bridge ``EDSPATH``; euddraft path from
     ``getset('program','euddraft')``) with captured stdout/stderr and EXPLICIT
     stdin (``subprocess.DEVNULL``), resolved absolute path, cwd set.
  3. build SUCCESS signal = the output map (SaveMapName) exists.

``eud_agent.edd_runner`` does NOT exist during Step A, so this suite is expected
to FAIL on import until edd_runner.py is implemented (Step B).
"""

from __future__ import annotations

import pytest

# Imported at collection so the failing import is the first signal in Step A.
from eud_agent.edd_runner import (
    BuildError,
    EddRunner,
    parse_euddraft_output,
)
from eud_agent.tools import (
    BUILD_FIX_LIMIT,
    RequestState,
    ToolError,
    ToolLayer,
)

# --------------------------------------------------------------------------- #
# Fakes: a bridge recording BUILD/BUILDERR/EDSPATH/getset; a status.txt source;
# a recorded fake subprocess runner asserting spawn-arg compliance.
# --------------------------------------------------------------------------- #


class FakeBridge:
    """Records BUILD/BUILDERR/EDSPATH/getset and returns scripted replies."""

    def __init__(
        self,
        *,
        builderr: str = "",
        eds_path: str = r"C:\tmp\proj.eds",
        save_map: str = r"C:\maps\out.scx",
        euddraft: str = r"C:\euddraft\euddraft.exe",
    ):
        self.calls: list[str] = []
        self._builderr = builderr
        self._eds_path = eds_path
        self._save_map = save_map
        self._euddraft = euddraft

    def build(self, **kw):
        self.calls.append("build")
        return "OK: started"

    def builderr(self, **kw):
        self.calls.append("builderr")
        return self._builderr

    def edspath(self, **kw):
        self.calls.append("edspath")
        return f"{self._eds_path}\r\n{self._save_map}"

    def getset(self, scope, key, **kw):
        self.calls.append(f"getset:{scope}:{key}")
        if scope == "program" and key == "euddraft":
            return f"OK: program|euddraft = {self._euddraft}"
        return "OK: = "


class RecordingSpawn:
    """A fake subprocess.run replacement that records the call for compliance.

    Asserting: explicit stdin (DEVNULL), a resolved absolute exe path, cwd set.
    Returns a result object exposing ``stdout``/``stderr``/``returncode``.
    """

    def __init__(self, *, stdout="", stderr="", returncode=0):
        self.recorded: dict | None = None
        self._stdout = stdout
        self._stderr = stderr
        self._returncode = returncode

    def __call__(self, args, **kwargs):
        self.recorded = {"args": args, "kwargs": kwargs}

        class _R:
            pass

        r = _R()
        r.stdout = self._stdout
        r.stderr = self._stderr
        r.returncode = self._returncode
        return r


# Canonical fixtures for the two euddraft error formats (verbatim shapes the
# editor's regexes match).
MODULE_LINE_OUTPUT = (
    "Building...\n"
    '[Error/eps] Module "main" Line 42 : unexpected token ")"\n'
    '[Error] Module "util/helper" Line 7 : undefined name foo\n'
    "Build failed.\n"
)

TRACEBACK_OUTPUT = (
    "[Error] something went very wrong Traceback (most recent call last):\n"
    '  File "C:\\proj\\scripts\\boot.py", line 13, in run\n'
    "    raise ValueError('boom')\n"
    "ValueError: boom\n"
)


# --------------------------------------------------------------------------- #
# Parser: BOTH documented formats.
# --------------------------------------------------------------------------- #


def test_parse_module_line_format():
    errors = parse_euddraft_output(MODULE_LINE_OUTPUT, "")
    assert len(errors) == 2
    e0 = errors[0]
    assert isinstance(e0, BuildError)
    assert e0.source == "euddraft"
    assert e0.file == "main"
    assert e0.line == 42
    assert 'unexpected token ")"' in e0.message
    assert errors[1].file == "util/helper"
    assert errors[1].line == 7


def test_parse_traceback_format_basename_without_ext():
    errors = parse_euddraft_output(TRACEBACK_OUTPUT, "")
    assert len(errors) == 1
    e = errors[0]
    assert e.source == "euddraft"
    # FilePath.Split("\").Last.Split(".").First -> "boot"
    assert e.file == "boot"
    assert e.line == 13
    assert "something went very wrong" in e.message


def test_parse_traceback_multiline_description_is_kept():
    # Measured live (EUD-088): eudplib emits the [Error] description across
    # MULTIPLE lines before "Traceback (most recent call last):" — the
    # single-line desc regex matched nothing, so the entire human message was
    # dropped and codex only saw file/line with an empty message.
    blob = (
        "[Error] 연결맵에 조건에 맞는 플레이어가 없습니다: 플레이어 종류 Human\n"
        "스타트 로케이션을 제대로 배치했는지 확인해보세요. "
        "Traceback (most recent call last):\n"
        '  File "C:\\editor\\Data\\temp\\BuildData_test\\eudplibData'
        '\\TriggerEditor\\main.eps.eps", line 311, in onPluginStart\n'
        "    function onPluginStart() {\n"
        '  File "euddraft\\.venv\\Lib\\site-packages\\eudplib\\eudlib\\utilf'
        '\\pexist.py", line 100, in EUDLoopPlayer\n'
        "eudplib.utils.eperror.EPError: 연결맵에 조건에 맞는 플레이어가 "
        "없습니다: 플레이어 종류 Human\n"
    )
    errors = parse_euddraft_output(blob, "")
    assert len(errors) == 1
    e = errors[0]
    # The FULL multi-line description must survive into the message.
    assert "연결맵에 조건에 맞는 플레이어가 없습니다" in e.message
    assert "스타트 로케이션" in e.message
    # file/line still come from the FIRST traceback frame (editor mirror).
    assert e.file == "main"
    assert e.line == 311


def test_parse_module_form_takes_precedence_over_traceback():
    # The editor only falls back to the traceback form when the module form
    # matched ZERO lines (mcol.Count == 0). A mixed blob with module matches
    # must therefore yield ONLY module-form errors.
    blob = MODULE_LINE_OUTPUT + TRACEBACK_OUTPUT
    errors = parse_euddraft_output(blob, "")
    assert {e.file for e in errors} == {"main", "util/helper"}


def test_parse_reads_stderr_too():
    errors = parse_euddraft_output("", TRACEBACK_OUTPUT)
    assert len(errors) == 1
    assert errors[0].file == "boot"


def test_parse_no_errors_returns_empty():
    assert parse_euddraft_output("Build succeeded.\n", "") == []


# --------------------------------------------------------------------------- #
# Ladder behavior.
# --------------------------------------------------------------------------- #


def make_runner(
    bridge, *, spawn=None, statuses=None, exists=None, mtimes=None, **kw
):
    """Build an EddRunner over fakes.

    ``statuses`` is the sequence of status.txt contents returned per poll;
    ``exists`` maps a path -> bool for the output-map success check; ``mtimes``
    maps a path -> a SEQUENCE of mtimes returned per call (so a stale map returns
    the SAME mtime before/after, a fresh one returns an advanced mtime). When
    ``mtimes`` is omitted the output map mtime is None pre-build and a fixed
    positive value after (so an existing+absent-before map reads as fresh).
    """
    statuses = list(statuses or ["compiling=False"])

    def read_status():
        return statuses.pop(0) if len(statuses) > 1 else statuses[0]

    exists = exists or {}

    def path_exists(p):
        return exists.get(p, False)

    mtime_seqs = {k: list(v) for k, v in (mtimes or {}).items()}
    _calls = {}

    def path_mtime(p):
        seq = mtime_seqs.get(p)
        if seq is not None:
            return seq.pop(0) if len(seq) > 1 else seq[0]
        # Default: FIRST call (pre-build snapshot) -> None (treat as not-yet-built
        # so a successful build's map reads as freshly created); subsequent calls
        # (post-build) -> a fixed positive mtime.
        n = _calls.get(p, 0)
        _calls[p] = n + 1
        return None if n == 0 else 100.0

    return EddRunner(
        bridge,
        spawn=spawn or RecordingSpawn(),
        read_status=read_status,
        path_exists=path_exists,
        path_mtime=path_mtime,
        poll_interval=0.0,
        timeout=5.0,
        **kw,
    )


def test_macro_errors_short_circuit_no_euddraft_rerun():
    bridge = FakeBridge(builderr='[Error] Module "m" Line 3 : bad')
    spawn = RecordingSpawn()
    runner = make_runner(
        bridge, spawn=spawn, exists={r"C:\maps\out.scx": False}
    )
    result = runner.build_run()
    # macro errors present -> NO euddraft re-run (no euddraft subprocess spawned,
    # no getset for the euddraft path). edspath IS called once pre-build for the
    # freshness snapshot -- that is expected.
    assert spawn.recorded is None
    assert not any(c.startswith("getset") for c in bridge.calls)
    assert any(e.source == "macro" for e in result.errors)
    assert result.ok is False


def test_euddraft_rerun_only_when_failed_and_no_macro_errors():
    # No macro errors AND the output map is absent -> failed -> re-run euddraft.
    bridge = FakeBridge(builderr="")
    spawn = RecordingSpawn(stdout=MODULE_LINE_OUTPUT, returncode=1)
    runner = make_runner(
        bridge, spawn=spawn, exists={r"C:\maps\out.scx": False}
    )
    result = runner.build_run()
    assert spawn.recorded is not None
    assert any(e.source == "euddraft" for e in result.errors)
    assert result.ok is False


def test_success_when_output_map_exists_and_no_macro_errors():
    bridge = FakeBridge(builderr="")
    spawn = RecordingSpawn()
    runner = make_runner(
        bridge, spawn=spawn, exists={r"C:\maps\out.scx": True}
    )
    result = runner.build_run()
    # Output map exists -> success -> NO euddraft re-run, no errors.
    assert result.ok is True
    assert spawn.recorded is None
    assert result.errors == []


# --------------------------------------------------------------------------- #
# Poll timeout.
# --------------------------------------------------------------------------- #


def test_poll_timeout_raises():
    bridge = FakeBridge(builderr="")
    # status.txt always reports compiling -> the poll never clears -> timeout.
    runner = EddRunner(
        bridge,
        spawn=RecordingSpawn(),
        read_status=lambda: "compiling=True",
        path_exists=lambda p: False,
        poll_interval=0.0,
        timeout=0.0,
    )
    with pytest.raises(TimeoutError):
        runner.build_run()


# --------------------------------------------------------------------------- #
# Spawn-arg compliance (rules.md codex-invocation rules apply to ALL spawns).
# --------------------------------------------------------------------------- #


def test_euddraft_spawn_explicit_stdin_resolved_path_cwd():
    import subprocess
    from pathlib import Path

    bridge = FakeBridge(
        builderr="",
        eds_path=r"C:\tmp\proj.eds",
        euddraft=r"C:\euddraft\euddraft.exe",
    )
    spawn = RecordingSpawn(stdout="Build failed.\n", returncode=1)
    runner = make_runner(
        bridge, spawn=spawn, exists={r"C:\maps\out.scx": False}
    )
    runner.build_run()
    rec = spawn.recorded
    assert rec is not None
    args = rec["args"]
    kwargs = rec["kwargs"]
    # resolved absolute exe path (never a bare name); the eds path is the arg.
    assert args[0] == r"C:\euddraft\euddraft.exe"
    assert r"C:\tmp\proj.eds" in args
    # explicit stdin == DEVNULL.
    assert kwargs.get("stdin") == subprocess.DEVNULL
    # cwd is set (to the eds file's directory).
    assert kwargs.get("cwd") == str(Path(r"C:\tmp\proj.eds").parent)
    # output captured.
    assert kwargs.get("capture_output") is True or (
        kwargs.get("stdout") is not None and kwargs.get("stderr") is not None
    )
    # decode UTF-8 with replace (Korean-Windows OEM/cp949 would crash on UTF-8).
    assert kwargs.get("encoding") == "utf-8"
    assert kwargs.get("errors") == "replace"
    # an explicit wall-clock timeout caps a hung euddraft (no unbounded wait).
    assert kwargs.get("timeout") is not None and kwargs.get("timeout") > 0


# --------------------------------------------------------------------------- #
# Subprocess timeout + decode hardening (rules.md: never wait unbounded; Korean
# Windows cp949/strict decode must not crash on UTF-8 euddraft output).
# --------------------------------------------------------------------------- #


class _TimeoutSpawn:
    """A fake spawn that raises subprocess.TimeoutExpired (a hung euddraft)."""

    def __call__(self, args, **kwargs):
        import subprocess

        raise subprocess.TimeoutExpired(cmd=args, timeout=kwargs.get("timeout"))


def test_euddraft_subprocess_timeout_raises_timeouterror():
    bridge = FakeBridge(builderr="")
    runner = make_runner(
        bridge,
        spawn=_TimeoutSpawn(),
        exists={r"C:\maps\out.scx": False},
        subprocess_timeout=1.0,
    )
    # subprocess.TimeoutExpired -> TimeoutError (the ladder/tool-layer family).
    with pytest.raises(TimeoutError):
        runner.build_run()


def test_euddraft_output_decoded_with_replace_not_crash():
    # The real subprocess.run does the decoding; here we prove the runner passes
    # encoding/errors so a Python (utf-8) euddraft on Korean Windows (cp949) does
    # not raise UnicodeDecodeError. Drive a real subprocess.run via a tiny python
    # one-liner emitting bytes that are valid UTF-8 but invalid cp949.
    import sys

    bridge = FakeBridge(builderr="", euddraft=sys.executable)

    # The runner builds args [euddraft, eds_path]; we intercept the spawn to run a
    # script that writes a UTF-8 (Korean) error line, then parse it back.
    real_run = __import__("subprocess").run

    def spawn(args, **kwargs):
        # Replace the euddraft+eds args with a python -c that emits a UTF-8 error.
        script = (
            "import sys;"
            "sys.stdout.reconfigure(encoding='utf-8');"
            "print('[Error] \\ud55c\\uae00 \\uc624\\ub958 "
            "Traceback (most recent call last):');"
            "print('  File \"C:\\\\\\\\p\\\\\\\\boot.py\", line 5, in run')"
        )
        return real_run([sys.executable, "-c", script], **kwargs)

    runner = make_runner(
        bridge, spawn=spawn, exists={r"C:\maps\out.scx": False}
    )
    # Must NOT raise UnicodeDecodeError; the Korean text round-trips.
    result = runner.build_run()
    assert result.ok is False
    assert result.errors
    assert result.errors[0].file == "boot"


# --------------------------------------------------------------------------- #
# Stale-artifact false positive: a map from a PREVIOUS build must NOT read as a
# success for a FAILED current build.
# --------------------------------------------------------------------------- #


def test_stale_output_map_is_not_success():
    # The map exists both before and after the build with the SAME mtime (a
    # leftover from a prior build) -> NOT fresh -> failed -> euddraft re-run.
    bridge = FakeBridge(builderr="")
    spawn = RecordingSpawn(stdout=MODULE_LINE_OUTPUT, returncode=1)
    runner = make_runner(
        bridge,
        spawn=spawn,
        exists={r"C:\maps\out.scx": True},
        mtimes={r"C:\maps\out.scx": [500.0, 500.0]},  # unchanged across build
    )
    result = runner.build_run()
    assert result.ok is False
    # the stale map did not mask the failure -> the euddraft re-run happened.
    assert spawn.recorded is not None


def test_fresh_output_map_advanced_mtime_is_success():
    # The map existed before but its mtime ADVANCED across the build -> fresh.
    bridge = FakeBridge(builderr="")
    spawn = RecordingSpawn()
    runner = make_runner(
        bridge,
        spawn=spawn,
        exists={r"C:\maps\out.scx": True},
        mtimes={r"C:\maps\out.scx": [500.0, 999.0]},  # advanced -> fresh
    )
    result = runner.build_run()
    assert result.ok is True
    assert spawn.recorded is None


# --------------------------------------------------------------------------- #
# Config-error attempt-burn distinction (a static misconfig is NOT a build).
# --------------------------------------------------------------------------- #


def test_unconfigured_euddraft_raises_config_error():
    from eud_agent.edd_runner import ConfigError

    bridge = FakeBridge(builderr="", euddraft="")  # no euddraft path
    runner = make_runner(
        bridge, exists={r"C:\maps\out.scx": False}
    )
    with pytest.raises(ConfigError):
        runner.build_run()


def test_empty_eds_path_raises_config_error():
    from eud_agent.edd_runner import ConfigError

    bridge = FakeBridge(builderr="", eds_path="")  # no eds path
    runner = make_runner(
        bridge, exists={r"C:\maps\out.scx": False}
    )
    with pytest.raises(ConfigError):
        runner.build_run()


# --------------------------------------------------------------------------- #
# Tool-layer wiring: the 3-attempt self-fix cap.
# --------------------------------------------------------------------------- #


class _FixBridge:
    """Minimal bridge for ToolLayer build_run routing (records build calls)."""

    def __init__(self):
        self.calls: list[str] = []

    def build(self, **kw):
        self.calls.append("build")
        return "OK: started"


class _StubRunner:
    """A runner whose build_run returns a canned BuildRunResult and counts calls.

    Mirrors the real ``EddRunner`` contract: ``build_run`` records the outcome on
    ``last_result`` (the attribute ``build_errors`` reads).
    """

    def __init__(self, result):
        self.result = result
        self.runs = 0
        self.last_result = None

    def build_run(self):
        self.runs += 1
        self.last_result = self.result
        return self.result


def _result(ok=False, errors=None):
    from eud_agent.edd_runner import BuildRunResult

    return BuildRunResult(ok=ok, errors=errors or [])


def test_build_run_consumes_one_attempt_each():
    bridge = _FixBridge()
    runner = _StubRunner(_result(ok=False, errors=[]))
    layer = ToolLayer(bridge, runner_factory=lambda: runner)
    st = RequestState(request_id="r1")
    st.plan_approved = True  # lift the mutation gate for >2 build runs.
    for _ in range(BUILD_FIX_LIMIT):
        layer.call("build_run", {}, st, runner=runner)
    assert st.build_fix_attempts == BUILD_FIX_LIMIT
    assert runner.runs == BUILD_FIX_LIMIT


def test_fourth_build_run_returns_tool_error_budget_spent():
    bridge = _FixBridge()
    runner = _StubRunner(_result(ok=False, errors=[]))
    layer = ToolLayer(bridge, runner_factory=lambda: runner)
    st = RequestState(request_id="r1")
    st.plan_approved = True
    for _ in range(BUILD_FIX_LIMIT):
        layer.call("build_run", {}, st, runner=runner)
    with pytest.raises(ToolError) as ei:
        layer.call("build_run", {}, st, runner=runner)
    assert "self-fix" in str(ei.value).lower() or "budget" in str(ei.value).lower()
    # The 4th attempt did not run the runner again.
    assert runner.runs == BUILD_FIX_LIMIT


def test_build_fix_exhausted_flag_surfaced_for_changeset_note():
    # The "failure noted in the changeset" is surfaced via a RequestState flag the
    # engine can read (build_run is NOT journaled -- EUD-055 decision -- so it can
    # never be a changeset item). This is the minimal honest mechanism.
    bridge = _FixBridge()
    runner = _StubRunner(_result(ok=False, errors=[]))
    layer = ToolLayer(bridge, runner_factory=lambda: runner)
    st = RequestState(request_id="r1")
    st.plan_approved = True
    for _ in range(BUILD_FIX_LIMIT):
        layer.call("build_run", {}, st, runner=runner)
    assert st.build_fix_exhausted is False
    with pytest.raises(ToolError):
        layer.call("build_run", {}, st, runner=runner)
    assert st.build_fix_exhausted is True


def test_build_errors_returns_last_ladder_result():
    # build_errors returns the LAST build's structured ladder result (kept on the
    # runner). After a build_run, the read tool reflects those errors.
    bridge = _FixBridge()
    errs = [BuildError(source="euddraft", file="main", line=1, message="x", raw="x")]
    runner = _StubRunner(_result(ok=False, errors=errs))
    layer = ToolLayer(bridge, runner_factory=lambda: runner)
    st = RequestState(request_id="r1")
    layer.call("build_run", {}, st, runner=runner)
    out = layer.call("build_errors", {}, st, runner=runner)
    # structured entries reflect the last build.
    assert isinstance(out, list)
    assert out and out[0]["file"] == "main"


def test_build_run_without_runner_falls_back_to_plain_build():
    # A ToolLayer built WITHOUT a runner_factory keeps the current plain
    # bridge.build() behavior (additive integration; existing constructions work).
    bridge = _FixBridge()
    layer = ToolLayer(bridge)
    st = RequestState(request_id="r1")
    layer.call("build_run", {}, st)
    assert bridge.calls == ["build"]


class _RaisingRunner:
    """A runner whose build_run raises a given exception (pipeline failure)."""

    def __init__(self, exc):
        self.exc = exc
        self.runs = 0
        self.last_result = None

    def build_run(self):
        self.runs += 1
        raise self.exc


def test_build_run_pipeline_failure_raises_toolerror_and_burns_attempt():
    # A runner that raises TimeoutError (poll/subprocess timeout) -> the tool layer
    # surfaces a ToolError, the action/mutation are counted, and the attempt is
    # consumed (a real build was attempted).
    bridge = _FixBridge()
    runner = _RaisingRunner(TimeoutError("build did not finish"))
    layer = ToolLayer(bridge, runner_factory=lambda: runner)
    st = RequestState(request_id="r1")
    with pytest.raises(ToolError) as ei:
        layer.call("build_run", {}, st, runner=runner)
    assert "failed" in str(ei.value).lower()
    assert st.build_fix_attempts == 1
    assert st.action_count == 1
    assert st.mutation_count == 1


def test_build_run_config_error_does_not_burn_attempt():
    # A static misconfiguration (ConfigError) is NOT a build attempt: ToolError is
    # raised but NO action/mutation/attempt is counted (codex cannot fix it by
    # editing eps; 3 misconfigs must not exhaust the self-fix budget).
    from eud_agent.edd_runner import ConfigError

    bridge = _FixBridge()
    runner = _RaisingRunner(ConfigError("euddraft path not configured"))
    layer = ToolLayer(bridge, runner_factory=lambda: runner)
    st = RequestState(request_id="r1")
    for _ in range(BUILD_FIX_LIMIT + 2):
        with pytest.raises(ToolError) as ei:
            layer.call("build_run", {}, st, runner=runner)
        assert "misconfigured" in str(ei.value).lower()
    # Despite many calls, NO attempt was consumed and the exhausted flag is unset.
    assert st.build_fix_attempts == 0
    assert st.action_count == 0
    assert st.mutation_count == 0
    assert st.build_fix_exhausted is False
