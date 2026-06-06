"""euddraft build runner + error-retrieval ladder (features/05 "Build error
retrieval and self-fix").

``build_run`` drives one editor build to completion and returns a structured
result the ``build_errors`` tool can hand to codex for a self-fix:

  1. **BUILD** via the bridge (already hardened in EUD-040: SCArchive forced off +
     path preflight; the reply is ``OK: started``). Completion is observed by
     polling ``status.txt`` ``compiling=`` until it clears (timeout 300s,
     injectable for tests). The flag is parsed with :func:`engine.parse_status`
     -- the SAME parse the engine uses -- which lowercases the value, so VB.NET's
     ``compiling=True`` (capital T, the bridge's ToString) is handled correctly.

  2. **Error-retrieval ladder** (in order; the first rung that yields errors
     wins):

       (1) bridge ``BUILDERR`` -> ``macro.macroErrorList`` lines. Each non-empty
           line becomes a ``macro``-source :class:`BuildError`. If ANY macro error
           is present the ladder SHORT-CIRCUITS (no euddraft re-run).

       (2) Only when the build FAILED *with no macro errors* the runner re-runs
           ``euddraft.exe <eds>`` directly: the eds path + save-map path come from
           bridge ``EDSPATH``; the euddraft path from
           ``getset('program','euddraft')``. The spawn obeys rules.md's
           codex-invocation rules (they govern EVERY subprocess): a resolved
           ABSOLUTE exe path (never a bare name), an EXPLICIT stdin
           (``subprocess.DEVNULL`` -- euddraft reads nothing, but an inherited
           console-less stdin can hang a child), captured stdout/stderr, and
           ``cwd`` set (to the eds file's directory). The captured output is
           parsed with :func:`parse_euddraft_output`, which replicates BOTH of the
           editor's documented ``BuildErrorHandling`` regex formats.

  3. **Build success signal** = the output map (``SaveMapName`` from ``EDSPATH``)
     exists on disk AND is FRESH. The map's mtime (or absence) is snapshotted
     BEFORE the build; success requires it to exist afterwards AND be newly
     created OR have an advanced mtime, so a map left by a PREVIOUS build cannot
     mask a failed current build (mirrors the editor's own freshness tracking,
     ``Timer.vb`` ``LastOupputModifiyTimer``). When fresh AND there are no macro
     errors the build succeeded: the ladder stops, no euddraft re-run, no errors.

The last build's structured ladder result is retained on the runner
(``self.last_result``) so the ``build_errors`` tool returns the errors of the
most recent build (the tool layer routes through the runner -- see ``tools.py``).

euddraft error-format grounding (editor source, READ-ONLY)
----------------------------------------------------------
``EUD-Editor-3\\EUD Editor 3\\Module\\Tools\\BuildErrorHandling.vb``:

  * MODULE/LINE form -- ``BuildErrorHandling.vb:23``::

        \\[Error.*\\] Module "(.*)" Line (\\d+) : (.+)

    groups -> (module, line, message). The editor prefixes the message with
    "epScript 컴파일러 오류 : "; we keep the raw message (the prefix is a UI
    label, not part of the error).

  * PYTHON-TRACEBACK form (only when the module form matched ZERO lines --
    ``BuildErrorHandling.vb:40`` ``If mcol.Count = 0``):
      - the ``[Error]`` description regex -- line 42::

            \\[Error\\](.*)Traceback \\(most recent call last\\):

      - the file/line regex -- line 49::

            File "(.*)", line (\\d+), in ([\\w_]+)

      - the file path is reduced to its basename WITHOUT extension -- line 57::

            FilePath.Split("\\").Last.Split(".").First

    The editor's description loop overwrites on each match and ends on the LAST
    ``[Error]...Traceback`` match (vb:45-47); the file/line uses ``mcol(0)`` --
    the FIRST traceback frame (vb:54). We mirror both (one BuildError for the
    traceback form: last-match description, first-match file/line) with ONE
    deliberate divergence (EUD-088): the description regex runs with DOTALL
    (non-greedy) because eudplib emits the description across MULTIPLE lines
    before the Traceback marker — the editor's single-line regex drops the
    whole message in that case.

This module is stdlib-only and synchronous (the orchestrator runs ``build_run``
in a thread executor, like the bridge ``send``). ``spawn`` / ``read_status`` /
``path_exists`` / the timeouts are injectable so the whole pipeline is testable
without a real editor or euddraft.
"""

