"""map_info: read the connected map's SCMD2-set data (locations, units, forces).

The editor holds NO parsed map data the bridge can reach — ``pjData`` carries
only the ``OpenMapName``/``SaveMapName`` path strings (features/04 settings
surface). So the map-info tool reads the SOURCE map file (``OpenMapName``, the
map the user authored in SCMDraft 2) from disk:

  1. resolve the map path via bridge ``GETSET project|OpenMapName``;
  2. spawn ``IsomTerrain.exe chk <map.scx> <tmp.chk>`` to extract the raw CHK
     (the MPQ/protection handling lives in the verified isom-poc CLI — this
     server never parses MPQ itself; C++ stays untouched per the isom-poc
     contract "the grid/CLI is the only interface");
  3. parse the CHK sections HERE in Python (heavy work lives in Python —
     architecture.md dependency direction) and return a JSON-serializable
     summary the LLM can use.

CHK grounding (staredit.network CHK format; cross-checked against isom-poc's
MappingCoreLib ``Chk.h`` structs):

  * The file is a TLV walk: 4-byte section name, u32 (SIGNED) size, data.
    Duplicate scalar sections (DIM/ERA/OWNR/SIDE/FORC/MRGN/STR...) overwrite —
    LAST wins; duplicate ``UNIT`` sections STACK (entries append). A protected
    map may carry a NEGATIVE size (the jump trick): we follow the signed seek
    like StarCraft does, with an iteration cap + bounds guards so a crafted
    file cannot loop or read out of range.
  * ``UNIT`` is 36 bytes/entry (Chk.h ``struct Unit``); start locations are
    unit type 214. ``MRGN`` is 20 bytes/entry (u32 l/t/r/b px, u16 stringId,
    u16 elevationFlags); entry index 63 is the "Anywhere" location.
  * ``FORC`` may legally be SHORTER than 20 bytes — pad with zeros (Chk.h
    comment; SCMDraft writes short FORC on default settings).
  * Strings: ``STR `` (u16 count + u16 offsets) or ``STRx`` (u32 variants);
    ids are 1-based, 0 = no string. Korean maps usually store cp949 bytes —
    decode utf-8 first (hangul cp949 bytes are almost never valid utf-8), then
    cp949, then latin-1/replace so decode is total.

The subprocess obeys the rules.md codex-invocation rules (they govern EVERY
subprocess): an absolute exe path resolved from config (NEVER a bare name; a
missing exe is a clear error, not a crash), an EXPLICIT stdin
(``subprocess.DEVNULL``), captured output, ``cwd`` set, and a wall-clock
timeout. The tool is ADVISORY-shaped like epscript-lsp: when IsomTerrain.exe is
absent the tool returns a clear error and NOTHING else in the flow breaks.

Data freshness: the result is the LAST-SAVED file on disk; unsaved SCMDraft
edits are invisible. The map file's mtime is included so the LLM/user can see
how stale the snapshot is.
"""

from __future__ import annotations

import ctypes
import json
import os
import re
import struct
import subprocess
import sys
import tempfile
from collections.abc import Callable
from datetime import datetime
from pathlib import Path

# Wall-clock cap on the IsomTerrain.exe spawn (rules.md: never wait unbounded).
# CHK extraction is I/O-light; 60s is generous even for a huge protected map.
DEFAULT_SUBPROCESS_TIMEOUT = 60.0

# TLV-walk hard caps: a protected map's negative-size jump trick may revisit
# offsets; StarCraft itself just keeps seeking. The cap bounds a crafted file.
_MAX_SECTIONS = 4096

# Cap on the units list a single ``mode="units"`` reply may carry (use-map UNIT
# sections run to thousands; an uncapped list would blow the codex context).
UNITS_LIST_CAP = 200

# Pixels per tile: the grid format / LLM-facing tool speak tiles; CHK stores px.
TILE_PX = 32

# location_write actions (tool enum value -> locedit op verb).
LOCATION_ACTIONS = {"add": "add", "set": "set", "rename": "rename", "delete": "del"}

_TILESET_NAMES = (
    "badlands", "platform", "installation", "ashworld",
    "jungle", "desert", "ice", "twilight",
)

# OWNR slot controller values (staredit.network; Chk.h enum Type).
_OWNR_NAMES = {
    0: "Inactive",
    1: "Computer (game)",
    2: "Occupied by Human",
    3: "Rescue Passive",
    4: "Unused",
    5: "Computer",
    6: "Human (Open Slot)",
    7: "Neutral",
    8: "Closed",
}

