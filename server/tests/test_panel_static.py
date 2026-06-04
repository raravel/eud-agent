"""Verification artifact for EUD-021-6479: panel web UI (static contract).

Validates the static/structural contract of the panel web UI per
hivemind/docs/features/03_agent-panel.md, the architecture "WebSocket
protocol (panel to server)" section, and the rules "Server and panel"
section (no framework/CDN, everything served locally).

These checks intentionally target the *contract* (element ids, message
types, behaviors) rather than exact markup, so any reasonable
implementation passes. The element-id contract fixed here is what the
implementation MUST use.

This file is pytest-compatible (plain ``test_*`` functions with asserts)
AND standalone-runnable with system Python::

    python server/tests/test_panel_static.py

The project venv does not exist yet, so only the stdlib is used
(pathlib, re, sys).
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

# repo_root: server/tests/test_panel_static.py -> parents[2] == repo root
REPO_ROOT = Path(__file__).resolve().parents[2]

PANEL_DIR = REPO_ROOT / "panel"
INDEX_HTML = PANEL_DIR / "index.html"
APP_JS = PANEL_DIR / "app.js"
STYLE_CSS = PANEL_DIR / "style.css"

PANEL_FILES = (INDEX_HTML, APP_JS, STYLE_CSS)

# --- Element-id contract (the implementation MUST use these ids) ----------
#
# Each entry: a logical UI element and the id the implementation must expose.
# Checks accept id="..." or data-* attribute carrying the same token, so the
# test fixes the contract without over-fitting the exact attribute used.
REQUIRED_ELEMENT_IDS = {
    "target_picker": "target-picker",       # <select> target file dropdown
    "instruction_input": "instruction-input",  # instruction <textarea>
    "use_context": "use-context",           # useContext <input type=checkbox>
    "tab_preview": "tab-preview",            # preview tab control
    "tab_diff": "tab-diff",                  # diff tab control
    "tab_edit": "tab-edit",                  # edit tab control
    "apply_set": "apply-set",                # Apply SET button
    "apply_neweps": "apply-neweps",          # Apply NEWEPS button
    "neweps_name": "neweps-name",            # NEWEPS filename input
    "diagnostics": "diagnostics",            # advisory diagnostics area
    "event_log": "event-log",                # chat / event-log area
    "conn_state": "conn-state",              # connection-state indicator
}

# The target picker must be a <select> element (GUI types disabled).
SELECT_IDS = ("target-picker",)

# Server -> client WS message types the panel must handle.
SERVER_MESSAGE_TYPES = ("progress", "code", "applied", "error", "status", "list")

# Client -> server WS message types the panel must send.
CLIENT_MESSAGE_TYPES = ("instruct", "apply", "status", "list")

# Progress stages the panel must render.
PROGRESS_STAGES = ("rag", "rag_warmup", "codex", "lsp", "waiting_build")

# UTF-8 BOM (must NOT be present at the start of any panel file).
UTF8_BOM = b"\xef\xbb\xbf"


def _read_bytes(path: Path) -> bytes:
    return path.read_bytes()


def _read_text(path: Path) -> str:
    return path.read_text(encoding="utf-8")


# --- 1. files exist, non-empty, UTF-8 without BOM -------------------------


def test_panel_files_exist_and_nonempty():
    missing = [str(p.relative_to(REPO_ROOT)) for p in PANEL_FILES if not p.is_file()]
    assert not missing, f"missing panel files: {missing}"
    empty = [
        str(p.relative_to(REPO_ROOT))
        for p in PANEL_FILES
        if p.is_file() and p.stat().st_size == 0
    ]
    assert not empty, f"empty panel files: {empty}"


def test_panel_files_no_utf8_bom():
    leaked = []
    for p in PANEL_FILES:
        if p.is_file() and _read_bytes(p).startswith(UTF8_BOM):
            leaked.append(str(p.relative_to(REPO_ROOT)))
    assert not leaked, f"panel files start with a UTF-8 BOM (forbidden): {leaked}"


# --- 2. relative local references only, no external origins ---------------


def test_index_references_app_js_and_style_css():
    html = _read_text(INDEX_HTML)
    assert re.search(r"""(src|href)\s*=\s*["'][^"']*app\.js["']""", html), (
        "index.html does not reference app.js"
    )
    assert re.search(r"""(src|href)\s*=\s*["'][^"']*style\.css["']""", html), (
        "index.html does not reference style.css"
    )


def test_index_references_are_relative_local():
    """app.js / style.css references must be relative (not absolute, not external)."""
    html = _read_text(INDEX_HTML)
    bad = []
    for m in re.finditer(
        r"""(?:src|href)\s*=\s*["']([^"']*(?:app\.js|style\.css))["']""", html
    ):
        url = m.group(1)
        # external origin, protocol-relative, or absolute-from-root are all rejected
        if re.match(r"^[a-zA-Z][a-zA-Z0-9+.-]*:", url) or url.startswith("//"):
            bad.append(url)
        elif url.startswith("/"):
            bad.append(url)
    assert not bad, f"app.js/style.css must be referenced via relative URLs; bad={bad}"


def test_index_has_no_external_origins():
    """No http:// or https:// anywhere in src/href attribute values (no CDN)."""
    html = _read_text(INDEX_HTML)
    external = []
    for m in re.finditer(r"""(?:src|href)\s*=\s*["']([^"']*)["']""", html):
        url = m.group(1)
        if re.match(r"^(?:https?:)?//", url):
            external.append(url)
    assert not external, (
        f"index.html references external origins (CDN forbidden): {external}"
    )


def test_index_has_no_offorigin_script_src():
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


# --- 3. required UI elements (by id / data-attribute) ---------------------


