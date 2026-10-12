use super::compile_trigger_tree;
use super::test_support::{document, node, values};
use crate::e2_import::{E2IssueCode, E2RawValue, E2TriggerNodeKind};

#[test]
fn active_operand_count_mismatch_is_malformed() {
    for (kind, name, count, id) in [
        (E2TriggerNodeKind::Action, "SetMemory", 2, "action-short"),
        (E2TriggerNodeKind::Action, "SetMemory", 4, "action-long"),
        (E2TriggerNodeKind::Condition, "Memory", 2, "condition-short"),
        (E2TriggerNodeKind::Condition, "Memory", 4, "condition-long"),
    ] {
        let error = compile_trigger_tree(&document(vec![node(kind, id, name, values(count))]))
            .expect_err("active operand count mismatch");
        assert_eq!(error.code, E2IssueCode::MalformedInput);
        assert_eq!(
            error.message,
            format!("{name} {id} expects 3 serialized values, found {count}")
        );
    }
}

#[test]
fn none_placeholders_and_real_sample_operands_remain_valid() {
    let mut nested = node(
        E2TriggerNodeKind::Group,
        "nested",
        "type-21",
        vec![
            E2RawValue::String("// nested start".to_string()),
            E2RawValue::String("// nested end".to_string()),
            E2RawValue::String("False".to_string()),
        ],
    );
    nested.children = vec![
        node(
            E2TriggerNodeKind::Action,
            "victory",
            "Victory",
            vec![E2RawValue::Null],
        ),
        node(
            E2TriggerNodeKind::Condition,
            "always",
            "Always",
            vec![E2RawValue::Null],
        ),
        node(
            E2TriggerNodeKind::Action,
            "sample-memory",
            "SetMemoryEPD",
            vec![
                E2RawValue::String("energy".to_string()),
                E2RawValue::String("t/2".to_string()),
                E2RawValue::String("1".to_string()),
            ],
        ),
        node(
            E2TriggerNodeKind::Action,
            "nested-raw",
            "RawCode",
            vec![E2RawValue::String(
                "SetMemoryXEPD(energy, SetTo, 0, t<<24)".to_string(),
            )],
        ),
    ];
    let compiled =
        compile_trigger_tree(&document(vec![nested])).expect("valid serialized operands");
    let main = &compiled.files["src/main.eps"];
    assert!(main.contains("Victory();"));
    assert!(main.contains("if (Always()) {}"));
    assert!(main.contains("SetMemoryEPD(energy, 8, t/2);"));
    assert!(main.contains("SetMemoryXEPD(energy, SetTo, 0, t<<24);"));
}

#[test]
fn disabled_operand_mismatches_remain_inert() {
    let mut direct = node(
        E2TriggerNodeKind::Action,
        "disabled-short",
        "SetMemory",
        values(2),
    );
    direct.enabled = false;
    let mut ancestor = node(
        E2TriggerNodeKind::Group,
        "disabled-parent",
        "type-21",
        Vec::new(),
    );
    ancestor.enabled = false;
    ancestor.children.push(node(
        E2TriggerNodeKind::Condition,
        "inert-long",
        "Memory",
        values(4),
    ));
    let compiled = compile_trigger_tree(&document(vec![direct, ancestor])).expect("inert mismatch");
    let main = &compiled.files["src/main.eps"];
    assert!(main.contains("id=disabled-short"));
    assert!(main.contains("id=inert-long"));
    assert!(!main.contains("SetMemory(1"));
    assert!(!main.contains("if (Memory("));
}
