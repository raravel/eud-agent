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
    EvidenceRequired,
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


def test_search_docs_routes_to_injected_rag_search():
    """search_docs is the RAG tool (EUD-086): routed to the injected callable,
    never the bridge; the query/k pass through (k defaulting to 5)."""
    calls = []

    def fake_rag(query, k):
        calls.append((query, k))
        return [{"title": "t", "url": "u", "distance": 0.1, "text": "본문"}]

    bridge = FakeBridge()
    layer = ToolLayer(bridge, rag_search=fake_rag)
    state = fresh_state()
    out = layer.call("search_docs", {"query": "음수 로케이션", "k": 3}, state)
    assert calls == [("음수 로케이션", 3)]
    assert out[0]["text"] == "본문"
    assert bridge.calls == []  # RAG is a separate subsystem
    assert state.action_count == 1  # a READ: one action, no mutation
    assert state.mutation_count == 0

    layer.call("search_docs", {"query": "유닛 체력"}, state)
    assert calls[-1] == ("유닛 체력", 5)  # default k


def test_search_docs_clamps_k_and_rejects_bad_k():
    calls = []
    _bridge = FakeBridge()
    layer = ToolLayer(_bridge, rag_search=lambda q, k: calls.append((q, k)) or [])
    layer.call("search_docs", {"query": "디텍터", "k": 99}, fresh_state())
    assert calls == [("디텍터", 10)]  # SEARCH_DOCS_MAX_K cap
    for bad in (0, -1, "five", True):
        with pytest.raises(ToolError):
            layer.call("search_docs", {"query": "x", "k": bad}, fresh_state())
    assert len(calls) == 1  # rejected args never reach the callable


def test_search_docs_without_injection_is_tool_error():
    """A layer built WITHOUT rag_search rejects clearly (advisory shape, like
    map_info without a service) — never a silent empty result."""
    _, layer = make_layer()
    with pytest.raises(ToolError, match="search_docs unavailable"):
        layer.call("search_docs", {"query": "trigger loop"}, fresh_state())


def test_search_docs_rejects_missing_query():
    _, layer = make_layer()
    with pytest.raises(ToolError):
        layer.call("search_docs", {}, fresh_state())


def test_missing_args_error_carries_usage_line():
    """EUD-087: a misnamed call (codex invented table/field/id) gets a
    SELF-CORRECTING error carrying the tool's real usage line."""
    bridge, layer = make_layer()
    with pytest.raises(
        ToolError, match=r"Usage: xdat_get\(dat, name, objId\)"
    ) as ei:
        layer.call(
            "xdat_get",
            {"table": "units", "field": "ButtonSet", "id": 65},
            fresh_state(),
        )
    assert "'dat'" in str(ei.value)  # names every missing required arg
    assert bridge.calls == []  # rejected before any bridge touch


def test_value_whitelists_advertised_as_schema_enums():
    """EUD-091: plain-string args with server-side whitelists advertise them
    as JSON-schema enums. The live sessions showed codex inventing values
    (xdat kind 'units', dat names) and burning 4-5 calls per request
    rediscovering each whitelist from error text alone."""
    _, layer = make_layer()
    props = {
        s["name"]: s["parameters"]["properties"] for s in layer.tool_specs()
    }
    dat_names = {
        "units", "weapons", "flingy", "sprites", "images",
        "upgrades", "techdata", "orders", "portdata", "sfxdata",
    }
    xdat_kinds = {"statusinfor", "wireframe", "ButtonSet"}
    req_dats = {"units", "upgrades", "techdata", "Stechdata", "orders"}
    assert set(props["dat_get"]["dat"]["enum"]) == dat_names
    assert set(props["dat_set"]["dat"]["enum"]) == dat_names
    assert set(props["xdat_get"]["dat"]["enum"]) == xdat_kinds
    assert set(props["xdat_set"]["dat"]["enum"]) == xdat_kinds
    assert set(props["req_get"]["dat"]["enum"]) == req_dats
    assert set(props["req_set"]["dat"]["enum"]) == req_dats
    assert set(props["settings_get"]["scope"]["enum"]) == {
        "project", "program",
    }
    assert set(props["settings_set"]["scope"]["enum"]) == {
        "project", "program",
    }
    assert set(props["dat_reset"]["kind"]["enum"]) == {"dat", "xdat", "tbl"}
    assert set(props["file_create"]["ftype"]["enum"]) == {
        "CUIEps", "CUIPy", "RawText",
    }


