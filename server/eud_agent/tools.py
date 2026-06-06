"""eud-tools: the tool registry, server-side validation, mutation gate, budgets.

This is the policy layer of the v2 agent core (features/05 "Tools (registry)" +
"Triage and plan gating"). codex calls editor tools through the ``eud-tools`` MCP
shim (``mcp_shim.py``), which is DUMB TRANSPORT: every tool definition, argument
validation, mutation gate, and per-request budget lives HERE, in the FastAPI
process. The shim only forwards JSON to ``/tools/call`` with the ``server.ready``
token (architecture.md: "All tool logic, validation, journaling, and budget live
in the FastAPI process — the shim is dumb transport").

What this module owns:

  * **Registry** — every spec tool (read / write / flow) mapped to a ``BridgeIO``
    method, with a small JSON-schema params spec the shim advertises.
  * **Validation FIRST** — each tool validates its args (ranges, index bounds,
    type whitelists, FileType guards) BEFORE the bridge call, REUSING the
    ``bridge_io`` helpers (``_require_in`` / ``_require_nonneg_int`` /
    ``_require_numeric_value`` / ``_require_pathlike`` / ``_normalize_req_payload``
    and the whitelists). The bridge's ``ERROR:`` reply is the SECOND line of
    defense (features/05). A validation failure raises :class:`ToolError`, which
    the endpoint turns into an ``ok=false`` tool result (codex can correct it),
    NOT an HTTP 5xx.
  * **Mutation gate** — the 3rd MUTATING call WITHOUT an approved plan raises
    :class:`PlanRequired` directing codex to ``propose_plan``; after the request's
    ``plan_approved`` flag is set, the gate lifts (features/05 "Triage and plan
    gating", mechanical not advisory).
  * **Budgets** — 30 tool actions per request (the 31st raises
    :class:`BudgetExceeded` with a wrap-up message); 3 build self-fix attempts
    (tracked on the request state; the loop itself is a later task). The budget is
    queryable (``RequestState.budget_snapshot``) for panel display.

Journaling (snapshots / rollback, EUD-055) is wired in additively: ``ToolLayer``
takes an optional ``journal_factory``; when present, each WRITE tool snapshots
BEFORE mutating (after the gate/budget/validation pass) and records ``after``
AFTER the bridge write. A ToolLayer built without a factory behaves exactly as
before (reads/flow never journal). See ``journal.py``.

Build self-fix (EUD-057) is wired in the SAME additive way: ``ToolLayer`` takes an
optional ``runner_factory`` (a zero-arg factory returning an ``EddRunner``-shaped
object with ``build_run() -> BuildRunResult`` and a ``last_result`` attribute).
When present, ``build_run`` routes through the runner pipeline (BUILD -> poll ->
error ladder) instead of the plain ``bridge.build()``, ``build_errors`` returns
the LAST build's structured ladder errors (kept on the runner's ``last_result``),
and each ``build_run`` consumes one of the request's 3 build-fix attempts; the 4th
``build_run`` returns a :class:`ToolError` (the self-fix budget is spent) and sets
``RequestState.build_fix_exhausted`` so the engine can note the failure on the
changeset (build_run is never journaled, so it cannot be a changeset item). A real
build failure or a poll/subprocess timeout consumes an attempt; a STATIC
misconfiguration (``edd_runner.ConfigError``: unset euddraft / eds path) is
re-raised as a ToolError WITHOUT consuming an attempt (codex cannot fix it by
editing eps; 3 misconfigs must not exhaust the budget). A ToolLayer built WITHOUT a
runner_factory keeps the current plain ``bridge.build()`` behavior (existing
constructions keep working). See ``edd_runner.py``.

Map info (features/08) is wired in the SAME additive way: ``ToolLayer`` takes an
optional ``map_info`` (a :class:`chk_info.MapInfoService`). When present the
``map_info`` READ tool returns the connected map's SCMD2-set data (locations /
units / forces) by extracting + parsing the raw CHK; absent (or a misconfigured
IsomTerrain.exe) makes the tool a clear ToolError while NOTHING else in the flow
changes (the same advisory shape as epscript-lsp). See ``chk_info.py``.

Project memory (EUD-079, features/07 "MCP tool: memory_write") is wired in the SAME
additive way: ``ToolLayer`` takes an optional ``memory`` (a :class:`ProjectMemory`
store). When present the ``memory_write`` tool records a durable project-memory
fact (a full-file replacement of one of resources/structure/conventions/lessons)
THROUGH that store. ``memory_write`` is ``kind="write"`` so it is journaled and
consumes the 30-action budget, but it is PLAN-GATE EXEMPT (recording a fact must
never force ``propose_plan``): it never raises :class:`PlanRequired` and does NOT
advance the mutation counter. The tool layer is the FIRST line of defense — it
checks the ``file`` enum, the 8 192-byte UTF-8 cap, and the disabled/missing store
BEFORE any disk write (the store re-validates as the second line). A ``structure``
write also refreshes the store's LIST hash (the staleness signal) from the
``list_reply`` the engine threads through. A ToolLayer built WITHOUT a ``memory``
store rejects ``memory_write`` with a ToolError ("no project is open"). See
``memory.py``.
"""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass
from typing import Any

from .bridge_io import (
    _CREATABLE_TYPES,
    _DAT_NAMES,
    _PROGRAM_WRITABLE_KEYS,
    _REQ_DATS,
    _RESET_KINDS,
    _SETTING_SCOPES,
    _XDAT_KINDS,
    BridgeError,
    _normalize_req_payload,
    _require_in,
    _require_nonneg_int,
    _require_numeric_value,
    _require_pathlike,
)
from .chk_info import LOCATION_ACTIONS, MapInfoError
from .memory import CONTENT_CAP_BYTES, MEMORY_FILES

# Budgets (features/05 "Triage and plan gating"). Pinned constants; the request
# state exposes them in budget_snapshot for the panel.
ACTION_BUDGET = 30
BUILD_FIX_LIMIT = 3
# The gate fires on the Nth mutating call WITHOUT a plan. "small edits (<=2
# mutations) may apply directly; the 3rd mutating call without a plan is blocked".
MUTATION_GATE_THRESHOLD = 3

# Flow tool name (ends the codex turn for plan review).
PLAN_TOOL = "propose_plan"

# Project-memory write tool (EUD-079). A WRITE (journaled + budgeted) that is
# PLAN-GATE EXEMPT: recording a durable fact must never force a propose_plan.
MEMORY_TOOL = "memory_write"

# Map-info read tool (features/08). Routed to the injected MapInfoService (the
# editor exposes only the OpenMapName path string — the CHK is read from disk).
MAP_INFO_TOOL = "map_info"
_MAP_INFO_MODES = ("summary", "locations", "units", "players")

# Location write tool (features/09). A REAL write: plan-gated, budgeted, and
# journaled (snapshot = the service's full-file map backup; reject restores it).
LOCATION_TOOL = "location_write"


# --------------------------------------------------------------------------- #
# Errors. All are ToolError so the endpoint surfaces them uniformly as a tool
# result codex can read/correct (NOT a transport crash).
# --------------------------------------------------------------------------- #


