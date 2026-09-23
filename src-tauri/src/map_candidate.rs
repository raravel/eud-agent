use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::DataDirs;
use crate::map_context::{MapContextService, MapContextSnapshot};
use crate::map_image::MapImageConversionMetadata;
use crate::map_import::{MapImportStore, MapStampSourceRef, ResolvedImportedStamp};
use crate::map_model::{
    hex_sha256, CandidateRevision, CandidateSession, MapEditBatch, MapEditExpected, MapLayer,
    MapMentionSnapshot, MapObjectKind, MapOperation, SelectionMask, SelectionRole,
    VerificationReport, MAP_EDIT_SCHEMA,
};
use crate::map_stamp::{
    compile_stamp_placement, MapStampToolSource, PersistentSelection, PersistentSelectionLibrary,
    StampCollisionPolicy, StampDestination, StampPlacementReport, StampPlacementResult,
};
use crate::map_verify::{MapRequestAuthority, MapVerificationService};

#[derive(Clone)]
pub struct CandidateStore {
    inner: Arc<CandidateStoreInner>,
}

struct CandidateStoreInner {
    dirs: DataDirs,
    context: MapContextService,
    verifier: MapVerificationService,
    active: Mutex<HashMap<String, ActiveRequest>>,
    selection_palette: Mutex<()>,
    /// One lock per session around every load → follow → save window, so a
    /// rebase (seconds of isom replay) cannot interleave with Apply, Undo,
    /// commit, or another rebase of the same session.
    session_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    imports: MapImportStore,
}

#[derive(Clone)]
struct ActiveRequest {
    request_id: String,
    parent_revision: u32,
    parent_hash: String,
    authority: MapRequestAuthority,
    draft_path: Option<PathBuf>,
    batches: Vec<Vec<MapOperation>>,
    reports: Vec<Value>,
    image_conversions: Vec<MapImageConversionMetadata>,
    imported_sources: BTreeMap<String, ResolvedImportedStamp>,
    imported_provenance: Vec<ImportedStampProvenance>,
    pending_revision: Option<PendingRevision>,
    finalized: bool,
}

#[derive(Clone)]
struct PendingRevision {
    revision: CandidateRevision,
    object_ids: BTreeMap<String, String>,
}

/// Every revision replayed onto a fresh copy of the saved source, keyed by
/// revision number (0 is the copied source itself).
struct RebasedChain {
    base: PathBuf,
    revisions: Vec<CandidateRevision>,
    /// Off-chain revisions the new source cannot reproduce.
    dropped: Vec<CandidateRevision>,
    outputs: BTreeMap<u32, PathBuf>,
    object_ids: BTreeMap<u32, BTreeMap<String, String>>,
}

