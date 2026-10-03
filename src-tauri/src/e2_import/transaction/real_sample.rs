use std::cell::Cell;
use std::fs;
use std::path::Path;

use crate::bootstrap::process_tree::ProcessCancellation;
use crate::e2_import::{
    E2Feature, E2ImportRequest, E2ImportResult, E2PreflightRequest, E2PreflightResult, E2RawValue,
    E2RelinkKind,
};
use crate::native_build::{run_native_build, EuddraftLaunch};
use crate::native_project::{DatScalar, DatTarget, NativeProject};

#[path = "real_sample/support.rs"]
mod support;

use support::*;

#[test]
#[ignore = "requires EUD_AGENT_EUDDRAFT and protected defense_mob inputs"]
fn real_e2s_import_reopen_build_is_semantically_stable() {
    let paths = protected_paths();
    let original_hashes = protected_hashes(&paths);
    let evidence = fresh_evidence_root();
    let root = disposable_root("positive");
    let package = root.0.join("package");
    let copied_source = package.join(SAMPLE_E2S);
    let copied_input = package.join("owned-input").join(INPUT_MAP);
    fs::create_dir_all(copied_input.parent().expect("owned map parent")).unwrap();
    fs::copy(&paths.source, &copied_source).expect("copy protected E2S");
    fs::copy(&paths.input_map, &copied_input).expect("copy protected input map");

    let destination = root.0.join("imported-project");
    fs::create_dir(&destination).expect("empty import destination");
    let dirs = data_dirs(&root.0);
    let (fingerprint, relinks, features) =
        ready_with_owned_input(&copied_source, &destination, &copied_input);
    assert!(features.contains(&E2Feature::Dat));
    assert!(features.contains(&E2Feature::StructGui));

    let parsed =
        crate::e2_import::parser::parse_e2_path(&copied_source).expect("copied E2S parses");
    let dat_rows = parsed
        .document
        .raw_sections
        .iter()
        .find(|section| section.name == "DatEditSET")
        .and_then(|section| match &section.payload {
            E2RawValue::List(rows) => Some(rows),
            _ => None,
        })
        .expect("real DAT receipt");
    assert_eq!(
        dat_rows
            .iter()
            .filter(|row| matches!(row, E2RawValue::String(value) if !value.is_empty()))
            .count(),
        12
    );

    let request = E2ImportRequest {
        source_path: copied_source.to_string_lossy().into_owned(),
        destination_path: destination.to_string_lossy().into_owned(),
        fingerprint: fingerprint.clone(),
        relinks,
    };
    let activation_count = Cell::new(0_u32);
    let manager = crate::native_runtime::NativeProjectManager::new(dirs.clone());
    let result = super::import(
        "task-15-real-sample",
        &request,
        &dirs,
        &compat_root(),
        &ProcessCancellation::default(),
        |_| {},
        |project| {
            activation_count.set(activation_count.get() + 1);
            manager.activate_project(project)
        },
    );
    assert!(
        matches!(result, E2ImportResult::Completed { .. }),
        "import result: {result:?}"
    );
    assert_eq!(activation_count.get(), 1);

    let reopened = NativeProject::open(&destination).expect("published EAP reopens");
    assert_eq!(
        fs::read(reopened.source_map_path().unwrap()).unwrap(),
        fs::read(&copied_input).unwrap()
    );
    assert_eq!(reopened.dat_overrides().len(), 12, "reopened DAT receipt");
    assert_eq!(
        reopened.current_dat_value(&DatTarget::Dat {
            dat: "units".to_string(),
            object_id: 0,
            field: "Elevation Level".to_string()
        }),
        Some(DatScalar::Number(0))
    );
    assert_eq!(
        reopened.current_dat_value(&DatTarget::Dat {
            dat: "sprites".to_string(),
            object_id: 235,
            field: "Image File".to_string()
        }),
        Some(DatScalar::Number(36))
    );
    let main = reopened
        .read_source("src/main.eps")
        .expect("generated GUI EPS");
    assert!(main.contains("function markMobs"));
    assert!(main.contains("EUDLoopUnit2()"));
    assert!(main.contains("for(var loc = $L(\"0\"); loc <= $L(\"11\"); loc += 1) {"));

    let revision_before_edit = reopened.revision().unwrap();
    reopened
        .write_source(
            "src/main.eps",
            &("// bounded Task 15 edit\n".to_string() + &main),
        )
        .expect("generated EPS remains editable");
    let revision_after_edit = reopened.revision().unwrap();
    assert_ne!(revision_before_edit, revision_after_edit);
    let reopened = NativeProject::open(&destination).expect("edited EAP reopens");
    assert_eq!(reopened.revision().unwrap(), revision_after_edit);

    let euddraft = EuddraftLaunch::resolve(Path::new(&required_env("EUD_AGENT_EUDDRAFT"))).unwrap();
    let build = run_native_build(&reopened, &compat_root(), &euddraft);
    let protected_after_build = protected_hashes(&paths);
    retain_build_evidence(
        &evidence,
        &reopened,
        &build,
        &main,
        &original_hashes,
        &protected_after_build,
    );
    let result = build.expect("frozen build runs");
    assert!(result.ok, "build errors: {:?}", result.errors);
    assert!(result.errors.is_empty());
    assert_eq!(reopened.revision().unwrap(), revision_after_edit);
    let output = reopened.output_map_path().unwrap();
    assert!(
        output.is_file() && fs::metadata(&output).unwrap().len() > 0,
        "fresh SCX exists"
    );
    let log = fs::read_to_string(&result.log_path).expect("fresh complete build log");
    for marker in [
        "[epScript]",
        "[[ freeze activated ]]",
        "Injecting plugins",
        "[Stage 4/3]",
    ] {
        assert!(log.contains(marker), "missing build marker {marker}");
    }
    assert!(
        !log.to_ascii_lowercase().contains("error"),
        "build log contains Error"
    );
    assert_eq!(
        protected_after_build, original_hashes,
        "protected originals changed"
    );
    eprintln!("TASK15_POSITIVE imported=1 dat_rows=12 editable=1 frozen_build=1 game_equivalence=not_claimed");
}

