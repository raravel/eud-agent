mod eud;
mod eud_memory;
mod metadata;
mod render;
mod semantics;
mod tree;
mod values;

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;

use super::{E2CompiledSources, E2ImportIr, E2ImportIssue};

pub fn compile_trigger_tree(document: &E2ImportIr) -> Result<E2CompiledSources, E2ImportIssue> {
    render::compile(document)
}

pub fn render_tct_helpers() -> String {
    render::helpers()
}