from __future__ import annotations

import re
import subprocess
import time
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path

from .engine import parse_status

# Default build-completion poll timeout (features/05 line 50: "timeout 300s").
DEFAULT_BUILD_TIMEOUT = 300.0
DEFAULT_POLL_INTERVAL = 0.5
# Wall-clock cap on the euddraft.exe re-run spawn. rules.md forbids any unbounded
# wait: a hung euddraft would block the runner thread forever. Defaults to the
# same 300s budget as the build poll; injectable for tests.
DEFAULT_SUBPROCESS_TIMEOUT = 300.0


class ConfigError(RuntimeError):
    """A STATIC misconfiguration the agent cannot fix by editing eps.

    Raised when the euddraft path is unset or the eds path is unavailable. These
    are operator/setup problems, NOT build failures — the tool layer re-raises
    them as a ToolError WITHOUT consuming a build self-fix attempt (3 retries of
    an unconfigured path would silently exhaust the budget). A real build failure
    or a poll/subprocess timeout still consumes an attempt.
    """


# --------------------------------------------------------------------------- #
# Editor BuildErrorHandling.vb regexes (replicated verbatim; see module docstring
# for the exact source line numbers). VB uses doubled quotes ("") inside a string
# literal -> a single literal double-quote in the pattern.
# --------------------------------------------------------------------------- #

# MODULE/LINE form (BuildErrorHandling.vb:23).
_MODULE_LINE_RE = re.compile(r'\[Error.*\] Module "(.*)" Line (\d+) : (.+)')
# PYTHON-TRACEBACK form (BuildErrorHandling.vb:42 + :49). The description regex
# DELIBERATELY diverges from the editor's single-line original: eudplib emits
# the [Error] description across MULTIPLE lines before "Traceback ..."
# (measured live, EUD-088), and the editor's regex then matches nothing — the
# whole human message is lost. DOTALL + non-greedy captures the full multi-line
# description up to the NEAREST following Traceback marker. The file/line regex
# still captures the FIRST traceback frame (editor mirror).
_TRACEBACK_DESC_RE = re.compile(
    r"\[Error\](.*?)Traceback \(most recent call last\):", re.DOTALL
)
_TRACEBACK_FILE_RE = re.compile(r'File "(.*)", line (\d+), in ([\w_]+)')


# --------------------------------------------------------------------------- #
# Result shapes consumed by the build_errors tool.
# --------------------------------------------------------------------------- #


@dataclass
class BuildError:
    """One structured build error.

    ``source`` is ``macro`` (from bridge BUILDERR) or ``euddraft`` (from the
    direct re-run parse). ``file``/``line`` may be ``""``/``0`` when the source
    line carried no location (a bare macro error). ``raw`` is the original text.
    """

    source: str  # "macro" | "euddraft"
    file: str
    line: int
    message: str
    raw: str

    def to_dict(self) -> dict:
        return {
            "source": self.source,
            "file": self.file,
            "line": self.line,
            "message": self.message,
            "raw": self.raw,
        }


@dataclass
class BuildRunResult:
    """The outcome of one ``build_run``: success flag + structured errors."""

    ok: bool
    errors: list[BuildError] = field(default_factory=list)

    def errors_as_dicts(self) -> list[dict]:
        return [e.to_dict() for e in self.errors]


# --------------------------------------------------------------------------- #
# euddraft output parser (BOTH documented formats).
# --------------------------------------------------------------------------- #


