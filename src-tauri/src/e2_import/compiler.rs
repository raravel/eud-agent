mod branch;
mod eud;
mod eud_memory;
mod for_loop;
mod metadata;
mod render;
mod semantics;
mod tree;
mod values;

#[cfg(test)]
mod operand_tests;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;

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
