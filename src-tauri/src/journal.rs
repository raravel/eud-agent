use serde::{Deserialize, Serialize};
use similar::TextDiff;
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use thiserror::Error;

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
    PlayerSetup,
    SwitchWrite,
    WorkspaceWrite,
    WorkspaceCreate,
    WorkspaceDelete,
    MapSound,
    PythonDependenciesSet,
    ProjectManifestMigrate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum JournalTarget {
    Dat {
        table: DatTable,
        /// The dat-file identifier the agent addressed (e.g. `units`, `weapons`)
        /// for dat/xdat/req; empty for tbl/btn (index/set keyed). Part of the
        /// changeset identity so `units #5` and `weapons #5` never merge, and so
        /// the panel can label the edit by its real table. `#[serde(default)]`
        /// keeps journals written before this field deserializable.
        #[serde(default)]
        dat: String,
        obj_id: u32,
        property: String,
    },
    Path {
        path: String,
    },
    WorkspacePath {
        workspace_id: String,
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
    MapSound {
        source_map: PathBuf,
        mpq_path: String,
        normalized_sha256: String,
    },
    ProjectManifest {
        path: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapSoundEffects {
    pub volume_percent: u16,
    pub fade_in_ms: u64,
    pub fade_out_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapSoundEditChange {
    pub previous_mpq_path: String,
    pub before: MapSoundEffects,
    pub after: MapSoundEffects,
}
/// Journal snapshots stay inline to avoid one heap allocation per entry; their
/// serde wire shape is durable and pending-review collections are tightly bounded.
#[allow(clippy::large_enum_variant)]
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
        #[serde(default)]
        switch_id: Option<i64>,
        name: Option<String>,
    },
    MapSound {
        source_sha256: String,
        source_codec: String,
        duration_ms: u64,
        channels: u32,
        sample_rate: u32,
        normalization_profile: String,
        normalized_sha256: String,
        normalized_bytes: u64,
        mpq_path: String,
        wav_index: u64,
        string_id: u64,
        map_sha256_before: String,
        map_sha256_after: String,
        backup_path: PathBuf,
        native_report_sha256: String,
        map_bytes_before: u64,
        map_bytes_after: u64,
        source_display_name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        edit: Option<MapSoundEditChange>,
    },
    ManifestBytes {
        bytes: Vec<u8>,
        manifest_sha256: String,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Changeset {
    pub request_id: String,
    pub items: Vec<ChangesetItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangesetItem {
    pub id: String,
    pub kind: ChangesetItemKind,
    /// Editor-relative path for file content ops (write/create/delete); `None`
    /// for dat/settings/plugin/location items. Drives the panel's `file`
    /// category + title bar (created/deleted carry no diff to parse it from).
    pub path: Option<String>,
    /// Identity of a grouped dat/xdat/tbl/req/btn edit (table + dat-file + objId);
    /// `None` for file/flat items. Drives the panel's dat-change card header.
    pub dat_ref: Option<DatRef>,
    pub properties: Vec<PropertyChange>,
    pub diff: Option<String>,
}

/// Identity of a grouped dat-family changeset item (units #5, weapons #3, …).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatRef {
    /// The dat family (Dat/Xdat/Tbl/Req/Btn).
    pub table: DatTable,
    /// The dat-file identifier (e.g. `units`); empty for tbl/btn.
    pub dat: String,
    /// The object index within the table.
    pub obj_id: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangesetItemKind {
    Dat,
    Created,
    Modified,
    Deleted,
    WorkspaceCreated,
    WorkspaceModified,
    WorkspaceDeleted,
    MapSound,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PropertyChange {
    pub property: String,
    pub old: serde_json::Value,
    pub new: serde_json::Value,
    /// The originating journal entry id — the per-property decision target the
    /// panel dispatches (a dat group decides every property's id together).
    pub id: String,
    /// The originating entry's journal sequence (stable render order).
    pub seq: u64,
}

#[derive(Debug, Error)]
pub enum JournalError {
    #[error("journal for request {request_id} was not found")]
    MissingJournal { request_id: String },
    #[error("invalid journal entry {entry_id}: {message}")]
    InvalidEntry { entry_id: String, message: String },
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
}

impl JournalStore {
    pub fn new(data_dir: impl AsRef<Path>) -> Self {
        Self {
            data_dir: data_dir.as_ref().to_path_buf(),
            journals: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn load(data_dir: impl AsRef<Path>, request_id: &str) -> Result<Journal, JournalError> {
        let path = journal_path(data_dir.as_ref(), request_id);
        let bytes = fs::read(path)?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub fn archived_exists(data_dir: impl AsRef<Path>, request_id: &str) -> bool {
        data_dir
            .as_ref()
            .join("journal")
            .join("accepted")
            .join(format!("{request_id}.json"))
            .is_file()
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

    pub fn forget_unpersisted_entry(
        &self,
        request_id: &str,
        entry_id: &str,
    ) -> Result<(), JournalError> {
        if let Some(journal) = lock(&self.journals)?.get_mut(request_id) {
            journal.entries.retain(|entry| entry.id != entry_id);
        }
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

    /// Number of raw journal entries recorded for a request (0 when none exist).
    ///
    /// Unlike [`Self::changeset`] (which GROUPS dat writes per objId), this counts
    /// individual entries — the executor uses it as the monotonic `seq` source so
    /// distinct writes never collide on a sequence number.
    pub fn entry_count(&self, request_id: &str) -> usize {
        self.journal(request_id)
            .map(|journal| journal.entries.len())
            .unwrap_or(0)
    }

    pub fn selected_entries(
        &self,
        request_id: &str,
        ids: &DecisionIds,
    ) -> Result<Vec<JournalEntry>, JournalError> {
        let journal = self.journal(request_id)?;
        Ok(selected_by_ids(&journal, ids)
            .into_iter()
            .cloned()
            .collect())
    }

    /// Mark selected entries accepted without archiving still-undecided items.
    /// The caller must promote any session-workspace bytes before this removal.
    /// Returns `true` only when the request is fully settled and archived.
    pub fn accept_entries(
        &self,
        request_id: &str,
        ids: &DecisionIds,
    ) -> Result<bool, JournalError> {
        if matches!(ids, DecisionIds::All) {
            self.archive(request_id)?;
            return Ok(true);
        }
        let journal = self.journal(request_id)?;
        let accepted_ids = selected_by_ids(&journal, ids)
            .into_iter()
            .map(|entry| entry.id.clone())
            .collect::<Vec<_>>();
        self.forget_entries(request_id, &accepted_ids)?;
        let settled = self
            .journal(request_id)
            .map(|journal| journal.entries.is_empty())
            .unwrap_or(true);
        if settled {
            self.archive(request_id)?;
        }
        Ok(settled)
    }

    /// Archive a live journal that no longer holds any entry.
    pub fn archive_if_empty(&self, request_id: &str) -> Result<bool, JournalError> {
        let empty = self
            .journal(request_id)
            .map(|journal| journal.entries.is_empty())?;
        if empty {
            self.archive(request_id)?;
        }
        Ok(empty)
    }

    /// Drop the listed entry ids from the in-memory journal and re-persist it, so a
    /// rebuilt changeset omits them. A no-op (returns `Ok`) when no journal is loaded
    /// for the request.
    fn forget_entries(&self, request_id: &str, ids: &[String]) -> Result<(), JournalError> {
        {
            let mut journals = lock(&self.journals)?;
            let Some(journal) = journals.get_mut(request_id) else {
                return Ok(());
            };
            journal
                .entries
                .retain(|entry| !ids.iter().any(|id| id == &entry.id));
        }
        self.persist(request_id)
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
    let mut dat_items: HashMap<(DatTable, String, u32), usize> = HashMap::new();

    for entry in &journal.entries {
        match &entry.target {
            JournalTarget::Dat {
                table,
                dat,
                obj_id,
                property,
            } => {
                let (old, new) = dat_values(entry)?;
                // Group by (family, dat-file, objId) so units #5 and weapons #5
                // are distinct items, never a silently-merged "Dat #5".
                let key = (*table, dat.clone(), *obj_id);
                let item_index = match dat_items.get(&key) {
                    Some(index) => *index,
                    None => {
                        let index = items.len();
                        dat_items.insert(key, index);
                        items.push(ChangesetItem {
                            id: changeset_item_id(entry),
                            kind: ChangesetItemKind::Dat,
                            path: None,
                            dat_ref: Some(DatRef {
                                table: *table,
                                dat: dat.clone(),
                                obj_id: *obj_id,
                            }),
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
                    id: entry.id.clone(),
                    seq: entry.seq,
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
            path: Some(entry_path(entry)?),
            dat_ref: None,
            properties: Vec::new(),
            diff: None,
        },
        WriteTool::FileDelete => ChangesetItem {
            id: entry.id.clone(),
            kind: ChangesetItemKind::Deleted,
            path: Some(entry_path(entry)?),
            dat_ref: None,
            properties: Vec::new(),
            diff: None,
        },
        WriteTool::FileWrite => {
            let path = entry_path(entry)?;
            let (old, new) = file_contents(entry)?;
            ChangesetItem {
                id: entry.id.clone(),
                kind: ChangesetItemKind::Modified,
                diff: Some(unified_diff(&path, &old, &new)),
                path: Some(path),
                dat_ref: None,
                properties: Vec::new(),
            }
        }
        WriteTool::WorkspaceCreate => {
            let path = workspace_entry_path(entry)?;
            let Snapshot::FileContent { content } = &entry.after else {
                return Err(invalid_entry(entry, "expected created workspace content"));
            };
            ChangesetItem {
                id: entry.id.clone(),
                kind: ChangesetItemKind::WorkspaceCreated,
                diff: Some(unified_diff(&path, "", content)),
                path: Some(path),
                dat_ref: None,
                properties: Vec::new(),
            }
        }
        WriteTool::WorkspaceDelete => {
            let path = workspace_entry_path(entry)?;
            let Snapshot::DeletedFile { content, .. } = &entry.before else {
                return Err(invalid_entry(entry, "expected deleted workspace content"));
            };
            ChangesetItem {
                id: entry.id.clone(),
                kind: ChangesetItemKind::WorkspaceDeleted,
                diff: Some(unified_diff(&path, content, "")),
                path: Some(path),
                dat_ref: None,
                properties: Vec::new(),
            }
        }
        WriteTool::WorkspaceWrite => {
            let path = workspace_entry_path(entry)?;
            let (old, new) = file_contents(entry)?;
            ChangesetItem {
                id: entry.id.clone(),
                kind: ChangesetItemKind::WorkspaceModified,
                diff: Some(unified_diff(&path, &old, &new)),
                path: Some(path),
                dat_ref: None,
                properties: Vec::new(),
            }
        }
        WriteTool::PythonDependenciesSet | WriteTool::ProjectManifestMigrate => {
            let path = project_manifest_path(entry)?;
            let (old, new) = manifest_contents(entry)?;
            ChangesetItem {
                id: entry.id.clone(),
                kind: ChangesetItemKind::Modified,
                diff: Some(unified_diff(&path, &old, &new)),
                path: Some(path),
                dat_ref: None,
                properties: Vec::new(),
            }
        }
        WriteTool::FileRename | WriteTool::FileMove | WriteTool::SetMain => ChangesetItem {
            id: entry.id.clone(),
            kind: ChangesetItemKind::Modified,
            path: None,
            dat_ref: None,
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
            path: None,
            dat_ref: None,
            properties: Vec::new(),
            diff: None,
        },
        WriteTool::LocationWrite => ChangesetItem {
            id: entry.id.clone(),
            kind: location_write_changeset_kind(entry)?,
            path: None,
            dat_ref: None,
            properties: location_write_changeset_properties(entry)?,
            diff: None,
        },
        WriteTool::PlayerSetup | WriteTool::SwitchWrite => ChangesetItem {
            id: entry.id.clone(),
            kind: ChangesetItemKind::Modified,
            path: None,
            dat_ref: None,
            properties: location_write_changeset_properties(entry)?,
            diff: None,
        },
        WriteTool::MapSound => ChangesetItem {
            id: entry.id.clone(),
            kind: ChangesetItemKind::MapSound,
            path: None,
            dat_ref: None,
            properties: map_sound_changeset_properties(entry)?,
            diff: None,
        },
        WriteTool::DatSet
        | WriteTool::XdatSet
        | WriteTool::TblSet
        | WriteTool::ReqSet
        | WriteTool::BtnSet => return Ok(None),
    };
    Ok(Some(item))
}

fn workspace_entry_path(entry: &JournalEntry) -> Result<String, JournalError> {
    match &entry.target {
        JournalTarget::WorkspacePath { path, .. } => Ok(path.clone()),
        _ => Err(invalid_entry(entry, "expected workspace path target")),
    }
}

fn project_manifest_path(entry: &JournalEntry) -> Result<String, JournalError> {
    match &entry.target {
        JournalTarget::ProjectManifest { path } => Ok(path.clone()),
        _ => Err(invalid_entry(entry, "expected project manifest target")),
    }
}

fn manifest_contents(entry: &JournalEntry) -> Result<(String, String), JournalError> {
    let decode = |snapshot: &Snapshot| match snapshot {
        Snapshot::ManifestBytes { bytes, .. } => String::from_utf8(bytes.clone())
            .map_err(|_| invalid_entry(entry, "project manifest snapshot is not UTF-8")),
        _ => Err(invalid_entry(
            entry,
            "expected project manifest before/after snapshots",
        )),
    };
    Ok((decode(&entry.before)?, decode(&entry.after)?))
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
            id: entry.id.clone(),
            seq: entry.seq,
        },
        PropertyChange {
            property: "map".to_owned(),
            old: serde_json::Value::Null,
            new: serde_json::json!(path),
            id: entry.id.clone(),
            seq: entry.seq,
        },
    ])
}

fn map_sound_changeset_properties(
    entry: &JournalEntry,
) -> Result<Vec<PropertyChange>, JournalError> {
    let JournalTarget::MapSound {
        source_map,
        mpq_path,
        normalized_sha256,
    } = &entry.target
    else {
        return Err(invalid_entry(entry, "expected map sound target"));
    };
    let Snapshot::MapSound {
        source_codec,
        duration_ms,
        normalized_bytes,
        wav_index,
        map_sha256_before,
        map_sha256_after,
        map_bytes_before,
        map_bytes_after,
        source_display_name,
        edit,
        ..
    } = &entry.after
    else {
        return Err(invalid_entry(entry, "expected map sound after snapshot"));
    };
    let map_size_delta = i128::from(*map_bytes_after) - i128::from(*map_bytes_before);
    let previous_mpq_path = edit
        .as_ref()
        .map(|change| serde_json::json!(change.previous_mpq_path))
        .unwrap_or(serde_json::Value::Null);
    let mut values = vec![
        (
            "source",
            serde_json::Value::Null,
            serde_json::json!(source_display_name),
        ),
        (
            "sourceCodec",
            serde_json::Value::Null,
            serde_json::json!(source_codec),
        ),
        (
            "durationMs",
            serde_json::Value::Null,
            serde_json::json!(duration_ms),
        ),
        ("mpqPath", previous_mpq_path, serde_json::json!(mpq_path)),
        (
            "normalizedSha256",
            serde_json::Value::Null,
            serde_json::json!(normalized_sha256),
        ),
        (
            "normalizedBytes",
            serde_json::Value::Null,
            serde_json::json!(normalized_bytes),
        ),
        (
            "wavIndex",
            serde_json::Value::Null,
            serde_json::json!(wav_index),
        ),
        (
            "map",
            serde_json::Value::Null,
            serde_json::json!(source_map),
        ),
        (
            "mapSha256Before",
            serde_json::Value::Null,
            serde_json::json!(map_sha256_before),
        ),
        (
            "mapSha256After",
            serde_json::Value::Null,
            serde_json::json!(map_sha256_after),
        ),
        (
            "mapSizeDelta",
            serde_json::Value::Null,
            serde_json::json!(map_size_delta),
        ),
        (
            "rightsNotice",
            serde_json::Value::Null,
            serde_json::json!("이 오디오를 맵에 배포할 권한은 사용자에게 있어야 합니다."),
        ),
    ];
    if let Some(change) = edit {
        values.extend([
            (
                "volumePercent",
                serde_json::json!(change.before.volume_percent),
                serde_json::json!(change.after.volume_percent),
            ),
            (
                "fadeInMs",
                serde_json::json!(change.before.fade_in_ms),
                serde_json::json!(change.after.fade_in_ms),
            ),
            (
                "fadeOutMs",
                serde_json::json!(change.before.fade_out_ms),
                serde_json::json!(change.after.fade_out_ms),
            ),
        ]);
    }
    Ok(values
        .into_iter()
        .map(|(property, old, new)| PropertyChange {
            property: property.to_string(),
            old,
            new,
            id: entry.id.clone(),
            seq: entry.seq,
        })
        .collect())
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

fn selected_by_ids<'a>(journal: &'a Journal, ids: &DecisionIds) -> Vec<&'a JournalEntry> {
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

fn changeset_item_id(entry: &JournalEntry) -> String {
    match &entry.target {
        JournalTarget::Dat {
            table, dat, obj_id, ..
        } => format!("dat:{table}:{dat}:{obj_id}"),
        _ => entry.id.clone(),
    }
}

fn entry_path(entry: &JournalEntry) -> Result<String, JournalError> {
    match &entry.target {
        JournalTarget::Path { path } => Ok(path.clone()),
        JournalTarget::Rename { to, .. } => Ok(to.clone()),
        _ => Err(invalid_entry(entry, "expected path target")),
    }
}

fn invalid_entry(entry: &JournalEntry, message: &str) -> JournalError {
    JournalError::InvalidEntry {
        entry_id: entry.id.clone(),
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

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

    fn dat_target_named(table: DatTable, dat: &str, obj_id: u32, property: &str) -> JournalTarget {
        JournalTarget::Dat {
            table,
            dat: dat.to_owned(),
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
                switch_id: None,
                name: name.map(str::to_owned),
            },
        )
    }

    fn player_setup_entry(id: &str, action: &str, summary: &str) -> JournalEntry {
        entry(
            id,
            1,
            WriteTool::PlayerSetup,
            JournalTarget::Map {
                path: "C:/maps/demo.scx".to_owned(),
                summary: summary.to_owned(),
            },
            Snapshot::MapBackup {
                map_path: "C:/maps/demo.scx".to_owned(),
                backup_path: "C:/Users/me/AppData/Roaming/eud-agent/map_backups/demo.bak"
                    .to_owned(),
            },
            Snapshot::MapEdit {
                action: action.to_owned(),
                location_id: None,
                switch_id: None,
                name: None,
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
                id: format!("loc-{action}"),
                seq: 1,
            }));
            assert!(changeset.items[0].properties.contains(&PropertyChange {
                property: "map".to_owned(),
                old: serde_json::Value::Null,
                new: json!("C:/maps/demo.scx"),
                id: format!("loc-{action}"),
                seq: 1,
            }));
            assert!(changeset.items[0].diff.is_none());
        }
    }

    #[test]
    fn player_setup_changeset_reuses_map_summary_and_marks_modified() {
        let journal = Journal {
            request_id: "req-player-setup".to_owned(),
            entries: vec![player_setup_entry(
                "plr-1",
                "controller",
                "P1 controller = human",
            )],
        };

        let changeset = changeset_from_journal(&journal).unwrap();

        assert_eq!(changeset.items.len(), 1);
        assert_eq!(changeset.items[0].id, "plr-1");
        assert_eq!(changeset.items[0].kind, ChangesetItemKind::Modified);
        assert!(changeset.items[0].properties.contains(&PropertyChange {
            property: "summary".to_owned(),
            old: serde_json::Value::Null,
            new: json!("P1 controller = human"),
            id: "plr-1".to_owned(),
            seq: 1,
        }));
        assert!(changeset.items[0].properties.contains(&PropertyChange {
            property: "map".to_owned(),
            old: serde_json::Value::Null,
            new: json!("C:/maps/demo.scx"),
            id: "plr-1".to_owned(),
            seq: 1,
        }));
        assert!(changeset.items[0].diff.is_none());
    }

    #[test]
    fn map_sound_changeset_keeps_a_coherent_exact_backup_contract() {
        let before_hash = "1".repeat(64);
        let after_hash = "2".repeat(64);
        let normalized_hash = "3".repeat(64);
        let backup =
            PathBuf::from("C:/Users/me/AppData/Roaming/eud-agent/map_backups/demo.sound.bak");
        let entry = JournalEntry {
            id: "sound-1".to_string(),
            seq: 1,
            tool: WriteTool::MapSound,
            target: JournalTarget::MapSound {
                source_map: PathBuf::from("C:/maps/demo.scx"),
                mpq_path: "staredit\\wav\\ea_3333333333333333.ogg".to_string(),
                normalized_sha256: normalized_hash.clone(),
            },
            before: Snapshot::MapBackup {
                map_path: "C:/maps/demo.scx".to_string(),
                backup_path: backup.to_string_lossy().into_owned(),
            },
            after: Snapshot::MapSound {
                source_sha256: "4".repeat(64),
                source_codec: "flac".to_string(),
                duration_ms: 183_420,
                channels: 2,
                sample_rate: 44_100,
                normalization_profile: "8.1.2;ogg/vorbis/44100/stereo/q4".to_string(),
                normalized_sha256: normalized_hash,
                normalized_bytes: 3_018_201,
                mpq_path: "staredit\\wav\\ea_3333333333333333.ogg".to_string(),
                wav_index: 12,
                string_id: 418,
                map_sha256_before: before_hash,
                map_sha256_after: after_hash,
                backup_path: backup,
                native_report_sha256: "5".repeat(64),
                map_bytes_before: 10_000,
                map_bytes_after: 3_028_301,
                source_display_name: "battle-theme.flac".to_string(),
                edit: None,
            },
            ts: 1,
        };
        let changeset = changeset_from_journal(&Journal {
            request_id: "req-sound".to_string(),
            entries: vec![entry.clone()],
        })
        .unwrap();
        assert_eq!(changeset.items.len(), 1);
        assert_eq!(changeset.items[0].kind, ChangesetItemKind::MapSound);
        assert!(changeset.items[0].properties.iter().any(|property| {
            property.property == "mpqPath"
                && property.new == json!("staredit\\wav\\ea_3333333333333333.ogg")
        }));
        assert!(changeset.items[0].properties.iter().any(|property| {
            property.property == "mapSizeDelta" && property.new == json!(3_018_301_i64)
        }));

        let mut replacement = entry.clone();
        if let JournalTarget::MapSound { mpq_path, .. } = &mut replacement.target {
            *mpq_path = "staredit\\wav\\ea_aaaaaaaaaaaaaaaa.ogg".to_string();
        }
        if let Snapshot::MapSound { edit, .. } = &mut replacement.after {
            *edit = Some(MapSoundEditChange {
                previous_mpq_path: "staredit\\wav\\ea_3333333333333333.ogg".to_string(),
                before: MapSoundEffects {
                    volume_percent: 100,
                    fade_in_ms: 0,
                    fade_out_ms: 0,
                },
                after: MapSoundEffects {
                    volume_percent: 50,
                    fade_in_ms: 1_000,
                    fade_out_ms: 2_000,
                },
            });
        }
        let replacement_changeset = changeset_from_journal(&Journal {
            request_id: "req-sound-edit".to_string(),
            entries: vec![replacement],
        })
        .unwrap();
        assert!(replacement_changeset.items[0]
            .properties
            .iter()
            .any(|property| {
                property.property == "mpqPath"
                    && property.old == json!("staredit\\wav\\ea_3333333333333333.ogg")
                    && property.new == json!("staredit\\wav\\ea_aaaaaaaaaaaaaaaa.ogg")
            }));
        assert!(replacement_changeset.items[0]
            .properties
            .iter()
            .any(|property| {
                property.property == "volumePercent"
                    && property.old == json!(100)
                    && property.new == json!(50)
            }));
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
                    dat_target_named(DatTable::Dat, "units", 5, "Name"),
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
                    dat_target_named(DatTable::Dat, "units", 5, "HitPoints"),
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
            .find(|item| item.id == "dat:Dat:units:5")
            .expect("dat properties for the same (table, dat, objId) should be grouped");
        assert_eq!(dat_item.kind, ChangesetItemKind::Dat);
        assert_eq!(
            dat_item.dat_ref,
            Some(DatRef {
                table: DatTable::Dat,
                dat: "units".to_owned(),
                obj_id: 5,
            })
        );
        assert_eq!(
            dat_item.properties,
            vec![
                PropertyChange {
                    property: "Name".to_owned(),
                    old: json!("Marine"),
                    new: json!("Veteran Marine"),
                    id: "dat-name".to_owned(),
                    seq: 1,
                },
                PropertyChange {
                    property: "HitPoints".to_owned(),
                    old: json!(40),
                    new: json!(45),
                    id: "dat-hp".to_owned(),
                    seq: 2,
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
    fn changeset_keeps_same_objid_in_different_dat_files_as_separate_items() {
        let data_dir = temp_data_dir("changeset-dat-disambig");
        let store = JournalStore::new(&data_dir);
        let request_id = "req-dat-disambig";

        // units #5 and weapons #5 share the family + objId but are DIFFERENT
        // edits — they must not collapse into one "Dat #5" group.
        store
            .record(
                request_id,
                entry(
                    "dat-units",
                    1,
                    WriteTool::DatSet,
                    dat_target_named(DatTable::Dat, "units", 5, "HitPoints"),
                    Snapshot::DatValue {
                        value: json!(40),
                        was_default: false,
                    },
                    Snapshot::DatValue {
                        value: json!(80),
                        was_default: false,
                    },
                ),
            )
            .expect("units entry should record");
        store
            .record(
                request_id,
                entry(
                    "dat-weapons",
                    2,
                    WriteTool::DatSet,
                    dat_target_named(DatTable::Dat, "weapons", 5, "DamageAmount"),
                    Snapshot::DatValue {
                        value: json!(6),
                        was_default: false,
                    },
                    Snapshot::DatValue {
                        value: json!(12),
                        was_default: false,
                    },
                ),
            )
            .expect("weapons entry should record");

        let changeset = store
            .changeset(request_id)
            .expect("changeset should be emitted");

        assert_eq!(changeset.items.len(), 2, "distinct dat files stay separate");
        let ids: Vec<&str> = changeset.items.iter().map(|i| i.id.as_str()).collect();
        assert!(ids.contains(&"dat:Dat:units:5"));
        assert!(ids.contains(&"dat:Dat:weapons:5"));
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
    fn legacy_workspace_journal_session_id_is_read_and_ignored() {
        // Journals written before the project-root cwd cutover carried a
        // per-session copy id on workspace targets; pending reviews from those
        // builds must still load after upgrade.
        let raw = json!({
            "request_id": "req-legacy",
            "entries": [{
                "id": "ws-write",
                "seq": 1,
                "tool": "WorkspaceWrite",
                "target": {
                    "WorkspacePath": {
                        "workspace_id": "w".repeat(64),
                        "session_id": "legacy-session",
                        "path": "specs/game.md",
                    }
                },
                "before": { "FileContent": { "content": "old" } },
                "after": { "FileContent": { "content": "new" } },
                "ts": 1,
            }],
        });
        let journal: Journal = serde_json::from_value(raw).expect("legacy journal should load");
        match &journal.entries[0].target {
            JournalTarget::WorkspacePath { path, .. } => {
                assert_eq!(path, "specs/game.md");
            }
            other => panic!("unexpected target: {other:?}"),
        }
    }

    #[test]
    fn python_file_changeset_marks_modified_and_deleted_sources() {
        let data_dir = temp_data_dir("python-file-review");
        let store = JournalStore::new(&data_dir);
        let request_id = "req-python-review";
        store
            .record(
                request_id,
                entry(
                    "python-write",
                    1,
                    WriteTool::FileWrite,
                    path_target("src/helper.py"),
                    Snapshot::FileContent {
                        content: "VALUE = 1\n".to_owned(),
                    },
                    Snapshot::FileContent {
                        content: "VALUE = 2\n".to_owned(),
                    },
                ),
            )
            .unwrap();
        store
            .record(
                request_id,
                entry(
                    "python-delete",
                    2,
                    WriteTool::FileDelete,
                    path_target("src/deleted.py"),
                    Snapshot::DeletedFile {
                        content: "DELETED = True\n".to_owned(),
                        position: None,
                    },
                    Snapshot::Deleted,
                ),
            )
            .unwrap();
        let changeset = store.changeset(request_id).unwrap();
        assert!(changeset.items.iter().any(|item| {
            item.path.as_deref() == Some("src/helper.py")
                && item.kind == ChangesetItemKind::Modified
        }));
        assert!(changeset.items.iter().any(|item| {
            item.path.as_deref() == Some("src/deleted.py")
                && item.kind == ChangesetItemKind::Deleted
        }));
        fs::remove_dir_all(data_dir).ok();
    }

    #[test]
    fn accept_archives_journal() {
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

        store
            .accept_entries(request_id, &DecisionIds::All)
            .expect("accept should archive");

        assert!(!data_dir.join("journal").join("req-accept.json").exists());
        assert!(data_dir
            .join("journal")
            .join("accepted")
            .join("req-accept.json")
            .exists());
    }
}
