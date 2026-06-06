"""Change journal + rollback engine for the v2 agent core (features/05 "Change
journal and rollback").

Every WRITE tool the agent runs is journaled: BEFORE the bridge mutation the
:class:`Journal` reads the old value through the SAME bridge (the corresponding
``GET`` command), records a ``before`` snapshot, performs the write (driven by the
tool layer), then records ``after``. Entries accumulate per request and are
PERSISTED to ``<data-dir>/journal/<request-id>.json`` (UTF-8 **without BOM**,
atomic temp+replace — rules.md "IPC and encoding") after every write so a server
crash cannot strand an un-reviewable changeset; a reload reproduces an identical
changeset.

On turn completion the server emits ``changeset{request_id, items[]}`` (WS v2
shape: file items with ``kind`` created|modified|deleted and a server-side unified
diff for modified; dat-kind items grouped per objId with property/old/new).
``changeset_decision{reject, ids|all}`` replays the INVERSE ops via the bridge in
REVERSE seq order; ``accept`` archives the journal to ``<request-id>.accepted.json``.

Snapshot parsing
----------------
The bridge's GET replies are ``"OK: <ids...> = <value>"`` for the dat family
(``GETDAT``/``GETXDAT``/``GETTBL``/``GETREQ``/``GETBTN``/``GETSET``); only the
FIRST ``" = "`` separates the id prefix from the value (a value may contain
``" = "``). :func:`parse_get_value` and :func:`parse_main_path` are the small,
unit-tested parsers. ``GET`` (file content) and ``GETMAIN`` return the raw text.

was_default availability (honest limitations)
---------------------------------------------
The bridge does NOT expose ``IsDefault`` for dat/xdat/req/btn, so those entries
record ``was_default: false`` (we have no real signal — we never invent a bridge
command). ``tbl`` is the one kind with a usable signal: ``GETTBL`` returns the
StatTxt value, and an EMPTY old value means the entry sits at its default
(``SETTBL NULLSTRING`` / ``DataReset`` semantics), so a tbl rollback of a
previously-default entry uses ``RESETDAT tbl`` rather than writing back ``""``.

plugin_edit / plugin_remove partial honesty
-------------------------------------------
``PLUGLIST`` exposes only the FIRST LINE of each block's Texts; there is no GET
for the full Texts. So ``plugin_edit`` and ``plugin_remove`` snapshots are
``partial: true`` (carrying ``first_line`` and ``index`` only). Rolling those
back would require the full old Texts, which we do not have — rather than writing
a TRUNCATED Texts (silent corruption), the rollback REFUSES that single item with
a clear per-item error and leaves the other items' rollbacks intact.

set_main with no prior main (honest refusal)
--------------------------------------------
``set_main`` snapshots the prior main via ``GETMAIN``. When the project had NO
main file set, ``GETMAIN`` returns ``""`` — and the bridge has NO clear-main
primitive (``SETMAIN`` requires a real path; ``SETMAIN ""`` would raise a
path-validation error). So a ``set_main`` whose prior main was empty is marked
``partial: true`` at snapshot time and its rollback REFUSES per-item with a clear
message (same honesty model as the plugin partials), rather than emitting an
incidental ``BridgeError``.

build_run is NOT journaled
--------------------------
``build_run`` remains ``kind="write"`` for the gate/budget (it counts as an
action/mutation), but a build has no reversible state — it produces an output map
and macro errors, nothing the journal can snapshot or undo. The journal therefore
SKIPS it entirely: no snapshot, no entry, no changeset item, and it can never
appear in a rollback selection. (See :func:`Journal.snapshot` returning ``None``
for build_run and the tool layer's skip.)

memory_write targets the ProjectMemory store
---------------------------------------------
``memory_write`` (EUD-079, features/07) records a durable project-memory fact —
a full-file replacement of one of resources/structure/conventions/lessons. Unlike
every other journaled write it touches the injected :class:`ProjectMemory` STORE,
not the editor bridge. Its snapshot is ``{content: <old or "">, existed: <bool>}``;
the changeset item is ``{kind: memory, target: memory/<file>, diff}`` (a server-
side unified diff old -> new, same styling as a modified file); and its inverse
restores the old content when the file pre-existed, or DELETES the file when it
did not — wired into the same reverse-seq rollback replay (the store IO replaces
the bridge for that one tool). A journal with no memory store refuses a
memory_write rollback per item.

Snapshot-before-mutate is a hard guarantee
------------------------------------------
``file_write``/``file_delete`` read the existing content BEFORE mutating; if that
pre-write GET FAILS on a file the write would touch, the journal raises rather
than recording ``content=""`` — a later rollback from an empty snapshot would
silently EMPTY the file. A retriable failure (the whole tool call fails with no
write performed and no entry) is safer than a corrupting rollback.

file_create / mkdir rollback leaves auto-created parents
-------------------------------------------------------
``file_create``/``mkdir`` rollback deletes the created node (``DELFILE``) but does
NOT remove parent folders the bridge auto-created along the way: those folders may
already be shared by other (kept) nodes, and deleting a shared parent is worse
than leaving an empty folder. Documented limitation; no parent cleanup attempted.

Archive layout
--------------
The live journal is ``journal/<request-id>.json``; ``accept`` (or ``finalize``
after a partial reject, where undecided items default to accepted) renames it to
``journal/<request-id>.accepted.json`` with a ``note`` recording the defaulting.
"""

