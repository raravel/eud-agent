//! The panel's read-only view of the WHOLE version-matched DAT catalog — the
//! reference EUD Editor 3's DAT Editor gives, served from the compatibility
//! assets this app already ships.
//!
//! Two commands, both read-only:
//!   * `dat_wiki_schema` — every table, its objects (resolved names) and its
//!     field metadata (order, width, value range, reference target, flag
//!     labels). Loaded once per project and cached by the panel.
//!   * `dat_wiki_sheet` — one GRP sheet (command icons, the three wireframe
//!     sheets) as a single grid PNG plus its frame geometry, so the object
//!     list can draw hundreds of thumbnails without one request each.
//!   * `dat_wiki_graphic` — the unit/sprite graphic one object resolves to,
//!     following the project's own values so an overridden `Graphics` shows
//!     the graphic the project actually runs.
//!   * `dat_wiki_object` — one object's values: the catalog stock value and,
//!     when `dat/*.json` holds a sparse override, what the project changed it
//!     to. Nothing here writes: an override is only ever created by
//!     `dat_patch` through the ordinary journal/review path.
//!
//! Two index conventions meet here and must not be mixed up. A DAT label field
//! (`Type=11`) stores a ONE-based `stat_txt.tbl` string id, so its text is
//! `tbl_strings()[value - 1]`. The sparse TBL document (`dat/tbl.json`, and
//! `DatTarget::Tbl`) addresses the same strings ZERO-based, which is what the
//! `tbl` table below lists.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::DataDirs;
use crate::ipc::AppManaged;
use crate::native_build::{DatCatalog, DatFieldMeta, DAT_TABLES};
use crate::native_project::{DatScalar, DatTarget, NativeProject};

/// Which catalog a `.def` `Type=` value points at. The mapping is the one the
/// data itself proves: units.Graphics (`Type=2`) is flingy 78 for the Marine,
/// flingy.Sprite (`Type=3`) is a sprite id, sprites."Image File" (`Type=4`) an
/// image id, and weapons.Label (`Type=11`) the one-based `stat_txt` id whose
/// predecessor slot reads "Gauss Rifle".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DatReference {
    /// Another DAT table, named by its id ("units", "flingy", ...).
    Table(&'static str),
    /// A one-based `stat_txt.tbl` string id.
    Text,
    /// A `cmdicons.grp` frame; the catalog carries no names for these.
    Icon,
    /// An `iscript.bin` script id.
    Iscript,
}

fn reference_of(meta: &DatFieldMeta) -> Option<DatReference> {
    Some(match meta.value_type? {
        0 => DatReference::Table("units"),
        1 => DatReference::Table("weapons"),
        2 => DatReference::Table("flingy"),
        3 => DatReference::Table("sprites"),
        4 => DatReference::Table("images"),
        5 => DatReference::Table("upgrades"),
        6 => DatReference::Table("techdata"),
        7 => DatReference::Table("orders"),
        8 => DatReference::Table("portdata"),
        9 => DatReference::Table("sfxdata"),
        10 => DatReference::Icon,
        11 => DatReference::Text,
        12 => DatReference::Iscript,
        // An unknown `Type=` is shown as a plain number rather than guessed at.
        _ => return None,
    })
}

/// A GRP sheet the wiki draws frames out of. Each is one file in the
/// installed StarCraft whose frames are addressed by number, which is exactly
/// how the DAT fields that point at them are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DatWikiSheet {
    /// `unit\cmdicons\cmdicons.grp`: every command icon, unit ids first.
    Cmdicons,
    /// `unit\wirefram\wirefram.grp`: the 228 unit wireframes.
    Wirefram,
    /// `unit\wirefram\grpwire.grp`: the 131 group wireframes.
    Grpwire,
    /// `unit\wirefram\tranwire.grp`: the 106 transport wireframes.
    Tranwire,
}

impl DatWikiSheet {
    fn asset(self) -> &'static str {
        match self {
            Self::Cmdicons => crate::grp::CMDICONS_ASSET,
            Self::Wirefram => crate::grp::WIREFRAM_ASSET,
            Self::Grpwire => crate::grp::GRPWIRE_ASSET,
            Self::Tranwire => crate::grp::TRANWIRE_ASSET,
        }
    }
}

/// One frame of one sheet: what a row's thumbnail and a field's inline picture
/// both resolve to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatWikiPicture {
    pub sheet: DatWikiSheet,
    pub frame: u32,
}

/// Which sparse document a wiki table's overrides live in — the same split the
/// project keeps on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DatWikiKind {
    /// `dat/standard.json`: the ten numeric DAT tables.
    Dat,
    /// `dat/xdat.json`: wireframe, statusinfor, ButtonSet.
    Xdat,
    /// `dat/tbl.json`: `stat_txt.tbl` strings, addressed zero-based.
    Tbl,
    /// `dat/requirements.json`.
    Requirements,
    /// `dat/buttons.json`.
    Buttons,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatWikiField {
    pub name: String,
    /// Inclusive object-id range this field covers; a field outside its range
    /// simply does not exist for that object (units.Infestation is 106..201).
    pub var_start: u32,
    pub var_end: u32,
    /// Byte width in the DAT file (1, 2 or 4).
    pub size: u8,
    pub min: i64,
    pub max: i64,
    /// The runtime address the generator patches, shown as EUD reference.
    pub offset: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference: Option<DatReference>,
    /// The sheet this field's value is a frame number in, when it is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sheet: Option<DatWikiSheet>,
    /// One label per bit; empty when the field is not a flag field.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatWikiObject {
    pub id: u32,
    /// The display name, when the catalog can resolve one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The list thumbnail, when this object has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub picture: Option<DatWikiPicture>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatWikiTable {
    /// The id `dat_patch` and the sparse documents use ("units", "wireframe").
    pub id: String,
    pub kind: DatWikiKind,
    /// Korean panel label.
    pub label: String,
    pub objects: Vec<DatWikiObject>,
    pub fields: Vec<DatWikiField>,
    /// Whether `dat_wiki_graphic` can draw this table's objects.
    pub graphic: bool,
    /// Why some objects show no name, when that is the case.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatWikiSchema {
    pub tables: Vec<DatWikiTable>,
    /// Whether the pictures are available at all. Every one of them is read
    /// from the installed StarCraft, so without a resolvable install the wiki
    /// is text-only and says so instead of showing broken images.
    pub pictures: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pictures_notice: Option<String>,
}

