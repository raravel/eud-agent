"""Verification artifact for the React panel scaffold (EUD-031-01ec).

Validates the static/structural contract of the React panel toolchain per
hivemind/docs/features/03_agent-panel.md ("Toolchain / layout",
"Verification contract"), the decisions 03/04/05 (React rebuild, dist never
committed, Monaco adoption), and the rules "Server and panel" section
(no runtime CDN; dist never committed; Monaco from the npm bundle).

This REVISES the former vanilla-panel contract (element ids + app.js message
handling). Those checks are retired by Decision 03: the vanilla element-id /
app.js / progress-stage assertions move to runtime verification (EUD-034).
The vanilla files ``panel/app.js`` / ``panel/style.css`` are DELETED at the
EUD-035 switchover; this test now asserts they are GONE (a regression guard
against resurrection) and the ``--selfcheck`` panel gate is dist-based. It
otherwise asserts the React-source contract and the no-CDN / no-BOM invariants.

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
# Vanilla-era files deleted at the EUD-035 switchover (regression guard below).
VANILLA_APP_JS = PANEL_DIR / "app.js"
VANILLA_STYLE_CSS = PANEL_DIR / "style.css"
PACKAGE_JSON = PANEL_DIR / "package.json"
SRC_DIR = PANEL_DIR / "src"
COMPONENTS_DIR = PANEL_DIR / "components"
AI_ELEMENTS_DIR = COMPONENTS_DIR / "ai-elements"
UI_DIR = COMPONENTS_DIR / "ui"
DIST_DIR = PANEL_DIR / "dist"
DIST_INDEX_HTML = DIST_DIR / "index.html"

# Panel v2 component contract (features/06_changeset-review-panel.md). The v1
# target-picker / apply-bar flow was REPLACED ENTIRELY (the agent chooses
# files/targets itself); those components are DELETED (EUD-058) and must stay
# gone. The chat-first v2 review UI adds ChangesetView + AgentStream (EUD-059).
PANEL_SRC_COMPONENTS = SRC_DIR / "components"
# v1 components that must be ABSENT (regression guard against resurrection).
V2_ABSENT_COMPONENTS = (
    PANEL_SRC_COMPONENTS / "TargetPicker.tsx",
    PANEL_SRC_COMPONENTS / "ApplyBar.tsx",
)
# v2 components that must be PRESENT.
V2_PRESENT_COMPONENTS = (
    PANEL_SRC_COMPONENTS / "ChangesetView.tsx",
    PANEL_SRC_COMPONENTS / "AgentStream.tsx",
    PANEL_SRC_COMPONENTS / "Header.tsx",
)

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
    # Decision 06: agent-authored markdown renders via Streamdown (npm-bundled,
    # never a runtime CDN). EUD-065 adopts it for the chat surface.
    "streamdown",
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


def test_ai_elements_vendored_present():
    """AI Elements vendored as SOURCE (decision 06, EUD-065).

    History: EUD-031 vendored Vercel AI Elements; EUD-035 dropped them in the
    dep-pruning carry-forward. Decision 06 (2026-06-05) SUPERSEDES that prune —
    the panel chat surface is rebuilt on vendored AI Elements + Streamdown to
    fix the three rendering defects (reasoning invisible / answer faint / raw
    kind leak). The mandatory components (Message, PromptInput, Plan, Reasoning)
    plus the adopted set (Conversation, Response, Tool, Loader) are vendored as
    SOURCE under ``panel/components/ai-elements/`` and committed (never a runtime
    CDN). This asserts the vendored mandatory + adopted source files exist.
    """
    assert AI_ELEMENTS_DIR.is_dir(), (
        f"panel/components/ai-elements/ must be PRESENT (vendored AI Elements "
        f"source, decision 06): {AI_ELEMENTS_DIR}"
    )
    required = (
        "message.tsx",
        "prompt-input.tsx",
        "plan.tsx",
        "reasoning.tsx",
        "conversation.tsx",
        "response.tsx",
        "tool.tsx",
        "loader.tsx",
    )
    missing = [
        name for name in required if not (AI_ELEMENTS_DIR / name).is_file()
    ]
    assert not missing, (
        f"vendored AI Elements source missing (decision 06 mandatory + adopted "
        f"set): {missing}"
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


# --- 5b. vanilla files deleted (regression guard against resurrection) ----


def test_vanilla_app_js_deleted():
    """panel/app.js (dead vanilla file) is removed at the EUD-035 switchover."""
    assert not VANILLA_APP_JS.exists(), (
        f"panel/app.js must be deleted (vanilla panel retired): {VANILLA_APP_JS}"
    )


def test_vanilla_style_css_deleted():
    """panel/style.css (dead vanilla file) is removed at the EUD-035 switchover."""
    assert not VANILLA_STYLE_CSS.exists(), (
        f"panel/style.css must be deleted (vanilla panel retired): "
        f"{VANILLA_STYLE_CSS}"
    )


# --- 5c. panel v2 component contract (absence + presence) -----------------


def test_v1_target_picker_apply_bar_absent():
    """TargetPicker.tsx / ApplyBar.tsx are DELETED (panel v2 full replacement).

    features/06: the v1 target-picker / apply-bar flow is replaced ENTIRELY —
    the agent chooses files/targets itself, so there is no picker and no manual
    apply bar. EUD-058 removed them; this guards against resurrection.
    """
    present = [
        str(p.relative_to(REPO_ROOT)) for p in V2_ABSENT_COMPONENTS if p.exists()
    ]
    assert not present, (
        f"v1 panel components must be ABSENT (panel v2 full replacement, "
        f"features/06): {present}"
    )


def test_v2_components_present():
    """ChangesetView / AgentStream / Header v2 components exist (EUD-059)."""
    missing = [
        str(p.relative_to(REPO_ROOT)) for p in V2_PRESENT_COMPONENTS if not p.is_file()
    ]
    assert not missing, (
        f"panel v2 components missing (features/06 ## Implementation): {missing}"
    )


# --- 6. built dist (skip-aware): no external origins in dist ----

# Forbidden CDN host patterns scanned in the built JS chunks (F4). Matched on the
# HOSTNAME only (not bare words) so benign substrings — W3C namespace URIs in SVG,
# example.com placeholders, repository/homepage strings — do not false-positive.
# Streamdown bundles shiki/mermaid/CJK/math locally; if a future dep regressed to
# fetching a grammar/theme/font from one of these hosts at runtime, this catches it.
FORBIDDEN_CDN_HOST_RE = re.compile(
    r"https?://[A-Za-z0-9.-]*\b("
    r"cdn\.jsdelivr\.net|"
    r"unpkg\.com|"
    r"esm\.sh|"
    r"cdnjs\.cloudflare\.com|"
    r"fonts\.googleapis\.com|"
    r"fonts\.gstatic\.com|"
    r"esm\.run|"
    r"skypack\.dev"
    r")",
    re.IGNORECASE,
)


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


def test_dist_assets_js_has_no_cdn_hosts():
    """The BUILT dist/assets/*.js must not reference any forbidden CDN host (F4).

    The index.html scan above only covers the entry HTML. Streamdown pulls in
    shiki/mermaid/CJK/math; this guards against a transitive dep fetching a
    grammar/theme/font from a CDN at runtime (rules.md: no runtime CDN — every
    asset npm-bundled). Matches on the CDN HOSTNAME only so benign URLs (W3C SVG
    namespaces, example.com, package homepage strings) do not false-positive.

    Skips with a note when panel/dist/ has not been built yet.
    """
    assets_dir = DIST_DIR / "assets"
    if not assets_dir.is_dir():
        _skip(
            "panel/dist/assets/ absent (not built yet); dist is gitignored and "
            "built locally with `npm --prefix panel run build`."
        )
        return  # standalone path falls through after _Skipped is raised
    js_files = sorted(assets_dir.rglob("*.js"))
    assert js_files, "panel/dist/assets/ contains no .js chunks (build incomplete)"
    offenders: list[str] = []
    for js in js_files:
        text = js.read_text(encoding="utf-8", errors="ignore")
        for m in FORBIDDEN_CDN_HOST_RE.finditer(text):
            offenders.append(f"{js.name}: {m.group(0)}")
    assert not offenders, (
        f"built panel/dist/assets/*.js reference forbidden CDN hosts "
        f"(runtime CDN forbidden — all assets must be npm-bundled): {offenders}"
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
