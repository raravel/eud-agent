"""Verification artifact for the map_info tool (features/08).

Drives ``eud_agent.chk_info`` (the CHK TLV walk, section decoders, mode
slicing, and the IsomTerrain-spawning service) plus the ``map_info`` routing in
``eud_agent.tools`` — all headless: CHK bytes are synthesized in-test (no
binary fixtures committed), the IsomTerrain spawn is a fake that writes the
fixture CHK, and the bridge is a recording fake (the test_tools pattern).

What this suite pins:

  * **TLV walk** — file-order sections, EOF clamp on a lying size, and the
    protected-map NEGATIVE-size jump terminating via the iteration cap;
  * **duplicate resolution** — UNIT payloads STACK, scalar sections last-win;
  * **decoders** — MRGN (unused-entry skip, Anywhere flag, 1-based ids, names),
    UNIT (36-byte struct, owner labels, start locations = type 214), FORC
    (SHORT section zero-padded), OWNR/SIDE names, STR + STRx, and the
    utf-8 -> cp949 fallback for Korean strings;
  * **mode slicing** — summary aggregates (no raw unit list), the units-mode
    owner/unitType filters and the :data:`UNITS_LIST_CAP` truncation marker;
  * **service errors** — each failure (unset exe, empty OpenMapName, missing
    map file, nonzero exit, no output CHK) is a distinct, clear MapInfoError;
  * **tool routing** — ``map_info`` is a registered READ tool; a missing
    service or a MapInfoError is a ToolError (codex-correctable, no crash); an
    invalid mode is rejected BEFORE the service runs and counts NOTHING; a
    successful read counts one action and no mutation.
"""

from __future__ import annotations

import struct
from pathlib import Path

import pytest

from eud_agent.chk_info import (
    UNIT_ENTRY_SIZE,
    UNITS_LIST_CAP,
    MapInfoError,
    MapInfoService,
    assemble_sections,
    decode_text,
    detect_str_encoding,
    digest_chk,
    owner_label,
    parse_strings,
    restore_map_backup,
    slice_digest,
    unit_name,
    walk_sections,
)
from eud_agent.journal import Journal
from eud_agent.tools import (
    READ_TOOLS,
    WRITE_TOOLS,
    PlanRequired,
    RequestState,
    ToolError,
    ToolLayer,
)

# --------------------------------------------------------------------------- #
# CHK byte builders (synthesized in-test; no binary fixtures).
# --------------------------------------------------------------------------- #


def sec(name: str, body: bytes) -> bytes:
    """One TLV section: 4-byte name + u32 size + body."""
    assert len(name) == 4
    return name.encode("latin-1") + struct.pack("<I", len(body)) + body


def str_section(strings: list[bytes], *, extended: bool = False) -> bytes:
    """A ``STR ``/``STRx`` body: count + offsets + null-terminated data."""
    width = 4 if extended else 2
    fmt = "<I" if extended else "<H"
    base = width + width * len(strings)
    offsets, data = [], b""
    for s in strings:
        offsets.append(base + len(data))
        data += s + b"\x00"
    return (
        struct.pack(fmt, len(strings))
        + b"".join(struct.pack(fmt, o) for o in offsets)
        + data
    )


def unit_entry(
    unit_id: int, owner: int, x: int, y: int, *, resources: int = 0
) -> bytes:
    """One 36-byte UNIT entry (Chk.h struct Unit field order)."""
    return struct.pack(
        "<IHHHHHHBBBBIHHII",
        0, x, y, unit_id, 0, 0, 0, owner, 100, 100, 100, resources, 0, 0, 0, 0,
    )


def mrgn_entry(
    left: int, top: int, right: int, bottom: int, string_id: int
) -> bytes:
    return struct.pack("<IIIIHH", left, top, right, bottom, string_id, 0)


