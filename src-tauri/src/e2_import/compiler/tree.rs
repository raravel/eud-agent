use super::super::definitions::{
    action_definition, condition_definition, structural_node_kind, E2SemanticTemplate,
    E2StructuralNodeKind,
};
use super::super::{E2ImportIr, E2ImportIssue, E2TriggerNode, E2TriggerNodeKind};
use super::metadata::CompileMetadata;
use super::render::line;
use super::semantics::{
    action, condition, condition_expression, integer, malformed, raw, unknown, value_expression,
};

pub(super) struct Renderer {
    pub(super) hooks: [String; 3],
    pub(super) provenance: String,
    pub(super) needs_bgm: bool,
    metadata: CompileMetadata,
}

impl Renderer {
    pub(super) fn new(document: &E2ImportIr) -> Result<Self, E2ImportIssue> {
        Ok(Self {
            hooks: std::array::from_fn(|_| String::new()),
            provenance: String::new(),
            needs_bgm: false,
            metadata: CompileMetadata::from_document(document).map_err(malformed)?,
        })
    }

    pub(super) fn top(&mut self, node: &E2TriggerNode) -> Result<(), E2ImportIssue> {
        if node.kind != E2TriggerNodeKind::Lifecycle {
            let mut body = String::new();
            self.node(node, true, 1, &mut body)?;
            self.hooks[1].push_str(&body);
            return Ok(());
        }
        let target = match node.name.as_str() {
            "onPluginStart" | "start" => 0,
            "beforeTriggerExec" | "before" => 1,
            "afterTriggerExec" | "after" => 2,
            _ => return Err(unknown(node)),
        };
        let mut body = String::new();
        self.children(&node.children, node.enabled, 1, &mut body)?;
        self.hooks[target].push_str(&body);
        self.record(node, node.enabled);
        Ok(())
    }

    fn children(
        &mut self,
        nodes: &[E2TriggerNode],
        active: bool,
        depth: usize,
        out: &mut String,
    ) -> Result<(), E2ImportIssue> {
        nodes
            .iter()
            .try_for_each(|node| self.node(node, active, depth, out))
    }

    fn node(
        &mut self,
        node: &E2TriggerNode,
        parent_active: bool,
        depth: usize,
        out: &mut String,
    ) -> Result<(), E2ImportIssue> {
        let active = parent_active && node.enabled;
        self.record(node, active);
        if !active {
            return self.children(&node.children, false, depth, out);
        }
        match node.kind {
            E2TriggerNodeKind::Action => {
                let definition = action_definition(&node.name).ok_or_else(|| unknown(node))?;
                self.needs_bgm |= definition.template == E2SemanticTemplate::Bgm;
                line(out, depth, &format!("// E2 id={}", node.id));
                if let Some(statement) = action(node, definition, &self.metadata)? {
                    line(out, depth, &statement);
                }
                self.children(&node.children, true, depth, out)
            }
            E2TriggerNodeKind::Condition => {
                let definition = condition_definition(&node.name).ok_or_else(|| unknown(node))?;
                self.needs_bgm |= definition.template == E2SemanticTemplate::Bgm;
                line(out, depth, &format!("// E2 id={}", node.id));
                line(
                    out,
                    depth,
                    &format!("if ({}) {{}}", condition(node, definition, &self.metadata)?),
                );
                self.children(&node.children, true, depth, out)
            }
            E2TriggerNodeKind::Group => self.group(node, depth, out),
            E2TriggerNodeKind::Lifecycle => Err(unknown(node)),
        }
    }

