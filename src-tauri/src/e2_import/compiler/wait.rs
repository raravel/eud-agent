use super::super::{E2ImportIssue, E2RawValue, E2TriggerNode};
use super::function_inventory::{definition, FunctionInventory, FunctionSignature};
use super::render::line;
use super::state_chain::{malformed, plan, StateTerminal};
use super::tree::Renderer;
use super::values::value_expression;

const PLAYERS: &str = "[0, 0, 0, 0, 0, 0, 0, 0]";

#[derive(Default)]
pub(super) struct WaitLowering {
    inventory: FunctionInventory,
    module: String,
}

impl WaitLowering {
    pub(super) fn module(&self) -> String {
        let mut output = String::new();
        for function in self.inventory.ordered() {
            if function.wait_enabled {
                for argument in &function.arguments {
                    line(
                        &mut output,
                        0,
                        &format!("const {}{argument} = {PLAYERS};", function.name),
                    );
                }
                line(
                    &mut output,
                    0,
                    &format!("const {}Timer = {PLAYERS};", function.name),
                );
            }
            line(
                &mut output,
                0,
                &format!("function {};", signature(function)),
            );
        }
        output.push_str(&self.module);
        if !self.has_enabled() {
            return output;
        }
        line(&mut output, 0, "function WaitableTriggerExec() {");
        for function in self.inventory.ordered() {
            if !function.wait_enabled {
                continue;
            }
            let arguments = function
                .arguments
                .iter()
                .map(|argument| format!("{}{argument}[getcurpl()]", function.name))
                .collect::<Vec<_>>()
                .join(", ");
            line(&mut output, 1, &format!("{}({arguments});", function.name));
        }
        line(&mut output, 0, "}");
        output
    }

    pub(super) fn has_enabled(&self) -> bool {
        self.inventory
            .ordered()
            .any(|function| function.wait_enabled)
    }

    pub(super) fn install(&mut self, inventory: FunctionInventory) {
        self.inventory = inventory;
    }

    fn function(&self, name: &str) -> Option<FunctionSignature> {
        self.inventory.get(name).cloned()
    }
}

impl Renderer {
    pub(super) fn wait_definition(&mut self, node: &E2TriggerNode) -> Result<(), E2ImportIssue> {
        let (parsed, code) = definition(node)?;
        let function = self
            .wait
            .function(&parsed.name)
            .ok_or_else(|| malformed(node, format!("undefined function {}", parsed.name)))?;
        self.record_descendants(&node.children[0], true);
        self.record(code, code.enabled);
        let mut rendered = String::new();
        if !code.enabled {
            for child in &code.children {
                self.record_descendants(child, false);
            }
            if function.wait_enabled {
                self.render_wait_enabled(&function, &[], &mut rendered)?;
            } else {
                line(
                    &mut rendered,
                    0,
                    &format!("function {} {{", signature(&function)),
                );
                line(&mut rendered, 0, "}");
            }
        } else if function.wait_enabled {
            self.render_wait_enabled(&function, &code.children, &mut rendered)?;
        } else {
            self.render_wait_disabled(&function, code, &mut rendered)?;
        }
        self.wait.module.push_str(&rendered);
        Ok(())
    }

    pub(super) fn wait_call(
        &mut self,
        node: &E2TriggerNode,
        depth: usize,
        out: &mut String,
    ) -> Result<(), E2ImportIssue> {
        let name = text_value(node, 0, "Function call name")?;
        let function = self
            .wait
            .function(name)
            .ok_or_else(|| malformed(node, format!("undefined function {name}")))?;
        let values = &node.values[1..];
        if values.len() != function.arguments.len() {
            return Err(malformed(
                node,
                format!(
                    "Function call {} expects {} arguments, found {}",
                    node.id,
                    function.arguments.len(),
                    values.len()
                ),
            ));
        }
        if !function.wait_enabled {
            let arguments = values
                .iter()
                .map(value_expression)
                .collect::<Vec<_>>()
                .join(", ");
            line(out, depth, &format!("{name}({arguments});"));
            return Ok(());
        }
        let timer = format!("{name}Timer[getcurpl()]");
        line(out, depth, &format!("if ({timer} == 0) {{"));
        for (argument, value) in function.arguments.iter().zip(values) {
            let value = value_expression(value);
            line(
                out,
                depth + 1,
                &format!("{name}{argument}[getcurpl()] = {value};"),
            );
        }
        line(out, depth + 1, &format!("{timer} = 1;"));
        let snapshots = function
            .arguments
            .iter()
            .map(|argument| format!("{name}{argument}[getcurpl()]"))
            .collect::<Vec<_>>()
            .join(", ");
        line(out, depth + 1, &format!("{name}({snapshots});"));
        line(out, depth, "}");
        Ok(())
    }

    fn render_wait_disabled(
        &mut self,
        function: &FunctionSignature,
        code: &E2TriggerNode,
        out: &mut String,
    ) -> Result<(), E2ImportIssue> {
        let signature = signature(function);
        line(out, 0, &format!("function {signature} {{"));
        for child in &code.children {
            if is_wait(child) {
                self.record_descendants(child, false);
            } else {
                self.node(child, true, 1, out)?;
            }
        }
        line(out, 0, "}");
        Ok(())
    }

    fn render_wait_enabled(
        &mut self,
        function: &FunctionSignature,
        children: &[E2TriggerNode],
        out: &mut String,
    ) -> Result<(), E2ImportIssue> {
        let signature = signature(function);
        let name = &function.name;
        let timer = format!("{name}Timer[getcurpl()]");
        line(out, 0, &format!("function {signature} {{"));
        line(out, 1, &format!("if ({timer} > 0) {{"));
        let chain = plan(children, StateTerminal::FunctionIdle)?;
        for &(marker, active) in &chain.markers {
            self.record(marker, active);
        }
        for (index, stage) in chain.stages.iter().enumerate() {
            let prefix = if index == 0 { "if" } else { "} else if" };
            line(out, 2, &format!("{prefix} ({timer} == {}) {{", stage.timer));
            for &child in &stage.children {
                self.node(child, true, 3, out)?;
            }
            if index + 1 == chain.stages.len() {
                let terminal = chain.terminal.expect("function terminal");
                line(out, 3, &format!("{timer} = {terminal};"));
            }
        }
        line(out, 2, "}");
        line(out, 2, &format!("{timer} += 1;"));
        line(out, 1, "}");
        line(out, 0, "}");
        Ok(())
    }
}

fn signature(function: &FunctionSignature) -> String {
    format!("{}({})", function.name, function.arguments.join(", "))
}

fn text_value<'a>(
    node: &'a E2TriggerNode,
    index: usize,
    label: &str,
) -> Result<&'a str, E2ImportIssue> {
    match node.values.get(index) {
        Some(E2RawValue::String(value)) if !value.is_empty() && value != "0" => Ok(value),
        _ => Err(malformed(node, format!("{label} {} is invalid", node.id))),
    }
}

fn is_wait(node: &E2TriggerNode) -> bool {
    node.name == "type-20"
}
