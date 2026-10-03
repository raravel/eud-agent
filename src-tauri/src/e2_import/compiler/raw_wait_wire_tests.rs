use super::compile_trigger_tree;
use crate::e2_import::parser::parse_e2_bytes;
use crate::e2_import::{E2RawValue, E2SourceKind};

fn source(trigger: &str) -> String {
    format!(
        "S_ProjectSET\nVersion : 0.17.9.7\nInputMap : Map/input.scx\nOutputMap : Map/output.scx\nUsedSetting6 : True\nE_ProjectSET\n{trigger}"
    )
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
fn authentic_v0179_raw_wait_state_chain_preserves_scope_and_lifecycle_order() {
    let trigger = concat!(
        "S_TriggerEditorSET\n",
        "&functions&\nType:15,False,False,False\nElementsCount:1\n",
        "Type:16,False,False,False\npulse\u{0b90}True\nElementsCount:2\n",
        "Type:17,False,False,False\nElementsCount:0\nEND\n",
        "Type:18,False,False,False\nElementsCount:1\n",
        "Type:8,False,False,False\nact:RawCode\nvar pulseTick = 1\n",
        "ElementsCount:0\nEND\nEND\nEND\nEND\n",
        "&RawTriggers&\nType:25,False,False,False\nElementsCount:1\n",
        "Type:24,False,False,False\n1\nElementsCount:2\n",
        "Type:26,False,False,False\nAnd\nElementsCount:1\n",
        "Type:7,False,False,False\ncon:Always\n0\nElementsCount:0\nEND\nEND\n",
        "Type:27,False,False,False\nElementsCount:4\n",
        "Type:8,False,False,False\nact:RawCode\nvar rawBefore = 1\nElementsCount:0\nEND\n",
        "Type:20,False,False,False\n2\nElementsCount:0\nEND\n",
        "Type:8,False,False,False\nact:RawCode\nvar rawAfter = 1\nElementsCount:0\nEND\n",
        "Type:8,False,False,False\nact:PreserveTrigger\n0\nElementsCount:0\nEND\n",
        "END\nEND\nEND\nE_TriggerEditorSET\n",
    );
    let output = parse_e2_bytes(source(trigger).as_bytes(), E2SourceKind::E2s)
        .expect("authentic joint RAW and WAIT wire parses");
    let function = &output.document.triggers[0].children[0].children[0];
    assert_eq!(
        function.values,
        [
            E2RawValue::String("pulse".to_string()),
            E2RawValue::String("True".to_string()),
        ]
    );
    let raw = &output.document.triggers[1].children[0].children[0];
    assert_eq!(raw.values, vec![E2RawValue::String("1".to_string())]);
    assert_eq!(raw.children[0].name, "type-26");
    assert_eq!(raw.children[1].name, "type-27");
    let actions = &raw.children[1].children;
    assert_eq!(actions[0].name, "RawCode");
    assert_eq!(actions[1].name, "type-20");
    assert_eq!(actions[2].name, "RawCode");
    assert_eq!(actions[3].name, "PreserveTrigger");
    assert_eq!(actions[1].values, vec![E2RawValue::String("2".to_string())]);

    let compiled = compile_trigger_tree(&output.document).expect("joint RAW and WAIT compiles");
    let main = &compiled.files["src/main.eps"];
    let executor_start = main.find("function ClassicTriggerExec0()").unwrap();
    let starter_start = main.find("function ClassicTriggerStarter0()").unwrap();
    let executor = &main[executor_start..starter_start];
    let state_one = executor.find(" == 1").unwrap();
    let state_three = executor.find(" == 3").unwrap();
    let reset = executor
        .find("ClassicTriggerExecTimer0[getcurpl()] = 0;")
        .unwrap();
    assert!(executor[state_one..state_three].contains("var rawBefore = 1;"));
    assert!(!executor[state_one..state_three].contains("var rawAfter = 1;"));
    assert!(executor[state_three..reset].contains("var rawAfter = 1;"));
    assert!(!executor[state_three..reset].contains("var rawBefore = 1;"));
    assert!(executor[state_three..reset].contains("PreserveTrigger();"));
    assert!(!executor.contains("ClassicTriggerExecTimer0[getcurpl()] = -1;"));
    assert_ordered(
        &executor[reset..],
        &[
            "ClassicTriggerExecTimer0[getcurpl()] = 0;",
            "ClassicTriggerExecTimer0[getcurpl()] > 0",
            "ClassicTriggerExecTimer0[getcurpl()] += 1;",
        ],
    );
    let starter = &main[starter_start..];
    assert!(starter.contains("Always()"));
    assert_ordered(
        starter,
        &[
            "Timer0[getcurpl()] == 0",
            "Timer0[getcurpl()] = 1;",
            "ClassicTriggerExec0();",
        ],
    );
    assert_eq!(main.matches("ClassicTriggerStarter0();").count(), 1);
    let p1 = &main[main.find("playerexist(0)").unwrap()..main.find("playerexist(1)").unwrap()];
    assert_ordered(
        p1,
        &[
            "playerexist(0)",
            "setcurpl(0);",
            "ClassicTriggerStarter0();",
        ],
    );
    let wait_start = main.find("function WaitableTriggerExec()").unwrap();
    let wait_aggregate = &main[wait_start..];
    let wait_aggregate = &wait_aggregate[..wait_aggregate.find("\n}").unwrap()];
    assert!(wait_aggregate.contains("pulse();"));
    let hook = &main[main.find("function beforeTriggerExec()").unwrap()..];
    assert_ordered(
        hook,
        &[
            "EUDPlayerLoop()();",
            "WaitableTriggerExec();",
            "ClassicTriggerExec();",
            "EUDEndPlayerLoop();",
            "ClassicTriggerStarter();",
        ],
    );
}
