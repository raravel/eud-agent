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

#[test]
fn authentic_v0179_wait_function_snapshots_arguments_and_resumes_by_frames() {
    let trigger = concat!(
        "S_TriggerEditorSET\n",
        "&functions&\nType:15,False,False,False\nElementsCount:1\n",
        "Type:16,False,False,False\ndelayed\u{0b90}True\nElementsCount:2\n",
        "Type:17,False,False,False\nElementsCount:1\n",
        "Type:8,False,False,False\nact:CreateVariableWithNoini\namount\u{0b90}0\n",
        "ElementsCount:0\nEND\nEND\n",
        "Type:18,False,False,False\nElementsCount:3\n",
        "Type:8,False,False,False\nact:RawCode\nvar before = amount\nElementsCount:0\nEND\n",
        "Type:20,False,False,False\n3\nElementsCount:0\nEND\n",
        "Type:8,False,False,False\nact:RawCode\nvar after = amount\nElementsCount:0\nEND\n",
        "END\nEND\nEND\n",
        "&beforeTriggerExec&\nType:0,False,False,False\nElementsCount:2\n",
        "Type:19,False,False,False\ndelayed\u{0b90}7\nElementsCount:0\nEND\n",
        "Type:19,False,False,False\ndelayed\u{0b90}9\nElementsCount:0\nEND\n",
        "END\nE_TriggerEditorSET\n",
    );
    let output = parse_e2_bytes(source(trigger).as_bytes(), E2SourceKind::E2s)
        .expect("authentic v0.17.9.7 wait function wire parses");
    let definition = &output.document.triggers[0].children[0].children[0];
    assert_eq!(
        definition.values,
        vec![
            E2RawValue::String("delayed".to_string()),
            E2RawValue::String("True".to_string()),
        ]
    );
    assert_eq!(
        definition.children[0].children[0].values,
        vec![
            E2RawValue::String("amount".to_string()),
            E2RawValue::String("0".to_string()),
        ]
    );
    let code = &definition.children[1].children;
    assert_eq!(
        code.iter()
            .map(|node| node.name.as_str())
            .collect::<Vec<_>>(),
        ["RawCode", "type-20", "RawCode"]
    );
    assert_eq!(code[1].values, vec![E2RawValue::String("3".to_string())]);
    let calls = &output.document.triggers[1].children[0].children;
    assert_eq!(
        calls
            .iter()
            .map(|node| node.values.clone())
            .collect::<Vec<_>>(),
        [
            vec![
                E2RawValue::String("delayed".to_string()),
                E2RawValue::String("7".to_string()),
            ],
            vec![
                E2RawValue::String("delayed".to_string()),
                E2RawValue::String("9".to_string()),
            ],
        ]
    );

    let compiled = compile_trigger_tree(&output.document).expect("wait function compiles");
    let main = &compiled.files["src/main.eps"];
    let before_hook = main
        .find("function beforeTriggerExec()")
        .expect("before hook exists");
    for declaration in [
        "const delayedamount = [0, 0, 0, 0, 0, 0, 0, 0];",
        "const delayedTimer = [0, 0, 0, 0, 0, 0, 0, 0];",
        "function delayed(amount);",
        "function delayed(amount) {",
    ] {
        assert!(main[..before_hook].contains(declaration), "{main}");
    }
    let definition_start = main[..before_hook]
        .rfind("function delayed(amount) {")
        .expect("wait definition is module scoped");
    assert_ordered(
        &main[definition_start..before_hook],
        &[
            "if (delayedTimer[getcurpl()] > 0) {",
            "if (delayedTimer[getcurpl()] == 1) {",
            "var before = amount;",
            "} else if (delayedTimer[getcurpl()] == 4) {",
            "var after = amount;",
            "delayedTimer[getcurpl()] = -1;",
            "delayedTimer[getcurpl()] += 1;",
        ],
    );
    let definition_body = &main[definition_start..before_hook];
    let state_one = definition_body
        .find("if (delayedTimer[getcurpl()] == 1) {")
        .expect("initial wait state");
    let state_four = definition_body
        .find("} else if (delayedTimer[getcurpl()] == 4) {")
        .expect("resumed wait state");
    let terminal = definition_body
        .find("delayedTimer[getcurpl()] = -1;")
        .expect("terminal wait state");
    assert!(definition_body[state_one..state_four].contains("var before = amount;"));
    assert!(!definition_body[state_one..state_four].contains("var after = amount;"));
    assert!(definition_body[state_four..terminal].contains("var after = amount;"));
    assert!(!definition_body[state_four..terminal].contains("var before = amount;"));
    let hook = &main[before_hook..];
    assert_ordered(hook, &["WaitableTriggerExec();", "EUDEndPlayerLoop();"]);
    let guards = hook
        .match_indices("if (delayedTimer[getcurpl()] == 0) {")
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let calls = hook
        .match_indices("delayed(delayedamount[getcurpl()]);")
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let snapshot_seven = hook
        .find("delayedamount[getcurpl()] = 7;")
        .expect("first snapshot");
    let snapshot_nine = hook
        .find("delayedamount[getcurpl()] = 9;")
        .expect("second snapshot");
    assert_eq!(guards.len(), 2);
    assert_eq!(calls.len(), 2);
    assert!(
        guards[0] < snapshot_seven
            && snapshot_seven < calls[0]
            && calls[0] < guards[1]
            && guards[1] < snapshot_nine
            && snapshot_nine < calls[1],
        "{hook}"
    );
}

