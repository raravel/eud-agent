use std::cell::Cell;

use crate::bootstrap::process_tree::ProcessCancellation;
use crate::e2_import::{E2ImportPhase, E2ImportResult, E2PreflightResult};
use crate::native_project::NativeProject;

use super::scenario_fixture::ScenarioFixture;

pub(super) fn run() {
    // Given: a real E2 source, sibling input map, and still-empty destination.
    let fixture = ScenarioFixture::create("success");
    let protected_hashes = fixture.source_hashes();
    let preflight = super::preflight(&fixture.preflight_request());
    let fingerprint = match preflight {
        E2PreflightResult::Ready { fingerprint, .. } => fingerprint,
        other => panic!("preflight must be ready, found {other:?}"),
    };
    let request = fixture.import_request(fingerprint.clone());
    let cancellation = ProcessCancellation::default();
    let activation_count = Cell::new(0_u32);
    let mut progress = Vec::new();
    let manager = crate::native_runtime::NativeProjectManager::new(fixture.dirs.clone());

    // When: the transaction stages, publishes, reopens, and activates the project.
    let result = super::import(
        "import-success",
        &request,
        &fixture.dirs,
        &fixture.compat_root(),
        &cancellation,
        |event| progress.push(event.clone()),
        |project| {
            activation_count.set(activation_count.get() + 1);
            manager.activate_project(project)
        },
    );

    // Then: completion is real on disk, ordered, hash-bound, and activated once.
    let canonical_destination = std::fs::canonicalize(&fixture.destination)
        .expect("published destination must canonicalize")
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        result,
        E2ImportResult::Completed {
            import_id: "import-success".to_string(),
            fingerprint,
            project_opened: true,
            project_path: canonical_destination.clone(),
        }
    );
    assert_eq!(activation_count.get(), 1);
    let config = fixture.dirs.load_config().expect("activation saves config");
    assert_eq!(config.project_path, canonical_destination);
    assert_eq!(config.project_recents.len(), 1);
    assert_eq!(config.project_recents[0].path, config.project_path);
    let reopened =
        NativeProject::open(&fixture.destination).expect("published project must reopen");
    let snapshot = reopened
        .resource_snapshot()
        .expect("published resource snapshot must validate");
    assert_eq!(snapshot.resources.len(), 1);
    assert!(!reopened.revision().expect("revision must exist").is_empty());
    assert_eq!(fixture.source_hashes(), protected_hashes);
    let phases = progress
        .iter()
        .filter_map(|event| match event {
            E2ImportResult::Progress { phase, .. } => Some(*phase),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        phases,
        vec![
            E2ImportPhase::Preflight,
            E2ImportPhase::Resolving,
            E2ImportPhase::Compiling,
            E2ImportPhase::CollectingResources,
            E2ImportPhase::Materializing,
            E2ImportPhase::Validating,
            E2ImportPhase::Publishing,
            E2ImportPhase::Opening,
            E2ImportPhase::Completed,
        ]
    );
    assert_eq!(
        std::fs::read(fixture.destination.join("maps/source.scx"))
            .expect("immutable map copy must exist"),
        b"literal-map-bytes\0task-12"
    );
    assert!(fixture.destination.join("src/main.eps").is_file());
    assert!(fixture.destination.join("src/e2/tct.py").is_file());
    assert!(!fixture.destination.join("build").exists());
    assert!(!fixture
        .root
        .join(".destination.e2-stage-import-success")
        .exists());
}
