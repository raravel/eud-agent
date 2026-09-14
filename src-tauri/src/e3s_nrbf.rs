//! Independent MS-NRBF reader/writer for EUD Editor `.e3s` compatibility.
//!
//! Supported legacy state is projected semantically: CUI epScript trees, MainFile,
//! map paths, EDS settings/plugins, standard DAT, XDAT, TBL, requirements, and
//! buttons. GUIEps/GUIPy/RawText sources are rejected instead of silently dropped;
//! the built-in ClassicTrigger and Setting nodes remain opaque in the retained base.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::memory::write_atomic_bytes;
use crate::native_build::DatCatalog;
use crate::native_project::{
    EditorCompatibility, EdsPlugin, NativeDatState, NativeProject, NativeSourceFile,
    NumericOverride, OpaqueEditorRecord, ProjectManifest, ProjectSettings, RequirementDocument,
    TextDatDocument, TextOverride, PROJECT_SCHEMA_VERSION,
};
use crate::nrbf::{Document, FieldValue, ObjectValue, Primitive, Record};

const EXTENSION_MARKER: &str = "EUD_AGENT_NATIVE_PROJECT_V1\0";
const MAX_E3S_BYTES: usize = 64 * 1024 * 1024;
const MAX_EXTENSION_BYTES: usize = 32 * 1024 * 1024;
const NULL_TBL_STRING: &str = "NULLSTRING";

const SAVEABLE_DATA: &str = "EUD_Editor_3.SaveableData";
const CUI_SCRIPT_EDITOR: &str = "EUD_Editor_3.CUIScriptEditor";
const TE_FILE: &str = "EUD_Editor_3.TEFile";
const TE_FILE_TYPE: &str = "EUD_Editor_3.TEFile+EFileType";
const TE_FILE_LIST: &str = "System.Collections.Generic.List`1[[EUD_Editor_3.TEFile, EUD Editor 3, Version=0.19.6.0, Culture=neutral, PublicKeyToken=null]]";
const TE_TAB_UI: &str = "EUD_Editor_3.TETabItemUI";
const SCRIPT_TYPE: &str = "EUD_Editor_3.ScriptEditor+SType";
const EDS_ITEM: &str = "EUD_Editor_3.BuildData+EdsBlock+EdsBlockItem";
const EDS_ITEM_TYPE: &str = "EUD_Editor_3.BuildData+EdsBlockType";
const BUTTON_DATA: &str = "EUD_Editor_3.CButtonData";
const REQUIRE_BLOCK: &str = "EUD_Editor_3.CRequireData+RequireBlock";
const REQUIRE_OPCODE: &str = "EUD_Editor_3.CRequireData+EOpCode";

const TE_FOLDER: i64 = 0;
const TE_CUI_EPS: i64 = 1;
const TE_GUI_EPS: i64 = 3;
const TE_GUI_PY: i64 = 4;
const TE_SETTING: i64 = 5;
const TE_CLASSIC: i64 = 6;
const TE_RAW_TEXT: i64 = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NrbfDocument {
    bytes: Vec<u8>,
    graph: Document,
}

impl NrbfDocument {
    pub fn parse(bytes: Vec<u8>) -> Result<Self, String> {
        if bytes.len() < 18 || bytes.len() > MAX_E3S_BYTES {
            return Err(format!("E3S length must be in 18..={MAX_E3S_BYTES} bytes"));
        }
        let graph = Document::parse(&bytes)?;
        Ok(Self { bytes, graph })
    }

    pub fn read(path: &Path) -> Result<Self, String> {
        Self::parse(fs::read(path).map_err(stringify_io)?)
    }