def parse_euddraft_output(stdout: str, stderr: str) -> list[BuildError]:
    """Parse captured euddraft output into structured :class:`BuildError`s.

    Replicates the editor's ``BuildErrorHandling.ErrorHandle`` logic: the
    MODULE/LINE form is tried first; the PYTHON-TRACEBACK form is used ONLY when
    the module form matched zero lines (the editor's ``If mcol.Count = 0``
    fallback). Both stdout and stderr are searched (euddraft writes errors to
    either depending on the failure path). Returns ``[]`` when nothing matches.
    """
    blob = (stdout or "") + "\n" + (stderr or "")

    module_errors: list[BuildError] = []
    for m in _MODULE_LINE_RE.finditer(blob):
        module = m.group(1).strip()
        line = int(m.group(2))
        message = m.group(3).strip()
        module_errors.append(
            BuildError(
                source="euddraft",
                file=module,
                line=line,
                message=message,
                raw=m.group(0).strip(),
            )
        )
    if module_errors:
        return module_errors

    # Fallback: the python-traceback form (editor only does this when the module
    # form matched zero lines). The editor's description loop OVERWRITES
    # ``Description`` on every match and ends on the LAST one
    # (BuildErrorHandling.vb:45-47), while the file/line uses ``mcol(0)`` -- the
    # FIRST traceback frame (vb:54-58). Mirror both: description = last match,
    # file/line = first match.
    desc_matches = list(_TRACEBACK_DESC_RE.finditer(blob))
    desc_m = desc_matches[-1] if desc_matches else None
    file_m = _TRACEBACK_FILE_RE.search(blob)
    if desc_m or file_m:
        description = desc_m.group(1).strip() if desc_m else ""
        file_name = ""
        line = 0
        if file_m:
            raw_path = file_m.group(1)
            # FilePath.Split("\").Last.Split(".").First (BuildErrorHandling.vb:57).
            file_name = raw_path.split("\\")[-1].split(".")[0]
            line = int(file_m.group(2))
        if description or file_name:
            return [
                BuildError(
                    source="euddraft",
                    file=file_name,
                    line=line,
                    message=description,
                    raw=(desc_m.group(0) if desc_m else file_m.group(0)).strip(),
                )
            ]
    return []


def parse_setting_value(reply: str) -> str:
    """Extract the value from a bridge ``GETSET`` reply (``OK: ... = <value>``).

    Only the FIRST ``" = "`` separates the id prefix from the value (a value may
    contain ``" = "``). Mirrors ``journal.parse_get_value`` -- duplicated tiny to
    avoid coupling the runner to the journal module.
    """
    _, sep, value = reply.partition(" = ")
    return (value if sep else reply).strip()


def parse_edspath(reply: str) -> tuple[str, str]:
    """Parse a bridge ``EDSPATH`` reply: line 1 = eds path, line 2 = SaveMapName."""
    lines = [ln.rstrip("\r") for ln in reply.splitlines()]
    eds = lines[0] if len(lines) > 0 else ""
    save_map = lines[1] if len(lines) > 1 else ""
    return eds.strip(), save_map.strip()


def parse_macro_errors(reply: str) -> list[BuildError]:
    """Turn a bridge ``BUILDERR`` reply (one error per line) into BuildErrors.

    An empty (non-``ERROR:``) reply means zero macro errors. Each line keeps its
    raw text; a macro line that happens to match the module/line form is also
    decomposed into file/line so the entry is as structured as the source allows.
    """
    errors: list[BuildError] = []
    for line in reply.splitlines():
        line = line.rstrip("\r").strip()
        if not line:
            continue
        m = _MODULE_LINE_RE.search(line)
        if m:
            errors.append(
                BuildError(
                    source="macro",
                    file=m.group(1).strip(),
                    line=int(m.group(2)),
                    message=m.group(3).strip(),
                    raw=line,
                )
            )
        else:
            errors.append(
                BuildError(source="macro", file="", line=0, message=line, raw=line)
            )
    return errors


# --------------------------------------------------------------------------- #
# The runner.
# --------------------------------------------------------------------------- #