from __future__ import annotations

import difflib
import json
import os
import time
import uuid
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from .bridge_io import BridgeError

# Tool kinds grouped by snapshot/inverse strategy.
_DAT_FAMILY = {"dat_set", "xdat_set", "req_set", "btn_set"}
_FILE_TOOLS = {
    "file_write", "file_create", "mkdir", "file_delete", "file_rename",
    "file_move",
}
_PLUGIN_PARTIAL = {"plugin_edit", "plugin_remove"}


# --------------------------------------------------------------------------- #
# Reply parsers (small + unit-tested).
# --------------------------------------------------------------------------- #


def parse_get_value(reply: str) -> str:
    """Extract the value from a dat-family GET reply.

    The bridge replies ``"OK: <id-prefix> = <value>"`` (GETDAT/GETXDAT/GETTBL/
    GETREQ/GETBTN/GETSET). Only the FIRST ``" = "`` separates the prefix from the
    value, so a value that itself contains ``" = "`` survives intact. A reply
    without a separator returns ``""`` (defensive; the caller validated OK first).
    """
    _, sep, value = reply.partition(" = ")
    return value if sep else ""


def parse_main_path(reply: str) -> str:
    """GETMAIN returns the raw main-file path (or ``""`` when none)."""
    return reply.strip()


def _parent_of(path: str) -> str:
    """Project-path parent folder ("" for a root node). Paths use ``/``."""
    norm = path.replace("\\", "/")
    if "/" not in norm:
        return ""
    return norm.rsplit("/", 1)[0]


def _basename_of(path: str) -> str:
    norm = path.replace("\\", "/")
    return norm.rsplit("/", 1)[-1] if "/" in norm else norm


def _join(parent: str, name: str) -> str:
    return f"{parent}/{name}" if parent else name


# --------------------------------------------------------------------------- #
# Journal entry.
# --------------------------------------------------------------------------- #


@dataclass
class JournalEntry:
    """One journaled write: ``{id, seq, tool, target, before, after, ts}``.

    ``before`` is the snapshot dict captured BEFORE the mutation; ``after`` is the
    new state recorded after the bridge write. ``target`` is the human-facing
    subject (a file path, a ``dat|param|objId`` triple, an index, etc.).
    """

    id: str
    seq: int
    tool: str
    target: str
    before: dict[str, Any]
    after: dict[str, Any]
    ts: float

    def to_dict(self) -> dict[str, Any]:
        return {
            "id": self.id,
            "seq": self.seq,
            "tool": self.tool,
            "target": self.target,
            "before": self.before,
            "after": self.after,
            "ts": self.ts,
        }

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> JournalEntry:
        return cls(
            id=d["id"],
            seq=d["seq"],
            tool=d["tool"],
            target=d["target"],
            before=d.get("before", {}),
            after=d.get("after", {}),
            ts=d.get("ts", 0.0),
        )


# --------------------------------------------------------------------------- #
# Inverse-op selection for the dat family (standalone, testable).
# --------------------------------------------------------------------------- #


def inverse_dat_op(*, tool: str, args: dict, before: dict) -> dict:
    """Pick the inverse op for a dat-family write.

    When ``before['was_default']`` is true the inverse is a RESET (RESETDAT);
    otherwise it writes the snapshotted old value back. Returns
    ``{"method": <bridge method>, "args": (...)}``. ``objId`` is read per-branch
    (btn_set keys on ``setId``, not ``objId``).
    """
    old = before.get("value", "")
    if tool == "dat_set":
        obj_id = int(args["objId"])
        dat, param = args["dat"], args["param"]
        if before.get("was_default"):
            return {"method": "resetdat", "args": ("dat", dat, param, obj_id)}
        return {"method": "setdat", "args": (dat, param, obj_id, old)}
    if tool == "xdat_set":
        obj_id = int(args["objId"])
        dat, name = args["dat"], args["name"]
        if before.get("was_default"):
            return {"method": "resetdat", "args": ("xdat", dat, name, obj_id)}
        return {"method": "setxdat", "args": (dat, name, obj_id, old)}
    if tool == "req_set":
        return {"method": "setreq", "args": (args["dat"], int(args["objId"]), old)}
    if tool == "btn_set":
        return {"method": "setbtn", "args": (int(args["setId"]), old)}
    raise ValueError(f"not a dat-family tool: {tool!r}")  # pragma: no cover


# --------------------------------------------------------------------------- #
# The journal.
# --------------------------------------------------------------------------- #


