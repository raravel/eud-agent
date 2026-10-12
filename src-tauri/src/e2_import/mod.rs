use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

pub mod bgm;
pub mod compat;
pub mod compiler;
pub mod dat_projection;
pub mod definitions;
pub mod parser;
pub mod resources;
pub mod transaction;

#[cfg(test)]
mod resource_relocation_scenario;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum E2Version {
    #[serde(rename = "0.17.9.7")]
    V0_17_9_7,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum E2SourceKind {
    E2s,
    E2p,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum E2Feature {
    ProjectMap,
    Dat,
    FireGraft,
    Bin,
    CustomTiles,
    ButtonsBgm,
    Grp,
    StructGui,
    Plugin,
    FileManager,
    Scdb,
    SoundBgmReferences,
    DisabledPayload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum E2Activation {
    Inactive,
    Active,
    Inert,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum E2RawValue {
    Null,
    Bool(bool),
    Integer(i64),
    String(String),
    List(Vec<E2RawValue>),
    Object(BTreeMap<String, E2RawValue>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct E2RawSection {
    pub name: String,
    pub activation: E2Activation,
    pub fields: BTreeMap<String, E2RawValue>,
    pub payload: E2RawValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct E2ProjectSettings {
    pub name: String,
    pub input_map: String,
    pub output_map: String,
    pub sound_interval: u32,
    pub raw_fields: BTreeMap<String, E2RawValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct E2FeatureState {
    pub feature: E2Feature,
    pub activation: E2Activation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_payload: Option<E2RawValue>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum E2TriggerNodeKind {
    Action,
    Condition,
    Group,
    Lifecycle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct E2TriggerNode {
    pub id: String,
    pub enabled: bool,
    pub folded: bool,
    pub not_condition: bool,
    pub kind: E2TriggerNodeKind,
    pub name: String,
    pub values: Vec<E2RawValue>,
    pub children: Vec<E2TriggerNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct E2ResourceReference {
    pub id: String,
    pub feature: E2Feature,
    pub kind: ProjectResourceKind,
    pub source_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct E2Document {
    pub version: E2Version,
    pub source_kind: E2SourceKind,
    pub settings: E2ProjectSettings,
    pub features: Vec<E2FeatureState>,
    pub triggers: Vec<E2TriggerNode>,
    pub resources: Vec<E2ResourceReference>,
    pub raw_sections: Vec<E2RawSection>,
}

pub type E2ImportIr = E2Document;
pub type E2ImportIssue = E2Issue;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum E2IntegerWidth {
    Byte,
    Word,
    Dword,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct E2DatDelta {
    pub table: String,
    pub object_id: u32,
    pub field: String,
    pub before: i64,
    pub after: i64,
    pub width: E2IntegerWidth,
    pub signed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub var_start: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct E2DatProjection {
    pub deltas: Vec<E2DatDelta>,
    pub xdat: Vec<E2RawSection>,
    pub tbl: Vec<E2RawSection>,
    pub requirements: Vec<E2RawSection>,
    pub buttons: Vec<E2RawSection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct E2CompiledSources {
    pub files: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct E2BgmSegment {
    pub index: u32,
    pub resource_id: String,
    pub alias: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct E2BgmTrack {
    pub music_number: u32,
    pub looped: bool,
    pub segments: Vec<E2BgmSegment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct E2BgmPlan {
    pub sound_interval: u32,
    pub tracks: Vec<E2BgmTrack>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct E2MpqAsset {
    pub alias: String,
    pub resource_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct E2CompiledProject {
    pub settings: E2ProjectSettings,
    pub sources: E2CompiledSources,
    pub dat: E2DatProjection,
    pub resources: Vec<ProjectResource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bgm: Option<E2BgmPlan>,
    pub mpq_assets: Vec<E2MpqAsset>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProjectResourceKind {
    Map,
    Audio,
    Bin,
    Tile,
    Plugin,
    Dat,
    File,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProjectResourceConsumer {
    SourceMap,
    Mpq,
    BinCompat,
    TileCompat,
    Plugin,
    Dat,
    FileManager,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectResourceProvenance {
    pub feature: E2Feature,
    pub original_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectResource {
    pub id: String,
    pub path: String,
    pub kind: ProjectResourceKind,
    pub size: u64,
    pub sha256: String,
    pub consumer: ProjectResourceConsumer,
    pub build_destination: String,
    pub provenance: ProjectResourceProvenance,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct E2DisabledSectionProvenance {
    pub feature: E2Feature,
    pub activation: E2Activation,
    pub raw_payload: E2RawValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct E2ImportProvenance {
    pub source_name: String,
    pub disabled_sections: Vec<E2DisabledSectionProvenance>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct E2ImportManifest {
    pub version: E2Version,
    pub source_kind: E2SourceKind,
    pub fingerprint: String,
    pub resources: Vec<ProjectResource>,
    pub provenance: E2ImportProvenance,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectResourceSnapshot {
    pub resources: Vec<ProjectResourceSnapshotEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectResourceSnapshotEntry {
    pub id: String,
    pub path: String,
    pub kind: ProjectResourceKind,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum E2IssueSeverity {
    Info,
    Warning,
    Fatal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum E2IssueCode {
    UnsupportedVersion,
    MalformedInput,
    SourceChanged,
    MissingInputMap,
    AmbiguousInputMap,
    ActiveScdbUnsupported,
    ActiveGrpUnsupported,
    ActiveChatwriteUnverified,
    ActiveDefinitionUnknown,
    InvalidResource,
    UnsafePlugin,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct E2Issue {
    pub code: E2IssueCode,
    pub severity: E2IssueSeverity,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feature: Option<E2Feature>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub need_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum E2RelinkKind {
    InputMap,
    Resource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct E2RelinkNeed {
    pub id: String,
    pub kind: E2RelinkKind,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_path: Option<String>,
    pub candidates: Vec<String>,
    pub extensions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct E2RelinkChoice {
    pub need_id: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct E2RelinkPickerRequest {
    pub need: E2RelinkNeed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct E2PreflightRequest {
    pub source_path: String,
    pub destination_path: String,
    pub relinks: Vec<E2RelinkChoice>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct E2ImportRequest {
    pub source_path: String,
    pub destination_path: String,
    pub fingerprint: String,
    pub relinks: Vec<E2RelinkChoice>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct E2CancelRequest {
    pub import_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum E2PreflightResult {
    Ready {
        fingerprint: String,
        source_kind: E2SourceKind,
        features: Vec<E2Feature>,
        warnings: Vec<E2Issue>,
    },
    NeedsRelink {
        fingerprint: String,
        needs: Vec<E2RelinkNeed>,
    },
    Fatal {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fingerprint: Option<String>,
        issues: Vec<E2Issue>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum E2ImportPhase {
    Preflight,
    Resolving,
    Compiling,
    CollectingResources,
    Materializing,
    Validating,
    Publishing,
    Opening,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum E2ImportResult {
    Progress {
        import_id: String,
        fingerprint: String,
        phase: E2ImportPhase,
        completed: u64,
        total: u64,
        message: String,
    },
    Completed {
        import_id: String,
        fingerprint: String,
        project_opened: bool,
        project_path: String,
    },
    Cancelled {
        import_id: String,
        fingerprint: String,
    },
    Fatal {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        import_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fingerprint: Option<String>,
        issues: Vec<E2Issue>,
    },
}

impl E2PreflightRequest {
    pub fn validate(&self) -> Result<(), String> {
        validate_required(&self.source_path, "sourcePath")?;
        validate_required(&self.destination_path, "destinationPath")?;
        validate_relinks(&self.relinks)
    }
}

impl E2ImportRequest {
    pub fn validate(&self) -> Result<(), String> {
        validate_required(&self.source_path, "sourcePath")?;
        validate_required(&self.destination_path, "destinationPath")?;
        validate_required(&self.fingerprint, "fingerprint")?;
        validate_relinks(&self.relinks)
    }
}

impl E2CancelRequest {
    pub fn validate(&self) -> Result<(), String> {
        validate_required(&self.import_id, "importId")
    }
}

impl E2PreflightResult {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Ready {
                fingerprint,
                warnings,
                ..
            } => {
                validate_required(fingerprint, "fingerprint")?;
                validate_issues(warnings)
            }
            Self::NeedsRelink { fingerprint, needs } => {
                validate_required(fingerprint, "fingerprint")?;
                if needs.is_empty() {
                    return Err("needsRelink must contain at least one need".to_string());
                }
                for need in needs {
                    validate_required(&need.id, "need.id")?;
                    validate_required(&need.label, "need.label")?;
                    if let Some(path) = &need.original_path {
                        validate_required(path, "need.originalPath")?;
                    }
                }
                Ok(())
            }
            Self::Fatal {
                fingerprint,
                issues,
            } => {
                if let Some(value) = fingerprint {
                    validate_required(value, "fingerprint")?;
                }
                validate_fatal_issues(issues)
            }
        }
    }
}

impl E2ImportResult {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Progress {
                import_id,
                fingerprint,
                completed,
                total,
                ..
            } => {
                validate_required(import_id, "importId")?;
                validate_required(fingerprint, "fingerprint")?;
                if completed > total {
                    return Err("progress completed must not exceed total".to_string());
                }
                Ok(())
            }
            Self::Completed {
                import_id,
                fingerprint,
                project_opened,
                project_path,
            } => {
                validate_required(import_id, "importId")?;
                validate_required(fingerprint, "fingerprint")?;
                validate_required(project_path, "projectPath")?;
                if !project_opened {
                    return Err("completed import must have projectOpened=true".to_string());
                }
                Ok(())
            }
            Self::Cancelled {
                import_id,
                fingerprint,
            } => {
                validate_required(import_id, "importId")?;
                validate_required(fingerprint, "fingerprint")
            }
            Self::Fatal {
                import_id,
                fingerprint,
                issues,
            } => {
                if let Some(value) = import_id {
                    validate_required(value, "importId")?;
                }
                if let Some(value) = fingerprint {
                    validate_required(value, "fingerprint")?;
                }
                validate_fatal_issues(issues)
            }
        }
    }
}

fn validate_relinks(relinks: &[E2RelinkChoice]) -> Result<(), String> {
    let mut ids = BTreeSet::new();
    for relink in relinks {
        validate_required(&relink.need_id, "relink.needId")?;
        validate_required(&relink.path, "relink.path")?;
        if !ids.insert(&relink.need_id) {
            return Err(format!("duplicate relink needId: {}", relink.need_id));
        }
    }
    Ok(())
}

fn validate_fatal_issues(issues: &[E2Issue]) -> Result<(), String> {
    if issues.is_empty() {
        return Err("fatal result must contain at least one issue".to_string());
    }
    validate_issues(issues)
}

fn validate_issues(issues: &[E2Issue]) -> Result<(), String> {
    for issue in issues {
        validate_required(&issue.message, "issue.message")?;
        if let Some(value) = &issue.need_id {
            validate_required(value, "issue.needId")?;
        }
    }
    Ok(())
}

fn validate_required(value: &str, label: &str) -> Result<(), String> {
    if value.trim().is_empty() || value.contains('\0') {
        return Err(format!("{label} must be non-empty"));
    }
    Ok(())
}

#[cfg(test)]
mod contracts;