struct StampRequestContext {
    source: PathBuf,
    draft: PathBuf,
    selection: SelectionMask,
    authority: MapRequestAuthority,
    provenance: Option<ImportedStampProvenance>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ImportedStampProvenance {
    import_id: String,
    source_file_sha256: String,
    source_chk_sha256: String,
    snapshot_hash: String,
    width: u16,
    height: u16,
    layers: BTreeSet<MapLayer>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RevisionManifest {
    schema: String,
    revision: u32,
    parent: u32,
    request_id: String,
    authority: MapRequestAuthority,
    batches: Vec<Vec<MapOperation>>,
    #[serde(default)]
    image_conversions: Vec<MapImageConversionMetadata>,
    #[serde(default)]
    imported_stamps: Vec<ImportedStampProvenance>,
    #[serde(default)]
    object_ids: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateRevisionView {
    pub revision: u32,
    pub parent: u32,
    pub request_id: String,
    pub map_sha256: String,
    pub diff: crate::map_model::MapDiff,
    pub verification: VerificationReport,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionView {
    #[serde(flatten)]
    pub selection: SelectionMask,
    pub snapshot_hash: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateStateView {
    pub session_id: String,
    pub baseline: crate::map_model::MapRevision,
    pub current_revision: u32,
    pub current_hash: String,
    pub revision_key: String,
    pub revisions: Vec<CandidateRevisionView>,
    pub selections: Vec<SelectionView>,
    pub stale: bool,
    pub source_diverged: bool,
    pub can_apply: bool,
    pub can_undo: bool,
}

/// One CHK player slot as the Map window's properties dialog submits it.
/// `type`/`race` use the native `player.set` vocabulary; `force` (0..3) is
/// required for slots 0..7 and must be absent for slots 8..11.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapPropertyPlayer {
    pub r#type: String,
    pub race: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapPropertyForce {
    pub name: String,
    pub allied: bool,
    pub allied_victory: bool,
    pub shared_vision: bool,
    pub random_start: bool,
}

/// The complete scenario-property set (title, description, exactly 12 slots,
/// exactly 4 forces). Only fields that differ from the source map's current
/// digest become `eud-map-edit/1` operations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapPropertiesInput {
    pub title: String,
    pub description: String,
    pub players: Vec<MapPropertyPlayer>,
    pub forces: Vec<MapPropertyForce>,
}

/// A verified properties work map, ready for `MapSafe::apply`.
#[derive(Debug, Clone)]
pub struct PropertiesStage {
    pub work: PathBuf,
    pub work_sha256: String,
    pub verification: VerificationReport,
    pub operations: usize,
}

const PROPERTIES_WORK_FILE: &str = "properties-work.tmp.scx";

/// `player.set` type vocabulary → raw OWNR byte (`Sc::Player::SlotType`).
const SLOT_TYPES: [(&str, u8); 6] = [
    ("human", 6),
    ("computer", 5),
    ("rescuable", 3),
    ("neutral", 7),
    ("inactive", 0),
    ("closed", 8),
];

/// Legacy OWNR bytes the game treats like a vocabulary entry: 1 (game
/// computer) is a computer slot, 2 (occupied human) a human slot, 4 (unused)
/// an inactive one. A form that shows them under that entry has not changed
/// the slot, so saving leaves the original byte alone.
fn slot_type_class(id: u8) -> u8 {
    match id {
        1 => 5,
        2 => 6,
        4 => 0,
        other => other,
    }
}

/// `player.set` race vocabulary → raw SIDE byte (`Chk::Race`).
const RACES: [(&str, u8); 8] = [
    ("zerg", 0),
    ("terran", 1),
    ("protoss", 2),
    ("independent", 3),
    ("userSelectable", 5),
    ("random", 6),
    ("neutral", 4),
    ("inactive", 7),
];

impl CandidateStore {
    pub fn new(dirs: DataDirs, imports: MapImportStore) -> Self {
        Self {
            inner: Arc::new(CandidateStoreInner {
                context: MapContextService::new(dirs.clone()),
                dirs,
                verifier: MapVerificationService,
                active: Mutex::new(HashMap::new()),
                selection_palette: Mutex::new(()),
                session_locks: Mutex::new(HashMap::new()),
                imports,
            }),
        }
    }

    pub fn persistent_selections(
        &self,
        project_id: &str,
    ) -> Result<Vec<PersistentSelection>, String> {
        validate_component(project_id, "project id")?;
        let _palette = self.inner.selection_palette.lock();
        let library = self.read_selection_library(project_id)?;
        for (key, selection) in &library.selections {
            if key != &selection.id {
                return Err(
                    "map selection palette key does not match its persistent selection id"
                        .to_string(),
                );
            }
        }
        Ok(library.selections.into_values().collect())
    }
    pub fn cleanup_startup(&self) -> Result<usize, String> {
        const UNUSED_MAX_AGE: std::time::Duration =
            std::time::Duration::from_secs(30 * 24 * 60 * 60);
        let root = self.inner.dirs.map_candidates_dir();
        std::fs::create_dir_all(&root)
            .map_err(|error| format!("candidate cache root could not be created: {error}"))?;
        let mut removed = 0;
        for project in std::fs::read_dir(&root)
            .map_err(|error| format!("candidate projects could not be inspected: {error}"))?
        {
            let project = project.map_err(|error| error.to_string())?;
            if !project
                .file_type()
                .map_err(|error| error.to_string())?
                .is_dir()
            {
                continue;
            }
            for session in std::fs::read_dir(project.path())
                .map_err(|error| format!("candidate sessions could not be inspected: {error}"))?
            {
                let session = session.map_err(|error| error.to_string())?;
                if !session
                    .file_type()
                    .map_err(|error| error.to_string())?
                    .is_dir()
                {
                    continue;
                }
                let session_root = session.path();
                let state_path = session_root.join("state.json");
                if !state_path.is_file() {
                    std::fs::remove_dir_all(&session_root).map_err(|error| {
                        format!("incomplete candidate cache could not be removed: {error}")
                    })?;
                    removed += 1;
                    continue;
                }
                if let Ok(state) = self.load_state_path(&state_path) {
                    let old = state_path
                        .metadata()
                        .and_then(|metadata| metadata.modified())
                        .ok()
                        .and_then(|modified| {
                            std::time::SystemTime::now().duration_since(modified).ok()
                        })
                        .is_some_and(|age| age > UNUSED_MAX_AGE);
                    if old && state.current_revision == 0 && state.last_apply_backup.is_none() {
                        std::fs::remove_dir_all(&session_root).map_err(|error| {
                            format!("unused candidate cache could not be removed: {error}")
                        })?;
                        removed += 1;
                        continue;
                    }
                }
                let drafts = session_root.join("drafts");
                std::fs::create_dir_all(&drafts)
                    .map_err(|error| format!("candidate drafts could not be created: {error}"))?;
                cleanup_drafts(&drafts)?;
                for entry in std::fs::read_dir(&session_root).map_err(|error| {
                    format!("candidate temporaries could not be inspected: {error}")
                })? {
                    let entry = entry.map_err(|error| error.to_string())?;
                    let file_type = entry.file_type().map_err(|error| error.to_string())?;
                    let name = entry.file_name().to_string_lossy().to_string();
                    if file_type.is_dir() && name.starts_with("rebase-") {
                        std::fs::remove_dir_all(entry.path()).map_err(|error| {
                            format!("interrupted candidate rebase could not be removed: {error}")
                        })?;
                        removed += 1;
                        continue;
                    }
                    if !file_type.is_file() {
                        continue;
                    }
                    if name.contains(".tmp.") || name.ends_with(".tmp") {
                        std::fs::remove_file(entry.path()).map_err(|error| {
                            format!("candidate temporary could not be removed: {error}")
                        })?;
                        removed += 1;
                    }
                }
            }
        }
        Ok(removed)
    }

    pub fn context(&self) -> &MapContextService {
        &self.inner.context
    }
    pub(crate) fn session_source(
        &self,
        project_id: &str,
        session_id: &str,
    ) -> Result<Option<PathBuf>, String> {
        validate_component(project_id, "project id")?;
        validate_component(session_id, "map session id")?;
        let state_path = self.session_root(project_id, session_id).join("state.json");
        if !candidate_state_exists(&state_path)? {
            return Ok(None);
        }
        Ok(Some(
            self.load_state_path(&state_path)?.baseline.source_path,
        ))
    }

    /// Whether the session's last Apply is still undoable against the saved
    /// source whose hash is `source_sha256`: what `canUndo` becomes once the
    /// session follows that source, without replaying anything.
    pub(crate) fn apply_undoable_against(
        &self,
        project_id: &str,
        session_id: &str,
        source_sha256: &str,
    ) -> Result<bool, String> {
        validate_component(project_id, "project id")?;
        validate_component(session_id, "map session id")?;
        let state_path = self.session_root(project_id, session_id).join("state.json");
        if !candidate_state_exists(&state_path)? {
            return Ok(false);
        }
        let state = self.load_state_path(&state_path)?;
        Ok(state.last_apply_backup.is_some()
            && state.last_apply_source_hash.as_deref() == Some(source_sha256))
    }

    pub fn create_session(
        &self,
        session_id: &str,
        context: &MapContextSnapshot,
    ) -> Result<CandidateStateView, String> {
        validate_component(session_id, "map session id")?;
        validate_component(&context.revision.project_id, "project id")?;
        let root = self.session_root(&context.revision.project_id, session_id);
        let state_path = root.join("state.json");
        if candidate_state_exists(&state_path)? {
            return Err("candidate session already exists".to_string());
        }
        std::fs::create_dir_all(root.join("revisions")).map_err(|error| {
            format!("candidate revision directory could not be created: {error}")
        })?;
        std::fs::create_dir_all(root.join("drafts"))
            .map_err(|error| format!("candidate draft directory could not be created: {error}"))?;
        let baseline = root.join("baseline.scx");
        let current = root.join("current.scx");
        copy_atomic(&context.revision.source_path, &baseline)?;
        copy_atomic(&context.revision.source_path, &current)?;
        let mut state = CandidateSession {
            session_id: session_id.to_string(),
            baseline: context.revision.clone(),
            baseline_snapshot: baseline,
            current_revision: 0,
            current_map: current,
            revisions: Vec::new(),
            selections: BTreeMap::new(),
            persistent_protections: Default::default(),
            candidate_object_ids: BTreeMap::new(),
            stale: false,
            source_diverged: false,
            last_apply_backup: None,
            last_apply_source_hash: None,
            last_apply_before_hash: None,
        };
        self.sync_selection_palette(&mut state)?;
        self.save_state(&state)?;
        self.view(&state)
    }

    pub fn open_session(
        &self,
        session_id: &str,
        context: &MapContextSnapshot,
    ) -> Result<CandidateStateView, String> {
        validate_component(session_id, "map session id")?;
        validate_component(&context.revision.project_id, "project id")?;
        let root = self.session_root(&context.revision.project_id, session_id);
        let state_path = root.join("state.json");
        if !candidate_state_exists(&state_path)? {
            return Err("candidate session does not exist".to_string());
        }
        let lock = self.session_lock(session_id);
        let _guard = lock.lock();
        let mut state = self.load_state_path(&state_path)?;
        if state.session_id != session_id
            || state.baseline.project_id != context.revision.project_id
            || state.baseline.source_path != context.revision.source_path
        {
            return Err(
                "candidate session belongs to a different project or source map".to_string(),
            );
        }
        if !state.baseline_snapshot.is_file() || !state.current_map.is_file() {
            return Err(
                "candidate session is incomplete; baseline/current map is missing".to_string(),
            );
        }
        let current_hash = file_hash(&state.current_map)?;
        let expected_hash = state
            .revisions
            .iter()
            .find(|revision| revision.revision == state.current_revision)
            .map(|revision| revision.map_sha256.clone())
            .unwrap_or_else(|| state.baseline.file_sha256.clone());
        if current_hash != expected_hash {
            self.replay_into(&state, state.current_revision, &state.current_map)?;
            let replayed_hash = file_hash(&state.current_map)?;
            if let Some(revision) = state
                .revisions
                .iter_mut()
                .find(|revision| revision.revision == state.current_revision)
            {
                revision.map_sha256 = replayed_hash;
            }
        }
        self.follow_source(&mut state)?;
        self.save_state(&state)?;
        self.view(&state)
    }

    pub fn state(&self, project_id: &str, session_id: &str) -> Result<CandidateStateView, String> {
        let lock = self.session_lock(session_id);
        let _guard = lock.lock();
        let mut state = self.load_state(project_id, session_id)?;
        if self.follow_source(&mut state)? {
            self.save_state(&state)?;
        }
        self.view(&state)
    }

    /// The persisted view without following the source: `stale` reports a
    /// changed source, nothing is replayed or written. Startup recovery uses
    /// it so a project is never rebased before it is explicitly opened.
    pub fn peek_state(
        &self,
        project_id: &str,
        session_id: &str,
    ) -> Result<CandidateStateView, String> {
        let lock = self.session_lock(session_id);
        let _guard = lock.lock();
        let mut state = self.load_state(project_id, session_id)?;
        state.stale =
            state.stale || file_hash(&state.baseline.source_path)? != state.baseline.file_sha256;
        self.view(&state)
    }

    fn session_lock(&self, session_id: &str) -> Arc<Mutex<()>> {
        self.inner
            .session_locks
            .lock()
            .entry(session_id.to_string())
            .or_default()
            .clone()
    }

    /// The saved source map is the authority a session follows, never one it
    /// owns. Whenever the bytes on disk differ from the recorded baseline, the
    /// session re-reads the source and replays every candidate revision onto
    /// it with fresh verification, so a save from another session, the
    /// properties dialog, an Undo, or SCMDraft simply becomes the new base.
    /// When a revision on the visible chain cannot be replayed (the map
    /// changed shape, an operation's expected-before no longer holds, or the
    /// replay fails verification) the candidate wins: it keeps descending from
    /// the old snapshot, `baseline.file_sha256` names the source bytes Apply
    /// will overwrite, and `source_diverged` tells the panel. Only a session
    /// with an active request defers, marking `stale` until the request
    /// settles. Returns whether `state` changed; the caller persists it.
    fn follow_source(&self, state: &mut CandidateSession) -> Result<bool, String> {
        self.follow_source_with(state, false)
    }

    /// `retry_diverged` re-attempts the replay of a diverged session whose
    /// visible chain just changed (revert), where the source itself did not.
    fn follow_source_with(
        &self,
        state: &mut CandidateSession,
        retry_diverged: bool,
    ) -> Result<bool, String> {
        let disk_hash = file_hash(&state.baseline.source_path)?;
        if disk_hash == state.baseline.file_sha256 && !(retry_diverged && state.source_diverged) {
            let changed = state.stale;
            state.stale = false;
            return Ok(changed);
        }
        if self.inner.active.lock().contains_key(&state.session_id) {
            let changed = !state.stale;
            state.stale = true;
            return Ok(changed);
        }
        let root = self
            .session_root(&state.baseline.project_id, &state.session_id)
            .join(format!("rebase-{}", uuid::Uuid::new_v4()));
        let rebase = (|| {
            std::fs::create_dir_all(&root).map_err(|error| {
                format!("candidate rebase directory could not be created: {error}")
            })?;
            // Identity comes from the copied bytes, never from a second read
            // of a source another writer may still be saving.
            let base = root.join("r0000.scx");
            copy_atomic(&state.baseline.source_path, &base)?;
            let mut revision = self
                .inner
                .context
                .revision_for_path(state.baseline.project_id.clone(), &base)?;
            revision.source_path = state.baseline.source_path.clone();
            revision.mtime_ns = source_mtime_ns(&state.baseline.source_path)?;
            let rebased = self.rebase_revisions(state, &revision, &base, &root)?;
            Ok::<_, String>((revision, rebased))
        })();
        let outcome = match rebase {
            Ok((revision, Some(rebased))) => self.promote_rebase(state, revision, rebased),
            Ok((revision, None)) => {
                // Candidate wins: keep the snapshot chain, adopt the source identity.
                state.baseline.file_sha256 = revision.file_sha256;
                state.baseline.chk_sha256 = revision.chk_sha256;
                state.baseline.mtime_ns = revision.mtime_ns;
                state.source_diverged = true;
                state.forget_last_apply();
                Ok(())
            }
            Err(error) => Err(error),
        };
        let _ = std::fs::remove_dir_all(&root);
        outcome?;
        state.stale = false;
        Ok(true)
    }

    /// Replay every revision (parents first) onto a copy of the saved source
    /// and re-verify it. `None` means a revision on the visible chain could
    /// not be reproduced; off-chain revisions that fail are dropped.
    fn rebase_revisions(
        &self,
        state: &CandidateSession,
        source: &crate::map_model::MapRevision,
        base: &Path,
        root: &Path,
    ) -> Result<Option<RebasedChain>, String> {
        let chain = revision_chain(state, state.current_revision)?;
        let mut outputs: BTreeMap<u32, PathBuf> = BTreeMap::from([(0, base.to_path_buf())]);
        let mut object_ids: BTreeMap<u32, BTreeMap<String, String>> =
            BTreeMap::from([(0, BTreeMap::new())]);
        let mut dropped = Vec::new();
        if source.width != state.baseline.width
            || source.height != state.baseline.height
            || source.tileset != state.baseline.tileset
        {
            // A reshaped map reproduces no revision at all.
            return Ok(chain.is_empty().then(|| RebasedChain {
                base: base.to_path_buf(),
                revisions: Vec::new(),
                dropped: state.revisions.clone(),
                outputs,
                object_ids,
            }));
        }
        let starcraft_path = self.inner.context.starcraft_path()?;
        let mut revisions = state.revisions.clone();
        revisions.sort_by_key(|revision| revision.revision);
        let mut rebased = Vec::new();
        for mut revision in revisions {
            let Some(parent) = outputs.get(&revision.parent).cloned() else {
                if chain.contains(&revision.revision) {
                    return Ok(None);
                }
                dropped.push(revision);
                continue;
            };
            let manifest: RevisionManifest = read_json(&revision.operation_manifest)?;
            let output = root.join(format!("r{:04}.scx", revision.revision));
            let replayed = self.replay_batches(
                state,
                &parent,
                &output,
                &manifest.batches,
                &starcraft_path,
                root,
            )?;
            let verification = replayed.then(|| {
                self.inner.verifier.verify(
                    &parent,
                    &output,
                    &manifest.authority,
                    &starcraft_path,
                    None,
                )
            });
            let Some(verification) = verification.filter(|report| report.valid) else {
                if chain.contains(&revision.revision) {
                    return Ok(None);
                }
                dropped.push(revision);
                continue;
            };
            let parent_ids = object_ids
                .get(&revision.parent)
                .cloned()
                .unwrap_or_default();
            let ids = update_candidate_object_ids(&parent_ids, &parent, &output)?;
            revision.map_sha256 = file_hash(&output)?;
            revision.diff = verification.diff.clone();
            revision.verification = verification;
            outputs.insert(revision.revision, output);
            object_ids.insert(revision.revision, ids);
            rebased.push(revision);
        }
        Ok(Some(RebasedChain {
            base: base.to_path_buf(),
            revisions: rebased,
            dropped,
            outputs,
            object_ids,
        }))
    }

    /// Replay one revision's batches from `parent` into `output`. A false
    /// result is an operation the new base rejects, not an I/O failure.
    fn replay_batches(
        &self,
        state: &CandidateSession,
        parent: &Path,
        output: &Path,
        batches: &[Vec<MapOperation>],
        starcraft_path: &Path,
        root: &Path,
    ) -> Result<bool, String> {
        let work = root.join("replay.tmp.scx");
        let next = root.join("replay.next.scx");
        copy_atomic(parent, &work)?;
        for operations in batches {
            let batch = MapEditBatch {
                schema: MAP_EDIT_SCHEMA.to_string(),
                expected: MapEditExpected {
                    input_file_sha256: file_hash(&work)?,
                    tileset: state.baseline.tileset,
                    width: state.baseline.width,
                    height: state.baseline.height,
                },
                operations: operations.clone(),
            };
            let bytes = serde_json::to_vec(&batch)
                .map_err(|error| format!("rebase batch could not be serialized: {error}"))?;
            remove_if_exists(&next)?;
            // The engine reports an operation the map rejects (expected-before
            // conflict, missing object, failed invariant) as `Engine` with a
            // report; everything else is an environment failure to surface.
            match isom::mapedit(&work, &next, starcraft_path, &bytes) {
                Ok(_) => {}
                Err(error) if error.status == isom::IsomError::Engine => {
                    remove_if_exists(&work)?;
                    remove_if_exists(&next)?;
                    return Ok(false);
                }
                Err(error) => return Err(format!("candidate rebase replay failed: {error}")),
            }
            copy_atomic(&next, &work)?;
            remove_if_exists(&next)?;
        }
        copy_atomic(&work, output)?;
        remove_if_exists(&work)?;
        Ok(true)
    }

    fn promote_rebase(
        &self,
        state: &mut CandidateSession,
        source: crate::map_model::MapRevision,
        rebased: RebasedChain,
    ) -> Result<(), String> {
        let current = rebased
            .outputs
            .get(&state.current_revision)
            .ok_or_else(|| "rebased candidate lost its visible revision".to_string())?;
        copy_atomic(&rebased.base, &state.baseline_snapshot)?;
        copy_atomic(current, &state.current_map)?;
        for revision in &rebased.revisions {
            let mut manifest: RevisionManifest = read_json(&revision.operation_manifest)?;
            manifest.object_ids = rebased
                .object_ids
                .get(&revision.revision)
                .cloned()
                .unwrap_or_default();
            write_json_atomic(&revision.operation_manifest, &manifest)?;
        }
        for revision in &rebased.dropped {
            remove_if_exists(&revision.operation_manifest)?;
        }
        if state.baseline.file_sha256 != source.file_sha256 {
            state.forget_last_apply();
        }
        state.baseline = source;
        state.revisions = rebased.revisions;
        state.candidate_object_ids = rebased
            .object_ids
            .get(&state.current_revision)
            .cloned()
            .unwrap_or_default();
        state.selections.clear();
        state.persistent_protections.clear();
        state.source_diverged = false;
        self.sync_selection_palette(state)?;
        Ok(())
    }

    pub fn save_selection(
        &self,
        project_id: &str,
        session_id: &str,
        selection: SelectionMask,
    ) -> Result<CandidateStateView, String> {
        let lock = self.session_lock(session_id);
        let _guard = lock.lock();
        let mut state = self.load_state(project_id, session_id)?;
        if selection.source_revision
            != revision_key(state.current_revision, &file_hash(&state.current_map)?)
        {
            return Err(
                "selection source revision does not match the visible candidate".to_string(),
            );
        }
        let canonical = SelectionMask::canonical(
            selection.id.clone(),
            selection.label.clone(),
            selection.source_revision.clone(),
            selection.role,
            selection.layers.clone(),
            crate::map_model::MaskGrid {
                width: state.baseline.width,
                height: state.baseline.height,
                rows: selection.rows.clone(),
            },
        )?;
        if canonical != selection {
            return Err("selection rows, bounds, and selectedCells are not canonical".to_string());
        }
        let _palette = self.inner.selection_palette.lock();
        let mut library = self.read_selection_library(project_id)?;
        let previous_library = library.clone();
        library.selections.insert(
            selection.id.clone(),
            PersistentSelection::from_selection(&canonical),
        );
        self.write_selection_library(project_id, &library)?;
        if selection.role == SelectionRole::Protect {
            state.persistent_protections.insert(selection.id.clone());
        } else {
            state.persistent_protections.remove(&selection.id);
        }
        state.selections.insert(selection.id.clone(), canonical);
        if let Err(error) = self.save_state(&state) {
            let _ = self.write_selection_library(project_id, &previous_library);
            return Err(error);
        }
        self.view(&state)
    }

    pub fn delete_selection(
        &self,
        project_id: &str,
        session_id: &str,
        selection_id: &str,
    ) -> Result<CandidateStateView, String> {
        let lock = self.session_lock(session_id);
        let _guard = lock.lock();
        let mut state = self.load_state(project_id, session_id)?;
        let _palette = self.inner.selection_palette.lock();
        let mut library = self.read_selection_library(project_id)?;
        let previous_library = library.clone();
        library.selections.remove(selection_id);
        self.write_selection_library(project_id, &library)?;
        state.selections.remove(selection_id);
        state.persistent_protections.remove(selection_id);
        if let Err(error) = self.save_state(&state) {
            let _ = self.write_selection_library(project_id, &previous_library);
            return Err(error);
        }
        self.view(&state)
    }

    pub fn prepare_request(
        &self,
        project_id: &str,
        session_id: &str,
        request_id: &str,
        parent_revision: u32,
        mentions: &[MapMentionSnapshot],
    ) -> Result<MapRequestAuthority, String> {
        validate_component(request_id, "request id")?;
        let lock = self.session_lock(session_id);
        let _guard = lock.lock();
        let mut state = self.load_state(project_id, session_id)?;
        if self.follow_source(&mut state)? {
            self.save_state(&state)?;
        }
        if state.current_revision != parent_revision {
            return Err(format!(
                "candidate revision conflict: requested r{parent_revision}, visible r{}",
                state.current_revision
            ));
        }
        let current_hash = file_hash(&state.current_map)?;
        let expected_revision = revision_key(state.current_revision, &current_hash);
        let mut targets = Vec::new();
        let mut forbidden = Vec::new();
        let mut imported_sources: BTreeMap<String, ResolvedImportedStamp> = BTreeMap::new();
        for mention in mentions {
            match mention {
                MapMentionSnapshot::Region {
                    selection_id,
                    snapshot_hash,
                    source_revision,
                } => {
                    let selection = state.selections.get(selection_id).ok_or_else(|| {
                        format!("selection mention '{selection_id}' no longer exists")
                    })?;
                    if selection.snapshot_hash() != *snapshot_hash {
                        return Err(format!(
                            "selection mention '{selection_id}' snapshot is stale"
                        ));
                    }
                    if source_revision != &expected_revision
                        || selection.source_revision != expected_revision
                    {
                        return Err(format!(
                            "selection mention '{selection_id}' belongs to another revision"
                        ));
                    }
                    match selection.role {
                        SelectionRole::Target => targets.push(selection.clone()),
                        SelectionRole::Protect => forbidden.push(selection.clone()),
                        SelectionRole::Reference | SelectionRole::Anchor => {}
                    }
                }
                MapMentionSnapshot::Object { object_ref, .. } => {
                    if object_ref.revision_key != expected_revision
                        || object_ref.baseline_hash != state.baseline.file_sha256
                    {
                        return Err(
                            "object instance mention belongs to another candidate map revision"
                                .to_string(),
                        );
                    }
                    let expected_id = state
                        .candidate_object_ids
                        .get(&object_ref_key(object_ref))
                        .map(String::as_str);
                    if expected_id != object_ref.candidate_id.as_deref() {
                        return Err("candidate object UUID is stale or ambiguous".to_string());
                    }
                    if let Some(type_id) = validate_object_ref(&state.current_map, object_ref)? {
                        let chk = isom::chk_extract(&state.current_map).map_err(|error| {
                            format!("object kind could not be verified: {error}")
                        })?;
                        let digest = crate::chk::digest_chk(&chk);
                        let buildings = crate::tool_exec::map_building_ids(
                            &self.inner.context.starcraft_path()?,
                            &digest.map.tileset,
                        )?;
                        let actual_building = buildings.contains(&type_id);
                        if (object_ref.kind == MapObjectKind::Building) != actual_building {
                            return Err("object instance mention kind does not match units.dat classification".to_string());
                        }
                    }
                }
                MapMentionSnapshot::Palette { entry, qualifiers } => {
                    if entry.tileset != state.baseline.tileset {
                        return Err("palette mention belongs to another tileset".to_string());
                    }
                    self.validate_palette_entry(&state, entry)?;
                    if entry.kind == crate::map_model::PaletteKind::NewLocation {
                        self.validate_new_location_qualifiers(
                            &state,
                            qualifiers,
                            &expected_revision,
                        )?;
                    }
                }
                MapMentionSnapshot::Stamp {
                    selection_id,
                    snapshot_hash,
                } => {
                    let selection = state.selections.get(selection_id).ok_or_else(|| {
                        format!("stamp mention '{selection_id}' no longer exists")
                    })?;
                    if selection.snapshot_hash() != *snapshot_hash {
                        return Err(format!("stamp mention '{selection_id}' snapshot is stale"));
                    }
                }
                MapMentionSnapshot::ImportedStamp {
                    import_id,
                    snapshot_hash,
                } => {
                    let resolved = self.inner.imports.resolve_imported(
                        project_id,
                        import_id,
                        snapshot_hash,
                        state.baseline.tileset,
                    )?;
                    if let Some(existing) = imported_sources.get(import_id) {
                        if existing.stamp.snapshot_hash != resolved.stamp.snapshot_hash {
                            return Err(format!(
                                "imported stamp '{import_id}' has conflicting request snapshots"
                            ));
                        }
                    } else {
                        imported_sources.insert(import_id.clone(), resolved);
                    }
                }
                MapMentionSnapshot::Location {
                    location_id,
                    revision_key,
                    baseline_hash,
                } => {
                    if revision_key != &expected_revision
                        || baseline_hash != &state.baseline.file_sha256
                    {
                        return Err("location mention belongs to another candidate map revision"
                            .to_string());
                    }
                    let chk = isom::chk_extract(&state.current_map).map_err(|error| {
                        format!("candidate location mention could not be resolved: {error}")
                    })?;
                    if !crate::chk::digest_chk(&chk)
                        .locations
                        .iter()
                        .any(|location| location.id == usize::from(*location_id))
                    {
                        return Err(format!("location #{location_id} no longer exists"));
                    }
                }
            }
        }
        for selection_id in &state.persistent_protections {
            let selection = state.selections.get(selection_id).ok_or_else(|| {
                "persistent protection references a missing selection".to_string()
            })?;
            if !forbidden.iter().any(|existing| existing.id == selection.id) {
                forbidden.push(selection.clone());
            }
        }
        let authority = MapRequestAuthority::calculate(
            session_id.to_string(),
            request_id.to_string(),
            parent_revision,
            state.baseline.width,
            state.baseline.height,
            targets,
            forbidden,
        )?;
        let mut requests = self.inner.active.lock();
        if requests.contains_key(session_id) {
            return Err("another map request is already active for this session".to_string());
        }
        for source in imported_sources.values() {
            self.inner
                .imports
                .bind_blob(&source.stamp.source_file_sha256);
        }
        let active = ActiveRequest {
            request_id: request_id.to_string(),
            parent_revision,
            parent_hash: current_hash,
            authority: authority.clone(),
            draft_path: None,
            batches: Vec::new(),
            reports: Vec::new(),
            image_conversions: Vec::new(),
            imported_sources,
            imported_provenance: Vec::new(),
            pending_revision: None,
            finalized: false,
        };
        requests.insert(session_id.to_string(), active);
        Ok(authority)
    }

    pub fn direct_terrain_authority(
        &self,
        project_id: &str,
        session_id: &str,
        request_id: &str,
        expected_revision_key: &str,
    ) -> Result<MapRequestAuthority, String> {
        let lock = self.session_lock(session_id);
        let _guard = lock.lock();
        let mut state = self.load_state(project_id, session_id)?;
        if self.follow_source(&mut state)? {
            self.save_state(&state)?;
        }
        let current_hash = file_hash(&state.current_map)?;
        if revision_key(state.current_revision, &current_hash) != expected_revision_key {
            return Err("direct placement candidate revision is stale".to_string());
        }
        let mut protections = Vec::new();
        for selection_id in &state.persistent_protections {
            let selection = state.selections.get(selection_id).ok_or_else(|| {
                "persistent protection references a missing selection".to_string()
            })?;
            protections.push(selection.clone());
        }
        MapRequestAuthority::calculate(
            session_id.to_string(),
            request_id.to_string(),
            state.current_revision,
            state.baseline.width,
            state.baseline.height,
            Vec::new(),
            protections,
        )
    }

    pub fn image_request_context(
        &self,
        project_id: &str,
        session_id: &str,
        request_id: &str,
    ) -> Result<(MapRequestAuthority, crate::map_model::MapRevision, PathBuf), String> {
        let state = self.load_state(project_id, session_id)?;
        let active = self.inner.active.lock();
        let request = active_request(&active, session_id, request_id)?;
        let draft = request
            .draft_path
            .clone()
            .ok_or_else(|| "call map_draft_begin before map_image_place".to_string())?;
        Ok((request.authority.clone(), state.baseline, draft))
    }

    pub fn draft_begin(
        &self,
        project_id: &str,
        session_id: &str,
        request_id: &str,
    ) -> Result<Value, String> {
        let state = self.load_state(project_id, session_id)?;
        let mut active = self.inner.active.lock();
        let request = active_request_mut(&mut active, session_id, request_id)?;
        if request.finalized {
            return Err("request already finalized one visible candidate revision".to_string());
        }
        if request.draft_path.is_some() {
            return Err("request draft already exists".to_string());
        }
        if state.current_revision != request.parent_revision
            || file_hash(&state.current_map)? != request.parent_hash
        {
            return Err("candidate parent changed after request preparation".to_string());
        }
        let draft = self
            .session_root(project_id, session_id)
            .join("drafts")
            .join(format!("{request_id}.tmp.scx"));
        copy_atomic(&state.current_map, &draft)?;
        request.draft_path = Some(draft);
        Ok(json!({
            "ok": true,
            "requestId": request_id,
            "parentRevision": request.parent_revision,
            "parentHash": request.parent_hash,
        }))
    }

    pub fn normalize_stamp_tool_source(
        &self,
        project_id: &str,
        session_id: &str,
        request_id: &str,
        source: &MapStampToolSource,
    ) -> Result<MapStampSourceRef, String> {
        match source {
            MapStampToolSource::CandidateSelection { selection_id } => {
                let state = self.load_state(project_id, session_id)?;
                let selection = state
                    .selections
                    .get(selection_id)
                    .ok_or_else(|| "stamp selection does not exist".to_string())?;
                Ok(MapStampSourceRef::CandidateSelection {
                    selection_id: selection_id.clone(),
                    snapshot_hash: selection.snapshot_hash(),
                })
            }
            MapStampToolSource::Imported { import_id } => {
                let active = self.inner.active.lock();
                let request = active_request(&active, session_id, request_id)?;
                let source = request.imported_sources.get(import_id).ok_or_else(|| {
                    format!(
                        "imported stamp '{import_id}' is not authorized by a current-request mention"
                    )
                })?;
                Ok(MapStampSourceRef::Imported {
                    import_id: import_id.clone(),
                    snapshot_hash: source.stamp.snapshot_hash.clone(),
                })
            }
        }
    }

    pub fn compact_imported_mention(
        &self,
        project_id: &str,
        import_id: &str,
        snapshot_hash: &str,
        destination_tileset: crate::map_model::Tileset,
    ) -> Result<Value, String> {
        self.inner.imports.compact_projection(
            project_id,
            import_id,
            snapshot_hash,
            destination_tileset,
        )
    }

    pub fn direct_stamp_preview(
        &self,
        project_id: &str,
        session_id: &str,
        expected_revision_key: &str,
        source_ref: &MapStampSourceRef,
        destinations: &[StampDestination],
    ) -> Result<StampPlacementReport, String> {
        let authority = self.direct_terrain_authority(
            project_id,
            session_id,
            "direct-stamp-preview",
            expected_revision_key,
        )?;
        let state = self.load_state(project_id, session_id)?;
        let (source, selection) = match source_ref {
            MapStampSourceRef::CandidateSelection {
                selection_id,
                snapshot_hash,
            } => {
                let selection = state
                    .selections
                    .get(selection_id)
                    .cloned()
                    .ok_or_else(|| "stamp selection does not exist".to_string())?;
                if selection.snapshot_hash() != *snapshot_hash {
                    return Err("stamp selection snapshot is stale".to_string());
                }
                (state.current_map.clone(), selection)
            }
            MapStampSourceRef::Imported {
                import_id,
                snapshot_hash,
            } => {
                let resolved = self.inner.imports.resolve_imported(
                    project_id,
                    import_id,
                    snapshot_hash,
                    state.baseline.tileset,
                )?;
                (resolved.blob_path, resolved.stamp.selection()?)
            }
        };
        Ok(compile_stamp_placement(
            &source,
            &state.current_map,
            &self.inner.context.starcraft_path()?,
            &selection,
            destinations,
            None,
            &authority,
        )?
        .report)
    }

    pub fn draft_stamp_preview(
        &self,
        project_id: &str,
        session_id: &str,
        request_id: &str,
        source_ref: &MapStampSourceRef,
        destinations: &[StampDestination],
    ) -> Result<StampPlacementReport, String> {
        let context = self.stamp_request_context(project_id, session_id, request_id, source_ref)?;
        Ok(compile_stamp_placement(
            &context.source,
            &context.draft,
            &self.inner.context.starcraft_path()?,
            &context.selection,
            destinations,
            None,
            &context.authority,
        )?
        .report)
    }

    pub fn draft_stamp_place(
        &self,
        project_id: &str,
        session_id: &str,
        request_id: &str,
        source_ref: &MapStampSourceRef,
        destinations: &[StampDestination],
        policy: StampCollisionPolicy,
    ) -> Result<StampPlacementResult, String> {
        let context = self.stamp_request_context(project_id, session_id, request_id, source_ref)?;
        let compiled = compile_stamp_placement(
            &context.source,
            &context.draft,
            &self.inner.context.starcraft_path()?,
            &context.selection,
            destinations,
            Some(policy),
            &context.authority,
        )?;
        let patch = self.draft_patch(project_id, session_id, request_id, compiled.operations)?;
        if let Some(provenance) = context.provenance {
            let mut active = self.inner.active.lock();
            let request = active_request_mut(&mut active, session_id, request_id)?;
            if !request.imported_provenance.contains(&provenance) {
                request.imported_provenance.push(provenance);
            }
        }
        Ok(StampPlacementResult {
            report: compiled.report,
            patch,
        })
    }

    fn stamp_request_context(
        &self,
        project_id: &str,
        session_id: &str,
        request_id: &str,
        source_ref: &MapStampSourceRef,
    ) -> Result<StampRequestContext, String> {
        let state = self.load_state(project_id, session_id)?;
        let active = self.inner.active.lock();
        let request = active_request(&active, session_id, request_id)?;
        let draft = request
            .draft_path
            .clone()
            .ok_or_else(|| "call map_draft_begin before placing a stamp".to_string())?;
        let (source, selection, provenance) = match source_ref {
            MapStampSourceRef::CandidateSelection {
                selection_id,
                snapshot_hash,
            } => {
                let selection = state
                    .selections
                    .get(selection_id)
                    .cloned()
                    .ok_or_else(|| "stamp selection does not exist".to_string())?;
                if selection.snapshot_hash() != *snapshot_hash {
                    return Err("stamp selection snapshot is stale".to_string());
                }
                (state.current_map, selection, None)
            }
            MapStampSourceRef::Imported {
                import_id,
                snapshot_hash,
            } => {
                let resolved = request.imported_sources.get(import_id).ok_or_else(|| {
                    format!(
                        "imported stamp '{import_id}' is not authorized by a current-request mention"
                    )
                })?;
                if resolved.stamp.snapshot_hash != *snapshot_hash {
                    return Err(format!("imported stamp '{import_id}' snapshot is stale"));
                }
                self.inner.imports.validate_resolved(resolved)?;
                let selection = resolved.stamp.selection()?;
                let provenance = ImportedStampProvenance {
                    import_id: resolved.stamp.id.clone(),
                    source_file_sha256: resolved.stamp.source_file_sha256.clone(),
                    source_chk_sha256: resolved.stamp.source_chk_sha256.clone(),
                    snapshot_hash: resolved.stamp.snapshot_hash.clone(),
                    width: resolved.stamp.bounds.right - resolved.stamp.bounds.left,
                    height: resolved.stamp.bounds.bottom - resolved.stamp.bounds.top,
                    layers: resolved.stamp.layers.clone(),
                };
                (resolved.blob_path.clone(), selection, Some(provenance))
            }
        };
        Ok(StampRequestContext {
            source,
            draft,
            selection,
            authority: request.authority.clone(),
            provenance,
        })
    }

    pub fn draft_patch(
        &self,
        project_id: &str,
        session_id: &str,
        request_id: &str,
        operations: Vec<MapOperation>,
    ) -> Result<Value, String> {
        self.draft_patch_inner(project_id, session_id, request_id, operations, None)
    }

    pub fn draft_patch_image(
        &self,
        project_id: &str,
        session_id: &str,
        request_id: &str,
        operation: MapOperation,
        metadata: MapImageConversionMetadata,
    ) -> Result<Value, String> {
        metadata.validate_operation(&operation)?;
        self.draft_patch_inner(
            project_id,
            session_id,
            request_id,
            vec![operation],
            Some(metadata),
        )
    }

    fn draft_patch_inner(
        &self,
        project_id: &str,
        session_id: &str,
        request_id: &str,
        operations: Vec<MapOperation>,
        image_metadata: Option<MapImageConversionMetadata>,
    ) -> Result<Value, String> {
        if operations.is_empty() {
            return Err("map draft patch requires at least one operation".to_string());
        }
        let state = self.load_state(project_id, session_id)?;
        let mut active = self.inner.active.lock();
        let request = active_request_mut(&mut active, session_id, request_id)?;
        if request.finalized {
            return Err("request already finalized one visible candidate revision".to_string());
        }
        let draft = request
            .draft_path
            .clone()
            .ok_or_else(|| "call map_draft_begin before patching".to_string())?;
        let batch = MapEditBatch {
            schema: MAP_EDIT_SCHEMA.to_string(),
            expected: MapEditExpected {
                input_file_sha256: file_hash(&draft)?,
                tileset: state.baseline.tileset,
                width: state.baseline.width,
                height: state.baseline.height,
            },
            operations: operations.clone(),
        };
        batch.validate()?;
        let bytes = serde_json::to_vec(&batch)
            .map_err(|error| format!("map edit batch could not be serialized: {error}"))?;
        let output = self
            .session_root(project_id, session_id)
            .join("drafts")
            .join(format!("{request_id}.next.scx"));
        remove_if_exists(&output)?;
        let report = isom::mapedit(
            &draft,
            &output,
            &self.inner.context.starcraft_path()?,
            &bytes,
        )
        .map_err(|error| format!("native map draft patch failed: {error}"))?;
        let report: Value = serde_json::from_str(&report)
            .map_err(|error| format!("native map report is invalid: {error}"))?;
        let verification = self.inner.verifier.verify(
            &draft,
            &output,
            &request.authority,
            &self.inner.context.starcraft_path()?,
            Some(&report),
        );
        if !verification.valid {
            remove_if_exists(&output)?;
            return Err(format!(
                "map draft patch violates the current request authority: {}",
                verification.errors.join("; ")
            ));
        }
        copy_atomic(&output, &draft)?;
        remove_if_exists(&output)?;
        request.batches.push(operations);
        request.reports.push(report.clone());
        if let Some(metadata) = image_metadata {
            request.image_conversions.push(metadata);
        }
        Ok(json!({
            "ok": true,
            "requestId": request_id,
            "draftHash": file_hash(&draft)?,
            "nativeReport": report,
            "verification": verification,
        }))
    }

    pub fn draft_reset(
        &self,
        project_id: &str,
        session_id: &str,
        request_id: &str,
    ) -> Result<Value, String> {
        let state = self.load_state(project_id, session_id)?;
        let mut active = self.inner.active.lock();
        let request = active_request_mut(&mut active, session_id, request_id)?;
        let draft = request
            .draft_path
            .clone()
            .ok_or_else(|| "request has no draft to reset".to_string())?;
        if state.current_revision != request.parent_revision
            || file_hash(&state.current_map)? != request.parent_hash
        {
            return Err("candidate parent changed after request preparation".to_string());
        }
        copy_atomic(&state.current_map, &draft)?;
        request.batches.clear();
        request.reports.clear();
        request.image_conversions.clear();
        request.imported_provenance.clear();
        Ok(json!({"ok": true, "requestId": request_id, "draftHash": request.parent_hash}))
    }

    pub fn draft_analyze(
        &self,
        project_id: &str,
        session_id: &str,
        request_id: &str,
    ) -> Result<VerificationReport, String> {
        let state = self.load_state(project_id, session_id)?;
        let active = self.inner.active.lock();
        let request = active_request(&active, session_id, request_id)?;
        let draft = request
            .draft_path
            .as_deref()
            .ok_or_else(|| "request has no draft".to_string())?;
        let report = request.reports.last();
        Ok(self.inner.verifier.verify(
            &state.current_map,
            draft,
            &request.authority,
            &self.inner.context.starcraft_path()?,
            report,
        ))
    }

    pub fn finalize(
        &self,
        project_id: &str,
        session_id: &str,
        request_id: &str,
    ) -> Result<CandidateStateView, String> {
        let state = self.load_state(project_id, session_id)?;
        let mut active = self.inner.active.lock();
        let request = active_request_mut(&mut active, session_id, request_id)?;
        if request.finalized {
            return Err("one user request can finalize at most one visible revision".to_string());
        }
        if request.batches.is_empty() {
            return Err(
                "candidate finalize requires at least one successful draft patch".to_string(),
            );
        }
        if state.current_revision != request.parent_revision
            || file_hash(&state.current_map)? != request.parent_hash
        {
            return Err("candidate parent changed while the request draft was active".to_string());
        }
        let draft = request
            .draft_path
            .clone()
            .ok_or_else(|| "request has no draft".to_string())?;
        let verification = self.inner.verifier.verify(
            &state.current_map,
            &draft,
            &request.authority,
            &self.inner.context.starcraft_path()?,
            request.reports.last(),
        );
        if !verification.valid {
            return Err(format!(
                "candidate verification failed: {}",
                verification.errors.join("; ")
            ));
        }
        let revision = state
            .revisions
            .iter()
            .map(|revision| revision.revision)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| "candidate revision overflow".to_string())?;
        let object_ids =
            update_candidate_object_ids(&state.candidate_object_ids, &state.current_map, &draft)?;
        let manifest = RevisionManifest {
            schema: "eud-map-candidate-manifest/1".to_string(),
            revision,
            parent: request.parent_revision,
            request_id: request_id.to_string(),
            authority: request.authority.clone(),
            batches: request.batches.clone(),
            image_conversions: request.image_conversions.clone(),
            imported_stamps: request.imported_provenance.clone(),
            object_ids: object_ids.clone(),
        };
        let manifest_path = self
            .session_root(project_id, session_id)
            .join("revisions")
            .join(format!("r{revision:04}.json"));
        write_json_atomic(&manifest_path, &manifest)?;
        let map_sha256 = file_hash(&draft)?;
        let candidate_revision = CandidateRevision {
            revision,
            parent: request.parent_revision,
            request_id: request_id.to_string(),
            operation_manifest: manifest_path,
            map_sha256,
            diff: verification.diff.clone(),
            verification,
        };
        request.pending_revision = Some(PendingRevision {
            revision: candidate_revision.clone(),
            object_ids: object_ids.clone(),
        });
        request.finalized = true;
        let mut preview = state;
        preview.current_revision = revision;
        preview.current_map = draft;
        preview.revisions.push(candidate_revision);
        preview.candidate_object_ids = object_ids;
        self.sync_selection_palette(&mut preview)?;
        self.view(&preview)
    }

    pub fn commit_request(
        &self,
        project_id: &str,
        session_id: &str,
        request_id: &str,
    ) -> Result<CandidateStateView, String> {
        let lock = self.session_lock(session_id);
        let _guard = lock.lock();
        let mut state = self.load_state(project_id, session_id)?;
        let mut active = self.inner.active.lock();
        let request = active_request_mut(&mut active, session_id, request_id)?;
        let Some(pending) = request.pending_revision.clone() else {
            return self.view(&state);
        };
        if state.current_revision != request.parent_revision
            || file_hash(&state.current_map)? != request.parent_hash
        {
            return Err("candidate parent changed before turn commit".to_string());
        }
        let draft = request
            .draft_path
            .clone()
            .ok_or_else(|| "finalized request draft is missing".to_string())?;
        if file_hash(&draft)? != pending.revision.map_sha256 {
            return Err("finalized request draft changed before turn commit".to_string());
        }
        copy_atomic(&draft, &state.current_map)?;
        state.current_revision = pending.revision.revision;
        state.revisions.push(pending.revision);
        state.candidate_object_ids = pending.object_ids;
        self.sync_selection_palette(&mut state)?;
        self.save_state(&state)?;
        request.pending_revision = None;
        request.draft_path = None;
        let _ = remove_if_exists(&draft);
        self.view(&state)
    }

    pub fn finish_request(&self, session_id: &str, request_id: &str) -> Result<(), String> {
        let mut active = self.inner.active.lock();
        let Some(request) = active.get(session_id) else {
            return Ok(());
        };
        if request.request_id != request_id {
            return Err("request cleanup ownership mismatch".to_string());
        }
        if let Some(draft) = request.draft_path.as_deref() {
            remove_if_exists(draft)?;
        }
        if let Some(pending) = request.pending_revision.as_ref() {
            remove_if_exists(&pending.revision.operation_manifest)?;
        }
        let source_hashes = request
            .imported_sources
            .values()
            .map(|source| source.stamp.source_file_sha256.clone())
            .collect::<Vec<_>>();
        active.remove(session_id);
        drop(active);
        for source_hash in source_hashes {
            self.inner.imports.release_blob(&source_hash);
        }
        Ok(())
    }

    pub fn cancel_session(&self, session_id: &str) -> Result<(), String> {
        let request_id = self
            .inner
            .active
            .lock()
            .get(session_id)
            .map(|request| request.request_id.clone());
        if let Some(request_id) = request_id {
            self.finish_request(session_id, &request_id)?;
        }
        Ok(())
    }

    pub fn revert(
        &self,
        project_id: &str,
        session_id: &str,
        revision: u32,
    ) -> Result<CandidateStateView, String> {
        if self.inner.active.lock().contains_key(session_id) {
            return Err("cannot revert while a map request is active".to_string());
        }
        let lock = self.session_lock(session_id);
        let _guard = lock.lock();
        let mut state = self.load_state(project_id, session_id)?;
        if revision != 0 && !state.revisions.iter().any(|item| item.revision == revision) {
            return Err(format!("candidate revision r{revision} does not exist"));
        }
        self.replay_into(&state, revision, &state.current_map)?;
        state.current_revision = revision;
        state.candidate_object_ids = if revision == 0 {
            BTreeMap::new()
        } else {
            let revision = state
                .revisions
                .iter()
                .find(|item| item.revision == revision)
                .ok_or_else(|| "candidate revision disappeared during revert".to_string())?;
            read_json::<RevisionManifest>(&revision.operation_manifest)?.object_ids
        };
        // A shorter chain may replay onto the saved source after all.
        self.follow_source_with(&mut state, true)?;
        self.sync_selection_palette(&mut state)?;
        self.save_state(&state)?;
        self.view(&state)
    }

    pub fn discard(&self, project_id: &str, session_id: &str) -> Result<(), String> {
        if self.inner.active.lock().contains_key(session_id) {
            return Err("cannot discard while a map request is active".to_string());
        }
        let root = self.session_root(project_id, session_id);
        if root.exists() {
            std::fs::remove_dir_all(&root)
                .map_err(|error| format!("candidate session could not be discarded: {error}"))?;
        }
        Ok(())
    }

    pub fn current_map(&self, project_id: &str, session_id: &str) -> Result<PathBuf, String> {
        Ok(self.load_state(project_id, session_id)?.current_map)
    }

    pub fn baseline_map(&self, project_id: &str, session_id: &str) -> Result<PathBuf, String> {
        Ok(self.load_state(project_id, session_id)?.baseline_snapshot)
    }

    pub fn object_ids(
        &self,
        project_id: &str,
        session_id: &str,
    ) -> Result<BTreeMap<String, String>, String> {
        Ok(self
            .load_state(project_id, session_id)?
            .candidate_object_ids)
    }
    pub fn annotate_object_page(
        &self,
        project_id: &str,
        session_id: &str,
        mut page: Value,
    ) -> Result<Value, String> {
        let ids = self.object_ids(project_id, session_id)?;
        if let Some(items) = page.get_mut("items").and_then(Value::as_array_mut) {
            for item in items {
                let Some(reference) = item.get_mut("objectRef").and_then(Value::as_object_mut)
                else {
                    continue;
                };
                let Some(kind) = reference.get("kind").and_then(Value::as_str) else {
                    continue;
                };
                let kind = if kind == "unit" || kind == "building" {
                    "unit"
                } else {
                    kind
                };
                let Some(ordinal) = reference.get("ordinal").and_then(Value::as_u64) else {
                    continue;
                };
                let Some(fingerprint) =
                    reference.get("semanticFingerprint").and_then(Value::as_str)
                else {
                    continue;
                };
                let key = format!("{kind}:{ordinal}:{fingerprint}");
                if let Some(id) = ids.get(&key) {
                    reference.insert("candidateId".to_string(), Value::String(id.clone()));
                }
            }
        }
        Ok(page)
    }

    pub fn draft_map(&self, session_id: &str, request_id: &str) -> Result<PathBuf, String> {
        let active = self.inner.active.lock();
        active_request(&active, session_id, request_id)?
            .draft_path
            .clone()
            .ok_or_else(|| "request has no draft".to_string())
    }

    pub fn request_has_draft(&self, session_id: &str, request_id: &str) -> Result<bool, String> {
        let active = self.inner.active.lock();
        Ok(active_request(&active, session_id, request_id)?
            .draft_path
            .is_some())
    }

    pub fn verify_current_for_apply(
        &self,
        project_id: &str,
        session_id: &str,
    ) -> Result<VerificationReport, String> {
        let lock = self.session_lock(session_id);
        let _guard = lock.lock();
        let mut state = self.load_state(project_id, session_id)?;
        if self.follow_source(&mut state)? {
            self.save_state(&state)?;
        }
        if state.current_revision == 0 {
            return Err("there is no candidate revision to Apply".to_string());
        }
        if state.stale {
            return Err("the saved source changed while a map request is active; Apply follows it once the request settles".to_string());
        }
        let root = self.session_root(project_id, session_id);
        let parent = root.join("apply-parent.tmp.scx");
        let work = root.join("apply-work.tmp.scx");
        let next = root.join("apply-next.scx");
        let result = (|| {
            copy_atomic(&state.baseline_snapshot, &parent)?;
            let mut final_report = None;
            for revision_id in revision_chain(&state, state.current_revision)? {
                copy_atomic(&parent, &work)?;
                let revision = state
                    .revisions
                    .iter()
                    .find(|item| item.revision == revision_id)
                    .ok_or_else(|| format!("candidate revision r{revision_id} is missing"))?;
                let manifest: RevisionManifest = read_json(&revision.operation_manifest)?;
                let mut native_report = None;
                for operations in manifest.batches {
                    let batch = MapEditBatch {
                        schema: MAP_EDIT_SCHEMA.to_string(),
                        expected: MapEditExpected {
                            input_file_sha256: file_hash(&work)?,
                            tileset: state.baseline.tileset,
                            width: state.baseline.width,
                            height: state.baseline.height,
                        },
                        operations,
                    };
                    let bytes = serde_json::to_vec(&batch).map_err(|error| {
                        format!("Apply verification batch serialization failed: {error}")
                    })?;
                    remove_if_exists(&next)?;
                    let report =
                        isom::mapedit(&work, &next, &self.inner.context.starcraft_path()?, &bytes)
                            .map_err(|error| {
                                format!("Apply verification replay failed: {error}")
                            })?;
                    native_report =
                        Some(serde_json::from_str::<Value>(&report).map_err(|error| {
                            format!("Apply verification report is invalid: {error}")
                        })?);
                    copy_atomic(&next, &work)?;
                }
                let report = self.inner.verifier.verify(
                    &parent,
                    &work,
                    &manifest.authority,
                    &self.inner.context.starcraft_path()?,
                    native_report.as_ref(),
                );
                if !report.valid {
                    return Err(format!(
                        "candidate revision r{revision_id} failed Apply verification: {}",
                        report.errors.join("; ")
                    ));
                }
                copy_atomic(&work, &parent)?;
                final_report = Some(report);
            }
            let replayed_chk = isom::chk_extract(&parent)
                .map_err(|error| format!("Apply verification replay is unreadable: {error}"))?;
            let current_chk = isom::chk_extract(&state.current_map)
                .map_err(|error| format!("current candidate is unreadable: {error}"))?;
            if crate::chk::canonical_chk_digest(&replayed_chk)
                != crate::chk::canonical_chk_digest(&current_chk)
            {
                return Err(
                    "current candidate differs from its deterministic manifest replay".to_string(),
                );
            }
            let replayed_assets: Value = serde_json::from_str(
                &isom::map_digest(&parent)
                    .map_err(|error| format!("replayed container digest failed: {error}"))?,
            )
            .map_err(|error| format!("replayed container digest is invalid: {error}"))?;
            let current_assets: Value = serde_json::from_str(
                &isom::map_digest(&state.current_map)
                    .map_err(|error| format!("candidate container digest failed: {error}"))?,
            )
            .map_err(|error| format!("candidate container digest is invalid: {error}"))?;
            if replayed_assets.pointer("/extraAssets/digest")
                != current_assets.pointer("/extraAssets/digest")
            {
                return Err(
                    "current candidate extra assets differ from manifest replay".to_string()
                );
            }
            final_report.ok_or_else(|| "candidate verification chain is empty".to_string())
        })();
        let _ = remove_if_exists(&parent);
        let _ = remove_if_exists(&work);
        let _ = remove_if_exists(&next);
        result
    }

    /// Refuse while any request (agent turn, direct image/stamp confirm) owns
    /// the session lane.
    pub fn require_no_active_request(&self, session_id: &str) -> Result<(), String> {
        if self.inner.active.lock().contains_key(session_id) {
            return Err(
                "맵 요청이 진행 중입니다. 요청이 끝나거나 취소된 뒤 다시 시도해 주세요."
                    .to_string(),
            );
        }
        Ok(())
    }

    /// Stage a Map window properties save: diff `input` against the saved
    /// source map, run only the changed `scenario.set`/`player.set`/`force.set`
    /// operations into a session work file, verify it under a properties
    /// authority, and confirm the work map carries exactly the requested
    /// properties. The source map is not touched; the caller applies the
    /// returned work file through `MapSafe` and then `complete_apply`.
    pub fn properties_stage(
        &self,
        project_id: &str,
        session_id: &str,
        input: &MapPropertiesInput,
    ) -> Result<PropertiesStage, String> {
        self.require_no_active_request(session_id)?;
        let lock = self.session_lock(session_id);
        let _guard = lock.lock();
        let mut state = self.load_state(project_id, session_id)?;
        if self.follow_source(&mut state)? {
            self.save_state(&state)?;
        }
        if state.current_revision != 0 {
            return Err(format!(
                "적용하지 않은 후보 r{}이(가) 있습니다. 맵 속성을 저장하려면 먼저 후보를 적용하거나 폐기해 주세요.",
                state.current_revision
            ));
        }
        let source = state.baseline.source_path.clone();
        let chk = isom::chk_extract(&source)
            .map_err(|error| format!("소스 맵의 CHK를 읽지 못했습니다: {error}"))?;
        let encoded = EncodedProperties::validate(input, crate::chk::chk_has_strx(&chk))?;
        let operations = encoded.operations(&crate::chk::digest_chk(&chk));
        if operations.is_empty() {
            return Err("변경된 항목이 없습니다.".to_string());
        }
        let work = self.properties_work_path(project_id, session_id);
        let result = (|| {
            remove_if_exists(&work)?;
            let batch = MapEditBatch {
                schema: MAP_EDIT_SCHEMA.to_string(),
                expected: MapEditExpected {
                    input_file_sha256: state.baseline.file_sha256.clone(),
                    tileset: state.baseline.tileset,
                    width: state.baseline.width,
                    height: state.baseline.height,
                },
                operations,
            };
            batch.validate()?;
            let bytes = serde_json::to_vec(&batch).map_err(|error| {
                format!("map properties batch could not be serialized: {error}")
            })?;
            let starcraft_path = self.inner.context.starcraft_path()?;
            let report = isom::mapedit(&source, &work, &starcraft_path, &bytes)
                .map_err(|error| format!("맵 속성을 적용하지 못했습니다: {error}"))?;
            let report: Value = serde_json::from_str(&report)
                .map_err(|error| format!("native map report is invalid: {error}"))?;
            let authority = MapRequestAuthority::properties_only(
                session_id.to_string(),
                format!("properties-{}", uuid::Uuid::new_v4()),
                0,
                state.baseline.width,
                state.baseline.height,
            )?;
            let verification = self.inner.verifier.verify(
                &source,
                &work,
                &authority,
                &starcraft_path,
                Some(&report),
            );
            if !verification.valid {
                return Err(format!(
                    "맵 속성 검증에 실패해 저장하지 않았습니다: {}",
                    verification.errors.join("; ")
                ));
            }
            let work_chk = isom::chk_extract(&work)
                .map_err(|error| format!("맵 속성 결과 CHK를 읽지 못했습니다: {error}"))?;
            encoded.require_exact(&crate::chk::digest_chk(&work_chk))?;
            Ok(PropertiesStage {
                work: work.clone(),
                work_sha256: file_hash(&work)?,
                verification,
                operations: batch.operations.len(),
            })
        })();
        if result.is_err() {
            let _ = remove_if_exists(&work);
        }
        result
    }

    /// Remove the staged properties work file on every outcome.
    pub fn discard_properties_stage(&self, stage: &PropertiesStage) -> Result<(), String> {
        remove_if_exists(&stage.work)
    }

    fn properties_work_path(&self, project_id: &str, session_id: &str) -> PathBuf {
        self.session_root(project_id, session_id)
            .join(PROPERTIES_WORK_FILE)
    }

    pub fn complete_apply(
        &self,
        project_id: &str,
        session_id: &str,
        record: &crate::mapsafe::CandidateApplyRecord,
    ) -> Result<CandidateStateView, String> {
        let lock = self.session_lock(session_id);
        let _guard = lock.lock();
        let mut state = self.load_state(project_id, session_id)?;
        if state.baseline.source_path != record.source_path {
            return Err("Apply record source does not match the candidate session".to_string());
        }
        let revision = self
            .inner
            .context
            .revision_for_path(project_id.to_string(), &record.source_path)?;
        copy_atomic(&record.source_path, &state.baseline_snapshot)?;
        copy_atomic(&record.source_path, &state.current_map)?;
        state.baseline = revision;
        state.current_revision = 0;
        state.revisions.clear();
        state.selections.clear();
        state.persistent_protections.clear();
        state.candidate_object_ids.clear();
        state.stale = false;
        state.source_diverged = false;
        state.last_apply_backup = Some(record.backup_path.clone());
        state.last_apply_source_hash = Some(record.applied_sha256.clone());
        state.last_apply_before_hash = Some(record.before_sha256.clone());
        self.sync_selection_palette(&mut state)?;
        self.save_state(&state)?;
        self.view(&state)
    }

    pub fn last_apply_record(
        &self,
        project_id: &str,
        session_id: &str,
    ) -> Result<crate::mapsafe::CandidateApplyRecord, String> {
        let state = self.load_state(project_id, session_id)?;
        Ok(crate::mapsafe::CandidateApplyRecord {
            source_path: state.baseline.source_path.clone(),
            backup_path: state
                .last_apply_backup
                .ok_or_else(|| "there is no Map Agent Apply to undo".to_string())?,
            before_sha256: state
                .last_apply_before_hash
                .ok_or_else(|| "last Apply journal is missing its source hash".to_string())?,
            applied_sha256: state
                .last_apply_source_hash
                .ok_or_else(|| "last Apply journal is missing its applied hash".to_string())?,
        })
    }

    pub fn complete_undo(
        &self,
        project_id: &str,
        session_id: &str,
    ) -> Result<CandidateStateView, String> {
        let lock = self.session_lock(session_id);
        let _guard = lock.lock();
        let mut state = self.load_state(project_id, session_id)?;
        let revision = self
            .inner
            .context
            .revision_for_path(project_id.to_string(), &state.baseline.source_path)?;
        copy_atomic(&state.baseline.source_path, &state.baseline_snapshot)?;
        copy_atomic(&state.baseline.source_path, &state.current_map)?;
        state.baseline = revision;
        state.current_revision = 0;
        state.revisions.clear();
        state.selections.clear();
        state.persistent_protections.clear();
        state.candidate_object_ids.clear();
        state.stale = false;
        state.source_diverged = false;
        state.last_apply_backup = None;
        state.last_apply_source_hash = None;
        state.last_apply_before_hash = None;
        self.sync_selection_palette(&mut state)?;
        self.save_state(&state)?;
        self.view(&state)
    }

    fn validate_palette_entry(
        &self,
        state: &CandidateSession,
        entry: &crate::map_model::PaletteRef,
    ) -> Result<(), String> {
        use crate::map_model::PaletteKind;
        if entry.kind == PaletteKind::NewLocation {
            return if entry.layer == MapLayer::Locations
                && entry.entry_id == 0
                && entry.fingerprint == "new-location/1"
            {
                Ok(())
            } else {
                Err("location palette mention is invalid".to_string())
            };
        }
        let (kind, expected_layer, exact) = match entry.kind {
            PaletteKind::SemanticTerrain => ("brushes", MapLayer::Terrain, false),
            PaletteKind::ExactTile => ("tiles", MapLayer::Terrain, true),
            PaletteKind::Unit => ("units", MapLayer::Units, false),
            PaletteKind::Building => ("buildings", MapLayer::Buildings, false),
            PaletteKind::Doodad => ("doodads", MapLayer::Doodads, false),
            PaletteKind::Sprite => ("sprites", MapLayer::Sprites, false),
            PaletteKind::NewLocation => unreachable!(),
        };
        if entry.layer != expected_layer {
            return Err("palette mention kind and layer do not match".to_string());
        }
        let request = json!({
            "schema": "eud-map-catalog/1",
            "kind": kind,
            "tileset": state.baseline.tileset.era(),
            "offset": if exact { entry.entry_id } else { 0 },
            "limit": if exact { 1 } else { 512 }
        });
        let result = isom::catalog_query(
            &self.inner.context.starcraft_path()?,
            request.to_string().as_bytes(),
        )
        .map_err(|error| format!("palette mention could not be resolved: {error}"))?;
        let result: Value = serde_json::from_str(&result)
            .map_err(|error| format!("palette catalog response is invalid: {error}"))?;
        let valid = result["entries"].as_array().is_some_and(|entries| {
            entries.iter().any(|candidate| {
                candidate["id"].as_u64() == Some(u64::from(entry.entry_id))
                    && candidate["fingerprint"].as_str() == Some(entry.fingerprint.as_str())
            })
        });
        if valid {
            Ok(())
        } else {
            Err("palette mention is stale or does not match the current catalog".to_string())
        }
    }

    fn validate_new_location_qualifiers(
        &self,
        state: &CandidateSession,
        qualifiers: &crate::map_model::MentionQualifiers,
        expected_revision: &str,
    ) -> Result<(), String> {
        match qualifiers.location_name.as_deref() {
            Some(name) if !name.trim().is_empty() => {}
            _ => {
                return Err("new location mention requires a non-empty locationName".to_string());
            }
        }
        match (&qualifiers.location_selection, &qualifiers.location_bounds) {
            (Some(reference), None) => {
                let selection = state
                    .selections
                    .get(&reference.selection_id)
                    .ok_or_else(|| "new location bounds selection no longer exists".to_string())?;
                if selection.snapshot_hash() != reference.snapshot_hash
                    || selection.source_revision != reference.source_revision
                    || reference.source_revision != expected_revision
                {
                    return Err("new location bounds selection is stale".to_string());
                }
            }
            (None, Some(bounds)) => {
                if bounds.left >= bounds.right
                    || bounds.top >= bounds.bottom
                    || bounds.right > state.baseline.width
                    || bounds.bottom > state.baseline.height
                {
                    return Err("new location tile bounds are empty or outside DIM".to_string());
                }
            }
            (Some(_), Some(_)) => {
                return Err(
                    "new location mention must choose either a saved selection or direct bounds"
                        .to_string(),
                );
            }
            (None, None) => {
                return Err(
                    "new location mention requires saved-selection or direct tile bounds"
                        .to_string(),
                );
            }
        }
        Ok(())
    }

    fn replay_into(
        &self,
        state: &CandidateSession,
        revision: u32,
        output: &Path,
    ) -> Result<(), String> {
        let root = output
            .parent()
            .ok_or_else(|| "candidate output has no parent directory".to_string())?;
        let replay = root.join("replay.tmp.scx");
        copy_atomic(&state.baseline_snapshot, &replay)?;
        let chain = revision_chain(state, revision)?;
        for revision_id in chain {
            let revision = state
                .revisions
                .iter()
                .find(|item| item.revision == revision_id)
                .ok_or_else(|| format!("candidate revision r{revision_id} is missing"))?;
            let manifest: RevisionManifest = read_json(&revision.operation_manifest)?;
            if manifest.schema != "eud-map-candidate-manifest/1" {
                return Err("candidate manifest schema is unsupported".to_string());
            }
            for operations in manifest.batches {
                let batch = MapEditBatch {
                    schema: MAP_EDIT_SCHEMA.to_string(),
                    expected: MapEditExpected {
                        input_file_sha256: file_hash(&replay)?,
                        tileset: state.baseline.tileset,
                        width: state.baseline.width,
                        height: state.baseline.height,
                    },
                    operations,
                };
                let bytes = serde_json::to_vec(&batch)
                    .map_err(|error| format!("replay batch could not be serialized: {error}"))?;
                let next = root.join("replay.next.scx");
                remove_if_exists(&next)?;
                isom::mapedit(
                    &replay,
                    &next,
                    &self.inner.context.starcraft_path()?,
                    &bytes,
                )
                .map_err(|error| format!("candidate replay failed: {error}"))?;
                copy_atomic(&next, &replay)?;
                remove_if_exists(&next)?;
            }
            let chk = isom::chk_extract(&replay)
                .map_err(|error| format!("replayed candidate could not be parsed: {error}"))?;
            let canonical = crate::chk::canonical_chk_digest(&chk).overall_sha256;
            if canonical != revision.verification.canonical_digest {
                remove_if_exists(&replay)?;
                return Err(format!(
                    "candidate replay canonical digest mismatch at r{revision_id}"
                ));
            }
        }
        copy_atomic(&replay, output)?;
        remove_if_exists(&replay)?;
        Ok(())
    }

    fn view(&self, state: &CandidateSession) -> Result<CandidateStateView, String> {
        let current_hash = file_hash(&state.current_map)?;
        let valid = state.current_revision == 0
            || state
                .revisions
                .iter()
                .find(|revision| revision.revision == state.current_revision)
                .is_some_and(|revision| {
                    revision.verification.valid && revision.map_sha256 == current_hash
                });
        let current_revision_key = revision_key(state.current_revision, &current_hash);
        Ok(CandidateStateView {
            session_id: state.session_id.clone(),
            baseline: state.baseline.clone(),
            current_revision: state.current_revision,
            current_hash,
            revision_key: current_revision_key,
            revisions: state
                .revisions
                .iter()
                .map(|revision| CandidateRevisionView {
                    revision: revision.revision,
                    parent: revision.parent,
                    request_id: revision.request_id.clone(),
                    map_sha256: revision.map_sha256.clone(),
                    diff: revision.diff.clone(),
                    verification: revision.verification.clone(),
                })
                .collect(),
            selections: state
                .selections
                .values()
                .cloned()
                .map(|selection| SelectionView {
                    snapshot_hash: selection.snapshot_hash(),
                    selection,
                })
                .collect(),
            stale: state.stale,
            source_diverged: state.source_diverged,
            can_apply: state.current_revision > 0 && !state.stale && valid,
            can_undo: state.last_apply_backup.is_some()
                && state.last_apply_source_hash.as_deref()
                    == Some(state.baseline.file_sha256.as_str()),
        })
    }

    fn load_state(&self, project_id: &str, session_id: &str) -> Result<CandidateSession, String> {
        validate_component(project_id, "project id")?;
        validate_component(session_id, "map session id")?;
        let mut state =
            self.load_state_path(&self.session_root(project_id, session_id).join("state.json"))?;
        if self.sync_selection_palette(&mut state)? {
            self.save_state(&state)?;
        }
        Ok(state)
    }

    fn load_state_path(&self, path: &Path) -> Result<CandidateSession, String> {
        read_json(path).map_err(|error| format!("candidate state could not be loaded: {error}"))
    }

    fn save_state(&self, state: &CandidateSession) -> Result<(), String> {
        write_json_atomic(
            &self
                .session_root(&state.baseline.project_id, &state.session_id)
                .join("state.json"),
            state,
        )
    }

    fn sync_selection_palette(&self, state: &mut CandidateSession) -> Result<bool, String> {
        let _palette = self.inner.selection_palette.lock();
        let path = self.selection_library_path(&state.baseline.project_id);
        let existed = path.is_file();
        let mut library = self.read_selection_library(&state.baseline.project_id)?;
        if !existed && !state.selections.is_empty() {
            library
                .selections
                .extend(state.selections.values().map(|selection| {
                    (
                        selection.id.clone(),
                        PersistentSelection::from_selection(selection),
                    )
                }));
            self.write_selection_library(&state.baseline.project_id, &library)?;
        }
        if !existed && library.selections.is_empty() {
            return Ok(false);
        }
        let current_revision =
            revision_key(state.current_revision, &file_hash(&state.current_map)?);
        let selections = library
            .selections
            .values()
            .map(|selection| {
                selection
                    .bind(
                        current_revision.clone(),
                        state.baseline.width,
                        state.baseline.height,
                    )
                    .map(|bound| (bound.id.clone(), bound))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        let protections = selections
            .values()
            .filter(|selection| selection.role == SelectionRole::Protect)
            .map(|selection| selection.id.clone())
            .collect::<BTreeSet<_>>();
        let changed = state.selections != selections || state.persistent_protections != protections;
        state.selections = selections;
        state.persistent_protections = protections;
        Ok(changed)
    }

    fn read_selection_library(
        &self,
        project_id: &str,
    ) -> Result<PersistentSelectionLibrary, String> {
        let path = self.selection_library_path(project_id);
        if !path.is_file() {
            return Ok(PersistentSelectionLibrary::empty());
        }
        let library: PersistentSelectionLibrary = read_json(&path)
            .map_err(|error| format!("map selection palette could not be loaded: {error}"))?;
        if library.schema != "eud-map-selection-palette/1" {
            return Err("map selection palette schema is unsupported".to_string());
        }
        Ok(library)
    }

    fn write_selection_library(
        &self,
        project_id: &str,
        library: &PersistentSelectionLibrary,
    ) -> Result<(), String> {
        write_json_atomic(&self.selection_library_path(project_id), library)
    }

    fn selection_library_path(&self, project_id: &str) -> PathBuf {
        self.inner
            .dirs
            .map_candidates_dir()
            .join(project_id)
            .join("selection-palette.json")
    }

    fn session_root(&self, project_id: &str, session_id: &str) -> PathBuf {
        self.inner
            .dirs
            .map_candidates_dir()
            .join(project_id)
            .join(session_id)
    }
}

fn active_request<'a>(
    active: &'a HashMap<String, ActiveRequest>,
    session_id: &str,
    request_id: &str,
) -> Result<&'a ActiveRequest, String> {
    let request = active
        .get(session_id)
        .ok_or_else(|| "map request is not prepared".to_string())?;
    if request.request_id != request_id {
        return Err("map request ownership mismatch".to_string());
    }
    Ok(request)
}

fn active_request_mut<'a>(
    active: &'a mut HashMap<String, ActiveRequest>,
    session_id: &str,
    request_id: &str,
) -> Result<&'a mut ActiveRequest, String> {
    let request = active
        .get_mut(session_id)
        .ok_or_else(|| "map request is not prepared".to_string())?;
    if request.request_id != request_id {
        return Err("map request ownership mismatch".to_string());
    }
    Ok(request)
}

fn validate_component(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 128
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        })
    {
        return Err(format!("{label} is not a safe path component"));
    }
    Ok(())
}

