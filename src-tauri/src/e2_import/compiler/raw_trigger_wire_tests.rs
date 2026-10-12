use super::compile_trigger_tree;
use crate::e2_import::parser::parse_e2_bytes;
use crate::e2_import::{E2Feature, E2IssueCode, E2RawValue, E2SourceKind};

fn source(trigger: &str) -> String {
    let mut text = String::from(
        "S_ProjectSET\nVersion : 0.17.9.7\nInputMap : Map/input.scx\nOutputMap : Map/output.scx\n",
    );
    for index in 0..=8 {
        text.push_str(&format!(
            "UsedSetting{index} : {}\n",
            if index == 6 { "True" } else { "False" }
        ));
    }
    text.push_str("E_ProjectSET\n");
    text.push_str(trigger);
    text
}

fn assert_ordered(text: &str, fragments: &[&str]) {
    let positions = fragments
        .iter()
        .map(|fragment| {
            text.find(fragment)
                .unwrap_or_else(|| panic!("missing {fragment:?} in {text}"))
        })
        .collect::<Vec<_>>();
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]), "{text}");
}

fn player_block(main: &str, player: usize) -> &str {
    let marker = format!("if (playerexist({player}))");
    let start = main
        .find(&marker)
        .unwrap_or_else(|| panic!("missing {marker:?} in {main}"));
    let end = (player + 1..8)
        .find_map(|next| main[start..].find(&format!("if (playerexist({next}))")))
        .map_or(main.len(), |offset| start + offset);
    &main[start..end]
}

#[test]
fn authentic_v0179_raw_trigger_preserves_player_scope_condition_order_and_timer_state() {
    let trigger = concat!(
        "S_TriggerEditorSET\n&RawTriggers&\n",
        "Type:25,False,False,False\nElementsCount:1\n",
        "Type:24,False,False,False\n3\nElementsCount:2\n",
        "Type:26,False,False,False\nAnd\nElementsCount:2\n",
        "Type:7,False,False,False\ncon:Always\n0\nElementsCount:0\nEND\n",
        "Type:7,False,False,False\ncon:Never\n0\nElementsCount:0\nEND\nEND\n",
        "Type:27,False,False,False\nElementsCount:2\n",
        "Type:8,False,False,False\nact:RawCode\nvar rawCounter = 1\n",
        "ElementsCount:0\nEND\n",
        "Type:8,False,False,False\nact:PreserveTrigger\n0\n",
        "ElementsCount:0\nEND\nEND\nEND\nEND\nE_TriggerEditorSET\n",
    );
    let output = parse_e2_bytes(source(trigger).as_bytes(), E2SourceKind::E2s)
        .expect("authentic v0.17.9.7 RAW trigger wire parses");
    let raw = &output.document.triggers[0].children[0].children[0];
    assert_eq!(raw.name, "type-24");
    assert_eq!(raw.values, vec![E2RawValue::String("3".to_string())]);
    assert_eq!(
        raw.children
            .iter()
            .map(|node| node.name.as_str())
            .collect::<Vec<_>>(),
        ["type-26", "type-27"]
    );
    assert_eq!(
        raw.children[0].values,
        vec![E2RawValue::String("And".to_string())]
    );
    assert_eq!(
        raw.children[0]
            .children
            .iter()
            .map(|node| node.name.as_str())
            .collect::<Vec<_>>(),
        ["Always", "Never"]
    );
    assert_eq!(
        raw.children[1]
            .children
            .iter()
            .map(|node| node.name.as_str())
            .collect::<Vec<_>>(),
        ["RawCode", "PreserveTrigger"]
    );
    assert_eq!(
        raw.children[1].children[0].values,
        vec![E2RawValue::String("var rawCounter = 1".to_string())]
    );
    assert_eq!(
        raw.children[1].children[1].values,
        vec![E2RawValue::String("0".to_string())]
    );

    let compiled = compile_trigger_tree(&output.document).expect("direct-mask RAW compiles");
    let main = &compiled.files["src/main.eps"];
    let executor_start = main
        .find("function ClassicTriggerExec0()")
        .expect("executor");
    let starter_start = main
        .find("function ClassicTriggerStarter0()")
        .expect("starter");
    let executor = &main[executor_start..starter_start];
    assert!(main[..executor_start]
        .contains("const ClassicTriggerExecTimer0 = [0, 0, 0, 0, 0, 0, 0, 0];"));
    assert_ordered(
        executor,
        &[
            "ClassicTriggerExecTimer0[getcurpl()] == 1",
            "var rawCounter = 1;",
            "PreserveTrigger();",
            "ClassicTriggerExecTimer0[getcurpl()] = 0;",
            "ClassicTriggerExecTimer0[getcurpl()] > 0",
            "ClassicTriggerExecTimer0[getcurpl()] += 1;",
        ],
    );
    let aggregate_start = main[starter_start..]
        .find("function ClassicTriggerExec()")
        .map(|offset| starter_start + offset)
        .expect("aggregate executor");
    let starter = &main[starter_start..aggregate_start];
    assert!(starter.contains("Always() && Never()"), "{starter}");
    assert_ordered(
        starter,
        &[
            "ClassicTriggerExecTimer0[getcurpl()] == 0",
            "ClassicTriggerExecTimer0[getcurpl()] = 1;",
            "ClassicTriggerExec0();",
        ],
    );
    for player in 0..8 {
        let block = player_block(main, player);
        assert!(block.contains(&format!("setcurpl({player});")), "{block}");
        assert_eq!(
            block.contains("ClassicTriggerStarter0();"),
            player < 2,
            "{block}"
        );
    }
    let before_hook = &main[main
        .find("function beforeTriggerExec()")
        .expect("before hook")..];
    assert_ordered(
        before_hook,
        &[
            "ClassicTriggerExec();",
            "EUDEndPlayerLoop();",
            "ClassicTriggerStarter();",
        ],
    );
}

