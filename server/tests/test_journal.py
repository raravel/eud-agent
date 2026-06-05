"""Verification artifact for EUD-055-f4cc: the change journal + rollback engine.

These tests drive ``eud_agent.journal`` (snapshots, JSON persistence, changeset
assembly, inverse-op rollback) and its integration into ``eud_agent.tools``
(every WRITE tool snapshots BEFORE mutating, records ``after`` AFTER). They follow
the FakeBridge pattern from test_tools.py: a recording bridge whose GET methods
return canned reply strings (the real bridge reply formats, e.g.
``"OK: units|HP|0 = 100"``) so the journal's reply parsers are exercised, and
whose write methods record the inverse ``.cmd``-equivalent calls for rollback.

``eud_agent.journal`` does NOT exist during Step A, so this suite is expected to
FAIL on import until journal.py / the tools.py wiring are implemented (Step B).

Contract (features/05 "Change journal and rollback" + "WS protocol v2"):

  * snapshot-then-rollback round-trip per tool kind: apply records the forward
    .cmd, reject replays the INVERSE .cmd in REVERSE seq order;
  * was_default fields roll back via dat_reset (RESETDAT), not a value-write;
  * journal survives a server restart (reload over the same data dir reproduces
    an identical changeset);
  * changeset items match the WS v2 shape (file kinds created|modified|deleted,
    unified diff for modified, dat items grouped per objId);
  * accept archives the journal; undecided items default to accepted; mixed
    per-item decisions supported; a gated/validation-rejected write leaves NO
    journal entry.
"""

from __future__ import annotations

import json

import pytest

# Imported at collection so the failing import is the first signal in Step A.
from eud_agent.journal import (
    Journal,
    JournalEntry,
    parse_get_value,
    parse_main_path,
)

# --------------------------------------------------------------------------- #
# Fake bridge: GET methods return REAL bridge reply strings (so the journal's
# parsers are exercised); write methods record (name, args) for inverse-sequence
# assertions. State is held in dicts so a round-trip can verify final state.
# --------------------------------------------------------------------------- #


class FakeBridge:
    def __init__(self):
        self.calls: list[tuple] = []
        # backing state the GET/SET pairs read/write so a round-trip is real.
        self.dat: dict[tuple, str] = {}
        self.xdat: dict[tuple, str] = {}
        self.tbl: dict[int, str] = {}
        self.req: dict[tuple, str] = {}
        self.btn: dict[int, str] = {}
        self.files: dict[str, str] = {}
        self.settings: dict[tuple, str] = {}
        self.main: str = ""

    def _rec(self, name, *args):
        self.calls.append((name, args))

    # ---- reads (return real reply strings) ----
    def status(self, **kw):
        self._rec("status")
        return "compiling=false\nproject=p\n"

    def getdat(self, dat, param, obj_id, **kw):
        self._rec("getdat", dat, param, obj_id)
        v = self.dat.get((dat, param, obj_id), "0")
        return f"OK: {dat}|{param}|{obj_id} = {v}"

    def getxdat(self, dat, name, obj_id, **kw):
        self._rec("getxdat", dat, name, obj_id)
        v = self.xdat.get((dat, name, obj_id), "0")
        return f"OK: {dat}|{name}|{obj_id} = {v}"

    def gettbl(self, index, **kw):
        self._rec("gettbl", index)
        v = self.tbl.get(index, "")
        return f"OK: {index} = {v}"

    def getreq(self, dat, obj_id, **kw):
        self._rec("getreq", dat, obj_id)
        v = self.req.get((dat, obj_id), "0")
        return f"OK: {dat}|{obj_id} = {v}"

    def getbtn(self, set_id, **kw):
        self._rec("getbtn", set_id)
        v = self.btn.get(set_id, "")
        return f"OK: {set_id} = {v}"

    def getset(self, scope, key, **kw):
        self._rec("getset", scope, key)
        v = self.settings.get((scope, key), "")
        return f"OK: {scope}|{key} = {v}"

    def getmain(self, **kw):
        self._rec("getmain")
        return self.main

    def get(self, path, **kw):
        self._rec("get", path)
        if path not in self.files:
            from eud_agent.bridge_io import BridgeError

            raise BridgeError("ERROR: not found")
        return self.files[path]

    def pluglist(self, **kw):
        self._rec("pluglist")
        return []

    # ---- writes (record + mutate backing state) ----
    def setdat(self, dat, param, obj_id, value, **kw):
        self._rec("setdat", dat, param, obj_id, value)
        self.dat[(dat, param, obj_id)] = str(value)
        return f"OK: {dat}|{param}|{obj_id}"

    def setxdat(self, dat, name, obj_id, value, **kw):
        self._rec("setxdat", dat, name, obj_id, value)
        self.xdat[(dat, name, obj_id)] = str(value)
        return "OK"

    def settbl(self, index, value, **kw):
        self._rec("settbl", index, value)
        if value == "NULLSTRING":
            self.tbl.pop(index, None)
        else:
            self.tbl[index] = value
        return "OK"

    def setreq(self, dat, obj_id, payload, **kw):
        self._rec("setreq", dat, obj_id, payload)
        self.req[(dat, obj_id)] = payload
        return "OK"

    def setbtn(self, set_id, csv, **kw):
        self._rec("setbtn", set_id, csv)
        self.btn[set_id] = csv
        return "OK"

    def resetdat(self, kind, dat, param_or_name, obj_id, **kw):
        self._rec("resetdat", kind, dat, param_or_name, obj_id)
        if kind == "dat":
            self.dat.pop((dat, param_or_name, obj_id), None)
        elif kind == "xdat":
            self.xdat.pop((dat, param_or_name, obj_id), None)
        elif kind == "tbl":
            self.tbl.pop(obj_id, None)
        return "OK"

    def newfile(self, path, ftype, code, **kw):
        self._rec("newfile", path, ftype, code)
        self.files[path] = code
        return "OK"

    def set(self, path, code, **kw):
        self._rec("set", path, code)
        self.files[path] = code
        return "OK"

    def rename(self, path, newname, **kw):
        self._rec("rename", path, newname)
        return "OK"

    def delfile(self, path, **kw):
        self._rec("delfile", path)
        self.files.pop(path, None)
        return "OK"

    def movefile(self, path, dest_folder, **kw):
        self._rec("movefile", path, dest_folder)
        return "OK"

    def mkdir(self, path, **kw):
        self._rec("mkdir", path)
        return "OK"

    def setmain(self, path, **kw):
        self._rec("setmain", path)
        self.main = path
        return "OK"

    def setset(self, scope, key, value, **kw):
        self._rec("setset", scope, key, value)
        self.settings[(scope, key)] = value
        return "OK"

    def plugadd(self, index, texts, **kw):
        self._rec("plugadd", index, texts)
        return "OK"

    def plugset(self, index, texts, **kw):
        self._rec("plugset", index, texts)
        return "OK"

    def plugdel(self, index, **kw):
        self._rec("plugdel", index)
        return "OK"

    def plugmove(self, from_index, to_index, **kw):
        self._rec("plugmove", from_index, to_index)
        return "OK"

    def build(self, **kw):
        self._rec("build")
        return "OK: started"

    # convenience for tests: forward-call names only (no GETs)
    def write_calls(self):
        reads = {
            "getdat", "getxdat", "gettbl", "getreq", "getbtn", "getset",
            "getmain", "get", "pluglist",
        }
        return [c for c in self.calls if c[0] not in reads]


