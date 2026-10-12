use std::fs as test_fs;

use super::*;

pub(super) fn run() {
    // Given: missing, ambiguous, excluded-output, traversal, collision, and relink fixtures.
    let missing = FixtureRoot::new("missing");
    let missing_source = missing.write("project.e2s", b"source");
    let missing_document = map_only_document(E2SourceKind::E2s, "basemap.scx");
    let relink_source = FixtureRoot::new("relink-source");
    let relink_map = relink_source.write("selected.scx", b"selected-map");
    let E2ResourceResolution::NeedsRelink { fingerprint, needs } = resolve_copy_plan(
        E2ResourceResolveRequest {
            source_path: &missing_source,
            document: &missing_document,
            relinks: &[],
            expected_fingerprint: None,
        },
        || false,
    )
    .expect("missing map is recoverable") else {
        panic!("missing map must request relink");
    };
    assert_eq!(needs.len(), 1);

    let inventory_drift = FixtureRoot::new("inventory-drift");
    let inventory_source = inventory_drift.write("project.e2s", b"source");
    let inventory_document = map_only_document(E2SourceKind::E2s, "missing.scx");
    let E2ResourceResolution::NeedsRelink {
        fingerprint: inventory_fingerprint,
        ..
    } = resolve_copy_plan(
        E2ResourceResolveRequest {
            source_path: &inventory_source,
            document: &inventory_document,
            relinks: &[],
            expected_fingerprint: None,
        },
        || false,
    )
    .expect("missing inventory map is recoverable")
    else {
        panic!("missing inventory map must request relink");
    };
    inventory_drift.write("new-candidate.scx", b"new-map");
    let inventory_changed = resolve_copy_plan(
        E2ResourceResolveRequest {
            source_path: &inventory_source,
            document: &inventory_document,
            relinks: &[],
            expected_fingerprint: Some(&inventory_fingerprint),
        },
        || false,
    );
    assert!(matches!(
        inventory_changed,
        Err(E2ResourceResolveError::Fatal(E2Issue {
            code: E2IssueCode::SourceChanged,
            ..
        }))
    ));

    let ambiguous = FixtureRoot::new("ambiguous");
    let ambiguous_source = ambiguous.write("project.e2p", b"source");
    ambiguous.write("Map/basemap.scx", b"map-one");
    ambiguous.write("basemap.scx", b"map-two");
    let ambiguous_document = map_only_document(E2SourceKind::E2p, "missing/basemap.scx");

    let output = FixtureRoot::new("output");
    let output_source = output.write("project.e2s", b"source");
    let output_map = output.write("basemap.scx", b"must-not-be-input");
    let mut output_document = map_only_document(E2SourceKind::E2s, "basemap.scx");
    output_document.settings.output_map = output_map.to_string_lossy().into_owned();

    // When: unsafe or non-unique authority is presented to preflight.
    let ambiguous_result = resolve_copy_plan(
        E2ResourceResolveRequest {
            source_path: &ambiguous_source,
            document: &ambiguous_document,
            relinks: &[],
            expected_fingerprint: None,
        },
        || false,
    )
    .expect("ambiguity is recoverable");
    let output_result = resolve_copy_plan(
        E2ResourceResolveRequest {
            source_path: &output_source,
            document: &output_document,
            relinks: &[],
            expected_fingerprint: None,
        },
        || false,
    )
    .expect("excluded output is recoverable");

    // Then: ambiguity and output identity require explicit relink rather than silent choice.
    assert!(
        matches!(ambiguous_result, E2ResourceResolution::NeedsRelink { ref needs, .. } if needs[0].candidates.len() == 2)
    );
    assert!(
        matches!(output_result, E2ResourceResolution::NeedsRelink { ref needs, .. } if needs[0].candidates.is_empty())
    );

    let choices = [E2RelinkChoice {
        need_id: "input-map".to_string(),
        path: relink_map.to_string_lossy().into_owned(),
    }];
    let linked = ready(
        &missing_source,
        &missing_document,
        &choices,
        Some(&fingerprint),
    );
    test_fs::write(&relink_map, b"drifted-map").expect("mutate selected relink");
    let stale = resolve_copy_plan(
        E2ResourceResolveRequest {
            source_path: &missing_source,
            document: &missing_document,
            relinks: &choices,
            expected_fingerprint: Some(&linked.fingerprint),
        },
        || false,
    );
    assert!(matches!(
        stale,
        Err(E2ResourceResolveError::Fatal(E2Issue {
            code: E2IssueCode::SourceChanged,
            ..
        }))
    ));

    let traversal = FixtureRoot::new("traversal");
    let traversal_source = traversal.write("project.e2s", b"source");
    traversal.write("basemap.scx", b"map");
    let mut traversal_document = document(E2SourceKind::E2s, "basemap.scx");
    traversal_document.resources[0].source_path = "../music.wav".to_string();
    let traversal_result = resolve_copy_plan(
        E2ResourceResolveRequest {
            source_path: &traversal_source,
            document: &traversal_document,
            relinks: &[],
            expected_fingerprint: None,
        },
        || false,
    );
    assert!(matches!(
        traversal_result,
        Err(E2ResourceResolveError::Fatal(E2Issue {
            code: E2IssueCode::InvalidResource,
            ..
        }))
    ));

    let mut collision_document = map_only_document(E2SourceKind::E2s, "basemap.scx");
    collision_document.features.push(E2FeatureState {
        feature: E2Feature::Bin,
        activation: E2Activation::Active,
        raw_payload: None,
    });
    collision_document.resources = vec![
        reference(
            "Bin:Payload",
            E2Feature::Bin,
            ProjectResourceKind::Bin,
            "payload.bin",
        ),
        reference(
            "bin:payload",
            E2Feature::Bin,
            ProjectResourceKind::Bin,
            "payload.bin",
        ),
    ];
    let collision = FixtureRoot::new("collision");
    let collision_source = collision.write("project.e2s", b"source");
    collision.write("basemap.scx", b"map");
    collision.write("payload.bin", b"bin");
    let collision_result = resolve_copy_plan(
        E2ResourceResolveRequest {
            source_path: &collision_source,
            document: &collision_document,
            relinks: &[],
            expected_fingerprint: None,
        },
        || false,
    );
    assert!(matches!(
        collision_result,
        Err(E2ResourceResolveError::Fatal(_))
    ));
    assert_eq!(
        resolve_copy_plan(
            E2ResourceResolveRequest {
                source_path: &missing_source,
                document: &missing_document,
                relinks: &[],
                expected_fingerprint: None,
            },
            || true,
        ),
        Err(E2ResourceResolveError::Cancelled)
    );

    let sequence = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
    let temp_root = std::env::temp_dir()
        .join("eud-agent-task7-fixtures")
        .join(format!("{}-linked-{sequence}", std::process::id()));
    register_fixture(&temp_root);
    let outside = temp_root.join("outside");
    test_fs::create_dir_all(&outside).expect("create registered temp fixture");
    let linked_source = temp_root.join("project.e2s");
    test_fs::write(&linked_source, b"source").expect("write linked source");
    test_fs::write(temp_root.join("basemap.scx"), b"map").expect("write linked map");
    test_fs::write(outside.join("payload.bin"), b"outside").expect("write outside payload");
    let link = temp_root.join("linked-dir");
    #[cfg(windows)]
    {
        let status = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "$ErrorActionPreference='Stop'; New-Item -ItemType Junction -Path $env:TASK7_JUNCTION_LINK -Target $env:TASK7_JUNCTION_TARGET | Out-Null",
            ])
            .env("TASK7_JUNCTION_LINK", &link)
            .env("TASK7_JUNCTION_TARGET", &outside)
            .status()
            .expect("junction command must start");
        assert!(status.success(), "fixture junction must be created");
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, &link).expect("fixture symlink must be created");
    let linked_result = resolve_copy_plan(
        E2ResourceResolveRequest {
            source_path: &linked_source,
            document: &map_only_document(E2SourceKind::E2s, "basemap.scx"),
            relinks: &[],
            expected_fingerprint: None,
        },
        || false,
    );
    assert!(matches!(
        linked_result,
        Err(E2ResourceResolveError::Fatal(_))
    ));
    #[cfg(windows)]
    test_fs::remove_dir(&link).expect("remove fixture junction without traversal");
    #[cfg(unix)]
    test_fs::remove_file(&link).expect("remove fixture symlink without traversal");
    test_fs::remove_file(outside.join("payload.bin")).expect("remove exact outside payload");
    test_fs::remove_file(temp_root.join("project.e2s")).expect("remove exact linked source");
    test_fs::remove_file(temp_root.join("basemap.scx")).expect("remove exact linked map");
    test_fs::remove_dir(&outside).expect("remove exact outside directory");
    test_fs::remove_dir(&temp_root).expect("remove exact registered temp fixture");
}
