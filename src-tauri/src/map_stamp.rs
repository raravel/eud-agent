use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::map_model::{
    DoodadState, LocationState, MapLayer, MapOperation, SelectionMask, SpriteState, TileRect,
    UnitState,
};
use crate::map_verify::{MapRequestAuthority, SUPPORTED_MAP_LAYERS};

const MAX_STAMP_DESTINATIONS: usize = 64;
const ANYWHERE_LOCATION_INDEX: usize = 63;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StampCollisionPolicy {
    Merge,
    Replace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StampDestination {
    pub x: u16,
    pub y: u16,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StampPreviewInput {
    pub selection_id: String,
    pub destinations: Vec<StampDestination>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StampPlaceInput {
    pub selection_id: String,
    pub destinations: Vec<StampDestination>,
    pub collision_policy: StampCollisionPolicy,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StampLayerCounts {
    pub units: u32,
    pub buildings: u32,
    pub doodads: u32,
    pub sprites: u32,
    pub locations: u32,
}

impl StampLayerCounts {
    pub fn total(&self) -> u32 {
        self.units
            .saturating_add(self.buildings)
            .saturating_add(self.doodads)
            .saturating_add(self.sprites)
            .saturating_add(self.locations)
    }

    fn increment(&mut self, layer: MapLayer) {
        match layer {
            MapLayer::Units => self.units = self.units.saturating_add(1),
            MapLayer::Buildings => self.buildings = self.buildings.saturating_add(1),
            MapLayer::Doodads => self.doodads = self.doodads.saturating_add(1),
            MapLayer::Sprites => self.sprites = self.sprites.saturating_add(1),
            MapLayer::Locations => self.locations = self.locations.saturating_add(1),
            MapLayer::Terrain => {}
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StampPlacementReport {
    pub selection_id: String,
    pub label: String,
    pub width: u16,
    pub height: u16,
    pub layers: BTreeSet<MapLayer>,
    pub destinations: Vec<StampDestination>,
    pub terrain_cells_per_destination: u32,
    pub source: StampLayerCounts,
    pub collisions: StampLayerCounts,
    pub partial_collisions: StampLayerCounts,
    pub outside_authority_cells: u32,
    pub protected_cells: u32,
    pub required_location_slots: u32,
    pub available_location_slots: u32,
}

impl StampPlacementReport {
    pub fn has_collisions(&self) -> bool {
        self.collisions.total() > 0
    }

    pub fn blocked(&self) -> bool {
        self.outside_authority_cells > 0
            || self.protected_cells > 0
            || self.required_location_slots > self.available_location_slots
    }
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StampPlacementResult {
    pub report: StampPlacementReport,
    pub patch: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct CompiledStampPlacement {
    pub operations: Vec<MapOperation>,
    pub report: StampPlacementReport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersistentSelection {
    pub id: String,
    pub label: String,
    pub role: crate::map_model::SelectionRole,
    pub layers: BTreeSet<MapLayer>,
    pub bounds: TileRect,
    pub selected_cells: u32,
    pub rows: Vec<crate::map_model::RowSpan>,
}

impl PersistentSelection {
    pub fn from_selection(selection: &SelectionMask) -> Self {
        Self {
            id: selection.id.clone(),
            label: selection.label.clone(),
            role: selection.role,
            layers: selection.layers.clone(),
            bounds: selection.bounds,
            selected_cells: selection.selected_cells,
            rows: selection.rows.clone(),
        }
    }

    pub fn bind(
        &self,
        source_revision: impl Into<String>,
        map_width: u16,
        map_height: u16,
    ) -> Result<SelectionMask, String> {
        SelectionMask::canonical(
            self.id.clone(),
            self.label.clone(),
            source_revision,
            self.role,
            self.layers.clone(),
            crate::map_model::MaskGrid {
                width: map_width,
                height: map_height,
                rows: self.rows.clone(),
            },
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersistentSelectionLibrary {
    pub schema: String,
    pub selections: BTreeMap<String, PersistentSelection>,
}

impl PersistentSelectionLibrary {
    pub fn empty() -> Self {
        Self {
            schema: "eud-map-selection-palette/1".to_string(),
            selections: BTreeMap::new(),
        }
    }
}

#[derive(Clone)]
struct UnitRecord {
    ordinal: usize,
    fingerprint: String,
    layer: MapLayer,
    state: UnitState,
}

#[derive(Clone)]
struct DoodadRecord {
    ordinal: usize,
    fingerprint: String,
    state: DoodadState,
}

#[derive(Clone)]
struct SpriteRecord {
    ordinal: usize,
    fingerprint: String,
    state: SpriteState,
}

#[derive(Clone)]
struct LocationRecord {
    state: LocationState,
}

#[derive(Clone, Copy)]
struct DoodadMetadata {
    width: u16,
    height: u16,
    overlay_id: Option<u16>,
    overlay_flags: Option<u16>,
}

#[derive(Clone)]
struct ParsedMap {
    width: u16,
    height: u16,
    tiles: Vec<u16>,
    units: Vec<UnitRecord>,
    doodads: Vec<DoodadRecord>,
    sprites: Vec<SpriteRecord>,
    locations: Vec<LocationRecord>,
    available_location_slots: u32,
}

#[derive(Clone)]
struct StampCatalog {
    unit_extents: BTreeMap<u16, crate::map_verify::UnitExtent>,
    building_ids: BTreeSet<u16>,
    doodads: BTreeMap<u16, DoodadMetadata>,
}

#[derive(Default)]
struct SourceContent {
    units: Vec<UnitRecord>,
    doodads: Vec<DoodadRecord>,
    sprites: Vec<SpriteRecord>,
    locations: Vec<LocationRecord>,
    counts: StampLayerCounts,
}

#[derive(Default)]
struct DestinationCollisions {
    units: BTreeSet<usize>,
    doodads: BTreeSet<usize>,
    sprites: BTreeSet<usize>,
    locations: BTreeSet<u16>,
    partial_units: BTreeSet<usize>,
    partial_doodads: BTreeSet<usize>,
    partial_sprites: BTreeSet<usize>,
    partial_locations: BTreeSet<u16>,
}

pub fn compile_stamp_placement(
    source_map: &Path,
    destination_map: &Path,
    starcraft_path: &Path,
    selection: &SelectionMask,
    destinations: &[StampDestination],
    policy: Option<StampCollisionPolicy>,
    authority: &MapRequestAuthority,
) -> Result<CompiledStampPlacement, String> {
    validate_destinations(
        selection,
        destinations,
        authority.map_width,
        authority.map_height,
    )?;
    let tileset_name = map_tileset_name(source_map)?;
    let catalog = load_catalog(starcraft_path, &tileset_name)?;
    let source = parse_map(source_map, &catalog)?;
    let destination = if source_map == destination_map {
        source.clone()
    } else {
        parse_map(destination_map, &catalog)?
    };
    if source.width != destination.width
        || source.height != destination.height
        || source.width != authority.map_width
        || source.height != authority.map_height
    {
        return Err("stamp source, destination, and authority dimensions do not match".to_string());
    }

    let layers = stamp_layers(selection);
    let content = capture_source_content(&source, &catalog, selection, &layers);
    let collisions =
        destination_collisions(&destination, &catalog, selection, destinations, &layers);
    let collision_counts = collision_counts(&destination, &collisions);
    let partial_counts = partial_collision_counts(&destination, &collisions);
    let required_location_slots = (content.locations.len() * destinations.len()) as u32;
    let released_location_slots = if policy == Some(StampCollisionPolicy::Replace) {
        collisions.locations.len() as u32
    } else {
        0
    };
    let available_location_slots = destination
        .available_location_slots
        .saturating_add(released_location_slots);
    let (outside_authority_cells, protected_cells) = authority_conflicts(
        authority,
        selection,
        destinations,
        &layers,
        &content,
        &catalog,
        &destination,
        &collisions,
        policy,
    );
    let report = StampPlacementReport {
        selection_id: selection.id.clone(),
        label: selection.label.clone(),
        width: selection.bounds.right - selection.bounds.left,
        height: selection.bounds.bottom - selection.bounds.top,
        layers: layers.clone(),
        destinations: destinations.to_vec(),
        terrain_cells_per_destination: if layers.contains(&MapLayer::Terrain) {
            selection.selected_cells
        } else {
            0
        },
        source: content.counts.clone(),
        collisions: collision_counts,
        partial_collisions: partial_counts,
        outside_authority_cells,
        protected_cells,
        required_location_slots,
        available_location_slots,
    };

    if policy.is_none() {
        return Ok(CompiledStampPlacement {
            operations: Vec::new(),
            report,
        });
    }
    if report.blocked() {
        return Err(blocked_report_message(&report));
    }
    if policy == Some(StampCollisionPolicy::Replace) && report.partial_collisions.total() > 0 {
        return Err(
            "replace cannot remove an object or location that crosses the stamp boundary"
                .to_string(),
        );
    }

    let operations = build_operations(
        &source,
        &destination,
        &catalog,
        selection,
        destinations,
        &layers,
        &content,
        &collisions,
        policy.expect("checked above"),
    )?;
    if operations.is_empty() {
        return Err("stamp placement would not change the candidate".to_string());
    }
    Ok(CompiledStampPlacement { operations, report })
}

fn blocked_report_message(report: &StampPlacementReport) -> String {
    if report.outside_authority_cells > 0 {
        return format!(
            "stamp placement changes {} cell(s) outside the current request authority",
            report.outside_authority_cells
        );
    }
    if report.protected_cells > 0 {
        return format!(
            "stamp placement changes {} persistently protected cell(s)",
            report.protected_cells
        );
    }
    format!(
        "stamp placement needs {} free location slot(s), but only {} are available",
        report.required_location_slots, report.available_location_slots
    )
}

fn stamp_layers(selection: &SelectionMask) -> BTreeSet<MapLayer> {
    if selection.layers.is_empty() {
        SUPPORTED_MAP_LAYERS.into_iter().collect()
    } else {
        selection.layers.clone()
    }
}

fn validate_destinations(
    selection: &SelectionMask,
    destinations: &[StampDestination],
    map_width: u16,
    map_height: u16,
) -> Result<(), String> {
    if destinations.is_empty() {
        return Err("stamp placement requires at least one destination".to_string());
    }
    if destinations.len() > MAX_STAMP_DESTINATIONS {
        return Err(format!(
            "stamp placement supports at most {MAX_STAMP_DESTINATIONS} destinations"
        ));
    }
    let width = selection.bounds.right - selection.bounds.left;
    let height = selection.bounds.bottom - selection.bounds.top;
    let mut occupied = BTreeSet::new();
    for destination in destinations {
        if destination.x.saturating_add(width) > map_width
            || destination.y.saturating_add(height) > map_height
        {
            return Err("stamp destination is outside map bounds".to_string());
        }
        for (x, y) in shifted_selection_cells(selection, *destination) {
            if !occupied.insert((x, y)) {
                return Err("stamp destinations overlap each other".to_string());
            }
        }
    }
    Ok(())
}

fn shifted_selection_cells(
    selection: &SelectionMask,
    destination: StampDestination,
) -> BTreeSet<(u16, u16)> {
    selection
        .cells()
        .into_iter()
        .map(|(x, y)| {
            (
                destination.x + (x - selection.bounds.left),
                destination.y + (y - selection.bounds.top),
            )
        })
        .collect()
}

fn load_catalog(starcraft_path: &Path, tileset_name: &str) -> Result<StampCatalog, String> {
    let (building_ids, unit_extents) =
        crate::map_verify::unit_catalog(starcraft_path, tileset_name)?;
    let tileset = [
        "badlands",
        "platform",
        "installation",
        "ashworld",
        "jungle",
        "desert",
        "arctic",
        "twilight",
    ]
    .iter()
    .position(|candidate| *candidate == tileset_name)
    .ok_or_else(|| format!("unknown map tileset: {tileset_name}"))?;
    let request = json!({
        "schema": "eud-map-catalog/1",
        "kind": "doodads",
        "tileset": tileset,
        "offset": 0,
        "limit": 512,
    });
    let response = isom::catalog_query(starcraft_path, request.to_string().as_bytes())
        .map_err(|error| format!("doodad DAT catalog is unavailable: {error}"))?;
    let value: serde_json::Value = serde_json::from_str(&response)
        .map_err(|error| format!("doodad DAT catalog response is invalid: {error}"))?;
    let mut doodads = BTreeMap::new();
    for entry in value["entries"]
        .as_array()
        .ok_or_else(|| "doodad DAT catalog has no entries array".to_string())?
    {
        let id = json_u16(entry, "id", "doodad")?;
        let width = json_u16(entry, "width", "doodad")?;
        let height = json_u16(entry, "height", "doodad")?;
        let (overlay_id, overlay_flags) = if entry["overlay"].as_bool().unwrap_or(false) {
            (
                Some(json_u16(entry, "overlayId", "doodad")?),
                Some(json_u16(entry, "overlayFlags", "doodad")?),
            )
        } else {
            (None, None)
        };
        doodads.insert(
            id,
            DoodadMetadata {
                width,
                height,
                overlay_id,
                overlay_flags,
            },
        );
    }
    Ok(StampCatalog {
        unit_extents,
        building_ids,
        doodads,
    })
}

fn map_tileset_name(map_path: &Path) -> Result<String, String> {
    let chk = isom::chk_extract(map_path).map_err(|error| error.to_string())?;
    Ok(crate::chk::digest_chk(&chk).map.tileset)
}

fn parse_map(map_path: &Path, catalog: &StampCatalog) -> Result<ParsedMap, String> {
    let chk = isom::chk_extract(map_path).map_err(|error| error.to_string())?;
    let digest = crate::chk::digest_chk(&chk);
    let sections = crate::chk::assemble_sections(&crate::chk::walk_sections(&chk));
    let raw_units = sections.get("UNIT").map(Vec::as_slice).unwrap_or(&[]);
    let units = raw_units
        .chunks_exact(crate::chk::UNIT_ENTRY_SIZE)
        .enumerate()
        .map(|(ordinal, entry)| {
            let type_id = read_u16(entry, 8);
            UnitRecord {
                ordinal,
                fingerprint: crate::map_model::hex_sha256(entry),
                layer: if catalog.building_ids.contains(&type_id) {
                    MapLayer::Buildings
                } else {
                    MapLayer::Units
                },
                state: UnitState {
                    type_id,
                    owner: entry[16],
                    x: read_u16(entry, 4),
                    y: read_u16(entry, 6),
                    class_id: read_u32(entry, 0),
                    relation_flags: read_u16(entry, 10),
                    valid_state_flags: read_u16(entry, 12),
                    valid_field_flags: read_u16(entry, 14),
                    hp_percent: entry[17],
                    shield_percent: entry[18],
                    energy_percent: entry[19],
                    resource_amount: read_u32(entry, 20),
                    hangar_amount: read_u16(entry, 24),
                    state_flags: read_u16(entry, 26),
                    unused: read_u32(entry, 28),
                    relation_class_id: read_u32(entry, 32),
                },
            }
        })
        .collect();
    let raw_doodads = sections.get("DD2 ").map(Vec::as_slice).unwrap_or(&[]);
    let doodads = raw_doodads
        .chunks_exact(crate::chk::DD2_ENTRY_SIZE)
        .enumerate()
        .map(|(ordinal, entry)| DoodadRecord {
            ordinal,
            fingerprint: crate::map_model::hex_sha256(entry),
            state: DoodadState {
                doodad_id: read_u16(entry, 0),
                x: read_u16(entry, 2),
                y: read_u16(entry, 4),
                owner: entry[6],
                disabled: entry[7] != 0,
            },
        })
        .collect();
    let raw_sprites = sections.get("THG2").map(Vec::as_slice).unwrap_or(&[]);
    let sprites = raw_sprites
        .chunks_exact(crate::chk::THG2_ENTRY_SIZE)
        .enumerate()
        .map(|(ordinal, entry)| SpriteRecord {
            ordinal,
            fingerprint: crate::map_model::hex_sha256(entry),
            state: SpriteState {
                sprite_id: read_u16(entry, 0),
                x: read_u16(entry, 2),
                y: read_u16(entry, 4),
                owner: entry[6],
                flags: read_u16(entry, 8),
            },
        })
        .collect();
    let raw_strings = raw_strings(&sections);
    let raw_locations = sections.get("MRGN").map(Vec::as_slice).unwrap_or(&[]);
    let mut available_location_slots = 0u32;
    let mut locations = Vec::new();
    for (index, entry) in raw_locations
        .chunks_exact(crate::chk::MRGN_ENTRY_SIZE)
        .enumerate()
    {
        if index == ANYWHERE_LOCATION_INDEX {
            continue;
        }
        let blank = entry.iter().all(|byte| *byte == 0);
        if blank {
            available_location_slots = available_location_slots.saturating_add(1);
            continue;
        }
        let string_id = usize::from(read_u16(entry, 16));
        let name = string_id
            .checked_sub(1)
            .and_then(|index| raw_strings.get(index))
            .cloned()
            .unwrap_or_default();
        locations.push(LocationRecord {
            state: LocationState {
                location_id: (index + 1) as u16,
                left: read_i32(entry, 0),
                top: read_i32(entry, 4),
                right: read_i32(entry, 8),
                bottom: read_i32(entry, 12),
                elevation_flags: read_u16(entry, 18),
                name_bytes_hex: if name.is_empty() {
                    None
                } else {
                    Some(bytes_hex(&name))
                },
            },
        });
    }
    Ok(ParsedMap {
        width: digest.map.width,
        height: digest.map.height,
        tiles: digest.tiles,
        units,
        doodads,
        sprites,
        locations,
        available_location_slots,
    })
}

fn capture_source_content(
    map: &ParsedMap,
    catalog: &StampCatalog,
    selection: &SelectionMask,
    layers: &BTreeSet<MapLayer>,
) -> SourceContent {
    let mut content = SourceContent::default();
    for unit in &map.units {
        if !layers.contains(&unit.layer) {
            continue;
        }
        let cells = unit_cells(unit, catalog, map.width, map.height);
        if fully_contained(selection, &cells) {
            content.counts.increment(unit.layer);
            content.units.push(unit.clone());
        }
    }
    for doodad in &map.doodads {
        if !layers.contains(&MapLayer::Doodads) {
            break;
        }
        let cells = doodad_cells(doodad, catalog, map.width, map.height);
        if fully_contained(selection, &cells) {
            content.counts.increment(MapLayer::Doodads);
            content.doodads.push(doodad.clone());
        }
    }
    let mut source_overlay_counts = doodad_overlay_counts(&content.doodads, catalog);
    for sprite in &map.sprites {
        if !layers.contains(&MapLayer::Sprites) {
            break;
        }
        if consume_overlay(
            &mut source_overlay_counts,
            &sprite_overlay_key(&sprite.state),
        ) {
            continue;
        }
        let cells = sprite_cells(sprite, map.width, map.height);
        if fully_contained(selection, &cells) {
            content.counts.increment(MapLayer::Sprites);
            content.sprites.push(sprite.clone());
        }
    }
    for location in &map.locations {
        if !layers.contains(&MapLayer::Locations) {
            break;
        }
        let cells = location_cells(location, map.width, map.height);
        if fully_contained(selection, &cells) {
            content.counts.increment(MapLayer::Locations);
            content.locations.push(location.clone());
        }
    }
    content
}

fn destination_collisions(
    map: &ParsedMap,
    catalog: &StampCatalog,
    selection: &SelectionMask,
    destinations: &[StampDestination],
    layers: &BTreeSet<MapLayer>,
) -> DestinationCollisions {
    let masks = destinations
        .iter()
        .map(|destination| shifted_selection_cells(selection, *destination))
        .collect::<Vec<_>>();
    let mut result = DestinationCollisions::default();
    for unit in &map.units {
        if !layers.contains(&unit.layer) {
            continue;
        }
        let cells = unit_cells(unit, catalog, map.width, map.height);
        record_collision(
            unit.ordinal,
            &cells,
            &masks,
            &mut result.units,
            &mut result.partial_units,
        );
    }
    for doodad in &map.doodads {
        if !layers.contains(&MapLayer::Doodads) {
            break;
        }
        let cells = doodad_cells(doodad, catalog, map.width, map.height);
        record_collision(
            doodad.ordinal,
            &cells,
            &masks,
            &mut result.doodads,
            &mut result.partial_doodads,
        );
    }
    for sprite in &map.sprites {
        if !layers.contains(&MapLayer::Sprites) {
            break;
        }
        let cells = sprite_cells(sprite, map.width, map.height);
        record_collision(
            sprite.ordinal,
            &cells,
            &masks,
            &mut result.sprites,
            &mut result.partial_sprites,
        );
    }
    for location in &map.locations {
        if !layers.contains(&MapLayer::Locations) {
            break;
        }
        let cells = location_cells(location, map.width, map.height);
        record_collision(
            location.state.location_id,
            &cells,
            &masks,
            &mut result.locations,
            &mut result.partial_locations,
        );
    }
    result
}

fn record_collision<T: Ord + Copy>(
    id: T,
    cells: &[(u16, u16)],
    masks: &[BTreeSet<(u16, u16)>],
    collisions: &mut BTreeSet<T>,
    partial: &mut BTreeSet<T>,
) {
    for mask in masks {
        if !cells.iter().any(|cell| mask.contains(cell)) {
            continue;
        }
        collisions.insert(id);
        if !cells.iter().all(|cell| mask.contains(cell)) {
            partial.insert(id);
        }
        return;
    }
}

fn collision_counts(map: &ParsedMap, collisions: &DestinationCollisions) -> StampLayerCounts {
    let mut counts = StampLayerCounts::default();
    for ordinal in &collisions.units {
        if let Some(unit) = map.units.iter().find(|unit| unit.ordinal == *ordinal) {
            counts.increment(unit.layer);
        }
    }
    counts.doodads = collisions.doodads.len() as u32;
    counts.sprites = collisions.sprites.len() as u32;
    counts.locations = collisions.locations.len() as u32;
    counts
}

fn partial_collision_counts(
    map: &ParsedMap,
    collisions: &DestinationCollisions,
) -> StampLayerCounts {
    let mut counts = StampLayerCounts::default();
    for ordinal in &collisions.partial_units {
        if let Some(unit) = map.units.iter().find(|unit| unit.ordinal == *ordinal) {
            counts.increment(unit.layer);
        }
    }
    counts.doodads = collisions.partial_doodads.len() as u32;
    counts.sprites = collisions.partial_sprites.len() as u32;
    counts.locations = collisions.partial_locations.len() as u32;
    counts
}

#[allow(clippy::too_many_arguments)]
fn authority_conflicts(
    authority: &MapRequestAuthority,
    selection: &SelectionMask,
    destinations: &[StampDestination],
    layers: &BTreeSet<MapLayer>,
    content: &SourceContent,
    catalog: &StampCatalog,
    destination_map: &ParsedMap,
    collisions: &DestinationCollisions,
    policy: Option<StampCollisionPolicy>,
) -> (u32, u32) {
    let mut outside = BTreeSet::new();
    let mut protected = BTreeSet::new();
    let mut check = |layer: MapLayer, x: u16, y: u16| {
        if !authority.allows(layer, x, y) {
            outside.insert((layer, x, y));
        }
        if authority.forbids(layer, x, y) {
            protected.insert((layer, x, y));
        }
    };
    for destination in destinations {
        let dx = i32::from(destination.x) - i32::from(selection.bounds.left);
        let dy = i32::from(destination.y) - i32::from(selection.bounds.top);
        if layers.contains(&MapLayer::Terrain) {
            for (x, y) in shifted_selection_cells(selection, *destination) {
                check(MapLayer::Terrain, x, y);
            }
        }
        for unit in &content.units {
            for (x, y) in shifted_cells(&unit_cells_for_state(unit, catalog), dx, dy) {
                check(unit.layer, x, y);
            }
        }
        for doodad in &content.doodads {
            for (x, y) in shifted_cells(&doodad_cells_for_state(doodad, catalog), dx, dy) {
                check(MapLayer::Doodads, x, y);
                check(MapLayer::Terrain, x, y);
            }
            check(
                MapLayer::Sprites,
                shifted_tile(doodad.state.x / 32, dx),
                shifted_tile(doodad.state.y / 32, dy),
            );
        }
        for sprite in &content.sprites {
            check(
                MapLayer::Sprites,
                shifted_tile(sprite.state.x / 32, dx),
                shifted_tile(sprite.state.y / 32, dy),
            );
        }
        for location in &content.locations {
            for (x, y) in shifted_cells(&location_cells_unbounded(location), dx, dy) {
                check(MapLayer::Locations, x, y);
            }
        }
    }
    if policy == Some(StampCollisionPolicy::Replace) {
        for unit in destination_map
            .units
            .iter()
            .filter(|unit| collisions.units.contains(&unit.ordinal))
        {
            for (x, y) in unit_cells(unit, catalog, destination_map.width, destination_map.height) {
                check(unit.layer, x, y);
            }
        }
        for doodad in destination_map
            .doodads
            .iter()
            .filter(|doodad| collisions.doodads.contains(&doodad.ordinal))
        {
            for (x, y) in doodad_cells(
                doodad,
                catalog,
                destination_map.width,
                destination_map.height,
            ) {
                check(MapLayer::Doodads, x, y);
                check(MapLayer::Terrain, x, y);
            }
            check(MapLayer::Sprites, doodad.state.x / 32, doodad.state.y / 32);
        }
        for sprite in destination_map
            .sprites
            .iter()
            .filter(|sprite| collisions.sprites.contains(&sprite.ordinal))
        {
            check(MapLayer::Sprites, sprite.state.x / 32, sprite.state.y / 32);
        }
        for location in destination_map
            .locations
            .iter()
            .filter(|location| collisions.locations.contains(&location.state.location_id))
        {
            for (x, y) in location_cells(location, destination_map.width, destination_map.height) {
                check(MapLayer::Locations, x, y);
            }
        }
    }
    (outside.len() as u32, protected.len() as u32)
}

#[allow(clippy::too_many_arguments)]
fn build_operations(
    source: &ParsedMap,
    destination: &ParsedMap,
    catalog: &StampCatalog,
    selection: &SelectionMask,
    destinations: &[StampDestination],
    layers: &BTreeSet<MapLayer>,
    content: &SourceContent,
    collisions: &DestinationCollisions,
    policy: StampCollisionPolicy,
) -> Result<Vec<MapOperation>, String> {
    let mut operations = Vec::new();
    if policy == StampCollisionPolicy::Replace {
        let deleted_doodads = destination
            .doodads
            .iter()
            .filter(|item| collisions.doodads.contains(&item.ordinal))
            .cloned()
            .collect::<Vec<_>>();
        let mut overlay_counts = doodad_overlay_counts(&deleted_doodads, catalog);
        let mut sprites = Vec::new();
        for item in destination
            .sprites
            .iter()
            .filter(|item| collisions.sprites.contains(&item.ordinal))
        {
            if consume_overlay(&mut overlay_counts, &sprite_overlay_key(&item.state)) {
                continue;
            }
            sprites.push(item.clone());
        }
        sprites.sort_by_key(|item| std::cmp::Reverse(item.ordinal));
        operations.extend(sprites.into_iter().map(|item| MapOperation::SpriteDelete {
            ordinal: item.ordinal as u32,
            before_fingerprint: item.fingerprint,
        }));
        let mut doodads = deleted_doodads;
        doodads.sort_by_key(|item| std::cmp::Reverse(item.ordinal));
        for item in doodads {
            let replacement_tiles = doodad_replacement_tiles(destination, catalog, &item)?;
            operations.push(MapOperation::DoodadDelete {
                ordinal: item.ordinal as u32,
                before_fingerprint: item.fingerprint,
                replacement_tiles,
            });
        }
        let mut units = destination
            .units
            .iter()
            .filter(|item| collisions.units.contains(&item.ordinal))
            .cloned()
            .collect::<Vec<_>>();
        units.sort_by_key(|item| std::cmp::Reverse(item.ordinal));
        operations.extend(units.into_iter().map(|item| MapOperation::UnitDelete {
            ordinal: item.ordinal as u32,
            before_fingerprint: item.fingerprint,
        }));
        for location_id in collisions.locations.iter().rev() {
            operations.push(MapOperation::LocationDelete {
                location_id: *location_id,
            });
        }
    }

    if layers.contains(&MapLayer::Terrain) {
        for destination in destinations {
            for row in &selection.rows {
                for (left, right) in &row.spans {
                    let source_start =
                        usize::from(row.y) * usize::from(source.width) + usize::from(*left);
                    let source_end = source_start + usize::from(*right - *left);
                    let tiles = source
                        .tiles
                        .get(source_start..source_end)
                        .ok_or_else(|| "stamp source terrain is truncated".to_string())?
                        .to_vec();
                    operations.push(MapOperation::TerrainBlit {
                        x: destination.x + (*left - selection.bounds.left),
                        y: destination.y + (row.y - selection.bounds.top),
                        tiles: vec![tiles],
                    });
                }
            }
        }
    }

    for destination in destinations {
        let dx = i32::from(destination.x) - i32::from(selection.bounds.left);
        let dy = i32::from(destination.y) - i32::from(selection.bounds.top);
        for unit in &content.units {
            let mut state = unit.state.clone();
            state.x = shift_pixel_u16(state.x, dx)?;
            state.y = shift_pixel_u16(state.y, dy)?;
            operations.push(MapOperation::UnitAdd { state });
        }
        for sprite in &content.sprites {
            let mut state = sprite.state.clone();
            state.x = shift_pixel_u16(state.x, dx)?;
            state.y = shift_pixel_u16(state.y, dy)?;
            operations.push(MapOperation::SpriteAdd { state });
        }
        for doodad in &content.doodads {
            let mut state = doodad.state.clone();
            state.x = shift_pixel_u16(state.x, dx)?;
            state.y = shift_pixel_u16(state.y, dy)?;
            operations.push(MapOperation::DoodadAdd { state });
        }
        for location in &content.locations {
            let mut state = location.state.clone();
            state.location_id = 0;
            state.left = shift_pixel_i32(state.left, dx)?;
            state.right = shift_pixel_i32(state.right, dx)?;
            state.top = shift_pixel_i32(state.top, dy)?;
            state.bottom = shift_pixel_i32(state.bottom, dy)?;
            operations.push(MapOperation::LocationAdd { state });
        }
    }
    Ok(operations)
}

fn doodad_replacement_tiles(
    map: &ParsedMap,
    catalog: &StampCatalog,
    doodad: &DoodadRecord,
) -> Result<Vec<Vec<u16>>, String> {
    let metadata = catalog
        .doodads
        .get(&doodad.state.doodad_id)
        .ok_or_else(|| "doodad metadata is missing".to_string())?;
    let center_x = doodad.state.x / 32;
    let center_y = doodad.state.y / 32;
    let left = center_x
        .checked_sub(metadata.width / 2)
        .ok_or_else(|| "doodad footprint is outside map".to_string())?;
    let top = center_y
        .checked_sub(metadata.height / 2)
        .ok_or_else(|| "doodad footprint is outside map".to_string())?;
    let mut rows = Vec::new();
    for y in top..top + metadata.height {
        let start = usize::from(y) * usize::from(map.width) + usize::from(left);
        let end = start + usize::from(metadata.width);
        rows.push(
            map.tiles
                .get(start..end)
                .ok_or_else(|| "doodad replacement terrain is truncated".to_string())?
                .to_vec(),
        );
    }
    Ok(rows)
}

fn fully_contained(selection: &SelectionMask, cells: &[(u16, u16)]) -> bool {
    !cells.is_empty() && cells.iter().all(|(x, y)| selection.contains(*x, *y))
}

fn unit_cells(
    unit: &UnitRecord,
    catalog: &StampCatalog,
    width: u16,
    height: u16,
) -> Vec<(u16, u16)> {
    clip_cells(unit_cells_for_state(unit, catalog), width, height)
}

fn unit_cells_for_state(unit: &UnitRecord, catalog: &StampCatalog) -> Vec<(u16, u16)> {
    let extent = catalog
        .unit_extents
        .get(&unit.state.type_id)
        .copied()
        .unwrap_or(crate::map_verify::UnitExtent {
            left: 16,
            up: 16,
            right: 16,
            down: 16,
        });
    pixel_rect_cells_unbounded(
        i32::from(unit.state.x) - i32::from(extent.left),
        i32::from(unit.state.y) - i32::from(extent.up),
        i32::from(unit.state.x) + i32::from(extent.right),
        i32::from(unit.state.y) + i32::from(extent.down),
    )
}

fn doodad_cells(
    doodad: &DoodadRecord,
    catalog: &StampCatalog,
    width: u16,
    height: u16,
) -> Vec<(u16, u16)> {
    clip_cells(doodad_cells_for_state(doodad, catalog), width, height)
}

fn doodad_cells_for_state(doodad: &DoodadRecord, catalog: &StampCatalog) -> Vec<(u16, u16)> {
    let Some(metadata) = catalog.doodads.get(&doodad.state.doodad_id) else {
        return Vec::new();
    };
    let center_x = doodad.state.x / 32;
    let center_y = doodad.state.y / 32;
    let Some(left) = center_x.checked_sub(metadata.width / 2) else {
        return Vec::new();
    };
    let Some(top) = center_y.checked_sub(metadata.height / 2) else {
        return Vec::new();
    };
    (top..top.saturating_add(metadata.height))
        .flat_map(|y| (left..left.saturating_add(metadata.width)).map(move |x| (x, y)))
        .collect()
}

fn sprite_cells(sprite: &SpriteRecord, width: u16, height: u16) -> Vec<(u16, u16)> {
    let cell = (sprite.state.x / 32, sprite.state.y / 32);
    if cell.0 < width && cell.1 < height {
        vec![cell]
    } else {
        Vec::new()
    }
}

fn location_cells(location: &LocationRecord, width: u16, height: u16) -> Vec<(u16, u16)> {
    clip_cells(location_cells_unbounded(location), width, height)
}

fn location_cells_unbounded(location: &LocationRecord) -> Vec<(u16, u16)> {
    pixel_rect_cells_unbounded(
        location.state.left,
        location.state.top,
        location.state.right,
        location.state.bottom,
    )
}

fn pixel_rect_cells_unbounded(left: i32, top: i32, right: i32, bottom: i32) -> Vec<(u16, u16)> {
    let pixel_left = left.min(right);
    let pixel_right = left.max(right);
    let pixel_top = top.min(bottom);
    let pixel_bottom = top.max(bottom);
    let left = pixel_left.div_euclid(32).max(0);
    let right = pixel_right
        .saturating_add(31)
        .div_euclid(32)
        .max(left.saturating_add(1));
    let top = pixel_top.div_euclid(32).max(0);
    let bottom = pixel_bottom
        .saturating_add(31)
        .div_euclid(32)
        .max(top.saturating_add(1));
    (top..bottom)
        .flat_map(|y| {
            (left..right)
                .filter_map(move |x| Some((u16::try_from(x).ok()?, u16::try_from(y).ok()?)))
        })
        .collect()
}

fn clip_cells(cells: Vec<(u16, u16)>, width: u16, height: u16) -> Vec<(u16, u16)> {
    cells
        .into_iter()
        .filter(|(x, y)| *x < width && *y < height)
        .collect()
}

fn shifted_cells(cells: &[(u16, u16)], dx: i32, dy: i32) -> Vec<(u16, u16)> {
    cells
        .iter()
        .filter_map(|(x, y)| {
            Some((
                u16::try_from(i32::from(*x) + dx).ok()?,
                u16::try_from(i32::from(*y) + dy).ok()?,
            ))
        })
        .collect()
}

fn shifted_tile(value: u16, delta: i32) -> u16 {
    u16::try_from(i32::from(value) + delta).unwrap_or(u16::MAX)
}

fn shift_pixel_u16(value: u16, tile_delta: i32) -> Result<u16, String> {
    let shifted = i32::from(value)
        .checked_add(
            tile_delta
                .checked_mul(32)
                .ok_or_else(|| "stamp pixel offset overflowed".to_string())?,
        )
        .ok_or_else(|| "stamp pixel coordinate overflowed".to_string())?;
    u16::try_from(shifted).map_err(|_| "stamp object is outside map bounds".to_string())
}

fn shift_pixel_i32(value: i32, tile_delta: i32) -> Result<i32, String> {
    value
        .checked_add(
            tile_delta
                .checked_mul(32)
                .ok_or_else(|| "stamp pixel offset overflowed".to_string())?,
        )
        .ok_or_else(|| "stamp location coordinate overflowed".to_string())
}

type OverlayKey = (u16, u16, u16, u8, u16);

fn doodad_overlay_counts(
    doodads: &[DoodadRecord],
    catalog: &StampCatalog,
) -> BTreeMap<OverlayKey, usize> {
    let mut counts = BTreeMap::new();
    for doodad in doodads {
        let Some(metadata) = catalog.doodads.get(&doodad.state.doodad_id) else {
            continue;
        };
        let (Some(overlay_id), Some(overlay_flags)) = (metadata.overlay_id, metadata.overlay_flags)
        else {
            continue;
        };
        *counts
            .entry((
                overlay_id,
                doodad.state.x,
                doodad.state.y,
                doodad.state.owner,
                overlay_flags,
            ))
            .or_insert(0) += 1;
    }
    counts
}

fn consume_overlay(counts: &mut BTreeMap<OverlayKey, usize>, key: &OverlayKey) -> bool {
    let Some(count) = counts.get_mut(key) else {
        return false;
    };
    if *count == 0 {
        return false;
    }
    *count -= 1;
    true
}

fn sprite_overlay_key(state: &SpriteState) -> OverlayKey {
    (state.sprite_id, state.x, state.y, state.owner, state.flags)
}

fn raw_strings(sections: &BTreeMap<String, Vec<u8>>) -> Vec<Vec<u8>> {
    if let Some(data) = sections.get("STRx") {
        raw_string_section(data, 4)
    } else if let Some(data) = sections.get("STR ") {
        raw_string_section(data, 2)
    } else {
        Vec::new()
    }
}

fn raw_string_section(data: &[u8], width: usize) -> Vec<Vec<u8>> {
    if data.len() < width {
        return Vec::new();
    }
    let Some(raw_count) = read_offset(data, 0, width) else {
        return Vec::new();
    };
    let count = (raw_count as usize).min((data.len() - width) / width);
    (0..count)
        .map(|index| {
            let Some(offset) = read_offset(data, width * (index + 1), width) else {
                return Vec::new();
            };
            let offset = offset as usize;
            if offset == 0 || offset >= data.len() {
                return Vec::new();
            }
            let end = data[offset..]
                .iter()
                .position(|byte| *byte == 0)
                .map(|length| offset + length)
                .unwrap_or(data.len());
            data[offset..end].to_vec()
        })
        .collect()
}

fn read_offset(data: &[u8], offset: usize, width: usize) -> Option<u32> {
    match width {
        2 if offset + 2 <= data.len() => Some(u32::from(read_u16(data, offset))),
        4 if offset + 4 <= data.len() => Some(read_u32(data, offset)),
        _ => None,
    }
}

fn read_u16(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

fn read_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(data[offset..offset + 4].try_into().expect("four bytes"))
}

fn read_i32(data: &[u8], offset: usize) -> i32 {
    i32::from_le_bytes(data[offset..offset + 4].try_into().expect("four bytes"))
}

fn bytes_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn json_u16(value: &serde_json::Value, key: &str, kind: &str) -> Result<u16, String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u16::try_from(value).ok())
        .ok_or_else(|| format!("{kind} DAT catalog entry has invalid {key}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_model::{MaskGrid, RowSpan, SelectionRole};

    fn selection(layers: &[MapLayer]) -> SelectionMask {
        SelectionMask::canonical(
            "stamp",
            "영역 A",
            "r0:hash",
            SelectionRole::Target,
            layers.iter().copied().collect(),
            MaskGrid {
                width: 8,
                height: 8,
                rows: vec![
                    RowSpan {
                        y: 1,
                        spans: vec![(1, 3)],
                    },
                    RowSpan {
                        y: 2,
                        spans: vec![(1, 3)],
                    },
                ],
            },
        )
        .unwrap()
    }

    fn unit_state(x: u16, y: u16) -> UnitState {
        UnitState {
            type_id: 0,
            owner: 1,
            x,
            y,
            class_id: 7,
            relation_flags: 1,
            valid_state_flags: 2,
            valid_field_flags: 3,
            hp_percent: 90,
            shield_percent: 80,
            energy_percent: 70,
            resource_amount: 6,
            hangar_amount: 5,
            state_flags: 4,
            unused: 9,
            relation_class_id: 8,
        }
    }

    fn catalog() -> StampCatalog {
        StampCatalog {
            unit_extents: BTreeMap::from([(
                0,
                crate::map_verify::UnitExtent {
                    left: 8,
                    up: 8,
                    right: 8,
                    down: 8,
                },
            )]),
            building_ids: BTreeSet::new(),
            doodads: BTreeMap::from([
                (
                    1,
                    DoodadMetadata {
                        width: 1,
                        height: 1,
                        overlay_id: Some(99),
                        overlay_flags: Some(0x1000),
                    },
                ),
                (
                    2,
                    DoodadMetadata {
                        width: 2,
                        height: 1,
                        overlay_id: None,
                        overlay_flags: None,
                    },
                ),
            ]),
        }
    }

    fn source_map() -> ParsedMap {
        ParsedMap {
            width: 8,
            height: 8,
            tiles: (0..64).collect(),
            units: vec![UnitRecord {
                ordinal: 0,
                fingerprint: "unit".to_string(),
                layer: MapLayer::Units,
                state: unit_state(48, 48),
            }],
            doodads: vec![DoodadRecord {
                ordinal: 0,
                fingerprint: "doodad".to_string(),
                state: DoodadState {
                    doodad_id: 1,
                    x: 64,
                    y: 64,
                    owner: 11,
                    disabled: false,
                },
            }],
            sprites: vec![
                SpriteRecord {
                    ordinal: 0,
                    fingerprint: "overlay".to_string(),
                    state: SpriteState {
                        sprite_id: 99,
                        x: 64,
                        y: 64,
                        owner: 11,
                        flags: 0x1000,
                    },
                },
                SpriteRecord {
                    ordinal: 1,
                    fingerprint: "same-position-independent-sprite".to_string(),
                    state: SpriteState {
                        sprite_id: 99,
                        x: 64,
                        y: 64,
                        owner: 11,
                        flags: 0x1000,
                    },
                },
                SpriteRecord {
                    ordinal: 2,
                    fingerprint: "sprite".to_string(),
                    state: SpriteState {
                        sprite_id: 5,
                        x: 48,
                        y: 48,
                        owner: 2,
                        flags: 0x1000,
                    },
                },
            ],
            locations: vec![LocationRecord {
                state: LocationState {
                    location_id: 1,
                    left: 32,
                    top: 32,
                    right: 64,
                    bottom: 64,
                    elevation_flags: 3,
                    name_bytes_hex: Some("74657374".to_string()),
                },
            }],
            available_location_slots: 62,
        }
    }

    #[test]
    fn exact_stamp_uses_live_tiles_and_all_supported_layers_without_isom() {
        let selection = selection(&SUPPORTED_MAP_LAYERS);
        let catalog = catalog();
        let source = source_map();
        let destination = ParsedMap {
            units: Vec::new(),
            doodads: Vec::new(),
            sprites: Vec::new(),
            locations: Vec::new(),
            available_location_slots: 62,
            ..source.clone()
        };
        let layers = stamp_layers(&selection);
        let content = capture_source_content(&source, &catalog, &selection, &layers);
        assert_eq!(
            content.counts,
            StampLayerCounts {
                units: 1,
                buildings: 0,
                doodads: 1,
                sprites: 2,
                locations: 1,
            }
        );
        let operations = build_operations(
            &source,
            &destination,
            &catalog,
            &selection,
            &[StampDestination { x: 4, y: 4 }],
            &layers,
            &content,
            &DestinationCollisions::default(),
            StampCollisionPolicy::Merge,
        )
        .unwrap();
        assert_eq!(
            operations
                .iter()
                .filter(|operation| matches!(operation, MapOperation::TerrainBlit { .. }))
                .count(),
            2
        );
        assert!(!operations
            .iter()
            .any(|operation| matches!(operation, MapOperation::TerrainIsomBrush { .. })));
        assert_eq!(
            operations
                .iter()
                .filter(|operation| matches!(operation, MapOperation::UnitAdd { .. }))
                .count(),
            1
        );
        assert_eq!(
            operations
                .iter()
                .filter(|operation| matches!(operation, MapOperation::DoodadAdd { .. }))
                .count(),
            1
        );
        assert_eq!(
            operations
                .iter()
                .filter(|operation| matches!(operation, MapOperation::SpriteAdd { .. }))
                .count(),
            2,
            "exactly one doodad overlay must be regenerated by doodad.add"
        );
        let terrain = operations
            .iter()
            .find_map(|operation| match operation {
                MapOperation::TerrainBlit { x, y, tiles } => Some((*x, *y, tiles.clone())),
                _ => None,
            })
            .unwrap();
        assert_eq!(terrain, (4, 4, vec![vec![9, 10]]));
        let unit = operations
            .iter()
            .find_map(|operation| match operation {
                MapOperation::UnitAdd { state } => Some(state),
                _ => None,
            })
            .unwrap();
        assert_eq!((unit.x, unit.y), (144, 144));
        let location = operations
            .iter()
            .find_map(|operation| match operation {
                MapOperation::LocationAdd { state } => Some(state),
                _ => None,
            })
            .unwrap();
        assert_eq!(location.location_id, 0);
        assert_eq!(
            (location.left, location.top, location.right, location.bottom),
            (128, 128, 160, 160)
        );
        assert_eq!(location.name_bytes_hex.as_deref(), Some("74657374"));
    }

    #[test]
    fn collision_scan_distinguishes_fully_contained_and_boundary_crossing_objects() {
        let selection = selection(&[MapLayer::Doodads]);
        let catalog = catalog();
        let map = ParsedMap {
            width: 8,
            height: 8,
            tiles: vec![0; 64],
            units: Vec::new(),
            doodads: vec![
                DoodadRecord {
                    ordinal: 0,
                    fingerprint: "inside".to_string(),
                    state: DoodadState {
                        doodad_id: 1,
                        x: 128,
                        y: 128,
                        owner: 11,
                        disabled: false,
                    },
                },
                DoodadRecord {
                    ordinal: 1,
                    fingerprint: "partial".to_string(),
                    state: DoodadState {
                        doodad_id: 2,
                        x: 128,
                        y: 160,
                        owner: 11,
                        disabled: false,
                    },
                },
            ],
            sprites: Vec::new(),
            locations: Vec::new(),
            available_location_slots: 63,
        };
        let collisions = destination_collisions(
            &map,
            &catalog,
            &selection,
            &[StampDestination { x: 4, y: 4 }],
            &stamp_layers(&selection),
        );
        assert_eq!(collisions.doodads, BTreeSet::from([0, 1]));
        assert_eq!(collisions.partial_doodads, BTreeSet::from([1]));
        let counts = partial_collision_counts(&map, &collisions);
        assert_eq!(counts.doodads, 1);
    }

    #[test]
    fn destinations_must_fit_and_must_not_overlap() {
        let selection = selection(&[MapLayer::Terrain]);
        assert!(
            validate_destinations(&selection, &[StampDestination { x: 6, y: 6 }], 8, 8).is_ok()
        );
        assert!(validate_destinations(
            &selection,
            &[
                StampDestination { x: 4, y: 4 },
                StampDestination { x: 5, y: 5 },
            ],
            8,
            8,
        )
        .unwrap_err()
        .contains("overlap"));
        assert!(
            validate_destinations(&selection, &[StampDestination { x: 7, y: 7 }], 8, 8,)
                .unwrap_err()
                .contains("outside")
        );
    }

    #[test]
    fn persistent_selection_rebinds_to_each_visible_candidate_revision() {
        let selection = selection(&[MapLayer::Terrain, MapLayer::Units]);
        let persistent = PersistentSelection::from_selection(&selection);
        let rebound = persistent.bind("r7:new", 8, 8).unwrap();
        assert_eq!(rebound.source_revision, "r7:new");
        assert_eq!(rebound.cells(), selection.cells());
        assert_eq!(rebound.layers, selection.layers);
    }
}
