use std::cell::Cell;
use std::fs;

use crate::bootstrap::process_tree::ProcessCancellation;
use crate::config::Config;
use crate::e2_import::{E2ImportResult, E2IssueCode, E2PreflightResult};

use super::scenario_fixture::ScenarioFixture;

pub(super) fn run() {
    nonempty_destination_preserves_state();
    concurrent_directory_entry_is_preserved_without_cleanup_error();
}

fn concurrent_directory_entry_is_preserved_without_cleanup_error() {
    let fixture = ScenarioFixture::create("concurrent-directory-entry");
    let stage = fixture.root.join("direct-stage");
    fs::create_dir(&stage).unwrap();
    fs::write(stage.join("owned.txt"), b"published").unwrap();
    let token = ProcessCancellation::default();
    let mut receipt = super::publication::publish(&stage, &fixture.destination, &token).unwrap();
    let concurrent = fixture.destination.join("arrival.txt");
    fs::write(&concurrent, b"concurrent").unwrap();
    assert_eq!(receipt.rollback(), None);
    assert!(!fixture.destination.join("owned.txt").exists());
    assert_eq!(fs::read(concurrent).unwrap(), b"concurrent");
}

fn nonempty_destination_preserves_state() {
    let fixture = ScenarioFixture::create("nonempty-destination");
    let fingerprint = match super::preflight(&fixture.preflight_request()) {
        E2PreflightResult::Ready { fingerprint, .. } => fingerprint,
        other => panic!("empty fixture destination must be ready, found {other:?}"),
    };
    let prior_root = fixture.root.join("prior-active");
    fs::create_dir(&prior_root).unwrap();
    fs::write(prior_root.join("sentinel.bin"), b"prior-state").unwrap();
    let mut prior = Config {
        project_path: prior_root.to_string_lossy().into_owned(),
        ..Default::default()
    };
    prior.record_recent_project("Prior".to_string(), prior.project_path.clone(), 17);
    fixture.dirs.save_config(&prior).unwrap();
    let protected_hashes = fixture.source_hashes();
    let sentinel = fixture.destination.join("concurrent-sentinel.txt");
    fs::write(&sentinel, b"preserve").unwrap();
    let activation_count = Cell::new(0_u32);

    let result = super::import(
        "nonempty-destination",
        &fixture.import_request(fingerprint),
        &fixture.dirs,
        &fixture.compat_root(),
        &ProcessCancellation::default(),
        |_| {},
        |_| {
            activation_count.set(activation_count.get() + 1);
            Ok(())
        },
    );

    let E2ImportResult::Fatal { issues, .. } = result else {
        panic!("nonempty destination must fail, found {result:?}");
    };
    assert_eq!(issues[0].code, E2IssueCode::MalformedInput);
    assert!(issues[0].message.contains("destination must remain empty"));
    assert_eq!(activation_count.get(), 0);
    assert_eq!(fixture.dirs.load_config().unwrap(), prior);
    assert_eq!(fs::read(&sentinel).unwrap(), b"preserve");
    assert_eq!(fixture.source_hashes(), protected_hashes);
    assert!(!fixture
        .root
        .join(".destination.e2-stage-nonempty-destination")
        .exists());
}