def make_journal(tmp_path, bridge=None, request_id="req-1"):
    bridge = bridge or FakeBridge()
    j = Journal(data_dir=str(tmp_path), request_id=request_id, bridge=bridge)
    return bridge, j


# --------------------------------------------------------------------------- #
# Reply parsers (small, unit-tested per MANDATORY reading).
# --------------------------------------------------------------------------- #


def test_parse_get_value_dat_family():
    assert parse_get_value("OK: units|HP|0 = 100") == "100"
    assert parse_get_value("OK: 5 = hello world") == "hello world"
    assert parse_get_value("OK: project|OpenMapName = C:\\maps\\x.scx") == (
        "C:\\maps\\x.scx"
    )


def test_parse_get_value_handles_equals_in_value():
    # the value itself may contain ' = '; only the FIRST ' = ' splits.
    assert parse_get_value("OK: 3 = a = b = c") == "a = b = c"


def test_parse_get_value_empty_value():
    assert parse_get_value("OK: 7 = ") == ""


def test_parse_main_path():
    assert parse_main_path("Folder/main.eps") == "Folder/main.eps"
    assert parse_main_path("") == ""


# --------------------------------------------------------------------------- #
# Snapshot then record: a journal entry has the spec shape.
# --------------------------------------------------------------------------- #


def test_entry_shape(tmp_path):
    bridge, j = make_journal(tmp_path)
    bridge.dat[("units", "HP", 0)] = "100"
    before = j.snapshot("dat_set", {"dat": "units", "param": "HP", "objId": 0,
                                    "value": 250})
    entry = j.record("dat_set",
                     {"dat": "units", "param": "HP", "objId": 0, "value": 250},
                     before, after={"value": "250"})
    assert isinstance(entry, JournalEntry)
    assert entry.tool == "dat_set"
    assert entry.seq == 0
    assert entry.id
    assert entry.ts
    assert entry.before["value"] == "100"


# --------------------------------------------------------------------------- #
# Per-kind snapshot -> rollback round-trips. apply through the ToolLayer (so the
# wiring is exercised end to end), then reject -> assert the INVERSE .cmd.
# --------------------------------------------------------------------------- #


def _layer_with_journal(tmp_path, bridge, request_id="req-1"):
    from eud_agent.tools import ToolLayer

    bridge2, j = make_journal(tmp_path, bridge=bridge, request_id=request_id)
    layer = ToolLayer(bridge, journal_factory=lambda rid: Journal(
        data_dir=str(tmp_path), request_id=rid, bridge=bridge))
    return layer, j


