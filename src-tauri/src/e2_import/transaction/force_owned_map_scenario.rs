use std::fs;
use std::path::{Path, PathBuf};

use crate::bootstrap::process_tree::ProcessCancellation;
use crate::e2_import::{E2Feature, E2IssueCode, E2PreflightResult, E2RawValue, E2SourceKind};

use super::player_scope_test_support::rewrite_owned_forces;
use super::scenario_fixture::ScenarioFixture;

const DELIMITER: &str = "\u{0b90}";

fn source(input_map: &Path) -> String {
    let input_map = input_map.file_name().unwrap().to_string_lossy();
    format!(
        "S_ProjectSET\nVersion : 0.17.9.7\nInputMap : {input_map}\nOutputMap : output.scx\nUsedSetting6 : True\nE_ProjectSET\nS_TriggerEditorSET\nType:10,False,False,False\nPlayerLoop{DELIMITER}False,True,False,False,False,False,False,False,True,False,False,False{DELIMITER}10\nElementsCount:1\nType:14,False,False,False\nElementsCount:1\nType:8,False,False,False\nact:CreateVariableWithNoini\nforceCounter\nElementsCount:0\nEND\nEND\nEND\nE_TriggerEditorSET\n"
    )
}

fn map_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("crates/isom/tests/fixtures/map_agent_rich.scx")
}

#[test]
fn owned_map_force_membership_drives_player_loop_projection() {
    let fixture = ScenarioFixture::create("force-owned-map");
    fs::copy(map_fixture(), &fixture.input_map).expect("copy read-only fixture to owned map");
    rewrite_owned_forces(
        &fixture.input_map,
        [0, 3, 0, 5, 0, 0, 0, 0],
        [1, 0, 0, 0, 1, 1, 1, 1],
    );
    fs::write(&fixture.source, source(&fixture.input_map)).expect("write owned E2S");
    let cancellation = ProcessCancellation::default();
    let E2PreflightResult::Ready { fingerprint, .. } =
        super::preflight(&fixture.preflight_request())
    else {
        panic!("owned fixture preflight must be ready");
    };
    let prepared =
        match super::preflight::prepare_import(&fixture.import_request(fingerprint), &cancellation)
        {
            Ok(prepared) => prepared,
            Err(error) => panic!("owned fixture import preparation failed: {error:?}"),
        };
    let loop_node = &prepared.document.triggers[0];
    assert_eq!(
        loop_node.values,
        vec![
            E2RawValue::String("PlayerLoop".to_string()),
            E2RawValue::String(
                "False,True,False,False,False,False,False,False,True,False,False,False".to_string(),
            ),
            E2RawValue::String("10".to_string()),
        ]
    );
    assert!(prepared
        .resources
        .copies
        .iter()
        .any(|copy| copy.bytes == fs::read(&fixture.input_map).unwrap()));
    fs::remove_file(&fixture.input_map).expect("remove only owned source map after planning");
    let projection = super::native_projection::compile(
        &prepared,
        &fixture.dirs,
        &fixture.compat_root(),
        &cancellation,
    )
    .expect("projection must consume owned map bytes");
    let main = String::from_utf8(projection.files["src/main.eps"].clone()).unwrap();
    let start = main.find("EUDPlayerLoop()();").expect("PlayerLoop start");
    let p2 = main.find("getcurpl() == 1").expect("P2 predicate");
    let p4 = main.find("getcurpl() == 3").expect("P4 predicate");
    let child = main.find("var forceCounter").expect("PlayerLoop child");
    let end = main.find("EUDEndPlayerLoop();").expect("PlayerLoop end");
    assert!(start < p2 && p2 < p4 && p4 < child && child < end);
    assert_eq!(
        main.matches("getcurpl() == 1").count(),
        1,
        "P2 deduplicated"
    );
    assert!(!main.contains("getcurpl() == 2"), "inactive P3 excluded");
}

#[test]
fn force_selected_player_loop_without_projection_context_is_typed() {
    let source = source(Path::new("fixture.scx"));
    let parsed = crate::e2_import::parser::parse_e2_bytes(source.as_bytes(), E2SourceKind::E2s)
        .expect("authentic PlayerLoop wire parses");
    let error = crate::e2_import::compiler::compile_trigger_tree(&parsed.document)
        .expect_err("force-selected PlayerLoop needs projection context");
    assert_eq!(error.code, E2IssueCode::MalformedInput);
    assert_eq!(error.feature, Some(E2Feature::StructGui));
}
