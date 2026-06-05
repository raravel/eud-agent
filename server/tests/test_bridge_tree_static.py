"""Verification artifact for EUD-050-2bf4: bridge file-tree CRUD + SETMAIN (static).

Static source checks against ``bridge/ZZZ_10_agent_bridge.lua`` and
``server/eud_agent/bridge_io.py`` for the v2 File-tree surface
(hivemind/docs/features/04_bridge-v2-surface.md "File tree (B2)" table +
"Scope decisions" + "Verification contract"; capability-survey.md rows 9-16):

  - New dispatcher branches: NEWFILE, MKDIR, RENAME, DELFILE, MOVEFILE, SETMAIN,
    GETMAIN -- each wired as a ``cmd == "<NAME>"`` branch. NEWEPS stays present as
    a compat alias (import-then-extend: the v6 branch is NOT removed).
  - Guards, bound to the OWNING branch region (``_branch_region`` idiom):
      * NEWFILE: type whitelist {CUIEps, CUIPy, RawText}; duplicate-path ERROR;
        ``FolderAdd`` for auto-created parent folders; FileType pre-check ref.
      * MKDIR: duplicate ERROR; ``FolderAdd``.
      * RENAME: top-node + Setting-node guards; duplicate-sibling ERROR; the new
        name travels in the BODY (multi-line/non-ASCII per the B2 table).
      * DELFILE: top + Setting guards; MainFile dangling-clear (+ ``main-cleared``
        result note); open-tab close via ``TECloseTabITem``; ``FileRemove``;
        ``SetDirty``.
      * MOVEFILE: top + Setting guards; ``FileRemove`` (old parent) + ``FileAdd``
        (dest, same instance); preserves MainFile identity; destFolder in BODY.
      * SETMAIN: assigns ``MainFile`` from a WALKED node reference.
      * GETMAIN: returns the current main path or empty.
  - FileType pre-check: a structural guard that rejects the GUI/GUIPy/
    ClassicTrigger/SCAScript family BEFORE any ``StringText`` assignment
    (capability-survey row 16: those classes have no ``StringText`` member, so the
    assignment THROWS a .NET exception lua pcall cannot catch -- the FileType MUST
    be checked first). Pinned in the SET region AND the NEWFILE region, and the
    guard must name each rejected family token.
  - v6 regression: every existing dispatcher command survives import-then-extend,
    including the nine DAT-surface commands from EUD-049 (GETXDAT/SETXDAT/GETTBL/
    SETTBL/RESETDAT/GETREQ/SETREQ/GETBTN/SETBTN).
  - Crash-rule lint: no ``os.execute`` / ``io.popen`` / ``:GetValue(`` anywhere;
    the new lua is pure ASCII, so the file's non-ASCII byte count must not grow
    above the verified baseline.
  - bridge_io wrappers: client wrappers ``newfile/mkdir/rename/delfile/movefile/
    setmain/getmain`` exist; arg validation rejects an out-of-whitelist NEWFILE
    type and an empty path BEFORE send; RENAME's newname and MOVEFILE's destFolder
    travel in the BODY (B2: "Multi-line or non-ASCII values travel in the body").

This file is pytest-compatible (plain ``test_*`` functions with asserts) AND
standalone-runnable with system Python::

    python server/tests/test_bridge_tree_static.py

Only the stdlib is used (the project venv may be unavailable for the standalone
run; source-level checks need no third-party deps).

Failure profile before implementation (Step A): the implementation-pinning checks
FAIL -- the seven new-branch checks (NEWFILE/MKDIR/RENAME/DELFILE/MOVEFILE/
SETMAIN/GETMAIN branches absent), every region-bound guard check (the regions do
not exist yet), the FileType pre-check checks (no FileType guard in SET/NEWFILE),
the SETMAIN/GETMAIN model checks, and the bridge_io wrapper/validation/body-
placement checks (the seven wrappers are unimplemented). The checks that PASS
throughout are the file-presence guard, the NEWEPS-still-present alias check, the
v6-command-marker regression (incl. the EUD-049 DAT commands), the forbidden-call
lint, and the non-ASCII-baseline guard -- the same "pass throughout" group idiom
as test_bridge_datx_static.py. The ``_SETTABLE_FAMILIES`` SCA-exclusion guard is
NOT duplicated here -- it lives in test_bridge_list_static.py (EUD-048).
"""

from __future__ import annotations

import re
import sys
from pathlib import Path
from unittest import SkipTest as _SkipTest  # pytest treats this as a skip

