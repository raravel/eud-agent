use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::map_model::{
    hex_sha256, LayerDiffCount, MapDiff, MapLayer, SelectionMask, SelectionRole, TileRect,
    VerificationReport,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapRequestAuthority {
    pub session_id: String,
    pub request_id: String,
    pub parent_revision: u32,
    #[serde(default)]
    pub map_width: u16,
    #[serde(default)]
    pub map_height: u16,
    pub target_masks: Vec<SelectionMask>,
    pub forbidden_masks: Vec<SelectionMask>,
}

pub const SUPPORTED_MAP_LAYERS: [MapLayer; 6] = [
    MapLayer::Terrain,
    MapLayer::Units,
    MapLayer::Buildings,
    MapLayer::Doodads,
    MapLayer::Sprites,
    MapLayer::Locations,
];

impl MapRequestAuthority {
    pub fn calculate(
        session_id: String,
        request_id: String,
        parent_revision: u32,
        map_width: u16,
        map_height: u16,
        target_masks: Vec<SelectionMask>,
        forbidden_masks: Vec<SelectionMask>,
    ) -> Result<Self, String> {
        if map_width == 0 || map_height == 0 {
            return Err("Map request authority requires non-empty map dimensions".to_string());
        }
        if target_masks
            .iter()
            .any(|mask| mask.role != SelectionRole::Target)
        {
            return Err("Map request target authority contains a non-target mask".to_string());
        }
        if forbidden_masks
            .iter()
            .any(|mask| mask.role != SelectionRole::Protect)
        {
            return Err("Map request protection authority contains a non-protect mask".to_string());
        }
        Ok(Self {
            session_id,
            request_id,
            parent_revision,
            map_width,
            map_height,
            target_masks,
            forbidden_masks,
        })
    }

    pub fn allows(&self, layer: MapLayer, x: u16, y: u16) -> bool {
        if x >= self.map_width || y >= self.map_height || !SUPPORTED_MAP_LAYERS.contains(&layer) {
            return false;
        }
        self.target_masks.is_empty()
            || self
                .target_masks
                .iter()
                .any(|mask| mask.layers.contains(&layer) && mask.contains(x, y))
    }

    pub fn forbids(&self, layer: MapLayer, x: u16, y: u16) -> bool {
        self.forbidden_masks.iter().any(|mask| {
            (mask.layers.is_empty() || mask.layers.contains(&layer)) && mask.contains(x, y)
        })
    }
}

#[derive(Clone, Default)]
pub struct MapVerificationService;

#[derive(Debug, Clone, Copy)]
struct UnitPlacement {
    type_id: u16,
    x: u16,
    y: u16,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct UnitExtent {
    pub(crate) left: u16,
    pub(crate) up: u16,
    pub(crate) right: u16,
    pub(crate) down: u16,
}
struct VerificationFailure {
    errors: Vec<String>,
    diff: MapDiff,
    candidate_sha256: String,
    canonical_digest: String,
    extra_assets_digest: String,
}

impl MapVerificationService {
    pub fn verify(
        &self,
        baseline_path: &Path,
        candidate_path: &Path,
        authority: &MapRequestAuthority,
        starcraft_path: &Path,
        native_report: Option<&serde_json::Value>,
    ) -> VerificationReport {
        match self.verify_inner(
            baseline_path,
            candidate_path,
            authority,
            starcraft_path,
            native_report,
        ) {
            Ok(report) => report,
            Err(failure) => VerificationReport {
                valid: false,
                errors: failure.errors,
                warnings: Vec::new(),
                diff: failure.diff,
                candidate_sha256: failure.candidate_sha256,
                canonical_digest: failure.canonical_digest,
                extra_assets_digest: failure.extra_assets_digest,
            },
        }
    }

    fn verify_inner(
        &self,
        baseline_path: &Path,
        candidate_path: &Path,
        authority: &MapRequestAuthority,
        starcraft_path: &Path,
        native_report: Option<&serde_json::Value>,
    ) -> Result<VerificationReport, Box<VerificationFailure>> {
        let candidate_bytes = match std::fs::read(candidate_path) {
            Ok(bytes) => bytes,
            Err(error) => {
                return Err(Box::new(VerificationFailure {
                    errors: vec![format!("candidate draft could not be read: {error}")],
                    diff: MapDiff::default(),
                    candidate_sha256: String::new(),
                    canonical_digest: String::new(),
                    extra_assets_digest: String::new(),
                }))
            }
        };
        let candidate_sha256 = hex_sha256(&candidate_bytes);
        if candidate_bytes.is_empty() {
            return Err(Box::new(VerificationFailure {
                errors: vec!["candidate draft is empty".to_string()],
                diff: MapDiff::default(),
                candidate_sha256,
                canonical_digest: String::new(),
                extra_assets_digest: String::new(),
            }));
        }
        let baseline_chk = match isom::chk_extract(baseline_path) {
            Ok(chk) => chk,
            Err(error) => {
                return Err(Box::new(VerificationFailure {
                    errors: vec![format!("baseline SCX is not parseable: {error}")],
                    diff: MapDiff::default(),
                    candidate_sha256,
                    canonical_digest: String::new(),
                    extra_assets_digest: String::new(),
                }))
            }
        };
        let candidate_chk = match isom::chk_extract(candidate_path) {
            Ok(chk) => chk,
            Err(error) => {
                return Err(Box::new(VerificationFailure {
                    errors: vec![format!("candidate SCX is not parseable: {error}")],
                    diff: MapDiff::default(),
                    candidate_sha256,
                    canonical_digest: String::new(),
                    extra_assets_digest: String::new(),
                }))
            }
        };
        let baseline_sections =
            crate::chk::assemble_sections(&crate::chk::walk_sections(&baseline_chk));
        let candidate_sections =
            crate::chk::assemble_sections(&crate::chk::walk_sections(&candidate_chk));
        let canonical = crate::chk::canonical_chk_digest(&candidate_chk);
        let mut errors = Vec::<String>::new();
        let mut diff = MapDiff::default();

        for section in ["DIM ", "ERA "] {
            if baseline_sections.get(section) != candidate_sections.get(section) {
                errors.push(format!(
                    "{section} changed; map dimensions and tileset are immutable"
                ));
            }
        }
        let candidate_mtxm = candidate_sections
            .get("MTXM")
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let candidate_tile = candidate_sections
            .get("TILE")
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        if candidate_mtxm.len() % 2 != 0 || candidate_mtxm.is_empty() {
            errors.push("candidate MTXM is missing or truncated".to_string());
        }
        if candidate_mtxm != candidate_tile {
            errors.push("candidate MTXM and TILE are not identical".to_string());
        }

        let baseline_digest = crate::chk::digest_chk(&baseline_chk);
        let candidate_digest = crate::chk::digest_chk(&candidate_chk);
        let width = candidate_digest.map.width;
        let height = candidate_digest.map.height;
        if width == 0 || height == 0 {
            errors.push("candidate dimensions are empty".to_string());
        }

        let baseline_tiles = &baseline_digest.tiles;
        let candidate_tiles = &candidate_digest.tiles;
        let mut terrain_cells = Vec::<(u16, u16)>::new();
        if baseline_tiles.len() != candidate_tiles.len() {
            errors.push("candidate terrain cell count changed".to_string());
        } else {
            for (index, (&before, &after)) in baseline_tiles.iter().zip(candidate_tiles).enumerate()
            {
                if before == after {
                    continue;
                }
                let x = (index % usize::from(width)) as u16;
                let y = (index / usize::from(width)) as u16;
                terrain_cells.push((x, y));
                self.check_cell(&mut errors, &mut diff, authority, MapLayer::Terrain, x, y);
            }
        }
        diff.terrain_cells = terrain_cells.len() as u32;
        diff.terrain_bounds = bounds(&terrain_cells);

        let (building_ids, extents) =
            unit_catalog(starcraft_path, candidate_digest.map.tileset.as_str()).map_err(
                |error| {
                    Box::new(VerificationFailure {
                        errors: vec![error],
                        diff: diff.clone(),
                        candidate_sha256: candidate_sha256.clone(),
                        canonical_digest: canonical.overall_sha256.clone(),
                        extra_assets_digest: String::new(),
                    })
                },
            )?;
        let baseline_units = raw_entries(&baseline_sections, "UNIT", crate::chk::UNIT_ENTRY_SIZE);
        let candidate_units = raw_entries(&candidate_sections, "UNIT", crate::chk::UNIT_ENTRY_SIZE);
        let unit_changes = compare_objects(
            &baseline_units,
            &candidate_units,
            &mut diff.units,
            &mut diff.buildings,
            |bytes| {
                let class_id = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
                if class_id != 0 {
                    format!("class:{class_id}")
                } else {
                    format!(
                        "unit:{}:{}:{}",
                        u16::from_le_bytes([bytes[8], bytes[9]]),
                        bytes[16],
                        u32::from_le_bytes(bytes[32..36].try_into().unwrap())
                    )
                }
            },
            |bytes| {
                let placement = unit_placement(bytes);
                let building = building_ids.contains(&placement.type_id);
                let layer = if building {
                    MapLayer::Buildings
                } else {
                    MapLayer::Units
                };
                let extent = extents
                    .get(&placement.type_id)
                    .copied()
                    .unwrap_or(UnitExtent {
                        left: 16,
                        up: 16,
                        right: 16,
                        down: 16,
                    });
                let cells = pixel_rect_cells(
                    i32::from(placement.x) - i32::from(extent.left),
                    i32::from(placement.y) - i32::from(extent.up),
                    i32::from(placement.x) + i32::from(extent.right),
                    i32::from(placement.y) + i32::from(extent.down),
                    width,
                    height,
                );
                (layer, cells)
            },
        );
        for (layer, cells) in unit_changes {
            for (x, y) in cells {
                self.check_cell(&mut errors, &mut diff, authority, layer, x, y);
            }
        }

        let baseline_doodads = raw_entries(&baseline_sections, "DD2 ", crate::chk::DD2_ENTRY_SIZE);
        let candidate_doodads =
            raw_entries(&candidate_sections, "DD2 ", crate::chk::DD2_ENTRY_SIZE);
        let mut unused = LayerDiffCount::default();
        let doodad_changes = compare_objects(
            &baseline_doodads,
            &candidate_doodads,
            &mut diff.doodads,
            &mut unused,
            |bytes| {
                format!(
                    "doodad:{}:{}",
                    u16::from_le_bytes([bytes[0], bytes[1]]),
                    bytes[6]
                )
            },
            |bytes| {
                let x = u16::from_le_bytes([bytes[2], bytes[3]]);
                let y = u16::from_le_bytes([bytes[4], bytes[5]]);
                (MapLayer::Doodads, vec![(x / 32, y / 32)])
            },
        );
        for (layer, cells) in doodad_changes {
            for (x, y) in cells {
                self.check_cell(&mut errors, &mut diff, authority, layer, x, y);
            }
        }

        let baseline_sprites = raw_entries(&baseline_sections, "THG2", crate::chk::THG2_ENTRY_SIZE);
        let candidate_sprites =
            raw_entries(&candidate_sections, "THG2", crate::chk::THG2_ENTRY_SIZE);
        let sprite_changes = compare_objects(
            &baseline_sprites,
            &candidate_sprites,
            &mut diff.sprites,
            &mut unused,
            |bytes| {
                format!(
                    "sprite:{}:{}:{}",
                    u16::from_le_bytes([bytes[0], bytes[1]]),
                    bytes[6],
                    u16::from_le_bytes([bytes[8], bytes[9]]) & 0x1000
                )
            },
            |bytes| {
                let x = u16::from_le_bytes([bytes[2], bytes[3]]);
                let y = u16::from_le_bytes([bytes[4], bytes[5]]);
                (MapLayer::Sprites, vec![(x / 32, y / 32)])
            },
        );
        for (layer, cells) in sprite_changes {
            for (x, y) in cells {
                self.check_cell(&mut errors, &mut diff, authority, layer, x, y);
            }
        }

        let baseline_locations =
            raw_entries(&baseline_sections, "MRGN", crate::chk::MRGN_ENTRY_SIZE);
        let candidate_locations =
            raw_entries(&candidate_sections, "MRGN", crate::chk::MRGN_ENTRY_SIZE);
        if baseline_locations.len() != candidate_locations.len() {
            errors
                .push("MRGN slot count changed; location IDs would not remain stable".to_string());
        }
        for index in 0..baseline_locations.len().max(candidate_locations.len()) {
            let before = baseline_locations.get(index);
            let after = candidate_locations.get(index);
            if before == after {
                continue;
            }
            if index == 63 {
                errors.push("location #64 Anywhere changed".to_string());
            }
            match (before, after) {
                (Some(before), Some(after)) => {
                    let before_blank = location_blank(before);
                    let after_blank = location_blank(after);
                    if before_blank && !after_blank {
                        diff.locations.added += 1;
                    } else if !before_blank && after_blank {
                        diff.locations.removed += 1;
                    } else {
                        diff.locations.changed += 1;
                    }
                    for (x, y) in location_cells(after, width, height)
                        .into_iter()
                        .chain(location_cells(before, width, height))
                    {
                        self.check_cell(
                            &mut errors,
                            &mut diff,
                            authority,
                            MapLayer::Locations,
                            x,
                            y,
                        );
                    }
                }
                (None, Some(after)) => {
                    diff.locations.added += 1;
                    for (x, y) in location_cells(after, width, height) {
                        self.check_cell(
                            &mut errors,
                            &mut diff,
                            authority,
                            MapLayer::Locations,
                            x,
                            y,
                        );
                    }
                }
                (Some(before), None) => {
                    diff.locations.removed += 1;
                    for (x, y) in location_cells(before, width, height) {
                        self.check_cell(
                            &mut errors,
                            &mut diff,
                            authority,
                            MapLayer::Locations,
                            x,
                            y,
                        );
                    }
                }
                (None, None) => {}
            }
        }

        let baseline_canonical = crate::chk::canonical_chk_digest(&baseline_chk);
        for (name, baseline_hash) in &baseline_canonical.unsupported_hashes {
            if canonical.unsupported_hashes.get(name) != Some(baseline_hash) {
                diff.unsupported_section_changes.push(name.clone());
            }
        }
        for name in canonical.unsupported_hashes.keys() {
            if !baseline_canonical.unsupported_hashes.contains_key(name) {
                diff.unsupported_section_changes.push(name.clone());
            }
        }
        diff.unsupported_section_changes.sort();
        diff.unsupported_section_changes.dedup();
        if !diff.unsupported_section_changes.is_empty() {
            errors.push(format!(
                "unsupported CHK sections changed: {}",
                diff.unsupported_section_changes.join(", ")
            ));
        }

        let baseline_container = container_digest(baseline_path);
        let candidate_container = container_digest(candidate_path);
        let extra_assets_digest = candidate_container
            .as_ref()
            .and_then(|value| value.pointer("/extraAssets/digest"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        if baseline_container
            .as_ref()
            .and_then(|value| value.pointer("/extraAssets/digest"))
            != candidate_container
                .as_ref()
                .and_then(|value| value.pointer("/extraAssets/digest"))
        {
            errors.push("extra MPQ asset inventory or bytes changed".to_string());
        }
        if let Some(report) = native_report {
            if report
                .get("outputSha256")
                .and_then(serde_json::Value::as_str)
                != Some(candidate_sha256.as_str())
            {
                errors.push("native report output hash does not match candidate bytes".to_string());
            }
            if report
                .get("extraAssetsDigest")
                .and_then(serde_json::Value::as_str)
                != Some(extra_assets_digest.as_str())
            {
                errors.push(
                    "native report asset digest does not match independent digest".to_string(),
                );
            }
        }

        if errors.is_empty() {
            Ok(VerificationReport {
                valid: true,
                errors,
                warnings: Vec::new(),
                diff,
                candidate_sha256,
                canonical_digest: canonical.overall_sha256,
                extra_assets_digest,
            })
        } else {
            Err(Box::new(VerificationFailure {
                errors,
                diff,
                candidate_sha256,
                canonical_digest: canonical.overall_sha256,
                extra_assets_digest,
            }))
        }
    }

    fn check_cell(
        &self,
        errors: &mut Vec<String>,
        diff: &mut MapDiff,
        authority: &MapRequestAuthority,
        layer: MapLayer,
        x: u16,
        y: u16,
    ) {
        if authority.forbids(layer, x, y) {
            diff.protected += 1;
            errors.push(format!(
                "{layer:?} change at ({x},{y}) intersects a protected mask"
            ));
        }
        if !authority.allows(layer, x, y) {
            diff.outside_target += 1;
            errors.push(format!(
                "{layer:?} change at ({x},{y}) is outside the current request authority"
            ));
        }
    }
}

fn bounds(cells: &[(u16, u16)]) -> Option<TileRect> {
    Some(TileRect {
        left: cells.iter().map(|cell| cell.0).min()?,
        top: cells.iter().map(|cell| cell.1).min()?,
        right: cells.iter().map(|cell| cell.0).max()?.saturating_add(1),
        bottom: cells.iter().map(|cell| cell.1).max()?.saturating_add(1),
    })
}

fn raw_entries<'a>(
    sections: &'a BTreeMap<String, Vec<u8>>,
    name: &str,
    size: usize,
) -> Vec<&'a [u8]> {
    sections
        .get(name)
        .map(|bytes| bytes.chunks_exact(size).collect())
        .unwrap_or_default()
}

fn compare_objects<I, F>(
    before: &[&[u8]],
    after: &[&[u8]],
    primary: &mut LayerDiffCount,
    secondary: &mut LayerDiffCount,
    identity: I,
    describe: F,
) -> Vec<(MapLayer, Vec<(u16, u16)>)>
where
    I: Fn(&[u8]) -> String,
    F: Fn(&[u8]) -> (MapLayer, Vec<(u16, u16)>),
{
    let mut changes = Vec::new();
    let mut used_before = vec![false; before.len()];
    let mut used_after = vec![false; after.len()];

    for (before_index, before_item) in before.iter().enumerate() {
        if let Some(after_index) = after
            .iter()
            .enumerate()
            .find(|(index, after_item)| !used_after[*index] && *after_item == before_item)
            .map(|(index, _)| index)
        {
            used_before[before_index] = true;
            used_after[after_index] = true;
        }
    }

    for (before_index, before_item) in before.iter().enumerate() {
        if used_before[before_index] {
            continue;
        }
        let key = identity(before_item);
        let Some(after_index) = after
            .iter()
            .enumerate()
            .find(|(index, after_item)| !used_after[*index] && identity(after_item) == key)
            .map(|(index, _)| index)
        else {
            continue;
        };
        used_before[before_index] = true;
        used_after[after_index] = true;
        let (before_layer, before_cells) = describe(before_item);
        let (after_layer, after_cells) = describe(after[after_index]);
        let moved = before_layer == after_layer && before_cells != after_cells;
        if after_layer == MapLayer::Buildings {
            if moved {
                secondary.moved += 1;
            } else {
                secondary.changed += 1;
            }
        } else if moved {
            primary.moved += 1;
        } else {
            primary.changed += 1;
        }
        changes.push((before_layer, before_cells));
        changes.push((after_layer, after_cells));
    }

    for (index, item) in after.iter().enumerate() {
        if used_after[index] {
            continue;
        }
        let (layer, cells) = describe(item);
        if layer == MapLayer::Buildings {
            secondary.added += 1;
        } else {
            primary.added += 1;
        }
        changes.push((layer, cells));
    }
    for (index, item) in before.iter().enumerate() {
        if used_before[index] {
            continue;
        }
        let (layer, cells) = describe(item);
        if layer == MapLayer::Buildings {
            secondary.removed += 1;
        } else {
            primary.removed += 1;
        }
        changes.push((layer, cells));
    }
    changes
}

fn unit_placement(bytes: &[u8]) -> UnitPlacement {
    UnitPlacement {
        x: u16::from_le_bytes([bytes[4], bytes[5]]),
        y: u16::from_le_bytes([bytes[6], bytes[7]]),
        type_id: u16::from_le_bytes([bytes[8], bytes[9]]),
    }
}

pub(crate) fn unit_catalog(
    starcraft_path: &Path,
    tileset_name: &str,
) -> Result<(BTreeSet<u16>, BTreeMap<u16, UnitExtent>), String> {
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
    .ok_or_else(|| format!("unknown map tileset in unit catalog: {tileset_name}"))?;
    let mut building_ids = BTreeSet::new();
    let mut extents = BTreeMap::new();
    for kind in ["units", "buildings"] {
        let request = json!({
            "schema": "eud-map-catalog/1",
            "kind": kind,
            "tileset": tileset,
            "offset": 0,
            "limit": 512
        });
        let result = isom::catalog_query(starcraft_path, request.to_string().as_bytes()).map_err(
            |error| {
                format!("{kind} DAT catalog is unavailable; verification failed closed: {error}")
            },
        )?;
        let value = serde_json::from_str::<serde_json::Value>(&result)
            .map_err(|error| format!("{kind} DAT catalog response is invalid: {error}"))?;
        let entries = value
            .get("entries")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| format!("{kind} DAT catalog has no entries array"))?;
        if entries.is_empty() {
            return Err(format!("{kind} DAT catalog is empty"));
        }
        for entry in entries {
            let id = entry
                .get("id")
                .and_then(serde_json::Value::as_u64)
                .and_then(|id| u16::try_from(id).ok())
                .ok_or_else(|| format!("{kind} DAT catalog contains an invalid id"))?;
            if kind == "buildings" {
                building_ids.insert(id);
            }
            extents.insert(
                id,
                UnitExtent {
                    left: json_u16(entry, "extentLeft", kind)?,
                    up: json_u16(entry, "extentUp", kind)?,
                    right: json_u16(entry, "extentRight", kind)?,
                    down: json_u16(entry, "extentDown", kind)?,
                },
            );
        }
    }
    if extents.len() != 228 || building_ids.is_empty() {
        return Err(format!(
            "DAT catalog is incomplete: {} of 228 unit types, {} buildings",
            extents.len(),
            building_ids.len()
        ));
    }
    Ok((building_ids, extents))
}

fn json_u16(value: &serde_json::Value, key: &str, kind: &str) -> Result<u16, String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u16::try_from(value).ok())
        .ok_or_else(|| format!("{kind} DAT catalog entry has invalid {key}"))
}

