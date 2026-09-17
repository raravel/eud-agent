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

/// EUD Editor writes every TriggerEditor source into `<eudplibData>/TriggerEditor/`,
/// right next to the generated EDS, so editor-authored bodies address project modules
/// as `import TriggerEditor.<module>`. The name is a build-output folder literal, not
/// a TE tree folder: the only `IsTopFile` node in a real save is the `ProjectMain`
/// root, whose name never appears in an import.
const EDITOR_BUILD_IMPORT_PREFIX: &str = "TriggerEditor";

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
const TE_SETTING: i64 = 5;
const TE_CLASSIC: i64 = 6;
#[cfg(test)]
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
    editor_import_prefixes: Vec<String>,
    rewritten_imports: usize,
}

fn native_payload_has_direct_python(
    manifest: &ProjectManifest,
    sources: &[NativeSourceFile],
) -> bool {
    !manifest.python_entrypoints.is_empty()
        || !manifest.python_dependencies.is_empty()
        || manifest.python_lock.is_some()
        || sources
            .iter()
            .any(|source| source.path.to_ascii_lowercase().ends_with(".py"))
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
        None => {
            if legacy.rewritten_imports > 0 {
                eprintln!(
                    "eud-agent: e3s import rewrote {} epScript import(s) to src-relative form",
                    legacy.rewritten_imports
                );
            }
            (legacy.manifest, legacy.dat, legacy.sources)
        }
    };
    if native_payload_has_direct_python(&manifest, &sources) {
        return Err(
            "직접 Python 상태가 있는 native E3S 확장은 가져올 수 없습니다. E3S 가져오기는 epScript 전용 프로젝트만 지원합니다."
                .to_string(),
        );
    }
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
        editor_import_prefixes: legacy.editor_import_prefixes,
    }))?;
    Ok(project)
}