class Journal:
    """Per-request change journal: snapshot, persist, assemble, roll back.

    Constructed with the editor ``data_dir`` (state lives under
    ``data_dir/journal/``), the ``request_id``, and the shared ``bridge`` (used
    BOTH for snapshot GETs and for inverse-op writes during rollback). The
    optional ``memory`` (a :class:`ProjectMemory` store) is the target of
    ``memory_write`` snapshots/inverses — its file IO replaces the bridge for that
    one tool (memory writes never touch the editor). Absent when no project memory
    is wired; a ``memory_write`` entry then cannot be rolled back (refused).
    """

    def __init__(
        self, *, data_dir: str | os.PathLike, request_id: str, bridge,
        memory=None,
    ):
        self.data_dir = Path(data_dir)
        self.request_id = request_id
        self._bridge = bridge
        self._memory = memory
        self.journal_dir = self.data_dir / "journal"
        self.entries: list[JournalEntry] = []
        self._decided: set[str] = set()  # ids already rolled back or accepted

    # ------------------------------------------------------------- paths
    @property
    def _live_path(self) -> Path:
        return self.journal_dir / f"{self.request_id}.json"

    @property
    def _archive_path(self) -> Path:
        return self.journal_dir / f"{self.request_id}.accepted.json"

    # ------------------------------------------------------------- snapshot
    def snapshot(self, tool: str, args: dict) -> dict[str, Any] | None:
        """Read the old state for a write tool BEFORE its mutation.

        The read goes through the SAME bridge (the corresponding GET). Returns the
        ``before`` dict to be passed to :meth:`record`, or ``None`` for a tool the
        journal SKIPS entirely (``build_run`` — see the module docstring). Pure
        read — no entry is created here (so a validation/gate rejection that never
        calls record leaves nothing behind).

        A snapshot GET that the bridge rejects raises :class:`BridgeError`, which
        the tool layer translates to a ToolError so the whole call fails WITHOUT
        mutating (snapshot-before-mutate is a hard guarantee — a corrupting
        rollback from an empty snapshot is worse than a retriable failure).
        """
        if tool == "build_run":
            return None  # builds have no reversible state; never journaled
        if tool == "memory_write":
            # memory_write targets the ProjectMemory STORE, not the bridge. Capture
            # the OLD content + whether the file already existed, so the inverse can
            # restore old content (existed) or DELETE the file (not existed).
            name = args["file"]
            old, existed = self._read_memory(name)
            return {"content": old, "existed": existed}
        if tool == "dat_set":
            reply = self._bridge.getdat(
                args["dat"], args["param"], int(args["objId"])
            )
            return {"value": parse_get_value(reply), "was_default": False}
        if tool == "xdat_set":
            reply = self._bridge.getxdat(
                args["dat"], args["name"], int(args["objId"])
            )
            return {"value": parse_get_value(reply), "was_default": False}
        if tool == "req_set":
            reply = self._bridge.getreq(args["dat"], int(args["objId"]))
            return {"value": parse_get_value(reply), "was_default": False}
        if tool == "btn_set":
            reply = self._bridge.getbtn(int(args["setId"]))
            return {"value": parse_get_value(reply), "was_default": False}
        if tool == "tbl_set":
            reply = self._bridge.gettbl(int(args["index"]))
            old = parse_get_value(reply)
            # tbl is the ONE kind with a real default signal: an empty old value
            # means the entry sits at its default (NULLSTRING/DataReset).
            return {"value": old, "was_default": old == ""}
        if tool == "dat_reset":
            # The pre-reset value IS readable via the matching GET; snapshot it so
            # rollback can write it back. kind routes the GET (dat/xdat/tbl).
            return {"value": self._read_reset_target(args), "was_default": False}
        if tool == "file_write":
            # Hard guarantee: fail (not content="") if the pre-write GET errors.
            return {"content": self._bridge.get(args["path"])}
        if tool in ("file_create", "mkdir"):
            return {"created": True}
        if tool == "file_delete":
            path = args["path"]
            return {
                "content": self._bridge.get(path),
                "ftype": _ftype_for_create(path),
            }
        if tool == "file_rename":
            return {"path": args["path"], "oldname": _basename_of(args["path"])}
        if tool == "file_move":
            path = args["path"]
            return {"path": path, "parent": _parent_of(path)}
        if tool == "set_main":
            old_main = parse_main_path(self._bridge.getmain())
            if old_main == "":
                # No prior main + no clear-main primitive -> honest refusal marker.
                return {"main": "", "partial": True}
            return {"main": old_main}
        if tool == "settings_set":
            reply = self._bridge.getset(args["scope"], args["key"])
            return {"value": parse_get_value(reply)}
        if tool == "plugin_add":
            # capture the index the new block will occupy so rollback can delete it.
            idx = int(args.get("index", -1))
            if idx < 0:
                idx = len(self._bridge.pluglist())  # append lands at the end
            return {"created_index": idx}
        if tool == "plugin_move":
            return {"from": int(args["from"]), "to": int(args["to"])}
        if tool in _PLUGIN_PARTIAL:
            # PLUGLIST gives only the first line of Texts; full Texts are NOT
            # recoverable -> mark the entry partial (rollback will refuse).
            index = int(args["index"])
            first = ""
            for block in self._bridge.pluglist():
                if str(block.get("index")) == str(index):
                    first = block.get("first_line", "")
                    break
            return {"partial": True, "index": index, "first_line": first}
        return {}

    def _read_reset_target(self, args: dict) -> str:
        """Read the value a ``dat_reset`` is about to clear, via the matching GET.

        ``kind`` selects the GET: dat -> getdat(dat,param,objId); xdat ->
        getxdat(dat,param,objId) (the tool's ``param`` carries the xdat NAME);
        tbl -> gettbl(objId) (the tool's ``objId`` carries the StatTxt index).
        """
        kind = args["kind"]
        obj_id = int(args["objId"])
        if kind == "dat":
            return parse_get_value(
                self._bridge.getdat(args["dat"], args["param"], obj_id)
            )
        if kind == "xdat":
            return parse_get_value(
                self._bridge.getxdat(args["dat"], args["param"], obj_id)
            )
        if kind == "tbl":
            return parse_get_value(self._bridge.gettbl(obj_id))
        return ""

    def _read_memory(self, name: str) -> tuple[str, bool]:
        """Read a memory file's OLD content + whether it existed (memory_write).

        Returns ``(content, existed)``: the current store content (``""`` when
        absent/disabled) and a flag for whether the ``<name>.md`` file is on disk
        (drives the inverse: restore old content vs DELETE the file). When no
        memory store is wired, reports ``("", False)`` (the inverse will refuse).
        """
        store = self._memory
        if store is None:
            return "", False
        path = store.store_dir / f"{name}.md" if store.store_dir else None
        existed = bool(path and path.is_file())
        return store.read(name), existed

    # ------------------------------------------------------------- record
    def record(
        self, tool: str, args: dict, before: dict, after: dict
    ) -> JournalEntry:
        """Append an entry for a SUCCESSFUL write and persist the journal.

        Called by the tool layer AFTER the bridge write returned. Persistence is
        after every write (crash safety): a JSON dump (UTF-8 no BOM, atomic).
        """
        entry = JournalEntry(
            id=uuid.uuid4().hex,
            seq=len(self.entries),
            tool=tool,
            target=_target_for(tool, args),
            before=before,
            after=after,
            ts=time.time(),
        )
        self.entries.append(entry)
        self._persist()
        return entry

    def compute_after(self, tool: str, args: dict, reply: Any) -> dict[str, Any]:
        """Build the ``after`` snapshot for a successful write.

        Mostly delegates to :func:`after_for` (identifying args + the new value
        from args). The exception is ``dat_reset``: the new value is the editor's
        STOCK value, which only the reset itself produced — so we RE-READ it via
        the matching GET (mirroring how the other dat writes capture a read-back),
        and carry the kind/dat/param/objId so the changeset groups it per objId.
        """
        if tool == "dat_reset":
            # Re-read the stock value AFTER the reset. The write already
            # succeeded; a failed read-back must not crash record -> best-effort "".
            try:
                stock = self._read_reset_target(args)
            except BridgeError:
                stock = ""
            return {
                "kind": args["kind"],
                "dat": args.get("dat", ""),
                "param": args.get("param", ""),
                "objId": int(args["objId"]),
                "value": stock,
            }
        return after_for(tool, args, reply)

    # ------------------------------------------------------------- persist
    def _persist(self) -> None:
        self.journal_dir.mkdir(parents=True, exist_ok=True)
        payload = {
            "request_id": self.request_id,
            "entries": [e.to_dict() for e in self.entries],
        }
        tmp = self._live_path.with_suffix(".json.tmp")
        # UTF-8 WITHOUT BOM (rules.md). Bytes so no BOM and no newline translation.
        tmp.write_bytes(
            json.dumps(payload, ensure_ascii=False, indent=2).encode("utf-8")
        )
        os.replace(tmp, self._live_path)

    @classmethod
    def load(cls, *, data_dir: str | os.PathLike, request_id: str, bridge,
             memory=None) -> Journal:
        """Reconstruct a journal from its persisted JSON (server-restart safe)."""
        j = cls(data_dir=data_dir, request_id=request_id, bridge=bridge,
                memory=memory)
        path = j._live_path
        if path.is_file():
            data = json.loads(path.read_text(encoding="utf-8"))
            j.entries = [JournalEntry.from_dict(d) for d in data.get("entries", [])]
        return j

    # ------------------------------------------------------------- changeset
    def changeset(self) -> dict[str, Any]:
        """Assemble the WS v2 ``changeset{request_id, items[]}``.

        File items: ``{category: file, kind: created|modified|deleted, path, id,
        seq, diff?}`` (a unified diff for ``modified``). dat-family items are
        GROUPED per ``(dat, objId)`` with a ``properties`` list of
        ``{property, old, new, id, seq}``. Other kinds (tbl/req/btn/settings/
        plugin/main) appear as flat items so the panel can still render + decide
        them.
        """
        items: list[dict[str, Any]] = []
        dat_groups: dict[tuple, dict[str, Any]] = {}
        for e in self.entries:
            if e.tool == "build_run":
                continue  # never journaled / never a changeset item (defensive)
            cat = _category_for(e)
            if cat == "file":
                items.append(self._file_item(e))
            elif cat == "memory":
                items.append(self._memory_item(e))
            elif cat == "map":
                items.append(self._map_item(e))
            elif cat == "dat":
                self._add_dat_to_group(dat_groups, e)
            else:
                items.append(self._flat_item(e, cat))
        # dat groups appear in first-seen objId order, after files (insertion-
        # stable: dict preserves order). Append at the end for a deterministic shape.
        items.extend(dat_groups.values())
        return {"request_id": self.request_id, "items": items}

    @staticmethod
    def _file_item(e: JournalEntry) -> dict[str, Any]:
        path = e.after.get("path") or e.before.get("path") or e.target
        if e.tool in ("file_create", "mkdir"):
            return {"category": "file", "kind": "created", "path": path,
                    "id": e.id, "seq": e.seq}
        if e.tool == "file_delete":
            return {"category": "file", "kind": "deleted", "path": path,
                    "id": e.id, "seq": e.seq}
        # file_write / rename / move -> modified (rename/move have no text diff).
        item: dict[str, Any] = {"category": "file", "kind": "modified",
                                "path": path, "id": e.id, "seq": e.seq}
        if e.tool == "file_write":
            item["diff"] = _unified_diff(
                e.before.get("content", ""), e.after.get("content", ""), path
            )
        return item

    @staticmethod
    def _memory_item(e: JournalEntry) -> dict[str, Any]:
        """A ``memory_write`` changeset item (features/07 "Changeset item").

        ``kind: memory``, ``target: memory/<file>``, and a server-side unified diff
        old -> new so the user reviews a memory change with the SAME diff styling as
        a modified file.
        """
        file = e.after.get("file") or e.before.get("file") or ""
        target = f"memory/{file}"
        return {
            "category": "memory",
            "kind": "memory",
            "target": target,
            "id": e.id,
            "seq": e.seq,
            "diff": _unified_diff(
                e.before.get("content", ""), e.after.get("content", ""), target
            ),
        }

    @staticmethod
    def _add_dat_to_group(groups: dict, e: JournalEntry) -> None:
        # only true dat_set is grouped per objId (features/05). Others flat.
        dat = e.before.get("dat") or e.after.get("dat") or ""
        obj_id = e.after.get("objId")
        key = (dat, obj_id)
        grp = groups.get(key)
        if grp is None:
            grp = {"category": "dat", "dat": dat, "objId": obj_id,
                   "properties": []}
            groups[key] = grp
        grp["properties"].append({
            "property": e.after.get("param", ""),
            "old": e.before.get("value", ""),
            "new": e.after.get("value", ""),
            "id": e.id,
            "seq": e.seq,
        })

    @staticmethod
    def _map_item(e: JournalEntry) -> dict[str, Any]:
        """A map-write changeset item (features/09; display fix EUD-087).

        Covers ``location_write`` AND ``player_setup`` (EUD-089). The journal
        entry's ``before`` is rollback BOOKKEEPING ({mapPath, backupPath} —
        exactly what ``_rollback_location`` needs), NOT a reviewable previous
        state; rendered verbatim by the panel's generic old → new row it read
        as a nonsense diff. The item carries a human summary instead: ``old``
        is empty (the pre-edit state lives in the backup file) and ``new``
        describes the applied edit. The entry itself is unchanged — rollback
        keys stay intact.
        """
        action = e.after.get("action", "")
        if e.tool == "player_setup":
            player = e.after.get("player", "P?")
            summary = {
                "start": (
                    f"{player} start location placed at tile "
                    f"({e.after.get('tileX', 0)},{e.after.get('tileY', 0)})"
                ),
                "delstart": f"{player} start location removed",
                "controller": (
                    f"{player} controller = {e.after.get('controller', '')}"
                ),
            }.get(action, f"{player} ({action})")
        else:
            name = e.after.get("name", "")
            loc_id = e.after.get("locationId", 0)
            label = f"location #{loc_id}" + (f" '{name}'" if name else "")
            summary = {
                "add": f"{label} created",
                "set": f"{label} bounds changed",
                "rename": f"{label} renamed",
                "delete": f"{label} deleted",
            }.get(action, f"{label} ({action})")
        map_name = Path(e.before.get("mapPath", "")).name
        if map_name:
            summary += f" in {map_name}"
        return {
            "category": "map",
            "tool": e.tool,
            "target": e.target,
            "old": "",
            "new": summary,
            "id": e.id,
            "seq": e.seq,
        }

    @staticmethod
    def _flat_item(e: JournalEntry, category: str) -> dict[str, Any]:
        return {
            "category": category,
            "tool": e.tool,
            "target": e.target,
            "old": e.before,
            "new": e.after,
            "id": e.id,
            "seq": e.seq,
        }

    # ------------------------------------------------------------- rollback
    def rollback(self, *, ids: list[str] | None = None, all: bool = False
                 ) -> dict[str, Any]:
        """Replay inverse ops via the bridge in REVERSE seq order.

        ``all=True`` rolls back every (undecided) entry; otherwise only the given
        ``ids``. Returns ``{request_id, items: [{id, ok, error?}]}``. A partial
        plugin_edit/remove entry REFUSES with a per-item error (no truncated
        write). After rollback the affected ids are marked decided.
        """
        if all:
            targets = [e for e in self.entries if e.id not in self._decided]
        else:
            want = set(ids or [])
            targets = [e for e in self.entries if e.id in want]
        results: list[dict[str, Any]] = []
        # reverse seq order (undo most-recent first).
        for e in sorted(targets, key=lambda x: x.seq, reverse=True):
            res = self._rollback_one(e)
            results.append(res)
            self._decided.add(e.id)
        return {"request_id": self.request_id, "items": results}

    def _rollback_one(self, e: JournalEntry) -> dict[str, Any]:
        # memory_write inverses target the ProjectMemory STORE, not the bridge.
        if e.tool == "memory_write":
            return self._rollback_memory(e)
        # location_write/player_setup inverses target the MAP FILE on disk
        # (features/09, EUD-089): restore the full-file backup the service
        # took before the edit.
        if e.tool in ("location_write", "player_setup"):
            return self._rollback_location(e)
        try:
            op = self._inverse_op(e)
        except _RefuseRollback as exc:
            return {"id": e.id, "ok": False, "error": str(exc)}
        try:
            getattr(self._bridge, op["method"])(*op["args"])
        except BridgeError as exc:
            return {"id": e.id, "ok": False, "error": str(exc)}
        return {"id": e.id, "ok": True}

    def _rollback_memory(self, e: JournalEntry) -> dict[str, Any]:
        """Inverse of a ``memory_write``: restore old content, or DELETE the file.

        When ``before['existed']`` the file pre-existed -> write the snapshotted old
        content back. Otherwise the write CREATED the file -> delete it (its prior
        state was "absent"). Operates on the injected store; a missing store or a
        store write rejection is reported as a per-item rollback failure.
        """
        store = self._memory
        if store is None or not getattr(store, "enabled", False):
            return {"id": e.id, "ok": False,
                    "error": "cannot roll back memory_write: no memory store"}
        name = e.after.get("file") or e.before.get("file") or ""
        if e.before.get("existed"):
            result = store.write(name, e.before.get("content", ""))
            if not result.ok:
                return {"id": e.id, "ok": False, "error": result.reason}
            return {"id": e.id, "ok": True}
        # Created from absent -> delete the file (best-effort; missing_ok).
        path = store.store_dir / f"{name}.md" if store.store_dir else None
        if path is not None:
            path.unlink(missing_ok=True)
        return {"id": e.id, "ok": True}

    def _rollback_location(self, e: JournalEntry) -> dict[str, Any]:
        """Inverse of a map write (location_write/player_setup): restore the
        backed-up map bytes.

        ``before`` carries ``{mapPath, backupPath}`` (recorded by the tool
        layer from the service result). The restore refuses while the map is
        locked (same share probe as the write path) and replaces atomically —
        see :func:`chk_info.restore_map_backup`. NOTE: restoring rolls the map
        back to the state BEFORE that edit, which also undoes any LATER map
        write in the same request — rollback runs in reverse seq order, so a
        full rollback lands on the original map.
        """
        # Lazy import: journal <- chk_info only on this path (tools.py already
        # imports chk_info; keep module-load graphs unchanged).
        from .chk_info import MapInfoError, restore_map_backup

        map_path = e.before.get("mapPath", "")
        backup_path = e.before.get("backupPath", "")
        if not map_path or not backup_path:
            return {"id": e.id, "ok": False,
                    "error": f"cannot roll back {e.tool}: no backup recorded"}
        try:
            restore_map_backup(map_path, backup_path)
        except MapInfoError as exc:
            return {"id": e.id, "ok": False, "error": str(exc)}
        return {"id": e.id, "ok": True}

    def _inverse_op(self, e: JournalEntry) -> dict:
        tool = e.tool
        if tool in _DAT_FAMILY:
            return inverse_dat_op(tool=tool, args=e.after, before=e.before)
        if tool == "tbl_set":
            index = int(e.after["index"])
            if e.before.get("was_default"):
                return {"method": "resetdat",
                        "args": ("tbl", "", "", index)}
            return {"method": "settbl", "args": (index, e.before["value"])}
        if tool == "dat_reset":
            # inverse of a reset = write the snapshotted pre-reset value back, via
            # the setter matching the reset kind.
            kind = e.after["kind"]
            obj_id = int(e.after["objId"])
            old = e.before.get("value", "")
            if kind == "dat":
                return {"method": "setdat",
                        "args": (e.after["dat"], e.after["param"], obj_id, old)}
            if kind == "xdat":
                return {"method": "setxdat",
                        "args": (e.after["dat"], e.after["param"], obj_id, old)}
            if kind == "tbl":
                return {"method": "settbl", "args": (obj_id, old)}
            raise _RefuseRollback(f"dat_reset: unknown kind {kind!r}")
        if tool == "file_write":
            return {"method": "set",
                    "args": (e.after["path"], e.before.get("content", ""))}
        if tool in ("file_create", "mkdir"):
            return {"method": "delfile", "args": (e.after["path"],)}
        if tool == "file_delete":
            path = e.after["path"]
            return {"method": "newfile",
                    "args": (path, e.before.get("ftype", "RawText"),
                             e.before.get("content", ""))}
        if tool == "file_rename":
            # the node now lives at parent/<newname>; rename it back to oldname.
            parent = _parent_of(e.before["path"])
            new_full = _join(parent, e.after["newname"])
            return {"method": "rename", "args": (new_full, e.before["oldname"])}
        if tool == "file_move":
            # the node now lives at <destFolder>/<basename>; move it back.
            base = _basename_of(e.before["path"])
            new_full = _join(e.after.get("destFolder", ""), base)
            return {"method": "movefile", "args": (new_full, e.before["parent"])}
        if tool == "set_main":
            if e.before.get("partial"):
                raise _RefuseRollback(
                    "cannot roll back set_main: the project had NO prior main "
                    "file and the bridge has no clear-main primitive (SETMAIN "
                    "requires a real path). Clear/reset the main file manually."
                )
            return {"method": "setmain", "args": (e.before["main"],)}
        if tool == "settings_set":
            return {"method": "setset",
                    "args": (e.after["scope"], e.after["key"],
                             e.before["value"])}
        if tool == "plugin_add":
            return {"method": "plugdel", "args": (e.before["created_index"],)}
        if tool == "plugin_move":
            return {"method": "plugmove",
                    "args": (e.before["to"], e.before["from"])}
        if tool in _PLUGIN_PARTIAL:
            raise _RefuseRollback(
                f"cannot roll back {tool}: the old plugin Texts are not "
                "recoverable (PLUGLIST exposes only the first line). Re-edit the "
                "block manually."
            )
        raise _RefuseRollback(f"no inverse op for tool {tool!r}")

    # ------------------------------------------------------------- accept
    def accept(self, *, ids: list[str] | None = None, all: bool = False,
               note: str | None = None) -> None:
        """Mark items accepted (no rollback) and, when nothing is left undecided,
        archive the live journal to ``<request-id>.accepted.json``.

        ``all=True`` accepts everything and archives immediately.
        """
        if all:
            for e in self.entries:
                self._decided.add(e.id)
        else:
            for i in ids or []:
                self._decided.add(i)
        if self._all_decided():
            self._archive(note=note)

    def finalize(self, *, note: str | None = None) -> None:
        """End-of-review: undecided items DEFAULT to accepted; archive with a note.

        Called on the next request (or turn end) after a partial reject — the
        spec: "un-decided items default to accepted on next request (journal
        archived with a note)".
        """
        undecided = [e for e in self.entries if e.id not in self._decided]
        for e in undecided:
            self._decided.add(e.id)
        msg = note or (
            f"{len(undecided)} undecided item(s) defaulted to accepted"
            if undecided else "finalized"
        )
        self._archive(note=msg)

    def _all_decided(self) -> bool:
        return all(e.id in self._decided for e in self.entries)

    def _archive(self, *, note: str | None) -> None:
        self.journal_dir.mkdir(parents=True, exist_ok=True)
        payload = {
            "request_id": self.request_id,
            "entries": [e.to_dict() for e in self.entries],
        }
        if note:
            payload["note"] = note
        tmp = self._archive_path.with_suffix(".json.tmp")
        tmp.write_bytes(
            json.dumps(payload, ensure_ascii=False, indent=2).encode("utf-8")
        )
        os.replace(tmp, self._archive_path)
        # remove the live file (the journal is now archived).
        self._live_path.unlink(missing_ok=True)