/// One GRP sheet laid out as a single grid image. The panel slices it with
/// `background-position`, so a list of 228 wireframes costs one request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatWikiSheetImage {
    pub sheet: DatWikiSheet,
    /// A `data:image/png;base64,` URL of the whole grid.
    pub png: String,
    pub frame_width: u32,
    pub frame_height: u32,
    pub columns: u32,
    pub frames: u32,
}

/// One object's own graphic, resolved through the project's effective values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatWikiGraphic {
    pub png: String,
    pub width: u32,
    pub height: u32,
    /// The GRP the chain ended at, named the way `arr\images.tbl` names it.
    pub grp: String,
    /// The image id the chain resolved to, so the panel can link to it.
    pub image_id: u32,
}

/// One field of one object: what the catalog ships and what the project runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatWikiValue {
    pub field: String,
    pub stock: DatWikiScalar,
    /// Present only when `dat/*.json` overrides this field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<DatWikiScalar>,
}

/// A DAT value as the panel reads it. Numbers stay numbers so the panel can
/// resolve a reference; text values (TBL, requirements, buttons) stay text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum DatWikiScalar {
    Number(i64),
    Text(String),
}

impl From<DatScalar> for DatWikiScalar {
    fn from(value: DatScalar) -> Self {
        match value {
            DatScalar::Number(value) => Self::Number(value),
            DatScalar::Text(value) => Self::Text(value),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatWikiObjectValues {
    pub table: String,
    pub object_id: u32,
    pub values: Vec<DatWikiValue>,
}

/// Korean labels for the tables, in the order the wiki lists them.
const TABLE_LABELS: [(&str, &str); 14] = [
    ("units", "유닛"),
    ("weapons", "무기"),
    ("flingy", "플링기"),
    ("sprites", "스프라이트"),
    ("images", "이미지"),
    ("upgrades", "업그레이드"),
    ("techdata", "테크"),
    ("orders", "오더"),
    ("portdata", "초상화"),
    ("sfxdata", "사운드"),
    ("wireframe", "와이어프레임"),
    ("statusinfor", "상태 표시"),
    ("ButtonSet", "버튼셋"),
    ("buttons", "버튼"),
];

/// Requirement tables reuse the DAT table names ("units", "orders", ...), so
/// the wiki gives them their own ids. Without this a requirement table and its
/// DAT table would be the same row, and only one of them would be reachable.
const REQUIREMENT_PREFIX: &str = "requirements:";

fn table_label(id: &str) -> String {
    if let Some(table) = id.strip_prefix(REQUIREMENT_PREFIX) {
        return format!("생산 조건 · {table}");
    }
    // The catalog name is the informative part here, so it replaces the id
    // rather than being appended to it.
    if id == "tbl" {
        return "문자열 (stat_txt)".to_string();
    }
    let korean = TABLE_LABELS
        .iter()
        .find(|(key, _)| *key == id)
        .map(|(_, label)| *label);
    match korean {
        Some(label) => format!("{label} ({id})"),
        None => id.to_string(),
    }
}

/// The player whose colour every wiki picture is drawn in. A GRP fills
/// palette indices 8-15 with its owner's ramp, so a picture has to pick one.
const WIKI_PLAYER: u8 = 0;
/// How wide a sheet grid is laid out. Any value works; 16 keeps even the
/// 390-frame icon sheet close to square.
const SHEET_COLUMNS: u32 = 16;

fn asset(starcraft: &Path, archive_path: &str) -> Result<Vec<u8>, String> {
    isom::game_asset(starcraft, archive_path)
        .map_err(|error| format!("스타크래프트에서 {archive_path}을(를) 읽지 못했습니다: {error}"))
}

/// The palette a sheet is drawn with.
///
/// StarCraft keeps one palette per screen element in a `game\t*.pcx`, next to
/// a remap row saying which entry each shade of that element uses. The wiki
/// draws the resting state: an available command button (gold), and a
/// full-health wireframe (the palette as it stands, since `twire.pcx`'s rows
/// are the yellow and red damage tints).
fn sheet_palette(starcraft: &Path, sheet: DatWikiSheet) -> Result<crate::grp::Palette, String> {
    match sheet {
        DatWikiSheet::Cmdicons => {
            let pcx = asset(starcraft, crate::grp::ICON_PALETTE_ASSET)?;
            let ramps = crate::grp::pcx_pixels(&pcx)?;
            let ramp = ramps
                .get(..crate::grp::ICON_RAMP)
                .ok_or("ticon.pcx가 아이콘 램프를 담고 있지 않습니다")?;
            crate::grp::Palette::from_pcx(&pcx)?.remapped(ramp, 0)
        }
        // Listed rather than defaulted: a sheet added later must state which
        // palette it is drawn with instead of silently borrowing this one.
        DatWikiSheet::Wirefram | DatWikiSheet::Grpwire | DatWikiSheet::Tranwire => {
            crate::grp::Palette::from_pcx(&asset(starcraft, crate::grp::WIRE_PALETTE_ASSET)?)
        }
    }
}

/// The palette a unit or sprite graphic is drawn with: a tileset `.wpe` for
/// indices 0-7 and 16-255, and `game\tunit.pcx` for the player-colour range
/// 8-15, exactly as `Sc::Sprite::PixelLine` documents.
fn graphic_palette(starcraft: &Path) -> Result<crate::grp::Palette, String> {
    let base = crate::grp::Palette::parse(&asset(starcraft, crate::grp::PALETTE_ASSET)?)?;
    let remap = crate::grp::pcx_pixels(&asset(starcraft, crate::grp::UNIT_REMAP_ASSET)?)?;
    base.with_player(&remap, WIKI_PLAYER)
}

fn data_url(png: &[u8]) -> String {
    use base64::Engine as _;
    format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png)
    )
}

/// One whole GRP sheet as a grid, so the object list draws hundreds of
/// thumbnails from a single image instead of a request per row.
pub fn sheet_payload(dirs: &DataDirs, sheet: DatWikiSheet) -> Result<DatWikiSheetImage, String> {
    let starcraft = crate::map_context::resolve_starcraft_path(dirs)?;
    let palette = sheet_palette(&starcraft, sheet)?;
    let grp = crate::grp::Grp::parse(asset(&starcraft, sheet.asset())?)?;
    let frames = u32::try_from(grp.frame_count()).map_err(|_| "이 GRP는 프레임이 너무 많습니다")?;
    let frame_width = u32::from(grp.width());
    let frame_height = u32::from(grp.height());
    let columns = SHEET_COLUMNS.min(frames.max(1));
    let rows = frames.div_ceil(columns);
    let width = columns * frame_width;
    let height = rows * frame_height;
    let stride = (width * 4) as usize;
    let run = (frame_width * 4) as usize;
    let mut canvas = vec![0_u8; stride * height as usize];
    for frame in 0..frames {
        let rgba = grp.frame_rgba(frame as usize, &palette)?;
        let left = ((frame % columns) * frame_width * 4) as usize;
        let top = ((frame / columns) * frame_height) as usize;
        for row in 0..frame_height as usize {
            let from = row * run;
            let to = (top + row) * stride + left;
            canvas[to..to + run].copy_from_slice(&rgba[from..from + run]);
        }
    }
    Ok(DatWikiSheetImage {
        sheet,
        png: data_url(&crate::grp::encode_png(width, height, &canvas)?),
        frame_width,
        frame_height,
        columns,
        frames,
    })
}

/// A DAT number as the project runs it: the override when there is one, the
/// catalog stock value otherwise — the same number the wiki shows in the row.
fn effective(
    catalog: &DatCatalog,
    project: Option<&NativeProject>,
    dat: &str,
    field: &str,
    object_id: u32,
) -> Result<i64, String> {
    let target = DatTarget::Dat {
        dat: dat.to_string(),
        object_id,
        field: field.to_string(),
    };
    let stock = crate::native_runtime::baseline_value(catalog, &target)?;
    let value = project
        .and_then(|project| project.current_dat_value(&target))
        .unwrap_or(stock);
    match value {
        DatScalar::Number(value) => Ok(value),
        DatScalar::Text(_) => Err(format!("{dat}.{field}은(는) 숫자 필드가 아닙니다")),
    }
}

/// The first field of a table that points at `into`, by `.def` order.
fn referencing_field<'a>(
    catalog: &'a DatCatalog,
    table: &str,
    into: DatReference,
) -> Option<&'a DatFieldMeta> {
    let mut metas: Vec<_> = catalog
        .table_fields(table)?
        .values()
        .filter(|meta| reference_of(meta) == Some(into))
        .collect();
    metas.sort_by_key(|meta| meta.index);
    metas.into_iter().next()
}

