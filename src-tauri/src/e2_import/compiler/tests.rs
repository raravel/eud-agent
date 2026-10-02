use super::test_support::{
    catalog_statement, catalog_values, document, lifecycle_document, malformed_button_document,
    malformed_tbl_document, node, persist_evidence, values, with_tbl_override, EUD_FORMS,
    TCT_HELPERS,
};
use super::{compile_trigger_tree, render_tct_helpers};
use crate::e2_import::definitions::{
    action_definitions, condition_definitions, E2SemanticTemplate,
};
use crate::e2_import::{E2Activation, E2Feature, E2IssueCode, E2RawValue, E2TriggerNodeKind};

#[test]
fn full_catalog_tree_oracle() {
    let mut triggers = Vec::new();
    for definition in action_definitions() {
        if definition.identifier != "ChatWrite" && !definition.identifier.starts_with("SCDB:") {
            let values = if definition.identifier == "AddCode" {
                vec![E2RawValue::String("1".to_string())]
            } else {
                catalog_values(definition)
            };
            triggers.push(node(
                E2TriggerNodeKind::Action,
                &format!("action-{}", definition.identifier),
                definition.identifier,
                values,
            ));
        }
    }
    for definition in condition_definitions() {
        if !definition.identifier.starts_with("SCDB:") {
            let values = if definition.identifier == "RawCode" {
                vec![E2RawValue::String("Always()".to_string())]
            } else {
                catalog_values(definition)
            };
            triggers.push(node(
                E2TriggerNodeKind::Condition,
                &format!("condition-{}", definition.identifier),
                definition.identifier,
                values,
            ));
        }
    }

    let compiled = compile_trigger_tree(&document(triggers)).expect("known catalog must compile");
    persist_evidence(&compiled, "full-catalog");
    let main = compiled.files.get("src/main.eps").expect("main EPS");
    assert!(main.contains("import e2.bgm as BGM;"));
    assert_eq!(main.matches("// E2 id=").count(), 146);
    for definition in action_definitions().iter().filter(|definition| {
        definition.identifier != "ChatWrite" && !definition.identifier.starts_with("SCDB:")
    }) {
        assert_eq!(catalog_values(definition).len(), definition.arity());
        if definition.template != E2SemanticTemplate::StructuralMarker {
            assert!(
                !catalog_statement(main, &format!("action-{}", definition.identifier))
                    .starts_with("// E2 id=")
            );
        }
    }
    for definition in condition_definitions()
        .iter()
        .filter(|definition| !definition.identifier.starts_with("SCDB:"))
    {
        assert_eq!(catalog_values(definition).len(), definition.arity());
        assert!(
            catalog_statement(main, &format!("condition-{}", definition.identifier))
                .starts_with("if (")
        );
    }
    assert_eq!(render_tct_helpers(), compiled.files["src/e2/tct.py"]);
    assert!(main.contains("SetMemory(1, 7, 2);"));
    assert!(main.contains("Memory(0x57F1B0, Exactly, 1)"));
    assert!(main.contains("tct.print(\"fixture-text\");"));
    assert!(main.contains("tct.chatAnnouncement(\"fixture-text\");"));
    assert!(main.contains("PlayWAV(\"sound\\\\Terran\\\\fixture.wav\");"));
    for (identifier, semantic) in EUD_FORMS {
        assert!(
            main.contains(semantic),
            "{identifier} did not lower to {semantic}"
        );
    }
    for helper in TCT_HELPERS {
        assert!(
            render_tct_helpers().contains(helper),
            "missing helper {helper}"
        );
    }
}

#[test]
fn recursive_sections_values_and_metadata_oracle() {
    let mut predicate = node(
        E2TriggerNodeKind::Group,
        "predicate",
        "type-3",
        vec![E2RawValue::String("And".to_string())],
    );
    predicate.children.push(node(
        E2TriggerNodeKind::Condition,
        "always",
        "Always",
        vec![E2RawValue::Null],
    ));
    let mut never = node(
        E2TriggerNodeKind::Condition,
        "never",
        "Never",
        vec![E2RawValue::Null],
    );
    never.not_condition = true;
    predicate.children.push(never);
    let mut then_node = node(E2TriggerNodeKind::Group, "then", "type-5", Vec::new());
    then_node.children.push(node(
        E2TriggerNodeKind::Action,
        "text",
        "DisplayText",
        vec![
            E2RawValue::String("fixture-trigger".to_string()),
            E2RawValue::Integer(0),
        ],
    ));
    let mut otherwise = node(E2TriggerNodeKind::Group, "else", "type-6", Vec::new());
    otherwise.children.push(node(
        E2TriggerNodeKind::Action,
        "loss",
        "Defeat",
        vec![E2RawValue::Null],
    ));
    let mut branch = node(E2TriggerNodeKind::Group, "branch", "type-2", Vec::new());
    branch.children = vec![predicate, then_node, otherwise];
    let mut section = node(
        E2TriggerNodeKind::Group,
        "section",
        "section:beforeTriggerExec",
        Vec::new(),
    );
    section.children.push(branch);

    let ir = with_tbl_override(document(vec![section]));
    let compiled = compile_trigger_tree(&ir).expect("recursive section tree");
    persist_evidence(&compiled, "recursive");
    let main = &compiled.files["src/main.eps"];
    assert!(main.contains("if (Always() && !(Never())) {"));
    assert!(main.contains("DisplayText(\"fixture-trigger\", 0);"));
    assert!(main.contains("else {"));
    assert_eq!(main.matches("Always()").count(), 1);
}

