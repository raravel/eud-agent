use super::super::{
    E2Feature, E2ImportIssue, E2IssueCode, E2IssueSeverity, E2RawValue, E2TriggerNode,
};

pub(super) enum ForLoop {
    Standard(String),
    PlayerLoop(Vec<u8>),
}

pub(super) fn lower(
    node: &E2TriggerNode,
    player_scope: Option<&[Option<u8>; 8]>,
) -> Result<ForLoop, E2ImportIssue> {
    let mode = value(node, 0, "For")?;
    match mode {
        "Custom" => {
            let header = value(node, 1, mode)?;
            if header.is_empty() {
                return Err(malformed(node, mode, "header is empty"));
            }
            Ok(ForLoop::Standard(format!("for({header}) {{")))
        }
        "Counting" => {
            let variable = value(node, 1, mode)?;
            let limit = value(node, 2, mode)?;
            if variable.is_empty() || limit.is_empty() {
                return Err(malformed(node, mode, "requires variable and limit"));
            }
            Ok(ForLoop::Standard(format!(
                "for(var {variable} = 0; {variable} < {limit}; {variable}++) {{"
            )))
        }
        "AllUnit" => all_unit(node, mode),
        "PlayerLoop" => player_loop(node, mode, player_scope),
        _ => Err(malformed(node, mode, "mode is unsupported")),
    }
}

fn all_unit(node: &E2TriggerNode, mode: &str) -> Result<ForLoop, E2ImportIssue> {
    let selector = value(node, 1, mode)?
        .parse::<u8>()
        .map_err(|_| malformed(node, mode, "selector is not numeric"))?;
    let iterator = match selector {
        0 => "EUDLoopUnit()".to_string(),
        13 => "EUDLoopUnit2()".to_string(),
        1..=12 => format!("EUDLoopPlayerUnit({})", selector - 1),
        _ => return Err(malformed(node, mode, "selector is outside 0..=13")),
    };
    Ok(ForLoop::Standard(format!(
        "foreach(ptr, epd : {iterator}) {{"
    )))
}

fn player_loop(
    node: &E2TriggerNode,
    mode: &str,
    player_scope: Option<&[Option<u8>; 8]>,
) -> Result<ForLoop, E2ImportIssue> {
    let flags = value(node, 1, mode)?
        .split(',')
        .map(|flag| match flag {
            "True" => Ok(true),
            "False" => Ok(false),
            _ => Err(malformed(node, mode, "flag must be True or False")),
        })
        .collect::<Result<Vec<_>, _>>()?;
    if flags.len() != 12 {
        return Err(malformed(node, mode, "expects 12 selection flags"));
    }
    let mut players = Vec::new();
    for (slot, selected) in flags[..8].iter().enumerate() {
        if *selected {
            players.push(
                u8::try_from(slot).map_err(|_| malformed(node, mode, "player slot exceeds u8"))?,
            );
        }
    }
    if flags[8..].iter().any(|selected| *selected) {
        let scope =
            player_scope.ok_or_else(|| malformed(node, mode, "force selection needs map scope"))?;
        for (force, selected) in flags[8..].iter().enumerate() {
            if *selected {
                let force = u8::try_from(force)
                    .map_err(|_| malformed(node, mode, "force index exceeds u8"))?;
                for (slot, member) in scope.iter().enumerate() {
                    if *member == Some(force) {
                        players.push(
                            u8::try_from(slot)
                                .map_err(|_| malformed(node, mode, "player slot exceeds u8"))?,
                        );
                    }
                }
            }
        }
    }
    players.sort_unstable();
    players.dedup();
    if players.is_empty() {
        return Err(malformed(node, mode, "selected no active players"));
    }
    Ok(ForLoop::PlayerLoop(players))
}

fn value<'a>(node: &'a E2TriggerNode, index: usize, mode: &str) -> Result<&'a str, E2ImportIssue> {
    match node.values.get(index) {
        Some(E2RawValue::String(value)) => Ok(value),
        _ => Err(malformed(node, mode, format!("expects value {index}"))),
    }
}

fn malformed(node: &E2TriggerNode, mode: &str, message: impl AsRef<str>) -> E2ImportIssue {
    E2ImportIssue {
        code: E2IssueCode::MalformedInput,
        severity: E2IssueSeverity::Fatal,
        message: format!("For mode {mode} {}: {}", node.id, message.as_ref()),
        path: None,
        feature: Some(E2Feature::StructGui),
        need_id: None,
    }
}