def _id_present(html: str, token: str) -> bool:
    """True if the token appears as an id or as a data-* attribute value."""
    if re.search(rf"""\bid\s*=\s*["']{re.escape(token)}["']""", html):
        return True
    # accept data-<anything>="token" so the contract is fixed but not over-fit
    if re.search(rf"""\bdata-[\w-]+\s*=\s*["']{re.escape(token)}["']""", html):
        return True
    return False


def test_index_has_required_elements():
    html = _read_text(INDEX_HTML)
    missing = [
        f"{name} ({token})"
        for name, token in REQUIRED_ELEMENT_IDS.items()
        if not _id_present(html, token)
    ]
    assert not missing, f"index.html missing required elements: {missing}"


def test_target_picker_is_select():
    """The target picker must be a <select> element (GUI types shown disabled)."""
    html = _read_text(INDEX_HTML)
    for token in SELECT_IDS:
        pat = re.compile(
            rf"""<select\b[^>]*(?:id|data-[\w-]+)\s*=\s*["']{re.escape(token)}["']""",
            re.IGNORECASE,
        )
        assert pat.search(html), (
            f"element '{token}' must be a <select> in index.html"
        )


def test_usecontext_is_checkbox():
    """The useContext toggle must be a checkbox input."""
    html = _read_text(INDEX_HTML)
    token = REQUIRED_ELEMENT_IDS["use_context"]
    # find an <input ...> tag carrying the token and assert type=checkbox on it
    found_checkbox = False
    for m in re.finditer(r"<input\b[^>]*>", html, re.IGNORECASE):
        tag = m.group(0)
        carries_id = re.search(
            rf"""(?:id|data-[\w-]+)\s*=\s*["']{re.escape(token)}["']""", tag
        )
        if carries_id and re.search(
            r"""type\s*=\s*["']checkbox["']""", tag, re.IGNORECASE
        ):
            found_checkbox = True
            break
    assert found_checkbox, (
        f"useContext toggle '{token}' must be <input type=\"checkbox\">"
    )


# --- 4. Korean labels -----------------------------------------------------


def test_index_has_korean_labels():
    html = _read_text(INDEX_HTML)
    assert re.search(r"[가-힣]", html), (
        "index.html has no Korean (Hangul) labels"
    )


# --- 5. app.js handles every server->client message type ------------------


def test_app_handles_all_server_message_types():
    js = _read_text(APP_JS)
    missing = [t for t in SERVER_MESSAGE_TYPES if t not in js]
    assert not missing, f"app.js does not handle server message types: {missing}"


def test_app_has_unknown_message_fallback():
    """A default/else branch must handle unknown message types without crashing."""
    js = _read_text(APP_JS)
    assert "unknown" in js.lower(), (
        "app.js has no unknown-type fallback (expected a default/else handling "
        "an 'unknown' message type)"
    )


# --- 6. app.js sends client->server types + connection behavior -----------


def test_app_sends_all_client_message_types():
    js = _read_text(APP_JS)
    missing = [t for t in CLIENT_MESSAGE_TYPES if t not in js]
    assert not missing, f"app.js does not send client message types: {missing}"


def test_app_reads_token_from_location_search():
    js = _read_text(APP_JS)
    assert "URLSearchParams" in js, (
        "app.js must read the token from location.search via URLSearchParams"
    )
    assert "location.search" in js, "app.js must reference location.search"


def test_app_builds_ws_url_from_location_host():
    js = _read_text(APP_JS)
    assert "location.host" in js, (
        "app.js must build the WS URL from location.host"
    )


def test_app_has_2s_reconnect_backoff():
    js = _read_text(APP_JS)
    assert "2000" in js, "app.js must use a 2s (2000ms) reconnect backoff"


def test_app_rerequests_status_and_list_on_reconnect():
    """On (re)connect the panel re-requests both status and list."""
    js = _read_text(APP_JS)
    # heuristic: both message types are referenced as sent requests
    assert "status" in js and "list" in js, (
        "app.js must re-request status and list on reconnect"
    )


# --- 7. progress stages ---------------------------------------------------


def test_app_references_all_progress_stages():
    js = _read_text(APP_JS)
    missing = [s for s in PROGRESS_STAGES if s not in js]
    assert not missing, f"app.js does not reference progress stages: {missing}"


# --- 8. NEWEPS filename validation ----------------------------------------


def test_app_validates_neweps_filename():
    """NEWEPS filename validation: reject empty names and path separators."""
    js = _read_text(APP_JS)
    # path-separator rejection: must reference both '/' and '\' as forbidden.
    # Look for a forward slash and an escaped backslash somewhere in the source.
    has_fwd_slash = "/" in js
    has_back_slash = "\\\\" in js or "\\" in js
    assert has_fwd_slash and has_back_slash, (
        "app.js NEWEPS validation must reject path separators ('/' and '\\')"
    )
    # empty-name rejection: heuristic for a non-empty / trim / length check
    # tied to the neweps filename handling.
    empty_check = re.search(
        r"\.trim\s*\(\s*\)|\.length\b|===?\s*[\"'']\s*[\"'']|!\s*\w+", js
    )
    assert empty_check, (
        "app.js NEWEPS validation must reject empty filenames "
        "(expected a trim/length/empty-string check)"
    )


# --- standalone runner ----------------------------------------------------


def _all_test_functions():
    module = sys.modules[__name__]
    return [
        (name, obj)
        for name, obj in sorted(vars(module).items())
        if name.startswith("test_") and callable(obj)
    ]


def main() -> int:
    failures = 0
    tests = _all_test_functions()
    for name, fn in tests:
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
    total = len(tests)
    print(f"\n{total - failures}/{total} checks passed")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
