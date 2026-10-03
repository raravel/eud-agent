use super::compile_trigger_tree;
use crate::e2_import::parser::parse_e2_bytes;
use crate::e2_import::{E2RawValue, E2SourceKind};

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
                .unwrap_or_else(|| panic!("missing {fragment:?}"))
        })
        .collect::<Vec<_>>();
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]), "{text}");
}

#[test]
fn authentic_v0179_function_inventory_preserves_definition_order_and_resolves_forward_call() {
    let trigger = concat!(
        "S_TriggerEditorSET\n&functions&\nType:15,False,False,False\nElementsCount:2\n",
        "Type:16,False,False,False\nZulu\u{0b90}True\nElementsCount:2\n",
        "Type:17,False,False,False\nElementsCount:0\nEND\n",
        "Type:18,False,False,False\nElementsCount:2\n",
        "Type:19,False,False,False\nAlpha\nElementsCount:0\nEND\n",
        "Type:19,False,False,False\nZulu\nElementsCount:0\nEND\nEND\nEND\n",
        "Type:16,False,False,False\nAlpha\u{0b90}True\nElementsCount:2\n",
        "Type:17,False,False,False\nElementsCount:0\nEND\n",
        "Type:18,False,False,False\nElementsCount:1\n",
        "Type:8,False,False,False\nact:RawCode\nvar alphaBody = 1\n",
        "ElementsCount:0\nEND\nEND\nEND\nEND\nE_TriggerEditorSET\n",
    );
    let parsed = parse_e2_bytes(source(trigger).as_bytes(), E2SourceKind::E2s)
        .expect("authentic function inventory wire parses");
    let definitions = &parsed.document.triggers[0].children[0].children;
    assert_eq!(
        definitions
            .iter()
            .map(|node| node.values[0].clone())
            .collect::<Vec<_>>(),
        [
            E2RawValue::String("Zulu".to_string()),
            E2RawValue::String("Alpha".to_string()),
        ]
    );

    let compiled = compile_trigger_tree(&parsed.document).expect("forward and self calls compile");
    let main = &compiled.files["src/main.eps"];
    let zulu = main.find("function Zulu() {").expect("Zulu definition");
    let alpha = main.find("function Alpha() {").expect("Alpha definition");
    assert!(zulu < alpha, "{main}");
    assert_ordered(&main[zulu..alpha], &["Alpha();", "Zulu();"]);
    let aggregate = &main[main
        .find("function WaitableTriggerExec()")
        .expect("aggregate")..];
    assert_ordered(aggregate, &["Zulu();", "Alpha();"]);
}

#[test]
fn disabled_switch_case_with_truncated_break_stays_inert() {
    let trigger = concat!(
        "S_TriggerEditorSET\n&beforeTriggerExec&\n",
        "Type:28,False,False,False\nselector\nElementsCount:2\n",
        "Type:29,True,False,False\n7\nElementsCount:1\n",
        "Type:8,False,False,False\nact:RawCode\nvar disabledOnly = 1\n",
        "ElementsCount:0\nEND\nEND\n",
        "Type:29,False,False,False\n9\u{0b90}True\nElementsCount:1\n",
        "Type:8,False,False,False\nact:RawCode\nvar activeCase = 1\n",
        "ElementsCount:0\nEND\nEND\nEND\nE_TriggerEditorSET\n",
    );
    let parsed = parse_e2_bytes(source(trigger).as_bytes(), E2SourceKind::E2s)
        .expect("authentic disabled malformed SwitchCase parses");
    let switch = &parsed.document.triggers[0].children[0];
    let disabled = &switch.children[0];
    assert!(!disabled.enabled);
    assert_eq!(disabled.values, [E2RawValue::String("7".to_string())]);

    let compiled =
        compile_trigger_tree(&parsed.document).expect("disabled malformed case is inert");
    let main = &compiled.files["src/main.eps"];
    assert!(main.contains(&format!(
        "id={} name=type-29 kind=Group enabled=false executable=false",
        disabled.id
    )));
    assert!(!main.contains("var disabledOnly = 1;"), "{main}");
    assert_ordered(
        main,
        &[
            "switch(selector) {",
            "case 9:",
            "var activeCase = 1;",
            "break;",
        ],
    );
}