def test_dat_set_roundtrip(tmp_path):
    bridge = FakeBridge()
    bridge.dat[("units", "HP", 0)] = "100"
    layer, _ = _layer_with_journal(tmp_path, bridge)
    layer.call_for_request("R", "dat_set",
                           {"dat": "units", "param": "HP", "objId": 0,
                            "value": 250})
    assert bridge.dat[("units", "HP", 0)] == "250"
    j = layer.get_journal("R")
    j.rollback(all=True)
    # inverse: setdat back to the snapshotted old value 100.
    assert ("setdat", ("units", "HP", 0, "100")) in bridge.calls
    assert bridge.dat[("units", "HP", 0)] == "100"


def test_dat_set_was_default_rolls_back_via_reset(tmp_path):
    """When the snapshot marks the field as default, rollback uses RESETDAT, not a
    value write."""
    bridge = FakeBridge()
    layer, _ = _layer_with_journal(tmp_path, bridge)
    # explicitly mark the entry's before as was_default via the journal API so the
    # rollback chooses dat_reset (the bridge has no IsDefault read; the journal
    # records was_default=false by default, so we drive the reset path through tbl,
    # which DOES have a real default signal — see test_tbl_set_nullstring below).
    # Here assert the inverse-op CHOICE function picks reset when was_default.
    from eud_agent.journal import inverse_dat_op

    op = inverse_dat_op(tool="dat_set",
                        args={"dat": "units", "param": "HP", "objId": 5},
                        before={"value": "0", "was_default": True})
    assert op["method"] == "resetdat"
    assert op["args"][0] == "dat"  # kind
    op2 = inverse_dat_op(tool="dat_set",
                         args={"dat": "units", "param": "HP", "objId": 5},
                         before={"value": "42", "was_default": False})
    assert op2["method"] == "setdat"


def test_tbl_set_nullstring_was_default_roundtrip(tmp_path):
    """tbl has a real default signal: an empty old value means it was at default,
    so rollback RESETs (NULLSTRING semantics) instead of writing back ''."""
    bridge = FakeBridge()
    # entry not present -> GETTBL returns empty -> was_default True
    layer, _ = _layer_with_journal(tmp_path, bridge)
    layer.call_for_request("R", "tbl_set", {"index": 3, "value": "Marine"})
    assert bridge.tbl[3] == "Marine"
    j = layer.get_journal("R")
    j.rollback(all=True)
    # default before -> reset via RESETDAT tbl (NOT settbl '').
    assert any(c[0] == "resetdat" and c[1][0] == "tbl" for c in bridge.calls)
    assert 3 not in bridge.tbl


def test_tbl_set_nondefault_roundtrip(tmp_path):
    bridge = FakeBridge()
    bridge.tbl[4] = "OldName"
    layer, _ = _layer_with_journal(tmp_path, bridge)
    layer.call_for_request("R", "tbl_set", {"index": 4, "value": "NewName"})
    j = layer.get_journal("R")
    j.rollback(all=True)
    assert ("settbl", (4, "OldName")) in bridge.calls
    assert bridge.tbl[4] == "OldName"


def test_file_write_roundtrip(tmp_path):
    bridge = FakeBridge()
    bridge.files["a.eps"] = "old = 1"
    layer, _ = _layer_with_journal(tmp_path, bridge)
    layer.call_for_request("R", "file_write", {"path": "a.eps", "code": "new = 2"})
    assert bridge.files["a.eps"] == "new = 2"
    j = layer.get_journal("R")
    j.rollback(all=True)
    assert ("set", ("a.eps", "old = 1")) in bridge.calls
    assert bridge.files["a.eps"] == "old = 1"


def test_file_create_roundtrip(tmp_path):
    bridge = FakeBridge()
    layer, _ = _layer_with_journal(tmp_path, bridge)
    layer.call_for_request("R", "file_create",
                           {"path": "n.eps", "ftype": "CUIEps", "code": "y=2"})
    assert bridge.files["n.eps"] == "y=2"
    j = layer.get_journal("R")
    j.rollback(all=True)
    # inverse of create == delete.
    assert ("delfile", ("n.eps",)) in bridge.calls
    assert "n.eps" not in bridge.files


def test_mkdir_roundtrip(tmp_path):
    bridge = FakeBridge()
    layer, _ = _layer_with_journal(tmp_path, bridge)
    layer.call_for_request("R", "mkdir", {"path": "NewFolder"})
    j = layer.get_journal("R")
    j.rollback(all=True)
    assert ("delfile", ("NewFolder",)) in bridge.calls


def test_file_delete_roundtrip(tmp_path):
    bridge = FakeBridge()
    bridge.files["Sub/gone.eps"] = "content here"
    layer, _ = _layer_with_journal(tmp_path, bridge)
    layer.call_for_request("R", "file_delete", {"path": "Sub/gone.eps"})
    assert "Sub/gone.eps" not in bridge.files
    j = layer.get_journal("R")
    j.rollback(all=True)
    # inverse of delete == recreate with the snapshotted content (NEWFILE+content).
    assert any(c[0] == "newfile" and c[1][0] == "Sub/gone.eps"
               and c[1][2] == "content here" for c in bridge.calls)
    assert bridge.files["Sub/gone.eps"] == "content here"