fn pixel_rect_cells(
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
    width: u16,
    height: u16,
) -> Vec<(u16, u16)> {
    let pixel_left = left.min(right);
    let pixel_right = left.max(right);
    let pixel_top = top.min(bottom);
    let pixel_bottom = top.max(bottom);
    let left = pixel_left.div_euclid(32).clamp(0, i32::from(width));
    let right = pixel_right
        .saturating_add(31)
        .div_euclid(32)
        .max(left.saturating_add(1))
        .clamp(0, i32::from(width));
    let top = pixel_top.div_euclid(32).clamp(0, i32::from(height));
    let bottom = pixel_bottom
        .saturating_add(31)
        .div_euclid(32)
        .max(top.saturating_add(1))
        .clamp(0, i32::from(height));
    (top..bottom)
        .flat_map(|y| (left..right).map(move |x| (x as u16, y as u16)))
        .collect()
}

fn location_blank(bytes: &[u8]) -> bool {
    bytes.iter().all(|byte| *byte == 0)
}

fn location_cells(bytes: &[u8], width: u16, height: u16) -> Vec<(u16, u16)> {
    if location_blank(bytes) {
        return Vec::new();
    }
    let left = i32::from_le_bytes(bytes[0..4].try_into().unwrap());
    let top = i32::from_le_bytes(bytes[4..8].try_into().unwrap());
    let right = i32::from_le_bytes(bytes[8..12].try_into().unwrap());
    let bottom = i32::from_le_bytes(bytes[12..16].try_into().unwrap());
    pixel_rect_cells(left, top, right, bottom, width, height)
}