class ToolError(Exception):
    """A tool call could not be performed (bad args, unknown tool, gate/budget).

    The endpoint returns ``{"ok": false, "error": str(exc)}`` for any ToolError
    so codex sees a correctable tool result rather than an HTTP 5xx.
    """


class PlanRequired(ToolError):
    """The mutation gate blocked a write because no plan is approved yet.

    The message directs codex to ``propose_plan`` (features/05).
    """


class BudgetExceeded(ToolError):
    """The per-request action budget is spent; codex is told to wrap up."""


# --------------------------------------------------------------------------- #
# Mutation gate: a standalone testable unit.
# --------------------------------------------------------------------------- #


@dataclass
class MutationGate:
    """Counts mutating calls per request; blocks the Nth without an approved plan.

    ``allow(mutations_so_far, plan_approved)`` returns whether the NEXT mutating
    call is permitted: with an approved plan it is always allowed; otherwise it is
    allowed only while ``mutations_so_far`` is below ``threshold - 1`` (so the 1st
    and 2nd writes pass, the 3rd is blocked — features/05).
    """

    threshold: int = MUTATION_GATE_THRESHOLD

    def allow(self, *, mutations_so_far: int, plan_approved: bool) -> bool:
        if plan_approved:
            return True
        return mutations_so_far < (self.threshold - 1)


# --------------------------------------------------------------------------- #
# Per-request state: budgets, mutation counter, plan flags. Keyed by request_id
# on the ToolLayer; exposed (budget_snapshot) for panel display.
# --------------------------------------------------------------------------- #


@dataclass
class RequestState:
    """Mutable state for ONE agent request (keyed by ``request_id``).

    Holds the action budget counter, the mutation counter (drives the plan gate),
    the plan flags, and the build self-fix attempt counter. ``budget_snapshot``
    is the panel-facing view (features/05: "Budget state must be exposed for panel
    display").
    """

    request_id: str
    action_count: int = 0
    mutation_count: int = 0
    plan_approved: bool = False
    plan_proposed: bool = False
    build_fix_attempts: int = 0
    action_limit: int = ACTION_BUDGET
    build_fix_limit: int = BUILD_FIX_LIMIT
    # Set when the 4th build_run in a request is rejected (the self-fix budget is
    # spent). build_run is NOT journaled (EUD-055 decision), so it can never be a
    # changeset item; this flag is the minimal honest mechanism the engine reads
    # to surface "build failed; self-fix budget spent" as a changeset note
    # (features/05: "after which the changeset is presented with the failure
    # noted"). See ToolLayer's build_run path.
    build_fix_exhausted: bool = False

    def approve_plan(self) -> None:
        """Lift the mutation gate for this request (panel sent ``plan_approve``)."""
        self.plan_approved = True

    def record_build_fix_attempt(self) -> None:
        self.build_fix_attempts += 1

    def budget_snapshot(self) -> dict[str, int]:
        """Panel-facing budget view (queryable counter)."""
        return {
            "actions_used": self.action_count,
            "actions_limit": self.action_limit,
            "actions_remaining": max(0, self.action_limit - self.action_count),
            "mutations": self.mutation_count,
            "plan_approved": int(self.plan_approved),
            "build_fix_attempts": self.build_fix_attempts,
            "build_fix_limit": self.build_fix_limit,
            "build_fix_exhausted": int(self.build_fix_exhausted),
        }


# --------------------------------------------------------------------------- #
# Tool spec: a registry entry. ``handler(bridge, args) -> result`` runs AFTER the
# gate/budget checks; it validates args (reusing bridge_io helpers) then calls the
# mapped BridgeIO method. ``mutating`` drives the gate + mutation counter.
# --------------------------------------------------------------------------- #


@dataclass(frozen=True)
class ToolSpec:
    name: str
    kind: str  # "read" | "write" | "flow"
    description: str
    parameters: dict  # JSON-schema-ish (advertised by the MCP shim)
    handler: Callable[[Any, dict], Any]

    @property
    def mutating(self) -> bool:
        return self.kind == "write"


# --------------------------------------------------------------------------- #
# Arg extraction helpers (raise ToolError, not BridgeError, for MISSING args —
# argument SHAPE is the tool layer's job; VALUE validation reuses bridge_io and is
# translated to ToolError so codex sees one error family).
# --------------------------------------------------------------------------- #


def _req(args: dict, key: str) -> Any:
    if key not in args or args[key] is None:
        raise ToolError(f"missing required argument {key!r}")
    return args[key]


def _opt(args: dict, key: str, default: Any) -> Any:
    val = args.get(key, default)
    return default if val is None else val


def _require_int_min(value: object, minimum: int, label: str) -> int:
    """Coerce ``value`` to an int >= ``minimum``, raising ToolError otherwise.

    Used for indexes that allow a sentinel below 0 (e.g. plugin_add's ``-1``
    append), where ``_require_nonneg_int`` is too strict. A non-integer or an
    out-of-range value raises :class:`ToolError` (not a bare ValueError) so the
    endpoint surfaces a tool RESULT (ok=false), never an HTTP 5xx.
    """
    try:
        n = int(value)  # type: ignore[arg-type]
    except (TypeError, ValueError) as exc:
        raise ToolError(f"{label} must be an integer, got {value!r}") from exc
    if n < minimum:
        raise ToolError(f"{label} must be >= {minimum}, got {n}")
    return n


def _as_bridge_error_tool_error(fn: Callable[[], Any]) -> Any:
    """Run a bridge_io validation helper, translating BridgeError -> ToolError.

    The bridge_io helpers raise ``BridgeError`` for an out-of-contract value;
    inside the tool layer we re-raise as ``ToolError`` so the endpoint surfaces
    ONE error family. ``BridgeError`` from the actual bridge round-trip (the
    second line of defense) is translated the same way at call time.
    """
    try:
        return fn()
    except BridgeError as exc:
        raise ToolError(str(exc)) from exc


# --------------------------------------------------------------------------- #
# Param schema fragments (kept tiny; the shim advertises them).
# --------------------------------------------------------------------------- #


def _schema(props: dict, required: list[str]) -> dict:
    return {
        "type": "object",
        "properties": props,
        "required": required,
        "additionalProperties": False,
    }


_STR = {"type": "string"}
_INT = {"type": "integer"}
_NUM = {"type": ["integer", "string"]}


# --------------------------------------------------------------------------- #
# Handlers. Each validates (reusing bridge_io helpers) then calls the mapped
# BridgeIO method. Validation runs BEFORE the bridge call (features/05).
# --------------------------------------------------------------------------- #


# ----- read handlers -----
def _h_project_status(bridge, args):
    return bridge.status()


def _h_list_files(bridge, args):
    return bridge.list_files()


def _h_read_file(bridge, args):
    path = _require_pathlike_te(_req(args, "path"), "path")
    return bridge.get(path)