def test_xdat_kind_units_error_carries_buttonset_hint():
    """EUD-091: dat='units' against the xdat tools was the top live repeat
    failure (every fresh request re-made it); the rejection must say WHERE a
    unit's button set actually lives so one retry suffices."""
    bridge, layer = make_layer()
    with pytest.raises(ToolError) as ei:
        layer.call(
            "xdat_get",
            {"dat": "units", "name": "ButtonSet", "objId": 65},
            fresh_state(),
        )
    msg = str(ei.value)
    assert "dat='ButtonSet'" in msg  # the corrected call shape
    assert "dat_get" in msg  # unit dat fields live in dat_get
    assert bridge.calls == []  # rejected before any bridge touch
    # Same hint on the write side.
    with pytest.raises(ToolError, match=r"dat='ButtonSet'"):
        layer.call(
            "xdat_set",
            {"dat": "units", "name": "ButtonSet", "objId": 65, "value": 1},
            fresh_state(),
        )
    assert bridge.calls == []


def test_bridge_busy_translates_to_tool_error_not_crash():
    """EUD-087: BridgeBusy (timeout, NOT a BridgeError subclass) escaped
    _dispatch untranslated and became an HTTP 500 at /tools/call."""
    from eud_agent.bridge_io import BridgeBusy

    class BusyBridge(FakeBridge):
        def builderr(self, **kw):
            raise BridgeBusy("editor busy: no .result before timeout")

    layer = ToolLayer(BusyBridge())
    state = fresh_state()
    with pytest.raises(ToolError, match="editor busy"):
        layer.call("build_errors", {}, state)
    assert state.action_count == 0  # a failed call counts nothing


def test_search_docs_translates_rag_failure_to_tool_error():
    """RagUnavailable (and any search crash) surfaces as a correctable
    ToolError that counts nothing — search is an advisory read."""
    from eud_agent.rag import RagUnavailable

    def broken(query, k):
        raise RagUnavailable("RAG DB directory not found: nope")

    layer = ToolLayer(FakeBridge(), rag_search=broken)
    state = fresh_state()
    with pytest.raises(ToolError, match="RAG unavailable"):
        layer.call("search_docs", {"query": "질문"}, state)
    assert state.action_count == 0


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


# --------------------------------------------------------------------------- #
# btn_set requirement-string rail (first principles #15): a disableable button
# (actval != 0, train/tech) MUST carry a nonzero disstr — rendering a 0/None
# requirement string crashes 64-bit StarCraft the moment the unit is selected.
# CSV: dot-separated button groups, each 8 comma fields
# pos,icon,con,act,conval,actval,enastr,disstr.
# --------------------------------------------------------------------------- #


def test_btn_set_rejects_train_button_with_zero_disstr():
    bridge, layer = make_layer()
    st = fresh_state()
    # A train button: actval carries the trained-unit id (nonzero) but disstr=0.
    csv = "0,228,4,2,0,7,100,0"
    with pytest.raises(ToolError) as ei:
        layer.call("btn_set", {"setId": 200, "csv": csv}, st)
    # The error names first principles #15 and the offending button position.
    assert "#15" in str(ei.value)
    assert "position 0" in str(ei.value)
    # The remedy is spelled out (reuse enastr).
    assert "enastr" in str(ei.value)
    # Rejected: it must NOT have reached the bridge nor counted a mutation.
    assert not any(c[0] == "setbtn" for c in bridge.calls)
    assert st.mutation_count == 0


def test_btn_set_accepts_train_button_with_disstr_equal_enastr():
    bridge, layer = make_layer()
    st = fresh_state()
    # Same train button, but disstr reuses enastr (100) -> valid.
    csv = "0,228,4,2,0,7,100,100"
    layer.call("btn_set", {"setId": 200, "csv": csv}, st)
    assert ("setbtn", (200, csv), {}) in bridge.calls
    assert st.mutation_count == 1


def test_btn_set_accepts_command_button_with_zero_actval_and_disstr():
    bridge, layer = make_layer()
    st = fresh_state()
    # An always-enabled command button (move): actval=0 -> disstr 0 is exempt.
    csv = "0,228,4,2,0,0,100,0"
    layer.call("btn_set", {"setId": 200, "csv": csv}, st)
    assert ("setbtn", (200, csv), {}) in bridge.calls
    assert st.mutation_count == 1