#[test]
fn authentic_v0179_wait_disabled_function_keeps_direct_call_and_omits_transition() {
    let trigger = concat!(
        "S_TriggerEditorSET\n",
        "&functions&\nType:15,False,False,False\nElementsCount:1\n",
        "Type:16,False,False,False\ndirect\u{0b90}False\nElementsCount:2\n",
        "Type:17,False,False,False\nElementsCount:1\n",
        "Type:8,False,False,False\nact:CreateVariableWithNoini\namount\u{0b90}0\n",
        "ElementsCount:0\nEND\nEND\n",
        "Type:18,False,False,False\nElementsCount:3\n",
        "Type:8,False,False,False\nact:RawCode\nvar beforeDirect = amount\nElementsCount:0\nEND\n",
        "Type:20,False,False,False\n3\nElementsCount:0\nEND\n",
        "Type:8,False,False,False\nact:RawCode\nvar afterDirect = amount\nElementsCount:0\nEND\n",
        "END\nEND\nEND\n",
        "&beforeTriggerExec&\nType:0,False,False,False\nElementsCount:1\n",
        "Type:19,False,False,False\ndirect\u{0b90}7\nElementsCount:0\nEND\n",
        "END\nE_TriggerEditorSET\n",
    );
    let output = parse_e2_bytes(source(trigger).as_bytes(), E2SourceKind::E2s)
        .expect("authentic disabled-wait function wire parses");
    let definition = &output.document.triggers[0].children[0].children[0];
    assert_eq!(
        definition.values[1],
        E2RawValue::String("False".to_string())
    );
    assert_eq!(definition.children[1].children[1].name, "type-20");

    let compiled = compile_trigger_tree(&output.document).expect("direct function compiles");
    let main = &compiled.files["src/main.eps"];
    let before_hook = main
        .find("function beforeTriggerExec()")
        .expect("before hook exists");
    assert_ordered(
        &main[..before_hook],
        &[
            "function direct(amount) {",
            "var beforeDirect = amount;",
            "var afterDirect = amount;",
        ],
    );
    assert!(main[before_hook..].contains("direct(7);"), "{main}");
    for omitted in [
        "directTimer",
        "directamount = [",
        "WaitableTriggerExec",
        "else if",
    ] {
        assert!(!main.contains(omitted), "unexpected {omitted:?} in {main}");
    }
}

#[test]
fn authentic_v0179_wait_duration_overflow_is_typed_malformed() {
    let trigger = concat!(
        "S_TriggerEditorSET\n",
        "&functions&\nType:15,False,False,False\nElementsCount:1\n",
        "Type:16,False,False,False\noverflow\u{0b90}True\nElementsCount:2\n",
        "Type:17,False,False,False\nElementsCount:0\nEND\n",
        "Type:18,False,False,False\nElementsCount:3\n",
        "Type:20,False,False,False\n2147483646\nElementsCount:0\nEND\n",
        "Type:8,False,False,False\nact:RawCode\nstillValid = 1\nElementsCount:0\nEND\n",
        "Type:20,False,False,False\n1\nElementsCount:0\nEND\n",
        "END\nEND\nEND\nE_TriggerEditorSET\n",
    );
    let output = parse_e2_bytes(source(trigger).as_bytes(), E2SourceKind::E2s)
        .expect("source-valid overflowing wait durations parse");
    let code = &output.document.triggers[0].children[0].children[0].children[1].children;
    assert_eq!(
        [code[0].values[0].clone(), code[2].values[0].clone()],
        [
            E2RawValue::String("2147483646".to_string()),
            E2RawValue::String("1".to_string()),
        ]
    );
    let overflow_node_id = code[2].id.clone();
    let error = compile_trigger_tree(&output.document)
        .expect_err("cumulative wait duration beyond i32::MAX must fail");
    assert_eq!(error.code, E2IssueCode::MalformedInput);
    assert_eq!(error.feature, Some(E2Feature::StructGui));
    for fragment in [&overflow_node_id, "Wait", "overflow"] {
        assert!(error.message.contains(fragment), "{error:?}");
    }
}
