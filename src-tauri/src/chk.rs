use encoding_rs::EUC_KR;
use serde::Serialize;
use std::collections::BTreeMap;

pub const _MAX_SECTIONS: usize = 10_000;
pub const MRGN_ENTRY_SIZE: usize = 20;
pub const UNIT_ENTRY_SIZE: usize = 36;
pub const MRGN_STRUCT_LAYOUT: &str = "<iiiiHH";
pub const UNIT_STRUCT_LAYOUT: &str = "<IHHHHHHBBBBIHHII";
pub const _ANYWHERE_INDEX: usize = 63;
pub const _START_LOCATION_TYPE: u16 = 214;

pub const _OWNR_NAMES: &[&str] = &[
    "Inactive",
    "Computer (game)",
    "Occupied by Human",
    "Rescue Passive",
    "Unused",
    "Computer",
    "Human (Open Slot)",
    "Neutral",
    "Closed",
];

pub const _RACE_NAMES: &[&str] = &[
    "Zerg",
    "Terran",
    "Protoss",
    "Independent",
    "Neutral",
    "User Selectable",
    "Random",
    "Inactive",
];

pub const _ACTIVE_CONTROLLERS: &[u8] = &[1, 2, 3, 5, 6];

pub const _FORCE_FLAG_BITS: &[(&str, u8)] = &[
    ("randomStartLocation", 0x01),
    ("allies", 0x02),
    ("alliedVictory", 0x04),
    ("sharedVision", 0x08),
];

pub const _TILESET_NAMES: &[&str] = &[
    "badlands",
    "platform",
    "installation",
    "ashworld",
    "jungle",
    "desert",
    "ice",
    "twilight",
];

