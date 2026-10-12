use std::fs;
use std::path::{Path, PathBuf};

use crate::bootstrap::process_tree::ProcessCancellation;
use crate::e2_import::{E2Feature, E2IssueCode, E2PreflightResult, E2RawValue, E2SourceKind};

use super::player_scope_test_support::rewrite_owned_forces;
use super::scenario_fixture::ScenarioFixture;

fn source(input_map: &Path, mask: u16) -> String {
    let input_map = input_map.file_name().unwrap().to_string_lossy();
    format!(
        "S_ProjectSET\nVersion : 0.17.9.7\nInputMap : {input_map}\nOutputMap : output.scx\nUsedSetting6 : True\nE_ProjectSET\nS_TriggerEditorSET\n&RawTriggers&\nType:25,False,False,False\nElementsCount:1\nType:24,False,False,False\n{mask}\nElementsCount:2\nType:26,False,False,False\nAnd\nElementsCount:1\nType:7,False,False,False\ncon:Always\n0\nElementsCount:0\nEND\nEND\nType:27,False,False,False\nElementsCount:2\nType:8,False,False,False\nact:RawCode\nvar rawForce = 1\nElementsCount:0\nEND\nType:8,False,False,False\nact:PreserveTrigger\n0\nElementsCount:0\nEND\nEND\nEND\nEND\nE_TriggerEditorSET\n"
    )
}

fn map_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("crates/isom/tests/fixtures/map_agent_rich.scx")
}

fn player_block(main: &str, player: usize) -> &str {
    let marker = format!("if (playerexist({player}))");
    let start = main.find(&marker).expect("player block");
    let end = (player + 1..8)
        .find_map(|next| main[start..].find(&format!("if (playerexist({next}))")))
        .map_or(main.len(), |offset| start + offset);
    &main[start..end]
}

#[test]
fn owned_map_force_membership_drives_raw_trigger_projection() {
    let fixture = ScenarioFixture::create("raw-force-scope");
    fs::copy(map_fixture(), &fixture.input_map).expect("copy read-only fixture to owned map");
    rewrite_owned_forces(
        &fixture.input_map,
        [0, 3, 0, 5, 0, 0, 0, 0],
        [1, 0, 0, 0, 1, 1, 1, 1],
    );
    let wire = source(&fixture.input_map, 258);
    fs::write(&fixture.source, &wire).expect("write owned RAW E2S");
    let parsed = crate::e2_import::parser::parse_e2_bytes(wire.as_bytes(), E2SourceKind::E2s)
        .expect("authentic direct-P2 Force1 RAW wire parses");
    let raw = &parsed.document.triggers[0].children[0].children[0];
    assert_eq!(raw.values, vec![E2RawValue::String("258".to_string())]);

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
            Err(error) => panic!("owned RAW import preparation failed: {error:?}"),
        };
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
    .expect("projection must use planned owned map bytes");
    let main = String::from_utf8(projection.files["src/main.eps"].clone()).unwrap();
    let executor = main
        .find("function ClassicTriggerExec0()")
        .expect("RAW executor");
    let raw_code = main.find("var rawForce = 1;").expect("RAW action");
    assert!(executor < raw_code);
    let p2 = player_block(&main, 1);
    let p3 = player_block(&main, 2);
    let p4 = player_block(&main, 3);
    assert_eq!(
        p2.matches("ClassicTriggerStarter0();").count(),
        1,
        "P2 deduped"
    );
    assert!(
        !p3.contains("ClassicTriggerStarter0();"),
        "inactive P3 excluded"
    );
    assert_eq!(
        p4.matches("ClassicTriggerStarter0();").count(),
        1,
        "Force1 P4 selected"
    );
    let hook = &main[main
        .find("function beforeTriggerExec()")
        .expect("before hook")..];
    let ordered = [
        "EUDPlayerLoop()();",
        "ClassicTriggerExec();",
        "EUDEndPlayerLoop();",
        "ClassicTriggerStarter();",
    ]
    .map(|fragment| hook.find(fragment).expect("RAW lifecycle fragment"));
    assert!(ordered.windows(2).all(|pair| pair[0] < pair[1]), "{hook}");
}

#[test]
fn force_selected_raw_trigger_without_projection_context_is_typed() {
    let parsed = crate::e2_import::parser::parse_e2_bytes(
        source(Path::new("fixture.scx"), 256).as_bytes(),
        E2SourceKind::E2s,
    )
    .expect("authentic Force1 RAW wire parses");
    let raw = &parsed.document.triggers[0].children[0].children[0];
    let error = crate::e2_import::compiler::compile_trigger_tree(&parsed.document)
        .expect_err("force-selected RAW requires map projection context");
    assert_eq!(error.code, E2IssueCode::MalformedInput);
    assert_eq!(error.feature, Some(E2Feature::StructGui));
    for fragment in [&raw.id, "RawTrigger"] {
        assert!(error.message.contains(fragment), "{error:?}");
    }
}