fn candidate_state_exists(path: &Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("candidate state could not be inspected: {error}")),
    }
}

fn file_hash(path: &Path) -> Result<String, String> {
    std::fs::read(path)
        .map(|bytes| hex_sha256(&bytes))
        .map_err(|error| format!("candidate map bytes could not be read: {error}"))
}

fn source_mtime_ns(path: &Path) -> Result<u128, String> {
    let modified = std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map_err(|error| format!("source map mtime could not be read: {error}"))?;
    Ok(modified
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos())
}

fn revision_key(revision: u32, hash: &str) -> String {
    format!("r{revision}:{hash}")
}

fn copy_atomic(source: &Path, destination: &Path) -> Result<(), String> {
    let bytes = std::fs::read(source)
        .map_err(|error| format!("candidate source could not be copied: {error}"))?;
    crate::memory::write_atomic_bytes(destination, &bytes)
        .map_err(|error| format!("candidate destination could not be promoted atomically: {error}"))
}

fn write_json_atomic(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("candidate JSON could not be serialized: {error}"))?;
    crate::memory::write_atomic_bytes(path, &bytes)
        .map_err(|error| format!("candidate JSON could not be written atomically: {error}"))
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

fn remove_if_exists(path: &Path) -> Result<(), String> {
    // The same Windows contention that stalls the promotion rename also stalls
    // deleting the file it left behind, and a scanner's handle is not a reason
    // to fail a finalize.
    match crate::memory::retry_transient(|| std::fs::remove_file(path)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "candidate temporary file could not be removed: {error}"
        )),
    }
}

