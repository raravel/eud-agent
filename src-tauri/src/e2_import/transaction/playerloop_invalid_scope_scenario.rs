use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};

use crate::bootstrap::process_tree::ProcessCancellation;
use crate::e2_import::{E2Feature, E2ImportResult, E2IssueCode, E2PreflightResult, E2SourceKind};

use super::player_scope_test_support::rewrite_owned_forces;
use super::scenario_fixture::ScenarioFixture;

const DELIMITER: &str = "\u{0b90}";

fn source(input_map: &Path, selection: &str) -> String {
    let input_map = input_map.file_name().unwrap().to_string_lossy();
    format!(
        "S_ProjectSET\nVersion : 0.17.9.7\nInputMap : {input_map}\nOutputMap : output.scx\nUsedSetting6 : True\nE_ProjectSET\nS_TriggerEditorSET\nType:10,False,False,False\nPlayerLoop{DELIMITER}{selection}{DELIMITER}10\nElementsCount:1\nType:14,False,False,False\nElementsCount:1\nType:8,False,False,False\nact:CreateVariableWithNoini\nforceCounter\nElementsCount:0\nEND\nEND\nEND\nE_TriggerEditorSET\n"
    )
}

fn map_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("crates/isom/tests/fixtures/map_agent_rich.scx")
}

#[test]
fn all_false_player_loop_is_typed_malformed_input() {
    let parsed = crate::e2_import::parser::parse_e2_bytes(
        source(
            Path::new("fixture.scx"),
            "False,False,False,False,False,False,False,False,False,False,False,False",
        )
        .as_bytes(),
        E2SourceKind::E2s,
    )
    .expect("all-false PlayerLoop wire parses");
    let node_id = parsed.document.triggers[0].id.clone();
    let error = crate::e2_import::compiler::compile_trigger_tree(&parsed.document)
        .expect_err("all-false PlayerLoop must be typed malformed");
    assert_eq!(error.code, E2IssueCode::MalformedInput);
    assert_eq!(error.feature, Some(E2Feature::StructGui));
    for fragment in [&node_id, "PlayerLoop"] {
        assert!(error.message.contains(fragment), "{error:?}");
    }
}

#[test]
fn force_only_player_loop_without_active_members_is_typed_malformed_input() {
    let fixture = ScenarioFixture::create("force-no-active-members");
    fs::copy(map_fixture(), &fixture.input_map).expect("copy read-only fixture to owned map");
    rewrite_owned_forces(&fixture.input_map, [0; 8], [1, 0, 0, 0, 1, 1, 1, 1]);
    let source = source(
        &fixture.input_map,
        "False,False,False,False,False,False,False,False,True,False,False,False",
    );
    fs::write(&fixture.source, &source).expect("write owned E2S");
    let parsed = crate::e2_import::parser::parse_e2_bytes(source.as_bytes(), E2SourceKind::E2s)
        .expect("force-only PlayerLoop wire parses");
    let node_id = parsed.document.triggers[0].id.clone();
    let E2PreflightResult::Ready { fingerprint, .. } =
        super::preflight(&fixture.preflight_request())
    else {
        panic!("owned fixture preflight must be ready");
    };
    let activations = Cell::new(0);
    let result = super::import(
        "force-no-active-members",
        &fixture.import_request(fingerprint),
        &fixture.dirs,
        &fixture.compat_root(),
        &ProcessCancellation::default(),
        |_| {},
        |_| {
            activations.set(activations.get() + 1);
            Ok(())
        },
    );
    let E2ImportResult::Fatal { issues, .. } = result else {
        panic!("force-only PlayerLoop must fail, found {result:?}");
    };
    assert_eq!(issues[0].code, E2IssueCode::MalformedInput);
    assert_eq!(issues[0].feature, Some(E2Feature::StructGui));
    for fragment in [&node_id, "PlayerLoop"] {
        assert!(issues[0].message.contains(fragment), "{issues:?}");
    }
    assert_eq!(activations.get(), 0);
    assert_eq!(fs::read_dir(&fixture.destination).unwrap().count(), 0);
}