#[test]
fn authentic_v0179_raw_trigger_without_preserve_remains_one_shot() {
    let trigger = concat!(
        "S_TriggerEditorSET\n&RawTriggers&\n",
        "Type:25,False,False,False\nElementsCount:1\n",
        "Type:24,False,False,False\n4096\nElementsCount:2\n",
        "Type:26,False,False,False\nOr\nElementsCount:2\n",
        "Type:7,False,False,False\ncon:Always\n0\nElementsCount:0\nEND\n",
        "Type:7,False,False,False\ncon:Never\n0\nElementsCount:0\nEND\nEND\n",
        "Type:27,False,False,False\nElementsCount:1\n",
        "Type:8,False,False,False\nact:RawCode\nvar oneShot = 1\n",
        "ElementsCount:0\nEND\nEND\nEND\nEND\nE_TriggerEditorSET\n",
    );
    let output = parse_e2_bytes(source(trigger).as_bytes(), E2SourceKind::E2s)
        .expect("authentic all-player RAW trigger wire parses");
    let raw = &output.document.triggers[0].children[0].children[0];
    assert_eq!(raw.values, vec![E2RawValue::String("4096".to_string())]);
    assert_eq!(
        raw.children[0].values,
        vec![E2RawValue::String("Or".to_string())]
    );
    assert_eq!(raw.children[1].children[0].name, "RawCode");
    assert_eq!(
        raw.children[1].children[0].values,
        vec![E2RawValue::String("var oneShot = 1".to_string())]
    );

    let compiled = compile_trigger_tree(&output.document).expect("all-player RAW compiles");
    let main = &compiled.files["src/main.eps"];
    let executor_start = main
        .find("function ClassicTriggerExec0()")
        .expect("executor");
    let starter_start = main
        .find("function ClassicTriggerStarter0()")
        .expect("starter");
    let executor = &main[executor_start..starter_start];
    assert_ordered(
        executor,
        &[
            "ClassicTriggerExecTimer0[getcurpl()] == 1",
            "var oneShot = 1;",
            "ClassicTriggerExecTimer0[getcurpl()] > 0",
            "ClassicTriggerExecTimer0[getcurpl()] += 1;",
        ],
    );
    assert!(!executor.contains("ClassicTriggerExecTimer0[getcurpl()] = 0;"));
    let starter = &main[starter_start..];
    assert!(starter.contains("Always() || Never()"), "{starter}");
    assert_ordered(
        starter,
        &[
            "ClassicTriggerExecTimer0[getcurpl()] = 1;",
            "ClassicTriggerExec0();",
        ],
    );
    assert_eq!(main.matches("if (playerexist(").count(), 8);
    assert_eq!(main.matches("ClassicTriggerStarter0();").count(), 8);
    for player in 0..8 {
        assert!(player_block(main, player).contains(&format!("setcurpl({player});")));
    }
}

#[test]
fn authentic_v0179_raw_trigger_malformed_container_is_typed() {
    let trigger = concat!(
        "S_TriggerEditorSET\n&RawTriggers&\n",
        "Type:25,False,False,False\nElementsCount:1\n",
        "Type:24,False,False,False\n3\nElementsCount:1\n",
        "Type:26,False,False,False\nAnd\nElementsCount:1\n",
        "Type:7,False,False,False\ncon:Always\n0\nElementsCount:0\nEND\n",
        "END\nEND\nEND\nE_TriggerEditorSET\n",
    );
    let output = parse_e2_bytes(source(trigger).as_bytes(), E2SourceKind::E2s)
        .expect("RAW trigger missing its action container remains parseable");
    let raw = &output.document.triggers[0].children[0].children[0];
    assert_eq!(raw.values, vec![E2RawValue::String("3".to_string())]);
    assert_eq!(raw.children.len(), 1);
    assert_eq!(raw.children[0].name, "type-26");
    let raw_id = raw.id.clone();
    let error = compile_trigger_tree(&output.document)
        .expect_err("RAW trigger without Type 27 must not flatten or compile");
    assert_eq!(error.code, E2IssueCode::MalformedInput);
    assert_eq!(error.feature, Some(E2Feature::StructGui));
    for fragment in [&raw_id, "type-26", "type-27"] {
        assert!(error.message.contains(fragment), "{error:?}");
    }
}
