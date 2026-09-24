//! Deterministic native EDS/build frontend for euddraft 0.10.2.5.
//!
//! euddraft accepts one UTF-8 `.eds` file, changes cwd to its directory, loads `[main]`
//! input/output, then loads every remaining section in declaration order as an EPS/Python/global
//! plugin. This module materializes that contract without invoking EUD Editor.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use encoding_rs::EUC_KR;
use serde::{Deserialize, Serialize};

use crate::bootstrap::process_tree::{run_process_tree, ProcessCancellation, ProcessEnd};
use crate::memory::write_atomic_bytes;
use crate::native_project::{
    NativeDatState, NativeProject, NumericOverride, ProjectManifest, TextOverride,
};

const EUDDRAFT_TIMEOUT: Duration = Duration::from_secs(300);
const PYTHON_PROBE_TIMEOUT: Duration = Duration::from_secs(120);
const TRACEBACK_MARKER: &str = "Traceback (most recent call last):";
/// Characters kept per diagnostic `raw` block in a build result.
const DIAGNOSTIC_RAW_LIMIT: usize = 4096;
/// Project-relative path of the complete stdout/stderr log of the last euddraft run.
pub const BUILD_LOG_RELATIVE_PATH: &str = "build/euddraft/build.log";
/// Characters kept per stream in the model-facing build output excerpt.
const EXCERPT_STREAM_LIMIT: usize = 6144;
/// Characters kept per line in the excerpt; euddraft prints every null tile on one line.
const EXCERPT_LINE_LIMIT: usize = 400;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatFieldMeta {
    pub name: String,
    /// The `.def` `[FORMAT]` field index — the order the DAT Editor lists fields
    /// in, which the name-keyed map no longer carries.
    pub index: u32,
    pub size: u8,
    pub var_start: u32,
    pub var_end: u32,
    pub var_array: u32,
    pub var_index: u32,
    pub init_var: i64,
    pub offset: u32,
    /// The `.def` `Type=` value: which catalog the number points at. `None` is a
    /// plain number. See `dat_wiki::DatReference` for the mapping.
    pub value_type: Option<u32>,
    /// One label per bit, from the `Name=...:a,b,"c,d"` suffix; empty when the
    /// field is not a flag field.
    pub flags: Vec<String>,
    pub baseline: Vec<i64>,
}

impl DatFieldMeta {
    /// The inclusive value range this field's width admits, `init_var` included
    /// (the baseline already carries it).
    pub fn value_range(&self) -> (i64, i64) {
        let width = match self.size {
            1 => u8::MAX as i64,
            2 => u16::MAX as i64,
            _ => u32::MAX as i64,
        };
        (self.init_var, width + self.init_var)
    }

    fn local_index(&self, object_id: u32) -> Result<u32, String> {
        if object_id < self.var_start || object_id > self.var_end {
            return Err(format!(
                "object id {object_id} is outside {} range {}..{}",
                self.name, self.var_start, self.var_end
            ));
        }
        Ok(object_id - self.var_start)
    }

    pub fn baseline_value(&self, object_id: u32) -> Result<i64, String> {
        let local = self.local_index(object_id)? as usize;
        self.baseline
            .get(local)
            .copied()
            .ok_or_else(|| format!("{} baseline is missing object {object_id}", self.name))
    }
}

/// The DAT tables the catalog carries, in the order EUD Editor 3's DAT Editor
/// lists them.
pub const DAT_TABLES: [&str; 10] = [
    "units", "weapons", "flingy", "sprites", "images", "upgrades", "techdata", "orders",
    "portdata", "sfxdata",
];

#[derive(Debug, Clone)]
pub struct DatCatalog {
    fields: BTreeMap<String, BTreeMap<String, DatFieldMeta>>,
    /// `InputEntrycount` per table: how many objects the table holds.
    entries: BTreeMap<String, u32>,
    button_defaults: Vec<ButtonSetDefault>,
    tbl: Vec<String>,
    status: Vec<(i64, i64)>,
    requirements: BTreeMap<String, Vec<RequirementDefault>>,
}