    pub fn sha256(&self) -> String {
        format!("{:x}", Sha256::digest(&self.bytes))
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn write_exact(&self, path: &Path) -> Result<(), String> {
        write_atomic_bytes(path, &self.bytes).map_err(|error| error.to_string())
    }

    pub fn native_payload(&self) -> Result<Option<NativeE3sPayload>, String> {
        native_payload(&self.graph)
    }

    fn without_native_payload(&self) -> Result<Self, String> {
        let mut graph = self.graph.clone();
        remove_native_payload(&mut graph);
        let bytes = graph.to_bytes()?;
        Ok(Self { bytes, graph })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeE3sPayload {
    pub schema_version: u32,
    pub manifest: ProjectManifest,
    pub dat: NativeDatState,
    pub sources: Vec<NativeSourceFile>,
    pub base_sha256: String,
}

impl NativeE3sPayload {
    pub fn from_project(project: &NativeProject, base_sha256: String) -> Result<Self, String> {
        Ok(Self {
            schema_version: 1,
            manifest: project.manifest().clone(),
            dat: project.dat().clone(),
            sources: project.source_snapshot()?.files,
            base_sha256,
        })
    }
}

#[derive(Debug)]
struct LegacyProjection {
    manifest: ProjectManifest,
    dat: NativeDatState,
    sources: Vec<NativeSourceFile>,
    source_map: PathBuf,
    opaque_records: Vec<OpaqueEditorRecord>,
}

/// Import a legacy or native-extended E3S into one canonical native project.
pub fn import_e3s(
    source: &Path,
    native_root: &Path,
    compat_root: &Path,
) -> Result<NativeProject, String> {
    if native_root.exists()
        && fs::read_dir(native_root)
            .map_err(stringify_io)?
            .next()
            .is_some()
    {
        return Err(format!(
            "native import destination must be empty: {}",
            native_root.display()
        ));
    }
    let original = NrbfDocument::read(source)?;
    let base = original.without_native_payload()?;
    let catalog = DatCatalog::load(compat_root)?;
    let legacy = project_from_graph(&base.graph, source, &catalog)?;
    let payload = original.native_payload()?;
    let (mut manifest, dat, sources) = match payload {
        Some(payload) => {
            if payload.schema_version != 1 {
                return Err(format!(
                    "native E3S extension schemaVersion {} is unsupported",
                    payload.schema_version
                ));
            }
            (payload.manifest, payload.dat, payload.sources)
        }
        None => (legacy.manifest, legacy.dat, legacy.sources),
    };
    let mut source_paths = BTreeSet::new();
    let mut has_main = false;
    for source_file in &sources {
        let expected = format!("{:x}", Sha256::digest(source_file.content.as_bytes()));
        if source_file.sha256 != expected {
            return Err(format!(
                "E3S native source hash mismatch: {}",
                source_file.path
            ));
        }
        if !source_paths.insert(source_file.path.to_ascii_lowercase()) {
            return Err(format!(
                "E3S native source path is duplicated: {}",
                source_file.path
            ));
        }
        has_main |= source_file.path.eq_ignore_ascii_case(&manifest.main_file);
    }
    if !has_main {
        return Err(format!(
            "E3S native payload is missing MainFile source: {}",
            manifest.main_file
        ));
    }
    manifest.editor_compatibility = None;

    let source_target = native_root.join(path_from_manifest(&manifest.source_map));
    let source_parent = source_target
        .parent()
        .ok_or_else(|| "native sourceMap has no parent".to_string())?;
    fs::create_dir_all(source_parent).map_err(stringify_io)?;
    fs::copy(&legacy.source_map, &source_target).map_err(|error| {
        format!(
            "cannot copy referenced source map '{}' to '{}': {error}",
            legacy.source_map.display(),
            source_target.display()
        )
    })?;

    let mut project = NativeProject::create(native_root, manifest)?;
    for source_file in sources {
        project.write_source(&source_file.path, &source_file.content)?;
    }
    project.replace_dat_state(dat)?;
    let compatibility_dir = project.root().join("compat");
    fs::create_dir_all(&compatibility_dir).map_err(stringify_io)?;
    let base_path = compatibility_dir.join("editor-project.e3s");
    base.write_exact(&base_path)?;
    project.set_editor_compatibility(Some(EditorCompatibility {
        source_e3s: "compat/editor-project.e3s".to_string(),
        source_sha256: base.sha256(),
        opaque_records: legacy.opaque_records,
    }))?;
    Ok(project)
}

/// Export an imported native project to a semantically updated, Editor-readable E3S.
pub fn export_e3s(
    project: &NativeProject,
    destination: &Path,
    compat_root: &Path,
) -> Result<(), String> {
    let compatibility = project
        .manifest()
        .editor_compatibility
        .as_ref()
        .ok_or_else(|| {
            "E3S export requires an imported compatibility base; create/open/build remain native"
                .to_string()
        })?;
    let base_path = project
        .root()
        .join(path_from_manifest(&compatibility.source_e3s));
    let base = NrbfDocument::read(&base_path)?.without_native_payload()?;
    let catalog = DatCatalog::load(compat_root)?;
    let destination_parent = destination
        .parent()
        .ok_or_else(|| "E3S destination has no parent".to_string())?;
    fs::create_dir_all(destination_parent).map_err(stringify_io)?;
    let stem = destination
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("eud-project");
    let project_source_map = project.source_map_path()?;
    let source_extension = project_source_map
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("scx");
    let referenced_source = destination_parent.join(format!("{stem}.source.{source_extension}"));
    fs::copy(&project_source_map, &referenced_source).map_err(stringify_io)?;
    let project_output_map = project.output_map_path()?;
    let output_extension = project_output_map
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("scx");
    let referenced_output = destination_parent.join(format!("{stem}.output.{output_extension}"));

    let mut graph = base.graph.clone();
    apply_project_to_graph(
        &mut graph,
        project,
        &catalog,
        &referenced_source,
        &referenced_output,
    )?;
    remove_native_payload(&mut graph);
    let payload = NativeE3sPayload::from_project(project, base.sha256())?;
    append_native_payload(&mut graph, &payload)?;
    let bytes = graph.to_bytes()?;
    Document::parse(&bytes)?;
    write_atomic_bytes(destination, &bytes).map_err(|error| error.to_string())
}

fn project_from_graph(
    graph: &Document,
    source_e3s: &Path,
    catalog: &DatCatalog,
) -> Result<LegacyProjection, String> {
    let root = graph.root_id()?;
    if graph.class_name(root)? != SAVEABLE_DATA {
        return Err(format!(
            "E3S root is {}, expected {SAVEABLE_DATA}",
            graph.class_name(root)?
        ));
    }
    let source_map = resolve_legacy_source_map(graph, root, source_e3s)?;
    let output_name = graph
        .field_string(root, "mSaveMapName")?
        .and_then(|value| Path::new(value).file_name())
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("output.scx");
    let source_name = source_map
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "legacy source map filename is not Unicode".to_string())?;

    let te_data = required_field_object(graph, root, "TEData")?;
    let main_object = required_field_object(graph, te_data, "_MainFile")?;
    let project_file = required_field_object(graph, te_data, "ProjectFile")?;
    let mut sources = Vec::new();
    let mut source_paths = BTreeMap::new();
    let mut opaque_records = Vec::new();
    collect_te_sources(
        graph,
        project_file,
        "",
        main_object,
        &mut sources,
        &mut source_paths,
        &mut opaque_records,
    )?;
    let main_file = source_paths.get(&main_object).cloned().ok_or_else(|| {
        "legacy MainFile is not a supported CUI epScript source; GUI/Classic main files are rejected"
            .to_string()
    })?;

    let mut settings = ProjectSettings::default();
    settings.use_custom_tbl = graph.field_bool(root, "mUseCustomTbl")?;
    let eds = required_field_object(graph, root, "EdsBlocks")?;
    let (plugins, eds_settings) = extract_plugins(graph, eds)?;
    merge_project_settings(&mut settings, &eds_settings)?;

    let name = source_e3s
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("Imported E3S")
        .to_string();
    let manifest = ProjectManifest {
        schema_version: PROJECT_SCHEMA_VERSION,
        name,
        source_map: format!("maps/{source_name}"),
        output_map: format!("build/{output_name}"),
        main_file,
        settings,
        plugins,
        python_entrypoints: Vec::new(),
        python_dependencies: Vec::new(),
        python_lock: None,
        editor_compatibility: None,
    };
    let dat = extract_dat(graph, root, catalog)?;
    Ok(LegacyProjection {
        manifest,
        dat,
        sources,
        source_map,
        opaque_records,
    })
}

fn extract_dat(
    graph: &Document,
    root: i32,
    catalog: &DatCatalog,
) -> Result<NativeDatState, String> {
    let mut state = NativeDatState::default();
    extract_standard_dat(
        graph,
        required_field_object(graph, root, "Dat")?,
        catalog,
        &mut state,
    )?;
    extract_extra_dat(
        graph,
        required_field_object(graph, root, "ExtraDat")?,
        catalog,
        &mut state,
    )?;
    Ok(state)
}

fn extract_standard_dat(
    graph: &Document,
    dat: i32,
    catalog: &DatCatalog,
    state: &mut NativeDatState,
) -> Result<(), String> {
    let dat_files = list_object_ids(graph, required_field_object(graph, dat, "Datfile")?)?;
    for dat_file in dat_files {
        let table = required_string(graph, dat_file, "FIleName")?.to_string();
        let parameters =
            list_object_ids(graph, required_field_object(graph, dat_file, "Paramaters")?)?;
        for parameter in parameters {
            let field = required_string(graph, parameter, "ParamaterName")?.to_string();
            let var_start = u32::try_from(graph.field_i64(parameter, "VarStart")?)
                .map_err(|_| format!("negative VarStart for {table}.{field}"))?;
            let init_var = graph.field_i64(parameter, "InitVar")?;
            let values =
                list_object_ids(graph, required_field_object(graph, parameter, "Values")?)?;
            let supported = catalog.field(&table, &field).is_ok();
            for (local, value) in values.into_iter().enumerate() {
                if graph.field_bool(value, "_IsDefault")? {
                    continue;
                }
                if !supported {
                    return Err(format!(
                        "legacy E3S modifies unsupported DAT field {table}.{field}; import refused"
                    ));
                }
                let object_id = var_start
                    .checked_add(
                        u32::try_from(local).map_err(|_| "DAT object id overflow".to_string())?,
                    )
                    .ok_or_else(|| "DAT object id overflow".to_string())?;
                let before = catalog.numeric_value(&table, object_id, &field)?;
                let after = graph.field_i64(value, "_Data")? + init_var;
                if before != after {
                    state
                        .standard
                        .tables
                        .entry(table.clone())
                        .or_default()
                        .entry(object_id)
                        .or_default()
                        .insert(field.clone(), NumericOverride { before, after });
                }
            }
        }
    }
    Ok(())
}

fn extract_extra_dat(
    graph: &Document,
    extra: i32,
    catalog: &DatCatalog,
    state: &mut NativeDatState,
) -> Result<(), String> {
    for (table, field, values_name, defaults_name) in [
        ("wireframe", "wire", "_WireFrame", "_DefaultWireFrame"),
        ("wireframe", "grp", "_GrpFrame", "_DefaultGrpFrame"),
        ("wireframe", "tran", "_TranFrame", "_DefaultTranFrame"),
        ("ButtonSet", "ButtonSet", "_ButtonSet", "_DefaultButtonSet"),
        ("statusinfor", "Status", "_statusFn1", "_statusFn1IsDefault"),
        (
            "statusinfor",
            "Display",
            "_statusFn2",
            "_statusFn2IsDefault",
        ),
    ] {
        let values = primitive_array(graph, extra, values_name)?;
        let defaults = primitive_array(graph, extra, defaults_name)?;
        if values.len() != defaults.len() {
            return Err(format!(
                "legacy XDAT arrays differ in length for {table}.{field}"
            ));
        }
        for (index, (value, is_default)) in values.iter().zip(defaults).enumerate() {
            if is_default.as_bool()? {
                continue;
            }
            let object_id = u32::try_from(index).unwrap();
            let before = catalog.xdat_value(table, object_id, field)?;
            let after = value.as_i64()?;
            if before != after {
                state
                    .xdat
                    .tables
                    .entry(table.to_string())
                    .or_default()
                    .entry(object_id)
                    .or_default()
                    .insert(field.to_string(), NumericOverride { before, after });
            }
        }
    }

    let tbl_array = required_field_object(graph, extra, "_Stat_txt")?;
    for (index, record) in graph.array_records(tbl_array)?.iter().enumerate() {
        let Some(value) = record_string(graph, record)? else {
            continue;
        };
        if value == NULL_TBL_STRING {
            continue;
        }
        let index = u32::try_from(index).unwrap();
        let before = catalog.tbl_value(index)?.to_string();
        if value != before {
            state.tbl.values.insert(
                index,
                TextOverride {
                    before,
                    after: value.to_string(),
                },
            );
        }
    }

    extract_requirements(graph, extra, catalog, &mut state.requirements)?;
    extract_buttons(graph, extra, catalog, &mut state.buttons)?;
    Ok(())
}

fn extract_requirements(
    graph: &Document,
    extra: i32,
    catalog: &DatCatalog,
    target: &mut RequirementDocument,
) -> Result<(), String> {
    let tables = ["units", "upgrades", "techdata", "Stechdata", "orders"];
    let require_array = required_field_object(graph, extra, "RequireDatas")?;
    let records = graph.array_records(require_array)?;
    if records.len() != tables.len() {
        return Err("legacy requirement table count is not 5".to_string());
    }
    for (table, record) in tables.into_iter().zip(&records) {
        let require_data = required_record_id(graph, record)?;
        let objects = list_object_ids(
            graph,
            required_field_object(graph, require_data, "RequireDatas")?,
        )?;
        for (index, object) in objects.into_iter().enumerate() {
            let object_id = u32::try_from(index).unwrap();
            let after = requirement_payload(graph, object)?;
            let before = catalog.requirement_payload(table, object_id)?;
            if after != before {
                target
                    .tables
                    .entry(table.to_string())
                    .or_default()
                    .insert(object_id, TextOverride { before, after });
            }
        }
    }
    Ok(())
}

fn requirement_payload(graph: &Document, object: i32) -> Result<String, String> {
    let mode = graph.field_i64(object, "_UseStatus")?;
    if !matches!(mode, 0 | 4) {
        return Ok(mode.to_string());
    }
    let blocks = list_object_ids(
        graph,
        required_field_object(graph, object, "ReauireBlocks")?,
    )?;
    let mut output = String::from("4");
    for block in blocks {
        let opcode = graph.field_i64(block, "_opCode")?;
        let value = graph.field_i64(block, "_value")?;
        output.push('.');
        output.push_str(&opcode.to_string());
        if matches!(opcode, 0 | 2 | 3 | 4 | 37) {
            output.push(',');
            output.push_str(&value.to_string());
        }
    }
    Ok(output)
}

fn extract_buttons(
    graph: &Document,
    extra: i32,
    catalog: &DatCatalog,
    target: &mut TextDatDocument,
) -> Result<(), String> {
    let button_sets = required_field_object(graph, extra, "_ButtonData")?;
    let array = required_field_object(graph, button_sets, "ButtonSets")?;
    for (set_id, record) in graph.array_records(array)?.iter().enumerate() {
        let set = required_record_id(graph, record)?;
        let buttons = list_object_ids(graph, required_field_object(graph, set, "pButtonSets")?)?;
        let mut rows = Vec::with_capacity(buttons.len());
        for button in buttons {
            rows.push(
                [
                    "_pos", "_icon", "_con", "_act", "_conval", "_actval", "_enaStr", "_disStr",
                ]
                .into_iter()
                .map(|field| {
                    graph
                        .field_i64(button, field)
                        .map(|value| value.to_string())
                })
                .collect::<Result<Vec<_>, _>>()?
                .join(","),
            );
        }
        let after = rows.join(".");
        let set_id = u32::try_from(set_id).unwrap();
        let before = catalog.button_csv(set_id)?;
        if after != before {
            target.values.insert(set_id, TextOverride { before, after });
        }
    }
    Ok(())
}

fn collect_te_sources(
    graph: &Document,
    file: i32,
    prefix: &str,
    main_object: i32,
    sources: &mut Vec<NativeSourceFile>,
    paths: &mut BTreeMap<i32, String>,
    opaque: &mut Vec<OpaqueEditorRecord>,
) -> Result<(), String> {
    let file_type = graph.field_i64(file, "_FileType")?;
    let name = required_string(graph, file, "_FileName")?;
    match file_type {
        TE_FOLDER => {
            let next_prefix = if graph.field_bool(file, "IsTopFile")? {
                prefix.to_string()
            } else {
                join_path(prefix, name)
            };
            for child in list_object_ids(graph, required_field_object(graph, file, "_Files")?)? {
                collect_te_sources(
                    graph,
                    child,
                    &next_prefix,
                    main_object,
                    sources,
                    paths,
                    opaque,
                )?;
            }
            for child in list_object_ids(graph, required_field_object(graph, file, "_Folders")?)? {
                collect_te_sources(
                    graph,
                    child,
                    &next_prefix,
                    main_object,
                    sources,
                    paths,
                    opaque,
                )?;
            }
        }
        TE_CUI_EPS => {
            let mut leaf = name.to_string();
            if !leaf.to_ascii_lowercase().ends_with(".eps") {
                leaf.push_str(".eps");
            }
            let path = format!("src/{}", join_path(prefix, &leaf));
            let scripter = required_field_object(graph, file, "_Scripter")?;
            if graph.class_name(scripter)? != CUI_SCRIPT_EDITOR {
                return Err(format!("CUI source {path} has unsupported scripter type"));
            }
            let content = graph
                .field_string(scripter, "_String")?
                .unwrap_or_default()
                .to_string();
            paths.insert(file, path.clone());
            sources.push(NativeSourceFile {
                sha256: format!("{:x}", Sha256::digest(content.as_bytes())),
                path,
                content,
            });
        }
        TE_CLASSIC | TE_SETTING => {
            opaque.push(OpaqueEditorRecord {
                record_id: u32::try_from(file).unwrap_or_default(),
                record_type: if file_type == TE_CLASSIC {
                    "ClassicTrigger/opaque"
                } else {
                    "Setting/opaque"
                }
                .to_string(),
                encoded: BASE64_STANDARD.encode(graph.class_name(file)?.as_bytes()),
            });
            if file == main_object {
                return Err(
                    "legacy MainFile is ClassicTrigger; lossless native import is unavailable"
                        .to_string(),
                );
            }
        }
        TE_GUI_EPS | TE_GUI_PY | TE_RAW_TEXT | _ => {
            return Err(format!(
                "legacy source '{name}' uses unsupported EFileType {file_type}; import refused"
            ));
        }
    }
    Ok(())
}

fn extract_plugins(
    graph: &Document,
    eds: i32,
) -> Result<(Vec<EdsPlugin>, BTreeMap<String, String>), String> {
    let blocks = list_object_ids(graph, required_field_object(graph, eds, "pBlocks")?)?;
    let mut plugins = Vec::new();
    let mut settings = BTreeMap::new();
    let mut sections = BTreeSet::new();
    for block in blocks {
        if graph.field_i64(block, "BType")? != 6 {
            continue;
        }
        let text = graph
            .field_string(block, "pTexts")?
            .unwrap_or_default()
            .replace("\r\n", "\n");
        let first = text.lines().map(str::trim).find(|line| !line.is_empty());
        if first.is_some_and(|line| line.starts_with('[') && line.ends_with(']')) {
            let section = first
                .and_then(|line| line.strip_prefix('['))
                .and_then(|line| line.strip_suffix(']'))
                .unwrap()
                .trim()
                .to_string();
            if section.is_empty() || section.eq_ignore_ascii_case("main") {
                return Err(format!("invalid legacy EDS plugin section [{section}]"));
            }
            if !sections.insert(section.to_ascii_lowercase()) {
                return Err(format!("duplicate legacy EDS plugin section [{section}]"));
            }
            plugins.push(EdsPlugin {
                section,
                entries: Vec::new(),
                raw_text: Some(text),
            });
        } else {
            for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
                let (key, value) = line
                    .split_once(':')
                    .ok_or_else(|| format!("unsupported legacy [main] setting line: {line}"))?;
                settings.insert(key.trim().to_string(), value.trim().to_string());
            }
        }
    }
    Ok((plugins, settings))
}

fn merge_project_settings(
    target: &mut ProjectSettings,
    values: &BTreeMap<String, String>,
) -> Result<(), String> {
    for (key, value) in values {
        match key.to_ascii_lowercase().as_str() {
            "shufflepayload" => target.shuffle_payload = parse_bool(value)?,
            "debug" => target.debug = parse_bool(value)?,
            "decodeunitname" => target.decode_unit_name = Some(value.clone()),
            "objectfieldcount" => {
                target.object_field_count = Some(
                    value
                        .parse()
                        .map_err(|_| format!("objectFieldCount is not an integer: {value}"))?,
                )
            }
            "sectorsize" => {
                target.sector_size = value
                    .parse()
                    .map_err(|_| format!("sectorSize is not an integer: {value}"))?
            }
            other => {
                return Err(format!(
                    "unsupported legacy [main] setting '{other}'; import refused"
                ))
            }
        }
    }
    Ok(())
}

fn apply_project_to_graph(
    graph: &mut Document,
    project: &NativeProject,
    catalog: &DatCatalog,
    source_map: &Path,
    output_map: &Path,
) -> Result<(), String> {
    let root = graph.root_id()?;
    graph.set_field_string(root, "mOpenMapName", &source_map.to_string_lossy())?;
    graph.set_field_string(root, "mSaveMapName", &output_map.to_string_lossy())?;
    graph.set_field_string(
        root,
        "mRelativeOpenMapName",
        source_map
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default(),
    )?;
    graph.set_field_string(
        root,
        "mRelativeSaveMapName",
        output_map
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default(),
    )?;
    graph.set_field_bool(
        root,
        "mUseCustomTbl",
        project.manifest().settings.use_custom_tbl,
    )?;
    apply_standard_dat(
        graph,
        required_field_object(graph, root, "Dat")?,
        project.dat(),
        catalog,
    )?;
    apply_extra_dat(
        graph,
        required_field_object(graph, root, "ExtraDat")?,
        project.dat(),
        catalog,
    )?;
    apply_sources(
        graph,
        required_field_object(graph, root, "TEData")?,
        project,
    )?;
    apply_plugins(
        graph,
        required_field_object(graph, root, "EdsBlocks")?,
        project.manifest(),
    )?;
    Ok(())
}

fn apply_standard_dat(
    graph: &mut Document,
    dat: i32,
    state: &NativeDatState,
    catalog: &DatCatalog,
) -> Result<(), String> {
    let dat_files = list_object_ids(graph, required_field_object(graph, dat, "Datfile")?)?;
    for dat_file in dat_files {
        let table = required_string(graph, dat_file, "FIleName")?.to_string();
        let parameters =
            list_object_ids(graph, required_field_object(graph, dat_file, "Paramaters")?)?;
        for parameter in parameters {
            let field = required_string(graph, parameter, "ParamaterName")?.to_string();
            let Ok(meta) = catalog.field(&table, &field) else {
                continue;
            };
            let var_start = u32::try_from(graph.field_i64(parameter, "VarStart")?)
                .map_err(|_| "negative DAT VarStart".to_string())?;
            let init_var = graph.field_i64(parameter, "InitVar")?;
            let values =
                list_object_ids(graph, required_field_object(graph, parameter, "Values")?)?;
            for (local, value) in values.into_iter().enumerate() {
                let object_id = var_start + u32::try_from(local).unwrap();
                let before = meta.baseline_value(object_id)?;
                let override_value = state
                    .standard
                    .tables
                    .get(&table)
                    .and_then(|objects| objects.get(&object_id))
                    .and_then(|fields| fields.get(&field));
                if let Some(change) = override_value {
                    if change.before != before {
                        return Err(format!(
                            "stale native DAT baseline {table}.{object_id}.{field}"
                        ));
                    }
                    graph.set_field_i64(value, "_Data", change.after - init_var)?;
                    graph.set_field_bool(value, "_IsDefault", false)?;
                } else {
                    graph.set_field_i64(value, "_Data", before - init_var)?;
                    graph.set_field_bool(value, "_IsDefault", true)?;
                }
            }
        }
    }
    Ok(())
}

fn apply_extra_dat(
    graph: &mut Document,
    extra: i32,
    state: &NativeDatState,
    catalog: &DatCatalog,
) -> Result<(), String> {
    for (table, field, values_name, defaults_name) in [
        ("wireframe", "wire", "_WireFrame", "_DefaultWireFrame"),
        ("wireframe", "grp", "_GrpFrame", "_DefaultGrpFrame"),
        ("wireframe", "tran", "_TranFrame", "_DefaultTranFrame"),
        ("ButtonSet", "ButtonSet", "_ButtonSet", "_DefaultButtonSet"),
        ("statusinfor", "Status", "_statusFn1", "_statusFn1IsDefault"),
        (
            "statusinfor",
            "Display",
            "_statusFn2",
            "_statusFn2IsDefault",
        ),
    ] {
        let values_id = required_field_object(graph, extra, values_name)?;
        let defaults_id = required_field_object(graph, extra, defaults_name)?;
        let length = graph.array_primitives(values_id)?.len();
        if graph.array_primitives(defaults_id)?.len() != length {
            return Err(format!("XDAT array length mismatch for {table}.{field}"));
        }
        for index in 0..length {
            let object_id = u32::try_from(index).unwrap();
            let before = catalog.xdat_value(table, object_id, field)?;
            let change = state
                .xdat
                .tables
                .get(table)
                .and_then(|objects| objects.get(&object_id))
                .and_then(|fields| fields.get(field));
            let after = if let Some(change) = change {
                if change.before != before {
                    return Err(format!(
                        "stale native XDAT baseline {table}.{object_id}.{field}"
                    ));
                }
                change.after
            } else {
                before
            };
            graph.array_primitives_mut(values_id)?[index].set_i64(after)?;
            graph.array_primitives_mut(defaults_id)?[index] = Primitive::boolean(change.is_none());
        }
    }

    let tbl_id = required_field_object(graph, extra, "_Stat_txt")?;
    let tbl_length = graph.array_records(tbl_id)?.len();
    let mut tbl_records = Vec::with_capacity(tbl_length);
    for index in 0..tbl_length {
        let index = u32::try_from(index).unwrap();
        let text = match state.tbl.values.get(&index) {
            Some(change) => {
                if change.before != catalog.tbl_value(index)? {
                    return Err(format!("stale native TBL baseline {index}"));
                }
                change.after.as_str()
            }
            None => NULL_TBL_STRING,
        };
        let id = graph.add_string(text.to_string())?;
        tbl_records.push(Record::Reference(id));
    }
    graph.set_array_records(tbl_id, tbl_records)?;
    apply_requirements(graph, extra, state, catalog)?;
    apply_buttons(graph, extra, state, catalog)?;
    Ok(())
}

fn apply_requirements(
    graph: &mut Document,
    extra: i32,
    state: &NativeDatState,
    catalog: &DatCatalog,
) -> Result<(), String> {
    let tables = ["units", "upgrades", "techdata", "Stechdata", "orders"];
    let records = graph.array_records(required_field_object(graph, extra, "RequireDatas")?)?;
    for (table, record) in tables.into_iter().zip(&records) {
        let require_data = required_record_id(graph, record)?;
        let current = list_object_ids(
            graph,
            required_field_object(graph, require_data, "RequireDatas")?,
        )?;
        let original = list_object_ids(
            graph,
            required_field_object(graph, require_data, "OrigRequireDatas")?,
        )?;
        if current.len() != original.len() {
            return Err(format!(
                "legacy requirement object count mismatch for {table}"
            ));
        }
        for (index, (object, original_object)) in current.into_iter().zip(original).enumerate() {
            let object_id = u32::try_from(index).unwrap();
            let baseline = catalog.requirement_payload(table, object_id)?;
            let resolved = match state
                .requirements
                .tables
                .get(table)
                .and_then(|objects| objects.get(&object_id))
            {
                Some(change) => {
                    if change.before != baseline {
                        return Err(format!(
                            "stale native requirement baseline {table}.{object_id}"
                        ));
                    }
                    change.after.clone()
                }
                None => baseline,
            };
            set_requirement(graph, object, original_object, &resolved)?;
        }
    }
    Ok(())
}

fn set_requirement(
    graph: &mut Document,
    object: i32,
    original: i32,
    payload: &str,
) -> Result<(), String> {
    let segments = payload.split('.').collect::<Vec<_>>();
    let mode = segments
        .first()
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| (0..=4).contains(value))
        .ok_or_else(|| format!("invalid requirement payload: {payload}"))?;
    let baseline_blocks = || requirement_block_pairs(graph, original);
    let (editor_mode, blocks) = match mode {
        0 => (0, baseline_blocks()?),
        1 | 2 => (mode, Vec::new()),
        3 => (
            3,
            baseline_blocks()?
                .into_iter()
                .filter(|(opcode, _)| *opcode == 2)
                .collect(),
        ),
        4 => (4, parse_requirement_blocks(&segments[1..])?),
        _ => unreachable!(),
    };
    graph.set_field_i64(object, "_UseStatus", editor_mode)?;
    let original_start = graph.field_i64(original, "_StartPos")?;
    graph.set_field_i64(
        object,
        "_StartPos",
        if mode == 1 { 0 } else { original_start.max(1) },
    )?;
    let list = required_field_object(graph, object, "ReauireBlocks")?;
    let block_meta = graph.metadata_id(REQUIRE_BLOCK)?;
    let opcode_meta = graph.metadata_id(REQUIRE_OPCODE)?;
    let mut records = Vec::with_capacity(blocks.len());
    for (opcode, value) in blocks {
        let opcode_object = graph.add_enum(opcode_meta, i32::try_from(opcode).unwrap())?;
        let block = graph.add_class(
            block_meta,
            BTreeMap::from([
                (
                    "_opCode".to_string(),
                    FieldValue::Record(Record::Reference(opcode_object)),
                ),
                (
                    "_value".to_string(),
                    FieldValue::Primitive(Primitive {
                        kind: 2,
                        data: vec![u8::try_from(value)
                            .map_err(|_| "requirement value exceeds Byte".to_string())?],
                    }),
                ),
            ]),
        )?;
        records.push(Record::Reference(block));
    }
    graph.set_list_records(list, records)
}