fn cleanup_drafts(drafts: &Path) -> Result<(), String> {
    let entries = std::fs::read_dir(drafts)
        .map_err(|error| format!("candidate drafts could not be inspected: {error}"))?;
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("candidate draft entry is unreadable: {error}"))?;
        if entry
            .file_type()
            .map_err(|error| format!("candidate draft type is unreadable: {error}"))?
            .is_file()
        {
            std::fs::remove_file(entry.path())
                .map_err(|error| format!("orphan candidate draft could not be removed: {error}"))?;
        }
    }
    Ok(())
}

fn revision_chain(state: &CandidateSession, revision: u32) -> Result<Vec<u32>, String> {
    let mut chain = Vec::new();
    let mut current = revision;
    while current != 0 {
        if chain.contains(&current) {
            return Err("candidate revision parent cycle detected".to_string());
        }
        chain.push(current);
        current = state
            .revisions
            .iter()
            .find(|item| item.revision == current)
            .ok_or_else(|| format!("candidate revision r{current} is missing"))?
            .parent;
    }
    chain.reverse();
    Ok(chain)
}

#[derive(Clone)]
struct ObjectSlot {
    kind: &'static str,
    ordinal: usize,
    fingerprint: String,
}

impl ObjectSlot {
    fn key(&self) -> String {
        format!("{}:{}:{}", self.kind, self.ordinal, self.fingerprint)
    }
}

fn object_slots(path: &Path) -> Result<Vec<ObjectSlot>, String> {
    let chk = isom::chk_extract(path)
        .map_err(|error| format!("candidate object identities could not be read: {error}"))?;
    let sections = crate::chk::assemble_sections(&crate::chk::walk_sections(&chk));
    let mut slots = Vec::new();
    for (kind, section, size) in [
        ("unit", "UNIT", crate::chk::UNIT_ENTRY_SIZE),
        ("doodad", "DD2 ", crate::chk::DD2_ENTRY_SIZE),
        ("sprite", "THG2", crate::chk::THG2_ENTRY_SIZE),
    ] {
        for (ordinal, bytes) in sections
            .get(section)
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .chunks_exact(size)
            .enumerate()
        {
            slots.push(ObjectSlot {
                kind,
                ordinal,
                fingerprint: hex_sha256(bytes),
            });
        }
    }
    Ok(slots)
}

fn update_candidate_object_ids(
    current: &BTreeMap<String, String>,
    before_path: &Path,
    after_path: &Path,
) -> Result<BTreeMap<String, String>, String> {
    let before = object_slots(before_path)?;
    let after = object_slots(after_path)?;
    let mut next = BTreeMap::new();
    for after_slot in &after {
        let matching = before
            .iter()
            .filter(|before_slot| {
                before_slot.kind == after_slot.kind
                    && before_slot.fingerprint == after_slot.fingerprint
            })
            .collect::<Vec<_>>();
        if matching.len() == 1 {
            if let Some(id) = current.get(&matching[0].key()) {
                next.insert(after_slot.key(), id.clone());
                continue;
            }
        }
        if let Some(before_slot) = before.iter().find(|before_slot| {
            before_slot.kind == after_slot.kind
                && before_slot.ordinal == after_slot.ordinal
                && !after.iter().any(|candidate| {
                    candidate.kind == before_slot.kind
                        && candidate.fingerprint == before_slot.fingerprint
                })
        }) {
            if let Some(id) = current.get(&before_slot.key()) {
                next.insert(after_slot.key(), id.clone());
                continue;
            }
        }
        let existed = before.iter().any(|before_slot| {
            before_slot.kind == after_slot.kind && before_slot.fingerprint == after_slot.fingerprint
        });
        if !existed {
            next.insert(after_slot.key(), uuid::Uuid::new_v4().to_string());
        }
    }
    Ok(next)
}

