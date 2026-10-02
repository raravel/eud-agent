use super::resources::E2PlannedCopy;
use super::{
    E2Activation, E2Feature, E2ImportIr, E2ImportIssue, E2IssueCode, E2IssueSeverity, E2RawSection,
};
use sha2::{Digest, Sha256};

mod bin;
mod bin_baseline;
mod bin_objects;
mod plugin_file;
mod tile;
mod tile_bmp;
mod tile_emit;

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct E2BinRect {
    pub x: i16,
    pub y: i16,
    pub x2: i16,
    pub y2: i16,
    pub width: i16,
    pub height: i16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct E2BinObject {
    pub rect: E2BinRect,
    pub control_type: i16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct E2BinBaseline {
    pub row: u8,
    pub name: &'static str,
    pub pointer: u32,
    pub rect: E2BinRect,
    pub objects: Vec<E2BinObject>,
}

pub struct E2CompatRequest<'a> {
    pub document: &'a E2ImportIr,
    pub resources: &'a [E2PlannedCopy],
    pub player_race: Option<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum E2ArtifactConsumer {
    Eps,
    Python,
    EdsPlugin,
    PythonEntrypoint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedE2Artifact {
    pub source_section: String,
    pub resource_ids: Vec<String>,
    pub path: String,
    pub build_destination: String,
    pub consumer: E2ArtifactConsumer,
    pub sha256: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct E2CompatPlan {
    pub artifacts: Vec<GeneratedE2Artifact>,
}

pub fn compile_compat(request: E2CompatRequest<'_>) -> Result<E2CompatPlan, E2ImportIssue> {
    plugin_file::reject_effective_grp(request.document, request.resources)?;
    let mut artifacts = Vec::new();
    if let Some(artifact) = bin::compile(&request)? {
        artifacts.push(artifact);
    }
    if let Some(artifact) = tile::compile(&request)? {
        artifacts.push(artifact);
    }
    artifacts.extend(plugin_file::compile(&request)?);
    Ok(E2CompatPlan { artifacts })
}

pub(super) fn active_section<'a>(document: &'a E2ImportIr, name: &str) -> Option<&'a E2RawSection> {
    document
        .raw_sections
        .iter()
        .find(|section| section.name == name && section.activation == E2Activation::Active)
}

pub(super) fn artifact(
    source_section: &str,
    resource_ids: Vec<String>,
    path: String,
    build_destination: String,
    consumer: E2ArtifactConsumer,
    bytes: Vec<u8>,
) -> GeneratedE2Artifact {
    GeneratedE2Artifact {
        source_section: source_section.to_string(),
        resource_ids,
        path,
        build_destination,
        consumer,
        sha256: format!("{:x}", Sha256::digest(&bytes)),
        bytes,
    }
}

pub(super) fn issue(
    code: E2IssueCode,
    feature: E2Feature,
    message: impl Into<String>,
) -> E2ImportIssue {
    E2ImportIssue {
        code,
        severity: E2IssueSeverity::Fatal,
        message: message.into(),
        path: None,
        feature: Some(feature),
        need_id: None,
    }
}

pub(super) fn resource_extension(copy: &E2PlannedCopy) -> Option<&str> {
    copy.resource
        .path
        .rsplit_once('.')
        .map(|(_, extension)| extension)
}

pub(super) fn slug(id: &str) -> String {
    id.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}