def test_file_rename_roundtrip(tmp_path):
    bridge = FakeBridge()
    bridge.files["old.eps"] = "x"
    layer, _ = _layer_with_journal(tmp_path, bridge)
    layer.call_for_request("R", "file_rename",
                           {"path": "Dir/old.eps", "newname": "new.eps"})
    j = layer.get_journal("R")
    j.rollback(all=True)
    # inverse: rename the now-renamed node (Dir/new.eps) back to the old basename.
    assert ("rename", ("Dir/new.eps", "old.eps")) in bridge.calls


def test_file_move_roundtrip(tmp_path):
    bridge = FakeBridge()
    layer, _ = _layer_with_journal(tmp_path, bridge)
    layer.call_for_request("R", "file_move",
                           {"path": "Dir/x.eps", "destFolder": "Other"})
    j = layer.get_journal("R")
    j.rollback(all=True)
    # inverse: move the node (now at Other/x.eps) back to its original parent Dir.
    assert ("movefile", ("Other/x.eps", "Dir")) in bridge.calls


def test_set_main_roundtrip(tmp_path):
    bridge = FakeBridge()
    bridge.main = "old_main.eps"
    layer, _ = _layer_with_journal(tmp_path, bridge)
    layer.call_for_request("R", "set_main", {"path": "new_main.eps"})
    assert bridge.main == "new_main.eps"
    j = layer.get_journal("R")
    j.rollback(all=True)
    assert ("setmain", ("old_main.eps",)) in bridge.calls
    assert bridge.main == "old_main.eps"


def test_settings_set_roundtrip(tmp_path):
    bridge = FakeBridge()
    bridge.settings[("program", "euddraft")] = "C:\\old\\euddraft.exe"
    layer, _ = _layer_with_journal(tmp_path, bridge)
    layer.call_for_request("R", "settings_set",
                           {"scope": "program", "key": "euddraft",
                            "value": "C:\\new\\euddraft.exe"})
    j = layer.get_journal("R")
    j.rollback(all=True)
    assert ("setset", ("program", "euddraft", "C:\\old\\euddraft.exe")) in (
        bridge.calls
    )
    assert bridge.settings[("program", "euddraft")] == "C:\\old\\euddraft.exe"


def test_plugin_add_roundtrip(tmp_path):
    bridge = FakeBridge()
    layer, _ = _layer_with_journal(tmp_path, bridge)
    layer.call_for_request("R", "plugin_add", {"index": 2, "texts": "[plug]"})
    j = layer.get_journal("R")
    j.rollback(all=True)
    # inverse of add at index 2 == delete index 2.
    assert ("plugdel", (2,)) in bridge.calls


def test_plugin_add_append_sentinel_roundtrip(tmp_path):
    """index=-1 appends; the created index is captured at apply time (pluglist
    length) so rollback can delete the right block."""
    bridge = FakeBridge()
    layer, _ = _layer_with_journal(tmp_path, bridge)
    # pluglist returns [] -> append lands at index 0.
    layer.call_for_request("R", "plugin_add", {"index": -1, "texts": "[plug]"})
    j = layer.get_journal("R")
    j.rollback(all=True)
    assert ("plugdel", (0,)) in bridge.calls


def test_plugin_move_roundtrip(tmp_path):
    bridge = FakeBridge()
    layer, _ = _layer_with_journal(tmp_path, bridge)
    layer.call_for_request("R", "plugin_move", {"from": 1, "to": 4})
    j = layer.get_journal("R")
    j.rollback(all=True)
    # inverse of move 1->4 == move 4->1.
    assert ("plugmove", (4, 1)) in bridge.calls


# --------------------------------------------------------------------------- #
# plugin_edit / plugin_remove partial-honesty: PLUGLIST gives only first_line, so
# full old Texts are NOT recoverable. The entry is marked partial; rollback of a
# partial edit/remove REFUSES with a per-item error rather than writing a
# truncated Texts.
# --------------------------------------------------------------------------- #


def test_plugin_edit_partial_refuses_rollback(tmp_path):
    bridge = FakeBridge()
    layer, _ = _layer_with_journal(tmp_path, bridge)
    layer.call_for_request("R", "plugin_edit", {"index": 0, "texts": "[new]"})
    j = layer.get_journal("R")
    entry = j.entries[-1]
    assert entry.before.get("partial") is True
    before_len = len(bridge.calls)
    result = j.rollback(all=True)
    # the partial entry is reported as a per-item rollback failure, and NO plugset
    # was issued DURING rollback (refuse rather than write a truncated Texts).
    assert not any(c[0] == "plugset" for c in bridge.calls[before_len:])
    assert any(not r["ok"] for r in result["items"])


def test_plugin_remove_partial_refuses_rollback(tmp_path):
    bridge = FakeBridge()
    layer, _ = _layer_with_journal(tmp_path, bridge)
    layer.call_for_request("R", "plugin_remove", {"index": 1})
    j = layer.get_journal("R")
    entry = j.entries[-1]
    assert entry.before.get("partial") is True
    before_len = len(bridge.calls)
    result = j.rollback(all=True)
    # cannot re-add the removed block's full Texts -> refuse; no plugadd issued
    # DURING rollback.
    assert not any(c[0] == "plugadd" for c in bridge.calls[before_len:])
    assert any(not r["ok"] for r in result["items"])


