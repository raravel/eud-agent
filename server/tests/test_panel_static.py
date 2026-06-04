"""Verification artifact for the React panel scaffold (EUD-031-01ec).

Validates the static/structural contract of the React panel toolchain per
hivemind/docs/features/03_agent-panel.md ("Toolchain / layout",
"Verification contract"), the decisions 03/04/05 (React rebuild, dist never
committed, Monaco adoption), and the rules "Server and panel" section
(no runtime CDN; dist never committed; Monaco from the npm bundle).

This REVISES the former vanilla-panel contract (element ids + app.js message
handling). Those checks are retired by Decision 03: the vanilla element-id /
app.js / progress-stage assertions move to runtime verification (EUD-034).
Per the SEQUENCING NOTE in EUD-031-01ec, ``panel/index.html`` becomes the
Vite template now, while ``panel/app.js`` / ``panel/style.css`` remain as DEAD
files until EUD-035 (the ``--selfcheck`` PANEL_FILES gate still requires all
three). This test therefore neither requires nor forbids app.js/style.css; it
asserts the React-source contract and the no-CDN / no-BOM invariants.

These checks target the *contract* (toolchain manifest, vendored component
source, gitignore, no external origins) rather than exact markup, so any
reasonable implementation passes.

This file is pytest-compatible (plain ``test_*`` functions with asserts)
AND standalone-runnable with system Python::

    python server/tests/test_panel_static.py

Only the stdlib is used (json, re, sys, pathlib) so it runs before the
project venv exists. Checks that require a build (dist content scan) SKIP with
a note when ``panel/dist/`` is absent.

Before the scaffold lands the React checks FAIL (no package.json yet); the
no-BOM / gitignore-file checks may partially pass; overall exit is non-zero.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

# repo_root: server/tests/test_panel_static.py -> parents[2] == repo root
REPO_ROOT = Path(__file__).resolve().parents[2]

PANEL_DIR = REPO_ROOT / "panel"
INDEX_HTML = PANEL_DIR / "index.html"            # Vite template
PACKAGE_JSON = PANEL_DIR / "package.json"
SRC_DIR = PANEL_DIR / "src"
COMPONENTS_DIR = PANEL_DIR / "components"
AI_ELEMENTS_DIR = COMPONENTS_DIR / "ai-elements"
UI_DIR = COMPONENTS_DIR / "ui"
DIST_DIR = PANEL_DIR / "dist"
DIST_INDEX_HTML = DIST_DIR / "index.html"

GITIGNORE = REPO_ROOT / ".gitignore"

# npm dependencies the committed React stack must declare (any of
# dependencies / devDependencies). These are the toolchain anchors:
# React runtime, the Vite build tool, Tailwind v4, and the Monaco bundle.
REQUIRED_NPM_DEPS = (
    "react",
    "react-dom",
    "vite",
    "tailwindcss",
    "monaco-editor",
    "@monaco-editor/react",
)

# npm scripts the scaffold must expose.
REQUIRED_NPM_SCRIPTS = ("dev", "build", "preview")

# Key text files that must not carry a UTF-8 BOM (BOM breaks Vite template
# parsing / first-line tooling and violates the no-BOM rule).
NO_BOM_FILES = (INDEX_HTML, PACKAGE_JSON)

# UTF-8 BOM (must NOT be present at the start of the listed files).
UTF8_BOM = b"\xef\xbb\xbf"


# --- skip plumbing (dual-mode: pytest skip OR standalone SKIP note) -------


class _Skipped(Exception):
    """Raised to signal a SKIP in standalone mode (dist absent, etc.)."""


def _skip(reason: str) -> None:
    """Skip the current check, recording a note in either run mode."""
    try:
        import pytest  # type: ignore
    except Exception:
        raise _Skipped(reason) from None
    pytest.skip(reason)


def _read_bytes(path: Path) -> bytes:
    return path.read_bytes()


def _read_text(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def _load_package_json() -> dict:
    assert PACKAGE_JSON.is_file(), (
        f"panel/package.json missing (React scaffold not present): {PACKAGE_JSON}"
    )
    try:
        return json.loads(_read_text(PACKAGE_JSON))
    except json.JSONDecodeError as exc:  # pragma: no cover - corruption guard
        raise AssertionError(f"panel/package.json is not valid JSON: {exc}") from exc


def _all_declared_deps(pkg: dict) -> dict:
    deps: dict = {}
    for key in ("dependencies", "devDependencies", "peerDependencies"):
        section = pkg.get(key)
        if isinstance(section, dict):
            deps.update(section)
    return deps


def _gitignore_lines() -> list[str]:
    """Non-empty, non-comment .gitignore lines (stripped, original case)."""
    lines: list[str] = []
    for raw in _read_text(GITIGNORE).splitlines():
        stripped = raw.strip()
        if not stripped or stripped.startswith("#"):
            continue
        lines.append(stripped)
    return lines


def _ignores(path_fragment: str) -> bool:
    """True if some .gitignore line targets the given path fragment.

    Accepts the bare fragment, a trailing-slash dir form, and a leading-slash
    anchored form (e.g. ``panel/dist``, ``panel/dist/``, ``/panel/dist/``).
    Backslashes are normalized to forward slashes.
    """
    target = path_fragment.strip("/")
    for line in _gitignore_lines():
        norm = line.replace("\\", "/").strip("/")
        if norm == target:
            return True
    return False


# --- 1. package.json: deps + scripts (React toolchain manifest) -----------


def test_package_json_exists_and_valid():
    pkg = _load_package_json()
    assert isinstance(pkg, dict), "panel/package.json must be a JSON object"


def test_package_json_declares_required_deps():
    """react/react-dom/vite/tailwindcss/monaco-editor/@monaco-editor/react present."""
    pkg = _load_package_json()
    deps = _all_declared_deps(pkg)
    missing = [d for d in REQUIRED_NPM_DEPS if d not in deps]
    assert not missing, (
        f"panel/package.json missing required deps: {missing}; declared={sorted(deps)}"
    )


def test_package_json_has_dev_build_preview_scripts():
    pkg = _load_package_json()
    scripts = pkg.get("scripts", {})
    assert isinstance(scripts, dict), "package.json 'scripts' must be an object"
    missing = [s for s in REQUIRED_NPM_SCRIPTS if s not in scripts]
    assert not missing, (
        f"panel/package.json missing scripts: {missing}; have={sorted(scripts)}"
    )


# --- 2. source + vendored component structure -----------------------------


def test_src_dir_present():
    assert SRC_DIR.is_dir(), f"panel/src/ missing (React app sources): {SRC_DIR}"


def test_vendored_ai_elements_present():
    """Vercel AI Elements vendored as SOURCE (no runtime registry/CDN)."""
    assert AI_ELEMENTS_DIR.is_dir(), (
        f"panel/components/ai-elements/ missing (vendored AI Elements source): "
        f"{AI_ELEMENTS_DIR}"
    )
    sources = [
        p
        for p in AI_ELEMENTS_DIR.rglob("*")
        if p.is_file() and p.suffix in (".tsx", ".ts", ".jsx", ".js")
    ]
    assert sources, (
        "panel/components/ai-elements/ contains no component source files"
    )


def test_vendored_shadcn_ui_present():
    """shadcn/ui primitives vendored as SOURCE under panel/components/ui/."""
    assert UI_DIR.is_dir(), (
        f"panel/components/ui/ missing (vendored shadcn/ui source): {UI_DIR}"
    )
    sources = [
        p
        for p in UI_DIR.rglob("*")
        if p.is_file() and p.suffix in (".tsx", ".ts", ".jsx", ".js")
    ]
    assert sources, "panel/components/ui/ contains no component source files"


# --- 3. gitignore: dist + node_modules never committed --------------------


def test_gitignore_ignores_panel_dist():
    assert _ignores("panel/dist"), (
        ".gitignore must ignore panel/dist/ (build output never committed); "
        f"lines={_gitignore_lines()}"
    )


def test_gitignore_ignores_panel_node_modules():
    """panel/node_modules must be ignored (a bare node_modules rule suffices)."""
    lines = [ln.replace("\\", "/") for ln in _gitignore_lines()]
    ignored = any(
        ln.strip("/") in ("panel/node_modules", "node_modules")
        for ln in lines
    )
    assert ignored, (
        ".gitignore must ignore panel/node_modules/ (or node_modules globally); "
        f"lines={_gitignore_lines()}"
    )


# --- 4. no-BOM on key text files ------------------------------------------


def test_key_text_files_no_utf8_bom():
    leaked = []
    for p in NO_BOM_FILES:
        if p.is_file() and _read_bytes(p).startswith(UTF8_BOM):
            leaked.append(str(p.relative_to(REPO_ROOT)))
    assert not leaked, f"files start with a UTF-8 BOM (forbidden): {leaked}"


# --- 5. Vite template (index.html): no external origins -------------------


def test_vite_template_present():
    assert INDEX_HTML.is_file(), (
        f"panel/index.html (Vite template) missing: {INDEX_HTML}"
    )


def test_vite_template_has_no_external_origins():
    """No http(s):// or protocol-relative URL in any src/href attribute value.

    Every byte must ship from the local server; the Vite template references
    only local module entrypoints / relative assets.
    """
    html = _read_text(INDEX_HTML)
    external = []
    for m in re.finditer(r"""(?:src|href)\s*=\s*["']([^"']*)["']""", html):
        url = m.group(1)
        if re.match(r"^(?:https?:)?//", url):
            external.append(url)
    assert not external, (
        f"panel/index.html references external origins (CDN forbidden): {external}"
    )


def test_vite_template_no_offorigin_script_src():
    """No <script src=...> pointing to an http(s) or protocol-relative origin."""
    html = _read_text(INDEX_HTML)
    offorigin = []
    for m in re.finditer(
        r"""<script\b[^>]*\bsrc\s*=\s*["']([^"']*)["']""", html, re.IGNORECASE
    ):
        url = m.group(1)
        if re.match(r"^(?:https?:)?//", url):
            offorigin.append(url)
    assert not offorigin, f"off-origin <script src>: {offorigin}"


# --- 6. built dist (skip-aware): no external origins in dist/index.html ----


def test_dist_index_has_no_external_origins():
    """The BUILT dist/index.html must reference zero external origins.

    Skips with a note when panel/dist/ has not been built yet (dev machines
    build locally; dist is never committed).
    """
    if not DIST_INDEX_HTML.is_file():
        _skip(
            "panel/dist/index.html absent (not built yet); dist is gitignored "
            "and built locally with `npm --prefix panel run build`."
        )
        return  # standalone path falls through after _Skipped is raised
    html = _read_text(DIST_INDEX_HTML)
    external = []
    for m in re.finditer(r"""(?:src|href)\s*=\s*["']([^"']*)["']""", html):
        url = m.group(1)
        if re.match(r"^(?:https?:)?//", url):
            external.append(url)
    assert not external, (
        f"built panel/dist/index.html references external origins "
        f"(CDN forbidden, incl. Monaco workers): {external}"
    )


# --- standalone runner ----------------------------------------------------


def _all_test_functions():
    module = sys.modules[__name__]
    return [
        (name, obj)
        for name, obj in sorted(vars(module).items())
        if name.startswith("test_") and callable(obj)
    ]


def _skip_exc_types() -> tuple[type[BaseException], ...]:
    """Skip exception types to treat as SKIP in the standalone runner.

    Always includes the local ``_Skipped`` sentinel; also includes pytest's
    ``Skipped`` when pytest is importable, since ``_skip`` defers to
    ``pytest.skip`` in that case.
    """
    types: list[type[BaseException]] = [_Skipped]
    try:
        from _pytest.outcomes import Skipped  # type: ignore

        types.append(Skipped)
    except Exception:
        pass
    return tuple(types)


def main() -> int:
    failures = 0
    skipped = 0
    skip_types = _skip_exc_types()
    tests = _all_test_functions()
    for name, fn in tests:
        try:
            fn()
        except skip_types as exc:
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
    total = len(tests)
    passed = total - failures - skipped
    print(f"\n{passed}/{total} checks passed ({skipped} skipped, {failures} failed)")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
