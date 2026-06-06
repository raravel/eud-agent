r"""Verification artifact for EUD-052-3993: bridge BUILD hardening + BUILDERR +
EDSPATH (static).

Static source checks against ``bridge/ZZZ_10_agent_bridge.lua`` and
``server/eud_agent/bridge_io.py`` for the v2 Build surface
(hivemind/docs/features/04_bridge-v2-surface.md "Build (B4)" table +
"Verification contract"; capability-survey.md rows 20-21 + "Cross-cutting safety
facts"):

  - BUILD branch is MODIFIED IN PLACE (import-then-extend -- a single ``cmd ==
    "BUILD"`` branch, not a second one):
      * BEFORE the ``Build(`` call, ``pj.TEData.SCArchive.IsUsed`` is forced false
        (the defunct SCA service would block on a dead modal login -- rules.md SCA
        rule; capability-survey "Cross-cutting safety facts"). Asserted by
        ORDERING: the ``IsUsed`` = false index < the ``Build(`` call index inside
        the BUILD region.
      * PREFLIGHT existence of OpenMapName / SaveMapName / the euddraft path
        BEFORE invoking Build -- all three are referenced before the ``Build(``
        call, and a missing path returns ERROR WITHOUT calling Build (avoids the
        editor's modal CheckBuildable dialogs -- BulidMain.vb:155-200).
      * still calls ``pj.EudplibData:Build(false)`` (v6 path, unchanged signature
        -- BulidMain.vb:24 ``Build(Optional isEdd As Boolean = False)``).
  - BUILDERR branch walks ``GlobalObj.macro.macroErrorList`` (one line per entry;
    ``macro`` is a Public field of the GlobalObj module of type MacroManager --
    GlobalObj.vb:21; ``macroErrorList`` is a ``List(Of String)`` --
    MacroPluginManager.vb:25, iterated via ``.Count`` + ``:get_Item(i)``).
  - EDSPATH branch references the temp-eds-path source (the editor's
    ``BuildData.EdsFilePath`` Shared property -- BulidPaths.vb defines a
    ``Partial Public Class BuildData`` whose ``EdsFilePath`` returns the temp
    ``...\EUDEditor.eds``) AND ``SaveMapName``.
  - bridge_io wrappers ``build`` / ``builderr`` / ``edspath`` exist (build may
    surface BridgeBusy like other commands; builderr/edspath are simple reads),
    with behavioral tests over a FakeBridge.
  - v6 + B1 (DAT) + B2 (file-tree) + B3 (settings/plugins) command-marker
    regression survives import-then-extend.
  - Crash-rule lint: no ``os.execute`` / ``io.popen`` / ``:GetValue(`` anywhere;
    the new lua is pure ASCII, so the file's non-ASCII byte count must not grow
    above the verified baseline.

This file is pytest-compatible (plain ``test_*`` functions with asserts) AND
standalone-runnable with system Python::

    python server/tests/test_bridge_build_static.py

Only the stdlib is used (the project venv may be unavailable for the standalone
run; source-level checks need no third-party deps).

Failure profile before implementation (Step A): the implementation-pinning checks
FAIL -- the BUILD SCArchive-before-Build ordering / preflight checks, the BUILDERR
``macroErrorList`` walk, the EDSPATH branch + token checks, and the bridge_io
``build``/``builderr``/``edspath`` wrapper + behavioral checks. The checks that
PASS throughout are the file-presence guard, the v6 + B1 + B2 + B3 command-marker
regression, the forbidden-call lint, and the non-ASCII-baseline guard -- the same
"pass throughout" group idiom as test_bridge_plug_static.py. The
``_SETTABLE_FAMILIES`` SCA-exclusion guard is NOT duplicated here -- it lives in
test_bridge_list_static.py (EUD-048).
"""

from __future__ import annotations

import re
import sys
from pathlib import Path
from unittest import SkipTest as _SkipTest  # pytest treats this as a skip