# repo_root: server/tests/test_bridge_tree_static.py -> parents[2] == repo root
REPO_ROOT = Path(__file__).resolve().parents[2]

BRIDGE = REPO_ROOT / "bridge" / "ZZZ_10_agent_bridge.lua"
BRIDGE_IO = REPO_ROOT / "server" / "eud_agent" / "bridge_io.py"

# New v2 File-tree dispatcher branches.
NEW_COMMANDS = (
    "NEWFILE",
    "MKDIR",
    "RENAME",
    "DELFILE",
    "MOVEFILE",
    "SETMAIN",
    "GETMAIN",
)

# The creatable/settable text file types (B2 "Scope decisions": SCA is fully
# defunct; only the CUI text families + RawText). NEWFILE's whitelist + the
# FileType pre-check accept these.
CREATABLE_TYPES = ("CUIEps", "CUIPy", "RawText")

# The read-only / non-creatable file families the FileType pre-check must REJECT
# before any StringText assignment (capability-survey row 16: no StringText member
# -> assignment THROWS, uncatchable by lua pcall).
REJECTED_FAMILIES = ("GUI", "GUIPy", "ClassicTrigger", "SCAScript")

# v6 command markers that must survive import-then-extend (each matched as a
# dispatcher branch so a stray substring elsewhere cannot satisfy the check).
# Includes the nine DAT-surface commands added by EUD-049.
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
    "PANEL",
    "BUILD",
    "LUA",
)

# bridge_io client wrappers (one method per new file-tree command). ``mkdir`` is a
# safe, non-shadowing method name on the BridgeIO class (it shadows no stdlib
# member there); the rest mirror the existing lowercase wrapper naming style.
IO_WRAPPERS = (
    "newfile",
    "mkdir",
    "rename",
    "delfile",
    "movefile",
    "setmain",
    "getmain",
)

# Known non-ASCII byte count in the current (pre-implementation) bridge source
# (Korean mojibake in comments + WPF/error strings). The file-tree extension is
# ASCII-only, so this count must not increase. Baseline computed from the
# checked-in file at task start (base commit 20b4471).
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
    same region-extraction idiom as test_bridge_datx_static.py.
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
# 1. New dispatcher branches present; NEWEPS alias kept. (FAIL before impl.)
# --------------------------------------------------------------------------- #


def test_new_file_tree_branches_present():
    """All seven new file-tree commands are wired into the dispatcher."""
    text = _read_bridge()
    missing = [c for c in NEW_COMMANDS if not _branch_re(c).search(text)]
    assert not missing, f"missing new file-tree command branches: {missing}"


def test_neweps_alias_still_present():
    """NEWEPS stays as a compat alias (import-then-extend; B2: 'NEWEPS kept as
    alias for compat'). PASS throughout -- the v6 branch must not be removed.
    """
    assert _branch_re("NEWEPS").search(_read_bridge()), (
        "NEWEPS branch must remain (kept as a NEWFILE alias for compat)"
    )


# --------------------------------------------------------------------------- #
# 2. NEWFILE guards: type whitelist, duplicate ERROR, FolderAdd, FileType ref.
#    (FAIL before impl.)
# --------------------------------------------------------------------------- #


def test_newfile_region_has_type_whitelist():
    """The NEWFILE region names all three creatable types (the whitelist)."""
    region = _branch_region(_read_bridge(), "NEWFILE")
    missing = [t for t in CREATABLE_TYPES if t not in region]
    assert not missing, (
        f"NEWFILE region missing creatable type token(s): {missing} "
        "(whitelist = CUIEps / CUIPy / RawText; B2 'Scope decisions')"
    )


def test_newfile_region_rejects_duplicate_path():
    """NEWFILE returns ERROR on a duplicate path (Decision 02 generalized)."""
    region = _branch_region(_read_bridge(), "NEWFILE")
    assert "duplicate" in region, (
        "NEWFILE must return an ERROR for a duplicate path (B2: 'Duplicate path "
        "-> ERROR')"
    )


def test_newfile_region_auto_creates_folders_via_folderadd():
    """NEWFILE auto-creates missing parent folders via ``FolderAdd``."""
    region = _branch_region(_read_bridge(), "NEWFILE")
    assert "FolderAdd" in region, (
        "NEWFILE must auto-create missing folders via parent:FolderAdd "
        "(B2: 'missing folders auto-created via FolderAdd')"
    )


