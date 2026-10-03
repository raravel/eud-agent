mod branch;
mod eud;
mod eud_memory;
mod for_loop;
mod function_inventory;
mod metadata;
mod raw_trigger;
mod render;
mod semantics;
mod state_chain;
mod switch;
mod tree;
mod values;
mod wait;

#[cfg(test)]
mod function_prototype_tests;
#[cfg(test)]
mod operand_tests;
#[cfg(test)]
mod raw_trigger_wire_tests;
#[cfg(all(test, windows))]
mod raw_wait_build_scenario;
#[cfg(test)]
mod raw_wait_wire_tests;
#[cfg(test)]
mod structural_edge_tests;
#[cfg(test)]
mod switch_wire_tests;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod wait_inert_code_tests;
#[cfg(test)]
mod wait_wire_tests;
#[cfg(test)]
mod wrapper_activation_tests;

use super::{E2CompiledSources, E2ImportIr, E2ImportIssue};

#[derive(Default)]
pub(crate) struct CompileContext {
    pub(crate) player_scope: Option<[Option<u8>; 8]>,
}

pub fn compile_trigger_tree(document: &E2ImportIr) -> Result<E2CompiledSources, E2ImportIssue> {
    compile_trigger_tree_with_context(document, &CompileContext::default())
}

pub(crate) fn compile_trigger_tree_with_context(
    document: &E2ImportIr,
    context: &CompileContext,
) -> Result<E2CompiledSources, E2ImportIssue> {
    render::compile(document, context)
}

pub fn render_tct_helpers() -> String {
    render::helpers()
}