def _h_dat_get(bridge, args):
    dat = _as_bridge_error_tool_error(
        lambda: _require_in(_req(args, "dat"), _DAT_NAMES, "dat name")
    )
    obj_id = _as_bridge_error_tool_error(
        lambda: _require_nonneg_int(_req(args, "objId"), "objId")
    )
    return bridge.getdat(dat, str(_req(args, "param")), obj_id)


def _h_xdat_get(bridge, args):
    dat = _as_bridge_error_tool_error(
        lambda: _require_in(_req(args, "dat"), _XDAT_KINDS, "xdat kind")
    )
    obj_id = _as_bridge_error_tool_error(
        lambda: _require_nonneg_int(_req(args, "objId"), "objId")
    )
    return bridge.getxdat(dat, str(_req(args, "name")), obj_id)


def _h_tbl_get(bridge, args):
    index = _as_bridge_error_tool_error(
        lambda: _require_nonneg_int(_req(args, "index"), "index")
    )
    return bridge.gettbl(index)


def _h_req_get(bridge, args):
    dat = _as_bridge_error_tool_error(
        lambda: _require_in(_req(args, "dat"), _REQ_DATS, "req dat")
    )
    obj_id = _as_bridge_error_tool_error(
        lambda: _require_nonneg_int(_req(args, "objId"), "objId")
    )
    return bridge.getreq(dat, obj_id)


def _h_btn_get(bridge, args):
    set_id = _as_bridge_error_tool_error(
        lambda: _require_nonneg_int(_req(args, "setId"), "setId")
    )
    return bridge.getbtn(set_id)


def _h_settings_get(bridge, args):
    scope = _as_bridge_error_tool_error(
        lambda: _require_in(_req(args, "scope"), _SETTING_SCOPES, "scope")
    )
    # key whitelist is scope-dependent; let the bridge wrapper enforce it (it
    # raises BridgeError -> ToolError before sending).
    return _as_bridge_error_tool_error(
        lambda: bridge.getset(scope, str(_req(args, "key")))
    )


def _h_plugins_list(bridge, args):
    return bridge.pluglist()


def _h_build_errors(bridge, args):
    return bridge.builderr()


def _h_map_info(bridge, args):
    """Validate ``map_info`` args; the service call is routed in ``call``.

    First line of defense (features/08): ``mode`` must be one of
    :data:`_MAP_INFO_MODES`; the filters are free strings (the service matches
    them loosely). No bridge call here — map_info targets the injected
    :class:`MapInfoService`, not the editor bridge; the actual read happens in
    :meth:`ToolLayer._map_info_via_service` after this passes.
    """
    mode = str(_opt(args, "mode", "summary"))
    if mode not in _MAP_INFO_MODES:
        raise ToolError(
            f"map_info mode must be one of {', '.join(_MAP_INFO_MODES)}; "
            f"got {mode!r}"
        )
    return {
        "mode": mode,
        "owner": str(_opt(args, "owner", "")),
        "unit_type": str(_opt(args, "unitType", "")),
    }


def _h_search_docs(bridge, args):
    """RAG top-k over the ECA store.

    TODO(EUD-055+): wire the in-process RAG (rag.search) once the tool layer has a
    handle to the rag_db path; the tool layer is constructed with only a bridge
    today. The tool EXISTS and validates its args now (features/05 guidance: do
    NOT block the whole task on RAG plumbing). Returns [] until wired.
    """
    query = _req(args, "query")
    if not str(query).strip():
        raise ToolError("search_docs: query must be non-empty")
    _ = _opt(args, "k", 5)  # validated shape; depth used once RAG is wired
    return []


# ----- write handlers -----
def _h_dat_set(bridge, args):
    dat = _as_bridge_error_tool_error(
        lambda: _require_in(_req(args, "dat"), _DAT_NAMES, "dat name")
    )
    obj_id = _as_bridge_error_tool_error(
        lambda: _require_nonneg_int(_req(args, "objId"), "objId")
    )
    value = _as_bridge_error_tool_error(
        lambda: _require_numeric_value(_req(args, "value"), "value")
    )
    return bridge.setdat(dat, str(_req(args, "param")), obj_id, value)


def _h_xdat_set(bridge, args):
    dat = _as_bridge_error_tool_error(
        lambda: _require_in(_req(args, "dat"), _XDAT_KINDS, "xdat kind")
    )
    obj_id = _as_bridge_error_tool_error(
        lambda: _require_nonneg_int(_req(args, "objId"), "objId")
    )
    value = _as_bridge_error_tool_error(
        lambda: _require_numeric_value(_req(args, "value"), "value")
    )
    return bridge.setxdat(dat, str(_req(args, "name")), obj_id, value)


def _h_tbl_set(bridge, args):
    index = _as_bridge_error_tool_error(
        lambda: _require_nonneg_int(_req(args, "index"), "index")
    )
    return bridge.settbl(index, str(_req(args, "value")))


def _h_req_set(bridge, args):
    dat = _as_bridge_error_tool_error(
        lambda: _require_in(_req(args, "dat"), _REQ_DATS, "req dat")
    )
    obj_id = _as_bridge_error_tool_error(
        lambda: _require_nonneg_int(_req(args, "objId"), "objId")
    )
    payload = _as_bridge_error_tool_error(
        lambda: _normalize_req_payload(str(_req(args, "payload")))
    )
    return bridge.setreq(dat, obj_id, payload)


def _h_btn_set(bridge, args):
    set_id = _as_bridge_error_tool_error(
        lambda: _require_nonneg_int(_req(args, "setId"), "setId")
    )
    return bridge.setbtn(set_id, str(_req(args, "csv")))


def _h_dat_reset(bridge, args):
    kind = _as_bridge_error_tool_error(
        lambda: _require_in(_req(args, "kind"), _RESET_KINDS, "reset kind")
    )
    obj_id = _as_bridge_error_tool_error(
        lambda: _require_nonneg_int(_req(args, "objId"), "objId")
    )
    return _as_bridge_error_tool_error(
        lambda: bridge.resetdat(
            kind, str(_opt(args, "dat", "")), str(_opt(args, "param", "")), obj_id
        )
    )


def _h_file_create(bridge, args):
    path = _require_pathlike_te(_req(args, "path"), "path")
    ftype = _as_bridge_error_tool_error(
        lambda: _require_in(_req(args, "ftype"), _CREATABLE_TYPES, "file type")
    )
    return bridge.newfile(path, ftype, str(_opt(args, "code", "")))


def _h_file_write(bridge, args):
    path = _require_pathlike_te(_req(args, "path"), "path")
    return bridge.set(path, str(_req(args, "code")))


def _h_file_rename(bridge, args):
    path = _require_pathlike_te(_req(args, "path"), "path")
    newname = _req(args, "newname")
    if not str(newname).strip():
        raise ToolError("newname must be non-empty")
    return bridge.rename(path, str(newname))


def _h_file_delete(bridge, args):
    path = _require_pathlike_te(_req(args, "path"), "path")
    return bridge.delfile(path)


def _h_file_move(bridge, args):
    path = _require_pathlike_te(_req(args, "path"), "path")
    return bridge.movefile(path, str(_opt(args, "destFolder", "")))


