use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use crate::config::DataDirs;
use crate::e2_import::resources::E2PlannedCopy;
use crate::e2_import::{
    E2Feature, E2ImportIr, E2ImportIssue, E2IssueCode, E2IssueSeverity, E2RawValue, E2TriggerNode,
    E2TriggerNodeKind, ProjectResourceKind,
};

pub(super) type PlayerScope = [Option<u8>; 8];

pub(super) fn force_selected_node(document: &E2ImportIr) -> Option<&E2TriggerNode> {
    document
        .triggers
        .iter()
        .find_map(|node| force_selected_node_in(node, true))
}

pub(super) fn resolve(
    copies: &[E2PlannedCopy],
    dirs: &DataDirs,
    node: &E2TriggerNode,
) -> Result<PlayerScope, E2ImportIssue> {
    let map = copies
        .iter()
        .find(|copy| copy.resource.kind == ProjectResourceKind::Map)
        .ok_or_else(|| malformed(node, "force selection has no owned input map"))?;
    let temp = OwnedMap::write(dirs, &map.bytes).map_err(|error| malformed(node, error))?;
    let chk = isom::chk_extract(&temp.0).map_err(|error| malformed(node, error))?;
    let digest = crate::chk::digest_chk(&chk);
    let mut scope = [None; 8];
    for (slot, player) in digest.players.iter().take(8).enumerate() {
        if matches!(player.controller_id, 3 | 5 | 6 | 7) {
            scope[slot] = match player.force {
                None => None,
                Some(1..=4) => player.force.map(|force| force - 1),
                Some(_) => return Err(malformed(node, "active player has invalid force")),
            };
        }
    }
    Ok(scope)
}

fn force_selected_node_in<'a>(
    node: &'a E2TriggerNode,
    parent_active: bool,
) -> Option<&'a E2TriggerNode> {
    let active = parent_active && node.enabled;
    if !active {
        return None;
    }
    if node.kind == E2TriggerNodeKind::Group
        && node.name == "type-10"
        && matches!(node.values.first(), Some(E2RawValue::String(mode)) if mode == "PlayerLoop")
        && matches!(node.values.get(1), Some(E2RawValue::String(flags)) if flags.split(',').skip(8).any(|flag| flag == "True"))
    {
        return Some(node);
    }
    node.children
        .iter()
        .find_map(|child| force_selected_node_in(child, active))
}

struct OwnedMap(PathBuf);

impl OwnedMap {
    fn write(dirs: &DataDirs, bytes: &[u8]) -> Result<Self, String> {
        let root = dirs.audio_temp_dir();
        fs::create_dir_all(&root).map_err(|error| format!("create scope temp: {error}"))?;
        let path = root.join(format!("e2-player-scope-{}.scx", uuid::Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| format!("create owned map: {error}"))?;
        let owned = Self(path);
        file.write_all(bytes)
            .map_err(|error| format!("write owned map: {error}"))?;
        Ok(owned)
    }
}

impl Drop for OwnedMap {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn malformed(node: &E2TriggerNode, message: impl std::fmt::Display) -> E2ImportIssue {
    E2ImportIssue {
        code: E2IssueCode::MalformedInput,
        severity: E2IssueSeverity::Fatal,
        message: format!("PlayerLoop {}: {message}", node.id),
        path: None,
        feature: Some(E2Feature::StructGui),
        need_id: None,
    }
}