# --------------------------------------------------------------------------- #
# xdat_set ButtonSet-reassignment rail (measured 2026-06-07): reassigning a
# unit's ButtonSet xdat to ANOTHER set id hard-crashes StarCraft on unit
# selection (32-bit and 64-bit). In-place editing of the unit's own set is the
# safe pattern (btn_set). Only ButtonSet/ButtonSet is guarded.
# --------------------------------------------------------------------------- #


def test_xdat_set_rejects_buttonset_reassignment_to_other_id():
    bridge, layer = make_layer()
    st = fresh_state()
    # Reassign unit 65's ButtonSet to set id 200 (another set) -> crash class.
    with pytest.raises(ToolError) as ei:
        layer.call(
            "xdat_set",
            {"dat": "ButtonSet", "name": "ButtonSet", "objId": 65, "value": 200},
            st,
        )
    msg = str(ei.value)
    assert "hard-crash" in msg
    assert "2026-06-07" in msg
    assert "btn_set" in msg  # the safe in-place alternative
    # Rejected before any bridge touch / mutation count.
    assert not any(c[0] == "setxdat" for c in bridge.calls)
    assert st.mutation_count == 0


def test_xdat_set_allows_buttonset_same_id_in_place():
    bridge, layer = make_layer()
    st = fresh_state()
    # value == objId: editing the unit's OWN set in place is the safe pattern.
    layer.call(
        "xdat_set",
        {"dat": "ButtonSet", "name": "ButtonSet", "objId": 65, "value": 65},
        st,
    )
    assert ("setxdat", ("ButtonSet", "ButtonSet", 65, "65"), {}) in bridge.calls
    assert st.mutation_count == 1


def test_xdat_set_other_kinds_unaffected_by_buttonset_rail():
    bridge, layer = make_layer()
    st = fresh_state()
    # A wireframe xdat write with value != objId must NOT be blocked (the rail
    # only guards ButtonSet/ButtonSet).
    layer.call(
        "xdat_set",
        {"dat": "wireframe", "name": "grp", "objId": 65, "value": 200},
        st,
    )
    assert ("setxdat", ("wireframe", "grp", 65, "200"), {}) in bridge.calls
    assert st.mutation_count == 1


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


# --------------------------------------------------------------------------- #
# Evidence gate (EUD-090): on a RAG-wired layer, mutating calls are rejected
# until ONE search_docs has run in the request (zero hits included); without
# RAG the gate never fires (degrade, don't brick writes); memory_write and
# build_run are exempt.
# --------------------------------------------------------------------------- #


def make_rag_layer(hits=None):
    bridge = FakeBridge()
    layer = ToolLayer(bridge, rag_search=lambda q, k: list(hits or []))
    return bridge, layer


def test_write_without_search_docs_is_evidence_gated():
    bridge, layer = make_rag_layer()
    st = fresh_state()
    with pytest.raises(EvidenceRequired) as ei:
        layer.call("file_write", {"path": "a.eps", "code": "1"}, st)
    # The error must direct codex to search_docs (with the citation contract).
    assert "search_docs" in str(ei.value)
    # EvidenceRequired is a ToolError subtype (a correctable tool result).
    assert isinstance(ei.value, ToolError)
    # The gated call counts NOTHING and never reached the bridge.
    assert st.action_count == 0
    assert st.mutation_count == 0
    assert bridge.calls == []


def test_search_docs_lifts_evidence_gate():
    bridge, layer = make_rag_layer(
        [{"title": "t", "url": "u", "distance": 0.1, "text": "본문"}]
    )
    st = fresh_state()
    layer.call("search_docs", {"query": "유닛 체력"}, st)
    assert st.docs_searched is True
    layer.call("file_write", {"path": "a.eps", "code": "1"}, st)
    assert st.mutation_count == 1
    assert any(c[0] == "set" for c in bridge.calls)
    # Exposed for panel display alongside the other budget flags.
    assert st.budget_snapshot()["docs_searched"] == 1


def test_zero_hit_search_still_lifts_evidence_gate():
    """The gate forces the SEARCH, not a hit — an empty result must not
    deadlock the request (the agent marks the item 근거 없음 and proceeds)."""
    _, layer = make_rag_layer([])
    st = fresh_state()
    layer.call("search_docs", {"query": "없는 주제"}, st)
    layer.call("file_write", {"path": "a.eps", "code": "1"}, st)
    assert st.mutation_count == 1