def _h_mkdir(bridge, args):
    path = _require_pathlike_te(_req(args, "path"), "path")
    return bridge.mkdir(path)


def _h_set_main(bridge, args):
    path = _require_pathlike_te(_req(args, "path"), "path")
    return bridge.setmain(path)


def _h_settings_set(bridge, args):
    scope = _as_bridge_error_tool_error(
        lambda: _require_in(_req(args, "scope"), _SETTING_SCOPES, "scope")
    )
    key = str(_req(args, "key"))
    # Guard: program-scope non-writable keys (Language and any future read-only
    # key) are rejected here BEFORE the send. The bridge wrapper remains the
    # authoritative key-whitelist validator (it also rejects unknown keys).
    if scope == "program" and key not in _PROGRAM_WRITABLE_KEYS:
        raise ToolError(
            f"program setting key {key!r} is not writable "
            f"(writable: {', '.join(_PROGRAM_WRITABLE_KEYS)})"
        )
    return _as_bridge_error_tool_error(
        lambda: bridge.setset(scope, key, str(_req(args, "value")))
    )


def _h_plugin_add(bridge, args):
    # index allows the -1 append sentinel; validate to ToolError BEFORE the bridge
    # (a non-int / < -1 must be a tool result, not an HTTP 5xx).
    index = _require_int_min(_opt(args, "index", -1), -1, "index")
    texts = str(_opt(args, "texts", ""))
    return _as_bridge_error_tool_error(lambda: bridge.plugadd(index, texts))


def _h_plugin_edit(bridge, args):
    index = _as_bridge_error_tool_error(
        lambda: _require_nonneg_int(_req(args, "index"), "index")
    )
    return bridge.plugset(index, str(_opt(args, "texts", "")))


def _h_plugin_remove(bridge, args):
    index = _as_bridge_error_tool_error(
        lambda: _require_nonneg_int(_req(args, "index"), "index")
    )
    return bridge.plugdel(index)


def _h_plugin_move(bridge, args):
    frm = _as_bridge_error_tool_error(
        lambda: _require_nonneg_int(_req(args, "from"), "from index")
    )
    to = _as_bridge_error_tool_error(
        lambda: _require_nonneg_int(_req(args, "to"), "to index")
    )
    return bridge.plugmove(frm, to)


def _h_build_run(bridge, args):
    return bridge.build()


def _h_memory_write(bridge, args):
    """Validate a ``memory_write`` (enum + cap); the store write is routed in ``call``.

    This is the FIRST line of defense (features/07): the ``file`` must be one of
    :data:`MEMORY_FILES` and ``content`` must be within the
    :data:`CONTENT_CAP_BYTES` UTF-8 budget. No bridge call — memory_write targets
    the injected :class:`ProjectMemory` store, not the editor bridge; the actual
    write happens in :meth:`ToolLayer._memory_write_via_store` after this passes.
    The probe-bridge ``_validate_args`` path runs exactly these checks, so an
    arg-invalid call leaves no journal snapshot/entry behind.
    """
    file = _as_bridge_error_tool_error(
        lambda: _require_in(_req(args, "file"), MEMORY_FILES, "memory file")
    )
    content = str(_req(args, "content"))
    encoded = content.encode("utf-8")
    if len(encoded) > CONTENT_CAP_BYTES:
        raise ToolError(
            f"memory content is {len(encoded)} bytes, over the "
            f"{CONTENT_CAP_BYTES}-byte budget; condense it."
        )
    return {"file": file, "content": content}


def _h_location_write(bridge, args):
    """Validate ``location_write`` args; the service call is routed in ``call``.

    First line of defense (features/09): the ``action`` enum and the per-action
    required fields are checked here (the service re-validates rect/name rules
    as the second line; the locedit CLI is the third). Coordinates stay in TILE
    units — the service converts to px. No bridge call.
    """
    action = str(_req(args, "action"))
    if action not in LOCATION_ACTIONS:
        raise ToolError(
            f"location_write action must be one of "
            f"{', '.join(LOCATION_ACTIONS)}; got {action!r}"
        )
    needs_name = action in ("add", "rename")
    needs_rect = action in ("add", "set")
    needs_id = action in ("set", "rename", "delete")
    name = str(_opt(args, "name", ""))
    if needs_name and not name.strip():
        raise ToolError(f"location_write {action}: 'name' is required")
    location_id = 0
    if needs_id:
        location_id = _as_bridge_error_tool_error(
            lambda: _require_nonneg_int(_req(args, "locationId"), "locationId")
        )
    rect = {}
    if needs_rect:
        for key in ("tileLeft", "tileTop", "tileRight", "tileBottom"):
            rect[key] = _as_bridge_error_tool_error(
                lambda k=key: _require_nonneg_int(_req(args, k), k)
            )
    return {
        "action": action,
        "name": name,
        "location_id": location_id,
        "left": rect.get("tileLeft", 0),
        "top": rect.get("tileTop", 0),
        "right": rect.get("tileRight", 0),
        "bottom": rect.get("tileBottom", 0),
    }


# ----- flow handler -----
def _h_propose_plan(bridge, args):
    """End the turn for user review (features/05). No bridge call; no mutation.

    Returns a sentinel the endpoint/runner uses to end the codex turn and render
    the plan in the panel. The request state's ``plan_proposed`` flag is set by
    the ToolLayer (it knows the request state).
    """
    markdown = _req(args, "markdown")
    if not str(markdown).strip():
        raise ToolError("propose_plan: markdown must be non-empty")
    return {"ends_turn": True, "markdown": str(markdown)}


def _require_pathlike_te(value, label):
    """``_require_pathlike`` (bridge_io) but raising ToolError for the tool layer."""
    return _as_bridge_error_tool_error(lambda: _require_pathlike(value, label))


# --------------------------------------------------------------------------- #
# Registry construction.
# --------------------------------------------------------------------------- #


