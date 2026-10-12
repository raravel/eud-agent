use super::super::{
    E2Feature, E2ImportIssue, E2IssueCode, E2IssueSeverity, E2RawValue, E2TriggerNode,
};
use super::render::line;
use super::tree::Renderer;

impl Renderer {
    pub(super) fn switch_node(
        &mut self,
        node: &E2TriggerNode,
        depth: usize,
        out: &mut String,
    ) -> Result<(), E2ImportIssue> {
        let expression = exact_text(node, 1, "Switch expression")?;
        line(out, depth, &format!("switch({expression}) {{"));
        for case in &node.children {
            if case.name != "type-29" {
                return Err(malformed(
                    case,
                    format!("Switch {} expects only SwitchCase children", node.id),
                ));
            }
            self.node(case, true, depth + 1, out)?;
        }
        line(out, depth, "}");
        Ok(())
    }

    pub(super) fn switch_case(
        &mut self,
        node: &E2TriggerNode,
        depth: usize,
        out: &mut String,
    ) -> Result<(), E2ImportIssue> {
        let (value, needs_break) = case_values(node)?;
        let label = if value == "-1" {
            "default:".to_string()
        } else {
            format!("case {value}:")
        };
        line(out, depth, &label);
        self.children(&node.children, true, depth + 1, out)?;
        if needs_break {
            line(out, depth + 1, "break;");
        }
        Ok(())
    }
}

fn case_values(node: &E2TriggerNode) -> Result<(&str, bool), E2ImportIssue> {
    if node.values.len() != 2 {
        return Err(malformed(
            node,
            format!(
                "SwitchCase {} break expects 2 values, found {}",
                node.id,
                node.values.len()
            ),
        ));
    }
    let value = text(&node.values[0])
        .ok_or_else(|| malformed(node, format!("SwitchCase {} value is not text", node.id)))?;
    let needs_break = match text(&node.values[1]) {
        Some("True") => true,
        Some("False") => false,
        _ => {
            return Err(malformed(
                node,
                format!("SwitchCase {} break must be True or False", node.id),
            ))
        }
    };
    Ok((value, needs_break))
}

fn exact_text<'a>(
    node: &'a E2TriggerNode,
    count: usize,
    label: &str,
) -> Result<&'a str, E2ImportIssue> {
    if node.values.len() != count {
        return Err(malformed(
            node,
            format!("{label} expects {count} value, found {}", node.values.len()),
        ));
    }
    text(&node.values[0]).ok_or_else(|| malformed(node, format!("{label} is not text")))
}

fn text(value: &E2RawValue) -> Option<&str> {
    match value {
        E2RawValue::String(value) => Some(value),
        _ => None,
    }
}

fn malformed(node: &E2TriggerNode, message: String) -> E2ImportIssue {
    E2ImportIssue {
        code: E2IssueCode::MalformedInput,
        severity: E2IssueSeverity::Fatal,
        message: format!("Switch {}: {message}", node.id),
        path: None,
        feature: Some(E2Feature::StructGui),
        need_id: None,
    }
}