def build_fixture_chk() -> bytes:
    """A small but complete map CHK: 64x96 jungle, 2 humans + 1 computer,
    2 named locations (one Korean/cp949), marines + minerals + start points."""
    # String ids are 1-based: 1=공격지점, 2=Base, 3=Attackers, 4=Defenders.
    strings = str_section([
        "공격지점".encode("cp949"),
        b"Base",
        b"Attackers",
        b"Defenders",
    ])
    ownr = bytes([6, 6, 5] + [0] * 8 + [7])  # P1/P2 human, P3 computer, P12 neutral
    side = bytes([1, 2, 0] + [7] * 8 + [4])  # terran, protoss, zerg
    # FORC short form (18 of 20 bytes): P1,P2 -> force 0; P3 -> force 1;
    # force names 3/4; flags only for force 1 (allies|allied victory) — the
    # final flags byte is CUT to prove zero-padding.
    forc = (
        bytes([0, 0, 1, 0, 0, 0, 0, 0])
        + struct.pack("<4H", 3, 4, 0, 0)
        + bytes([0x6, 0x9])
    )
    mrgn = (
        mrgn_entry(32, 32, 96, 96, 1)            # location 1 "공격지점"
        + mrgn_entry(0, 0, 0, 0, 0)              # unused -> skipped
        + mrgn_entry(320, 320, 640, 640, 2)      # location 3 "Base"
        + mrgn_entry(0, 0, 0, 0, 0) * 60         # pad to the Anywhere slot
        + mrgn_entry(0, 0, 2048, 3072, 0)        # index 63 = Anywhere
    )
    units = (
        unit_entry(214, 0, 64, 64)            # P1 start location
        + unit_entry(214, 1, 1984, 2944)      # P2 start location
        + unit_entry(0, 0, 100, 100)          # P1 Terran Marine
        + unit_entry(0, 0, 120, 100)          # P1 Terran Marine
        + unit_entry(65, 1, 700, 700)         # P2 Protoss Zealot
        + unit_entry(176, 11, 400, 416, resources=1500)  # neutral minerals
    )
    return (
        sec("VER ", struct.pack("<H", 206))
        + sec("DIM ", struct.pack("<HH", 64, 96))
        + sec("ERA ", struct.pack("<H", 4))      # jungle
        + sec("OWNR", ownr)
        + sec("SIDE", side)
        + sec("FORC", forc)
        + sec("MRGN", mrgn)
        + sec("STR ", strings)
        + sec("UNIT", units)
    )


# --------------------------------------------------------------------------- #
# TLV walk + duplicate resolution.
# --------------------------------------------------------------------------- #


def test_walk_sections_returns_file_order_and_clamps_lying_size():
    """A size pointing past EOF yields the remaining bytes (SC's short read)."""
    lying = b"UNIT" + struct.pack("<I", 9999) + b"tail"
    data = sec("DIM ", b"\x40\x00\x60\x00") + lying
    sections = walk_sections(data)
    assert [name for name, _ in sections] == ["DIM ", "UNIT"]
    assert sections[1][1] == b"tail"


def test_walk_sections_negative_size_jump_terminates():
    """The protected-map negative-size trick must not loop forever: a header
    whose size jumps the cursor back onto itself ends via the iteration cap."""
    data = b"PROT" + struct.pack("<i", -8) + sec("DIM ", b"\x10\x00\x10\x00")
    sections = walk_sections(data)  # must return, not hang
    assert ("DIM ", b"\x10\x00\x10\x00") not in sections[:1]  # PROT seen first
    assert len(sections) >= 1


def test_assemble_unit_sections_stack_and_scalars_last_win():
    sections = [
        ("DIM ", b"old!"),
        ("UNIT", b"A" * UNIT_ENTRY_SIZE),
        ("DIM ", b"new!"),
        ("UNIT", b"B" * UNIT_ENTRY_SIZE),
    ]
    resolved = assemble_sections(sections)
    assert resolved["DIM "] == b"new!"
    assert resolved["UNIT"] == b"A" * UNIT_ENTRY_SIZE + b"B" * UNIT_ENTRY_SIZE


# --------------------------------------------------------------------------- #
# Strings (STR/STRx + encoding fallback).
# --------------------------------------------------------------------------- #


def test_parse_strings_str_and_strx_variants():
    body = str_section([b"one", b"two"])
    assert parse_strings(body, extended=False) == ["one", "two"]
    bodyx = str_section([b"one", b"two"], extended=True)
    assert parse_strings(bodyx, extended=True) == ["one", "two"]


def test_parse_strings_out_of_range_offset_keeps_id_alignment():
    # Two entries; the first offset points past the section end -> "".
    body = struct.pack("<HHH", 2, 9999, 6 + 0) + b"ok\x00"
    assert parse_strings(body, extended=False) == ["", "ok"]