    fn group(
        &mut self,
        node: &E2TriggerNode,
        depth: usize,
        out: &mut String,
    ) -> Result<(), E2ImportIssue> {
        if node.name.starts_with("section:") {
            return self.children(&node.children, true, depth, out);
        }
        let kind = structural_kind(node);
        match kind {
            Some(
                E2StructuralNodeKind::If
                | E2StructuralNodeKind::IfElse
                | E2StructuralNodeKind::While,
            ) => self.branch(node, kind.expect("matched structural kind"), depth, out),
            Some(E2StructuralNodeKind::For) => {
                let mode = integer(node.values.first()).unwrap_or(0);
                let iterator = match mode {
                    0 => "EUDLoopUnit()".to_string(),
                    13 => "EUDLoopUnit2()".to_string(),
                    player => format!("EUDLoopPlayerUnit({})", player.saturating_sub(1)),
                };
                line(out, depth, &format!("foreach(ptr, epd : {iterator}) {{"));
                self.children(&node.children, true, depth + 1, out)?;
                line(out, depth, "}");
                Ok(())
            }
            Some(E2StructuralNodeKind::Function) => {
                let name = raw(node, 0);
                if name == "0" {
                    return Err(unknown(node));
                }
                let args = node
                    .values
                    .iter()
                    .skip(1)
                    .map(value_expression)
                    .collect::<Vec<_>>()
                    .join(", ");
                line(out, depth, &format!("{name}({args});"));
                Ok(())
            }
            Some(E2StructuralNodeKind::FunctionDefinition) => {
                let name = raw(node, 0);
                if name == "0" {
                    return Err(unknown(node));
                }
                let arguments = node
                    .children
                    .iter()
                    .find(|child| structural_kind(child) == Some(E2StructuralNodeKind::Argument));
                let names = arguments
                    .into_iter()
                    .flat_map(|container| &container.children)
                    .map(|argument| raw(argument, 0))
                    .collect::<Vec<_>>()
                    .join(", ");
                line(out, depth, &format!("function {name}({names}) {{"));
                for child in &node.children {
                    if structural_kind(child) != Some(E2StructuralNodeKind::Argument) {
                        self.node(child, true, depth + 1, out)?;
                    }
                }
                line(out, depth, "}");
                Ok(())
            }
            Some(E2StructuralNodeKind::RawString) => {
                line(out, depth, raw(node, 0));
                Ok(())
            }
            Some(_) => self.children(&node.children, true, depth, out),
            None => Err(unknown(node)),
        }
    }

    fn branch(
        &mut self,
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
        self.record_descendants(predicate, true);
        let keyword = match kind {
            E2StructuralNodeKind::While => "while",
            _ => "if",
        };
        line(
            out,
            depth,
            &format!(
                "{keyword} ({}) {{",
                condition_expression(predicate, &self.metadata)?
            ),
        );
        let then_kind = match kind {
            E2StructuralNodeKind::While => E2StructuralNodeKind::WhileBody,
            _ => E2StructuralNodeKind::Then,
        };
        if let Some(body) = node
            .children
            .iter()
            .find(|child| structural_kind(child) == Some(then_kind))
        {
            self.node(body, true, depth + 1, out)?;
        } else {
            for child in node.children.iter().filter(|child| !is_predicate(child)) {
                self.node(child, true, depth + 1, out)?;
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
                self.node(otherwise, true, depth + 1, out)?;
                line(out, depth, "}");
            }
        }
        Ok(())
    }

    fn record_descendants(&mut self, node: &E2TriggerNode, parent_active: bool) {
        let active = parent_active && node.enabled;
        self.record(node, active);
        for child in &node.children {
            self.record_descendants(child, active);
        }
    }

    fn record(&mut self, node: &E2TriggerNode, active: bool) {
        let values = serde_json::to_string(&node.values).unwrap_or_else(|_| "[]".to_string());
        self.provenance.push_str(&format!(
            "// E2-PROVENANCE id={} name={} kind={:?} enabled={} executable={} folded={} notCondition={} values={}\n",
            node.id, node.name, node.kind, node.enabled, active, node.folded, node.not_condition, values
        ));
    }
}

fn structural_kind(node: &E2TriggerNode) -> Option<E2StructuralNodeKind> {
    structural_node_kind(node.name.strip_prefix("type-")?.parse().ok()?)
}

fn is_predicate(node: &E2TriggerNode) -> bool {
    node.kind == E2TriggerNodeKind::Condition
        || matches!(
            structural_kind(node),
            Some(
                E2StructuralNodeKind::ConditionClause
                    | E2StructuralNodeKind::WhileCondition
                    | E2StructuralNodeKind::TriggerCondition
            )
        )
}
