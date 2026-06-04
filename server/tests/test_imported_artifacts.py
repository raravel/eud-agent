"""Verification artifact for EUD-008-cddc: imported verified artifacts.

Validates that the verified external artifacts have been imported
(import-then-extend rule, hivemind/docs/rules.md "Editor integrity") into
this repo:

  - bridge/ZZZ_10_agent_bridge.lua          (v6 bridge, now extended)
  - vendor/webview2/*.dll                    (WebView2 SDK 1.0.3800.47, 3 DLLs)
  - server/eud_agent/runner_legacy.py        (legacy runner draft, 7,336 bytes)

DLL and runner_legacy sizes are the ground truth from
hivemind/docs/tech-stack.md "Build Artifacts" and "Legacy / Vendored"; they
remain pure vendored/reference copies and are still checked for exact size and,
when the source path exists on this machine, byte-identity.

The bridge byte-identity / exact-size checks were dropped at EUD-011: the
"import-then-extend" phase began there (the LIST command was added in
``ZZZ_10_agent_bridge.lua``), so the bridge is intentionally no longer
byte-identical to its verified v6 source. It is now validated structurally
instead: the file exists and is non-empty, and every v6 dispatcher command
marker is still present (a regression guard that the extension did not delete a
verified v6 code path). Crash-rule / LIST-specific checks live in
``test_bridge_list_static.py``.

This file is pytest-compatible (plain ``test_*`` functions with asserts)
AND standalone-runnable with system Python::

    python server/tests/test_imported_artifacts.py

The project venv does not exist yet, so only the stdlib is used.
"""

from __future__ import annotations

import filecmp
import re
import sys
from pathlib import Path

# repo_root: server/tests/test_imported_artifacts.py -> parents[2] == repo root
REPO_ROOT = Path(__file__).resolve().parents[2]

# Verified source location for runner_legacy (READ-ONLY; never written/moved).
SRC_RUNNER = Path(r"C:\Users\ifthe\proj\eud\ECA\eud_agent_runner.py")

# Bridge destination (no longer size/byte-identity checked; see module docstring).
BRIDGE_DEST = REPO_ROOT / "bridge" / "ZZZ_10_agent_bridge.lua"

# v6 dispatcher command markers that must survive import-then-extend. Matched as
# ``cmd == "<NAME>"`` branches so a stray substring elsewhere cannot satisfy it.
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

RUNNER_DEST = REPO_ROOT / "server" / "eud_agent" / "runner_legacy.py"
RUNNER_SIZE = 7_336

# WebView2 SDK 1.0.3800.47 DLLs: (filename, expected size in bytes).
WEBVIEW2_DLLS = (
    ("Microsoft.Web.WebView2.Core.dll", 649_840),
    ("Microsoft.Web.WebView2.Wpf.dll", 82_544),
    ("WebView2Loader.dll", 160_880),
)
WEBVIEW2_DIR = REPO_ROOT / "vendor" / "webview2"


def _assert_exact_size(path: Path, expected: int) -> None:
    assert path.is_file(), f"missing file: {path}"
    actual = path.stat().st_size
    assert actual == expected, (
        f"{path} size {actual} != expected {expected}"
    )


def _assert_identical(dest: Path, src: Path) -> None:
    """Byte-identity check, only when the source still exists on this machine."""
    if not src.is_file():
        # Other machine: source absent; size check elsewhere is the guarantee.
        return
    assert filecmp.cmp(dest, src, shallow=False), (
        f"{dest} is not byte-identical to source {src}"
    )


def _bridge_branch_re(name: str) -> "re.Pattern[str]":
    """Match a dispatcher branch comparing ``cmd`` to a command name."""
    return re.compile(r'cmd\s*==\s*"' + re.escape(name) + r'"')


def test_bridge_lua_exists_and_nonempty():
    """bridge/ZZZ_10_agent_bridge.lua is present and non-empty.

    Byte-identity / exact-size checks ended at EUD-011 (import-then-extend
    began with the LIST command); see the module docstring.
    """
    assert BRIDGE_DEST.is_file(), f"missing file: {BRIDGE_DEST}"
    assert BRIDGE_DEST.stat().st_size > 0, f"empty file: {BRIDGE_DEST}"


def test_bridge_lua_v6_command_markers_present():
    """Every v6 dispatcher command survives the extension (regression guard)."""
    # latin-1 round-trips every byte 1:1 (the source carries Korean mojibake).
    text = BRIDGE_DEST.read_bytes().decode("latin-1")
    missing = [c for c in V6_COMMANDS if not _bridge_branch_re(c).search(text)]
    assert not missing, f"missing v6 command branches: {missing}"


def test_webview2_core_dll_exact_size():
    name, size = WEBVIEW2_DLLS[0]
    _assert_exact_size(WEBVIEW2_DIR / name, size)


def test_webview2_wpf_dll_exact_size():
    name, size = WEBVIEW2_DLLS[1]
    _assert_exact_size(WEBVIEW2_DIR / name, size)


def test_webview2_loader_dll_exact_size():
    name, size = WEBVIEW2_DLLS[2]
    _assert_exact_size(WEBVIEW2_DIR / name, size)


def test_runner_legacy_exists_with_exact_size():
    """server/eud_agent/runner_legacy.py imported at exactly 7,336 bytes."""
    _assert_exact_size(RUNNER_DEST, RUNNER_SIZE)


def test_runner_legacy_byte_identical_to_source():
    """When the ECA runner source exists, the reference copy is byte-identical."""
    assert RUNNER_DEST.is_file(), f"missing file: {RUNNER_DEST}"
    _assert_identical(RUNNER_DEST, SRC_RUNNER)


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
        except Exception as exc:  # unexpected (e.g. permission error)
            failures += 1
            print(f"ERROR {name}: {type(exc).__name__}: {exc}")
        else:
            print(f"PASS {name}")
    total = len(_all_test_functions())
    print(f"\n{total - failures}/{total} checks passed")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