/// Export an imported native project to a semantically updated, Editor-readable E3S.
pub fn export_e3s(
    project: &NativeProject,
    destination: &Path,
    compat_root: &Path,
) -> Result<(), String> {
    if project.has_direct_python()? {
        return Err("직접 Python 소스 또는 의존성이 있는 프로젝트는 E3S로 내보낼 수 없습니다. E3S는 epScript 전용 프로젝트만 지원합니다.".to_string());
    }
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

/// Repair a project imported before the E3S import rewrite existed. Its `src/**.eps`
/// bodies still address project modules through the editor build prefix, so the native
/// build fails with `No module named 'TriggerEditor'`. Re-importing the E3S would be
/// destructive, so this strips the prefixes in place and returns how many import
/// statements were rewritten.
pub fn migrate_editor_import_prefixes(project: &mut NativeProject) -> Result<usize, String> {
    let mut prefixes = project
        .manifest()
        .editor_compatibility
        .as_ref()
        .map(|compatibility| {
            compatibility
                .editor_import_prefixes
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    prefixes.insert(EDITOR_BUILD_IMPORT_PREFIX.to_string());
    let snapshot = project.source_snapshot()?;
    let modules = eps_module_names(snapshot.files.iter().map(|source| source.path.as_str()));
    let mut rewritten = 0;
    for source in &snapshot.files {
        if !is_eps_path(&source.path) {
            continue;
        }
        let rewrite = strip_editor_import_prefixes(&source.content, &prefixes, &modules);
        if rewrite.count == 0 {
            continue;
        }
        project.write_source(&source.path, &rewrite.content)?;
        rewritten += rewrite.count;
    }
    Ok(rewritten)
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
    let mut collected = CollectedTeSources::default();
    collect_te_sources(graph, project_file, "", true, main_object, &mut collected)?;
    let CollectedTeSources {
        sources,
        paths: source_paths,
        opaque: opaque_records,
        top_folders,
    } = collected;
    // Two passes: a body may import a module collected later in the traversal, and the
    // rewrite only applies to imports that resolve to a module this project owns.
    let (sources, editor_import_prefixes, rewritten_imports) =
        rewrite_editor_imports(sources, &top_folders);
    let main_file = source_paths.get(&main_object).cloned().ok_or_else(|| {
        "legacy MainFile is not a supported CUI epScript source; GUI/Classic main files are rejected"
            .to_string()
    })?;

    let mut settings = ProjectSettings {
        use_custom_tbl: graph.field_bool(root, "mUseCustomTbl")?,
        ..ProjectSettings::default()
    };
    let eds = required_field_object(graph, root, "EdsBlocks")?;
    let (plugins, eds_settings) = extract_plugins(graph, eds, te_data)?;
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
        editor_import_prefixes,
        rewritten_imports,
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

#[derive(Default)]
struct CollectedTeSources {
    sources: Vec<NativeSourceFile>,
    paths: BTreeMap<i32, String>,
    opaque: Vec<OpaqueEditorRecord>,
    top_folders: BTreeSet<String>,
}

fn collect_te_sources(
    graph: &Document,
    file: i32,
    prefix: &str,
    is_root: bool,
    main_object: i32,
    collected: &mut CollectedTeSources,
) -> Result<(), String> {
    let file_type = graph.field_i64(file, "_FileType")?;
    let name = required_string(graph, file, "_FileName")?;
    match file_type {
        TE_FOLDER => {
            let is_top_file = graph.field_bool(file, "IsTopFile")?;
            if is_top_file && !is_root {
                // A non-root top-level folder keeps its children at `src/…`, so its name
                // is an editor-side import prefix rather than a native path segment.
                collected.top_folders.insert(name.to_string());
            }
            let next_prefix = if is_top_file {
                prefix.to_string()
            } else {
                join_path(prefix, name)
            };
            for child in list_object_ids(graph, required_field_object(graph, file, "_Files")?)? {
                collect_te_sources(graph, child, &next_prefix, false, main_object, collected)?;
            }
            for child in list_object_ids(graph, required_field_object(graph, file, "_Folders")?)? {
                collect_te_sources(graph, child, &next_prefix, false, main_object, collected)?;
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
            collected.paths.insert(file, path.clone());
            collected.sources.push(NativeSourceFile {
                // Left empty on purpose: `rewrite_editor_imports` owns the final bytes
                // and hashes them once, after the import rewrite.
                sha256: String::new(),
                path,
                content,
            });
        }
        TE_CLASSIC | TE_SETTING => {
            collected.opaque.push(OpaqueEditorRecord {
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
        _ => {
            return Err(format!(
                "legacy source '{name}' uses unsupported EFileType {file_type}; import refused"
            ));
        }
    }
    Ok(())
}

/// One epScript `import` statement located inside a source body.
#[derive(Debug, PartialEq, Eq)]
struct EpsImportSite {
    /// Byte offset of the first identifier of the dotted name.
    name_start: usize,
    /// Byte offset just past that identifier.
    name_end: usize,
    /// Dotted name with `.` separators, for example `TriggerEditor.sub.deep`.
    dotted: String,
}

/// Outcome of one editor-prefix rewrite pass over a single source body.
#[derive(Debug, PartialEq, Eq)]
struct EpsImportRewrite {
    content: String,
    count: usize,
    prefixes: BTreeSet<String>,
}

/// Locate every well-formed absolute `import <dottedName> [as <name>];` statement.
///
/// epScript has two import productions (`libepScriptLib`):
/// `IMPORT dottedName [AS NAME] SEMICOLON` and the relative
/// `IMPORT PERIOD… [AS NAME] SEMICOLON`. Only the absolute form can carry an editor
/// build prefix, so relative imports are recognised and skipped. String literals and
/// `//` and `/* */` comments are masked first, which leaves editor-authored text such
/// as `"import TriggerEditor.x"` untouched.
fn scan_import_sites(content: &str) -> Vec<EpsImportSite> {
    let bytes = content.as_bytes();
    let mut sites = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'/' && matches!(bytes.get(index + 1), Some(&b'/') | Some(&b'*')) {
            index = skip_trivia(content, index);
            continue;
        }
        if byte == b'"' || byte == b'\'' {
            index = skip_string_literal(bytes, index);
            continue;
        }
        if is_identifier_byte(byte) {
            let start = index;
            index = skip_identifier(content, index);
            if &content[start..index] == "import" && is_keyword_start(bytes, start) {
                if let Some((site, next)) = parse_import_site(content, index) {
                    sites.push(site);
                    index = next;
                }
            }
            continue;
        }
        index += 1;
    }
    sites
}

/// Parse the tail of an absolute import statement starting just past `import`.
/// Returns the site and the offset just past the terminating `;`.
fn parse_import_site(content: &str, cursor: usize) -> Option<(EpsImportSite, usize)> {
    let mut index = skip_trivia(content, cursor);
    if content.as_bytes().get(index) == Some(&b'.') {
        // `import .sibling;` and `import ..folder.mod;` are already src-relative.
        return None;
    }
    let name_start = index;
    index = skip_identifier(content, index);
    if index == name_start {
        return None;
    }
    let name_end = index;
    let mut dotted = content[name_start..name_end].to_string();
    while content.as_bytes().get(index) == Some(&b'.') {
        let next = skip_identifier(content, index + 1);
        if next == index + 1 {
            break;
        }
        dotted.push('.');
        dotted.push_str(&content[index + 1..next]);
        index = next;
    }
    let mut tail = skip_trivia(content, index);
    if let Some((_, after_keyword)) = read_word(content, tail).filter(|(word, _)| *word == "as") {
        let alias_start = skip_trivia(content, after_keyword);
        let alias_end = skip_identifier(content, alias_start);
        if alias_end == alias_start {
            return None;
        }
        tail = skip_trivia(content, alias_end);
    }
    if content.as_bytes().get(tail) != Some(&b';') {
        return None;
    }
    Some((
        EpsImportSite {
            name_start,
            name_end,
            dotted,
        },
        tail + 1,
    ))
}

fn read_word(content: &str, index: usize) -> Option<(&str, usize)> {
    let end = skip_identifier(content, index);
    (end > index).then_some((&content[index..end], end))
}

/// Skip whitespace plus `//` and `/* */` comments, mirroring the lexer's skip and
/// hidden channels. An unterminated block comment consumes the rest of the body, which
/// is the non-destructive reading of invalid source.
fn skip_trivia(content: &str, mut index: usize) -> usize {
    let bytes = content.as_bytes();
    loop {
        match bytes.get(index) {
            Some(byte) if byte.is_ascii_whitespace() => index += 1,
            Some(b'/') if bytes.get(index + 1) == Some(&b'/') => {
                index += 2;
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
            }
            Some(b'/') if bytes.get(index + 1) == Some(&b'*') => {
                index += 2;
                loop {
                    match bytes.get(index) {
                        None => break,
                        Some(b'*') if bytes.get(index + 1) == Some(&b'/') => {
                            index += 2;
                            break;
                        }
                        _ => index += 1,
                    }
                }
            }
            _ => return index,
        }
    }
}

/// Skip a `"…"` or `'…'` literal starting at its opening quote. A raw newline ends an
/// unterminated literal, because epScript string characters exclude `\r` and `\n`.
fn skip_string_literal(bytes: &[u8], start: usize) -> usize {
    let quote = bytes[start];
    let mut index = start + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => {
                index += 1;
                if index < bytes.len() {
                    index += 1;
                    // `\` plus a line terminator is a line continuation, and `\r\n` is
                    // one terminator.
                    if bytes.get(index - 1) == Some(&b'\r') && bytes.get(index) == Some(&b'\n') {
                        index += 1;
                    }
                }
            }
            b'\r' | b'\n' => return index,
            byte if byte == quote => return index + 1,
            _ => index += 1,
        }
    }
    index
}

fn is_identifier_byte(byte: u8) -> bool {
    // Bytes >= 0x80 are UTF-8 lead or continuation bytes. epScript identifiers accept
    // `\p{L}`, so treating every non-ASCII byte as an identifier byte both matches
    // Korean identifiers and guarantees no slice ever lands inside a character.
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$' || byte >= 0x80
}

fn skip_identifier(content: &str, mut index: usize) -> usize {
    let bytes = content.as_bytes();
    while index < bytes.len() && is_identifier_byte(bytes[index]) {
        index += 1;
    }
    index
}

fn is_keyword_start(bytes: &[u8], start: usize) -> bool {
    match start.checked_sub(1) {
        None => true,
        Some(previous) => !is_identifier_byte(bytes[previous]) && bytes[previous] != b'.',
    }
}

/// Dotted module names of the project's own `src/**.eps` files, for example
/// `src/sub/deep.eps` -> `sub.deep`.
fn eps_module_names<'a>(paths: impl Iterator<Item = &'a str>) -> BTreeSet<String> {
    paths
        .filter(|path| is_eps_path(path))
        .filter_map(|path| {
            path.strip_prefix("src/")
                .and_then(|relative| relative.rsplit_once('.'))
                .map(|(stem, _)| stem.replace('/', "."))
        })
        .collect()
}

fn is_eps_path(path: &str) -> bool {
    path.rsplit_once('.')
        .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("eps"))
}

/// Rewrite `import <prefix>.<rest> [as <alias>];` to `import <rest> [as <alias>];`.
///
/// Only imports that resolve to a module this project owns are rewritten, which is what
/// makes the transform exactly invertible: [`restore_editor_import_prefixes`] re-adds
/// the prefix under the same condition. Editor-provided modules outside the native
/// `src/` tree keep their original spelling in both directions.
fn strip_editor_import_prefixes(
    content: &str,
    prefixes: &BTreeSet<String>,
    modules: &BTreeSet<String>,
) -> EpsImportRewrite {
    let bytes = content.as_bytes();
    let mut cuts = Vec::new();
    let mut used = BTreeSet::new();
    for site in scan_import_sites(content) {
        let Some((head, rest)) = site.dotted.split_once('.') else {
            continue;
        };
        if !prefixes.contains(head) || !modules.contains(rest) {
            continue;
        }
        // Require an adjacent dot so the inverse rewrite lands on the same byte offset.
        if bytes.get(site.name_end) != Some(&b'.') {
            continue;
        }
        used.insert(head.to_string());
        cuts.push(site.name_start..site.name_end + 1);
    }
    let count = cuts.len();
    let mut rewritten = String::with_capacity(content.len());
    let mut cursor = 0;
    for cut in cuts {
        rewritten.push_str(&content[cursor..cut.start]);
        cursor = cut.end;
    }
    rewritten.push_str(&content[cursor..]);
    EpsImportRewrite {
        content: rewritten,
        count,
        prefixes: used,
    }
}

/// Inverse of [`strip_editor_import_prefixes`]: `import <rest> [as <alias>];` becomes
/// `import <prefix>.<rest> [as <alias>];` for every `<rest>` naming a module inside the
/// project's own `src/` tree.
fn restore_editor_import_prefixes(
    content: &str,
    prefix: &str,
    modules: &BTreeSet<String>,
) -> String {
    let prefixed = format!("{prefix}.");
    let offsets = scan_import_sites(content)
        .into_iter()
        .filter(|site| modules.contains(site.dotted.as_str()))
        .map(|site| site.name_start)
        .collect::<Vec<_>>();
    let mut restored = content.to_string();
    for offset in offsets.iter().rev() {
        restored.insert_str(*offset, &prefixed);
    }
    restored
}

/// Apply the editor-prefix strip to every imported source and fill in the hashes
/// [`collect_te_sources`] left empty. Returns the sources, the prefixes that were
/// actually removed (persisted so export can restore them), and the rewrite count.
fn rewrite_editor_imports(
    mut sources: Vec<NativeSourceFile>,
    top_folders: &BTreeSet<String>,
) -> (Vec<NativeSourceFile>, Vec<String>, usize) {
    let mut prefixes = top_folders.clone();
    prefixes.insert(EDITOR_BUILD_IMPORT_PREFIX.to_string());
    let modules = eps_module_names(sources.iter().map(|source| source.path.as_str()));
    let mut used = BTreeSet::new();
    let mut total = 0;
    for source in &mut sources {
        let rewrite = strip_editor_import_prefixes(&source.content, &prefixes, &modules);
        total += rewrite.count;
        used.extend(rewrite.prefixes);
        source.content = rewrite.content;
        source.sha256 = format!("{:x}", Sha256::digest(source.content.as_bytes()));
    }
    (sources, used.into_iter().collect(), total)
}

/// Export can invert the import rewrite only when exactly one prefix was stripped. With
/// several candidates the prefix of a given import is not recoverable from the native
/// body, and guessing would emit Editor-unreadable output; leaving bodies untouched
/// still round-trips byte-identically.
fn export_import_prefix(compatibility: &EditorCompatibility) -> Option<&str> {
    match compatibility.editor_import_prefixes.as_slice() {
        [only] => Some(only.as_str()),
        _ => None,
    }
}

fn extract_plugins(
    graph: &Document,
    eds: i32,
    te_data: i32,
) -> Result<(Vec<EdsPlugin>, BTreeMap<String, String>), String> {
    let blocks = list_object_ids(graph, required_field_object(graph, eds, "pBlocks")?)?;
    let mut projection = LegacyEds::default();
    for block in blocks {
        match graph.field_i64(block, "BType")? {
            0 => projection.section = LegacyEdsSection::Main,
            1..=4 => {
                // These sections are regenerated from native DAT/source state. Do not
                // mistake any user continuation after them for a [main] setting.
                projection.section = LegacyEdsSection::Generated;
            }
            5 => {
                if graph.field_bool(te_data, "UseMSQC")? {
                    let mouse = graph
                        .field_string(te_data, "MouseLocation")?
                        .unwrap_or_default();
                    if !mouse.is_empty() {
                        projection.append(&format!("[MSQC]\nmouse : {mouse}\n"))?;
                    }
                }
            }
            6 => projection.append(graph.field_string(block, "pTexts")?.unwrap_or_default())?,
            7 => {
                if graph.field_bool(te_data, "UseChatEvent")? {
                    // MacroPluginManager.GetChatEventCode always emits these addresses.
                    // Headerless UserPlugin blocks following it extend [chatEvent].
                    projection.append("[chatEvent]\n")?;
                    for (field, key, default) in [
                        ("_addr", "__addr__", 0x58D900),
                        ("_ptrAddr", "__ptrAddr__", 0x58D904),
                        ("_patternAddr", "__patternAddr__", 0x58D908),
                        ("_lenAddr", "__lenAddr__", 0x58D90C),
                    ] {
                        let value = graph.field_i64(te_data, field)?;
                        let value = if value == 0 { default } else { value };
                        projection.append(&format!("{key} : 0x{value:X}\n"))?;
                    }
                }
            }
            kind => return Err(format!("unsupported legacy EDS block type {kind}")),
        }
    }
    Ok((projection.plugins, projection.settings))
}

#[derive(Default)]
enum LegacyEdsSection {
    #[default]
    None,
    Main,
    Plugin(usize),
    Generated,
}

#[derive(Default)]
struct LegacyEds {
    plugins: Vec<EdsPlugin>,
    settings: BTreeMap<String, String>,
    section: LegacyEdsSection,
}

impl LegacyEds {
    fn append(&mut self, text: &str) -> Result<(), String> {
        // Editor concatenates block text in order; a block is not necessarily a
        // complete section, and one block may contain several section headers.
        for line in text.lines() {
            let trimmed = line.trim();
            if let Some(section) = trimmed.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                let section = section.trim();
                if section.is_empty() || section.eq_ignore_ascii_case("main") {
                    return Err(format!("invalid legacy EDS plugin section [{section}]"));
                }
                if self
                    .plugins
                    .iter()
                    .any(|plugin| plugin.section.eq_ignore_ascii_case(section))
                {
                    return Err(format!("duplicate legacy EDS plugin section [{section}]"));
                }
                self.section = LegacyEdsSection::Plugin(self.plugins.len());
                self.plugins.push(EdsPlugin {
                    section: section.to_string(),
                    entries: Vec::new(),
                    raw_text: Some(format!("[{section}]\n")),
                });
                continue;
            }
            if let LegacyEdsSection::Plugin(index) = self.section {
                let raw = self.plugins[index].raw_text.as_mut().unwrap();
                raw.push_str(line);
                raw.push('\n');
            } else if trimmed.is_empty() || trimmed.starts_with([';', '#']) {
                continue;
            } else if matches!(self.section, LegacyEdsSection::Main) {
                let (key, value) = trimmed
                    .split_once(':')
                    .ok_or_else(|| format!("unsupported legacy [main] setting line: {line}"))?;
                self.settings
                    .insert(key.trim().to_string(), value.trim().to_string());
            } else {
                return Err(format!(
                    "unsupported legacy EDS continuation outside a user plugin: {line}"
                ));
            }
        }
        Ok(())
    }
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
    // Generated input plugins have been materialized in manifest.plugins. Leaving
    // these switches enabled would emit duplicate sections in Editor on export.
    let te_data = required_field_object(graph, root, "TEData")?;
    graph.set_field_bool(te_data, "UseChatEvent", false)?;
    graph.set_field_bool(te_data, "UseMSQC", false)?;
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
    let export_prefix = project
        .manifest()
        .editor_compatibility
        .as_ref()
        .and_then(export_import_prefix);
    let modules = eps_module_names(snapshot.files.iter().map(|source| source.path.as_str()));
    let mut tree = SourceTree::default();
    for source in &snapshot.files {
        let relative = source
            .path
            .strip_prefix("src/")
            .ok_or_else(|| format!("native source is outside src/: {}", source.path))?;
        let content = match export_prefix {
            Some(prefix) if is_eps_path(&source.path) => {
                restore_editor_import_prefixes(&source.content, prefix, &modules)
            }
            _ => source.content.clone(),
        };
        tree.insert(relative, content)?;
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

    fn eds_fixture(blocks: &[(i32, &str)], chat_enabled: bool) -> (Document, i32, i32) {
        use crate::nrbf::{
            AdditionalTypeInfo, ArrayObject, ClassMetadata, ClassObject, MemberType,
        };

        fn class(graph: &mut Document, name: &str, fields: Vec<(&str, FieldValue)>) -> i32 {
            let id = graph.add_string(String::new()).unwrap();
            let metadata = ClassMetadata {
                object_id: id,
                name: name.to_string(),
                members: fields.iter().map(|(name, _)| name.to_string()).collect(),
                member_types: fields
                    .iter()
                    .map(|(_, value)| match value {
                        FieldValue::Primitive(value) => MemberType {
                            binary_type: 0,
                            additional: Some(AdditionalTypeInfo::Primitive(value.kind)),
                        },
                        FieldValue::Record(_) => MemberType {
                            binary_type: 2,
                            additional: None,
                        },
                    })
                    .collect(),
                library_id: None,
            };
            graph.metadata.insert(id, metadata.clone());
            graph.object_mut(id).unwrap().value = ObjectValue::Class(ClassObject {
                record_type: 4,
                metadata_id: id,
                metadata: Some(metadata),
                fields: fields.into_iter().map(|(_, value)| value).collect(),
            });
            id
        }

        let mut graph = Document::parse(&minimal_e3s()).unwrap();
        let mut records = Vec::new();
        for (kind, text) in blocks {
            let text = graph.add_string(text.to_string()).unwrap();
            let block = class(
                &mut graph,
                "EdsBlockItem",
                vec![
                    ("BType", FieldValue::Primitive(Primitive::int32(*kind))),
                    ("pTexts", FieldValue::Record(Record::Reference(text))),
                ],
            );
            records.push(Record::Reference(block));
        }
        let array = graph
            .append_object(ObjectValue::Array(ArrayObject::Object {
                string_only: false,
                length: records.len(),
                values: records,
            }))
            .unwrap();
        let list = class(
            &mut graph,
            "List",
            vec![
                (
                    "_size",
                    FieldValue::Primitive(Primitive::int32(blocks.len() as i32)),
                ),
                ("_items", FieldValue::Record(Record::Reference(array))),
            ],
        );
        let eds = class(
            &mut graph,
            "EdsBlock",
            vec![("pBlocks", FieldValue::Record(Record::Reference(list)))],
        );
        let te = class(
            &mut graph,
            "TriggerEditorData",
            vec![
                (
                    "UseChatEvent",
                    FieldValue::Primitive(Primitive::boolean(chat_enabled)),
                ),
                ("UseMSQC", FieldValue::Primitive(Primitive::boolean(false))),
                ("_addr", FieldValue::Primitive(Primitive::int32(0))),
                (
                    "_ptrAddr",
                    FieldValue::Primitive(Primitive::int32(0x58D910)),
                ),
                (
                    "_patternAddr",
                    FieldValue::Primitive(Primitive::int32(0x58D908)),
                ),
                (
                    "_lenAddr",
                    FieldValue::Primitive(Primitive::int32(0x58D90C)),
                ),
            ],
        );
        (
            Document::parse(&graph.to_bytes().unwrap()).unwrap(),
            eds,
            te,
        )
    }

    #[test]
    fn legacy_eds_continuations_keep_their_section_across_blocks() {
        let (graph, eds, te) = eds_fixture(
            &[
                (6, "[cammove]\ninertia: 5"),
                (0, ""),
                (6, "debug: True"),
                (7, ""),
                (6, "-next : 2\r\n-boss : 4"),
                (3, ""),
                (6, "[MSQC]\nNotTyping ; KeyPress(W) : Overmind Cocoon, 1"),
                (6, "0x58D900, Exactly, 4 : Cave-in, 2"),
                (6, "0x58D900, Exactly, 5 : Cave-in, 4"),
            ],
            true,
        );
        let (plugins, settings) = extract_plugins(&graph, eds, te).unwrap();
        assert_eq!(
            settings,
            BTreeMap::from([("debug".to_string(), "True".to_string())])
        );
        assert_eq!(
            plugins
                .iter()
                .map(|p| p.section.as_str())
                .collect::<Vec<_>>(),
            ["cammove", "chatEvent", "MSQC"]
        );
        assert_eq!(plugins[1].raw_text.as_deref(), Some(
            "[chatEvent]\n__addr__ : 0x58D900\n__ptrAddr__ : 0x58D910\n__patternAddr__ : 0x58D908\n__lenAddr__ : 0x58D90C\n-next : 2\n-boss : 4\n"
        ));
        assert_eq!(plugins[2].raw_text.as_deref(), Some(
            "[MSQC]\nNotTyping ; KeyPress(W) : Overmind Cocoon, 1\n0x58D900, Exactly, 4 : Cave-in, 2\n0x58D900, Exactly, 5 : Cave-in, 4\n"
        ));
    }

    #[test]
    fn disabled_chat_block_does_not_interrupt_user_plugin_continuation() {
        let (graph, eds, te) = eds_fixture(
            &[
                (6, "[MSQC]\nQCDebug: False"),
                (7, ""),
                (6, "QCUnit: 58\n[eudTurbo]\nt: 1"),
            ],
            false,
        );
        let (plugins, settings) = extract_plugins(&graph, eds, te).unwrap();
        assert!(settings.is_empty());
        assert_eq!(
            plugins
                .iter()
                .map(|p| p.section.as_str())
                .collect::<Vec<_>>(),
            ["MSQC", "eudTurbo"]
        );
        assert_eq!(
            plugins[0].raw_text.as_deref(),
            Some("[MSQC]\nQCDebug: False\nQCUnit: 58\n")
        );
        assert_eq!(plugins[1].raw_text.as_deref(), Some("[eudTurbo]\nt: 1\n"));
    }

    #[test]
    fn legacy_eds_rejects_ambiguous_or_unprojectable_continuations() {
        let (graph, eds, te) = eds_fixture(&[(6, "[MSQC]\n[msqc]")], false);
        assert!(extract_plugins(&graph, eds, te)
            .unwrap_err()
            .contains("duplicate"));
        let (graph, eds, te) =
            eds_fixture(&[(0, ""), (4, ""), (6, "asset.bin: 0x58D900, copy")], false);
        assert!(extract_plugins(&graph, eds, te)
            .unwrap_err()
            .contains("continuation"));
    }

    #[test]
    fn python_project_refuses_e3s_export_before_compatibility_lookup() {
        let root =
            std::env::temp_dir().join(format!("eud-agent-e3s-python-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("maps")).unwrap();
        fs::write(root.join("maps/source.scx"), b"map").unwrap();
        let project = NativeProject::create(
            &root,
            ProjectManifest {
                schema_version: PROJECT_SCHEMA_VERSION,
                name: "Python".to_string(),
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
        project
            .create_source("src/direct.py", "from eudplib import *\n")
            .unwrap();
        let error = export_e3s(&project, &root.join("out.e3s"), &root.join("compat")).unwrap_err();
        assert!(error.contains("Python"), "got: {error}");
        fs::remove_dir_all(root).ok();
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
        let mut dependency_manifest = payload.manifest.clone();
        dependency_manifest.python_dependencies = vec!["six==1.17.0".to_string()];
        assert!(native_payload_has_direct_python(
            &dependency_manifest,
            &payload.sources,
        ));
        let mut python_sources = payload.sources.clone();
        python_sources.push(NativeSourceFile {
            path: "src/helper.py".to_string(),
            content: "pass".to_string(),
            sha256: format!("{:x}", Sha256::digest(b"pass")),
        });
        assert!(native_payload_has_direct_python(
            &payload.manifest,
            &python_sources,
        ));
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

    fn compat_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/eud-editor-compat")
    }

    /// `main.eps` exactly as the Editor stores it: project modules are addressed
    /// through the editor build prefix, while `SCArchive` is an editor-provided module
    /// outside the native `src/` tree and must keep its spelling in both directions.
    const MAIN_EDITOR_BODY: &str = "import TriggerEditor.leaf as l;\nimport TriggerEditor.SCArchive as sca;\n\nfunction onPluginStart() {\n    l.run();\n}\n";
    /// The same body after the import rewrite: src-relative.
    const MAIN_NATIVE_BODY: &str = "import leaf as l;\nimport TriggerEditor.SCArchive as sca;\n\nfunction onPluginStart() {\n    l.run();\n}\n";
    /// `leaf.eps` sits under the `IsTopFile` folder. Its string literals, line comment
    /// and block comment all contain text that looks like a prefixed import of a real
    /// project module; only the first line is a statement. `.sibling` is relative and
    /// never a rewrite target.
    const LEAF_EDITOR_BODY: &str = "import TriggerEditor.main as m;\nimport .sibling as sib;\nconst q = \"import TriggerEditor.main as q;\";\nconst sq = 'import TriggerEditor.main;';\n// import TriggerEditor.main as c;\n/* import TriggerEditor.main as b; */\nconst ko = \"\u{d50c}\u{b808}\u{c774}\u{c5b4} import TriggerEditor.main\";\nfunction run() { m.onPluginStart(); }\n";
    const LEAF_NATIVE_BODY: &str = "import main as m;\nimport .sibling as sib;\nconst q = \"import TriggerEditor.main as q;\";\nconst sq = 'import TriggerEditor.main;';\n// import TriggerEditor.main as c;\n/* import TriggerEditor.main as b; */\nconst ko = \"\u{d50c}\u{b808}\u{c774}\u{c5b4} import TriggerEditor.main\";\nfunction run() { m.onPluginStart(); }\n";

    /// A synthetic `SaveableData` stream: a root `ProjectMain` folder holding
    /// `main.eps`, plus an `IsTopFile=true` `TriggerEditor` folder holding `leaf.eps`,
    /// empty DAT/XDAT/TBL/requirement/button state, and one `[main]` EDS block. Class
    /// names and member layouts match what import and export read and write on real
    /// saves, so the whole `import_e3s`/`export_e3s` path runs without a fixture file.
    fn saveable_e3s(source_map: &Path, main_body: &str, leaf_body: &str) -> Vec<u8> {
        use crate::nrbf::{
            AdditionalTypeInfo, ArrayObject, ClassMetadata, ClassObject, MemberType,
        };

        /// Define class metadata and its first instance in one object, mirroring how
        /// `eds_fixture` builds records. `Some(id)` pins the record to an existing
        /// object id (the NRBF root is object 1).
        fn define_class(
            graph: &mut Document,
            id: Option<i32>,
            name: &str,
            fields: Vec<(&str, FieldValue)>,
        ) -> i32 {
            let id = id.unwrap_or_else(|| graph.add_string(String::new()).unwrap());
            let metadata = ClassMetadata {
                object_id: id,
                name: name.to_string(),
                members: fields.iter().map(|(name, _)| (*name).to_string()).collect(),
                member_types: fields
                    .iter()
                    .map(|(_, value)| match value {
                        FieldValue::Primitive(value) => MemberType {
                            binary_type: 0,
                            additional: Some(AdditionalTypeInfo::Primitive(value.kind)),
                        },
                        FieldValue::Record(_) => MemberType {
                            binary_type: 2,
                            additional: None,
                        },
                    })
                    .collect(),
                library_id: None,
            };
            graph.metadata.insert(id, metadata.clone());
            graph.object_mut(id).unwrap().value = ObjectValue::Class(ClassObject {
                record_type: 4,
                metadata_id: id,
                metadata: Some(metadata),
                fields: fields.into_iter().map(|(_, value)| value).collect(),
            });
            id
        }

        fn map(fields: Vec<(&str, FieldValue)>) -> BTreeMap<String, FieldValue> {
            fields
                .into_iter()
                .map(|(name, value)| (name.to_string(), value))
                .collect()
        }

        fn text(graph: &mut Document, value: &str) -> Record {
            Record::Reference(graph.add_string(value.to_string()).unwrap())
        }

        fn bool_array(graph: &mut Document) -> Record {
            Record::Reference(
                graph
                    .append_object(ObjectValue::Array(ArrayObject::Primitive {
                        primitive_type: 1,
                        values: Vec::new(),
                    }))
                    .unwrap(),
            )
        }

        fn object_array(graph: &mut Document, values: Vec<Record>) -> Record {
            let length = values.len();
            Record::Reference(
                graph
                    .append_object(ObjectValue::Array(ArrayObject::Object {
                        string_only: false,
                        length,
                        values,
                    }))
                    .unwrap(),
            )
        }

        fn make_list(graph: &mut Document, list_meta: i32, records: Vec<Record>) -> Record {
            let size = i32::try_from(records.len()).unwrap();
            let items = object_array(graph, records);
            Record::Reference(
                graph
                    .add_class(
                        list_meta,
                        map(vec![
                            ("_size", FieldValue::Primitive(Primitive::int32(size))),
                            ("_items", FieldValue::Record(items)),
                        ]),
                    )
                    .unwrap(),
            )
        }

        fn scripter_fields(
            graph: &mut Document,
            body: &str,
            script_type: i32,
        ) -> Vec<(&'static str, FieldValue)> {
            let text = Record::Reference(graph.add_string(body.to_string()).unwrap());
            let empty = Record::Reference(graph.add_string(String::new()).unwrap());
            vec![
                ("foldedData", FieldValue::Record(Record::Null)),
                ("_String", FieldValue::Record(text)),
                ("_ConnectFile", FieldValue::Record(empty.clone())),
                ("_ConnectRelativeFile", FieldValue::Record(empty)),
                (
                    "ScriptType",
                    FieldValue::Record(Record::Reference(script_type)),
                ),
                (
                    "ScriptEditor+ScriptType",
                    FieldValue::Record(Record::Reference(script_type)),
                ),
            ]
        }

        fn te_file_fields(
            name: Record,
            file_type: Record,
            scripter: Record,
            files: Record,
            folders: Record,
            is_top: bool,
        ) -> Vec<(&'static str, FieldValue)> {
            vec![
                (
                    "_IsExpaned",
                    FieldValue::Primitive(Primitive::boolean(false)),
                ),
                ("CreateDate", FieldValue::Primitive(Primitive::date_time(0))),
                ("pLastDate", FieldValue::Primitive(Primitive::date_time(0))),
                ("LastConnectTimer", FieldValue::Record(Record::Null)),
                ("_UIBinding", FieldValue::Record(Record::Null)),
                ("_Scripter", FieldValue::Record(scripter)),
                (
                    "IsTopFile",
                    FieldValue::Primitive(Primitive::boolean(is_top)),
                ),
                ("_Folders", FieldValue::Record(folders)),
                ("ParentFolder", FieldValue::Record(Record::Null)),
                ("_Files", FieldValue::Record(files)),
                ("_FileType", FieldValue::Record(file_type)),
                ("_FileName", FieldValue::Record(name)),
            ]
        }

        let mut graph = Document::parse(&minimal_e3s()).unwrap();
        // Enum metadata: the zero-valued defining instance doubles as the folder type.
        let file_type = define_class(
            &mut graph,
            None,
            TE_FILE_TYPE,
            vec![(
                "value__",
                FieldValue::Primitive(Primitive::int32(i32::try_from(TE_FOLDER).unwrap())),
            )],
        );
        let folder_type = Record::Reference(file_type);
        let eps_type = Record::Reference(
            graph
                .add_enum(file_type, i32::try_from(TE_CUI_EPS).unwrap())
                .unwrap(),
        );
        let script_type = define_class(
            &mut graph,
            None,
            SCRIPT_TYPE,
            vec![("value__", FieldValue::Primitive(Primitive::int32(0)))],
        );
        define_class(
            &mut graph,
            None,
            EDS_ITEM_TYPE,
            vec![("value__", FieldValue::Primitive(Primitive::int32(0)))],
        );
        let list_items = object_array(&mut graph, Vec::new());
        let list_meta = define_class(
            &mut graph,
            None,
            "System.Collections.Generic.List`1",
            vec![
                ("_size", FieldValue::Primitive(Primitive::int32(0))),
                ("_items", FieldValue::Record(list_items)),
            ],
        );
        // Metadata that only export touches at runtime; the defining instances are
        // inert records that no tree or list references.
        let te_file_list_items = object_array(&mut graph, Vec::new());
        define_class(
            &mut graph,
            None,
            TE_FILE_LIST,
            vec![
                ("_items", FieldValue::Record(te_file_list_items)),
                ("_size", FieldValue::Primitive(Primitive::int32(0))),
                ("_version", FieldValue::Primitive(Primitive::int32(0))),
            ],
        );
        define_class(
            &mut graph,
            None,
            TE_TAB_UI,
            vec![("TEFile", FieldValue::Record(Record::Null))],
        );
        define_class(
            &mut graph,
            None,
            BUTTON_DATA,
            [
                "_pos", "_icon", "_con", "_act", "_conval", "_actval", "_enaStr", "_disStr",
            ]
            .into_iter()
            .map(|name| (name, FieldValue::Primitive(Primitive::int32(0))))
            .collect(),
        );
        let te_file_meta = {
            let fields = te_file_fields(
                text(&mut graph, ""),
                folder_type.clone(),
                Record::Null,
                make_list(&mut graph, list_meta, Vec::new()),
                make_list(&mut graph, list_meta, Vec::new()),
                false,
            );
            define_class(&mut graph, None, TE_FILE, fields)
        };

        // leaf.eps defines the CUIScriptEditor metadata main.eps reuses.
        let leaf_fields = scripter_fields(&mut graph, leaf_body, script_type);
        let leaf_scripter = define_class(&mut graph, None, CUI_SCRIPT_EDITOR, leaf_fields);
        let main_fields = map(scripter_fields(&mut graph, main_body, script_type));
        let main_scripter = graph.add_class(leaf_scripter, main_fields).unwrap();

        let leaf_files = make_list(&mut graph, list_meta, Vec::new());
        let leaf_folders = make_list(&mut graph, list_meta, Vec::new());
        let leaf_name = text(&mut graph, "leaf");
        let leaf = graph
            .add_class(
                te_file_meta,
                map(te_file_fields(
                    leaf_name,
                    eps_type.clone(),
                    Record::Reference(leaf_scripter),
                    leaf_files,
                    leaf_folders,
                    false,
                )),
            )
            .unwrap();
        let editor_files = make_list(&mut graph, list_meta, vec![Record::Reference(leaf)]);
        let editor_folders = make_list(&mut graph, list_meta, Vec::new());
        let editor_name = text(&mut graph, "TriggerEditor");
        let editor_folder = graph
            .add_class(
                te_file_meta,
                map(te_file_fields(
                    editor_name,
                    folder_type,
                    Record::Null,
                    editor_files,
                    editor_folders,
                    true,
                )),
            )
            .unwrap();
        let main_files = make_list(&mut graph, list_meta, Vec::new());
        let main_folders = make_list(&mut graph, list_meta, Vec::new());
        let main_name = text(&mut graph, "main");
        let main_file = graph
            .add_class(
                te_file_meta,
                map(te_file_fields(
                    main_name,
                    eps_type,
                    Record::Reference(main_scripter),
                    main_files,
                    main_folders,
                    false,
                )),
            )
            .unwrap();
        let root_files = make_list(&mut graph, list_meta, vec![Record::Reference(main_file)]);
        let root_folders = make_list(
            &mut graph,
            list_meta,
            vec![Record::Reference(editor_folder)],
        );
        let root_name = text(&mut graph, "ProjectMain");
        let project_file = graph
            .add_class(
                te_file_meta,
                map(te_file_fields(
                    root_name,
                    Record::Reference(file_type),
                    Record::Null,
                    root_files,
                    root_folders,
                    true,
                )),
            )
            .unwrap();

        let te_data = define_class(
            &mut graph,
            None,
            "EUD_Editor_3.TriggerEditorData",
            vec![
                (
                    "_MainFile",
                    FieldValue::Record(Record::Reference(main_file)),
                ),
                (
                    "ProjectFile",
                    FieldValue::Record(Record::Reference(project_file)),
                ),
                (
                    "UseChatEvent",
                    FieldValue::Primitive(Primitive::boolean(false)),
                ),
                ("UseMSQC", FieldValue::Primitive(Primitive::boolean(false))),
                ("_addr", FieldValue::Primitive(Primitive::int32(0))),
                (
                    "_ptrAddr",
                    FieldValue::Primitive(Primitive::int32(0x58D910)),
                ),
                (
                    "_patternAddr",
                    FieldValue::Primitive(Primitive::int32(0x58D908)),
                ),
                (
                    "_lenAddr",
                    FieldValue::Primitive(Primitive::int32(0x58D90C)),
                ),
            ],
        );
        let dat_list = make_list(&mut graph, list_meta, Vec::new());
        let dat = define_class(
            &mut graph,
            None,
            "EUD_Editor_3.DatFiles",
            vec![("Datfile", FieldValue::Record(dat_list))],
        );
        let require_list = make_list(&mut graph, list_meta, Vec::new());
        let orig_require_list = make_list(&mut graph, list_meta, Vec::new());
        let require_meta = define_class(
            &mut graph,
            None,
            "EUD_Editor_3.CRequireData",
            vec![
                ("RequireDatas", FieldValue::Record(require_list)),
                ("OrigRequireDatas", FieldValue::Record(orig_require_list)),
            ],
        );
        let requires = (0..4)
            .map(|_| {
                let require_list = make_list(&mut graph, list_meta, Vec::new());
                let orig_list = make_list(&mut graph, list_meta, Vec::new());
                Record::Reference(
                    graph
                        .add_class(
                            require_meta,
                            map(vec![
                                ("RequireDatas", FieldValue::Record(require_list)),
                                ("OrigRequireDatas", FieldValue::Record(orig_list)),
                            ]),
                        )
                        .unwrap(),
                )
            })
            .collect::<Vec<_>>();
        let mut require_records = vec![Record::Reference(require_meta)];
        require_records.extend(requires);
        let button_array = object_array(&mut graph, Vec::new());
        let button_sets = define_class(
            &mut graph,
            None,
            "EUD_Editor_3.CButtonData",
            vec![("ButtonSets", FieldValue::Record(button_array))],
        );
        let wire_frame = bool_array(&mut graph);
        let default_wire_frame = bool_array(&mut graph);
        let grp_frame = bool_array(&mut graph);
        let default_grp_frame = bool_array(&mut graph);
        let tran_frame = bool_array(&mut graph);
        let default_tran_frame = bool_array(&mut graph);
        let button_set = bool_array(&mut graph);
        let default_button_set = bool_array(&mut graph);
        let status_fn1 = bool_array(&mut graph);
        let status_fn1_default = bool_array(&mut graph);
        let status_fn2 = bool_array(&mut graph);
        let status_fn2_default = bool_array(&mut graph);
        let stat_txt = object_array(&mut graph, Vec::new());
        let require_array = object_array(&mut graph, require_records);
        let extra = define_class(
            &mut graph,
            None,
            "EUD_Editor_3.ExtraDatFiles",
            vec![
                ("_WireFrame", FieldValue::Record(wire_frame)),
                ("_DefaultWireFrame", FieldValue::Record(default_wire_frame)),
                ("_GrpFrame", FieldValue::Record(grp_frame)),
                ("_DefaultGrpFrame", FieldValue::Record(default_grp_frame)),
                ("_TranFrame", FieldValue::Record(tran_frame)),
                ("_DefaultTranFrame", FieldValue::Record(default_tran_frame)),
                ("_ButtonSet", FieldValue::Record(button_set)),
                ("_DefaultButtonSet", FieldValue::Record(default_button_set)),
                ("_statusFn1", FieldValue::Record(status_fn1)),
                (
                    "_statusFn1IsDefault",
                    FieldValue::Record(status_fn1_default),
                ),
                ("_statusFn2", FieldValue::Record(status_fn2)),
                (
                    "_statusFn2IsDefault",
                    FieldValue::Record(status_fn2_default),
                ),
                ("_Stat_txt", FieldValue::Record(stat_txt)),
                ("RequireDatas", FieldValue::Record(require_array)),
                (
                    "_ButtonData",
                    FieldValue::Record(Record::Reference(button_sets)),
                ),
            ],
        );
        let eds_texts = text(&mut graph, "");
        let eds_item = define_class(
            &mut graph,
            None,
            EDS_ITEM,
            vec![
                ("BType", FieldValue::Primitive(Primitive::int32(0))),
                ("pTexts", FieldValue::Record(eds_texts)),
            ],
        );
        let blocks = make_list(&mut graph, list_meta, vec![Record::Reference(eds_item)]);
        let eds = define_class(
            &mut graph,
            None,
            "EUD_Editor_3.BuildData+EdsBlock",
            vec![("pBlocks", FieldValue::Record(blocks))],
        );
        let map_name = source_map
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let open_map = text(&mut graph, &source_map.to_string_lossy());
        let relative_open_map = text(&mut graph, &map_name);
        let save_map = text(&mut graph, "output.scx");
        let relative_save_map = text(&mut graph, "output.scx");
        define_class(
            &mut graph,
            Some(1),
            SAVEABLE_DATA,
            vec![
                ("mOpenMapName", FieldValue::Record(open_map)),
                (
                    "mRelativeOpenMapName",
                    FieldValue::Record(relative_open_map),
                ),
                ("mSaveMapName", FieldValue::Record(save_map)),
                (
                    "mRelativeSaveMapName",
                    FieldValue::Record(relative_save_map),
                ),
                (
                    "mUseCustomTbl",
                    FieldValue::Primitive(Primitive::boolean(false)),
                ),
                ("TEData", FieldValue::Record(Record::Reference(te_data))),
                ("Dat", FieldValue::Record(Record::Reference(dat))),
                ("ExtraDat", FieldValue::Record(Record::Reference(extra))),
                ("EdsBlocks", FieldValue::Record(Record::Reference(eds))),
            ],
        );
        graph.to_bytes().unwrap()
    }

    fn synthetic_project(tag: &str) -> (PathBuf, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("eud-agent-e3s-{tag}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let source_map = root.join("test.scx");
        fs::write(&source_map, b"map").unwrap();
        let e3s = root.join("project.e3s");
        fs::write(
            &e3s,
            saveable_e3s(&source_map, MAIN_EDITOR_BODY, LEAF_EDITOR_BODY),
        )
        .unwrap();
        (root, e3s)
    }

    /// Walk an exported TE tree the way the Editor reads it and collect every epScript
    /// body with its `src/`-relative path, exactly like `collect_te_sources` maps them.
    fn editor_te_bodies(
        graph: &Document,
        folder: i32,
        prefix: &str,
        out: &mut Vec<(String, String)>,
    ) {
        let kind = graph.field_i64(folder, "_FileType").unwrap();
        let name = graph
            .field_string(folder, "_FileName")
            .unwrap()
            .unwrap()
            .to_string();
        if kind == TE_FOLDER {
            let next = if graph.field_bool(folder, "IsTopFile").unwrap() {
                prefix.to_string()
            } else {
                join_path(prefix, &name)
            };
            for list in ["_Files", "_Folders"] {
                let children =
                    list_object_ids(graph, required_field_object(graph, folder, list).unwrap())
                        .unwrap();
                for child in children {
                    editor_te_bodies(graph, child, &next, out);
                }
            }
        } else if kind == TE_CUI_EPS {
            let mut leaf = name;
            if !leaf.to_ascii_lowercase().ends_with(".eps") {
                leaf.push_str(".eps");
            }
            let scripter = required_field_object(graph, folder, "_Scripter").unwrap();
            out.push((
                format!("src/{}", join_path(prefix, &leaf)),
                graph
                    .field_string(scripter, "_String")
                    .unwrap()
                    .unwrap_or_default()
                    .to_string(),
            ));
        }
    }

    fn exported_te_bodies(exported: &Path) -> Vec<(String, String)> {
        let base = NrbfDocument::read(exported)
            .unwrap()
            .without_native_payload()
            .unwrap();
        let graph = &base.graph;
        let root = graph.root_id().unwrap();
        let te_data = required_field_object(graph, root, "TEData").unwrap();
        let project_file = required_field_object(graph, te_data, "ProjectFile").unwrap();
        let mut bodies = Vec::new();
        editor_te_bodies(graph, project_file, "", &mut bodies);
        bodies.sort();
        bodies
    }

    #[test]
    fn import_rewrites_editor_prefixed_imports_to_src_relative() {
        let (root, e3s) = synthetic_project("import");
        let project = import_e3s(&e3s, &root.join("native"), &compat_root()).unwrap();
        assert_eq!(
            project.read_source("src/main.eps").unwrap(),
            MAIN_NATIVE_BODY
        );
        assert_eq!(
            project.read_source("src/leaf.eps").unwrap(),
            LEAF_NATIVE_BODY
        );
        assert_eq!(
            project
                .manifest()
                .editor_compatibility
                .as_ref()
                .unwrap()
                .editor_import_prefixes,
            vec!["TriggerEditor".to_string()]
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn export_restores_editor_prefixed_imports() {
        let (root, e3s) = synthetic_project("export");
        let compat = compat_root();
        let project = import_e3s(&e3s, &root.join("native"), &compat).unwrap();
        let exported = root.join("export.e3s");
        export_e3s(&project, &exported, &compat).unwrap();
        assert_eq!(
            exported_te_bodies(&exported),
            vec![
                ("src/leaf.eps".to_string(), LEAF_EDITOR_BODY.to_string()),
                ("src/main.eps".to_string(), MAIN_EDITOR_BODY.to_string()),
            ]
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn synthetic_import_export_import_round_trip_is_byte_stable() {
        let (root, e3s) = synthetic_project("roundtrip");
        let compat = compat_root();
        let first = import_e3s(&e3s, &root.join("first"), &compat).unwrap();
        let exported = root.join("roundtrip.e3s");
        export_e3s(&first, &exported, &compat).unwrap();
        // The exported Editor graph, re-projected through the legacy path, must match
        // the imported native state byte for byte.
        let exported_base = NrbfDocument::read(&exported)
            .unwrap()
            .without_native_payload()
            .unwrap();
        let legacy = project_from_graph(
            &exported_base.graph,
            &exported,
            &DatCatalog::load(&compat).unwrap(),
        )
        .unwrap();
        let mut legacy_sources = legacy.sources.clone();
        legacy_sources
            .sort_by(|left, right| left.path.to_lowercase().cmp(&right.path.to_lowercase()));
        assert_eq!(first.source_snapshot().unwrap().files, legacy_sources);
        // The exported file carries the native payload, so re-import takes that path.
        let second = import_e3s(&exported, &root.join("second"), &compat).unwrap();
        assert_eq!(
            first.source_snapshot().unwrap().files,
            second.source_snapshot().unwrap().files
        );
        assert_eq!(first.manifest().main_file, second.manifest().main_file);
        assert_eq!(
            second
                .manifest()
                .editor_compatibility
                .as_ref()
                .unwrap()
                .editor_import_prefixes,
            vec!["TriggerEditor".to_string()]
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn export_never_prepends_when_several_prefixes_were_stripped() {
        let (root, e3s) = synthetic_project("multiprefix");
        let compat = compat_root();
        let mut project = import_e3s(&e3s, &root.join("native"), &compat).unwrap();
        let mut compatibility = project.manifest().editor_compatibility.clone().unwrap();
        compatibility
            .editor_import_prefixes
            .push("SecondPrefix".to_string());
        project
            .set_editor_compatibility(Some(compatibility))
            .unwrap();
        let exported = root.join("export.e3s");
        export_e3s(&project, &exported, &compat).unwrap();
        // Without a single unambiguous candidate the bodies stay src-relative instead
        // of being guessed at, and the round trip remains byte-stable.
        assert_eq!(
            exported_te_bodies(&exported),
            vec![
                ("src/leaf.eps".to_string(), LEAF_NATIVE_BODY.to_string()),
                ("src/main.eps".to_string(), MAIN_NATIVE_BODY.to_string()),
            ]
        );
        let second = import_e3s(&exported, &root.join("second"), &compat).unwrap();
        assert_eq!(
            project.source_snapshot().unwrap().files,
            second.source_snapshot().unwrap().files
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn import_rewrite_skips_strings_comments_and_relative_imports() {
        let prefixes = BTreeSet::from(["TriggerEditor".to_string()]);
        let modules = BTreeSet::from(["main".to_string(), "leaf".to_string()]);
        let stripped = strip_editor_import_prefixes(LEAF_EDITOR_BODY, &prefixes, &modules);
        assert_eq!(stripped.count, 1);
        assert_eq!(stripped.content, LEAF_NATIVE_BODY);
        assert_eq!(
            restore_editor_import_prefixes(&stripped.content, "TriggerEditor", &modules),
            LEAF_EDITOR_BODY
        );

        // Lookalikes that are not import statements stay untouched.
        let body = "reimport TriggerEditor.leaf;\nx.import TriggerEditor.leaf;\nimport TriggerEditor.leaf as l; /* import TriggerEditor.leaf;\n";
        let stripped = strip_editor_import_prefixes(body, &prefixes, &modules);
        assert_eq!(stripped.count, 1);
        assert_eq!(
            stripped.content,
            "reimport TriggerEditor.leaf;\nx.import TriggerEditor.leaf;\nimport leaf as l; /* import TriggerEditor.leaf;\n"
        );
    }

    #[test]
    fn import_rewrite_only_touches_modules_the_project_owns() {
        let prefixes = BTreeSet::from(["TriggerEditor".to_string()]);
        let modules = BTreeSet::from(["sub.deep".to_string()]);
        // `SCArchive` is editor-provided, the bare prefix has no module part, and
        // `deepX` is not `sub.deep`; only the real project import is rewritten.
        let body = "import TriggerEditor.SCArchive as sca;\nimport TriggerEditor.sub.deep as m;\nimport TriggerEditor;\nimport sub.deepX;\n";
        let stripped = strip_editor_import_prefixes(body, &prefixes, &modules);
        assert_eq!(stripped.count, 1);
        assert_eq!(
            stripped.content,
            "import TriggerEditor.SCArchive as sca;\nimport sub.deep as m;\nimport TriggerEditor;\nimport sub.deepX;\n"
        );
        assert_eq!(
            restore_editor_import_prefixes(&stripped.content, "TriggerEditor", &modules),
            body
        );
    }

    #[test]
    fn migrate_strips_editor_prefixes_from_previously_imported_sources() {
        let (root, e3s) = synthetic_project("migrate");
        let compat = compat_root();
        let mut project = import_e3s(&e3s, &root.join("native"), &compat).unwrap();
        // Simulate a project imported before the rewrite existed: bodies still carry
        // the editor prefix.
        project
            .write_source("src/main.eps", MAIN_EDITOR_BODY)
            .unwrap();
        project
            .write_source("src/leaf.eps", LEAF_EDITOR_BODY)
            .unwrap();
        assert_eq!(migrate_editor_import_prefixes(&mut project).unwrap(), 2);
        assert_eq!(
            project.read_source("src/main.eps").unwrap(),
            MAIN_NATIVE_BODY
        );
        assert_eq!(
            project.read_source("src/leaf.eps").unwrap(),
            LEAF_NATIVE_BODY
        );
        assert_eq!(migrate_editor_import_prefixes(&mut project).unwrap(), 0);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn migrate_works_without_editor_compatibility_metadata() {
        let root = std::env::temp_dir().join(format!(
            "eud-agent-e3s-migrate-plain-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(root.join("maps")).unwrap();
        fs::write(root.join("maps/source.scx"), b"map").unwrap();
        let mut project = NativeProject::create(
            &root,
            ProjectManifest {
                schema_version: PROJECT_SCHEMA_VERSION,
                name: "Plain".to_string(),
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
        project.create_source("src/helper.eps", "").unwrap();
        project
            .write_source("src/main.eps", "import TriggerEditor.helper as h;\n")
            .unwrap();
        assert_eq!(migrate_editor_import_prefixes(&mut project).unwrap(), 1);
        assert_eq!(
            project.read_source("src/main.eps").unwrap(),
            "import helper as h;\n"
        );
        fs::remove_dir_all(&root).ok();
    }
    #[test]
    #[ignore = "requires EUD_AGENT_E3S_FIXTURE and EUD_AGENT_E3S_COMPAT"]
    fn real_python_native_extension_is_rejected_before_destination_mutation() {
        let fixture = PathBuf::from(std::env::var("EUD_AGENT_E3S_FIXTURE").unwrap());
        let compat = PathBuf::from(std::env::var("EUD_AGENT_E3S_COMPAT").unwrap());
        let root = std::env::temp_dir().join(format!(
            "eud-agent-e3s-python-import-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        let mut document = NrbfDocument::read(&fixture).unwrap();
        let base = document.without_native_payload().unwrap();
        let legacy =
            project_from_graph(&base.graph, &fixture, &DatCatalog::load(&compat).unwrap()).unwrap();
        let mut sources = legacy.sources;
        sources.push(NativeSourceFile {
            path: "src/helper.py".to_string(),
            content: "pass".to_string(),
            sha256: format!("{:x}", Sha256::digest(b"pass")),
        });
        let payload = NativeE3sPayload {
            schema_version: 1,
            manifest: legacy.manifest,
            dat: legacy.dat,
            sources,
            base_sha256: base.sha256(),
        };
        append_native_payload(&mut document.graph, &payload).unwrap();
        document.bytes = document.graph.to_bytes().unwrap();
        let source = root.join("python.e3s");
        document.write_exact(&source).unwrap();
        fs::copy(fixture.with_file_name("test.scx"), root.join("test.scx")).unwrap();
        let destination = root.join("project");
        let error = import_e3s(&source, &destination, &compat).unwrap_err();
        assert!(error.contains("Python"), "got: {error}");
        assert!(!destination.exists());
        fs::remove_dir_all(root).ok();
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
        // The native extension must not mask a lossy or invalid Editor graph.
        let exported_base = NrbfDocument::read(&exported)
            .unwrap()
            .without_native_payload()
            .unwrap();
        let legacy = project_from_graph(
            &exported_base.graph,
            &exported,
            &DatCatalog::load(&compat).unwrap(),
        )
        .unwrap();
        assert_eq!(first.dat(), &legacy.dat);
        assert_eq!(first.source_snapshot().unwrap().files, legacy.sources);
        assert_eq!(first.manifest().main_file, legacy.manifest.main_file);
        assert_eq!(first.manifest().settings, legacy.manifest.settings);
        assert_eq!(first.manifest().plugins, legacy.manifest.plugins);
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
