"""Verification artifact for EUD-054-8c97: the eud-tools tool layer + MCP shim.

These tests drive ``eud_agent.tools`` (the tool registry + validation + mutation
gate + budgets) and the token-authenticated HTTP endpoint the MCP shim forwards
to (``eud_agent.app``), per features/05 "Tools (registry)" and "Triage and plan
gating". They use a FAKE BridgeIO that RECORDS calls (the same behavioral pattern
as test_bridge_io.FakeBridge / test_bridge_datx_static), so the suite can assert:

  * **registry completeness** — every spec tool name (read/write/flow) is present
    and each validates its args; rejected args NEVER reach the fake bridge;
  * **mutation gate** — writes 1-2 pass without an approved plan; the 3rd write
    WITHOUT a plan returns a tool error directing codex to ``propose_plan``; after
    the plan-approve flag is set the gate lifts;
  * **budgets** — 30 tool actions per request; the 31st is rejected with a
    wrap-up message; the budget counter is queryable on the request state;
  * **endpoint/shim token auth** — the ``/tools/call`` endpoint rejects an
    unauthenticated request (wrong/missing token) and forwards a valid one to the
    tool layer, 127.0.0.1 only.

``eud_agent.tools`` does NOT exist during Step A, so this suite is expected to
FAIL on import until tools.py / the endpoint are implemented (Step B).
"""

from __future__ import annotations

import pytest

# Imported at collection so the failing import is the first signal in Step A.
from eud_agent.tools import (
    PLAN_TOOL,
    READ_TOOLS,
    WRITE_TOOLS,
    BudgetExceeded,
    MutationGate,
    PlanRequired,
    RequestState,
    ToolError,
    ToolLayer,
)

# --------------------------------------------------------------------------- #
# Spec tool sets (features/05 "Tools (registry)"). Pinned here so the registry
# can be asserted complete WITHOUT importing the module's own constants (the test
# is the independent contract).
# --------------------------------------------------------------------------- #

SPEC_READ_TOOLS = {
    "project_status",
    "list_files",
    "read_file",
    "dat_get",
    "xdat_get",
    "tbl_get",
    "req_get",
    "btn_get",
    "settings_get",
    "plugins_list",
    "build_errors",
    "search_docs",
}
SPEC_WRITE_TOOLS = {
    "dat_set",
    "xdat_set",
    "tbl_set",
    "req_set",
    "btn_set",
    "dat_reset",
    "file_create",
    "file_write",
    "file_rename",
    "file_delete",
    "file_move",
    "mkdir",
    "set_main",
    "settings_set",
    "plugin_add",
    "plugin_edit",
    "plugin_remove",
    "plugin_move",
    "build_run",
}
SPEC_FLOW_TOOLS = {"propose_plan"}


# --------------------------------------------------------------------------- #
# Fake bridge: records every call (method name + args) so the test can assert
# whether a call reached the bridge AND with what arguments. Each method returns
# a deterministic string (the bridge wrappers return reply text).
# --------------------------------------------------------------------------- #


