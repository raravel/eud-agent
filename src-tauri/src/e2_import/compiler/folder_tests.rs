use super::compile_trigger_tree;
use crate::e2_import::parser::parse_e2_bytes;
use crate::e2_import::{E2RawValue, E2SourceKind};

fn compile(trigger: &str) -> String {
    let parsed = parse_e2_bytes(source(trigger).as_bytes(), E2SourceKind::E2s).unwrap();
    compile_trigger_tree(&parsed.document).unwrap().files["src/main.eps"].clone()
}

fn source(trigger: &str) -> String {
    format!(
        "S_ProjectSET\nVersion : 0.17.9.7\nInputMap : Map/input.scx\nOutputMap : Map/output.scx\nUsedSetting6 : True\nE_ProjectSET\n{trigger}"
    )
}

fn function(folder: &str, wait_enabled: bool) -> String {
    format!(
        "S_TriggerEditorSET\n&functions&\nType:15,False,False,False\nElementsCount:1\nType:16,False,False,False\nwrapped\u{0b90}{}\nElementsCount:2\nType:17,False,False,False\nElementsCount:0\nEND\nType:18,False,False,False\nElementsCount:1\n{folder}END\nEND\nEND\nE_TriggerEditorSET\n",
        if wait_enabled { "True" } else { "False" }
    )
}

#[test]
fn authentic_v0179_folder_literals_preserve_nested_prefix_body_suffix() {
    let folder = concat!(
        "Type:21,False,False,False\nforeach(i:py_range(2)){\u{0b90}} // outer-folder-close\u{0b90}False\nElementsCount:1\n",
        "Type:22,False,False,False\nElementsCount:1\n",
        "Type:21,False,False,False\nEUDIf()(i >= 0);\u{0b90}EUDEndIf();\u{0b90}True\nElementsCount:1\n",
        "Type:22,False,False,False\nElementsCount:1\n",
        "Type:8,False,False,False\nact:RawCode\nnested = i;\nElementsCount:0\nEND\n",
        "END\nEND\nEND\nEND\n",
    );
    let trigger = function(folder, false);
    let parsed = parse_e2_bytes(source(&trigger).as_bytes(), E2SourceKind::E2s).unwrap();
    let definition = &parsed.document.triggers[0].children[0].children[0];
    assert_eq!(definition.values[1], E2RawValue::String("False".into()));
    let outer = &definition.children[1].children[0];
    let inner = &outer.children[0].children[0];
    assert_eq!(outer.values[2], E2RawValue::String("False".into()));
    assert_eq!(inner.values[2], E2RawValue::String("True".into()));

    let main = compile_trigger_tree(&parsed.document).unwrap().files["src/main.eps"].clone();
    let body = &main[main.find("function wrapped() {").unwrap()..];
    let prefix = body.find("foreach(i:py_range(2)){").unwrap();
    let if_prefix = body.find("EUDIf()(i >= 0);").unwrap();
    let body_at = body.find("nested = i;").unwrap();
    let if_suffix = body.find("EUDEndIf();").unwrap();
    let suffix = body.find("} // outer-folder-close").unwrap();
    assert!(
        prefix < if_prefix && if_prefix < body_at && body_at < if_suffix && if_suffix < suffix,
        "{body}"
    );
}

#[test]
fn authentic_v0179_folder_wait_preserves_wrapper_state_order() {
    let folder = concat!(
        "Type:21,False,False,False\nforeach(loc:py_range(2)){\u{0b90}} // wait-folder-close\u{0b90}True\nElementsCount:1\n",
        "Type:22,False,False,False\nElementsCount:3\n",
        "Type:8,False,False,False\nact:RawCode\nbefore = loc;\nElementsCount:0\nEND\n",
        "Type:20,False,False,False\n1\nElementsCount:0\nEND\n",
        "Type:8,False,False,False\nact:RawCode\nafter = loc;\nElementsCount:0\nEND\n",
        "END\nEND\n",
    );
    let trigger = function(folder, true);
    let parsed = parse_e2_bytes(source(&trigger).as_bytes(), E2SourceKind::E2s).unwrap();
    let definition = &parsed.document.triggers[0].children[0].children[0];
    assert_eq!(definition.values[1], E2RawValue::String("True".into()));
    let folder = &definition.children[1].children[0];
    assert_eq!(folder.values[2], E2RawValue::String("True".into()));
    assert_eq!(folder.children[0].children[1].name, "type-20");

    let main = compile_trigger_tree(&parsed.document).unwrap().files["src/main.eps"].clone();
    let body_start = main.find("function wrapped() {").unwrap();
    let body_end = main[body_start..]
        .find("function WaitableTriggerExec()")
        .map(|offset| body_start + offset)
        .unwrap();
    let body = &main[body_start..body_end];
    let state_one = body.find("wrappedTimer[getcurpl()] == 1").unwrap();
    let state_two = body.find("wrappedTimer[getcurpl()] == 2").unwrap();
    let terminal = body.find("wrappedTimer[getcurpl()] = -1;").unwrap();
    let increment = body.find("wrappedTimer[getcurpl()] += 1;").unwrap();
    let first = &body[state_one..state_two];
    let second = &body[state_two..terminal];
    assert!(
        first.contains("foreach(loc:py_range(2)){")
            && first.contains("before = loc;")
            && !first.contains("after = loc;")
            && !first.contains("} // wait-folder-close"),
        "{first}"
    );
    assert!(
        second.contains("} // wait-folder-close")
            && second.contains("after = loc;")
            && !second.contains("before = loc;")
            && !second.contains("foreach(loc:py_range(2)){"),
        "{second}"
    );
    assert!(state_one < state_two && state_two < terminal && terminal < increment);
}

#[test]
fn authentic_v0179_disabled_folder_malformed_descendant_is_inert() {
    let folder = "Type:21,True,False,False\n// off\u{0b90}// end\u{0b90}True\nElementsCount:1\nType:22,False,False,False\nElementsCount:1\nType:20,False,False,False\ninvalid\nElementsCount:0\nEND\nEND\nEND\n";
    let main = compile(&function(folder, false));
    assert_eq!(
        main.matches("name=type-21 kind=Group enabled=false executable=false")
            .count(),
        1
    );
    assert_eq!(
        main.matches("name=type-20 kind=Group enabled=true executable=false")
            .count(),
        1
    );
    assert!(!main.contains("invalid;"), "{main}");
    assert!(!main.contains("wrappedTimer"), "{main}");
}
