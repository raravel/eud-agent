use super::super::{E2ImportIssue, E2RawValue, E2TriggerNode, E2TriggerNodeKind};
use super::render::line;
use super::semantics::condition_expression;
use super::state_chain::{malformed, plan, StateTerminal};
use super::tree::Renderer;

const PLAYERS: &str = "[0, 0, 0, 0, 0, 0, 0, 0]";

struct RawPlan {
    ordinal: usize,
    players: Vec<u8>,
}

#[derive(Default)]
pub(super) struct RawLowering {
    plans: Vec<RawPlan>,
    module: String,
}

impl RawLowering {
    pub(super) fn module(&self) -> String {
        let mut output = self.module.clone();
        if self.plans.is_empty() {
            return output;
        }
        line(&mut output, 0, "function ClassicTriggerExec() {");
        for plan in &self.plans {
            line(
                &mut output,
                1,
                &format!("ClassicTriggerExec{}();", plan.ordinal),
            );
        }
        line(&mut output, 0, "}");
        line(&mut output, 0, "function ClassicTriggerStarter() {");
        for player in 0_u8..8 {
            line(&mut output, 1, &format!("if (playerexist({player})) {{"));
            line(&mut output, 2, &format!("setcurpl({player});"));
            for plan in self
                .plans
                .iter()
                .filter(|plan| plan.players.contains(&player))
            {
                line(
                    &mut output,
                    2,
                    &format!("ClassicTriggerStarter{}();", plan.ordinal),
                );
            }
            line(&mut output, 1, "}");
        }
        line(&mut output, 0, "}");
        output
    }

    pub(super) fn has_triggers(&self) -> bool {
        !self.plans.is_empty()
    }

    fn push(&mut self, players: Vec<u8>, rendered: String) {
        let ordinal = self.plans.len();
        self.module.push_str(&rendered);
        self.plans.push(RawPlan { ordinal, players });
    }
}

impl Renderer {
    pub(super) fn raw_trigger(&mut self, node: &E2TriggerNode) -> Result<(), E2ImportIssue> {
        let mask = mask(node)?;
        let players = selected_players(node, mask, self.player_scope.as_ref())?;
        let (conditions, actions) = containers(node)?;
        self.record_descendants(conditions, true);
        self.record(actions, actions.enabled);
        let condition = condition_expression(conditions, &self.metadata)?;
        let action_children = if actions.enabled {
            actions.children.as_slice()
        } else {
            for child in &actions.children {
                self.record_descendants(child, false);
            }
            &[]
        };
        let preserve = action_children.iter().any(|action| {
            action.enabled
                && action.kind == E2TriggerNodeKind::Action
                && action.name == "PreserveTrigger"
        });
        let chain = plan(action_children, StateTerminal::Raw { preserve })?;
        for &(marker, active) in &chain.markers {
            self.record(marker, active);
        }
        let ordinal = self.raw.plans.len();
        let timer = format!("ClassicTriggerExecTimer{ordinal}[getcurpl()]");
        let mut output = String::new();
        line(
            &mut output,
            0,
            &format!("const ClassicTriggerExecTimer{ordinal} = {PLAYERS};"),
        );
        line(
            &mut output,
            0,
            &format!("function ClassicTriggerExec{ordinal}() {{"),
        );
        for (index, stage) in chain.stages.iter().enumerate() {
            let prefix = if index == 0 { "if" } else { "} else if" };
            line(
                &mut output,
                1,
                &format!("{prefix} ({timer} == {}) {{", stage.timer),
            );
            for &item in &stage.items {
                self.state_item(item, 2, &mut output)?;
            }
            if index + 1 == chain.stages.len() {
                if let Some(terminal) = chain.terminal {
                    line(&mut output, 2, &format!("{timer} = {terminal};"));
                }
            }
        }
        line(&mut output, 1, "}");
        line(&mut output, 1, &format!("if ({timer} > 0) {{"));
        line(&mut output, 2, &format!("{timer} += 1;"));
        line(&mut output, 1, "}");
        line(&mut output, 0, "}");
        line(
            &mut output,
            0,
            &format!("function ClassicTriggerStarter{ordinal}() {{"),
        );
        line(
            &mut output,
            1,
            &format!("if (({condition}) && {timer} == 0) {{"),
        );
        line(&mut output, 2, &format!("{timer} = 1;"));
        line(&mut output, 2, &format!("ClassicTriggerExec{ordinal}();"));
        line(&mut output, 1, "}");
        line(&mut output, 0, "}");
        self.raw.push(players, output);
        Ok(())
    }
}

fn mask(node: &E2TriggerNode) -> Result<u16, E2ImportIssue> {
    if node.values.len() != 1 {
        return Err(malformed(
            node,
            format!(
                "RawTrigger {} expects 1 mask, found {}",
                node.id,
                node.values.len()
            ),
        ));
    }
    let source = match &node.values[0] {
        E2RawValue::String(source) => source,
        _ => {
            return Err(malformed(
                node,
                format!("RawTrigger {} mask is not text", node.id),
            ))
        }
    };
    let mask = source
        .parse::<u16>()
        .map_err(|_| malformed(node, format!("RawTrigger {} mask is not unsigned", node.id)))?;
    if mask & !0x1fff != 0 {
        return Err(malformed(
            node,
            format!("RawTrigger {} mask exceeds 13 bits", node.id),
        ));
    }
    Ok(mask)
}

fn containers(node: &E2TriggerNode) -> Result<(&E2TriggerNode, &E2TriggerNode), E2ImportIssue> {
    if node.children.len() != 2
        || node.children[0].name != "type-26"
        || node.children[1].name != "type-27"
    {
        return Err(malformed(
            node,
            format!(
                "RawTrigger {} expects direct type-26 and type-27 containers",
                node.id
            ),
        ));
    }
    Ok((&node.children[0], &node.children[1]))
}

fn selected_players(
    node: &E2TriggerNode,
    mask: u16,
    player_scope: Option<&[Option<u8>; 8]>,
) -> Result<Vec<u8>, E2ImportIssue> {
    let mut players = (0_u8..8)
        .filter(|player| mask & (1 << player) != 0)
        .collect::<Vec<_>>();
    if mask & (1 << 12) != 0 {
        players.extend(0_u8..8);
    }
    let force_mask = (mask >> 8) & 0x0f;
    if force_mask != 0 {
        let scope = player_scope.ok_or_else(|| {
            malformed(
                node,
                format!("RawTrigger {} force mask needs map scope", node.id),
            )
        })?;
        for (player, force) in scope.iter().enumerate() {
            if force.is_some_and(|force| force_mask & (1 << force) != 0) {
                players.push(player as u8);
            }
        }
    }
    players.sort_unstable();
    players.dedup();
    Ok(players)
}