/// Tables whose objects resolve to a drawable unit/sprite graphic.
const GRAPHIC_TABLES: [&str; 5] = ["units", "weapons", "flingy", "sprites", "images"];

/// Walks `table` -> flingy -> sprites -> images to the image id this object
/// draws as, following the project's own values at every step.
fn graphic_image(
    catalog: &DatCatalog,
    project: Option<&NativeProject>,
    table: &str,
    object_id: u32,
) -> Result<u32, String> {
    let step = |dat: &str, field: &str, id: i64| -> Result<u32, String> {
        let id = u32::try_from(id).map_err(|_| format!("{dat} 번호가 범위를 벗어났습니다"))?;
        let value = effective(catalog, project, dat, field, id)?;
        u32::try_from(value).map_err(|_| format!("{dat}.{field} 값이 범위를 벗어났습니다"))
    };
    let sprite_to_image = |sprite: i64| step("sprites", "Image File", sprite);
    let flingy_to_image =
        |flingy: i64| sprite_to_image(i64::from(step("flingy", "Sprite", flingy)?));
    match table {
        "images" => Ok(object_id),
        "sprites" => sprite_to_image(i64::from(object_id)),
        "flingy" => flingy_to_image(i64::from(object_id)),
        _ => {
            let field = referencing_field(catalog, table, DatReference::Table("flingy"))
                .ok_or_else(|| format!("{table}에는 그래픽 필드가 없습니다"))?
                .name
                .clone();
            let flingy = effective(catalog, project, table, &field, object_id)?;
            flingy_to_image(flingy)
        }
    }
}

/// One object's own graphic: frame 0 of the GRP its chain resolves to.
pub fn graphic_payload(
    dirs: &DataDirs,
    table: &str,
    object_id: u32,
) -> Result<DatWikiGraphic, String> {
    if !GRAPHIC_TABLES.contains(&table) {
        return Err(format!("'{table}'은(는) 그래픽이 없는 테이블입니다"));
    }
    let catalog = DatCatalog::load(&dirs.native_assets_dir())?;
    let config = dirs.load_config().map_err(|error| error.to_string())?;
    let project = NativeProject::open(Path::new(&config.project_path)).ok();
    let image_id = graphic_image(&catalog, project.as_ref(), table, object_id)?;
    let starcraft = crate::map_context::resolve_starcraft_path(dirs)?;
    let names = crate::iscript::tbl_strings(&asset(&starcraft, crate::iscript::IMAGES_TBL_ASSET)?)?;
    // `GRP File` is a one-based index into images.tbl, like every other
    // string id a DAT field carries.
    let entry = effective(&catalog, project.as_ref(), "images", "GRP File", image_id)?;
    let grp_name = usize::try_from(entry - 1)
        .ok()
        .and_then(|index| names.get(index))
        .ok_or_else(|| format!("images.tbl에 {entry}번 GRP가 없습니다"))?
        .clone();
    let palette = graphic_palette(&starcraft)?;
    let grp = crate::grp::Grp::parse(asset(
        &starcraft,
        &format!("{}{grp_name}", crate::grp::UNIT_GRP_PREFIX),
    )?)?;
    Ok(DatWikiGraphic {
        png: data_url(&grp.frame_png(0, &palette)?),
        width: u32::from(grp.width()),
        height: u32::from(grp.height()),
        grp: grp_name,
        image_id,
    })
}

