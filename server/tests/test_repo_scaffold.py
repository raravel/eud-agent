"""Verification artifact for EUD-007-761d: repo scaffolding.

Validates that the repository skeleton exists per
hivemind/docs/architecture.md "Repository layout":

  - required directories (with .gitkeep where they would be empty)
  - .gitignore covering venv/caches/node_modules, and NOT leaking
    editor runtime-state paths (those live in the editor folder)
  - .gitattributes marking vendored WebView2 DLLs as binary

This file is pytest-compatible (plain ``test_*`` functions with asserts)
AND standalone-runnable with system Python::

    python server/tests/test_repo_scaffold.py

The project venv does not exist yet, so no third-party imports are used.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

# repo_root: server/tests/test_repo_scaffold.py -> parents[2] == repo root
REPO_ROOT = Path(__file__).resolve().parents[2]

# Directories required by the architecture "Repository layout" section.
REQUIRED_DIRS = (
    "bridge",
    "server/eud_agent",
    "server/tests",
    "panel",
    "vendor/webview2",
    "scripts",
)

# Runtime-state path fragments that must NOT appear in .gitignore: these
# live in the editor's Data\agent folder, never in this repo.
FORBIDDEN_GITIGNORE_FRAGMENTS = (
    "data/agent",
    "data\\agent",
    "inbox",
    "outbox",
    "heartbeat",
    "server.ready",
    "agent.cfg",
)


def _read_text(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def _gitignore_lines() -> list[str]:
    """Return non-empty, non-comment .gitignore lines, normalized lowercase."""
    path = REPO_ROOT / ".gitignore"
    lines: list[str] = []
    for raw in _read_text(path).splitlines():
        stripped = raw.strip()
        if not stripped or stripped.startswith("#"):
            continue
        lines.append(stripped)
    return lines


def test_required_directories_exist():
    """All architecture-layout directories exist as real directories."""
    missing = [d for d in REQUIRED_DIRS if not (REPO_ROOT / d).is_dir()]
    assert not missing, f"missing directories: {missing}"


def test_empty_dirs_have_gitkeep():
    """Each required dir that holds no tracked content keeps a .gitkeep.

    A directory is considered "would otherwise be empty" if it contains no
    files other than .gitkeep itself (subdirectories that themselves carry
    content do not count). server/tests carries this test file, so it is
    naturally non-empty and exempt.
    """
    needs_gitkeep = []
    for d in REQUIRED_DIRS:
        dir_path = REPO_ROOT / d
        if not dir_path.is_dir():
            # covered by test_required_directories_exist
            continue
        entries = [p for p in dir_path.iterdir() if p.name != ".gitkeep"]
        non_empty = any(
            p.is_file() or (p.is_dir() and any(p.iterdir())) for p in entries
        )
        if not non_empty and not (dir_path / ".gitkeep").is_file():
            needs_gitkeep.append(d)
    assert not needs_gitkeep, f"empty dirs missing .gitkeep: {needs_gitkeep}"


def test_gitignore_exists():
    assert (REPO_ROOT / ".gitignore").is_file(), ".gitignore missing at repo root"


def test_gitignore_covers_venv():
    """server/.venv must be ignored (accept .venv patterns scoped to server)."""
    lines = _gitignore_lines()
    venv_re = re.compile(r"(^|/)\.venv/?$|server/\.venv")
    assert any(venv_re.search(line) for line in lines), (
        f".gitignore does not cover server/.venv; lines={lines}"
    )


def test_gitignore_covers_pycache():
    lines = _gitignore_lines()
    assert any("__pycache__" in line for line in lines), (
        f".gitignore does not cover __pycache__; lines={lines}"
    )


def test_gitignore_covers_pyc():
    lines = _gitignore_lines()
    assert any(line.endswith("*.pyc") or line == "*.pyc" for line in lines), (
        f".gitignore does not cover *.pyc; lines={lines}"
    )


def test_gitignore_covers_node_modules():
    lines = _gitignore_lines()
    assert any("node_modules" in line for line in lines), (
        f".gitignore does not cover node_modules; lines={lines}"
    )


def test_gitignore_covers_ruff_cache():
    lines = _gitignore_lines()
    assert any(".ruff_cache" in line for line in lines), (
        f".gitignore does not cover .ruff_cache; lines={lines}"
    )


def test_gitignore_covers_pytest_cache():
    lines = _gitignore_lines()
    assert any(".pytest_cache" in line for line in lines), (
        f".gitignore does not cover .pytest_cache; lines={lines}"
    )


def test_gitignore_has_no_runtime_state_paths():
    """Editor runtime-state paths must never appear in this repo's .gitignore."""
    text = _read_text(REPO_ROOT / ".gitignore").lower()
    leaked = [frag for frag in FORBIDDEN_GITIGNORE_FRAGMENTS if frag in text]
    assert not leaked, f".gitignore leaks editor runtime-state paths: {leaked}"


def test_gitattributes_exists():
    assert (REPO_ROOT / ".gitattributes").is_file(), (
        ".gitattributes missing at repo root"
    )


def test_gitattributes_marks_webview2_dlls_binary():
    """vendor/webview2/*.dll must be marked binary (binary macro or -text -diff)."""
    text = _read_text(REPO_ROOT / ".gitattributes")
    binary_marked = False
    for raw in text.splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        # match a rule whose pattern targets vendor/webview2 dll files
        if "webview2" not in line.lower() or ".dll" not in line.lower():
            continue
        attrs = line.lower()
        if "binary" in attrs or ("-text" in attrs and "-diff" in attrs):
            binary_marked = True
            break
    assert binary_marked, (
        ".gitattributes does not mark vendor/webview2/*.dll as binary; "
        f"content=\n{text}"
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