#[test]
fn comparison_output_cannot_satisfy_missing_input_map() {
    let paths = protected_paths();
    let original_hashes = protected_hashes(&paths);
    let root = disposable_root("negative");
    let package = root.0.join("comparison-only");
    fs::create_dir_all(&package).unwrap();
    let copied_source = package.join(SAMPLE_E2S);
    let copied_comparison = package.join(COMPARISON_OUTPUT);
    fs::copy(&paths.source, &copied_source).unwrap();
    fs::copy(&paths.comparison_output, &copied_comparison).unwrap();
    let destination = root.0.join("must-not-publish");
    let dirs = data_dirs(&root.0);
    let config_before = dirs.load_config().unwrap();

    let result = super::preflight(&E2PreflightRequest {
        source_path: copied_source.to_string_lossy().into_owned(),
        destination_path: destination.to_string_lossy().into_owned(),
        relinks: Vec::new(),
    });
    let E2PreflightResult::NeedsRelink { needs, .. } = result else {
        panic!("comparison output must not satisfy missing input map: {result:?}");
    };
    let input_needs = needs
        .iter()
        .filter(|need| need.kind == E2RelinkKind::InputMap)
        .collect::<Vec<_>>();
    assert_eq!(
        input_needs.len(),
        1,
        "exactly one input-map relink is required"
    );
    assert!(!input_needs[0]
        .candidates
        .iter()
        .any(|path| path == &copied_comparison.to_string_lossy()));
    assert!(!destination.exists());
    assert_eq!(dirs.load_config().unwrap(), config_before);
    assert_eq!(
        fs::read(&copied_comparison).unwrap(),
        fs::read(&paths.comparison_output).unwrap()
    );
    assert_eq!(
        protected_hashes(&paths),
        original_hashes,
        "protected originals changed"
    );
    eprintln!("TASK15_NEGATIVE needs_relink=input_map comparison_output_input=refused");
}