def test_evidence_gate_absent_without_rag_wiring():
    """A layer built WITHOUT rag_search cannot satisfy the gate, so it must not
    fire (advisory subsystem: degrade, don't brick all writes)."""
    bridge, layer = make_layer()
    st = fresh_state()
    layer.call("file_write", {"path": "a.eps", "code": "1"}, st)
    assert st.mutation_count == 1


def test_evidence_gate_is_per_request():
    _, layer = make_rag_layer()
    layer.call_for_request("rid-A", "search_docs", {"query": "근거"})
    # rid-A searched; rid-B did not — only rid-B's write is gated.
    layer.call_for_request("rid-A", "file_write", {"path": "a.eps", "code": "1"})
    with pytest.raises(EvidenceRequired):
        layer.call_for_request("rid-B", "file_write", {"path": "b.eps", "code": "2"})


def test_build_run_exempt_from_evidence_gate():
    """build_run verifies existing writes (which already passed the gate) and
    creates no new content — a bare rebuild request must not force a search."""
    bridge, layer = make_rag_layer()
    st = fresh_state()
    layer.call("build_run", {}, st)
    assert any(c[0] == "build" for c in bridge.calls)
    assert st.mutation_count == 1


def test_failed_search_does_not_lift_evidence_gate():
    """A search that ERRORED (RAG down) never ran — the gate stays."""
    from eud_agent.rag import RagUnavailable

    def broken(query, k):
        raise RagUnavailable("RAG DB directory not found: nope")

    layer = ToolLayer(FakeBridge(), rag_search=broken)
    st = fresh_state()
    with pytest.raises(ToolError, match="RAG unavailable"):
        layer.call("search_docs", {"query": "질문"}, st)
    assert st.docs_searched is False
    with pytest.raises(EvidenceRequired):
        layer.call("file_write", {"path": "a.eps", "code": "1"}, st)


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


async def test_mcp_shim_advertises_server_schema_verbatim(tmp_path, monkeypatch):
    """EUD-087 regression: the shim must advertise the server's params JSON
    schema as the MCP inputSchema. The old FastMCP wrapper derived the schema
    from its ``_tool(args: dict)`` signature, so codex never saw the real
    parameter names and invented its own (table/field/id for xdat_get)."""
    import json

    import mcp.types as types

    from eud_agent import mcp_shim

    ready = tmp_path / "server.ready"
    ready.write_text(
        json.dumps({"port": 9999, "token": "tok"}), encoding="utf-8"
    )
    monkeypatch.setenv("EUD_DATA_DIR", str(tmp_path))
    params = {
        "type": "object",
        "properties": {
            "dat": {"type": "string"},
            "name": {"type": "string"},
            "objId": {"type": "integer"},
        },
        "required": ["dat", "name", "objId"],
        "additionalProperties": False,
    }
    monkeypatch.setattr(
        mcp_shim, "fetch_tool_specs",
        lambda port, token: [
            {"name": "xdat_get", "description": "desc", "parameters": params}
        ],
    )
    server = mcp_shim.build_server()
    handler = server.request_handlers[types.ListToolsRequest]
    result = await handler(types.ListToolsRequest(method="tools/list"))
    tools = result.root.tools
    assert [t.name for t in tools] == ["xdat_get"]
    assert tools[0].inputSchema == params  # VERBATIM, not a generic args:dict


async def test_mcp_shim_call_tool_forwards_and_validates(tmp_path, monkeypatch):
    """The lowlevel call path: args forwarded verbatim (no {'args': ...}
    wrapper), result wrapped as text content; schema-invalid args are
    rejected SHIM-SIDE (validate_input) as an isError tool result."""
    import json

    import mcp.types as types

    from eud_agent import mcp_shim

    ready = tmp_path / "server.ready"
    ready.write_text(
        json.dumps({"port": 9999, "token": "tok"}), encoding="utf-8"
    )
    monkeypatch.setenv("EUD_DATA_DIR", str(tmp_path))
    monkeypatch.setattr(
        mcp_shim, "fetch_tool_specs",
        lambda port, token: [{
            "name": "tbl_get", "description": "d",
            "parameters": {
                "type": "object",
                "properties": {"index": {"type": "integer"}},
                "required": ["index"],
                "additionalProperties": False,
            },
        }],
    )
    seen = {}

    def fake_forward(port, token, request_id, tool, args, **kw):
        seen["call"] = (tool, args)
        return "OK: 5 = text"

    monkeypatch.setattr(mcp_shim, "forward_call", fake_forward)
    server = mcp_shim.build_server()
    handler = server.request_handlers[types.CallToolRequest]

    ok = await handler(types.CallToolRequest(
        method="tools/call",
        params=types.CallToolRequestParams(
            name="tbl_get", arguments={"index": 5}
        ),
    ))
    assert seen["call"] == ("tbl_get", {"index": 5})
    assert not ok.root.isError
    assert "OK: 5 = text" in ok.root.content[0].text

    bad = await handler(types.CallToolRequest(
        method="tools/call",
        params=types.CallToolRequestParams(
            name="tbl_get", arguments={"idx": "five"}
        ),
    ))
    assert bad.root.isError  # rejected by the advertised schema, shim-side
    assert seen["call"] == ("tbl_get", {"index": 5})  # never forwarded


