"""Verification artifact for EUD-051-6657: bridge settings + plugin commands (static).

Static source checks against ``bridge/ZZZ_10_agent_bridge.lua`` and
``server/eud_agent/bridge_io.py`` for the v2 Settings & plugins surface
(hivemind/docs/features/04_bridge-v2-surface.md "Settings & plugins (B3)" table +
"Verification contract"; capability-survey.md rows 17-19 + "Cross-cutting safety
facts"):

  - New dispatcher branches: GETSET, SETSET, PLUGLIST, PLUGADD, PLUGSET,
    PLUGDEL, PLUGMOVE -- each wired as a ``cmd == "<NAME>"`` branch.
  - GETSET/SETSET scope+key whitelists, REGION-BOUND (``_branch_region`` idiom):
      * scope ``project`` -> plain ``pjData`` props, whitelist OpenMapName/
        SaveMapName/AutoBuild/UseCustomtbl/ViewLog/TempFileLoc.
      * scope ``program`` -> ``pgData:get_Setting/set_Setting(TSetting enum)``,
        whitelist euddraft/starcraft (read/write), Language (READ-ONLY: SETSET on
        it returns ERROR).
      * any OTHER scope/key -> ERROR (no theme/UX chrome leaks through).
      * ``TSetting`` is IMPORTED (luanet.import_type) and used as an enum OBJECT
        in the program-scope path (never an int/string).
      * ``SaveSetting`` is flushed after a program-scope write (SETSET region).
  - PLUGLIST walks ``pjData.EdsBlock.Blocks`` -> one line per block
    ``index TAB BType TAB first-line-of-Texts`` (the Texts FIRST line only).
  - PLUGADD: ``EdsBlockItem(EdsBlockType.UserPlugin)`` + ``.Texts = body`` +
    ``Blocks:Insert(index, item)``; index=-1 appends; ``SetDirty`` after; Texts
    travel in the BODY (the PLUGADD region references ``body``).
  - PLUGSET: ``Blocks:get_Item(i).Texts = body`` -- UserPlugin guard (else ERROR);
    ``SetDirty``; Texts from the BODY.
  - PLUGDEL: ``Blocks:RemoveAt(i)`` -- UserPlugin guard (else ERROR); ``SetDirty``.
  - PLUGMOVE: RemoveAt + Insert reorder; ``SetDirty``.
  - v6 regression: every existing dispatcher command survives import-then-extend,
    including the nine DAT-surface commands (EUD-049) and the seven file-tree
    commands (EUD-050).
  - Crash-rule lint: no ``os.execute`` / ``io.popen`` / ``:GetValue(`` anywhere;
    the new lua is pure ASCII, so the file's non-ASCII byte count must not grow
    above the verified baseline.
  - bridge_io wrappers: client wrappers ``getset/setset/pluglist/plugadd/plugset/
    plugdel/plugmove`` exist; whitelist mirrors of the bridge (project keys,
    program keys, Language read-only); behavioral validation rejects (BEFORE send)
    a bad scope, a bad key, a Language SETSET write, and a bad plugin index
    (plugadd allows -1; the others require non-negative).

This file is pytest-compatible (plain ``test_*`` functions with asserts) AND
standalone-runnable with system Python::

    python server/tests/test_bridge_plug_static.py

Only the stdlib is used (the project venv may be unavailable for the standalone
run; source-level checks need no third-party deps).

Failure profile before implementation (Step A): the implementation-pinning checks
FAIL -- the seven new-branch checks, the region-bound whitelist / Language read-
only / TSetting-import / SaveSetting checks, the PLUGLIST/PLUGADD/PLUGSET/PLUGDEL/
PLUGMOVE model + UserPlugin-guard + SetDirty + body checks, and the bridge_io
wrapper / whitelist-mirror / behavioral-validation checks. The checks that PASS
throughout are the file-presence guard, the v6 + EUD-049 + EUD-050 command-marker
regression, the forbidden-call lint, and the non-ASCII-baseline guard -- the same
"pass throughout" group idiom as test_bridge_tree_static.py. The
``_SETTABLE_FAMILIES`` SCA-exclusion guard is NOT duplicated here -- it lives in
test_bridge_list_static.py (EUD-048).
"""

