"""Verification artifact for EUD-014-1b4e: bridge WebView2 panel hosting (static).

Static source checks against ``bridge/ZZZ_10_agent_bridge.lua`` for the WebView2
panel-hosting extension that REPLACES the v6 WPF 4-button control panel
(hivemind/docs/features/01_lua-bridge.md "WebView2 panel" + "Removal /
unchanged", architecture.md "Boot and lifecycle" flowchart, rules.md WebView2
rules + Lua crash rules + the "luanet static proxy" caution):

  1. WebView2 assemblies load + types import: ``Microsoft.Web.WebView2.Core``
     and ``Microsoft.Web.WebView2.Wpf`` loaded (app-base probing -- plain
     ``luanet.load_assembly("Microsoft.Web.WebView2.Wpf")`` etc.), and the
     ``WebView2`` control type imported.
  2. UserDataFolder: a ``UserDataFolder`` assignment whose value contains
     ``webview2`` under the agent dir; NEVER the default next-to-exe (no default
     UDF -- rules.md hard rule).
  3. Core init: an ``EnsureCoreWebView2Async`` call AND a
     ``CoreWebView2InitializationCompleted`` subscription.
  4. NavigationCompleted: a ``NavigationCompleted`` subscription AND a
     re-navigate / backoff marker (a 3-second constant) AND a navOk-style flag
     (WebView2 never auto-retries -- rules.md).
  5. Navigate URL: a ``Navigate`` call building the URL from the EUD-013
     lifecycle globals ``agentSrvPort`` and ``agentSrvToken`` -- the literal
     ``127.0.0.1`` host and the ``?token=`` query.
  6. Re-arm: a Tick-region window-alive tracking marker (a bare-global window
     handle + recreate-when-dead logic), NOT a ``pjData==nil``-only re-arm
     (the editor closes auxiliary windows on project create/switch -- rules.md).
  7. PANEL no longer builds the WPF StackPanel: ``mkBtn`` / ``StackPanel`` MUST
     be absent from the file (v6 had them in ``showPanel``; the WPF panel +
     auto-show + ``panelShown`` flag are REMOVED per spec), and the PANEL branch
     references the WebView2 window show/focus (Show/Activate) instead.
  8. Regression: ALL v6 + LIST + NEWEPS command branches remain; the EUD-013
     lifecycle markers (heartbeat write, ProcessStartInfo spawn) still present;
     no ``os.execute`` / ``io.popen`` / ``:GetValue(`` anywhere; non-ASCII byte
     count must not GROW over the current baseline (the WPF-panel removal will
     LOWER it later -- ``<=`` handles both before and after).

This file is pytest-compatible (plain ``test_*`` functions with asserts)
AND standalone-runnable with system Python::

    python server/tests/test_bridge_webview_static.py

The project venv does not exist yet, so only the stdlib is used.

Checks 1-7 FAIL before the WebView2 panel is implemented (check 7 fails because
``mkBtn`` / ``StackPanel`` are still present in the v6 ``showPanel``); check 8
passes throughout (regression guard).
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

# repo_root: server/tests/test_bridge_webview_static.py -> parents[2] == repo root
REPO_ROOT = Path(__file__).resolve().parents[2]

BRIDGE = REPO_ROOT / "bridge" / "ZZZ_10_agent_bridge.lua"

# v6 command markers + LIST (EUD-011) + NEWEPS (EUD-012) that must survive
# import-then-extend. Each is matched as a dispatcher branch ``cmd == "<NAME>"``
# so a stray substring elsewhere cannot satisfy the check.
ALL_COMMANDS = (
    "PING",
    "STATUS",
    "LIST",
    "DUMP",
    "GET",
    "SET",
    "NEWEPS",
    "GETDAT",
    "SETDAT",
    "BUILD",
    "LUA",
    "PANEL",
)

# Known non-ASCII byte count in the current source (Korean mojibake in comments
# + WPF panel UI strings + Korean error messages). Computed from the file on
# disk at task start: 1,263 bytes (unchanged from the v6 / LIST / NEWEPS /
# lifecycle baselines -- all those extensions were ASCII-only). The WebView2
# extension keeps the ASCII window title "EUD Agent"; removing the v6 WPF panel
# strings will LOWER this count, so the guard is ``<=`` (handles both states).
BASELINE_NONASCII_BYTES = 1_263


def _read_text() -> str:
    # latin-1 round-trips every byte 1:1, matching how KopiLua reads the source.
    return BRIDGE.read_bytes().decode("latin-1")


def _branch_re(name: str) -> re.Pattern[str]:
    """Match a dispatcher branch comparing ``cmd`` to a command name."""
    return re.compile(r'cmd\s*==\s*"' + re.escape(name) + r'"')


def _tick_region(text: str) -> str:
    """Source region of the DispatcherTimer Tick handler.

    From ``timer.Tick:Add`` to the following ``timer:Start()`` -- the span where
    the per-Tick lifecycle + re-arm logic lives.
    """
    start = re.search(r"timer\.Tick:Add", text)
    assert start, "no `timer.Tick:Add` handler found"
    end = re.search(r"timer:Start\(\)", text[start.start():])
    assert end, "no `timer:Start()` after the Tick handler"
    return text[start.start(): start.start() + end.start()]


def _panel_branch_region(text: str) -> str:
    """Source region of the ``cmd == "PANEL"`` dispatcher branch body.

    From the PANEL branch to the next ``elseif cmd ==`` so the markers are
    attributable to PANEL rather than to a neighboring branch.
    """
    m = _branch_re("PANEL").search(text)
    assert m, 'PANEL branch missing'
    region = text[m.start():]
    nxt = re.search(r"\n\s*elseif cmd ==", region[1:])
    if nxt:
        region = region[: nxt.start() + 1]
    return region


def _strip_comments(src: str) -> str:
    """Drop Lua line comments so markers match CODE, not prose.

    Removes everything from a ``--`` to the end of each line. (The bridge has no
    ``--[[ ]]`` block comments; line comments are sufficient and keep this a
    pure-stdlib heuristic.) Line structure is preserved so positional ordering
    in callers stays meaningful.
    """
    out = []
    for line in src.splitlines(keepends=True):
        idx = line.find("--")
        if idx != -1:
            nl = line[len(line.rstrip("\r\n")):]  # preserve the trailing newline
            out.append(line[:idx] + nl)
        else:
            out.append(line)
    return "".join(out)


def _function_body(text: str, name: str) -> str:
    """Bounded body of ``local function <name>(...)`` up to the matching ``end``.

    Balances Lua block openers (function/if/for/while/do) against ``end`` from
    the function header so markers are attributable to THIS function, not the
    file at large. Comments are stripped first so ``end`` inside a comment can't
    skew the balance.
    """
    code = _strip_comments(text)
    m = re.search(r"local\s+function\s+" + re.escape(name) + r"\b", code)
    assert m, f"no `local function {name}` found"
    i = m.end()
    # Token scan from the function header, tracking block depth (the function
    # header itself opens depth 1). `for`/`while` open a block whose `do` is part
    # of the SAME block (one `end`), so the `do` closing a loop header must not
    # add depth -- track a pending loop-header to swallow exactly that `do`.
    depth = 1
    pending_loop = False
    token = re.compile(r"\b(function|if|for|while|do|end)\b")
    pos = i
    while depth > 0:
        t = token.search(code, pos)
        if not t:
            break
        word = t.group(1)
        if word in ("for", "while"):
            depth += 1
            pending_loop = True
        elif word == "do":
            if pending_loop:
                pending_loop = False  # this `do` belongs to the loop header
            else:
                depth += 1  # standalone do ... end block
        elif word in ("function", "if"):
            depth += 1
        elif word == "end":
            depth -= 1
        pos = t.end()
        if depth == 0:
            return code[i:t.start()]
    return code[i:pos]


# --------------------------------------------------------------------------
# baseline
# --------------------------------------------------------------------------
def test_bridge_file_present_and_nonempty():
    assert BRIDGE.is_file(), f"missing file: {BRIDGE}"
    assert BRIDGE.stat().st_size > 0, f"empty file: {BRIDGE}"


# --------------------------------------------------------------------------
# 1. WebView2 assemblies load + control type import
# --------------------------------------------------------------------------
def test_webview2_assemblies_loaded():
    """Core + Wpf WebView2 assemblies are loaded (app-base probing)."""
    text = _read_text()
    assert re.search(
        r'load_assembly\s*\(\s*["\']Microsoft\.Web\.WebView2\.Core["\']', text
    ), "Microsoft.Web.WebView2.Core assembly is not loaded"
    assert re.search(
        r'load_assembly\s*\(\s*["\']Microsoft\.Web\.WebView2\.Wpf["\']', text
    ), "Microsoft.Web.WebView2.Wpf assembly is not loaded"


def test_webview2_control_type_imported():
    """The WebView2 control type is imported via luanet.import_type."""
    text = _read_text()
    # The Wpf WebView2 control: Microsoft.Web.WebView2.Wpf.WebView2
    assert re.search(
        r'import_type\s*\(\s*["\']Microsoft\.Web\.WebView2\.Wpf\.WebView2["\']', text
    ), "the WebView2 control type (Microsoft.Web.WebView2.Wpf.WebView2) is not imported"


# --------------------------------------------------------------------------
# 2. UserDataFolder under the agent dir (never the default next-to-exe)
# --------------------------------------------------------------------------
def test_userdatafolder_under_agent_dir():
    """A UserDataFolder is set to a 'webview2' path under the agent dir."""
    text = _read_text()
    assert "UserDataFolder" in text, (
        "no UserDataFolder assignment (WebView2 must NOT use the default UDF "
        "next to the editor exe -- rules.md hard rule)"
    )
    # The UDF value must mention 'webview2'. Heuristic: a UserDataFolder
    # assignment whose RHS (same line) contains the 'webview2' token.
    assert re.search(r"UserDataFolder\s*=\s*[^\n]*webview2", text, re.I), (
        "UserDataFolder must point at a 'webview2' path under Data\\agent "
        "(never the default next-to-exe location)"
    )


def test_creation_properties_set():
    """CreationProperties carries the explicit UserDataFolder (probe11 path)."""
    text = _read_text()
    assert "CreationProperties" in text, (
        "no CreationProperties (the UserDataFolder must be set via "
        "CoreWebView2CreationProperties before EnsureCoreWebView2Async)"
    )


def test_creation_properties_imported_from_wpf_namespace():
    """CoreWebView2CreationProperties is imported from the Wpf namespace.

    Reflection over the vendored DLLs (EUD-038): the Core assembly exports NO
    ``*CreationProperties*`` type; ``Microsoft.Web.WebView2.Wpf.dll`` exports
    ``Microsoft.Web.WebView2.Wpf.CoreWebView2CreationProperties`` (and the WPF
    control's ``CreationProperties`` property is typed in that namespace). The
    bridge MUST import the FULL Wpf-namespaced type name, else ``import_type``
    returns nil and ``createPanel()`` dies every Tick with "attempt to call
    upvalue 'CoreWebView2CreationProperties' (a nil value)".

    The ``import_type`` call is line-wrapped (the argument sits on the line
    after the assignment), so match against comment-stripped, whitespace-
    collapsed text -- per this file's conventions -- to span both source lines.
    """
    code = _strip_comments(_read_text())
    flat = re.sub(r"\s+", " ", code)
    assert re.search(
        r'import_type\s*\(\s*["\']'
        r"Microsoft\.Web\.WebView2\.Wpf\.CoreWebView2CreationProperties"
        r'["\']\s*\)',
        flat,
    ), (
        "CoreWebView2CreationProperties must be imported from the Wpf namespace "
        '(import_type("Microsoft.Web.WebView2.Wpf.CoreWebView2CreationProperties"))'
        "; the Core assembly does not export this type and import_type returns "
        "nil there -- createPanel() crashes every Tick (EUD-038)"
    )


def test_creation_properties_not_imported_from_core_namespace():
    """Regression guard: the Core-namespaced type name must NOT appear.

    ``Microsoft.Web.WebView2.Core.CoreWebView2CreationProperties`` is the broken
    import that returns nil -- it must not survive anywhere in the bridge (code
    or comment), or ``createPanel()`` regresses to the per-Tick nil-upvalue
    crash (EUD-038).
    """
    text = _read_text()
    assert (
        "Microsoft.Web.WebView2.Core.CoreWebView2CreationProperties" not in text
    ), (
        "the Core-namespaced CoreWebView2CreationProperties import is still "
        "present; import_type returns nil for it (the type lives in the Wpf "
        "assembly, not Core) -- createPanel() crashes every Tick (EUD-038)"
    )


# --------------------------------------------------------------------------
# 3. EnsureCoreWebView2Async + CoreWebView2InitializationCompleted
# --------------------------------------------------------------------------
def test_ensure_core_webview2_async_called():
    """The control's EnsureCoreWebView2Async is invoked."""
    text = _read_text()
    assert "EnsureCoreWebView2Async" in text, (
        "no EnsureCoreWebView2Async call (CoreWebView2 must be initialized "
        "before Navigate)"
    )


def test_core_init_completed_subscription():
    """CoreWebView2InitializationCompleted is subscribed (success -> Navigate)."""
    text = _read_text()
    assert "CoreWebView2InitializationCompleted" in text, (
        "no CoreWebView2InitializationCompleted subscription (Navigate must "
        "happen on init success)"
    )


def test_new_window_requested_opens_default_browser():
    """Citation links (EUD-090) raise NewWindowRequested; the bridge must mark
    it Handled (no WebView2 popup; the panel never navigates away) and
    shell-open the http(s) uri in the user's default browser."""
    text = _read_text()
    assert "NewWindowRequested" in text, (
        "no NewWindowRequested subscription (evidence citation links render "
        "target=_blank; without a handler WebView2 spawns its own popup)"
    )
    assert re.search(r"Handled\s*=\s*true", text), (
        "NewWindowRequested must set Handled=true (suppress the WebView2 popup)"
    )
    assert re.search(r"UseShellExecute\s*=\s*true", text), (
        "the uri must be shell-opened (UseShellExecute=true -> default browser)"
    )
    assert '"http://' in text and '"https://' in text, (
        "the handler must whitelist http(s) uris only"
    )


# --------------------------------------------------------------------------
# 4. NavigationCompleted subscription + re-navigate backoff + navOk flag
# --------------------------------------------------------------------------
def test_navigation_completed_subscription():
    """NavigationCompleted is subscribed (IsSuccess gates the re-navigate)."""
    text = _read_text()
    assert "NavigationCompleted" in text, (
        "no NavigationCompleted subscription (WebView2 never auto-retries; the "
        "bridge must re-Navigate on IsSuccess==false)"
    )
    assert "IsSuccess" in text, (
        "no IsSuccess check on NavigationCompleted (cannot detect a failed nav)"
    )


def test_navigation_retry_backoff_and_navok_flag():
    """A 3-second re-navigate backoff constant AND a navOk-style flag exist."""
    text = _read_text()
    # navOk-style flag: a bare 'navOk' marker (the spec names it navOk).
    assert re.search(r"\bnavOk\b", text), (
        "no navOk-style flag (NavigationCompleted IsSuccess==false must set a "
        "navOk flag that drives the later re-Navigate)"
    )
    # 3-second backoff constant. Accept a bare 3 used as a seconds backoff or
    # 3000 ms. Require a standalone 3 (the spec's '3s backoff').
    assert re.search(r"\b3\b", text) or re.search(r"\b3000\b", text), (
        "no 3-second re-navigate backoff constant"
    )


# --------------------------------------------------------------------------
# 5. Navigate URL from agentSrvPort + agentSrvToken (EUD-013 globals)
# --------------------------------------------------------------------------
def test_navigate_url_from_lifecycle_globals():
    """Navigate builds the URL from agentSrvPort + agentSrvToken (?token=)."""
    text = _read_text()
    assert "Navigate" in text, "no Navigate call (the panel never loads the URL)"
    assert "127.0.0.1" in text, (
        "the Navigate URL must target the literal 127.0.0.1 loopback host"
    )
    assert "?token=" in text, (
        "the Navigate URL must carry the ?token= query (server.ready token)"
    )
    # The URL must be built from the EUD-013 lifecycle globals, not a hardcoded
    # port/token.
    assert "agentSrvPort" in text, (
        "the Navigate URL port must come from the agentSrvPort lifecycle global"
    )
    assert "agentSrvToken" in text, (
        "the Navigate URL token must come from the agentSrvToken lifecycle global"
    )
    # CoreWebView2 is a plain property: access it via dot (`.CoreWebView2`), and
    # call Navigate on it with a colon. rules.md reserves the `get_X()` form for
    # PARAMETERIZED properties only (Finding F3).
    assert re.search(r"\.CoreWebView2\s*:\s*Navigate\s*\(", text), (
        "Navigate must be called via plain property access "
        "(panelView.CoreWebView2:Navigate(...)); rules.md reserves get_X() for "
        "parameterized properties (Finding F3)"
    )
    assert "get_CoreWebView2" not in text, (
        "CoreWebView2 is a plain property; do not use the parameterized-property "
        "getter form get_CoreWebView2() (Finding F3)"
    )


def test_panel_creation_gated_on_server_ready():
    """createPanel / navigatePanel / showPanel each guard on agentSrvReady.

    A file-wide substring is too weak: the gate must appear in EACH panel
    function body (bounded extraction) so the server-ready precondition cannot
    be satisfied by a single unrelated mention.
    """
    text = _read_text()
    assert "agentSrvReady" in text, (
        "panel creation must be gated on the agentSrvReady lifecycle global "
        "(never create/navigate before the server is ready)"
    )
    for fn in ("createPanel", "navigatePanel", "showPanel"):
        body = _function_body(text, fn)
        assert "agentSrvReady" in body, (
            f"{fn}() does not guard on agentSrvReady (each panel function must "
            f"early-return / gate when the server is not ready)"
        )
        # The guard must be an early-return / negative check, not an incidental
        # mention: `not agentSrvReady` (createPanel/navigatePanel/showPanel all
        # use the `if not agentSrvReady then` form).
        assert re.search(r"not\s+agentSrvReady", body), (
            f"{fn}() references agentSrvReady but not as a `not agentSrvReady` "
            f"guard (expected an early-return when the server is not ready)"
        )


# --------------------------------------------------------------------------
# 6. re-arm: window-alive tracking in the Tick (not pjData==nil only)
# --------------------------------------------------------------------------
def test_window_handle_tracked_in_global():
    """The WebView2 window object/handle is retained in a bare global (GC guard)."""
    code = _strip_comments(_read_text())
    # A bare-global assignment (no `local`) of the panel window, mirroring the
    # agentProc / agentSrv* idiom (rules.md: keep state in bare Lua globals, not
    # on the luanet static proxy). Matched on stripped code so a comment cannot
    # satisfy it.
    assert re.search(
        r"(?m)^(?!\s*local\b)\s*[A-Za-z_][\w]*[Ww]in(dow)?\b\s*=", code
    ) or re.search(
        r"(?m)^(?!\s*local\b)\s*agentPanel\w*\s*=", code
    ), (
        "the panel Window is not retained in a bare global (needed as GC guard "
        "+ alive-tracking source for re-arm; rules.md 'luanet static proxy' "
        "caution -- keep state in bare Lua globals)"
    )


def test_rearm_window_alive_tracking_in_tick():
    """The Tick drives re-arm via a window-handle recreate (not pjData==nil-only).

    Matches CODE (comments stripped): the Tick must CALL the maintain/re-arm
    routine, and that routine's body must recreate on a dead window
    (``panelWin == nil`` -> ``createPanel``) -- window-handle tracking, not a
    pjData==nil-only re-arm (the editor closes aux windows on project switch;
    rules.md).
    """
    text = _read_text()
    region = _strip_comments(_tick_region(text))
    # The Tick must invoke the per-Tick panel maintainer (the re-arm entry
    # point), either directly (`maintainPanel()`) or via the file's established
    # pcall-isolation idiom (`pcall(maintainPanel)`). A bare comment no longer
    # satisfies this.
    maintain_call = (
        re.search(r"\bmaintainPanel\s*\(", region)
        or re.search(r"\bpcall\s*\(\s*maintainPanel\b", region)
        or re.search(r"\b(ensurePanel|recreatePanel|createPanel)\s*\(", region)
    )
    assert maintain_call, (
        "the Tick does not CALL the panel maintainer (maintainPanel / "
        "pcall(maintainPanel) / ensurePanel / createPanel) -- re-arm must run "
        "every Tick"
    )
    # The maintainer body must recreate the window when the tracked handle is
    # dead: `panelWin == nil` -> createPanel(). Matched on stripped code.
    body = _function_body(text, "maintainPanel")
    assert re.search(r"panelWin\s*==\s*nil", body), (
        "maintainPanel() does not test `panelWin == nil` (window-handle alive "
        "tracking is the re-arm trigger, not pjData==nil)"
    )
    assert re.search(r"\bcreatePanel\s*\(", body), (
        "maintainPanel() does not call createPanel() when the window is dead "
        "(no recreate-on-dead path)"
    )
    # And it must still require an open project (project open AND window dead).
    assert re.search(r"pjData\s*==\s*nil", body), (
        "maintainPanel() does not also require an open project (the re-arm "
        "condition is 'project open AND window not alive')"
    )


def test_renavigate_on_respawn_url_freshness():
    """A loaded panel re-navigates when the server respawns (URL changes).

    After a respawn, validateReady writes NEW port/token; the already-loaded
    panel's last navigation succeeded (navOk stays true -- a WS disconnect is
    NOT a NavigationCompleted failure), so a `not navOk`-only retry never fires.
    maintainPanel MUST also re-navigate when the freshly built URL differs from
    the last navigated URL: track ``lastNavUrl`` and compare ``panelUrl()`` to
    it. (Finding F1.)
    """
    text = _read_text()
    # lastNavUrl must be a tracked bare global (same idiom as the other panel
    # state), recorded inside navigatePanel.
    code = _strip_comments(text)
    assert re.search(r"(?m)^(?!\s*local\b)\s*lastNavUrl\s*=", code), (
        "no `lastNavUrl` bare-global tracking (needed to detect a respawned "
        "server's new URL)"
    )
    nav_body = _function_body(text, "navigatePanel")
    assert re.search(r"lastNavUrl\s*=", nav_body), (
        "navigatePanel() does not record lastNavUrl (the freshness comparison "
        "in maintainPanel has nothing to compare against)"
    )
    # maintainPanel must re-navigate when panelUrl() differs from lastNavUrl
    # (the respawn path), in addition to the failed-nav retry.
    maint_body = _function_body(text, "maintainPanel")
    assert re.search(r"panelUrl\s*\(\s*\)\s*~=\s*lastNavUrl", maint_body) or re.search(
        r"lastNavUrl\s*~=\s*panelUrl\s*\(\s*\)", maint_body
    ), (
        "maintainPanel() does not compare panelUrl() against lastNavUrl; a "
        "respawned server (new port/token) would leave the panel on the dead "
        "old-token URL (Finding F1)"
    )
    # The re-navigate must still call navigatePanel() under that condition.
    assert re.search(r"\bnavigatePanel\s*\(", maint_body), (
        "maintainPanel() does not call navigatePanel() on the re-navigate path"
    )


# --------------------------------------------------------------------------
# 7. WPF panel removed; PANEL shows/focuses the WebView2 window
# --------------------------------------------------------------------------
def test_wpf_stackpanel_removed():
    """The v6 WPF StackPanel + mkBtn helper are removed (spec: Removal/unchanged)."""
    text = _read_text()
    # The v6 showPanel built a StackPanel with a mkBtn() button factory. Both
    # must be gone (the WPF control panel is replaced by the WebView2 window).
    assert "mkBtn" not in text, (
        "v6 WPF 'mkBtn' button factory is still present; the WPF control panel "
        "(StackPanel + 4 buttons) must be REMOVED (features/01 'Removal / "
        "unchanged')"
    )
    assert "StackPanel" not in text, (
        "v6 WPF 'StackPanel' is still present; the WPF control panel must be "
        "REMOVED in favor of the WebView2-hosted panel"
    )


def test_panelshown_flag_removed():
    """The v6 panelShown auto-show flag is replaced by window-handle tracking."""
    text = _read_text()
    assert "panelShown" not in text, (
        "the v6 'panelShown' boolean is still present; it must be replaced by "
        "window-handle / alive tracking (features/01 'Removal / unchanged')"
    )


def test_panel_branch_shows_webview_window():
    """The PANEL branch shows/refocuses the WebView2 window (Show/Activate)."""
    text = _read_text()
    region = _panel_branch_region(text)
    # The PANEL branch must no longer call the removed WPF showPanel(); it must
    # show/refocus the WebView2 window. Heuristic: a Show/Activate/Focus call or
    # an ensure/create-panel call in the branch body.
    assert re.search(
        r":Show\s*\(|:Activate\s*\(|:Focus\s*\(|ensurePanel|createPanel|showPanel",
        region,
    ), (
        "the PANEL branch must show/refocus the WebView2 window "
        "(Show/Activate/Focus or an ensure/create-panel call)"
    )
    # mkBtn / StackPanel must not appear in the PANEL region (belt-and-braces
    # over the file-wide check above).
    assert "mkBtn" not in region and "StackPanel" not in region, (
        "the PANEL branch still references the v6 WPF panel (mkBtn/StackPanel)"
    )


# --------------------------------------------------------------------------
# 8. regression (passes throughout)
# --------------------------------------------------------------------------
def test_all_command_markers_present():
    """All v6 + LIST + NEWEPS dispatcher commands survive (import-then-extend)."""
    text = _read_text()
    missing = [c for c in ALL_COMMANDS if not _branch_re(c).search(text)]
    assert not missing, f"missing command branches: {missing}"


def test_lifecycle_markers_present():
    """EUD-013 lifecycle markers survive: heartbeat write + ProcessStartInfo spawn."""
    text = _read_text()
    assert "heartbeat.txt" in text, "lifecycle heartbeat.txt write missing"
    assert "ProcessStartInfo" in text, "lifecycle ProcessStartInfo spawn missing"


def test_no_forbidden_lua_calls():
    """Crash-rule lint: no os.execute / io.popen / :GetValue( anywhere."""
    text = _read_text()
    forbidden = [tok for tok in ("os.execute", "io.popen", ":GetValue(") if tok in text]
    assert not forbidden, f"forbidden lua calls present: {forbidden}"


def test_extension_adds_no_raw_nonascii_bytes():
    """The WebView2 extension keeps non-ASCII bytes <= the baseline.

    The WebView2 window title is ASCII ("EUD Agent"); removing the v6 WPF panel
    strings will LOWER the count. The guard is ``<=`` so it holds both before
    the implementation (1,263) and after (lower).
    """
    raw = BRIDGE.read_bytes()
    nonascii = sum(1 for b in raw if b > 0x7F)
    assert nonascii <= BASELINE_NONASCII_BYTES, (
        f"non-ASCII byte count grew to {nonascii} (baseline "
        f"{BASELINE_NONASCII_BYTES}); the WebView2 extension must not add raw "
        f"non-ASCII bytes (ASCII window title; Korean UI lives in the web panel)"
    )


# --------------------------------------------------------------------------
# 9. busy status write + self-correcting dat/xdat errors (EUD-091)
# --------------------------------------------------------------------------
def test_busy_status_written_before_compiling_early_return():
    """status.txt is written INSIDE the IsCompilng branch, before its return.

    status.txt is the server's only busy signal (the 10s->180s poll-timeout
    extension + the panel's waiting_build notice). Writing it only on idle
    Ticks left it permanently compiling=false, so every command during a
    build timed out at 10s with a misleading compiling=False.
    """
    text = _read_text()
    region = _strip_comments(_tick_region(text))
    # The busy branch: from the IsCompilng test to its early `return`.
    m = re.search(r"IsCompilng\s+then\b", region)
    assert m, "no `IsCompilng then` early-return branch in the Tick"
    ret = re.search(r"\breturn\b", region[m.end():])
    assert ret, "no `return` after the IsCompilng test"
    busy = region[m.end(): m.end() + ret.start()]
    assert "status.txt" in busy, (
        "the IsCompilng branch does not write status.txt before its early "
        "return; the server can then never observe compiling=true (10s "
        "timeouts during every build)"
    )
    assert "compiling=True" in busy, (
        "the busy status write does not report the literal compiling=True"
    )
    # heartbeat stays FIRST (rules.md): its write precedes the IsCompilng test.
    hb = region.find("heartbeat.txt")
    assert 0 <= hb < m.start(), (
        "heartbeat.txt is not written before the IsCompilng early-return"
    )


def test_busy_status_does_not_touch_pjdata():
    """The busy status write reuses a cached project line (no pjData access).

    Editor objects must not be touched while IsCompilng (rules.md: the build
    shares the lua_State from a BackgroundWorker) — the busy write may only
    use the `lastProjectLine` bare global cached on the previous idle Tick.
    """
    text = _read_text()
    region = _strip_comments(_tick_region(text))
    m = re.search(r"IsCompilng\s+then\b", region)
    assert m, "no `IsCompilng then` early-return branch in the Tick"
    ret = re.search(r"\breturn\b", region[m.end():])
    busy = region[m.end(): m.end() + ret.start()]
    assert "lastProjectLine" in busy, (
        "the busy status write does not use the cached lastProjectLine"
    )
    assert "pjData" not in busy and "pj.Filename" not in busy, (
        "the busy status write touches pjData while IsCompilng (forbidden: "
        "the build shares the lua_State from a BackgroundWorker)"
    )
    # The cache is a bare global (luanet static-proxy idiom) refreshed on the
    # idle path, where pjData access is legal.
    code = _strip_comments(text)
    assert re.search(r"(?m)^(?!\s*local\b)\s*lastProjectLine\s*=", code), (
        "no `lastProjectLine` bare-global tracking"
    )
    idle = region[m.end() + ret.end():]
    assert re.search(r"lastProjectLine\s*=", idle), (
        "the idle Tick path does not refresh lastProjectLine"
    )


def test_datbinding_error_lists_valid_params():
    """A failed GETDAT/SETDAT resolve names the valid params (self-correcting).

    The live sessions burned 4-5 calls per request guessing display names
    ('Gas Cost', 'HitPoints') against a bare 'param/index' error; the error
    must enumerate the dat's valid param names.
    """
    text = _read_text()
    body = _function_body(text, "resolveDatBinding")
    assert "datParamNames" in body, (
        "resolveDatBinding() does not consult datParamNames() for the error"
    )
    assert "valid params for" in body, (
        "the param/index error does not carry the valid-param list"
    )
    # The walker uses the rules.md-safe accessors: parameterized property
    # get_GetDatFile, List get_Item, parameterless GetParamname via dot.
    walker = _function_body(text, "datParamNames")
    assert re.search(r":get_GetDatFile\s*\(", walker), (
        "datParamNames() does not use :get_GetDatFile( (VB parameterized "
        "property; plain access throws TargetParameterCountException)"
    )
    assert re.search(r":get_Item\s*\(", walker), (
        "datParamNames() does not use :get_Item( for the List walk"
    )
    assert ".GetParamname" in walker, (
        "datParamNames() does not read .GetParamname"
    )


def test_xdatbinding_error_lists_valid_names():
    """A failed GETXDAT/SETXDAT resolve names the kind's valid names.

    The xdat name sets are FIXED (editor BindingManager.vb:340-380):
    statusinfor Status/Display/Joint, wireframe wire/grp/tran, ButtonSet
    ButtonSet. The null-binding error must enumerate them.
    """
    text = _read_text()
    body = _function_body(text, "resolveXDatBinding")
    assert "valid names for" in body, (
        "the null-binding error does not carry the kind's valid names"
    )
    assert "Status, Display, Joint" in text, (
        "statusinfor's valid names (Status, Display, Joint) are not listed"
    )
    assert "wire, grp, tran" in text, (
        "wireframe's valid names (wire, grp, tran) are not listed"
    )


def _setbtn_branch_region(text: str) -> str:
    """Source region of the ``cmd == "SETBTN"`` dispatcher branch body.

    From the SETBTN branch to the next ``elseif cmd ==`` so the markers are
    attributable to SETBTN rather than a neighboring branch.
    """
    m = _branch_re("SETBTN").search(text)
    assert m, "SETBTN branch missing"
    region = text[m.start():]
    nxt = re.search(r"\n\s*elseif cmd ==", region[1:])
    if nxt:
        region = region[: nxt.start() + 1]
    return region


def test_setbtn_clears_isdefault():
    """SETBTN clears the button set's IsDefault flag after PasteFromString.

    Measured 2026-06-07 (EUD Editor 3 v0.19.6.0 + SC:R): PasteFromString never
    clears IsDefault; WriteButtonData.vb skips Db bytebuffer emission for
    IsDefault sets, so the runtime patch table keeps a stale default address
    with the new button count -> wild pointer -> StarCraft hard-crash on unit
    selection (32-bit and 64-bit, no EUD error dialog). The branch MUST set the
    set's IsDefault to false (plain dot-property assignment) so the edited set
    is emitted.
    """
    text = _read_text()
    region = _strip_comments(_setbtn_branch_region(text))
    assert "PasteFromString" in region, (
        "SETBTN branch does not call PasteFromString (precondition for the "
        "IsDefault-clear rail)"
    )
    assert re.search(r"\bIsDefault\s*=\s*false\b", region), (
        "SETBTN branch does not clear IsDefault after PasteFromString; "
        "WriteButtonData.vb skips Db emission for IsDefault sets -> stale "
        "default address -> SC hard-crash on unit selection (measured "
        "2026-06-07)"
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