# --------------------------------------------------------------------------- #
# memory_write tool (features/07 "MCP tool: memory_write", Decision 07). The tool
# is a JOURNALED write that records a durable project-memory fact, but it is
# PLAN-GATE EXEMPT (recording a fact must never force propose_plan) while still
# consuming the 30-action budget like any other tool call. The store is injected
# into the ToolLayer the SAME way the journal is (an additive, optional seam):
# ``ToolLayer(bridge, memory=<ProjectMemory>)``. A layer built WITHOUT a memory
# store still rejects memory_write with a ToolError (no project open).
#
# These tests fail in Step A because memory_write is not in the registry yet.
# --------------------------------------------------------------------------- #


def make_memory_layer(tmp_path, *, project_name="MyMap"):
    """A ToolLayer wired to a real ProjectMemory store rooted under tmp_path.

    Mirrors make_layer() but injects a ProjectMemory the same additive way the
    journal is injected, so memory_write has a real store to write through.
    """
    from eud_agent.memory import ProjectMemory

    bridge = FakeBridge()
    mem = ProjectMemory(data_dir=str(tmp_path / "data"), project_name=project_name)
    layer = ToolLayer(bridge, memory=mem)
    return bridge, layer, mem


def test_registry_has_memory_write_tool():
    _, layer = make_layer()
    assert layer.has_tool("memory_write")
    # memory_write is a write tool (journaled) — it IS mutating for journaling,
    # but the plan gate exempts it (asserted separately below).
    assert layer.is_mutating("memory_write")


def test_memory_write_spec_advertises_file_enum_and_content():
    _, layer = make_layer()
    spec = next(s for s in layer.tool_specs() if s["name"] == "memory_write")
    assert spec.get("description")
    params = spec["parameters"]
    props = params["properties"]
    # file: enum of the four memory files; content: a string (full replacement).
    assert set(props["file"].get("enum", [])) == {
        "resources", "structure", "conventions", "lessons"
    }
    assert props["content"]["type"] == "string"
    assert set(params["required"]) == {"file", "content"}


def test_memory_write_rejects_unknown_file_enum_nothing_written(tmp_path):
    bridge, layer, mem = make_memory_layer(tmp_path)
    st = fresh_state()
    with pytest.raises(ToolError):
        layer.call("memory_write", {"file": "nope", "content": "x"}, st)
    # nothing reached the disk store (no markdown file created).
    assert mem.read("nope") == ""
    assert not (mem.store_dir / "nope.md").exists()
    # the rejected call did not touch the bridge nor count an action.
    assert bridge.calls == []
    assert st.action_count == 0


def test_memory_write_rejects_over_cap_content_nothing_written(tmp_path):
    from eud_agent.memory import CONTENT_CAP_BYTES

    bridge, layer, mem = make_memory_layer(tmp_path)
    st = fresh_state()
    oversize = "a" * (CONTENT_CAP_BYTES + 1)
    with pytest.raises(ToolError) as ei:
        layer.call("memory_write", {"file": "resources", "content": oversize}, st)
    # the error tells codex to condense.
    assert "condense" in str(ei.value).lower()
    # the prior (absent) file content is intact: nothing written.
    assert mem.read("resources") == ""
    assert st.action_count == 0


def test_memory_write_no_project_open_is_tool_error(tmp_path):
    """A ToolLayer whose memory store is DISABLED (no project open -> empty name)
    rejects memory_write with a ToolError explaining no project is open."""
    from eud_agent.memory import ProjectMemory

    bridge = FakeBridge()
    disabled = ProjectMemory(data_dir=str(tmp_path / "data"), project_name="")
    assert disabled.enabled is False
    layer = ToolLayer(bridge, memory=disabled)
    with pytest.raises(ToolError) as ei:
        layer.call(
            "memory_write", {"file": "lessons", "content": "x"}, fresh_state()
        )
    assert "project" in str(ei.value).lower()


