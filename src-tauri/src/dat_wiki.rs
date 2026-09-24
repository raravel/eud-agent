//! The panel's read-only view of the WHOLE version-matched DAT catalog — the
//! reference EUD Editor 3's DAT Editor gives, served from the compatibility
//! assets this app already ships.
//!
//! Two commands, both read-only:
//!   * `dat_wiki_schema` — every table, its objects (resolved names) and its
//!     field metadata (order, width, value range, reference target, flag
//!     labels). Loaded once per project and cached by the panel.
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

use serde::Serialize;

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
    /// Why some objects show no name, when that is the case.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatWikiSchema {
    pub tables: Vec<DatWikiTable>,
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

fn dat_table(
    id: &str,
    kind: DatWikiKind,
    count: u32,
    names: Vec<Option<String>>,
    fields: Vec<DatWikiField>,
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
            })
            .collect(),
        fields,
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
            DatWikiField {
                name: meta.name.clone(),
                var_start: meta.var_start,
                var_end: meta.var_end,
                size: meta.size,
                min,
                max,
                offset: meta.offset,
                reference: reference_of(meta),
                flags: meta.flags.clone(),
            }
        })
        .collect()
}

/// One synthetic field for a table whose objects hold a single value.
fn single_field(name: &str, count: u32, max: i64) -> Vec<DatWikiField> {
    vec![DatWikiField {
        name: name.to_string(),
        var_start: 0,
        var_end: count.saturating_sub(1),
        size: 0,
        min: 0,
        max,
        offset: 0,
        reference: None,
        flags: Vec::new(),
    }]
}

pub fn schema_payload(dirs: &DataDirs) -> Result<DatWikiSchema, String> {
    let catalog = DatCatalog::load(&dirs.native_assets_dir())?;
    let images = image_names(dirs);
    let missing_images = images.is_none();
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
            numeric_fields(&catalog, id),
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
            fields
                .into_iter()
                .flat_map(|field| single_field(field, count, u32::MAX as i64))
                .collect(),
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
        single_field("문자열", u32::try_from(strings.len()).unwrap_or(0), 0),
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
            single_field("생산 조건", count, 0),
            None,
        ));
    }

    let buttons = u32::try_from(catalog.button_set_count()).unwrap_or(0);
    tables.push(dat_table(
        "buttons",
        DatWikiKind::Buttons,
        buttons,
        vec![None; buttons as usize],
        single_field("버튼", buttons, 0),
        None,
    ));

    Ok(DatWikiSchema { tables })
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
