use std::cell::Cell;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::config::DataDirs;

use super::super::resources::{E2PlannedCopy, E2ResourceCopyPlan};
use super::super::{
    E2Activation, E2Document, E2Feature, E2FeatureState, E2ProjectSettings, E2SourceKind,
    E2Version, ProjectResource, ProjectResourceConsumer, ProjectResourceKind,
    ProjectResourceProvenance,
};
use super::{convert_bgm, process, E2BgmError, E2BgmRequest, LIFECYCLE_CALL};

struct TestRoot(PathBuf);

impl TestRoot {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!("e2-bgm-{tag}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).expect("create test root");
        Self(path)
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove test root");
    }
}

fn installed_dirs() -> DataDirs {
    DataDirs::from_bases(
        Path::new(&std::env::var("APPDATA").expect("APPDATA")),
        Path::new(&std::env::var("LOCALAPPDATA").expect("LOCALAPPDATA")),
    )
}

fn document(active: bool, sound_interval: u32) -> E2Document {
    E2Document {
        version: E2Version::V0_17_9_7,
        source_kind: E2SourceKind::E2p,
        settings: E2ProjectSettings {
            name: "bgm-oracle".to_string(),
            input_map: "Map/input.scx".to_string(),
            output_map: "Map/output.scx".to_string(),
            sound_interval,
            raw_fields: BTreeMap::new(),
        },
        features: vec![E2FeatureState {
            feature: E2Feature::SoundBgmReferences,
            activation: if active {
                E2Activation::Active
            } else {
                E2Activation::Inert
            },
            raw_payload: None,
        }],
        triggers: Vec::new(),
        resources: Vec::new(),
        raw_sections: Vec::new(),
    }
}

fn copy(track: u32, bytes: &[u8]) -> E2PlannedCopy {
    let sha256 = format!("{:x}", Sha256::digest(bytes));
    E2PlannedCopy {
        resource: ProjectResource {
            id: format!("SoundPlayerSET:{track}"),
            path: format!("resources/e2/audio/track-{track}-{}.ogg", &sha256[..12]),
            kind: ProjectResourceKind::Audio,
            size: u64::try_from(bytes.len()).expect("fixture size"),
            sha256,
            consumer: ProjectResourceConsumer::Mpq,
            build_destination: format!("M{track}_0.ogg"),
            provenance: ProjectResourceProvenance {
                feature: E2Feature::SoundBgmReferences,
                original_path: format!("C:/missing/track-{track}.mp3"),
            },
        },
        bytes: bytes.to_vec(),
    }
}

