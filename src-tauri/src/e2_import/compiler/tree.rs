use super::super::definitions::{
    action_definition, condition_definition, structural_node_kind, E2SemanticTemplate,
    E2StructuralNodeKind,
};
use super::super::{E2ImportIr, E2ImportIssue, E2RawValue, E2TriggerNode, E2TriggerNodeKind};
use super::function_inventory::{whole_raw_inert, FunctionInventory};
use super::metadata::CompileMetadata;
use super::raw_trigger::RawLowering;
use super::render::line;
use super::semantics::{action, condition, malformed, raw, unknown};
use super::state_chain::{malformed as state_malformed, StateItem};
use super::wait::WaitLowering;
use super::CompileContext;

pub(super) struct Renderer {
    pub(super) hooks: [String; 3],
    pub(super) provenance: String,
    pub(super) needs_bgm: bool,
    pub(super) metadata: CompileMetadata,
    pub(super) player_scope: Option<[Option<u8>; 8]>,
    pub(super) raw: RawLowering,
    pub(super) wait: WaitLowering,
}

impl Renderer {
    pub(super) fn new(
        document: &E2ImportIr,
        context: &CompileContext,
    ) -> Result<Self, E2ImportIssue> {
        let mut wait = WaitLowering::default();
        wait.install(FunctionInventory::discover(&document.triggers)?);
        Ok(Self {
            hooks: std::array::from_fn(|_| String::new()),
            provenance: String::new(),
            needs_bgm: false,
            metadata: CompileMetadata::from_document(document).map_err(malformed)?,
            player_scope: context.player_scope,
            raw: RawLowering::default(),
            wait,
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

    pub(super) fn children(
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

    pub(super) fn state_item(
        &mut self,
        item: StateItem<'_>,
        depth: usize,
        out: &mut String,
    ) -> Result<(), E2ImportIssue> {
        let (node, index, boundary) = match item {
            StateItem::Node(node) => return self.node(node, true, depth, out),
            StateItem::FolderPrefix(node) => (node, 0, "prefix"),
            StateItem::FolderSuffix(node) => (node, 1, "suffix"),
        };
        let Some(E2RawValue::String(text)) = node.values.get(index) else {
            return Err(state_malformed(
                node,
                format!("Folder {} {boundary} is not text", node.id),
            ));
        };
        line(out, depth, text);
        Ok(())
    }

    pub(super) fn node(
        &mut self,
        node: &E2TriggerNode,
        parent_active: bool,
        depth: usize,
        out: &mut String,
    ) -> Result<(), E2ImportIssue> {
        let active = parent_active && node.enabled;
        if active && whole_raw_inert(node) {
            self.record_descendants(node, false);
            return Ok(());
        }
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
            ) => super::branch::render(
                self,
                node,
                kind.expect("matched structural kind"),
                depth,
                out,
            ),
            Some(E2StructuralNodeKind::For) => {
                match super::for_loop::lower(node, self.player_scope.as_ref())? {
                    super::for_loop::ForLoop::Standard(open) => {
                        line(out, depth, &open);
                        self.children(&node.children, true, depth + 1, out)?;
                        line(out, depth, "}");
                    }
                    super::for_loop::ForLoop::PlayerLoop(players) => {
                        line(out, depth, "EUDPlayerLoop()();");
                        let predicate = players
                            .iter()
                            .map(|player| format!("getcurpl() == {player}"))
                            .collect::<Vec<_>>()
                            .join(" || ");
                        line(out, depth, &format!("if ({predicate}) {{"));
                        self.children(&node.children, true, depth + 1, out)?;
                        line(out, depth, "}");
                        line(out, depth, "EUDEndPlayerLoop();");
                    }
                }
                Ok(())
            }
            Some(E2StructuralNodeKind::Function) => self.wait_call(node, depth, out),
            Some(E2StructuralNodeKind::FunctionDefinition) => self.wait_definition(node),
            Some(E2StructuralNodeKind::Folder) => {
                self.state_item(StateItem::FolderPrefix(node), depth, out)?;
                self.children(&node.children, true, depth, out)?;
                self.state_item(StateItem::FolderSuffix(node), depth, out)
            }
            Some(E2StructuralNodeKind::FolderAction) => {
                self.children(&node.children, true, depth, out)
            }
            Some(E2StructuralNodeKind::RawString) => {
                line(out, depth, raw(node, 0));
                Ok(())
            }
            Some(E2StructuralNodeKind::RawTrigger) => self.raw_trigger(node),
            Some(E2StructuralNodeKind::Switch) => self.switch_node(node, depth, out),
            Some(E2StructuralNodeKind::SwitchCase) => self.switch_case(node, depth, out),
            Some(_) => self.children(&node.children, true, depth, out),
            None => Err(unknown(node)),
        }
    }

    pub(super) fn record_descendants(&mut self, node: &E2TriggerNode, parent_active: bool) {
        let active = parent_active && node.enabled;
        self.record(node, active);
        for child in &node.children {
            self.record_descendants(child, active);
        }
    }

    pub(super) fn record(&mut self, node: &E2TriggerNode, active: bool) {
        let values = serde_json::to_string(&node.values).unwrap_or_else(|_| "[]".to_string());
        self.provenance.push_str(&format!(
            "// E2-PROVENANCE id={} name={} kind={:?} enabled={} executable={} folded={} notCondition={} values={}\n",
            node.id, node.name, node.kind, node.enabled, active, node.folded, node.not_condition, values
        ));
    }
}

pub(super) fn structural_kind(node: &E2TriggerNode) -> Option<E2StructuralNodeKind> {
    structural_node_kind(node.name.strip_prefix("type-")?.parse().ok()?)
}

pub(super) fn is_predicate(node: &E2TriggerNode) -> bool {
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