# OWNR values that mean the slot actually participates (computer/human/rescue).
# Inactive/Unused/Closed/Neutral slots default to force byte 0 in SCMDraft, so
# listing them under force 1 would misreport the teams.
_ACTIVE_CONTROLLERS = frozenset({1, 2, 3, 5, 6})

# SIDE races (Chk.h enum Race).
_RACE_NAMES = {
    0: "Zerg",
    1: "Terran",
    2: "Protoss",
    3: "Independent",
    4: "Neutral",
    5: "User Selectable",
    6: "Random",
    7: "Inactive",
}

# FORC per-force flag bits (Chk.h ForceFlags).
_FORCE_FLAG_BITS = (
    (0x1, "randomStartLocation"),
    (0x2, "allies"),
    (0x4, "alliedVictory"),
    (0x8, "sharedVision"),
)

# UNIT entry: Chk.h struct Unit, 36 bytes (see module docstring).
_UNIT_STRUCT = struct.Struct("<IHHHHHHBBBBIHHII")
UNIT_ENTRY_SIZE = _UNIT_STRUCT.size  # 36
# MRGN entry: u32 left/top/right/bottom (px), u16 stringId, u16 elevationFlags.
_MRGN_STRUCT = struct.Struct("<IIIIHH")
MRGN_ENTRY_SIZE = _MRGN_STRUCT.size  # 20
_START_LOCATION_TYPE = 214
_ANYWHERE_INDEX = 63

# Vendored Sc::Unit::defaultDisplayNames 0-227 (isom-poc MappingCoreLib Sc.cpp;
# the grid `unit` directive uses the same canonical names, so the two tools
# agree on spelling). Loaded lazily; ids past the table render as "ID:<n>".
_UNIT_NAMES_PATH = Path(__file__).parent / "data" / "unit_names.json"
_unit_names: list[str] | None = None


def unit_name(unit_id: int) -> str:
    """Canonical display name for a unit type id (``ID:<n>`` when unknown)."""
    global _unit_names
    if _unit_names is None:
        _unit_names = json.loads(_UNIT_NAMES_PATH.read_text(encoding="utf-8"))
    if 0 <= unit_id < len(_unit_names):
        return _unit_names[unit_id]
    return f"ID:{unit_id}"


class MapInfoError(RuntimeError):
    """map_info could not produce a result (unconfigured exe, bad map, ...).

    The tool layer re-raises this as a ToolError so codex sees a correctable
    tool result; nothing else in the flow is affected (advisory shape).
    """


# --------------------------------------------------------------------------- #
# CHK section walk + assembly.
# --------------------------------------------------------------------------- #


def walk_sections(data: bytes) -> list[tuple[str, bytes]]:
    """Walk the CHK TLV stream, following StarCraft's SIGNED-size seek.

    Returns ``(name, payload)`` in file order. The payload is clamped to EOF
    (a size past the end yields the remaining bytes, like SC's short read). A
    NEGATIVE size moves the cursor backwards (protection trick) with an empty
    payload for that header; the iteration cap + bounds guards stop a crafted
    loop. Trailing bytes too short for a header are ignored.
    """
    out: list[tuple[str, bytes]] = []
    pos = 0
    n = len(data)
    for _ in range(_MAX_SECTIONS):
        if pos < 0 or pos + 8 > n:
            break
        name = data[pos:pos + 4].decode("latin-1")
        (size,) = struct.unpack_from("<i", data, pos + 4)
        body_start = pos + 8
        if size >= 0:
            body = data[body_start:body_start + size]
            out.append((name, body))
        else:
            out.append((name, b""))
        pos = body_start + size
    return out


def assemble_sections(sections: list[tuple[str, bytes]]) -> dict[str, bytes]:
    """Resolve duplicates: ``UNIT`` payloads STACK; every other name last-wins."""
    resolved: dict[str, bytes] = {}
    for name, body in sections:
        if name == "UNIT":
            resolved[name] = resolved.get(name, b"") + body
        else:
            resolved[name] = body
    return resolved


# --------------------------------------------------------------------------- #
# Section decoders. Each is total over malformed input (truncated entries are
# dropped; missing sections decode to empty/None) — a protected map must yield
# a partial result, never an exception past MapInfoError.
# --------------------------------------------------------------------------- #


