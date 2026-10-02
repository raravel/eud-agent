use std::fs;

use crate::bootstrap::process_tree::ProcessCancellation;
use crate::e2_import::{
    E2ImportPhase, E2ImportResult, E2IssueCode, E2PreflightResult, E2RelinkChoice,
};

use super::scenario_fixture::ScenarioFixture;

pub(super) fn run() {
    stale_fingerprint_preserves_everything();
    relink_bytes_are_bound_to_fingerprint();
    malformed_source_is_typed_and_read_only();
    unsupported_active_is_fatal_but_inactive_is_ready();
    every_interruptible_phase_cleans_up();
    concurrent_arrival_is_preserved();
    changed_receipt_file_is_preserved();
    locked_cleanup_retains_primary_cause();
    activation_failure_preserves_prior_config();
}

fn ready_fingerprint(fixture: &ScenarioFixture) -> String {
    match super::preflight(&fixture.preflight_request()) {
        E2PreflightResult::Ready { fingerprint, .. } => fingerprint,
        other => panic!("fixture preflight must be ready, found {other:?}"),
    }
}

fn stale_fingerprint_preserves_everything() {
    let fixture = ScenarioFixture::create("stale");
    let sentinel = fixture.destination.join("concurrent-sentinel.txt");
    fs::write(&sentinel, b"concurrent arrival").unwrap();
    let result = run_import(&fixture, fixture.import_request("0".repeat(64)), |_| {});
    let E2ImportResult::Fatal { issues, .. } = result else {
        panic!()
    };
    assert_eq!(issues[0].code, E2IssueCode::SourceChanged);
    assert_eq!(fs::read(sentinel).unwrap(), b"concurrent arrival");
}

fn relink_bytes_are_bound_to_fingerprint() {
    let fixture = ScenarioFixture::create("relink-stale");
    fs::remove_file(&fixture.input_map).unwrap();
    let E2PreflightResult::NeedsRelink { needs, .. } =
        super::preflight(&fixture.preflight_request())
    else {
        panic!("missing map must request relink")
    };
    let relocated = fixture.source.with_file_name("relocated.scx");
    fs::write(&relocated, b"relocated-map-v1").unwrap();
    let choice = E2RelinkChoice {
        need_id: needs[0].id.clone(),
        path: relocated.to_string_lossy().into_owned(),
    };
    let mut preflight = fixture.preflight_request();
    preflight.relinks.push(choice.clone());
    let E2PreflightResult::Ready { fingerprint, .. } = super::preflight(&preflight) else {
        panic!("manual relink must be ready")
    };
    fs::write(&relocated, b"relocated-map-v2").unwrap();
    let mut request = fixture.import_request(fingerprint);
    request.relinks.push(choice);
    let result = run_import(&fixture, request, |_| {});
    assert!(matches!(result, E2ImportResult::Fatal { ref issues, .. }
        if issues[0].code == E2IssueCode::SourceChanged));
    assert_eq!(fs::read_dir(&fixture.destination).unwrap().count(), 0);
}

fn malformed_source_is_typed_and_read_only() {
    let fixture = ScenarioFixture::create("malformed");
    fs::write(&fixture.source, b"not an E2 project").unwrap();
    let protected_hashes = fixture.source_hashes();
    let result = super::preflight(&fixture.preflight_request());
    assert!(matches!(result, E2PreflightResult::Fatal { ref issues, .. }
        if issues[0].code == E2IssueCode::MalformedInput));
    assert_eq!(fixture.source_hashes(), protected_hashes);
    assert_eq!(fs::read_dir(&fixture.destination).unwrap().count(), 0);
}

fn unsupported_active_is_fatal_but_inactive_is_ready() {
    let fixture = ScenarioFixture::create("unsupported-active");
    let base = fs::read_to_string(&fixture.source).unwrap();
    let trigger = |disabled| {
        format!(
        "S_TriggerEditorSET\r\nType:8,{disabled},False,False\r\nact:SCDB:Exec\r\nvalue\r\nElementsCount:0\r\nEND\r\nE_TriggerEditorSET\r\n"
    )
    };
    let active = base.replace("UsedSetting6 : False", "UsedSetting6 : True") + &trigger("False");
    fs::write(&fixture.source, active).unwrap();
    assert!(matches!(super::preflight(&fixture.preflight_request()),
        E2PreflightResult::Fatal { ref issues, .. }
        if issues[0].code == E2IssueCode::ActiveScdbUnsupported));
    fs::write(&fixture.source, base + &trigger("True")).unwrap();
    assert!(matches!(
        super::preflight(&fixture.preflight_request()),
        E2PreflightResult::Ready { .. }
    ));
}