# --------------------------------------------------------------------------- #
# Reverse-order inverse sequencing: a multi-write request rolls back in REVERSE
# seq order.
# --------------------------------------------------------------------------- #


def test_rollback_reverse_seq_order(tmp_path):
    bridge = FakeBridge()
    bridge.files["a.eps"] = "A0"
    bridge.files["b.eps"] = "B0"
    bridge.main = "a.eps"
    layer, _ = _layer_with_journal(tmp_path, bridge)
    layer.approve_plan_for_request("R")  # lift gate so 3 writes go through
    layer.call_for_request("R", "file_write", {"path": "a.eps", "code": "A1"})
    layer.call_for_request("R", "file_write", {"path": "b.eps", "code": "B1"})
    layer.call_for_request("R", "set_main", {"path": "b.eps"})
    j = layer.get_journal("R")
    before_len = len(bridge.calls)
    j.rollback(all=True)
    inverse = [c for c in bridge.calls[before_len:]]
    inverse_names = [c[0] for c in inverse]
    # reverse seq: set_main first (setmain old), then b.eps, then a.eps.
    assert inverse_names == ["setmain", "set", "set"]
    assert inverse[0] == ("setmain", ("a.eps",))
    assert inverse[1] == ("set", ("b.eps", "B0"))
    assert inverse[2] == ("set", ("a.eps", "A0"))


# --------------------------------------------------------------------------- #
# Persistence: JSON written per write, UTF-8 no BOM; reload reproduces an
# identical changeset.
# --------------------------------------------------------------------------- #


def test_journal_persisted_as_utf8_no_bom(tmp_path):
    bridge = FakeBridge()
    bridge.dat[("units", "HP", 0)] = "100"
    layer, _ = _layer_with_journal(tmp_path, bridge)
    layer.call_for_request("R", "dat_set",
                           {"dat": "units", "param": "HP", "objId": 0,
                            "value": 250})
    path = tmp_path / "journal" / "R.json"
    assert path.is_file()
    raw = path.read_bytes()
    assert not raw.startswith(b"\xef\xbb\xbf")  # no BOM
    data = json.loads(raw.decode("utf-8"))
    assert data["request_id"] == "R"
    assert data["entries"][0]["tool"] == "dat_set"


def test_reload_reproduces_identical_changeset(tmp_path):
    bridge = FakeBridge()
    bridge.files["a.eps"] = "old"
    bridge.dat[("units", "HP", 0)] = "100"
    layer, _ = _layer_with_journal(tmp_path, bridge)
    layer.call_for_request("R", "file_write", {"path": "a.eps", "code": "new"})
    layer.call_for_request("R", "dat_set",
                           {"dat": "units", "param": "HP", "objId": 0,
                            "value": 7})
    original = layer.get_journal("R").changeset()
    # NEW journal instance over the same data dir (simulates server restart).
    reloaded = Journal.load(data_dir=str(tmp_path), request_id="R", bridge=bridge)
    assert reloaded.changeset() == original


# --------------------------------------------------------------------------- #
# Changeset WS v2 shape (features/05): file kinds created|modified|deleted with a
# unified diff for modified; dat items grouped per objId with property/old/new.
# --------------------------------------------------------------------------- #


def test_changeset_file_kinds_and_diff(tmp_path):
    bridge = FakeBridge()
    bridge.files["mod.eps"] = "line1\nline2\n"
    bridge.files["del.eps"] = "bye\n"
    layer, _ = _layer_with_journal(tmp_path, bridge)
    layer.approve_plan_for_request("R")
    layer.call_for_request("R", "file_write",
                           {"path": "mod.eps", "code": "line1\nCHANGED\n"})
    layer.call_for_request("R", "file_create",
                           {"path": "new.eps", "ftype": "CUIEps", "code": "z\n"})
    layer.call_for_request("R", "file_delete", {"path": "del.eps"})
    cs = layer.get_journal("R").changeset()
    items = {i["path"]: i for i in cs["items"] if i.get("category") == "file"}
    assert items["mod.eps"]["kind"] == "modified"
    assert "diff" in items["mod.eps"] and "CHANGED" in items["mod.eps"]["diff"]
    assert items["new.eps"]["kind"] == "created"
    assert items["del.eps"]["kind"] == "deleted"


def test_changeset_dat_grouped_per_objid(tmp_path):
    bridge = FakeBridge()
    bridge.dat[("units", "HP", 0)] = "100"
    bridge.dat[("units", "Armor", 0)] = "1"
    bridge.dat[("units", "HP", 7)] = "50"
    layer, _ = _layer_with_journal(tmp_path, bridge)
    layer.approve_plan_for_request("R")
    layer.call_for_request("R", "dat_set",
                           {"dat": "units", "param": "HP", "objId": 0,
                            "value": 250})
    layer.call_for_request("R", "dat_set",
                           {"dat": "units", "param": "Armor", "objId": 0,
                            "value": 5})
    layer.call_for_request("R", "dat_set",
                           {"dat": "units", "param": "HP", "objId": 7,
                            "value": 999})
    cs = layer.get_journal("R").changeset()
    dat_items = [i for i in cs["items"] if i.get("category") == "dat"]
    # grouped per objId: one group for (units,0) with 2 properties, one for (units,7).
    by_obj = {(i["dat"], i["objId"]): i for i in dat_items}
    assert set(by_obj) == {("units", 0), ("units", 7)}
    props0 = {p["property"]: p for p in by_obj[("units", 0)]["properties"]}
    assert props0["HP"]["old"] == "100" and props0["HP"]["new"] == "250"
    assert props0["Armor"]["old"] == "1" and props0["Armor"]["new"] == "5"
    assert len(by_obj[("units", 7)]["properties"]) == 1


