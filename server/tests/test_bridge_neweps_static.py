"""Verification artifact for EUD-012-241a: bridge NEWEPS command (static).

Static source checks against ``bridge/ZZZ_10_agent_bridge.lua`` for the new
NEWEPS command (hivemind/docs/features/01_lua-bridge.md "New command: NEWEPS",
decisions/02_neweps-duplicate-error.md, and architecture.md "File IPC protocol"):

  - a NEWEPS branch wired into the inbox dispatcher (same ``elseif cmd ==``
    idiom the v6 dispatcher uses);
  - a duplicate pre-check: the branch body references ``findFile`` and emits the
    literal ``ERROR: duplicate '`` (Decision 02 -- no auto-suffix, no side
    effects on duplicate);
  - the verified v6 creation chain inside the branch body: the ``TEFile``
    constructor, ``EFileType.CUIEps`` (enum object -- never a raw number),
    ``StringText`` assignment, ``FileAdd``, and ``TEOpenFile``;
  - a usage-error path: empty name / missing body returns an ``ERROR`` literal;
  - ALL v6 command markers (plus LIST from EUD-011) remain present
    (import-then-extend regression);
  - crash-rule lint: no ``os.execute`` / ``io.popen`` / ``:GetValue(`` anywhere,
    and the NEWEPS extension introduces no new raw non-ASCII bytes (the v6
    source already carries Korean mojibake in comments + UI strings; the
    extension is ASCII-only -- ERROR strings are English per spec -- so the
    file's total non-ASCII byte count must not grow).

This file is pytest-compatible (plain ``test_*`` functions with asserts)
AND standalone-runnable with system Python::

    python server/tests/test_bridge_neweps_static.py

The project venv does not exist yet, so only the stdlib is used.

Checks 1-4 FAIL before NEWEPS is implemented; checks 5-6 pass throughout.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

# repo_root: server/tests/test_bridge_neweps_static.py -> parents[2] == repo root
REPO_ROOT = Path(__file__).resolve().parents[2]

BRIDGE = REPO_ROOT / "bridge" / "ZZZ_10_agent_bridge.lua"

# v6 command markers + LIST (EUD-011) that must survive import-then-extend. Each
# is matched as a dispatcher branch ``cmd == "<NAME>"`` so a stray substring
# elsewhere cannot satisfy the check.
V6_COMMANDS = (
    "PING",
    "STATUS",
    "LIST",
    "DUMP",
    "GET",
    "SET",
    "GETDAT",
    "SETDAT",
    "BUILD",
    "LUA",
    "PANEL",
)

# Known non-ASCII byte count in the current source (Korean mojibake in comments
# + WPF panel UI strings + error messages). Computed from the current file at
# task start: 1,263 bytes (unchanged from the v6 baseline -- the LIST extension
# was ASCII-only). The NEWEPS extension is ASCII-only too (English ERROR
# strings per spec), so this count must not increase.
BASELINE_NONASCII_BYTES = 1_263


def _read_text() -> str:
    # latin-1 round-trips every byte 1:1, matching how KopiLua reads the source.
    return BRIDGE.read_bytes().decode("latin-1")


def _branch_re(name: str) -> re.Pattern[str]:
    """Match a dispatcher branch comparing ``cmd`` to a command name."""
    return re.compile(r'cmd\s*==\s*"' + re.escape(name) + r'"')


def _neweps_body() -> str:
    """Return the source region from the NEWEPS branch to the next branch.

    Mirrors the LIST test's bounded extraction so creation-chain markers are
    attributable to NEWEPS rather than to SET/PANEL/the v6 PANEL button.
    """
    text = _read_text()
    m = _branch_re("NEWEPS").search(text)
    assert m, 'NEWEPS branch missing (see test_neweps_branch_in_dispatcher)'
    region = text[m.start():]
    # Cut at the next dispatcher branch so we only inspect NEWEPS's own body.
    nxt = re.search(r'\n\s*elseif cmd ==', region[1:])
    if nxt:
        region = region[: nxt.start() + 1]
    return region


def test_bridge_file_present_and_nonempty():
    assert BRIDGE.is_file(), f"missing file: {BRIDGE}"
    assert BRIDGE.stat().st_size > 0, f"empty file: {BRIDGE}"


def test_neweps_branch_in_dispatcher():
    """A ``cmd == "NEWEPS"`` branch is wired into the inbox dispatcher."""
    text = _read_text()
    assert _branch_re("NEWEPS").search(text), (
        'no NEWEPS branch (expected `cmd == "NEWEPS"`) in the dispatcher'
    )


def test_neweps_duplicate_precheck():
    """The branch pre-checks findFile and emits `ERROR: duplicate '` (Decision 02)."""
    region = _neweps_body()
    assert "findFile" in region, (
        "NEWEPS handler does not call findFile() to pre-check for duplicates"
    )
    assert "ERROR: duplicate '" in region, (
        "NEWEPS handler does not return the literal \"ERROR: duplicate '<name>'\" "
        "on a duplicate filename (Decision 02 -- no auto-suffix)"
    )


def test_neweps_creation_chain_markers():
    """The verified v6 creation chain appears inside the NEWEPS branch body."""
    region = _neweps_body()
    # TEFile constructor with the CUIEps enum object (not a raw number).
    assert re.search(r"TEFile\s*\(", region), (
        "NEWEPS handler does not construct a TEFile"
    )
    assert "EFileType.CUIEps" in region, (
        "NEWEPS handler does not use the EFileType.CUIEps enum object "
        "(rules.md: enum args MUST be enum objects, never raw numbers)"
    )
    assert "StringText" in region, (
        "NEWEPS handler does not assign Scripter.StringText (the body)"
    )
    assert "FileAdd" in region, (
        "NEWEPS handler does not call PFIles:FileAdd(nf)"
    )
    assert "TEOpenFile" in region, (
        "NEWEPS handler does not call WindowControl.TEOpenFile(nf, 0)"
    )


def test_neweps_usage_error_path():
    """Empty name / missing body returns a usage ERROR literal."""
    region = _neweps_body()
    # An ERROR for the usage path that is NOT the duplicate ERROR. We require an
    # ``ERROR: ...`` literal mentioning NEWEPS usage (name/body), distinct from
    # the duplicate message.
    usage = re.findall(r'"ERROR:[^"]*"', region)
    non_dup = [e for e in usage if "duplicate" not in e]
    assert non_dup, (
        "NEWEPS handler has no usage ERROR literal for the empty-name / "
        "missing-body path (distinct from the duplicate ERROR)"
    )


def test_v6_command_markers_present():
    """All v6 dispatcher commands + LIST survive (import-then-extend regression)."""
    text = _read_text()
    missing = [c for c in V6_COMMANDS if not _branch_re(c).search(text)]
    assert not missing, f"missing v6/LIST command branches: {missing}"


def test_no_forbidden_lua_calls():
    """Crash-rule lint: no os.execute / io.popen / :GetValue( anywhere."""
    text = _read_text()
    forbidden = [tok for tok in ("os.execute", "io.popen", ":GetValue(") if tok in text]
    assert not forbidden, f"forbidden lua calls present: {forbidden}"


def test_extension_adds_no_raw_nonascii_bytes():
    """The NEWEPS extension is ASCII-only: total non-ASCII bytes must not grow.

    The current source carries Korean mojibake (1,263 non-ASCII bytes, computed
    at task start -- unchanged from the v6 baseline since LIST was ASCII-only).
    The NEWEPS handler and its ERROR strings are ASCII-only (English per spec),
    so the count must stay <= the baseline. (A full pure-ASCII check is
    impossible: the verified v6 import is not ASCII.)
    """
    raw = BRIDGE.read_bytes()
    nonascii = sum(1 for b in raw if b > 0x7F)
    assert nonascii <= BASELINE_NONASCII_BYTES, (
        f"non-ASCII byte count grew to {nonascii} (baseline "
        f"{BASELINE_NONASCII_BYTES}); the NEWEPS extension must be ASCII-only"
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
