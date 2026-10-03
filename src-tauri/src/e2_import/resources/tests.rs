use std::collections::{BTreeMap, BTreeSet};
use std::fs as test_fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};

use super::*;
use crate::e2_import::{
    E2FeatureState, E2ProjectSettings, E2ResourceReference, E2SourceKind, E2Version,
};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct FixtureRoot(PathBuf);

fn register_fixture(root: &Path) {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root");
    let mut ledger = test_fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(workspace.join("target/task7-fixture-registry.txt"))
        .expect("open fixture registry");
    ledger
        .write_all(format!("{}\n", root.display()).as_bytes())
        .expect("register fixture root");
}

impl FixtureRoot {
    fn new(label: &str) -> Self {
        let sequence = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root");
        let root = workspace
            .join("target/task7-fixtures")
            .join(format!("{}-{label}-{sequence}", std::process::id()));
        register_fixture(&root);
        test_fs::create_dir_all(&root).expect("create registered fixture root");
        Self(root)
    }

    fn write(&self, relative: &str, bytes: &[u8]) -> PathBuf {
        let path = self
            .0
            .join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
        test_fs::create_dir_all(path.parent().expect("fixture file parent"))
            .expect("create fixture parent");
        test_fs::write(&path, bytes).expect("write fixture file");
        path
    }
}

impl Drop for FixtureRoot {
    fn drop(&mut self) {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root");
        if self.0.starts_with(workspace.join("target/task7-fixtures")) {
            test_fs::remove_dir_all(&self.0).expect("remove exact fixture root");
        }
    }
}

fn reference(
    id: &str,
    feature: E2Feature,
    kind: ProjectResourceKind,
    source_path: &str,
) -> E2ResourceReference {
    E2ResourceReference {
        id: id.to_string(),
        feature,
        kind,
        source_path: source_path.to_string(),
    }
}

fn document(kind: E2SourceKind, input_map: &str) -> E2Document {
    let feature = |feature, activation| E2FeatureState {
        feature,
        activation,
        raw_payload: None,
    };
    E2Document {
        version: E2Version::V0_17_9_7,
        source_kind: kind,
        settings: E2ProjectSettings {
            name: "resources".to_string(),
            input_map: input_map.to_string(),
            output_map: "output.scx".to_string(),
            sound_interval: 1,
            raw_fields: BTreeMap::new(),
        },
        features: vec![
            feature(E2Feature::ProjectMap, E2Activation::Active),
            feature(E2Feature::SoundBgmReferences, E2Activation::Active),
            feature(E2Feature::Bin, E2Activation::Active),
            feature(E2Feature::CustomTiles, E2Activation::Active),
            feature(E2Feature::Plugin, E2Activation::Active),
            feature(E2Feature::FileManager, E2Activation::Active),
            feature(E2Feature::DisabledPayload, E2Activation::Inert),
        ],
        triggers: Vec::new(),
        resources: vec![
            reference(
                "SoundPlayerSET:0",
                E2Feature::SoundBgmReferences,
                ProjectResourceKind::Audio,
                "music.wav",
            ),
            reference(
                "BinEditSET:DataFile",
                E2Feature::Bin,
                ProjectResourceKind::Bin,
                "payload.bin",
            ),
            reference(
                "TileSET:ProjectTileSetFileName",
                E2Feature::CustomTiles,
                ProjectResourceKind::Tile,
                "tile.bmp",
            ),
            reference(
                "PluginSET:0",
                E2Feature::Plugin,
                ProjectResourceKind::Plugin,
                "plugin.eps",
            ),
            reference(
                "FileManagerSET:TBL",
                E2Feature::FileManager,
                ProjectResourceKind::File,
                "stat.tbl",
            ),
            reference(
                "Disabled:legacy",
                E2Feature::DisabledPayload,
                ProjectResourceKind::File,
                "absent.tbl",
            ),
        ],
        raw_sections: Vec::new(),
    }
}

fn map_only_document(kind: E2SourceKind, input_map: &str) -> E2Document {
    let mut value = document(kind, input_map);
    value.features.truncate(1);
    value.resources.clear();
    value
}