fn requirement_block_pairs(graph: &Document, object: i32) -> Result<Vec<(i64, i64)>, String> {
    list_object_ids(
        graph,
        required_field_object(graph, object, "ReauireBlocks")?,
    )?
    .into_iter()
    .map(|block| {
        Ok((
            graph.field_i64(block, "_opCode")?,
            graph.field_i64(block, "_value")?,
        ))
    })
    .collect()
}

fn parse_requirement_blocks(segments: &[&str]) -> Result<Vec<(i64, i64)>, String> {
    segments
        .iter()
        .map(|segment| {
            let mut values = segment.split(',');
            let opcode = values
                .next()
                .and_then(|value| value.parse::<i64>().ok())
                .ok_or_else(|| format!("invalid requirement opcode: {segment}"))?;
            let value = values
                .next()
                .map(|value| value.parse::<i64>())
                .transpose()
                .map_err(|_| format!("invalid requirement value: {segment}"))?
                .unwrap_or(255);
            if values.next().is_some() {
                return Err(format!("invalid requirement block: {segment}"));
            }
            Ok((opcode, value))
        })
        .collect()
}

fn apply_buttons(
    graph: &mut Document,
    extra: i32,
    state: &NativeDatState,
    catalog: &DatCatalog,
) -> Result<(), String> {
    let button_sets = required_field_object(graph, extra, "_ButtonData")?;
    let records = graph.array_records(required_field_object(graph, button_sets, "ButtonSets")?)?;
    let button_meta = graph.metadata_id(BUTTON_DATA)?;
    for (set_id, record) in records.iter().enumerate() {
        let set = required_record_id(graph, record)?;
        let set_id = u32::try_from(set_id).unwrap();
        let baseline = catalog.button_csv(set_id)?;
        let resolved = match state.buttons.values.get(&set_id) {
            Some(change) => {
                if change.before != baseline {
                    return Err(format!("stale native button baseline {set_id}"));
                }
                change.after.clone()
            }
            None => baseline,
        };
        let list = required_field_object(graph, set, "pButtonSets")?;
        let mut button_records = Vec::new();
        if !resolved.is_empty() {
            for row in resolved.split('.') {
                let fields = row
                    .split(',')
                    .map(|value| {
                        value
                            .parse::<i64>()
                            .map_err(|_| format!("invalid button row: {row}"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if fields.len() != 8 {
                    return Err(format!("button row must have 8 fields: {row}"));
                }
                let names = [
                    "_pos", "_icon", "_con", "_act", "_conval", "_actval", "_enaStr", "_disStr",
                ];
                let mut values = BTreeMap::new();
                let metadata = graph.metadata.get(&button_meta).unwrap();
                for ((name, value), member) in
                    names.into_iter().zip(fields).zip(&metadata.member_types)
                {
                    let kind = member
                        .primitive_kind()
                        .ok_or_else(|| format!("button field {name} is not primitive"))?;
                    let mut primitive = zero_for_kind(kind)?;
                    primitive.set_i64(value)?;
                    values.insert(name.to_string(), FieldValue::Primitive(primitive));
                }
                let button = graph.add_class(button_meta, values)?;
                button_records.push(Record::Reference(button));
            }
        }
        graph.set_list_records(list, button_records)?;
        graph.set_field_bool(
            set,
            "_DefaultUse",
            !state.buttons.values.contains_key(&set_id),
        )?;
    }
    Ok(())
}

fn apply_sources(
    graph: &mut Document,
    te_data: i32,
    project: &NativeProject,
) -> Result<(), String> {
    let root_file = required_field_object(graph, te_data, "ProjectFile")?;
    let existing_files =
        list_object_ids(graph, required_field_object(graph, root_file, "_Files")?)?;
    let existing_folders =
        list_object_ids(graph, required_field_object(graph, root_file, "_Folders")?)?;
    let retained_files = existing_files
        .into_iter()
        .filter(|id| {
            graph
                .field_i64(*id, "_FileType")
                .is_ok_and(|kind| kind == TE_CLASSIC)
        })
        .map(Record::Reference)
        .collect::<Vec<_>>();
    let retained_folders = existing_folders
        .into_iter()
        .filter(|id| {
            graph
                .field_i64(*id, "_FileType")
                .is_ok_and(|kind| kind == TE_SETTING)
        })
        .map(Record::Reference)
        .collect::<Vec<_>>();

    let snapshot = project.source_snapshot()?;
    let mut tree = SourceTree::default();
    for source in &snapshot.files {
        let relative = source
            .path
            .strip_prefix("src/")
            .ok_or_else(|| format!("native source is outside src/: {}", source.path))?;
        tree.insert(relative, source.content.clone())?;
    }
    let mut main_object = None;
    let (mut generated_files, mut generated_folders) = create_tree_children(
        graph,
        root_file,
        "",
        &tree,
        &snapshot.main_file,
        &mut main_object,
    )?;
    let mut files = retained_files;
    files.append(&mut generated_files);
    let mut folders = retained_folders;
    folders.append(&mut generated_folders);
    graph.set_list_records(required_field_object(graph, root_file, "_Files")?, files)?;
    graph.set_list_records(
        required_field_object(graph, root_file, "_Folders")?,
        folders,
    )?;
    graph.set_field_record(
        te_data,
        "_MainFile",
        Record::Reference(
            main_object.ok_or_else(|| "native MainFile was not exported".to_string())?,
        ),
    )?;
    Ok(())
}

#[derive(Debug, Default)]
struct SourceTree {
    files: BTreeMap<String, String>,
    folders: BTreeMap<String, SourceTree>,
}

impl SourceTree {
    fn insert(&mut self, path: &str, content: String) -> Result<(), String> {
        let mut parts = path.split('/').filter(|part| !part.is_empty()).peekable();
        let mut current = self;
        while let Some(part) = parts.next() {
            if parts.peek().is_none() {
                if current.files.insert(part.to_string(), content).is_some() {
                    return Err(format!("duplicate native source path: {path}"));
                }
                return Ok(());
            }
            current = current.folders.entry(part.to_string()).or_default();
        }
        Err("empty native source path".to_string())
    }
}

fn create_tree_children(
    graph: &mut Document,
    parent: i32,
    prefix: &str,
    tree: &SourceTree,
    main_file: &str,
    main_object: &mut Option<i32>,
) -> Result<(Vec<Record>, Vec<Record>), String> {
    let mut files = Vec::new();
    for (name, content) in &tree.files {
        let full = format!("src/{}", join_path(prefix, name));
        let object = create_te_node(graph, name, TE_CUI_EPS, parent, Some(content))?;
        if full.eq_ignore_ascii_case(main_file) {
            *main_object = Some(object);
        }
        files.push(Record::Reference(object));
    }
    let mut folders = Vec::new();
    for (name, child) in &tree.folders {
        let folder = create_te_node(graph, name, TE_FOLDER, parent, None)?;
        let child_prefix = join_path(prefix, name);
        let (child_files, child_folders) =
            create_tree_children(graph, folder, &child_prefix, child, main_file, main_object)?;
        graph.set_list_records(required_field_object(graph, folder, "_Files")?, child_files)?;
        graph.set_list_records(
            required_field_object(graph, folder, "_Folders")?,
            child_folders,
        )?;
        folders.push(Record::Reference(folder));
    }
    Ok((files, folders))
}

fn create_te_node(
    graph: &mut Document,
    name: &str,
    file_type: i64,
    parent: i32,
    content: Option<&str>,
) -> Result<i32, String> {
    let list_meta = graph.metadata_id(TE_FILE_LIST)?;
    let folders = graph.add_typed_list(list_meta, Vec::new(), TE_FILE, 2)?;
    let files = graph.add_typed_list(list_meta, Vec::new(), TE_FILE, 2)?;
    let file_type_object = graph.add_enum(
        graph.metadata_id(TE_FILE_TYPE)?,
        i32::try_from(file_type).unwrap(),
    )?;
    let name_object = graph.add_string(editor_file_name(name, file_type))?;
    let scripter = if file_type == TE_CUI_EPS {
        let empty = graph.add_string(String::new())?;
        let text = graph.add_string(content.unwrap_or_default().to_string())?;
        let script_type = graph.add_enum(graph.metadata_id(SCRIPT_TYPE)?, 0)?;
        Some(graph.add_class(
            graph.metadata_id(CUI_SCRIPT_EDITOR)?,
            BTreeMap::from([
                ("foldedData".to_string(), FieldValue::Record(Record::Null)),
                (
                    "_String".to_string(),
                    FieldValue::Record(Record::Reference(text)),
                ),
                (
                    "_ConnectFile".to_string(),
                    FieldValue::Record(Record::Reference(empty)),
                ),
                (
                    "_ConnectRelativeFile".to_string(),
                    FieldValue::Record(Record::Reference(empty)),
                ),
                (
                    "ScriptType".to_string(),
                    FieldValue::Record(Record::Reference(script_type)),
                ),
                (
                    "ScriptEditor+ScriptType".to_string(),
                    FieldValue::Record(Record::Reference(script_type)),
                ),
            ]),
        )?)
    } else {
        None
    };
    let object = graph.add_class(
        graph.metadata_id(TE_FILE)?,
        BTreeMap::from([
            (
                "_IsExpaned".to_string(),
                FieldValue::Primitive(Primitive::boolean(false)),
            ),
            (
                "CreateDate".to_string(),
                FieldValue::Primitive(Primitive::date_time(0)),
            ),
            (
                "pLastDate".to_string(),
                FieldValue::Primitive(Primitive::date_time(0)),
            ),
            (
                "LastConnectTimer".to_string(),
                FieldValue::Record(Record::Null),
            ),
            ("_UIBinding".to_string(), FieldValue::Record(Record::Null)),
            (
                "_Scripter".to_string(),
                FieldValue::Record(scripter.map(Record::Reference).unwrap_or(Record::Null)),
            ),
            (
                "IsTopFile".to_string(),
                FieldValue::Primitive(Primitive::boolean(false)),
            ),
            (
                "_Folders".to_string(),
                FieldValue::Record(Record::Reference(folders)),
            ),
            (
                "ParentFolder".to_string(),
                FieldValue::Record(Record::Reference(parent)),
            ),
            (
                "_Files".to_string(),
                FieldValue::Record(Record::Reference(files)),
            ),
            (
                "_FileType".to_string(),
                FieldValue::Record(Record::Reference(file_type_object)),
            ),
            (
                "_FileName".to_string(),
                FieldValue::Record(Record::Reference(name_object)),
            ),
        ]),
    )?;
    let ui = graph.add_class(
        graph.metadata_id(TE_TAB_UI)?,
        BTreeMap::from([(
            "TEFile".to_string(),
            FieldValue::Record(Record::Reference(object)),
        )]),
    )?;
    graph.set_field_record(object, "_UIBinding", Record::Reference(ui))?;
    Ok(object)
}

fn apply_plugins(graph: &mut Document, eds: i32, manifest: &ProjectManifest) -> Result<(), String> {
    let list = required_field_object(graph, eds, "pBlocks")?;
    let existing = list_object_ids(graph, list)?;
    let mut fixed = existing
        .into_iter()
        .filter(|item| graph.field_i64(*item, "BType").is_ok_and(|kind| kind != 6))
        .collect::<Vec<_>>();
    fixed.sort_by_key(|item| graph.field_i64(*item, "BType").unwrap_or(i64::MAX));
    let mut records = Vec::new();
    if let Some(main) = fixed
        .iter()
        .position(|item| graph.field_i64(*item, "BType").ok() == Some(0))
    {
        records.push(Record::Reference(fixed.remove(main)));
    }
    let main_settings = serialize_main_settings(&manifest.settings);
    if !main_settings.is_empty() {
        records.push(Record::Reference(create_eds_item(
            graph,
            6,
            Some(&main_settings),
        )?));
    }
    for plugin in &manifest.plugins {
        let text = match &plugin.raw_text {
            Some(text) => text.clone(),
            None => {
                let mut text = format!("[{}]\n", plugin.section);
                for entry in &plugin.entries {
                    match &entry.value {
                        Some(value) => text.push_str(&format!("{}: {}\n", entry.key, value)),
                        None => text.push_str(&format!("{}\n", entry.key)),
                    }
                }
                text
            }
        };
        records.push(Record::Reference(create_eds_item(graph, 6, Some(&text))?));
    }
    records.extend(fixed.into_iter().map(Record::Reference));
    graph.set_list_records(list, records)
}

fn create_eds_item(graph: &mut Document, kind: i32, text: Option<&str>) -> Result<i32, String> {
    let kind_object = graph.add_enum(graph.metadata_id(EDS_ITEM_TYPE)?, kind)?;
    let text_record = match text {
        Some(text) => Record::Reference(graph.add_string(text.to_string())?),
        None => Record::Null,
    };
    graph.add_class(
        graph.metadata_id(EDS_ITEM)?,
        BTreeMap::from([
            (
                "BType".to_string(),
                FieldValue::Record(Record::Reference(kind_object)),
            ),
            ("pTexts".to_string(), FieldValue::Record(text_record)),
        ]),
    )
}

fn serialize_main_settings(settings: &ProjectSettings) -> String {
    let mut output = String::new();
    if settings.shuffle_payload {
        output.push_str("shufflePayload: True\n");
    }
    if settings.debug {
        output.push_str("debug: True\n");
    }
    if let Some(value) = &settings.decode_unit_name {
        output.push_str(&format!("decodeUnitName: {value}\n"));
    }
    if let Some(value) = settings.object_field_count {
        output.push_str(&format!("objectFieldCount: {value}\n"));
    }
    if settings.sector_size != 15 {
        output.push_str(&format!("sectorSize: {}\n", settings.sector_size));
    }
    output
}

fn native_payload(graph: &Document) -> Result<Option<NativeE3sPayload>, String> {
    for object in graph.objects.values() {
        let ObjectValue::String(value) = &object.value else {
            continue;
        };
        if let Some(json) = value.strip_prefix(EXTENSION_MARKER) {
            return serde_json::from_str(json)
                .map(Some)
                .map_err(|error| format!("invalid native E3S extension: {error}"));
        }
    }
    Ok(None)
}

fn remove_native_payload(graph: &mut Document) {
    let ids = graph
        .objects
        .iter()
        .filter_map(|(id, object)| match &object.value {
            ObjectValue::String(value) if value.starts_with(EXTENSION_MARKER) => Some(*id),
            _ => None,
        })
        .collect::<Vec<_>>();
    for id in ids {
        graph.remove_object_record(id);
    }
}

fn append_native_payload(graph: &mut Document, payload: &NativeE3sPayload) -> Result<(), String> {
    let json = serde_json::to_string(payload).map_err(|error| error.to_string())?;
    let value = format!("{EXTENSION_MARKER}{json}");
    if value.len() > MAX_EXTENSION_BYTES {
        return Err(format!(
            "native E3S extension exceeds {MAX_EXTENSION_BYTES} bytes"
        ));
    }
    graph.add_string(value)?;
    Ok(())
}

fn required_field_object(graph: &Document, object: i32, field: &str) -> Result<i32, String> {
    graph
        .field_object_id(object, field)?
        .ok_or_else(|| format!("NRBF field {field} is null"))
}

fn required_record_id(graph: &Document, record: &Record) -> Result<i32, String> {
    graph
        .resolved_id(record)?
        .ok_or_else(|| "NRBF array contains null where an object is required".to_string())
}

fn required_string<'a>(graph: &'a Document, object: i32, field: &str) -> Result<&'a str, String> {
    graph
        .field_string(object, field)?
        .ok_or_else(|| format!("NRBF string field {field} is null"))
}

fn list_object_ids(graph: &Document, list: i32) -> Result<Vec<i32>, String> {
    graph
        .list_records(list)?
        .iter()
        .map(|record| required_record_id(graph, record))
        .collect()
}

fn primitive_array<'a>(
    graph: &'a Document,
    object: i32,
    field: &str,
) -> Result<&'a [Primitive], String> {
    graph.array_primitives(required_field_object(graph, object, field)?)
}

