use super::super::definitions::E2StructuralNodeKind;
use super::super::{E2ImportIssue, E2TriggerNode};
use super::render::line;
use super::semantics::{condition_expression, unknown};
use super::tree::{is_predicate, structural_kind, Renderer};

pub(super) fn render(
    renderer: &mut Renderer,
    node: &E2TriggerNode,
    kind: E2StructuralNodeKind,
    depth: usize,
    out: &mut String,
) -> Result<(), E2ImportIssue> {
    let predicate = node
        .children
        .iter()
        .find(|child| is_predicate(child))
        .ok_or_else(|| unknown(node))?;
    renderer.record_descendants(predicate, true);
    let keyword = if kind == E2StructuralNodeKind::While {
        "while"
    } else {
        "if"
    };
    line(
        out,
        depth,
        &format!(
            "{keyword} ({}) {{",
            condition_expression(predicate, &renderer.metadata)?
        ),
    );
    let then_kind = if kind == E2StructuralNodeKind::While {
        E2StructuralNodeKind::WhileBody
    } else {
        E2StructuralNodeKind::Then
    };
    if let Some(body) = node
        .children
        .iter()
        .find(|child| structural_kind(child) == Some(then_kind))
    {
        renderer.node(body, true, depth + 1, out)?;
    } else {
        for child in node.children.iter().filter(|child| !is_predicate(child)) {
            renderer.node(child, true, depth + 1, out)?;
        }
    }
    line(out, depth, "}");
    if kind == E2StructuralNodeKind::IfElse {
        if let Some(otherwise) = node
            .children
            .iter()
            .find(|child| structural_kind(child) == Some(E2StructuralNodeKind::Else))
        {
            line(out, depth, "else {");
            renderer.node(otherwise, true, depth + 1, out)?;
            line(out, depth, "}");
        }
    }
    Ok(())
}
