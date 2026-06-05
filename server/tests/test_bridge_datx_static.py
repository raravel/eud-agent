"""Verification artifact for EUD-049-5d70: bridge DAT surface commands (static).

Static source checks against ``bridge/ZZZ_10_agent_bridge.lua`` and
``server/eud_agent/bridge_io.py`` for the v2 DAT surface
(hivemind/docs/features/04_bridge-v2-surface.md "DAT surface (B1)" table +
"Verification contract"; capability-survey.md rows 1-8 + "Cross-cutting safety
facts"):

  - Resolver REPLACED: a bridge-local name->enum table over ``SCDatFiles+DatFiles``
    covering all ten dat names (units/weapons/flingy/sprites/images/upgrades/
    techdata/orders + portdata + sfxdata), bypassing ``GetDatFileE``'s 8-name
    whitelist (so GETDAT/SETDAT no longer route through ``GetDatFileE``).
  - New dispatcher branches: GETXDAT/SETXDAT, GETTBL/SETTBL, RESETDAT,
    GETREQ/SETREQ, GETBTN/SETBTN -- each wired as a ``cmd == "<NAME>"`` branch.
  - Model API tokens pinned to the RIGHT branch regions: ``get_ExtraDatBinding``
    (XDAT), ``StatTxtBinding`` (TBL), ``get_RequireData`` +
    ``get_RequireDataBinding`` + ``PasteCopyData`` (SETREQ), ``GetCopyString``
    (GETBTN/GETREQ), ``PasteFromString`` + ``SetDirty`` (SETBTN), ``DataReset``
    (RESETDAT).
  - SETREQ use-mode safety: payloads are NUMERIC-prefixed (RequireUse 0-4). The
    server maps keywords (Default/Dont/Always/AlwaysCurrent -> 0/1/2/3) and
    rejects a non-numeric first segment before send; the bridge guards the first
    segment structurally and routes DefaultUse ("0") through
    ``get_RequireDataBinding(...).IsDefaultUse`` (PasteCopyData has no DefaultUse
    branch -> silent no-op), all other segments via ``PasteCopyData``.
  - SETXDAT reads back ``.Value`` after assignment (Byte setters silently
    swallow bad values -- capability-survey "ExtraDat setters ... read back").
  - SETTBL takes its value from the command BODY (UTF-8 .NET read, Korean-safe)
    and honours the ``NULLSTRING`` reset keyword.
  - v6 regression: every existing dispatcher command survives import-then-extend.
  - Crash-rule lint: no ``os.execute`` / ``io.popen`` / ``:GetValue(`` anywhere;
    the new lua is pure ASCII, so the file's non-ASCII byte count must not grow
    above the verified baseline.
  - bridge_io wrappers: the server defines client wrappers + arg-validation
    helpers for each new command (source-level checks, stdlib-only).

This file is pytest-compatible (plain ``test_*`` functions with asserts) AND
standalone-runnable with system Python::

    python server/tests/test_bridge_datx_static.py

Only the stdlib is used (the project venv may be unavailable for the standalone
run; source-level checks need no third-party deps).

Failure profile before implementation (Step A): the 15 implementation-pinning
checks FAIL -- the three resolver-table checks, the new-branch check, the six
model-token region checks, the SETXDAT read-back, the SETTBL body/NULLSTRING
check, and the three bridge_io wrapper/validation checks. The 5 checks that PASS
throughout are the file-presence, v6-command-marker, forbidden-call, and
non-ASCII-baseline guards (same "pass throughout" group as
test_bridge_list_static.py) plus the bridge_io import-surface check (the module
already exists). The ``_SETTABLE_FAMILIES`` SCA-exclusion guard is NOT duplicated
here -- it lives in test_bridge_list_static.py (EUD-048).
"""

from __future__ import annotations

import re
import sys
from pathlib import Path
from unittest import SkipTest as _SkipTest  # pytest treats this as a skip

# repo_root: server/tests/test_bridge_datx_static.py -> parents[2] == repo root
REPO_ROOT = Path(__file__).resolve().parents[2]

BRIDGE = REPO_ROOT / "bridge" / "ZZZ_10_agent_bridge.lua"
BRIDGE_IO = REPO_ROOT / "server" / "eud_agent" / "bridge_io.py"