# --------------------------------------------------------------------------- #
# Accept archives; undecided default to accepted; mixed per-item decisions.
# --------------------------------------------------------------------------- #


def test_accept_archives_journal(tmp_path):
    bridge = FakeBridge()
    bridge.files["a.eps"] = "old"
    layer, _ = _layer_with_journal(tmp_path, bridge)
    layer.call_for_request("R", "file_write", {"path": "a.eps", "code": "new"})
    j = layer.get_journal("R")
    j.accept(all=True)
    live = tmp_path / "journal" / "R.json"
    archived = tmp_path / "journal" / "R.accepted.json"
    assert not live.is_file()
    assert archived.is_file()
    # accept does NOT roll back: the value stays applied.
    assert bridge.files["a.eps"] == "new"


def test_mixed_per_item_decision_rejects_only_selected(tmp_path):
    bridge = FakeBridge()
    bridge.files["a.eps"] = "A0"
    bridge.files["b.eps"] = "B0"
    layer, _ = _layer_with_journal(tmp_path, bridge)
    layer.approve_plan_for_request("R")
    e_a = layer.call_for_request("R", "file_write", {"path": "a.eps", "code": "A1"})
    layer.call_for_request("R", "file_write", {"path": "b.eps", "code": "B1"})
    j = layer.get_journal("R")
    # reject only the a.eps entry by its id.
    a_id = j.entries[0].id
    result = j.rollback(ids=[a_id])
    assert bridge.files["a.eps"] == "A0"  # rolled back
    assert bridge.files["b.eps"] == "B1"  # kept
    assert all(r["ok"] for r in result["items"] if r["id"] == a_id)
    # the call result of the apply returned a usable handle (entry/result).
    assert e_a is not None


def test_undecided_items_default_to_accepted(tmp_path):
    """After a partial reject, the remaining (undecided) items default to accepted:
    finalize archives the journal with a note and does NOT roll them back."""
    bridge = FakeBridge()
    bridge.files["a.eps"] = "A0"
    bridge.files["b.eps"] = "B0"
    layer, _ = _layer_with_journal(tmp_path, bridge)
    layer.approve_plan_for_request("R")
    layer.call_for_request("R", "file_write", {"path": "a.eps", "code": "A1"})
    layer.call_for_request("R", "file_write", {"path": "b.eps", "code": "B1"})
    j = layer.get_journal("R")
    a_id = j.entries[0].id
    j.rollback(ids=[a_id])
    j.finalize()  # undecided (b.eps) defaults to accepted
    assert bridge.files["b.eps"] == "B1"  # NOT rolled back
    archived = tmp_path / "journal" / "R.accepted.json"
    assert archived.is_file()
    data = json.loads(archived.read_text(encoding="utf-8"))
    assert data.get("note")  # archived with a note about defaulted items


# --------------------------------------------------------------------------- #
# Journal-only-on-success: a gated or validation-rejected write leaves NO entry.
# --------------------------------------------------------------------------- #


def test_gated_write_leaves_no_journal_entry(tmp_path):
    from eud_agent.tools import PlanRequired

    bridge = FakeBridge()
    bridge.files["a.eps"] = bridge.files["b.eps"] = bridge.files["c.eps"] = "x"
    layer, _ = _layer_with_journal(tmp_path, bridge)
    layer.call_for_request("R", "file_write", {"path": "a.eps", "code": "1"})
    layer.call_for_request("R", "file_write", {"path": "b.eps", "code": "2"})
    with pytest.raises(PlanRequired):
        layer.call_for_request("R", "file_write", {"path": "c.eps", "code": "3"})
    j = layer.get_journal("R")
    # only the 2 successful writes are journaled; the gated 3rd left no entry.
    assert len(j.entries) == 2
    assert all(e.target != "c.eps" for e in j.entries)


def test_validation_rejected_write_leaves_no_journal_entry(tmp_path):
    from eud_agent.tools import ToolError

    bridge = FakeBridge()
    layer, _ = _layer_with_journal(tmp_path, bridge)
    with pytest.raises(ToolError):
        layer.call_for_request("R", "dat_set",
                               {"dat": "nope", "param": "HP", "objId": 0,
                                "value": 1})
    j = layer.get_journal("R")
    assert len(j.entries) == 0
    # no snapshot GET was even issued for an arg-invalid write.
    assert bridge.calls == []


def test_read_tool_never_journals(tmp_path):
    bridge = FakeBridge()
    bridge.files["a.eps"] = "x"
    layer, _ = _layer_with_journal(tmp_path, bridge)
    layer.call_for_request("R", "read_file", {"path": "a.eps"})
    layer.call_for_request("R", "project_status", {})
    j = layer.get_journal("R")
    assert len(j.entries) == 0