def test_decode_text_korean_cp949_fallback_and_utf8_first():
    assert decode_text("공격지점".encode("cp949")) == "공격지점"
    assert decode_text("한글".encode()) == "한글"
    # Arbitrary bytes never raise (latin-1/replace tail).
    assert isinstance(decode_text(b"\xff\xfe\x80"), str)


# --------------------------------------------------------------------------- #
# Full digest over the fixture map.
# --------------------------------------------------------------------------- #


@pytest.fixture()
def digest():
    return digest_chk(build_fixture_chk())


def test_digest_map_header(digest):
    assert digest["map"] == {"width": 64, "height": 96, "tileset": "jungle"}


def test_digest_locations_skip_unused_and_flag_anywhere(digest):
    locs = digest["locations"]
    assert [loc["id"] for loc in locs] == [1, 3, 64]
    assert locs[0]["name"] == "공격지점"          # cp949 round-trip
    assert locs[0]["tileRect"] == [1, 1, 3, 3]   # px // 32
    assert locs[1]["name"] == "Base"
    assert locs[2].get("anywhere") is True


def test_digest_units_and_start_locations(digest):
    units = digest["units"]
    assert len(units) == 6
    marine = units[2]
    assert marine["type"] == "Terran Marine" and marine["typeId"] == 0
    assert marine["owner"] == "P1"
    assert (marine["tileX"], marine["tileY"]) == (3, 3)
    minerals = units[5]
    assert minerals["type"] == "Mineral Field (Type 1)"
    assert minerals["owner"] == "P12 (neutral)"
    assert minerals["resources"] == 1500
    starts = digest["startLocations"]
    assert [(s["player"], s["tileX"], s["tileY"]) for s in starts] == [
        ("P1", 2, 2), ("P2", 62, 92),
    ]


def test_digest_players_forces_short_forc_padded(digest):
    players = digest["players"]
    assert players[0] == {
        "player": "P1", "controller": "Human (Open Slot)",
        "race": "Terran", "force": 1,
    }
    assert players[2]["controller"] == "Computer"
    assert players[2]["force"] == 2
    assert players[11]["controller"] == "Neutral"
    forces = digest["forces"]
    assert forces[0]["name"] == "Attackers"
    assert forces[0]["players"] == ["P1", "P2"]
    assert forces[0]["flags"]["allies"] and forces[0]["flags"]["alliedVictory"]
    assert forces[1]["name"] == "Defenders" and forces[1]["players"] == ["P3"]
    # Cut flags byte (force 3) decoded as zero-padded -> all False; default name.
    assert forces[2]["name"] == "Force 3"
    assert not any(forces[2]["flags"].values())


def test_owner_label_and_unit_name_fallback():
    assert owner_label(0) == "P1"
    assert owner_label(11) == "P12 (neutral)"
    assert unit_name(214) == "Start Location"
    assert unit_name(9999) == "ID:9999"


# --------------------------------------------------------------------------- #
# Mode slicing + filters.
# --------------------------------------------------------------------------- #


def test_slice_summary_aggregates_without_raw_unit_list(digest):
    out = slice_digest(digest, "summary")
    assert out["unitCount"] == 6
    assert out["unitsByOwner"]["P1"]["Terran Marine"] == 2
    assert out["locationCount"] == 3
    assert "공격지점" in out["locationNames"]
    assert "units" not in out and "locations" not in out
    # Inactive slots are dropped from the summary players view.
    assert [p["player"] for p in out["players"]] == ["P1", "P2", "P3", "P12"]


def test_slice_units_filters_by_owner_and_type(digest):
    by_owner = slice_digest(digest, "units", owner="P2")
    assert {u["type"] for u in by_owner["units"]} == {
        "Start Location", "Protoss Zealot",
    }
    by_name = slice_digest(digest, "units", unit_type="marine")
    assert by_name["matched"] == 2
    by_id = slice_digest(digest, "units", unit_type="65")
    assert by_id["matched"] == 1 and by_id["units"][0]["type"] == "Protoss Zealot"
    neutral = slice_digest(digest, "units", owner="neutral")
    assert neutral["matched"] == 1


