use super::super::{
    E2Feature, E2ImportIssue, E2IssueCode, E2IssueSeverity, E2RawValue, E2TriggerNode,
    E2TriggerNodeKind,
};

#[derive(Clone, Copy)]
pub(super) enum StateTerminal {
    FunctionIdle,
    Raw { preserve: bool },
}

#[derive(Clone, Copy)]
pub(super) enum StateItem<'a> {
    Node(&'a E2TriggerNode),
    FolderPrefix(&'a E2TriggerNode),
    FolderSuffix(&'a E2TriggerNode),
}

pub(super) struct StateStage<'a> {
    pub(super) timer: i32,
    pub(super) items: Vec<StateItem<'a>>,
}

pub(super) struct StateChain<'a> {
    pub(super) stages: Vec<StateStage<'a>>,
    pub(super) terminal: Option<i32>,
    pub(super) markers: Vec<(&'a E2TriggerNode, bool)>,
}

pub(super) fn plan(
    children: &[E2TriggerNode],
    terminal: StateTerminal,
) -> Result<StateChain<'_>, E2ImportIssue> {
    let mut stages = Vec::new();
    let mut timer = 1_i32;
    let mut current = Vec::new();
    let mut markers = Vec::new();
    walk(
        children,
        true,
        &mut timer,
        &mut current,
        &mut stages,
        &mut markers,
    )?;
    stages.push(StateStage {
        timer,
        items: current,
    });
    let terminal = match terminal {
        StateTerminal::FunctionIdle => Some(-1),
        StateTerminal::Raw { preserve: true } => Some(0),
        StateTerminal::Raw { preserve: false } => None,
    };
    Ok(StateChain {
        stages,
        terminal,
        markers,
    })
}

fn walk<'a>(
    nodes: &'a [E2TriggerNode],
    parent_active: bool,
    timer: &mut i32,
    current: &mut Vec<StateItem<'a>>,
    stages: &mut Vec<StateStage<'a>>,
    markers: &mut Vec<(&'a E2TriggerNode, bool)>,
) -> Result<(), E2ImportIssue> {
    for node in nodes {
        let active = parent_active && node.enabled;
        if !active {
            mark_tree(node, false, markers);
        } else if is_group(node, "type-20") {
            markers.push((node, true));
            stages.push(StateStage {
                timer: *timer,
                items: std::mem::take(current),
            });
            *timer = timer
                .checked_add(duration(node)?)
                .ok_or_else(|| malformed(node, format!("Wait {} duration overflow", node.id)))?;
            for child in &node.children {
                mark_tree(child, false, markers);
            }
        } else if is_group(node, "type-21") {
            markers.push((node, true));
            current.push(StateItem::FolderPrefix(node));
            walk(&node.children, true, timer, current, stages, markers)?;
            current.push(StateItem::FolderSuffix(node));
        } else if is_group(node, "type-22") {
            markers.push((node, true));
            walk(&node.children, true, timer, current, stages, markers)?;
        } else {
            current.push(StateItem::Node(node));
        }
    }
    Ok(())
}

fn mark_tree<'a>(
    node: &'a E2TriggerNode,
    active: bool,
    markers: &mut Vec<(&'a E2TriggerNode, bool)>,
) {
    markers.push((node, active));
    for child in &node.children {
        mark_tree(child, false, markers);
    }
}

fn is_group(node: &E2TriggerNode, name: &str) -> bool {
    node.kind == E2TriggerNodeKind::Group && node.name == name
}

fn duration(node: &E2TriggerNode) -> Result<i32, E2ImportIssue> {
    if node.values.len() != 1 {
        return Err(malformed(
            node,
            format!(
                "Wait {} expects 1 duration, found {}",
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
                format!("Wait {} duration is not text", node.id),
            ))
        }
    };
    let value = source
        .parse::<u32>()
        .ok()
        .and_then(|value| i32::try_from(value).ok())
        .ok_or_else(|| malformed(node, format!("Wait {} duration is invalid", node.id)))?;
    Ok(value)
}

pub(super) fn malformed(node: &E2TriggerNode, message: impl Into<String>) -> E2ImportIssue {
    E2ImportIssue {
        code: E2IssueCode::MalformedInput,
        severity: E2IssueSeverity::Fatal,
        message: format!("StructGui {}: {}", node.id, message.into()),
        path: None,
        feature: Some(E2Feature::StructGui),
        need_id: None,
    }
}