# repo_root: server/tests/test_bridge_build_static.py -> parents[2] == repo root
REPO_ROOT = Path(__file__).resolve().parents[2]

BRIDGE = REPO_ROOT / "bridge" / "ZZZ_10_agent_bridge.lua"
BRIDGE_IO = REPO_ROOT / "server" / "eud_agent" / "bridge_io.py"

# New v2 Build dispatcher branches added/changed by this task.
NEW_COMMANDS = ("BUILDERR", "EDSPATH")

# v6 + B1 (DAT) + B2 (file-tree) + B3 (settings/plugins) command markers that must
# survive import-then-extend (each matched as a dispatcher branch). BUILD is in
# this set: it is MODIFIED in place, never removed nor duplicated.
V6_COMMANDS = (
    "PING",
    "STATUS",
    "LIST",
    "DUMP",
    "GET",
    "SET",
    "NEWEPS",
    "GETDAT",
    "SETDAT",
    "GETXDAT",
    "SETXDAT",
    "GETTBL",
    "SETTBL",
    "RESETDAT",
    "GETREQ",
    "SETREQ",
    "GETBTN",
    "SETBTN",
    "NEWFILE",
    "MKDIR",
    "RENAME",
    "DELFILE",
    "MOVEFILE",
    "SETMAIN",
    "GETMAIN",
    "GETSET",
    "SETSET",
    "PLUGLIST",
    "PLUGADD",
    "PLUGSET",
    "PLUGDEL",
    "PLUGMOVE",
    "PANEL",
    "BUILD",
    "LUA",
)

# bridge_io client wrappers (one method per new/changed command).
IO_WRAPPERS = ("build", "builderr", "edspath")

# Known non-ASCII byte count in the current (pre-implementation) bridge source
# (Korean mojibake in comments + WPF/error strings). The Build extension is
# ASCII-only, so this count must not increase. Baseline computed from the
# checked-in file at task start (main commit 60398c7 / EUD-051).
BASELINE_NONASCII_BYTES = 519


def _read_bridge() -> str:
    # latin-1 round-trips every byte 1:1, matching how KopiLua reads the source.
    return BRIDGE.read_bytes().decode("latin-1")


def _read_io() -> str:
    return BRIDGE_IO.read_text(encoding="utf-8")


def _branch_re(name: str) -> re.Pattern[str]:
    """Match a dispatcher branch comparing ``cmd`` to a command name."""
    return re.compile(r'cmd\s*==\s*"' + re.escape(name) + r'"')


def _branch_region(text: str, name: str) -> str:
    """Return the source from a ``cmd == "<name>"`` branch to the next branch.

    Bounds the region at the NEXT ``elseif cmd ==`` (or the unknown-command
    fallback) so token assertions are attributable to THIS command's body, the
    same region-extraction idiom as test_bridge_plug_static.py.
    """
    m = _branch_re(name).search(text)
    assert m, f'{name} branch missing (expected `cmd == "{name}"`)'
    region = text[m.start():]
    nxt = re.search(r'\n\s*elseif cmd ==', region[1:])
    if nxt:
        region = region[: nxt.start() + 1]
    return region


# --------------------------------------------------------------------------- #
# 0. File presence. (PASS throughout.)
# --------------------------------------------------------------------------- #


def test_bridge_file_present_and_nonempty():
    assert BRIDGE.is_file(), f"missing file: {BRIDGE}"
    assert BRIDGE.stat().st_size > 0, f"empty file: {BRIDGE}"


# --------------------------------------------------------------------------- #
# 1. New dispatcher branches present + BUILD stays a SINGLE branch.
#    (FAIL before impl for BUILDERR/EDSPATH; BUILD-single PASSES throughout.)
# --------------------------------------------------------------------------- #


def test_new_build_branches_present():
    """BUILDERR and EDSPATH are wired into the dispatcher."""
    text = _read_bridge()
    missing = [c for c in NEW_COMMANDS if not _branch_re(c).search(text)]
    assert not missing, f"missing new build command branches: {missing}"


