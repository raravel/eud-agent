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

Journaling (snapshots / rollback) is a LATER task (EUD-055): write tools route to
the bridge and increment the mutation counter here, no snapshotting yet.
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

# Budgets (features/05 "Triage and plan gating"). Pinned constants; the request
# state exposes them in budget_snapshot for the panel.
ACTION_BUDGET = 30
BUILD_FIX_LIMIT = 3
# The gate fires on the Nth mutating call WITHOUT a plan. "small edits (<=2
# mutations) may apply directly; the 3rd mutating call without a plan is blocked".
MUTATION_GATE_THRESHOLD = 3

# Flow tool name (ends the codex turn for plan review).
PLAN_TOOL = "propose_plan"


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


class ToolLayer:
    """Validates + gates + budgets + routes tool calls to the bridge.

    One instance per server (the bridge is a singleton). Per-request state is
    tracked in ``_requests`` keyed by ``request_id`` so the action budget, the
    mutation counter (plan gate), and the plan-approved flag are isolated per
    agent request and queryable for the panel.
    """

    def __init__(self, bridge, *, gate: MutationGate | None = None) -> None:
        self._bridge = bridge
        self._gate = gate or MutationGate()
        self._requests: dict[str, RequestState] = {}

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

    # ---- the call paths ----
    def call_for_request(self, request_id: str, name: str, args: dict) -> Any:
        return self.call(name, args, self.get_request_state(request_id))

    def call(self, name: str, args: dict | None, state: RequestState) -> Any:
        """Run one tool call under the gate + budget for ``state``.

        Order (features/05): unknown-tool -> budget -> mutation gate -> validate +
        bridge. A rejection (unknown tool, bad args, gate, budget) raises a
        ToolError subtype and does NOT consume the action budget or the mutation
        counter (codex corrects and retries; a burnt slot would strand the
        request). Only a call that actually reaches the bridge counts.
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