class FakeBridge:
    """A BridgeIO stand-in that records calls and returns canned replies.

    Every BridgeIO method the tool layer maps to is defined here; calling one
    appends ``(name, args, kwargs)`` to ``self.calls`` and returns a string. The
    tests assert validation rejects an out-of-contract arg BEFORE any call lands
    here (``self.calls`` stays empty on a rejected tool call).
    """

    def __init__(self):
        self.calls: list[tuple] = []

    def _record(self, name, *args, **kwargs):
        self.calls.append((name, args, kwargs))
        return f"OK:{name}"

    # read
    def status(self, **kw):
        return self._record("status")

    def list_files(self, **kw):
        self.calls.append(("list_files", (), {}))
        return [{"path": "a.eps", "ftype": "CUIEps", "settable": True}]

    def get(self, path, **kw):
        return self._record("get", path)

    def getdat(self, dat, param, obj_id, **kw):
        return self._record("getdat", dat, param, obj_id)

    def getxdat(self, dat, name, obj_id, **kw):
        return self._record("getxdat", dat, name, obj_id)

    def gettbl(self, index, **kw):
        return self._record("gettbl", index)

    def getreq(self, dat, obj_id, **kw):
        return self._record("getreq", dat, obj_id)

    def getbtn(self, set_id, **kw):
        return self._record("getbtn", set_id)

    def getset(self, scope, key, **kw):
        return self._record("getset", scope, key)

    def pluglist(self, **kw):
        self.calls.append(("pluglist", (), {}))
        return [{"index": "0", "btype": "UserPlugin", "first_line": "x"}]

    def builderr(self, **kw):
        return self._record("builderr")

    # write
    def setdat(self, dat, param, obj_id, value, **kw):
        return self._record("setdat", dat, param, obj_id, value)

    def setxdat(self, dat, name, obj_id, value, **kw):
        return self._record("setxdat", dat, name, obj_id, value)

    def settbl(self, index, value, **kw):
        return self._record("settbl", index, value)

    def setreq(self, dat, obj_id, payload, **kw):
        return self._record("setreq", dat, obj_id, payload)

    def setbtn(self, set_id, csv, **kw):
        return self._record("setbtn", set_id, csv)

    def resetdat(self, kind, dat, param_or_name, obj_id, **kw):
        return self._record("resetdat", kind, dat, param_or_name, obj_id)

    def newfile(self, path, ftype, code, **kw):
        return self._record("newfile", path, ftype, code)

    def set(self, path, code, **kw):
        return self._record("set", path, code)

    def rename(self, path, newname, **kw):
        return self._record("rename", path, newname)

    def delfile(self, path, **kw):
        return self._record("delfile", path)

    def movefile(self, path, dest_folder, **kw):
        return self._record("movefile", path, dest_folder)

    def mkdir(self, path, **kw):
        return self._record("mkdir", path)

    def setmain(self, path, **kw):
        return self._record("setmain", path)

    def setset(self, scope, key, value, **kw):
        return self._record("setset", scope, key, value)

    def plugadd(self, index, texts, **kw):
        return self._record("plugadd", index, texts)

    def plugset(self, index, texts, **kw):
        return self._record("plugset", index, texts)

    def plugdel(self, index, **kw):
        return self._record("plugdel", index)

    def plugmove(self, from_index, to_index, **kw):
        return self._record("plugmove", from_index, to_index)

    def build(self, **kw):
        return self._record("build")


def make_layer():
    bridge = FakeBridge()
    layer = ToolLayer(bridge)
    return bridge, layer


def fresh_state():
    return RequestState(request_id="req-1")


# --------------------------------------------------------------------------- #
# Registry completeness: every spec tool name present + classified correctly.
# --------------------------------------------------------------------------- #


def test_registry_has_every_read_tool():
    assert SPEC_READ_TOOLS <= set(READ_TOOLS)
    _, layer = make_layer()
    for name in SPEC_READ_TOOLS:
        assert layer.has_tool(name), f"missing read tool {name!r}"
        assert not layer.is_mutating(name), f"{name} must be read-only"


def test_registry_has_every_write_tool():
    assert SPEC_WRITE_TOOLS <= set(WRITE_TOOLS)
    _, layer = make_layer()
    for name in SPEC_WRITE_TOOLS:
        assert layer.has_tool(name), f"missing write tool {name!r}"
        assert layer.is_mutating(name), f"{name} must be mutating"


def test_registry_has_propose_plan_flow_tool():
    _, layer = make_layer()
    assert layer.has_tool(PLAN_TOOL)
    assert PLAN_TOOL == "propose_plan"
    # propose_plan is a flow tool, NOT a mutation (it must not consume the gate).
    assert not layer.is_mutating(PLAN_TOOL)