class EddRunner:
    """Drives one editor build + the error-retrieval ladder.

    ``bridge`` is the shared :class:`bridge_io.BridgeIO`. The remaining args are
    injectable so the pipeline is testable without a real editor:

      * ``spawn`` -- callable ``(args, **kwargs) -> result`` with
        ``.stdout``/``.stderr``/``.returncode`` (defaults to ``subprocess.run``).
      * ``read_status`` -- returns the current ``status.txt`` text (defaults to
        reading ``<data_dir>/status.txt``; falls back to the bridge's status file).
      * ``path_exists`` -- ``(path) -> bool`` for the output-map success check
        (defaults to ``os.path.exists``).
      * ``path_mtime`` -- ``(path) -> float | None`` (the output map's mtime, or
        None when absent). Used for the STALE-ARTIFACT guard: success requires the
        map to exist AND be FRESH (newly created, or its mtime advanced past the
        pre-build snapshot) so a map left by a PREVIOUS build does not read a
        failed current build as ok. Mirrors the editor's own freshness tracking
        (``Timer.vb`` ``LastOupputModifiyTimer``). Defaults to
        ``os.path.getmtime`` (None on a missing file).
      * ``poll_interval`` / ``timeout`` -- the build-completion poll cadence and
        deadline (timeout default 300s; injectable for tests).
      * ``subprocess_timeout`` -- wall-clock cap on the euddraft.exe re-run
        (default 300s); a hung euddraft would otherwise block the runner thread
        forever (rules.md: never wait unbounded).

    ``last_result`` holds the most recent build's :class:`BuildRunResult` so the
    ``build_errors`` tool can return the last ladder result.

    LIVE-EDITOR RISK (flagged for the E2E, no behavior change here): the bridge
    runs ``EudplibData:Build`` SYNCHRONOUSLY inside the 1s DispatcherTimer Tick
    handler, and the ``status.txt`` write happens BEHIND the ``IsCompilng``
    early-return. So a server poll of ``status.txt`` may NEVER observe
    ``compiling=True`` — the whole build can begin and finish within one Tick
    while ``status.txt`` still reads the pre-build state. ``_poll_until_done`` may
    therefore return immediately (not-compiling on the first read). The ladder
    still resolves correctly afterwards (BUILDERR + the fresh-output-map check do
    not depend on having seen the compiling flag), but the "wait for the build to
    finish" guarantee is best-effort against this synchronous-Tick reality and
    must be confirmed in the live E2E.
    """

    def __init__(
        self,
        bridge,
        *,
        spawn: Callable | None = None,
        read_status: Callable[[], str] | None = None,
        path_exists: Callable[[str], bool] | None = None,
        path_mtime: Callable[[str], float | None] | None = None,
        poll_interval: float = DEFAULT_POLL_INTERVAL,
        timeout: float = DEFAULT_BUILD_TIMEOUT,
        subprocess_timeout: float = DEFAULT_SUBPROCESS_TIMEOUT,
    ) -> None:
        self._bridge = bridge
        self._spawn = spawn or subprocess.run
        self._read_status = read_status or self._default_read_status
        self._path_exists = path_exists or (lambda p: Path(p).exists())
        self._path_mtime = path_mtime or self._default_path_mtime
        self._poll_interval = poll_interval
        self._timeout = timeout
        self._subprocess_timeout = subprocess_timeout
        self.last_result: BuildRunResult | None = None

    @staticmethod
    def _default_path_mtime(path: str) -> float | None:
        """Output-map mtime, or None when the file is absent (best-effort)."""
        try:
            return Path(path).stat().st_mtime
        except OSError:
            return None

    def _default_read_status(self) -> str:
        """Read ``status.txt`` from the bridge's data dir (best-effort)."""
        try:
            return self._bridge.status_file.read_text(encoding="utf-8")
        except (OSError, AttributeError):
            return ""

    # ------------------------------------------------------------- pipeline
    def build_run(self) -> BuildRunResult:
        """Run one build to completion and resolve the error ladder.

        Raises :class:`TimeoutError` if ``status.txt`` never clears ``compiling``
        within ``timeout``; :class:`ConfigError` for a static misconfiguration
        (unset euddraft / eds path). Otherwise returns a :class:`BuildRunResult`
        and retains it on ``self.last_result``.

        STALE-ARTIFACT guard: the output map's mtime (or absence) is snapshotted
        BEFORE the build; success requires the map to exist AFTER the build AND be
        FRESH (it did not exist before, or its mtime advanced), so a map from a
        prior build cannot mask a failed current build.
        """
        # Snapshot output-map freshness BEFORE the build. SaveMapName is a static
        # project setting, available pre-build (EDSPATH returns eds + SaveMapName).
        eds_path, save_map = parse_edspath(self._bridge.edspath())
        before_mtime = self._path_mtime(save_map) if save_map else None

        self._bridge.build()
        self._poll_until_done()

        # Rung 1: macro errors short-circuit the ladder.
        macro_errors = parse_macro_errors(self._bridge.builderr())
        if macro_errors:
            result = BuildRunResult(ok=False, errors=macro_errors)
            self.last_result = result
            return result

        # No macro errors: success iff the output map exists AND is FRESH (new or
        # mtime advanced past the pre-build snapshot). A stale map from a previous
        # build (same mtime, same existence) is NOT success.
        if save_map and self._is_fresh_output(save_map, before_mtime):
            result = BuildRunResult(ok=True, errors=[])
            self.last_result = result
            return result

        # Rung 2: failed with no macro errors -> re-run euddraft directly.
        errors = self._rerun_euddraft(eds_path)
        result = BuildRunResult(ok=False, errors=errors)
        self.last_result = result
        return result

    def _is_fresh_output(self, save_map: str, before_mtime: float | None) -> bool:
        """True iff the output map exists AND is newer than the pre-build snapshot.

        ``before_mtime`` is the map's mtime BEFORE the build (None when it did not
        exist). After the build: a now-present map that was ABSENT before is fresh;
        a map present both times is fresh only if its mtime ADVANCED. This mirrors
        the editor's own output-freshness tracking (Timer.vb LastOupputModifiyTimer).
        """
        if not self._path_exists(save_map):
            return False
        after_mtime = self._path_mtime(save_map)
        if before_mtime is None:
            # Was absent, now present -> newly created -> fresh.
            return True
        if after_mtime is None:
            # Exists per path_exists but mtime unreadable -> cannot prove freshness.
            return False
        return after_mtime > before_mtime

    def _poll_until_done(self) -> None:
        """Poll ``status.txt`` ``compiling`` until false or the deadline lapses.

        The ``compiling`` flag is parsed with :func:`engine.parse_status` (the
        same parse the engine uses; it lowercases the value so VB's
        ``compiling=True`` is handled). A first poll already showing not-compiling
        returns immediately. On the deadline a :class:`TimeoutError` is raised
        (the .cmd-style "leave it" semantics do not apply here -- BUILD already
        returned ``OK: started``; this only waits for the flag to clear).
        """
        start = time.monotonic()
        while True:
            compiling, _ = parse_status(self._read_status())
            if not compiling:
                return
            if time.monotonic() - start >= self._timeout:
                raise TimeoutError(
                    f"build did not finish within {self._timeout:.0f}s "
                    "(status.txt still reports compiling=true)"
                )
            time.sleep(self._poll_interval)

    def _rerun_euddraft(self, eds_path: str) -> list[BuildError]:
        """Re-run ``euddraft.exe <eds>`` directly, parse stdout/stderr.

        rules.md codex-invocation rules govern this spawn (they apply to EVERY
        subprocess): a resolved ABSOLUTE exe path (never a bare name -> fail fast
        if unresolved), an EXPLICIT stdin (``subprocess.DEVNULL``), captured
        stdout/stderr, and ``cwd`` set to the eds file's directory.

        Hardening:
          * a wall-clock ``timeout`` caps a hung euddraft (rules.md: never wait
            unbounded); ``subprocess.TimeoutExpired`` -> :class:`TimeoutError`
            (the same failure family the ladder/tool layer already handles).
          * decode with ``encoding="utf-8", errors="replace"``: euddraft is a
            Python app and can emit UTF-8, while ``text=True`` alone would use the
            Korean-Windows OEM codepage (cp949) and could raise an uncaught
            ``UnicodeDecodeError`` past the tool layer's catch. ``errors="replace"``
            makes decode total.

        A static misconfiguration (unset euddraft / eds path) raises
        :class:`ConfigError` (NOT a build attempt — the tool layer re-raises it
        without consuming a self-fix attempt).
        """
        euddraft = parse_setting_value(
            self._bridge.getset("program", "euddraft")
        )
        if not euddraft:
            raise ConfigError(
                "euddraft path not configured (settings_get program euddraft "
                "returned empty); cannot re-run the build directly."
            )
        if not eds_path:
            raise ConfigError(
                "eds path not available (bridge EDSPATH returned empty); cannot "
                "re-run euddraft directly."
            )
        cwd = str(Path(eds_path).parent)
        try:
            proc = self._spawn(
                [euddraft, eds_path],
                stdin=subprocess.DEVNULL,
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                cwd=cwd,
                timeout=self._subprocess_timeout,
            )
        except subprocess.TimeoutExpired as exc:
            raise TimeoutError(
                f"euddraft.exe did not finish within "
                f"{self._subprocess_timeout:.0f}s (process killed)."
            ) from exc
        stdout = getattr(proc, "stdout", "") or ""
        stderr = getattr(proc, "stderr", "") or ""
        return parse_euddraft_output(stdout, stderr)