def test_build_branch_is_single():
    """BUILD is MODIFIED in place -- exactly ONE ``cmd == "BUILD"`` branch.

    Import-then-extend: the v6 BUILD branch is changed, never duplicated (a second
    branch would be dead code after the first matches).
    """
    text = _read_bridge()
    count = len(_branch_re("BUILD").findall(text))
    assert count == 1, (
        f'expected exactly one `cmd == "BUILD"` branch (modified in place), '
        f"found {count}"
    )


# --------------------------------------------------------------------------- #
# 2. BUILD hardening: SCArchive.IsUsed forced false BEFORE Build + preflight.
#    (FAIL before impl.)
# --------------------------------------------------------------------------- #


def test_build_region_forces_scarchive_isused_false():
    """The BUILD region forces ``pj.TEData.SCArchive.IsUsed = false`` (SCA rule).

    capability-survey "Cross-cutting safety facts" + rules.md SCA rule: the
    defunct SCA service pops a dead login modal during Build when IsUsed is true;
    the bridge must clear it via the model before Build.
    """
    region = _branch_region(_read_bridge(), "BUILD")
    assert "SCArchive" in region, (
        "BUILD region must reference pj.TEData.SCArchive (SCA rule)"
    )
    assert "IsUsed" in region, (
        "BUILD region must set SCArchive.IsUsed = false before Build"
    )
    assert re.search(r"SCArchive\.IsUsed\s*=\s*false", region), (
        "BUILD must force SCArchive.IsUsed = false (literal `false`)"
    )


def test_build_region_isused_false_precedes_build_call():
    """ORDERING: ``SCArchive.IsUsed = false`` appears BEFORE the ``Build(`` call.

    Forcing IsUsed false must happen first so Build never reaches the SCA login
    modal path (BulidMain.vb:68-103).
    """
    region = _branch_region(_read_bridge(), "BUILD")
    m_isused = re.search(r"SCArchive\.IsUsed\s*=\s*false", region)
    m_build = re.search(r":Build\(", region)
    assert m_isused, "BUILD region missing `SCArchive.IsUsed = false`"
    assert m_build, "BUILD region missing the `:Build(` call"
    assert m_isused.start() < m_build.start(), (
        "SCArchive.IsUsed = false must come BEFORE the :Build( call "
        "(avoid the SCA login modal)"
    )


def test_build_region_preflights_map_and_euddraft_paths():
    """PREFLIGHT: OpenMapName / SaveMapName / euddraft path referenced before Build.

    CheckBuildable (BulidMain.vb:155-200) pops modal dialogs for a missing
    OpenMapName file, SaveMap directory, or euddraft exe. The bridge preflights
    those existence checks and returns ERROR WITHOUT calling Build so no modal can
    appear in the headless agent flow.
    """
    region = _branch_region(_read_bridge(), "BUILD")
    m_build = re.search(r":Build\(", region)
    assert m_build, "BUILD region missing the `:Build(` call"
    before_build = region[: m_build.start()]
    for token in ("OpenMapName", "SaveMapName", "euddraft"):
        assert token in before_build, (
            f"BUILD preflight must reference {token!r} BEFORE the Build call "
            "(avoid the modal CheckBuildable dialogs)"
        )
    # the preflight must be able to BAIL OUT (ERROR) before Build.
    assert "ERROR" in before_build, (
        "BUILD preflight must return ERROR (without invoking Build) when a "
        "required path is missing"
    )


def test_build_region_uses_existence_checks():
    """The preflight checks path EXISTENCE (File.Exists / Directory.Exists)."""
    region = _branch_region(_read_bridge(), "BUILD")
    assert ("File.Exists" in region) or ("Directory.Exists" in region), (
        "BUILD preflight must check path existence via File.Exists / "
        "Directory.Exists"
    )