fn object_ref_key(object_ref: &crate::map_model::MapObjectRef) -> String {
    let kind = match object_ref.kind {
        MapObjectKind::Unit | MapObjectKind::Building => "unit",
        MapObjectKind::Doodad => "doodad",
        MapObjectKind::Sprite => "sprite",
    };
    format!(
        "{kind}:{}:{}",
        object_ref.ordinal, object_ref.semantic_fingerprint
    )
}

fn validate_object_ref(
    map_path: &Path,
    object_ref: &crate::map_model::MapObjectRef,
) -> Result<Option<u16>, String> {
    let chk = isom::chk_extract(map_path)
        .map_err(|error| format!("object mention could not be resolved: {error}"))?;
    let sections = crate::chk::assemble_sections(&crate::chk::walk_sections(&chk));
    let (section, size) = match object_ref.kind {
        MapObjectKind::Unit | MapObjectKind::Building => ("UNIT", crate::chk::UNIT_ENTRY_SIZE),
        MapObjectKind::Doodad => ("DD2 ", crate::chk::DD2_ENTRY_SIZE),
        MapObjectKind::Sprite => ("THG2", crate::chk::THG2_ENTRY_SIZE),
    };
    let entry = sections
        .get(section)
        .and_then(|bytes| bytes.chunks_exact(size).nth(object_ref.ordinal as usize))
        .ok_or_else(|| "object instance mention no longer resolves".to_string())?;
    if hex_sha256(entry) != object_ref.semantic_fingerprint {
        return Err("object instance mention fingerprint is stale".to_string());
    }
    Ok(matches!(
        object_ref.kind,
        MapObjectKind::Unit | MapObjectKind::Building
    )
    .then(|| u16::from_le_bytes([entry[8], entry[9]])))
}

/// Text encoded for the source map's string table plus the form
/// `chk::digest_chk` decodes it back to, so change detection and the
/// exact-delta check share one rule with the digest.
struct EncodedText {
    hex: String,
    decoded: String,
}

impl EncodedText {
    fn encode(
        text: &str,
        has_strx: bool,
        label: &str,
        min_bytes: usize,
        max_bytes: usize,
    ) -> Result<Self, String> {
        let bytes = crate::chk::encode_chk_text(text, has_strx);
        if bytes.len() < min_bytes {
            return Err(format!("{label}은(는) 비워 둘 수 없습니다."));
        }
        if bytes.len() > max_bytes {
            return Err(format!(
                "{label}이(가) 너무 깁니다 ({}바이트, 최대 {max_bytes}바이트).",
                bytes.len()
            ));
        }
        Ok(Self {
            hex: bytes_hex(&bytes),
            decoded: crate::chk::decode_text(&bytes),
        })
    }
}

struct EncodedPlayer {
    controller_id: u8,
    race_id: u8,
    force: Option<u8>,
    r#type: String,
    race: String,
}

struct EncodedForce {
    name: EncodedText,
    flags: [bool; 4],
}

/// A validated properties request in the source map's string encoding.
struct EncodedProperties {
    title: EncodedText,
    description: EncodedText,
    players: Vec<EncodedPlayer>,
    forces: Vec<EncodedForce>,
}