def test_memory_write_without_injected_store_is_tool_error():
    """A ToolLayer built WITHOUT a memory store rejects memory_write (no project
    open) rather than crashing — same degradation contract as the disabled store."""
    _, layer = make_layer()  # no memory= injection
    with pytest.raises(ToolError):
        layer.call(
            "memory_write", {"file": "lessons", "content": "x"}, fresh_state()
        )


def test_memory_write_valid_call_writes_via_store(tmp_path):
    import json

    bridge, layer, mem = make_memory_layer(tmp_path)
    st = fresh_state()
    out = layer.call(
        "memory_write",
        {"file": "resources", "content": "switch 12 = doorOpen\n"},
        st,
    )
    # the content landed in the injected store's file.
    assert mem.read("resources") == "switch 12 = doorOpen\n"
    # memory_write is not a bridge command — the fake bridge saw nothing.
    assert bridge.calls == []
    # the return is a plain JSON-serializable dict (NOT the WriteResult dataclass):
    # the /tools/call endpoint json-dumps the result, so this boundary must hold.
    assert out == {"ok": True, "file": "resources"}
    assert json.dumps(out)  # must not raise (TypeError on a non-serializable value)


def test_memory_write_does_not_trip_plan_gate(tmp_path):
    """Three consecutive memory_write calls must NOT trip the 3-mutation plan gate
    (recording a fact must never force propose_plan), but each DOES consume the
    30-action budget."""
    bridge, layer, mem = make_memory_layer(tmp_path)
    st = fresh_state()
    layer.call("memory_write", {"file": "resources", "content": "r1"}, st)
    layer.call("memory_write", {"file": "structure", "content": "s1"}, st)
    # the THIRD write would be gated for a normal write tool; memory_write is
    # exempt, so it succeeds without a PlanRequired.
    layer.call("memory_write", {"file": "conventions", "content": "c1"}, st)
    assert mem.read("conventions") == "c1"
    # plan gate untouched: mutation_count did not advance toward the gate.
    assert st.mutation_count == 0
    # but each call consumed an action-budget slot.
    assert st.action_count == 3


def test_memory_write_counts_toward_action_budget(tmp_path):
    """memory_write is exempt from the plan gate but NOT from the 30-action budget:
    after the budget is spent the next memory_write is rejected with BudgetExceeded."""
    bridge, layer, mem = make_memory_layer(tmp_path)
    st = fresh_state()
    for _ in range(30):
        layer.call("project_status", {}, st)
    assert st.action_count == 30
    with pytest.raises(BudgetExceeded):
        layer.call("memory_write", {"file": "lessons", "content": "x"}, st)


def test_memory_write_structure_refreshes_list_hash(tmp_path):
    """Per the staleness rule, a memory_write targeting ``structure`` refreshes the
    stored LIST hash so is_stale() goes false against the current LIST reply."""
    from eud_agent.memory import list_hash

    bridge, layer, mem = make_memory_layer(tmp_path)
    st = fresh_state()
    # bridge.list_files() is canned; the staleness signal is keyed on the LIST
    # reply text, so drive the layer with the same LIST text the store will hash.
    list_reply = "a.eps\tCUIEps\nb.eps\tCUIEps\n"
    # before the write the store has no hash -> stale.
    assert mem.is_stale(list_reply) is True
    layer.call(
        "memory_write",
        {"file": "structure", "content": "a.eps: entry point\n"},
        st,
        list_reply=list_reply,
    )
    assert mem.read_meta().get("list_hash") == list_hash(list_reply)
    assert mem.is_stale(list_reply) is False


def test_memory_write_non_structure_does_not_refresh_list_hash(tmp_path):
    """Only a ``structure`` write refreshes the LIST hash; a write to another file
    leaves the staleness signal unchanged."""
    bridge, layer, mem = make_memory_layer(tmp_path)
    st = fresh_state()
    list_reply = "a.eps\tCUIEps\n"
    layer.call(
        "memory_write",
        {"file": "resources", "content": "r"},
        st,
        list_reply=list_reply,
    )
    # no structure write happened -> no recorded hash.
    assert mem.read_meta().get("list_hash") in (None, "")