def test_build_region_still_calls_build_false():
    """The BUILD region still invokes ``pj.EudplibData:Build(false)`` (v6 path)."""
    region = _branch_region(_read_bridge(), "BUILD")
    assert "EudplibData" in region, (
        "BUILD must call pj.EudplibData:Build(false) (v6 model path, unchanged)"
    )
    assert re.search(r":Build\(\s*false\s*\)", region), (
        "BUILD must call :Build(false) (Build signature isEdd=False, "
        "BulidMain.vb:24)"
    )


def test_build_region_returns_ok_started():
    """On success the BUILD region returns an ``OK: started``-style reply (B4)."""
    region = _branch_region(_read_bridge(), "BUILD")
    assert "started" in region, (
        "BUILD must return 'OK: started' on a successful Build invocation (B4)"
    )


# --------------------------------------------------------------------------- #
# 3. BUILDERR: walk GlobalObj.macro.macroErrorList. (FAIL before impl.)
# --------------------------------------------------------------------------- #


def test_builderr_region_walks_macro_error_list():
    """BUILDERR walks ``GlobalObj.macro.macroErrorList`` (B4 model path).

    ``macro`` is a Public field of the GlobalObj module (GlobalObj.vb:21) of type
    MacroManager; ``macroErrorList`` is a ``List(Of String)``
    (MacroPluginManager.vb:25).
    """
    region = _branch_region(_read_bridge(), "BUILDERR")
    assert "macro" in region, (
        "BUILDERR must reach GlobalObj.macro (the MacroManager field)"
    )
    assert "macroErrorList" in region, (
        "BUILDERR must walk macro.macroErrorList (B4 model path)"
    )


def test_builderr_region_iterates_the_list():
    """BUILDERR iterates the List(Of String) via .Count + :get_Item(i).

    rules.md: a List(Of T) default ``Item`` property is accessed as
    ``:get_Item(i)`` from KopiLua; the count via ``.Count``.
    """
    region = _branch_region(_read_bridge(), "BUILDERR")
    assert "Count" in region, "BUILDERR must read macroErrorList.Count"
    assert "get_Item" in region, (
        "BUILDERR must read each entry via macroErrorList:get_Item(i) "
        "(List(Of String) default Item -> get_Item in luanet)"
    )


# --------------------------------------------------------------------------- #
# 4. EDSPATH: BuildData.EdsFilePath + SaveMapName. (FAIL before impl.)
# --------------------------------------------------------------------------- #


def test_edspath_region_references_eds_path_source_and_savemap():
    """EDSPATH returns the temp eds path source + SaveMapName (B4 model path).

    The temp eds path is the editor's ``BuildData.EdsFilePath`` Shared property
    (BulidPaths.vb -- ``Partial Public Class BuildData``); SaveMapName is the
    output-map name (pjData.SaveMapName).
    """
    region = _branch_region(_read_bridge(), "EDSPATH")
    assert "EdsFilePath" in region, (
        "EDSPATH must reference BuildData.EdsFilePath (the temp .eds path source)"
    )
    assert "SaveMapName" in region, (
        "EDSPATH must reference pjData.SaveMapName (the output-map path)"
    )


def test_edspath_imports_builddata_type():
    """``BuildData`` is imported via luanet.import_type for its Shared eds path.

    EdsFilePath is a ``Public Shared ReadOnly Property`` on BuildData, so it is
    read off the imported TYPE proxy (rules.md: load_assembly before import_type;
    enum/type members come from import_type).
    """
    text = _read_bridge()
    assert re.search(
        r'import_type\(\s*"EUD_Editor_3\.BuildData"\s*\)', text
    ), (
        "BuildData must be imported via "
        'luanet.import_type("EUD_Editor_3.BuildData") for the Shared EdsFilePath'
    )


# --------------------------------------------------------------------------- #
# 5. v6 regression + crash-rule lint + non-ASCII baseline. (PASS throughout.)
# --------------------------------------------------------------------------- #


