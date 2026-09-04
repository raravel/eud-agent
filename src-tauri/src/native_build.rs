//! Deterministic native EDS/build frontend for euddraft 0.10.2.5.
//!
//! euddraft accepts one UTF-8 `.eds` file, changes cwd to its directory, loads `[main]`
//! input/output, then loads every remaining section in declaration order as an EPS/Python/global
//! plugin. This module materializes that contract without invoking EUD Editor.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use encoding_rs::EUC_KR;
use serde::Serialize;

use crate::memory::write_atomic_bytes;
use crate::native_project::{
    NativeDatState, NativeProject, NumericOverride, ProjectManifest, TextOverride,
};

const EUDDRAFT_TIMEOUT: Duration = Duration::from_secs(300);
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(50);
const TRACEBACK_MARKER: &str = "Traceback (most recent call last):";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatFieldMeta {
    pub name: String,
    pub size: u8,
    pub var_start: u32,
    pub var_end: u32,
    pub var_array: u32,
    pub var_index: u32,
    pub init_var: i64,
    pub offset: u32,
    pub baseline: Vec<i64>,
}

impl DatFieldMeta {
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

#[derive(Debug, Clone)]
pub struct DatCatalog {
    fields: BTreeMap<String, BTreeMap<String, DatFieldMeta>>,
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
        for table in [
            "units", "weapons", "flingy", "sprites", "images", "upgrades", "techdata", "orders",
            "portdata", "sfxdata",
        ] {
            let definition = compat_root.join("DatFiles").join(format!("{table}.def"));
            let data_path = compat_root.join("DatFiles").join(format!("{table}.dat"));
            fields.insert(
                table.to_string(),
                parse_dat_definition(
                    table,
                    &read_text(&definition)?,
                    &fs::read(&data_path).map_err(stringify_io)?,
                    &offsets,
                )?,
            );
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
            button_defaults,
            tbl,
            status,
            requirements,
        })
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeBuildError {
    pub source: String,
    pub file: String,
    pub line: u64,
    pub message: String,
    pub raw: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeBuildResult {
    pub ok: bool,
    pub errors: Vec<NativeBuildError>,
    pub stdout: String,
    pub stderr: String,
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
                return Ok(Self::Executable(executable));
            }
        }
        Err(format!(
            "euddraft path does not identify euddraft.exe, euddraft.py, or a source root: {}",
            configured.display()
        ))
    }

    fn command(&self, eds_path: &Path) -> Result<Command, String> {
        let mut command = match self {
            Self::Executable(executable) => Command::new(executable),
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
        run_process(self.command(eds_path)?, eds_path, timeout)
    }
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
    let catalog = DatCatalog::load(compat_root)?;
    let build_dir = project.root().join("build/euddraft");
    fs::create_dir_all(&build_dir).map_err(stringify_io)?;

    let data_editor = generate_data_editor(project.dat(), &catalog)?;
    let data_editor_path = build_dir.join("DataEditor.py");
    let data_editor_output = if data_editor.is_some() {
        write_atomic_bytes(&data_editor_path, data_editor.as_ref().unwrap().as_bytes())
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
    let extra_editor_output = if extra_editor.is_some() {
        write_atomic_bytes(
            &extra_editor_path,
            extra_editor.as_ref().unwrap().as_bytes(),
        )
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

    let eds_path = build_dir.join("eud-agent.eds");
    let eds = generate_eds(
        project.root(),
        project.manifest(),
        &eds_path,
        data_editor_output.is_some(),
        extra_editor_output.is_some(),
        custom_tbl.is_some(),
        wireframe_output.is_some(),
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
    })
}

pub fn run_native_build(
    project: &NativeProject,
    compat_root: &Path,
    euddraft: &EuddraftLaunch,
) -> Result<NativeBuildResult, String> {
    let artifacts = generate_native_build(project, compat_root)?;
    let eds_path = PathBuf::from(&artifacts.eds_path);
    let output_map = PathBuf::from(&artifacts.output_map);
    let before_output = modified_time(&output_map)?;
    let captured = euddraft.run(&eds_path, EUDDRAFT_TIMEOUT)?;
    let fresh_output = is_fresh_output(before_output, modified_time(&output_map)?);
    let mut errors = parse_euddraft_output(&captured.stdout, &captured.stderr);
    let ok = captured.success && fresh_output;
    if !ok && errors.is_empty() {
        let raw = joined_output(&captured.stdout, &captured.stderr);
        errors.push(NativeBuildError {
            source: "euddraft".to_string(),
            file: String::new(),
            line: 0,
            message: if captured.success {
                "euddraft exited successfully but did not produce a fresh output map".to_string()
            } else if raw.is_empty() {
                "euddraft failed without diagnostic output".to_string()
            } else {
                "euddraft failed with an unrecognized diagnostic format".to_string()
            },
            raw,
        });
    }
    Ok(NativeBuildResult {
        ok,
        errors,
        stdout: captured.stdout,
        stderr: captured.stderr,
        artifacts,
    })
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
    let max = match meta.size {
        1 => u8::MAX as i64,
        2 => u16::MAX as i64,
        4 => u32::MAX as i64,
        other => return Err(format!("unsupported DAT field width {other}")),
    } + meta.init_var;
    let min = meta.init_var;
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
            let pointer = u16::try_from(section.len() / 2)
                .map_err(|_| format!("{table} requirement pointer overflow"))?;
            pointers[object_id] = pointer;
            if writes_object_id {
                section.extend_from_slice(&(object_id as u16).to_le_bytes());
            }
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
    if wireframe.is_some() {
        output.push_str("import WireFrameDataEditor\n\n");
        output.push_str("def init_wireframe():\n");
        output.push_str("    WireFrameDataEditor.WireFrameInit()\n");
        for (object_id, fields) in wireframe.unwrap() {
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

fn generate_eds(
    project_root: &Path,
    manifest: &ProjectManifest,
    eds_path: &Path,
    has_data_editor: bool,
    has_extra_editor: bool,
    has_custom_tbl: bool,
    has_wireframe_editor: bool,
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
    if has_data_editor {
        eds.push_str("\n[DataEditor.py]\n");
    }
    if has_wireframe_editor {
        eds.push_str("\n[WireFrameDataEditor.eps]\n");
    }
    if has_extra_editor {
        eds.push_str("\n[ExtraDataEditor.py]\n");
    }
    if has_custom_tbl {
        eds.push_str("\n[dataDumper]\ncustom_txt.tbl: 0x6D5A30, copy\n");
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
) -> Result<BTreeMap<String, DatFieldMeta>, String> {
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
    for properties in raw_fields.into_values() {
        let name = properties
            .get("Name")
            .ok_or_else(|| format!("{table}.def field is missing Name"))?
            .split(':')
            .next()
            .unwrap()
            .trim()
            .to_string();
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
        fields.insert(
            name.clone(),
            DatFieldMeta {
                name,
                size,
                var_start,
                var_end,
                var_array,
                var_index,
                init_var,
                offset,
                baseline,
            },
        );
    }
    Ok(fields)
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
        let end = bytes[start..]
            .iter()
            .position(|value| *value == 0)
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
    let mut values = baseline[..=last].to_vec();
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
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

fn run_process(
    mut command: Command,
    eds_path: &Path,
    timeout: Duration,
) -> Result<CapturedProcess, String> {
    let cwd = eds_path
        .parent()
        .ok_or_else(|| "EDS path has no parent".to_string())?;
    command
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to start euddraft: {error}"))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < timeout => thread::sleep(PROCESS_POLL_INTERVAL),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "euddraft did not finish within {}s",
                    timeout.as_secs()
                ));
            }
            Err(error) => return Err(format!("failed to poll euddraft: {error}")),
        }
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("failed to collect euddraft output: {error}"))?;
    Ok(CapturedProcess {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn parse_euddraft_output(stdout: &str, stderr: &str) -> Vec<NativeBuildError> {
    let combined = joined_output(stdout, stderr);
    let mut errors = Vec::new();
    for line in combined.lines() {
        let trimmed = line.trim();
        if let Some((file, rest)) = parse_python_file_line(trimmed) {
            errors.push(NativeBuildError {
                source: "euddraft".to_string(),
                file,
                line: rest.0,
                message: rest.1.clone(),
                raw: trimmed.to_string(),
            });
        }
    }
    if errors.is_empty() && combined.contains(TRACEBACK_MARKER) {
        let message = combined
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("euddraft traceback")
            .trim()
            .to_string();
        errors.push(NativeBuildError {
            source: "euddraft".to_string(),
            file: String::new(),
            line: 0,
            message,
            raw: combined,
        });
    }
    errors
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

fn parse_python_file_line(line: &str) -> Option<(String, (u64, String))> {
    let marker = "File \"";
    let start = line.find(marker)? + marker.len();
    let rest = &line[start..];
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
    Some((file, (line_number, "euddraft source error".to_string())))
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
        DatScalar, DatTarget, EdsPlugin, NativeDatChange, NativeDatPatch, ProjectManifest,
        ProjectSettings,
    };

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
            schema_version: 1,
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
                schema_version: 1,
                name: "Native Real Build".to_string(),
                source_map: "maps/source.scx".to_string(),
                output_map: "build/output.scx".to_string(),
                main_file: "src/main.eps".to_string(),
                settings: ProjectSettings::default(),
                plugins: Vec::new(),
                editor_compatibility: None,
            },
        )
        .unwrap();
        project
            .write_source("src/main.eps", "function onPluginStart() {}\n")
            .unwrap();
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

        let result = run_native_build(&project, &compat_root(), &euddraft).unwrap();

        assert!(result.ok, "{:?}", result.errors);
        assert!(root.join("build/output.scx").is_file());
        fs::remove_dir_all(root).ok();
    }
}
