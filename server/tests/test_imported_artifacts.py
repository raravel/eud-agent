"""Verification artifact for EUD-008-cddc: imported verified artifacts.

Validates that the verified external artifacts have been imported UNCHANGED
(import-then-extend rule, hivemind/docs/rules.md "Editor integrity") into
this repo:

  - bridge/ZZZ_10_agent_bridge.lua          (v6 bridge, 16,115 bytes)
  - vendor/webview2/*.dll                    (WebView2 SDK 1.0.3800.47, 3 DLLs)
  - server/eud_agent/runner_legacy.py        (legacy runner draft, 7,336 bytes)

Sizes are the ground truth from hivemind/docs/tech-stack.md "Build Artifacts"
and "Legacy / Vendored". When a source path still exists on this machine the
import is additionally checked for byte-identity (filecmp.cmp shallow=False);
on a machine where the source is absent only the size check runs.

This file is pytest-compatible (plain ``test_*`` functions with asserts)
AND standalone-runnable with system Python::

    python server/tests/test_imported_artifacts.py

The project venv does not exist yet, so only the stdlib is used.
"""

from __future__ import annotations

import filecmp
import sys
from pathlib import Path

# repo_root: server/tests/test_imported_artifacts.py -> parents[2] == repo root
REPO_ROOT = Path(__file__).resolve().parents[2]

# Verified source locations (READ-ONLY; never written/moved/deleted).
SRC_BRIDGE = Path(
    r"C:\Users\ifthe\eud-agent-analysis\test-lua\ZZZ_10_agent_bridge.lua"
)
SRC_RUNNER = Path(r"C:\Users\ifthe\proj\eud\ECA\eud_agent_runner.py")

# Imported destinations, with expected exact sizes in bytes.
BRIDGE_DEST = REPO_ROOT / "bridge" / "ZZZ_10_agent_bridge.lua"
BRIDGE_SIZE = 16_115

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


def test_bridge_lua_exists_with_exact_size():
    """bridge/ZZZ_10_agent_bridge.lua imported at exactly 16,115 bytes."""
    _assert_exact_size(BRIDGE_DEST, BRIDGE_SIZE)


def test_bridge_lua_byte_identical_to_source():
    """When the verified v6 source exists, the import is byte-identical."""
    assert BRIDGE_DEST.is_file(), f"missing file: {BRIDGE_DEST}"
    _assert_identical(BRIDGE_DEST, SRC_BRIDGE)


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