# The ten standard dat names the resolver table must cover. The first eight are
# GetDatFileE's whitelist; portdata/sfxdata are the bypass targets (capability-
# survey rows 1-2: same store, excluded by the GetDatFileE whitelist).
DAT_NAMES = (
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

# New v2 DAT-surface dispatcher branches.
NEW_COMMANDS = (
    "GETXDAT",
    "SETXDAT",
    "GETTBL",
    "SETTBL",
    "RESETDAT",
    "GETREQ",
    "SETREQ",
    "GETBTN",
    "SETBTN",
)

# v6 command markers that must survive import-then-extend (each matched as a
# dispatcher branch so a stray substring elsewhere cannot satisfy the check).
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
    "PANEL",
    "BUILD",
    "LUA",
)

# bridge_io client wrappers (one method per new + replaced dat command).
IO_WRAPPERS = (
    "getdat",
    "setdat",
    "getxdat",
    "setxdat",
    "gettbl",
    "settbl",
    "resetdat",
    "getreq",
    "setreq",
    "getbtn",
    "setbtn",
)

# Known non-ASCII byte count in the current (pre-implementation) bridge source
# (Korean mojibake in comments + WPF/error strings). The DAT-surface extension
# is ASCII-only, so this count must not increase. Baseline computed from the
# checked-in file at task start.
BASELINE_NONASCII_BYTES = 582


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
    same region-extraction idiom as test_bridge_list_static.py.
    """
    m = _branch_re(name).search(text)
    assert m, f'{name} branch missing (expected `cmd == "{name}"`)'
    region = text[m.start():]
    nxt = re.search(r'\n\s*elseif cmd ==', region[1:])
    if nxt:
        region = region[: nxt.start() + 1]
    return region


# --------------------------------------------------------------------------- #
# 0. File presence
# --------------------------------------------------------------------------- #


def test_bridge_file_present_and_nonempty():
    assert BRIDGE.is_file(), f"missing file: {BRIDGE}"
    assert BRIDGE.stat().st_size > 0, f"empty file: {BRIDGE}"


# --------------------------------------------------------------------------- #
# 1. Resolver table over SCDatFiles+DatFiles, all ten dat names, no GetDatFileE
#    on the GETDAT/SETDAT path. (FAILS before implementation.)
# --------------------------------------------------------------------------- #


def test_resolver_table_covers_all_ten_dat_names():
    """A bridge-local name->enum table covering all ten standard dat names.

    The resolver must map every name -- including portdata/sfxdata, which the
    editor's ``GetDatFileE`` whitelist excludes (capability-survey row 2) -- so
    each name must appear as a string key in the bridge source.
    """
    text = _read_bridge()
    missing = [n for n in DAT_NAMES if ('"' + n + '"') not in text]
    assert not missing, (
        f"resolver table missing dat-name keys: {missing} "
        "(must cover units/weapons/flingy/sprites/images/upgrades/techdata/"
        "orders + portdata + sfxdata)"
    )


def test_resolver_references_scdatfiles_or_datfiles_store():
    """The table is built over the ``SCDatFiles+DatFiles`` enum store.

    capability-survey row 2: the standard + portdata/sfxdata bindings live in the
    same store reached via ``DatFiles`` / ``SCDatFiles`` -- the bridge must
    reference that store rather than the whitelisting ``GetDatFileE`` accessor.
    """
    text = _read_bridge()
    assert "DatFiles" in text, (
        "resolver must build its name->enum table over the SCDatFiles+DatFiles "
        "store (no 'DatFiles' reference found)"
    )


def test_getdat_setdat_do_not_use_getdatfilee():
    """GETDAT/SETDAT no longer route through ``GetDatFileE`` (whitelist regression).

    ``GetDatFileE`` caps the resolvable names at the 8-entry whitelist (excludes
    portdata/sfxdata). After the table replaces it, the token must be absent from
    the GETDAT/SETDAT path. Guarding the whole file catches a revert too -- the
    resolver table is the single replacement, so ``GetDatFileE`` should disappear.
    """
    text = _read_bridge()
    assert "GetDatFileE" not in text, (
        "GetDatFileE (the 8-name whitelist) must be REPLACED by the bridge-local "
        "name->enum table; it still appears in the source"
    )


# --------------------------------------------------------------------------- #
# 2. New dispatcher branches present. (FAIL before implementation.)
# --------------------------------------------------------------------------- #


def test_new_dat_surface_branches_present():
    """All nine new DAT-surface commands are wired into the dispatcher."""
    text = _read_bridge()
    missing = [c for c in NEW_COMMANDS if not _branch_re(c).search(text)]
    assert not missing, f"missing new DAT-surface command branches: {missing}"


# --------------------------------------------------------------------------- #
# 3. Model API tokens pinned to the RIGHT branch regions. (FAIL before impl.)
# --------------------------------------------------------------------------- #


def test_xdat_path_uses_extra_dat_binding():
    """The XDAT model access goes through ``get_ExtraDatBinding``.

    GETXDAT/SETXDAT resolve via the bridge-local ``resolveXDatBinding`` helper
    (the same factoring the v6 GETDAT/SETDAT use with ``resolveDatBinding``); the
    ExtraDat model call lives in that helper. Pin the token to the helper body AND
    assert both XDAT branches invoke the helper, so a regression that drops the
    ExtraDat binding (or rewires a branch off it) is caught either way.
    """
    text = _read_bridge()
    # Helper region: from the local function def to the next blank-line-delimited
    # local function (mirrors _branch_region's "until the next sibling" idiom).
    m = re.search(r"local function resolveXDatBinding\(", text)
    assert m, "resolveXDatBinding helper missing"
    helper = text[m.start():]
    nxt = re.search(r"\n\s*local function ", helper[1:])
    if nxt:
        helper = helper[: nxt.start() + 1]
    assert "get_ExtraDatBinding" in helper, (
        "resolveXDatBinding must call get_ExtraDatBinding (capability-survey 4-6)"
    )
    for cmd in ("GETXDAT", "SETXDAT"):
        region = _branch_region(text, cmd)
        assert "resolveXDatBinding" in region, (
            f"{cmd} must resolve via resolveXDatBinding (the ExtraDat binding path)"
        )


def test_tbl_regions_use_stat_txt_binding():
    """``StatTxtBinding`` appears in BOTH the GETTBL and SETTBL regions."""
    text = _read_bridge()
    for cmd in ("GETTBL", "SETTBL"):
        region = _branch_region(text, cmd)
        assert "StatTxtBinding" in region, (
            f"{cmd} must use get_StatTxtBinding (capability-survey row 3)"
        )


def test_req_regions_use_require_data_and_pastecopydata():
    """REQ regions reach the require store and round-trip via the copy-string API.

    The custom copy-string round-trip lives on ``CRequireData`` (reached through
    ``ExtraDat.RequireData(enum)``); the DefaultUse (segment "0") path is a silent
    no-op in PasteCopyData, so it is routed through the parameterized
    ``get_RequireDataBinding(objId, enum).IsDefaultUse`` setter instead (verified
    against the editor source, CRequireData.vb:88 has no DefaultUse branch).
    Load-bearing SETREQ tokens: ``get_RequireData`` (store) AND
    ``get_RequireDataBinding`` (DefaultUse path) AND ``PasteCopyData`` (custom).
    """
    text = _read_bridge()
    for cmd in ("GETREQ", "SETREQ"):
        region = _branch_region(text, cmd)
        assert "get_RequireData" in region, (
            f"{cmd} must reach the require store via ExtraDat:get_RequireData "
            "(capability-survey row 7)"
        )
    set_region = _branch_region(text, "SETREQ")
    assert "get_RequireDataBinding" in set_region, (
        "SETREQ must route DefaultUse through "
        "BindingManager:get_RequireDataBinding(...).IsDefaultUse (PasteCopyData "
        "has no DefaultUse branch -> silent no-op)"
    )
    assert "PasteCopyData" in set_region, (
        "SETREQ must round-trip custom payloads through CRequireData PasteCopyData"
    )


def test_setreq_region_guards_numeric_first_segment():
    """SETREQ rejects a non-numeric first dot-segment BEFORE any .NET call.

    The editor's PasteCopyData coerces the first segment String->Enum (number); a
    keyword like "Always" throws an uncatchable InvalidCastException (lua pcall
    does NOT catch .NET exceptions) -> editor error dialog. The bridge must guard
    structurally: extract the first segment and reject a non-numeric one with the
    exact ERROR literal, with no model call on that path.
    """
    region = _branch_region(_read_bridge(), "SETREQ")
    assert "payload first segment must be numeric" in region, (
        "SETREQ must return the numeric-first-segment ERROR guard literal"
    )
    # The guard tests the segment with a digit pattern before touching the model.
    assert "%d" in region, (
        "SETREQ must pattern-check the first segment is numeric (^%d+$)"
    )


def test_getreq_region_uses_getcopystring():
    """GETREQ reads via ``GetCopyString`` (the editor's own copy-string format)."""
    region = _branch_region(_read_bridge(), "GETREQ")
    assert "GetCopyString" in region, (
        "GETREQ must read the require data via GetCopyString"
    )


def test_btn_regions_use_button_set_roundtrip():
    """SETBTN pastes via ``PasteFromString`` and dirties via ``SetDirty``;
    GETBTN reads via ``GetCopyString``.
    """
    text = _read_bridge()
    set_region = _branch_region(text, "SETBTN")
    assert "PasteFromString" in set_region, (
        "SETBTN must call GetButtonSet(id):PasteFromString(csv) "
        "(capability-survey row 6)"
    )
    assert "SetDirty" in set_region, (
        "SETBTN must call SetDirty(true) -- direct mutations don't auto-dirty"
    )
    get_region = _branch_region(text, "GETBTN")
    assert "GetCopyString" in get_region, (
        "GETBTN must read the button table via GetCopyString"
    )


def test_resetdat_region_uses_datareset():
    """RESETDAT routes ``DataReset()`` over kind in {dat, xdat, tbl}."""
    region = _branch_region(_read_bridge(), "RESETDAT")
    assert "DataReset" in region, (
        "RESETDAT must call binding:DataReset() (capability-survey row 8)"
    )


# --------------------------------------------------------------------------- #
# 4. SETXDAT reads back .Value after assignment. (FAIL before impl.)
# --------------------------------------------------------------------------- #


def test_setxdat_reads_back_value_after_assignment():
    """The SETXDAT region re-reads ``.Value`` after the assignment.

    Byte-backed ExtraDat setters silently swallow bad values (capability-survey
    "Cross-cutting safety facts": "ExtraDat setters are Byte-typed Try/Catch
    (silently swallow bad values -- read back to confirm)"). So an assignment
    ``binding.Value = ...`` must be followed by a ``.Value`` read in the same
    region (the read-back the server verifies against).
    """
    region = _branch_region(_read_bridge(), "SETXDAT")
    # The region must both ASSIGN .Value and READ .Value back. Require at least
    # one assignment (``.Value = ``) and at least one further ``.Value``
    # occurrence after it (the read-back used to build the result).
    assign = re.search(r"\.Value\s*=", region)
    assert assign, "SETXDAT must assign binding.Value"
    after = region[assign.end():]
    assert "Value" in after, (
        "SETXDAT must read back .Value AFTER the assignment (Byte setters swallow "
        "bad values -- return the re-read value so the server can verify)"
    )


# --------------------------------------------------------------------------- #
# 5. SETTBL value from BODY + NULLSTRING reset. (FAIL before impl.)
# --------------------------------------------------------------------------- #


def test_settbl_uses_body_value_and_nullstring_reset():
    """SETTBL sources its value from ``body`` (never the arg line) and honours
    the ``NULLSTRING`` reset keyword (capability-survey row 3: value in BODY,
    UTF-8 .NET read; ``NULLSTRING`` body resets to default).
    """
    region = _branch_region(_read_bridge(), "SETTBL")
    assert "body" in region, (
        "SETTBL must read its value from the command BODY (UTF-8 / Korean-safe), "
        "never the pipe-separated arg line"
    )
    assert "NULLSTRING" in region, (
        "SETTBL must handle the NULLSTRING body keyword (reset to default)"
    )


# --------------------------------------------------------------------------- #
# 6. v6 regression + crash-rule lint + non-ASCII baseline. (PASS throughout.)
# --------------------------------------------------------------------------- #


def test_v6_command_markers_present():
    """All v6 dispatcher commands survive (import-then-extend regression)."""
    text = _read_bridge()
    missing = [c for c in V6_COMMANDS if not _branch_re(c).search(text)]
    assert not missing, f"missing v6 command branches: {missing}"


def test_no_forbidden_lua_calls():
    """Crash-rule lint: no os.execute / io.popen / :GetValue( anywhere."""
    text = _read_bridge()
    forbidden = [
        tok for tok in ("os.execute", "io.popen", ":GetValue(") if tok in text
    ]
    assert not forbidden, f"forbidden lua calls present: {forbidden}"


def test_extension_adds_no_raw_nonascii_bytes():
    """The DAT-surface extension is ASCII-only: total non-ASCII bytes must not
    grow above the pre-implementation baseline.

    The current source already carries Korean mojibake; the new lua and all new
    result strings are pure ASCII, so the count must stay <= the baseline.
    """
    raw = BRIDGE.read_bytes()
    nonascii = sum(1 for b in raw if b > 0x7F)
    assert nonascii <= BASELINE_NONASCII_BYTES, (
        f"non-ASCII byte count grew to {nonascii} (baseline "
        f"{BASELINE_NONASCII_BYTES}); the DAT-surface extension must be ASCII-only"
    )


# --------------------------------------------------------------------------- #
# 7. bridge_io client wrappers + arg-validation helpers. (FAIL before impl.)
# --------------------------------------------------------------------------- #


def test_bridge_io_defines_dat_surface_wrappers():
    """``bridge_io.py`` defines a client method per new/replaced dat command.

    Source-level check (stdlib-only, standalone-safe): each wrapper is a
    ``def <name>(`` on the BridgeIO class. Matches the established source-check
    convention (test_bridge_list_static.test_settable_families_exclude_sca).
    """
    src = _read_io()
    missing = [
        w for w in IO_WRAPPERS if not re.search(r"\bdef\s+" + w + r"\s*\(", src)
    ]
    assert not missing, f"bridge_io missing wrapper methods: {missing}"


def test_bridge_io_validates_dat_names():
    """bridge_io carries the dat-name whitelist (incl. portdata/sfxdata).

    Arg validation must reject unknown dat names before sending; the whitelist
    therefore names portdata and sfxdata (the bypass targets). Their presence in
    the module source is a robust, location-agnostic signal of the validation
    helper.
    """
    src = _read_io()
    missing = [
        n for n in ("portdata", "sfxdata") if ('"' + n + '"') not in src
        and ("'" + n + "'") not in src
    ]
    assert not missing, (
        f"bridge_io dat-name validation missing: {missing} "
        "(the whitelist must include portdata/sfxdata)"
    )


def test_bridge_io_validates_xdat_kinds():
    """bridge_io carries the xdat-kind whitelist {statusinfor, wireframe, ButtonSet}.

    GETXDAT/SETXDAT accept dat in {statusinfor, wireframe, ButtonSet} (B1 table);
    the validation helper must name them so out-of-set kinds are rejected
    server-side before the bridge round-trip.
    """
    src = _read_io()
    missing = [
        k for k in ("statusinfor", "wireframe", "ButtonSet")
        if ('"' + k + '"') not in src and ("'" + k + "'") not in src
    ]
    assert not missing, (
        f"bridge_io xdat-kind validation missing: {missing}"
    )


# --------------------------------------------------------------------------- #
# 8. Behavioral: validation helpers reject BEFORE any .cmd is written.
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

    data_dir = Path(tempfile.mkdtemp(prefix="datx-"))
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


def test_getdat_rejects_unknown_dat_name():
    bio, data_dir = _fresh_io()
    _assert_rejected_before_send(
        lambda: bio.getdat("nosuchdat", "HP", 0, timeout=0.2, poll_interval=0.02),
        data_dir,
    )


def test_setdat_rejects_non_numeric_value():
    bio, data_dir = _fresh_io()
    _assert_rejected_before_send(
        lambda: bio.setdat(
            "units", "HP", 0, "notanumber", timeout=0.2, poll_interval=0.02
        ),
        data_dir,
    )


def test_setdat_rejects_negative_objid():
    bio, data_dir = _fresh_io()
    _assert_rejected_before_send(
        lambda: bio.setdat("units", "HP", -1, 100, timeout=0.2, poll_interval=0.02),
        data_dir,
    )


def test_getxdat_rejects_unknown_kind():
    bio, data_dir = _fresh_io()
    _assert_rejected_before_send(
        lambda: bio.getxdat("bogus", "Status", 0, timeout=0.2, poll_interval=0.02),
        data_dir,
    )


def test_getreq_rejects_unknown_req_dat():
    bio, data_dir = _fresh_io()
    _assert_rejected_before_send(
        lambda: bio.getreq("flingy", 0, timeout=0.2, poll_interval=0.02),
        data_dir,
    )


def test_resetdat_rejects_unknown_kind():
    bio, data_dir = _fresh_io()
    _assert_rejected_before_send(
        lambda: bio.resetdat(
            "bogus", "units", "HP", 0, timeout=0.2, poll_interval=0.02
        ),
        data_dir,
    )


def test_gettbl_rejects_negative_index():
    bio, data_dir = _fresh_io()
    _assert_rejected_before_send(
        lambda: bio.gettbl(-5, timeout=0.2, poll_interval=0.02),
        data_dir,
    )


def test_portdata_sfxdata_are_accepted_dat_names():
    """portdata/sfxdata pass validation (the GetDatFileE bypass targets).

    They must NOT be rejected at the validation gate. We reuse the FakeBridge
    from test_bridge_io (import-only) so the round-trip completes without a real
    editor. The standalone runner SKIPS this when pytest/test_bridge_io is not
    importable (it pulls in pytest); under pytest the import always works.
    """
    try:
        from test_bridge_io import FakeBridge  # noqa: PLC0415  (import-only reuse)
    except Exception as exc:  # noqa: BLE001  (standalone: no pytest)
        raise _SkipTest(f"test_bridge_io unavailable: {exc}") from exc

    bio, data_dir = _fresh_io()

    def responder(first_line, body):
        return "OK: portdata|x|0 = 7"

    with FakeBridge(data_dir, responder):
        for name in ("portdata", "sfxdata"):
            # Must not raise a validation error (it reaches the bridge).
            bio.getdat(name, "x", 0, timeout=3.0, poll_interval=0.02)


def test_setreq_maps_use_mode_keywords_to_digits():
    """setreq maps Default/Dont/Always/AlwaysCurrent -> "0"/"1"/"2"/"3" in the
    sent body (the editor's PasteCopyData needs a NUMERIC first segment).
    """
    try:
        from test_bridge_io import FakeBridge  # noqa: PLC0415  (import-only reuse)
    except Exception as exc:  # noqa: BLE001  (standalone: no pytest)
        raise _SkipTest(f"test_bridge_io unavailable: {exc}") from exc

    expected = {
        "Default": "0",
        "Dont": "1",
        "Always": "2",
        "AlwaysCurrent": "3",
    }
    for keyword, digit in expected.items():
        bio, data_dir = _fresh_io()
        captured: dict[str, str] = {}

        def responder(first_line, body, _cap=captured):
            _cap["first_line"] = first_line
            _cap["body"] = body
            return f"OK: units|0 = {body}"

        with FakeBridge(data_dir, responder):
            bio.setreq("units", 0, keyword, timeout=3.0, poll_interval=0.02)

        assert captured["first_line"] == "SETREQ units|0"
        assert captured["body"] == digit, (
            f"{keyword!r} must map to {digit!r} in the body, got {captured['body']!r}"
        )


def test_setreq_passes_custom_copy_string_through():
    """A custom copy-string (first segment "4") passes through unchanged."""
    try:
        from test_bridge_io import FakeBridge  # noqa: PLC0415  (import-only reuse)
    except Exception as exc:  # noqa: BLE001  (standalone: no pytest)
        raise _SkipTest(f"test_bridge_io unavailable: {exc}") from exc

    bio, data_dir = _fresh_io()
    captured: dict[str, str] = {}
    payload = "4.0,1.2,3"

    def responder(first_line, body):
        captured["body"] = body
        return f"OK: units|0 = {body}"

    with FakeBridge(data_dir, responder):
        bio.setreq("units", 0, payload, timeout=3.0, poll_interval=0.02)
    assert captured["body"] == payload


def test_setreq_rejects_non_numeric_keyword_before_send():
    """An unknown keyword (non-numeric first segment) raises BEFORE any send."""
    bio, data_dir = _fresh_io()
    _assert_rejected_before_send(
        lambda: bio.setreq("units", 0, "Sometimes", timeout=0.2, poll_interval=0.02),
        data_dir,
    )


# --------------------------------------------------------------------------- #
# Standalone runner (mirrors test_bridge_list_static.py).
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