fn container_digest(path: &Path) -> Option<serde_json::Value> {
    isom::map_digest(path)
        .ok()
        .and_then(|value| serde_json::from_str(&value).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_model::{MapLayer, MaskGrid, RowSpan, SelectionMask, SelectionRole};

    fn mask(role: SelectionRole, layers: &[MapLayer], rows: Vec<RowSpan>) -> SelectionMask {
        SelectionMask::canonical(
            format!("{role:?}"),
            format!("{role:?}"),
            "r0",
            role,
            layers.iter().copied().collect(),
            MaskGrid {
                width: 128,
                height: 128,
                rows,
            },
        )
        .unwrap()
    }
    fn authority(
        targets: Vec<SelectionMask>,
        protections: Vec<SelectionMask>,
    ) -> MapRequestAuthority {
        MapRequestAuthority::calculate(
            "map".to_string(),
            "request".to_string(),
            0,
            128,
            128,
            targets,
            protections,
        )
        .unwrap()
    }

    #[test]
    fn no_target_authorizes_every_supported_layer_across_the_map() {
        let authority = authority(Vec::new(), Vec::new());
        for layer in SUPPORTED_MAP_LAYERS {
            assert!(authority.allows(layer, 0, 0), "{layer:?}");
            assert!(authority.allows(layer, 127, 127), "{layer:?}");
        }
        assert!(!authority.allows(MapLayer::Terrain, 128, 0));
        assert!(!authority.allows(MapLayer::Terrain, 0, 128));
    }

    #[test]
    fn target_narrows_layers_and_protect_wins_with_or_without_target() {
        let protect_all_layers = mask(
            SelectionRole::Protect,
            &[],
            vec![RowSpan {
                y: 2,
                spans: vec![(4, 5)],
            }],
        );
        let targeted = authority(
            vec![mask(
                SelectionRole::Target,
                &[MapLayer::Terrain],
                vec![RowSpan {
                    y: 2,
                    spans: vec![(2, 6)],
                }],
            )],
            vec![protect_all_layers.clone()],
        );
        assert!(targeted.allows(MapLayer::Terrain, 2, 2));
        assert!(!targeted.allows(MapLayer::Units, 2, 2));
        assert!(!targeted.allows(MapLayer::Terrain, 1, 2));
        assert!(targeted.forbids(MapLayer::Terrain, 4, 2));
        assert!(targeted.forbids(MapLayer::Units, 4, 2));

        let untargeted = authority(Vec::new(), vec![protect_all_layers]);
        assert!(untargeted.allows(MapLayer::Terrain, 4, 2));
        assert!(untargeted.forbids(MapLayer::Terrain, 4, 2));
    }

    #[test]
    fn protect_layers_are_respected_when_present() {
        let authority = authority(
            Vec::new(),
            vec![mask(
                SelectionRole::Protect,
                &[MapLayer::Terrain],
                vec![RowSpan {
                    y: 7,
                    spans: vec![(8, 9)],
                }],
            )],
        );
        assert!(authority.forbids(MapLayer::Terrain, 8, 7));
        assert!(!authority.forbids(MapLayer::Units, 8, 7));
    }

    #[test]
    fn fixture_noop_verification_preserves_every_digest() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("crates")
            .join("isom")
            .join("tests")
            .join("fixtures")
            .join("map_agent_rich.scx");
        let authority = authority(Vec::new(), Vec::new());
        let report = MapVerificationService.verify(
            &fixture,
            &fixture,
            &authority,
            Path::new(r"C:\Program Files (x86)\StarCraft"),
            None,
        );
        assert!(report.valid, "{:?}", report.errors);
        assert_eq!(report.diff, MapDiff::default());
    }

    #[test]
    fn missing_empty_and_unparseable_candidate_drafts_are_distinct() {
        let root = std::env::temp_dir().join(format!("map-verification-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("crates")
            .join("isom")
            .join("tests")
            .join("fixtures")
            .join("map_agent_rich.scx");
        let authority = authority(Vec::new(), Vec::new());
        let starcraft_path = Path::new(r"C:\Program Files (x86)\StarCraft");

        let missing = MapVerificationService.verify(
            &fixture,
            &root.join("missing.scx"),
            &authority,
            starcraft_path,
            None,
        );
        assert!(missing.errors[0].starts_with("candidate draft could not be read: "));
        assert!(missing.errors[0].contains("os error 2"));
        assert_eq!(missing.candidate_sha256, "");

        let empty_path = root.join("empty.scx");
        std::fs::write(&empty_path, []).unwrap();
        let empty =
            MapVerificationService.verify(&fixture, &empty_path, &authority, starcraft_path, None);
        assert_eq!(empty.errors, ["candidate draft is empty"]);
        assert_eq!(empty.candidate_sha256, hex_sha256(&[]));

        let invalid_path = root.join("invalid.scx");
        std::fs::write(&invalid_path, b"not an SCX").unwrap();
        let invalid = MapVerificationService.verify(
            &fixture,
            &invalid_path,
            &authority,
            starcraft_path,
            None,
        );
        assert!(invalid.errors[0].starts_with("candidate SCX is not parseable: "));
        assert_eq!(
            invalid.candidate_sha256,
            hex_sha256(std::fs::read(&invalid_path).unwrap().as_slice())
        );
        std::fs::remove_dir_all(root).ok();
    }
}