/// The list thumbnail every object of a table gets, when the table has one.
///
/// Units and the wireframe table index their sheets by object id; weapons,
/// upgrades, techs and orders carry an explicit icon field (`.def` `Type=10`).
fn object_pictures(catalog: &DatCatalog, table: &str, count: u32) -> Vec<Option<DatWikiPicture>> {
    let by_id = |sheet: DatWikiSheet| {
        (0..count)
            .map(|frame| Some(DatWikiPicture { sheet, frame }))
            .collect()
    };
    match table {
        "units" => by_id(DatWikiSheet::Cmdicons),
        "wireframe" => by_id(DatWikiSheet::Wirefram),
        _ => {
            let Some(meta) = referencing_field(catalog, table, DatReference::Icon) else {
                return vec![None; count as usize];
            };
            (0..count)
                .map(|id| {
                    let frame = u32::try_from(meta.baseline_value(id).ok()?).ok()?;
                    Some(DatWikiPicture {
                        sheet: DatWikiSheet::Cmdicons,
                        frame,
                    })
                })
                .collect()
        }
    }
}

/// The name shown for every object of a DAT table.
///
/// Units carry hardcoded names; weapons, upgrades, techs and orders name
/// themselves through their own one-based `Label` string id. Flingy, sprites
/// and images ultimately name themselves through `images.tbl`, which lives in
/// the StarCraft install rather than in the compatibility assets, so those
/// three degrade to the bare id when no install is resolvable.
fn object_names(
    catalog: &DatCatalog,
    table: &str,
    count: u32,
    image_names: Option<&[String]>,
) -> Vec<Option<String>> {
    let text = |value: i64| -> Option<String> {
        let index = usize::try_from(value.checked_sub(1)?).ok()?;
        catalog
            .tbl_strings()
            .get(index)
            .filter(|value| !value.is_empty())
            .cloned()
    };
    let labelled = |field: &str| -> Vec<Option<String>> {
        let meta = catalog
            .table_fields(table)
            .and_then(|fields| fields.get(field));
        (0..count)
            .map(|id| meta?.baseline_value(id).ok().and_then(text))
            .collect()
    };
    // The GRP name an image points at; the chain sprites -> images and
    // flingy -> sprites -> images reuses it.
    let image_name = |image_id: i64| -> Option<String> {
        let names = image_names?;
        let grp = catalog
            .table_fields("images")?
            .get("GRP File")?
            .baseline_value(u32::try_from(image_id).ok()?)
            .ok()?;
        let name = names.get(usize::try_from(grp.checked_sub(1)?).ok()?)?;
        Some(
            name.rsplit(['\\', '/'])
                .next()
                .unwrap_or(name)
                .trim_end_matches(".grp")
                .to_string(),
        )
    };
    let reference = |from: &str, field: &str, id: u32| -> Option<i64> {
        catalog
            .table_fields(from)?
            .get(field)?
            .baseline_value(id)
            .ok()
    };
    match table {
        "units" => (0..count)
            .map(|id| u16::try_from(id).ok().map(crate::chk::unit_name))
            .collect(),
        "weapons" | "upgrades" | "techdata" | "orders" => labelled("Label"),
        "images" => (0..count).map(|id| image_name(id as i64)).collect(),
        "sprites" => (0..count)
            .map(|id| image_name(reference("sprites", "Image File", id)?))
            .collect(),
        "flingy" => (0..count)
            .map(|id| {
                let sprite = reference("flingy", "Sprite", id)?;
                let image = reference("sprites", "Image File", u32::try_from(sprite).ok()?)?;
                image_name(image)
            })
            .collect(),
        _ => vec![None; count as usize],
    }
}

/// `arr\images.tbl` out of the installed StarCraft, when one is resolvable.
/// Its absence only costs GRP names, so it is never an error here.
fn image_names(dirs: &DataDirs) -> Option<Vec<String>> {
    let starcraft = crate::map_context::resolve_starcraft_path(dirs).ok()?;
    let bytes = isom::game_asset(&starcraft, crate::iscript::IMAGES_TBL_ASSET).ok()?;
    crate::iscript::tbl_strings(&bytes).ok()
}

#[allow(clippy::too_many_arguments)]
fn dat_table(
    id: &str,
    kind: DatWikiKind,
    count: u32,
    names: Vec<Option<String>>,
    pictures: Vec<Option<DatWikiPicture>>,
    fields: Vec<DatWikiField>,
    graphic: bool,
    notice: Option<String>,
) -> DatWikiTable {
    DatWikiTable {
        id: id.to_string(),
        kind,
        label: table_label(id),
        objects: (0..count)
            .map(|object_id| DatWikiObject {
                id: object_id,
                name: names.get(object_id as usize).cloned().flatten(),
                picture: pictures.get(object_id as usize).copied().flatten(),
            })
            .collect(),
        fields,
        graphic,
        notice,
    }
}

fn numeric_fields(catalog: &DatCatalog, table: &str) -> Vec<DatWikiField> {
    let Some(metas) = catalog.table_fields(table) else {
        return Vec::new();
    };
    let mut fields: Vec<_> = metas.values().collect();
    fields.sort_by_key(|meta| meta.index);
    fields
        .into_iter()
        .map(|meta| {
            let (min, max) = meta.value_range();
            let reference = reference_of(meta);
            DatWikiField {
                name: meta.name.clone(),
                var_start: meta.var_start,
                var_end: meta.var_end,
                size: meta.size,
                min,
                max,
                offset: meta.offset,
                reference,
                sheet: (reference == Some(DatReference::Icon)).then_some(DatWikiSheet::Cmdicons),
                flags: meta.flags.clone(),
            }
        })
        .collect()
}