pub const UNIT_NAMES: &[&str] = &[
    "Terran Marine",
    "Terran Ghost",
    "Terran Vulture",
    "Terran Goliath",
    "Goliath Turret",
    "Terran Siege Tank (Tank Mode)",
    "Siege Tank Turret (Tank Mode)",
    "Terran SCV",
    "Terran Wraith",
    "Terran Science Vessel",
    "Gui Montag (Firebat)",
    "Terran Dropship",
    "Terran Battlecruiser",
    "Spider Mine",
    "Nuclear Missile",
    "Terran Civilian",
    "Sarah Kerrigan (Ghost)",
    "Alan Schezar (Goliath)",
    "Alan Schezar Turret",
    "Jim Raynor (Vulture)",
    "Jim Raynor (Marine)",
    "Tom Kazansky (Wraith)",
    "Magellan (Science Vessel)",
    "Edmund Duke (Tank Mode)",
    "Edmund Duke Turret (Tank Mode)",
    "Edmund Duke (Siege Mode)",
    "Edmund Duke Turret (Siege Mode)",
    "Arcturus Mengsk (Battlecruiser)",
    "Hyperion (Battlecruiser)",
    "Norad II (Battlecruiser)",
    "Terran Siege Tank (Siege Mode)",
    "Siege Tank Turret (Siege Mode)",
    "Terran Firebat",
    "Scanner Sweep",
    "Terran Medic",
    "Zerg Larva",
    "Zerg Egg",
    "Zerg Zergling",
    "Zerg Hydralisk",
    "Zerg Ultralisk",
    "Zerg Broodling",
    "Zerg Drone",
    "Zerg Overlord",
    "Zerg Mutalisk",
    "Zerg Guardian",
    "Zerg Queen",
    "Zerg Defiler",
    "Zerg Scourge",
    "Torrasque (Ultralisk)",
    "Matriarch (Queen)",
    "Infested Terran",
    "Infested Kerrigan (Infested Terran)",
    "Unclean One (Defiler)",
    "Hunter Killer (Hydralisk)",
    "Devouring One (Zergling)",
    "Kukulza (Mutalisk)",
    "Kukulza (Guardian)",
    "Yggdrasill (Overlord)",
    "Terran Valkyrie",
    "Mutalisk Cocoon",
    "Protoss Corsair",
    "Protoss Dark Templar",
    "Zerg Devourer",
    "Protoss Dark Archon",
    "Protoss Probe",
    "Protoss Zealot",
    "Protoss Dragoon",
    "Protoss High Templar",
    "Protoss Archon",
    "Protoss Shuttle",
    "Protoss Scout",
    "Protoss Arbiter",
    "Protoss Carrier",
    "Protoss Interceptor",
    "Protoss Dark Templar (Hero)",
    "Zeratul (Dark Templar)",
    "Tassadar/Zeratul (Archon)",
    "Fenix (Zealot)",
    "Fenix (Dragoon)",
    "Tassadar (Templar)",
    "Mojo (Scout)",
    "Warbringer (Reaver)",
    "Gantrithor (Carrier)",
    "Protoss Reaver",
    "Protoss Observer",
    "Protoss Scarab",
    "Danimoth (Arbiter)",
    "Aldaris (Templar)",
    "Artanis (Scout)",
    "Rhynadon (Badlands Critter)",
    "Bengalaas (Jungle Critter)",
    "Cargo Ship (Unused)",
    "Mercenary Gunship (Unused)",
    "Scantid (Desert Critter)",
    "Kakaru (Twilight Critter)",
    "Ragnasaur (Ashworld Critter)",
    "Ursadon (Ice World Critter)",
    "Lurker Egg",
    "Raszagal (Corsair)",
    "Samir Duran (Ghost)",
    "Alexei Stukov (Ghost)",
    "Map Revealer",
    "Gerard DuGalle (BattleCruiser)",
    "Zerg Lurker",
    "Infested Duran",
    "Disruption Web",
    "Terran Command Center",
    "Terran Comsat Station",
    "Terran Nuclear Silo",
    "Terran Supply Depot",
    "Terran Refinery",
    "Terran Barracks",
    "Terran Academy",
    "Terran Factory",
    "Terran Starport",
    "Terran Control Tower",
    "Terran Science Facility",
    "Terran Covert Ops",
    "Terran Physics Lab",
    "Starbase (Unused)",
    "Terran Machine Shop",
    "Repair Bay (Unused)",
    "Terran Engineering Bay",
    "Terran Armory",
    "Terran Missile Turret",
    "Terran Bunker",
    "Norad II (Crashed)",
    "Ion Cannon",
    "Uraj Crystal",
    "Khalis Crystal",
    "Infested Command Center",
    "Zerg Hatchery",
    "Zerg Lair",
    "Zerg Hive",
    "Zerg Nydus Canal",
    "Zerg Hydralisk Den",
    "Zerg Defiler Mound",
    "Zerg Greater Spire",
    "Zerg Queen's Nest",
    "Zerg Evolution Chamber",
    "Zerg Ultralisk Cavern",
    "Zerg Spire",
    "Zerg Spawning Pool",
    "Zerg Creep Colony",
    "Zerg Spore Colony",
    "Unused Zerg Building 1",
    "Zerg Sunken Colony",
    "Zerg Overmind (With Shell)",
    "Zerg Overmind",
    "Zerg Extractor",
    "Mature Crysalis",
    "Zerg Cerebrate",
    "Zerg Cerebrate Daggoth",
    "Unused Zerg Building 2",
    "Protoss Nexus",
    "Protoss Robotics Facility",
    "Protoss Pylon",
    "Protoss Assimilator",
    "Unused Protoss Building 1",
    "Protoss Observatory",
    "Protoss Gateway",
    "Unused Protoss Building 2",
    "Protoss Photon Cannon",
    "Protoss Citadel of Adun",
    "Protoss Cybernetics Core",
    "Protoss Templar Archives",
    "Protoss Forge",
    "Protoss Stargate",
    "Stasis Cell/Prison",
    "Protoss Fleet Beacon",
    "Protoss Arbiter Tribunal",
    "Protoss Robotics Support Bay",
    "Protoss Shield Battery",
    "Khaydarin Crystal Formation",
    "Protoss Temple",
    "Xel'Naga Temple",
    "Mineral Field (Type 1)",
    "Mineral Field (Type 2)",
    "Mineral Field (Type 3)",
    "Cave (Unused)",
    "Cave-in (Unused)",
    "Cantina (Unused)",
    "Mining Platform (Unused)",
    "Independent Command Center (Unused)",
    "Independent Starport (Unused)",
    "Independent Jump Gate (Unused)",
    "Ruins (Unused)",
    "Khadarin Crystal Formation (Unused)",
    "Vespene Geyser",
    "Warp Gate",
    "Psi Disrupter",
    "Zerg Marker",
    "Terran Marker",
    "Protoss Marker",
    "Zerg Beacon",
    "Terran Beacon",
    "Protoss Beacon",
    "Zerg Flag Beacon",
    "Terran Flag Beacon",
    "Protoss Flag Beacon",
    "Power Generator",
    "Overmind Cocoon",
    "Dark Swarm",
    "Floor Missile Trap",
    "Floor Hatch (Unused)",
    "Left Upper Level Door",
    "Right Upper Level Door",
    "Left Pit Door",
    "Right Pit Door",
    "Floor Gun Trap",
    "Left Wall Missile Trap",
    "Left Wall Flame Trap",
    "Right Wall Missile Trap",
    "Right Wall Flame Trap",
    "Start Location",
    "Flag",
    "Young Chrysalis",
    "Psi Emitter",
    "Data Disc",
    "Khaydarin Crystal",
    "Mineral Cluster Type 1",
    "Mineral Cluster Type 2",
    "Protoss Vespene Gas Orb Type 1",
    "Protoss Vespene Gas Orb Type 2",
    "Zerg Vespene Gas Sac Type 1",
    "Zerg Vespene Gas Sac Type 2",
    "Terran Vespene Gas Tank Type 1",
    "Terran Vespene Gas Tank Type 2",
];

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Digest {
    pub map: MapHeader,
    pub players: Vec<Player>,
    pub forces: Vec<Force>,
    pub locations: Vec<Location>,
    pub units: Vec<Unit>,
    pub start_locations: Vec<StartLocation>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MapHeader {
    pub width: u16,
    pub height: u16,
    pub tileset: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Player {
    pub player: String,
    pub controller: String,
    pub race: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub force: Option<u8>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Force {
    pub force: u8,
    pub name: String,
    pub players: Vec<String>,
    pub flags: ForceFlags,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForceFlags {
    pub random_start_location: bool,
    pub allies: bool,
    pub allied_victory: bool,
    pub shared_vision: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Location {
    pub id: usize,
    pub name: String,
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub tile_rect: [i32; 4],
    pub elevation_flags: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inverted: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anywhere: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Unit {
    #[serde(rename = "type")]
    pub type_name: String,
    pub type_id: u16,
    pub owner: String,
    pub x: u16,
    pub y: u16,
    pub tile_x: u16,
    pub tile_y: u16,
    pub hp_percent: u8,
    pub shield_percent: u8,
    pub energy_percent: u8,
    pub resources: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartLocation {
    pub player: String,
    pub x: u16,
    pub y: u16,
    pub tile_x: u16,
    pub tile_y: u16,
}

pub fn walk_sections(data: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut sections = Vec::new();
    let mut pos: i64 = 0;
    let len = data.len() as i64;

    while pos >= 0 && pos + 8 <= len && sections.len() < _MAX_SECTIONS {
        let start = pos as usize;
        let name = data[start..start + 4]
            .iter()
            .map(|&byte| byte as char)
            .collect::<String>();
        let size = i32::from_le_bytes([
            data[start + 4],
            data[start + 5],
            data[start + 6],
            data[start + 7],
        ]);
        let body_start = pos + 8;

        if size >= 0 {
            let body_end = (body_start + i64::from(size)).min(len);
            let body = data[body_start as usize..body_end as usize].to_vec();
            sections.push((name, body));
            pos = body_end;
        } else {
            sections.push((name, Vec::new()));
            pos = body_start + i64::from(size);
        }
    }

    sections
}

pub fn assemble_sections(sections: &[(String, Vec<u8>)]) -> BTreeMap<String, Vec<u8>> {
    let mut assembled = BTreeMap::new();

    for (name, body) in sections {
        if name == "UNIT" {
            assembled
                .entry(name.clone())
                .or_insert_with(Vec::new)
                .extend_from_slice(body);
        } else {
            assembled.insert(name.clone(), body.clone());
        }
    }

    assembled
}

pub fn decode_text(raw: &[u8]) -> String {
    if let Ok(text) = std::str::from_utf8(raw) {
        return text.to_string();
    }

    let (text, _encoding, had_errors) = EUC_KR.decode(raw);
    if !had_errors {
        return text.into_owned();
    }

    raw.iter().map(|&byte| char::from(byte)).collect()
}

pub fn parse_strings(sections: &BTreeMap<String, Vec<u8>>) -> Vec<String> {
    if let Some(data) = sections.get("STRx") {
        parse_string_section(data, 4)
    } else if let Some(data) = sections.get("STR ") {
        parse_string_section(data, 2)
    } else {
        Vec::new()
    }
}

pub fn parse_locations(mrgn: &[u8], strings: &[String]) -> Vec<Location> {
    let mut locations = Vec::new();

    for (index, entry) in mrgn.chunks_exact(MRGN_ENTRY_SIZE).enumerate() {
        let left = read_i32_le(entry, 0);
        let top = read_i32_le(entry, 4);
        let right = read_i32_le(entry, 8);
        let bottom = read_i32_le(entry, 12);
        let string_id = read_u16_le(entry, 16);
        let elevation_flags = read_u16_le(entry, 18);

        let is_anywhere = index == _ANYWHERE_INDEX;
        let is_empty = left == 0 && top == 0 && right == 0 && bottom == 0 && string_id == 0;
        if is_empty && !is_anywhere {
            continue;
        }

        let mut inverted = String::new();
        if left > right {
            inverted.push('x');
        }
        if top > bottom {
            inverted.push('y');
        }

        locations.push(Location {
            id: index + 1,
            name: _string_at(strings, u32::from(string_id)),
            left,
            top,
            right,
            bottom,
            tile_rect: [
                left.div_euclid(32),
                top.div_euclid(32),
                right.div_euclid(32),
                bottom.div_euclid(32),
            ],
            elevation_flags,
            inverted: if inverted.is_empty() {
                None
            } else {
                Some(inverted)
            },
            anywhere: if is_anywhere { Some(true) } else { None },
        });
    }

    locations
}

pub fn parse_units(unit: &[u8]) -> Vec<Unit> {
    unit.chunks_exact(UNIT_ENTRY_SIZE)
        .map(|entry| {
            let x = read_u16_le(entry, 4);
            let y = read_u16_le(entry, 6);
            let type_id = read_u16_le(entry, 8);
            let owner = entry[16];

            Unit {
                type_name: unit_name(type_id),
                type_id,
                owner: owner_label(owner),
                x,
                y,
                tile_x: x / 32,
                tile_y: y / 32,
                hp_percent: entry[17],
                shield_percent: entry[18],
                energy_percent: entry[19],
                resources: read_u32_le(entry, 20),
            }
        })
        .collect()
}

pub fn unit_name(id: u16) -> String {
    UNIT_NAMES
        .get(usize::from(id))
        .map(|name| (*name).to_string())
        .unwrap_or_else(|| format!("ID:{id}"))
}

pub fn owner_label(owner: u8) -> String {
    if owner == 11 {
        "P12 (neutral)".to_string()
    } else {
        format!("P{}", u16::from(owner) + 1)
    }
}

pub fn parse_players(
    ownr: &[u8],
    side: &[u8],
    forc: &[u8],
    strings: &[String],
) -> (Vec<Player>, Vec<Force>) {
    let mut forc_padded = [0u8; 20];
    let copy_len = forc.len().min(forc_padded.len());
    forc_padded[..copy_len].copy_from_slice(&forc[..copy_len]);

    let force_of_slot = &forc_padded[0..8];
    let force_strings = [
        u16::from_le_bytes([forc_padded[8], forc_padded[9]]),
        u16::from_le_bytes([forc_padded[10], forc_padded[11]]),
        u16::from_le_bytes([forc_padded[12], forc_padded[13]]),
        u16::from_le_bytes([forc_padded[14], forc_padded[15]]),
    ];
    let force_flags = &forc_padded[16..20];

    let players = (0..12)
        .map(|slot| {
            let controller_id = ownr.get(slot).copied().unwrap_or(0);
            let race_id = side.get(slot).copied().unwrap_or(7);
            Player {
                player: format!("P{}", slot + 1),
                controller: lookup_controller_name(controller_id),
                race: lookup_race_name(race_id),
                force: if slot < 8 {
                    Some(force_of_slot[slot] + 1)
                } else {
                    None
                },
            }
        })
        .collect();

    let forces = (0..4)
        .map(|force_index| {
            let players = (0..8)
                .filter(|&slot| {
                    force_of_slot[slot] == force_index as u8
                        && slot < ownr.len()
                        && _ACTIVE_CONTROLLERS.contains(&ownr[slot])
                })
                .map(|slot| format!("P{}", slot + 1))
                .collect();
            let flags = force_flags[force_index];
            let mut name = _string_at(strings, u32::from(force_strings[force_index]));
            if name.is_empty() {
                name = format!("Force {}", force_index + 1);
            }

            Force {
                force: (force_index + 1) as u8,
                name,
                players,
                flags: ForceFlags {
                    random_start_location: flags & 0x01 != 0,
                    allies: flags & 0x02 != 0,
                    allied_victory: flags & 0x04 != 0,
                    shared_vision: flags & 0x08 != 0,
                },
            }
        })
        .collect();

    (players, forces)
}

pub fn parse_map_header(dim: &[u8], era: &[u8]) -> MapHeader {
    let (width, height) = if dim.len() >= 4 {
        (read_u16_le(dim, 0), read_u16_le(dim, 2))
    } else {
        (0, 0)
    };

    let tileset = if era.len() >= 2 {
        let era_value = read_u16_le(era, 0) & 0x7;
        _TILESET_NAMES
            .get(usize::from(era_value))
            .unwrap_or(&"")
            .to_string()
    } else {
        String::new()
    };

    MapHeader {
        width,
        height,
        tileset,
    }
}

pub fn digest_chk(data: &[u8]) -> Digest {
    let sections = assemble_sections(&walk_sections(data));
    let strings = parse_strings(&sections);
    let empty = Vec::new();

    let map = parse_map_header(
        sections.get("DIM ").unwrap_or(&empty),
        sections.get("ERA ").unwrap_or(&empty),
    );
    let (players, forces) = parse_players(
        sections.get("OWNR").unwrap_or(&empty),
        sections.get("SIDE").unwrap_or(&empty),
        sections.get("FORC").unwrap_or(&empty),
        &strings,
    );
    let locations = parse_locations(sections.get("MRGN").unwrap_or(&empty), &strings);
    let units = parse_units(sections.get("UNIT").unwrap_or(&empty));
    let start_locations = units
        .iter()
        .filter(|unit| unit.type_id == _START_LOCATION_TYPE)
        .map(|unit| StartLocation {
            player: unit.owner.clone(),
            x: unit.x,
            y: unit.y,
            tile_x: unit.tile_x,
            tile_y: unit.tile_y,
        })
        .collect();

    Digest {
        map,
        players,
        forces,
        locations,
        units,
        start_locations,
    }
}

fn _string_at(strings: &[String], id: u32) -> String {
    if id == 0 {
        return String::new();
    }

    strings.get((id - 1) as usize).cloned().unwrap_or_default()
}

fn parse_string_section(data: &[u8], width: usize) -> Vec<String> {
    if data.len() < width {
        return Vec::new();
    }

    let raw_count = read_offset(data, 0, width).unwrap_or(0) as usize;
    let max_count = (data.len() - width) / width;
    let count = raw_count.min(max_count);

    (0..count)
        .map(|idx| {
            let offset_pos = width * (idx + 1);
            let Some(offset) = read_offset(data, offset_pos, width) else {
                return String::new();
            };
            let offset = offset as usize;
            if offset == 0 || offset >= data.len() {
                return String::new();
            }

            let end = data[offset..]
                .iter()
                .position(|&byte| byte == 0)
                .map(|nul| offset + nul)
                .unwrap_or(data.len());
            decode_text(&data[offset..end])
        })
        .collect()
}

fn read_offset(data: &[u8], offset: usize, width: usize) -> Option<u32> {
    match width {
        2 if offset + 2 <= data.len() => Some(u32::from(read_u16_le(data, offset))),
        4 if offset + 4 <= data.len() => Some(read_u32_le(data, offset)),
        _ => None,
    }
}

fn read_u16_le(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

fn read_u32_le(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

fn read_i32_le(data: &[u8], offset: usize) -> i32 {
    i32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

fn lookup_controller_name(id: u8) -> String {
    _OWNR_NAMES
        .get(usize::from(id))
        .map(|name| (*name).to_string())
        .unwrap_or_else(|| format!("controller:{id}"))
}

fn lookup_race_name(id: u8) -> String {
    _RACE_NAMES
        .get(usize::from(id))
        .map(|name| (*name).to_string())
        .unwrap_or_else(|| format!("race:{id}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use encoding_rs::EUC_KR;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn section(name: &str, body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&(body.len() as i32).to_le_bytes());
        out.extend_from_slice(body);
        out
    }

    fn section_with_size(name: &str, size: i32, body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(body);
        out
    }

    fn str_section(width: usize, values: &[&[u8]]) -> Vec<u8> {
        let count = values.len();
        let table_len = width * (count + 1);
        let mut out = vec![0; table_len];
        if width == 2 {
            out[0..2].copy_from_slice(&(count as u16).to_le_bytes());
        } else {
            out[0..4].copy_from_slice(&(count as u32).to_le_bytes());
        }

        let mut cursor = table_len;
        for (idx, value) in values.iter().enumerate() {
            if width == 2 {
                out[width * (idx + 1)..width * (idx + 2)]
                    .copy_from_slice(&(cursor as u16).to_le_bytes());
            } else {
                out[width * (idx + 1)..width * (idx + 2)]
                    .copy_from_slice(&(cursor as u32).to_le_bytes());
            }
            out.extend_from_slice(value);
            out.push(0);
            cursor = out.len();
        }
        out
    }

    fn mrgn_entry(
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
        string_id: u16,
        elevation_flags: u16,
    ) -> [u8; MRGN_ENTRY_SIZE] {
        let mut out = [0u8; MRGN_ENTRY_SIZE];
        out[0..4].copy_from_slice(&left.to_le_bytes());
        out[4..8].copy_from_slice(&top.to_le_bytes());
        out[8..12].copy_from_slice(&right.to_le_bytes());
        out[12..16].copy_from_slice(&bottom.to_le_bytes());
        out[16..18].copy_from_slice(&string_id.to_le_bytes());
        out[18..20].copy_from_slice(&elevation_flags.to_le_bytes());
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn unit_entry(
        x: u16,
        y: u16,
        type_id: u16,
        owner: u8,
        hp: u8,
        shield: u8,
        energy: u8,
        resources: u32,
    ) -> [u8; UNIT_ENTRY_SIZE] {
        let mut out = [0u8; UNIT_ENTRY_SIZE];
        out[0..4].copy_from_slice(&1234u32.to_le_bytes());
        out[4..6].copy_from_slice(&x.to_le_bytes());
        out[6..8].copy_from_slice(&y.to_le_bytes());
        out[8..10].copy_from_slice(&type_id.to_le_bytes());
        out[10..12].copy_from_slice(&0u16.to_le_bytes());
        out[12..14].copy_from_slice(&0u16.to_le_bytes());
        out[14..16].copy_from_slice(&0u16.to_le_bytes());
        out[16] = owner;
        out[17] = hp;
        out[18] = shield;
        out[19] = energy;
        out[20..24].copy_from_slice(&resources.to_le_bytes());
        out[24..26].copy_from_slice(&0u16.to_le_bytes());
        out[26..28].copy_from_slice(&0u16.to_le_bytes());
        out[28..32].copy_from_slice(&0u32.to_le_bytes());
        out[32..36].copy_from_slice(&0u32.to_le_bytes());
        out
    }

    #[test]
    fn chk_walk_sections_clamps_oversized_section_to_eof() {
        let data = section_with_size("DIM ", 99, &[1, 2, 3]);

        assert_eq!(
            walk_sections(&data),
            vec![("DIM ".to_string(), vec![1, 2, 3])]
        );
    }

    #[test]
    fn chk_walk_sections_handles_negative_size_and_iteration_cap() {
        let data = section_with_size("ABCD", -1, &[]);
        assert_eq!(walk_sections(&data), vec![("ABCD".to_string(), Vec::new())]);

        let mut many = Vec::new();
        for _ in 0..(_MAX_SECTIONS + 5) {
            many.extend_from_slice(&section("TEST", &[]));
        }
        assert_eq!(walk_sections(&many).len(), _MAX_SECTIONS);
    }

    #[test]
    fn chk_walk_sections_ignores_trailing_short_header() {
        let mut data = section("DIM ", &[1, 0, 2, 0]);
        data.extend_from_slice(b"ERA");

        assert_eq!(
            walk_sections(&data),
            vec![("DIM ".to_string(), vec![1, 0, 2, 0])]
        );
    }

    #[test]
    fn chk_assemble_sections_stacks_unit_and_last_wins_other_sections() {
        let sections = vec![
            ("DIM ".to_string(), vec![1]),
            ("UNIT".to_string(), vec![10, 11]),
            ("DIM ".to_string(), vec![2]),
            ("UNIT".to_string(), vec![12]),
        ];

        let assembled = assemble_sections(&sections);
        assert_eq!(assembled.get("DIM ").unwrap(), &vec![2]);
        assert_eq!(assembled.get("UNIT").unwrap(), &vec![10, 11, 12]);
    }

    #[test]
    fn chk_decode_text_and_parse_strings_handle_utf8_cp949_and_strx_precedence() {
        assert_eq!(decode_text("Blue base".as_bytes()), "Blue base");

        let korean = "테란 본진";
        let (encoded, _, had_errors) = EUC_KR.encode(korean);
        assert!(!had_errors);
        assert_eq!(decode_text(&encoded), korean);

        let mut sections = BTreeMap::new();
        sections.insert("STR ".to_string(), str_section(2, &[b"from-str"]));
        sections.insert("STRx".to_string(), str_section(4, &[b"from-strx"]));

        assert_eq!(parse_strings(&sections), vec!["from-strx".to_string()]);

        let mut zero_offset = str_section(2, &[b"first", b"second"]);
        zero_offset[4..6].copy_from_slice(&0u16.to_le_bytes());
        assert_eq!(
            parse_string_section(&zero_offset, 2),
            vec!["first".to_string(), "".to_string()]
        );
    }

    #[test]
    fn chk_parse_locations_skips_zero_keeps_anywhere_and_marks_inversions() {
        let strings = vec![
            "Alpha".to_string(),
            "XInvert".to_string(),
            "YInvert".to_string(),
            "XYInvert".to_string(),
            "Anywhere".to_string(),
        ];
        let mut mrgn = Vec::new();
        mrgn.extend_from_slice(&mrgn_entry(0, 0, 0, 0, 0, 0));
        mrgn.extend_from_slice(&mrgn_entry(96, 64, 32, 128, 2, 7));
        mrgn.extend_from_slice(&mrgn_entry(32, 160, 96, 128, 3, 9));
        mrgn.extend_from_slice(&mrgn_entry(160, 160, 96, 64, 4, 11));
        while mrgn.len() < _ANYWHERE_INDEX * MRGN_ENTRY_SIZE {
            mrgn.extend_from_slice(&mrgn_entry(0, 0, 0, 0, 0, 0));
        }
        mrgn.extend_from_slice(&mrgn_entry(0, 0, 4096, 4096, 5, 0));

        let locations = parse_locations(&mrgn, &strings);

        assert_eq!(
            locations,
            vec![
                Location {
                    id: 2,
                    name: "XInvert".to_string(),
                    left: 96,
                    top: 64,
                    right: 32,
                    bottom: 128,
                    tile_rect: [3, 2, 1, 4],
                    elevation_flags: 7,
                    inverted: Some("x".to_string()),
                    anywhere: None,
                },
                Location {
                    id: 3,
                    name: "YInvert".to_string(),
                    left: 32,
                    top: 160,
                    right: 96,
                    bottom: 128,
                    tile_rect: [1, 5, 3, 4],
                    elevation_flags: 9,
                    inverted: Some("y".to_string()),
                    anywhere: None,
                },
                Location {
                    id: 4,
                    name: "XYInvert".to_string(),
                    left: 160,
                    top: 160,
                    right: 96,
                    bottom: 64,
                    tile_rect: [5, 5, 3, 2],
                    elevation_flags: 11,
                    inverted: Some("xy".to_string()),
                    anywhere: None,
                },
                Location {
                    id: 64,
                    name: "Anywhere".to_string(),
                    left: 0,
                    top: 0,
                    right: 4096,
                    bottom: 4096,
                    tile_rect: [0, 0, 128, 128],
                    elevation_flags: 0,
                    inverted: None,
                    anywhere: Some(true),
                },
            ]
        );
    }

    #[test]
    fn chk_parse_units_decodes_fields_start_locations_and_drops_trailing_bytes() {
        let mut data = Vec::new();
        data.extend_from_slice(&unit_entry(96, 160, 0, 0, 100, 75, 50, 1_500));
        data.extend_from_slice(&unit_entry(
            320,
            64,
            _START_LOCATION_TYPE,
            11,
            100,
            100,
            100,
            0,
        ));
        data.extend_from_slice(&[1, 2, 3]);

        let units = parse_units(&data);

        assert_eq!(
            units,
            vec![
                Unit {
                    type_name: "Terran Marine".to_string(),
                    type_id: 0,
                    owner: "P1".to_string(),
                    x: 96,
                    y: 160,
                    tile_x: 3,
                    tile_y: 5,
                    hp_percent: 100,
                    shield_percent: 75,
                    energy_percent: 50,
                    resources: 1_500,
                },
                Unit {
                    type_name: "Start Location".to_string(),
                    type_id: _START_LOCATION_TYPE,
                    owner: "P12 (neutral)".to_string(),
                    x: 320,
                    y: 64,
                    tile_x: 10,
                    tile_y: 2,
                    hp_percent: 100,
                    shield_percent: 100,
                    energy_percent: 100,
                    resources: 0,
                },
            ]
        );

        let starts: Vec<StartLocation> = units
            .iter()
            .filter(|unit| unit.type_id == _START_LOCATION_TYPE)
            .map(|unit| StartLocation {
                player: unit.owner.clone(),
                x: unit.x,
                y: unit.y,
                tile_x: unit.tile_x,
                tile_y: unit.tile_y,
            })
            .collect();
        assert_eq!(
            starts,
            vec![StartLocation {
                player: "P12 (neutral)".to_string(),
                x: 320,
                y: 64,
                tile_x: 10,
                tile_y: 2,
            }]
        );
    }

    #[test]
    fn chk_parse_players_decodes_short_forc_padding_membership_and_flags() {
        let ownr = [2, 5, 0, 6, 3, 7, 2, 0, 2, 5, 6, 0];
        let side = [0, 1, 2, 6, 7, 3, 4, 5, 1, 2, 0, 7];
        let mut forc = Vec::new();
        forc.extend_from_slice(&[0, 0, 1, 1, 2, 2, 0, 0]);
        forc.extend_from_slice(&1u16.to_le_bytes());
        forc.extend_from_slice(&2u16.to_le_bytes());
        forc.extend_from_slice(&0u16.to_le_bytes());
        forc.extend_from_slice(&0u16.to_le_bytes());
        forc.push(0x0f);
        let strings = vec!["Force One".to_string(), "Force Two".to_string()];

        let (players, forces) = parse_players(&ownr, &side, &forc, &strings);

        assert_eq!(
            players,
            vec![
                Player {
                    player: "P1".to_string(),
                    controller: "Occupied by Human".to_string(),
                    race: "Zerg".to_string(),
                    force: Some(1),
                },
                Player {
                    player: "P2".to_string(),
                    controller: "Computer".to_string(),
                    race: "Terran".to_string(),
                    force: Some(1),
                },
                Player {
                    player: "P3".to_string(),
                    controller: "Inactive".to_string(),
                    race: "Protoss".to_string(),
                    force: Some(2),
                },
                Player {
                    player: "P4".to_string(),
                    controller: "Human (Open Slot)".to_string(),
                    race: "Random".to_string(),
                    force: Some(2),
                },
                Player {
                    player: "P5".to_string(),
                    controller: "Rescue Passive".to_string(),
                    race: "Inactive".to_string(),
                    force: Some(3),
                },
                Player {
                    player: "P6".to_string(),
                    controller: "Neutral".to_string(),
                    race: "Independent".to_string(),
                    force: Some(3),
                },
                Player {
                    player: "P7".to_string(),
                    controller: "Occupied by Human".to_string(),
                    race: "Neutral".to_string(),
                    force: Some(1),
                },
                Player {
                    player: "P8".to_string(),
                    controller: "Inactive".to_string(),
                    race: "User Selectable".to_string(),
                    force: Some(1),
                },
                Player {
                    player: "P9".to_string(),
                    controller: "Occupied by Human".to_string(),
                    race: "Terran".to_string(),
                    force: None,
                },
                Player {
                    player: "P10".to_string(),
                    controller: "Computer".to_string(),
                    race: "Protoss".to_string(),
                    force: None,
                },
                Player {
                    player: "P11".to_string(),
                    controller: "Human (Open Slot)".to_string(),
                    race: "Zerg".to_string(),
                    force: None,
                },
                Player {
                    player: "P12".to_string(),
                    controller: "Inactive".to_string(),
                    race: "Inactive".to_string(),
                    force: None,
                },
            ]
        );

        assert_eq!(
            forces,
            vec![
                Force {
                    force: 1,
                    name: "Force One".to_string(),
                    players: vec!["P1".to_string(), "P2".to_string(), "P7".to_string()],
                    flags: ForceFlags {
                        random_start_location: true,
                        allies: true,
                        allied_victory: true,
                        shared_vision: true,
                    },
                },
                Force {
                    force: 2,
                    name: "Force Two".to_string(),
                    players: vec!["P4".to_string()],
                    flags: ForceFlags {
                        random_start_location: false,
                        allies: false,
                        allied_victory: false,
                        shared_vision: false,
                    },
                },
                Force {
                    force: 3,
                    name: "Force 3".to_string(),
                    players: vec!["P5".to_string()],
                    flags: ForceFlags {
                        random_start_location: false,
                        allies: false,
                        allied_victory: false,
                        shared_vision: false,
                    },
                },
                Force {
                    force: 4,
                    name: "Force 4".to_string(),
                    players: vec![],
                    flags: ForceFlags {
                        random_start_location: false,
                        allies: false,
                        allied_victory: false,
                        shared_vision: false,
                    },
                },
            ]
        );
    }

    #[test]
    fn chk_digest_chk_end_to_end_shape_matches_python_contract() {
        let mut dim = Vec::new();
        dim.extend_from_slice(&64u16.to_le_bytes());
        dim.extend_from_slice(&128u16.to_le_bytes());

        let era = 4u16.to_le_bytes();
        let strx = str_section(4, &[b"Force One", b"Main", b"Anywhere"]);

        let ownr = [2, 5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let side = [1, 2, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7];
        let mut forc = Vec::new();
        forc.extend_from_slice(&[0, 0, 1, 1, 2, 2, 3, 3]);
        forc.extend_from_slice(&1u16.to_le_bytes());
        forc.extend_from_slice(&0u16.to_le_bytes());
        forc.extend_from_slice(&0u16.to_le_bytes());
        forc.extend_from_slice(&0u16.to_le_bytes());
        forc.extend_from_slice(&[0x06, 0, 0, 0]);

        let mut mrgn = Vec::new();
        mrgn.extend_from_slice(&mrgn_entry(64, 96, 160, 224, 2, 3));
        while mrgn.len() < _ANYWHERE_INDEX * MRGN_ENTRY_SIZE {
            mrgn.extend_from_slice(&mrgn_entry(0, 0, 0, 0, 0, 0));
        }
        mrgn.extend_from_slice(&mrgn_entry(0, 0, 2048, 4096, 3, 0));

        let mut units = Vec::new();
        units.extend_from_slice(&unit_entry(96, 160, 0, 0, 100, 100, 100, 0));
        units.extend_from_slice(&unit_entry(
            320,
            64,
            _START_LOCATION_TYPE,
            1,
            100,
            100,
            100,
            0,
        ));

        let mut chk = Vec::new();
        chk.extend_from_slice(&section("DIM ", &dim));
        chk.extend_from_slice(&section("ERA ", &era));
        chk.extend_from_slice(&section("STRx", &strx));
        chk.extend_from_slice(&section("OWNR", &ownr));
        chk.extend_from_slice(&section("SIDE", &side));
        chk.extend_from_slice(&section("FORC", &forc));
        chk.extend_from_slice(&section("MRGN", &mrgn));
        chk.extend_from_slice(&section("UNIT", &units));

        let digest = digest_chk(&chk);
        let value = serde_json::to_value(digest).unwrap();

        assert_eq!(
            value,
            json!({
                "map": {"width": 64, "height": 128, "tileset": "jungle"},
                "players": [
                    {"player": "P1", "controller": "Occupied by Human", "race": "Terran", "force": 1},
                    {"player": "P2", "controller": "Computer", "race": "Protoss", "force": 1},
                    {"player": "P3", "controller": "Inactive", "race": "Inactive", "force": 2},
                    {"player": "P4", "controller": "Inactive", "race": "Inactive", "force": 2},
                    {"player": "P5", "controller": "Inactive", "race": "Inactive", "force": 3},
                    {"player": "P6", "controller": "Inactive", "race": "Inactive", "force": 3},
                    {"player": "P7", "controller": "Inactive", "race": "Inactive", "force": 4},
                    {"player": "P8", "controller": "Inactive", "race": "Inactive", "force": 4},
                    {"player": "P9", "controller": "Inactive", "race": "Inactive"},
                    {"player": "P10", "controller": "Inactive", "race": "Inactive"},
                    {"player": "P11", "controller": "Inactive", "race": "Inactive"},
                    {"player": "P12", "controller": "Inactive", "race": "Inactive"}
                ],
                "forces": [
                    {
                        "force": 1,
                        "name": "Force One",
                        "players": ["P1", "P2"],
                        "flags": {
                            "randomStartLocation": false,
                            "allies": true,
                            "alliedVictory": true,
                            "sharedVision": false
                        }
                    },
                    {
                        "force": 2,
                        "name": "Force 2",
                        "players": [],
                        "flags": {
                            "randomStartLocation": false,
                            "allies": false,
                            "alliedVictory": false,
                            "sharedVision": false
                        }
                    },
                    {
                        "force": 3,
                        "name": "Force 3",
                        "players": [],
                        "flags": {
                            "randomStartLocation": false,
                            "allies": false,
                            "alliedVictory": false,
                            "sharedVision": false
                        }
                    },
                    {
                        "force": 4,
                        "name": "Force 4",
                        "players": [],
                        "flags": {
                            "randomStartLocation": false,
                            "allies": false,
                            "alliedVictory": false,
                            "sharedVision": false
                        }
                    }
                ],
                "locations": [
                    {
                        "id": 1,
                        "name": "Main",
                        "left": 64,
                        "top": 96,
                        "right": 160,
                        "bottom": 224,
                        "tileRect": [2, 3, 5, 7],
                        "elevationFlags": 3
                    },
                    {
                        "id": 64,
                        "name": "Anywhere",
                        "left": 0,
                        "top": 0,
                        "right": 2048,
                        "bottom": 4096,
                        "tileRect": [0, 0, 64, 128],
                        "elevationFlags": 0,
                        "anywhere": true
                    }
                ],
                "units": [
                    {
                        "type": "Terran Marine",
                        "typeId": 0,
                        "owner": "P1",
                        "x": 96,
                        "y": 160,
                        "tileX": 3,
                        "tileY": 5,
                        "hpPercent": 100,
                        "shieldPercent": 100,
                        "energyPercent": 100,
                        "resources": 0
                    },
                    {
                        "type": "Start Location",
                        "typeId": 214,
                        "owner": "P2",
                        "x": 320,
                        "y": 64,
                        "tileX": 10,
                        "tileY": 2,
                        "hpPercent": 100,
                        "shieldPercent": 100,
                        "energyPercent": 100,
                        "resources": 0
                    }
                ],
                "startLocations": [
                    {"player": "P2", "x": 320, "y": 64, "tileX": 10, "tileY": 2}
                ]
            })
        );
    }
}
