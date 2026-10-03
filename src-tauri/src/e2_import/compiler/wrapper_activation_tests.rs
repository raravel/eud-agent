use super::compile_trigger_tree;
use crate::e2_import::parser::parse_e2_bytes;
use crate::e2_import::E2SourceKind;

fn source(trigger: &str) -> String {
    format!(
        "S_ProjectSET\nVersion : 0.17.9.7\nInputMap : Map/input.scx\nOutputMap : Map/output.scx\nUsedSetting6 : True\nE_ProjectSET\n{trigger}"
    )
}

fn compile(trigger: &str) -> String {
    let parsed = parse_e2_bytes(source(trigger).as_bytes(), E2SourceKind::E2s)
        .expect("authentic disabled-wrapper wire parses");
    compile_trigger_tree(&parsed.document)
        .expect("disabled wrapper payload is inert")
        .files["src/main.eps"]
        .clone()
}

#[test]
fn disabled_structural_wrappers_are_inert_without_validating_payloads() {
    let disabled_code = concat!(
        "S_TriggerEditorSET\n&functions&\nType:15,False,False,False\nElementsCount:1\n",
        "Type:16,False,False,False\ndirect\u{0b90}False\nElementsCount:2\n",
        "Type:17,False,False,False\nElementsCount:0\nEND\n",
        "Type:18,True,False,False\nElementsCount:1\n",
        "Type:8,False,False,False\nact:MissingDisabledAction\n0\n",
        "ElementsCount:0\nEND\nEND\nEND\nEND\n",
        "&beforeTriggerExec&\nType:0,False,False,False\nElementsCount:1\n",
        "Type:19,False,False,False\ndirect\nElementsCount:0\nEND\n",
        "END\nE_TriggerEditorSET\n",
    );
    let main = compile(disabled_code);
    assert!(main.contains("function direct() {"), "{main}");
    assert!(main.contains("direct();"), "{main}");
    assert!(main.contains("name=type-18 kind=Group enabled=false executable=false"));
    assert!(main.contains("name=MissingDisabledAction kind=Action enabled=true executable=false"));
    assert!(!main.contains("MissingDisabledAction();"), "{main}");

    let disabled_actions = concat!(
        "S_TriggerEditorSET\n&RawTriggers&\n",
        "Type:25,False,False,False\nElementsCount:1\n",
        "Type:24,False,False,False\n1\nElementsCount:2\n",
        "Type:26,False,False,False\nAnd\nElementsCount:1\n",
        "Type:7,False,False,False\ncon:Always\n0\nElementsCount:0\nEND\nEND\n",
        "Type:27,True,False,False\nElementsCount:2\n",
        "Type:8,False,False,False\nact:MissingDisabledAction\n0\n",
        "ElementsCount:0\nEND\n",
        "Type:8,False,False,False\nact:PreserveTrigger\n0\n",
        "ElementsCount:0\nEND\nEND\nEND\nEND\nE_TriggerEditorSET\n",
    );
    let main = compile(disabled_actions);
    assert!(main.contains("name=type-27 kind=Group enabled=false executable=false"));
    assert!(main.contains("name=PreserveTrigger kind=Action enabled=true executable=false"));
    assert!(main.contains("function ClassicTriggerExec0()"), "{main}");
    assert!(!main.contains("MissingDisabledAction();"), "{main}");
    assert!(!main.contains("PreserveTrigger();"), "{main}");
    assert!(!main.contains("ClassicTriggerExecTimer0[getcurpl()] = 0;"));

    let disabled_conditions = concat!(
        "S_TriggerEditorSET\n&RawTriggers&\n",
        "Type:25,False,False,False\nElementsCount:1\n",
        "Type:24,False,False,False\n1\nElementsCount:2\n",
        "Type:26,True,False,False\nAnd\nElementsCount:1\n",
        "Type:7,False,False,False\ncon:MissingDisabledCondition\n0\n",
        "ElementsCount:0\nEND\nEND\n",
        "Type:27,False,False,False\nElementsCount:1\n",
        "Type:8,False,False,False\nact:RawCode\nvar mustStayInert = 1\n",
        "ElementsCount:0\nEND\nEND\nEND\nEND\nE_TriggerEditorSET\n",
    );
    let main = compile(disabled_conditions);
    assert!(main.contains("name=type-26 kind=Group enabled=false executable=false"));
    assert!(
        main.contains("name=MissingDisabledCondition kind=Condition enabled=true executable=false")
    );
    assert!(main.contains("name=RawCode kind=Action enabled=true executable=false"));
    assert!(!main.contains("function ClassicTriggerExec0()"), "{main}");
    assert!(
        !main.contains("function ClassicTriggerStarter0()"),
        "{main}"
    );
    assert!(!main.contains("var mustStayInert = 1;"), "{main}");
}

