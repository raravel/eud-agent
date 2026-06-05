"""Verification artifact for EUD-011-cb9e: bridge LIST command (static).

Static source checks against ``bridge/ZZZ_10_agent_bridge.lua`` for the new
LIST command (hivemind/docs/features/01_lua-bridge.md "New command: LIST" and
architecture.md "File IPC protocol"):

  - a LIST branch wired into the inbox dispatcher (same ``elseif cmd ==``
    idiom the v6 dispatcher uses);
  - the handler returns ``ERROR: no project`` when no project is loaded;
  - the handler reuses the ``walk()`` helper over ``PFIles`` and builds
    ``<path>\\t<type>`` tab-separated lines (no file contents, no disk writes);
  - ALL v6 command markers remain present (import-then-extend regression);
  - crash-rule lint: no ``os.execute`` / ``io.popen`` / ``:GetValue(`` anywhere,
    and the LIST extension introduces no new raw non-ASCII bytes (the v6 source
    already carries Korean mojibake in comments + UI strings; the extension is
    ASCII-only, so the file's total non-ASCII byte count must not grow).

This file is pytest-compatible (plain ``test_*`` functions with asserts)
AND standalone-runnable with system Python::

    python server/tests/test_bridge_list_static.py

The project venv does not exist yet, so only the stdlib is used.

Checks 1-3 FAIL before LIST is implemented; checks 4-5 pass throughout.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

# repo_root: server/tests/test_bridge_list_static.py -> parents[2] == repo root
REPO_ROOT = Path(__file__).resolve().parents[2]

BRIDGE = REPO_ROOT / "bridge" / "ZZZ_10_agent_bridge.lua"

# v6 command markers that must survive import-then-extend. Each is matched as a
# dispatcher branch ``cmd == "<NAME>"`` so a stray substring elsewhere cannot
# satisfy the check.
V6_COMMANDS = (
    "PING",
    "STATUS",
    "DUMP",
    "GET",
    "SET",
    "GETDAT",
    "SETDAT",
    "BUILD",
    "LUA",
    "PANEL",
)

# Known non-ASCII byte count in the verified v6 import (Korean mojibake in
# comments + WPF panel UI strings + error messages). The LIST extension is
# ASCII-only, so this count must not increase.
V6_NONASCII_BYTES = 1_263


def _read_text() -> str:
    # latin-1 round-trips every byte 1:1, matching how KopiLua reads the source.
    return BRIDGE.read_bytes().decode("latin-1")


def _branch_re(name: str) -> re.Pattern[str]:
    """Match a dispatcher branch comparing ``cmd`` to a command name."""
    return re.compile(r'cmd\s*==\s*"' + re.escape(name) + r'"')


def test_bridge_file_present_and_nonempty():
    assert BRIDGE.is_file(), f"missing file: {BRIDGE}"
    assert BRIDGE.stat().st_size > 0, f"empty file: {BRIDGE}"


def test_list_branch_in_dispatcher():
    """A ``cmd == "LIST"`` branch is wired into the inbox dispatcher."""
    text = _read_text()
    assert _branch_re("LIST").search(text), (
        'no LIST branch (expected `cmd == "LIST"`) in the dispatcher'
    )


def test_list_returns_error_no_project():
    """The handler emits the exact `ERROR: no project` for the nil-project path."""
    text = _read_text()
    assert "ERROR: no project" in text, (
        "LIST must return the literal 'ERROR: no project' when pjData is nil"
    )


def test_list_walks_pfiles_and_builds_tab_lines():
    """LIST reuses walk() over PFIles and emits tab-separated path/type lines."""
    text = _read_text()
    # The LIST handler must reference both the walk helper and PFIles, and
    # build tab-joined lines. Locate the region from the LIST branch onward so
    # the references are attributable to LIST rather than to DUMP/PANEL.
    m = _branch_re("LIST").search(text)
    assert m, 'LIST branch missing (see test_list_branch_in_dispatcher)'
    region = text[m.start():]
    # Cut the region at the next dispatcher branch / unknown-command fallback so
    # we only inspect LIST's own body.
    nxt = re.search(r'\n\s*elseif cmd ==', region[1:])
    if nxt:
        region = region[: nxt.start() + 1]
    assert "walk(" in region, "LIST handler does not call the walk() helper"
    assert "PFIles" in region, "LIST handler does not reference PFIles"
    assert "\\t" in region, (
        r"LIST handler does not build tab-separated (\t) lines"
    )


def test_list_reads_filetype_uppercase():
    """LIST reads the real VB property ``f.FileType`` (uppercase T).

    EUD-040: the VB type exposes ``FileType`` (uppercase); the lowercase
    ``f.Filetype`` silently yields ``"?"`` (pcall fallback) for every file, so
    the server marks all files non-settable. Bind the check to the LIST body via
    the same region-extraction idiom used above.
    """
    text = _read_text()
    m = _branch_re("LIST").search(text)
    assert m, 'LIST branch missing (see test_list_branch_in_dispatcher)'
    region = text[m.start():]
    nxt = re.search(r'\n\s*elseif cmd ==', region[1:])
    if nxt:
        region = region[: nxt.start() + 1]
    assert re.search(r'f\.FileType\b', region), (
        "LIST handler must read f.FileType (uppercase T), the real VB property"
    )


def test_no_lowercase_filetype_anywhere():
    """Regression guard: the lowercase ``f.Filetype`` must not appear at all.

    EUD-040: ``f.Filetype`` (lowercase t) is the bug; guarding the whole bridge
    also catches a revert.
    """
    text = _read_text()
    assert not re.search(r'f\.Filetype\b', text), (
        "f.Filetype (lowercase t) is the EUD-040 bug; use f.FileType (uppercase)"
    )


def test_v6_command_markers_present():
    """All v6 dispatcher commands survive (import-then-extend regression)."""
    text = _read_text()
    missing = [c for c in V6_COMMANDS if not _branch_re(c).search(text)]
    assert not missing, f"missing v6 command branches: {missing}"


def test_no_forbidden_lua_calls():
    """Crash-rule lint: no os.execute / io.popen / :GetValue( anywhere."""
    text = _read_text()
    forbidden = [tok for tok in ("os.execute", "io.popen", ":GetValue(") if tok in text]
    assert not forbidden, f"forbidden lua calls present: {forbidden}"


def test_extension_adds_no_raw_nonascii_bytes():
    """The LIST extension is ASCII-only: total non-ASCII bytes must not grow.

    The v6 source already carries Korean mojibake (1,263 non-ASCII bytes).
    The LIST output and source require none, so the count must stay <= the v6
    baseline. (A full pure-ASCII check is impossible: the verified v6 import is
    not ASCII.)
    """
    raw = BRIDGE.read_bytes()
    nonascii = sum(1 for b in raw if b > 0x7F)
    assert nonascii <= V6_NONASCII_BYTES, (
        f"non-ASCII byte count grew to {nonascii} (v6 baseline "
        f"{V6_NONASCII_BYTES}); the LIST extension must be ASCII-only"
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
