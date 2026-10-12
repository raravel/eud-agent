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

#[test]
fn authentic_v0179_switch_cases_preserve_scope_order_default_and_break() {
    let trigger = concat!(
        "S_TriggerEditorSET\n",
        "&beforeTriggerExec&\n",
        "Type:28,False,False,False\nscore\nElementsCount:2\n",
        "Type:29,False,False,False\n1\u{0b90}True\nElementsCount:1\n",
        "Type:8,False,False,False\nact:CreateVariableWithNoini\ncaseCounter\n",
        "ElementsCount:0\nEND\nEND\n",
        "Type:29,False,False,False\n-1\u{0b90}False\nElementsCount:1\n",
        "Type:8,False,False,False\nact:CreateVariableWithNoini\ndefaultCounter\n",
        "ElementsCount:0\nEND\nEND\nEND\n",
        "E_TriggerEditorSET\n",
    );
    let output = parse_e2_bytes(source(trigger).as_bytes(), E2SourceKind::E2s)
        .expect("authentic v0.17.9.7 Switch wire parses");
    let switch = &output.document.triggers[0].children[0];
    assert_eq!(switch.name, "type-28");
    assert_eq!(switch.values, vec![E2RawValue::String("score".to_string())]);
    assert_eq!(switch.children.len(), 2);
    assert_eq!(switch.children[0].name, "type-29");
    assert_eq!(
        switch.children[0].values,
        vec![
            E2RawValue::String("1".to_string()),
            E2RawValue::String("True".to_string()),
        ]
    );
    assert_eq!(
        switch.children[0].children[0].name,
        "CreateVariableWithNoini"
    );
    assert_eq!(switch.children[1].name, "type-29");
    assert_eq!(
        switch.children[1].values,
        vec![
            E2RawValue::String("-1".to_string()),
            E2RawValue::String("False".to_string()),
        ]
    );
    assert_eq!(
        switch.children[1].children[0].name,
        "CreateVariableWithNoini"
    );

    let compiled = compile_trigger_tree(&output.document).expect("authentic Switch compiles");
    let main = &compiled.files["src/main.eps"];
    let ordered = [
        "switch(score) {",
        "case 1:",
        "var caseCounter",
        "break;",
        "default:",
        "var defaultCounter",
    ]
    .map(|fragment| {
        main.find(fragment)
            .unwrap_or_else(|| panic!("missing {fragment:?} in {main}"))
    });
    assert!(ordered.windows(2).all(|pair| pair[0] < pair[1]), "{main}");
    let close = ordered[5]
        + main[ordered[5]..]
            .find('}')
            .unwrap_or_else(|| panic!("Switch is not closed: {main}"));
    assert!(!main[ordered[4]..close].contains("break;"), "{main}");
}

#[test]
fn authentic_v0179_switch_case_truncated_break_value_is_typed_malformed() {
    let trigger = concat!(
        "S_TriggerEditorSET\n",
        "&beforeTriggerExec&\n",
        "Type:28,False,False,False\nscore\nElementsCount:1\n",
        "Type:29,False,False,False\n1\nElementsCount:1\n",
        "Type:8,False,False,False\nact:CreateVariableWithNoini\ncaseCounter\n",
        "ElementsCount:0\nEND\nEND\nEND\n",
        "E_TriggerEditorSET\n",
    );
    let output = parse_e2_bytes(source(trigger).as_bytes(), E2SourceKind::E2s)
        .expect("truncated SwitchCase remains parseable for semantic rejection");
    let error = compile_trigger_tree(&output.document)
        .expect_err("SwitchCase without its break value must not compile");
    assert_eq!(error.code, E2IssueCode::MalformedInput);
    assert_eq!(error.feature, Some(E2Feature::StructGui));
    for fragment in ["SwitchCase", "break", "expects 2", "found 1"] {
        assert!(error.message.contains(fragment), "{error:?}");
    }
}