def _build_registry() -> dict[str, ToolSpec]:
    specs: list[ToolSpec] = [
        # ---- read ----
        ToolSpec(
            "project_status", "read",
            "Editor state: whether a build is compiling and the open project.",
            _schema({}, []), _h_project_status,
        ),
        ToolSpec(
            "list_files", "read",
            "Project file tree as a list of {path, ftype, settable}.",
            _schema({}, []), _h_list_files,
        ),
        ToolSpec(
            "read_file", "read",
            "Read a project file's text by its project path.",
            _schema({"path": _STR}, ["path"]), _h_read_file,
        ),
        ToolSpec(
            "dat_get", "read",
            "Read a standard dat field (units/weapons/... param) for an object id.",
            _schema({"dat": _STR, "param": _STR, "objId": _INT},
                    ["dat", "param", "objId"]),
            _h_dat_get,
        ),
        ToolSpec(
            "xdat_get", "read",
            "Read an ExtraDat field (statusinfor/wireframe/ButtonSet).",
            _schema({"dat": _STR, "name": _STR, "objId": _INT},
                    ["dat", "name", "objId"]),
            _h_xdat_get,
        ),
        ToolSpec(
            "tbl_get", "read",
            "Read a stat_txt/tbl string by index.",
            _schema({"index": _INT}, ["index"]), _h_tbl_get,
        ),
        ToolSpec(
            "req_get", "read",
            "Read a requirement as the editor copy-string.",
            _schema({"dat": _STR, "objId": _INT}, ["dat", "objId"]), _h_req_get,
        ),
        ToolSpec(
            "btn_get", "read",
            "Read a button set as the editor CSV.",
            _schema({"setId": _INT}, ["setId"]), _h_btn_get,
        ),
        ToolSpec(
            "settings_get", "read",
            "Read a project/program setting value.",
            _schema({"scope": _STR, "key": _STR}, ["scope", "key"]),
            _h_settings_get,
        ),
        ToolSpec(
            "plugins_list", "read",
            "List the eds plugin blocks.",
            _schema({}, []), _h_plugins_list,
        ),
        ToolSpec(
            "build_errors", "read",
            "Macro/eps errors accumulated by the last build.",
            _schema({}, []), _h_build_errors,
        ),
        ToolSpec(
            "search_docs", "read",
            "RAG top-k search over the ECA epScript document store.",
            _schema({"query": _STR, "k": _INT}, ["query"]), _h_search_docs,
        ),
        ToolSpec(
            MAP_INFO_TOOL, "read",
            "Read the connected map's SCMD2-set data from the OpenMapName file "
            "on disk (last saved state): locations, unit placement, "
            "forces/teams, players. mode: summary|locations|units|players; "
            "units mode accepts owner (P1..P12|neutral) and unitType "
            "(id or name substring) filters.",
            _schema(
                {
                    "mode": {"type": "string", "enum": list(_MAP_INFO_MODES)},
                    "owner": _STR,
                    "unitType": _STR,
                },
                [],
            ),
            _h_map_info,
        ),
        # ---- write ----
        ToolSpec(
            "dat_set", "write",
            "Write a standard dat field (numeric value).",
            _schema({"dat": _STR, "param": _STR, "objId": _INT, "value": _NUM},
                    ["dat", "param", "objId", "value"]),
            _h_dat_set,
        ),
        ToolSpec(
            "xdat_set", "write",
            "Write an ExtraDat field (numeric value).",
            _schema({"dat": _STR, "name": _STR, "objId": _INT, "value": _NUM},
                    ["dat", "name", "objId", "value"]),
            _h_xdat_set,
        ),
        ToolSpec(
            "tbl_set", "write",
            "Write a stat_txt/tbl string (NULLSTRING resets to default).",
            _schema({"index": _INT, "value": _STR}, ["index", "value"]),
            _h_tbl_set,
        ),
        ToolSpec(
            "req_set", "write",
            "Write a requirement (use-mode keyword or copy-string).",
            _schema({"dat": _STR, "objId": _INT, "payload": _STR},
                    ["dat", "objId", "payload"]),
            _h_req_set,
        ),
        ToolSpec(
            "btn_set", "write",
            "Write a button set (editor CSV).",
            _schema({"setId": _INT, "csv": _STR}, ["setId", "csv"]), _h_btn_set,
        ),
        ToolSpec(
            "dat_reset", "write",
            "Reset a dat/xdat/tbl field to its stock value.",
            _schema(
                {"kind": _STR, "dat": _STR, "param": _STR, "objId": _INT},
                ["kind", "objId"],
            ),
            _h_dat_reset,
        ),
        ToolSpec(
            "file_create", "write",
            "Create a file of type CUIEps/CUIPy/RawText at a project path.",
            _schema({"path": _STR, "ftype": _STR, "code": _STR},
                    ["path", "ftype"]),
            _h_file_create,
        ),
        ToolSpec(
            "file_write", "write",
            "Replace a file's text (memory-only; CUI/RawText only).",
            _schema({"path": _STR, "code": _STR}, ["path", "code"]),
            _h_file_write,
        ),
        ToolSpec(
            "file_rename", "write",
            "Rename a project node.",
            _schema({"path": _STR, "newname": _STR}, ["path", "newname"]),
            _h_file_rename,
        ),
        ToolSpec(
            "file_delete", "write",
            "Delete a project node.",
            _schema({"path": _STR}, ["path"]), _h_file_delete,
        ),
        ToolSpec(
            "file_move", "write",
            "Move a node into a destination folder (empty = project root).",
            _schema({"path": _STR, "destFolder": _STR}, ["path"]), _h_file_move,
        ),
        ToolSpec(
            "mkdir", "write",
            "Create a folder (nested ok).",
            _schema({"path": _STR}, ["path"]), _h_mkdir,
        ),
        ToolSpec(
            "set_main", "write",
            "Point MainFile at the node at the given path.",
            _schema({"path": _STR}, ["path"]), _h_set_main,
        ),
        ToolSpec(
            "settings_set", "write",
            "Write a project/program setting (Language is read-only).",
            _schema({"scope": _STR, "key": _STR, "value": _STR},
                    ["scope", "key", "value"]),
            _h_settings_set,
        ),
        ToolSpec(
            "plugin_add", "write",
            "Add a UserPlugin eds block (index=-1 appends).",
            _schema({"index": _INT, "texts": _STR}, []), _h_plugin_add,
        ),
        ToolSpec(
            "plugin_edit", "write",
            "Replace a UserPlugin block's Texts.",
            _schema({"index": _INT, "texts": _STR}, ["index"]), _h_plugin_edit,
        ),
        ToolSpec(
            "plugin_remove", "write",
            "Delete a UserPlugin block.",
            _schema({"index": _INT}, ["index"]), _h_plugin_remove,
        ),
        ToolSpec(
            "plugin_move", "write",
            "Reorder an eds block.",
            _schema({"from": _INT, "to": _INT}, ["from", "to"]), _h_plugin_move,
        ),
        ToolSpec(
            "build_run", "write",
            "Run the editor build (SCArchive forced off, preflight paths).",
            _schema({}, []), _h_build_run,
        ),
        ToolSpec(
            LOCATION_TOOL, "write",
            "Edit the connected map's locations IN PLACE (saved to the "
            "OpenMapName .scx; visible in SCMDraft after reopen). action: "
            "add (name + tile rect, returns the assigned id) | set (locationId "
            "+ tile rect) | rename (locationId + name) | delete (locationId). "
            "Ids are never renumbered; #64 Anywhere is protected; a full-file "
            "backup is taken before every edit.",
            _schema(
                {
                    "action": {
                        "type": "string", "enum": list(LOCATION_ACTIONS),
                    },
                    "name": _STR,
                    "locationId": _INT,
                    "tileLeft": _INT,
                    "tileTop": _INT,
                    "tileRight": _INT,
                    "tileBottom": _INT,
                },
                ["action"],
            ),
            _h_location_write,
        ),
        ToolSpec(
            MEMORY_TOOL, "write",
            "Record a durable project-memory fact (full replacement of one memory "
            "file). Plan-gate exempt; journaled so the user can review/reject it.",
            _schema(
                {
                    "file": {"type": "string", "enum": list(MEMORY_FILES)},
                    "content": _STR,
                },
                ["file", "content"],
            ),
            _h_memory_write,
        ),
        # ---- flow ----
        ToolSpec(
            PLAN_TOOL, "flow",
            "Propose a markdown plan and end the turn for user review.",
            _schema({"markdown": _STR}, ["markdown"]), _h_propose_plan,
        ),
    ]
    return {s.name: s for s in specs}


