use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

pub const MAP_EDIT_SCHEMA: &str = "eud-map-edit/1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tileset {
    Badlands,
    Platform,
    Installation,
    Ashworld,
    Jungle,
    Desert,
    Arctic,
    Twilight,
}

impl Tileset {
    pub fn from_era(value: u16) -> Result<Self, String> {
        match value & 0x7 {
            0 => Ok(Self::Badlands),
            1 => Ok(Self::Platform),
            2 => Ok(Self::Installation),
            3 => Ok(Self::Ashworld),
            4 => Ok(Self::Jungle),
            5 => Ok(Self::Desert),
            6 => Ok(Self::Arctic),
            7 => Ok(Self::Twilight),
            _ => Err(format!("unsupported tileset value {value}")),
        }
    }

    pub fn era(self) -> u16 {
        match self {
            Self::Badlands => 0,
            Self::Platform => 1,
            Self::Installation => 2,
            Self::Ashworld => 3,
            Self::Jungle => 4,
            Self::Desert => 5,
            Self::Arctic => 6,
            Self::Twilight => 7,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MapRevision {
    pub project_id: String,
    pub source_path: PathBuf,
    pub file_sha256: String,
    pub chk_sha256: String,
    #[serde(with = "u128_string")]
    pub mtime_ns: u128,
    pub tileset: Tileset,
    pub width: u16,
    pub height: u16,
}

pub(crate) mod u128_string {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &u128, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&value.to_string())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<u128, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SelectionRole {
    Target,
    Reference,
    Protect,
    Anchor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MapLayer {
    Terrain,
    Units,
    Buildings,
    Doodads,
    Sprites,
    Locations,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TileRect {
    pub left: u16,
    pub top: u16,
    pub right: u16,
    pub bottom: u16,
}

impl TileRect {
    pub fn contains(self, x: u16, y: u16) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RowSpan {
    pub y: u16,
    pub spans: Vec<(u16, u16)>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaskGrid {
    pub width: u16,
    pub height: u16,
    pub rows: Vec<RowSpan>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelectionMask {
    pub id: String,
    pub label: String,
    pub source_revision: String,
    pub role: SelectionRole,
    pub layers: BTreeSet<MapLayer>,
    pub bounds: TileRect,
    pub selected_cells: u32,
    pub rows: Vec<RowSpan>,
}

impl SelectionMask {
    pub fn canonical(
        id: impl Into<String>,
        label: impl Into<String>,
        source_revision: impl Into<String>,
        role: SelectionRole,
        layers: BTreeSet<MapLayer>,
        grid: MaskGrid,
    ) -> Result<Self, String> {
        let rows = canonical_rows(grid.width, grid.height, grid.rows)?;
        if rows.is_empty() {
            return Err("selection mask cannot be empty".to_string());
        }
        let selected_cells = rows.iter().flat_map(|row| row.spans.iter()).try_fold(
            0_u32,
            |total, (left, right)| {
                total
                    .checked_add(u32::from(right - left))
                    .ok_or_else(|| "selection cell count overflow".to_string())
            },
        )?;
        let left = rows
            .iter()
            .flat_map(|row| row.spans.iter().map(|span| span.0))
            .min()
            .expect("non-empty canonical rows have a left edge");
        let right = rows
            .iter()
            .flat_map(|row| row.spans.iter().map(|span| span.1))
            .max()
            .expect("non-empty canonical rows have a right edge");
        let top = rows.first().expect("non-empty canonical rows have a top").y;
        let bottom = rows
            .last()
            .expect("non-empty canonical rows have a bottom")
            .y
            .checked_add(1)
            .ok_or_else(|| "selection bottom overflow".to_string())?;
        Ok(Self {
            id: id.into(),
            label: label.into(),
            source_revision: source_revision.into(),
            role,
            layers,
            bounds: TileRect {
                left,
                top,
                right,
                bottom,
            },
            selected_cells,
            rows,
        })
    }

    pub fn contains(&self, x: u16, y: u16) -> bool {
        if !self.bounds.contains(x, y) {
            return false;
        }
        self.rows
            .binary_search_by_key(&y, |row| row.y)
            .ok()
            .is_some_and(|index| {
                self.rows[index]
                    .spans
                    .iter()
                    .any(|(left, right)| x >= *left && x < *right)
            })
    }

    pub fn snapshot_hash(&self) -> String {
        let bytes = serde_json::to_vec(self).expect("selection masks are serializable");
        hex_sha256(&bytes)
    }

    pub fn cells(&self) -> BTreeSet<(u16, u16)> {
        rows_to_cells(&self.rows)
    }
}

pub fn canonical_rows(width: u16, height: u16, rows: Vec<RowSpan>) -> Result<Vec<RowSpan>, String> {
    let mut by_row = BTreeMap::<u16, Vec<(u16, u16)>>::new();
    for row in rows {
        if row.y >= height {
            return Err(format!(
                "selection row {} exceeds map height {height}",
                row.y
            ));
        }
        for (left, right) in row.spans {
            if left >= right {
                return Err(format!(
                    "selection span [{left},{right}) is empty or reversed"
                ));
            }
            if right > width {
                return Err(format!(
                    "selection span [{left},{right}) exceeds map width {width}"
                ));
            }
            by_row.entry(row.y).or_default().push((left, right));
        }
    }

    let mut output = Vec::new();
    for (y, mut spans) in by_row {
        spans.sort_unstable();
        let mut merged = Vec::<(u16, u16)>::with_capacity(spans.len());
        for (left, right) in spans {
            if let Some(last) = merged.last_mut() {
                if left <= last.1 {
                    last.1 = last.1.max(right);
                    continue;
                }
            }
            merged.push((left, right));
        }
        if !merged.is_empty() {
            output.push(RowSpan { y, spans: merged });
        }
    }
    Ok(output)
}

pub fn rows_from_cells(cells: &BTreeSet<(u16, u16)>) -> Vec<RowSpan> {
    let mut rows = Vec::new();
    let mut current_y = None;
    let mut spans = Vec::<(u16, u16)>::new();
    for &(x, y) in cells {
        if current_y != Some(y) {
            if let Some(previous_y) = current_y {
                rows.push(RowSpan {
                    y: previous_y,
                    spans: std::mem::take(&mut spans),
                });
            }
            current_y = Some(y);
        }
        if let Some(last) = spans.last_mut() {
            if last.1 == x {
                last.1 = x.saturating_add(1);
                continue;
            }
        }
        spans.push((x, x.saturating_add(1)));
    }
    if let Some(y) = current_y {
        rows.push(RowSpan { y, spans });
    }
    rows
}

pub fn rows_to_cells(rows: &[RowSpan]) -> BTreeSet<(u16, u16)> {
    rows.iter()
        .flat_map(|row| {
            row.spans
                .iter()
                .flat_map(move |&(left, right)| (left..right).map(move |x| (x, row.y)))
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SelectionOperation {
    Replace,
    Add,
    Subtract,
    Invert,
    Clear,
}

pub fn combine_selection_cells(
    width: u16,
    height: u16,
    current: &BTreeSet<(u16, u16)>,
    incoming: &BTreeSet<(u16, u16)>,
    operation: SelectionOperation,
) -> Result<BTreeSet<(u16, u16)>, String> {
    let validate = |cells: &BTreeSet<(u16, u16)>| {
        cells.iter().try_for_each(|&(x, y)| {
            if x < width && y < height {
                Ok(())
            } else {
                Err(format!(
                    "selection cell ({x},{y}) is outside {width}x{height}"
                ))
            }
        })
    };
    validate(current)?;
    validate(incoming)?;
    Ok(match operation {
        SelectionOperation::Replace => incoming.clone(),
        SelectionOperation::Add => current.union(incoming).copied().collect(),
        SelectionOperation::Subtract => current.difference(incoming).copied().collect(),
        SelectionOperation::Invert => current.symmetric_difference(incoming).copied().collect(),
        SelectionOperation::Clear => BTreeSet::new(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MapObjectKind {
    Unit,
    Building,
    Doodad,
    Sprite,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapObjectRef {
    pub kind: MapObjectKind,
    pub ordinal: u32,
    pub semantic_fingerprint: String,
    pub revision_key: String,
    pub baseline_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ObjectMentionRole {
    Subject,
    Reference,
    Protect,
    Anchor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PaletteKind {
    SemanticTerrain,
    ExactTile,
    Unit,
    Building,
    Doodad,
    Sprite,
    NewLocation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PaletteRef {
    pub layer: MapLayer,
    pub kind: PaletteKind,
    pub entry_id: u32,
    pub tileset: Tileset,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelectionQualifierRef {
    pub selection_id: String,
    pub snapshot_hash: String,
    pub source_revision: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MentionQualifiers {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facing: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hp_percent: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shield_percent: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub energy_percent: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_amount: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invincible: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location_selection: Option<SelectionQualifierRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location_bounds: Option<TileRect>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum MapMentionSnapshot {
    Region {
        selection_id: String,
        snapshot_hash: String,
        source_revision: String,
    },
    Object {
        object_ref: MapObjectRef,
        role: ObjectMentionRole,
    },
    Palette {
        entry: PaletteRef,
        qualifiers: MentionQualifiers,
    },
    Stamp {
        selection_id: String,
        snapshot_hash: String,
    },
    Location {
        location_id: u16,
        revision_key: String,
        baseline_hash: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapEditExpected {
    pub input_file_sha256: String,
    pub tileset: Tileset,
    pub width: u16,
    pub height: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnitState {
    pub type_id: u16,
    pub owner: u8,
    pub x: u16,
    pub y: u16,
    #[serde(default)]
    pub class_id: u32,
    #[serde(default)]
    pub relation_flags: u16,
    #[serde(default)]
    pub valid_state_flags: u16,
    #[serde(default)]
    pub valid_field_flags: u16,
    #[serde(default = "one_hundred")]
    pub hp_percent: u8,
    #[serde(default = "one_hundred")]
    pub shield_percent: u8,
    #[serde(default = "one_hundred")]
    pub energy_percent: u8,
    #[serde(default)]
    pub resource_amount: u32,
    #[serde(default)]
    pub hangar_amount: u16,
    #[serde(default)]
    pub state_flags: u16,
    #[serde(default)]
    pub unused: u32,
    #[serde(default)]
    pub relation_class_id: u32,
}

const fn one_hundred() -> u8 {
    100
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnitPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_id: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relation_flags: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_state_flags: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_field_flags: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hp_percent: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shield_percent: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub energy_percent: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_amount: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hangar_amount: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_flags: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unused: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relation_class_id: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DoodadState {
    pub doodad_id: u16,
    pub x: u16,
    pub y: u16,
    #[serde(default = "neutral_owner")]
    pub owner: u8,
    #[serde(default)]
    pub disabled: bool,
}

const fn neutral_owner() -> u8 {
    11
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpriteState {
    pub sprite_id: u16,
    pub x: u16,
    pub y: u16,
    #[serde(default = "neutral_owner")]
    pub owner: u8,
    #[serde(default)]
    pub flags: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocationState {
    pub location_id: u16,
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    #[serde(default)]
    pub elevation_flags: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_bytes_hex: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all_fields = "camelCase", deny_unknown_fields)]
pub enum MapOperation {
    #[serde(rename = "terrain.set")]
    TerrainSet {
        x: u16,
        y: u16,
        before: u16,
        after: u16,
    },
    #[serde(rename = "terrain.rect")]
    TerrainRect {
        x: u16,
        y: u16,
        width: u16,
        height: u16,
        after: u16,
    },
    #[serde(rename = "terrain.blit")]
    TerrainBlit {
        x: u16,
        y: u16,
        tiles: Vec<Vec<u16>>,
    },
    #[serde(rename = "terrain.isom_brush")]
    TerrainIsomBrush {
        isom_x: u16,
        isom_y: u16,
        brush: u16,
        #[serde(default = "one")]
        extent: u16,
    },
    #[serde(rename = "unit.add")]
    UnitAdd { state: UnitState },
    #[serde(rename = "unit.set")]
    UnitSet {
        ordinal: u32,
        before_fingerprint: String,
        state: UnitPatch,
    },
    #[serde(rename = "unit.delete")]
    UnitDelete {
        ordinal: u32,
        before_fingerprint: String,
    },
    #[serde(rename = "unit.move")]
    UnitMove {
        ordinal: u32,
        before_fingerprint: String,
        x: u16,
        y: u16,
    },
    #[serde(rename = "doodad.add")]
    DoodadAdd { state: DoodadState },
    #[serde(rename = "doodad.set")]
    DoodadSet {
        ordinal: u32,
        before_fingerprint: String,
        state: DoodadState,
        replacement_tiles: Vec<Vec<u16>>,
    },
    #[serde(rename = "doodad.delete")]
    DoodadDelete {
        ordinal: u32,
        before_fingerprint: String,
        replacement_tiles: Vec<Vec<u16>>,
    },
    #[serde(rename = "doodad.move")]
    DoodadMove {
        ordinal: u32,
        before_fingerprint: String,
        x: u16,
        y: u16,
        replacement_tiles: Vec<Vec<u16>>,
    },
    #[serde(rename = "sprite.add")]
    SpriteAdd { state: SpriteState },
    #[serde(rename = "sprite.set")]
    SpriteSet {
        ordinal: u32,
        before_fingerprint: String,
        state: SpriteState,
    },
    #[serde(rename = "sprite.delete")]
    SpriteDelete {
        ordinal: u32,
        before_fingerprint: String,
    },
    #[serde(rename = "sprite.move")]
    SpriteMove {
        ordinal: u32,
        before_fingerprint: String,
        x: u16,
        y: u16,
    },
    #[serde(rename = "location.add")]
    LocationAdd { state: LocationState },
    #[serde(rename = "location.set")]
    LocationSet { state: LocationState },
    #[serde(rename = "location.rename")]
    LocationRename {
        location_id: u16,
        name_bytes_hex: String,
    },
    #[serde(rename = "location.delete")]
    LocationDelete { location_id: u16 },
}

const fn one() -> u16 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapEditBatch {
    pub schema: String,
    pub expected: MapEditExpected,
    pub operations: Vec<MapOperation>,
}

impl MapEditBatch {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != MAP_EDIT_SCHEMA {
            return Err(format!(
                "unsupported map edit schema '{}'; expected {MAP_EDIT_SCHEMA}",
                self.schema
            ));
        }
        if self.operations.is_empty() {
            return Err("map edit batch requires at least one operation".to_string());
        }
        if self.expected.width == 0 || self.expected.height == 0 {
            return Err("map edit expected dimensions must be non-zero".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LayerDiffCount {
    pub added: u32,
    pub removed: u32,
    pub moved: u32,
    pub changed: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapDiff {
    pub terrain_cells: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terrain_bounds: Option<TileRect>,
    pub units: LayerDiffCount,
    pub buildings: LayerDiffCount,
    pub doodads: LayerDiffCount,
    pub sprites: LayerDiffCount,
    pub locations: LayerDiffCount,
    pub outside_target: u32,
    pub protected: u32,
    pub unsupported_section_changes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerificationReport {
    pub valid: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub diff: MapDiff,
    pub candidate_sha256: String,
    pub canonical_digest: String,
    pub extra_assets_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CandidateRevision {
    pub revision: u32,
    pub parent: u32,
    pub request_id: String,
    pub operation_manifest: PathBuf,
    pub map_sha256: String,
    pub diff: MapDiff,
    pub verification: VerificationReport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CandidateSession {
    pub session_id: String,
    pub baseline: MapRevision,
    pub baseline_snapshot: PathBuf,
    pub current_revision: u32,
    pub current_map: PathBuf,
    pub revisions: Vec<CandidateRevision>,
    pub selections: BTreeMap<String, SelectionMask>,
    pub persistent_protections: BTreeSet<String>,
    #[serde(default)]
    pub candidate_object_ids: BTreeMap<String, String>,
    #[serde(default)]
    pub stale: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_apply_backup: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_apply_source_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_apply_before_hash: Option<String>,
}

pub fn hex_sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn layers() -> BTreeSet<MapLayer> {
        [MapLayer::Terrain, MapLayer::Units].into_iter().collect()
    }

    #[test]
    fn rows_canonicalize_sort_merge_overlap_and_adjacency() {
        let mask = SelectionMask::canonical(
            "s1",
            "Region A",
            "r0",
            SelectionRole::Target,
            layers(),
            MaskGrid {
                width: 12,
                height: 8,
                rows: vec![
                    RowSpan {
                        y: 3,
                        spans: vec![(8, 10), (2, 5), (5, 8)],
                    },
                    RowSpan {
                        y: 1,
                        spans: vec![(1, 2), (4, 6)],
                    },
                    RowSpan {
                        y: 3,
                        spans: vec![(1, 3)],
                    },
                ],
            },
        )
        .unwrap();
        assert_eq!(
            mask.rows,
            vec![
                RowSpan {
                    y: 1,
                    spans: vec![(1, 2), (4, 6)]
                },
                RowSpan {
                    y: 3,
                    spans: vec![(1, 10)]
                }
            ]
        );
        assert_eq!(mask.selected_cells, 12);
        assert_eq!(
            mask.bounds,
            TileRect {
                left: 1,
                top: 1,
                right: 10,
                bottom: 4
            }
        );
    }

    #[test]
    fn canonical_mask_rejects_empty_reversed_and_out_of_bounds_spans() {
        assert!(SelectionMask::canonical(
            "s",
            "empty",
            "r0",
            SelectionRole::Target,
            layers(),
            MaskGrid {
                width: 4,
                height: 4,
                rows: vec![],
            }
        )
        .is_err());
        assert!(canonical_rows(
            4,
            4,
            vec![RowSpan {
                y: 0,
                spans: vec![(2, 2)]
            }]
        )
        .is_err());
        assert!(canonical_rows(
            4,
            4,
            vec![RowSpan {
                y: 4,
                spans: vec![(0, 1)]
            }]
        )
        .is_err());
    }

    #[test]
    fn cell_set_operations_cover_disjoint_holes_and_invert() {
        let current = [
            (0, 0),
            (1, 0),
            (2, 0),
            (0, 1),
            (2, 1),
            (0, 2),
            (1, 2),
            (2, 2),
        ]
        .into_iter()
        .collect::<BTreeSet<_>>();
        let incoming = [(1, 1), (4, 4)].into_iter().collect::<BTreeSet<_>>();
        let added =
            combine_selection_cells(8, 8, &current, &incoming, SelectionOperation::Add).unwrap();
        assert!(added.contains(&(1, 1)) && added.contains(&(4, 4)));
        let subtracted =
            combine_selection_cells(8, 8, &added, &incoming, SelectionOperation::Subtract).unwrap();
        assert_eq!(subtracted, current);
        let inverted =
            combine_selection_cells(8, 8, &current, &incoming, SelectionOperation::Invert).unwrap();
        assert!(!inverted.contains(&(0, 3)));
        assert!(inverted.contains(&(1, 1)) && inverted.contains(&(4, 4)));
        assert_eq!(rows_to_cells(&rows_from_cells(&inverted)), inverted);
    }

    #[test]
    fn selection_hash_changes_with_authority_fields() {
        let base = SelectionMask::canonical(
            "s",
            "Area",
            "r0",
            SelectionRole::Target,
            layers(),
            MaskGrid {
                width: 8,
                height: 8,
                rows: vec![RowSpan {
                    y: 2,
                    spans: vec![(1, 4)],
                }],
            },
        )
        .unwrap();
        let mut changed = base.clone();
        changed.layers.insert(MapLayer::Sprites);
        assert_ne!(base.snapshot_hash(), changed.snapshot_hash());
    }

    #[test]
    fn strict_batch_schema_rejects_unknown_fields_and_variants() {
        let unknown_top = json!({
            "schema": MAP_EDIT_SCHEMA,
            "expected": {
                "inputFileSha256": "a",
                "tileset": "jungle",
                "width": 64,
                "height": 64
            },
            "operations": [{"op": "terrain.set", "x": 1, "y": 1, "before": 0, "after": 1}],
            "extra": true
        });
        assert!(serde_json::from_value::<MapEditBatch>(unknown_top).is_err());
        let unknown_operation = json!({
            "schema": MAP_EDIT_SCHEMA,
            "expected": {
                "inputFileSha256": "a",
                "tileset": "jungle",
                "width": 64,
                "height": 64
            },
            "operations": [{"op": "fog.set", "x": 1, "y": 1}]
        });
        assert!(serde_json::from_value::<MapEditBatch>(unknown_operation).is_err());
    }

    #[test]
    fn batch_schema_and_non_empty_operations_are_enforced() {
        let batch: MapEditBatch = serde_json::from_value(json!({
            "schema": "eud-map-edit/0",
            "expected": {
                "inputFileSha256": "a",
                "tileset": "jungle",
                "width": 64,
                "height": 64
            },
            "operations": []
        }))
        .unwrap();
        assert!(batch.validate().is_err());
    }

    #[test]
    fn mention_snapshots_use_frontend_camel_case_fields() {
        let payloads = vec![
            json!({
                "kind": "region",
                "selectionId": "selection-1",
                "snapshotHash": "snapshot-1",
                "sourceRevision": "revision-1"
            }),
            json!({
                "kind": "object",
                "objectRef": {
                    "kind": "unit",
                    "ordinal": 7,
                    "semanticFingerprint": "fingerprint-1",
                    "revisionKey": "revision-1",
                    "baselineHash": "baseline-1"
                },
                "role": "subject"
            }),
            json!({
                "kind": "palette",
                "entry": {
                    "layer": "terrain",
                    "kind": "semanticTerrain",
                    "entryId": 3,
                    "tileset": "jungle",
                    "fingerprint": "palette-1"
                },
                "qualifiers": {}
            }),
            json!({
                "kind": "stamp",
                "selectionId": "selection-1",
                "snapshotHash": "snapshot-1"
            }),
            json!({
                "kind": "location",
                "locationId": 4,
                "revisionKey": "revision-1",
                "baselineHash": "baseline-1"
            }),
        ];

        let mentions = payloads
            .iter()
            .cloned()
            .map(serde_json::from_value::<MapMentionSnapshot>)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let round_trip = mentions
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(round_trip, payloads);
    }
}