# --------------------------------------------------------------------------- #
# 3. MKDIR guards: duplicate ERROR, FolderAdd. (FAIL before impl.)
# --------------------------------------------------------------------------- #


def test_mkdir_region_uses_folderadd():
    region = _branch_region(_read_bridge(), "MKDIR")
    assert "FolderAdd" in region, (
        "MKDIR must create the folder node via FolderAdd (B2 model path)"
    )


def test_mkdir_region_rejects_duplicate():
    region = _branch_region(_read_bridge(), "MKDIR")
    assert "duplicate" in region, (
        "MKDIR must return ERROR on a duplicate folder path (B2: 'duplicate -> ERROR')"
    )


# --------------------------------------------------------------------------- #
# 4. RENAME guards: top/Setting/duplicate; newname in BODY. (FAIL before impl.)
# --------------------------------------------------------------------------- #


def test_rename_region_guards_top_and_setting_and_duplicate():
    """RENAME rejects the top node, the Setting node, and a duplicate sibling."""
    region = _branch_region(_read_bridge(), "RENAME")
    assert "Setting" in region, (
        "RENAME must reject the Setting node (B2: 'rejects top node, Setting node')"
    )
    assert "duplicate" in region, (
        "RENAME must reject a duplicate sibling name (B2: 'duplicate sibling name')"
    )


def test_rename_takes_newname_from_body():
    """RENAME's new name travels in the BODY (B2: 'newname in body'; the B2 table
    note: 'Multi-line or non-ASCII values travel in the body').
    """
    region = _branch_region(_read_bridge(), "RENAME")
    assert "body" in region, (
        "RENAME must read the new name from the command BODY (UTF-8/Korean-safe), "
        "never the pipe-separated arg line"
    )


# --------------------------------------------------------------------------- #
# 5. DELFILE guards: top/Setting, MainFile dangling-clear + note, tab close,
#    FileRemove, SetDirty. (FAIL before impl.)
# --------------------------------------------------------------------------- #


def test_delfile_region_guards_top_and_setting():
    region = _branch_region(_read_bridge(), "DELFILE")
    assert "Setting" in region, (
        "DELFILE must reject the top/Setting nodes (B2: 'rejects top/Setting nodes')"
    )


def test_delfile_region_clears_dangling_mainfile_with_note():
    """DELFILE clears MainFile if the target IS main, and notes ``main-cleared``."""
    region = _branch_region(_read_bridge(), "DELFILE")
    assert "MainFile" in region, (
        "DELFILE must clear a dangling MainFile when the target is the main file "
        "(B2: 'if target IS MainFile, clears MainFile first')"
    )
    assert "main-cleared" in region, (
        "DELFILE result must note 'main-cleared' when it cleared the MainFile"
    )


def test_delfile_region_closes_open_tab():
    region = _branch_region(_read_bridge(), "DELFILE")
    assert "TECloseTabITem" in region, (
        "DELFILE must close an open tab via WindowControl.TECloseTabITem "
        "(B2: 'closes an open tab via WindowControl.TECloseTabITem')"
    )


def test_delfile_region_uses_fileremove_and_setdirty():
    region = _branch_region(_read_bridge(), "DELFILE")
    assert "FileRemove" in region, (
        "DELFILE must remove the node via parent:FileRemove (B2 model path)"
    )
    assert "SetDirty" in region, (
        "DELFILE must mark the project dirty via SetDirty "
        "(B2: '+ pjData:SetDirty(true)')"
    )


# --------------------------------------------------------------------------- #
# 6. MOVEFILE guards: top/Setting, FileRemove + FileAdd, destFolder in BODY.
#    (FAIL before impl.)
# --------------------------------------------------------------------------- #


def test_movefile_region_guards_setting():
    region = _branch_region(_read_bridge(), "MOVEFILE")
    assert "Setting" in region, (
        "MOVEFILE must reject a move into the Setting/top node "
        "(B2: 'rejects move into Setting/top')"
    )


def test_movefile_region_uses_fileremove_and_fileadd():
    """MOVEFILE re-adds the SAME instance: ``FileRemove`` then ``FileAdd``."""
    region = _branch_region(_read_bridge(), "MOVEFILE")
    assert "FileRemove" in region, (
        "MOVEFILE must remove from the old parent via FileRemove (B2 model path)"
    )
    assert "FileAdd" in region, (
        "MOVEFILE must re-add to the dest folder via FileAdd (same instance, "
        "preserving MainFile identity)"
    )