_REGISTRY: dict[str, ToolSpec] = _build_registry()

# Public name sets (the test asserts completeness against these).
READ_TOOLS: tuple[str, ...] = tuple(
    s.name for s in _REGISTRY.values() if s.kind == "read"
)
WRITE_TOOLS: tuple[str, ...] = tuple(
    s.name for s in _REGISTRY.values() if s.kind == "write"
)
FLOW_TOOLS: tuple[str, ...] = tuple(
    s.name for s in _REGISTRY.values() if s.kind == "flow"
)


# --------------------------------------------------------------------------- #
# The tool layer: ties the registry to per-request gate/budget enforcement and a
# BridgeIO instance. The endpoint calls ``call_for_request`` (request_id keyed);
# unit tests can call ``call`` with an explicit RequestState.
# --------------------------------------------------------------------------- #


class _ProbeBridge:
    """A no-op bridge stand-in used ONLY for arg validation (``_validate_args``).

    Every attribute resolves to a callable that ignores its arguments and returns
    a benign reply string, so a handler's bridge call is absorbed while its
    BEFORE-the-bridge argument checks (which raise ToolError) still run. It is
    never used for a real mutation — the actual write goes through ``_dispatch``
    against the real bridge after a successful validation + snapshot.
    """

    def __getattr__(self, _name):
        def _noop(*_a, **_kw):
            return "OK"

        return _noop