impl EncodedProperties {
    fn validate(input: &MapPropertiesInput, has_strx: bool) -> Result<Self, String> {
        if input.players.len() != 12 {
            return Err(format!(
                "플레이어 슬롯은 정확히 12개여야 합니다 (받은 값: {}개).",
                input.players.len()
            ));
        }
        if input.forces.len() != 4 {
            return Err(format!(
                "포스는 정확히 4개여야 합니다 (받은 값: {}개).",
                input.forces.len()
            ));
        }
        let title = EncodedText::encode(&input.title, has_strx, "맵 제목", 1, 1024)?;
        let description = EncodedText::encode(&input.description, has_strx, "맵 설명", 0, 4096)?;
        let players = input
            .players
            .iter()
            .enumerate()
            .map(|(slot, player)| {
                let controller_id = SLOT_TYPES
                    .iter()
                    .find(|(name, _)| *name == player.r#type)
                    .map(|(_, id)| *id)
                    .ok_or_else(|| {
                        format!(
                            "P{}의 플레이어 종류 '{}'은(는) 지원하지 않습니다.",
                            slot + 1,
                            player.r#type
                        )
                    })?;
                let race_id = RACES
                    .iter()
                    .find(|(name, _)| *name == player.race)
                    .map(|(_, id)| *id)
                    .ok_or_else(|| {
                        format!(
                            "P{}의 종족 '{}'은(는) 지원하지 않습니다.",
                            slot + 1,
                            player.race
                        )
                    })?;
                let force = match (slot < 8, player.force) {
                    (true, Some(force)) if force < 4 => Some(force),
                    (true, Some(force)) => {
                        return Err(format!(
                            "P{}의 포스 {}은(는) 1..4 범위를 벗어납니다.",
                            slot + 1,
                            u16::from(force) + 1
                        ))
                    }
                    (true, None) => {
                        return Err(format!("P{}의 포스를 지정해 주세요.", slot + 1));
                    }
                    (false, Some(_)) => {
                        return Err(format!(
                            "P{}은(는) 포스에 속할 수 없습니다 (P9..P12).",
                            slot + 1
                        ));
                    }
                    (false, None) => None,
                };
                Ok(EncodedPlayer {
                    controller_id,
                    race_id,
                    force,
                    r#type: player.r#type.clone(),
                    race: player.race.clone(),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let forces = input
            .forces
            .iter()
            .enumerate()
            .map(|(index, force)| {
                Ok(EncodedForce {
                    name: EncodedText::encode(
                        &force.name,
                        has_strx,
                        &format!("포스 {} 이름", index + 1),
                        1,
                        256,
                    )?,
                    flags: [
                        force.allied,
                        force.allied_victory,
                        force.shared_vision,
                        force.random_start,
                    ],
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Self {
            title,
            description,
            players,
            forces,
        })
    }

    /// Only the fields that differ from `digest`, as native operations.
    fn operations(&self, digest: &crate::chk::Digest) -> Vec<MapOperation> {
        let mut operations = Vec::new();
        let title_changed = digest.map.title != self.title.decoded;
        let description_changed = digest.map.description != self.description.decoded;
        if title_changed || description_changed {
            operations.push(MapOperation::ScenarioSet {
                title_bytes_hex: title_changed.then(|| self.title.hex.clone()),
                description_bytes_hex: description_changed.then(|| self.description.hex.clone()),
            });
        }
        for (slot, player) in self.players.iter().enumerate() {
            let current = digest.players.get(slot);
            let type_changed = current.map(|current| slot_type_class(current.controller_id))
                != Some(player.controller_id);
            let race_changed = current.map(|current| current.race_id) != Some(player.race_id);
            let force_changed = current.and_then(digest_force) != player.force;
            if type_changed || race_changed || force_changed {
                operations.push(MapOperation::PlayerSet {
                    slot: slot as u8,
                    r#type: type_changed.then(|| player.r#type.clone()),
                    race: race_changed.then(|| player.race.clone()),
                    force: force_changed.then_some(player.force).flatten(),
                });
            }
        }
        for (index, force) in self.forces.iter().enumerate() {
            let current = digest.forces.get(index);
            let name_changed =
                current.map(|current| current.name.as_str()) != Some(force.name.decoded.as_str());
            let current_flags = current.map(|current| digest_flags(&current.flags));
            let flag_changed =
                |flag: usize| current_flags.map(|flags| flags[flag]) != Some(force.flags[flag]);
            let changed = [
                flag_changed(0),
                flag_changed(1),
                flag_changed(2),
                flag_changed(3),
            ];
            if name_changed || changed.iter().any(|changed| *changed) {
                operations.push(MapOperation::ForceSet {
                    force: index as u8,
                    name_bytes_hex: name_changed.then(|| force.name.hex.clone()),
                    allied: changed[0].then_some(force.flags[0]),
                    allied_victory: changed[1].then_some(force.flags[1]),
                    shared_vision: changed[2].then_some(force.flags[2]),
                    random_start: changed[3].then_some(force.flags[3]),
                });
            }
        }
        operations
    }

    /// The staged map must carry exactly the requested properties.
    fn require_exact(&self, digest: &crate::chk::Digest) -> Result<(), String> {
        let mismatch = |field: String| {
            format!("맵 속성 결과가 요청과 다릅니다 ({field}). 소스 맵은 변경하지 않았습니다.")
        };
        if digest.map.title != self.title.decoded {
            return Err(mismatch("맵 제목".to_string()));
        }
        if digest.map.description != self.description.decoded {
            return Err(mismatch("맵 설명".to_string()));
        }
        for (slot, player) in self.players.iter().enumerate() {
            let current = digest
                .players
                .get(slot)
                .ok_or_else(|| mismatch(format!("P{} 슬롯 없음", slot + 1)))?;
            if slot_type_class(current.controller_id) != player.controller_id {
                return Err(mismatch(format!("P{} 플레이어 종류", slot + 1)));
            }
            if current.race_id != player.race_id {
                return Err(mismatch(format!("P{} 종족", slot + 1)));
            }
            if digest_force(current) != player.force {
                return Err(mismatch(format!("P{} 포스", slot + 1)));
            }
        }
        for (index, force) in self.forces.iter().enumerate() {
            let current = digest
                .forces
                .get(index)
                .ok_or_else(|| mismatch(format!("포스 {} 없음", index + 1)))?;
            if current.name != force.name.decoded {
                return Err(mismatch(format!("포스 {} 이름", index + 1)));
            }
            if digest_flags(&current.flags) != force.flags {
                return Err(mismatch(format!("포스 {} 설정", index + 1)));
            }
        }
        Ok(())
    }
}

/// `chk::Player.force` is the 1-based force label; the request uses 0..3.
fn digest_force(player: &crate::chk::Player) -> Option<u8> {
    player.force.map(|force| force.saturating_sub(1))
}

/// Force flags in request order: allied, allied victory, shared vision,
/// random start location.
fn digest_flags(flags: &crate::chk::ForceFlags) -> [bool; 4] {
    [
        flags.allies,
        flags.allied_victory,
        flags.shared_vision,
        flags.random_start_location,
    ]
}

fn bytes_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_model::{MapLayer, RowSpan, SelectionMask, UnitState};

    fn unique_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!("map-candidate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("crates")
            .join("isom")
            .join("tests")
            .join("fixtures")
            .join("map_agent_rich.scx")
    }

    fn context(dirs: &DataDirs, source: &Path) -> MapContextSnapshot {
        let service = MapContextService::new(dirs.clone());
        let revision = service
            .revision_for_path("project".to_string(), source)
            .unwrap();
        let chk = isom::chk_extract(source).unwrap();
        MapContextSnapshot {
            revision,
            saved_source_notice: "saved".to_string(),
            source_file_size: std::fs::metadata(source).unwrap().len(),
            starcraft_path: PathBuf::from(r"C:\Program Files (x86)\StarCraft"),
            digest: crate::chk::digest_chk(&chk),
        }
    }

    fn full_target(view: &CandidateStateView, id: &str) -> SelectionMask {
        let rows = (0..view.baseline.height)
            .map(|y| crate::map_model::RowSpan {
                y,
                spans: vec![(0, view.baseline.width)],
            })
            .collect();
        SelectionMask::canonical(
            id,
            id,
            revision_key(view.current_revision, &view.current_hash),
            SelectionRole::Target,
            [
                MapLayer::Terrain,
                MapLayer::Units,
                MapLayer::Buildings,
                MapLayer::Doodads,
                MapLayer::Sprites,
                MapLayer::Locations,
            ]
            .into_iter()
            .collect(),
            crate::map_model::MaskGrid {
                width: view.baseline.width,
                height: view.baseline.height,
                rows,
            },
        )
        .unwrap()
    }

    fn region_mention(mask: &SelectionMask) -> MapMentionSnapshot {
        MapMentionSnapshot::Region {
            selection_id: mask.id.clone(),
            snapshot_hash: mask.snapshot_hash(),
            source_revision: mask.source_revision.clone(),
        }
    }

    /// Save `operations` into `source` the way another writer would.
    fn edit_source(dirs: &DataDirs, source: &Path, operations: Vec<MapOperation>) {
        let service = MapContextService::new(dirs.clone());
        let revision = service
            .revision_for_path("project".to_string(), source)
            .unwrap();
        let batch = MapEditBatch {
            schema: MAP_EDIT_SCHEMA.to_string(),
            expected: MapEditExpected {
                input_file_sha256: revision.file_sha256,
                tileset: revision.tileset,
                width: revision.width,
                height: revision.height,
            },
            operations,
        };
        let edited = source.with_extension("edited.scx");
        isom::mapedit(
            source,
            &edited,
            &service.starcraft_path().unwrap(),
            &serde_json::to_vec(&batch).unwrap(),
        )
        .unwrap();
        copy_atomic(&edited, source).unwrap();
        std::fs::remove_file(edited).unwrap();
    }

    fn first_brush(starcraft: &Path, tileset: u8) -> u16 {
        let request = json!({
            "schema": "eud-map-catalog/1",
            "kind": "brushes",
            "tileset": tileset,
            "offset": 0,
            "limit": 64,
        });
        let catalog: Value = serde_json::from_str(
            &isom::catalog_query(starcraft, request.to_string().as_bytes()).unwrap(),
        )
        .unwrap();
        catalog["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["graphicsValid"] == true)
            .expect("installed tileset must expose a graphics-valid brush")["terrainType"]
            .as_u64()
            .unwrap() as u16
    }

    fn blank_spec(tileset: u8, width: u16, height: u16, terrain_type: u16) -> isom::MapNewSpec {
        isom::MapNewSpec {
            version: "remastered".to_string(),
            tileset,
            width,
            height,
            terrain_type,
            title_bytes_hex: "6d6170".to_string(),
            description_bytes_hex: String::new(),
            players: vec![isom::MapNewPlayer {
                slot: 0,
                r#type: "human".to_string(),
                race: "userSelectable".to_string(),
                force: Some(0),
                start: Some(isom::MapNewStart { x: 128, y: 128 }),
            }],
            forces: vec![isom::MapNewForce {
                name_bytes_hex: "666f726365".to_string(),
                allied: true,
                allied_victory: true,
                shared_vision: false,
                random_start: false,
            }],
        }
    }

    fn unit_count(map: &Path) -> usize {
        object_slots(map)
            .unwrap()
            .iter()
            .filter(|slot| slot.kind == "unit")
            .count()
    }

    fn tile_at(map: &Path, x: usize, y: usize) -> u16 {
        let chk = isom::chk_extract(map).unwrap();
        let digest = crate::chk::digest_chk(&chk);
        digest.tiles[y * usize::from(digest.map.width) + x]
    }

    fn add_unit(x: u16, y: u16) -> MapOperation {
        MapOperation::UnitAdd {
            state: UnitState {
                type_id: 0,
                owner: 0,
                x,
                y,
                class_id: 0,
                relation_flags: 0,
                valid_state_flags: 0,
                valid_field_flags: 0,
                hp_percent: 100,
                shield_percent: 100,
                energy_percent: 100,
                resource_amount: 0,
                hangar_amount: 0,
                state_flags: 0,
                unused: 0,
                relation_class_id: 0,
            },
        }
    }

    #[test]
    fn startup_cleanup_removes_incomplete_candidate_directories() {
        let root = unique_root();
        let dirs = DataDirs::from_bases(&root.join("roaming"), &root.join("local"));
        let incomplete = dirs.map_candidates_dir().join("project").join("incomplete");
        std::fs::create_dir_all(incomplete.join("drafts")).unwrap();
        std::fs::write(
            incomplete.join("drafts").join("request.tmp.scx"),
            b"partial",
        )
        .unwrap();
        let store =
            CandidateStore::new((dirs).clone(), crate::map_import::MapImportStore::new(dirs));
        assert_eq!(store.cleanup_startup().unwrap(), 1);
        assert!(!incomplete.exists());
        std::fs::remove_dir_all(root).ok();
    }
    #[test]
    fn request_authority_defaults_to_full_map_and_ignores_unmentioned_targets() {
        let root = unique_root();
        let dirs = DataDirs::from_bases(&root.join("roaming"), &root.join("local"));
        dirs.ensure_dirs().unwrap();
        let source = root.join("source.scx");
        std::fs::copy(fixture(), &source).unwrap();
        let snapshot = context(&dirs, &source);
        let store =
            CandidateStore::new((dirs).clone(), crate::map_import::MapImportStore::new(dirs));
        let view = store.create_session("map-session", &snapshot).unwrap();

        let stored_target = full_target(&view, "stored-target");
        store
            .save_selection("project", "map-session", stored_target)
            .unwrap();
        let mut reference = full_target(&view, "reference");
        reference.role = SelectionRole::Reference;
        store
            .save_selection("project", "map-session", reference.clone())
            .unwrap();
        let mut anchor = full_target(&view, "anchor");
        anchor.role = SelectionRole::Anchor;
        store
            .save_selection("project", "map-session", anchor.clone())
            .unwrap();
        let mut protect = full_target(&view, "protect");
        protect.role = SelectionRole::Protect;
        protect.layers = [MapLayer::Terrain].into_iter().collect();
        store
            .save_selection("project", "map-session", protect)
            .unwrap();

        let authority = store
            .prepare_request(
                "project",
                "map-session",
                "request",
                0,
                &[region_mention(&reference), region_mention(&anchor)],
            )
            .unwrap();
        assert!(authority.target_masks.is_empty());
        assert_eq!(authority.forbidden_masks.len(), 1);
        for layer in crate::map_verify::SUPPORTED_MAP_LAYERS {
            assert!(authority.allows(layer, 0, 0), "{layer:?}");
        }
        assert!(authority.forbids(MapLayer::Terrain, 0, 0));
        assert!(!authority.forbids(MapLayer::Units, 0, 0));
        store.finish_request("map-session", "request").unwrap();
        std::fs::remove_dir_all(root).ok();
    }
    #[test]
    fn exact_object_and_location_mentions_remain_revision_bound() {
        let root = unique_root();
        let dirs = DataDirs::from_bases(&root.join("roaming"), &root.join("local"));
        dirs.ensure_dirs().unwrap();
        let source = root.join("source.scx");
        std::fs::copy(fixture(), &source).unwrap();
        let snapshot = context(&dirs, &source);
        let store =
            CandidateStore::new((dirs).clone(), crate::map_import::MapImportStore::new(dirs));
        let view = store.create_session("map-session", &snapshot).unwrap();
        let slot = object_slots(&source)
            .unwrap()
            .into_iter()
            .find(|slot| slot.kind == "doodad")
            .expect("rich fixture doodad");
        let object_ref = crate::map_model::MapObjectRef {
            kind: MapObjectKind::Doodad,
            ordinal: slot.ordinal as u32,
            semantic_fingerprint: slot.fingerprint,
            revision_key: view.revision_key.clone(),
            baseline_hash: view.baseline.file_sha256.clone(),
            candidate_id: None,
        };
        let location_id = snapshot.digest.locations[0].id as u16;
        let mentions = [
            MapMentionSnapshot::Object {
                object_ref: object_ref.clone(),
                role: crate::map_model::ObjectMentionRole::Subject,
            },
            MapMentionSnapshot::Location {
                location_id,
                revision_key: view.revision_key.clone(),
                baseline_hash: view.baseline.file_sha256.clone(),
            },
        ];
        let authority = store
            .prepare_request("project", "map-session", "request", 0, &mentions)
            .unwrap();
        assert!(authority.target_masks.is_empty());
        store.finish_request("map-session", "request").unwrap();

        let mut stale_object = object_ref;
        stale_object.semantic_fingerprint = "stale".to_string();
        assert!(store
            .prepare_request(
                "project",
                "map-session",
                "stale-object",
                0,
                &[MapMentionSnapshot::Object {
                    object_ref: stale_object,
                    role: crate::map_model::ObjectMentionRole::Subject,
                }],
            )
            .unwrap_err()
            .contains("fingerprint is stale"));
        assert!(store
            .prepare_request(
                "project",
                "map-session",
                "stale-location",
                0,
                &[MapMentionSnapshot::Location {
                    location_id,
                    revision_key: view.revision_key,
                    baseline_hash: "stale".to_string(),
                }],
            )
            .unwrap_err()
            .contains("another candidate map revision"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn active_request_draft_survives_candidate_reopen() {
        let root = unique_root();
        let dirs = DataDirs::from_bases(&root.join("roaming"), &root.join("local"));
        dirs.ensure_dirs().unwrap();
        let source = root.join("source.scx");
        std::fs::copy(fixture(), &source).unwrap();
        let snapshot = context(&dirs, &source);
        let store =
            CandidateStore::new((dirs).clone(), crate::map_import::MapImportStore::new(dirs));
        let view = store.create_session("map-session", &snapshot).unwrap();
        let target = full_target(&view, "target");
        store
            .save_selection("project", "map-session", target.clone())
            .unwrap();
        store
            .prepare_request(
                "project",
                "map-session",
                "request",
                0,
                &[region_mention(&target)],
            )
            .unwrap();
        store
            .draft_begin("project", "map-session", "request")
            .unwrap();
        let draft_path = store
            .inner
            .active
            .lock()
            .get("map-session")
            .and_then(|request| request.draft_path.clone())
            .unwrap();
        let draft_bytes = std::fs::read(&draft_path).unwrap();
        let draft_hash = file_hash(&draft_path).unwrap();

        store.open_session("map-session", &snapshot).unwrap();

        let reopened_path = store
            .inner
            .active
            .lock()
            .get("map-session")
            .and_then(|request| request.draft_path.clone())
            .unwrap();
        assert_eq!(reopened_path, draft_path);
        assert_eq!(std::fs::read(&reopened_path).unwrap(), draft_bytes);
        assert_eq!(file_hash(&reopened_path).unwrap(), draft_hash);
        let report = store
            .draft_analyze("project", "map-session", "request")
            .unwrap();
        assert!(report.valid, "{:?}", report.errors);
        store.finish_request("map-session", "request").unwrap();
        assert!(!reopened_path.exists());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn candidate_reopen_behavioral_smoke_preserves_source_and_commits_one_revision() {
        let root = unique_root();
        let dirs = DataDirs::from_bases(&root.join("roaming"), &root.join("local"));
        dirs.ensure_dirs().unwrap();
        let source = root.join("source.scx");
        std::fs::copy(fixture(), &source).unwrap();
        let source_hash = file_hash(&source).unwrap();
        let snapshot = context(&dirs, &source);
        let store =
            CandidateStore::new((dirs).clone(), crate::map_import::MapImportStore::new(dirs));
        let view = store.create_session("map-session", &snapshot).unwrap();
        let mut target = full_target(&view, "terrain-target");
        target.layers = [MapLayer::Terrain].into_iter().collect();
        store
            .save_selection("project", "map-session", target.clone())
            .unwrap();
        store
            .prepare_request(
                "project",
                "map-session",
                "request",
                0,
                &[region_mention(&target)],
            )
            .unwrap();
        store
            .draft_begin("project", "map-session", "request")
            .unwrap();
        let draft_path = store
            .inner
            .active
            .lock()
            .get("map-session")
            .and_then(|request| request.draft_path.clone())
            .unwrap();

        store.open_session("map-session", &snapshot).unwrap();
        let before = snapshot.digest.tiles[0];
        let after = snapshot
            .digest
            .tiles
            .iter()
            .copied()
            .find(|tile| *tile != before)
            .expect("fixture must contain at least two terrain tiles");
        let patch = store
            .draft_patch(
                "project",
                "map-session",
                "request",
                vec![MapOperation::TerrainSet {
                    x: 0,
                    y: 0,
                    before,
                    after,
                }],
            )
            .unwrap();
        let patched_hash = patch["draftHash"].as_str().unwrap().to_string();
        assert!(!patched_hash.is_empty());
        assert_eq!(file_hash(&draft_path).unwrap(), patched_hash);

        store.open_session("map-session", &snapshot).unwrap();
        assert_eq!(file_hash(&draft_path).unwrap(), patched_hash);
        let render_request = json!({
            "schema": "eud-map-render/1",
            "mode": "region",
            "x": 0,
            "y": 0,
            "width": 1,
            "height": 1,
            "scale": 1,
            "layers": ["terrain"],
        });
        let image = isom::render_region(
            &draft_path,
            &store.inner.context.starcraft_path().unwrap(),
            render_request.to_string().as_bytes(),
        )
        .unwrap();
        assert_eq!((image.width, image.height), (32, 32));
        assert!(!image.rgba.is_empty());
        let report = store
            .draft_analyze("project", "map-session", "request")
            .unwrap();
        assert!(report.valid, "{:?}", report.errors);
        assert_eq!(report.candidate_sha256, patched_hash);
        assert_eq!(report.diff.terrain_cells, 1);

        let preview = store.finalize("project", "map-session", "request").unwrap();
        assert_eq!(preview.current_revision, 1);
        let preview_selection = preview
            .selections
            .iter()
            .find(|item| item.selection.id == target.id)
            .unwrap();
        assert_eq!(
            preview_selection.selection.source_revision,
            preview.revision_key
        );
        assert!(store.finalize("project", "map-session", "request").is_err());
        assert_eq!(
            store
                .state("project", "map-session")
                .unwrap()
                .current_revision,
            0
        );
        let committed = store
            .commit_request("project", "map-session", "request")
            .unwrap();
        assert_eq!(committed.current_revision, 1);
        let committed_selection = committed
            .selections
            .iter()
            .find(|item| item.selection.id == target.id)
            .unwrap();
        assert_eq!(
            committed_selection.selection.source_revision,
            committed.revision_key
        );
        assert_eq!(file_hash(&source).unwrap(), source_hash);
        store.finish_request("map-session", "request").unwrap();
        assert!(!draft_path.exists());
        for revision in [0, 1] {
            let reverted = store.revert("project", "map-session", revision).unwrap();
            let rebound = reverted
                .selections
                .iter()
                .find(|item| item.selection.id == target.id)
                .unwrap();
            assert_eq!(rebound.selection.source_revision, reverted.revision_key);
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[cfg(windows)]
    #[test]
    fn long_windows_candidate_paths_patch_and_finalize_without_touching_source() {
        use std::os::windows::ffi::OsStrExt;

        let project_id = "p".repeat(64);
        let session_id = "s".repeat(36);
        let request_id = "r".repeat(40);
        let base_root = unique_root();
        let mut root = base_root.clone();
        loop {
            let dirs = DataDirs::from_bases(&root.join("roaming"), &root.join("local"));
            let draft = dirs
                .map_candidates_dir()
                .join(&project_id)
                .join(&session_id)
                .join("drafts")
                .join(format!("{request_id}.tmp.scx"));
            if draft.as_os_str().encode_wide().count() >= 271 {
                break;
            }
            root.push("long-path-segment");
        }
        let dirs = DataDirs::from_bases(&root.join("roaming"), &root.join("local"));
        dirs.ensure_dirs().unwrap();
        let source = base_root.join("source.scx");
        std::fs::copy(fixture(), &source).unwrap();
        let source_hash = file_hash(&source).unwrap();
        let context_service = MapContextService::new(dirs.clone());
        let revision = context_service
            .revision_for_path(project_id.clone(), &source)
            .unwrap();
        let chk = isom::chk_extract(&source).unwrap();
        let snapshot = MapContextSnapshot {
            revision,
            saved_source_notice: "saved".to_string(),
            source_file_size: std::fs::metadata(&source).unwrap().len(),
            starcraft_path: PathBuf::from(r"C:\Program Files (x86)\StarCraft"),
            digest: crate::chk::digest_chk(&chk),
        };
        let store = CandidateStore::new(dirs.clone(), crate::map_import::MapImportStore::new(dirs));
        let view = store.create_session(&session_id, &snapshot).unwrap();
        let target = full_target(&view, "target");
        store
            .save_selection(&project_id, &session_id, target.clone())
            .unwrap();
        store
            .prepare_request(
                &project_id,
                &session_id,
                &request_id,
                0,
                &[region_mention(&target)],
            )
            .unwrap();
        store
            .draft_begin(&project_id, &session_id, &request_id)
            .unwrap();
        let draft = store
            .inner
            .active
            .lock()
            .get(&session_id)
            .and_then(|request| request.draft_path.clone())
            .unwrap();
        assert!(draft.as_os_str().encode_wide().count() >= 271);
        assert!(
            draft
                .with_file_name(format!("{request_id}.next.scx"))
                .as_os_str()
                .encode_wide()
                .count()
                >= 271
        );

        let before = snapshot.digest.tiles[0];
        let after = snapshot
            .digest
            .tiles
            .iter()
            .copied()
            .find(|tile| *tile != before)
            .unwrap();
        store
            .draft_patch(
                &project_id,
                &session_id,
                &request_id,
                vec![MapOperation::TerrainSet {
                    x: 0,
                    y: 0,
                    before,
                    after,
                }],
            )
            .unwrap();
        let preview = store
            .finalize(&project_id, &session_id, &request_id)
            .unwrap();
        assert_eq!(preview.current_revision, 1);
        assert_eq!(
            preview
                .revisions
                .iter()
                .find(|revision| revision.revision == 1)
                .map(|revision| revision.diff.terrain_cells),
            Some(1)
        );
        assert_eq!(file_hash(&source).unwrap(), source_hash);

        store.finish_request(&session_id, &request_id).unwrap();
        drop(store);
        std::fs::remove_dir_all(base_root).unwrap();
    }

    #[test]
    fn strict_candidate_session_create_and_open_fail_closed() {
        let root = unique_root();
        let dirs = DataDirs::from_bases(&root.join("roaming"), &root.join("local"));
        dirs.ensure_dirs().unwrap();
        let source = root.join("source.scx");
        std::fs::copy(fixture(), &source).unwrap();
        let snapshot = context(&dirs, &source);
        let store = CandidateStore::new(
            (dirs.clone()).clone(),
            crate::map_import::MapImportStore::new(dirs.clone()),
        );
        let session_root = store.session_root("project", "map-session");

        assert_eq!(
            store.open_session("map-session", &snapshot).unwrap_err(),
            "candidate session does not exist"
        );
        assert!(!session_root.exists());

        store.create_session("map-session", &snapshot).unwrap();
        let state_path = session_root.join("state.json");
        let original_state = std::fs::read(&state_path).unwrap();
        assert_eq!(
            store.create_session("map-session", &snapshot).unwrap_err(),
            "candidate session already exists"
        );
        assert_eq!(std::fs::read(&state_path).unwrap(), original_state);

        let other_source = root.join("other.scx");
        std::fs::copy(fixture(), &other_source).unwrap();
        let other_snapshot = context(&dirs, &other_source);
        assert_eq!(
            store
                .open_session("map-session", &other_snapshot)
                .unwrap_err(),
            "candidate session belongs to a different project or source map"
        );

        std::fs::remove_file(session_root.join("current.scx")).unwrap();
        assert_eq!(
            store.open_session("map-session", &snapshot).unwrap_err(),
            "candidate session is incomplete; baseline/current map is missing"
        );

        let corrupt_root = store.session_root("project", "corrupt");
        std::fs::create_dir_all(&corrupt_root).unwrap();
        std::fs::write(corrupt_root.join("state.json"), b"not json").unwrap();
        assert!(store
            .open_session("corrupt", &snapshot)
            .unwrap_err()
            .contains("candidate state could not be loaded"));
        assert_eq!(
            store.create_session("corrupt", &snapshot).unwrap_err(),
            "candidate session already exists"
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn draft_finalize_revert_recovery_and_stale_source_are_safe() {
        let root = unique_root();
        let dirs = DataDirs::from_bases(&root.join("roaming"), &root.join("local"));
        dirs.ensure_dirs().unwrap();
        let source = root.join("source.scx");
        std::fs::copy(fixture(), &source).unwrap();
        let snapshot = context(&dirs, &source);
        let store = CandidateStore::new(
            (dirs.clone()).clone(),
            crate::map_import::MapImportStore::new(dirs.clone()),
        );
        let mut view = store.create_session("map-session", &snapshot).unwrap();

        let target0 = full_target(&view, "target-r0");
        store
            .save_selection("project", "map-session", target0.clone())
            .unwrap();
        store
            .prepare_request(
                "project",
                "map-session",
                "request-r1",
                0,
                &[region_mention(&target0)],
            )
            .unwrap();
        store
            .draft_begin("project", "map-session", "request-r1")
            .unwrap();
        store
            .draft_patch(
                "project",
                "map-session",
                "request-r1",
                vec![add_unit(512, 512)],
            )
            .unwrap();
        view = store
            .finalize("project", "map-session", "request-r1")
            .unwrap();
        assert_eq!(view.current_revision, 1);
        assert!(view.can_apply);
        let r1_canonical = view.revisions[0].verification.canonical_digest.clone();
        assert_eq!(view.revisions[0].diff.units.added, 1);
        assert_eq!(
            store
                .state("project", "map-session")
                .unwrap()
                .current_revision,
            0,
            "finalize remains request-local until the whole turn succeeds"
        );
        view = store
            .commit_request("project", "map-session", "request-r1")
            .unwrap();
        let r1_object_ids = store.object_ids("project", "map-session").unwrap();
        assert_eq!(r1_object_ids.len(), 1);
        assert!(r1_object_ids
            .values()
            .all(|id| uuid::Uuid::parse_str(id).is_ok()));
        let object_page = crate::tool_exec::map_objects_page(
            &store.current_map("project", "map-session").unwrap(),
            Path::new(r"C:\\Program Files (x86)\\StarCraft"),
            &view.revision_key,
            &view.baseline.file_sha256,
            "units",
            0,
            500,
        )
        .unwrap();
        let object_page = store
            .annotate_object_page("project", "map-session", object_page)
            .unwrap();
        assert!(object_page["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["objectRef"]["candidateId"].is_string()));
        assert!(store
            .finalize("project", "map-session", "request-r1")
            .is_err());
        store.finish_request("map-session", "request-r1").unwrap();

        let visible_hash = view.current_hash.clone();
        let target1 = full_target(&view, "target-r1");
        store
            .save_selection("project", "map-session", target1.clone())
            .unwrap();
        store
            .prepare_request(
                "project",
                "map-session",
                "request-failed",
                1,
                &[region_mention(&target1)],
            )
            .unwrap();
        store
            .draft_begin("project", "map-session", "request-failed")
            .unwrap();
        let invalid = MapOperation::UnitAdd {
            state: UnitState {
                type_id: 999,
                ..match add_unit(600, 600) {
                    MapOperation::UnitAdd { state } => state,
                    _ => unreachable!(),
                }
            },
        };
        assert!(store
            .draft_patch("project", "map-session", "request-failed", vec![invalid],)
            .is_err());
        store
            .finish_request("map-session", "request-failed")
            .unwrap();
        assert_eq!(
            store.state("project", "map-session").unwrap().current_hash,
            visible_hash
        );

        store
            .prepare_request(
                "project",
                "map-session",
                "request-r2",
                1,
                &[region_mention(&target1)],
            )
            .unwrap();
        store
            .draft_begin("project", "map-session", "request-r2")
            .unwrap();
        store
            .draft_patch(
                "project",
                "map-session",
                "request-r2",
                vec![add_unit(640, 640)],
            )
            .unwrap();
        view = store
            .finalize("project", "map-session", "request-r2")
            .unwrap();
        assert_eq!(view.current_revision, 2);
        assert_eq!(
            store
                .state("project", "map-session")
                .unwrap()
                .current_revision,
            1
        );
        store
            .commit_request("project", "map-session", "request-r2")
            .unwrap();
        store.finish_request("map-session", "request-r2").unwrap();

        view = store.revert("project", "map-session", 1).unwrap();
        let replayed =
            isom::chk_extract(&store.current_map("project", "map-session").unwrap()).unwrap();
        assert_eq!(
            crate::chk::canonical_chk_digest(&replayed).overall_sha256,
            r1_canonical
        );
        assert_eq!(view.current_revision, 1);

        let repaired_hash = view.current_hash.clone();
        let current_map = store.current_map("project", "map-session").unwrap();
        std::fs::copy(
            store.baseline_map("project", "map-session").unwrap(),
            &current_map,
        )
        .unwrap();
        assert_ne!(file_hash(&current_map).unwrap(), repaired_hash);

        let drafts = dirs
            .map_candidates_dir()
            .join("project")
            .join("map-session")
            .join("drafts");
        let orphan = drafts.join("orphan.tmp.scx");
        std::fs::write(&orphan, b"incomplete").unwrap();
        let reopened_view = store.open_session("map-session", &snapshot).unwrap();
        assert_eq!(reopened_view.current_hash, repaired_hash);
        assert!(orphan.exists(), "normal open must not sweep orphan drafts");

        let recovered = CandidateStore::new(
            (dirs.clone()).clone(),
            crate::map_import::MapImportStore::new(dirs.clone()),
        );
        recovered.cleanup_startup().unwrap();
        let recovered_view = recovered.open_session("map-session", &snapshot).unwrap();
        assert_eq!(recovered_view.current_revision, 1);
        assert_eq!(recovered_view.current_hash, repaired_hash);
        assert_eq!(
            recovered.object_ids("project", "map-session").unwrap(),
            r1_object_ids
        );
        assert!(!orphan.exists());

        // Another writer saves the source: the candidate follows it, keeping
        // its own unit on top of the newly saved one.
        let units_before = unit_count(&source);
        edit_source(&dirs, &source, vec![add_unit(256, 256)]);
        let followed = recovered.open_session("map-session", &snapshot).unwrap();
        assert!(!followed.stale);
        assert!(!followed.source_diverged);
        assert!(followed.can_apply);
        assert_eq!(followed.current_revision, 1);
        assert_eq!(followed.baseline.file_sha256, file_hash(&source).unwrap());
        assert_ne!(followed.current_hash, repaired_hash);
        assert_eq!(followed.revisions[0].diff.units.added, 1);
        assert_eq!(
            unit_count(&recovered.current_map("project", "map-session").unwrap()),
            units_before + 2
        );
        assert_eq!(
            recovered
                .object_ids("project", "map-session")
                .unwrap()
                .len(),
            1,
            "the rebased candidate keeps naming its own added unit"
        );
        let verification = recovered
            .verify_current_for_apply("project", "map-session")
            .unwrap();
        assert!(verification.valid, "{:?}", verification.errors);
        assert_eq!(verification.candidate_sha256, followed.current_hash);
        assert!(!dirs
            .map_candidates_dir()
            .join("project")
            .join("map-session")
            .read_dir()
            .unwrap()
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().starts_with("rebase-")));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn session_without_candidate_follows_the_saved_source() {
        let root = unique_root();
        let dirs = DataDirs::from_bases(&root.join("roaming"), &root.join("local"));
        dirs.ensure_dirs().unwrap();
        let source = root.join("source.scx");
        std::fs::copy(fixture(), &source).unwrap();
        let snapshot = context(&dirs, &source);
        let store = CandidateStore::new(
            (dirs.clone()).clone(),
            crate::map_import::MapImportStore::new(dirs.clone()),
        );
        let created = store.create_session("map-session", &snapshot).unwrap();

        edit_source(&dirs, &source, vec![add_unit(256, 256)]);
        let saved_hash = file_hash(&source).unwrap();
        assert_ne!(saved_hash, created.baseline.file_sha256);

        let view = store.state("project", "map-session").unwrap();
        assert!(!view.stale);
        assert!(!view.source_diverged);
        assert_eq!(view.current_revision, 0);
        assert_eq!(view.baseline.file_sha256, saved_hash);
        assert_eq!(view.current_hash, saved_hash);
        assert_eq!(
            file_hash(&store.baseline_map("project", "map-session").unwrap()).unwrap(),
            saved_hash
        );

        // A new request starts from the saved map without any reopen.
        store
            .prepare_request("project", "map-session", "request", 0, &[])
            .unwrap();
        store.finish_request("map-session", "request").unwrap();
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn session_without_candidate_follows_a_reshaped_source() {
        let root = unique_root();
        let dirs = DataDirs::from_bases(&root.join("roaming"), &root.join("local"));
        dirs.ensure_dirs().unwrap();
        let source = root.join("source.scx");
        std::fs::copy(fixture(), &source).unwrap();
        let snapshot = context(&dirs, &source);
        let store = CandidateStore::new(
            (dirs.clone()).clone(),
            crate::map_import::MapImportStore::new(dirs.clone()),
        );
        let created = store.create_session("map-session", &snapshot).unwrap();

        // A blank map of another size is saved under the same path.
        let starcraft = MapContextService::new(dirs.clone())
            .starcraft_path()
            .unwrap();
        let reshaped = root.join("reshaped.scx");
        let tileset = created.baseline.tileset as u8;
        isom::map_new(
            &reshaped,
            &starcraft,
            &blank_spec(tileset, 96, 64, first_brush(&starcraft, tileset)),
        )
        .unwrap();
        copy_atomic(&reshaped, &source).unwrap();
        let saved = MapContextService::new(dirs.clone())
            .revision_for_path("project".to_string(), &source)
            .unwrap();
        assert_ne!(
            (saved.width, saved.height),
            (created.baseline.width, created.baseline.height)
        );

        let view = store.state("project", "map-session").unwrap();
        assert!(!view.stale && !view.source_diverged);
        assert_eq!(view.current_revision, 0);
        assert_eq!(view.baseline.file_sha256, saved.file_sha256);
        assert_eq!(
            (view.baseline.width, view.baseline.height),
            (saved.width, saved.height)
        );
        assert_eq!(view.current_hash, saved.file_sha256);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn active_request_defers_following_until_it_settles() {
        let root = unique_root();
        let dirs = DataDirs::from_bases(&root.join("roaming"), &root.join("local"));
        dirs.ensure_dirs().unwrap();
        let source = root.join("source.scx");
        std::fs::copy(fixture(), &source).unwrap();
        let snapshot = context(&dirs, &source);
        let store = CandidateStore::new(
            (dirs.clone()).clone(),
            crate::map_import::MapImportStore::new(dirs.clone()),
        );
        let created = store.create_session("map-session", &snapshot).unwrap();
        store
            .prepare_request("project", "map-session", "request", 0, &[])
            .unwrap();
        store
            .draft_begin("project", "map-session", "request")
            .unwrap();

        edit_source(&dirs, &source, vec![add_unit(256, 256)]);
        let deferred = store.state("project", "map-session").unwrap();
        assert!(
            deferred.stale,
            "a live draft keeps its parent until it settles"
        );
        assert_eq!(deferred.baseline.file_sha256, created.baseline.file_sha256);
        assert!(store
            .verify_current_for_apply("project", "map-session")
            .is_err());

        store.finish_request("map-session", "request").unwrap();
        let followed = store.state("project", "map-session").unwrap();
        assert!(!followed.stale);
        assert_eq!(followed.baseline.file_sha256, file_hash(&source).unwrap());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn candidate_wins_when_the_saved_source_rejects_its_replay() {
        let root = unique_root();
        let dirs = DataDirs::from_bases(&root.join("roaming"), &root.join("local"));
        dirs.ensure_dirs().unwrap();
        let source = root.join("source.scx");
        std::fs::copy(fixture(), &source).unwrap();
        let snapshot = context(&dirs, &source);
        let store = CandidateStore::new(
            (dirs.clone()).clone(),
            crate::map_import::MapImportStore::new(dirs.clone()),
        );
        let view = store.create_session("map-session", &snapshot).unwrap();
        let target = full_target(&view, "target");
        store
            .save_selection("project", "map-session", target.clone())
            .unwrap();
        let before = tile_at(&source, 3, 3);
        store
            .prepare_request(
                "project",
                "map-session",
                "request",
                0,
                &[region_mention(&target)],
            )
            .unwrap();
        store
            .draft_begin("project", "map-session", "request")
            .unwrap();
        store
            .draft_patch(
                "project",
                "map-session",
                "request",
                vec![MapOperation::TerrainSet {
                    x: 3,
                    y: 3,
                    before,
                    after: before ^ 1,
                }],
            )
            .unwrap();
        store.finalize("project", "map-session", "request").unwrap();
        let view = store
            .commit_request("project", "map-session", "request")
            .unwrap();
        store.finish_request("map-session", "request").unwrap();
        let candidate_hash = view.current_hash.clone();

        // Someone else changes the very tile the candidate edited: the replay
        // conflicts, so the candidate keeps its bytes and wins on Apply.
        edit_source(
            &dirs,
            &source,
            vec![MapOperation::TerrainSet {
                x: 3,
                y: 3,
                before,
                after: before ^ 2,
            }],
        );
        let saved_hash = file_hash(&source).unwrap();
        let diverged = store.state("project", "map-session").unwrap();
        assert!(!diverged.stale);
        assert!(diverged.source_diverged);
        assert!(diverged.can_apply);
        assert_eq!(diverged.current_revision, 1);
        assert_eq!(diverged.current_hash, candidate_hash);
        assert_eq!(
            diverged.baseline.file_sha256, saved_hash,
            "Apply names the saved bytes it will overwrite"
        );
        let verification = store
            .verify_current_for_apply("project", "map-session")
            .unwrap();
        assert!(verification.valid);
        assert_eq!(verification.candidate_sha256, candidate_hash);

        // Reverting below the conflicting revision lets the session follow
        // the saved source again; the unreplayable revision is dropped.
        let reverted = store.revert("project", "map-session", 0).unwrap();
        assert!(!reverted.source_diverged);
        assert_eq!(reverted.current_revision, 0);
        assert_eq!(reverted.current_hash, saved_hash);
        assert!(reverted.revisions.is_empty());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn persistent_protect_blocks_finalize_even_when_prompt_omits_it() {
        let root = unique_root();
        let dirs = DataDirs::from_bases(&root.join("roaming"), &root.join("local"));
        dirs.ensure_dirs().unwrap();
        let source = root.join("source.scx");
        std::fs::copy(fixture(), &source).unwrap();
        let snapshot = context(&dirs, &source);
        let store = CandidateStore::new(
            (dirs.clone()).clone(),
            crate::map_import::MapImportStore::new(dirs.clone()),
        );
        let mut view = store.create_session("map-session", &snapshot).unwrap();
        let target = full_target(&view, "target");
        view = store
            .save_selection("project", "map-session", target.clone())
            .unwrap();
        let protect = SelectionMask::canonical(
            "protect",
            "protect",
            revision_key(view.current_revision, &view.current_hash),
            SelectionRole::Protect,
            Default::default(),
            crate::map_model::MaskGrid {
                width: view.baseline.width,
                height: view.baseline.height,
                rows: vec![RowSpan {
                    y: 16,
                    spans: vec![(16, 17)],
                }],
            },
        )
        .unwrap();
        store
            .save_selection("project", "map-session", protect)
            .unwrap();
        store
            .prepare_request(
                "project",
                "map-session",
                "request",
                0,
                &[region_mention(&target)],
            )
            .unwrap();
        store
            .draft_begin("project", "map-session", "request")
            .unwrap();
        let error = store
            .draft_patch(
                "project",
                "map-session",
                "request",
                vec![add_unit(512, 512)],
            )
            .unwrap_err();
        assert!(error.contains("protected"));
        assert_eq!(
            store
                .state("project", "map-session")
                .unwrap()
                .current_revision,
            0
        );
        store.finish_request("map-session", "request").unwrap();
        std::fs::remove_dir_all(root).ok();
    }
    #[test]
    fn layer_capability_cannot_be_expanded_by_draft_operations() {
        let root = unique_root();
        let dirs = DataDirs::from_bases(&root.join("roaming"), &root.join("local"));
        dirs.ensure_dirs().unwrap();
        let source = root.join("source.scx");
        std::fs::copy(fixture(), &source).unwrap();
        let snapshot = context(&dirs, &source);
        let store = CandidateStore::new(
            (dirs.clone()).clone(),
            crate::map_import::MapImportStore::new(dirs.clone()),
        );
        let view = store.create_session("map-session", &snapshot).unwrap();
        let mut target = full_target(&view, "terrain-only");
        target.layers = [MapLayer::Terrain].into_iter().collect();
        store
            .save_selection("project", "map-session", target.clone())
            .unwrap();
        store
            .prepare_request(
                "project",
                "map-session",
                "request",
                0,
                &[region_mention(&target)],
            )
            .unwrap();
        store
            .draft_begin("project", "map-session", "request")
            .unwrap();
        let error = store
            .draft_patch(
                "project",
                "map-session",
                "request",
                vec![add_unit(512, 512)],
            )
            .unwrap_err();
        assert!(error.contains("outside the current request authority"));
        assert_eq!(
            store
                .state("project", "map-session")
                .unwrap()
                .current_revision,
            0
        );
        store.finish_request("map-session", "request").unwrap();
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn semantic_isom_transition_outside_target_is_never_clipped_or_finalized() {
        let root = unique_root();
        let dirs = DataDirs::from_bases(&root.join("roaming"), &root.join("local"));
        dirs.ensure_dirs().unwrap();
        let source = root.join("source.scx");
        std::fs::copy(fixture(), &source).unwrap();
        let snapshot = context(&dirs, &source);
        let store = CandidateStore::new(
            (dirs.clone()).clone(),
            crate::map_import::MapImportStore::new(dirs.clone()),
        );
        let view = store.create_session("map-session", &snapshot).unwrap();
        let target = SelectionMask::canonical(
            "tiny-target",
            "tiny-target",
            revision_key(view.current_revision, &view.current_hash),
            SelectionRole::Target,
            [MapLayer::Terrain].into_iter().collect(),
            crate::map_model::MaskGrid {
                width: view.baseline.width,
                height: view.baseline.height,
                rows: vec![RowSpan {
                    y: 0,
                    spans: vec![(0, 1)],
                }],
            },
        )
        .unwrap();
        store
            .save_selection("project", "map-session", target.clone())
            .unwrap();
        store
            .prepare_request(
                "project",
                "map-session",
                "request",
                0,
                &[region_mention(&target)],
            )
            .unwrap();
        store
            .draft_begin("project", "map-session", "request")
            .unwrap();
        let catalog = isom::catalog_query(
            Path::new(r"C:\\Program Files (x86)\\StarCraft"),
            json!({
                "schema": "eud-map-catalog/1",
                "kind": "brushes",
                "tileset": view.baseline.tileset.era(),
                "offset": 0,
                "limit": 512
            })
            .to_string()
            .as_bytes(),
        )
        .unwrap();
        let catalog: Value = serde_json::from_str(&catalog).unwrap();
        let original_draft_hash =
            file_hash(&store.draft_map("map-session", "request").unwrap()).unwrap();
        let mut outside_error = None;
        for entry in catalog["entries"].as_array().unwrap() {
            store
                .draft_reset("project", "map-session", "request")
                .unwrap();
            let operation = MapOperation::TerrainIsomBrush {
                isom_x: 10,
                isom_y: 10,
                brush: entry["id"].as_u64().unwrap() as u16,
                extent: 1,
            };
            if let Err(error) =
                store.draft_patch("project", "map-session", "request", vec![operation])
            {
                if error.contains("outside the current request authority") {
                    outside_error = Some(error);
                    break;
                }
            }
        }
        assert!(outside_error
            .expect("fixture/catalog must produce one ISOM transition outside the target")
            .contains("outside the current request authority"));
        assert_eq!(
            file_hash(&store.draft_map("map-session", "request").unwrap()).unwrap(),
            original_draft_hash
        );
        assert_eq!(
            store
                .state("project", "map-session")
                .unwrap()
                .current_revision,
            0
        );
        store.finish_request("map-session", "request").unwrap();
        std::fs::remove_dir_all(root).ok();
    }

    /// A tile rectangle painted with a semantic brush reaches the draft through the ordinary
    /// patch/verify path, and the single-diamond form refuses off-lattice coordinates with the
    /// rule the model must correct instead of a bare placement failure.
    #[test]
    fn semantic_isom_rect_paints_a_plateau_and_off_lattice_brush_names_the_rule() {
        let root = unique_root();
        let dirs = DataDirs::from_bases(&root.join("roaming"), &root.join("local"));
        dirs.ensure_dirs().unwrap();
        let source = root.join("source.scx");
        std::fs::copy(fixture(), &source).unwrap();
        let snapshot = context(&dirs, &source);
        let store = CandidateStore::new(
            (dirs.clone()).clone(),
            crate::map_import::MapImportStore::new(dirs.clone()),
        );
        let view = store.create_session("map-session", &snapshot).unwrap();
        store
            .prepare_request("project", "map-session", "request", 0, &[])
            .unwrap();
        store
            .draft_begin("project", "map-session", "request")
            .unwrap();
        let catalog = isom::catalog_query(
            Path::new(r"C:\\Program Files (x86)\\StarCraft"),
            json!({
                "schema": "eud-map-catalog/1",
                "kind": "brushes",
                "tileset": view.baseline.tileset.era(),
                "offset": 0,
                "limit": 64
            })
            .to_string()
            .as_bytes(),
        )
        .unwrap();
        let catalog: Value = serde_json::from_str(&catalog).unwrap();
        let brushes = catalog["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["graphicsValid"] == true)
            .map(|entry| entry["id"].as_u64().unwrap() as u16)
            .collect::<Vec<_>>();
        // The Installation fixture: "Floor" (raised over "Substructure") stands in for a hill.
        assert!(brushes.len() >= 2, "{catalog}");
        let brush = brushes[1];

        let odd = store
            .draft_patch(
                "project",
                "map-session",
                "request",
                vec![MapOperation::TerrainIsomBrush {
                    isom_x: 3,
                    isom_y: 4,
                    brush,
                    extent: 1,
                }],
            )
            .unwrap_err();
        assert!(odd.contains("isomX + isomY must be even"), "{odd}");

        let width = view.baseline.width.min(24);
        let height = view.baseline.height.min(12);
        let result = store
            .draft_patch(
                "project",
                "map-session",
                "request",
                vec![MapOperation::TerrainIsomRect {
                    x: 8,
                    y: 8,
                    width,
                    height,
                    brush,
                }],
            )
            .unwrap();
        let effect = &result["nativeReport"]["effects"][0];
        assert_eq!(effect["op"], "terrain.isom_rect");
        assert!(effect["diamonds"].as_u64().unwrap() >= 1);
        let changed = effect["changedTiles"].as_u64().unwrap();
        assert!(changed > 0);
        assert_eq!(result["verification"]["valid"], true);
        assert_eq!(result["verification"]["diff"]["terrainCells"], changed);
        store.finish_request("map-session", "request").unwrap();
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    #[ignore = "requires installed StarCraft terrain and DAT assets"]
    fn no_target_request_mutates_every_supported_candidate_layer() {
        let root = std::env::temp_dir().join(format!("map-authority-all-{}", uuid::Uuid::new_v4()));
        let dirs = DataDirs::from_bases(&root.join("roaming"), &root.join("local"));
        dirs.ensure_dirs().unwrap();
        let source = root.join("source.scx");
        std::fs::copy(fixture(), &source).unwrap();
        let source_bytes = std::fs::read(&source).unwrap();
        let snapshot = context(&dirs, &source);
        let store =
            CandidateStore::new((dirs).clone(), crate::map_import::MapImportStore::new(dirs));
        let view = store.create_session("map-session", &snapshot).unwrap();
        let starcraft = Path::new(r"C:\Program Files (x86)\StarCraft");
        let catalog = |kind: &str| -> Value {
            serde_json::from_str(
                &isom::catalog_query(
                    starcraft,
                    json!({
                        "schema": "eud-map-catalog/1",
                        "kind": kind,
                        "tileset": view.baseline.tileset.era(),
                        "offset": 0,
                        "limit": 512,
                    })
                    .to_string()
                    .as_bytes(),
                )
                .unwrap(),
            )
            .unwrap()
        };
        let source_chk = isom::chk_extract(&source).unwrap();
        let digest = crate::chk::digest_chk(&source_chk);
        let before = digest.tiles[0];
        let after = catalog("tiles")["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["graphicsValid"] == true && entry["id"] != before)
            .and_then(|entry| entry["id"].as_u64())
            .unwrap();
        let building = catalog("buildings")["entries"][0]["id"].as_u64().unwrap();
        let doodad = catalog("doodads")["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["graphicsValid"] == true)
            .and_then(|entry| entry["id"].as_u64())
            .unwrap();
        let unit = catalog("units")["entries"][0]["id"].as_u64().unwrap();
        let operations: Vec<MapOperation> = serde_json::from_value(json!([
            {"op": "terrain.set", "x": 0, "y": 0, "before": before, "after": after},
            {"op": "unit.add", "state": {"typeId": unit, "owner": 4, "x": 160, "y": 160}},
            {"op": "unit.add", "state": {"typeId": building, "owner": 5, "x": 224, "y": 224}},
            {"op": "doodad.add", "state": {"doodadId": doodad, "x": 320, "y": 320, "owner": 11}},
            {"op": "sprite.add", "state": {"spriteId": 301, "x": 384, "y": 320, "owner": 3, "flags": 4096}},
            {"op": "location.add", "state": {
                "locationId": 0,
                "left": 128,
                "top": 128,
                "right": 256,
                "bottom": 256,
                "nameBytesHex": "4e6f20546172676574"
            }}
        ]))
        .unwrap();
        let authority = store
            .prepare_request("project", "map-session", "request", 0, &[])
            .unwrap();
        assert!(authority.target_masks.is_empty());
        store
            .draft_begin("project", "map-session", "request")
            .unwrap();
        let patch = store
            .draft_patch("project", "map-session", "request", operations)
            .unwrap();
        assert_eq!(patch["verification"]["valid"], true);
        let preview = store.finalize("project", "map-session", "request").unwrap();
        let diff = &preview.revisions.last().unwrap().diff;
        assert!(diff.terrain_cells > 0);
        assert!(diff.units.added > 0);
        assert!(diff.buildings.added > 0);
        assert!(diff.doodads.added > 0);
        assert!(diff.sprites.added > 0);
        assert!(diff.locations.added > 0);
        assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
        store.finish_request("map-session", "request").unwrap();
        std::fs::remove_dir_all(root).ok();
    }
    #[test]
    #[ignore = "requires installed StarCraft terrain and DAT assets"]
    fn exact_selection_stamp_roundtrips_real_map_without_isom() {
        let root = unique_root();
        let dirs = DataDirs::from_bases(&root.join("roaming"), &root.join("local"));
        dirs.ensure_dirs().unwrap();
        let source = root.join("source.scx");
        std::fs::copy(fixture(), &source).unwrap();
        let source_bytes = std::fs::read(&source).unwrap();
        let snapshot = context(&dirs, &source);
        let store =
            CandidateStore::new((dirs).clone(), crate::map_import::MapImportStore::new(dirs));
        let view = store.create_session("map-session", &snapshot).unwrap();
        let stamp = SelectionMask::canonical(
            "terrain-stamp",
            "Terrain stamp",
            view.revision_key.clone(),
            SelectionRole::Target,
            [MapLayer::Terrain].into_iter().collect(),
            crate::map_model::MaskGrid {
                width: view.baseline.width,
                height: view.baseline.height,
                rows: vec![
                    RowSpan {
                        y: 0,
                        spans: vec![(0, 2)],
                    },
                    RowSpan {
                        y: 1,
                        spans: vec![(0, 2)],
                    },
                ],
            },
        )
        .unwrap();
        let stamp_source = MapStampSourceRef::CandidateSelection {
            selection_id: stamp.id.clone(),
            snapshot_hash: stamp.snapshot_hash(),
        };
        store
            .save_selection("project", "map-session", stamp)
            .unwrap();
        store
            .prepare_request("project", "map-session", "stamp-request", 0, &[])
            .unwrap();
        store
            .draft_begin("project", "map-session", "stamp-request")
            .unwrap();
        let destination = [StampDestination { x: 4, y: 4 }];
        let preview = store
            .draft_stamp_preview(
                "project",
                "map-session",
                "stamp-request",
                &stamp_source,
                &destination,
            )
            .unwrap();
        assert_eq!(preview.terrain_cells_per_destination, 4);
        assert!(!preview.has_collisions());
        store
            .draft_stamp_place(
                "project",
                "map-session",
                "stamp-request",
                &stamp_source,
                &destination,
                StampCollisionPolicy::Merge,
            )
            .unwrap();
        store
            .finalize("project", "map-session", "stamp-request")
            .unwrap();
        store
            .commit_request("project", "map-session", "stamp-request")
            .unwrap();
        store
            .finish_request("map-session", "stamp-request")
            .unwrap();

        let original = crate::chk::digest_chk(&isom::chk_extract(&source).unwrap());
        let candidate_path = store.current_map("project", "map-session").unwrap();
        let candidate = crate::chk::digest_chk(&isom::chk_extract(&candidate_path).unwrap());
        for (source_x, source_y, destination_x, destination_y) in [
            (0usize, 0usize, 4usize, 4usize),
            (1, 0, 5, 4),
            (0, 1, 4, 5),
            (1, 1, 5, 5),
        ] {
            assert_eq!(
                candidate.tiles[destination_y * usize::from(candidate.map.width) + destination_x],
                original.tiles[source_y * usize::from(original.map.width) + source_x],
            );
        }
        let state = store.load_state("project", "map-session").unwrap();
        let manifest: RevisionManifest = read_json(&state.revisions[0].operation_manifest).unwrap();
        assert!(manifest
            .batches
            .iter()
            .flatten()
            .all(|operation| { !matches!(operation, MapOperation::TerrainIsomBrush { .. }) }));
        assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
        std::fs::remove_dir_all(root).ok();
    }
    #[test]
    #[ignore = "requires installed StarCraft terrain assets"]
    fn imported_stamp_is_request_bound_and_replay_is_blob_independent() {
        let root = unique_root();
        let dirs = DataDirs::from_bases(&root.join("roaming"), &root.join("local"));
        dirs.ensure_dirs().unwrap();
        let destination_source = root.join("destination.scx");
        let external_source = root.join("external.scx");
        std::fs::copy(fixture(), &destination_source).unwrap();
        std::fs::copy(fixture(), &external_source).unwrap();
        let destination_before = std::fs::read(&destination_source).unwrap();
        let external_before = std::fs::read(&external_source).unwrap();
        let snapshot = context(&dirs, &destination_source);
        let imports = crate::map_import::MapImportStore::new(dirs.clone());
        let store = CandidateStore::new(dirs.clone(), imports.clone());
        let view = store.create_session("map-session", &snapshot).unwrap();
        let selection = SelectionMask::canonical(
            "external-selection",
            "외부 영역",
            "import-source",
            SelectionRole::Reference,
            [MapLayer::Terrain].into_iter().collect(),
            crate::map_model::MaskGrid {
                width: view.baseline.width,
                height: view.baseline.height,
                rows: vec![
                    RowSpan {
                        y: 0,
                        spans: vec![(0, 2)],
                    },
                    RowSpan {
                        y: 1,
                        spans: vec![(0, 2)],
                    },
                ],
            },
        )
        .unwrap();
        let imported = imports
            .insert_test_stamp("project", &external_source, &selection)
            .unwrap();
        let source_ref = MapStampSourceRef::Imported {
            import_id: imported.id.clone(),
            snapshot_hash: imported.snapshot_hash.clone(),
        };
        let destination = [StampDestination { x: 4, y: 4 }];
        let preview = store
            .direct_stamp_preview(
                "project",
                "map-session",
                &view.revision_key,
                &source_ref,
                &destination,
            )
            .unwrap();
        assert_eq!(preview.terrain_cells_per_destination, 4);
        assert_eq!(
            store
                .state("project", "map-session")
                .unwrap()
                .current_revision,
            0
        );

        store
            .prepare_request("project", "map-session", "unmentioned", 0, &[])
            .unwrap();
        store
            .draft_begin("project", "map-session", "unmentioned")
            .unwrap();
        assert!(store
            .normalize_stamp_tool_source(
                "project",
                "map-session",
                "unmentioned",
                &MapStampToolSource::Imported {
                    import_id: imported.id.clone(),
                },
            )
            .unwrap_err()
            .contains("current-request mention"));
        store.finish_request("map-session", "unmentioned").unwrap();

        let mention = MapMentionSnapshot::ImportedStamp {
            import_id: imported.id.clone(),
            snapshot_hash: imported.snapshot_hash.clone(),
        };
        store
            .prepare_request("project", "map-session", "import-request", 0, &[mention])
            .unwrap();
        store
            .draft_begin("project", "map-session", "import-request")
            .unwrap();
        let normalized = store
            .normalize_stamp_tool_source(
                "project",
                "map-session",
                "import-request",
                &MapStampToolSource::Imported {
                    import_id: imported.id.clone(),
                },
            )
            .unwrap();
        store
            .draft_stamp_place(
                "project",
                "map-session",
                "import-request",
                &normalized,
                &destination,
                StampCollisionPolicy::Merge,
            )
            .unwrap();
        store
            .finalize("project", "map-session", "import-request")
            .unwrap();
        let committed = store
            .commit_request("project", "map-session", "import-request")
            .unwrap();
        let committed_hash = committed.current_hash.clone();
        let state = store.load_state("project", "map-session").unwrap();
        let manifest: RevisionManifest = read_json(&state.revisions[0].operation_manifest).unwrap();
        assert_eq!(manifest.imported_stamps.len(), 1);
        let manifest_json = serde_json::to_string(&manifest).unwrap();
        assert!(!manifest_json.contains("external.scx"));
        assert!(!manifest_json.to_ascii_lowercase().contains("blob"));
        let blob = imports
            .resolve_imported(
                "project",
                &imported.id,
                &imported.snapshot_hash,
                view.baseline.tileset,
            )
            .unwrap()
            .blob_path;
        store
            .finish_request("map-session", "import-request")
            .unwrap();
        std::fs::remove_file(&blob).unwrap();

        store.revert("project", "map-session", 0).unwrap();
        let replayed = store.revert("project", "map-session", 1).unwrap();
        assert_eq!(replayed.current_hash, committed_hash);
        assert!(store
            .direct_stamp_preview(
                "project",
                "map-session",
                &replayed.revision_key,
                &source_ref,
                &destination,
            )
            .unwrap_err()
            .contains("missing"));
        assert_eq!(
            std::fs::read(&destination_source).unwrap(),
            destination_before
        );
        assert_eq!(std::fs::read(&external_source).unwrap(), external_before);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    #[ignore = "requires installed StarCraft assets plus MAP_IMPORT_SMOKE_SOURCE and MAP_IMPORT_SMOKE_DESTINATION"]
    fn real_cross_dimension_import_direct_and_request_paths_are_exact_and_blob_free_on_replay() {
        let source_path = PathBuf::from(
            std::env::var("MAP_IMPORT_SMOKE_SOURCE").expect("MAP_IMPORT_SMOKE_SOURCE is required"),
        );
        let destination_path = PathBuf::from(
            std::env::var("MAP_IMPORT_SMOKE_DESTINATION")
                .expect("MAP_IMPORT_SMOKE_DESTINATION is required"),
        );
        let source_before = std::fs::read(&source_path).unwrap();
        let destination_before = std::fs::read(&destination_path).unwrap();
        let roaming = PathBuf::from(std::env::var("APPDATA").expect("APPDATA is required"));
        let local = PathBuf::from(std::env::var("LOCALAPPDATA").expect("LOCALAPPDATA is required"));
        let dirs = DataDirs::from_bases(&roaming, &local);
        let project_id = format!("cross-map-smoke-{}", uuid::Uuid::new_v4());
        let context_service = MapContextService::new(dirs.clone());
        let destination_revision = context_service
            .revision_for_path(project_id.clone(), &destination_path)
            .unwrap();
        let source_revision = context_service
            .revision_for_path(project_id.clone(), &source_path)
            .unwrap();
        assert_eq!(source_revision.tileset, destination_revision.tileset);
        assert_ne!(
            (source_revision.width, source_revision.height),
            (destination_revision.width, destination_revision.height)
        );
        let destination_chk = isom::chk_extract(&destination_path).unwrap();
        let snapshot = MapContextSnapshot {
            revision: destination_revision,
            saved_source_notice: "real cross-map smoke".to_string(),
            source_file_size: destination_before.len() as u64,
            starcraft_path: context_service.starcraft_path().unwrap(),
            digest: crate::chk::digest_chk(&destination_chk),
        };
        let imports = crate::map_import::MapImportStore::new(dirs.clone());
        let store = CandidateStore::new(dirs, imports.clone());
        let view = store
            .create_session("cross-map-session", &snapshot)
            .unwrap();
        let selection = SelectionMask::canonical(
            "real-cross-map-selection",
            "real cross-map selection",
            "import-source",
            SelectionRole::Reference,
            crate::map_verify::SUPPORTED_MAP_LAYERS
                .into_iter()
                .collect(),
            crate::map_model::MaskGrid {
                width: source_revision.width,
                height: source_revision.height,
                rows: vec![
                    RowSpan {
                        y: 0,
                        spans: vec![(0, 2)],
                    },
                    RowSpan {
                        y: 1,
                        spans: vec![(0, 2)],
                    },
                ],
            },
        )
        .unwrap();
        let imported = imports
            .insert_test_stamp(&project_id, &source_path, &selection)
            .unwrap();
        let source_ref = MapStampSourceRef::Imported {
            import_id: imported.id.clone(),
            snapshot_hash: imported.snapshot_hash.clone(),
        };
        let destinations = [StampDestination { x: 4, y: 4 }];
        let direct_preview = store
            .direct_stamp_preview(
                &project_id,
                "cross-map-session",
                &view.revision_key,
                &source_ref,
                &destinations,
            )
            .unwrap();
        assert_eq!(direct_preview.terrain_cells_per_destination, 4);
        assert_eq!(
            store
                .state(&project_id, "cross-map-session")
                .unwrap()
                .current_revision,
            0
        );
        let mention = MapMentionSnapshot::ImportedStamp {
            import_id: imported.id.clone(),
            snapshot_hash: imported.snapshot_hash.clone(),
        };
        store
            .prepare_request(
                &project_id,
                "cross-map-session",
                "cross-map-request",
                0,
                &[mention],
            )
            .unwrap();
        store
            .draft_begin(&project_id, "cross-map-session", "cross-map-request")
            .unwrap();
        store
            .draft_stamp_place(
                &project_id,
                "cross-map-session",
                "cross-map-request",
                &source_ref,
                &destinations,
                StampCollisionPolicy::Merge,
            )
            .unwrap();
        store
            .finalize(&project_id, "cross-map-session", "cross-map-request")
            .unwrap();
        let committed = store
            .commit_request(&project_id, "cross-map-session", "cross-map-request")
            .unwrap();
        assert_eq!(committed.current_revision, 1);
        let committed_hash = committed.current_hash.clone();
        let state = store.load_state(&project_id, "cross-map-session").unwrap();
        let manifest: RevisionManifest = read_json(&state.revisions[0].operation_manifest).unwrap();
        assert_eq!(manifest.imported_stamps.len(), 1);
        assert!(manifest
            .batches
            .iter()
            .flatten()
            .all(|operation| !matches!(operation, MapOperation::TerrainIsomBrush { .. })));
        let imported_digest = crate::chk::digest_chk(&isom::chk_extract(&source_path).unwrap());
        let candidate_digest = crate::chk::digest_chk(
            &isom::chk_extract(&store.current_map(&project_id, "cross-map-session").unwrap())
                .unwrap(),
        );
        for (source_x, source_y, destination_x, destination_y) in [
            (0usize, 0usize, 4usize, 4usize),
            (1, 0, 5, 4),
            (0, 1, 4, 5),
            (1, 1, 5, 5),
        ] {
            assert_eq!(
                candidate_digest.tiles
                    [destination_y * usize::from(candidate_digest.map.width) + destination_x],
                imported_digest.tiles[source_y * usize::from(imported_digest.map.width) + source_x],
            );
        }
        store
            .finish_request("cross-map-session", "cross-map-request")
            .unwrap();
        imports
            .remove_test_stamp(&project_id, &imported.id)
            .unwrap();
        store.revert(&project_id, "cross-map-session", 0).unwrap();
        let replayed = store.revert(&project_id, "cross-map-session", 1).unwrap();
        assert_eq!(replayed.current_hash, committed_hash);
        assert!(store
            .direct_stamp_preview(
                &project_id,
                "cross-map-session",
                &replayed.revision_key,
                &source_ref,
                &destinations,
            )
            .unwrap_err()
            .contains("no longer exists"));
        assert_eq!(std::fs::read(&source_path).unwrap(), source_before);
        assert_eq!(
            std::fs::read(&destination_path).unwrap(),
            destination_before
        );
        store.discard(&project_id, "cross-map-session").unwrap();
    }

    #[test]
    fn saved_selection_palette_is_map_persistent_and_delete_is_shared() {
        let root = unique_root();
        let dirs = DataDirs::from_bases(&root.join("roaming"), &root.join("local"));
        dirs.ensure_dirs().unwrap();
        let source = root.join("source.scx");
        std::fs::copy(fixture(), &source).unwrap();
        let snapshot = context(&dirs, &source);
        let store =
            CandidateStore::new((dirs).clone(), crate::map_import::MapImportStore::new(dirs));
        let first = store.create_session("map-session-a", &snapshot).unwrap();
        let mut selection = full_target(&first, "shared-stamp");
        selection.label = "공유 영역".to_string();
        selection.layers = [MapLayer::Terrain, MapLayer::Units].into_iter().collect();
        store
            .save_selection("project", "map-session-a", selection)
            .unwrap();

        let state_path = store
            .session_root("project", "map-session-a")
            .join("state.json");
        let state_before_read = std::fs::read(&state_path).unwrap();
        let persistent = store.persistent_selections("project").unwrap();
        assert_eq!(persistent.len(), 1);
        assert_eq!(persistent[0].id, "shared-stamp");
        assert_eq!(persistent[0].label, "공유 영역");
        assert_eq!(std::fs::read(&state_path).unwrap(), state_before_read);

        let second = store.create_session("map-session-b", &snapshot).unwrap();
        let shared = second
            .selections
            .iter()
            .find(|item| item.selection.id == "shared-stamp")
            .unwrap();
        assert_eq!(shared.selection.label, "공유 영역");
        assert_eq!(shared.selection.source_revision, second.revision_key);
        assert_eq!(
            shared.selection.layers,
            [MapLayer::Terrain, MapLayer::Units].into_iter().collect()
        );
        let authority = store
            .prepare_request(
                "project",
                "map-session-b",
                "stamp-mention",
                second.current_revision,
                &[MapMentionSnapshot::Stamp {
                    selection_id: shared.selection.id.clone(),
                    snapshot_hash: shared.snapshot_hash.clone(),
                }],
            )
            .unwrap();
        assert!(authority.target_masks.is_empty());
        store
            .finish_request("map-session-b", "stamp-mention")
            .unwrap();

        store
            .delete_selection("project", "map-session-b", "shared-stamp")
            .unwrap();
        assert!(store
            .state("project", "map-session-a")
            .unwrap()
            .selections
            .is_empty());
        std::fs::remove_dir_all(root).ok();
    }

    struct IdleStatus;
    impl crate::mapsafe::CompilingStatus for IdleStatus {
        fn is_compiling(&self) -> bool {
            false
        }
    }

    struct UnlockedProbe;
    impl crate::mapsafe::LockProbe for UnlockedProbe {
        fn is_locked(&self, _path: &Path) -> bool {
            false
        }
    }

    /// The properties dialog's identity submission for a digest: every slot
    /// and force as the map currently has it.
    fn properties_from_digest(digest: &crate::chk::Digest) -> MapPropertiesInput {
        MapPropertiesInput {
            title: digest.map.title.clone(),
            description: digest.map.description.clone(),
            players: digest
                .players
                .iter()
                .map(|player| MapPropertyPlayer {
                    r#type: SLOT_TYPES
                        .iter()
                        .find(|(_, id)| *id == player.controller_id)
                        .map(|(name, _)| (*name).to_string())
                        .unwrap_or_else(|| panic!("fixture OWNR {}", player.controller_id)),
                    race: RACES
                        .iter()
                        .find(|(_, id)| *id == player.race_id)
                        .map(|(name, _)| (*name).to_string())
                        .unwrap_or_else(|| panic!("fixture SIDE {}", player.race_id)),
                    force: digest_force(player),
                })
                .collect(),
            forces: digest
                .forces
                .iter()
                .map(|force| MapPropertyForce {
                    name: force.name.clone(),
                    allied: force.flags.allies,
                    allied_victory: force.flags.allied_victory,
                    shared_vision: force.flags.shared_vision,
                    random_start: force.flags.random_start_location,
                })
                .collect(),
        }
    }

    #[test]
    fn properties_stage_writes_only_changed_fields_and_apply_undo_round_trips_the_source() {
        let root = unique_root();
        let dirs = DataDirs::from_bases(&root.join("roaming"), &root.join("local"));
        dirs.ensure_dirs().unwrap();
        let source = root.join("source.scx");
        std::fs::copy(fixture(), &source).unwrap();
        let source_hash = file_hash(&source).unwrap();
        let snapshot = context(&dirs, &source);
        let store =
            CandidateStore::new((dirs).clone(), crate::map_import::MapImportStore::new(dirs));
        let view = store.create_session("map-session", &snapshot).unwrap();
        let identity = properties_from_digest(&snapshot.digest);

        assert_eq!(
            store
                .properties_stage("project", "map-session", &identity)
                .unwrap_err(),
            "변경된 항목이 없습니다."
        );
        let mut short = identity.clone();
        short.players.pop();
        assert!(store
            .properties_stage("project", "map-session", &short)
            .unwrap_err()
            .contains("12개"));
        let mut bad_type = identity.clone();
        bad_type.players[0].r#type = "observer".to_string();
        assert!(store
            .properties_stage("project", "map-session", &bad_type)
            .unwrap_err()
            .contains("observer"));
        let mut stray_force = identity.clone();
        stray_force.players[10].force = Some(0);
        assert!(store
            .properties_stage("project", "map-session", &stray_force)
            .unwrap_err()
            .contains("P11"));
        let mut empty_title = identity.clone();
        empty_title.title.clear();
        assert!(store
            .properties_stage("project", "map-session", &empty_title)
            .unwrap_err()
            .contains("맵 제목"));

        store
            .prepare_request("project", "map-session", "request", 0, &[])
            .unwrap();
        assert!(store
            .properties_stage("project", "map-session", &identity)
            .unwrap_err()
            .contains("진행 중"));
        store.finish_request("map-session", "request").unwrap();

        let mut changed = identity.clone();
        changed.title = "속성 테스트".to_string();
        changed.players[4] = MapPropertyPlayer {
            r#type: "computer".to_string(),
            race: "terran".to_string(),
            force: Some(3),
        };
        changed.forces[1].name = "청팀".to_string();
        changed.forces[1].allied_victory = !changed.forces[1].allied_victory;
        assert_ne!(identity.players[4], changed.players[4]);
        let stage = store
            .properties_stage("project", "map-session", &changed)
            .unwrap();
        assert_eq!(stage.operations, 3);
        assert!(stage.verification.valid, "{:?}", stage.verification.errors);
        assert!(stage
            .verification
            .diff
            .unsupported_section_changes
            .is_empty());
        let slot_changes = u32::from(identity.players[4].r#type != changed.players[4].r#type)
            + u32::from(identity.players[4].race != changed.players[4].race)
            + u32::from(identity.players[4].force != changed.players[4].force);
        assert!(slot_changes >= 1);
        assert_eq!(stage.verification.diff.properties, 3 + slot_changes);
        assert_eq!(stage.verification.diff.terrain_cells, 0);
        assert_eq!(file_hash(&stage.work).unwrap(), stage.work_sha256);
        assert_ne!(stage.work_sha256, source_hash);
        assert_eq!(file_hash(&source).unwrap(), source_hash);
        let staged = crate::chk::digest_chk(&isom::chk_extract(&stage.work).unwrap());
        assert_eq!(staged.map.title, "속성 테스트");
        assert_eq!(staged.map.description, snapshot.digest.map.description);
        assert_eq!(staged.players[4].controller_id, 5);
        assert_eq!(staged.players[4].race_id, 1);
        assert_eq!(staged.players[4].force, Some(4));
        assert_eq!(staged.forces[1].name, "청팀");
        assert_eq!(
            staged.forces[1].flags.allied_victory,
            changed.forces[1].allied_victory
        );
        assert_eq!(staged.tiles, snapshot.digest.tiles);
        assert_eq!(staged.units, snapshot.digest.units);
        assert_eq!(staged.locations, snapshot.digest.locations);

        let safe =
            crate::mapsafe::CandidateMapSafe::new(root.join("backups"), IdleStatus, UnlockedProbe);
        let record = safe
            .apply(
                &source,
                &stage.work,
                &view.baseline.file_sha256,
                &stage.work_sha256,
            )
            .unwrap();
        let applied = store
            .complete_apply("project", "map-session", &record)
            .unwrap();
        safe.complete_pending(&record).unwrap();
        store.discard_properties_stage(&stage).unwrap();
        assert!(!stage.work.exists());
        assert_eq!(applied.current_revision, 0);
        assert!(applied.can_undo);
        assert!(!applied.can_apply);
        assert_eq!(applied.baseline.file_sha256, stage.work_sha256);
        assert_eq!(file_hash(&source).unwrap(), stage.work_sha256);
        let saved = crate::chk::digest_chk(&isom::chk_extract(&source).unwrap());
        assert_eq!(saved.map.title, "속성 테스트");
        assert_eq!(saved.forces[1].name, "청팀");
        assert_eq!(
            store
                .properties_stage("project", "map-session", &changed)
                .unwrap_err(),
            "변경된 항목이 없습니다."
        );

        let undo = store.last_apply_record("project", "map-session").unwrap();
        safe.undo(&undo).unwrap();
        let restored = store.complete_undo("project", "map-session").unwrap();
        assert!(!restored.can_undo);
        assert_eq!(file_hash(&source).unwrap(), source_hash);
        assert_eq!(restored.baseline.file_sha256, source_hash);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn independent_race_round_trips_through_the_properties_vocabulary() {
        // The panel sends an "independent" (SIDE 3) slot back unchanged; the
        // request must accept it and emit no operation for it.
        let mut digest = crate::chk::digest_chk(&isom::chk_extract(&fixture()).unwrap());
        digest.players[6].race_id = 3;
        digest.players[6].race = "Independent".to_string();
        let identity = properties_from_digest(&digest);
        assert_eq!(identity.players[6].race, "independent");
        let encoded = EncodedProperties::validate(&identity, false).unwrap();
        assert_eq!(encoded.players[6].race_id, 3);
        assert!(encoded.operations(&digest).is_empty());
        encoded.require_exact(&digest).unwrap();

        let mut changed = identity.clone();
        changed.players[6].race = "terran".to_string();
        let operations = EncodedProperties::validate(&changed, false)
            .unwrap()
            .operations(&digest);
        assert_eq!(
            operations,
            vec![MapOperation::PlayerSet {
                slot: 6,
                r#type: None,
                race: Some("terran".to_string()),
                force: None,
            }]
        );
    }

    #[test]
    fn properties_stage_refuses_while_a_candidate_revision_is_visible() {
        let root = unique_root();
        let dirs = DataDirs::from_bases(&root.join("roaming"), &root.join("local"));
        dirs.ensure_dirs().unwrap();
        let source = root.join("source.scx");
        std::fs::copy(fixture(), &source).unwrap();
        let snapshot = context(&dirs, &source);
        let store =
            CandidateStore::new((dirs).clone(), crate::map_import::MapImportStore::new(dirs));
        store.create_session("map-session", &snapshot).unwrap();
        store
            .prepare_request("project", "map-session", "request", 0, &[])
            .unwrap();
        store
            .draft_begin("project", "map-session", "request")
            .unwrap();
        store
            .draft_patch("project", "map-session", "request", vec![add_unit(64, 64)])
            .unwrap();
        store.finalize("project", "map-session", "request").unwrap();
        store
            .commit_request("project", "map-session", "request")
            .unwrap();
        store.finish_request("map-session", "request").unwrap();

        let mut changed = properties_from_digest(&snapshot.digest);
        changed.title = "후보 있음".to_string();
        let error = store
            .properties_stage("project", "map-session", &changed)
            .unwrap_err();
        assert!(error.contains("r1"), "{error}");
        assert!(error.contains("적용하거나 폐기"), "{error}");
        std::fs::remove_dir_all(root).ok();
    }
}
