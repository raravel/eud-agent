use super::compile_trigger_tree;
use crate::e2_import::parser::parse_e2_bytes;
use crate::e2_import::{E2RawValue, E2SourceKind};

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
fn authentic_v0179_direct_function_inventory_predeclares_forward_and_self_calls() {
    let source = concat!(
        "S_ProjectSET\nVersion : 0.17.9.7\nInputMap : Map/input.scx\n",
        "OutputMap : Map/output.scx\nUsedSetting6 : True\nE_ProjectSET\n",
        "S_TriggerEditorSET\n&functions&\nType:15,False,False,False\nElementsCount:2\n",
        "Type:16,False,False,False\nZulu\u{0b90}False\nElementsCount:2\n",
        "Type:17,False,False,False\nElementsCount:0\nEND\n",
        "Type:18,False,False,False\nElementsCount:2\n",
        "Type:19,False,False,False\nAlpha\nElementsCount:0\nEND\n",
        "Type:19,False,False,False\nZulu\nElementsCount:0\nEND\nEND\nEND\n",
        "Type:16,False,False,False\nAlpha\u{0b90}False\nElementsCount:2\n",
        "Type:17,False,False,False\nElementsCount:0\nEND\n",
        "Type:18,False,False,False\nElementsCount:1\n",
        "Type:8,False,False,False\nact:RawCode\nvar alphaBody = 1\n",
        "ElementsCount:0\nEND\nEND\nEND\nEND\nE_TriggerEditorSET\n",
    );
    let parsed = parse_e2_bytes(source.as_bytes(), E2SourceKind::E2s)
        .expect("authentic direct-function wire parses");
    let definitions = &parsed.document.triggers[0].children[0].children;
    assert_eq!(definitions[0].values[1], E2RawValue::String("False".into()));
    assert_eq!(definitions[1].values[1], E2RawValue::String("False".into()));

    let compiled = compile_trigger_tree(&parsed.document).expect("direct functions compile");
    let main = &compiled.files["src/main.eps"];
    let zulu = main.find("function Zulu() {").expect("Zulu definition");
    let alpha = main.find("function Alpha() {").expect("Alpha definition");
    assert_ordered(&main[..zulu], &["function Zulu();", "function Alpha();"]);
    assert!(zulu < alpha, "{main}");
    assert_ordered(&main[zulu..alpha], &["Alpha();", "Zulu();"]);
    assert!(main[alpha..].contains("var alphaBody = 1;"));
    for omitted in ["ZuluTimer", "AlphaTimer", "WaitableTriggerExec"] {
        assert!(!main.contains(omitted), "unexpected {omitted:?} in {main}");
    }
}

fn wait_enabled_forward_wire() -> &'static str {
    concat!(
        "S_ProjectSET\nVersion : 0.17.9.7\nInputMap : Map/input.scx\nOutputMap : Map/output.scx\nUsedSetting6 : True\nE_ProjectSET\n",
        "S_TriggerEditorSET\n&functions&\nType:15,False,False,False\nElementsCount:2\nType:16,False,False,False\nZulu\u{0b90}True\nElementsCount:2\n",
        "Type:17,False,False,False\nElementsCount:1\nType:8,False,False,False\nact:CreateVariableWithNoini\nseed\u{0b90}0\nElementsCount:0\nEND\nEND\n",
        "Type:18,False,False,False\nElementsCount:2\nType:19,False,False,False\nAlpha\u{0b90}seed\nElementsCount:0\nEND\n",
        "Type:19,False,False,False\nZulu\u{0b90}seed\nElementsCount:0\nEND\nEND\nEND\n",
        "Type:16,False,False,False\nAlpha\u{0b90}True\nElementsCount:2\nType:17,False,False,False\nElementsCount:1\n",
        "Type:8,False,False,False\nact:CreateVariableWithNoini\namount\u{0b90}0\nElementsCount:0\nEND\nEND\nType:18,False,False,False\nElementsCount:1\n",
        "Type:8,False,False,False\nact:RawCode\nvar alphaBody = amount\nElementsCount:0\nEND\nEND\nEND\nEND\nE_TriggerEditorSET\n",
    )
}

#[test]
fn authentic_v0179_wait_state_arrays_precede_forward_function_body() {
    let parsed = parse_e2_bytes(wait_enabled_forward_wire().as_bytes(), E2SourceKind::E2s)
        .expect("authentic wait-enabled forward-call wire parses");
    let compiled = compile_trigger_tree(&parsed.document).expect("wait-enabled functions compile");
    let main = &compiled.files["src/main.eps"];
    let zulu = main.find("function Zulu(seed) {").expect("Zulu definition");
    let alpha = main.find("function Alpha(amount) {").expect("Alpha");
    for declaration in [
        "const Zuluseed = [",
        "const ZuluTimer = [",
        "const Alphaamount = [",
        "const AlphaTimer = [",
        "function Zulu(seed);",
        "function Alpha(amount);",
    ] {
        assert!(main[..zulu].contains(declaration), "{main}");
    }
    assert_ordered(
        &main[zulu..alpha],
        &[
            "Alphaamount[getcurpl()] = seed;",
            "AlphaTimer[getcurpl()] = 1;",
            "Alpha(Alphaamount[getcurpl()]);",
            "Zuluseed[getcurpl()] = seed;",
            "ZuluTimer[getcurpl()] = 1;",
            "Zulu(Zuluseed[getcurpl()]);",
        ],
    );
    let aggregate = &main[main
        .find("function WaitableTriggerExec()")
        .expect("aggregate")..];
    assert_ordered(
        aggregate,
        &[
            "Zulu(Zuluseed[getcurpl()]);",
            "Alpha(Alphaamount[getcurpl()]);",
        ],
    );
}