def test_movefile_takes_destfolder_from_body():
    """MOVEFILE's destFolder travels in the BODY (B2: 'destFolder in body')."""
    region = _branch_region(_read_bridge(), "MOVEFILE")
    assert "body" in region, (
        "MOVEFILE must read destFolder from the command BODY (B2 table note: "
        "'Multi-line or non-ASCII values travel in the body')"
    )


# --------------------------------------------------------------------------- #
# 7. SETMAIN / GETMAIN. (FAIL before impl.)
# --------------------------------------------------------------------------- #


def test_setmain_region_assigns_mainfile_from_walked_node():
    """SETMAIN assigns ``MainFile`` from a node located via the file walk.

    B2 + capability-survey row 13: ``pj.TEData.MainFile = <node>`` where <node>
    is the TEFile found by path (the bridge's ``findFile`` walk). Pin both the
    ``MainFile`` assignment AND the ``findFile`` lookup in the SETMAIN region so a
    regression that assigns a raw path string (instead of the node ref) is caught.
    """
    region = _branch_region(_read_bridge(), "SETMAIN")
    assert re.search(r"MainFile\s*=", region), (
        "SETMAIN must assign pj.TEData.MainFile = <node>"
    )
    assert "findFile" in region, (
        "SETMAIN must resolve the target to a WALKED node ref (findFile) before "
        "assigning MainFile -- not a raw path string"
    )


def test_getmain_region_returns_main_path_or_empty():
    """GETMAIN returns the current main path, or empty when none is set."""
    region = _branch_region(_read_bridge(), "GETMAIN")
    assert "MainFile" in region, (
        "GETMAIN must read pj.TEData.MainFile "
        "(B2: 'returns current main path or empty')"
    )


# --------------------------------------------------------------------------- #
# 8. FileType pre-check rejecting GUI/GUIPy/ClassicTrigger/SCAScript BEFORE any
#    StringText assignment, referenced from SET and NEWFILE. (FAIL before impl.)
# --------------------------------------------------------------------------- #


def test_filetype_precheck_helper_rejects_unsettable_families():
    """A structural FileType guard names every rejected (non-StringText) family.

    capability-survey row 16: GUI/GUIPy/ClassicTrigger have no ``StringText``
    member, so assignment THROWS (uncatchable by lua pcall). SCAScript is defunct
    (B2 'Scope decisions'). The bridge must structurally reject these families. We
    pin a dedicated helper (``isSettableType`` / a guard function) that names every
    rejected family token, so the check is BEFORE-assignment by construction.
    """
    text = _read_bridge()
    m = re.search(r"local function isSettableType\(", text)
    assert m, (
        "expected a FileType pre-check helper `local function isSettableType(...)` "
        "(structural guard checked BEFORE any StringText assignment)"
    )
    helper = text[m.start():]
    nxt = re.search(r"\n\s*local function ", helper[1:])
    if nxt:
        helper = helper[: nxt.start() + 1]
    missing = [fam for fam in REJECTED_FAMILIES if fam not in helper]
    assert not missing, (
        f"FileType pre-check helper must reject family token(s): {missing} "
        "(GUI/GUIPy/ClassicTrigger have no StringText member; SCAScript is defunct)"
    )


def test_set_region_references_filetype_precheck():
    """The SET region runs the FileType pre-check BEFORE the StringText assignment.

    Pin that ``isSettableType`` is referenced in the SET region AND that the
    reference precedes the first ``StringText =`` assignment (the throw-avoidance
    ordering capability-survey row 16 demands).
    """
    region = _branch_region(_read_bridge(), "SET")
    guard = region.find("isSettableType")
    assert guard >= 0, (
        "SET must call the FileType pre-check (isSettableType) before assigning "
        "StringText (capability-survey row 16: assignment THROWS on GUI/Classic)"
    )
    assign = region.find("StringText")
    assert assign >= 0, "SET region must assign StringText (sanity)"
    assert guard < assign, (
        "the FileType pre-check must come BEFORE the StringText assignment in SET"
    )


def test_newfile_region_references_filetype_precheck():
    """NEWFILE also runs the FileType pre-check (reject non-creatable types)."""
    region = _branch_region(_read_bridge(), "NEWFILE")
    assert "isSettableType" in region, (
        "NEWFILE must reference the FileType pre-check (isSettableType) so only "
        "settable/creatable types are created"
    )