fn every_interruptible_phase_cleans_up() {
    let phases = [
        E2ImportPhase::Preflight,
        E2ImportPhase::Resolving,
        E2ImportPhase::Compiling,
        E2ImportPhase::CollectingResources,
        E2ImportPhase::Materializing,
        E2ImportPhase::Validating,
        E2ImportPhase::Publishing,
        E2ImportPhase::Opening,
    ];
    for phase in phases {
        let fixture = ScenarioFixture::create(&format!("cancel-{phase:?}"));
        let fingerprint = ready_fingerprint(&fixture);
        let cancellation = ProcessCancellation::default();
        let result = super::import(
            "cancel-phase",
            &fixture.import_request(fingerprint),
            &fixture.dirs,
            &fixture.compat_root(),
            &cancellation,
            |event| {
                if matches!(event, E2ImportResult::Progress { phase: found, .. } if *found == phase)
                {
                    cancellation.cancel();
                    cancellation.cancel();
                    cancellation.cancel();
                }
            },
            |_| panic!("cancelled import must not activate"),
        );
        assert!(
            matches!(result, E2ImportResult::Cancelled { .. }),
            "{phase:?}"
        );
        assert_eq!(
            fs::read_dir(&fixture.destination).unwrap().count(),
            0,
            "{phase:?}"
        );
        assert!(!fixture
            .root
            .join(".destination.e2-stage-cancel-phase")
            .exists());
    }
}

fn concurrent_arrival_is_preserved() {
    let fixture = ScenarioFixture::create("concurrent-arrival");
    let fingerprint = ready_fingerprint(&fixture);
    let sentinel = fixture.destination.join("arrival.txt");
    let result = run_import(&fixture, fixture.import_request(fingerprint), |event| {
        if matches!(
            event,
            E2ImportResult::Progress {
                phase: E2ImportPhase::Validating,
                ..
            }
        ) {
            fs::write(&sentinel, b"user-arrival").unwrap();
        }
    });
    assert!(matches!(result, E2ImportResult::Fatal { .. }));
    assert_eq!(fs::read(sentinel).unwrap(), b"user-arrival");
}

fn changed_receipt_file_is_preserved() {
    let fixture = ScenarioFixture::create("changed-receipt");
    let stage = fixture.root.join("direct-stage");
    fs::create_dir(&stage).unwrap();
    fs::write(stage.join("owned.txt"), b"published").unwrap();
    let token = ProcessCancellation::default();
    let mut receipt = super::publication::publish(&stage, &fixture.destination, &token).unwrap();
    fs::write(fixture.destination.join("owned.txt"), b"user-change").unwrap();
    fs::write(fixture.destination.join("arrival.txt"), b"concurrent").unwrap();
    let cleanup = receipt.rollback().expect("changed file must be reported");
    assert!(cleanup.contains("changed and was preserved"));
    assert_eq!(
        fs::read(fixture.destination.join("owned.txt")).unwrap(),
        b"user-change"
    );
    assert_eq!(
        fs::read(fixture.destination.join("arrival.txt")).unwrap(),
        b"concurrent"
    );
}

#[cfg(windows)]
fn locked_cleanup_retains_primary_cause() {
    use std::os::windows::fs::OpenOptionsExt;
    let fixture = ScenarioFixture::create("locked-cleanup");
    let stage = fixture.root.join("locked-stage");
    fs::create_dir(&stage).unwrap();
    fs::write(stage.join("owned.txt"), b"published").unwrap();
    let token = ProcessCancellation::default();
    let receipt = super::publication::publish(&stage, &fixture.destination, &token).unwrap();
    let _lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(0)
        .open(fixture.destination.join("owned.txt"))
        .unwrap();
    let result = super::rollback_failure(
        receipt,
        "locked",
        &"a".repeat(64),
        "primary failure".to_string(),
    );
    let E2ImportResult::Fatal { issues, .. } = result else {
        panic!()
    };
    assert!(issues[0].message.contains("primary failure"));
    assert!(issues[0].message.contains("cleanup failed"));
}

#[cfg(not(windows))]
fn locked_cleanup_retains_primary_cause() {}

fn activation_failure_preserves_prior_config() {
    let fixture = ScenarioFixture::create("activation-failure");
    let fingerprint = ready_fingerprint(&fixture);
    let prior_root = fixture.root.join("prior-active");
    fs::create_dir(&prior_root).unwrap();
    fs::write(prior_root.join("sentinel.bin"), b"prior-state").unwrap();
    let mut prior = crate::config::Config {
        project_path: prior_root.to_string_lossy().into_owned(),
        ..Default::default()
    };
    prior.record_recent_project("Prior".to_string(), prior.project_path.clone(), 7);
    fixture.dirs.save_config(&prior).unwrap();
    let token = ProcessCancellation::default();
    let result = super::import(
        "activation-failure",
        &fixture.import_request(fingerprint),
        &fixture.dirs,
        &fixture.compat_root(),
        &token,
        |_| {},
        |_| Err("activation primary".to_string()),
    );
    assert!(matches!(result, E2ImportResult::Fatal { .. }));
    assert_eq!(fixture.dirs.load_config().unwrap(), prior);
    assert_eq!(
        fs::read(prior_root.join("sentinel.bin")).unwrap(),
        b"prior-state"
    );
    assert_eq!(fs::read_dir(&fixture.destination).unwrap().count(), 0);
}

fn run_import(
    fixture: &ScenarioFixture,
    request: crate::e2_import::E2ImportRequest,
    emit: impl FnMut(&E2ImportResult),
) -> E2ImportResult {
    let token = ProcessCancellation::default();
    super::import(
        "failure-case",
        &request,
        &fixture.dirs,
        &fixture.compat_root(),
        &token,
        emit,
        |_| panic!("failure scenario must not activate"),
    )
}
