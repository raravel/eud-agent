use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::DataDirs;
use crate::map_context::{MapContextService, MapContextSnapshot};
use crate::map_image::MapImageConversionMetadata;
use crate::map_model::{
    hex_sha256, CandidateRevision, CandidateSession, MapEditBatch, MapEditExpected, MapLayer,
    MapMentionSnapshot, MapObjectKind, MapOperation, SelectionMask, SelectionRole,
    VerificationReport, MAP_EDIT_SCHEMA,
};
use crate::map_stamp::{
    compile_stamp_placement, PersistentSelection, PersistentSelectionLibrary, StampCollisionPolicy,
    StampDestination, StampPlacementReport, StampPlacementResult,
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
    pending_revision: Option<PendingRevision>,
    finalized: bool,
}

#[derive(Clone)]
struct PendingRevision {
    revision: CandidateRevision,
    object_ids: BTreeMap<String, String>,
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
    pub can_apply: bool,
    pub can_undo: bool,
}

impl CandidateStore {
    pub fn new(dirs: DataDirs) -> Self {
        Self {
            inner: Arc::new(CandidateStoreInner {
                context: MapContextService::new(dirs.clone()),
                dirs,
                verifier: MapVerificationService,
                active: Mutex::new(HashMap::new()),
                selection_palette: Mutex::new(()),
            }),
        }
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
                    if !entry
                        .file_type()
                        .map_err(|error| error.to_string())?
                        .is_file()
                    {
                        continue;
                    }
                    let name = entry.file_name().to_string_lossy().to_string();
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
        state.stale = file_hash(&context.revision.source_path)? != state.baseline.file_sha256;
        self.save_state(&state)?;
        self.view(&state)
    }

    pub fn state(&self, project_id: &str, session_id: &str) -> Result<CandidateStateView, String> {
        let mut state = self.load_state(project_id, session_id)?;
        state.stale = file_hash(&state.baseline.source_path)? != state.baseline.file_sha256;
        if state.stale {
            self.save_state(&state)?;
        }
        self.view(&state)
    }