def decode_text(raw: bytes) -> str:
    """utf-8 -> cp949 -> latin-1/replace (total; Korean maps are usually cp949)."""
    for enc in ("utf-8", "cp949"):
        try:
            return raw.decode(enc)
        except UnicodeDecodeError:
            continue
    return raw.decode("latin-1", errors="replace")


def parse_strings(body: bytes, *, extended: bool) -> list[str]:
    """Decode ``STR ``/``STRx`` into a 0-indexed list (string id 1 = index 0).

    Offsets are relative to the section start; an out-of-range offset yields
    ``""`` for that id (drop nothing — ids must stay aligned).
    """
    if not body:
        return []
    width = 4 if extended else 2
    fmt = "<I" if extended else "<H"
    if len(body) < width:
        return []
    (count,) = struct.unpack_from(fmt, body, 0)
    # Clamp a lying count to what the offset table can actually hold.
    count = min(count, max(0, (len(body) - width) // width))
    strings: list[str] = []
    for i in range(count):
        (off,) = struct.unpack_from(fmt, body, width + i * width)
        if 0 < off < len(body):
            end = body.find(b"\x00", off)
            raw = body[off:] if end < 0 else body[off:end]
            strings.append(decode_text(raw))
        else:
            strings.append("")
    return strings


def _string_at(strings: list[str], string_id: int) -> str:
    """1-based CHK string lookup (id 0 / out of range -> "")."""
    if 1 <= string_id <= len(strings):
        return strings[string_id - 1]
    return ""


def parse_locations(body: bytes, strings: list[str]) -> list[dict]:
    """MRGN -> location dicts; all-zero (unused) entries are skipped.

    ``id`` is the 1-based location number triggers use; entry index 63 is the
    engine "Anywhere" location (kept, flagged) even when unnamed.
    """
    locations: list[dict] = []
    count = len(body) // MRGN_ENTRY_SIZE
    for i in range(count):
        left, top, right, bottom, string_id, elevation = _MRGN_STRUCT.unpack_from(
            body, i * MRGN_ENTRY_SIZE
        )
        is_anywhere = i == _ANYWHERE_INDEX
        if not any((left, top, right, bottom, string_id)) and not is_anywhere:
            continue
        loc = {
            "id": i + 1,
            "name": _string_at(strings, string_id),
            "left": left, "top": top, "right": right, "bottom": bottom,
            "tileRect": [left // 32, top // 32, right // 32, bottom // 32],
            "elevationFlags": elevation,
        }
        # 음수(Inverted) 로케이션: swapped bounds change Bring semantics (the
        # location must sit fully INSIDE the unit's collision box) — flag the
        # axis so the LLM recognizes precision-detection locations.
        inverted = ("x" if left > right else "") + ("y" if top > bottom else "")
        if inverted:
            loc["inverted"] = inverted
        if is_anywhere:
            loc["anywhere"] = True
        locations.append(loc)
    return locations


def parse_units(body: bytes) -> list[dict]:
    """UNIT -> unit dicts (truncated trailing bytes are dropped)."""
    units: list[dict] = []
    count = len(body) // UNIT_ENTRY_SIZE
    for i in range(count):
        (
            _serial, x, y, unit_id, _relation, _special, _valid,
            owner, hp, shields, energy, resources, _hangar, _state,
            _unused, _related,
        ) = _UNIT_STRUCT.unpack_from(body, i * UNIT_ENTRY_SIZE)
        units.append({
            "type": unit_name(unit_id),
            "typeId": unit_id,
            "owner": owner_label(owner),
            "x": x, "y": y,
            "tileX": x // 32, "tileY": y // 32,
            "hpPercent": hp, "shieldPercent": shields,
            "energyPercent": energy, "resources": resources,
        })
    return units


def owner_label(owner: int) -> str:
    """UNIT owner byte -> the player label the grid format / triggers use."""
    if owner == 11:
        return "P12 (neutral)"
    return f"P{owner + 1}"


def parse_players(resolved: dict[str, bytes], strings: list[str]) -> dict:
    """OWNR + SIDE + FORC -> per-slot players + per-force teams."""
    ownr = resolved.get("OWNR", b"")
    side = resolved.get("SIDE", b"")
    forc = resolved.get("FORC", b"").ljust(20, b"\x00")[:20]
    force_of_slot = forc[0:8]
    force_strings = struct.unpack("<4H", forc[8:16])
    force_flags = forc[16:20]

    players: list[dict] = []
    for slot in range(12):
        controller = ownr[slot] if slot < len(ownr) else 0
        race = side[slot] if slot < len(side) else 7
        entry = {
            "player": f"P{slot + 1}",
            "controller": _OWNR_NAMES.get(controller, f"controller:{controller}"),
            "race": _RACE_NAMES.get(race, f"race:{race}"),
        }
        if slot < 8:
            # FORC values are 0-based force indexes (Chk.h playerForce).
            entry["force"] = force_of_slot[slot] + 1
        players.append(entry)

    forces: list[dict] = []
    for f in range(4):
        flags = force_flags[f]
        forces.append({
            "force": f + 1,
            "name": _string_at(strings, force_strings[f]) or f"Force {f + 1}",
            "players": [
                f"P{slot + 1}"
                for slot in range(8)
                if force_of_slot[slot] == f
                and slot < len(ownr)
                and ownr[slot] in _ACTIVE_CONTROLLERS
            ],
            "flags": {
                label: bool(flags & bit) for bit, label in _FORCE_FLAG_BITS
            },
        })
    return {"players": players, "forces": forces}


def parse_map_header(resolved: dict[str, bytes]) -> dict:
    """DIM + ERA -> map dimensions (tiles) and tileset name."""
    dim = resolved.get("DIM ", b"")
    era = resolved.get("ERA ", b"")
    width = height = 0
    if len(dim) >= 4:
        width, height = struct.unpack_from("<HH", dim, 0)
    tileset = ""
    if len(era) >= 2:
        (era_val,) = struct.unpack_from("<H", era, 0)
        tileset = _TILESET_NAMES[era_val & 0x7]
    return {"width": width, "height": height, "tileset": tileset}


# --------------------------------------------------------------------------- #
# The full-map digest the service modes slice from.
# --------------------------------------------------------------------------- #


def digest_chk(data: bytes) -> dict:
    """Parse raw CHK bytes into the full map digest (every mode reads this)."""
    resolved = assemble_sections(walk_sections(data))
    # STRx (u32 offsets, SC:R) supersedes STR when both are present.
    if "STRx" in resolved:
        strings = parse_strings(resolved["STRx"], extended=True)
    else:
        strings = parse_strings(resolved.get("STR ", b""), extended=False)

    units = parse_units(resolved.get("UNIT", b""))
    return {
        "map": parse_map_header(resolved),
        **parse_players(resolved, strings),
        "locations": parse_locations(resolved.get("MRGN", b""), strings),
        "units": units,
        "startLocations": [
            {
                "player": u["owner"],
                "x": u["x"], "y": u["y"],
                "tileX": u["tileX"], "tileY": u["tileY"],
            }
            for u in units if u["typeId"] == _START_LOCATION_TYPE
        ],
    }


# --------------------------------------------------------------------------- #
# Mode slicing + unit filters (the tool's reply must stay context-sized).
# --------------------------------------------------------------------------- #


def _match_owner(unit: dict, owner: str) -> bool:
    want = owner.strip().lower()
    if not want:
        return True
    if want == "neutral":
        return unit["owner"].startswith("P12")
    return unit["owner"].split(" ")[0].lower() == want


def _match_type(unit: dict, unit_type: str) -> bool:
    want = unit_type.strip()
    if not want:
        return True
    if want.isdigit():
        return unit["typeId"] == int(want)
    return want.lower() in unit["type"].lower()


def _unit_counts(units: list[dict]) -> dict:
    """Aggregate ``{owner: {type: count}}`` (the summary's compact view)."""
    by_owner: dict[str, dict[str, int]] = {}
    for u in units:
        per = by_owner.setdefault(u["owner"], {})
        per[u["type"]] = per.get(u["type"], 0) + 1
    return by_owner


def slice_digest(
    digest: dict, mode: str, *, owner: str = "", unit_type: str = ""
) -> dict:
    """Cut the full digest down to one mode's reply (summary stays aggregate)."""
    if mode == "players":
        return {
            "map": digest["map"],
            "players": digest["players"],
            "forces": digest["forces"],
            "startLocations": digest["startLocations"],
        }
    if mode == "locations":
        return {
            "map": digest["map"],
            "locationCount": len(digest["locations"]),
            "locations": digest["locations"],
        }
    if mode == "units":
        units = [
            u for u in digest["units"]
            if _match_owner(u, owner) and _match_type(u, unit_type)
        ]
        reply = {
            "map": digest["map"],
            "matched": len(units),
            "units": units[:UNITS_LIST_CAP],
        }
        if len(units) > UNITS_LIST_CAP:
            reply["truncated"] = True
            reply["hint"] = (
                f"{len(units)} units matched; showing {UNITS_LIST_CAP}. "
                "Narrow with the owner/unitType filters."
            )
        return reply
    # summary (default)
    return {
        "map": digest["map"],
        "players": [
            p for p in digest["players"] if p["controller"] != "Inactive"
        ],
        "forces": digest["forces"],
        "startLocations": digest["startLocations"],
        "locationCount": len(digest["locations"]),
        "locationNames": [
            loc["name"] for loc in digest["locations"] if loc["name"]
        ],
        "unitCount": len(digest["units"]),
        "unitsByOwner": _unit_counts(digest["units"]),
    }


# --------------------------------------------------------------------------- #
# Write-path helpers: lock probe, encoding detection, backup restore.
# --------------------------------------------------------------------------- #


def windows_file_locked(path: str | Path) -> bool:
    """True when another process holds ``path`` open (Windows share probe).

    Opens the file with dwShareMode=0 (no sharing): SCMDraft or any editor
    holding the map open makes CreateFileW fail with ERROR_SHARING_VIOLATION
    (32). The probe handle is closed immediately. On non-Windows (CI) the probe
    reports unlocked — the locedit spawn itself still fails safely if needed.
    """
    if sys.platform != "win32":
        return False
    GENERIC_READ = 0x80000000
    OPEN_EXISTING = 3
    INVALID_HANDLE = ctypes.c_void_p(-1).value
    handle = ctypes.windll.kernel32.CreateFileW(
        str(path), GENERIC_READ, 0, None, OPEN_EXISTING, 0, None
    )
    if handle == INVALID_HANDLE:
        return ctypes.GetLastError() == 32  # ERROR_SHARING_VIOLATION
    ctypes.windll.kernel32.CloseHandle(handle)
    return False


def detect_str_encoding(chk_data: bytes) -> str:
    """Pick the byte encoding for a NEW location name from the map's own strings.

    ``STRx`` maps are SC:R-era -> utf-8. Otherwise: any existing string whose
    bytes are not valid utf-8 marks a cp949 (Korean SCMDraft) string table.
    All-ASCII/empty tables are ambiguous -> cp949, the safe default for this
    machine's SCMDraft locale (ASCII bytes are identical in both encodings).
    """
    resolved = assemble_sections(walk_sections(chk_data))
    if "STRx" in resolved:
        return "utf-8"
    body = resolved.get("STR ", b"")
    if len(body) >= 2:
        (count,) = struct.unpack_from("<H", body, 0)
        count = min(count, max(0, (len(body) - 2) // 2))
        for i in range(count):
            (off,) = struct.unpack_from("<H", body, 2 + i * 2)
            if 0 < off < len(body):
                end = body.find(b"\x00", off)
                raw = body[off:] if end < 0 else body[off:end]
                try:
                    raw.decode("utf-8")
                except UnicodeDecodeError:
                    return "cp949"
    return "cp949"


def restore_map_backup(map_path: str, backup_path: str) -> None:
    """Roll a ``location_write`` back: copy the backed-up map bytes over the map.

    Used by the journal's rollback (changeset reject). Refuses while the map is
    locked (the same share probe as the write path) and writes via temp +
    ``os.replace`` so a crash cannot leave a half-written map.
    """
    backup = Path(backup_path)
    if not backup.is_file():
        raise MapInfoError(f"map backup not found: {backup}")
    target = Path(map_path)
    if windows_file_locked(target):
        raise MapInfoError(
            f"cannot restore map: {target} is open in another program "
            "(close SCMDraft and retry)."
        )
    tmp = target.with_suffix(target.suffix + ".restoretmp")
    tmp.write_bytes(backup.read_bytes())
    os.replace(tmp, target)


# --------------------------------------------------------------------------- #
# The service: bridge path lookup -> IsomTerrain chk extraction -> digest.
# --------------------------------------------------------------------------- #


def _parse_setting_value(reply: str) -> str:
    """Extract the value from a bridge ``GETSET`` reply (``OK: ... = <value>``).

    Only the FIRST ``" = "`` separates the id prefix from the value. Mirrors
    ``edd_runner.parse_setting_value`` — duplicated tiny to avoid coupling this
    module to the runner (same decision as journal/edd_runner).
    """
    _, sep, value = reply.partition(" = ")
    return (value if sep else reply).strip()


class MapInfoService:
    """Reads (and edits, features/09) the connected map via the IsomTerrain CLI.

    ``bridge`` is the shared :class:`bridge_io.BridgeIO` (``GETSET
    project|OpenMapName`` for the path; ``STATUS`` for the write path's
    compiling guard). ``isomterrain_cmd`` is the ABSOLUTE exe
    path from config (env ``ISOMTERRAIN_CMD`` > agent.cfg ``isomterrain_cmd``
    > built-in default); empty/missing makes every call a clear
    :class:`MapInfoError` — advisory shape, nothing else breaks. ``spawn`` and
    ``subprocess_timeout`` are injectable so tests run without the real exe.
    """

    def __init__(
        self,
        bridge,
        *,
        isomterrain_cmd: str,
        spawn: Callable | None = None,
        subprocess_timeout: float = DEFAULT_SUBPROCESS_TIMEOUT,
        data_dir: str | None = None,
        lock_probe: Callable[[str], bool] | None = None,
    ) -> None:
        self._bridge = bridge
        self._isomterrain_cmd = isomterrain_cmd
        self._spawn = spawn or subprocess.run
        self._subprocess_timeout = subprocess_timeout
        # Write-path collaborators (location_write, features/09): backups land
        # under ``<data_dir>/map_backups`` (or next to the map when no data_dir
        # is wired — tests/standalone); ``lock_probe`` is injectable so tests
        # exercise the locked-map refusal without a real second process.
        self._data_dir = data_dir
        self._lock_probe = lock_probe or windows_file_locked

    # ------------------------------------------------------------------ api
    def map_info(
        self, mode: str = "summary", owner: str = "", unit_type: str = ""
    ) -> dict:
        """One tool call: resolve path, extract CHK, digest, slice to ``mode``."""
        map_path = self._resolve_map_path()
        data = self._extract_chk(map_path)
        digest = digest_chk(data)
        reply = slice_digest(digest, mode, owner=owner, unit_type=unit_type)
        reply["map"] = {
            "path": str(map_path),
            # Disk snapshot staleness signal: unsaved SCMDraft edits are
            # invisible; the LLM/user can compare this against "now".
            "savedAt": datetime.fromtimestamp(
                map_path.stat().st_mtime
            ).isoformat(timespec="seconds"),
            **reply["map"],
        }
        return reply

    def location_write(
        self,
        action: str,
        *,
        name: str = "",
        location_id: int = 0,
        left: int = 0,
        top: int = 0,
        right: int = 0,
        bottom: int = 0,
        invert_x: bool = False,
        invert_y: bool = False,
    ) -> dict:
        """Apply ONE location edit to the connected map IN PLACE (features/09).

        ``action`` ∈ ``add|set|rename|delete``; coordinates are TILE units
        (converted to px here — the locedit CLI is a dumb px applier).
        ``invert_x``/``invert_y`` (add/set) produce a 음수(Inverted) location —
        the rect is given as a NORMAL rectangle and the bounds are swapped per
        axis AFTER validation/px conversion, exactly what SCMDraft's Invert
        X/Y buttons store in MRGN (edac/76715). NOTE: ``set`` writes the
        bounds it is given — re-pass the invert flags when moving an inverted
        location, or it reverts to normal. Safety
        rails, in order: editor not compiling -> map file not locked (SCMDraft
        share probe) -> full-file backup -> ``IsomTerrain.exe locedit`` (which
        itself aborts BEFORE saving on any bad op, never renumbers ids, and
        protects #64 Anywhere). Returns the applied op (with the assigned id
        for ``add``), the post-edit location list, and the backup path the
        journal stores for changeset rollback.
        """
        op = LOCATION_ACTIONS.get(action)
        if op is None:
            raise MapInfoError(
                f"location_write action must be one of "
                f"{', '.join(LOCATION_ACTIONS)}; got {action!r}"
            )
        map_path = self._resolve_map_path()
        if self._editor_compiling():
            raise MapInfoError(
                "the editor is building right now; retry after the build "
                "finishes (writing the map mid-build risks a corrupt read)."
            )
        if self._lock_probe(str(map_path)):
            raise MapInfoError(
                f"map file is open in another program: {map_path} "
                "(close it in SCMDraft and retry)."
            )

        line = self._ops_line(op, map_path, name, location_id,
                              left, top, right, bottom,
                              invert_x=invert_x, invert_y=invert_y)
        backup_path = self._backup_map(map_path)
        stdout = self._run_locedit(map_path, line)

        result: dict = {
            "ok": True,
            "action": action,
            "mapPath": str(map_path),
            "backupPath": str(backup_path),
        }
        m = re.search(r"OK \w+ #(\d+)", stdout)
        if m:
            result["locationId"] = int(m.group(1))
        # Post-edit verification: re-digest and return the live location list so
        # codex sees the applied state. The edit ALREADY saved — a verify failure
        # must not read as "not applied", so it degrades to a warning.
        try:
            digest = digest_chk(self._extract_chk(map_path))
            result["locations"] = digest["locations"]
        except MapInfoError as exc:
            result["verifyWarning"] = str(exc)
        return result

    # ------------------------------------------------------------- internals
    def _editor_compiling(self) -> bool:
        """Best-effort ``compiling`` flag from the bridge STATUS reply.

        Mirrors ``engine.parse_status``'s flag handling (lowercased value;
        VB.NET emits ``True``) — duplicated tiny to keep chk_info free of an
        engine import (tools.py imports chk_info; engine imports tools). A
        bridge without ``status`` (tests) reads as not-compiling.
        """
        try:
            reply = str(self._bridge.status())
        except AttributeError:
            return False
        m = re.search(r"compiling\s*=\s*['\"]?(\w+)", reply)
        return bool(m) and m.group(1).lower() in ("true", "1")

    def _ops_line(
        self, op: str, map_path: Path, name: str, location_id: int,
        left: int, top: int, right: int, bottom: int,
        *, invert_x: bool = False, invert_y: bool = False,
    ) -> bytes:
        """Validate + render one locedit ops line (px coords, raw name bytes).

        Inversion happens LAST: the caller supplies a normal rectangle (so the
        sanity check below stays meaningful) and the px bounds are swapped per
        inverted axis — the exact bytes SCMDraft's Invert buttons would store.
        """
        needs_name = op in ("add", "rename")
        needs_rect = op in ("add", "set")
        needs_id = op in ("set", "rename", "del")
        if needs_name:
            if not name.strip():
                raise MapInfoError(f"location {op}: name must be non-empty")
            if "|" in name or "\n" in name or "\r" in name:
                raise MapInfoError(
                    "location name must not contain '|' or line breaks"
                )
        if needs_rect and (right <= left or bottom <= top):
            raise MapInfoError(
                "location rect needs tileRight > tileLeft and "
                f"tileBottom > tileTop; got ({left},{top})-({right},{bottom})"
            )
        if needs_id and location_id < 1:
            raise MapInfoError(
                f"location {op}: locationId must be >= 1, got {location_id}"
            )

        name_bytes = b""
        if needs_name:
            # Match the map's OWN string-table encoding so SCMDraft renders the
            # name correctly (ASCII names are encoding-invariant -> skip the
            # extra chk extraction).
            try:
                name_bytes = name.encode("ascii")
            except UnicodeEncodeError:
                enc = detect_str_encoding(self._extract_chk(map_path))
                try:
                    name_bytes = name.encode(enc)
                except UnicodeEncodeError as exc:
                    raise MapInfoError(
                        f"location name {name!r} cannot be encoded as {enc} "
                        "(the map's string-table encoding)."
                    ) from exc

        px = [v * TILE_PX for v in (left, top, right, bottom)]
        if invert_x:
            px[0], px[2] = px[2], px[0]
        if invert_y:
            px[1], px[3] = px[3], px[1]
        if op == "add":
            fields = [b"add"] + [str(v).encode() for v in px] + [name_bytes]
        elif op == "set":
            fields = [b"set", str(location_id).encode()] + [
                str(v).encode() for v in px
            ]
        elif op == "rename":
            fields = [b"rename", str(location_id).encode(), name_bytes]
        else:  # del
            fields = [b"del", str(location_id).encode()]
        return b"|".join(fields) + b"\n"

    def _backup_map(self, map_path: Path) -> Path:
        """Full-file backup BEFORE the edit (the journal's rollback source).

        Lands under ``<data_dir>/map_backups`` (or next to the map without a
        data_dir), timestamped — every write keeps its own restore point.
        """
        backup_dir = (
            Path(self._data_dir) / "map_backups"
            if self._data_dir else map_path.parent
        )
        backup_dir.mkdir(parents=True, exist_ok=True)
        stamp = datetime.now().strftime("%Y%m%d-%H%M%S-%f")
        backup = backup_dir / f"{map_path.name}.{stamp}.bak"
        backup.write_bytes(map_path.read_bytes())
        return backup

    def _run_locedit(self, map_path: Path, ops_line: bytes) -> str:
        """Spawn ``IsomTerrain.exe locedit <map> <ops>`` (rules.md subprocess)."""
        exe = self._isomterrain_cmd
        if not exe or not Path(exe).is_file():
            raise MapInfoError(
                "location_write unavailable: IsomTerrain.exe not found "
                f"(configured: {exe or '<unset>'}). Set the ISOMTERRAIN_CMD "
                "env var or the agent.cfg 'isomterrain_cmd' key to the built "
                "isom-poc CLI."
            )
        with tempfile.TemporaryDirectory(prefix="eud_locedit_") as tmp:
            ops_path = Path(tmp) / "ops.txt"
            ops_path.write_bytes(ops_line)  # raw bytes; no BOM, no re-encode
            try:
                proc = self._spawn(
                    [exe, "locedit", str(map_path), str(ops_path)],
                    stdin=subprocess.DEVNULL,
                    capture_output=True,
                    text=True,
                    encoding="utf-8",
                    errors="replace",
                    cwd=str(Path(exe).parent),
                    timeout=self._subprocess_timeout,
                )
            except subprocess.TimeoutExpired as exc:
                raise MapInfoError(
                    f"IsomTerrain.exe locedit did not finish within "
                    f"{self._subprocess_timeout:.0f}s (process killed)."
                ) from exc
            stdout = (getattr(proc, "stdout", "") or "").strip()
            stderr = (getattr(proc, "stderr", "") or "").strip()
            if getattr(proc, "returncode", 1) != 0:
                detail = stderr or stdout or "no output"
                raise MapInfoError(f"location edit failed: {detail}")
            return stdout

    def _resolve_map_path(self) -> Path:
        reply = self._bridge.getset("project", "OpenMapName")
        map_path = _parse_setting_value(reply)
        if not map_path:
            raise MapInfoError(
                "no map is connected: project setting OpenMapName is empty "
                "(open a map project in the editor first)."
            )
        p = Path(map_path)
        if not p.is_file():
            raise MapInfoError(f"connected map file not found on disk: {p}")
        return p

    def _extract_chk(self, map_path: Path) -> bytes:
        exe = self._isomterrain_cmd
        if not exe or not Path(exe).is_file():
            raise MapInfoError(
                "map_info unavailable: IsomTerrain.exe not found "
                f"(configured: {exe or '<unset>'}). Set the ISOMTERRAIN_CMD "
                "env var or the agent.cfg 'isomterrain_cmd' key to the built "
                "isom-poc CLI."
            )
        with tempfile.TemporaryDirectory(prefix="eud_mapinfo_") as tmp:
            out_chk = Path(tmp) / "map.chk"
            try:
                proc = self._spawn(
                    [exe, "chk", str(map_path), str(out_chk)],
                    stdin=subprocess.DEVNULL,
                    capture_output=True,
                    text=True,
                    encoding="utf-8",
                    errors="replace",
                    cwd=str(Path(exe).parent),
                    timeout=self._subprocess_timeout,
                )
            except subprocess.TimeoutExpired as exc:
                raise MapInfoError(
                    f"IsomTerrain.exe chk did not finish within "
                    f"{self._subprocess_timeout:.0f}s (process killed)."
                ) from exc
            stderr = (getattr(proc, "stderr", "") or "").strip()
            if getattr(proc, "returncode", 1) != 0:
                stdout = (getattr(proc, "stdout", "") or "").strip()
                detail = stderr or stdout or "no output"
                raise MapInfoError(
                    f"IsomTerrain.exe chk failed for {map_path}: {detail}"
                )
            if not out_chk.is_file() or out_chk.stat().st_size == 0:
                raise MapInfoError(
                    f"IsomTerrain.exe chk produced no CHK for {map_path}"
                    + (f" ({stderr})" if stderr else "")
                )
            return out_chk.read_bytes()