class ToolLayer:
    """Validates + gates + budgets + routes tool calls to the bridge.

    One instance per server (the bridge is a singleton). Per-request state is
    tracked in ``_requests`` keyed by ``request_id`` so the action budget, the
    mutation counter (plan gate), and the plan-approved flag are isolated per
    agent request and queryable for the panel.
    """

    def __init__(
        self,
        bridge,
        *,
        gate: MutationGate | None = None,
        journal_factory: Callable[[str], Any] | None = None,
        runner_factory: Callable[[], Any] | None = None,
        memory: Any | None = None,
        map_info: Any | None = None,
    ) -> None:
        self._bridge = bridge
        self._gate = gate or MutationGate()
        self._requests: dict[str, RequestState] = {}
        # Optional/injectable map-info service (features/08). When present the
        # ``map_info`` READ tool digests the connected map's CHK through it;
        # absent makes ``map_info`` a clear ToolError (advisory shape, like
        # epscript-lsp). Additive: existing constructions keep working.
        self._map_info = map_info
        # Optional/injectable project-memory store (EUD-079). When present the
        # ``memory_write`` tool records a durable fact through it; absent (or a
        # disabled store) makes ``memory_write`` a ToolError ("no project is
        # open"). Additive: existing constructions keep working unchanged.
        self._memory = memory
        # Optional/injectable journal (EUD-055). When present, each WRITE tool
        # snapshots BEFORE mutating and records ``after`` AFTER; reads/flow never
        # journal. A ToolLayer built WITHOUT a factory behaves exactly as before
        # (additive integration; existing constructions keep working).
        self._journal_factory = journal_factory
        self._journals: dict[str, Any] = {}
        # Optional/injectable euddraft runner (EUD-057). A single instance per
        # ToolLayer (one editor build at a time; ``last_result`` is the LAST
        # build's errors, shared by build_run + build_errors). A ToolLayer built
        # WITHOUT a factory keeps the plain ``bridge.build()`` behavior.
        self._runner_factory = runner_factory
        self._runner: Any | None = None

    # ---- registry introspection (used by the shim/endpoint) ----
    def has_tool(self, name: str) -> bool:
        return name in _REGISTRY

    def is_mutating(self, name: str) -> bool:
        spec = _REGISTRY.get(name)
        return bool(spec and spec.mutating)

    def tool_specs(self) -> list[dict]:
        """Name + description + params for every tool (the shim advertises these)."""
        return [
            {"name": s.name, "description": s.description, "parameters": s.parameters}
            for s in _REGISTRY.values()
        ]

    # ---- request-state registry ----
    def get_request_state(self, request_id: str) -> RequestState:
        st = self._requests.get(request_id)
        if st is None:
            st = RequestState(request_id=request_id)
            self._requests[request_id] = st
        return st

    def approve_plan_for_request(self, request_id: str) -> None:
        self.get_request_state(request_id).approve_plan()

    def budget_snapshot(self, request_id: str) -> dict[str, int]:
        return self.get_request_state(request_id).budget_snapshot()

    # ---- journal registry (optional; EUD-055) ----
    def get_journal(self, request_id: str):
        """The journal for ``request_id`` (creating it lazily from the factory).

        Returns ``None`` when no ``journal_factory`` was injected (the additive
        no-journal mode — existing ToolLayer constructions keep working).
        """
        if self._journal_factory is None:
            return None
        j = self._journals.get(request_id)
        if j is None:
            j = self._journal_factory(request_id)
            self._journals[request_id] = j
        return j

    # ---- runner registry (optional; EUD-057) ----
    def get_runner(self):
        """The shared euddraft runner (created lazily from the factory).

        Returns ``None`` when no ``runner_factory`` was injected (the additive
        no-runner mode -> build_run falls back to the plain ``bridge.build()``).
        One instance per ToolLayer: ``last_result`` carries the LAST build's
        ladder errors that ``build_errors`` returns.
        """
        if self._runner_factory is None:
            return None
        if self._runner is None:
            self._runner = self._runner_factory()
        return self._runner

    # ---- the call paths ----
    def call_for_request(
        self, request_id: str, name: str, args: dict, *,
        list_reply: str | None = None,
    ) -> Any:
        return self.call(
            name,
            args,
            self.get_request_state(request_id),
            journal=self.get_journal(request_id),
            runner=self.get_runner(),
            list_reply=list_reply,
        )

    def call(
        self,
        name: str,
        args: dict | None,
        state: RequestState,
        *,
        journal: Any | None = None,
        runner: Any | None = None,
        list_reply: str | None = None,
    ) -> Any:
        """Run one tool call under the gate + budget for ``state``.

        Order (features/05): unknown-tool -> budget -> mutation gate -> validate ->
        [journal snapshot] -> bridge write -> [journal record]. A rejection
        (unknown tool, bad args, gate, budget) raises a ToolError subtype and does
        NOT consume the action budget or the mutation counter, NOR leave a journal
        entry (codex corrects and retries; a burnt slot/half-entry would strand the
        request). Only a call that actually reaches the bridge counts.

        ``journal`` (optional) is the per-request change journal: for a WRITE tool
        the snapshot happens AFTER the gate/budget/validation pass but BEFORE the
        bridge write; the entry is recorded AFTER the write returns successfully.
        Reads and the flow tool never journal.

        ``runner`` (optional, EUD-057) is the euddraft build runner: when present,
        ``build_run`` routes through its pipeline (consuming a build-fix attempt;
        the 4th -> ToolError + ``build_fix_exhausted``) and ``build_errors``
        returns the runner's last ladder result. Both behaviors fall back to the
        plain bridge when ``runner`` is None.
        """
        args = args or {}
        spec = _REGISTRY.get(name)
        if spec is None:
            raise ToolError(f"unknown tool {name!r}")

        # Budget: the 31st ACTION is rejected with a wrap-up message. The flow
        # tool (propose_plan) is HOW codex satisfies the gate, so it is exempt
        # from the action budget (it ends the turn anyway).
        if spec.kind != "flow" and state.action_count >= state.action_limit:
            raise BudgetExceeded(
                f"action budget exhausted ({state.action_limit} per request); "
                "wrap up and ask the user whether to continue with a fresh budget."
            )

        # memory_write (EUD-079) is a WRITE for journaling/budget but PLAN-GATE
        # EXEMPT: recording a durable fact must never force a propose_plan, so it
        # bypasses the mutation gate below and does NOT advance the mutation
        # counter. It still consumed the action budget (checked above) and is
        # journaled (with snapshot/inverse on the memory store). Routed here so the
        # gate check never sees it.
        if spec.name == MEMORY_TOOL:
            return self._memory_write_via_store(
                spec, args, state, journal, list_reply
            )

        # map_info (features/08) is a READ routed to the injected service (no
        # bridge method maps to it beyond the OpenMapName lookup the service
        # does itself). Routed here so the plain _dispatch path never sees it.
        if spec.name == MAP_INFO_TOOL:
            return self._map_info_via_service(spec, args, state)

        # Mutation gate: the Nth mutating call WITHOUT an approved plan is blocked
        # and directs codex to propose_plan. Checked BEFORE incrementing anything.
        if spec.mutating and not self._gate.allow(
            mutations_so_far=state.mutation_count,
            plan_approved=state.plan_approved,
        ):
            raise PlanRequired(
                "mutation gate: more than 2 edits without an approved plan. "
                "Call propose_plan(markdown) to outline the change for review "
                "before continuing."
            )

        # location_write (features/09) is a WRITE routed to the injected map
        # service AFTER the gate above passed. Its snapshot is the service's own
        # full-file map backup (taken atomically with the lock/compiling checks,
        # BEFORE the locedit spawn), so it bypasses the generic journal.snapshot
        # path below and records the entry itself.
        if spec.name == LOCATION_TOOL:
            return self._location_write_via_service(spec, args, state, journal)

        # build_run via the runner pipeline (EUD-057). The gate/budget above have
        # already passed (build_run is a write -> it counts as an action/mutation
        # and obeys the plan gate). The self-fix budget is a SEPARATE cap: the 4th
        # build_run in a request returns a ToolError (self-fix spent) and sets the
        # changeset-note flag. A successful (<=3) attempt routes through
        # ``runner.build_run`` and counts the action + mutation + the attempt. With
        # NO runner, fall through to the normal plain-bridge path below.
        if spec.name == "build_run" and runner is not None:
            return self._build_run_via_runner(state, runner)

        # build_errors via the runner's last ladder result (EUD-057). When a runner
        # is present the read returns the LAST build's structured errors (dicts);
        # with no runner, fall through to the plain bridge.builderr() read below.
        if spec.name == "build_errors" and runner is not None:
            return self._build_errors_via_runner(state, runner)

        # Journaled write path: validate args FIRST (no bridge touch), then
        # snapshot BEFORE the mutation, then write, then record AFTER. A
        # validation failure here means nothing was sent AND no snapshot GET was
        # issued (the snapshot is gated behind a successful arg validation). The
        # snapshot itself may raise BridgeError (e.g. the pre-write GET fails on an
        # existing file): we translate it to ToolError (one error family) and the
        # write is NOT performed and NO entry is recorded — snapshot-before-mutate
        # is a hard guarantee (a corrupting rollback from an empty snapshot is
        # worse than a retriable failure). ``snapshot`` returning None marks a tool
        # the journal SKIPS (build_run): write + count, but record no entry.
        if spec.mutating and journal is not None:
            self._validate_args(spec, args)  # ToolError -> nothing sent/recorded
            try:
                before = journal.snapshot(spec.name, args)
            except BridgeError as exc:
                raise ToolError(str(exc)) from exc
            result = self._dispatch(spec, args)
            if before is not None:
                journal.record(
                    spec.name, args, before,
                    journal.compute_after(spec.name, args, result),
                )
            state.action_count += 1
            state.mutation_count += 1
            return result

        # Validate + route. Validation (inside the handler) runs BEFORE the bridge
        # call; a ToolError here means nothing was sent and nothing is counted.
        result = self._dispatch(spec, args)

        # Success path: this call reached the bridge (or the flow tool ran). Count
        # the action (flow tools end the turn and are not budgeted) and, for a
        # mutating tool, the mutation.
        if spec.kind == "flow":
            if spec.name == PLAN_TOOL:
                state.plan_proposed = True
            return result
        state.action_count += 1
        if spec.mutating:
            state.mutation_count += 1
        return result

    def _build_run_via_runner(self, state: RequestState, runner: Any) -> Any:
        """Route ``build_run`` through the euddraft runner under the self-fix cap.

        The action budget + mutation gate have already passed in :meth:`call`.
        Here we enforce the SEPARATE 3-attempt self-fix budget
        (``RequestState.build_fix_limit``): the 4th build_run in a request raises a
        :class:`ToolError` telling codex the budget is spent and sets
        ``build_fix_exhausted`` so the engine notes the failure on the changeset
        (build_run is never journaled, so it cannot be a changeset item). A
        permitted attempt runs the pipeline, records the attempt, and counts the
        action + mutation. The runner's :class:`BuildRunResult` is returned as a
        dict (``{ok, errors}``) so codex sees the structured outcome directly.
        """
        if state.build_fix_attempts >= state.build_fix_limit:
            state.build_fix_exhausted = True
            raise ToolError(
                f"build self-fix budget spent ({state.build_fix_limit} attempts "
                "per request). The changeset will be presented with the build "
                "failure noted; stop trying to build and wrap up."
            )
        # Lazy import to avoid a tools <- edd_runner <- engine <- tools cycle at
        # module load. ConfigError is a STATIC misconfiguration codex cannot fix by
        # editing eps -> it does NOT consume a self-fix attempt (3 misconfigs would
        # otherwise silently exhaust the budget).
        from .edd_runner import ConfigError

        try:
            result = runner.build_run()
        except ConfigError as exc:
            # Static misconfiguration: surface as a tool error WITHOUT counting an
            # action/mutation or a build-fix attempt (nothing codex can fix by
            # retrying the build; the operator must set the path).
            raise ToolError(f"build_run misconfigured: {exc}") from exc
        except (TimeoutError, RuntimeError) as exc:
            # A real pipeline failure (poll/subprocess timeout, or any other
            # RuntimeError) is a tool result codex can read, not a transport crash.
            # The attempt still counts (it consumed a build) and the
            # action/mutation are counted so the request budgets stay honest.
            state.action_count += 1
            state.mutation_count += 1
            state.record_build_fix_attempt()
            raise ToolError(f"build_run failed: {exc}") from exc
        state.action_count += 1
        state.mutation_count += 1
        state.record_build_fix_attempt()
        return {"ok": result.ok, "errors": result.errors_as_dicts()}

    def _map_info_via_service(
        self, spec: ToolSpec, args: dict, state: RequestState
    ) -> Any:
        """Digest the connected map through the injected MapInfoService.

        First line of defense (features/08): :func:`_h_map_info` validates the
        ``mode`` enum (ToolError -> nothing runs, nothing counted). A missing
        service means IsomTerrain.exe was never configured — a clear ToolError
        codex can relay, never a crash. A :class:`MapInfoError` (unconfigured
        exe, no map connected, extraction failure) is translated the same way;
        the bridge GETSET inside the service may raise BridgeError, translated
        to the SAME family. A successful read counts one action (READ: no
        mutation counter, no journal, no plan gate).
        """
        validated = _h_map_info(_ProbeBridge(), args)
        service = self._map_info
        if service is None:
            raise ToolError(
                "map_info unavailable: no map-info service is configured "
                "(set ISOMTERRAIN_CMD or the agent.cfg 'isomterrain_cmd' key)."
            )
        try:
            result = service.map_info(**validated)
        except MapInfoError as exc:
            raise ToolError(str(exc)) from exc
        except BridgeError as exc:
            raise ToolError(str(exc)) from exc
        state.action_count += 1
        return result

    def _location_write_via_service(
        self,
        spec: ToolSpec,
        args: dict,
        state: RequestState,
        journal: Any | None,
    ) -> Any:
        """Apply one map-location edit through the injected MapInfoService.

        Order (features/09): validate args (ToolError -> nothing runs, nothing
        counted) -> service call, which INTERNALLY enforces snapshot-before-
        mutate (compiling guard -> lock probe -> full-file backup -> locedit,
        which aborts pre-save on any bad op) -> journal the entry with the
        backup pointer as ``before`` (rollback = restore the backed-up bytes;
        see ``journal._rollback_location``). A MapInfoError/BridgeError at any
        rail is a ToolError; nothing is counted or journaled. The gate/budget
        already passed in :meth:`call` (this is a REAL write: it advances both
        the action and mutation counters).
        """
        validated = _h_location_write(_ProbeBridge(), args)
        service = self._map_info
        if service is None:
            raise ToolError(
                "location_write unavailable: no map service is configured "
                "(set ISOMTERRAIN_CMD or the agent.cfg 'isomterrain_cmd' key)."
            )
        action = validated.pop("action")
        try:
            result = service.location_write(action, **validated)
        except MapInfoError as exc:
            raise ToolError(str(exc)) from exc
        except BridgeError as exc:
            raise ToolError(str(exc)) from exc

        if journal is not None:
            before = {
                "mapPath": result.get("mapPath", ""),
                "backupPath": result.get("backupPath", ""),
            }
            after = {
                "action": action,
                "name": validated.get("name", ""),
                "locationId": result.get(
                    "locationId", validated.get("location_id", 0)
                ),
            }
            journal.record(spec.name, args, before, after)

        state.action_count += 1
        state.mutation_count += 1
        return result

    def _memory_write_via_store(
        self,
        spec: ToolSpec,
        args: dict,
        state: RequestState,
        journal: Any | None,
        list_reply: str | None,
    ) -> Any:
        """Record a durable project-memory fact through the injected store.

        First line of defense (features/07): :func:`_h_memory_write` validates the
        ``file`` enum + the 8 192-byte cap (raising ToolError -> nothing written).
        A missing or DISABLED store (no project open) is a ToolError "no project is
        open". The write is JOURNALED like any other write (snapshot BEFORE, record
        AFTER) so the user can review/reject it; the snapshot reads the OLD content
        through the journal's own memory handle. A ``structure`` write also
        refreshes the store's LIST hash from ``list_reply`` (staleness rule). The
        call consumes the action budget but NOT the mutation counter (plan-gate
        exempt — handled by the caller routing here before the gate).
        """
        # Validate enum + cap (no disk touch) BEFORE anything else.
        validated = _h_memory_write(_ProbeBridge(), args)
        file, content = validated["file"], validated["content"]

        store = self._memory
        if store is None or not getattr(store, "enabled", False):
            raise ToolError(
                "no project is open; project memory is disabled (open a map "
                "project before recording memory)."
            )

        # Snapshot BEFORE the write (journaled path only). The journal reads the
        # old content via its own memory handle.
        before = None
        if journal is not None:
            before = journal.snapshot(spec.name, args)

        write_result = store.write(file, content)
        if not write_result.ok:
            # The store re-validates (second line of defense); surface its reason.
            raise ToolError(write_result.reason)

        # A structure write refreshes the LIST-hash staleness signal when the
        # engine threaded the current LIST reply through.
        if file == "structure" and list_reply is not None:
            store.update_list_hash(list_reply)

        if journal is not None and before is not None:
            journal.record(
                spec.name, args, before,
                journal.compute_after(spec.name, args, write_result),
            )

        state.action_count += 1
        # Return a plain JSON-serializable dict: the /tools/call endpoint
        # json-dumps the result, and the WriteResult dataclass is NOT
        # JSON-serializable (matches the {ok, ...} shape of the runner build_run
        # return).
        return {"ok": True, "file": file}

    @staticmethod
    def _build_errors_via_runner(state: RequestState, runner: Any) -> Any:
        """Return the LAST build's structured ladder errors (runner-backed).

        ``build_errors`` is a READ (no budget/gate/journal). When the runner has a
        ``last_result`` we return its structured entries (dicts); before any
        build_run in the request there is nothing -> ``[]``.
        """
        last = getattr(runner, "last_result", None)
        if last is None:
            return []
        return last.errors_as_dicts()

    def _validate_args(self, spec: ToolSpec, args: dict) -> None:
        """Run the handler's arg validation WITHOUT touching the real bridge.

        Dispatches the handler against a probe bridge whose methods are no-ops
        returning a benign string: the handler runs its argument checks (which
        raise ToolError BEFORE any bridge call in the real path) and the probe
        absorbs the would-be bridge call. This lets the journal snapshot run only
        AFTER args are known valid, so an arg-invalid write issues no snapshot GET
        and leaves no entry (rules.md: a rejection strands nothing).
        """
        try:
            spec.handler(_ProbeBridge(), args)
        except ToolError:
            raise
        except BridgeError as exc:
            raise ToolError(str(exc)) from exc

    def _dispatch(self, spec: ToolSpec, args: dict) -> Any:
        """Run the handler, translating a bridge round-trip BridgeError -> ToolError.

        The handler validates args (raising ToolError) then calls the bridge; the
        bridge wrapper may raise BridgeError (its own pre-check OR the editor's
        ERROR reply — the second line of defense). Either way codex must see a
        ToolError, never an unhandled exception.
        """
        try:
            return spec.handler(self._bridge, args)
        except ToolError:
            raise
        except BridgeError as exc:
            raise ToolError(str(exc)) from exc