from __future__ import annotations

import re
import sys
from pathlib import Path
from unittest import SkipTest as _SkipTest  # pytest treats this as a skip

# repo_root: server/tests/test_bridge_plug_static.py -> parents[2] == repo root
REPO_ROOT = Path(__file__).resolve().parents[2]

BRIDGE = REPO_ROOT / "bridge" / "ZZZ_10_agent_bridge.lua"
BRIDGE_IO = REPO_ROOT / "server" / "eud_agent" / "bridge_io.py"

# New v2 Settings & plugins dispatcher branches.
NEW_COMMANDS = (
    "GETSET",
    "SETSET",
    "PLUGLIST",
    "PLUGADD",
    "PLUGSET",
    "PLUGDEL",
    "PLUGMOVE",
)

# Project-scope key whitelist (plain pjData properties; B3 table).
PROJECT_KEYS = (
    "OpenMapName",
    "SaveMapName",
    "AutoBuild",
    "UseCustomtbl",
    "ViewLog",
    "TempFileLoc",
)
# Program-scope key whitelist (TSetting enum members; B3 table). Language is
# READ-ONLY (SETSET on it returns ERROR).
PROGRAM_KEYS = ("euddraft", "starcraft", "Language")

# v6 + EUD-049 (DAT) + EUD-050 (file-tree) command markers that must survive
# import-then-extend (each matched as a dispatcher branch).
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
    "PANEL",
    "BUILD",
    "LUA",
)

# bridge_io client wrappers (one method per new command).
IO_WRAPPERS = (
    "getset",
    "setset",
    "pluglist",
    "plugadd",
    "plugset",
    "plugdel",
    "plugmove",
)

# Known non-ASCII byte count in the current (pre-implementation) bridge source
# (Korean mojibake in comments + WPF/error strings). The settings/plugins
# extension is ASCII-only, so this count must not increase. Baseline computed from
# the checked-in file at task start (base commit bc1ba8e / EUD-042).
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
    same region-extraction idiom as test_bridge_tree_static.py.
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
# 1. New dispatcher branches present. (FAIL before impl.)
# --------------------------------------------------------------------------- #


def test_new_settings_plugin_branches_present():
    """All seven new settings/plugin commands are wired into the dispatcher."""
    text = _read_bridge()
    missing = [c for c in NEW_COMMANDS if not _branch_re(c).search(text)]
    assert not missing, f"missing new settings/plugin command branches: {missing}"


# --------------------------------------------------------------------------- #
# 2. GETSET/SETSET whitelists + Language read-only + TSetting + SaveSetting.
#    (FAIL before impl.)
# --------------------------------------------------------------------------- #


def test_getset_region_has_project_and_program_keys():
    """The GETSET region names every whitelisted project + program key.

    The project keys (plain pjData props) and the program keys (TSetting members)
    are both readable, so both whitelists must be present in the GETSET region.
    """
    region = _branch_region(_read_bridge(), "GETSET")
    missing_proj = [k for k in PROJECT_KEYS if k not in region]
    assert not missing_proj, (
        f"GETSET region missing project key(s): {missing_proj} "
        "(whitelist OpenMapName/SaveMapName/AutoBuild/UseCustomtbl/ViewLog/"
        "TempFileLoc)"
    )
    missing_prog = [k for k in PROGRAM_KEYS if k not in region]
    assert not missing_prog, (
        f"GETSET region missing program key(s): {missing_prog} "
        "(whitelist euddraft/starcraft/Language)"
    )


