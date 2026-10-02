use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const E2_VERSION: &str = "0.17.9.7";
pub const MAX_TEXT_BYTES: usize = 256 * 1024;
pub const MAX_RESOURCE_BYTES: usize = 64 * 1024;
pub const MAX_FIXTURE_BYTES: usize = 512 * 1024;

const SOURCE_SECTIONS: [&str; 13] = [
    "ProjectSET",
    "DatEditSET",
    "FireGraftSET",
    "BtnSET",
    "ReqSET",
    "BinEditSET",
    "SoundPlayerSET",
    "GRPEditorSET",
    "TileSET",
    "SCDBSet",
    "FileManagerSET",
    "TriggerEditorSET",
    "PluginSET",
];

#[derive(Debug)]
pub enum FixtureError {
    Io(io::Error),
    InvalidRoot(String),
    InvalidRelativePath(String),
    LimitExceeded(String),
    OracleMismatch(String),
}

impl std::fmt::Display for FixtureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "fixture I/O error: {error}"),
            Self::InvalidRoot(path) => write!(f, "fixture root is not writable: {path}"),
            Self::InvalidRelativePath(path) => write!(f, "fixture path escapes root: {path}"),
            Self::LimitExceeded(value) => write!(f, "fixture bound exceeded: {value}"),
            Self::OracleMismatch(value) => write!(f, "fixture oracle mismatch: {value}"),
        }
    }
}

impl std::error::Error for FixtureError {}

