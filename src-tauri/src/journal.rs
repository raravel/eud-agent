use serde::{Deserialize, Serialize};
use similar::TextDiff;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use thiserror::Error;

pub trait JournalBridge {
    type Error;

    fn set_dat_value(
        &self,
        table: DatTable,
        obj_id: u32,
        property: &str,
        value: serde_json::Value,
    ) -> Result<(), Self::Error>;

    fn reset_dat_value(
        &self,
        table: DatTable,
        obj_id: u32,
        property: &str,
    ) -> Result<(), Self::Error>;

    fn write_file(&self, path: &str, content: &str) -> Result<(), Self::Error>;

    fn delete_file(&self, path: &str) -> Result<(), Self::Error>;

    fn create_file(
        &self,
        path: &str,
        content: &str,
        position: Option<usize>,
    ) -> Result<(), Self::Error>;

    fn rename_path(&self, from: &str, to: &str) -> Result<(), Self::Error>;

    fn set_main(&self, path: Option<&str>) -> Result<(), Self::Error>;

    fn set_setting(&self, key: &str, value: serde_json::Value) -> Result<(), Self::Error>;

    fn plugin_add(
        &self,
        plugin_id: &str,
        texts: Vec<String>,
        index: usize,
    ) -> Result<(), Self::Error>;

    fn plugin_edit(
        &self,
        plugin_id: &str,
        texts: Vec<String>,
        index: usize,
    ) -> Result<(), Self::Error>;

    fn plugin_remove(&self, plugin_id: &str) -> Result<(), Self::Error>;

    fn plugin_move(&self, plugin_id: &str, index: usize) -> Result<(), Self::Error>;

