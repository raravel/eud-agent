use super::super::definitions::{
    condition_definition, structural_node_kind, E2Definition, E2SemanticTemplate,
    E2StructuralNodeKind,
};
use super::super::{
    E2Feature, E2ImportIssue, E2Issue, E2IssueCode, E2IssueSeverity, E2RawValue, E2TriggerNode,
    E2TriggerNodeKind,
};
use super::eud;
use super::metadata::CompileMetadata;
pub(super) use super::values::{arg, raw, value_expression};

pub(super) fn action(
    node: &E2TriggerNode,
    definition: &E2Definition,
    metadata: &CompileMetadata,
) -> Result<Option<String>, E2ImportIssue> {
    validate_serialized_values(node, definition)?;
    let body = match definition.template {
        E2SemanticTemplate::UnsupportedChatWrite => {
            return Err(issue(E2IssueCode::ActiveChatwriteUnverified, node))
        }
        E2SemanticTemplate::Scdb => return Err(issue(E2IssueCode::ActiveScdbUnsupported, node)),
        E2SemanticTemplate::StructuralMarker => return Ok(None),
        E2SemanticTemplate::RawCode => raw(node, 0).to_string(),
        E2SemanticTemplate::Bgm => call(
            &format!("BGM.{}", node.name.trim_start_matches("BGM")),
            node,
            definition,
        ),
        E2SemanticTemplate::CustomText => custom_text(node, definition),
        E2SemanticTemplate::Builtin => call(
            if node.name == "Wait(Don't Use)" {
                "Wait"
            } else {
                &node.name
            },
            node,
            definition,
        ),
        E2SemanticTemplate::EudExtension => eud::action(node, definition, metadata)?,
    };
    Ok(Some(format!("{body};")))
}

pub(super) fn condition(
    node: &E2TriggerNode,
    definition: &E2Definition,
    metadata: &CompileMetadata,
) -> Result<String, E2ImportIssue> {
    validate_serialized_values(node, definition)?;
    let body = match definition.template {
        E2SemanticTemplate::Scdb => return Err(issue(E2IssueCode::ActiveScdbUnsupported, node)),
        E2SemanticTemplate::RawCode => raw(node, 0).to_string(),
        E2SemanticTemplate::Bgm => call(
            if node.name == "CurrentBGM" {
                "BGM.CurrentPlayMusic"
            } else {
                "BGM.isplaying"
            },
            node,
            definition,
        ),
        E2SemanticTemplate::StructuralMarker => "Always()".to_string(),
        E2SemanticTemplate::Builtin if node.name == "Briefing" => {
            "Condition(0, 0, 0, 0, 0, 13, 0, 0)".to_string()
        }
        E2SemanticTemplate::Builtin => call(&node.name, node, definition),
        E2SemanticTemplate::EudExtension => eud::condition(node, definition, metadata)?,
        E2SemanticTemplate::CustomText | E2SemanticTemplate::UnsupportedChatWrite => {
            return Err(unknown(node))
        }
    };
    Ok(if node.not_condition {
        format!("!({body})")
    } else {
        body
    })
}

fn custom_text(node: &E2TriggerNode, definition: &E2Definition) -> String {
    let (name, indices): (&str, &[usize]) = match node.name.as_str() {
        "PrintError" => ("printError", &[0]),
        "ChatAnnouncement" => ("chatAnnouncement", &[1]),
        "DisplayCText" => ("print", &[0]),
        "MakeCText" => ("makeText", &[0]),
        "AddCText" | "AddCode" => ("addText", &[0]),
        "DisplaySavedCText" => ("displayText", &[]),
        _ => (&node.name, &[]),
    };
    format!(
        "tct.{name}({})",
        indices
            .iter()
            .map(|index| arg(node, definition, *index))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

pub(super) fn call(name: &str, node: &E2TriggerNode, definition: &E2Definition) -> String {
    let args = definition
        .value_kinds
        .iter()
        .enumerate()
        .filter(|(_, kind)| **kind != "None")
        .map(|(index, _)| arg(node, definition, index))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{name}({args})")
}

pub(super) fn condition_expression(
    node: &E2TriggerNode,
    metadata: &CompileMetadata,
) -> Result<String, E2ImportIssue> {
    if node.kind == E2TriggerNodeKind::Condition {
        let definition = condition_definition(&node.name).ok_or_else(|| unknown(node))?;
        return condition(node, definition, metadata);
    }
    let kind = node
        .name
        .strip_prefix("type-")
        .and_then(|code| code.parse().ok())
        .and_then(structural_node_kind);
    if kind == Some(E2StructuralNodeKind::Function) {
        let name = raw(node, 0);
        if name == "0" {
            return Err(unknown(node));
        }
        let args = node
            .values
            .iter()
            .skip(1)
            .map(|value| value_expression(value))
            .collect::<Vec<_>>()
            .join(", ");
        let expression = format!("{name}({args})");
        return Ok(if node.not_condition {
            format!("!({expression})")
        } else {
            expression
        });
    }
    if !matches!(
        kind,
        Some(
            E2StructuralNodeKind::ConditionClause
                | E2StructuralNodeKind::WhileCondition
                | E2StructuralNodeKind::TriggerCondition
        )
    ) {
        return Err(unknown(node));
    }
    let parts = node
        .children
        .iter()
        .filter(|child| child.enabled)
        .map(|child| condition_expression(child, metadata))
        .collect::<Result<Vec<_>, _>>()?;
    if parts.is_empty() {
        return Err(unknown(node));
    }
    let joiner = if raw(node, 0).eq_ignore_ascii_case("And") {
        " && "
    } else {
        " || "
    };
    let expression = parts.join(joiner);
    Ok(if node.not_condition {
        format!("!({expression})")
    } else {
        expression
    })
}

pub(super) fn integer(input: Option<&E2RawValue>) -> Option<i64> {
    match input {
        Some(E2RawValue::Integer(value)) => Some(*value),
        Some(E2RawValue::String(value)) => value.parse().ok(),
        _ => None,
    }
}

fn validate_serialized_values(
    node: &E2TriggerNode,
    definition: &E2Definition,
) -> Result<(), E2ImportIssue> {
    let expected = definition.arity();
    let found = node.values.len();
    if found != expected {
        return Err(malformed(format!(
            "{} {} expects {expected} serialized values, found {found}",
            node.name, node.id
        )));
    }
    Ok(())
}

fn issue(code: E2IssueCode, node: &E2TriggerNode) -> E2Issue {
    E2Issue {
        code,
        severity: E2IssueSeverity::Fatal,
        message: format!(
            "active E2 definition {} at {} is unsupported",
            node.name, node.id
        ),
        path: None,
        feature: Some(E2Feature::StructGui),
        need_id: None,
    }
}

pub(super) fn malformed(message: String) -> E2Issue {
    E2Issue {
        code: E2IssueCode::MalformedInput,
        severity: E2IssueSeverity::Fatal,
        message,
        path: None,
        feature: Some(E2Feature::StructGui),
        need_id: None,
    }
}

pub(super) fn unknown(node: &E2TriggerNode) -> E2Issue {
    issue(E2IssueCode::ActiveDefinitionUnknown, node)
}