# --------------------------------------------------------------------------- #
# Optional/injectable: a ToolLayer WITHOUT a journal still works (additive).
# --------------------------------------------------------------------------- #


def test_tool_layer_without_journal_still_writes(tmp_path):
    from eud_agent.tools import ToolLayer

    bridge = FakeBridge()
    bridge.files["a.eps"] = "old"
    layer = ToolLayer(bridge)  # no journal_factory
    layer.call_for_request("R", "file_write", {"path": "a.eps", "code": "new"})
    assert bridge.files["a.eps"] == "new"
    # no journal handle exists when none was injected.
    assert layer.get_journal("R") is None


# --------------------------------------------------------------------------- #
# dat_reset (review round 1 BLOCKING): snapshot the pre-reset value via the
# matching GET, write it back on rollback (per kind dat/xdat/tbl).
# --------------------------------------------------------------------------- #


def test_dat_reset_dat_roundtrip(tmp_path):
    bridge = FakeBridge()
    bridge.dat[("units", "HP", 0)] = "100"
    layer, _ = _layer_with_journal(tmp_path, bridge)
    layer.call_for_request("R", "dat_reset",
                           {"kind": "dat", "dat": "units", "param": "HP",
                            "objId": 0})
    # reset cleared the override -> getdat now reports the default "0".
    assert ("units", "HP", 0) not in bridge.dat
    j = layer.get_journal("R")
    # before captured the pre-reset value 100; after captured the stock read-back.
    assert j.entries[-1].before["value"] == "100"
    j.rollback(all=True)
    # inverse: write the snapshotted pre-reset value 100 back via setdat.
    assert ("setdat", ("units", "HP", 0, "100")) in bridge.calls
    assert bridge.dat[("units", "HP", 0)] == "100"


def test_dat_reset_xdat_roundtrip(tmp_path):
    bridge = FakeBridge()
    bridge.xdat[("statusinfor", "Status", 5)] = "7"
    layer, _ = _layer_with_journal(tmp_path, bridge)
    layer.call_for_request("R", "dat_reset",
                           {"kind": "xdat", "dat": "statusinfor",
                            "param": "Status", "objId": 5})
    assert ("statusinfor", "Status", 5) not in bridge.xdat
    j = layer.get_journal("R")
    assert j.entries[-1].before["value"] == "7"
    j.rollback(all=True)
    assert ("setxdat", ("statusinfor", "Status", 5, "7")) in bridge.calls
    assert bridge.xdat[("statusinfor", "Status", 5)] == "7"


def test_dat_reset_tbl_roundtrip(tmp_path):
    bridge = FakeBridge()
    bridge.tbl[9] = "Custom Name"
    layer, _ = _layer_with_journal(tmp_path, bridge)
    layer.call_for_request("R", "dat_reset",
                           {"kind": "tbl", "objId": 9})
    assert 9 not in bridge.tbl
    j = layer.get_journal("R")
    assert j.entries[-1].before["value"] == "Custom Name"
    j.rollback(all=True)
    assert ("settbl", (9, "Custom Name")) in bridge.calls
    assert bridge.tbl[9] == "Custom Name"


def test_dat_reset_groups_in_changeset_per_objid(tmp_path):
    """A dat-kind reset appears in the changeset grouped per (dat,objId) like a
    dat_set, with old=pre-reset value and new=stock value."""
    bridge = FakeBridge()
    bridge.dat[("units", "HP", 0)] = "100"
    layer, _ = _layer_with_journal(tmp_path, bridge)
    layer.call_for_request("R", "dat_reset",
                           {"kind": "dat", "dat": "units", "param": "HP",
                            "objId": 0})
    cs = layer.get_journal("R").changeset()
    dat_items = [i for i in cs["items"] if i.get("category") == "dat"]
    assert len(dat_items) == 1
    grp = dat_items[0]
    assert grp["dat"] == "units" and grp["objId"] == 0
    prop = grp["properties"][0]
    assert prop["property"] == "HP"
    assert prop["old"] == "100"
    assert prop["new"] == "0"  # stock value read back after the reset


# --------------------------------------------------------------------------- #
# build_run is NOT journaled (review round 1 advisory 1): no entry, no changeset
# item, never a rollback selection. Gate/budget still count it as a mutation.
# --------------------------------------------------------------------------- #


def test_build_run_not_journaled(tmp_path):
    bridge = FakeBridge()
    layer, _ = _layer_with_journal(tmp_path, bridge)
    layer.call_for_request("R", "build_run", {})
    assert ("build", ()) in bridge.calls
    j = layer.get_journal("R")
    # no journal entry for the build.
    assert len(j.entries) == 0
    # no changeset item for the build.
    assert j.changeset()["items"] == []
    # but the action/mutation budget still counted it (kind=write).
    st = layer.get_request_state("R")
    assert st.action_count == 1
    assert st.mutation_count == 1