# --------------------------------------------------------------------------- #
# Helpers shared with the tool layer.
# --------------------------------------------------------------------------- #


class _RefuseRollback(Exception):
    """A single item cannot be rolled back (partial snapshot); reported per-item."""


def _category_for(entry: JournalEntry) -> str:
    """Changeset category for an entry.

    ``dat_set`` and a ``dat_reset`` whose kind is ``dat`` group per objId; a
    ``dat_reset`` of kind xdat/tbl falls through to its own flat category (its
    grouping key would be ill-formed under the dat group).
    """
    tool = entry.tool
    if tool == "memory_write":
        return "memory"
    if tool in ("location_write", "player_setup"):
        return "map"  # human-summary item via _map_item
    if tool in _FILE_TOOLS:
        return "file"
    if tool == "dat_set":
        return "dat"
    if tool == "dat_reset":
        return "dat" if entry.after.get("kind") == "dat" else "reset"
    if tool in ("plugin_add", "plugin_edit", "plugin_remove", "plugin_move"):
        return "plugin"
    if tool == "settings_set":
        return "settings"
    if tool == "set_main":
        return "main"
    # tbl_set / xdat_set / req_set / btn_set
    return tool.split("_", 1)[0]


def _target_for(tool: str, args: dict) -> str:
    """A human-facing subject string for the entry/changeset."""
    if tool == "memory_write":
        return f"memory/{args.get('file', '')}"
    if tool == "location_write":
        subject = args.get("name") or f"#{args.get('locationId', '?')}"
        return f"location:{args.get('action', '')} {subject}"
    if tool == "player_setup":
        return f"player:{args.get('action', '')} P{args.get('player', '?')}"
    if tool in _FILE_TOOLS:
        return str(args.get("path", ""))
    if tool in ("dat_set", "xdat_set"):
        name = args.get("param") or args.get("name", "")
        return f"{args.get('dat', '')}|{name}|{args.get('objId', '')}"
    if tool == "dat_reset":
        return (
            f"reset:{args.get('kind', '')}:{args.get('dat', '')}|"
            f"{args.get('param', '')}|{args.get('objId', '')}"
        )
    if tool == "req_set":
        return f"{args.get('dat', '')}|{args.get('objId', '')}"
    if tool == "btn_set":
        return f"button:{args.get('setId', '')}"
    if tool == "tbl_set":
        return f"tbl:{args.get('index', '')}"
    if tool == "set_main":
        return str(args.get("path", ""))
    if tool == "settings_set":
        return f"{args.get('scope', '')}|{args.get('key', '')}"
    if tool in ("plugin_add", "plugin_edit", "plugin_remove"):
        return f"plugin:{args.get('index', '')}"
    if tool == "plugin_move":
        return f"plugin:{args.get('from', '')}->{args.get('to', '')}"
    return tool


