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
fn authentic_v0179_wait_enabled_disabled_code_keeps_state_surface_inert() {
    let source = concat!(
        "S_ProjectSET\nVersion : 0.17.9.7\nInputMap : Map/input.scx\n",
        "OutputMap : Map/output.scx\nUsedSetting6 : True\nE_ProjectSET\n",
        "S_TriggerEditorSET\n&functions&\nType:15,False,False,False\nElementsCount:1\n",
        "Type:16,False,False,False\ndormant\u{0b90}True\nElementsCount:2\n",
        "Type:17,False,False,False\nElementsCount:1\n",
        "Type:8,False,False,False\nact:CreateVariableWithNoini\namount\u{0b90}0\n",
        "ElementsCount:0\nEND\nEND\n",
        "Type:18,True,False,False\nElementsCount:1\n",
        "Type:8,False,False,False\nact:MissingDisabledAction\n0\n",
        "ElementsCount:0\nEND\nEND\nEND\nEND\n",
        "&beforeTriggerExec&\nType:0,False,False,False\nElementsCount:1\n",
        "Type:19,False,False,False\ndormant\u{0b90}7\nElementsCount:0\nEND\n",
        "END\nE_TriggerEditorSET\n",
    );
    let parsed = parse_e2_bytes(source.as_bytes(), E2SourceKind::E2s)
        .expect("authentic wait-enabled disabled-code wire parses");
    let definition = &parsed.document.triggers[0].children[0].children[0];
    let code = &definition.children[1];
    assert_eq!(definition.values[1], E2RawValue::String("True".into()));
    assert!(!code.enabled);
    assert_eq!(code.children[0].name, "MissingDisabledAction");

    let compiled = compile_trigger_tree(&parsed.document)
        .expect("disabled code payload stays inert while WAIT state compiles");
    let main = &compiled.files["src/main.eps"];
    let body = main.find("function dormant(amount) {").expect("definition");
    let aggregate = main
        .find("function WaitableTriggerExec()")
        .expect("aggregate");
    for declaration in [
        "const dormantamount = [0, 0, 0, 0, 0, 0, 0, 0];",
        "const dormantTimer = [0, 0, 0, 0, 0, 0, 0, 0];",
        "function dormant(amount);",
    ] {
        assert!(main[..body].contains(declaration), "{main}");
    }
    assert_ordered(
        &main[body..aggregate],
        &[
            "dormantTimer[getcurpl()] > 0",
            "dormantTimer[getcurpl()] == 1",
            "dormantTimer[getcurpl()] = -1;",
            "dormantTimer[getcurpl()] += 1;",
        ],
    );
    assert_eq!(
        main.matches("name=type-18 kind=Group enabled=false executable=false")
            .count(),
        1
    );
    assert_eq!(
        main.matches("name=MissingDisabledAction kind=Action enabled=true executable=false")
            .count(),
        1
    );
    assert!(!main.contains("MissingDisabledAction();"), "{main}");
    let hook = &main[main
        .find("function beforeTriggerExec()")
        .expect("before hook")..];
    assert_ordered(
        hook,
        &[
            "EUDPlayerLoop()();",
            "WaitableTriggerExec();",
            "EUDEndPlayerLoop();",
            "if (dormantTimer[getcurpl()] == 0) {",
            "dormantamount[getcurpl()] = 7;",
            "dormantTimer[getcurpl()] = 1;",
            "dormant(dormantamount[getcurpl()]);",
        ],
    );
    let aggregate = &main[aggregate..];
    let aggregate = &aggregate[..aggregate.find("\n}").expect("aggregate close")];
    assert!(aggregate.contains("dormant(dormantamount[getcurpl()]);"));
}