def test_slice_units_caps_the_list_with_truncation_marker():
    body = b"".join(
        unit_entry(0, 0, 32 * (i % 60), 32 * (i // 60)) for i in range(250)
    )
    digest = digest_chk(sec("DIM ", struct.pack("<HH", 64, 64)) + sec("UNIT", body))
    out = slice_digest(digest, "units")
    assert out["matched"] == 250
    assert len(out["units"]) == UNITS_LIST_CAP
    assert out["truncated"] is True and "filters" in out["hint"]


def test_slice_locations_and_players_modes(digest):
    locs = slice_digest(digest, "locations")
    assert locs["locationCount"] == 3 and len(locs["locations"]) == 3
    players = slice_digest(digest, "players")
    assert len(players["players"]) == 12 and len(players["forces"]) == 4


# --------------------------------------------------------------------------- #
# The service (fake bridge + fake spawn; no real exe).
# --------------------------------------------------------------------------- #


class FakeBridge:
    """GETSET/STATUS bridge fake returning the configured OpenMapName."""

    def __init__(self, open_map: str, compiling: str = "False"):
        self.open_map = open_map
        self.compiling = compiling
        self.calls: list[tuple] = []

    def getset(self, scope, key, **kw):
        self.calls.append(("getset", scope, key))
        return f"OK: {scope}|{key} = {self.open_map}"

    def status(self, **kw):
        return f"compiling={self.compiling} project='demo'"


def make_service(tmp_path, *, chk: bytes | None = None, returncode=0,
                 stderr="", write_output=True):
    """A MapInfoService over a dummy exe + a fake spawn writing ``chk``."""
    exe = tmp_path / "IsomTerrain.exe"
    exe.write_bytes(b"MZ")
    map_path = tmp_path / "demo.scx"
    map_path.write_bytes(b"MPQ")
    bridge = FakeBridge(str(map_path))
    spawn_calls: list[list] = []

    def fake_spawn(argv, **kwargs):
        spawn_calls.append(argv)
        assert kwargs["stdin"] is not None  # explicit stdin (rules.md)
        assert "timeout" in kwargs and kwargs["cwd"]
        if write_output and returncode == 0:
            from pathlib import Path
            Path(argv[3]).write_bytes(chk or build_fixture_chk())

        class _P:
            pass

        p = _P()
        p.returncode = returncode
        p.stdout = ""
        p.stderr = stderr
        return p

    svc = MapInfoService(bridge, isomterrain_cmd=str(exe), spawn=fake_spawn)
    return svc, bridge, spawn_calls, map_path


def test_service_happy_path_summary_includes_path_and_saved_at(tmp_path):
    svc, bridge, spawn_calls, map_path = make_service(tmp_path)
    out = svc.map_info("summary")
    assert out["map"]["path"] == str(map_path)
    assert out["map"]["tileset"] == "jungle"
    assert "savedAt" in out["map"]  # disk-staleness signal
    assert out["unitCount"] == 6
    # The spawn ran `<exe> chk <map> <tmp.chk>` and GETSET hit OpenMapName.
    assert spawn_calls[0][1] == "chk" and spawn_calls[0][2] == str(map_path)
    assert ("getset", "project", "OpenMapName") in bridge.calls


def test_service_unconfigured_exe_is_a_clear_error(tmp_path):
    map_path = tmp_path / "demo.scx"
    map_path.write_bytes(b"MPQ")
    svc = MapInfoService(FakeBridge(str(map_path)), isomterrain_cmd="")
    with pytest.raises(MapInfoError, match="map_info unavailable"):
        svc.map_info()


def test_service_empty_open_map_name_is_a_clear_error(tmp_path):
    svc, *_ = make_service(tmp_path)
    svc._bridge.open_map = ""
    with pytest.raises(MapInfoError, match="no map is connected"):
        svc.map_info()


def test_service_missing_map_file_is_a_clear_error(tmp_path):
    svc, bridge, *_ = make_service(tmp_path)
    bridge.open_map = str(tmp_path / "gone.scx")
    with pytest.raises(MapInfoError, match="not found on disk"):
        svc.map_info()


def test_service_nonzero_exit_surfaces_stderr(tmp_path):
    svc, *_ = make_service(tmp_path, returncode=1, stderr="bad MPQ header")
    with pytest.raises(MapInfoError, match="bad MPQ header"):
        svc.map_info()


def test_service_no_output_chk_is_a_clear_error(tmp_path):
    svc, *_ = make_service(tmp_path, write_output=False)
    with pytest.raises(MapInfoError, match="produced no CHK"):
        svc.map_info()


# --------------------------------------------------------------------------- #
# location_write service (features/09): fake spawn handles BOTH locedit + chk.
# --------------------------------------------------------------------------- #


def make_locedit_service(tmp_path, *, returncode=0, stderr="",
                         lock_probe=None, compiling="False"):
    """A MapInfoService whose fake spawn records the locedit ops bytes."""
    exe = tmp_path / "IsomTerrain.exe"
    exe.write_bytes(b"MZ")
    map_path = tmp_path / "demo.scx"
    map_path.write_bytes(b"ORIGINAL-MAP-BYTES")
    bridge = FakeBridge(str(map_path), compiling=compiling)
    recorded: dict = {"ops": [], "argv": []}

    def fake_spawn(argv, **kwargs):
        from pathlib import Path as _P
        recorded["argv"].append(argv)
        assert kwargs["stdin"] is not None and kwargs["cwd"]

        class _R:
            pass

        p = _R()
        p.returncode = returncode
        p.stdout = ""
        p.stderr = stderr
        if returncode != 0:
            return p
        if argv[1] == "locedit":
            recorded["ops"].append(_P(argv[3]).read_bytes())
            p.stdout = "OK add #1\nSAVED 1 ops"
        elif argv[1] == "chk":
            _P(argv[3]).write_bytes(build_fixture_chk())
        return p

    svc = MapInfoService(
        bridge,
        isomterrain_cmd=str(exe),
        spawn=fake_spawn,
        data_dir=str(tmp_path / "data"),
        lock_probe=lock_probe or (lambda p: False),
    )
    return svc, map_path, recorded


def test_location_write_add_converts_tiles_backs_up_and_verifies(tmp_path):
    svc, map_path, recorded = make_locedit_service(tmp_path)
    out = svc.location_write("add", name="TestZone",
                             left=10, top=10, right=20, bottom=20)
    assert out["ok"] is True and out["locationId"] == 1
    assert out["mapPath"] == str(map_path)
    # tiles * 32 -> px in the ops line; name passed as raw bytes.
    assert recorded["ops"] == [b"add|320|320|640|640|TestZone\n"]
    # Full-file backup under data_dir/map_backups, holding the PRE-edit bytes.
    backup = Path(out["backupPath"])
    assert backup.is_file() and backup.read_bytes() == b"ORIGINAL-MAP-BYTES"
    assert str(tmp_path / "data") in str(backup)
    # Post-edit verification re-digested the map (fixture: 3 locations).
    assert len(out["locations"]) == 3


def test_location_write_korean_name_follows_map_string_encoding(tmp_path):
    # Fixture STR holds a cp949 string -> a Korean name must encode cp949.
    svc, _, recorded = make_locedit_service(tmp_path)
    svc.location_write("add", name="한글존", left=0, top=0, right=4, bottom=4)
    assert recorded["ops"][-1].endswith("한글존".encode("cp949") + b"\n")


def test_location_write_set_rename_delete_op_lines(tmp_path):
    svc, _, recorded = make_locedit_service(tmp_path)
    svc.location_write("set", location_id=2, left=1, top=1, right=3, bottom=3)
    svc.location_write("rename", location_id=2, name="NewName")
    svc.location_write("delete", location_id=5)
    assert recorded["ops"] == [
        b"set|2|32|32|96|96\n",
        b"rename|2|NewName\n",
        b"del|5\n",
    ]


def test_location_write_inverted_swaps_bounds_after_validation(tmp_path):
    """invertX/invertY produce 음수 로케이션 bounds (edac/76715: the same bytes
    SCMDraft's Invert buttons store) — the INPUT rect stays a normal rectangle
    so the sanity validation still applies."""
    svc, _, recorded = make_locedit_service(tmp_path)
    svc.location_write("add", name="Hit", left=10, top=10, right=12, bottom=11,
                       invert_x=True, invert_y=True)
    svc.location_write("set", location_id=3, left=1, top=1, right=2, bottom=2,
                       invert_x=True)
    assert recorded["ops"] == [
        b"add|384|352|320|320|Hit\n",   # l/r and t/b swapped
        b"set|3|64|32|32|64\n",         # only x swapped
    ]
    # An inverted rect must still be SUPPLIED as a normal rect.
    with pytest.raises(MapInfoError, match="tileRight > tileLeft"):
        svc.location_write("add", name="X", left=12, top=10, right=10,
                           bottom=11, invert_x=True)


def test_parse_locations_flags_inverted_axes():
    mrgn = (
        mrgn_entry(96, 96, 32, 32, 1)    # both axes inverted
        + mrgn_entry(96, 32, 32, 96, 0)  # x only
    )
    strings = str_section([b"Hit"])
    digest = digest_chk(
        sec("DIM ", struct.pack("<HH", 64, 64))
        + sec("MRGN", mrgn) + sec("STR ", strings)
    )
    locs = digest["locations"]
    assert locs[0]["inverted"] == "xy" and locs[0]["name"] == "Hit"
    assert locs[1]["inverted"] == "x"
    # Normal locations (the other fixtures) carry NO inverted key.
    assert "inverted" not in digest_chk(build_fixture_chk())["locations"][0]


def test_location_write_tool_passes_invert_flags():
    svc = FakeLocService()
    layer = ToolLayer(object(), map_info=svc)
    state = RequestState(request_id="r1")
    layer.call("location_write", {
        "action": "add", "name": "Hit",
        "tileLeft": 0, "tileTop": 0, "tileRight": 1, "tileBottom": 1,
        "invertX": True, "invertY": True,
    }, state)
    _, kwargs = svc.calls[0]
    assert kwargs["invert_x"] is True and kwargs["invert_y"] is True
    # Defaults stay False; flags are ignored for id-only actions.
    layer.call("location_write", {"action": "delete", "locationId": 2,
                                  "invertX": True}, state)
    _, kwargs = svc.calls[1]
    assert kwargs["invert_x"] is False and kwargs["invert_y"] is False


def test_system_prompt_carries_map_location_guide(tmp_path):
    from eud_agent.engine import build_system_prompt

    sp = build_system_prompt(
        "anything",
        tool_layer=ToolLayer(object()),
        bridge=object(),  # state/RAG degrade best-effort
        rag_db=str(tmp_path / "no-rag"),
    )
    assert "[map locations]" in sp
    assert "invertX" in sp and "map_info(mode=locations)" in sp
    # Never-do rules outrank workflow guidance.
    assert sp.index("[first principles]") < sp.index("[map locations]")


def test_location_write_refuses_while_compiling(tmp_path):
    svc, *_ = make_locedit_service(tmp_path, compiling="True")
    with pytest.raises(MapInfoError, match="building right now"):
        svc.location_write("delete", location_id=1)


def test_location_write_refuses_locked_map(tmp_path):
    svc, *_ = make_locedit_service(tmp_path, lock_probe=lambda p: True)
    with pytest.raises(MapInfoError, match="open in another program"):
        svc.location_write("delete", location_id=1)


def test_location_write_arg_validation(tmp_path):
    svc, _, recorded = make_locedit_service(tmp_path)
    with pytest.raises(MapInfoError, match="action must be one of"):
        svc.location_write("explode")
    with pytest.raises(MapInfoError, match="tileRight > tileLeft"):
        svc.location_write("add", name="X", left=5, top=0, right=5, bottom=4)
    with pytest.raises(MapInfoError, match="must not contain"):
        svc.location_write("add", name="a|b", left=0, top=0, right=1, bottom=1)
    with pytest.raises(MapInfoError, match="name must be non-empty"):
        svc.location_write("rename", location_id=1, name="  ")
    with pytest.raises(MapInfoError, match="locationId must be >= 1"):
        svc.location_write("delete", location_id=0)
    assert recorded["ops"] == []  # nothing reached the CLI


def test_location_write_cli_failure_surfaces_stderr(tmp_path):
    svc, map_path, _ = make_locedit_service(
        tmp_path, returncode=1, stderr="location #64 (Anywhere) is protected"
    )
    with pytest.raises(MapInfoError, match="Anywhere"):
        svc.location_write("delete", location_id=64)
    # The map is untouched (locedit aborts pre-save; our fake wrote nothing).
    assert map_path.read_bytes() == b"ORIGINAL-MAP-BYTES"


def test_detect_str_encoding_variants():
    assert detect_str_encoding(build_fixture_chk()) == "cp949"  # Korean STR
    ascii_only = sec("STR ", str_section([b"abc"]))
    assert detect_str_encoding(ascii_only) == "cp949"  # ambiguous -> cp949
    strx = sec("STRx", str_section([b"abc"], extended=True))
    assert detect_str_encoding(strx) == "utf-8"  # SC:R table


def test_restore_map_backup_roundtrip(tmp_path):
    target = tmp_path / "m.scx"
    target.write_bytes(b"EDITED")
    backup = tmp_path / "m.bak"
    backup.write_bytes(b"ORIGINAL")
    restore_map_backup(str(target), str(backup))
    assert target.read_bytes() == b"ORIGINAL"
    with pytest.raises(MapInfoError, match="backup not found"):
        restore_map_backup(str(target), str(tmp_path / "gone.bak"))


# --------------------------------------------------------------------------- #
# location_write tool routing + journal/changeset (features/09).
# --------------------------------------------------------------------------- #


class FakeLocService:
    def __init__(self, *, error: MapInfoError | None = None):
        self.calls: list[tuple] = []
        self.error = error

    def location_write(self, action, **kwargs):
        self.calls.append((action, kwargs))
        if self.error:
            raise self.error
        return {
            "ok": True, "action": action, "locationId": 7,
            "mapPath": "C:/maps/demo.scx", "backupPath": "C:/bk/demo.bak",
            "locations": [],
        }


def test_location_write_is_a_registered_write_tool():
    assert "location_write" in WRITE_TOOLS
    layer = ToolLayer(object())
    spec = next(s for s in layer.tool_specs() if s["name"] == "location_write")
    assert spec["parameters"]["required"] == ["action"]
    assert set(spec["parameters"]["properties"]["action"]["enum"]) == {
        "add", "set", "rename", "delete",
    }


def test_location_write_routes_and_counts_action_and_mutation():
    svc = FakeLocService()
    layer = ToolLayer(object(), map_info=svc)
    state = RequestState(request_id="r1")
    out = layer.call("location_write", {
        "action": "add", "name": "Zone",
        "tileLeft": 1, "tileTop": 2, "tileRight": 3, "tileBottom": 4,
    }, state)
    assert out["locationId"] == 7
    assert svc.calls == [("add", {
        "name": "Zone", "location_id": 0,
        "left": 1, "top": 2, "right": 3, "bottom": 4,
        "invert_x": False, "invert_y": False,
    })]
    assert state.action_count == 1 and state.mutation_count == 1


def test_location_write_invalid_args_rejected_before_the_service():
    svc = FakeLocService()
    layer = ToolLayer(object(), map_info=svc)
    state = RequestState(request_id="r1")
    with pytest.raises(ToolError, match="action must be one of"):
        layer.call("location_write", {"action": "nuke"}, state)
    with pytest.raises(ToolError, match="'name' is required"):
        layer.call("location_write", {"action": "add", "tileLeft": 0,
                                      "tileTop": 0, "tileRight": 1,
                                      "tileBottom": 1}, state)
    with pytest.raises(ToolError, match="locationId"):
        layer.call("location_write", {"action": "delete"}, state)
    assert svc.calls == [] and state.action_count == 0


def test_location_write_obeys_the_mutation_gate():
    svc = FakeLocService()
    layer = ToolLayer(object(), map_info=svc)
    state = RequestState(request_id="r1")
    for _ in range(2):
        layer.call("location_write", {"action": "delete", "locationId": 1},
                   state)
    with pytest.raises(PlanRequired):
        layer.call("location_write", {"action": "delete", "locationId": 1},
                   state)
    state.approve_plan()
    layer.call("location_write", {"action": "delete", "locationId": 1}, state)
    assert len(svc.calls) == 3


def test_location_write_without_service_or_on_error_is_a_tool_error():
    state = RequestState(request_id="r1")
    with pytest.raises(ToolError, match="location_write unavailable"):
        ToolLayer(object()).call(
            "location_write", {"action": "delete", "locationId": 1}, state
        )
    svc = FakeLocService(error=MapInfoError("map file is open in another program"))
    layer = ToolLayer(object(), map_info=svc)
    with pytest.raises(ToolError, match="another program"):
        layer.call("location_write", {"action": "delete", "locationId": 1},
                   state)
    assert state.action_count == 0


def test_location_write_journal_entry_and_rollback_restores_backup(tmp_path):
    # Real files: the journal entry's before carries the backup pointer and a
    # rollback restores the pre-edit bytes over the edited map.
    map_file = tmp_path / "demo.scx"
    map_file.write_bytes(b"EDITED-MAP")
    backup = tmp_path / "demo.scx.bak"
    backup.write_bytes(b"ORIGINAL-MAP")

    class _Svc(FakeLocService):
        def location_write(self, action, **kwargs):
            self.calls.append((action, kwargs))
            return {
                "ok": True, "action": action, "locationId": 1,
                "mapPath": str(map_file), "backupPath": str(backup),
                "locations": [],
            }

    journal = Journal(data_dir=str(tmp_path / "data"), request_id="rq",
                      bridge=object())
    layer = ToolLayer(object(), map_info=_Svc())
    state = RequestState(request_id="rq")
    layer.call("location_write", {
        "action": "add", "name": "Zone",
        "tileLeft": 0, "tileTop": 0, "tileRight": 1, "tileBottom": 1,
    }, state, journal=journal)

    assert len(journal.entries) == 1
    entry = journal.entries[0]
    assert entry.before["backupPath"] == str(backup)
    item = journal.changeset()["items"][0]
    assert item["category"] == "map" and "location:add" in item["target"]

    result = journal.rollback(all=True)
    assert result["items"][0]["ok"] is True
    assert map_file.read_bytes() == b"ORIGINAL-MAP"


# --------------------------------------------------------------------------- #
# Tool-layer routing (features/08).
# --------------------------------------------------------------------------- #


class FakeMapInfoService:
    def __init__(self, *, error: MapInfoError | None = None):
        self.calls: list[dict] = []
        self.error = error

    def map_info(self, mode="summary", owner="", unit_type=""):
        self.calls.append({"mode": mode, "owner": owner, "unit_type": unit_type})
        if self.error:
            raise self.error
        return {"map": {"tileset": "jungle"}, "mode": mode}


def test_map_info_is_a_registered_read_tool():
    assert "map_info" in READ_TOOLS
    layer = ToolLayer(object())
    spec = next(s for s in layer.tool_specs() if s["name"] == "map_info")
    assert "locations" in spec["description"]
    assert spec["parameters"]["properties"]["mode"]["enum"] == [
        "summary", "locations", "units", "players",
    ]


def test_map_info_without_service_is_a_tool_error():
    layer = ToolLayer(object())  # no map_info injected
    state = RequestState(request_id="r1")
    with pytest.raises(ToolError, match="map_info unavailable"):
        layer.call("map_info", {}, state)
    assert state.action_count == 0


def test_map_info_routes_to_service_and_counts_one_read_action():
    svc = FakeMapInfoService()
    layer = ToolLayer(object(), map_info=svc)
    state = RequestState(request_id="r1")
    out = layer.call(
        "map_info", {"mode": "units", "owner": "P1", "unitType": "marine"}, state
    )
    assert out["mode"] == "units"
    assert svc.calls == [{"mode": "units", "owner": "P1", "unit_type": "marine"}]
    assert state.action_count == 1
    assert state.mutation_count == 0  # READ: never advances the plan gate


def test_map_info_invalid_mode_rejected_before_the_service_runs():
    svc = FakeMapInfoService()
    layer = ToolLayer(object(), map_info=svc)
    state = RequestState(request_id="r1")
    with pytest.raises(ToolError, match="mode must be one of"):
        layer.call("map_info", {"mode": "everything"}, state)
    assert svc.calls == []          # nothing ran
    assert state.action_count == 0  # nothing counted


def test_map_info_service_error_is_a_tool_error_not_a_crash():
    svc = FakeMapInfoService(error=MapInfoError("no map is connected"))
    layer = ToolLayer(object(), map_info=svc)
    state = RequestState(request_id="r1")
    with pytest.raises(ToolError, match="no map is connected"):
        layer.call("map_info", {}, state)