/// One synthetic field for a table whose objects hold a single value.
fn single_field(
    name: &str,
    count: u32,
    max: i64,
    sheet: Option<DatWikiSheet>,
) -> Vec<DatWikiField> {
    vec![DatWikiField {
        name: name.to_string(),
        var_start: 0,
        var_end: count.saturating_sub(1),
        size: 0,
        min: 0,
        max,
        offset: 0,
        reference: None,
        sheet,
        flags: Vec::new(),
    }]
}

/// The wireframe table's three fields are frame numbers into three different
/// sheets; every other synthetic field is a plain value.
fn xdat_sheet(table: &str, field: &str) -> Option<DatWikiSheet> {
    match (table, field) {
        ("wireframe", "wire") => Some(DatWikiSheet::Wirefram),
        ("wireframe", "grp") => Some(DatWikiSheet::Grpwire),
        ("wireframe", "tran") => Some(DatWikiSheet::Tranwire),
        _ => None,
    }
}

pub fn schema_payload(dirs: &DataDirs) -> Result<DatWikiSchema, String> {
    let catalog = DatCatalog::load(&dirs.native_assets_dir())?;
    let images = image_names(dirs);
    let missing_images = images.is_none();
    // Every picture is read from the installed StarCraft, so one resolvable
    // install decides whether the wiki has pictures at all.
    let pictures = crate::map_context::resolve_starcraft_path(dirs).is_ok();
    let mut tables = Vec::new();
    for id in DAT_TABLES {
        let count = catalog
            .table_entries(id)
            .ok_or_else(|| format!("카탈로그에 {id} 테이블이 없습니다"))?;
        let notice = (missing_images && matches!(id, "images" | "sprites" | "flingy")).then(|| {
            "스타크래프트 설치 폴더를 찾지 못해 GRP 이름을 표시할 수 없습니다. 설정 → 컴파일에서 폴더를 지정하세요.".to_string()
        });
        tables.push(dat_table(
            id,
            DatWikiKind::Dat,
            count,
            object_names(&catalog, id, count, images.as_deref()),
            object_pictures(&catalog, id, count),
            numeric_fields(&catalog, id),
            GRAPHIC_TABLES.contains(&id),
            notice,
        ));
    }

    // XDAT: three small tables the catalog answers through `xdat_value`, whose
    // object counts are the ranges that function accepts.
    for (id, count, fields) in [
        ("wireframe", 228_u32, vec!["wire", "grp", "tran"]),
        (
            "statusinfor",
            u32::try_from(catalog.status_count()).unwrap_or(0),
            vec!["Status", "Display"],
        ),
        (
            "ButtonSet",
            u32::try_from(catalog.button_set_count()).unwrap_or(0),
            vec!["ButtonSet"],
        ),
    ] {
        let names = if id == "wireframe" {
            object_names(&catalog, "units", count, images.as_deref())
        } else {
            vec![None; count as usize]
        };
        tables.push(dat_table(
            id,
            DatWikiKind::Xdat,
            count,
            names,
            object_pictures(&catalog, id, count),
            fields
                .into_iter()
                .flat_map(|field| {
                    single_field(field, count, u32::MAX as i64, xdat_sheet(id, field))
                })
                .collect(),
            false,
            None,
        ));
    }

    // The TBL strings themselves, addressed the way `dat/tbl.json` addresses
    // them: zero-based. The name IS the value here, so the object list carries
    // the text and the single field repeats it.
    let strings = catalog.tbl_strings();
    tables.push(dat_table(
        "tbl",
        DatWikiKind::Tbl,
        u32::try_from(strings.len()).unwrap_or(0),
        strings.iter().map(|value| Some(value.clone())).collect(),
        vec![None; strings.len()],
        single_field("문자열", u32::try_from(strings.len()).unwrap_or(0), 0, None),
        false,
        None,
    ));

    for (id, count) in catalog.requirement_tables() {
        let count = u32::try_from(count).unwrap_or(0);
        // "Stechdata" is the catalog's own second tech table; it has no DAT
        // twin, so it simply carries no names.
        let names = object_names(&catalog, id, count, images.as_deref());
        tables.push(dat_table(
            &format!("{REQUIREMENT_PREFIX}{id}"),
            DatWikiKind::Requirements,
            count,
            names,
            vec![None; count as usize],
            single_field("생산 조건", count, 0, None),
            false,
            None,
        ));
    }

    let buttons = u32::try_from(catalog.button_set_count()).unwrap_or(0);
    tables.push(dat_table(
        "buttons",
        DatWikiKind::Buttons,
        buttons,
        vec![None; buttons as usize],
        vec![None; buttons as usize],
        single_field("버튼", buttons, 0, None),
        false,
        None,
    ));

    Ok(DatWikiSchema {
        tables,
        pictures,
        pictures_notice: (!pictures).then(|| {
            "스타크래프트 설치 폴더를 찾지 못해 그림을 표시할 수 없습니다. 설정 → 컴파일에서 폴더를 지정하세요."
                .to_string()
        }),
    })
}

/// The target one wiki row addresses, so stock and override are read through
/// exactly the addressing the sparse documents use.
fn target_of(kind: DatWikiKind, table: &str, object_id: u32, field: &str) -> DatTarget {
    match kind {
        DatWikiKind::Dat => DatTarget::Dat {
            dat: table.to_string(),
            object_id,
            field: field.to_string(),
        },
        DatWikiKind::Xdat => DatTarget::Xdat {
            dat: table.to_string(),
            object_id,
            field: field.to_string(),
        },
        DatWikiKind::Tbl => DatTarget::Tbl(object_id),
        DatWikiKind::Requirements => DatTarget::Requirement {
            dat: table
                .strip_prefix(REQUIREMENT_PREFIX)
                .unwrap_or(table)
                .to_string(),
            object_id,
        },
        DatWikiKind::Buttons => DatTarget::Button(object_id),
    }
}

