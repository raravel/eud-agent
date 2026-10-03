use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::path::{case_fold, invalid, SelectedFile};
use super::{
    E2Document, E2Feature, E2Issue, E2RelinkChoice, E2RelinkNeed, ProjectResource,
    ProjectResourceConsumer, ProjectResourceKind, ProjectResourceProvenance,
};

/// Read-only inputs for E2 map and resource resolution.
pub struct E2ResourceResolveRequest<'a> {
    pub source_path: &'a Path,
    pub document: &'a E2Document,
    pub relinks: &'a [E2RelinkChoice],
    pub expected_fingerprint: Option<&'a str>,
}

/// One byte-owned copy operation. Its destination is project-relative.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct E2PlannedCopy {
    pub resource: ProjectResource,
    pub bytes: Vec<u8>,
}

/// Provenance retained for an inactive reference without declaring a consumer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct E2InactiveResourceProvenance {
    pub id: String,
    pub feature: E2Feature,
    pub kind: ProjectResourceKind,
    pub original_path: String,
}

/// Complete read-only output consumed by the import transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct E2ResourceCopyPlan {
    pub fingerprint: String,
    pub copies: Vec<E2PlannedCopy>,
    pub inactive: Vec<E2InactiveResourceProvenance>,
}

/// Resource preflight either owns all required bytes or asks for explicit relinks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum E2ResourceResolution {
    Ready(E2ResourceCopyPlan),
    NeedsRelink {
        fingerprint: String,
        needs: Vec<E2RelinkNeed>,
    },
}

/// Failures distinct from recoverable relink requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum E2ResourceResolveError {
    Cancelled,
    Fatal(E2Issue),
}

pub(super) fn push_copy(
    copies: &mut Vec<E2PlannedCopy>,
    destination_keys: &mut BTreeSet<String>,
    id: &str,
    feature: E2Feature,
    kind: ProjectResourceKind,
    original_path: &str,
    selected: SelectedFile,
) -> Result<(), E2ResourceResolveError> {
    let destination = owned_destination(id, kind, &selected.extension, &selected.sha256);
    if !destination_keys.insert(case_fold(&destination)) {
        return Err(invalid("owned resource destinations collide", None));
    }
    let (consumer, build_destination) = consumer_destination(id, kind, &destination);
    let size = u64::try_from(selected.bytes.len())
        .map_err(|_| invalid("resource size cannot be represented", None))?;
    copies.push(E2PlannedCopy {
        resource: ProjectResource {
            id: id.to_string(),
            path: destination,
            kind,
            size,
            sha256: selected.sha256,
            consumer,
            build_destination,
            provenance: ProjectResourceProvenance {
                feature,
                original_path: if selected.display_path.is_empty() {
                    original_path.to_string()
                } else {
                    selected.display_path.clone()
                },
            },
        },
        bytes: selected.bytes,
    });
    Ok(())
}

pub(super) fn relink_choices(
    choices: &[E2RelinkChoice],
) -> Result<BTreeMap<&str, &str>, E2ResourceResolveError> {
    let mut result = BTreeMap::new();
    for choice in choices {
        if choice.need_id.is_empty() || choice.path.is_empty() || choice.path.contains('\0') {
            return Err(invalid("relink choice is malformed", None));
        }
        if result
            .insert(choice.need_id.as_str(), choice.path.as_str())
            .is_some()
        {
            return Err(invalid("relink need id is duplicated", None));
        }
    }
    Ok(result)
}

fn owned_destination(id: &str, kind: ProjectResourceKind, extension: &str, hash: &str) -> String {
    if kind == ProjectResourceKind::Map {
        return format!("maps/source.{extension}");
    }
    let folder = match kind {
        ProjectResourceKind::Audio => "audio",
        ProjectResourceKind::Bin => "bin",
        ProjectResourceKind::Tile => "tiles",
        ProjectResourceKind::Plugin => "plugins",
        ProjectResourceKind::Dat | ProjectResourceKind::File => "files",
        ProjectResourceKind::Map => "maps",
    };
    let slug = id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    let prefix = hash.get(..12).unwrap_or(hash);
    format!("resources/e2/{folder}/{slug}-{prefix}.{extension}")
}

fn consumer_destination(
    id: &str,
    kind: ProjectResourceKind,
    owned_path: &str,
) -> (ProjectResourceConsumer, String) {
    match kind {
        ProjectResourceKind::Map => (ProjectResourceConsumer::SourceMap, owned_path.to_string()),
        ProjectResourceKind::Audio => (
            ProjectResourceConsumer::Mpq,
            format!(
                "M{}_0.ogg",
                id.strip_prefix("SoundPlayerSET:").unwrap_or(id)
            ),
        ),
        ProjectResourceKind::Bin => (
            ProjectResourceConsumer::BinCompat,
            "src/e2/bin_compat.eps".to_string(),
        ),
        ProjectResourceKind::Tile => (
            ProjectResourceConsumer::TileCompat,
            "src/e2/tile_compat.eps".to_string(),
        ),
        ProjectResourceKind::Plugin => (ProjectResourceConsumer::Plugin, owned_path.to_string()),
        ProjectResourceKind::Dat => (
            ProjectResourceConsumer::Dat,
            "dat/e2-resources.json".to_string(),
        ),
        ProjectResourceKind::File => (
            ProjectResourceConsumer::FileManager,
            "src/e2/file_manager_compat.py".to_string(),
        ),
    }
}