impl From<io::Error> for FixtureError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FixtureExpectation {
    Enabled,
    Disabled,
    EnabledNoOp,
    Fatal,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LayerOracle {
    pub ir: String,
    pub eap: String,
    pub build: String,
    pub artifact: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeatureOracle {
    pub id: String,
    pub section: String,
    pub gate: String,
    pub enabled: bool,
    pub expectation: FixtureExpectation,
    pub oracle: LayerOracle,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileOracle {
    pub path: String,
    pub bytes: usize,
    pub sha256: String,
    pub consumer: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FailureOracle {
    pub id: String,
    pub path: String,
    pub diagnostic: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FixtureOracle {
    pub format: String,
    pub version: String,
    pub source_bytes: usize,
    pub source_sha256: String,
    pub input_map: String,
    pub input_map_sha256: String,
    pub output_map_identity: String,
    pub output_map_is_input: bool,
    pub map_source: String,
    pub sections: Vec<String>,
    pub features: Vec<FeatureOracle>,
    pub files: Vec<FileOracle>,
    pub failures: Vec<FailureOracle>,
    pub path_audit: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixtureCase {
    Complete,
    DisabledPayload,
    ActiveScdb,
    EffectiveGrp,
    EmptyGrpNoOp,
    TileFile,
    ActiveChatWrite,
    UnknownVersion,
    Malformed,
    OverBound,
    MissingMap,
    AmbiguousMap,
    StaleMap,
}

impl FixtureCase {
    pub const fn file_name(self) -> &'static str {
        match self {
            Self::Complete => "complete.e2s",
            Self::DisabledPayload => "disabled-payload.e2s",
            Self::ActiveScdb => "active-scdb.e2s",
            Self::EffectiveGrp => "effective-grp.e2s",
            Self::EmptyGrpNoOp => "empty-grp-no-op.e2s",
            Self::TileFile => "tile-file.e2s",
            Self::ActiveChatWrite => "active-chatwrite.e2s",
            Self::UnknownVersion => "unknown-version.e2s",
            Self::Malformed => "malformed.e2s",
            Self::OverBound => "over-bound.e2s",
            Self::MissingMap | Self::AmbiguousMap | Self::StaleMap => "project.e2s",
        }
    }
}

#[derive(Debug, Clone)]
pub struct FixtureBundle {
    pub root: PathBuf,
    pub source_path: PathBuf,
    pub e2p_root: PathBuf,
    pub oracle: FixtureOracle,
    files: BTreeMap<String, Vec<u8>>,
}

impl FixtureBundle {
    pub fn oracle_json(&self) -> Result<String, FixtureError> {
        serde_json::to_string_pretty(&self.oracle)
            .map_err(|error| FixtureError::OracleMismatch(error.to_string()))
    }

    pub fn source_bytes(&self) -> Result<Vec<u8>, FixtureError> {
        fs::read(&self.source_path).map_err(FixtureError::from)
    }

    pub fn package_file(&self, relative: &str) -> Option<&[u8]> {
        self.files.get(relative).map(Vec::as_slice)
    }

    pub fn verify_oracle(&self) -> Result<(), FixtureError> {
        let source = self.source_bytes()?;
        if sha256_hex(&source) != self.oracle.source_sha256 {
            return Err(FixtureError::OracleMismatch(
                "source SHA-256 changed".to_string(),
            ));
        }
        if source.len() != self.oracle.source_bytes {
            return Err(FixtureError::OracleMismatch(
                "source size changed".to_string(),
            ));
        }
        for file in &self.oracle.files {
            let bytes = self.package_file(&file.path).ok_or_else(|| {
                FixtureError::OracleMismatch(format!("missing package file {}", file.path))
            })?;
            if bytes.len() != file.bytes || sha256_hex(bytes) != file.sha256 {
                return Err(FixtureError::OracleMismatch(format!(
                    "package file changed: {}",
                    file.path
                )));
            }
        }
        Ok(())
    }

    pub fn audit_paths(&self) -> Result<usize, FixtureError> {
        let mut count = 0;
        audit_tree(&self.root, &self.root, &mut count)?;
        Ok(count)
    }
}

pub fn build_complete_fixture(
    root: &Path,
    supplied_basemap: Option<&Path>,
) -> Result<FixtureBundle, FixtureError> {
    let fixture_root = FixtureRoot::new(root)?;
    let map_bytes = match supplied_basemap {
        Some(path) => {
            let bytes = fs::read(path)?;
            if bytes.len() > MAX_RESOURCE_BYTES * 16 {
                return Err(FixtureError::LimitExceeded("supplied basemap".to_string()));
            }
            bytes
        }
        None => b"E2-FIXTURE-SYNTHETIC-BASEMAP-V1\n".to_vec(),
    };
    let map_source = if supplied_basemap.is_some() {
        "caller_supplied_copy"
    } else {
        "synthetic_parser_only"
    };
    let source = complete_source(E2_VERSION, SourceMode::Complete);
    if source.len() > MAX_TEXT_BYTES {
        return Err(FixtureError::LimitExceeded("complete E2S".to_string()));
    }
    let source_path = fixture_root.write(
        FixtureCase::Complete.file_name(),
        source.as_bytes(),
        MAX_TEXT_BYTES,
    )?;
    let root_map = fixture_root.write("Map/basemap.scx", &map_bytes, MAX_RESOURCE_BYTES * 16)?;
    let _root_output = fixture_root.write(
        "Map/output.scx",
        b"E2-FIXTURE-OUTPUT-DESTINATION-V1\n",
        MAX_RESOURCE_BYTES,
    )?;

    let e2p_root = fixture_root.directory("portable.e2p")?;
    let package_source = fixture_root.write(
        "portable.e2p/project.e2s",
        source.as_bytes(),
        MAX_TEXT_BYTES,
    )?;
    let package_map = fixture_root.write(
        "portable.e2p/Map/basemap.scx",
        &map_bytes,
        MAX_RESOURCE_BYTES * 16,
    )?;
    let package_output = fixture_root.write(
        "portable.e2p/Map/output.scx",
        b"E2-FIXTURE-OUTPUT-DESTINATION-V1\n",
        MAX_RESOURCE_BYTES,
    )?;
    let _ = (root_map, package_source, package_map, package_output);

    let resources = resource_bytes();
    for (relative, bytes) in &resources {
        fixture_root.write(
            &format!("portable.e2p/{relative}"),
            bytes,
            MAX_RESOURCE_BYTES,
        )?;
    }
    write_case_files(&fixture_root, &map_bytes)?;

    let mut files = BTreeMap::new();
    for (relative, bytes) in resources {
        files.insert(relative, bytes);
    }
    files.insert("Map/basemap.scx".to_string(), map_bytes.clone());
    files.insert(
        "Map/output.scx".to_string(),
        b"E2-FIXTURE-OUTPUT-DESTINATION-V1\n".to_vec(),
    );

    let oracle_files = files
        .iter()
        .map(|(path, bytes)| FileOracle {
            path: path.clone(),
            bytes: bytes.len(),
            sha256: sha256_hex(bytes),
            consumer: consumer_for(path),
        })
        .collect::<Vec<_>>();
    let oracle = FixtureOracle {
        format: "E2S/E2P-0.17.9.7-fixture".to_string(),
        version: E2_VERSION.to_string(),
        source_bytes: source.len(),
        source_sha256: sha256_hex(source.as_bytes()),
        input_map: "Map/basemap.scx".to_string(),
        input_map_sha256: sha256_hex(&map_bytes),
        output_map_identity: "Map/output.scx".to_string(),
        output_map_is_input: false,
        map_source: map_source.to_string(),
        sections: SOURCE_SECTIONS
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
        features: feature_matrix(),
        files: oracle_files,
        failures: failure_matrix(),
        path_audit:
            "all fixture writes are relative to caller root; output map is destination-only"
                .to_string(),
    };
    let oracle_json = serde_json::to_vec_pretty(&oracle)
        .map_err(|error| FixtureError::OracleMismatch(error.to_string()))?;
    fixture_root.write("fixture-oracle.json", &oracle_json, MAX_TEXT_BYTES)?;
    fixture_root.write(
        "portable.e2p/fixture-oracle.json",
        &oracle_json,
        MAX_TEXT_BYTES,
    )?;

    Ok(FixtureBundle {
        root: fixture_root.root,
        source_path,
        e2p_root,
        oracle,
        files,
    })
}

pub fn assert_safe_fixture_root(root: &Path) -> Result<(), FixtureError> {
    let canonical = fs::canonicalize(root).map_err(FixtureError::from)?;
    if canonical.components().any(|component| {
        matches!(component, Component::Normal(value) if value == ".research" || value == "defense_mob")
    }) {
        return Err(FixtureError::InvalidRoot(canonical.display().to_string()));
    }
    Ok(())
}

pub fn write_confined_fixture_file(
    root: &Path,
    relative: &str,
    bytes: &[u8],
) -> Result<PathBuf, FixtureError> {
    FixtureRoot::new(root)?.write(relative, bytes, MAX_RESOURCE_BYTES)
}

struct FixtureRoot {
    root: PathBuf,
}

impl FixtureRoot {
    fn new(root: &Path) -> Result<Self, FixtureError> {
        if root.as_os_str().is_empty() {
            return Err(FixtureError::InvalidRoot("empty path".to_string()));
        }
        if root.components().any(|component| {
            matches!(component, Component::Normal(value) if value == ".research" || value == "defense_mob")
        }) {
            return Err(FixtureError::InvalidRoot(root.display().to_string()));
        }
        fs::create_dir_all(root)?;
        let canonical = fs::canonicalize(root)?;
        if canonical.components().any(|component| {
            matches!(component, Component::Normal(value) if value == ".research" || value == "defense_mob")
        }) {
            return Err(FixtureError::InvalidRoot(canonical.display().to_string()));
        }
        Ok(Self { root: canonical })
    }

    fn directory(&self, relative: &str) -> Result<PathBuf, FixtureError> {
        let path = self.confined(relative)?;
        fs::create_dir_all(&path)?;
        Ok(path)
    }

    fn write(&self, relative: &str, bytes: &[u8], limit: usize) -> Result<PathBuf, FixtureError> {
        if bytes.len() > limit {
            return Err(FixtureError::LimitExceeded(relative.to_string()));
        }
        let path = self.confined(relative)?;
        let parent = path
            .parent()
            .ok_or_else(|| FixtureError::InvalidRelativePath(relative.to_string()))?;
        fs::create_dir_all(parent)?;
        let canonical_parent = fs::canonicalize(parent)?;
        if !canonical_parent.starts_with(&self.root) {
            return Err(FixtureError::InvalidRelativePath(relative.to_string()));
        }
        if path.exists() && !fs::canonicalize(&path)?.starts_with(&self.root) {
            return Err(FixtureError::InvalidRelativePath(relative.to_string()));
        }
        fs::write(&path, bytes)?;
        Ok(path)
    }

    fn confined(&self, relative: &str) -> Result<PathBuf, FixtureError> {
        let candidate = Path::new(relative);
        if candidate.as_os_str().is_empty()
            || relative.as_bytes().contains(&0)
            || candidate.is_absolute()
            || candidate.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(FixtureError::InvalidRelativePath(relative.to_string()));
        }
        let path = self.root.join(candidate);
        if !path.starts_with(&self.root) {
            return Err(FixtureError::InvalidRelativePath(relative.to_string()));
        }
        Ok(path)
    }
}

fn write_case_files(root: &FixtureRoot, map_bytes: &[u8]) -> Result<(), FixtureError> {
    let cases = [
        (
            FixtureCase::DisabledPayload,
            complete_source(E2_VERSION, SourceMode::Disabled),
        ),
        (
            FixtureCase::ActiveScdb,
            complete_source(E2_VERSION, SourceMode::ActiveScdb),
        ),
        (
            FixtureCase::EffectiveGrp,
            complete_source(E2_VERSION, SourceMode::EffectiveGrp),
        ),
        (
            FixtureCase::EmptyGrpNoOp,
            complete_source(E2_VERSION, SourceMode::EmptyGrpNoOp),
        ),
        (
            FixtureCase::TileFile,
            complete_source(E2_VERSION, SourceMode::TileFile),
        ),
        (
            FixtureCase::ActiveChatWrite,
            complete_source(E2_VERSION, SourceMode::ActiveChatWrite),
        ),
        (
            FixtureCase::UnknownVersion,
            complete_source("0.17.9.8", SourceMode::Complete),
        ),
    ];
    for (case, source) in cases {
        root.write(
            &format!("cases/{}", case.file_name()),
            source.as_bytes(),
            MAX_TEXT_BYTES,
        )?;
    }
    root.write(
        &format!("cases/{}", FixtureCase::Malformed.file_name()),
        b"S_ProjectSET\r\nVersion : 0.17.9.7\r\nE_ProjectSET\r\nS_DatEditSET\r\nnot,a,dat,delta\r\n",
        MAX_TEXT_BYTES,
    )?;
    let over_bound = vec![b'X'; MAX_TEXT_BYTES + 1];
    root.write(
        &format!("cases/{}", FixtureCase::OverBound.file_name()),
        &over_bound,
        MAX_TEXT_BYTES + 1,
    )?;

    let map_cases = [
        (FixtureCase::MissingMap, "Map/missing.scx", false),
        (FixtureCase::AmbiguousMap, "Map/basemap.scx", true),
        (FixtureCase::StaleMap, "C:/old/basemap.scx", false),
    ];
    for (case, input_map, include_map) in map_cases {
        let source = complete_source_with_map(E2_VERSION, input_map, SourceMode::Complete);
        let prefix = format!(
            "cases/{}.e2p",
            match case {
                FixtureCase::MissingMap => "missing-map",
                FixtureCase::AmbiguousMap => "ambiguous-map",
                FixtureCase::StaleMap => "stale-map",
                _ => "invalid-map-case",
            }
        );
        root.directory(&prefix)?;
        root.write(
            &format!("{prefix}/project.e2s"),
            source.as_bytes(),
            MAX_TEXT_BYTES,
        )?;
        root.write(
            &format!("{prefix}/Map/output.scx"),
            b"E2-FIXTURE-OUTPUT-DESTINATION-V1\n",
            MAX_RESOURCE_BYTES,
        )?;
        if include_map {
            root.write(
                &format!("{prefix}/Map/basemap.scx"),
                map_bytes,
                MAX_RESOURCE_BYTES * 16,
            )?;
            root.write(
                &format!("{prefix}/Map/alternate/basemap.scx"),
                map_bytes,
                MAX_RESOURCE_BYTES * 16,
            )?;
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum SourceMode {
    Complete,
    Disabled,
    ActiveScdb,
    EffectiveGrp,
    EmptyGrpNoOp,
    TileFile,
    ActiveChatWrite,
}

fn complete_source(version: &str, mode: SourceMode) -> String {
    complete_source_with_map(version, "Map/basemap.scx", mode)
}

fn complete_source_with_map(version: &str, input_map: &str, mode: SourceMode) -> String {
    let all_enabled = !matches!(mode, SourceMode::Disabled);
    let grp_enabled = matches!(mode, SourceMode::EffectiveGrp | SourceMode::EmptyGrpNoOp);
    let scdb_enabled = matches!(mode, SourceMode::ActiveScdb);
    let chatwrite = matches!(mode, SourceMode::ActiveChatWrite);
    let bool_value = |enabled: bool| if enabled { "True" } else { "False" };
    let mut text = String::new();
    section(
        &mut text,
        "ProjectSET",
        &[
            format!("Version : {version}"),
            format!("InputMap : {input_map}"),
            "OutputMap : Map/output.scx".to_string(),
            "euddraftuse : True".to_string(),
            "loadfromCHK : False".to_string(),
            format!("UsedSetting0 : {}", bool_value(all_enabled)),
            format!("UsedSetting1 : {}", bool_value(all_enabled)),
            format!("UsedSetting2 : {}", bool_value(all_enabled)),
            format!("UsedSetting3 : {}", bool_value(all_enabled)),
            format!("UsedSetting4 : {}", bool_value(all_enabled)),
            format!("UsedSetting5 : {}", bool_value(grp_enabled)),
            format!("UsedSetting6 : {}", bool_value(all_enabled)),
            format!("UsedSetting7 : {}", bool_value(all_enabled)),
            format!("UsedSetting8 : {}", bool_value(all_enabled)),
            "EUDEditorDebug : False".to_string(),
            "epTraceDebug : False".to_string(),
            "triggerSetTouse : FixtureTriggers".to_string(),
            "triggerPlayer : 0".to_string(),
        ],
    );
    section(
        &mut text,
        "DatEditSET",
        &[
            "0,7,42,3".to_string(),
            "0,8,43,-2".to_string(),
            "0,9,44,255".to_string(),
        ],
    );
    section(
        &mut text,
        "FireGraftSET",
        &["FireGraft7 : 1,2,3".to_string()],
    );
    section(
        &mut text,
        "BtnSET",
        &[
            "BtnUse0 : True".to_string(),
            "BtnData0 : 1,2,3,4,5,6,Enabled,Disabled".to_string(),
        ],
    );
    section(
        &mut text,
        "ReqSET",
        &[
            "ReqUse0,0 : 3".to_string(),
            "ReqPos0,0 : 7".to_string(),
            "ReqCount0,0 : 2".to_string(),
            "ReqData0,0,0 : 12".to_string(),
            "ReqData0,0,1 : -4".to_string(),
        ],
    );
    section(
        &mut text,
        "BinEditSET",
        &[
            "BinCount : 1".to_string(),
            "Bin0 : object=7,pointer=0x006D5EC8,width=4,value=0x00000002,PlayerRace=0".to_string(),
        ],
    );
    section(
        &mut text,
        "SoundPlayerSET",
        &[
            "Soundinterval : 3".to_string(),
            "SoundCount : 2".to_string(),
            "Sound0 : M0_0.ogg".to_string(),
            "Sound1 : M0_1.ogg".to_string(),
        ],
    );
    section(
        &mut text,
        "GRPEditorSET",
        &[
            format!("GRPCount : {}", if grp_enabled { 1 } else { 0 }),
            "GRP0 : inert-metadata,fixture.grp".to_string(),
            format!(
                "GRPUse0 : {}",
                bool_value(matches!(mode, SourceMode::EffectiveGrp))
            ),
        ],
    );
    section(
        &mut text,
        "TileSET",
        &[
            format!(
                "ProjectTileUseFile : {}",
                bool_value(matches!(mode, SourceMode::TileFile))
            ),
            "TileCV5Group : 4".to_string(),
            "TileCV5Index : 9".to_string(),
            "TileVX4Halfword : 0x0021".to_string(),
            "TileVR4Halfword : 0x0034".to_string(),
        ],
    );
    section(
        &mut text,
        "SCDBSet",
        &[
            format!("SCDBUse : {}", bool_value(scdb_enabled)),
            "SCDBDeath : 1,2".to_string(),
            "SCDBLoc : fixture-location".to_string(),
            "SCDBVariable : fixture-variable".to_string(),
        ],
    );
    section(
        &mut text,
        "FileManagerSET",
        &[
            "wire : 7,8,9,10".to_string(),
            "stat_txt : resources/e2/files/stat_txt.tbl".to_string(),
            "iscript : resources/e2/files/iscript.bin".to_string(),
            "aiscript : resources/e2/files/aiscript.bin".to_string(),
        ],
    );
    let trigger = if chatwrite {
        "ChatWrite"
    } else {
        "DisplayText"
    };
    section(&mut text, "TriggerEditorSET", &[
        "&AddText&".to_string(),
        format!("Type:0,False,False,False\r\nact:{trigger}\r\nfixture-trigger\r\nElementsCount:0\r\nEND"),
        "&functions&\r\nType:3,False,False,False\r\nfixture-function\r\nElementsCount:0\r\nEND".to_string(),
    ]);
    section(
        &mut text,
        "PluginSET",
        &[
            "fixturePlugin : resources/e2/plugins/fixture.eps".to_string(),
            "fixturePluginEnabled : True".to_string(),
        ],
    );
    text
}

fn section(text: &mut String, name: &str, lines: &[String]) {
    text.push_str("S_");
    text.push_str(name);
    text.push_str("\r\n");
    for line in lines {
        text.push_str(line);
        text.push_str("\r\n");
    }
    text.push_str("E_");
    text.push_str(name);
    text.push_str("\r\n");
}

fn resource_bytes() -> BTreeMap<String, Vec<u8>> {
    BTreeMap::from([
        ("resources/e2/audio/M0_0.ogg".to_string(), tiny_ogg(0)),
        ("resources/e2/audio/M0_1.ogg".to_string(), tiny_ogg(1)),
        (
            "resources/e2/bin/fixture.bin".to_string(),
            vec![0x02, 0x00, 0x00, 0x00, 0x07, 0x00, 0x21, 0x00],
        ),
        (
            "resources/e2/tiles/fixture.cv5".to_string(),
            vec![0x04, 0x00, 0x09, 0x00],
        ),
        (
            "resources/e2/tiles/fixture.vx4".to_string(),
            vec![0x21, 0x00],
        ),
        (
            "resources/e2/tiles/fixture.vr4".to_string(),
            vec![0x34, 0x00],
        ),
        (
            "resources/e2/files/stat_txt.tbl".to_string(),
            b"fixture text\r\n".to_vec(),
        ),
        (
            "resources/e2/files/iscript.bin".to_string(),
            vec![0x49, 0x53, 0x43, 0x52, 0x01],
        ),
        (
            "resources/e2/files/aiscript.bin".to_string(),
            vec![0x41, 0x49, 0x53, 0x43, 0x01],
        ),
        (
            "resources/e2/plugins/fixture.eps".to_string(),
            b"function fixturePlugin() {}\r\n".to_vec(),
        ),
    ])
}

fn feature_matrix() -> Vec<FeatureOracle> {
    let rows = [
        (
            "dat",
            "S_DatEditSET",
            "UsedSetting0",
            "DAT[0,7,42]=+3 width=1; DAT[0,8,43]=-2 width=2",
            "dat/standard.json:delta records",
            "DataEditor.py:AddSubtract(width-aware)",
            "src/e2/dat_delta.eps",
        ),
        (
            "firegraft-buttons",
            "S_FireGraftSET",
            "UsedSetting1",
            "FireGraft[7]=(1,2,3); BtnSet injection",
            "dat/xdat.json + dat/buttons.json",
            "ExtraDataEditor.py + button consumer",
            "src/e2/firegraft_buttons.eps",
        ),
        (
            "bin",
            "S_BinEditSET",
            "UsedSetting2",
            "object=7 pointer=0x006D5EC8 race=0 width=4",
            "resources/e2/bin/fixture.bin",
            "bounded BIN compatibility consumer",
            "src/e2/bin_compat.eps",
        ),
        (
            "tile-memory",
            "S_TileSET",
            "UsedSetting3",
            "CV5(4,9),VX4=0x0021,VR4=0x0034",
            "resources/e2/tiles/*.cv5|vx4|vr4",
            "in-memory tile halfword patch",
            "src/e2/tile_compat.eps",
        ),
        (
            "tile-file",
            "S_TileSET",
            "UsedSetting3",
            "ProjectTileUseFile=true file refs",
            "resources/e2/tiles/file-mode/*",
            "file-backed tile patch",
            "src/e2/tile_file_compat.eps",
        ),
        (
            "buttons-bgm",
            "S_BtnSET/SoundPlayerSET",
            "UsedSetting4",
            "interval=3; M0_0.ogg,M0_1.ogg contiguous",
            "dat/buttons.json + resources/e2/audio",
            "ordered MPQAddFile aliases",
            "src/e2/bgm.eps",
        ),
        (
            "grp",
            "S_GRPEditorSET",
            "UsedSetting5",
            "effective replacement/use record",
            "fatal inventory only",
            "active_grp_unsupported",
            "none",
        ),
        (
            "struct-gui",
            "S_TriggerEditorSET",
            "UsedSetting6",
            "recursive node order/flags/raw values",
            "src/main.eps provenance",
            "EUDLoop lifecycle compiler",
            "src/main.eps",
        ),
        (
            "plugin",
            "S_PluginSET",
            "UsedSetting7",
            "ordered fixturePlugin entrypoint",
            "plugins/fixture.eps",
            "ordered plugin consumer",
            "src/e2/plugin_compat.eps",
        ),
        (
            "file-manager",
            "S_FileManagerSET",
            "UsedSetting8",
            "TBL/iscript/aiscript owned refs",
            "resources/e2/files",
            "TBL/wireframe/binary consumer",
            "src/e2/file_manager.eps",
        ),
        (
            "scdb",
            "S_SCDBSet",
            "SCDBUse",
            "SCDBUse=true inventory",
            "fatal inventory only",
            "active_scdb_unsupported",
            "none",
        ),
        (
            "sound",
            "S_SoundPlayerSET",
            "sound refs",
            "M0_0 then M0_1; interval*1050-70",
            "resources/e2/audio/*.ogg",
            "BGM player + MPQ aliases",
            "src/e2/bgm.eps",
        ),
        (
            "chatwrite",
            "S_TriggerEditorSET",
            "enabled ChatWrite",
            "ChatWrite preserved as unresolved action",
            "no authority",
            "active_chatwrite_unverified",
            "none",
        ),
    ];
    let mut result = Vec::with_capacity(rows.len() * 2 + 3);
    for (id, section_name, gate, ir, eap, build, artifact) in rows {
        let expectation = if id == "grp" || id == "scdb" || id == "chatwrite" {
            FixtureExpectation::Fatal
        } else {
            FixtureExpectation::Enabled
        };
        result.push(FeatureOracle {
            id: format!("{id}_enabled"),
            section: section_name.to_string(),
            gate: gate.to_string(),
            enabled: true,
            expectation,
            oracle: LayerOracle {
                ir: ir.to_string(),
                eap: eap.to_string(),
                build: build.to_string(),
                artifact: artifact.to_string(),
            },
        });
        result.push(FeatureOracle {
            id: format!("{id}_disabled"),
            section: section_name.to_string(),
            gate: gate.to_string(),
            enabled: false,
            expectation: FixtureExpectation::Disabled,
            oracle: LayerOracle {
                ir: format!("inert provenance: {ir}"),
                eap: "optional import provenance only".to_string(),
                build: "no active section".to_string(),
                artifact: "none".to_string(),
            },
        });
    }
    result.push(FeatureOracle {
        id: "grp_enabled_empty_no_op".to_string(),
        section: "S_GRPEditorSET".to_string(),
        gate: "UsedSetting5=true and no effective record".to_string(),
        enabled: true,
        expectation: FixtureExpectation::EnabledNoOp,
        oracle: LayerOracle {
            ir: "GRP metadata retained with zero effective replacement/use rows".to_string(),
            eap: "provenance only".to_string(),
            build: "no EUDGrp load or pointer write".to_string(),
            artifact: "none".to_string(),
        },
    });
    result.push(FeatureOracle {
        id: "disabled_payload_matrix".to_string(),
        section: "all known sections".to_string(),
        gate: "all feature flags false".to_string(),
        enabled: false,
        expectation: FixtureExpectation::Disabled,
        oracle: LayerOracle {
            ir: "raw payload retained as inert provenance".to_string(),
            eap: "optional import provenance only".to_string(),
            build: "no active DAT/BIN/tile/plugin/resource output".to_string(),
            artifact: "none".to_string(),
        },
    });
    result
}

fn failure_matrix() -> Vec<FailureOracle> {
    vec![
        FailureOracle {
            id: "unknown_version".to_string(),
            path: "cases/unknown-version.e2s".to_string(),
            diagnostic: "unsupported_e2_version".to_string(),
        },
        FailureOracle {
            id: "malformed".to_string(),
            path: "cases/malformed.e2s".to_string(),
            diagnostic: "malformed_e2_section".to_string(),
        },
        FailureOracle {
            id: "over_bound".to_string(),
            path: "cases/over-bound.e2s".to_string(),
            diagnostic: "e2_input_too_large".to_string(),
        },
        FailureOracle {
            id: "active_scdb".to_string(),
            path: "cases/active-scdb.e2s".to_string(),
            diagnostic: "active_scdb_unsupported".to_string(),
        },
        FailureOracle {
            id: "effective_grp".to_string(),
            path: "cases/effective-grp.e2s".to_string(),
            diagnostic: "active_grp_unsupported".to_string(),
        },
        FailureOracle {
            id: "active_chatwrite".to_string(),
            path: "cases/active-chatwrite.e2s".to_string(),
            diagnostic: "active_chatwrite_unverified".to_string(),
        },
        FailureOracle {
            id: "missing_map".to_string(),
            path: "cases/missing-map.e2p/project.e2s".to_string(),
            diagnostic: "needs_relink_missing_input_map".to_string(),
        },
        FailureOracle {
            id: "ambiguous_map".to_string(),
            path: "cases/ambiguous-map.e2p/project.e2s".to_string(),
            diagnostic: "needs_relink_ambiguous_input_map".to_string(),
        },
        FailureOracle {
            id: "stale_map".to_string(),
            path: "cases/stale-map.e2p/project.e2s".to_string(),
            diagnostic: "needs_relink_stale_input_map".to_string(),
        },
    ]
}

fn consumer_for(path: &str) -> String {
    if path.ends_with(".ogg") {
        "bgm_mpq_alias".to_string()
    } else if path.contains("/bin/") {
        "bin_compat".to_string()
    } else if path.contains("/tiles/") {
        "tile_compat".to_string()
    } else if path.contains("/plugins/") {
        "plugin_entrypoint".to_string()
    } else {
        "file_manager_or_map".to_string()
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn audit_tree(path: &Path, root: &Path, count: &mut usize) -> Result<(), FixtureError> {
    let canonical = fs::canonicalize(path)?;
    if !canonical.starts_with(root) {
        return Err(FixtureError::InvalidRelativePath(
            path.display().to_string(),
        ));
    }
    *count += 1;
    if canonical.is_dir() {
        for entry in fs::read_dir(path)? {
            audit_tree(&entry?.path(), root, count)?;
        }
    }
    Ok(())
}

fn tiny_ogg(sequence: u8) -> Vec<u8> {
    let mut packet = b"OpusHead".to_vec();
    packet.extend_from_slice(&[1, 1, 0x38, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    ogg_page(2, 0xE200 + u32::from(sequence), 0, &packet)
}

fn ogg_page(header_type: u8, serial: u32, sequence: u64, packet: &[u8]) -> Vec<u8> {
    let segment_len = u8::try_from(packet.len()).unwrap_or(255);
    let mut page = Vec::with_capacity(27 + 1 + packet.len());
    page.extend_from_slice(b"OggS");
    page.extend_from_slice(&[0, header_type]);
    page.extend_from_slice(&sequence.to_le_bytes());
    page.extend_from_slice(&serial.to_le_bytes());
    page.extend_from_slice(&0u32.to_le_bytes());
    page.extend_from_slice(&[1, segment_len]);
    page.extend_from_slice(packet);
    let checksum = ogg_crc(&page);
    page[22..26].copy_from_slice(&checksum.to_le_bytes());
    page
}

fn ogg_crc(bytes: &[u8]) -> u32 {
    let mut crc = 0u32;
    for byte in bytes {
        crc ^= u32::from(*byte) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04C1_1DB7
            } else {
                crc << 1
            };
        }
    }
    crc
}