    fn restore_map_backup(&self, map_path: &str, backup_path: &str) -> Result<(), Self::Error>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DatTable {
    Dat,
    Xdat,
    Tbl,
    Req,
    Btn,
}

impl fmt::Display for DatTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Dat => f.write_str("Dat"),
            Self::Xdat => f.write_str("Xdat"),
            Self::Tbl => f.write_str("Tbl"),
            Self::Req => f.write_str("Req"),
            Self::Btn => f.write_str("Btn"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WriteTool {
    DatSet,
    XdatSet,
    TblSet,
    ReqSet,
    BtnSet,
    FileWrite,
    FileCreate,
    Mkdir,
    FileDelete,
    FileRename,
    FileMove,
    SetMain,
    SettingsSet,
    PluginAdd,
    PluginEdit,
    PluginRemove,
    PluginMove,
    LocationWrite,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum JournalTarget {
    Dat {
        table: DatTable,
        obj_id: u32,
        property: String,
    },
    Path {
        path: String,
    },
    Rename {
        from: String,
        to: String,
    },
    Setting {
        key: String,
    },
    Plugin {
        plugin_id: String,
    },
    Map {
        path: String,
        summary: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Snapshot {
    DatValue {
        value: serde_json::Value,
        was_default: bool,
    },
    FileContent {
        content: String,
    },
    Created,
    DeletedFile {
        content: String,
        position: Option<usize>,
    },
    Deleted,
    Path {
        path: String,
    },
    MainPath {
        path: Option<String>,
    },
    SettingValue {
        value: serde_json::Value,
    },
    PluginTexts {
        texts: Vec<String>,
        index: usize,
    },
    PluginAbsent,
    MapBackup {
        map_path: String,
        backup_path: String,
    },
    MapEdit {
        action: String,
        location_id: Option<i64>,
        name: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEntry {
    pub id: String,
    pub seq: u64,
    pub tool: WriteTool,
    pub target: JournalTarget,
    pub before: Snapshot,
    pub after: Snapshot,
    pub ts: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Journal {
    pub request_id: String,
    pub entries: Vec<JournalEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecisionIds {
    All,
    Items(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangesetDecision {
    Accept,
    Reject(DecisionIds),
}

impl ChangesetDecision {
    pub fn accept() -> Self {
        Self::Accept
    }

    pub fn reject(ids: DecisionIds) -> Self {
        Self::Reject(ids)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Changeset {
    pub request_id: String,
    pub items: Vec<ChangesetItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangesetItem {
    pub id: String,
    pub kind: ChangesetItemKind,
    pub properties: Vec<PropertyChange>,
    pub diff: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangesetItemKind {
    Dat,
    Created,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PropertyChange {
    pub property: String,
    pub old: serde_json::Value,
    pub new: serde_json::Value,
}

#[derive(Debug, Error)]
pub enum JournalError {
    #[error("journal for request {request_id} was not found")]
    MissingJournal { request_id: String },
    #[error("decision already in progress for request {request_id}")]
    DecisionInProgress { request_id: String },
    #[error("non-tail rejection for request {request_id} would clobber later accepted edits on {target}")]
    NonTailReject { request_id: String, target: String },
    #[error("invalid journal entry {entry_id}: {message}")]
    InvalidEntry { entry_id: String, message: String },
    #[error("bridge operation failed: {0}")]
    Bridge(String),
    #[error("journal lock poisoned")]
    LockPoisoned,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone)]
pub struct JournalStore {
    data_dir: PathBuf,
    journals: Arc<Mutex<HashMap<String, Journal>>>,
    decisions: Arc<Mutex<HashSet<String>>>,
}

impl JournalStore {
    pub fn new(data_dir: impl AsRef<Path>) -> Self {
        Self {
            data_dir: data_dir.as_ref().to_path_buf(),
            journals: Arc::new(Mutex::new(HashMap::new())),
            decisions: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub fn load(data_dir: impl AsRef<Path>, request_id: &str) -> Result<Journal, JournalError> {
        let path = journal_path(data_dir.as_ref(), request_id);
        let bytes = fs::read(path)?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub fn record(&self, request_id: &str, entry: JournalEntry) -> Result<(), JournalError> {
        let mut journals = lock(&self.journals)?;
        let journal = journals
            .entry(request_id.to_owned())
            .or_insert_with(|| Journal {
                request_id: request_id.to_owned(),
                entries: Vec::new(),
            });
        journal.entries.push(entry);
        journal.entries.sort_by_key(|entry| entry.seq);
        Ok(())
    }

    pub fn persist(&self, request_id: &str) -> Result<(), JournalError> {
        let journal = self.journal(request_id)?;
        write_journal(&self.data_dir, &journal)
    }

    pub fn archive(&self, request_id: &str) -> Result<(), JournalError> {
        let src = journal_path(&self.data_dir, request_id);
        let dst_dir = self.data_dir.join("journal").join("accepted");
        let dst = dst_dir.join(format!("{request_id}.json"));

        fs::create_dir_all(&dst_dir)?;
        if dst.exists() {
            fs::remove_file(&dst)?;
        }

        if src.exists() {
            fs::rename(&src, &dst)?;
        } else {
            let journal = self.journal(request_id)?;
            let bytes = serde_json::to_vec_pretty(&journal)?;
            fs::write(&dst, bytes)?;
        }

        lock(&self.journals)?.remove(request_id);
        Ok(())
    }

    pub fn changeset(&self, request_id: &str) -> Result<Changeset, JournalError> {
        let journal = self.journal(request_id)?;
        changeset_from_journal(&journal)
    }

    pub fn decide<B>(
        &self,
        request_id: &str,
        decision: ChangesetDecision,
        bridge: &B,
    ) -> Result<(), JournalError>
    where
        B: JournalBridge,
        B::Error: fmt::Display,
    {
        let _guard = self.begin_decision(request_id)?;
        match decision {
            ChangesetDecision::Accept => self.archive(request_id),
            ChangesetDecision::Reject(ids) => {
                let journal = self.journal(request_id)?;
                let rejected = rejected_entries(&journal, &ids);
                validate_tail_reject(request_id, &journal, &rejected)?;
                for entry in rejected.iter().rev() {
                    apply_inverse(entry, bridge)?;
                }

                if matches!(ids, DecisionIds::All) {
                    self.archive(request_id)?;
                }
                Ok(())
            }
        }
    }

    pub fn finalize_undecided_as_accepted(&self, request_id: &str) -> Result<(), JournalError> {
        self.archive(request_id)
    }

    pub fn begin_decision(&self, request_id: &str) -> Result<DecisionGuard, JournalError> {
        let mut decisions = lock(&self.decisions)?;
        if !decisions.insert(request_id.to_owned()) {
            return Err(JournalError::DecisionInProgress {
                request_id: request_id.to_owned(),
            });
        }

        Ok(DecisionGuard {
            request_id: request_id.to_owned(),
            decisions: Arc::clone(&self.decisions),
        })
    }

    fn journal(&self, request_id: &str) -> Result<Journal, JournalError> {
        if let Some(journal) = lock(&self.journals)?.get(request_id).cloned() {
            return Ok(journal);
        }

        Self::load(&self.data_dir, request_id).map_err(|error| match error {
            JournalError::Io(io_error) if io_error.kind() == std::io::ErrorKind::NotFound => {
                JournalError::MissingJournal {
                    request_id: request_id.to_owned(),
                }
            }
            other => other,
        })
    }
}

#[derive(Debug)]
pub struct DecisionGuard {
    request_id: String,
    decisions: Arc<Mutex<HashSet<String>>>,
}

impl Drop for DecisionGuard {
    fn drop(&mut self) {
        if let Ok(mut decisions) = self.decisions.lock() {
            decisions.remove(&self.request_id);
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, JournalError> {
    mutex.lock().map_err(|_| JournalError::LockPoisoned)
}

fn journal_path(data_dir: &Path, request_id: &str) -> PathBuf {
    data_dir.join("journal").join(format!("{request_id}.json"))
}

fn write_journal(data_dir: &Path, journal: &Journal) -> Result<(), JournalError> {
    let path = journal_path(data_dir, &journal.request_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(journal)?;
    fs::write(path, bytes)?;
    Ok(())
}

fn changeset_from_journal(journal: &Journal) -> Result<Changeset, JournalError> {
    let mut items = Vec::new();
    let mut dat_items: HashMap<(DatTable, u32), usize> = HashMap::new();

    for entry in &journal.entries {
        match &entry.target {
            JournalTarget::Dat {
                table,
                obj_id,
                property,
            } => {
                let (old, new) = dat_values(entry)?;
                let key = (*table, *obj_id);
                let item_index = match dat_items.get(&key) {
                    Some(index) => *index,
                    None => {
                        let index = items.len();
                        dat_items.insert(key, index);
                        items.push(ChangesetItem {
                            id: changeset_item_id(entry),
                            kind: ChangesetItemKind::Dat,
                            properties: Vec::new(),
                            diff: None,
                        });
                        index
                    }
                };
                items[item_index].properties.push(PropertyChange {
                    property: property.clone(),
                    old,
                    new,
                });
            }
            _ => {
                if let Some(item) = file_changeset_item(entry)? {
                    items.push(item);
                }
            }
        }
    }

    Ok(Changeset {
        request_id: journal.request_id.clone(),
        items,
    })
}

fn dat_values(
    entry: &JournalEntry,
) -> Result<(serde_json::Value, serde_json::Value), JournalError> {
    match (&entry.before, &entry.after) {
        (Snapshot::DatValue { value: old, .. }, Snapshot::DatValue { value: new, .. }) => {
            Ok((old.clone(), new.clone()))
        }
        _ => Err(invalid_entry(entry, "expected dat before/after snapshots")),
    }
}

fn file_changeset_item(entry: &JournalEntry) -> Result<Option<ChangesetItem>, JournalError> {
    let item = match entry.tool {
        WriteTool::FileCreate | WriteTool::Mkdir => ChangesetItem {
            id: entry.id.clone(),
            kind: ChangesetItemKind::Created,
            properties: Vec::new(),
            diff: None,
        },
        WriteTool::FileDelete => ChangesetItem {
            id: entry.id.clone(),
            kind: ChangesetItemKind::Deleted,
            properties: Vec::new(),
            diff: None,
        },
        WriteTool::FileWrite => {
            let path = entry_path(entry)?;
            let (old, new) = file_contents(entry)?;
            ChangesetItem {
                id: entry.id.clone(),
                kind: ChangesetItemKind::Modified,
                properties: Vec::new(),
                diff: Some(unified_diff(&path, &old, &new)),
            }
        }
        WriteTool::FileRename | WriteTool::FileMove | WriteTool::SetMain => ChangesetItem {
            id: entry.id.clone(),
            kind: ChangesetItemKind::Modified,
            properties: Vec::new(),
            diff: None,
        },
        WriteTool::SettingsSet
        | WriteTool::PluginAdd
        | WriteTool::PluginEdit
        | WriteTool::PluginRemove
        | WriteTool::PluginMove => ChangesetItem {
            id: entry.id.clone(),
            kind: ChangesetItemKind::Modified,
            properties: Vec::new(),
            diff: None,
        },
        WriteTool::LocationWrite => ChangesetItem {
            id: entry.id.clone(),
            kind: location_write_changeset_kind(entry)?,
            properties: location_write_changeset_properties(entry)?,
            diff: None,
        },
        WriteTool::DatSet
        | WriteTool::XdatSet
        | WriteTool::TblSet
        | WriteTool::ReqSet
        | WriteTool::BtnSet => {
            return Ok(None);
        }
    };
    Ok(Some(item))
}

fn location_write_changeset_kind(entry: &JournalEntry) -> Result<ChangesetItemKind, JournalError> {
    let Snapshot::MapEdit { action, .. } = &entry.after else {
        return Err(invalid_entry(entry, "expected map edit after snapshot"));
    };
    match action.as_str() {
        "add" => Ok(ChangesetItemKind::Created),
        "delete" => Ok(ChangesetItemKind::Deleted),
        "set" | "rename" => Ok(ChangesetItemKind::Modified),
        _ => Err(invalid_entry(entry, "expected location_write action")),
    }
}

fn location_write_changeset_properties(
    entry: &JournalEntry,
) -> Result<Vec<PropertyChange>, JournalError> {
    let JournalTarget::Map { path, summary } = &entry.target else {
        return Err(invalid_entry(entry, "expected map target"));
    };
    Ok(vec![
        PropertyChange {
            property: "summary".to_owned(),
            old: serde_json::Value::Null,
            new: serde_json::json!(summary),
        },
        PropertyChange {
            property: "map".to_owned(),
            old: serde_json::Value::Null,
            new: serde_json::json!(path),
        },
    ])
}

fn unified_diff(path: &str, old: &str, new: &str) -> String {
    TextDiff::from_lines(old, new)
        .unified_diff()
        .header(&format!("old/{path}"), &format!("new/{path}"))
        .to_string()
}

fn file_contents(entry: &JournalEntry) -> Result<(String, String), JournalError> {
    match (&entry.before, &entry.after) {
        (Snapshot::FileContent { content: old }, Snapshot::FileContent { content: new }) => {
            Ok((old.clone(), new.clone()))
        }
        _ => Err(invalid_entry(entry, "expected file content snapshots")),
    }
}

fn rejected_entries<'a>(journal: &'a Journal, ids: &DecisionIds) -> Vec<&'a JournalEntry> {
    match ids {
        DecisionIds::All => journal.entries.iter().collect(),
        DecisionIds::Items(ids) => journal
            .entries
            .iter()
            .filter(|entry| {
                ids.iter()
                    .any(|id| id == &entry.id || id == &changeset_item_id(entry))
            })
            .collect(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RejectTarget {
    Dat {
        table: DatTable,
        obj_id: u32,
        property: String,
    },
    Path(String),
    Setting(String),
    PluginIndex(usize),
}

impl fmt::Display for RejectTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Dat {
                table,
                obj_id,
                property,
            } => write!(f, "dat:{table}:{obj_id}:{property}"),
            Self::Path(path) => write!(f, "path:{path}"),
            Self::Setting(key) => write!(f, "setting:{key}"),
            Self::PluginIndex(index) => write!(f, "plugin-index:{index}"),
        }
    }
}

fn validate_tail_reject(
    request_id: &str,
    journal: &Journal,
    rejected: &[&JournalEntry],
) -> Result<(), JournalError> {
    let rejected_ids: HashSet<&str> = rejected.iter().map(|entry| entry.id.as_str()).collect();

    for entry in rejected {
        let targets = reject_targets(entry)?;
        for later in journal
            .entries
            .iter()
            .filter(|later| later.seq > entry.seq && !rejected_ids.contains(later.id.as_str()))
        {
            let Ok(later_targets) = reject_targets(later) else {
                continue;
            };
            if let Some(target) = targets.iter().find(|target| {
                later_targets
                    .iter()
                    .any(|later_target| later_target == *target)
            }) {
                return Err(JournalError::NonTailReject {
                    request_id: request_id.to_owned(),
                    target: target.to_string(),
                });
            }
        }
    }

    Ok(())
}

fn reject_targets(entry: &JournalEntry) -> Result<Vec<RejectTarget>, JournalError> {
    let mut targets = match &entry.target {
        JournalTarget::Dat {
            table,
            obj_id,
            property,
        } => vec![RejectTarget::Dat {
            table: *table,
            obj_id: *obj_id,
            property: property.clone(),
        }],
        JournalTarget::Path { path } => vec![RejectTarget::Path(path.clone())],
        JournalTarget::Rename { from, to } => {
            vec![
                RejectTarget::Path(from.clone()),
                RejectTarget::Path(to.clone()),
            ]
        }
        JournalTarget::Setting { key } => vec![RejectTarget::Setting(key.clone())],
        JournalTarget::Plugin { .. } => plugin_targets(entry)?,
        JournalTarget::Map { path, .. } => vec![RejectTarget::Path(path.clone())],
    };
    targets.dedup();
    Ok(targets)
}

fn plugin_targets(entry: &JournalEntry) -> Result<Vec<RejectTarget>, JournalError> {
    let mut targets = Vec::new();
    if let Some(index) = plugin_snapshot_index(entry, &entry.before)? {
        targets.push(RejectTarget::PluginIndex(index));
    }
    if let Some(index) = plugin_snapshot_index(entry, &entry.after)? {
        targets.push(RejectTarget::PluginIndex(index));
    }
    targets.dedup();

    if targets.is_empty() {
        return Err(invalid_entry(entry, "expected plugin index snapshot"));
    }

    Ok(targets)
}

fn plugin_snapshot_index(
    entry: &JournalEntry,
    snapshot: &Snapshot,
) -> Result<Option<usize>, JournalError> {
    match snapshot {
        Snapshot::PluginTexts { index, .. } => Ok(Some(*index)),
        Snapshot::PluginAbsent => Ok(None),
        _ => Err(invalid_entry(entry, "expected plugin snapshot")),
    }
}

fn changeset_item_id(entry: &JournalEntry) -> String {
    match &entry.target {
        JournalTarget::Dat { table, obj_id, .. } => format!("dat:{table}:{obj_id}"),
        _ => entry.id.clone(),
    }
}

fn apply_inverse<B>(entry: &JournalEntry, bridge: &B) -> Result<(), JournalError>
where
    B: JournalBridge,
    B::Error: fmt::Display,
{
    match entry.tool {
        WriteTool::DatSet
        | WriteTool::XdatSet
        | WriteTool::TblSet
        | WriteTool::ReqSet
        | WriteTool::BtnSet => {
            let (table, obj_id, property) = dat_target_parts(entry)?;
            match &entry.before {
                Snapshot::DatValue {
                    value: _,
                    was_default: true,
                } => bridge
                    .reset_dat_value(table, obj_id, property)
                    .map_err(bridge_error),
                Snapshot::DatValue {
                    value,
                    was_default: false,
                } => bridge
                    .set_dat_value(table, obj_id, property, value.clone())
                    .map_err(bridge_error),
                _ => Err(invalid_entry(entry, "expected dat before snapshot")),
            }
        }
        WriteTool::FileWrite => {
            let path = entry_path(entry)?;
            match &entry.before {
                Snapshot::FileContent { content } => {
                    bridge.write_file(&path, content).map_err(bridge_error)
                }
                _ => Err(invalid_entry(
                    entry,
                    "expected file content before snapshot",
                )),
            }
        }
        WriteTool::FileCreate | WriteTool::Mkdir => {
            let path = entry_path(entry)?;
            bridge.delete_file(&path).map_err(bridge_error)
        }
        WriteTool::FileDelete => {
            let path = entry_path(entry)?;
            match &entry.before {
                Snapshot::DeletedFile { content, position } => bridge
                    .create_file(&path, content, *position)
                    .map_err(bridge_error),
                _ => Err(invalid_entry(
                    entry,
                    "expected deleted file before snapshot",
                )),
            }
        }
        WriteTool::FileRename | WriteTool::FileMove => {
            let (from, to) = rename_inverse(entry)?;
            bridge.rename_path(&from, &to).map_err(bridge_error)
        }
        WriteTool::SetMain => match &entry.before {
            Snapshot::MainPath { path } => bridge.set_main(path.as_deref()).map_err(bridge_error),
            _ => Err(invalid_entry(entry, "expected main path before snapshot")),
        },
        WriteTool::SettingsSet => {
            let key = setting_key(entry)?;
            match &entry.before {
                Snapshot::SettingValue { value } => {
                    bridge.set_setting(key, value.clone()).map_err(bridge_error)
                }
                _ => Err(invalid_entry(entry, "expected setting before snapshot")),
            }
        }
        WriteTool::PluginAdd => {
            let plugin_id = plugin_id(entry)?;
            bridge.plugin_remove(plugin_id).map_err(bridge_error)
        }
        WriteTool::PluginEdit => {
            let plugin_id = plugin_id(entry)?;
            match &entry.before {
                Snapshot::PluginTexts { texts, index } => bridge
                    .plugin_edit(plugin_id, texts.clone(), *index)
                    .map_err(bridge_error),
                _ => Err(invalid_entry(
                    entry,
                    "expected plugin texts before snapshot",
                )),
            }
        }
        WriteTool::PluginRemove => {
            let plugin_id = plugin_id(entry)?;
            match &entry.before {
                Snapshot::PluginTexts { texts, index } => bridge
                    .plugin_add(plugin_id, texts.clone(), *index)
                    .map_err(bridge_error),
                _ => Err(invalid_entry(
                    entry,
                    "expected plugin texts before snapshot",
                )),
            }
        }
        WriteTool::PluginMove => {
            let plugin_id = plugin_id(entry)?;
            match &entry.before {
                Snapshot::PluginTexts { index, .. } => {
                    bridge.plugin_move(plugin_id, *index).map_err(bridge_error)
                }
                _ => Err(invalid_entry(
                    entry,
                    "expected plugin texts before snapshot",
                )),
            }
        }
        WriteTool::LocationWrite => match &entry.before {
            Snapshot::MapBackup {
                map_path,
                backup_path,
            } => bridge
                .restore_map_backup(map_path, backup_path)
                .map_err(bridge_error),
            _ => Err(invalid_entry(entry, "expected map backup before snapshot")),
        },
    }
}

fn dat_target_parts(entry: &JournalEntry) -> Result<(DatTable, u32, &str), JournalError> {
    match &entry.target {
        JournalTarget::Dat {
            table,
            obj_id,
            property,
        } => Ok((*table, *obj_id, property.as_str())),
        _ => Err(invalid_entry(entry, "expected dat target")),
    }
}

fn entry_path(entry: &JournalEntry) -> Result<String, JournalError> {
    match &entry.target {
        JournalTarget::Path { path } => Ok(path.clone()),
        JournalTarget::Rename { to, .. } => Ok(to.clone()),
        _ => Err(invalid_entry(entry, "expected path target")),
    }
}

fn rename_inverse(entry: &JournalEntry) -> Result<(String, String), JournalError> {
    match &entry.target {
        JournalTarget::Rename { from, to } => Ok((to.clone(), from.clone())),
        _ => match (&entry.before, &entry.after) {
            (Snapshot::Path { path: old }, Snapshot::Path { path: new }) => {
                Ok((new.clone(), old.clone()))
            }
            _ => Err(invalid_entry(
                entry,
                "expected rename target or path snapshots",
            )),
        },
    }
}

fn setting_key(entry: &JournalEntry) -> Result<&str, JournalError> {
    match &entry.target {
        JournalTarget::Setting { key } => Ok(key),
        _ => Err(invalid_entry(entry, "expected setting target")),
    }
}

fn plugin_id(entry: &JournalEntry) -> Result<&str, JournalError> {
    match &entry.target {
        JournalTarget::Plugin { plugin_id } => Ok(plugin_id),
        _ => Err(invalid_entry(entry, "expected plugin target")),
    }
}

fn invalid_entry(entry: &JournalEntry, message: &str) -> JournalError {
    JournalError::InvalidEntry {
        entry_id: entry.id.clone(),
        message: message.to_owned(),
    }
}

fn bridge_error(error: impl fmt::Display) -> JournalError {
    JournalError::Bridge(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::RefCell;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum AppliedInverse {
        DatSet {
            table: DatTable,
            obj_id: u32,
            property: String,
            value: serde_json::Value,
        },
        DatReset {
            table: DatTable,
            obj_id: u32,
            property: String,
        },
        WriteFile {
            path: String,
            content: String,
        },
        DeleteFile {
            path: String,
        },
        CreateFile {
            path: String,
            content: String,
            position: Option<usize>,
        },
        Rename {
            from: String,
            to: String,
        },
        SetMain {
            path: Option<String>,
        },
        SettingsSet {
            key: String,
            value: serde_json::Value,
        },
        PluginAdd {
            plugin_id: String,
            texts: Vec<String>,
            index: usize,
        },
        PluginEdit {
            plugin_id: String,
            texts: Vec<String>,
            index: usize,
        },
        PluginRemove {
            plugin_id: String,
        },
        PluginMove {
            plugin_id: String,
            index: usize,
        },
        RestoreMapBackup {
            map_path: String,
            backup_path: String,
        },
    }

    #[derive(Default)]
    struct FakeBridge {
        ops: RefCell<Vec<AppliedInverse>>,
    }

    impl FakeBridge {
        fn ops(&self) -> Vec<AppliedInverse> {
            self.ops.borrow().clone()
        }
    }

    impl JournalBridge for FakeBridge {
        type Error = String;

        fn set_dat_value(
            &self,
            table: DatTable,
            obj_id: u32,
            property: &str,
            value: serde_json::Value,
        ) -> Result<(), Self::Error> {
            self.ops.borrow_mut().push(AppliedInverse::DatSet {
                table,
                obj_id,
                property: property.to_owned(),
                value,
            });
            Ok(())
        }

        fn reset_dat_value(
            &self,
            table: DatTable,
            obj_id: u32,
            property: &str,
        ) -> Result<(), Self::Error> {
            self.ops.borrow_mut().push(AppliedInverse::DatReset {
                table,
                obj_id,
                property: property.to_owned(),
            });
            Ok(())
        }

        fn write_file(&self, path: &str, content: &str) -> Result<(), Self::Error> {
            self.ops.borrow_mut().push(AppliedInverse::WriteFile {
                path: path.to_owned(),
                content: content.to_owned(),
            });
            Ok(())
        }

        fn delete_file(&self, path: &str) -> Result<(), Self::Error> {
            self.ops.borrow_mut().push(AppliedInverse::DeleteFile {
                path: path.to_owned(),
            });
            Ok(())
        }

        fn create_file(
            &self,
            path: &str,
            content: &str,
            position: Option<usize>,
        ) -> Result<(), Self::Error> {
            self.ops.borrow_mut().push(AppliedInverse::CreateFile {
                path: path.to_owned(),
                content: content.to_owned(),
                position,
            });
            Ok(())
        }

        fn rename_path(&self, from: &str, to: &str) -> Result<(), Self::Error> {
            self.ops.borrow_mut().push(AppliedInverse::Rename {
                from: from.to_owned(),
                to: to.to_owned(),
            });
            Ok(())
        }

        fn set_main(&self, path: Option<&str>) -> Result<(), Self::Error> {
            self.ops.borrow_mut().push(AppliedInverse::SetMain {
                path: path.map(str::to_owned),
            });
            Ok(())
        }

        fn set_setting(&self, key: &str, value: serde_json::Value) -> Result<(), Self::Error> {
            self.ops.borrow_mut().push(AppliedInverse::SettingsSet {
                key: key.to_owned(),
                value,
            });
            Ok(())
        }

        fn plugin_add(
            &self,
            plugin_id: &str,
            texts: Vec<String>,
            index: usize,
        ) -> Result<(), Self::Error> {
            self.ops.borrow_mut().push(AppliedInverse::PluginAdd {
                plugin_id: plugin_id.to_owned(),
                texts,
                index,
            });
            Ok(())
        }

        fn plugin_edit(
            &self,
            plugin_id: &str,
            texts: Vec<String>,
            index: usize,
        ) -> Result<(), Self::Error> {
            self.ops.borrow_mut().push(AppliedInverse::PluginEdit {
                plugin_id: plugin_id.to_owned(),
                texts,
                index,
            });
            Ok(())
        }

        fn plugin_remove(&self, plugin_id: &str) -> Result<(), Self::Error> {
            self.ops.borrow_mut().push(AppliedInverse::PluginRemove {
                plugin_id: plugin_id.to_owned(),
            });
            Ok(())
        }

        fn plugin_move(&self, plugin_id: &str, index: usize) -> Result<(), Self::Error> {
            self.ops.borrow_mut().push(AppliedInverse::PluginMove {
                plugin_id: plugin_id.to_owned(),
                index,
            });
            Ok(())
        }

        fn restore_map_backup(&self, map_path: &str, backup_path: &str) -> Result<(), Self::Error> {
            self.ops
                .borrow_mut()
                .push(AppliedInverse::RestoreMapBackup {
                    map_path: map_path.to_owned(),
                    backup_path: backup_path.to_owned(),
                });
            Ok(())
        }
    }

    fn temp_data_dir(test_name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("eud-agent-{test_name}-{stamp}"));
        fs::create_dir_all(&dir).expect("temp data dir should be creatable");
        dir
    }

    fn entry(
        id: &str,
        seq: u64,
        tool: WriteTool,
        target: JournalTarget,
        before: Snapshot,
        after: Snapshot,
    ) -> JournalEntry {
        JournalEntry {
            id: id.to_owned(),
            seq,
            tool,
            target,
            before,
            after,
            ts: 1_718_000_000 + seq,
        }
    }

    fn dat_target(table: DatTable, obj_id: u32, property: &str) -> JournalTarget {
        JournalTarget::Dat {
            table,
            obj_id,
            property: property.to_owned(),
        }
    }

    fn path_target(path: &str) -> JournalTarget {
        JournalTarget::Path {
            path: path.to_owned(),
        }
    }

    fn location_write_entry(
        id: &str,
        action: &str,
        location_id: Option<i64>,
        name: Option<&str>,
    ) -> JournalEntry {
        entry(
            id,
            1,
            WriteTool::LocationWrite,
            JournalTarget::Map {
                path: "C:/maps/demo.scx".to_owned(),
                summary: format!(
                    "{} {}",
                    action,
                    name.map(str::to_owned)
                        .or_else(|| location_id.map(|id| format!("#{id}")))
                        .unwrap_or_else(|| "location".to_owned())
                ),
            },
            Snapshot::MapBackup {
                map_path: "C:/maps/demo.scx".to_owned(),
                backup_path: "C:/Users/me/AppData/Roaming/eud-agent/map_backups/demo.bak"
                    .to_owned(),
            },
            Snapshot::MapEdit {
                action: action.to_owned(),
                location_id,
                name: name.map(str::to_owned),
            },
        )
    }

    #[test]
    fn location_write_changeset_kind_follows_map_edit_action() {
        for (action, expected_kind) in [
            ("add", ChangesetItemKind::Created),
            ("delete", ChangesetItemKind::Deleted),
            ("set", ChangesetItemKind::Modified),
            ("rename", ChangesetItemKind::Modified),
        ] {
            let journal = Journal {
                request_id: format!("req-location-{action}"),
                entries: vec![location_write_entry(
                    &format!("loc-{action}"),
                    action,
                    Some(5),
                    Some("spot"),
                )],
            };

            let changeset = changeset_from_journal(&journal).unwrap();

            assert_eq!(changeset.items.len(), 1);
            assert_eq!(changeset.items[0].id, format!("loc-{action}"));
            assert_eq!(changeset.items[0].kind, expected_kind);
            assert!(changeset.items[0].properties.contains(&PropertyChange {
                property: "summary".to_owned(),
                old: serde_json::Value::Null,
                new: json!(format!("{action} spot")),
            }));
            assert!(changeset.items[0].properties.contains(&PropertyChange {
                property: "map".to_owned(),
                old: serde_json::Value::Null,
                new: json!("C:/maps/demo.scx"),
            }));
            assert!(changeset.items[0].diff.is_none());
        }
    }

    #[test]
    fn location_write_inverse_restores_recorded_map_backup() {
        let entry = location_write_entry("loc-rename", "rename", Some(5), Some("spot"));
        let bridge = FakeBridge::default();

        apply_inverse(&entry, &bridge).expect("location_write inverse should restore backup");

        assert_eq!(
            bridge.ops(),
            vec![AppliedInverse::RestoreMapBackup {
                map_path: "C:/maps/demo.scx".to_owned(),
                backup_path: "C:/Users/me/AppData/Roaming/eud-agent/map_backups/demo.bak"
                    .to_owned(),
            }]
        );
    }

    #[test]
    fn snapshot_rollback_round_trip_per_tool_kind_uses_reverse_seq_inverse_ops() {
        let data_dir = temp_data_dir("rollback-all-kinds");
        let store = JournalStore::new(&data_dir);
        let request_id = "req-all-kinds";

        let entries = vec![
            entry(
                "dat-1",
                1,
                WriteTool::DatSet,
                dat_target(DatTable::Dat, 7, "HitPoints"),
                Snapshot::DatValue {
                    value: json!(40),
                    was_default: false,
                },
                Snapshot::DatValue {
                    value: json!(80),
                    was_default: false,
                },
            ),
            entry(
                "xdat-1",
                2,
                WriteTool::XdatSet,
                dat_target(DatTable::Xdat, 3, "ButtonSet"),
                Snapshot::DatValue {
                    value: json!(null),
                    was_default: true,
                },
                Snapshot::DatValue {
                    value: json!(9),
                    was_default: false,
                },
            ),
            entry(
                "tbl-1",
                3,
                WriteTool::TblSet,
                dat_target(DatTable::Tbl, 11, "String"),
                Snapshot::DatValue {
                    value: json!("old tbl"),
                    was_default: false,
                },
                Snapshot::DatValue {
                    value: json!("new tbl"),
                    was_default: false,
                },
            ),
            entry(
                "req-1",
                4,
                WriteTool::ReqSet,
                dat_target(DatTable::Req, 12, "Use"),
                Snapshot::DatValue {
                    value: json!("old req"),
                    was_default: false,
                },
                Snapshot::DatValue {
                    value: json!("new req"),
                    was_default: false,
                },
            ),
            entry(
                "btn-1",
                5,
                WriteTool::BtnSet,
                dat_target(DatTable::Btn, 13, "actstr"),
                Snapshot::DatValue {
                    value: json!("old btn"),
                    was_default: false,
                },
                Snapshot::DatValue {
                    value: json!("new btn"),
                    was_default: false,
                },
            ),
            entry(
                "file-write",
                6,
                WriteTool::FileWrite,
                path_target("scripts/main.eps"),
                Snapshot::FileContent {
                    content: "old code\n".to_owned(),
                },
                Snapshot::FileContent {
                    content: "new code\n".to_owned(),
                },
            ),
            entry(
                "file-create",
                7,
                WriteTool::FileCreate,
                path_target("scripts/new.eps"),
                Snapshot::Created,
                Snapshot::FileContent {
                    content: "created code\n".to_owned(),
                },
            ),
            entry(
                "mkdir",
                8,
                WriteTool::Mkdir,
                path_target("scripts/generated"),
                Snapshot::Created,
                Snapshot::Created,
            ),
            entry(
                "file-delete",
                9,
                WriteTool::FileDelete,
                path_target("scripts/deleted.eps"),
                Snapshot::DeletedFile {
                    content: "deleted code\n".to_owned(),
                    position: Some(4),
                },
                Snapshot::Deleted,
            ),
            entry(
                "file-rename",
                10,
                WriteTool::FileRename,
                JournalTarget::Rename {
                    from: "scripts/old-name.eps".to_owned(),
                    to: "scripts/new-name.eps".to_owned(),
                },
                Snapshot::Path {
                    path: "scripts/old-name.eps".to_owned(),
                },
                Snapshot::Path {
                    path: "scripts/new-name.eps".to_owned(),
                },
            ),
            entry(
                "file-move",
                11,
                WriteTool::FileMove,
                JournalTarget::Rename {
                    from: "scripts/moved-from.eps".to_owned(),
                    to: "lib/moved-to.eps".to_owned(),
                },
                Snapshot::Path {
                    path: "scripts/moved-from.eps".to_owned(),
                },
                Snapshot::Path {
                    path: "lib/moved-to.eps".to_owned(),
                },
            ),
            entry(
                "set-main",
                12,
                WriteTool::SetMain,
                path_target("scripts/main.eps"),
                Snapshot::MainPath {
                    path: Some("scripts/old-main.eps".to_owned()),
                },
                Snapshot::MainPath {
                    path: Some("scripts/main.eps".to_owned()),
                },
            ),
            entry(
                "settings",
                13,
                WriteTool::SettingsSet,
                JournalTarget::Setting {
                    key: "program.euddraft".to_owned(),
                },
                Snapshot::SettingValue {
                    value: json!("old.exe"),
                },
                Snapshot::SettingValue {
                    value: json!("new.exe"),
                },
            ),
            entry(
                "plugin-add",
                14,
                WriteTool::PluginAdd,
                JournalTarget::Plugin {
                    plugin_id: "alpha".to_owned(),
                },
                Snapshot::PluginAbsent,
                Snapshot::PluginTexts {
                    texts: vec!["alpha text".to_owned()],
                    index: 0,
                },
            ),
            entry(
                "plugin-edit",
                15,
                WriteTool::PluginEdit,
                JournalTarget::Plugin {
                    plugin_id: "beta".to_owned(),
                },
                Snapshot::PluginTexts {
                    texts: vec!["old beta".to_owned()],
                    index: 2,
                },
                Snapshot::PluginTexts {
                    texts: vec!["new beta".to_owned()],
                    index: 2,
                },
            ),
            entry(
                "plugin-remove",
                16,
                WriteTool::PluginRemove,
                JournalTarget::Plugin {
                    plugin_id: "gamma".to_owned(),
                },
                Snapshot::PluginTexts {
                    texts: vec!["gamma text".to_owned()],
                    index: 3,
                },
                Snapshot::PluginAbsent,
            ),
            entry(
                "plugin-move",
                17,
                WriteTool::PluginMove,
                JournalTarget::Plugin {
                    plugin_id: "delta".to_owned(),
                },
                Snapshot::PluginTexts {
                    texts: vec!["delta text".to_owned()],
                    index: 1,
                },
                Snapshot::PluginTexts {
                    texts: vec!["delta text".to_owned()],
                    index: 5,
                },
            ),
        ];

        for entry in entries {
            store
                .record(request_id, entry)
                .expect("journal entry should record");
        }
        store
            .persist(request_id)
            .expect("journal should persist before rollback");

        let bridge = FakeBridge::default();
        store
            .decide(
                request_id,
                ChangesetDecision::reject(DecisionIds::All),
                &bridge,
            )
            .expect("reject all should rollback");

        assert_eq!(
            bridge.ops(),
            vec![
                AppliedInverse::PluginMove {
                    plugin_id: "delta".to_owned(),
                    index: 1,
                },
                AppliedInverse::PluginAdd {
                    plugin_id: "gamma".to_owned(),
                    texts: vec!["gamma text".to_owned()],
                    index: 3,
                },
                AppliedInverse::PluginEdit {
                    plugin_id: "beta".to_owned(),
                    texts: vec!["old beta".to_owned()],
                    index: 2,
                },
                AppliedInverse::PluginRemove {
                    plugin_id: "alpha".to_owned(),
                },
                AppliedInverse::SettingsSet {
                    key: "program.euddraft".to_owned(),
                    value: json!("old.exe"),
                },
                AppliedInverse::SetMain {
                    path: Some("scripts/old-main.eps".to_owned()),
                },
                AppliedInverse::Rename {
                    from: "lib/moved-to.eps".to_owned(),
                    to: "scripts/moved-from.eps".to_owned(),
                },
                AppliedInverse::Rename {
                    from: "scripts/new-name.eps".to_owned(),
                    to: "scripts/old-name.eps".to_owned(),
                },
                AppliedInverse::CreateFile {
                    path: "scripts/deleted.eps".to_owned(),
                    content: "deleted code\n".to_owned(),
                    position: Some(4),
                },
                AppliedInverse::DeleteFile {
                    path: "scripts/generated".to_owned(),
                },
                AppliedInverse::DeleteFile {
                    path: "scripts/new.eps".to_owned(),
                },
                AppliedInverse::WriteFile {
                    path: "scripts/main.eps".to_owned(),
                    content: "old code\n".to_owned(),
                },
                AppliedInverse::DatSet {
                    table: DatTable::Btn,
                    obj_id: 13,
                    property: "actstr".to_owned(),
                    value: json!("old btn"),
                },
                AppliedInverse::DatSet {
                    table: DatTable::Req,
                    obj_id: 12,
                    property: "Use".to_owned(),
                    value: json!("old req"),
                },
                AppliedInverse::DatSet {
                    table: DatTable::Tbl,
                    obj_id: 11,
                    property: "String".to_owned(),
                    value: json!("old tbl"),
                },
                AppliedInverse::DatReset {
                    table: DatTable::Xdat,
                    obj_id: 3,
                    property: "ButtonSet".to_owned(),
                },
                AppliedInverse::DatSet {
                    table: DatTable::Dat,
                    obj_id: 7,
                    property: "HitPoints".to_owned(),
                    value: json!(40),
                },
            ]
        );
    }

    #[test]
    fn explicit_reset_recreate_deleted_file_and_rename_back_inverses() {
        let data_dir = temp_data_dir("specific-inverses");
        let store = JournalStore::new(&data_dir);
        let request_id = "req-specific";

        store
            .record(
                request_id,
                entry(
                    "default-reset",
                    1,
                    WriteTool::DatSet,
                    dat_target(DatTable::Dat, 1, "Cost"),
                    Snapshot::DatValue {
                        value: json!(null),
                        was_default: true,
                    },
                    Snapshot::DatValue {
                        value: json!(99),
                        was_default: false,
                    },
                ),
            )
            .expect("dat entry should record");
        store
            .record(
                request_id,
                entry(
                    "deleted-file",
                    2,
                    WriteTool::FileDelete,
                    path_target("a.eps"),
                    Snapshot::DeletedFile {
                        content: "before delete\n".to_owned(),
                        position: Some(0),
                    },
                    Snapshot::Deleted,
                ),
            )
            .expect("delete entry should record");
        store
            .record(
                request_id,
                entry(
                    "rename",
                    3,
                    WriteTool::FileRename,
                    JournalTarget::Rename {
                        from: "old.eps".to_owned(),
                        to: "new.eps".to_owned(),
                    },
                    Snapshot::Path {
                        path: "old.eps".to_owned(),
                    },
                    Snapshot::Path {
                        path: "new.eps".to_owned(),
                    },
                ),
            )
            .expect("rename entry should record");

        let bridge = FakeBridge::default();
        store
            .decide(
                request_id,
                ChangesetDecision::reject(DecisionIds::All),
                &bridge,
            )
            .expect("reject should apply inverses");

        assert_eq!(
            bridge.ops(),
            vec![
                AppliedInverse::Rename {
                    from: "new.eps".to_owned(),
                    to: "old.eps".to_owned(),
                },
                AppliedInverse::CreateFile {
                    path: "a.eps".to_owned(),
                    content: "before delete\n".to_owned(),
                    position: Some(0),
                },
                AppliedInverse::DatReset {
                    table: DatTable::Dat,
                    obj_id: 1,
                    property: "Cost".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn changeset_groups_dat_by_obj_id_and_includes_unified_diff_for_modified_file() {
        let data_dir = temp_data_dir("changeset");
        let store = JournalStore::new(&data_dir);
        let request_id = "req-changeset";

        store
            .record(
                request_id,
                entry(
                    "dat-name",
                    1,
                    WriteTool::DatSet,
                    dat_target(DatTable::Dat, 5, "Name"),
                    Snapshot::DatValue {
                        value: json!("Marine"),
                        was_default: false,
                    },
                    Snapshot::DatValue {
                        value: json!("Veteran Marine"),
                        was_default: false,
                    },
                ),
            )
            .expect("first dat entry should record");
        store
            .record(
                request_id,
                entry(
                    "dat-hp",
                    2,
                    WriteTool::DatSet,
                    dat_target(DatTable::Dat, 5, "HitPoints"),
                    Snapshot::DatValue {
                        value: json!(40),
                        was_default: false,
                    },
                    Snapshot::DatValue {
                        value: json!(45),
                        was_default: false,
                    },
                ),
            )
            .expect("second dat entry should record");
        store
            .record(
                request_id,
                entry(
                    "modified-file",
                    3,
                    WriteTool::FileWrite,
                    path_target("scripts/main.eps"),
                    Snapshot::FileContent {
                        content: "function main() {\n    old_call();\n}\n".to_owned(),
                    },
                    Snapshot::FileContent {
                        content: "function main() {\n    new_call();\n}\n".to_owned(),
                    },
                ),
            )
            .expect("file entry should record");

        let changeset = store
            .changeset(request_id)
            .expect("changeset should be emitted");

        let dat_item = changeset
            .items
            .iter()
            .find(|item| item.id == "dat:Dat:5")
            .expect("dat properties for the same objId should be grouped");
        assert_eq!(dat_item.kind, ChangesetItemKind::Dat);
        assert_eq!(
            dat_item.properties,
            vec![
                PropertyChange {
                    property: "Name".to_owned(),
                    old: json!("Marine"),
                    new: json!("Veteran Marine"),
                },
                PropertyChange {
                    property: "HitPoints".to_owned(),
                    old: json!(40),
                    new: json!(45),
                },
            ]
        );

        let file_item = changeset
            .items
            .iter()
            .find(|item| item.id == "modified-file")
            .expect("modified file should appear in changeset");
        assert_eq!(file_item.kind, ChangesetItemKind::Modified);
        let diff = file_item
            .diff
            .as_deref()
            .expect("modified file includes diff");
        assert!(diff.contains("--- old/scripts/main.eps"));
        assert!(diff.contains("+++ new/scripts/main.eps"));
        assert!(diff.contains("-    old_call();"));
        assert!(diff.contains("+    new_call();"));
    }

    #[test]
    fn changeset_includes_settings_and_plugin_items_without_path_targets() {
        let data_dir = temp_data_dir("settings-plugin-changeset");
        let store = JournalStore::new(&data_dir);
        let request_id = "req-settings-plugin-changeset";

        store
            .record(
                request_id,
                entry(
                    "settings",
                    1,
                    WriteTool::SettingsSet,
                    JournalTarget::Setting {
                        key: "program.euddraft".to_owned(),
                    },
                    Snapshot::SettingValue {
                        value: json!("old.exe"),
                    },
                    Snapshot::SettingValue {
                        value: json!("new.exe"),
                    },
                ),
            )
            .expect("settings entry should record");
        store
            .record(
                request_id,
                entry(
                    "plugin-add",
                    2,
                    WriteTool::PluginAdd,
                    JournalTarget::Plugin {
                        plugin_id: "alpha".to_owned(),
                    },
                    Snapshot::PluginAbsent,
                    Snapshot::PluginTexts {
                        texts: vec!["alpha text".to_owned()],
                        index: 0,
                    },
                ),
            )
            .expect("plugin entry should record");

        let changeset = store
            .changeset(request_id)
            .expect("changeset should include settings and plugin entries");

        assert_eq!(
            changeset
                .items
                .iter()
                .find(|item| item.id == "settings")
                .map(|item| item.kind),
            Some(ChangesetItemKind::Modified)
        );
        assert_eq!(
            changeset
                .items
                .iter()
                .find(|item| item.id == "plugin-add")
                .map(|item| item.kind),
            Some(ChangesetItemKind::Modified)
        );
    }

    #[test]
    fn journal_json_persists_under_data_dir_without_utf8_bom_and_loads() {
        let data_dir = temp_data_dir("persist");
        let store = JournalStore::new(&data_dir);
        let request_id = "req-persist";

        store
            .record(
                request_id,
                entry(
                    "file-write",
                    1,
                    WriteTool::FileWrite,
                    path_target("main.eps"),
                    Snapshot::FileContent {
                        content: "old\n".to_owned(),
                    },
                    Snapshot::FileContent {
                        content: "new\n".to_owned(),
                    },
                ),
            )
            .expect("entry should record");
        store.persist(request_id).expect("journal should persist");

        let path = data_dir.join("journal").join("req-persist.json");
        assert!(Path::new(&path).exists());
        let bytes = fs::read(&path).expect("journal file should be readable");
        assert_ne!(bytes.first().copied(), Some(0xEF));

        let loaded = JournalStore::load(&data_dir, request_id).expect("journal should load");
        assert_eq!(loaded.request_id, request_id);
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.entries[0].id, "file-write");
    }

    #[test]
    fn accept_archives_journal_without_bridge_ops() {
        let data_dir = temp_data_dir("accept");
        let store = JournalStore::new(&data_dir);
        let request_id = "req-accept";
        store
            .record(
                request_id,
                entry(
                    "created-file",
                    1,
                    WriteTool::FileCreate,
                    path_target("created.eps"),
                    Snapshot::Created,
                    Snapshot::FileContent {
                        content: "new\n".to_owned(),
                    },
                ),
            )
            .expect("entry should record");
        store.persist(request_id).expect("journal should persist");

        let bridge = FakeBridge::default();
        store
            .decide(request_id, ChangesetDecision::accept(), &bridge)
            .expect("accept should archive");

        assert!(bridge.ops().is_empty());
        assert!(!data_dir.join("journal").join("req-accept.json").exists());
        assert!(data_dir
            .join("journal")
            .join("accepted")
            .join("req-accept.json")
            .exists());
    }

    #[test]
    fn mixed_decision_rejects_selected_ids_and_defaults_undecided_to_accepted_on_next_request() {
        let data_dir = temp_data_dir("mixed");
        let store = JournalStore::new(&data_dir);
        let request_id = "req-mixed";

        store
            .record(
                request_id,
                entry(
                    "reject-me",
                    1,
                    WriteTool::FileWrite,
                    path_target("reject.eps"),
                    Snapshot::FileContent {
                        content: "old reject\n".to_owned(),
                    },
                    Snapshot::FileContent {
                        content: "new reject\n".to_owned(),
                    },
                ),
            )
            .expect("first entry should record");
        store
            .record(
                request_id,
                entry(
                    "accept-by-default",
                    2,
                    WriteTool::FileWrite,
                    path_target("accept.eps"),
                    Snapshot::FileContent {
                        content: "old accept\n".to_owned(),
                    },
                    Snapshot::FileContent {
                        content: "new accept\n".to_owned(),
                    },
                ),
            )
            .expect("second entry should record");

        let bridge = FakeBridge::default();
        store
            .decide(
                request_id,
                ChangesetDecision::reject(DecisionIds::Items(vec!["reject-me".to_owned()])),
                &bridge,
            )
            .expect("selected rejection should apply only selected inverse");
        store
            .finalize_undecided_as_accepted(request_id)
            .expect("next request should accept undecided entries");

        assert_eq!(
            bridge.ops(),
            vec![AppliedInverse::WriteFile {
                path: "reject.eps".to_owned(),
                content: "old reject\n".to_owned(),
            }]
        );
        assert!(data_dir
            .join("journal")
            .join("accepted")
            .join("req-mixed.json")
            .exists());
    }

    #[test]
    fn partial_reject_of_non_tail_target_errors_before_inverse_ops() {
        let data_dir = temp_data_dir("non-tail-reject");
        let store = JournalStore::new(&data_dir);
        let request_id = "req-non-tail-reject";

        store
            .record(
                request_id,
                entry(
                    "first-write",
                    1,
                    WriteTool::FileWrite,
                    path_target("scripts/main.eps"),
                    Snapshot::FileContent {
                        content: "old\n".to_owned(),
                    },
                    Snapshot::FileContent {
                        content: "mid\n".to_owned(),
                    },
                ),
            )
            .expect("first write should record");
        store
            .record(
                request_id,
                entry(
                    "second-write",
                    2,
                    WriteTool::FileWrite,
                    path_target("scripts/main.eps"),
                    Snapshot::FileContent {
                        content: "mid\n".to_owned(),
                    },
                    Snapshot::FileContent {
                        content: "new\n".to_owned(),
                    },
                ),
            )
            .expect("second write should record");

        let bridge = FakeBridge::default();
        let result = store.decide(
            request_id,
            ChangesetDecision::reject(DecisionIds::Items(vec!["first-write".to_owned()])),
            &bridge,
        );

        assert!(matches!(
            result,
            Err(JournalError::NonTailReject { request_id: id, target })
                if id == request_id && target == "path:scripts/main.eps"
        ));
        assert!(bridge.ops().is_empty());

        store
            .decide(
                request_id,
                ChangesetDecision::reject(DecisionIds::All),
                &bridge,
            )
            .expect("reject all should rollback the full target tail");

        assert_eq!(
            bridge.ops(),
            vec![
                AppliedInverse::WriteFile {
                    path: "scripts/main.eps".to_owned(),
                    content: "mid\n".to_owned(),
                },
                AppliedInverse::WriteFile {
                    path: "scripts/main.eps".to_owned(),
                    content: "old\n".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn second_concurrent_decision_errors() {
        let data_dir = temp_data_dir("decision-guard");
        let store = JournalStore::new(&data_dir);
        let request_id = "req-decision-guard";

        let _guard = store
            .begin_decision(request_id)
            .expect("first decision guard should be acquired");
        let second = store.begin_decision(request_id);

        assert!(matches!(
            second,
            Err(JournalError::DecisionInProgress { request_id: id }) if id == request_id
        ));
    }
}