def test_build_run_among_writes_only_skips_itself(tmp_path):
    bridge = FakeBridge()
    bridge.files["a.eps"] = "A0"
    layer, _ = _layer_with_journal(tmp_path, bridge)
    layer.approve_plan_for_request("R")
    layer.call_for_request("R", "file_write", {"path": "a.eps", "code": "A1"})
    layer.call_for_request("R", "build_run", {})
    j = layer.get_journal("R")
    # only the file_write is journaled.
    assert len(j.entries) == 1 and j.entries[0].tool == "file_write"
    items = j.changeset()["items"]
    assert all(i.get("tool") != "build_run" for i in items)
    # rollback never presents a build item.
    res = j.rollback(all=True)
    assert all("build" not in str(r) for r in res["items"])
    assert bridge.files["a.eps"] == "A0"


# --------------------------------------------------------------------------- #
# set_main with no prior main (review round 1 advisory 2): honest per-item
# refusal at rollback (no clear-main primitive), not an incidental BridgeError.
# --------------------------------------------------------------------------- #


def test_set_main_empty_prior_main_refuses_rollback(tmp_path):
    bridge = FakeBridge()
    bridge.main = ""  # no prior main file
    layer, _ = _layer_with_journal(tmp_path, bridge)
    layer.call_for_request("R", "set_main", {"path": "new_main.eps"})
    assert bridge.main == "new_main.eps"
    j = layer.get_journal("R")
    assert j.entries[-1].before.get("partial") is True
    before_len = len(bridge.calls)
    result = j.rollback(all=True)
    # refusal: no setmain("") issued during rollback (would be a path error).
    assert not any(c[0] == "setmain" for c in bridge.calls[before_len:])
    item = result["items"][0]
    assert item["ok"] is False
    assert "clear-main" in item["error"] or "no prior main" in item["error"].lower()


# --------------------------------------------------------------------------- #
# Snapshot-before-mutate hard guarantee (review round 1 advisory 3): a pre-write
# GET failure FAILS the call (ToolError), performs no write, leaves no entry.
# --------------------------------------------------------------------------- #


def test_file_write_snapshot_get_failure_fails_call_no_write(tmp_path):
    from eud_agent.tools import ToolError

    bridge = FakeBridge()
    # 'a.eps' is NOT in bridge.files -> the pre-write GET raises BridgeError.
    layer, _ = _layer_with_journal(tmp_path, bridge)
    with pytest.raises(ToolError):
        layer.call_for_request("R", "file_write",
                               {"path": "a.eps", "code": "new"})
    # no SET .cmd was issued (the write must not happen on a failed snapshot).
    assert not any(c[0] == "set" for c in bridge.calls)
    assert "a.eps" not in bridge.files
    # no journal entry recorded.
    j = layer.get_journal("R")
    assert len(j.entries) == 0


def test_file_delete_snapshot_get_failure_fails_call_no_write(tmp_path):
    from eud_agent.tools import ToolError

    bridge = FakeBridge()
    layer, _ = _layer_with_journal(tmp_path, bridge)
    with pytest.raises(ToolError):
        layer.call_for_request("R", "file_delete", {"path": "missing.eps"})
    assert not any(c[0] == "delfile" for c in bridge.calls)
    j = layer.get_journal("R")
    assert len(j.entries) == 0


# --------------------------------------------------------------------------- #
# Through-the-layer round-trips for the remaining dat-family writes (review round
# 1 advisory 6): xdat_set, req_set, btn_set.
# --------------------------------------------------------------------------- #


def test_xdat_set_roundtrip(tmp_path):
    bridge = FakeBridge()
    bridge.xdat[("statusinfor", "Status", 3)] = "2"
    layer, _ = _layer_with_journal(tmp_path, bridge)
    layer.call_for_request("R", "xdat_set",
                           {"dat": "statusinfor", "name": "Status", "objId": 3,
                            "value": 9})
    assert bridge.xdat[("statusinfor", "Status", 3)] == "9"
    j = layer.get_journal("R")
    j.rollback(all=True)
    assert ("setxdat", ("statusinfor", "Status", 3, "2")) in bridge.calls
    assert bridge.xdat[("statusinfor", "Status", 3)] == "2"


def test_req_set_roundtrip(tmp_path):
    bridge = FakeBridge()
    bridge.req[("units", 0)] = "0"  # old use-mode Default
    layer, _ = _layer_with_journal(tmp_path, bridge)
    layer.call_for_request("R", "req_set",
                           {"dat": "units", "objId": 0, "payload": "Always"})
    j = layer.get_journal("R")
    assert j.entries[-1].before["value"] == "0"
    j.rollback(all=True)
    # inverse writes the snapshotted old copy-string ("0") back via setreq.
    assert ("setreq", ("units", 0, "0")) in bridge.calls
    assert bridge.req[("units", 0)] == "0"


def test_btn_set_roundtrip(tmp_path):
    bridge = FakeBridge()
    old_csv = "0,1,2,3,4,5,6,7"
    bridge.btn[2] = old_csv
    layer, _ = _layer_with_journal(tmp_path, bridge)
    layer.call_for_request("R", "btn_set",
                           {"setId": 2, "csv": "8,9,10,11,12,13,14,15"})
    assert bridge.btn[2] == "8,9,10,11,12,13,14,15"
    j = layer.get_journal("R")
    j.rollback(all=True)
    assert ("setbtn", (2, old_csv)) in bridge.calls
    assert bridge.btn[2] == old_csv