def test_registry_exposes_tool_specs_with_descriptions_and_params():
    """Each tool must publish a name + description + JSON-schema params so the MCP
    shim can advertise it. (The shim is dumb transport but still needs schemas.)"""
    _, layer = make_layer()
    specs = layer.tool_specs()
    names = {s["name"] for s in specs}
    assert SPEC_READ_TOOLS | SPEC_WRITE_TOOLS | SPEC_FLOW_TOOLS <= names
    for s in specs:
        assert s.get("description"), f"{s['name']} missing description"
        assert "parameters" in s, f"{s['name']} missing parameters schema"


# --------------------------------------------------------------------------- #
# Read tools route to the bridge with correct args.
# --------------------------------------------------------------------------- #


def test_read_tool_routes_to_bridge():
    bridge, layer = make_layer()
    st = fresh_state()
    layer.call("dat_get", {"dat": "units", "param": "HP", "objId": 0}, st)
    assert ("getdat", ("units", "HP", 0), {}) in bridge.calls


def test_project_status_routes_to_status():
    bridge, layer = make_layer()
    layer.call("project_status", {}, fresh_state())
    assert ("status", (), {}) in bridge.calls


def test_read_file_routes_to_get():
    bridge, layer = make_layer()
    layer.call("read_file", {"path": "a.eps"}, fresh_state())
    assert ("get", ("a.eps",), {}) in bridge.calls


def test_settings_get_routes_to_getset():
    bridge, layer = make_layer()
    layer.call(
        "settings_get", {"scope": "project", "key": "OpenMapName"}, fresh_state()
    )
    assert ("getset", ("project", "OpenMapName"), {}) in bridge.calls


def test_search_docs_exists_and_validates_without_bridge_call():
    """search_docs is the RAG tool. RAG plumbing is not required here (stub ->
    []), but the tool MUST exist and validate its args; it never hits the bridge."""
    bridge, layer = make_layer()
    out = layer.call("search_docs", {"query": "trigger loop", "k": 3}, fresh_state())
    # No bridge call — RAG is a separate subsystem (stub allowed).
    assert all(c[0] != "get" for c in bridge.calls)
    assert isinstance(out, (list, dict, str))


def test_search_docs_rejects_missing_query():
    _, layer = make_layer()
    with pytest.raises(ToolError):
        layer.call("search_docs", {}, fresh_state())


# --------------------------------------------------------------------------- #
# Validation BEFORE bridge: rejected args never reach the fake bridge.
# --------------------------------------------------------------------------- #


def test_dat_get_rejects_unknown_dat_before_bridge():
    bridge, layer = make_layer()
    with pytest.raises(ToolError):
        layer.call("dat_get", {"dat": "nope", "param": "HP", "objId": 0}, fresh_state())
    assert bridge.calls == []


def test_dat_set_rejects_nonnumeric_value_before_bridge():
    bridge, layer = make_layer()
    with pytest.raises(ToolError):
        layer.call(
            "dat_set",
            {"dat": "units", "param": "HP", "objId": 0, "value": "abc"},
            fresh_state(),
        )
    assert bridge.calls == []


def test_tbl_get_rejects_negative_index_before_bridge():
    bridge, layer = make_layer()
    with pytest.raises(ToolError):
        layer.call("tbl_get", {"index": -3}, fresh_state())
    assert bridge.calls == []


def test_file_create_rejects_bad_type_before_bridge():
    bridge, layer = make_layer()
    with pytest.raises(ToolError):
        layer.call(
            "file_create",
            {"path": "x.eps", "ftype": "GUI", "code": ""},
            fresh_state(),
        )
    assert bridge.calls == []


def test_settings_set_rejects_readonly_language_before_bridge():
    bridge, layer = make_layer()
    with pytest.raises(ToolError):
        layer.call(
            "settings_set",
            {"scope": "program", "key": "Language", "value": "en"},
            fresh_state(),
        )
    assert bridge.calls == []