def test_setset_region_has_project_and_writable_program_keys():
    """The SETSET region names the project keys + the WRITABLE program keys.

    euddraft and starcraft are writable; Language is read-only (SETSET on it is an
    ERROR) but must still be NAMED in the region as the rejected/read-only case.
    """
    region = _branch_region(_read_bridge(), "SETSET")
    missing_proj = [k for k in PROJECT_KEYS if k not in region]
    assert not missing_proj, (
        f"SETSET region missing project key(s): {missing_proj}"
    )
    for k in ("euddraft", "starcraft"):
        assert k in region, f"SETSET region missing writable program key {k!r}"


def test_setset_region_language_is_read_only():
    """SETSET on the Language key returns ERROR (read-only, B3 table)."""
    region = _branch_region(_read_bridge(), "SETSET")
    assert "Language" in region, (
        "SETSET region must name the Language key as the read-only case"
    )
    # the read-only rejection must be an ERROR result bound to this region.
    assert "ERROR" in region, (
        "SETSET region must return an ERROR (Language read-only / bad scope-key)"
    )


def test_setset_region_rejects_unknown_scope_key():
    """SETSET returns ERROR for a non-whitelisted scope/key (no chrome leak)."""
    region = _branch_region(_read_bridge(), "SETSET")
    assert "ERROR" in region, (
        "SETSET must return ERROR for an out-of-whitelist scope/key "
        "(B3: 'Anything else -> ERROR (no theme/UX chrome)')"
    )


def test_getset_region_rejects_unknown_scope_key():
    """GETSET returns ERROR for a non-whitelisted scope/key."""
    region = _branch_region(_read_bridge(), "GETSET")
    assert "ERROR" in region, (
        "GETSET must return ERROR for an out-of-whitelist scope/key"
    )


def test_tsetting_enum_imported_and_used_in_program_path():
    """``TSetting`` is imported via luanet.import_type and used as an enum OBJECT.

    capability-survey + rules.md: enum args ALWAYS as imported objects, never
    ints/strings. The bridge must import ``...ProgramData+TSetting`` and reference
    it (the program-scope GETSET/SETSET path passes ``TSetting.<key>`` objects to
    ``get_Setting``/``set_Setting``).
    """
    text = _read_bridge()
    assert re.search(
        r'import_type\(\s*"[^"]*ProgramData\+TSetting"\s*\)', text
    ), (
        "TSetting must be imported via luanet.import_type"
        "(\"EUD_Editor_3.ProgramData+TSetting\") (nested enum -> '+' separator)"
    )
    # used as enum objects in the program path (get_Setting / set_Setting).
    assert re.search(r"get_Setting\(", text), (
        "program-scope GETSET must call pgData:get_Setting(TSetting enum)"
    )
    assert re.search(r"set_Setting\(", text), (
        "program-scope SETSET must call pgData:set_Setting(TSetting enum, value)"
    )


def test_setset_program_write_flushes_savesetting():
    """A program-scope SETSET flushes ``SaveSetting()`` after the write (B3)."""
    region = _branch_region(_read_bridge(), "SETSET")
    assert "SaveSetting" in region, (
        "SETSET must flush pgData:SaveSetting() after a program-scope write "
        "(B3 model path: 'set_Setting(...) + SaveSetting()')"
    )


# --------------------------------------------------------------------------- #
# 3. PLUGLIST. (FAIL before impl.)
# --------------------------------------------------------------------------- #


def test_pluglist_region_walks_edsblock_blocks():
    """PLUGLIST walks ``pjData.EdsBlock.Blocks`` (B3 model path)."""
    region = _branch_region(_read_bridge(), "PLUGLIST")
    assert "EdsBlock" in region, (
        "PLUGLIST must walk pjData.EdsBlock.Blocks (B3 model path)"
    )
    assert "Blocks" in region, "PLUGLIST must read the .Blocks collection"
    assert "BType" in region, (
        "PLUGLIST output is 'index TAB BType TAB first-line-of-Texts'"
    )


# --------------------------------------------------------------------------- #
# 4. PLUGADD: EdsBlockItem(UserPlugin) + Texts=body + Insert + SetDirty.
#    (FAIL before impl.)
# --------------------------------------------------------------------------- #