    pub fn save_selection(
        &self,
        project_id: &str,
        session_id: &str,
        selection: SelectionMask,
    ) -> Result<CandidateStateView, String> {
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
        let state = self.load_state(project_id, session_id)?;
        if state.stale || file_hash(&state.baseline.source_path)? != state.baseline.file_sha256 {
            return Err(
                "candidate source is stale; discard it and reopen from the saved source"
                    .to_string(),
            );
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
        let active = ActiveRequest {
            request_id: request_id.to_string(),
            parent_revision,
            parent_hash: current_hash,
            authority: authority.clone(),
            draft_path: None,
            batches: Vec::new(),
            reports: Vec::new(),
            image_conversions: Vec::new(),
            pending_revision: None,
            finalized: false,
        };
        let mut requests = self.inner.active.lock();
        if requests.contains_key(session_id) {
            return Err("another map request is already active for this session".to_string());
        }
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
        let state = self.load_state(project_id, session_id)?;
        if state.stale || file_hash(&state.baseline.source_path)? != state.baseline.file_sha256 {
            return Err(
                "candidate source is stale; discard it and reopen from the saved source"
                    .to_string(),
            );
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

    pub fn direct_stamp_preview(
        &self,
        project_id: &str,
        session_id: &str,
        expected_revision_key: &str,
        selection_id: &str,
        destinations: &[StampDestination],
    ) -> Result<StampPlacementReport, String> {
        let authority = self.direct_terrain_authority(
            project_id,
            session_id,
            "direct-stamp-preview",
            expected_revision_key,
        )?;
        let state = self.load_state(project_id, session_id)?;
        let selection = state
            .selections
            .get(selection_id)
            .ok_or_else(|| "stamp selection does not exist".to_string())?;
        Ok(compile_stamp_placement(
            &state.current_map,
            &state.current_map,
            &self.inner.context.starcraft_path()?,
            selection,
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
        selection_id: &str,
        destinations: &[StampDestination],
    ) -> Result<StampPlacementReport, String> {
        let (source, draft, selection, authority) =
            self.stamp_request_context(project_id, session_id, request_id, selection_id)?;
        Ok(compile_stamp_placement(
            &source,
            &draft,
            &self.inner.context.starcraft_path()?,
            &selection,
            destinations,
            None,
            &authority,
        )?
        .report)
    }

    pub fn draft_stamp_place(
        &self,
        project_id: &str,
        session_id: &str,
        request_id: &str,
        selection_id: &str,
        destinations: &[StampDestination],
        policy: StampCollisionPolicy,
    ) -> Result<StampPlacementResult, String> {
        let (source, draft, selection, authority) =
            self.stamp_request_context(project_id, session_id, request_id, selection_id)?;
        let compiled = compile_stamp_placement(
            &source,
            &draft,
            &self.inner.context.starcraft_path()?,
            &selection,
            destinations,
            Some(policy),
            &authority,
        )?;
        let patch = self.draft_patch(project_id, session_id, request_id, compiled.operations)?;
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
        selection_id: &str,
    ) -> Result<(PathBuf, PathBuf, SelectionMask, MapRequestAuthority), String> {
        let state = self.load_state(project_id, session_id)?;
        let selection = state
            .selections
            .get(selection_id)
            .cloned()
            .ok_or_else(|| "stamp selection does not exist".to_string())?;
        let active = self.inner.active.lock();
        let request = active_request(&active, session_id, request_id)?;
        let draft = request
            .draft_path
            .clone()
            .ok_or_else(|| "call map_draft_begin before placing a stamp".to_string())?;
        Ok((
            state.current_map,
            draft,
            selection,
            request.authority.clone(),
        ))
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
        self.view(&preview)
    }

    pub fn commit_request(
        &self,
        project_id: &str,
        session_id: &str,
        request_id: &str,
    ) -> Result<CandidateStateView, String> {
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
        active.remove(session_id);
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
        let state = self.load_state(project_id, session_id)?;
        if state.current_revision == 0 {
            return Err("there is no candidate revision to Apply".to_string());
        }
        if file_hash(&state.baseline.source_path)? != state.baseline.file_sha256 {
            return Err("candidate source hash is stale".to_string());
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

    pub fn complete_apply(
        &self,
        project_id: &str,
        session_id: &str,
        record: &crate::mapsafe::CandidateApplyRecord,
    ) -> Result<CandidateStateView, String> {
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
        state.last_apply_backup = Some(record.backup_path.clone());
        state.last_apply_source_hash = Some(record.applied_sha256.clone());
        state.last_apply_before_hash = Some(record.before_sha256.clone());
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
        state.last_apply_backup = None;
        state.last_apply_source_hash = None;
        state.last_apply_before_hash = None;
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
            can_apply: state.current_revision > 0 && !state.stale && valid,
            can_undo: state.last_apply_backup.is_some(),
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
    match std::fs::remove_file(path) {
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
        let store = CandidateStore::new(dirs);
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
        let store = CandidateStore::new(dirs);
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
        let store = CandidateStore::new(dirs);
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
        let store = CandidateStore::new(dirs);
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
        let store = CandidateStore::new(dirs);
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
        assert_eq!(file_hash(&source).unwrap(), source_hash);
        store.finish_request("map-session", "request").unwrap();
        assert!(!draft_path.exists());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn strict_candidate_session_create_and_open_fail_closed() {
        let root = unique_root();
        let dirs = DataDirs::from_bases(&root.join("roaming"), &root.join("local"));
        dirs.ensure_dirs().unwrap();
        let source = root.join("source.scx");
        std::fs::copy(fixture(), &source).unwrap();
        let snapshot = context(&dirs, &source);
        let store = CandidateStore::new(dirs.clone());
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
        let store = CandidateStore::new(dirs.clone());
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

        let recovered = CandidateStore::new(dirs.clone());
        recovered.cleanup_startup().unwrap();
        let recovered_view = recovered.open_session("map-session", &snapshot).unwrap();
        assert_eq!(recovered_view.current_revision, 1);
        assert_eq!(recovered_view.current_hash, repaired_hash);
        assert_eq!(
            recovered.object_ids("project", "map-session").unwrap(),
            r1_object_ids
        );
        assert!(!orphan.exists());

        let mut source_bytes = std::fs::read(&source).unwrap();
        source_bytes.push(0);
        std::fs::write(&source, source_bytes).unwrap();
        let stale = recovered.open_session("map-session", &snapshot).unwrap();
        assert!(stale.stale);
        assert!(!stale.can_apply);
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
        let store = CandidateStore::new(dirs.clone());
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
        let store = CandidateStore::new(dirs.clone());
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
        let store = CandidateStore::new(dirs.clone());
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
        let store = CandidateStore::new(dirs);
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
        let store = CandidateStore::new(dirs);
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
                "terrain-stamp",
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
                "terrain-stamp",
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
    fn saved_selection_palette_is_map_persistent_and_delete_is_shared() {
        let root = unique_root();
        let dirs = DataDirs::from_bases(&root.join("roaming"), &root.join("local"));
        dirs.ensure_dirs().unwrap();
        let source = root.join("source.scx");
        std::fs::copy(fixture(), &source).unwrap();
        let snapshot = context(&dirs, &source);
        let store = CandidateStore::new(dirs);
        let first = store.create_session("map-session-a", &snapshot).unwrap();
        let mut selection = full_target(&first, "shared-stamp");
        selection.label = "공유 영역".to_string();
        selection.layers = [MapLayer::Terrain, MapLayer::Units].into_iter().collect();
        store
            .save_selection("project", "map-session-a", selection)
            .unwrap();

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
}
