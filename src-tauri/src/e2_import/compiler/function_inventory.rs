use std::collections::BTreeMap;

use super::super::{E2ImportIssue, E2RawValue, E2TriggerNode, E2TriggerNodeKind};
use super::state_chain::malformed;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct FunctionSignature {
    pub(super) name: String,
    pub(super) arguments: Vec<String>,
    pub(super) wait_enabled: bool,
}

#[derive(Default)]
pub(super) struct FunctionInventory {
    order: Vec<String>,
    by_name: BTreeMap<String, FunctionSignature>,
}

impl FunctionInventory {
    pub(super) fn discover(nodes: &[E2TriggerNode]) -> Result<Self, E2ImportIssue> {
        let mut inventory = Self::default();
        inventory.visit(nodes, true)?;
        Ok(inventory)
    }

    pub(super) fn get(&self, name: &str) -> Option<&FunctionSignature> {
        self.by_name.get(name)
    }

    pub(super) fn ordered(&self) -> impl Iterator<Item = &FunctionSignature> {
        self.order.iter().map(|name| &self.by_name[name])
    }

    fn visit(&mut self, nodes: &[E2TriggerNode], parent_active: bool) -> Result<(), E2ImportIssue> {
        for node in nodes {
            let active = parent_active && node.enabled;
            if !active || whole_raw_inert(node) {
                continue;
            }
            if node.kind == E2TriggerNodeKind::Group && node.name == "type-16" {
                let (signature, _) = definition(node)?;
                if self.by_name.contains_key(&signature.name) {
                    return Err(malformed(
                        node,
                        format!("duplicate function {}", signature.name),
                    ));
                }
                self.order.push(signature.name.clone());
                self.by_name.insert(signature.name.clone(), signature);
            }
            self.visit(&node.children, active)?;
        }
        Ok(())
    }
}

pub(super) fn whole_raw_inert(node: &E2TriggerNode) -> bool {
    node.kind == E2TriggerNodeKind::Group
        && node.name == "type-24"
        && matches!(
            node.children.first(),
            Some(conditions) if conditions.name == "type-26" && !conditions.enabled
        )
}

pub(super) fn definition(
    node: &E2TriggerNode,
) -> Result<(FunctionSignature, &E2TriggerNode), E2ImportIssue> {
    if node.kind != E2TriggerNodeKind::Group || node.name != "type-16" || node.values.len() != 2 {
        return Err(malformed(node, "invalid FunctionDefinition shape"));
    }
    let name = text(node, 0, "FunctionDefinition name")?;
    let wait_enabled = match text(node, 1, "FunctionDefinition wait flag")? {
        "True" => true,
        "False" => false,
        _ => return Err(malformed(node, "invalid FunctionDefinition wait flag")),
    };
    let (arguments, code) = match node.children.as_slice() {
        [arguments, code]
            if arguments.kind == E2TriggerNodeKind::Group
                && arguments.name == "type-17"
                && code.kind == E2TriggerNodeKind::Group
                && code.name == "type-18" =>
        {
            (arguments, code)
        }
        _ => {
            return Err(malformed(
                node,
                "FunctionDefinition expects type-17 and type-18",
            ))
        }
    };
    let arguments = arguments
        .children
        .iter()
        .map(|argument| text(argument, 0, "Function argument").map(str::to_string))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((
        FunctionSignature {
            name: name.to_string(),
            arguments,
            wait_enabled,
        },
        code,
    ))
}

fn text<'a>(node: &'a E2TriggerNode, index: usize, label: &str) -> Result<&'a str, E2ImportIssue> {
    match node.values.get(index) {
        Some(E2RawValue::String(value)) if !value.is_empty() && value != "0" => Ok(value),
        _ => Err(malformed(node, format!("{label} {} is invalid", node.id))),
    }
}