def test_plugadd_region_constructs_userplugin_item_and_inserts():
    region = _branch_region(_read_bridge(), "PLUGADD")
    assert "EdsBlockItem" in region, (
        "PLUGADD must construct an EdsBlockItem (B3 model path)"
    )
    assert "UserPlugin" in region, (
        "PLUGADD must construct an EdsBlockItem(EdsBlockType.UserPlugin)"
    )
    assert "Insert" in region, (
        "PLUGADD must insert into Blocks via Blocks:Insert(index, item)"
    )


def test_plugadd_region_texts_from_body_and_setdirty():
    region = _branch_region(_read_bridge(), "PLUGADD")
    assert "body" in region, (
        "PLUGADD Texts must travel in the BODY (B3: '+ Texts in BODY')"
    )
    assert "Texts" in region, "PLUGADD must assign item.Texts"
    assert "SetDirty" in region, "PLUGADD must SetDirty after the insert"


# --------------------------------------------------------------------------- #
# 5. PLUGSET: Blocks:get_Item(i).Texts = body; UserPlugin guard; SetDirty.
#    (FAIL before impl.)
# --------------------------------------------------------------------------- #


def test_plugset_region_userplugin_guard_and_texts_from_body():
    region = _branch_region(_read_bridge(), "PLUGSET")
    assert "UserPlugin" in region, (
        "PLUGSET must guard UserPlugin only (else ERROR; B3)"
    )
    assert "body" in region, (
        "PLUGSET Texts must travel in the BODY (B3: '+ Texts in BODY')"
    )
    assert "Texts" in region, (
        "PLUGSET must assign Blocks:get_Item(i).Texts = body"
    )
    assert "SetDirty" in region, "PLUGSET must SetDirty after the write"


def test_plugset_region_accesses_item_via_get_item():
    """PLUGSET reads the block via ``Blocks:get_Item(i)`` (List default Item).

    rules.md: a List(Of T) default ``Item`` property is accessed as
    ``:get_Item(i)`` from KopiLua (Blocks(i) in VB is the default property).
    """
    region = _branch_region(_read_bridge(), "PLUGSET")
    assert "get_Item" in region, (
        "PLUGSET must access the block via Blocks:get_Item(i) "
        "(List(Of T) default Item -> get_Item in luanet)"
    )


# --------------------------------------------------------------------------- #
# 6. PLUGDEL: Blocks:RemoveAt(i); UserPlugin guard; SetDirty. (FAIL before impl.)
# --------------------------------------------------------------------------- #


def test_plugdel_region_userplugin_guard_removeat_setdirty():
    region = _branch_region(_read_bridge(), "PLUGDEL")
    assert "UserPlugin" in region, (
        "PLUGDEL must guard UserPlugin only (built-ins auto-reinsert; B3)"
    )
    assert "RemoveAt" in region, (
        "PLUGDEL must remove via Blocks:RemoveAt(i) (B3 model path)"
    )
    assert "SetDirty" in region, "PLUGDEL must SetDirty after the removal"


# --------------------------------------------------------------------------- #
# 7. PLUGMOVE: RemoveAt + Insert reorder; SetDirty. (FAIL before impl.)
# --------------------------------------------------------------------------- #


def test_plugmove_region_removeat_insert_setdirty():
    region = _branch_region(_read_bridge(), "PLUGMOVE")
    assert "RemoveAt" in region, (
        "PLUGMOVE must RemoveAt the source index (reorder via RemoveAt + Insert)"
    )
    assert "Insert" in region, (
        "PLUGMOVE must Insert at the destination index (reorder)"
    )
    assert "SetDirty" in region, "PLUGMOVE must SetDirty after the reorder"


# --------------------------------------------------------------------------- #
# 8. v6 regression + crash-rule lint + non-ASCII baseline. (PASS throughout.)
# --------------------------------------------------------------------------- #