#[derive(Debug, Clone)]
struct ButtonSetDefault {
    count: u32,
    address: u32,
    buttons: Vec<Button>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RequirementDefault {
    allocated: bool,
    blocks: Vec<RequirementBlock>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RequirementBlock {
    opcode: u16,
    value: Option<u16>,
}

#[derive(Debug, Clone)]
struct RequirementArtifacts {
    bytes: Vec<u8>,
    pointers: BTreeMap<String, Vec<u16>>,
}

impl DatCatalog {
    pub fn load(compat_root: &Path) -> Result<Self, String> {
        let offsets = parse_offsets(&read_text(&compat_root.join("Offset.txt"))?)?;
        let mut fields = BTreeMap::new();
        let mut entries = BTreeMap::new();
        for table in DAT_TABLES {
            let definition = compat_root.join("DatFiles").join(format!("{table}.def"));
            let data_path = compat_root.join("DatFiles").join(format!("{table}.dat"));
            let (count, metas) = parse_dat_definition(
                table,
                &read_text(&definition)?,
                &fs::read(&data_path).map_err(stringify_io)?,
                &offsets,
            )?;
            fields.insert(table.to_string(), metas);
            entries.insert(table.to_string(), count);
        }
        let button_defaults = parse_button_defaults(
            &fs::read(compat_root.join("DatFiles/btnset.dat")).map_err(stringify_io)?,
        )?;
        let tbl =
            decode_tbl(&fs::read(compat_root.join("Tbls/stat_txt.tbl")).map_err(stringify_io)?)?;
        let status = parse_status_defaults(
            &fs::read(compat_root.join("DatFiles/statusInfor.dat")).map_err(stringify_io)?,
        )?;
        let requirements = parse_requirement_defaults(
            &fs::read(compat_root.join("DatFiles/require.dat")).map_err(stringify_io)?,
        )?;
        Ok(Self {
            fields,
            entries,
            button_defaults,
            tbl,
            status,
            requirements,
        })
    }

    /// Every field of one DAT table, keyed by field name. The DAT wiki orders
    /// them by `DatFieldMeta::index`, not by this map's alphabetical keys.
    pub fn table_fields(&self, table: &str) -> Option<&BTreeMap<String, DatFieldMeta>> {
        self.fields.get(table)
    }

    /// How many objects a DAT table holds (`InputEntrycount`).
    pub fn table_entries(&self, table: &str) -> Option<u32> {
        self.entries.get(table).copied()
    }

    /// Every decoded `stat_txt.tbl` string, in index order. A DAT label field
    /// stores a ONE-based string id, so its text is `tbl_strings()[value - 1]`;
    /// the sparse TBL document addresses the same strings zero-based.
    pub fn tbl_strings(&self) -> &[String] {
        &self.tbl
    }

    /// How many button sets `btnset.dat` defines.
    pub fn button_set_count(&self) -> usize {
        self.button_defaults.len()
    }

    /// How many `statusInfor.dat` objects the catalog carries.
    pub fn status_count(&self) -> usize {
        self.status.len()
    }

    /// Requirement tables and how many objects each covers.
    pub fn requirement_tables(&self) -> Vec<(&str, usize)> {
        self.requirements
            .iter()
            .map(|(table, objects)| (table.as_str(), objects.len()))
            .collect()
    }

    pub fn field(&self, table: &str, field: &str) -> Result<&DatFieldMeta, String> {
        self.fields
            .get(table)
            .and_then(|fields| fields.get(field))
            .ok_or_else(|| format!("unknown DAT field {table}.{field}"))
    }

    fn button_default(&self, set_id: u32) -> Result<&ButtonSetDefault, String> {
        self.button_defaults
            .get(set_id as usize)
            .ok_or_else(|| format!("button set {set_id} is outside the default catalog"))
    }

    pub fn numeric_value(&self, table: &str, object_id: u32, field: &str) -> Result<i64, String> {
        self.field(table, field)?.baseline_value(object_id)
    }

    pub fn tbl_value(&self, index: u32) -> Result<&str, String> {
        self.tbl
            .get(index as usize)
            .map(String::as_str)
            .ok_or_else(|| format!("TBL index {index} is outside the default catalog"))
    }

    pub fn xdat_value(&self, table: &str, object_id: u32, field: &str) -> Result<i64, String> {
        match (table, field) {
            ("wireframe", "wire") if object_id < 228 => Ok(object_id as i64),
            ("wireframe", "grp") if object_id < 131 => Ok(object_id as i64),
            ("wireframe", "tran") if object_id < 106 => Ok(object_id as i64),
            ("ButtonSet", "ButtonSet") if (object_id as usize) < self.button_defaults.len() => {
                Ok(object_id as i64)
            }
            ("statusinfor", "Status") => self
                .status
                .get(object_id as usize)
                .map(|value| value.0)
                .ok_or_else(|| format!("status object {object_id} is outside the catalog")),
            ("statusinfor", "Display") => self
                .status
                .get(object_id as usize)
                .map(|value| value.1)
                .ok_or_else(|| format!("status object {object_id} is outside the catalog")),
            _ => Err(format!("unknown XDAT field {table}.{field}")),
        }
    }

    pub fn button_csv(&self, set_id: u32) -> Result<String, String> {
        Ok(self
            .button_default(set_id)?
            .buttons
            .iter()
            .map(button_csv)
            .collect::<Vec<_>>()
            .join("."))
    }

    pub fn requirement_payload(&self, table: &str, object_id: u32) -> Result<String, String> {
        let requirement = self
            .requirements
            .get(table)
            .and_then(|objects| objects.get(object_id as usize))
            .ok_or_else(|| format!("requirement {table}.{object_id} is outside the catalog"))?;
        let mut output = String::from("4");
        for block in &requirement.blocks {
            output.push('.');
            output.push_str(&block.opcode.to_string());
            if let Some(value) = block.value {
                output.push(',');
                output.push_str(&value.to_string());
            }
        }
        Ok(output)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeBuildArtifacts {
    pub build_dir: String,
    pub wireframe_editor: Option<String>,
    pub requirement_file: Option<String>,
    pub eds_path: String,
    pub output_map: String,
    pub data_editor: Option<String>,
    pub extra_data_editor: Option<String>,
    pub custom_tbl: Option<String>,
    pub python_path_bootstrap: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeBuildError {
    pub source: String,
    pub file: String,
    pub line: u64,
    pub message: String,
    pub raw: String,
    /// How many identical diagnostics this entry stands for (warnings merge).
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeBuildResult {
    pub ok: bool,
    pub errors: Vec<NativeBuildError>,
    /// euddraft warnings (`warn_with_traceback` stacks, `[Warning]` lines); never fail a build.
    pub warnings: Vec<NativeBuildError>,
    pub raw_status: u32,
    pub stdout: String,
    pub stderr: String,
    /// Absolute path of the complete stdout/stderr log written for this run; empty when the
    /// log could not be written (then `warnings` says why). The tool observation exposes the
    /// project-relative `BUILD_LOG_RELATIVE_PATH` instead.
    pub log_path: String,
    pub artifacts: NativeBuildArtifacts,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EuddraftLaunch {
    Executable(PathBuf),
    SourceRepository {
        uv: PathBuf,
        root: PathBuf,
        script: PathBuf,
    },
}

impl EuddraftLaunch {
    pub fn resolve(configured: &Path) -> Result<Self, String> {
        if configured.is_file() {
            if configured
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("py"))
            {
                let root = configured
                    .parent()
                    .ok_or_else(|| "euddraft.py has no parent".to_string())?
                    .to_path_buf();
                let uv = which::which("uv").map_err(|_| {
                    "uv is required to run the euddraft source repository".to_string()
                })?;
                return Ok(Self::SourceRepository {
                    uv,
                    root,
                    script: configured.to_path_buf(),
                });
            }
            crate::bootstrap::validate_managed_install(configured)?;
            return Ok(Self::Executable(configured.to_path_buf()));
        }
        if configured.is_dir() {
            let script = configured.join("euddraft.py");
            if script.is_file() {
                let uv = which::which("uv").map_err(|_| {
                    "uv is required to run the euddraft source repository".to_string()
                })?;
                return Ok(Self::SourceRepository {
                    uv,
                    root: configured.to_path_buf(),
                    script,
                });
            }
            let executable = configured.join("euddraft.exe");
            if executable.is_file() {
                crate::bootstrap::validate_managed_install(&executable)?;
                return Ok(Self::Executable(executable));
            }
        }
        Err(format!(
            "euddraft path does not identify euddraft.exe, euddraft.py, or a source root: {}",
            configured.display()
        ))
    }

    pub fn frozen_executable(&self) -> Result<&Path, String> {
        match self {
            Self::Executable(path)
                if path.is_file()
                    && path.file_name().is_some_and(|name| {
                        name.to_string_lossy().eq_ignore_ascii_case("euddraft.exe")
                    }) =>
            {
                Ok(path)
            }
            Self::Executable(path) => Err(format!(
                "설정 경로가 frozen euddraft.exe를 가리키지 않습니다: {}",
                path.display()
            )),
            Self::SourceRepository { .. } => Err(
                "직접 Python 실행에는 소스 저장소가 아닌 frozen euddraft.exe가 필요합니다."
                    .to_string(),
            ),
        }
    }

    pub fn frozen_fingerprint(&self) -> Result<String, String> {
        crate::bootstrap::sha256_file(self.frozen_executable()?)
            .map_err(|error| format!("frozen euddraft.exe 지문을 계산하지 못했습니다: {error}"))
    }

    fn command(&self, eds_path: &Path) -> Result<Command, String> {
        let mut command = match self {
            Self::Executable(executable) => {
                let mut command = Command::new(executable);
                configure_frozen_euddraft_environment(&mut command);
                command
            }
            Self::SourceRepository { uv, root, script } => {
                let mut command = Command::new(uv);
                command
                    .arg("run")
                    .arg("--project")
                    .arg(root)
                    .arg("python")
                    .arg(script);
                command
            }
        };
        command.arg(eds_path);
        Ok(command)
    }

    pub(crate) fn run(
        &self,
        eds_path: &Path,
        timeout: Duration,
    ) -> Result<CapturedProcess, String> {
        self.run_with_cancellation(eds_path, timeout, None)
    }

    pub(crate) fn run_with_cancellation(
        &self,
        eds_path: &Path,
        timeout: Duration,
        cancellation: Option<&ProcessCancellation>,
    ) -> Result<CapturedProcess, String> {
        run_process(self.command(eds_path)?, eds_path, timeout, cancellation)
    }
}

fn configure_frozen_euddraft_environment(command: &mut Command) {
    command.env_clear();
    for key in ["SystemRoot", "WINDIR", "TEMP", "TMP"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command
        .env("PYTHONNOUSERSITE", "1")
        .env("PYTHONDONTWRITEBYTECODE", "1");
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FrozenPythonIdentity {
    pub python_abi: String,
    pub euddraft_fingerprint: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrozenPythonProbeReport {
    python_abi: String,
    imported: Vec<String>,
}

/// Execute a real plugin inside the configured frozen runtime. With an environment,
/// every importable top-level module advertised by the locked distributions is imported.
pub fn probe_frozen_python(
    project: &NativeProject,
    euddraft: &EuddraftLaunch,
    probe_root: &Path,
    site_packages: Option<&Path>,
    package_names: &[String],
    cancellation: Option<&ProcessCancellation>,
) -> Result<FrozenPythonIdentity, String> {
    let executable = euddraft.frozen_executable()?;
    let fingerprint = crate::bootstrap::sha256_file(executable)
        .map_err(|error| format!("frozen euddraft.exe 지문을 계산하지 못했습니다: {error}"))?;
    fs::create_dir_all(probe_root).map_err(stringify_io)?;
    let report_path = probe_root.join("python-probe.json");
    let output_map = probe_root.join("python-probe.scx");
    let script_path = probe_root.join("PythonRuntimeProbe.py");
    let eds_path = probe_root.join("python-runtime-probe.eds");
    let source_map = project.source_map_path()?;
    let site_json = match site_packages {
        Some(path) => serde_json::to_string(&path.to_string_lossy().into_owned())
            .map_err(|error| error.to_string())?,
        None => "None".to_string(),
    };
    let report_json =
        serde_json::to_string(&report_path.to_string_lossy()).map_err(|error| error.to_string())?;
    let packages_json = serde_json::to_string(package_names).map_err(|error| error.to_string())?;
    let script = format!(
        r#"import importlib
import importlib.metadata
import json
import pathlib
import re
import sys

sys.dont_write_bytecode = True
site_packages = {site_json}
if site_packages is not None and site_packages not in sys.path:
    sys.path.insert(0, site_packages)
wanted = {{re.sub(r"[-_.]+", "-", name).lower() for name in {packages_json}}}
imported = []
if wanted:
    installed = {{
        re.sub(r"[-_.]+", "-", distribution.metadata["Name"]).lower()
        for distribution in importlib.metadata.distributions(path=[site_packages])
    }}
    missing = sorted(wanted - installed)
    if missing:
        raise RuntimeError("잠금 패키지 메타데이터가 설치 환경에 없습니다: " + ", ".join(missing))
    for module, distributions in sorted(importlib.metadata.packages_distributions().items()):
        normalized = {{re.sub(r"[-_.]+", "-", name).lower() for name in distributions}}
        if wanted.intersection(normalized):
            importlib.import_module(module)
            imported.append(module)
tag = sys.implementation.cache_tag
if not isinstance(tag, str) or not tag.startswith("cpython-"):
    raise RuntimeError("지원되는 CPython ABI 태그를 확인할 수 없습니다.")
python_abi = "cp" + tag.removeprefix("cpython-")
pathlib.Path({report_json}).write_text(
    json.dumps({{"python_abi": python_abi, "imported": imported}}, sort_keys=True),
    encoding="utf-8",
)
"#
    );
    write_atomic_bytes(&script_path, script.as_bytes()).map_err(|error| error.to_string())?;
    let eds = format!(
        "[main]\ninput: {}\noutput: {}\n\n[PythonRuntimeProbe.py]\n",
        path_text(&source_map).replace('\\', "/"),
        path_text(&output_map).replace('\\', "/")
    );
    write_atomic_bytes(&eds_path, eds.as_bytes()).map_err(|error| error.to_string())?;
    let captured = euddraft.run_with_cancellation(&eds_path, PYTHON_PROBE_TIMEOUT, cancellation)?;
    if !captured.success {
        return Err(format!(
            "frozen euddraft Python 호환성 검사가 실패했습니다 (종료 코드 0x{:08X}): {}",
            captured.raw_status,
            joined_output(&captured.stdout, &captured.stderr)
        ));
    }
    let report: FrozenPythonProbeReport = serde_json::from_slice(
        &fs::read(&report_path)
            .map_err(|error| format!("frozen Python 검사 결과를 읽지 못했습니다: {error}"))?,
    )
    .map_err(|error| format!("frozen Python 검사 결과가 올바르지 않습니다: {error}"))?;
    if !package_names.is_empty() && report.imported.is_empty() {
        return Err("설치된 Python 의존성에서 가져올 모듈을 찾지 못했습니다.".to_string());
    }
    if !report.python_abi.starts_with("cp")
        || !report.python_abi[2..]
            .bytes()
            .all(|byte| byte.is_ascii_digit())
    {
        return Err("frozen Python ABI 결과가 올바르지 않습니다.".to_string());
    }
    Ok(FrozenPythonIdentity {
        python_abi: report.python_abi,
        euddraft_fingerprint: fingerprint,
    })
}

pub fn sync_compat_assets(source: &Path, destination: &Path) -> Result<usize, String> {
    if !source.is_dir() {
        return Err(format!(
            "bundled native compatibility assets are missing: {}",
            source.display()
        ));
    }
    let mut copied = 0;
    sync_asset_directory(source, source, destination, &mut copied)?;
    Ok(copied)
}

fn sync_asset_directory(
    source_root: &Path,
    current: &Path,
    destination: &Path,
    copied: &mut usize,
) -> Result<(), String> {
    let mut entries: Vec<_> = fs::read_dir(current)
        .map_err(stringify_io)?
        .collect::<Result<_, _>>()
        .map_err(stringify_io)?;
    entries.sort_by_key(|entry| entry.file_name().to_string_lossy().to_lowercase());
    for entry in entries {
        let file_type = entry.file_type().map_err(stringify_io)?;
        if file_type.is_symlink() {
            return Err(format!(
                "compatibility asset symlinks are forbidden: {}",
                entry.path().display()
            ));
        }
        if file_type.is_dir() {
            sync_asset_directory(source_root, &entry.path(), destination, copied)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(source_root)
            .map_err(|error| error.to_string())?
            .to_path_buf();
        let target = destination.join(relative);
        let source_bytes = fs::read(entry.path()).map_err(stringify_io)?;
        if fs::read(&target).ok().as_deref() == Some(source_bytes.as_slice()) {
            continue;
        }
        write_atomic_bytes(&target, &source_bytes).map_err(|error| error.to_string())?;
        *copied += 1;
    }
    Ok(())
}

pub fn generate_native_build(
    project: &NativeProject,
    compat_root: &Path,
) -> Result<NativeBuildArtifacts, String> {
    generate_native_build_with_python(project, compat_root, None)
}

pub fn generate_native_build_with_python(
    project: &NativeProject,
    compat_root: &Path,
    python_site_packages: Option<&Path>,
) -> Result<NativeBuildArtifacts, String> {
    let catalog = DatCatalog::load(compat_root)?;
    let build_dir = project.root().join("build/euddraft");
    fs::create_dir_all(&build_dir).map_err(stringify_io)?;

    let data_editor = generate_data_editor(project.dat(), &catalog)?;
    let data_editor_path = build_dir.join("DataEditor.py");
    let data_editor_output = if let Some(data_editor) = &data_editor {
        write_atomic_bytes(&data_editor_path, data_editor.as_bytes())
            .map_err(|error| error.to_string())?;
        Some(path_text(&data_editor_path))
    } else {
        fs::remove_file(&data_editor_path).ok();
        None
    };

    let requirement_artifacts = generate_requirement_artifacts(project.dat(), &catalog)?;
    let requirement_path = build_dir.join("RequireData");
    let requirement_output = if let Some(requirements) = &requirement_artifacts {
        write_atomic_bytes(&requirement_path, &requirements.bytes)
            .map_err(|error| error.to_string())?;
        Some(path_text(&requirement_path))
    } else {
        fs::remove_file(&requirement_path).ok();
        None
    };

    let wireframe_path = build_dir.join("WireFrameDataEditor.eps");
    let wireframe_output = if project.dat().xdat.tables.contains_key("wireframe") {
        let bytes = fs::read(compat_root.join("WireFrameDataEditor.eps")).map_err(stringify_io)?;
        write_atomic_bytes(&wireframe_path, &bytes).map_err(|error| error.to_string())?;
        Some(path_text(&wireframe_path))
    } else {
        fs::remove_file(&wireframe_path).ok();
        None
    };

    let extra_editor =
        generate_extra_data_editor(project.dat(), &catalog, requirement_artifacts.as_ref())?;
    let extra_editor_path = build_dir.join("ExtraDataEditor.py");
    let extra_editor_output = if let Some(extra_editor) = &extra_editor {
        write_atomic_bytes(&extra_editor_path, extra_editor.as_bytes())
            .map_err(|error| error.to_string())?;
        Some(path_text(&extra_editor_path))
    } else {
        fs::remove_file(&extra_editor_path).ok();
        None
    };

    let custom_tbl_path = build_dir.join("custom_txt.tbl");
    let custom_tbl = if project.dat().tbl.values.is_empty() {
        fs::remove_file(&custom_tbl_path).ok();
        None
    } else {
        let baseline =
            decode_tbl(&fs::read(compat_root.join("Tbls/stat_txt.tbl")).map_err(stringify_io)?)?;
        let encoded = encode_custom_tbl(&baseline, &project.dat().tbl.values)?;
        write_atomic_bytes(&custom_tbl_path, &encoded).map_err(|error| error.to_string())?;
        Some(path_text(&custom_tbl_path))
    };

    let python_bootstrap_path = build_dir.join("PythonPath.py");
    let mut python_search_paths = Vec::new();
    if !project.manifest().python_entrypoints.is_empty() {
        python_search_paths.push(path_text(
            &fs::canonicalize(project.root().join("src")).map_err(stringify_io)?,
        ));
    }
    if let Some(site_packages) = python_site_packages {
        python_search_paths.push(path_text(site_packages));
    }
    let python_path_bootstrap = if python_search_paths.is_empty() {
        fs::remove_file(&python_bootstrap_path).ok();
        None
    } else {
        let quoted_paths =
            serde_json::to_string(&python_search_paths).map_err(|error| error.to_string())?;
        let source = format!(
            "import importlib.machinery\nimport sys\nsys.dont_write_bytecode = True\n_eud_agent_paths = {quoted_paths}\nclass _EudAgentSourceFinder:\n    @staticmethod\n    def find_spec(fullname, path=None, target=None):\n        if path is not None:\n            return None\n        return importlib.machinery.PathFinder.find_spec(fullname, _eud_agent_paths)\nsys.meta_path.insert(0, _EudAgentSourceFinder)\n"
        );
        write_atomic_bytes(&python_bootstrap_path, source.as_bytes())
            .map_err(|error| error.to_string())?;
        Some(path_text(&python_bootstrap_path))
    };

    let eds_path = build_dir.join("eud-agent.eds");
    let eds = generate_eds(
        project.root(),
        project.manifest(),
        &eds_path,
        GeneratedEdsSections {
            python_path: python_path_bootstrap.is_some(),
            data_editor: data_editor_output.is_some(),
            extra_editor: extra_editor_output.is_some(),
            custom_tbl: custom_tbl.is_some(),
            wireframe_editor: wireframe_output.is_some(),
        },
    )?;
    write_atomic_bytes(&eds_path, eds.as_bytes()).map_err(|error| error.to_string())?;

    Ok(NativeBuildArtifacts {
        build_dir: path_text(&build_dir),
        requirement_file: requirement_output,
        eds_path: path_text(&eds_path),
        output_map: path_text(&project.output_map_path()?),
        data_editor: data_editor_output,
        extra_data_editor: extra_editor_output,
        custom_tbl,
        wireframe_editor: wireframe_output,
        python_path_bootstrap,
    })
}

pub fn run_native_build(
    project: &NativeProject,
    compat_root: &Path,
    euddraft: &EuddraftLaunch,
) -> Result<NativeBuildResult, String> {
    run_native_build_with_python(project, compat_root, euddraft, None)
}

pub fn run_native_build_with_python(
    project: &NativeProject,
    compat_root: &Path,
    euddraft: &EuddraftLaunch,
    python_site_packages: Option<&Path>,
) -> Result<NativeBuildResult, String> {
    run_native_build_with_python_and_cancellation(
        project,
        compat_root,
        euddraft,
        python_site_packages,
        None,
    )
}

pub fn run_native_build_with_python_and_cancellation(
    project: &NativeProject,
    compat_root: &Path,
    euddraft: &EuddraftLaunch,
    python_site_packages: Option<&Path>,
    cancellation: Option<&ProcessCancellation>,
) -> Result<NativeBuildResult, String> {
    let has_direct_python = project.has_direct_python()?;
    if has_direct_python {
        euddraft.frozen_executable()?;
    }
    match (
        project.manifest().python_lock.as_ref(),
        python_site_packages,
    ) {
        (Some(lock), None) if !lock.packages.is_empty() => {
            return Err("Python 잠금에 맞는 검증된 의존성 환경이 없습니다.".to_string());
        }
        (None, Some(_)) => {
            return Err("Python 잠금 없이 의존성 환경을 빌드에 주입할 수 없습니다.".to_string());
        }
        (_, Some(path)) if !path.is_dir() => {
            return Err("검증된 Python 의존성 환경 디렉터리가 없습니다.".to_string());
        }
        _ => {}
    }
    let artifacts = generate_native_build_with_python(project, compat_root, python_site_packages)?;
    let eds_path = PathBuf::from(&artifacts.eds_path);
    let output_map = PathBuf::from(&artifacts.output_map);
    let before_output = modified_time(&output_map)?;
    // A run that times out, is cancelled, or fails to spawn must not leave the previous
    // run's log behind for build_log_read to page as if it were this build's.
    let stale_log = remove_stale_build_log(project.root()).err();
    let captured = euddraft.run_with_cancellation(&eds_path, EUDDRAFT_TIMEOUT, cancellation)?;
    let fresh_output = is_fresh_output(before_output, modified_time(&output_map)?);
    let eds_dir = eds_path
        .parent()
        .ok_or_else(|| "EDS path has no parent".to_string())?;
    let log = match stale_log {
        // A stale log that could not be removed must not be presented as this run's.
        Some(error) => Err(error),
        None => write_build_log(project.root(), &captured),
    };
    Ok(assemble_build_result(
        captured,
        fresh_output,
        artifacts,
        project.root(),
        eds_dir,
        log,
    ))
}

fn remove_stale_build_log(project_root: &Path) -> Result<(), String> {
    match fs::remove_file(build_log_path(project_root)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("이전 빌드 로그를 지우지 못했습니다: {error}")),
    }
}

/// Path of the complete build log inside the project `build/` tree.
pub fn build_log_path(project_root: &Path) -> PathBuf {
    project_root.join(BUILD_LOG_RELATIVE_PATH)
}

/// Persist the raw euddraft stdout/stderr so a bounded tool observation can point at it.
fn write_build_log(project_root: &Path, captured: &CapturedProcess) -> Result<String, String> {
    let path = build_log_path(project_root);
    let content = format!(
        "# euddraft build log\n# exit status: 0x{:08X}\n# ===== stdout =====\n{}\n# ===== stderr =====\n{}\n",
        captured.raw_status, captured.stdout, captured.stderr
    );
    write_atomic_bytes(&path, content.as_bytes())
        .map_err(|error| format!("빌드 로그를 쓰지 못했습니다 ({}): {error}", path.display()))?;
    Ok(path_text(&path))
}

/// Bounded, model-facing view of one run's stdout/stderr: long lines are cut and each
/// stream keeps its head and tail. The complete text stays in `BUILD_LOG_RELATIVE_PATH`.
pub fn output_excerpt(stdout: &str, stderr: &str) -> String {
    let mut excerpt = String::new();
    for (name, text) in [("stdout", stdout), ("stderr", stderr)] {
        let text = text.trim_end();
        let line_count = text.lines().count();
        excerpt.push_str(&format!(
            "[{name}: {} lines, {} chars]\n",
            line_count,
            text.chars().count()
        ));
        if text.is_empty() {
            continue;
        }
        let cut: Vec<String> = text
            .lines()
            .map(|line| {
                let length = line.chars().count();
                if length <= EXCERPT_LINE_LIMIT {
                    line.to_string()
                } else {
                    let head: String = line.chars().take(EXCERPT_LINE_LIMIT).collect();
                    format!(
                        "{head} [… line cut, {} more chars]",
                        length - EXCERPT_LINE_LIMIT
                    )
                }
            })
            .collect();
        let joined = cut.join("\n");
        if joined.chars().count() <= EXCERPT_STREAM_LIMIT {
            excerpt.push_str(&joined);
            excerpt.push('\n');
            continue;
        }
        let head_budget = EXCERPT_STREAM_LIMIT * 2 / 3;
        let tail_budget = EXCERPT_STREAM_LIMIT - head_budget;
        let mut head_end = 0;
        let mut used = 0;
        for (index, line) in cut.iter().enumerate() {
            let cost = line.chars().count() + 1;
            if used + cost > head_budget {
                break;
            }
            used += cost;
            head_end = index + 1;
        }
        let mut tail_start = cut.len();
        used = 0;
        while tail_start > head_end {
            let cost = cut[tail_start - 1].chars().count() + 1;
            if used + cost > tail_budget {
                break;
            }
            used += cost;
            tail_start -= 1;
        }
        excerpt.push_str(&cut[..head_end].join("\n"));
        excerpt.push_str(&format!(
            "\n[… {} lines elided; full text in {BUILD_LOG_RELATIVE_PATH} via build_log_read …]\n",
            tail_start - head_end
        ));
        excerpt.push_str(&cut[tail_start..].join("\n"));
        excerpt.push('\n');
    }
    excerpt
}

fn assemble_build_result(
    captured: CapturedProcess,
    fresh_output: bool,
    artifacts: NativeBuildArtifacts,
    project_root: &Path,
    eds_dir: &Path,
    log: Result<String, String>,
) -> NativeBuildResult {
    let EuddraftDiagnostics {
        mut errors,
        mut warnings,
    } = parse_euddraft_output(&captured.stdout, &captured.stderr, project_root, eds_dir);
    // The log is a convenience copy, never build authority: a failed write is reported,
    // not allowed to discard euddraft's verdict.
    let log_path = match log {
        Ok(path) => path,
        Err(error) => {
            warnings.push(NativeBuildError {
                source: "eud-agent".to_string(),
                file: BUILD_LOG_RELATIVE_PATH.to_string(),
                line: 0,
                message: error,
                raw: String::new(),
                count: 1,
            });
            String::new()
        }
    };
    let ok = captured.success && fresh_output && errors.is_empty();
    if !ok && errors.is_empty() {
        let raw = joined_output(&captured.stdout, &captured.stderr);
        errors.push(NativeBuildError {
            source: "euddraft".to_string(),
            file: String::new(),
            line: 0,
            message: if captured.success {
                "euddraft exited successfully but did not produce a fresh output map".to_string()
            } else if raw.is_empty() {
                format!(
                    "euddraft가 진단 출력 없이 종료되었습니다 (종료 코드 0x{:08X}).",
                    captured.raw_status
                )
            } else {
                format!(
                    "euddraft가 인식되지 않는 진단 형식으로 종료되었습니다 (종료 코드 0x{:08X}).",
                    captured.raw_status
                )
            },
            raw: bound_text(&raw, DIAGNOSTIC_RAW_LIMIT),
            count: 1,
        });
    }
    NativeBuildResult {
        ok,
        errors,
        warnings,
        raw_status: captured.raw_status,
        stdout: captured.stdout,
        stderr: captured.stderr,
        log_path,
        artifacts,
    }
}

fn generate_data_editor(
    dat: &NativeDatState,
    catalog: &DatCatalog,
) -> Result<Option<String>, String> {
    if dat.standard.tables.is_empty() {
        return Ok(None);
    }
    let mut lines = Vec::new();
    for (table, objects) in &dat.standard.tables {
        for (object_id, fields) in objects {
            for (field, change) in fields {
                let meta = catalog.field(table, field)?;
                validate_numeric_override(meta, *object_id, change)?;
                if meta.offset == 0 {
                    return Err(format!("DAT field {table}.{field} has no runtime offset"));
                }
                let local = meta.local_index(*object_id)?;
                let byte_offset = meta
                    .offset
                    .checked_add(local.saturating_mul(meta.size as u32 * meta.var_array))
                    .ok_or_else(|| format!("DAT offset overflow for {table}.{field}"))?;
                let shift = byte_offset % 4;
                let real_offset = byte_offset - shift;
                let delta = change.after - change.before;
                let real_value = delta
                    .checked_mul(256_i64.pow(shift))
                    .ok_or_else(|| format!("DAT delta overflow for {table}.{field}"))?;
                lines.push(format!(
                    "        SetMemory(0x{real_offset:X}, Add, {real_value}),# {table}:{field}  index:{object_id}    from {} To {}",
                    change.before, change.after
                ));
            }
        }
    }
    let mut output = String::from(
        "from eudplib import *\n\n\ndef onPluginStart():\n    DoActions([  # Basic DatFile Actions\n",
    );
    for line in lines {
        output.push_str(&line);
        output.push('\n');
    }
    output.push_str("    ])\n");
    Ok(Some(output))
}

fn validate_numeric_override(
    meta: &DatFieldMeta,
    object_id: u32,
    change: &NumericOverride,
) -> Result<(), String> {
    meta.local_index(object_id)?;
    let (min, max) = meta.value_range();
    if change.before < min || change.before > max || change.after < min || change.after > max {
        return Err(format!(
            "{} value must be in {min}..{max} (before={}, after={})",
            meta.name, change.before, change.after
        ));
    }
    Ok(())
}

fn generate_requirement_artifacts(
    dat: &NativeDatState,
    catalog: &DatCatalog,
) -> Result<Option<RequirementArtifacts>, String> {
    if dat.requirements.tables.is_empty() {
        return Ok(None);
    }
    const SPECS: [(&str, usize, bool); 5] = [
        ("units", 1096, false),
        ("upgrades", 840, false),
        ("techdata", 320, false),
        ("Stechdata", 688, false),
        ("orders", 1316, true),
    ];
    let mut bytes = Vec::new();
    let mut pointers_by_table = BTreeMap::new();
    for (table, capacity, writes_object_id) in SPECS {
        let defaults = &catalog.requirements[table];
        let overrides = dat.requirements.tables.get(table);
        let mut section = vec![0_u8, 0_u8];
        let mut pointers = vec![0_u16; defaults.len()];
        for (object_id, default) in defaults.iter().enumerate() {
            let resolved = match overrides.and_then(|values| values.get(&(object_id as u32))) {
                Some(change) => resolve_requirement_payload(&change.after, default)?,
                None => ResolvedRequirement {
                    allocated: default.allocated,
                    blocks: default.blocks.clone(),
                },
            };
            if !resolved.allocated {
                continue;
            }
            if writes_object_id {
                // Editor records StartPos after the order id word; stock require.dat
                // points every order past its id as well.
                section.extend_from_slice(&(object_id as u16).to_le_bytes());
            }
            let pointer = u16::try_from(section.len() / 2)
                .map_err(|_| format!("{table} requirement pointer overflow"))?;
            pointers[object_id] = pointer;
            for block in &resolved.blocks {
                write_requirement_block(&mut section, block)?;
            }
            section.extend_from_slice(&0xffff_u16.to_le_bytes());
        }
        section.extend_from_slice(&0xffff_u16.to_le_bytes());
        if section.len() > capacity {
            return Err(format!(
                "{table} requirements use {} bytes, exceeding {capacity}",
                section.len()
            ));
        }
        section.resize(capacity, 0);
        bytes.extend_from_slice(&section);
        pointers_by_table.insert(table.to_string(), pointers);
    }
    Ok(Some(RequirementArtifacts {
        bytes,
        pointers: pointers_by_table,
    }))
}

#[derive(Debug)]
struct ResolvedRequirement {
    allocated: bool,
    blocks: Vec<RequirementBlock>,
}

fn resolve_requirement_payload(
    payload: &str,
    default: &RequirementDefault,
) -> Result<ResolvedRequirement, String> {
    let segments: Vec<_> = payload.split('.').collect();
    let mode = segments
        .first()
        .and_then(|value| value.parse::<u8>().ok())
        .ok_or_else(|| "requirement payload mode must be 0..4".to_string())?;
    match mode {
        0 => Ok(ResolvedRequirement {
            allocated: default.allocated,
            blocks: default.blocks.clone(),
        }),
        1 => Ok(ResolvedRequirement {
            allocated: false,
            blocks: Vec::new(),
        }),
        2 => Ok(ResolvedRequirement {
            allocated: true,
            blocks: Vec::new(),
        }),
        3 => {
            let current: Vec<_> = default
                .blocks
                .iter()
                .filter(|block| block.opcode == 2)
                .cloned()
                .collect();
            let mut blocks = Vec::new();
            for block in current {
                if !blocks.is_empty() {
                    blocks.push(RequirementBlock {
                        opcode: 1,
                        value: None,
                    });
                }
                blocks.push(block);
            }
            Ok(ResolvedRequirement {
                allocated: true,
                blocks,
            })
        }
        4 => {
            let mut blocks = Vec::new();
            for segment in segments.into_iter().skip(1) {
                let (opcode, value) = match segment.split_once(',') {
                    Some((opcode, value)) => (
                        opcode
                            .parse::<u16>()
                            .map_err(|_| "requirement opcode is invalid".to_string())?,
                        Some(
                            value
                                .parse::<u16>()
                                .map_err(|_| "requirement value is invalid".to_string())?,
                        ),
                    ),
                    None => (
                        segment
                            .parse::<u16>()
                            .map_err(|_| "requirement opcode is invalid".to_string())?,
                        None,
                    ),
                };
                if opcode > 255 {
                    return Err("requirement opcode must be in 0..255".to_string());
                }
                if matches!(opcode, 0 | 2 | 3 | 4 | 37) != value.is_some() {
                    return Err(format!(
                        "requirement opcode {opcode} has an invalid value shape"
                    ));
                }
                blocks.push(RequirementBlock { opcode, value });
            }
            Ok(ResolvedRequirement {
                allocated: true,
                blocks,
            })
        }
        _ => Err("requirement payload mode must be 0..4".to_string()),
    }
}

fn write_requirement_block(output: &mut Vec<u8>, block: &RequirementBlock) -> Result<(), String> {
    if block.opcode == 0 {
        let value = block
            .value
            .ok_or_else(|| "Must-have requirement has no value".to_string())?;
        output.extend_from_slice(&value.to_le_bytes());
        return Ok(());
    }
    output.extend_from_slice(&(0xff00_u16 + block.opcode).to_le_bytes());
    if let Some(value) = block.value {
        output.extend_from_slice(&value.to_le_bytes());
    }
    Ok(())
}

fn generate_extra_data_editor(
    dat: &NativeDatState,
    catalog: &DatCatalog,
    requirements: Option<&RequirementArtifacts>,
) -> Result<Option<String>, String> {
    if dat.xdat.tables.is_empty() && dat.buttons.values.is_empty() && requirements.is_none() {
        return Ok(None);
    }
    const STATUS_FUNCTIONS: [u32; 9] = [
        4343040, 4344192, 4346240, 4345616, 4344656, 4344560, 4344512, 4348160, 4343072,
    ];
    const DISPLAY_FUNCTIONS: [u32; 9] = [
        4353872, 4356240, 4357264, 4355232, 4355040, 4354656, 4357424, 4353760, 4349664,
    ];
    let wireframe = dat.xdat.tables.get("wireframe");
    let mut output = String::from("from eudplib import *\n");
    if let Some(wireframe) = wireframe {
        output.push_str("import WireFrameDataEditor\n\n");
        output.push_str("def init_wireframe():\n");
        output.push_str("    WireFrameDataEditor.WireFrameInit()\n");
        for (object_id, fields) in wireframe {
            for (field, change) in fields {
                let (function, limit) = match field.as_str() {
                    "wire" => ("ChangeWireframe", 228),
                    "grp" => ("ChangeGrpframe", 131),
                    "tran" => ("ChangeTranframe", 106),
                    _ => return Err(format!("unknown wireframe field {field}")),
                };
                if change.after < 0 || change.after >= limit {
                    return Err(format!(
                        "wireframe {field} value {} is outside 0..{}",
                        change.after,
                        limit - 1
                    ));
                }
                output.push_str(&format!(
                    "    WireFrameDataEditor.{function}({object_id}, {})\n",
                    change.after
                ));
            }
        }
    }
    output.push_str("\n\ndef onPluginStart():\n");
    if wireframe.is_some() {
        output.push_str("    init_wireframe()\n");
    }

    if let Some(objects) = dat.xdat.tables.get("statusinfor") {
        output.push_str("    DoActions([  # status functions\n");
        for (object_id, fields) in objects {
            for (field, change) in fields {
                let functions = match field.as_str() {
                    "Status" => &STATUS_FUNCTIONS,
                    "Display" => &DISPLAY_FUNCTIONS,
                    _ => return Err(format!("unknown statusinfor field {field}")),
                };
                let index = usize::try_from(change.after)
                    .ok()
                    .filter(|index| *index < functions.len())
                    .ok_or_else(|| format!("statusinfor {field} value is outside 0..8"))?;
                let base = if field == "Status" {
                    0x5193A4_u32
                } else {
                    0x5193A8
                };
                output.push_str(&format!(
                    "        SetMemory(0x{:X}, SetTo, {}),\n",
                    base + 12 * object_id,
                    functions[index]
                ));
            }
        }
        output.push_str("    ])\n");
    }

    output.push_str("    # button sets\n");
    let mut custom_buttons = BTreeMap::new();
    for (set_id, change) in &dat.buttons.values {
        let buttons = parse_button_csv(&change.after)?;
        let bytes = button_bytes(&buttons);
        output.push_str("    bytebuffer = bytearray([");
        output.push_str(
            &bytes
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(","),
        );
        output.push_str("])\n");
        output.push_str(&format!("    btnptr{set_id} = Db(bytebuffer)\n"));
        custom_buttons.insert(*set_id, buttons.len() as u32);
    }
    let mut assignments = BTreeMap::new();
    for (set_id, count) in &custom_buttons {
        assignments.insert(*set_id, (*set_id, *count));
    }
    if let Some(objects) = dat.xdat.tables.get("ButtonSet") {
        for (unit_id, fields) in objects {
            if let Some(change) = fields.get("ButtonSet") {
                if change.after < 0 {
                    return Err(format!("ButtonSet {unit_id} target must be non-negative"));
                }
                let target = change.after as u32;
                let count = custom_buttons
                    .get(&target)
                    .copied()
                    .unwrap_or(catalog.button_default(target)?.count);
                assignments.insert(*unit_id, (target, count));
            }
        }
    }
    for (unit_id, (set_id, count)) in assignments {
        let address = if custom_buttons.contains_key(&set_id) {
            format!("btnptr{set_id}")
        } else {
            catalog.button_default(set_id)?.address.to_string()
        };
        output.push_str("    DoActions([\n");
        output.push_str(&format!(
            "        SetMemory(0x{:X}, SetTo, {address}),\n",
            0x5187EC_u32 + 12 * unit_id
        ));
        output.push_str(&format!(
            "        SetMemory(0x{:X}, SetTo, {count}),\n",
            0x5187E8_u32 + 12 * unit_id
        ));
        output.push_str("    ])\n");
    }

    if let Some(requirements) = requirements {
        output.push_str("    with open('RequireData', 'rb') as file:\n");
        output.push_str("        inputData = file.read()\n");
        output.push_str("        inputData_db = Db(inputData)\n");
        output.push_str("        inputDwordN = (len(inputData) + 3) // 4\n");
        output.push_str("    addrEPD = EPD(0x514178)\n");
        output.push_str("    f_repmovsd_epd(addrEPD, EPD(inputData_db), inputDwordN)\n\n");
        output.push_str("def beforeTriggerExec():\n    DoActions([\n");
        for (table, pointer_base) in [
            ("units", 0x660A70_u32),
            ("upgrades", 0x6558C0),
            ("techdata", 0x656198),
            ("Stechdata", 0x6562F8),
            ("orders", 0x665580),
        ] {
            let pointers = &requirements.pointers[table];
            for (pair_index, pair) in pointers.chunks(2).enumerate() {
                let low = u32::from(pair[0]);
                let high = pair.get(1).copied().map(u32::from).unwrap_or(0);
                let value = low | (high << 16);
                output.push_str(&format!(
                    "        SetMemory(0x{pointer_base:X} + {}, SetTo, {value}),\n",
                    pair_index * 4
                ));
            }
        }
        output.push_str("    ])\n");
    }
    Ok(Some(output))
}

#[derive(Debug, Clone, Copy)]
struct Button {
    pos: u16,
    icon: u16,
    condition: u32,
    action: u32,
    condition_value: u16,
    action_value: u16,
    enabled_string: u16,
    disabled_string: u16,
}

fn parse_button_csv(csv: &str) -> Result<Vec<Button>, String> {
    let mut buttons = Vec::new();
    for (index, group) in csv.split('.').enumerate() {
        let values: Vec<_> = group.split(',').collect();
        if values.len() != 8 {
            return Err(format!(
                "button {} must contain exactly 8 fields",
                index + 1
            ));
        }
        let parse_u16 = |position: usize| {
            values[position]
                .parse::<u16>()
                .map_err(|_| format!("button {} field {} is invalid", index + 1, position + 1))
        };
        let parse_u32 = |position: usize| {
            values[position]
                .parse::<u32>()
                .map_err(|_| format!("button {} field {} is invalid", index + 1, position + 1))
        };
        buttons.push(Button {
            pos: parse_u16(0)?,
            icon: parse_u16(1)?,
            condition: parse_u32(2)?,
            action: parse_u32(3)?,
            condition_value: parse_u16(4)?,
            action_value: parse_u16(5)?,
            enabled_string: parse_u16(6)?,
            disabled_string: parse_u16(7)?,
        });
    }
    buttons.sort_by_key(|button| button.pos);
    Ok(buttons)
}
fn button_csv(button: &Button) -> String {
    format!(
        "{},{},{},{},{},{},{},{}",
        button.pos,
        button.icon,
        button.condition,
        button.action,
        button.condition_value,
        button.action_value,
        button.enabled_string,
        button.disabled_string
    )
}

fn button_bytes(buttons: &[Button]) -> Vec<u8> {
    let mut output = Vec::with_capacity(buttons.len() * 20);
    for button in buttons {
        output.extend_from_slice(&button.pos.to_le_bytes());
        output.extend_from_slice(&button.icon.to_le_bytes());
        output.extend_from_slice(&button.condition.to_le_bytes());
        output.extend_from_slice(&button.action.to_le_bytes());
        output.extend_from_slice(&button.condition_value.to_le_bytes());
        output.extend_from_slice(&button.action_value.to_le_bytes());
        output.extend_from_slice(&button.enabled_string.to_le_bytes());
        output.extend_from_slice(&button.disabled_string.to_le_bytes());
    }
    output
}

#[derive(Clone, Copy)]
struct GeneratedEdsSections {
    python_path: bool,
    data_editor: bool,
    extra_editor: bool,
    custom_tbl: bool,
    wireframe_editor: bool,
}

fn generate_eds(
    project_root: &Path,
    manifest: &ProjectManifest,
    eds_path: &Path,
    sections: GeneratedEdsSections,
) -> Result<String, String> {
    let eds_dir = eds_path
        .parent()
        .ok_or_else(|| "EDS path has no parent".to_string())?;
    let input = relative_path(
        eds_dir,
        &project_root.join(path_to_os(&manifest.source_map)),
    )?;
    let output = relative_path(
        eds_dir,
        &project_root.join(path_to_os(&manifest.output_map)),
    )?;
    let main = relative_path(eds_dir, &project_root.join(path_to_os(&manifest.main_file)))?;
    let mut eds = String::from("[main]\n");
    eds.push_str(&format!("input: {input}\noutput: {output}\n"));
    if manifest.settings.shuffle_payload {
        eds.push_str("shufflePayload: True\n");
    }
    if manifest.settings.debug {
        eds.push_str("debug: True\n");
    }
    if let Some(value) = &manifest.settings.decode_unit_name {
        eds.push_str(&format!("decodeUnitName: {value}\n"));
    }
    if let Some(value) = manifest.settings.object_field_count {
        eds.push_str(&format!("objFieldN: {value}\n"));
    }
    eds.push_str(&format!("sectorSize: {}\n", manifest.settings.sector_size));

    if sections.python_path {
        eds.push_str("\n[PythonPath.py]\n");
    }

    for plugin in &manifest.plugins {
        if let Some(raw_text) = &plugin.raw_text {
            eds.push('\n');
            eds.push_str(raw_text.trim());
            eds.push('\n');
            continue;
        }
        eds.push_str(&format!("\n[{}]\n", plugin.section));
        for entry in &plugin.entries {
            match &entry.value {
                Some(value) => eds.push_str(&format!("{}: {}\n", entry.key, value)),
                None => eds.push_str(&format!("{}\n", entry.key)),
            }
        }
    }
    if sections.data_editor {
        eds.push_str("\n[DataEditor.py]\n");
    }
    if sections.wireframe_editor {
        eds.push_str("\n[WireFrameDataEditor.eps]\n");
    }
    if sections.extra_editor {
        eds.push_str("\n[ExtraDataEditor.py]\n");
    }
    if sections.custom_tbl {
        eds.push_str("\n[dataDumper]\ncustom_txt.tbl: 0x6D5A30, copy\n");
    }
    for entrypoint in &manifest.python_entrypoints {
        let entrypoint = relative_path(eds_dir, &project_root.join(path_to_os(entrypoint)))?;
        eds.push_str(&format!("\n[{entrypoint}]\n"));
    }
    eds.push_str(&format!("\n[{main}]\n"));
    Ok(eds)
}

fn parse_offsets(source: &str) -> Result<BTreeMap<String, u32>, String> {
    let mut offsets = BTreeMap::new();
    for (line_index, line) in source.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (name, value) = line
            .split_once('=')
            .ok_or_else(|| format!("Offset.txt line {} is invalid", line_index + 1))?;
        let value = value.trim().trim_start_matches("0x");
        let parsed = u32::from_str_radix(value, 16)
            .map_err(|_| format!("Offset.txt line {} has invalid hex", line_index + 1))?;
        if offsets.insert(name.trim().to_string(), parsed).is_some() {
            return Err(format!("Offset.txt duplicates {}", name.trim()));
        }
    }
    Ok(offsets)
}

fn parse_dat_definition(
    table: &str,
    source: &str,
    data: &[u8],
    offsets: &BTreeMap<String, u32>,
) -> Result<(u32, BTreeMap<String, DatFieldMeta>), String> {
    let mut header = BTreeMap::new();
    let mut raw_fields: BTreeMap<u32, BTreeMap<String, String>> = BTreeMap::new();
    let mut section = "";
    for line in source.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line;
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if section == "[HEADER]" {
            header.insert(key.to_string(), value.to_string());
        } else if section == "[FORMAT]" {
            let digits = key
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>();
            let index = digits
                .parse::<u32>()
                .map_err(|_| format!("{table}.def contains an invalid field index"))?;
            let property = &key[digits.len()..];
            raw_fields
                .entry(index)
                .or_default()
                .insert(property.to_string(), value.to_string());
        }
    }
    let entries = header
        .get("InputEntrycount")
        .ok_or_else(|| format!("{table}.def is missing InputEntrycount"))?
        .parse::<u32>()
        .map_err(|_| format!("{table}.def InputEntrycount is invalid"))?;
    let mut fields = BTreeMap::new();
    let mut data_cursor = 0_usize;
    for (index, properties) in raw_fields {
        // `Name=Special Ability Flags:Building,Addon,...` — the text after the
        // first colon is one label per bit, which the DAT wiki shows instead of
        // a raw bitmask.
        let raw_name = properties
            .get("Name")
            .ok_or_else(|| format!("{table}.def field is missing Name"))?;
        let (name, flag_suffix) = match raw_name.split_once(':') {
            Some((name, flags)) => (name.trim().to_string(), flags),
            None => (raw_name.trim().to_string(), ""),
        };
        let flags = parse_flag_labels(flag_suffix);
        let parse = |key: &str, default: u32| -> Result<u32, String> {
            properties
                .get(key)
                .map(|value| value.parse::<u32>())
                .transpose()
                .map_err(|_| format!("{table}.{name} {key} is invalid"))
                .map(|value| value.unwrap_or(default))
        };
        let size = parse("Size", 1)? as u8;
        let var_start = parse("VarStart", 0)?;
        let var_end = parse("VarEnd", entries - 1)?;
        let var_array = parse("VarArray", 1)?;
        let var_index = parse("VarArrayIndex", 1)?;
        let init_var = properties
            .get("InitVar")
            .map(|value| value.parse::<i64>())
            .transpose()
            .map_err(|_| format!("{table}.{name} InitVar is invalid"))?
            .unwrap_or(0);
        if var_start > var_end || var_index == 0 || var_index > var_array {
            return Err(format!(
                "{table}.{name} has an invalid range/array definition"
            ));
        }
        if !matches!(size, 1 | 2 | 4) {
            return Err(format!("{table}.{name} has unsupported width {size}"));
        }
        let count = (var_end - var_start + 1) as usize;
        let component_bytes = size as usize * count;
        let rewind = (var_index as usize - 1) * component_bytes;
        let field_base = data_cursor
            .checked_sub(rewind)
            .ok_or_else(|| format!("{table}.{name} interleaved data offset underflow"))?;
        let mut baseline = Vec::with_capacity(count);
        for local in 0..count {
            let position = field_base
                + local * size as usize * var_array as usize
                + (var_index as usize - 1) * size as usize;
            baseline.push(read_le_value(data, position, size)? as i64 + init_var);
        }
        data_cursor = data_cursor
            .checked_add(component_bytes)
            .ok_or_else(|| format!("{table}.{name} data cursor overflow"))?;
        let offset = offsets
            .get(&format!("{table}_{name}"))
            .copied()
            .ok_or_else(|| format!("Offset.txt is missing {table}_{name}"))?;
        let value_type = properties
            .get("Type")
            .map(|value| value.parse::<u32>())
            .transpose()
            .map_err(|_| format!("{table}.{name} Type is invalid"))?;
        fields.insert(
            name.clone(),
            DatFieldMeta {
                name,
                index,
                size,
                var_start,
                var_end,
                var_array,
                var_index,
                init_var,
                offset,
                value_type,
                flags,
                baseline,
            },
        );
    }
    Ok((entries, fields))
}

/// Split a `.def` flag-label list into one label per bit. Labels are
/// comma-separated, a label holding a comma is `"quoted"`, and `&&` is the
/// Editor form's escape for a literal `&`.
fn parse_flag_labels(suffix: &str) -> Vec<String> {
    if suffix.trim().is_empty() {
        return Vec::new();
    }
    let mut labels = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for character in suffix.chars() {
        match character {
            '"' => quoted = !quoted,
            ',' if !quoted => labels.push(std::mem::take(&mut current)),
            _ => current.push(character),
        }
    }
    labels.push(current);
    labels
        .into_iter()
        .map(|label| label.trim().replace("&&", "&"))
        .collect()
}

fn parse_button_defaults(bytes: &[u8]) -> Result<Vec<ButtonSetDefault>, String> {
    let mut cursor = Cursor::new(bytes);
    let mut values = Vec::new();
    while cursor.position() < bytes.len() as u64 {
        let count = read_u32(&mut cursor)?;
        let address = read_u32(&mut cursor)?;
        let mut buttons = Vec::with_capacity(count as usize);
        for _ in 0..count {
            buttons.push(Button {
                pos: read_u16(&mut cursor)?,
                icon: read_u16(&mut cursor)?,
                condition: read_u32(&mut cursor)?,
                action: read_u32(&mut cursor)?,
                condition_value: read_u16(&mut cursor)?,
                action_value: read_u16(&mut cursor)?,
                enabled_string: read_u16(&mut cursor)?,
                disabled_string: read_u16(&mut cursor)?,
            });
        }
        values.push(ButtonSetDefault {
            count,
            address,
            buttons,
        });
    }
    Ok(values)
}

fn parse_status_defaults(bytes: &[u8]) -> Result<Vec<(i64, i64)>, String> {
    const STATUS: [u32; 9] = [
        4343040, 4344192, 4346240, 4345616, 4344656, 4344560, 4344512, 4348160, 4343072,
    ];
    const DISPLAY: [u32; 9] = [
        4353872, 4356240, 4357264, 4355232, 4355040, 4354656, 4357424, 4353760, 4349664,
    ];
    if bytes.len() % 12 != 0 {
        return Err("statusInfor.dat length is not divisible by 12".to_string());
    }
    let mut values = Vec::with_capacity(bytes.len() / 12);
    for record in bytes.chunks_exact(12) {
        let status = u32::from_le_bytes(record[4..8].try_into().unwrap());
        let display = u32::from_le_bytes(record[8..12].try_into().unwrap());
        values.push((
            STATUS
                .iter()
                .position(|value| *value == status)
                .map(|index| index as i64)
                .unwrap_or(255),
            DISPLAY
                .iter()
                .position(|value| *value == display)
                .map(|index| index as i64)
                .unwrap_or(255),
        ));
    }
    Ok(values)
}

fn parse_requirement_defaults(
    bytes: &[u8],
) -> Result<BTreeMap<String, Vec<RequirementDefault>>, String> {
    const SPECS: [(&str, usize, usize); 5] = [
        ("units", 228, 0x46c),
        ("upgrades", 61, 0x8b4),
        ("techdata", 44, 0xbfc),
        ("Stechdata", 44, 0xd3c),
        ("orders", 189, 0xfec),
    ];
    let mut pointer_offset = 0;
    let mut tables = BTreeMap::new();
    for (table, count, code_base) in SPECS {
        let mut objects = Vec::with_capacity(count);
        for object_id in 0..count {
            let pointer = read_u16_at(bytes, pointer_offset + object_id * 2)? as usize;
            let mut blocks = Vec::new();
            if pointer != 0 {
                let mut position = code_base + pointer * 2;
                let mut sublist_end_pending = false;
                loop {
                    let raw = read_u16_at(bytes, position)?;
                    position += 2;
                    if raw == 0xffff {
                        if sublist_end_pending {
                            blocks.push(RequirementBlock {
                                opcode: 255,
                                value: None,
                            });
                            sublist_end_pending = false;
                            continue;
                        }
                        break;
                    }
                    if raw <= 0xff {
                        blocks.push(RequirementBlock {
                            opcode: 0,
                            value: Some(raw),
                        });
                        continue;
                    }
                    let opcode = raw - 0xff00;
                    sublist_end_pending = matches!(opcode, 31 | 32);
                    let value = if matches!(opcode, 2 | 3 | 4 | 37) {
                        let value = read_u16_at(bytes, position)?;
                        position += 2;
                        Some(value)
                    } else {
                        None
                    };
                    blocks.push(RequirementBlock { opcode, value });
                }
            }
            objects.push(RequirementDefault {
                allocated: pointer != 0,
                blocks,
            });
        }
        pointer_offset += count * 2;
        tables.insert(table.to_string(), objects);
    }
    Ok(tables)
}

fn decode_tbl(bytes: &[u8]) -> Result<Vec<String>, String> {
    if bytes.len() < 2 {
        return Err("stat_txt.tbl is truncated".to_string());
    }
    let count = u16::from_le_bytes([bytes[0], bytes[1]]) as usize;
    if bytes.len() < 2 + count * 2 {
        return Err("stat_txt.tbl offset table is truncated".to_string());
    }
    let mut values = Vec::with_capacity(count);
    for index in 0..count {
        let start = u16::from_le_bytes([bytes[2 + index * 2], bytes[3 + index * 2]]) as usize;
        if start >= bytes.len() {
            return Err(format!("stat_txt.tbl offset {index} is invalid"));
        }
        // Editor's tblReader only treats a NUL after at least two bytes as the terminator,
        // so hotkey strings such as "o<00>Tank Mode" keep their text past the separator.
        let end = bytes[start..]
            .iter()
            .enumerate()
            .position(|(relative, value)| *value == 0 && relative >= 2)
            .map(|relative| start + relative)
            .unwrap_or(bytes.len());
        let slice = &bytes[start..end];
        let value = if slice.ends_with(&[0xe2, 0x80, 0x89]) {
            String::from_utf8(slice[..slice.len() - 3].to_vec())
                .map_err(|error| error.to_string())?
        } else {
            let (decoded, _, had_errors) = EUC_KR.decode(slice);
            if had_errors {
                String::from_utf8_lossy(slice).into_owned()
            } else {
                decoded.into_owned()
            }
        };
        values.push(value);
    }
    Ok(values)
}

fn encode_custom_tbl(
    baseline: &[String],
    overrides: &BTreeMap<u32, TextOverride>,
) -> Result<Vec<u8>, String> {
    let last = overrides
        .keys()
        .max()
        .copied()
        .ok_or_else(|| "custom TBL has no overrides".to_string())? as usize;
    if last >= baseline.len() {
        return Err(format!(
            "custom TBL index {last} exceeds baseline count {}",
            baseline.len()
        ));
    }
    // Editor always dumps the complete table: the game keeps indexing the original
    // 1547-entry header, so a shorter copy leaves the tail reading string bytes as offsets.
    let mut values = baseline.to_vec();
    for (index, change) in overrides {
        values[*index as usize] = change.after.clone();
    }
    let header_bytes = 2 + values.len() * 2;
    let mut encoded_values = Vec::new();
    let mut offsets = Vec::new();
    for value in values {
        let parsed = parse_tbl_escapes(&value)?;
        offsets.push(header_bytes + encoded_values.len());
        let (cp949, _, errors) = EUC_KR.encode(&parsed);
        if errors {
            encoded_values.extend_from_slice(parsed.as_bytes());
            encoded_values.extend_from_slice(&[0xe2, 0x80, 0x89]);
        } else {
            encoded_values.extend_from_slice(&cp949);
        }
        encoded_values.push(0);
    }
    let mut output = Vec::with_capacity(header_bytes + encoded_values.len());
    output.extend_from_slice(&(offsets.len() as u16).to_le_bytes());
    for offset in offsets {
        let offset =
            u16::try_from(offset).map_err(|_| "custom TBL exceeds 65535 bytes".to_string())?;
        output.extend_from_slice(&offset.to_le_bytes());
    }
    output.extend_from_slice(&encoded_values);
    Ok(output)
}

fn parse_tbl_escapes(value: &str) -> Result<String, String> {
    let mut output = String::new();
    let chars: Vec<_> = value.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '<' {
            let mut end = index + 1;
            while end < chars.len() && chars[end] != '>' {
                end += 1;
            }
            if end < chars.len() {
                let token: String = chars[index + 1..end].iter().collect();
                if token.len() == 2 && token.chars().all(|ch| ch.is_ascii_hexdigit()) {
                    let byte = u8::from_str_radix(&token, 16).unwrap();
                    output.push(char::from(byte));
                    index = end + 1;
                    continue;
                }
            }
        }
        output.push(chars[index]);
        index += 1;
    }
    Ok(output)
}

fn relative_path(from: &Path, to: &Path) -> Result<String, String> {
    let from_components: Vec<_> = from.components().collect();
    let to_components: Vec<_> = to.components().collect();
    let mut common = 0;
    while common < from_components.len()
        && common < to_components.len()
        && component_eq(from_components[common], to_components[common])
    {
        common += 1;
    }
    if common == 0 {
        return Ok(path_text(to));
    }
    let mut parts = Vec::new();
    for component in &from_components[common..] {
        if matches!(component, Component::Normal(_)) {
            parts.push("..".to_string());
        }
    }
    for component in &to_components[common..] {
        if let Component::Normal(value) = component {
            parts.push(value.to_string_lossy().into_owned());
        }
    }
    Ok(parts.join("/"))
}

fn component_eq(left: Component<'_>, right: Component<'_>) -> bool {
    left.as_os_str()
        .to_string_lossy()
        .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
}

fn path_to_os(path: &str) -> PathBuf {
    PathBuf::from(path.replace('/', std::path::MAIN_SEPARATOR_STR))
}

fn read_text(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(stringify_io)?;
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
    String::from_utf8(bytes.to_vec()).map_err(|error| error.to_string())
}

fn read_u16(cursor: &mut Cursor<&[u8]>) -> Result<u16, String> {
    let mut bytes = [0_u8; 2];
    cursor
        .read_exact(&mut bytes)
        .map_err(|_| "binary compatibility data is truncated".to_string())?;
    Ok(u16::from_le_bytes(bytes))
}

fn read_u16_at(bytes: &[u8], position: usize) -> Result<u16, String> {
    let slice = bytes
        .get(position..position.saturating_add(2))
        .ok_or_else(|| "binary compatibility data is truncated".to_string())?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32(cursor: &mut Cursor<&[u8]>) -> Result<u32, String> {
    let mut bytes = [0_u8; 4];
    cursor
        .read_exact(&mut bytes)
        .map_err(|_| "btnset.dat is truncated".to_string())?;
    Ok(u32::from_le_bytes(bytes))
}

#[derive(Debug)]
pub(crate) struct CapturedProcess {
    pub(crate) success: bool,
    pub(crate) raw_status: u32,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

fn run_process(
    mut command: Command,
    eds_path: &Path,
    timeout: Duration,
    cancellation: Option<&ProcessCancellation>,
) -> Result<CapturedProcess, String> {
    let cwd = eds_path
        .parent()
        .ok_or_else(|| "EDS 경로에 상위 디렉터리가 없습니다.".to_string())?;
    command.current_dir(cwd);
    let output = run_process_tree(command, timeout, cancellation)?;
    let raw_status = match output.end {
        ProcessEnd::Exited(code) => code,
        ProcessEnd::TimedOut => {
            return Err(format!(
                "euddraft가 {}초 안에 끝나지 않아 프로세스 트리를 종료했습니다.",
                timeout.as_secs()
            ))
        }
        ProcessEnd::Cancelled => {
            return Err("euddraft 실행이 취소되어 프로세스 트리를 종료했습니다.".to_string())
        }
    };
    Ok(CapturedProcess {
        success: raw_status == 0,
        raw_status,
        stdout: output.stdout_lossy(),
        stderr: output.stderr_lossy(),
    })
}

/// Errors and warnings recovered from one euddraft run.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct EuddraftDiagnostics {
    errors: Vec<NativeBuildError>,
    warnings: Vec<NativeBuildError>,
}

/// One `File "<path>", line <n>, in <name>` frame of a Python stack.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TracebackFrame {
    file: String,
    line: u64,
    in_project: bool,
}

/// A traceback block whose exception line has not arrived yet.
struct OpenTraceback<'a> {
    frames: Vec<TracebackFrame>,
    raw: Vec<&'a str>,
    /// Text of the `[Error] …` prefix, used when the block never reaches an exception line.
    fallback: Option<String>,
}

/// `<file>:<line>: <Category>Warning: <text>` (Python `warnings.formatwarning`) or a plain
/// `[Warning] <text>` line.
struct WarningLine {
    site: Option<(String, u64)>,
    category: String,
    text: String,
}

/// Line-oriented state for one euddraft output.
struct OutputParser<'a> {
    project_root: &'a Path,
    eds_dir: &'a Path,
    compiled_modules: BTreeMap<String, Vec<String>>,
    diagnostics: EuddraftDiagnostics,
    /// Stack frames printed without a traceback header (`warn_with_traceback`).
    pending_frames: Vec<TracebackFrame>,
    pending_raw: Vec<&'a str>,
    traceback: Option<OpenTraceback<'a>>,
    /// `[Error] <message>` lines seen before their traceback header; the message may span
    /// several lines because euddraft prints `f"[Error] {err}"` followed by the traceback.
    error_prefix: Vec<&'a str>,
}

/// Parse euddraft's stdout/stderr into structured errors and warnings.
///
/// euddraft prints three diagnostic shapes:
/// - epScript compile errors: `[Error <code>] Module "<name>" Line <n> : <message>`;
/// - a failed run: `[Error] <message> Traceback (most recent call last):` (or a bare
///   traceback header), indented frames, then the `<Type>: <message>` exception line, possibly
///   chained through "During handling of the above exception";
/// - warnings via `warn_with_traceback`: indented stack frames WITHOUT a traceback header,
///   terminated by `<file>:<line>: <Category>Warning: <message>`.
///
/// Every traceback block or warning becomes exactly one diagnostic whose file/line is the
/// innermost frame inside the project, so a warning's stack never counts as errors and a
/// compile error keeps its epScript file and line. Identical warnings merge into one entry
/// with a `count`.
fn parse_euddraft_output(
    stdout: &str,
    stderr: &str,
    project_root: &Path,
    eds_dir: &Path,
) -> EuddraftDiagnostics {
    let combined = joined_output(stdout, stderr);
    let lines: Vec<&str> = combined.lines().collect();
    let mut parser = OutputParser {
        project_root,
        eds_dir,
        compiled_modules: compiled_module_paths(&combined, project_root, eds_dir),
        diagnostics: EuddraftDiagnostics::default(),
        pending_frames: Vec::new(),
        pending_raw: Vec::new(),
        traceback: None,
        error_prefix: Vec::new(),
    };
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        index += 1;
        parser.feed(&lines, &mut index, line);
    }
    parser.finish(&combined)
}

impl<'a> OutputParser<'a> {
    fn feed(&mut self, lines: &[&'a str], index: &mut usize, line: &'a str) {
        let trimmed = line.trim();
        let indented = line.starts_with(char::is_whitespace);
        if let Some(prefix) = split_traceback_header(trimmed, !self.error_prefix.is_empty()) {
            self.close_traceback(None);
            self.flush_pending_frames("a traceback header");
            let mut fallback = std::mem::take(&mut self.error_prefix)
                .iter()
                .map(|line| line.trim())
                .collect::<Vec<_>>();
            if !prefix.is_empty() {
                fallback.push(prefix);
            }
            let fallback = fallback
                .join("\n")
                .strip_prefix("[Error]")
                .map(str::trim)
                .filter(|message| !message.is_empty())
                .map(str::to_string);
            self.traceback = Some(OpenTraceback {
                frames: Vec::new(),
                raw: vec![line],
                fallback,
            });
            return;
        }
        if let Some((file, line_number)) = parse_python_file_line(trimmed) {
            let frame = traceback_frame(&file, line_number, self.project_root, self.eds_dir);
            self.error_prefix.clear();
            match self.traceback.as_mut() {
                Some(open) => {
                    open.frames.push(frame);
                    open.raw.push(line);
                }
                None => {
                    self.pending_frames.push(frame);
                    self.pending_raw.push(line);
                }
            }
            return;
        }
        if let Some(open) = self.traceback.as_mut() {
            if trimmed.is_empty() {
                return;
            }
            if indented {
                // Source excerpt or caret line under a frame.
                open.raw.push(line);
                return;
            }
            // The exception line ends the block; keep its own continuation lines.
            let mut message = trimmed.to_string();
            open.raw.push(line);
            take_continuation(lines, index, &mut message, &mut open.raw);
            self.close_traceback(Some(message));
            return;
        }
        if let Some((module, line_number, message)) = parse_compile_error_line(trimmed) {
            self.flush_pending_frames("an epScript compile error");
            self.error_prefix.clear();
            let (file, message) = match self.compiled_modules.get(&module) {
                Some(paths) if paths.len() == 1 => (paths[0].clone(), message),
                Some(paths) => (
                    paths[0].clone(),
                    format!("{message} (module name matches: {})", paths.join(", ")),
                ),
                None => (format!("{module}.eps"), message),
            };
            self.diagnostics.errors.push(NativeBuildError {
                source: "epScript".to_string(),
                file,
                line: line_number,
                message,
                raw: bound_text(trimmed, DIAGNOSTIC_RAW_LIMIT),
                count: 1,
            });
            return;
        }
        if let Some(warning) = parse_warning_line(trimmed) {
            self.error_prefix.clear();
            let mut message = format!("{}: {}", warning.category, warning.text);
            let mut raw = std::mem::take(&mut self.pending_raw);
            raw.push(line);
            if warning.site.is_some() {
                // Only Python `formatwarning` output continues onto further lines; a plain
                // `[Warning]` line is complete in itself.
                take_continuation(lines, index, &mut message, &mut raw);
            }
            let frames = std::mem::take(&mut self.pending_frames);
            let (file, line_number) = innermost_project_frame(&frames)
                .map(|frame| (frame.file.clone(), frame.line))
                .or_else(|| {
                    warning.site.map(|(file, line_number)| {
                        (
                            normalize_traceback_file(&file, self.project_root, self.eds_dir),
                            line_number,
                        )
                    })
                })
                .unwrap_or_default();
            self.diagnostics.warnings.push(NativeBuildError {
                source: "euddraft".to_string(),
                file,
                line: line_number,
                message,
                raw: bound_text(&raw.join("\n"), DIAGNOSTIC_RAW_LIMIT),
                count: 1,
            });
            return;
        }
        if !indented && trimmed.starts_with("[Error]") {
            self.flush_pending_frames("an [Error] line");
            self.error_prefix = vec![line];
            return;
        }
        if indented || trimmed.is_empty() {
            return;
        }
        if !self.error_prefix.is_empty() {
            // Continuation of a multi-line `[Error] {err}` message before its header.
            self.error_prefix.push(line);
            return;
        }
        self.flush_pending_frames(trimmed);
    }

    /// Frames that end without a warning or exception line stay errors: the output is not a
    /// shape this parser vouches for.
    fn flush_pending_frames(&mut self, before: &str) {
        if self.pending_frames.is_empty() {
            return;
        }
        let frames = std::mem::take(&mut self.pending_frames);
        let raw = std::mem::take(&mut self.pending_raw);
        self.diagnostics.errors.push(traceback_error(
            frames,
            raw,
            Some(format!("unterminated euddraft stack before: {before}")),
            self.project_root,
            self.eds_dir,
        ));
    }

    fn close_traceback(&mut self, message: Option<String>) {
        let Some(open) = self.traceback.take() else {
            return;
        };
        let message = message.or(open.fallback);
        let has_project_frame = open.frames.iter().any(|frame| frame.in_project);
        let error = traceback_error(
            open.frames,
            open.raw,
            message,
            self.project_root,
            self.eds_dir,
        );
        if is_launcher_stdin_eof(&error, has_project_frame) {
            // The bounded launcher runs euddraft with a closed stdin; euddraft's own
            // `input("Press Enter to continue...")` after a failed run then raises EOFError.
            // That is the launcher's artifact, never a project error, so it must not inflate
            // the error count the model repairs against.
            self.diagnostics.warnings.push(NativeBuildError {
                message: format!(
                    "{} — euddraft's post-failure console prompt under the launcher's closed stdin, not a project error",
                    error.message
                ),
                ..error
            });
        } else {
            self.diagnostics.errors.push(error);
        }
    }

    fn finish(mut self, combined: &str) -> EuddraftDiagnostics {
        self.close_traceback(None);
        self.flush_pending_frames("end of output");
        if !self.error_prefix.is_empty() {
            let raw = std::mem::take(&mut self.error_prefix);
            let message = raw
                .iter()
                .map(|line| line.trim())
                .collect::<Vec<_>>()
                .join("\n");
            self.diagnostics.errors.push(NativeBuildError {
                source: "euddraft".to_string(),
                file: String::new(),
                line: 0,
                message: bound_text(
                    message
                        .strip_prefix("[Error]")
                        .map(str::trim)
                        .unwrap_or(&message),
                    DIAGNOSTIC_RAW_LIMIT,
                ),
                raw: bound_text(&raw.join("\n"), DIAGNOSTIC_RAW_LIMIT),
                count: 1,
            });
        }
        if self.diagnostics.errors.is_empty() && combined.contains(TRACEBACK_MARKER) {
            let message = combined
                .lines()
                .rev()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("euddraft traceback")
                .trim()
                .to_string();
            self.diagnostics.errors.push(NativeBuildError {
                source: "euddraft".to_string(),
                file: String::new(),
                line: 0,
                message,
                raw: bound_text(combined, DIAGNOSTIC_RAW_LIMIT),
                count: 1,
            });
        }
        self.diagnostics.warnings = merge_identical(std::mem::take(&mut self.diagnostics.warnings));
        self.diagnostics
    }
}

/// Append the non-indented, non-marker lines that continue an exception or warning message.
fn take_continuation<'a>(
    lines: &[&'a str],
    index: &mut usize,
    message: &mut String,
    raw: &mut Vec<&'a str>,
) {
    while *index < lines.len() {
        let next = lines[*index];
        let next_trimmed = next.trim();
        if next_trimmed.is_empty()
            || next.starts_with(char::is_whitespace)
            || is_diagnostic_marker(next_trimmed)
        {
            break;
        }
        message.push('\n');
        message.push_str(next_trimmed);
        raw.push(next);
        *index += 1;
    }
}

/// Collapse diagnostics with the same file, line, and message into one entry with a count.
fn merge_identical(entries: Vec<NativeBuildError>) -> Vec<NativeBuildError> {
    let mut merged: Vec<NativeBuildError> = Vec::new();
    for entry in entries {
        match merged.iter_mut().find(|existing| {
            existing.file == entry.file
                && existing.line == entry.line
                && existing.message == entry.message
        }) {
            Some(existing) => existing.count += entry.count,
            None => merged.push(entry),
        }
    }
    merged
}

fn is_launcher_stdin_eof(error: &NativeBuildError, has_project_frame: bool) -> bool {
    !has_project_frame
        && error.message.starts_with("EOFError:")
        && error.file.ends_with("euddraft.py")
}

/// `[epScript] Compiling "<path>"...` lines map a compile-error module name (the file stem)
/// to every source that produced it.
fn compiled_module_paths(
    combined: &str,
    project_root: &Path,
    eds_dir: &Path,
) -> BTreeMap<String, Vec<String>> {
    let marker = "[epScript] Compiling \"";
    let mut modules: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for line in combined.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix(marker) else {
            continue;
        };
        let Some(end) = rest.find('"') else {
            continue;
        };
        let path = &rest[..end];
        let stem = Path::new(path)
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        if stem.is_empty() {
            continue;
        }
        let normalized = normalize_traceback_file(path, project_root, eds_dir);
        let paths = modules.entry(stem).or_default();
        if !paths.contains(&normalized) {
            paths.push(normalized);
        }
    }
    modules
}

/// Recognize a traceback header line and return the text before it. A bare header or one
/// prefixed by `[Error] …` always counts; an arbitrary prefix counts only while a multi-line
/// `[Error]` message is open, because euddraft prints the header at the end of that message.
fn split_traceback_header(trimmed: &str, error_prefix_open: bool) -> Option<&str> {
    let start = trimmed.find(TRACEBACK_MARKER)?;
    if !trimmed[start + TRACEBACK_MARKER.len()..].trim().is_empty() {
        return None;
    }
    let prefix = trimmed[..start].trim();
    (prefix.is_empty() || prefix.starts_with("[Error]") || error_prefix_open).then_some(prefix)
}

/// `[Error <code>] Module "<name>" Line <n> : <message>` from the epScript compiler.
fn parse_compile_error_line(trimmed: &str) -> Option<(String, u64, String)> {
    let rest = trimmed.strip_prefix("[Error ")?;
    let close = rest.find("] Module \"")?;
    let code = &rest[..close];
    if code.is_empty() || !code.chars().all(|ch| ch.is_ascii_digit() || ch == '-') {
        return None;
    }
    let rest = &rest[close + "] Module \"".len()..];
    let end = rest.find('"')?;
    let module = rest[..end].to_string();
    let rest = rest[end + 1..].trim_start().strip_prefix("Line ")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let line_number = digits.parse().ok()?;
    let message = rest[digits.len()..]
        .trim()
        .strip_prefix(':')
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    Some((module, line_number, message))
}

fn parse_warning_line(trimmed: &str) -> Option<WarningLine> {
    if let Some(text) = trimmed.strip_prefix("[Warning]") {
        return Some(WarningLine {
            site: None,
            category: "Warning".to_string(),
            text: text.trim().to_string(),
        });
    }
    let marker = "Warning: ";
    let mut search_from = 0;
    while let Some(found) = trimmed[search_from..].find(marker) {
        let position = search_from + found;
        let prefix = &trimmed[..position];
        if let Some((site, category_start)) = prefix.rsplit_once(": ") {
            let is_category = !category_start.is_empty()
                && category_start
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '.');
            if is_category {
                if let Some((file, digits)) = site.rsplit_once(':') {
                    if let Ok(line_number) = digits.parse::<u64>() {
                        return Some(WarningLine {
                            site: Some((file.to_string(), line_number)),
                            category: format!("{category_start}Warning"),
                            text: trimmed[position + marker.len()..].trim().to_string(),
                        });
                    }
                }
            }
        }
        search_from = position + marker.len();
    }
    None
}

fn is_diagnostic_marker(trimmed: &str) -> bool {
    trimmed.starts_with("[Error")
        || trimmed.starts_with("[Warning]")
        || trimmed.contains(TRACEBACK_MARKER)
        || trimmed.starts_with("During handling of the above exception")
        || trimmed.starts_with("The above exception was the direct cause")
        || trimmed.starts_with("File \"")
        || parse_warning_line(trimmed).is_some()
}

/// A Python stack frame line: `File "<path>", line <n>, in <name>` (already trimmed).
fn parse_python_file_line(trimmed: &str) -> Option<(String, u64)> {
    let rest = trimmed.strip_prefix("File \"")?;
    let end = rest.find('"')?;
    let file = rest[..end].to_string();
    let after = &rest[end + 1..];
    let line_marker = "line ";
    let line_start = after.find(line_marker)? + line_marker.len();
    let digits: String = after[line_start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    let line_number = digits.parse().ok()?;
    Some((file, line_number))
}

fn traceback_frame(file: &str, line: u64, project_root: &Path, eds_dir: &Path) -> TracebackFrame {
    let (file, in_project) = resolve_traceback_file(file, project_root, eds_dir);
    TracebackFrame {
        file,
        line,
        in_project,
    }
}

fn innermost_project_frame(frames: &[TracebackFrame]) -> Option<&TracebackFrame> {
    frames.iter().rev().find(|frame| frame.in_project)
}

fn traceback_error(
    frames: Vec<TracebackFrame>,
    raw: Vec<&str>,
    message: Option<String>,
    project_root: &Path,
    eds_dir: &Path,
) -> NativeBuildError {
    let message = message.unwrap_or_else(|| "euddraft traceback".to_string());
    let (file, line) = innermost_project_frame(&frames)
        .or_else(|| frames.last())
        .map(|frame| (frame.file.clone(), frame.line))
        .or_else(|| {
            // `EPError:  - Compiled failed for <path>` carries no frames; keep its file.
            message
                .lines()
                .find_map(|line| line.split("Compiled failed for ").nth(1))
                .map(|path| {
                    (
                        normalize_traceback_file(path.trim(), project_root, eds_dir),
                        0,
                    )
                })
        })
        .unwrap_or_default();
    NativeBuildError {
        source: "euddraft".to_string(),
        file,
        line,
        message,
        raw: bound_text(&raw.join("\n"), DIAGNOSTIC_RAW_LIMIT),
        count: 1,
    }
}

fn normalize_traceback_file(file: &str, project_root: &Path, eds_dir: &Path) -> String {
    resolve_traceback_file(file, project_root, eds_dir).0
}

/// Project-relative `/` path when `file` resolves under the project root (and `true`), else
/// the original text (and `false`). Relative traceback paths are relative to the EDS
/// directory because euddraft changes cwd there. A file that no longer exists is still
/// classified by text, with Windows verbatim (`\\?\`) prefixes ignored on both sides.
fn resolve_traceback_file(file: &str, project_root: &Path, eds_dir: &Path) -> (String, bool) {
    let original = Path::new(file);
    let resolved = if original.is_relative() {
        fs::canonicalize(eds_dir.join(original)).ok()
    } else {
        fs::canonicalize(original).ok()
    };
    let canonical_root = fs::canonicalize(project_root).ok();
    let path = without_verbatim_prefix(resolved.as_deref().unwrap_or(original));
    let root = without_verbatim_prefix(canonical_root.as_deref().unwrap_or(project_root));
    if let Ok(relative) = path.strip_prefix(&root) {
        let components = relative.components().collect::<Vec<_>>();
        if !components.is_empty()
            && components
                .iter()
                .all(|component| matches!(component, Component::Normal(_)))
        {
            let relative = components
                .iter()
                .filter_map(|component| match component {
                    Component::Normal(value) => Some(value.to_string_lossy().into_owned()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("/");
            return (relative, true);
        }
    }
    let root_components = root.components().collect::<Vec<_>>();
    let path_components = path.components().collect::<Vec<_>>();
    if path_components.len() >= root_components.len()
        && root_components
            .iter()
            .zip(&path_components)
            .all(|(root, path)| {
                root.as_os_str()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&path.as_os_str().to_string_lossy())
            })
    {
        let relative = path_components[root_components.len()..]
            .iter()
            .filter_map(|component| match component {
                Component::Normal(value) => Some(value.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("/");
        if !relative.is_empty() {
            return (relative, true);
        }
    }
    (file.to_string(), false)
}

/// `\\?\C:\x` → `C:\x` and `\\?\UNC\server\share` → `\\server\share`, so a canonical root and
/// a plain absolute path compare component by component.
fn without_verbatim_prefix(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        path.to_path_buf()
    }
}

/// Keep the head and tail of `text` within `limit` characters, marking what was elided.
pub fn bound_text(text: &str, limit: usize) -> String {
    let total = text.chars().count();
    if total <= limit {
        return text.to_string();
    }
    let head_len = limit * 3 / 4;
    let tail_len = limit - head_len;
    let head: String = text.chars().take(head_len).collect();
    let tail: String = text.chars().skip(total - tail_len).collect();
    format!(
        "{head}\n[… {} characters elided …]\n{tail}",
        total - head_len - tail_len
    )
}

fn read_le_value(bytes: &[u8], position: usize, size: u8) -> Result<u32, String> {
    let end = position
        .checked_add(size as usize)
        .ok_or_else(|| "DAT value offset overflow".to_string())?;
    let slice = bytes
        .get(position..end)
        .ok_or_else(|| "DAT file is truncated".to_string())?;
    Ok(match size {
        1 => slice[0] as u32,
        2 => u16::from_le_bytes([slice[0], slice[1]]) as u32,
        4 => u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]),
        _ => return Err(format!("unsupported DAT width {size}")),
    })
}

fn joined_output(stdout: &str, stderr: &str) -> String {
    match (stdout.trim(), stderr.trim()) {
        ("", "") => String::new(),
        (stdout, "") => stdout.to_string(),
        ("", stderr) => stderr.to_string(),
        (stdout, stderr) => format!("{stdout}\n{stderr}"),
    }
}

fn modified_time(path: &Path) -> Result<Option<SystemTime>, String> {
    match fs::metadata(path).and_then(|metadata| metadata.modified()) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn is_fresh_output(before: Option<SystemTime>, after: Option<SystemTime>) -> bool {
    match (before, after) {
        (None, Some(_)) => true,
        (Some(before), Some(after)) => after > before,
        _ => false,
    }
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn stringify_io(error: std::io::Error) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_project::{
        DatScalar, DatTarget, EdsPlugin, NativeDatChange, NativeDatPatch, NativeDatState,
        ProjectManifest, ProjectSettings,
    };
    use std::collections::HashMap;

    fn compat_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/eud-editor-compat")
    }

    fn root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "eud-agent-native-build-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("maps")).unwrap();
        fs::write(root.join("maps/source.scx"), b"map").unwrap();
        root
    }

    fn project(tag: &str) -> (PathBuf, NativeProject) {
        let root = root(tag);
        let manifest = ProjectManifest {
            schema_version: crate::native_project::PROJECT_SCHEMA_VERSION,
            name: "Build Demo".to_string(),
            source_map: "maps/source.scx".to_string(),
            output_map: "build/output.scx".to_string(),
            main_file: "src/main.eps".to_string(),
            settings: ProjectSettings::default(),
            plugins: vec![EdsPlugin {
                section: "eudTurbo".to_string(),
                entries: Vec::new(),
                raw_text: None,
            }],
            python_entrypoints: Vec::new(),
            python_dependencies: Vec::new(),
            python_lock: None,
            editor_compatibility: None,
        };
        let created = NativeProject::create(&root, manifest).unwrap();
        created
            .write_source("src/main.eps", "function onPluginStart() {}\n")
            .unwrap();
        let opened = NativeProject::open(&root).unwrap();
        (root, opened)
    }

    #[test]
    fn catalog_matches_editor_field_metadata() {
        let catalog = DatCatalog::load(&compat_root()).unwrap();
        let hp = catalog.field("units", "Hit Points").unwrap();
        assert_eq!(hp.offset, 0x662350);
        assert_eq!(hp.size, 4);
        assert_eq!(hp.var_end - hp.var_start + 1, 228);
        let placement_height = catalog
            .field("units", "StarEdit Placement Box Height")
            .unwrap();
        assert_eq!(placement_height.var_array, 2);
        assert_eq!(placement_height.var_index, 2);
    }

    #[test]
    fn order_requirement_pointers_skip_the_order_id_word_like_editor() {
        // Editor's WriteRequireData writes the order id before recording StartPos, and
        // the stock require.dat points every order at the word after its id. A pointer
        // at the id word makes the game read that id as a "must own unit" opcode.
        let catalog = DatCatalog::load(&compat_root()).unwrap();
        let mut dat = NativeDatState::default();
        dat.requirements.tables.insert(
            "orders".to_string(),
            BTreeMap::from([(
                6_u32,
                crate::native_project::TextOverride {
                    before: catalog.requirement_payload("orders", 6).unwrap(),
                    after: "2".to_string(),
                },
            )]),
        );
        let artifacts = generate_requirement_artifacts(&dat, &catalog)
            .unwrap()
            .unwrap();
        let orders_base = 1096 + 840 + 320 + 688;
        let orders = &artifacts.bytes[orders_base..orders_base + 1316];
        let word = |index: usize| u16::from_le_bytes([orders[index * 2], orders[index * 2 + 1]]);
        for (order_id, pointer) in artifacts.pointers["orders"].iter().enumerate() {
            if *pointer == 0 {
                continue;
            }
            assert_eq!(
                word(*pointer as usize - 1),
                order_id as u16,
                "order {order_id} pointer {pointer} must follow its id word"
            );
        }
        assert_eq!(artifacts.pointers["orders"][0], 2);
        assert_eq!(word(artifacts.pointers["orders"][6] as usize), 0xffff);
        assert_eq!(artifacts.pointers["units"][0], 1);
    }

    #[test]
    fn standard_dat_plugin_matches_editor_address_math() {
        let (root, mut project) = project("dat");
        let target = DatTarget::Dat {
            dat: "units".to_string(),
            object_id: 15,
            field: "Hit Points".to_string(),
        };
        project
            .apply_dat_patch(
                &NativeDatPatch {
                    changes: vec![NativeDatChange::Dat {
                        dat: "units".to_string(),
                        object_id: 15,
                        field: "Hit Points".to_string(),
                        before: 10240,
                        after: 20480,
                    }],
                },
                &BTreeMap::from([(target, DatScalar::Number(10240))]),
            )
            .unwrap();
        let artifacts = generate_native_build(&project, &compat_root()).unwrap();
        let generated = fs::read_to_string(artifacts.data_editor.unwrap()).unwrap();
        assert!(generated.contains("SetMemory(0x66238C, Add, 10240)"));
        let eds = fs::read_to_string(artifacts.eds_path).unwrap();
        assert!(eds.contains("[DataEditor.py]"));
        assert!(eds.contains("[../../src/main.eps]"));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn button_csv_becomes_editor_compatible_little_endian_bytes() {
        let buttons = parse_button_csv("1,228,4358864,4342848,0,0,664,0").unwrap();
        let bytes = button_bytes(&buttons);
        assert_eq!(bytes.len(), 20);
        assert_eq!(&bytes[..4], &[1, 0, 228, 0]);
    }

    #[test]
    fn custom_tbl_round_trips_cp949_and_utf8_fallback() {
        let baseline =
            decode_tbl(&fs::read(compat_root().join("Tbls/stat_txt.tbl")).unwrap()).unwrap();
        let overrides = BTreeMap::from([(
            0,
            TextOverride {
                before: baseline[0].clone(),
                after: "정예 해병 <03>".to_string(),
            },
        )]);
        let encoded = encode_custom_tbl(&baseline, &overrides).unwrap();
        let decoded = decode_tbl(&encoded).unwrap();
        assert!(decoded[0].starts_with("정예 해병"));
        assert_eq!(decoded.len(), baseline.len());
        assert_eq!(decoded[1..], baseline[1..]);
        // Hotkey strings separate the key from the text with a NUL in the second byte.
        assert_eq!(baseline[338], "o\u{0}Tank M\u{3}o\u{1}de");
        assert_eq!(baseline[0], "Terran Marine");
    }
    fn synthetic_artifacts(root: &Path) -> NativeBuildArtifacts {
        NativeBuildArtifacts {
            build_dir: path_text(&root.join("build/euddraft")),
            wireframe_editor: None,
            requirement_file: None,
            eds_path: path_text(&root.join("build/euddraft/test.eds")),
            output_map: path_text(&root.join("build/output.scx")),
            data_editor: None,
            extra_data_editor: None,
            custom_tbl: None,
            python_path_bootstrap: None,
        }
    }

    #[test]
    fn build_result_rejects_traceback_even_with_zero_exit_and_fresh_output() {
        let root = std::env::temp_dir().join("eud-agent-build-result-classification");
        let eds_dir = root.join("build/euddraft");
        fs::create_dir_all(&eds_dir).unwrap();
        let result = assemble_build_result(
            CapturedProcess {
                success: true,
                raw_status: 0,
                stdout: String::new(),
                stderr: format!(
                    "Traceback (most recent call last):\n  File \"{}\", line 7, in <module>\nRuntimeError: boom",
                    root.join("src/direct.py").display()
                ),
            },
            true,
            synthetic_artifacts(&root),
            &root,
            &eds_dir,
            Ok(String::new()),
        );
        assert!(!result.ok);
        assert!(!result.errors.is_empty());
        assert_eq!(result.raw_status, 0);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn build_result_rejects_zero_exit_without_fresh_output() {
        let root = std::env::temp_dir().join("eud-agent-build-result-stale-output");
        let eds_dir = root.join("build/euddraft");
        fs::create_dir_all(&eds_dir).unwrap();
        let result = assemble_build_result(
            CapturedProcess {
                success: true,
                raw_status: 0,
                stdout: String::new(),
                stderr: String::new(),
            },
            false,
            synthetic_artifacts(&root),
            &root,
            &eds_dir,
            Ok(String::new()),
        );
        assert!(!result.ok);
        assert!(result.errors[0].message.contains("fresh output"));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn frozen_euddraft_environment_drops_ambient_python_configuration() {
        let mut command = Command::new("euddraft.exe");
        command
            .env("PYTHONPATH", "hostile")
            .env("PYTHONHOME", "hostile")
            .env("PYTHONUSERBASE", "hostile");
        configure_frozen_euddraft_environment(&mut command);
        let configured = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_os_string()),
                )
            })
            .collect::<HashMap<_, _>>();
        assert!(!configured.contains_key("PYTHONPATH"));
        assert!(!configured.contains_key("PYTHONHOME"));
        assert!(!configured.contains_key("PYTHONUSERBASE"));
        assert_eq!(
            configured
                .get("PYTHONNOUSERSITE")
                .and_then(Option::as_ref)
                .map(|value| value.as_os_str()),
            Some(std::ffi::OsStr::new("1"))
        );
    }

    #[test]
    fn traceback_files_under_project_are_project_relative_and_external_paths_survive() {
        let root = std::env::temp_dir().join("eud-agent-traceback-root");
        let internal = root.join("src/feature.py");
        let eds_dir = root.join("build/euddraft");
        fs::create_dir_all(internal.parent().unwrap()).unwrap();
        fs::create_dir_all(&eds_dir).unwrap();
        fs::write(&internal, b"pass\n").unwrap();
        let external = std::env::temp_dir().join("external-package/module.py");
        let stderr = format!(
            "Traceback (most recent call last):\n  File \"{}\", line 17, in feature\n  File \"../../src/feature.py\", line 18, in feature\n  File \"{}\", line 4, in helper\nRuntimeError: boom\n",
            internal.display(),
            external.display(),
        );
        let diagnostics = parse_euddraft_output("", &stderr, &root, &eds_dir);
        // One traceback is one error, located at the innermost project frame.
        assert_eq!(diagnostics.errors.len(), 1);
        assert!(diagnostics.warnings.is_empty());
        let error = &diagnostics.errors[0];
        assert_eq!(error.file, "src/feature.py");
        assert_eq!(error.line, 18);
        assert_eq!(error.message, "RuntimeError: boom");
        assert!(error.raw.contains(external.to_string_lossy().as_ref()));
        fs::remove_dir_all(root).ok();
    }

    const NULL_TILE_WARNING_STACK: &str = "  File \"src/freeze_core/initscripts/__startup__.py\", line 147, in run\n  File \"D:\\a\\euddraft\\euddraft\\applyeuddraft.py\", line 227, in applyEUDDraft\n  File \"D:\\a\\euddraft\\euddraft\\.venv\\Lib\\site-packages\\eudplib\\core\\mapdata\\fixmapdata.py\", line 98, in _fix_mtxm_0_0_null\n  File \"D:\\a\\euddraft\\euddraft\\.venv\\Lib\\site-packages\\eudplib\\utils\\eperror.py\", line 41, in ep_warn\n  File \"D:\\a\\euddraft\\euddraft\\applyeuddraft.py\", line 114, in warn_with_traceback\nD:\\a\\euddraft\\euddraft\\.venv\\Lib\\site-packages\\eudplib\\utils\\eperror.py:41: EPWarning: [Warning] Input map has 0000.00 null tiles\nReplaced them to 0000.01, because they cause desync.\n";

    #[test]
    fn warning_stacks_are_warnings_and_a_fresh_zero_exit_build_stays_ok() {
        let root = std::env::temp_dir().join("eud-agent-build-warning-stack");
        let eds_dir = root.join("build/euddraft");
        fs::create_dir_all(&eds_dir).unwrap();
        let stdout = format!(
            "Saving to ../[EUD]rpg.scx...\nNull tiles at: {} - Allocating objects..\nOutput scenario.chk : 1.794MB\n",
            (0..65_056).map(|i| format!("({}, {})", i % 256, i / 256)).collect::<Vec<_>>().join(", ")
        );
        let stderr = format!("{NULL_TILE_WARNING_STACK}{NULL_TILE_WARNING_STACK}");
        let result = assemble_build_result(
            CapturedProcess {
                success: true,
                raw_status: 0,
                stdout,
                stderr,
            },
            true,
            synthetic_artifacts(&root),
            &root,
            &eds_dir,
            Ok(String::new()),
        );
        assert!(result.ok, "{:?}", result.errors);
        assert!(result.errors.is_empty());
        // Two identical stacks merge into one warning that counts both.
        assert_eq!(result.warnings.len(), 1);
        let warning = &result.warnings[0];
        assert_eq!(warning.count, 2);
        assert_eq!(warning.source, "euddraft");
        assert_eq!(
            warning.message,
            "EPWarning: [Warning] Input map has 0000.00 null tiles\nReplaced them to 0000.01, because they cause desync."
        );
        for warning in &result.warnings {
            assert!(warning
                .raw
                .trim_start()
                .starts_with("File \"src/freeze_core"));
        }
        assert!(warning.raw.chars().count() <= DIAGNOSTIC_RAW_LIMIT + 64);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn epscript_compile_errors_keep_their_source_file_and_line() {
        let root = std::env::temp_dir().join("eud-agent-build-compile-errors");
        let eds_dir = root.join("build/euddraft");
        fs::create_dir_all(&eds_dir).unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.eps"), b"function onPluginStart() {\n").unwrap();
        let stdout = "Loading plugin ../../src/main.eps...\n[epScript] Compiling \"..\\..\\src\\main.eps\"...\n==========================================\nPress Enter to continue...\n";
        let stderr = format!(
            "[Error -2] Module \"main\" Line 2 : General syntax error\n[Error 6298] Module \"main\" Line 4 : Block not terminated properly.\n[Error] Error loading plugin \"../../src/main.eps\" Traceback (most recent call last):\neudplib.utils.eperror.EPError:  - Compiled failed for {}\n\nDuring handling of the above exception, another exception occurred:\n\nTraceback (most recent call last):\n  File \"D:\\a\\euddraft\\euddraft\\applyeuddraft.py\", line 209, in applyEUDDraft\n  File \"D:\\a\\euddraft\\euddraft\\pluginLoader.py\", line 225, in loadPluginsFromConfig\nRuntimeError: Error loading plugin \"../../src/main.eps\"\n",
            root.join("src/main.eps").display()
        );
        let diagnostics = parse_euddraft_output(stdout, &stderr, &root, &eds_dir);
        assert!(diagnostics.warnings.is_empty());
        let compile: Vec<_> = diagnostics
            .errors
            .iter()
            .filter(|error| error.source == "epScript")
            .collect();
        assert_eq!(compile.len(), 2);
        assert_eq!(compile[0].file, "src/main.eps");
        assert_eq!(compile[0].line, 2);
        assert_eq!(compile[0].message, "General syntax error");
        assert_eq!(compile[1].line, 4);
        assert_eq!(compile[1].message, "Block not terminated properly.");
        let tracebacks: Vec<_> = diagnostics
            .errors
            .iter()
            .filter(|error| error.source == "euddraft")
            .collect();
        assert_eq!(tracebacks.len(), 2);
        assert!(tracebacks[0]
            .message
            .starts_with("eudplib.utils.eperror.EPError:  - Compiled failed for"));
        assert_eq!(tracebacks[0].file, "src/main.eps");
        assert_eq!(
            tracebacks[1].message,
            "RuntimeError: Error loading plugin \"../../src/main.eps\""
        );
        assert_eq!(tracebacks[1].line, 225);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn epscript_runtime_errors_point_at_the_innermost_eps_frame() {
        let root = std::env::temp_dir().join("eud-agent-build-runtime-error");
        let eds_dir = root.join("build/euddraft");
        fs::create_dir_all(&eds_dir).unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("src/zones.eps"),
            b"const A = $L(\"Spawn Point\");\n",
        )
        .unwrap();
        let stderr = format!(
            "[Error] Cannot encode string Spawn Point as location. Traceback (most recent call last):\n  File \"D:\\a\\euddraft\\euddraft\\pluginLoader.py\", line 200, in loadPluginsFromConfig\n  File \"{}\", line 4, in <module>\n  File \"D:\\a\\euddraft\\euddraft\\.venv\\Lib\\site-packages\\eudplib\\core\\mapdata\\stringmap.py\", line 90, in EncodeLocation\neudplib.utils.eperror.EPError: Cannot encode string Spawn Point as location.\n",
            root.join("src/zones.eps").display()
        );
        let diagnostics = parse_euddraft_output("", &stderr, &root, &eds_dir);
        assert_eq!(diagnostics.errors.len(), 1);
        let error = &diagnostics.errors[0];
        assert_eq!(error.file, "src/zones.eps");
        assert_eq!(error.line, 4);
        assert_eq!(
            error.message,
            "eudplib.utils.eperror.EPError: Cannot encode string Spawn Point as location."
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn output_excerpt_cuts_long_lines_and_keeps_head_and_tail() {
        let short = output_excerpt("hello\nworld", "");
        assert_eq!(
            short,
            "[stdout: 2 lines, 11 chars]\nhello\nworld\n[stderr: 0 lines, 0 chars]\n"
        );

        let giant_line = "x".repeat(700_000);
        let many_lines = (0..2_000)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let excerpt = output_excerpt(&format!("head\n{giant_line}\n{many_lines}\ntail"), "warn");
        assert!(excerpt.chars().count() < 2 * EXCERPT_STREAM_LIMIT);
        assert!(excerpt.contains("head\n"));
        assert!(excerpt.contains("[… line cut, 699600 more chars]"));
        assert!(excerpt
            .contains("lines elided; full text in build/euddraft/build.log via build_log_read"));
        assert!(excerpt.ends_with("line 1999\ntail\n[stderr: 1 lines, 4 chars]\nwarn\n"));
    }

    #[test]
    fn warnings_never_rescue_a_nonzero_exit_and_plain_shapes_parse() {
        let root = std::env::temp_dir().join("eud-agent-build-warning-nonzero");
        let eds_dir = root.join("build/euddraft");
        fs::create_dir_all(&eds_dir).unwrap();
        // Only warnings, exit 1, no fresh output: the build fails with a synthetic error.
        let result = assemble_build_result(
            CapturedProcess {
                success: false,
                raw_status: 1,
                stdout: String::new(),
                stderr: NULL_TILE_WARNING_STACK.to_string(),
            },
            false,
            synthetic_artifacts(&root),
            &root,
            &eds_dir,
            Ok(String::new()),
        );
        assert!(!result.ok);
        assert_eq!(result.errors.len(), 1);
        assert!(result.errors[0].message.contains("0x00000001"));
        assert_eq!(result.warnings.len(), 1);

        // Empty output with exit 0 and no fresh map is still a failure, with nothing parsed.
        let empty = parse_euddraft_output("", "", &root, &eds_dir);
        assert_eq!(empty, EuddraftDiagnostics::default());

        // A plain `[Warning]` line, CRLF endings, and a failed log write are all reported.
        let result = assemble_build_result(
            CapturedProcess {
                success: true,
                raw_status: 0,
                stdout:
                    "Loading plugin\r\n[Warning] Unused manifest keys in [main]: foo\r\nDone\r\n"
                        .to_string(),
                stderr: String::new(),
            },
            true,
            synthetic_artifacts(&root),
            &root,
            &eds_dir,
            Err("disk full".to_string()),
        );
        assert!(result.ok);
        assert_eq!(result.warnings.len(), 2);
        assert_eq!(
            result.warnings[0].message,
            "Warning: Unused manifest keys in [main]: foo"
        );
        assert_eq!(result.warnings[1].source, "eud-agent");
        assert_eq!(result.warnings[1].message, "disk full");
        assert_eq!(result.log_path, "");
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn multi_line_error_prefix_and_launcher_eof_are_classified() {
        let root = std::env::temp_dir().join("eud-agent-build-multiline-error");
        let eds_dir = root.join("build/euddraft");
        fs::create_dir_all(&eds_dir).unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.eps"), b"// main\n").unwrap();
        // euddraft prints `[Error] {err}` where err spans two lines, then the header on the
        // second line, then the traceback; finally its `input()` fails on the closed stdin.
        let stderr = format!(
            "[Error] first line of the message\nsecond line of the message Traceback (most recent call last):\n  File \"{}\", line 9, in <module>\neudplib.utils.eperror.EPError: first line of the message\nsecond line of the message\nTraceback (most recent call last):\n  File \"D:\\a\\euddraft\\euddraft\\euddraft.py\", line 138, in <module>\nEOFError: EOF when reading a line\n",
            root.join("src/main.eps").display()
        );
        let diagnostics = parse_euddraft_output("", &stderr, &root, &eds_dir);
        assert_eq!(diagnostics.errors.len(), 1, "{:?}", diagnostics.errors);
        let error = &diagnostics.errors[0];
        assert_eq!(error.file, "src/main.eps");
        assert_eq!(error.line, 9);
        assert_eq!(
            error.message,
            "eudplib.utils.eperror.EPError: first line of the message\nsecond line of the message"
        );
        assert_eq!(diagnostics.warnings.len(), 1);
        assert!(diagnostics.warnings[0].message.starts_with(
            "EOFError: EOF when reading a line — euddraft's post-failure console prompt"
        ));

        // The same EOFError with a project frame stays an error.
        let stderr = format!(
            "Traceback (most recent call last):\n  File \"{}\", line 3, in <module>\nEOFError: EOF when reading a line\n",
            root.join("src/main.eps").display()
        );
        let diagnostics = parse_euddraft_output("", &stderr, &root, &eds_dir);
        assert_eq!(diagnostics.errors.len(), 1);
        assert!(diagnostics.warnings.is_empty());

        // A traceback header while frames are pending flushes them as an unterminated error.
        let stderr = "  File \"D:\\x\\y.py\", line 1, in f\nTraceback (most recent call last):\n  File \"D:\\x\\z.py\", line 2, in g\nRuntimeError: boom\n";
        let diagnostics = parse_euddraft_output("", stderr, &root, &eds_dir);
        assert_eq!(diagnostics.errors.len(), 2);
        assert!(diagnostics.errors[0]
            .message
            .starts_with("unterminated euddraft stack"));
        assert_eq!(diagnostics.errors[1].message, "RuntimeError: boom");
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn verbatim_root_prefix_does_not_hide_project_files_that_no_longer_exist() {
        let root = std::env::temp_dir().join("eud-agent-build-verbatim-root");
        let eds_dir = root.join("build/euddraft");
        fs::create_dir_all(&eds_dir).unwrap();
        let canonical_root = fs::canonicalize(&root).unwrap();
        let missing = root.join("src/gone.eps");
        let (file, in_project) =
            resolve_traceback_file(&missing.to_string_lossy(), &canonical_root, &eds_dir);
        assert_eq!(file, "src/gone.eps");
        assert!(in_project);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn bound_text_keeps_head_and_tail_within_limit() {
        assert_eq!(bound_text("short", 100), "short");
        let bounded = bound_text(&"ab".repeat(1_000), 100);
        assert!(bounded.starts_with("abab"));
        assert!(bounded.ends_with("abab"));
        assert!(bounded.contains("[… 1900 characters elided …]"));
    }

    #[test]
    fn dependency_bootstrap_precedes_plugins_and_ordered_python_entrypoints() {
        let (root, project) = project("python-order");
        let mut manifest = project.manifest().clone();
        manifest.python_entrypoints =
            vec!["src/bootstrap.py".to_string(), "src/feature.py".to_string()];
        let eds_path = root.join("build/euddraft/eud-agent.eds");
        let eds = generate_eds(
            &root,
            &manifest,
            &eds_path,
            GeneratedEdsSections {
                python_path: true,
                data_editor: true,
                extra_editor: false,
                custom_tbl: false,
                wireframe_editor: false,
            },
        )
        .unwrap();
        let python_path = eds.find("[PythonPath.py]").unwrap();
        let plugin = eds.find("[eudTurbo]").unwrap();
        let generated = eds.find("[DataEditor.py]").unwrap();
        let bootstrap = eds.find("[../../src/bootstrap.py]").unwrap();
        let feature = eds.find("[../../src/feature.py]").unwrap();
        let main = eds.find("[../../src/main.eps]").unwrap();
        assert!(python_path < plugin);
        assert!(plugin < generated);
        assert!(generated < bootstrap);
        assert!(bootstrap < feature);
        assert!(feature < main);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    #[ignore = "requires EUD_AGENT_EUDDRAFT and EUD_AGENT_BUILD_MAP"]
    fn real_euddraft_builds_generated_native_project() {
        let root = std::env::temp_dir().join(format!(
            "eud-agent-native-real-build-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(root.join("maps")).unwrap();
        fs::copy(
            std::env::var("EUD_AGENT_BUILD_MAP").unwrap(),
            root.join("maps/source.scx"),
        )
        .unwrap();
        let mut project = NativeProject::create(
            &root,
            ProjectManifest {
                schema_version: crate::native_project::PROJECT_SCHEMA_VERSION,
                name: "Native Real Build".to_string(),
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
            .write_source("src/main.eps", "function onPluginStart() {}\n")
            .unwrap();
        project
            .create_source(
                "src/helper.py",
                "from eudplib import *\n\ndef emit():\n    DoActions(SetMemory(0x58F500, SetTo, 1))\n",
            )
            .unwrap();
        project
            .create_source(
                "src/direct.py",
                "from eudplib import *\nimport helper\n\ndef onPluginStart():\n    helper.emit()\n\ndef beforeTriggerExec():\n    pass\n\ndef afterTriggerExec():\n    pass\n",
            )
            .unwrap();
        let mut manifest = project.manifest().clone();
        manifest.python_entrypoints = vec!["src/direct.py".to_string()];
        write_atomic_bytes(
            project.manifest_path(),
            &serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        project = NativeProject::open(&root).unwrap();
        let target = DatTarget::Dat {
            dat: "units".to_string(),
            object_id: 0,
            field: "Hit Points".to_string(),
        };
        project
            .apply_dat_patch(
                &NativeDatPatch {
                    changes: vec![NativeDatChange::Dat {
                        dat: "units".to_string(),
                        object_id: 0,
                        field: "Hit Points".to_string(),
                        before: 10240,
                        after: 20480,
                    }],
                },
                &BTreeMap::from([(target, DatScalar::Number(10240))]),
            )
            .unwrap();
        let euddraft =
            EuddraftLaunch::resolve(Path::new(&std::env::var("EUD_AGENT_EUDDRAFT").unwrap()))
                .unwrap();

        let mut revisions = std::collections::BTreeSet::new();
        let mut final_result = None;
        for cycle in 1..=4 {
            project
                .write_source(
                    "src/main.eps",
                    &format!(
                        "// autonomous real-build cycle {cycle}\nfunction onPluginStart() {{}}\n"
                    ),
                )
                .unwrap();
            revisions.insert(project.revision().unwrap());
            let result = run_native_build(&project, &compat_root(), &euddraft).unwrap();
            assert!(result.ok, "cycle {cycle}: {:?}", result.errors);
            assert!(root.join("build/output.scx").is_file());
            final_result = Some(result);
        }
        assert_eq!(revisions.len(), 4);
        let result = final_result.unwrap();
        // The complete run output is persisted next to the EDS for build_log_read.
        assert_eq!(result.log_path, path_text(&build_log_path(project.root())));
        let log = fs::read_to_string(&result.log_path).unwrap();
        eprintln!(
            "real build: {} errors, {} warnings, log {} bytes",
            result.errors.len(),
            result.warnings.len(),
            log.len()
        );
        assert!(log.starts_with("# euddraft build log\n# exit status: 0x00000000\n"));
        assert!(log.contains("# ===== stdout =====\n"));
        assert!(log.contains("# ===== stderr =====\n"));
        assert!(log.contains(&result.stdout));
        for warning in &result.warnings {
            assert!(!warning.message.is_empty());
            assert!(log.contains(warning.message.lines().next().unwrap()));
        }
        let eds = fs::read_to_string(&result.artifacts.eds_path).unwrap();
        assert!(
            eds.find("[../../src/direct.py]").unwrap() < eds.find("[../../src/main.eps]").unwrap()
        );
        assert!(!eds.contains("helper.py]"));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn resolve_rejects_a_corrupt_managed_install_and_accepts_an_intact_one() {
        let base = std::env::temp_dir().join(format!(
            "eud-agent-resolve-integrity-{}",
            uuid::Uuid::new_v4()
        ));
        let exe_sha = crate::bootstrap::sha256_hex_bytes(b"managed");

        // App-managed install: nearest ancestor marker drives full manifest re-validation.
        let managed = base.join("sha256-managed");
        fs::create_dir_all(&managed).unwrap();
        let managed_exe = managed.join("euddraft.exe");
        fs::write(&managed_exe, b"managed").unwrap();
        let marker = |files: serde_json::Value| {
            serde_json::json!({
                "version": "v0.10.2.5",
                "archive_sha256": exe_sha,
                "executable": "euddraft.exe",
                "files": files,
            })
        };
        let intact = serde_json::json!([
            { "path": "euddraft.exe", "sha256": exe_sha, "bytes": 7 }
        ]);
        let damaged = serde_json::json!([
            { "path": "euddraft.exe", "sha256": exe_sha, "bytes": 7 },
            { "path": "python3.dll", "sha256": exe_sha, "bytes": 7 },
        ]);

        // Corrupt: a declared dependency is missing → resolve refuses and names the file.
        fs::write(
            managed.join(".euddraft-install.json"),
            marker(damaged).to_string(),
        )
        .unwrap();
        let corrupt_error = EuddraftLaunch::resolve(&managed_exe).unwrap_err();
        assert!(
            corrupt_error.contains("corrupt"),
            "expected a corrupt-install error, got: {corrupt_error}"
        );
        assert!(
            corrupt_error.contains("python3.dll"),
            "error must name the missing file, got: {corrupt_error}"
        );
        assert!(EuddraftLaunch::resolve(&managed).is_err());

        // Intact: every declared file present with exact size/sha → resolve succeeds.
        fs::write(
            managed.join(".euddraft-install.json"),
            marker(intact).to_string(),
        )
        .unwrap();
        assert!(matches!(
            EuddraftLaunch::resolve(&managed_exe).unwrap(),
            EuddraftLaunch::Executable(path) if path == managed_exe
        ));
        assert!(matches!(
            EuddraftLaunch::resolve(&managed).unwrap(),
            EuddraftLaunch::Executable(_)
        ));

        // Manual distribution: no ancestor marker → validation is skipped, resolve succeeds.
        let manual = base.join("manual");
        fs::create_dir_all(&manual).unwrap();
        let manual_exe = manual.join("euddraft.exe");
        fs::write(&manual_exe, b"managed").unwrap();
        assert!(matches!(
            EuddraftLaunch::resolve(&manual_exe).unwrap(),
            EuddraftLaunch::Executable(path) if path == manual_exe
        ));

        fs::remove_dir_all(base).ok();
    }
}