def test_plugin_add_rejects_nonint_index_before_bridge():
    """plugin_add allows the -1 append sentinel, but a non-integer index must be
    a ToolError (NOT a bare ValueError -> HTTP 500): every validation failure is a
    tool RESULT codex can correct."""
    bridge, layer = make_layer()
    with pytest.raises(ToolError):
        layer.call("plugin_add", {"index": "abc", "texts": "x"}, fresh_state())
    assert bridge.calls == []


def test_plugin_add_rejects_index_below_minus_one_before_bridge():
    bridge, layer = make_layer()
    with pytest.raises(ToolError):
        layer.call("plugin_add", {"index": -2, "texts": "x"}, fresh_state())
    assert bridge.calls == []


def test_plugin_add_allows_append_sentinel():
    """index=-1 (the documented append sentinel) is valid and reaches the bridge."""
    bridge, layer = make_layer()
    layer.call("plugin_add", {"index": -1, "texts": "x"}, fresh_state())
    assert ("plugadd", (-1, "x"), {}) in bridge.calls


def test_unknown_tool_name_raises_tool_error():
    _, layer = make_layer()
    with pytest.raises(ToolError):
        layer.call("not_a_tool", {}, fresh_state())


def test_missing_required_arg_rejected_before_bridge():
    bridge, layer = make_layer()
    with pytest.raises(ToolError):
        layer.call("read_file", {}, fresh_state())  # no path
    assert bridge.calls == []


# --------------------------------------------------------------------------- #
# Write tools route + count mutations.
# --------------------------------------------------------------------------- #


def test_write_tool_routes_and_counts_mutation():
    bridge, layer = make_layer()
    st = fresh_state()
    layer.call("file_write", {"path": "a.eps", "code": "x = 1"}, st)
    # file_write -> SET (existing).
    assert ("set", ("a.eps", "x = 1"), {}) in bridge.calls
    assert st.mutation_count == 1


def test_file_create_routes_to_newfile():
    bridge, layer = make_layer()
    st = fresh_state()
    layer.call(
        "file_create", {"path": "n.eps", "ftype": "CUIEps", "code": "y=2"}, st
    )
    assert ("newfile", ("n.eps", "CUIEps", "y=2"), {}) in bridge.calls


def test_set_main_routes_to_setmain():
    bridge, layer = make_layer()
    layer.call("set_main", {"path": "main.eps"}, fresh_state())
    assert ("setmain", ("main.eps",), {}) in bridge.calls


def test_build_run_routes_to_build():
    bridge, layer = make_layer()
    layer.call("build_run", {}, fresh_state())
    assert ("build", (), {}) in bridge.calls


def test_read_tool_does_not_count_as_mutation():
    _, layer = make_layer()
    st = fresh_state()
    layer.call("project_status", {}, st)
    layer.call("read_file", {"path": "a.eps"}, st)
    assert st.mutation_count == 0


# --------------------------------------------------------------------------- #
# Mutation gate (features/05): writes 1-2 pass; 3rd without plan -> error;
# after plan-approve flag -> gate lifts.
# --------------------------------------------------------------------------- #


def test_first_two_writes_pass_without_plan():
    bridge, layer = make_layer()
    st = fresh_state()
    layer.call("file_write", {"path": "a.eps", "code": "1"}, st)
    layer.call("file_write", {"path": "b.eps", "code": "2"}, st)
    assert st.mutation_count == 2


def test_third_write_without_plan_is_gated():
    bridge, layer = make_layer()
    st = fresh_state()
    layer.call("file_write", {"path": "a.eps", "code": "1"}, st)
    layer.call("file_write", {"path": "b.eps", "code": "2"}, st)
    with pytest.raises(PlanRequired) as ei:
        layer.call("file_write", {"path": "c.eps", "code": "3"}, st)
    # The error must direct codex to propose_plan.
    assert "propose_plan" in str(ei.value)
    # PlanRequired is a ToolError subtype (returned as a tool error to codex).
    assert isinstance(ei.value, ToolError)
    # The gated call must NOT have reached the bridge nor incremented the count.
    assert st.mutation_count == 2
    assert not any(c[1] == ("c.eps", "3") for c in bridge.calls)