def test_v6_command_markers_present():
    """All v6 + EUD-049 DAT + EUD-050 file-tree branches survive."""
    text = _read_bridge()
    missing = [c for c in V6_COMMANDS if not _branch_re(c).search(text)]
    assert not missing, f"missing v6/DAT/file-tree command branches: {missing}"


def test_no_forbidden_lua_calls():
    """Crash-rule lint: no os.execute / io.popen / :GetValue( anywhere."""
    text = _read_bridge()
    forbidden = [
        tok for tok in ("os.execute", "io.popen", ":GetValue(") if tok in text
    ]
    assert not forbidden, f"forbidden lua calls present: {forbidden}"


def test_extension_adds_no_raw_nonascii_bytes():
    """The settings/plugins extension is ASCII-only: total non-ASCII bytes must
    not grow above the pre-implementation baseline."""
    raw = BRIDGE.read_bytes()
    nonascii = sum(1 for b in raw if b > 0x7F)
    assert nonascii <= BASELINE_NONASCII_BYTES, (
        f"non-ASCII byte count grew to {nonascii} (baseline "
        f"{BASELINE_NONASCII_BYTES}); the settings/plugins extension must be "
        "ASCII-only"
    )


# --------------------------------------------------------------------------- #
# 9. bridge_io client wrappers + whitelist mirrors (source-level).
#    (FAIL before impl.)
# --------------------------------------------------------------------------- #


def test_bridge_io_defines_settings_plugin_wrappers():
    """``bridge_io.py`` defines a client method per new command."""
    src = _read_io()
    missing = [
        w for w in IO_WRAPPERS if not re.search(r"\bdef\s+" + w + r"\s*\(", src)
    ]
    assert not missing, f"bridge_io missing settings/plugin wrapper methods: {missing}"


def test_bridge_io_mirrors_project_key_whitelist():
    """bridge_io carries the project-scope key whitelist (mirror of the bridge)."""
    src = _read_io()
    missing = [
        k for k in PROJECT_KEYS
        if ('"' + k + '"') not in src and ("'" + k + "'") not in src
    ]
    assert not missing, (
        f"bridge_io project-key whitelist missing: {missing} "
        "(must mirror the bridge's project-scope whitelist exactly)"
    )


def test_bridge_io_mirrors_program_key_whitelist():
    """bridge_io carries the program-scope key whitelist (euddraft/starcraft/
    Language), mirroring the bridge."""
    src = _read_io()
    missing = [
        k for k in PROGRAM_KEYS
        if ('"' + k + '"') not in src and ("'" + k + "'") not in src
    ]
    assert not missing, (
        f"bridge_io program-key whitelist missing: {missing} "
        "(must mirror the bridge's program-scope whitelist exactly)"
    )


# --------------------------------------------------------------------------- #
# 10. Behavioral: validation helpers reject BEFORE any .cmd is written, and the
# body-placement contract (SETSET value / PLUG* Texts in the body).
# --------------------------------------------------------------------------- #

import tempfile  # noqa: E402  (kept local to the behavioral section)

_SERVER_DIR = str(REPO_ROOT / "server")
if _SERVER_DIR not in sys.path:
    sys.path.insert(0, _SERVER_DIR)


def _fresh_io():
    """A BridgeIO bound to a fresh empty temp data dir, plus the dir path."""
    from eud_agent.bridge_io import BridgeIO  # noqa: PLC0415 (import-on-use)

    data_dir = Path(tempfile.mkdtemp(prefix="plug-"))
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


def test_getset_rejects_bad_scope():
    """GETSET rejects a scope outside {project, program} before send."""
    bio, data_dir = _fresh_io()
    _assert_rejected_before_send(
        lambda: bio.getset("theme", "OpenMapName", timeout=0.2, poll_interval=0.02),
        data_dir,
    )


def test_getset_rejects_bad_key():
    """GETSET rejects a key outside the scope's whitelist before send."""
    bio, data_dir = _fresh_io()
    _assert_rejected_before_send(
        lambda: bio.getset("project", "NotAKey", timeout=0.2, poll_interval=0.02),
        data_dir,
    )


