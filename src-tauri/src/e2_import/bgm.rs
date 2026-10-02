mod eps;
mod process;
#[cfg(test)]
#[path = "bgm/tests.rs"]
mod test_support;

use std::fs;
use std::path::{Path, PathBuf};

use crate::config::DataDirs;
use sha2::{Digest, Sha256};

use super::resources::{E2PlannedCopy, E2ResourceCopyPlan};
use super::{
    E2Activation, E2BgmPlan, E2BgmSegment, E2BgmTrack, E2CompiledSources, E2Document, E2Feature,
    E2Issue, E2IssueCode, E2IssueSeverity, E2MpqAsset, ProjectResource, ProjectResourceConsumer,
    ProjectResourceKind, ProjectResourceProvenance,
};

pub const LIFECYCLE_CALL: &str = "BGM.Player();";

pub struct E2BgmRequest<'a> {
    pub document: &'a E2Document,
    pub resources: &'a E2ResourceCopyPlan,
    pub data_dirs: &'a DataDirs,
    pub temp_root: &'a Path,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct E2BgmOutput {
    pub plan: E2BgmPlan,
    pub sources: E2CompiledSources,
    pub generated_copies: Vec<E2PlannedCopy>,
    pub mpq_assets: Vec<E2MpqAsset>,
    pub lifecycle_call: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum E2BgmError {
    Cancelled,
    Fatal(E2Issue),
}

pub fn convert_bgm(
    request: E2BgmRequest<'_>,
    is_cancelled: impl Fn() -> bool,
) -> Result<Option<E2BgmOutput>, E2BgmError> {
    if is_cancelled() {
        return Err(E2BgmError::Cancelled);
    }
    if !request.document.features.iter().any(|state| {
        state.feature == E2Feature::SoundBgmReferences && state.activation == E2Activation::Active
    }) {
        return Ok(None);
    }
    if request.document.settings.sound_interval == 0 {
        return Err(E2BgmError::Fatal(invalid(
            "active BGM requires a nonzero Soundinterval",
        )));
    }
    let mut inputs = bgm_inputs(request.resources)?;
    inputs.sort_by_key(|(track, _)| *track);
    if inputs.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err(E2BgmError::Fatal(invalid("duplicate BGM track number")));
    }
    let tools = crate::bootstrap::resolve_managed_ffmpeg(request.data_dirs).map_err(|_| {
        E2BgmError::Fatal(invalid("managed audio tools are unavailable or invalid"))
    })?;
    let temp = OwnedTemp::create(request.temp_root)?;
    let mut tracks = Vec::with_capacity(inputs.len());
    let mut generated_copies = Vec::new();
    let mut mpq_assets = Vec::new();
    for (track_number, input) in inputs {
        validate_input(input)?;
        let converted = process::convert_track(process::ConvertTrackRequest {
            tools: &tools,
            temp: temp.path(),
            track: track_number,
            source: &input.bytes,
            sound_interval: request.document.settings.sound_interval,
            is_cancelled: &is_cancelled,
        })?;
        let mut segments = Vec::with_capacity(converted.segments.len());
        for (index, bytes) in converted.segments.into_iter().enumerate() {
            let index = u32::try_from(index)
                .map_err(|_| E2BgmError::Fatal(invalid("BGM segment index overflow")))?;
            let alias = format!("M{track_number}_{index}.ogg");
            let resource_id = format!("{}:segment:{index}", input.resource.id);
            let sha256 = format!("{:x}", Sha256::digest(&bytes));
            let path = format!(
                "resources/e2/audio/bgm-{track_number}-{index}-{}.ogg",
                &sha256[..12]
            );
            let size = u64::try_from(bytes.len())
                .map_err(|_| E2BgmError::Fatal(invalid("BGM segment size overflow")))?;
            generated_copies.push(E2PlannedCopy {
                resource: ProjectResource {
                    id: resource_id.clone(),
                    path,
                    kind: ProjectResourceKind::Audio,
                    size,
                    sha256,
                    consumer: ProjectResourceConsumer::Mpq,
                    build_destination: alias.clone(),
                    provenance: ProjectResourceProvenance {
                        feature: E2Feature::SoundBgmReferences,
                        original_path: format!("generated:{}#{alias}", input.resource.id),
                    },
                },
                bytes,
            });
            mpq_assets.push(E2MpqAsset {
                alias: alias.clone(),
                resource_id: resource_id.clone(),
            });
            segments.push(E2BgmSegment {
                index,
                resource_id,
                alias,
            });
        }
        tracks.push(E2BgmTrack {
            music_number: track_number,
            looped: false,
            segments,
        });
    }
    let plan = E2BgmPlan {
        sound_interval: request.document.settings.sound_interval,
        tracks,
    };
    let sources = eps::render(&plan, super::compiler::render_tct_helpers());
    Ok(Some(E2BgmOutput {
        plan,
        sources,
        generated_copies,
        mpq_assets,
        lifecycle_call: LIFECYCLE_CALL.to_string(),
    }))
}

fn bgm_inputs(plan: &E2ResourceCopyPlan) -> Result<Vec<(u32, &E2PlannedCopy)>, E2BgmError> {
    plan.copies
        .iter()
        .filter(|copy| {
            copy.resource.kind == ProjectResourceKind::Audio
                && copy.resource.consumer == ProjectResourceConsumer::Mpq
                && copy.resource.provenance.feature == E2Feature::SoundBgmReferences
        })
        .map(|copy| {
            let suffix = copy
                .resource
                .id
                .strip_prefix("SoundPlayerSET:")
                .ok_or_else(|| E2BgmError::Fatal(invalid("active BGM resource id is invalid")))?;
            let track = suffix
                .parse::<u32>()
                .map_err(|_| E2BgmError::Fatal(invalid("active BGM track number is invalid")))?;
            Ok((track, copy))
        })
        .collect()
}

fn validate_input(input: &E2PlannedCopy) -> Result<(), E2BgmError> {
    let size = u64::try_from(input.bytes.len())
        .map_err(|_| E2BgmError::Fatal(invalid("BGM input size overflow")))?;
    let sha256 = format!("{:x}", Sha256::digest(&input.bytes));
    if size != input.resource.size || sha256 != input.resource.sha256 {
        return Err(E2BgmError::Fatal(invalid(
            "owned BGM input metadata does not match its bytes",
        )));
    }
    Ok(())
}

struct OwnedTemp {
    path: PathBuf,
}

impl OwnedTemp {
    fn create(root: &Path) -> Result<Self, E2BgmError> {
        let owned = Self {
            path: root.join(format!("owned-bgm-{}", uuid::Uuid::new_v4())),
        };
        fs::create_dir(&owned.path).map_err(|error| {
            E2BgmError::Fatal(invalid(format!("cannot create BGM temp: {error}")))
        })?;
        Ok(owned)
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for OwnedTemp {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.path) {
            eprintln!("failed to remove owned BGM temp: {error}");
        }
    }
}

fn invalid(message: impl Into<String>) -> E2Issue {
    E2Issue {
        code: E2IssueCode::InvalidResource,
        severity: E2IssueSeverity::Fatal,
        message: message.into(),
        path: None,
        feature: Some(E2Feature::SoundBgmReferences),
        need_id: None,
    }
}

#[cfg(test)]
#[test]
fn managed_conversion_and_player_oracle() {
    test_support::managed_conversion_and_player_oracle();
}

#[cfg(test)]
#[test]
fn failure_and_cancel_cleanup() {
    test_support::failure_and_cancel_cleanup();
}