/// Which kind a table id belongs to; the panel sends the id it was given in the
/// schema, so an unknown one is a caller error rather than a silent default.
fn kind_of(catalog: &DatCatalog, table: &str) -> Result<DatWikiKind, String> {
    if DAT_TABLES.contains(&table) {
        return Ok(DatWikiKind::Dat);
    }
    if matches!(table, "wireframe" | "statusinfor" | "ButtonSet") {
        return Ok(DatWikiKind::Xdat);
    }
    if table == "tbl" {
        return Ok(DatWikiKind::Tbl);
    }
    if table == "buttons" {
        return Ok(DatWikiKind::Buttons);
    }
    if let Some(name) = table.strip_prefix(REQUIREMENT_PREFIX) {
        if catalog
            .requirement_tables()
            .iter()
            .any(|(id, _)| *id == name)
        {
            return Ok(DatWikiKind::Requirements);
        }
    }
    Err(format!("'{table}'은(는) DAT 위키가 아는 테이블이 아닙니다"))
}

/// The field names one object exposes: every catalog field whose object range
/// covers it, or the single synthetic field of the text tables.
fn object_fields(
    catalog: &DatCatalog,
    kind: DatWikiKind,
    table: &str,
    object_id: u32,
) -> Vec<String> {
    match kind {
        DatWikiKind::Dat => {
            let Some(metas) = catalog.table_fields(table) else {
                return Vec::new();
            };
            let mut fields: Vec<_> = metas
                .values()
                .filter(|meta| object_id >= meta.var_start && object_id <= meta.var_end)
                .collect();
            fields.sort_by_key(|meta| meta.index);
            fields.into_iter().map(|meta| meta.name.clone()).collect()
        }
        DatWikiKind::Xdat => match table {
            "wireframe" => vec!["wire".into(), "grp".into(), "tran".into()],
            "statusinfor" => vec!["Status".into(), "Display".into()],
            _ => vec!["ButtonSet".into()],
        },
        DatWikiKind::Tbl => vec!["문자열".into()],
        DatWikiKind::Requirements => vec!["생산 조건".into()],
        DatWikiKind::Buttons => vec!["버튼".into()],
    }
}

pub fn object_payload(
    dirs: &DataDirs,
    table: &str,
    object_id: u32,
) -> Result<DatWikiObjectValues, String> {
    let catalog = DatCatalog::load(&dirs.native_assets_dir())?;
    let kind = kind_of(&catalog, table)?;
    // The project is optional on purpose: the catalog is readable before a
    // project is open, and then there are simply no overrides to show.
    let config = dirs.load_config().map_err(|error| error.to_string())?;
    let project = NativeProject::open(Path::new(&config.project_path)).ok();
    let mut values = Vec::new();
    for field in object_fields(&catalog, kind, table, object_id) {
        let target = target_of(kind, table, object_id, &field);
        let Ok(stock) = crate::native_runtime::baseline_value(&catalog, &target) else {
            // A field the catalog cannot answer for this object (an XDAT range
            // the object falls outside of) is left out rather than guessed at.
            continue;
        };
        let current = project
            .as_ref()
            .and_then(|project| project.current_dat_value(&target))
            .filter(|current| *current != stock);
        values.push(DatWikiValue {
            field,
            stock: stock.into(),
            current: current.map(Into::into),
        });
    }
    Ok(DatWikiObjectValues {
        table: table.to_string(),
        object_id,
        values,
    })
}

#[tauri::command]
pub async fn dat_wiki_schema(state: tauri::State<'_, AppManaged>) -> Result<DatWikiSchema, String> {
    let dirs = state.dirs().clone();
    tauri::async_runtime::spawn_blocking(move || schema_payload(&dirs))
        .await
        .map_err(|error| format!("DAT 위키 카탈로그를 읽지 못했습니다: {error}"))?
}

#[tauri::command]
pub async fn dat_wiki_object(
    state: tauri::State<'_, AppManaged>,
    table: String,
    object_id: u32,
) -> Result<DatWikiObjectValues, String> {
    let dirs = state.dirs().clone();
    tauri::async_runtime::spawn_blocking(move || object_payload(&dirs, &table, object_id))
        .await
        .map_err(|error| format!("DAT 값을 읽지 못했습니다: {error}"))?
}

#[tauri::command]
pub async fn dat_wiki_sheet(
    state: tauri::State<'_, AppManaged>,
    sheet: DatWikiSheet,
) -> Result<DatWikiSheetImage, String> {
    let dirs = state.dirs().clone();
    tauri::async_runtime::spawn_blocking(move || sheet_payload(&dirs, sheet))
        .await
        .map_err(|error| format!("그림을 읽지 못했습니다: {error}"))?
}