# --------------------------------------------------------------------------- #
# 9. v6 regression + crash-rule lint + non-ASCII baseline. (PASS throughout.)
# --------------------------------------------------------------------------- #


def test_v6_command_markers_present():
    """All v6 + EUD-049 DAT dispatcher commands survive (import-then-extend)."""
    text = _read_bridge()
    missing = [c for c in V6_COMMANDS if not _branch_re(c).search(text)]
    assert not missing, f"missing v6/DAT command branches: {missing}"


def test_no_forbidden_lua_calls():
    """Crash-rule lint: no os.execute / io.popen / :GetValue( anywhere."""
    text = _read_bridge()
    forbidden = [
        tok for tok in ("os.execute", "io.popen", ":GetValue(") if tok in text
    ]
    assert not forbidden, f"forbidden lua calls present: {forbidden}"


def test_extension_adds_no_raw_nonascii_bytes():
    """The file-tree extension is ASCII-only: total non-ASCII bytes must not grow
    above the pre-implementation baseline.

    The current source already carries Korean mojibake; the new lua and all new
    result strings are pure ASCII, so the count must stay <= the baseline.
    """
    raw = BRIDGE.read_bytes()
    nonascii = sum(1 for b in raw if b > 0x7F)
    assert nonascii <= BASELINE_NONASCII_BYTES, (
        f"non-ASCII byte count grew to {nonascii} (baseline "
        f"{BASELINE_NONASCII_BYTES}); the file-tree extension must be ASCII-only"
    )


# --------------------------------------------------------------------------- #
# 10. bridge_io client wrappers (source-level). (FAIL before impl.)
# --------------------------------------------------------------------------- #


def test_bridge_io_defines_file_tree_wrappers():
    """``bridge_io.py`` defines a client method per file-tree command.

    Source-level check (stdlib-only, standalone-safe): each wrapper is a
    ``def <name>(`` on the BridgeIO class. Matches the established source-check
    convention (test_bridge_datx_static.test_bridge_io_defines_dat_surface_wrappers).
    """
    src = _read_io()
    missing = [
        w for w in IO_WRAPPERS if not re.search(r"\bdef\s+" + w + r"\s*\(", src)
    ]
    assert not missing, f"bridge_io missing file-tree wrapper methods: {missing}"


def test_bridge_io_validates_newfile_types():
    """bridge_io carries the NEWFILE type whitelist (CUIEps/CUIPy/RawText).

    Their presence as string literals in the module source is a robust,
    location-agnostic signal of the creation-type validation helper (the SCA-
    family / GUI types are absent from the creatable set -- B2 'Scope decisions').
    """
    src = _read_io()
    missing = [
        t for t in CREATABLE_TYPES
        if ('"' + t + '"') not in src and ("'" + t + "'") not in src
    ]
    assert not missing, (
        f"bridge_io NEWFILE type validation missing: {missing} "
        "(the creatable whitelist must include CUIEps/CUIPy/RawText)"
    )


# --------------------------------------------------------------------------- #
# 11. Behavioral: validation helpers reject BEFORE any .cmd is written, and the
# body-placement contract (RENAME newname / MOVEFILE destFolder in the body).
#
# These import eud_agent.bridge_io by injecting ``REPO_ROOT / "server"`` onto
# sys.path. bridge_io is stdlib-only, so the import stays standalone-safe (no
# venv / third-party deps required). A BridgeIO bound to an empty tmp dir lets us
# assert that an invalid argument raises BEFORE send() touches the filesystem:
# no ``srv-*.cmd`` may appear in the inbox.
# --------------------------------------------------------------------------- #

import tempfile  # noqa: E402  (kept local to the behavioral section)

_SERVER_DIR = str(REPO_ROOT / "server")
if _SERVER_DIR not in sys.path:
    sys.path.insert(0, _SERVER_DIR)


def _fresh_io():
    """A BridgeIO bound to a fresh empty temp data dir, plus the dir path.

    The caller asserts the inbox stays empty after an expected validation raise.
    """
    from eud_agent.bridge_io import BridgeIO  # noqa: PLC0415 (import-on-use)

    data_dir = Path(tempfile.mkdtemp(prefix="tree-"))
    return BridgeIO(str(data_dir)), data_dir


def _inbox_cmds(data_dir: Path):
    inbox = data_dir / "inbox"
    return list(inbox.glob("srv-*.cmd")) if inbox.is_dir() else []