fn write_active_files(root: &FixtureRoot, prefix: &str) {
    for (name, bytes) in [
        ("payload.bin", b"bin".as_slice()),
        ("tile.bmp", b"tile".as_slice()),
        ("plugin.eps", b"plugin".as_slice()),
        ("stat.tbl", b"table".as_slice()),
    ] {
        root.write(&format!("{prefix}{name}"), bytes);
    }
}

fn ready(
    source_path: &Path,
    document: &E2Document,
    relinks: &[E2RelinkChoice],
    expected_fingerprint: Option<&str>,
) -> E2ResourceCopyPlan {
    let resolved = resolve_copy_plan(
        E2ResourceResolveRequest {
            source_path,
            document,
            relinks,
            expected_fingerprint,
        },
        || false,
    )
    .expect("resolution must succeed");
    let E2ResourceResolution::Ready(plan) = resolved else {
        panic!("complete fixture must be ready");
    };
    plan
}

pub(super) fn resolution_and_inventory_matrix_scenario() {
    // Given: independent E2S, E2P, and stored-path fixtures with real resource bytes.
    let e2s = FixtureRoot::new("e2s");
    let e2s_source = e2s.write("project.e2s", b"source");
    e2s.write("basemap.scx", b"e2s-map");
    e2s.write("music.wav", b"e2s-audio");
    write_active_files(&e2s, "");
    let sentinel = e2s.write("destination/sentinel.txt", b"preserve");

    let e2p = FixtureRoot::new("e2p");
    let e2p_source = e2p.write("project.e2p", b"source");
    e2p.write("Map/basemap.scx", b"e2p-map");
    e2p.write("Sound/music.wav", b"e2p-audio");
    write_active_files(&e2p, "Resource/");

    let stored = FixtureRoot::new("stored");
    let stored_source = stored.write("project.e2s", b"source");
    let stored_map = stored.write("chosen.scx", b"stored-map");
    stored.write("music.wav", b"stored-audio");
    write_active_files(&stored, "");

    // When: each source is resolved into a byte-owned plan without a destination write.
    let cases = [
        (e2s_source, document(E2SourceKind::E2s, "basemap.scx")),
        (e2p_source, document(E2SourceKind::E2p, "basemap.scx")),
        (
            stored_source,
            document(E2SourceKind::E2s, &stored_map.to_string_lossy()),
        ),
    ];
    for (source_path, document) in cases {
        let first = ready(&source_path, &document, &[], None);
        let second = ready(&source_path, &document, &[], Some(&first.fingerprint));

        // Then: every active non-GRP byte has one unique confined destination and consumer.
        assert_eq!(first, second);
        assert_eq!(first.copies.len(), 6);
        assert_eq!(first.inactive.len(), 1);
        let mut destinations = BTreeSet::new();
        for copy in &first.copies {
            assert!(!copy.bytes.is_empty());
            assert_eq!(copy.resource.size, copy.bytes.len() as u64);
            assert_eq!(
                copy.resource.sha256,
                format!("{:x}", Sha256::digest(&copy.bytes))
            );
            assert!(!Path::new(&copy.resource.path).is_absolute());
            assert!(
                copy.resource.path.starts_with("maps/")
                    || copy.resource.path.starts_with("resources/e2/")
            );
            assert!(destinations.insert(copy.resource.path.to_lowercase()));
            assert!(!copy.resource.build_destination.is_empty());
            assert!(Path::new(&copy.resource.provenance.original_path).is_absolute());
        }
        assert!(first.copies.iter().any(|copy| {
            copy.resource.id == "SoundPlayerSET:0"
                && copy.resource.build_destination == "M0_0.ogg"
                && copy.resource.consumer == ProjectResourceConsumer::Mpq
        }));
    }
    assert_eq!(test_fs::read(sentinel).expect("read sentinel"), b"preserve");
    packaged_audio::run();
    cancellation::run();
}

#[path = "tests/packaged_audio.rs"]
mod packaged_audio;

#[path = "tests/cancellation.rs"]
mod cancellation;

#[path = "tests/failures.rs"]
mod failures;

pub(super) fn relink_and_confinement_failures_scenario() {
    failures::run();
}