#[test]
fn loop_and_lifecycle_oracle() {
    let ir = lifecycle_document();
    let first = compile_trigger_tree(&ir).expect("lifecycle tree");
    let second = compile_trigger_tree(&ir).expect("deterministic lifecycle tree");
    assert_eq!(first, second);
    let main = &first.files["src/main.eps"];
    assert!(main.contains("function onPluginStart()"));
    assert!(main.contains("foreach(ptr, epd : EUDLoopUnit2())"));
    assert!(main.contains("foreach(ptr, epd : EUDLoopPlayerUnit(3))"));
    assert!(!main.contains("lp.EUDLoop"));
}

#[test]
fn unsupported_enabled_vs_disabled_matrix() {
    let chatwrite = node(E2TriggerNodeKind::Action, "chat", "ChatWrite", values(2));
    let error = compile_trigger_tree(&document(vec![chatwrite])).expect_err("active ChatWrite");
    assert_eq!(error.code, E2IssueCode::ActiveChatwriteUnverified);

    let unknown = node(
        E2TriggerNodeKind::Condition,
        "unknown",
        "UnknownCondition",
        Vec::new(),
    );
    let error = compile_trigger_tree(&document(vec![unknown])).expect_err("active unknown");
    assert_eq!(error.code, E2IssueCode::ActiveDefinitionUnknown);

    let scdb = node(
        E2TriggerNodeKind::Action,
        "scdb",
        "SCDB:SaveData",
        values(1),
    );
    let error = compile_trigger_tree(&document(vec![scdb])).expect_err("active SCDB action");
    assert_eq!(error.code, E2IssueCode::ActiveScdbUnsupported);

    let mut disabled = node(E2TriggerNodeKind::Group, "disabled", "type-21", Vec::new());
    disabled.enabled = false;
    disabled.folded = true;
    disabled.not_condition = true;
    disabled.children = vec![
        node(
            E2TriggerNodeKind::Action,
            "disabled-unknown",
            "UnknownAction",
            vec![E2RawValue::String("opaque".to_string())],
        ),
        node(
            E2TriggerNodeKind::Action,
            "disabled-chat",
            "ChatWrite",
            values(2),
        ),
        node(
            E2TriggerNodeKind::Action,
            "disabled-scdb",
            "SCDB:SaveData",
            values(1),
        ),
    ];
    let compiled =
        compile_trigger_tree(&document(vec![disabled])).expect("disabled unknown stays inert");
    let main = &compiled.files["src/main.eps"];
    assert!(main.contains("id=disabled"));
    assert!(main.contains("name=ChatWrite"));
    assert!(main.contains("name=SCDB:SaveData"));
    assert!(main.contains("folded=true"));
    assert!(main.contains("notCondition=true"));
    assert!(main.contains("opaque"));
    assert!(!main.contains("UnknownAction("));

    let error = compile_trigger_tree(&malformed_button_document(E2Activation::Active))
        .expect_err("interior empty BtnData field");
    assert_eq!(error.code, E2IssueCode::MalformedInput);
    assert_eq!(error.feature, Some(E2Feature::StructGui));
    assert_eq!(error.path, None);
    assert_eq!(error.message, "invalid BtnData0");

    let error = compile_trigger_tree(&malformed_tbl_document(E2Activation::Active))
        .expect_err("active malformed TBL metadata");
    assert_eq!(error.code, E2IssueCode::MalformedInput);
    assert_eq!(error.message, "missing stattextdickey0");
    for activation in [E2Activation::Inactive, E2Activation::Inert] {
        compile_trigger_tree(&malformed_button_document(activation)).expect("inactive BtnSET");
        compile_trigger_tree(&malformed_tbl_document(activation)).expect("inactive FileManagerSET");
    }
}