fn record_string<'a>(graph: &'a Document, record: &Record) -> Result<Option<&'a str>, String> {
    let Some(id) = graph.resolved_id(record)? else {
        return Ok(None);
    };
    match &graph.object(id)?.value {
        ObjectValue::String(value) => Ok(Some(value)),
        _ => Err(format!("NRBF object {id} is not String")),
    }
}

fn editor_file_name(name: &str, file_type: i64) -> String {
    if file_type == TE_CUI_EPS {
        name.strip_suffix(".eps")
            .or_else(|| name.strip_suffix(".EPS"))
            .unwrap_or(name)
            .to_string()
    } else {
        name.to_string()
    }
}

fn join_path(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.to_string()
    } else {
        format!("{parent}/{child}")
    }
}

fn parse_bool(value: &str) -> Result<bool, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => Err(format!("invalid boolean: {value}")),
    }
}

fn resolve_legacy_source_map(
    graph: &Document,
    root: i32,
    source_e3s: &Path,
) -> Result<PathBuf, String> {
    let configured = graph
        .field_string(root, "mOpenMapName")?
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| "legacy E3S has no OpenMapName".to_string())?;
    if configured.is_file() {
        return Ok(configured);
    }
    let parent = source_e3s.parent().unwrap_or_else(|| Path::new("."));
    if let Some(relative) = graph
        .field_string(root, "mRelativeOpenMapName")?
        .filter(|value| !value.trim().is_empty())
    {
        let candidate = parent.join(relative.replace('\\', std::path::MAIN_SEPARATOR_STR));
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    if let Some(file_name) = configured.file_name() {
        let candidate = parent.join(file_name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(format!(
        "legacy E3S referenced source map is missing: {}",
        configured.display()
    ))
}

fn zero_for_kind(kind: u8) -> Result<Primitive, String> {
    let data = match kind {
        1 | 2 | 10 => vec![0],
        7 | 14 => vec![0; 2],
        8 | 11 | 15 => vec![0; 4],
        6 | 9 | 12 | 13 | 16 => vec![0; 8],
        _ => return Err(format!("unsupported numeric NRBF primitive {kind}")),
    };
    Ok(Primitive { kind, data })
}

fn path_from_manifest(value: &str) -> PathBuf {
    PathBuf::from(value.replace('/', std::path::MAIN_SEPARATOR_STR))
}

fn stringify_io(error: std::io::Error) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_project::EdsPluginEntry;

    fn minimal_e3s() -> Vec<u8> {
        vec![
            0, 1, 0, 0, 0, 255, 255, 255, 255, 1, 0, 0, 0, 0, 0, 0, 0, 6, 1, 0, 0, 0, 4, b'b',
            b'a', b's', b'e', 11,
        ]
    }

    #[test]
    fn nrbf_envelope_round_trips_exactly() {
        let document = NrbfDocument::parse(minimal_e3s()).unwrap();
        assert_eq!(document.bytes(), minimal_e3s());
        assert_eq!(document.graph.to_bytes().unwrap(), minimal_e3s());
    }

    #[test]
    fn native_extension_is_parseable_and_removable() {
        let mut document = NrbfDocument::parse(minimal_e3s()).unwrap();
        let payload = NativeE3sPayload {
            schema_version: 1,
            manifest: ProjectManifest {
                schema_version: PROJECT_SCHEMA_VERSION,
                name: "Compat".to_string(),
                source_map: "maps/source.scx".to_string(),
                output_map: "build/output.scx".to_string(),
                main_file: "src/main.eps".to_string(),
                settings: ProjectSettings::default(),
                plugins: vec![EdsPlugin {
                    section: "eudTurbo".to_string(),
                    entries: vec![EdsPluginEntry {
                        key: "t".to_string(),
                        value: None,
                    }],
                    raw_text: None,
                }],
                python_entrypoints: Vec::new(),
                python_dependencies: Vec::new(),
                python_lock: None,
                editor_compatibility: None,
            },
            dat: NativeDatState::default(),
            sources: vec![NativeSourceFile {
                path: "src/main.eps".to_string(),
                content: "function onPluginStart() {}".to_string(),
                sha256: format!("{:x}", Sha256::digest(b"function onPluginStart() {}")),
            }],
            base_sha256: document.sha256(),
        };
        append_native_payload(&mut document.graph, &payload).unwrap();
        document.bytes = document.graph.to_bytes().unwrap();
        assert_eq!(document.native_payload().unwrap(), Some(payload));
        assert_eq!(
            document.without_native_payload().unwrap().bytes(),
            minimal_e3s()
        );
    }

    #[test]
    fn editor_filename_has_exactly_one_eps_extension() {
        assert_eq!(editor_file_name("main.eps", TE_CUI_EPS), "main");
        assert_eq!(editor_file_name("main", TE_CUI_EPS), "main");
        assert_eq!(editor_file_name("notes.txt", TE_RAW_TEXT), "notes.txt");
    }
    #[test]
    #[ignore = "requires EUD_AGENT_E3S_FIXTURE and EUD_AGENT_E3S_COMPAT"]
    fn real_fixture_import_export_import_is_semantically_stable() {
        let fixture = PathBuf::from(std::env::var("EUD_AGENT_E3S_FIXTURE").unwrap());
        let compat = PathBuf::from(std::env::var("EUD_AGENT_E3S_COMPAT").unwrap());
        let original = NrbfDocument::read(&fixture).unwrap();
        assert_eq!(original.graph.to_bytes().unwrap(), original.bytes());

        let root =
            std::env::temp_dir().join(format!("eud-agent-e3s-roundtrip-{}", uuid::Uuid::new_v4()));
        let first_root = root.join("first");
        let second_root = root.join("second");
        let exported = root.join("roundtrip.e3s");
        let first = import_e3s(&fixture, &first_root, &compat).unwrap();
        export_e3s(&first, &exported, &compat).unwrap();
        let second = import_e3s(&exported, &second_root, &compat).unwrap();

        assert_eq!(first.dat(), second.dat());
        assert_eq!(
            first.source_snapshot().unwrap().files,
            second.source_snapshot().unwrap().files
        );
        assert_eq!(first.manifest().main_file, second.manifest().main_file);
        assert_eq!(first.manifest().settings, second.manifest().settings);
        assert_eq!(first.manifest().plugins, second.manifest().plugins);
        fs::remove_dir_all(root).ok();
    }
}