def test_v6_command_markers_present():
    """All v6 + B1 + B2 + B3 branches (incl. BUILD) survive import-then-extend."""
    text = _read_bridge()
    missing = [c for c in V6_COMMANDS if not _branch_re(c).search(text)]
    assert not missing, f"missing v6/B1/B2/B3 command branches: {missing}"


def test_no_forbidden_lua_calls():
    """Crash-rule lint: no os.execute / io.popen / :GetValue( anywhere."""
    text = _read_bridge()
    forbidden = [
        tok for tok in ("os.execute", "io.popen", ":GetValue(") if tok in text
    ]
    assert not forbidden, f"forbidden lua calls present: {forbidden}"


def test_extension_adds_no_raw_nonascii_bytes():
    """The Build extension is ASCII-only: total non-ASCII bytes must not grow
    above the pre-implementation baseline."""
    raw = BRIDGE.read_bytes()
    nonascii = sum(1 for b in raw if b > 0x7F)
    assert nonascii <= BASELINE_NONASCII_BYTES, (
        f"non-ASCII byte count grew to {nonascii} (baseline "
        f"{BASELINE_NONASCII_BYTES}); the Build extension must be ASCII-only"
    )


# --------------------------------------------------------------------------- #
# 6. bridge_io client wrappers (source-level). (FAIL before impl.)
# --------------------------------------------------------------------------- #


def test_bridge_io_defines_build_wrappers():
    """``bridge_io.py`` defines a client method per new/changed command."""
    src = _read_io()
    missing = [
        w for w in IO_WRAPPERS if not re.search(r"\bdef\s+" + w + r"\s*\(", src)
    ]
    assert not missing, f"bridge_io missing build wrapper methods: {missing}"


# --------------------------------------------------------------------------- #
# 7. Behavioral: the wrappers send the right command text over a FakeBridge.
# --------------------------------------------------------------------------- #

import tempfile  # noqa: E402  (kept local to the behavioral section)

_SERVER_DIR = str(REPO_ROOT / "server")
if _SERVER_DIR not in sys.path:
    sys.path.insert(0, _SERVER_DIR)


def _fresh_io():
    """A BridgeIO bound to a fresh empty temp data dir, plus the dir path."""
    from eud_agent.bridge_io import BridgeIO  # noqa: PLC0415 (import-on-use)

    data_dir = Path(tempfile.mkdtemp(prefix="build-"))
    return BridgeIO(str(data_dir)), data_dir


def _fake_bridge():
    try:
        from test_bridge_io import FakeBridge  # noqa: PLC0415 (import-only reuse)
    except Exception as exc:  # noqa: BLE001  (standalone: no pytest)
        raise _SkipTest(f"test_bridge_io unavailable: {exc}") from exc
    return FakeBridge


def test_build_sends_build_command():
    """``build()`` sends the ``BUILD`` command (no args)."""
    FakeBridge = _fake_bridge()
    bio, data_dir = _fresh_io()
    captured: dict[str, str] = {}

    def responder(first_line, body):
        captured["first_line"] = first_line
        captured["body"] = body
        return "OK: started"

    with FakeBridge(data_dir, responder):
        out = bio.build(timeout=3.0, poll_interval=0.02)

    assert captured["first_line"] == "BUILD", (
        f"build() must send 'BUILD' (no args), got {captured['first_line']!r}"
    )
    assert out == "OK: started"


def test_build_raises_bridge_error_on_error_reply():
    """``build()`` surfaces an ``ERROR:`` reply (failed preflight) as BridgeError."""
    from eud_agent.bridge_io import BridgeError  # noqa: PLC0415

    FakeBridge = _fake_bridge()
    bio, data_dir = _fresh_io()

    def responder(first_line, body):
        return "ERROR: OpenMapName missing"

    raised = False
    with FakeBridge(data_dir, responder):
        try:
            bio.build(timeout=3.0, poll_interval=0.02)
        except BridgeError:
            raised = True
    assert raised, "build() must raise BridgeError on an ERROR: reply"