#[test]
fn authentic_v0179_folder_nested_wait_preserves_state_order_and_disabled_folder_is_inert() {
    let trigger = concat!(
        "S_TriggerEditorSET\n&functions&\nType:15,False,False,False\nElementsCount:1\n",
        "Type:16,False,False,False\nnested\u{0b90}True\nElementsCount:2\n",
        "Type:17,False,False,False\nElementsCount:0\nEND\n",
        "Type:18,False,False,False\nElementsCount:4\n",
        "Type:8,False,False,False\nact:RawCode\nvar outerBefore = 1\n",
        "ElementsCount:0\nEND\n",
        "Type:21,False,False,False\n// folder start\u{0b90}// folder end\u{0b90}False\n",
        "ElementsCount:1\nType:22,False,False,False\nElementsCount:3\n",
        "Type:8,False,False,False\nact:RawCode\nvar innerBefore = 1\n",
        "ElementsCount:0\nEND\n",
        "Type:20,False,False,False\n2\nElementsCount:0\nEND\n",
        "Type:8,False,False,False\nact:RawCode\nvar innerAfter = 1\n",
        "ElementsCount:0\nEND\nEND\nEND\n",
        "Type:8,False,False,False\nact:RawCode\nvar outerAfter = 1\n",
        "ElementsCount:0\nEND\n",
        "Type:21,True,False,False\n// disabled\u{0b90}// disabled end\u{0b90}False\n",
        "ElementsCount:1\nType:22,False,False,False\nElementsCount:1\n",
        "Type:20,False,False,False\nnot-a-duration\nElementsCount:0\nEND\n",
        "END\nEND\nEND\nEND\nEND\nE_TriggerEditorSET\n",
    );
    let parsed = parse_e2_bytes(source(trigger).as_bytes(), E2SourceKind::E2s)
        .expect("authentic folder-contained WAIT wire parses");
    let code = &parsed.document.triggers[0].children[0].children[0].children[1];
    assert_eq!(code.children[1].name, "type-21");
    assert_eq!(code.children[1].children[0].name, "type-22");
    assert_eq!(code.children[1].children[0].children[1].name, "type-20");
    assert!(!code.children[3].enabled);

    let compiled = compile_trigger_tree(&parsed.document).expect("nested WAIT compiles");
    let main = &compiled.files["src/main.eps"];
    let function = &main[main.find("function nested() {").expect("definition")..];
    let state_one = function.find("nestedTimer[getcurpl()] == 1").unwrap();
    let state_three = function.find("nestedTimer[getcurpl()] == 3").unwrap();
    let terminal = function.find("nestedTimer[getcurpl()] = -1;").unwrap();
    let first = &function[state_one..state_three];
    assert!(first.contains("var outerBefore = 1;") && first.contains("var innerBefore = 1;"));
    assert!(!first.contains("var innerAfter = 1;") && !first.contains("var outerAfter = 1;"));
    let second = &function[state_three..terminal];
    assert!(second.contains("var innerAfter = 1;") && second.contains("var outerAfter = 1;"));
    assert!(!second.contains("var outerBefore = 1;") && !second.contains("var innerBefore = 1;"));
    assert!(main.contains("name=type-21 kind=Group enabled=false executable=false"));
    assert!(main.contains("name=type-20 kind=Group enabled=true executable=false"));
    assert!(!main.contains("not-a-duration;"), "{main}");
}

#[test]
fn authentic_v0179_disabled_raw_condition_skips_malformed_function_inventory() {
    let trigger = concat!(
        "S_TriggerEditorSET\n&RawTriggers&\n",
        "Type:25,False,False,False\nElementsCount:1\n",
        "Type:24,False,False,False\n1\nElementsCount:2\n",
        "Type:26,True,False,False\nAnd\nElementsCount:1\n",
        "Type:7,False,False,False\ncon:Always\n0\nElementsCount:0\nEND\nEND\n",
        "Type:27,False,False,False\nElementsCount:1\n",
        "Type:16,False,False,False\nmalformedOnly\nElementsCount:0\nEND\n",
        "END\nEND\nEND\nE_TriggerEditorSET\n",
    );
    let main = compile(trigger);
    assert!(main.contains("name=type-24 kind=Group enabled=true executable=false"));
    assert!(main.contains("name=type-16 kind=Group enabled=true executable=false"));
    assert!(!main.contains("function malformedOnly("), "{main}");
    assert!(!main.contains("function ClassicTriggerExec0()"), "{main}");
    assert!(
        !main.contains("function ClassicTriggerStarter0()"),
        "{main}"
    );
}