#[tauri::command]
pub async fn dat_wiki_graphic(
    state: tauri::State<'_, AppManaged>,
    table: String,
    object_id: u32,
) -> Result<DatWikiGraphic, String> {
    let dirs = state.dirs().clone();
    tauri::async_runtime::spawn_blocking(move || graphic_payload(&dirs, &table, object_id))
        .await
        .map_err(|error| format!("그래픽을 읽지 못했습니다: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_build::sync_compat_assets;
    use crate::native_project::{
        NativeDatChange, NativeDatPatch, ProjectManifest, ProjectSettings,
    };
    use crate::native_runtime::NativeProjectManager;
    use std::fs;
    use std::path::PathBuf;

    /// Data directories carrying the real compatibility assets, plus a project
    /// the config points at — what both payload functions read.
    fn roots(tag: &str) -> (PathBuf, DataDirs) {
        let base = std::env::temp_dir().join(format!(
            "eud-dat-wiki-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        dirs.ensure_dirs().unwrap();
        sync_compat_assets(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/eud-editor-compat"),
            &dirs.native_assets_dir(),
        )
        .unwrap();
        (base, dirs)
    }

    fn project(tag: &str) -> (PathBuf, DataDirs, NativeProjectManager) {
        let (base, dirs) = roots(tag);
        let root = base.join("project");
        let manager = NativeProjectManager::new(dirs.clone());
        manager
            .create_project(
                &root,
                ProjectManifest {
                    schema_version: crate::native_project::PROJECT_SCHEMA_VERSION,
                    name: "DAT Wiki Demo".to_string(),
                    source_map: "maps/source.scx".to_string(),
                    output_map: "build/output.scx".to_string(),
                    main_file: "src/main.eps".to_string(),
                    settings: ProjectSettings::default(),
                    plugins: Vec::new(),
                    python_entrypoints: Vec::new(),
                    python_dependencies: Vec::new(),
                    python_lock: None,
                    editor_compatibility: None,
                },
            )
            .unwrap();
        fs::create_dir_all(root.join("maps")).unwrap();
        fs::write(root.join("maps/source.scx"), b"map").unwrap();
        manager
            .write_source("src/main.eps", "function onPluginStart() {}\n")
            .unwrap();
        let mut config = dirs.load_config().unwrap_or_default();
        config.project_path = root.to_string_lossy().into_owned();
        dirs.save_config(&config).unwrap();
        (base, dirs, manager)
    }

    fn catalog() -> DatCatalog {
        let (base, dirs) = roots("catalog");
        let catalog = DatCatalog::load(&dirs.native_assets_dir()).unwrap();
        fs::remove_dir_all(base).ok();
        catalog
    }

    /// A `Type=11` label is a ONE-based `stat_txt` id: weapon 0's label 229
    /// names "Gauss Rifle", which lives at zero-based slot 228.
    #[test]
    fn a_label_field_names_its_object_through_the_one_based_string_id() {
        let catalog = catalog();
        let names = object_names(&catalog, "weapons", 8, None);
        assert_eq!(names[0].as_deref(), Some("Gauss Rifle"));
        assert_eq!(names[2].as_deref(), Some("C-10 Canister Rifle"));
        assert_eq!(names[4].as_deref(), Some("Fragmentation Grenade"));
    }

    /// The reference mapping is the one the data proves: the Marine's Graphics
    /// is flingy 78, and flingy is what `Type=2` means.
    #[test]
    fn a_graphics_field_references_the_flingy_table() {
        let catalog = catalog();
        let meta = catalog
            .table_fields("units")
            .unwrap()
            .get("Graphics")
            .unwrap();
        assert_eq!(reference_of(meta), Some(DatReference::Table("flingy")));
        assert_eq!(meta.baseline_value(0).unwrap(), 78);
    }

    /// A flag field carries one label per bit, quoted labels keep their commas,
    /// and `&&` is a literal `&`.
    #[test]
    fn a_flag_field_carries_one_label_per_bit() {
        let catalog = catalog();
        let meta = catalog
            .table_fields("units")
            .unwrap()
            .get("Special Ability Flags")
            .unwrap();
        assert_eq!(meta.flags.len(), 32);
        assert_eq!(meta.flags[0], "Building");
        assert_eq!(meta.flags[10], "Two Units in 1 Egg");
        let availability = catalog
            .table_fields("units")
            .unwrap()
            .get("Staredit Availability Flags")
            .unwrap();
        assert_eq!(availability.flags[1], "Unit Listing&Palette");
    }

    /// A field only covers part of a table, and an object outside that range
    /// must not be offered the field at all.
    #[test]
    fn a_ranged_field_belongs_only_to_the_objects_it_covers() {
        let catalog = catalog();
        let inside = object_fields(&catalog, DatWikiKind::Dat, "units", 106);
        let outside = object_fields(&catalog, DatWikiKind::Dat, "units", 0);
        assert!(inside.contains(&"Infestation".to_string()));
        assert!(!outside.contains(&"Infestation".to_string()));
    }

    /// Fields are listed in `.def` order, not alphabetically, because that is
    /// the order the DAT Editor shows and the order the file itself declares.
    #[test]
    fn fields_keep_their_definition_order() {
        let catalog = catalog();
        let fields = object_fields(&catalog, DatWikiKind::Dat, "units", 0);
        assert_eq!(fields[0], "Graphics");
        assert_eq!(fields[1], "Subunit 1");
        assert_eq!(fields[fields.len() - 1], "Staredit Availability Flags");
    }

    /// The schema is the whole catalog, not just the ten numeric tables: the
    /// sparse documents also hold XDAT, TBL, requirements and buttons, and a
    /// wiki that stops at `units` would leave those unreadable.
    #[test]
    fn the_schema_carries_every_table_the_sparse_documents_address() {
        let (base, dirs) = roots("schema");
        let schema = schema_payload(&dirs).unwrap();
        let ids: Vec<&str> = schema
            .tables
            .iter()
            .map(|table| table.id.as_str())
            .collect();
        for expected in DAT_TABLES {
            assert!(ids.contains(&expected), "{expected} missing from {ids:?}");
        }
        for expected in ["wireframe", "statusinfor", "ButtonSet", "tbl", "buttons"] {
            assert!(ids.contains(&expected), "{expected} missing from {ids:?}");
        }
        let units = schema
            .tables
            .iter()
            .find(|table| table.id == "units")
            .unwrap();
        assert_eq!(units.objects.len(), 228);
        assert_eq!(units.objects[0].name.as_deref(), Some("Terran Marine"));

        // Requirement tables reuse the DAT table names; two rows with one id
        // would hide one of them behind the other in every lookup.
        let mut seen = std::collections::BTreeSet::new();
        for table in &schema.tables {
            assert!(
                seen.insert(table.id.clone()),
                "duplicate table id {}",
                table.id
            );
        }
        assert!(ids.contains(&"requirements:units"));
        assert!(ids.contains(&"requirements:Stechdata"));
        fs::remove_dir_all(base).ok();
    }

    /// An override is shown beside the stock value it replaces; a field the
    /// project left alone carries no `current` at all.
    #[test]
    fn an_object_shows_the_projects_override_beside_the_stock_value() {
        let (base, dirs, manager) = project("object");
        manager
            .apply_dat_patch(&NativeDatPatch {
                changes: vec![NativeDatChange::Dat {
                    dat: "units".to_string(),
                    object_id: 0,
                    field: "Hit Points".to_string(),
                    before: 10240,
                    after: 20480,
                }],
            })
            .unwrap();

        let values = object_payload(&dirs, "units", 0).unwrap();
        let hp = values
            .values
            .iter()
            .find(|value| value.field == "Hit Points")
            .unwrap();
        assert_eq!(hp.stock, DatWikiScalar::Number(10240));
        assert_eq!(hp.current, Some(DatWikiScalar::Number(20480)));

        let graphics = values
            .values
            .iter()
            .find(|value| value.field == "Graphics")
            .unwrap();
        assert_eq!(graphics.stock, DatWikiScalar::Number(78));
        assert_eq!(graphics.current, None, "an untouched field has no override");
        fs::remove_dir_all(base).ok();
    }

    /// Units and wireframes index their sheets by object id; every other
    /// table that has icons carries an explicit icon field instead, and a
    /// table with neither gets no thumbnails at all.
    #[test]
    fn a_thumbnail_comes_from_the_objects_own_icon_field_not_its_id() {
        let catalog = catalog();

        let units = object_pictures(&catalog, "units", 228);
        assert_eq!(
            units[0],
            Some(DatWikiPicture {
                sheet: DatWikiSheet::Cmdicons,
                frame: 0,
            })
        );
        assert_eq!(
            units[7],
            Some(DatWikiPicture {
                sheet: DatWikiSheet::Cmdicons,
                frame: 7,
            })
        );

        let wireframe = object_pictures(&catalog, "wireframe", 228);
        assert_eq!(
            wireframe[7],
            Some(DatWikiPicture {
                sheet: DatWikiSheet::Wirefram,
                frame: 7,
            })
        );

        // The Gauss Rifle's icon is not icon 0; it is whatever weapons.dat
        // says, which is the whole point of reading the field.
        let icon = catalog
            .table_fields("weapons")
            .unwrap()
            .values()
            .find(|meta| meta.value_type == Some(10))
            .unwrap()
            .baseline_value(0)
            .unwrap();
        assert_eq!(
            object_pictures(&catalog, "weapons", 130)[0],
            Some(DatWikiPicture {
                sheet: DatWikiSheet::Cmdicons,
                frame: u32::try_from(icon).unwrap(),
            })
        );

        assert!(
            object_pictures(&catalog, "sfxdata", 4)
                .iter()
                .all(Option::is_none),
            "a table with no icon field carries no thumbnails"
        );
    }

    /// The wireframe table's three fields are frame numbers into three
    /// different sheets, and an icon field is a frame number into a fourth.
    #[test]
    fn a_field_names_the_sheet_its_value_is_a_frame_of() {
        assert_eq!(
            xdat_sheet("wireframe", "wire"),
            Some(DatWikiSheet::Wirefram)
        );
        assert_eq!(xdat_sheet("wireframe", "grp"), Some(DatWikiSheet::Grpwire));
        assert_eq!(
            xdat_sheet("wireframe", "tran"),
            Some(DatWikiSheet::Tranwire)
        );
        assert_eq!(xdat_sheet("statusinfor", "Status"), None);

        let catalog = catalog();
        let fields = numeric_fields(&catalog, "weapons");
        let icon = fields
            .iter()
            .find(|field| field.reference == Some(DatReference::Icon))
            .expect("weapons has an icon field");
        assert_eq!(icon.sheet, Some(DatWikiSheet::Cmdicons));
        let damage = fields.iter().find(|field| field.name == "Damage Amount");
        assert_eq!(damage.and_then(|field| field.sheet), None);
    }

    /// The graphic chain is units -> flingy -> sprites -> images, and it reads
    /// the project's own values, so an overridden `Graphics` resolves to the
    /// graphic the project actually runs rather than the catalog's.
    #[test]
    fn an_overridden_graphics_field_resolves_to_the_new_image() {
        let (base, dirs, manager) = project("graphic");
        let catalog = catalog();

        // The Marine's stock chain: flingy 78 -> sprite -> image.
        let stock = graphic_image(&catalog, None, "units", 0).unwrap();
        let zergling = graphic_image(&catalog, None, "units", 37).unwrap();
        assert_ne!(stock, zergling);

        manager
            .apply_dat_patch(&NativeDatPatch {
                changes: vec![NativeDatChange::Dat {
                    dat: "units".to_string(),
                    object_id: 0,
                    field: "Graphics".to_string(),
                    before: 78,
                    after: catalog
                        .table_fields("units")
                        .unwrap()
                        .get("Graphics")
                        .unwrap()
                        .baseline_value(37)
                        .unwrap(),
                }],
            })
            .unwrap();
        let config = dirs.load_config().unwrap();
        let project = NativeProject::open(Path::new(&config.project_path)).unwrap();
        assert_eq!(
            graphic_image(&catalog, Some(&project), "units", 0).unwrap(),
            zergling,
            "the wiki draws what the project runs, not what the catalog ships"
        );
        fs::remove_dir_all(base).ok();
    }

    /// Only the five tables that reach a GRP advertise a graphic, and asking
    /// for one anywhere else is refused by name rather than drawn blank.
    #[test]
    fn a_table_without_a_graphic_chain_is_refused_by_name() {
        let (base, dirs) = roots("nographic");
        let error = graphic_payload(&dirs, "upgrades", 0).unwrap_err();
        assert!(error.contains("upgrades"), "{error}");
        assert!(!GRAPHIC_TABLES.contains(&"upgrades"));
        assert!(GRAPHIC_TABLES.contains(&"units"));
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn an_unknown_table_is_refused_by_name() {
        let catalog = catalog();
        assert!(kind_of(&catalog, "units").is_ok());
        assert!(kind_of(&catalog, "requirements:Stechdata").is_ok());
        assert!(
            kind_of(&catalog, "units").unwrap() == DatWikiKind::Dat,
            "a requirement table never shadows its DAT table"
        );
        let error = kind_of(&catalog, "nonsense").unwrap_err();
        assert!(error.contains("nonsense"), "{error}");
    }
}
