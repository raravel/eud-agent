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

import pytest

from eud_agent.chk_info import (
    UNIT_ENTRY_SIZE,
    UNITS_LIST_CAP,
    MapInfoError,
    MapInfoService,
    assemble_sections,
    decode_text,
    digest_chk,
    owner_label,
    parse_strings,
    slice_digest,
    unit_name,
    walk_sections,
)
from eud_agent.tools import (
    READ_TOOLS,
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
    """GETSET-only bridge fake returning the configured OpenMapName."""

    def __init__(self, open_map: str):
        self.open_map = open_map
        self.calls: list[tuple] = []

    def getset(self, scope, key, **kw):
        self.calls.append(("getset", scope, key))
        return f"OK: {scope}|{key} = {self.open_map}"


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
