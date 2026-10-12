#[path = "resource_relocation/consumer_matrix.rs"]
mod consumer_matrix;
#[path = "resource_relocation/drift_cases.rs"]
mod drift_cases;
#[path = "resource_relocation/package.rs"]
mod package;
#[path = "resource_relocation/relocated_build.rs"]
mod relocated_build;

use package::{OwnedRoot, Package, TileMode};

#[test]
#[ignore = "requires frozen euddraft, managed FFmpeg, and registered read-only fixtures"]
fn real_e2p_resources_relocate_and_build() {
    let owner = OwnedRoot::new("positive");
    let memory = Package::create(&owner, "memory", TileMode::Memory);
    let memory_build = relocated_build::import_move_and_build(&memory, "task-16-memory");
    consumer_matrix::assert_consumer_matrix(&memory_build, false);
    let file = Package::create(&owner, "file", TileMode::File);
    let file_build = relocated_build::import_move_and_build(&file, "task-16-file");
    consumer_matrix::assert_consumer_matrix(&file_build, true);
    eprintln!(
        "TASK16_POSITIVE actual_e2p=2 relocated=2 source_unavailable=2 fresh_scx=2 game_equivalence=not_claimed"
    );
}

#[test]
fn relocated_resource_drift_fails() {
    let owner = OwnedRoot::new("negative");
    let drift = Package::create(&owner, "drift", TileMode::Memory);
    drift_cases::assert_same_metadata_drift_fails(&drift);
    drift_cases::assert_missing_and_case_collision(&owner);
    eprintln!(
        "TASK16_NEGATIVE same_metadata_sha_refusal=1 launcher_spawned=0 missing=1 case_collision=1"
    );
}