def test_gate_lifts_after_plan_approve_flag():
    bridge, layer = make_layer()
    st = fresh_state()
    layer.call("file_write", {"path": "a.eps", "code": "1"}, st)
    layer.call("file_write", {"path": "b.eps", "code": "2"}, st)
    st.approve_plan()  # the panel approved the plan -> gate lifts for this request
    assert st.plan_approved is True
    layer.call("file_write", {"path": "c.eps", "code": "3"}, st)
    layer.call("file_write", {"path": "d.eps", "code": "4"}, st)
    assert st.mutation_count == 4
    assert any(c[1] == ("c.eps", "3") for c in bridge.calls)


def test_propose_plan_does_not_consume_gate_and_ends_turn():
    bridge, layer = make_layer()
    st = fresh_state()
    layer.call("file_write", {"path": "a.eps", "code": "1"}, st)
    layer.call("file_write", {"path": "b.eps", "code": "2"}, st)
    # propose_plan is a flow tool: it does NOT count as a mutation and is allowed
    # even at the gate threshold (it is HOW codex satisfies the gate).
    result = layer.call("propose_plan", {"markdown": "# plan\n1. step"}, st)
    assert st.mutation_count == 2  # unchanged
    # propose_plan ends the turn — the result/state records that.
    assert st.plan_proposed is True or (
        isinstance(result, dict) and result.get("ends_turn")
    )


def test_mutation_gate_standalone_logic():
    """MutationGate is a standalone testable unit (counts, threshold, lift)."""
    gate = MutationGate(threshold=3)
    assert gate.allow(mutations_so_far=0, plan_approved=False)
    assert gate.allow(mutations_so_far=1, plan_approved=False)
    # The 3rd mutating call (2 already done) without a plan is blocked.
    assert not gate.allow(mutations_so_far=2, plan_approved=False)
    # With an approved plan it is allowed regardless of count.
    assert gate.allow(mutations_so_far=2, plan_approved=True)
    assert gate.allow(mutations_so_far=50, plan_approved=True)


# --------------------------------------------------------------------------- #
# Budgets (features/05): 30 tool actions per request; 31st rejected; counter
# queryable; build self-fix attempts tracked (loop wired later).
# --------------------------------------------------------------------------- #


def test_action_budget_counter_is_queryable():
    _, layer = make_layer()
    st = fresh_state()
    layer.call("project_status", {}, st)
    layer.call("read_file", {"path": "a.eps"}, st)
    assert st.action_count == 2
    # Budget snapshot exposed for panel display.
    snap = st.budget_snapshot()
    assert snap["actions_used"] == 2
    assert snap["actions_limit"] == 30
    assert snap["actions_remaining"] == 28


def test_thirty_first_action_rejected_with_wrapup():
    bridge, layer = make_layer()
    st = fresh_state()
    # 30 read actions exhaust the per-request budget.
    for _ in range(30):
        layer.call("project_status", {}, st)
    assert st.action_count == 30
    with pytest.raises(BudgetExceeded) as ei:
        layer.call("project_status", {}, st)
    assert isinstance(ei.value, ToolError)
    msg = str(ei.value).lower()
    assert "wrap" in msg or "budget" in msg
    # The rejected 31st action did NOT reach the bridge nor increment the count.
    assert st.action_count == 30


def test_build_fix_attempts_tracked_on_state():
    """Build self-fix attempts are tracked (the loop itself is wired later)."""
    st = fresh_state()
    assert st.build_fix_attempts == 0
    assert st.build_fix_limit == 3
    st.record_build_fix_attempt()
    st.record_build_fix_attempt()
    assert st.build_fix_attempts == 2
    snap = st.budget_snapshot()
    assert snap["build_fix_attempts"] == 2
    assert snap["build_fix_limit"] == 3