def test_setset_rejects_language_write():
    """SETSET on the program Language key is rejected (read-only) before send."""
    bio, data_dir = _fresh_io()
    _assert_rejected_before_send(
        lambda: bio.setset(
            "program", "Language", "en-US", timeout=0.2, poll_interval=0.02
        ),
        data_dir,
    )


def test_setset_rejects_bad_key():
    """SETSET rejects an out-of-whitelist program key before send."""
    bio, data_dir = _fresh_io()
    _assert_rejected_before_send(
        lambda: bio.setset(
            "program", "Theme", "Dark", timeout=0.2, poll_interval=0.02
        ),
        data_dir,
    )


def test_plugset_rejects_negative_index():
    """PLUGSET requires a non-negative index (only PLUGADD allows -1)."""
    bio, data_dir = _fresh_io()
    _assert_rejected_before_send(
        lambda: bio.plugset(-1, "[x]", timeout=0.2, poll_interval=0.02),
        data_dir,
    )


def test_plugdel_rejects_negative_index():
    bio, data_dir = _fresh_io()
    _assert_rejected_before_send(
        lambda: bio.plugdel(-1, timeout=0.2, poll_interval=0.02),
        data_dir,
    )


def test_setset_value_travels_in_body():
    """SETSET sends ``SETSET scope|key`` on the arg line and the value in BODY."""
    try:
        from test_bridge_io import FakeBridge  # noqa: PLC0415  (import-only reuse)
    except Exception as exc:  # noqa: BLE001  (standalone: no pytest)
        raise _SkipTest(f"test_bridge_io unavailable: {exc}") from exc

    bio, data_dir = _fresh_io()
    captured: dict[str, str] = {}

    def responder(first_line, body):
        captured["first_line"] = first_line
        captured["body"] = body
        return "OK: set"

    with FakeBridge(data_dir, responder):
        bio.setset(
            "project", "OpenMapName", "C:/maps/x.scx",
            timeout=3.0, poll_interval=0.02,
        )

    assert captured["first_line"] == "SETSET project|OpenMapName", (
        f"SETSET arg line must be 'SETSET scope|key', got {captured['first_line']!r}"
    )
    assert captured["body"] == "C:/maps/x.scx", (
        f"SETSET value must travel in the BODY, got body={captured['body']!r}"
    )


def test_plugadd_texts_travel_in_body_and_append_with_minus_one():
    """PLUGADD sends ``PLUGADD <index>`` arg line + Texts in BODY; -1 appends."""
    try:
        from test_bridge_io import FakeBridge  # noqa: PLC0415  (import-only reuse)
    except Exception as exc:  # noqa: BLE001  (standalone: no pytest)
        raise _SkipTest(f"test_bridge_io unavailable: {exc}") from exc

    bio, data_dir = _fresh_io()
    captured: dict[str, str] = {}

    def responder(first_line, body):
        captured["first_line"] = first_line
        captured["body"] = body
        return "OK: added"

    with FakeBridge(data_dir, responder):
        bio.plugadd(-1, "[freeze]\nprompt: 1", timeout=3.0, poll_interval=0.02)

    assert captured["first_line"] == "PLUGADD -1", (
        f"PLUGADD arg line must be 'PLUGADD <index>' (-1 appends), got "
        f"{captured['first_line']!r}"
    )
    assert captured["body"] == "[freeze]\nprompt: 1", (
        f"PLUGADD Texts must travel in the BODY, got body={captured['body']!r}"
    )


def test_plugmove_sends_from_to_arg_line():
    """PLUGMOVE sends ``PLUGMOVE from|to`` on the arg line."""
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
        bio.plugmove(7, 2, timeout=3.0, poll_interval=0.02)

    assert captured["first_line"] == "PLUGMOVE 7|2", (
        f"PLUGMOVE arg line must be 'PLUGMOVE from|to', got "
        f"{captured['first_line']!r}"
    )


# --------------------------------------------------------------------------- #
# Standalone runner (mirrors test_bridge_tree_static.py).
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