def _assert_rejected_before_send(call, data_dir):
    """The call must raise (validation) and write NO ``.cmd`` to the inbox."""
    from eud_agent.bridge_io import BridgeError  # noqa: PLC0415

    raised = False
    try:
        call()
    except (BridgeError, ValueError):
        raised = True
    except Exception as exc:  # noqa: BLE001
        raise AssertionError(
            f"expected a validation error (BridgeError/ValueError), got "
            f"{type(exc).__name__}: {exc}"
        ) from exc
    assert raised, "invalid argument was NOT rejected"
    assert _inbox_cmds(data_dir) == [], (
        "validation must reject BEFORE writing a .cmd to the inbox"
    )


def test_newfile_rejects_unknown_type():
    """NEWFILE rejects a type outside {CUIEps, CUIPy, RawText} before send."""
    bio, data_dir = _fresh_io()
    _assert_rejected_before_send(
        lambda: bio.newfile(
            "folder/x", "GUIEps", "code", timeout=0.2, poll_interval=0.02
        ),
        data_dir,
    )


def test_newfile_rejects_empty_path():
    """NEWFILE rejects an empty path before send."""
    bio, data_dir = _fresh_io()
    _assert_rejected_before_send(
        lambda: bio.newfile("", "CUIEps", "code", timeout=0.2, poll_interval=0.02),
        data_dir,
    )


def test_mkdir_rejects_empty_path():
    bio, data_dir = _fresh_io()
    _assert_rejected_before_send(
        lambda: bio.mkdir("", timeout=0.2, poll_interval=0.02),
        data_dir,
    )


def test_rename_rejects_empty_path():
    bio, data_dir = _fresh_io()
    _assert_rejected_before_send(
        lambda: bio.rename("", "newname", timeout=0.2, poll_interval=0.02),
        data_dir,
    )


def test_setmain_rejects_empty_path():
    bio, data_dir = _fresh_io()
    _assert_rejected_before_send(
        lambda: bio.setmain("", timeout=0.2, poll_interval=0.02),
        data_dir,
    )


def test_rename_newname_travels_in_body():
    """RENAME sends ``RENAME <path>`` on the arg line and the newname in the BODY.

    B2 table note: 'Multi-line or non-ASCII values travel in the body'. We reuse
    the FakeBridge from test_bridge_io (import-only) so the round-trip completes
    without a real editor. The standalone runner SKIPS this when pytest/
    test_bridge_io is not importable (it pulls in pytest); under pytest the import
    always works.
    """
    try:
        from test_bridge_io import FakeBridge  # noqa: PLC0415  (import-only reuse)
    except Exception as exc:  # noqa: BLE001  (standalone: no pytest)
        raise _SkipTest(f"test_bridge_io unavailable: {exc}") from exc

    bio, data_dir = _fresh_io()
    captured: dict[str, str] = {}

    def responder(first_line, body):
        captured["first_line"] = first_line
        captured["body"] = body
        return "OK: renamed"

    with FakeBridge(data_dir, responder):
        bio.rename("dir/old", "newname", timeout=3.0, poll_interval=0.02)

    assert captured["first_line"] == "RENAME dir/old", (
        f"RENAME arg line must be 'RENAME <path>', got {captured['first_line']!r}"
    )
    assert captured["body"] == "newname", (
        f"RENAME newname must travel in the BODY, got body={captured['body']!r}"
    )


def test_movefile_destfolder_travels_in_body():
    """MOVEFILE sends ``MOVEFILE <path>`` on the arg line and destFolder in BODY."""
    try:
        from test_bridge_io import FakeBridge  # noqa: PLC0415  (import-only reuse)
    except Exception as exc:  # noqa: BLE001  (standalone: no pytest)
        raise _SkipTest(f"test_bridge_io unavailable: {exc}") from exc

    bio, data_dir = _fresh_io()
    captured: dict[str, str] = {}

    def responder(first_line, body):
        captured["first_line"] = first_line
        captured["body"] = body
        return "OK: moved"

    with FakeBridge(data_dir, responder):
        bio.movefile("dir/x", "other/dest", timeout=3.0, poll_interval=0.02)

    assert captured["first_line"] == "MOVEFILE dir/x", (
        f"MOVEFILE arg line must be 'MOVEFILE <path>', got {captured['first_line']!r}"
    )
    assert captured["body"] == "other/dest", (
        f"MOVEFILE destFolder must travel in the BODY, got body={captured['body']!r}"
    )


# --------------------------------------------------------------------------- #
# Standalone runner (mirrors test_bridge_datx_static.py).
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