def test_budget_counts_rejected_validation_action_does_not_consume():
    """A tool call rejected by VALIDATION (before the bridge) must not silently
    burn an action budget slot in a way that strands the request — validation
    errors are surfaced for codex to retry, so they do not count."""
    _, layer = make_layer()
    st = fresh_state()
    with pytest.raises(ToolError):
        layer.call("dat_get", {"dat": "nope", "param": "HP", "objId": 0}, st)
    assert st.action_count == 0


# --------------------------------------------------------------------------- #
# request-state registry on the ToolLayer (keyed by request_id) for the endpoint.
# --------------------------------------------------------------------------- #


def test_tool_layer_tracks_request_state_by_id():
    bridge, layer = make_layer()
    layer.call_for_request("rid-A", "project_status", {})
    layer.call_for_request("rid-A", "read_file", {"path": "a.eps"})
    st = layer.get_request_state("rid-A")
    assert st.action_count == 2
    # A different request id has an independent budget.
    layer.call_for_request("rid-B", "project_status", {})
    assert layer.get_request_state("rid-B").action_count == 1


def test_approve_plan_for_request_lifts_gate():
    bridge, layer = make_layer()
    layer.call_for_request("rid-C", "file_write", {"path": "a.eps", "code": "1"})
    layer.call_for_request("rid-C", "file_write", {"path": "b.eps", "code": "2"})
    layer.approve_plan_for_request("rid-C")
    # 3rd write now allowed for that request.
    layer.call_for_request("rid-C", "file_write", {"path": "c.eps", "code": "3"})
    assert layer.get_request_state("rid-C").mutation_count == 3


# --------------------------------------------------------------------------- #
# Endpoint / shim token auth: unauthenticated rejected; valid forwarded.
# --------------------------------------------------------------------------- #


def _make_app_config(tmp_path, *, token="tok-xyz", port=8765):
    from eud_agent.config import Config

    data_dir = tmp_path / "data"
    (data_dir / "inbox").mkdir(parents=True, exist_ok=True)
    (data_dir / "outbox").mkdir(parents=True, exist_ok=True)
    repo_root = tmp_path / "repo"
    repo_root.mkdir(parents=True, exist_ok=True)
    return Config(
        data_dir=str(data_dir),
        port=port,
        codex_cmd="",
        rag_db=str(tmp_path / "rag"),
        repo_root=str(repo_root),
        hf_cache_dir=str(tmp_path / "hf"),
        token=token,
    )


@pytest.fixture(autouse=True)
def _no_real_warmup(monkeypatch):
    import threading

    from eud_agent import rag as rag_mod

    monkeypatch.setattr(
        rag_mod, "start_warmup",
        lambda *a, **k: threading.Thread(target=lambda: None),
    )


def test_tools_call_endpoint_rejects_missing_token(tmp_path):
    from fastapi.testclient import TestClient

    from eud_agent import app as app_mod

    cfg = _make_app_config(tmp_path, token="right")
    with TestClient(app_mod.create_app(cfg, start_lifecycle=False)) as client:
        r = client.post(
            "/tools/call",
            json={"request_id": "r1", "tool": "project_status", "args": {}},
        )
    assert r.status_code in (401, 403)


def test_tools_call_endpoint_rejects_wrong_token(tmp_path):
    from fastapi.testclient import TestClient

    from eud_agent import app as app_mod

    cfg = _make_app_config(tmp_path, token="right")
    with TestClient(app_mod.create_app(cfg, start_lifecycle=False)) as client:
        r = client.post(
            "/tools/call",
            json={
                "token": "WRONG",
                "request_id": "r1",
                "tool": "project_status",
                "args": {},
            },
        )
    assert r.status_code in (401, 403)