def test_no_unparenthesized_tonumber_gsub():
    """EUD-087 regression: ``tonumber(string.gsub(...))`` crashes at runtime.

    ``string.gsub`` returns TWO values (string, substitution count); as the
    last argument of a call ALL results are passed, so the count lands in
    ``tonumber``'s second parameter (the BASE) -> "bad argument #2 to
    'tonumber' (base out of range)" (measured live on GETBTN). Every numeric
    trim must truncate to one value: ``tonumber((string.gsub(...)))``.
    """
    text = _read_bridge()
    bad = re.findall(r"tonumber\(string\.gsub", text)
    assert not bad, (
        f"{len(bad)} unparenthesized tonumber(string.gsub(...)) call(s): "
        "gsub's substitution count becomes tonumber's base -> runtime error. "
        "Wrap as tonumber((string.gsub(...)))."
    )


def test_builderr_sends_builderr_command_and_returns_reply():
    """``builderr()`` sends ``BUILDERR`` (no args) and returns the reply text."""
    FakeBridge = _fake_bridge()
    bio, data_dir = _fresh_io()
    captured: dict[str, str] = {}

    def responder(first_line, body):
        captured["first_line"] = first_line
        return "macro error 1\nmacro error 2"

    with FakeBridge(data_dir, responder):
        out = bio.builderr(timeout=3.0, poll_interval=0.02)

    assert captured["first_line"] == "BUILDERR", (
        f"builderr() must send 'BUILDERR' (no args), got "
        f"{captured['first_line']!r}"
    )
    assert out == "macro error 1\nmacro error 2"


def test_builderr_empty_reply_is_no_errors():
    """An EMPTY (non-ERROR) BUILDERR reply means zero macro errors, not a failure."""
    FakeBridge = _fake_bridge()
    bio, data_dir = _fresh_io()

    def responder(first_line, body):
        return ""

    with FakeBridge(data_dir, responder):
        out = bio.builderr(timeout=3.0, poll_interval=0.02)
    assert out == "", "an empty BUILDERR reply (no macro errors) must be returned"


def test_edspath_sends_edspath_command_and_returns_reply():
    """``edspath()`` sends ``EDSPATH`` (no args) and returns the reply text."""
    FakeBridge = _fake_bridge()
    bio, data_dir = _fresh_io()
    captured: dict[str, str] = {}

    def responder(first_line, body):
        captured["first_line"] = first_line
        return "C:/tmp/EUDEditor.eds\nC:/maps/out.scx"

    with FakeBridge(data_dir, responder):
        out = bio.edspath(timeout=3.0, poll_interval=0.02)

    assert captured["first_line"] == "EDSPATH", (
        f"edspath() must send 'EDSPATH' (no args), got {captured['first_line']!r}"
    )
    assert out == "C:/tmp/EUDEditor.eds\nC:/maps/out.scx"


# --------------------------------------------------------------------------- #
# Standalone runner (mirrors test_bridge_plug_static.py).
# --------------------------------------------------------------------------- #


def _all_test_functions():
    module = sys.modules[__name__]
    return [
        (name, obj)
        for name, obj in sorted(vars(module).items())
        if name.startswith("test_") and callable(obj)
    ]


def main() -> int:
    failures = 0
    skipped = 0
    funcs = _all_test_functions()
    for name, fn in funcs:
        try:
            fn()
        except _SkipTest as exc:
            skipped += 1
            print(f"SKIP {name}: {exc}")
        except AssertionError as exc:
            failures += 1
            print(f"FAIL {name}: {exc}")
        except Exception as exc:  # unexpected (e.g. missing file)
            failures += 1
            print(f"ERROR {name}: {type(exc).__name__}: {exc}")
        else:
            print(f"PASS {name}")
    total = len(funcs)
    passed = total - failures - skipped
    print(f"\n{passed}/{total} checks passed ({skipped} skipped, {failures} failed)")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