fn plan(copies: Vec<E2PlannedCopy>) -> E2ResourceCopyPlan {
    E2ResourceCopyPlan {
        fingerprint: "bgm-fixture".to_string(),
        copies,
        inactive: Vec::new(),
    }
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(super) fn managed_conversion_and_player_oracle() {
    // Given: active BGM rows whose whole-track bytes are already owned by the copy plan.
    let root = TestRoot::new("managed");
    let source_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates/isom/tests/fixtures/tone.ogg");
    let source = fs::read(&source_path).expect("read real OGG fixture");
    let source_hash = sha256(&source);
    let mut unrelated = copy(9, &source);
    unrelated.resource.id = "ProjectSET:InputMap".to_string();
    unrelated.resource.kind = ProjectResourceKind::Map;
    unrelated.resource.consumer = ProjectResourceConsumer::SourceMap;
    unrelated.resource.provenance.feature = E2Feature::ProjectMap;
    let resources = plan(vec![unrelated, copy(1, &source), copy(0, &source)]);
    let active_document = document(true, 1);
    let dirs = installed_dirs();

    // When: managed tools convert and segment both tracks.
    let output = convert_bgm(
        E2BgmRequest {
            document: &active_document,
            resources: &resources,
            data_dirs: &dirs,
            temp_root: &root.0,
        },
        || false,
    )
    .expect("conversion succeeds")
    .expect("active BGM emits output");

    // Then: aliases bind only generated owned bytes and the editable API is exact.
    assert_eq!(
        output
            .plan
            .tracks
            .iter()
            .map(|track| track.music_number)
            .collect::<Vec<_>>(),
        [0, 1]
    );
    assert!(output
        .generated_copies
        .iter()
        .all(|copy| copy.bytes.starts_with(b"OggS")));
    for copy in &output.generated_copies {
        assert_eq!(
            copy.resource.size,
            u64::try_from(copy.bytes.len()).expect("generated fixture size")
        );
        assert_eq!(copy.resource.sha256, sha256(&copy.bytes));
    }
    assert!(output
        .mpq_assets
        .iter()
        .all(|asset| output
            .generated_copies
            .iter()
            .any(|copy| copy.resource.id == asset.resource_id
                && copy.resource.build_destination == asset.alias)));
    let bgm = &output.sources.files["src/e2/bgm.eps"];
    for name in "musicLastTime musicFrame musicnum musicflag musicisplay Player Play Stop Resume CurrentPlayMusic isplaying"
        .split_whitespace()
    {
        assert!(bgm.contains(name), "missing editable symbol {name}");
    }
    assert!(bgm.contains("0x51CE8C"));
    assert!(bgm.contains("980"));
    assert!(bgm.contains("if (tplayer < 8) { return tplayer; }"));
    assert!(bgm.contains("if (musicflag[i] == 1)"));
    assert!(bgm.contains("else if (musicflag[i] == 0)"));
    assert_eq!(
        output.sources.files["src/e2/tct.py"],
        super::super::compiler::render_tct_helpers()
    );
    assert_eq!(output.lifecycle_call, LIFECYCLE_CALL);
    assert_eq!(
        sha256(&fs::read(source_path).expect("reread fixture")),
        source_hash
    );
    let relocated = TestRoot::new("relocated");
    let repeated = convert_bgm(
        E2BgmRequest {
            document: &active_document,
            resources: &resources,
            data_dirs: &dirs,
            temp_root: &relocated.0,
        },
        || false,
    )
    .expect("relocated conversion succeeds")
    .expect("relocated active BGM emits output");
    assert_eq!(output, repeated);
}

pub(super) fn failure_and_cancel_cleanup() {
    // Given: an owned temp root with a concurrent sentinel and an invalid active track.
    let root = TestRoot::new("failures");
    let sentinel = root.0.join("concurrent.sentinel");
    fs::write(&sentinel, b"preserve").expect("write sentinel");
    let invalid_resources = plan(vec![copy(0, b"not audio")]);
    let active_document = document(true, 1);
    let dirs = installed_dirs();

    // When: invalid audio and pre-spawn cancellation are requested independently.
    let invalid = convert_bgm(
        E2BgmRequest {
            document: &active_document,
            resources: &invalid_resources,
            data_dirs: &dirs,
            temp_root: &root.0,
        },
        || false,
    );
    let cancellation_checks = Cell::new(0_u8);
    let cancellable = plan(vec![copy(
        0,
        include_bytes!("../../../../crates/isom/tests/fixtures/tone.ogg"),
    )]);
    let cancelled = convert_bgm(
        E2BgmRequest {
            document: &active_document,
            resources: &cancellable,
            data_dirs: &dirs,
            temp_root: &root.0,
        },
        || {
            cancellation_checks.set(cancellation_checks.get() + 1);
            cancellation_checks.get() >= 3
        },
    );
    let gaps = root.0.join("gaps");
    fs::create_dir(&gaps).expect("create gap fixture");
    fs::write(gaps.join("M0_0.ogg"), b"OggS").expect("write segment zero");
    fs::write(gaps.join("M0_2.ogg"), b"OggS").expect("write segment two");
    let gap = process::contiguous_segments(&gaps, 0);
    let inactive_document = document(false, 1);
    let inactive = convert_bgm(
        E2BgmRequest {
            document: &inactive_document,
            resources: &invalid_resources,
            data_dirs: &dirs,
            temp_root: &root.0,
        },
        || false,
    );
    let mut malformed_copy = copy(0, b"not audio");
    malformed_copy.resource.id = "SoundPlayerSET:unknown".to_string();
    let malformed_resources = plan(vec![malformed_copy]);
    let malformed = convert_bgm(
        E2BgmRequest {
            document: &active_document,
            resources: &malformed_resources,
            data_dirs: &dirs,
            temp_root: &root.0,
        },
        || false,
    );

    // Then: each failure is typed and cleanup preserves only unrelated state.
    assert!(matches!(invalid, Err(E2BgmError::Fatal(_))));
    assert_eq!(cancelled, Err(E2BgmError::Cancelled));
    assert!(matches!(gap, Err(E2BgmError::Fatal(_))));
    assert_eq!(inactive, Ok(None));
    assert!(matches!(malformed, Err(E2BgmError::Fatal(_))));
    assert_eq!(fs::read(sentinel).expect("read sentinel"), b"preserve");
    assert!(fs::read_dir(&root.0)
        .expect("read test root")
        .all(|entry| !entry
            .expect("read entry")
            .file_name()
            .to_string_lossy()
            .starts_with("owned-")));
}