def test_tools_call_endpoint_accepts_valid_token_and_forwards(tmp_path, monkeypatch):
    from fastapi.testclient import TestClient

    from eud_agent import app as app_mod
    from eud_agent import bridge_io as bio_mod

    cfg = _make_app_config(tmp_path, token="right")

    # Patch the bridge STATUS so project_status returns a deterministic reply
    # without a real bridge round-trip (the tool routes status -> bridge.status).
    monkeypatch.setattr(
        bio_mod.BridgeIO, "status", lambda self, **kw: "compiling=false\nproject=p\n"
    )

    with TestClient(app_mod.create_app(cfg, start_lifecycle=False)) as client:
        r = client.post(
            "/tools/call",
            json={
                "token": "right",
                "request_id": "r1",
                "tool": "project_status",
                "args": {},
            },
        )
    assert r.status_code == 200
    body = r.json()
    assert body.get("ok") is True
    assert "result" in body


def test_tools_call_endpoint_validation_error_returns_tool_error(tmp_path):
    """A validation failure is a TOOL error (ok=false + message), not an HTTP 5xx
    — codex must see it as a tool result it can correct, not a transport crash."""
    from fastapi.testclient import TestClient

    from eud_agent import app as app_mod

    cfg = _make_app_config(tmp_path, token="right")
    with TestClient(app_mod.create_app(cfg, start_lifecycle=False)) as client:
        r = client.post(
            "/tools/call",
            json={
                "token": "right",
                "request_id": "r1",
                "tool": "dat_get",
                "args": {"dat": "nope", "param": "HP", "objId": 0},
            },
        )
    assert r.status_code == 200
    body = r.json()
    assert body.get("ok") is False
    assert body.get("error")


def test_tools_call_plugin_add_bad_index_returns_tool_error_not_500(tmp_path):
    """A non-integer plugin_add index must come back as {ok:false} (a tool result
    codex can correct), NEVER an unhandled HTTP 500. Regression guard for the
    handler that previously did a bare int() on the index."""
    from fastapi.testclient import TestClient

    from eud_agent import app as app_mod

    cfg = _make_app_config(tmp_path, token="right")
    with TestClient(app_mod.create_app(cfg, start_lifecycle=False)) as client:
        r = client.post(
            "/tools/call",
            json={
                "token": "right",
                "request_id": "r1",
                "tool": "plugin_add",
                "args": {"index": "abc", "texts": "x"},
            },
        )
    assert r.status_code == 200
    body = r.json()
    assert body.get("ok") is False
    assert body.get("error")


def test_tools_list_endpoint_advertises_specs_with_token(tmp_path):
    """The shim fetches the tool specs to register them; the endpoint serves them
    only with a valid token."""
    from fastapi.testclient import TestClient

    from eud_agent import app as app_mod

    cfg = _make_app_config(tmp_path, token="right")
    with TestClient(app_mod.create_app(cfg, start_lifecycle=False)) as client:
        bad = client.get("/tools/list", params={"token": "WRONG"})
        assert bad.status_code in (401, 403)
        good = client.get("/tools/list", params={"token": "right"})
    assert good.status_code == 200
    names = {s["name"] for s in good.json()["tools"]}
    assert SPEC_READ_TOOLS | SPEC_WRITE_TOOLS | SPEC_FLOW_TOOLS <= names


# --------------------------------------------------------------------------- #
# mcp_shim: thin transport. It must import and expose a way to forward a call to
# the running server using the server.ready token (no tool logic of its own).
# --------------------------------------------------------------------------- #


def test_mcp_shim_imports_and_reads_ready(tmp_path):
    import json

    from eud_agent import mcp_shim

    ready = tmp_path / "server.ready"
    ready.write_text(
        json.dumps({"port": 9999, "token": "shim-tok"}), encoding="utf-8"
    )
    port, token = mcp_shim.read_ready(ready)
    assert port == 9999
    assert token == "shim-tok"


def test_mcp_shim_build_url_targets_loopback():
    from eud_agent import mcp_shim

    url = mcp_shim.tools_call_url(8765)
    assert url.startswith("http://127.0.0.1:8765")
    assert "0.0.0.0" not in url
