use super::parse_e2_bytes;
use crate::e2_import::{E2Activation, E2Feature, E2IssueCode, E2SourceKind};

fn source(condition_disabled: bool) -> String {
    format!(
        concat!(
            "S_ProjectSET\nVersion : 0.17.9.7\nInputMap : Map/input.scx\n",
            "OutputMap : Map/output.scx\nUsedSetting6 : True\nE_ProjectSET\n",
            "S_TriggerEditorSET\n&RawTriggers&\n",
            "Type:25,False,False,False\nElementsCount:1\n",
            "Type:24,False,False,False\n1\nElementsCount:2\n",
            "Type:26,{condition_disabled},False,False\nAnd\nElementsCount:1\n",
            "Type:7,False,False,False\ncon:Always\n0\nElementsCount:0\nEND\nEND\n",
            "Type:27,False,False,False\nElementsCount:2\n",
            "Type:8,False,False,False\nact:SCDB:Exec\nvalue\nElementsCount:0\nEND\n",
            "Type:8,False,False,False\nact:ChatWrite\nvalue\nElementsCount:0\nEND\n",
            "END\nEND\nEND\nE_TriggerEditorSET\n",
        ),
        condition_disabled = if condition_disabled { "True" } else { "False" }
    )
}

fn activation(output: &super::E2ParseOutput, feature: E2Feature) -> E2Activation {
    output
        .document
        .features
        .iter()
        .find(|state| state.feature == feature)
        .expect("feature inventory")
        .activation
}

#[test]
fn authentic_v0179_disabled_raw_condition_makes_sibling_actions_inert() {
    let output = parse_e2_bytes(source(true).as_bytes(), E2SourceKind::E2s)
        .expect("disabled RAW condition wire parses");
    assert_eq!(activation(&output, E2Feature::Scdb), E2Activation::Inactive);
    assert!(!output.issues.iter().any(|issue| matches!(
        issue.code,
        E2IssueCode::ActiveScdbUnsupported | E2IssueCode::ActiveChatwriteUnverified
    )));

    let raw = &output.document.triggers[0].children[0].children[0];
    assert_eq!(raw.name, "type-24");
    assert!(!raw.children[0].enabled, "condition wrapper is disabled");
    assert_eq!(raw.children[1].children[0].name, "SCDB:Exec");
    assert_eq!(raw.children[1].children[1].name, "ChatWrite");
}

#[test]
fn authentic_v0179_enabled_raw_condition_keeps_both_typed_refusals() {
    let output = parse_e2_bytes(source(false).as_bytes(), E2SourceKind::E2s)
        .expect("enabled RAW condition wire parses");
    assert_eq!(activation(&output, E2Feature::Scdb), E2Activation::Active);
    for code in [
        E2IssueCode::ActiveScdbUnsupported,
        E2IssueCode::ActiveChatwriteUnverified,
    ] {
        assert!(
            output.issues.iter().any(|issue| issue.code == code),
            "{code:?}"
        );
    }
}