def _ftype_for_create(path: str) -> str:
    """Best-effort creatable file type for re-creating a deleted node.

    The bridge does not report a node's EFileType in DELFILE; we infer from the
    extension so a re-create uses a sensible creatable type. ``.eps`` -> CUIEps,
    ``.py`` -> CUIPy, anything else -> RawText (the only other creatable type).
    Limitation: a GUI/CUITrg node cannot be faithfully re-created (no creatable
    type), so a deleted GUI file's rollback is best-effort RawText.
    """
    lower = path.lower()
    if lower.endswith(".eps"):
        return "CUIEps"
    if lower.endswith(".py"):
        return "CUIPy"
    return "RawText"


def _unified_diff(old: str, new: str, label: str) -> str:
    """Server-side unified diff (Python difflib), same as the v1 orchestrator."""
    diff = difflib.unified_diff(
        old.splitlines(keepends=True),
        new.splitlines(keepends=True),
        fromfile=f"a/{label}",
        tofile=f"b/{label}",
    )
    return "".join(diff)


def after_for(tool: str, args: dict, reply: Any) -> dict[str, Any]:
    """Build the ``after`` snapshot for a successful write (called by tools.py).

    Carries the identifying args + new value so the changeset and the inverse-op
    builder have everything they need WITHOUT re-reading the bridge.
    """
    if tool == "dat_set":
        return {"dat": args["dat"], "param": args["param"],
                "objId": int(args["objId"]), "value": str(args["value"])}
    if tool == "xdat_set":
        return {"dat": args["dat"], "name": args["name"],
                "objId": int(args["objId"]), "value": str(args["value"])}
    if tool == "req_set":
        return {"dat": args["dat"], "objId": int(args["objId"]),
                "value": str(args["payload"])}
    if tool == "btn_set":
        return {"setId": int(args["setId"]), "value": str(args["csv"])}
    if tool == "tbl_set":
        return {"index": int(args["index"]), "value": str(args["value"])}
    if tool == "file_write":
        return {"path": args["path"], "content": str(args["code"])}
    if tool == "file_create":
        return {"path": args["path"], "ftype": args["ftype"],
                "content": str(args.get("code", ""))}
    if tool == "mkdir":
        return {"path": args["path"]}
    if tool == "file_delete":
        return {"path": args["path"]}
    if tool == "file_rename":
        return {"path": args["path"], "newname": str(args["newname"])}
    if tool == "file_move":
        return {"path": args["path"],
                "destFolder": str(args.get("destFolder", ""))}
    if tool == "set_main":
        return {"path": args["path"]}
    if tool == "settings_set":
        return {"scope": args["scope"], "key": args["key"],
                "value": str(args["value"])}
    if tool == "plugin_add":
        return {"index": int(args.get("index", -1)),
                "texts": str(args.get("texts", ""))}
    if tool == "plugin_edit":
        return {"index": int(args["index"]), "texts": str(args.get("texts", ""))}
    if tool == "plugin_remove":
        return {"index": int(args["index"])}
    if tool == "plugin_move":
        return {"from": int(args["from"]), "to": int(args["to"])}
    if tool == "memory_write":
        return {"file": args["file"], "content": str(args["content"])}
    return {}
