use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine as _;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{Emitter, Manager};

use crate::attachment::AttachmentStore;
use crate::config::DataDirs;
use crate::map_candidate::{CandidateStateView, CandidateStore, MapPropertiesInput};
use crate::map_image::{
    MapImageConversionReport, MapImageDescriptor, MapImageMapContext, MapImagePlacement,
    MapImageService,
};
use crate::map_import::MapStampSourceRef;
use crate::map_model::{MapLayer, MapMentionSnapshot, RowSpan, SelectionMask, TileRect};
use crate::map_stamp::{
    StampCollisionPolicy, StampDestination, StampPlacementReport, StampPlacementResult,
};
use crate::mapsafe::{CandidateMapSafe, CompilingStatus, WindowsLockProbe};

pub(crate) const MAP_WINDOW_LABEL: &str = "map-agent";

/// What withdrew a team session's candidate (see `team_tasks_withdrawn`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TeamWithdrawal {
    Discard,
    /// A revert to this revision: only a ready candidate above it ends.
    Revert(u32),
    /// An undo of the apply that produced this source hash: only the task
    /// applied as that output ends with the ready candidate (the user's own
    /// apply in the same session has no task).
    Undo {
        applied_sha256: Option<String>,
    },
    SessionDeleted,
}
const OBJECT_SNAPSHOT_CACHE_CAPACITY: usize = 4;

#[derive(Default)]
struct ObjectSnapshotCache {
    entries: HashMap<String, Arc<crate::tool_exec::MapObjectSnapshot>>,
    order: VecDeque<String>,
}

impl ObjectSnapshotCache {
    fn get(&mut self, key: &str) -> Option<Arc<crate::tool_exec::MapObjectSnapshot>> {
        let snapshot = self.entries.get(key)?.clone();
        self.order.retain(|entry| entry != key);
        self.order.push_back(key.to_string());
        Some(snapshot)
    }

    fn insert(&mut self, key: String, snapshot: Arc<crate::tool_exec::MapObjectSnapshot>) {
        self.entries.insert(key.clone(), snapshot);
        self.order.retain(|entry| entry != &key);
        self.order.push_back(key);
        while self.order.len() > OBJECT_SNAPSHOT_CACHE_CAPACITY {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
    }
}

#[derive(Clone)]
pub struct MapAgentService {
    dirs: DataDirs,
    candidates: CandidateStore,
    sessions: crate::session::SessionStore,
    writes: crate::write_coordinator::ProjectWriteCoordinator,
    safe: Arc<CandidateMapSafe<MapCompilingStatus, WindowsLockProbe>>,
    object_snapshots: Arc<Mutex<ObjectSnapshotCache>>,
    attachments: AttachmentStore,
    images: MapImageService,
    /// The Map session the next window bootstrap should load instead of the
    /// default resolution; set by `open_map_window` when it creates the window.
    pending_open_session: Arc<Mutex<Option<String>>>,
    /// Set when the main window's header asked for the "맵 속성" dialog and the
    /// Map window had to be created first; the next bootstrap takes it.
    pending_open_properties: Arc<Mutex<bool>>,
}

#[derive(Clone, Copy)]
enum CandidateSessionAction {
    Create,
    Open,
}

struct MapSessionResolution {
    session: crate::session::SessionRecord,
    candidate_action: CandidateSessionAction,
}

#[derive(Clone)]
struct MapCompilingStatus {
    dirs: DataDirs,
}

impl CompilingStatus for MapCompilingStatus {
    fn is_compiling(&self) -> bool {
        crate::native_runtime::NativeProjectManager::new(self.dirs.clone()).is_building()
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapBootstrapResponse {
    pub context: crate::map_context::MapContextSnapshot,
    pub candidate: CandidateStateView,
    pub session: crate::session::SessionRecord,
    /// Set when the persisted provider conversation could not be resumed; the
    /// window still opens, and chat stays blocked until an explicit reset.
    pub conversation_resume_error: Option<String>,
    /// The session's latest Map run (in flight, or ended after the window last
    /// saw it) so the window shows it like a request typed there.
    pub pending_run: Option<crate::engine::MapRunTranscript>,
    /// Set when this window was opened by the main window's "맵 속성" header
    /// action, so it shows the properties dialog as soon as it is ready.
    pub open_properties: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapDiffDetails {
    pub terrain_rows: Vec<RowSpan>,
    pub markers: Vec<MapDiffMarker>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapDiffMarker {
    pub layer: MapLayer,
    pub change: &'static str,
    pub ordinal: usize,
    pub bounds: TileRect,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapRenderCommand {
    pub session_id: String,
    pub view: MapView,
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    pub scale: u8,
    pub layers: Vec<String>,
    #[serde(default)]
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MapView {
    Original,
    Candidate,
    Diff,
    Draft,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapCatalogCommand {
    pub session_id: String,
    pub kind: String,
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub offset: u32,
    #[serde(default = "catalog_limit")]
    pub limit: u16,
    /// Tiles only: drop the null (megatile 0) variants from the page.
    #[serde(default)]
    pub hide_null_tiles: bool,
}

const fn catalog_limit() -> u16 {
    100
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapObjectsCommand {
    pub session_id: String,
    pub layer: String,
    #[serde(default)]
    pub view: Option<MapView>,
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub draft_generation: Option<u32>,
    #[serde(default)]
    pub offset: u32,
    #[serde(default = "catalog_limit")]
    pub limit: u16,
}

struct MapObjectSource {
    map: PathBuf,
    project_id: String,
    baseline_hash: String,
    revision_key: String,
    annotate_candidate_ids: bool,
    cache: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapThumbnailCommand {
    pub session_id: String,
    pub layer: String,
    pub id: u32,
    #[serde(default)]
    pub owner: u8,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapChatCommand {
    pub session_id: String,
    pub text: String,
    pub attachments: Vec<String>,
    pub candidate_revision: u32,
    pub mentions: Vec<MapMentionSnapshot>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapImagePreviewCommand {
    pub session_id: String,
    pub attachment_id: String,
    pub revision_key: String,
    pub placement: MapImagePlacement,
    pub preview_sequence: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapImageConfirmCommand {
    pub session_id: String,
    pub attachment_id: String,
    pub revision_key: String,
    pub placement: MapImagePlacement,
    pub preview_digest: String,
    pub preview_sequence: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapStampPreviewCommand {
    pub session_id: String,
    pub revision_key: String,
    pub source: MapStampSourceRef,
    pub destinations: Vec<StampDestination>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapStampConfirmCommand {
    pub session_id: String,
    pub revision_key: String,
    pub source: MapStampSourceRef,
    pub destinations: Vec<StampDestination>,
    pub collision_policy: StampCollisionPolicy,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapStampConfirmResponse {
    pub candidate: CandidateStateView,
    pub report: StampPlacementReport,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapImagePreviewHeader {
    pub preview_sequence: u64,
    pub descriptor: MapImageDescriptor,
    pub report: MapImageConversionReport,
    pub png_byte_length: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapImageConfirmResponse {
    pub preview_sequence: u64,
    pub candidate: CandidateStateView,
    pub report: MapImageConversionReport,
}

/// The Map window's "맵 속성" save: the complete property set for the
/// session's source map, written through the same MapSafe rails as Apply.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapPropertiesSaveCommand {
    pub session_id: String,
    pub properties: MapPropertiesInput,
}

impl MapAgentService {
    pub fn new(
        dirs: DataDirs,
        candidates: CandidateStore,
        writes: crate::write_coordinator::ProjectWriteCoordinator,
    ) -> Self {
        let sessions = crate::session::SessionStore::new(&dirs);
        let safe = Arc::new(CandidateMapSafe::new(
            dirs.map_backups_dir(),
            MapCompilingStatus { dirs: dirs.clone() },
            WindowsLockProbe,
        ));
        let attachments = AttachmentStore::new(dirs.attachments_dir());
        Self {
            dirs,
            candidates,
            sessions,
            writes,
            safe,
            object_snapshots: Arc::new(Mutex::new(ObjectSnapshotCache::default())),
            attachments,
            images: MapImageService::new(),
            pending_open_session: Arc::new(Mutex::new(None)),
            pending_open_properties: Arc::new(Mutex::new(false)),
        }
    }

    pub fn candidates(&self) -> CandidateStore {
        self.candidates.clone()
    }

    fn image_preview(
        &self,
        command: &MapImagePreviewCommand,
    ) -> Result<(MapImagePreviewHeader, Vec<u8>), String> {
        let session = self.session_record(&command.session_id)?;
        let state = self
            .candidates
            .state(&session.meta.project, &command.session_id)?;
        let authority = self.candidates.direct_terrain_authority(
            &session.meta.project,
            &command.session_id,
            &format!("direct-preview-{}", command.preview_sequence),
            &command.revision_key,
        )?;
        let attachment = self
            .attachments
            .bind_and_resolve_image(&command.attachment_id, &command.session_id)?;
        let descriptor = self.images.describe(&command.session_id, &attachment)?;
        let map = self
            .candidates
            .current_map(&session.meta.project, &command.session_id)?;
        let starcraft_path = self.candidates.context().starcraft_path()?;
        let conversion = self.images.convert(
            &command.session_id,
            &attachment,
            command.placement,
            MapImageMapContext {
                map_path: &map,
                revision: &state.baseline,
                authority: &authority,
                starcraft_path: &starcraft_path,
            },
        )?;
        let png_byte_length = u32::try_from(conversion.preview_png.len())
            .map_err(|_| "image preview PNG length exceeds u32".to_string())?;
        Ok((
            MapImagePreviewHeader {
                preview_sequence: command.preview_sequence,
                descriptor,
                report: conversion.report,
                png_byte_length,
            },
            conversion.preview_png,
        ))
    }

    fn image_confirm(
        &self,
        command: &MapImageConfirmCommand,
    ) -> Result<MapImageConfirmResponse, String> {
        let session = self.session_record(&command.session_id)?;
        let state = self
            .candidates
            .state(&session.meta.project, &command.session_id)?;
        self.candidates.direct_terrain_authority(
            &session.meta.project,
            &command.session_id,
            "direct-confirm-probe",
            &command.revision_key,
        )?;
        let attachment = self
            .attachments
            .bind_and_resolve_image(&command.attachment_id, &command.session_id)?;
        let request_id = format!("direct-image-{}", uuid::Uuid::new_v4());
        self.candidates.prepare_request(
            &session.meta.project,
            &command.session_id,
            &request_id,
            state.current_revision,
            &[],
        )?;
        let result = (|| {
            self.candidates
                .draft_begin(&session.meta.project, &command.session_id, &request_id)?;
            let (authority, expected_revision, draft) = self.candidates.image_request_context(
                &session.meta.project,
                &command.session_id,
                &request_id,
            )?;
            let starcraft_path = self.candidates.context().starcraft_path()?;
            let conversion = self.images.convert(
                &command.session_id,
                &attachment,
                command.placement,
                MapImageMapContext {
                    map_path: &draft,
                    revision: &expected_revision,
                    authority: &authority,
                    starcraft_path: &starcraft_path,
                },
            )?;
            if conversion.report.tile_grid_sha256 != command.preview_digest {
                return Err(
                    "image placement preview is stale; wait for the latest preview".to_string(),
                );
            }
            if conversion.report.protected_conflicts != 0 {
                return Err(format!(
                    "image placement changes {} persistently protected terrain cell(s)",
                    conversion.report.protected_conflicts
                ));
            }
            if conversion.report.outside_authority_conflicts != 0 {
                return Err(format!(
                    "image placement changes {} cell(s) outside the current terrain authority",
                    conversion.report.outside_authority_conflicts
                ));
            }
            let report = conversion.report.clone();
            self.candidates.draft_patch_image(
                &session.meta.project,
                &command.session_id,
                &request_id,
                conversion.operation,
                conversion.metadata,
            )?;
            self.candidates
                .finalize(&session.meta.project, &command.session_id, &request_id)?;
            let candidate = self.candidates.commit_request(
                &session.meta.project,
                &command.session_id,
                &request_id,
            )?;
            Ok(MapImageConfirmResponse {
                preview_sequence: command.preview_sequence,
                candidate,
                report,
            })
        })();
        let cleanup = self
            .candidates
            .finish_request(&command.session_id, &request_id);
        match (result, cleanup) {
            (Ok(response), Ok(())) => {
                self.images.clear_session(&command.session_id);
                Ok(response)
            }
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
        }
    }

    fn stamp_preview(
        &self,
        command: &MapStampPreviewCommand,
    ) -> Result<StampPlacementReport, String> {
        let session = self.session_record(&command.session_id)?;
        let context = self.candidates.context().current()?;
        let state = self
            .candidates
            .state(&session.meta.project, &command.session_id)?;
        require_current_source(
            &context.revision.project_id,
            &context.revision.source_path,
            &session.meta.project,
            &state.baseline.source_path,
            "stamp preview",
        )?;
        self.candidates.direct_stamp_preview(
            &session.meta.project,
            &command.session_id,
            &command.revision_key,
            &command.source,
            &command.destinations,
        )
    }

    fn stamp_confirm(
        &self,
        command: &MapStampConfirmCommand,
    ) -> Result<MapStampConfirmResponse, String> {
        let session = self.session_record(&command.session_id)?;
        let state = self
            .candidates
            .state(&session.meta.project, &command.session_id)?;
        self.candidates.direct_stamp_preview(
            &session.meta.project,
            &command.session_id,
            &command.revision_key,
            &command.source,
            &command.destinations,
        )?;
        let request_id = format!("direct-stamp-{}", uuid::Uuid::new_v4());
        let imported_mention = match &command.source {
            MapStampSourceRef::Imported {
                import_id,
                snapshot_hash,
            } => vec![MapMentionSnapshot::ImportedStamp {
                import_id: import_id.clone(),
                snapshot_hash: snapshot_hash.clone(),
            }],
            MapStampSourceRef::CandidateSelection { .. } => Vec::new(),
        };
        self.candidates.prepare_request(
            &session.meta.project,
            &command.session_id,
            &request_id,
            state.current_revision,
            &imported_mention,
        )?;
        let result = (|| {
            self.candidates
                .draft_begin(&session.meta.project, &command.session_id, &request_id)?;
            let StampPlacementResult { report, .. } = self.candidates.draft_stamp_place(
                &session.meta.project,
                &command.session_id,
                &request_id,
                &command.source,
                &command.destinations,
                command.collision_policy,
            )?;
            self.candidates
                .finalize(&session.meta.project, &command.session_id, &request_id)?;
            let candidate = self.candidates.commit_request(
                &session.meta.project,
                &command.session_id,
                &request_id,
            )?;
            Ok(MapStampConfirmResponse { candidate, report })
        })();
        let cleanup = self
            .candidates
            .finish_request(&command.session_id, &request_id);
        match (result, cleanup) {
            (Ok(response), Ok(())) => Ok(response),
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
        }
    }

    fn map_session(
        &self,
        context: &crate::map_context::MapContextSnapshot,
    ) -> Result<MapSessionResolution, String> {
        let project_id = &context.revision.project_id;
        let mut unbound = None;
        for session in self
            .sessions
            .list_kind(crate::session::SessionKind::Map)
            .map_err(|error| error.to_string())?
            .into_iter()
            .filter(|session| session.project == *project_id)
        {
            match self.candidates.session_source(project_id, &session.id)? {
                Some(source) if source == context.revision.source_path => {
                    let session = self
                        .sessions
                        .load(&session.id)
                        .map_err(|error| error.to_string())?;
                    return Ok(MapSessionResolution {
                        session,
                        candidate_action: CandidateSessionAction::Open,
                    });
                }
                None if unbound.is_none() => unbound = Some(session),
                Some(_) | None => {}
            }
        }
        if let Some(session) = unbound {
            let session = self
                .sessions
                .load(&session.id)
                .map_err(|error| error.to_string())?;
            return Ok(MapSessionResolution {
                session,
                candidate_action: CandidateSessionAction::Create,
            });
        }
        Ok(MapSessionResolution {
            session: self.create_map_session(context)?,
            candidate_action: CandidateSessionAction::Create,
        })
    }

    fn map_sessions(
        &self,
        context: &crate::map_context::MapContextSnapshot,
    ) -> Result<Vec<crate::session::SessionMeta>, String> {
        let project_id = &context.revision.project_id;
        let mut sessions = Vec::new();
        for session in self
            .sessions
            .list_kind(crate::session::SessionKind::Map)
            .map_err(|error| error.to_string())?
            .into_iter()
            .filter(|session| session.project == *project_id)
        {
            match self.candidates.session_source(project_id, &session.id)? {
                Some(source) if source == context.revision.source_path => sessions.push(session),
                None => sessions.push(session),
                Some(_) => {}
            }
        }
        Ok(sessions)
    }

    fn create_map_session(
        &self,
        context: &crate::map_context::MapContextSnapshot,
    ) -> Result<crate::session::SessionRecord, String> {
        let sessions = self.map_sessions(context)?;
        let created_at = crate::session::now_unix_seconds();
        let config = self.dirs.load_config().map_err(|error| error.to_string())?;
        let provider_binding = crate::provider::default_binding(&config)?;
        let record = crate::session::SessionRecord {
            meta: crate::session::SessionMeta {
                id: crate::session::new_session_id(),
                name: next_map_session_name(&sessions),
                project: context.revision.project_id.clone(),
                kind: crate::session::SessionKind::Map,
                provider: provider_binding.provider,
                model: provider_binding.model.clone(),
                created_at,
                last_conversation_at: crate::session::now_unix_millis(),
                team_parent: None,
            },
            provider_binding,
            pending_request_ids: Vec::new(),
            context_usage: None,
            panel_log: serde_json::Value::Null,
            context_state: Default::default(),
            task_state: Default::default(),
            autonomous_run: None,
            team_tasks: Vec::new(),
        };
        self.sessions
            .save(&record)
            .map_err(|error| error.to_string())?;
        Ok(record)
    }

    fn session_record(&self, session_id: &str) -> Result<crate::session::SessionRecord, String> {
        let record = self
            .sessions
            .load(session_id)
            .map_err(|error| error.to_string())?;
        if record.meta.kind != crate::session::SessionKind::Map {
            return Err("the requested session is not a Map Agent session".to_string());
        }
        Ok(record)
    }

    /// The session the Map window should bootstrap: the one `open_map_window`
    /// targeted when it is still a valid session of the current source map,
    /// otherwise the ordinary resolution. A team session whose task is still
    /// running is a valid target: the bootstrap never waits on that turn and
    /// the window adopts the run from its transcript.
    fn bootstrap_session(
        &self,
        context: &crate::map_context::MapContextSnapshot,
    ) -> Result<MapSessionResolution, String> {
        let requested = self.pending_open_session.lock().take();
        if let Some(session_id) = requested {
            match self.session_for_context(&session_id, context) {
                Ok(resolution) => return Ok(resolution),
                Err(error) => {
                    eprintln!(
                        "eud-agent: map window target {session_id} ignored: {error}; opening the default session"
                    );
                }
            }
        }
        self.map_session(context)
    }

    /// Resolve a Map session the window may load: it must belong to the
    /// current project and, when it has a candidate, to the current source map.
    fn session_for_context(
        &self,
        session_id: &str,
        context: &crate::map_context::MapContextSnapshot,
    ) -> Result<MapSessionResolution, String> {
        let session = self.session_record(session_id)?;
        if session.meta.project != context.revision.project_id {
            return Err("the requested Map Agent session belongs to another project".to_string());
        }
        let candidate_action = match self
            .candidates
            .session_source(&session.meta.project, session_id)?
        {
            Some(source) if source == context.revision.source_path => CandidateSessionAction::Open,
            Some(_) => {
                return Err(
                    "the requested Map Agent session belongs to another source map".to_string(),
                )
            }
            None => CandidateSessionAction::Create,
        };
        Ok(MapSessionResolution {
            session,
            candidate_action,
        })
    }

    fn map_path_for_render(&self, command: &MapRenderCommand) -> Result<PathBuf, String> {
        let session = self.session_record(&command.session_id)?;
        match command.view {
            MapView::Original => self
                .candidates
                .baseline_map(&session.meta.project, &command.session_id),
            MapView::Candidate | MapView::Diff => self
                .candidates
                .current_map(&session.meta.project, &command.session_id),
            MapView::Draft => self.candidates.draft_map(
                &command.session_id,
                command
                    .request_id
                    .as_deref()
                    .ok_or_else(|| "draft render requires requestId".to_string())?,
            ),
        }
    }

    pub fn render_rgba(&self, command: &MapRenderCommand) -> Result<isom::RgbaImage, String> {
        let session = self.session_record(&command.session_id)?;
        let state = self
            .candidates
            .state(&session.meta.project, &command.session_id)?;
        if command.width == 0
            || command.height == 0
            || command.x.saturating_add(command.width) > state.baseline.width
            || command.y.saturating_add(command.height) > state.baseline.height
        {
            return Err("render crop is outside map dimensions".to_string());
        }
        let request = json!({
            "schema": "eud-map-render/1",
            "mode": "region",
            "x": command.x,
            "y": command.y,
            "width": command.width,
            "height": command.height,
            "scale": command.scale,
            "layers": command.layers,
        });
        isom::render_region(
            &self.map_path_for_render(command)?,
            &self.candidates.context().starcraft_path()?,
            request.to_string().as_bytes(),
        )
        .map_err(|error| format!("map render failed: {error}"))
    }

    pub fn catalog(&self, command: &MapCatalogCommand) -> Result<Value, String> {
        if command.limit == 0 || command.limit > 512 {
            return Err("catalog limit must be 1..512".to_string());
        }
        let session = self.session_record(&command.session_id)?;
        let state = self
            .candidates
            .state(&session.meta.project, &command.session_id)?;
        let request = json!({
            "schema": "eud-map-catalog/1",
            "kind": command.kind,
            "tileset": state.baseline.tileset.era(),
            "offset": command.offset,
            "limit": command.limit,
            "query": command.query,
            "filter": if command.kind == "tiles" && command.hide_null_tiles {
                json!({"graphicsValid": true, "nullTile": false})
            } else if command.kind == "tiles" {
                json!({"graphicsValid": true})
            } else {
                json!({})
            },
        });
        let result = isom::catalog_query(
            &self.candidates.context().starcraft_path()?,
            request.to_string().as_bytes(),
        )
        .map_err(|error| format!("map catalog failed: {error}"))?;
        serde_json::from_str(&result)
            .map_err(|error| format!("map catalog response is invalid: {error}"))
    }

    fn object_source(&self, command: &MapObjectsCommand) -> Result<MapObjectSource, String> {
        let session = self.session_record(&command.session_id)?;
        let state = self
            .candidates
            .state(&session.meta.project, &command.session_id)?;
        match command.view.unwrap_or(MapView::Candidate) {
            MapView::Candidate | MapView::Diff => Ok(MapObjectSource {
                map: self
                    .candidates
                    .current_map(&session.meta.project, &command.session_id)?,
                revision_key: state.revision_key,
                annotate_candidate_ids: true,
                project_id: session.meta.project.clone(),
                baseline_hash: state.baseline.file_sha256.clone(),
                cache: true,
            }),
            MapView::Original => Ok(MapObjectSource {
                map: self
                    .candidates
                    .baseline_map(&session.meta.project, &command.session_id)?,
                revision_key: format!("r0:{}", state.baseline.file_sha256),
                annotate_candidate_ids: false,
                cache: true,
                project_id: session.meta.project.clone(),
                baseline_hash: state.baseline.file_sha256.clone(),
            }),
            MapView::Draft => {
                let request_id = command
                    .request_id
                    .as_deref()
                    .ok_or_else(|| "draft objects require requestId".to_string())?;
                let generation = command
                    .draft_generation
                    .ok_or_else(|| "draft objects require draftGeneration".to_string())?;
                Ok(MapObjectSource {
                    map: self.candidates.draft_map(&command.session_id, request_id)?,
                    revision_key: format!(
                        "{}:draft:{request_id}:g{generation}",
                        state.revision_key
                    ),
                    annotate_candidate_ids: false,
                    cache: true,
                    project_id: session.meta.project.clone(),
                    baseline_hash: state.baseline.file_sha256.clone(),
                })
            }
        }
    }

    pub fn objects(&self, command: &MapObjectsCommand) -> Result<Value, String> {
        if command.limit == 0 || command.limit > 500 {
            return Err("object page limit must be 1..500".to_string());
        }
        if !matches!(
            command.layer.as_str(),
            "units" | "buildings" | "doodads" | "sprites" | "locations"
        ) {
            return Err(format!("unsupported map object layer '{}'", command.layer));
        }
        let source = self.object_source(command)?;
        let snapshot = if source.cache {
            // The baseline hash is part of every object ref, and a diverged
            // session changes it without changing the revision key.
            let cache_key = format!(
                "{}|{}|{}",
                command.session_id, source.revision_key, source.baseline_hash
            );
            let mut cache = self.object_snapshots.lock();
            if let Some(snapshot) = cache.get(&cache_key) {
                snapshot
            } else {
                let snapshot = Arc::new(crate::tool_exec::map_object_snapshot(
                    &source.map,
                    &self.candidates.context().starcraft_path()?,
                    &source.revision_key,
                    &source.baseline_hash,
                )?);
                cache.insert(cache_key, snapshot.clone());
                snapshot
            }
        } else {
            Arc::new(crate::tool_exec::map_object_snapshot(
                &source.map,
                &self.candidates.context().starcraft_path()?,
                &source.revision_key,
                &source.baseline_hash,
            )?)
        };
        let page = snapshot.page(
            &command.layer,
            command.offset as usize,
            command.limit as usize,
        )?;
        if source.annotate_candidate_ids {
            self.candidates
                .annotate_object_page(&source.project_id, &command.session_id, page)
        } else {
            Ok(page)
        }
    }

    pub fn thumbnail_rgba(&self, command: &MapThumbnailCommand) -> Result<isom::RgbaImage, String> {
        let session = self.session_record(&command.session_id)?;
        let state = self
            .candidates
            .state(&session.meta.project, &command.session_id)?;
        let request = json!({
            "schema": "eud-map-render/1",
            "mode": "thumbnail",
            "layer": command.layer,
            "id": command.id,
            "owner": command.owner,
            "tileset": state.baseline.tileset.era(),
        });
        isom::render_region(
            &self
                .candidates
                .current_map(&session.meta.project, &command.session_id)?,
            &self.candidates.context().starcraft_path()?,
            request.to_string().as_bytes(),
        )
        .map_err(|error| format!("palette thumbnail failed: {error}"))
    }

    pub fn diff_details(&self, session_id: &str) -> Result<MapDiffDetails, String> {
        let session = self.session_record(session_id)?;
        let state = self.candidates.state(&session.meta.project, session_id)?;
        if state.current_revision == 0 {
            return Ok(MapDiffDetails {
                terrain_rows: Vec::new(),
                markers: Vec::new(),
            });
        }
        let baseline = self
            .candidates
            .baseline_map(&session.meta.project, session_id)?;
        let current = self
            .candidates
            .current_map(&session.meta.project, session_id)?;
        let before_chk = isom::chk_extract(&baseline).map_err(|error| error.to_string())?;
        let after_chk = isom::chk_extract(&current).map_err(|error| error.to_string())?;
        let before_digest = crate::chk::digest_chk(&before_chk);
        let after_digest = crate::chk::digest_chk(&after_chk);
        let before = crate::chk::assemble_sections(&crate::chk::walk_sections(&before_chk));
        let after = crate::chk::assemble_sections(&crate::chk::walk_sections(&after_chk));
        let mut terrain_cells = std::collections::BTreeSet::new();
        for (index, (left, right)) in before_digest
            .tiles
            .iter()
            .zip(&after_digest.tiles)
            .enumerate()
        {
            if left != right {
                terrain_cells.insert((
                    (index % usize::from(after_digest.map.width)) as u16,
                    (index / usize::from(after_digest.map.width)) as u16,
                ));
            }
        }
        let buildings = crate::tool_exec::map_building_ids(
            &self.candidates.context().starcraft_path()?,
            &after_digest.map.tileset,
        )?;
        let mut markers = Vec::new();
        append_object_diff(
            &mut markers,
            before.get("UNIT").map(Vec::as_slice).unwrap_or(&[]),
            after.get("UNIT").map(Vec::as_slice).unwrap_or(&[]),
            crate::chk::UNIT_ENTRY_SIZE,
            |bytes| {
                let class_id = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
                if class_id != 0 {
                    format!("class:{class_id}")
                } else {
                    format!(
                        "unit:{}:{}:{}",
                        u16::from_le_bytes([bytes[8], bytes[9]]),
                        bytes[16],
                        u32::from_le_bytes(bytes[32..36].try_into().unwrap())
                    )
                }
            },
            |bytes| {
                let type_id = u16::from_le_bytes([bytes[8], bytes[9]]);
                let layer = if buildings.contains(&type_id) {
                    MapLayer::Buildings
                } else {
                    MapLayer::Units
                };
                (
                    layer,
                    u16::from_le_bytes([bytes[4], bytes[5]]) / 32,
                    u16::from_le_bytes([bytes[6], bytes[7]]) / 32,
                )
            },
        );
        append_object_diff(
            &mut markers,
            before.get("DD2 ").map(Vec::as_slice).unwrap_or(&[]),
            after.get("DD2 ").map(Vec::as_slice).unwrap_or(&[]),
            crate::chk::DD2_ENTRY_SIZE,
            |bytes| {
                format!(
                    "doodad:{}:{}",
                    u16::from_le_bytes([bytes[0], bytes[1]]),
                    bytes[6]
                )
            },
            |bytes| {
                (
                    MapLayer::Doodads,
                    u16::from_le_bytes([bytes[2], bytes[3]]) / 32,
                    u16::from_le_bytes([bytes[4], bytes[5]]) / 32,
                )
            },
        );
        append_object_diff(
            &mut markers,
            before.get("THG2").map(Vec::as_slice).unwrap_or(&[]),
            after.get("THG2").map(Vec::as_slice).unwrap_or(&[]),
            crate::chk::THG2_ENTRY_SIZE,
            |bytes| {
                format!(
                    "sprite:{}:{}:{}",
                    u16::from_le_bytes([bytes[0], bytes[1]]),
                    bytes[6],
                    u16::from_le_bytes([bytes[8], bytes[9]]) & 0x1000
                )
            },
            |bytes| {
                (
                    MapLayer::Sprites,
                    u16::from_le_bytes([bytes[2], bytes[3]]) / 32,
                    u16::from_le_bytes([bytes[4], bytes[5]]) / 32,
                )
            },
        );
        append_location_diff(
            &mut markers,
            before.get("MRGN").map(Vec::as_slice).unwrap_or(&[]),
            after.get("MRGN").map(Vec::as_slice).unwrap_or(&[]),
            after_digest.map.width,
            after_digest.map.height,
        );
        Ok(MapDiffDetails {
            terrain_rows: crate::map_model::rows_from_cells(&terrain_cells),
            markers,
        })
    }

    fn require_editor_idle(&self) -> Result<(), String> {
        if crate::native_runtime::NativeProjectManager::new(self.dirs.clone()).is_building() {
            return Err("the native project is building; Apply is blocked".to_string());
        }
        Ok(())
    }

    pub(crate) fn apply(&self, session_id: &str) -> Result<CandidateStateView, String> {
        self.require_editor_idle()?;
        let session = self.session_record(session_id)?;
        let project_id = session.meta.project;
        self.writes.transaction(&project_id, || {
            let verification = self
                .candidates
                .verify_current_for_apply(&project_id, session_id)?;
            if !verification.valid {
                return Err(format!(
                    "candidate verification failed: {}",
                    verification.errors.join("; ")
                ));
            }
            let state = self.candidates.state(&project_id, session_id)?;
            let live = self
                .candidates
                .context()
                .current()
                .map_err(|error| format!("current OpenMapName could not be confirmed; Apply is blocked: {error}"))?;
            require_current_source(
                &live.revision.project_id,
                &live.revision.source_path,
                &project_id,
                &state.baseline.source_path,
                "Apply",
            )?;
            let candidate = self.candidates.current_map(&project_id, session_id)?;
            let record = self
                .safe
                .apply(
                    &state.baseline.source_path,
                    &candidate,
                    &state.baseline.file_sha256,
                    &state.current_hash,
                )
                .map_err(|error| error.to_string())?;
            match self.candidates.complete_apply(&project_id, session_id, &record) {
                Ok(state) => {
                    self.safe
                        .complete_pending(&record)
                        .map_err(|error| error.to_string())?;
                    Ok(state)
                }
                Err(error) => {
                    self.safe.undo(&record).map_err(|rollback| {
                        format!("candidate state persistence failed: {error}; backup restore failed: {rollback}")
                    })?;
                    Err(format!("candidate state persistence failed; original restored: {error}"))
                }
            }
        })?
    }

    pub(crate) fn undo(&self, session_id: &str) -> Result<CandidateStateView, String> {
        self.require_editor_idle()?;
        let session = self.session_record(session_id)?;
        let project_id = session.meta.project;
        self.writes.transaction(&project_id, || {
            let record = self.candidates.last_apply_record(&project_id, session_id)?;
            let live = self.candidates.context().current().map_err(|error| {
                format!("current OpenMapName could not be confirmed; undo is blocked: {error}")
            })?;
            require_current_source(
                &live.revision.project_id,
                &live.revision.source_path,
                &project_id,
                &record.source_path,
                "undo",
            )?;
            self.safe.undo(&record).map_err(|error| error.to_string())?;
            self.candidates.complete_undo(&project_id, session_id)
        })?
    }

    /// Save scenario properties straight to the source map: stage only the
    /// changed operations into a verified work file, then apply it through
    /// MapSafe exactly like a candidate Apply so the Map window's Undo works.
    pub(crate) fn properties_save(
        &self,
        command: &MapPropertiesSaveCommand,
    ) -> Result<CandidateStateView, String> {
        self.require_editor_idle()?;
        let session = self.session_record(&command.session_id)?;
        let project_id = session.meta.project;
        let session_id = command.session_id.as_str();
        self.writes.transaction(&project_id, || {
            let state = self.candidates.state(&project_id, session_id)?;
            let live = self.candidates.context().current().map_err(|error| {
                format!("current OpenMapName could not be confirmed; properties save is blocked: {error}")
            })?;
            require_current_source(
                &live.revision.project_id,
                &live.revision.source_path,
                &project_id,
                &state.baseline.source_path,
                "properties save",
            )?;
            let stage =
                self.candidates
                    .properties_stage(&project_id, session_id, &command.properties)?;
            let result = (|| {
                let record = self
                    .safe
                    .apply(
                        &state.baseline.source_path,
                        &stage.work,
                        &state.baseline.file_sha256,
                        &stage.work_sha256,
                    )
                    .map_err(|error| error.to_string())?;
                match self.candidates.complete_apply(&project_id, session_id, &record) {
                    Ok(state) => {
                        self.safe
                            .complete_pending(&record)
                            .map_err(|error| error.to_string())?;
                        Ok(state)
                    }
                    Err(error) => {
                        self.safe.undo(&record).map_err(|rollback| {
                            format!("candidate state persistence failed: {error}; backup restore failed: {rollback}")
                        })?;
                        Err(format!("candidate state persistence failed; original restored: {error}"))
                    }
                }
            })();
            let cleanup = self.candidates.discard_properties_stage(&stage);
            match (result, cleanup) {
                (Ok(state), Ok(())) => Ok(state),
                (Err(error), _) => Err(error),
                (Ok(_), Err(error)) => Err(error),
            }
        })?
    }
}

/// The Map side of one EPS → Map team handoff (plan Phase 2): the team Map
/// session, the ordinary candidate request it runs for the EPS goal, and the
/// task-status hooks behind the user's Apply/discard/undo.
impl MapAgentService {
    /// A fresh team Map session for one task of EPS session `parent`: every
    /// map task starts a new session (new provider thread, empty transcript,
    /// candidate r0 on the saved source) so earlier requests never stack.
    /// Returns the session, its state, and the parent's earlier team sessions
    /// that are now retired: the caller deletes them through the engine
    /// manager. An earlier session survives only while the Map window can
    /// still undo its Apply against the current source.
    pub(crate) fn fresh_team_session(
        &self,
        parent: &crate::session::SessionRecord,
    ) -> Result<
        (
            crate::session::SessionRecord,
            CandidateStateView,
            Vec<crate::session::SessionMeta>,
        ),
        String,
    > {
        let context = self.candidates.context().current().map_err(|error| {
            format!("the source map is not available for a team map task: {error}")
        })?;
        let project = crate::native_runtime::NativeProjectManager::new(self.dirs.clone())
            .open()
            .map_err(|error| {
                format!("the native project is not open for a team map task: {error}")
            })?;
        self.fresh_team_session_in(parent, &context, &project.manifest().name)
    }

    /// `fresh_team_session` against an explicit source-map context and the
    /// open project's manifest name. EPS sessions are keyed by manifest name
    /// while Map sessions and `MapRevision.project_id` are keyed by the
    /// project-root hash, so the two identities are never compared directly.
    pub(crate) fn fresh_team_session_in(
        &self,
        parent: &crate::session::SessionRecord,
        context: &crate::map_context::MapContextSnapshot,
        current_project_name: &str,
    ) -> Result<
        (
            crate::session::SessionRecord,
            CandidateStateView,
            Vec<crate::session::SessionMeta>,
        ),
        String,
    > {
        if parent.meta.kind != crate::session::SessionKind::Eps {
            return Err("only an EPS session owns a team Map session".to_string());
        }
        if parent.meta.project != current_project_name {
            return Err(format!(
                "EPS session '{}' belongs to project '{}', not the open project '{}'",
                parent.meta.name, parent.meta.project, current_project_name
            ));
        }
        let retired = self
            .sessions
            .team_sessions_of(&parent.meta.id)
            .map_err(|error| error.to_string())?
            .into_iter()
            .filter(|meta| {
                meta.project != context.revision.project_id
                    || !self
                        .candidates
                        .apply_undoable_against(
                            &meta.project,
                            &meta.id,
                            &context.revision.file_sha256,
                        )
                        .unwrap_or(false)
            })
            .collect::<Vec<_>>();
        let mut provider_binding = parent.provider_binding.clone();
        provider_binding.conversation =
            crate::provider::ProviderConversationState::empty(provider_binding.provider);
        let record = crate::session::SessionRecord {
            meta: crate::session::SessionMeta {
                id: crate::session::new_session_id(),
                name: format!("{} · 맵 작업", parent.meta.name),
                project: context.revision.project_id.clone(),
                kind: crate::session::SessionKind::Map,
                provider: provider_binding.provider,
                model: provider_binding.model.clone(),
                created_at: crate::session::now_unix_seconds(),
                last_conversation_at: crate::session::now_unix_millis(),
                team_parent: Some(parent.meta.id.clone()),
            },
            provider_binding,
            pending_request_ids: Vec::new(),
            context_usage: None,
            panel_log: serde_json::Value::Null,
            context_state: Default::default(),
            task_state: Default::default(),
            autonomous_run: None,
            team_tasks: Vec::new(),
        };
        self.sessions
            .save(&record)
            .map_err(|error| error.to_string())?;
        let state = self.candidates.create_session(&record.meta.id, context)?;
        Ok((record, state, retired))
    }

    /// Project the EPS task scope onto the team session's visible candidate:
    /// persistent target selections become `target` mentions and exact
    /// location ids become location mentions. Anything missing or not a target
    /// is an error the EPS model can correct.
    pub(crate) fn team_mentions(
        state: &CandidateStateView,
        selection_ids: &[String],
        location_ids: &[u16],
    ) -> Result<Vec<MapMentionSnapshot>, String> {
        let mut mentions = Vec::new();
        for selection_id in selection_ids {
            let view = state
                .selections
                .iter()
                .find(|view| &view.selection.id == selection_id)
                .ok_or_else(|| {
                    format!("selection '{selection_id}' does not exist on the current map")
                })?;
            if view.selection.role != crate::map_model::SelectionRole::Target {
                return Err(format!(
                    "selection '{selection_id}' is a {:?} selection, not a target; only target selections scope a map task",
                    view.selection.role
                ));
            }
            mentions.push(MapMentionSnapshot::Region {
                selection_id: selection_id.clone(),
                snapshot_hash: view.snapshot_hash.clone(),
                source_revision: state.revision_key.clone(),
            });
        }
        for location_id in location_ids {
            mentions.push(MapMentionSnapshot::Location {
                location_id: *location_id,
                revision_key: state.revision_key.clone(),
                baseline_hash: state.baseline.file_sha256.clone(),
            });
        }
        Ok(mentions)
    }

    /// The team session an earlier task ran in, for a request that revises
    /// that task's result: its conversation continues, and its candidate too
    /// while the task is still waiting for a decision.
    pub(crate) fn continued_team_session(
        &self,
        parent: &crate::session::SessionRecord,
        revised: &crate::team::TeamTask,
    ) -> Result<(crate::session::SessionRecord, CandidateStateView), String> {
        use crate::team::TeamTaskStatus;
        if parent.meta.kind != crate::session::SessionKind::Eps {
            return Err("only an EPS session owns a team Map session".to_string());
        }
        if !matches!(
            revised.status,
            TeamTaskStatus::CandidateReady | TeamTaskStatus::Applied
        ) {
            return Err(format!(
                "team task {} is {}; only a candidate_ready or applied task can be revised, so send this request without revisesTaskId",
                revised.id,
                revised.status.label()
            ));
        }
        let record = self
            .sessions
            .load(&revised.map_session_id)
            .ok()
            .filter(|record| record.meta.team_parent.as_deref() == Some(parent.meta.id.as_str()))
            .ok_or_else(|| {
                format!(
                    "the team session of task {} was retired; send this request without revisesTaskId and describe the whole result",
                    revised.id
                )
            })?;
        let state = self
            .candidates
            .state(&record.meta.project, &record.meta.id)?;
        Ok((record, state))
    }

    /// The rows of a team task's target scope: the union of `target` (the
    /// whole map when it is empty) minus `protect`. `None` when the request
    /// names neither, so the task is unscoped like an unselected Map request.
    pub(crate) fn team_scope_rows(
        width: u16,
        height: u16,
        target: &[crate::team::TeamTileRect],
        protect: &[crate::team::TeamTileRect],
    ) -> Result<Option<Vec<crate::map_model::RowSpan>>, String> {
        if target.is_empty() && protect.is_empty() {
            return Ok(None);
        }
        for (name, rect) in target
            .iter()
            .map(|rect| ("target", rect))
            .chain(protect.iter().map(|rect| ("protect", rect)))
        {
            if rect.width == 0
                || rect.height == 0
                || u32::from(rect.x) + u32::from(rect.width) > u32::from(width)
                || u32::from(rect.y) + u32::from(rect.height) > u32::from(height)
            {
                return Err(format!(
                    "{name} rectangle x={} y={} width={} height={} is empty or leaves the {width}x{height} map",
                    rect.x, rect.y, rect.width, rect.height
                ));
            }
        }
        let inside = |rects: &[crate::team::TeamTileRect], x: u16, y: u16| {
            rects.iter().any(|rect| {
                x >= rect.x && x - rect.x < rect.width && y >= rect.y && y - rect.y < rect.height
            })
        };
        let mut rows = Vec::new();
        for y in 0..height {
            let mut spans = Vec::new();
            let mut start = None;
            for x in 0..=width {
                let selected = x < width
                    && (target.is_empty() || inside(target, x, y))
                    && !inside(protect, x, y);
                match (selected, start) {
                    (true, None) => start = Some(x),
                    (false, Some(left)) => {
                        spans.push((left, x));
                        start = None;
                    }
                    _ => {}
                }
            }
            if !spans.is_empty() {
                rows.push(crate::map_model::RowSpan { y, spans });
            }
        }
        if rows.is_empty() {
            return Err(
                "protect covers the whole target; the map task has no cell to change".to_string(),
            );
        }
        Ok(Some(rows))
    }

    /// Save a team task's scope as the EPS session's target selection on the
    /// team session's visible candidate (one id per EPS session, replaced by
    /// each task) and return its mention. The selection carries the task's
    /// layers, so the verifier refuses a change outside the scope or on
    /// another layer exactly as for a region the user selected.
    pub(crate) fn team_scope_mention(
        &self,
        parent_id: &str,
        parent_name: &str,
        team: &crate::session::SessionRecord,
        state: &CandidateStateView,
        layers: &[MapLayer],
        rows: Vec<RowSpan>,
    ) -> Result<(String, MapMentionSnapshot), String> {
        let id = format!("team-{}", crate::session::short_session_id(parent_id));
        let mask = SelectionMask::canonical(
            id.clone(),
            format!("{parent_name} 작업 영역"),
            state.revision_key.clone(),
            crate::map_model::SelectionRole::Target,
            layers.iter().copied().collect(),
            crate::map_model::MaskGrid {
                width: state.baseline.width,
                height: state.baseline.height,
                rows,
            },
        )?;
        let snapshot_hash = mask.snapshot_hash();
        let view = self
            .candidates
            .save_selection(&team.meta.project, &team.meta.id, mask)?;
        let mention = MapMentionSnapshot::Region {
            selection_id: id.clone(),
            snapshot_hash,
            source_revision: view.revision_key,
        };
        Ok((id, mention))
    }

    /// The fixed Map-side prompt for one team task. The Map Agent keeps its
    /// own system prompt; this is the user message it receives. `revises` is
    /// the earlier task of this same conversation the request edits.
    pub(crate) fn team_request_text(
        parent_name: &str,
        parent_id: &str,
        task: &crate::team::TeamTask,
        revises: Option<&str>,
        mentions: &Value,
    ) -> String {
        let layers = task
            .layers
            .iter()
            .map(|layer| format!("{layer:?}").to_lowercase())
            .collect::<Vec<_>>()
            .join(", ");
        let locations = if task.location_ids.is_empty() {
            "(none)".to_string()
        } else {
            task.location_ids
                .iter()
                .map(|id| format!("#{id}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let parent_short_id = crate::session::short_session_id(parent_id);
        let revision = revises
            .map(|earlier| {
                format!(
                    " It revises team task {earlier}, which you worked on earlier in this conversation: make the change the goal asks for on the current candidate and keep the rest of that work."
                )
            })
            .unwrap_or_default();
        format!(
            "[map mention snapshots]\n{mentions}\n\n[team task]\nThis request comes from the EPS session \"{parent_name}\" (session id {parent_short_id}) as team task {}.{revision} Work the goal below exactly as you would a request the user typed in this window: explore the palette, draft, render and analyze, and iterate until it looks right, then finalize once; the user reviews and applies the candidate in this window, and the EPS session continues afterwards. The goal relays the user's request: its stated bounds, keep-clear areas, and constraints are binding, everything else is intent, and the choice of tiles, doodads, objects, and layout is yours. Change only these layers: {layers}. Location context: {locations}. Do not ask the EPS session questions; if the goal is impossible, answer with why and finalize nothing.\n\n[goal]\n{}",
            task.id, task.goal
        )
    }

    /// Run one team task's Map request on `map_session_id`: the same
    /// prepare → chat → commit lifecycle as the Map window's own request. The
    /// window, open or opened later, shows the run through its transcript
    /// with the task goal as the request bubble. `scope` rows become the
    /// request's target selection right before prepare, and that selection
    /// is removed again once the request ends: the committed revision keeps
    /// its authority in its manifest, and the selection must not stay in the
    /// project's selection palette. Returns the candidate state after commit;
    /// a cancelled turn commits nothing and settles `cancelled`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn run_team_request(
        &self,
        engines: &crate::engine::SessionEngineManager,
        map_session_id: &str,
        request_id: &str,
        mut mentions: Vec<MapMentionSnapshot>,
        parent_name: &str,
        parent_id: &str,
        task: &crate::team::TeamTask,
        revises: Option<&str>,
        scope: Option<Vec<RowSpan>>,
    ) -> Result<CandidateStateView, String> {
        let session = self.session_record(map_session_id)?;
        let current = self.candidates.context().current()?;
        let current_state = self
            .candidates
            .state(&session.meta.project, map_session_id)?;
        require_current_source(
            &current.revision.project_id,
            &current.revision.source_path,
            &session.meta.project,
            &current_state.baseline.source_path,
            "team map task",
        )?;
        // The request bubble keeps only the task's own mentions: the scope
        // selection is removed once the request ends.
        let prompt_mentions = mentions.clone();
        let scope_id = match scope {
            Some(rows) => {
                let (id, mention) = self.team_scope_mention(
                    parent_id,
                    parent_name,
                    &session,
                    &current_state,
                    &task.layers,
                    rows,
                )?;
                mentions.push(mention);
                Some(id)
            }
            None => None,
        };
        let outcome = self
            .run_prepared_team_request(
                engines,
                &session,
                current_state.current_revision,
                request_id,
                mentions,
                prompt_mentions,
                parent_name,
                parent_id,
                task,
                revises,
            )
            .await;
        if let Some(id) = scope_id {
            if let Err(error) =
                self.candidates
                    .delete_selection(&session.meta.project, map_session_id, &id)
            {
                eprintln!(
                    "eud-agent: team task scope selection {id} could not be removed: {error}"
                );
            }
        }
        outcome
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_prepared_team_request(
        &self,
        engines: &crate::engine::SessionEngineManager,
        session: &crate::session::SessionRecord,
        parent_revision: u32,
        request_id: &str,
        mentions: Vec<MapMentionSnapshot>,
        prompt_mentions: Vec<MapMentionSnapshot>,
        parent_name: &str,
        parent_id: &str,
        task: &crate::team::TeamTask,
        revises: Option<&str>,
    ) -> Result<CandidateStateView, String> {
        let map_session_id = session.meta.id.as_str();
        self.candidates.prepare_request(
            &session.meta.project,
            map_session_id,
            request_id,
            parent_revision,
            &mentions,
        )?;
        let state_before = self
            .candidates
            .state(&session.meta.project, map_session_id)?;
        let compact = match compact_mentions(
            &self.candidates,
            &session.meta.project,
            &state_before,
            &mentions,
        ) {
            Ok(value) => value,
            Err(error) => {
                self.candidates.finish_request(map_session_id, request_id)?;
                return Err(error);
            }
        };
        let outcome = engines
            .map_chat(
                map_session_id,
                request_id.to_string(),
                state_before.revision_key,
                Self::team_request_text(parent_name, parent_id, task, revises, &compact),
                Vec::new(),
                crate::engine::MapRunPrompt {
                    text: task.goal.clone(),
                    mentions: prompt_mentions,
                    origin: crate::engine::MapRunOrigin::Team,
                    team_task_id: Some(task.id.clone()),
                    parent_session_name: Some(parent_name.to_string()),
                },
            )
            .await;
        match outcome {
            Ok(crate::engine::MapTurnEnd::Completed) => {}
            // A stopped request settles `cancelled`, never `failed` (a failure
            // continues the EPS conversation, a cancellation leaves it to the
            // user), and leaves no candidate: a draft it finalized before the
            // stop is dropped, not committed under a task nobody tracks.
            Ok(crate::engine::MapTurnEnd::Cancelled) => {
                self.candidates.finish_request(map_session_id, request_id)?;
                return Err(crate::team::TEAM_MAP_REQUEST_CANCELLED.to_string());
            }
            Err(error) => {
                self.candidates.finish_request(map_session_id, request_id)?;
                return Err(error);
            }
        }
        let state =
            match self
                .candidates
                .commit_request(&session.meta.project, map_session_id, request_id)
            {
                Ok(state) => state,
                Err(error) => {
                    self.candidates.finish_request(map_session_id, request_id)?;
                    return Err(error);
                }
            };
        self.candidates.finish_request(map_session_id, request_id)?;
        Ok(state)
    }

    /// Summarize the candidate revision a team task produced, if the request
    /// committed a new one.
    pub(crate) fn team_candidate_summary(
        before_revision: u32,
        state: &CandidateStateView,
    ) -> Option<crate::team::TeamCandidateSummary> {
        if state.current_revision <= before_revision {
            return None;
        }
        let revision = state
            .revisions
            .iter()
            .find(|revision| revision.revision == state.current_revision)?;
        let diff = &revision.diff;
        let count = |layer: &crate::map_model::LayerDiffCount| {
            layer.added + layer.removed + layer.moved + layer.changed
        };
        let mut parts = Vec::new();
        if diff.terrain_cells > 0 {
            parts.push(format!("지형 {}칸", diff.terrain_cells));
        }
        for (label, layer) in [
            ("유닛", &diff.units),
            ("건물", &diff.buildings),
            ("두다드", &diff.doodads),
            ("스프라이트", &diff.sprites),
            ("로케이션", &diff.locations),
        ] {
            if count(layer) > 0 {
                parts.push(format!("{label} {}건", count(layer)));
            }
        }
        Some(crate::team::TeamCandidateSummary {
            revision: state.current_revision,
            revision_key: state.revision_key.clone(),
            map_sha256: state.current_hash.clone(),
            summary: if parts.is_empty() {
                "변경 없음".to_string()
            } else {
                parts.join(", ")
            },
            terrain_cells: diff.terrain_cells,
            units: count(&diff.units),
            buildings: count(&diff.buildings),
            doodads: count(&diff.doodads),
            sprites: count(&diff.sprites),
            locations: count(&diff.locations),
        })
    }

    /// After the user's trusted Apply on `map_session_id`: every ready team
    /// task targeting it becomes `applied` with the new source hash.
    pub(crate) fn team_tasks_applied(
        &self,
        map_session_id: &str,
        state: &CandidateStateView,
        actor: crate::team::TeamApplyActor,
    ) -> Vec<(String, crate::team::TeamTask)> {
        self.settle_team_tasks(map_session_id, |task| {
            if task.status != crate::team::TeamTaskStatus::CandidateReady {
                return false;
            }
            task.status = crate::team::TeamTaskStatus::Applied;
            task.applied_source_sha256 = Some(state.baseline.file_sha256.clone());
            task.applied_by = Some(actor);
            true
        })
    }

    /// The EPS agent's `map_task_apply` / `map_task_discard` on one of its
    /// tasks (plan Phase 2b). The task must still be `candidate_ready` and the
    /// team session must still hold exactly the announced revision; apply then
    /// runs the Map window's own apply path and discard drops the candidate.
    /// Returns the updated task and every other task the action settled.
    pub(crate) fn team_task_action(
        &self,
        action: &crate::tool_exec::TeamTaskAction,
    ) -> Result<(crate::team::TeamTask, Vec<(String, crate::team::TeamTask)>), String> {
        let parent = self
            .sessions
            .load(&action.session_id)
            .map_err(|error| format!("the EPS session could not be loaded: {error}"))?;
        let task = parent
            .team_tasks
            .iter()
            .find(|task| task.id == action.task_id)
            .cloned()
            .ok_or_else(|| format!("map task '{}' does not exist", action.task_id))?;
        if task.status != crate::team::TeamTaskStatus::CandidateReady {
            return Err(format!(
                "map task '{}' is {}, not candidate_ready",
                task.id,
                task.status.label()
            ));
        }
        let candidate = task
            .candidate
            .as_ref()
            .ok_or_else(|| format!("map task '{}' has no candidate", task.id))?;
        let team = self.session_record(&task.map_session_id)?;
        let state = self
            .candidates
            .state(&team.meta.project, &task.map_session_id)?;
        // Revision numbers never repeat within a session; the announced hash
        // legitimately changes when the session follows a changed source.
        if state.stale || state.current_revision != candidate.revision {
            return Err(format!(
                "map task '{}' candidate changed since it was announced (team session is at r{}{})",
                task.id,
                state.current_revision,
                if state.stale { ", stale" } else { "" }
            ));
        }
        let settled = match action.kind {
            crate::tool_exec::TeamTaskActionKind::Apply => {
                if state.source_diverged {
                    return Err(format!(
                        "map task '{}' candidate could not be moved onto the saved source map (someone saved the same area differently); applying it would overwrite that save, so only the user can Apply it from the Map window",
                        task.id
                    ));
                }
                let state = self.apply(&task.map_session_id)?;
                self.team_tasks_applied(
                    &task.map_session_id,
                    &state,
                    crate::team::TeamApplyActor::Agent,
                )
            }
            crate::tool_exec::TeamTaskActionKind::Discard => {
                self.candidates
                    .discard(&team.meta.project, &task.map_session_id)?;
                self.team_tasks_withdrawn(&task.map_session_id, TeamWithdrawal::Discard)
            }
        };
        let updated = settled
            .iter()
            .find(|(parent_id, updated)| parent_id == &action.session_id && updated.id == task.id)
            .map(|(_, updated)| updated.clone())
            .ok_or_else(|| {
                format!(
                    "map task '{}' did not settle after the {:?}",
                    task.id, action.kind
                )
            })?;
        Ok((updated, settled))
    }

    /// The team session's current candidate state, for refreshing an open Map
    /// window after the EPS agent changed it.
    /// The source hash the session's last apply produced, read before an
    /// undo reverts it (`TeamWithdrawal::Undo`).
    pub(crate) fn last_applied_sha256(&self, session_id: &str) -> Option<String> {
        let session = self.session_record(session_id).ok()?;
        self.candidates
            .last_apply_record(&session.meta.project, session_id)
            .ok()
            .map(|record| record.applied_sha256)
    }

    pub(crate) fn team_session_state(
        &self,
        map_session_id: &str,
    ) -> Result<CandidateStateView, String> {
        let team = self.session_record(map_session_id)?;
        self.candidates.state(&team.meta.project, map_session_id)
    }

    /// Settle the tasks a candidate withdrawal ends. A team session can hold
    /// several tasks (a revision continues its task's session), so each kind
    /// withdraws only what it undid: a discard or revert the ready candidate,
    /// an undo the ready candidate and the one apply it reverted, and a
    /// deleted session everything it could still deliver.
    pub(crate) fn team_tasks_withdrawn(
        &self,
        map_session_id: &str,
        withdrawal: TeamWithdrawal,
    ) -> Vec<(String, crate::team::TeamTask)> {
        use crate::team::TeamTaskStatus;
        self.settle_team_tasks(map_session_id, |task| {
            let withdrawn = match (&task.status, &withdrawal) {
                (TeamTaskStatus::CandidateReady, TeamWithdrawal::Revert(revision)) => task
                    .candidate
                    .as_ref()
                    .map_or(true, |candidate| *revision < candidate.revision),
                (TeamTaskStatus::CandidateReady, _) => true,
                (TeamTaskStatus::Applied, TeamWithdrawal::Undo { applied_sha256 }) => {
                    applied_sha256.is_some() && task.applied_source_sha256 == *applied_sha256
                }
                (TeamTaskStatus::Applied, TeamWithdrawal::SessionDeleted) => true,
                _ => false,
            };
            if withdrawn {
                task.status = TeamTaskStatus::Discarded;
            }
            withdrawn
        })
    }

    /// Apply `update` to every task targeting `map_session_id` and return the
    /// tasks that changed with their owning EPS session, for the caller to
    /// announce to that session's panel.
    fn settle_team_tasks(
        &self,
        map_session_id: &str,
        update: impl Fn(&mut crate::team::TeamTask) -> bool,
    ) -> Vec<(String, crate::team::TeamTask)> {
        let tasks = match self.sessions.team_tasks_for_map_session(map_session_id) {
            Ok(tasks) => tasks,
            Err(error) => {
                eprintln!("eud-agent: team task lookup failed: {error}");
                return Vec::new();
            }
        };
        let mut settled = Vec::new();
        for (parent_id, task) in tasks {
            match self
                .sessions
                .update_team_task(&parent_id, &task.id, &update)
            {
                Ok(Some(updated)) if updated != task => settled.push((parent_id, updated)),
                Ok(_) => {}
                Err(error) => eprintln!("eud-agent: team task update failed: {error}"),
            }
        }
        settled
    }

    /// Remove a deleted team session's candidate files; `project_id` is the
    /// map project the session belonged to, read before the record was deleted.
    pub(crate) fn discard_session_candidate(
        &self,
        project_id: &str,
        session_id: &str,
    ) -> Result<(), String> {
        self.candidates.discard(project_id, session_id)
    }

    /// Startup mapping: running tasks are interrupted; a ready candidate stays
    /// only while the team session still holds exactly that revision.
    ///
    /// An interrupted task's team Map session also gets the window's
    /// "대화 초기화" applied for it: the native run that died with the process
    /// leaves an unconfirmed receipt that would refuse every later turn, and
    /// nobody sits in that session to reset it by hand. Its candidate, draft
    /// state, and panel log stay; only the model-side thread restarts.
    pub(crate) fn recover_team_tasks(&self) -> Result<usize, String> {
        let recovery = self
            .sessions
            .recover_interrupted_team_tasks(|task, session| {
                let Some(candidate) = task.candidate.as_ref() else {
                    return false;
                };
                // The store lock is held here: `session` is the task's Map
                // session as loaded by the store; never re-read it.
                if session.meta.kind != crate::session::SessionKind::Map {
                    return false;
                }
                // Startup never follows the source; a changed map reads as stale here.
                self.candidates
                    .peek_state(&session.meta.project, &task.map_session_id)
                    .is_ok_and(|state| !state.stale && state.current_revision == candidate.revision)
            })
            .map_err(|error| error.to_string())?;
        let mut reset = std::collections::HashSet::new();
        for task in &recovery.interrupted {
            if !reset.insert(task.map_session_id.clone()) {
                continue;
            }
            if let Err(error) = self.reset_team_session_conversation(&task.map_session_id) {
                eprintln!(
                    "eud-agent: interrupted team map session {} could not be reset: {error}",
                    task.map_session_id
                );
            }
        }
        let mut changed = recovery.changed;
        for map_session_id in &reset {
            changed += self.restore_interrupted_revision(map_session_id);
        }
        Ok(changed)
    }

    /// A revision interrupted by the restart produced nothing, so the task it
    /// revised gets its candidate back when the team session still shows
    /// exactly that revision and nothing newer is waiting there.
    fn restore_interrupted_revision(&self, map_session_id: &str) -> usize {
        use crate::team::TeamTaskStatus;
        let Ok(tasks) = self.sessions.team_tasks_for_map_session(map_session_id) else {
            return 0;
        };
        if tasks
            .iter()
            .any(|(_, task)| task.status == TeamTaskStatus::CandidateReady)
        {
            return 0;
        }
        let Some((parent_id, revised)) = tasks
            .into_iter()
            .filter(|(_, task)| task.status == TeamTaskStatus::Superseded)
            .max_by(|(_, a), (_, b)| a.updated_at.cmp(&b.updated_at).then(a.id.cmp(&b.id)))
        else {
            return 0;
        };
        let Some(candidate) = revised.candidate.as_ref() else {
            return 0;
        };
        let intact = self.session_record(map_session_id).is_ok_and(|session| {
            self.candidates
                .peek_state(&session.meta.project, map_session_id)
                .is_ok_and(|state| !state.stale && state.current_revision == candidate.revision)
        });
        if !intact {
            return 0;
        }
        match self
            .sessions
            .update_team_task(&parent_id, &revised.id, |task| {
                let superseded = task.status == TeamTaskStatus::Superseded;
                if superseded {
                    task.status = TeamTaskStatus::CandidateReady;
                }
                superseded
            }) {
            Ok(Some(task)) if task.status == TeamTaskStatus::CandidateReady => 1,
            Ok(_) => 0,
            Err(error) => {
                eprintln!(
                    "eud-agent: revised team task {} could not be restored: {error}",
                    revised.id
                );
                0
            }
        }
    }

    /// Clear every unconfirmed native run receipt of a team Map session and
    /// forget its persisted provider conversation.
    fn reset_team_session_conversation(&self, map_session_id: &str) -> Result<(), String> {
        let journal = self.dirs.journal_dir();
        for receipt in crate::provider_tool_loop::unresolved_native_runs(&journal, map_session_id)?
        {
            crate::provider_tool_loop::clear_native_run_recovery(
                &journal,
                map_session_id,
                receipt.provider,
                receipt.prior_native_id.as_deref(),
            )?;
        }
        self.sessions
            .reset_provider_conversation(map_session_id)
            .map_err(|error| error.to_string())
    }
}

fn next_map_session_name(sessions: &[crate::session::SessionMeta]) -> String {
    let names = sessions
        .iter()
        .map(|session| session.name.as_str())
        .collect::<std::collections::HashSet<_>>();
    if !names.contains("Map Agent") {
        return "Map Agent".to_string();
    }
    for index in 2_u32.. {
        let candidate = format!("Map Agent {index}");
        if !names.contains(candidate.as_str()) {
            return candidate;
        }
    }
    unreachable!("unbounded map session suffix search must find a free name")
}

async fn bootstrap_map_session(
    service: &MapAgentService,
    engines: &crate::engine::SessionEngineManager,
    context: crate::map_context::MapContextSnapshot,
    resolution: MapSessionResolution,
) -> Result<MapBootstrapResponse, String> {
    let candidates = service.candidates.clone();
    let session_id = resolution.session.meta.id.clone();
    let open_context = context.clone();
    let candidate_action = resolution.candidate_action;
    // Opening may replay the candidate onto a changed source (seconds of isom
    // work); keep that off the async runtime.
    let candidate = run_map_blocking("map session open", move || match candidate_action {
        CandidateSessionAction::Create => candidates.create_session(&session_id, &open_context),
        CandidateSessionAction::Open => candidates.open_session(&session_id, &open_context),
    })
    .await?;
    let conversation_resume_error = engines
        .open_map_session(&resolution.session.meta.id)
        .await?;
    let pending_run = engines.map_run_snapshot(&resolution.session.meta.id).await;
    Ok(MapBootstrapResponse {
        context,
        candidate,
        session: resolution.session,
        conversation_resume_error,
        pending_run,
        open_properties: std::mem::take(&mut *service.pending_open_properties.lock()),
    })
}

fn require_current_source(
    current_project: &str,
    current_source: &Path,
    expected_project: &str,
    expected_source: &Path,
    operation: &str,
) -> Result<(), String> {
    if current_project != expected_project || current_source != expected_source {
        return Err(format!(
            "current OpenMapName changed; {operation} is blocked for the inactive source map"
        ));
    }
    Ok(())
}

fn append_object_diff<I, D>(
    markers: &mut Vec<MapDiffMarker>,
    before: &[u8],
    after: &[u8],
    entry_size: usize,
    identity: I,
    describe: D,
) where
    I: Fn(&[u8]) -> String,
    D: Fn(&[u8]) -> (MapLayer, u16, u16),
{
    let before = before.chunks_exact(entry_size).collect::<Vec<_>>();
    let after = after.chunks_exact(entry_size).collect::<Vec<_>>();
    let mut before_matched = vec![false; before.len()];
    let mut after_matched = vec![false; after.len()];

    for (before_index, before_entry) in before.iter().enumerate() {
        if let Some(after_index) = after
            .iter()
            .enumerate()
            .position(|(index, after_entry)| !after_matched[index] && after_entry == before_entry)
        {
            before_matched[before_index] = true;
            after_matched[after_index] = true;
        }
    }

    let mut push = |entry: &[u8], ordinal: usize, change: &'static str| {
        let (layer, x, y) = describe(entry);
        markers.push(MapDiffMarker {
            layer,
            change,
            ordinal,
            bounds: TileRect {
                left: x,
                top: y,
                right: x.saturating_add(1),
                bottom: y.saturating_add(1),
            },
        });
    };

    for (before_index, before_entry) in before.iter().enumerate() {
        if before_matched[before_index] {
            continue;
        }
        let before_identity = identity(before_entry);
        if let Some(after_index) = after.iter().enumerate().position(|(index, after_entry)| {
            !after_matched[index] && identity(after_entry) == before_identity
        }) {
            before_matched[before_index] = true;
            after_matched[after_index] = true;
            let (_, before_x, before_y) = describe(before_entry);
            let (_, after_x, after_y) = describe(after[after_index]);
            push(
                after[after_index],
                after_index,
                if before_x != after_x || before_y != after_y {
                    "moved"
                } else {
                    "changed"
                },
            );
        } else {
            push(before_entry, before_index, "removed");
        }
    }
    for (after_index, after_entry) in after.iter().enumerate() {
        if !after_matched[after_index] {
            push(after_entry, after_index, "added");
        }
    }
}

fn append_location_diff(
    markers: &mut Vec<MapDiffMarker>,
    before: &[u8],
    after: &[u8],
    width: u16,
    height: u16,
) {
    let before = before
        .chunks_exact(crate::chk::MRGN_ENTRY_SIZE)
        .collect::<Vec<_>>();
    let after = after
        .chunks_exact(crate::chk::MRGN_ENTRY_SIZE)
        .collect::<Vec<_>>();
    for ordinal in 0..before.len().max(after.len()) {
        let left = before.get(ordinal);
        let right = after.get(ordinal);
        if left == right {
            continue;
        }
        let selected = right.or(left).expect("changed location has one side");
        let read = |offset| i32::from_le_bytes(selected[offset..offset + 4].try_into().unwrap());
        let pixel_left = read(0).min(read(8));
        let pixel_right = read(0).max(read(8));
        let pixel_top = read(4).min(read(12));
        let pixel_bottom = read(4).max(read(12));
        let tile_left = pixel_left.div_euclid(32).clamp(0, i32::from(width)) as u16;
        let tile_top = pixel_top.div_euclid(32).clamp(0, i32::from(height)) as u16;
        let tile_right = pixel_right
            .saturating_add(31)
            .div_euclid(32)
            .clamp(i32::from(tile_left.saturating_add(1)), i32::from(width))
            as u16;
        let tile_bottom = pixel_bottom
            .saturating_add(31)
            .div_euclid(32)
            .clamp(i32::from(tile_top.saturating_add(1)), i32::from(height))
            as u16;
        markers.push(MapDiffMarker {
            layer: MapLayer::Locations,
            change: match (left, right) {
                (None, Some(_)) => "added",
                (Some(_), None) => "removed",
                _ => "changed",
            },
            ordinal,
            bounds: TileRect {
                left: tile_left,
                top: tile_top,
                right: tile_right,
                bottom: tile_bottom,
            },
        });
    }
}

/// The event the Map window receives when it is already open and should
/// switch to another session of the current source map.
pub(crate) const MAP_OPEN_SESSION_EVENT: &str = "map-agent-open-session";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MapOpenSessionEvent {
    session_id: String,
}

/// Open or focus the Map window. With `session_id`, the window loads that
/// session: a fresh window bootstraps it, an open window receives
/// [`MAP_OPEN_SESSION_EVENT`] and switches when it is idle. A team session
/// whose request is running is a valid target (the window adopts the run);
/// an invalid target falls back to the ordinary session resolution.
pub(crate) fn open_map_window(
    app: &tauri::AppHandle,
    session_id: Option<&str>,
) -> Result<(), String> {
    let service = app.state::<MapAgentService>();
    let target = session_id.and_then(|session_id| {
        let context = service.candidates.context().current().ok()?;
        match service.session_for_context(session_id, &context) {
            Ok(_) => Some(session_id.to_string()),
            Err(error) => {
                eprintln!("eud-agent: map window target {session_id} ignored: {error}");
                None
            }
        }
    });
    if let Some(window) = app.get_webview_window(MAP_WINDOW_LABEL) {
        window.show().map_err(|error| error.to_string())?;
        window.set_focus().map_err(|error| error.to_string())?;
        if let Some(session_id) = target {
            window
                .emit(MAP_OPEN_SESSION_EVENT, MapOpenSessionEvent { session_id })
                .map_err(|error| error.to_string())?;
        }
        return Ok(());
    }
    *service.pending_open_session.lock() = target;
    let builder = tauri::WebviewWindowBuilder::new(
        app,
        MAP_WINDOW_LABEL,
        tauri::WebviewUrl::App("map-agent.html".into()),
    )
    .title("Map Agent Workbench")
    .inner_size(1600.0, 960.0)
    .min_inner_size(1100.0, 700.0)
    .resizable(true);
    // Window-level OLE drag/drop is a Windows-only builder option; other platforms
    // disable the webview drag/drop handler so HTML5 drag/drop reaches the page.
    #[cfg(windows)]
    let builder = builder.drag_and_drop(false);
    #[cfg(not(windows))]
    let builder = builder.disable_drag_drop_handler();
    builder.build().map_err(|error| {
        *service.pending_open_session.lock() = None;
        error.to_string()
    })?;
    Ok(())
}

/// Close the Map window if it is open; the main window's shutdown calls this
/// so a Map-only process never lingers. Closing does not touch candidate or
/// session state, which the window reloads on its next open.
pub(crate) fn close_map_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(MAP_WINDOW_LABEL) {
        if let Err(error) = window.close() {
            eprintln!("eud-agent: map window could not be closed with the main window: {error}");
        }
    }
}

#[tauri::command]
pub async fn map_agent_open(
    app: tauri::AppHandle,
    session_id: Option<String>,
) -> Result<(), String> {
    open_map_window(&app, session_id.as_deref())
}

/// The event an already-open Map window receives when the main window's
/// header asked for the "맵 속성" dialog.
pub(crate) const MAP_OPEN_PROPERTIES_EVENT: &str = "map-agent-open-properties";

/// Open or focus the Map window on its "맵 속성" dialog. Scenario properties
/// stay a Map-window request — the save still runs through that window's
/// session, verification and Undo — so the main window's header only carries
/// the intent there.
#[tauri::command]
pub async fn map_agent_open_properties(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(MAP_WINDOW_LABEL) {
        window.show().map_err(|error| error.to_string())?;
        window.set_focus().map_err(|error| error.to_string())?;
        window
            .emit(MAP_OPEN_PROPERTIES_EVENT, ())
            .map_err(|error| error.to_string())?;
        return Ok(());
    }
    let service = app.state::<MapAgentService>();
    *service.pending_open_properties.lock() = true;
    open_map_window(&app, None).inspect_err(|_| {
        *service.pending_open_properties.lock() = false;
    })
}

#[tauri::command]
pub(crate) async fn map_agent_bootstrap(
    service: tauri::State<'_, MapAgentService>,
    engines: tauri::State<'_, crate::engine::SessionEngineManager>,
) -> Result<MapBootstrapResponse, String> {
    let context = service.candidates.context().current()?;
    let resolution = service.bootstrap_session(&context)?;
    bootstrap_map_session(&service, &engines, context, resolution).await
}

#[tauri::command]
pub fn map_agent_session_list(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, MapAgentService>,
) -> Result<Vec<crate::session::SessionMeta>, String> {
    require_map_window(&window)?;
    let context = service.candidates.context().current()?;
    service.map_sessions(&context)
}

#[tauri::command]
pub(crate) async fn map_agent_session_create(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, MapAgentService>,
    engines: tauri::State<'_, crate::engine::SessionEngineManager>,
) -> Result<MapBootstrapResponse, String> {
    require_map_window(&window)?;
    let context = service.candidates.context().current()?;
    let resolution = MapSessionResolution {
        session: service.create_map_session(&context)?,
        candidate_action: CandidateSessionAction::Create,
    };
    bootstrap_map_session(&service, &engines, context, resolution).await
}

#[tauri::command]
pub(crate) async fn map_agent_session_load(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, MapAgentService>,
    engines: tauri::State<'_, crate::engine::SessionEngineManager>,
    session_id: String,
) -> Result<MapBootstrapResponse, String> {
    require_map_window(&window)?;
    let context = service.candidates.context().current()?;
    let resolution = service.session_for_context(&session_id, &context)?;
    bootstrap_map_session(&service, &engines, context, resolution).await
}

#[tauri::command]
pub(crate) async fn map_agent_conversation_reset(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, MapAgentService>,
    engines: tauri::State<'_, crate::engine::SessionEngineManager>,
    session_id: String,
) -> Result<(), String> {
    require_map_window(&window)?;
    let context = service.candidates.context().current()?;
    service.session_for_context(&session_id, &context)?;
    engines.map_conversation_reset(&session_id).await
}

#[tauri::command]
pub(crate) async fn map_agent_conversation_rewind(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, MapAgentService>,
    engines: tauri::State<'_, crate::engine::SessionEngineManager>,
    session_id: String,
    panel_log: serde_json::Value,
) -> Result<(), String> {
    require_map_window(&window)?;
    let context = service.candidates.context().current()?;
    service.session_for_context(&session_id, &context)?;
    engines
        .map_conversation_rewind(&session_id, panel_log)
        .await
}

#[tauri::command]
pub fn map_agent_session_rename(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, MapAgentService>,
    session_id: String,
    name: String,
) -> Result<crate::session::SessionMeta, String> {
    require_map_window(&window)?;
    let context = service.candidates.context().current()?;
    service.session_for_context(&session_id, &context)?;
    let name = name.trim();
    if name.is_empty() {
        return Err("Map Agent session name cannot be empty".to_string());
    }
    if name.chars().count() > 80 {
        return Err("Map Agent session name cannot exceed 80 characters".to_string());
    }
    service
        .sessions
        .rename(&session_id, name)
        .map_err(|error| error.to_string())?;
    Ok(service.session_record(&session_id)?.meta)
}

#[tauri::command]
pub(crate) async fn map_agent_session_delete(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, MapAgentService>,
    engines: tauri::State<'_, crate::engine::SessionEngineManager>,
    session_id: String,
) -> Result<(), String> {
    require_map_window(&window)?;
    let context = service.candidates.context().current()?;
    let resolution = service.session_for_context(&session_id, &context)?;
    engines.delete_map_session(&session_id).await?;
    if let Err(error) = service
        .candidates
        .discard(&resolution.session.meta.project, &session_id)
    {
        eprintln!("eud-agent: deleted Map session candidate cleanup failed: {error}");
    }
    // A deleted team session can never deliver its candidate: its EPS parent
    // must not stay excluded behind a task that no longer exists.
    crate::engine::emit_team_tasks(
        window.app_handle(),
        service.team_tasks_withdrawn(&session_id, TeamWithdrawal::SessionDeleted),
    );
    Ok(())
}
#[tauri::command]
pub async fn map_agent_source_state(
    service: tauri::State<'_, MapAgentService>,
) -> Result<crate::map_context::MapSourceProbe, String> {
    let service = service.inner().clone();
    run_map_blocking("map source state", move || {
        service.candidates.context().probe_current()
    })
    .await
}

async fn run_map_blocking<T>(
    operation: &'static str,
    task: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String>
where
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(task)
        .await
        .map_err(|error| format!("{operation} worker failed: {error}"))?
}

#[tauri::command]
pub async fn map_agent_render(
    service: tauri::State<'_, MapAgentService>,
    command: MapRenderCommand,
) -> Result<tauri::ipc::Response, String> {
    let service = service.inner().clone();
    let bytes = run_map_blocking("map render", move || {
        let image = service.render_rgba(&command)?;
        encode_rgba_png(&image)
    })
    .await?;
    Ok(tauri::ipc::Response::new(bytes))
}

#[tauri::command]
pub async fn map_agent_thumbnail(
    service: tauri::State<'_, MapAgentService>,
    command: MapThumbnailCommand,
) -> Result<tauri::ipc::Response, String> {
    let service = service.inner().clone();
    let bytes = run_map_blocking("map thumbnail", move || {
        let image = service.thumbnail_rgba(&command)?;
        encode_rgba_png(&image)
    })
    .await?;
    Ok(tauri::ipc::Response::new(bytes))
}

#[tauri::command]
pub async fn map_agent_image_preview(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, MapAgentService>,
    command: MapImagePreviewCommand,
) -> Result<tauri::ipc::Response, String> {
    require_map_window(&window)?;
    let service = service.inner().clone();
    let bytes = run_map_blocking("map image preview", move || {
        let (header, png) = service.image_preview(&command)?;
        encode_image_preview_envelope(&header, &png)
    })
    .await?;
    Ok(tauri::ipc::Response::new(bytes))
}

#[tauri::command]
pub async fn map_agent_image_confirm(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, MapAgentService>,
    command: MapImageConfirmCommand,
) -> Result<MapImageConfirmResponse, String> {
    require_map_window(&window)?;
    let service = service.inner().clone();
    let response =
        run_map_blocking("map image confirm", move || service.image_confirm(&command)).await?;
    let _ = window.emit("map_candidate_state", &response.candidate);
    Ok(response)
}
#[tauri::command]
pub async fn map_agent_stamp_preview(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, MapAgentService>,
    command: MapStampPreviewCommand,
) -> Result<StampPlacementReport, String> {
    require_map_window(&window)?;
    let service = service.inner().clone();
    run_map_blocking("map stamp preview", move || service.stamp_preview(&command)).await
}

#[tauri::command]
pub async fn map_agent_stamp_confirm(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, MapAgentService>,
    command: MapStampConfirmCommand,
) -> Result<MapStampConfirmResponse, String> {
    require_map_window(&window)?;
    let service = service.inner().clone();
    let response =
        run_map_blocking("map stamp confirm", move || service.stamp_confirm(&command)).await?;
    let _ = window.emit("map_candidate_state", &response.candidate);
    Ok(response)
}

#[tauri::command]
pub fn map_agent_image_cancel(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, MapAgentService>,
    session_id: String,
) -> Result<(), String> {
    require_map_window(&window)?;
    service.session_record(&session_id)?;
    service.images.clear_session(&session_id);
    Ok(())
}

#[tauri::command]
pub async fn map_agent_catalog(
    service: tauri::State<'_, MapAgentService>,
    command: MapCatalogCommand,
) -> Result<Value, String> {
    let service = service.inner().clone();
    run_map_blocking("map catalog", move || service.catalog(&command)).await
}

#[tauri::command]
pub async fn map_agent_objects(
    service: tauri::State<'_, MapAgentService>,
    command: MapObjectsCommand,
) -> Result<Value, String> {
    let service = service.inner().clone();
    run_map_blocking("map objects", move || service.objects(&command)).await
}

#[tauri::command]
pub async fn map_agent_diff_details(
    service: tauri::State<'_, MapAgentService>,
    session_id: String,
) -> Result<MapDiffDetails, String> {
    let service = service.inner().clone();
    run_map_blocking("map diff", move || service.diff_details(&session_id)).await
}

#[tauri::command]
pub fn map_agent_selection_save(
    service: tauri::State<'_, MapAgentService>,
    session_id: String,
    selection: SelectionMask,
) -> Result<CandidateStateView, String> {
    let session = service.session_record(&session_id)?;
    service
        .candidates
        .save_selection(&session.meta.project, &session_id, selection)
}

#[tauri::command]
pub fn map_agent_selection_delete(
    service: tauri::State<'_, MapAgentService>,
    session_id: String,
    selection_id: String,
) -> Result<CandidateStateView, String> {
    let session = service.session_record(&session_id)?;
    service
        .candidates
        .delete_selection(&session.meta.project, &session_id, &selection_id)
}

#[tauri::command]
pub fn map_agent_candidate_revert(
    app: tauri::AppHandle,
    service: tauri::State<'_, MapAgentService>,
    session_id: String,
    revision: u32,
) -> Result<CandidateStateView, String> {
    let session = service.session_record(&session_id)?;
    let state = service
        .candidates
        .revert(&session.meta.project, &session_id, revision)?;
    crate::engine::emit_team_tasks(
        &app,
        service.team_tasks_withdrawn(&session_id, TeamWithdrawal::Revert(revision)),
    );
    Ok(state)
}

#[tauri::command]
pub fn map_agent_candidate_discard(
    app: tauri::AppHandle,
    service: tauri::State<'_, MapAgentService>,
    session_id: String,
) -> Result<(), String> {
    let session = service.session_record(&session_id)?;
    service
        .candidates
        .discard(&session.meta.project, &session_id)?;
    crate::engine::emit_team_tasks(
        &app,
        service.team_tasks_withdrawn(&session_id, TeamWithdrawal::Discard),
    );
    Ok(())
}

#[tauri::command]
pub fn map_agent_candidate_apply(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, MapAgentService>,
    session_id: String,
) -> Result<CandidateStateView, String> {
    require_map_window(&window)?;
    let state = service.apply(&session_id)?;
    crate::engine::emit_team_tasks(
        window.app_handle(),
        service.team_tasks_applied(&session_id, &state, crate::team::TeamApplyActor::User),
    );
    let _ = window.emit("map_apply_result", &state);
    Ok(state)
}

/// Save the "맵 속성" dialog straight to the source map (trusted Map window
/// only). The resulting state is announced like a candidate Apply.
#[tauri::command]
pub async fn map_agent_properties_save(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, MapAgentService>,
    command: MapPropertiesSaveCommand,
) -> Result<CandidateStateView, String> {
    require_map_window(&window)?;
    let service = service.inner().clone();
    let state = run_map_blocking("map properties save", move || {
        service.properties_save(&command)
    })
    .await?;
    let _ = window.emit("map_apply_result", &state);
    Ok(state)
}

/// Undo the last apply of a team Map session by its session id: the same
/// undo as the Map window's, announced to the owning EPS session and to the
/// Map window when it is open. The main window no longer shows a task card,
/// so nothing in the panel calls this today.
#[tauri::command]
pub fn map_task_apply_undo(
    app: tauri::AppHandle,
    service: tauri::State<'_, MapAgentService>,
    map_session_id: String,
) -> Result<CandidateStateView, String> {
    let team = service.session_record(&map_session_id)?;
    if team.meta.team_parent.is_none() {
        return Err("the requested session is not a team Map session".to_string());
    }
    let applied_sha256 = service.last_applied_sha256(&map_session_id);
    let state = service.undo(&map_session_id)?;
    crate::engine::emit_team_tasks(
        &app,
        service.team_tasks_withdrawn(&map_session_id, TeamWithdrawal::Undo { applied_sha256 }),
    );
    notify_map_window_candidate(&app, &state);
    Ok(state)
}

/// Push a team session's candidate state to the Map window when it is open,
/// so an agent-driven request, apply, or discard shows there immediately.
pub(crate) fn notify_map_window_candidate(app: &tauri::AppHandle, state: &CandidateStateView) {
    if let Some(window) = app.get_webview_window(MAP_WINDOW_LABEL) {
        let _ = window.emit("map_candidate_state", state);
    }
}

#[tauri::command]
pub fn map_agent_apply_undo(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, MapAgentService>,
    session_id: String,
) -> Result<CandidateStateView, String> {
    require_map_window(&window)?;
    let applied_sha256 = service.last_applied_sha256(&session_id);
    let state = service.undo(&session_id)?;
    crate::engine::emit_team_tasks(
        window.app_handle(),
        service.team_tasks_withdrawn(&session_id, TeamWithdrawal::Undo { applied_sha256 }),
    );
    let _ = window.emit("map_apply_result", &state);
    Ok(state)
}

#[tauri::command]
pub(crate) async fn map_agent_chat(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, MapAgentService>,
    engines: tauri::State<'_, crate::engine::SessionEngineManager>,
    command: MapChatCommand,
) -> Result<CandidateStateView, String> {
    require_map_window(&window)?;
    let session = service.session_record(&command.session_id)?;
    let current = service.candidates.context().current()?;
    let current_state = service
        .candidates
        .state(&session.meta.project, &command.session_id)?;
    require_current_source(
        &current.revision.project_id,
        &current.revision.source_path,
        &session.meta.project,
        &current_state.baseline.source_path,
        "Map request",
    )?;
    let request_id = format!("map-{}", uuid::Uuid::new_v4());
    service.candidates.prepare_request(
        &session.meta.project,
        &command.session_id,
        &request_id,
        command.candidate_revision,
        &command.mentions,
    )?;
    let state_before = service
        .candidates
        .state(&session.meta.project, &command.session_id)?;
    let compact_mentions = match compact_mentions(
        &service.candidates,
        &session.meta.project,
        &state_before,
        &command.mentions,
    ) {
        Ok(value) => value,
        Err(error) => {
            service
                .candidates
                .finish_request(&command.session_id, &request_id)?;
            return Err(error);
        }
    };
    let text = format!(
        "[map mention snapshots]\n{}\n\n[user message]\n{}",
        serde_json::to_string(&compact_mentions).map_err(|error| error.to_string())?,
        command.text
    );
    let outcome = engines
        .map_chat(
            &command.session_id,
            request_id.clone(),
            state_before.revision_key,
            text,
            command.attachments,
            crate::engine::MapRunPrompt {
                text: command.text,
                mentions: command.mentions,
                origin: crate::engine::MapRunOrigin::User,
                team_task_id: None,
                parent_session_name: None,
            },
        )
        .await;
    if let Err(error) = outcome {
        service
            .candidates
            .finish_request(&command.session_id, &request_id)?;
        return Err(error);
    }
    let state = match service.candidates.commit_request(
        &session.meta.project,
        &command.session_id,
        &request_id,
    ) {
        Ok(state) => state,
        Err(error) => {
            service
                .candidates
                .finish_request(&command.session_id, &request_id)?;
            return Err(error);
        }
    };
    service
        .candidates
        .finish_request(&command.session_id, &request_id)?;
    let _ = window.emit("map_candidate_state", &state);
    Ok(state)
}

#[tauri::command]
pub(crate) async fn map_agent_cancel(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, MapAgentService>,
    engines: tauri::State<'_, crate::engine::SessionEngineManager>,
    session_id: String,
) -> Result<(), String> {
    require_map_window(&window)?;
    let result = engines.cancel_map_session(&session_id).await;
    service.candidates.cancel_session(&session_id)?;
    result
}

/// The session's latest Map run transcript, for a window that learned of a
/// run from `map_run_started` after its bootstrap: the events emitted between
/// that header and the window's listeners are only in the transcript.
#[tauri::command]
pub(crate) async fn map_agent_run_snapshot(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, MapAgentService>,
    engines: tauri::State<'_, crate::engine::SessionEngineManager>,
    session_id: String,
) -> Result<Option<crate::engine::MapRunTranscript>, String> {
    require_map_window(&window)?;
    service.session_record(&session_id)?;
    Ok(engines.map_run_snapshot(&session_id).await)
}

fn compact_mentions(
    candidates: &CandidateStore,
    project_id: &str,
    state: &CandidateStateView,
    mentions: &[MapMentionSnapshot],
) -> Result<Value, String> {
    let values = mentions
        .iter()
        .map(|mention| {
            Ok(match mention {
                MapMentionSnapshot::Region { selection_id, .. } => {
                    let selection = state
                        .selections
                        .iter()
                        .find(|selection| &selection.selection.id == selection_id);
                    json!({
                        "kind": "region",
                        "id": selection_id,
                        "role": selection.map(|view| view.selection.role),
                        "layers": selection.map(|view| &view.selection.layers),
                        "selectedCells": selection.map(|view| view.selection.selected_cells),
                    })
                }
                MapMentionSnapshot::Object { object_ref, role } => json!({
                    "kind": "object",
                    "object": object_ref,
                    "role": role,
                }),
                MapMentionSnapshot::Palette { entry, qualifiers } => json!({
                    "kind": "palette",
                    "entry": entry,
                    "qualifiers": qualifiers,
                }),
                MapMentionSnapshot::Stamp { selection_id, .. } => {
                    let selection = state
                        .selections
                        .iter()
                        .find(|selection| &selection.selection.id == selection_id);
                    json!({
                        "kind": "stamp",
                        "selectionId": selection_id,
                        "label": selection.map(|view| &view.selection.label),
                        "layers": selection.map(|view| &view.selection.layers),
                        "bounds": selection.map(|view| view.selection.bounds),
                        "selectedCells": selection.map(|view| view.selection.selected_cells),
                    })
                }
                MapMentionSnapshot::ImportedStamp {
                    import_id,
                    snapshot_hash,
                } => candidates.compact_imported_mention(
                    project_id,
                    import_id,
                    snapshot_hash,
                    state.baseline.tileset,
                )?,
                MapMentionSnapshot::Location {
                    location_id,
                    revision_key,
                    baseline_hash,
                } => json!({
                    "kind": "location",
                    "locationId": location_id,
                    "revisionKey": revision_key,
                    "baselineHash": baseline_hash,
                }),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(json!({"candidateRevision": state.current_revision, "mentions": values}))
}

fn require_map_window(window: &tauri::WebviewWindow) -> Result<(), String> {
    if window.label() == MAP_WINDOW_LABEL {
        Ok(())
    } else {
        Err("trusted Map Agent command rejected outside the map-agent window".to_string())
    }
}

fn encode_image_preview_envelope(
    header: &MapImagePreviewHeader,
    png: &[u8],
) -> Result<Vec<u8>, String> {
    if usize::try_from(header.png_byte_length).ok() != Some(png.len()) {
        return Err("image preview header length does not match PNG bytes".to_string());
    }
    let json = serde_json::to_vec(header)
        .map_err(|error| format!("image preview header serialization failed: {error}"))?;
    let json_len = u32::try_from(json.len())
        .map_err(|_| "image preview JSON header exceeds u32".to_string())?;
    let capacity = 8_usize
        .checked_add(json.len())
        .and_then(|value| value.checked_add(png.len()))
        .ok_or_else(|| "image preview envelope length overflow".to_string())?;
    let mut output = Vec::with_capacity(capacity);
    output.extend_from_slice(b"MIP1");
    output.extend_from_slice(&json_len.to_le_bytes());
    output.extend_from_slice(&json);
    output.extend_from_slice(png);
    Ok(output)
}

pub fn encode_rgba_png(image: &isom::RgbaImage) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut output, image.width, image.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|error| format!("PNG header failed: {error}"))?;
        writer
            .write_image_data(&image.rgba)
            .map_err(|error| format!("PNG encoding failed: {error}"))?;
    }
    Ok(output)
}

pub fn mcp_image(image: &isom::RgbaImage) -> Result<Value, String> {
    let png = encode_rgba_png(image)?;
    Ok(json!({
        "image": {
            "mimeType": "image/png",
            "width": image.width,
            "height": image.height,
            "data": base64::engine::general_purpose::STANDARD.encode(png),
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_chat_command_accepts_attachment_ids() {
        let command: MapChatCommand = serde_json::from_value(json!({
            "sessionId": "map-session",
            "text": "첨부를 참고해 지형을 수정해 줘",
            "attachments": ["attachment-1", "attachment-2"],
            "candidateRevision": 3,
            "mentions": []
        }))
        .unwrap();

        assert_eq!(
            command.attachments,
            vec!["attachment-1".to_string(), "attachment-2".to_string()]
        );
    }

    #[test]
    fn map_objects_command_accepts_request_owned_draft() {
        let command: MapObjectsCommand = serde_json::from_value(json!({
            "sessionId": "map-session",
            "layer": "locations",
            "view": "draft",
            "requestId": "map-request",
            "draftGeneration": 2,
            "offset": 0,
            "limit": 500
        }))
        .unwrap();

        assert!(matches!(command.view, Some(MapView::Draft)));
        assert_eq!(command.request_id.as_deref(), Some("map-request"));
        assert_eq!(command.draft_generation, Some(2));
    }

    #[test]
    fn map_session_names_fill_the_first_available_history_slot() {
        let session = |name: &str| crate::session::SessionMeta {
            id: name.to_string(),
            name: name.to_string(),
            project: "project".to_string(),
            kind: crate::session::SessionKind::Map,
            provider: crate::provider::ProviderId::Codex,
            model: "gpt-test".to_string(),
            created_at: 1,
            last_conversation_at: 1,
            team_parent: None,
        };
        assert_eq!(next_map_session_name(&[]), "Map Agent");
        assert_eq!(
            next_map_session_name(&[session("Map Agent"), session("Map Agent 3")]),
            "Map Agent 2"
        );
        assert_eq!(
            next_map_session_name(&[session("Map Agent"), session("Map Agent 2")]),
            "Map Agent 3"
        );
    }

    #[test]
    fn rgba_png_is_binary_and_not_base64_json() {
        let image = isom::RgbaImage {
            width: 1,
            height: 1,
            rgba: vec![1, 2, 3, 255],
        };
        let png = encode_rgba_png(&image).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn current_open_map_must_match_candidate_source_for_apply_and_undo() {
        let expected = Path::new(r"C:\maps\active.scx");
        assert!(require_current_source("project", expected, "project", expected, "Apply").is_ok());
        assert!(require_current_source(
            "project",
            Path::new(r"C:\maps\other.scx"),
            "project",
            expected,
            "Apply",
        )
        .is_err());
        assert!(
            require_current_source("other-project", expected, "project", expected, "undo",)
                .is_err()
        );
    }

    #[test]
    fn bound_session_load_resolution_reopens_without_sweeping_active_draft() {
        let root = std::env::temp_dir().join(format!("map-agent-load-{}", uuid::Uuid::new_v4()));
        let dirs = DataDirs::from_bases(&root.join("roaming"), &root.join("local"));
        dirs.ensure_dirs().unwrap();
        let source = root.join("source.scx");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("crates")
            .join("isom")
            .join("tests")
            .join("fixtures")
            .join("map_agent_rich.scx");
        std::fs::copy(fixture, &source).unwrap();
        let context_service = crate::map_context::MapContextService::new(dirs.clone());
        let revision = context_service
            .revision_for_path("project".to_string(), &source)
            .unwrap();
        let chk = isom::chk_extract(&source).unwrap();
        let context = crate::map_context::MapContextSnapshot {
            revision,
            saved_source_notice: "saved".to_string(),
            source_file_size: std::fs::metadata(&source).unwrap().len(),
            starcraft_path: PathBuf::from(r"C:\Program Files (x86)\StarCraft"),
            digest: crate::chk::digest_chk(&chk),
        };
        let candidates = CandidateStore::new(
            (dirs.clone()).clone(),
            crate::map_import::MapImportStore::new(dirs.clone()),
        );
        let view = candidates.create_session("map-session", &context).unwrap();
        let service = MapAgentService::new(
            dirs,
            candidates.clone(),
            crate::write_coordinator::ProjectWriteCoordinator::silent(),
        );
        service
            .sessions
            .save(&crate::session::SessionRecord {
                meta: crate::session::SessionMeta {
                    id: "map-session".to_string(),
                    name: "Map Agent".to_string(),
                    project: "project".to_string(),
                    kind: crate::session::SessionKind::Map,
                    provider: crate::provider::ProviderId::Codex,
                    model: "gpt-test".to_string(),
                    created_at: 1,
                    last_conversation_at: 1,
                    team_parent: None,
                },
                provider_binding: crate::provider::ProviderBinding::new(
                    crate::provider::ProviderId::Codex,
                    "gpt-test".to_string(),
                    None,
                )
                .unwrap(),
                pending_request_ids: Vec::new(),
                context_usage: None,
                panel_log: Value::Null,
                context_state: Default::default(),
                task_state: Default::default(),
                autonomous_run: None,
                team_tasks: Vec::new(),
            })
            .unwrap();
        let target = SelectionMask::canonical(
            "target",
            "target",
            view.revision_key.clone(),
            crate::map_model::SelectionRole::Target,
            [MapLayer::Terrain].into_iter().collect(),
            crate::map_model::MaskGrid {
                width: view.baseline.width,
                height: view.baseline.height,
                rows: (0..view.baseline.height)
                    .map(|y| RowSpan {
                        y,
                        spans: vec![(0, view.baseline.width)],
                    })
                    .collect(),
            },
        )
        .unwrap();
        candidates
            .save_selection("project", "map-session", target.clone())
            .unwrap();
        candidates
            .prepare_request(
                "project",
                "map-session",
                "request",
                0,
                &[MapMentionSnapshot::Region {
                    selection_id: target.id.clone(),
                    snapshot_hash: target.snapshot_hash(),
                    source_revision: target.source_revision.clone(),
                }],
            )
            .unwrap();
        candidates
            .draft_begin("project", "map-session", "request")
            .unwrap();
        let draft_path = candidates.draft_map("map-session", "request").unwrap();
        let draft_bytes = std::fs::read(&draft_path).unwrap();
        let draft_hash = crate::map_model::hex_sha256(&draft_bytes);
        let object_source = service
            .object_source(&MapObjectsCommand {
                session_id: "map-session".to_string(),
                layer: "locations".to_string(),
                view: Some(MapView::Draft),
                request_id: Some("request".to_string()),
                draft_generation: Some(2),
                offset: 0,
                limit: 500,
            })
            .unwrap();
        assert_eq!(object_source.map, draft_path);
        assert_eq!(
            object_source.revision_key,
            format!("{}:draft:request:g2", view.revision_key)
        );
        assert!(!object_source.annotate_candidate_ids);

        let resolution = service
            .session_for_context("map-session", &context)
            .unwrap();
        assert!(matches!(
            resolution.candidate_action,
            CandidateSessionAction::Open
        ));
        candidates
            .open_session(&resolution.session.meta.id, &context)
            .unwrap();

        assert_eq!(std::fs::read(&draft_path).unwrap(), draft_bytes);
        assert_eq!(
            crate::map_model::hex_sha256(&std::fs::read(&draft_path).unwrap()),
            draft_hash
        );
        candidates.finish_request("map-session", "request").unwrap();
        assert!(!draft_path.exists());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    #[ignore = "requires installed StarCraft terrain assets"]
    fn direct_image_preview_protect_confirm_and_attachment_free_replay_are_safe() {
        let root = std::env::temp_dir().join(format!("map-agent-image-{}", uuid::Uuid::new_v4()));
        let dirs = DataDirs::from_bases(&root.join("roaming"), &root.join("local"));
        dirs.ensure_dirs().unwrap();
        let source = root.join("source.scx");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("crates")
            .join("isom")
            .join("tests")
            .join("fixtures")
            .join("map_agent_rich.scx");
        std::fs::copy(fixture, &source).unwrap();
        let source_before = std::fs::read(&source).unwrap();
        let context_service = crate::map_context::MapContextService::new(dirs.clone());
        let revision = context_service
            .revision_for_path("project".to_string(), &source)
            .unwrap();
        let chk = isom::chk_extract(&source).unwrap();
        let context = crate::map_context::MapContextSnapshot {
            revision,
            saved_source_notice: "saved".to_string(),
            source_file_size: std::fs::metadata(&source).unwrap().len(),
            starcraft_path: PathBuf::from(r"C:\Program Files (x86)\StarCraft"),
            digest: crate::chk::digest_chk(&chk),
        };
        let candidates = CandidateStore::new(
            (dirs.clone()).clone(),
            crate::map_import::MapImportStore::new(dirs.clone()),
        );
        let view = candidates.create_session("map-session", &context).unwrap();
        let service = MapAgentService::new(
            dirs.clone(),
            candidates.clone(),
            crate::write_coordinator::ProjectWriteCoordinator::silent(),
        );
        service
            .sessions
            .save(&crate::session::SessionRecord {
                meta: crate::session::SessionMeta {
                    id: "map-session".to_string(),
                    name: "Map Agent".to_string(),
                    project: "project".to_string(),
                    kind: crate::session::SessionKind::Map,
                    provider: crate::provider::ProviderId::Codex,
                    model: "gpt-test".to_string(),
                    created_at: 1,
                    last_conversation_at: 1,
                    team_parent: None,
                },
                provider_binding: crate::provider::ProviderBinding::new(
                    crate::provider::ProviderId::Codex,
                    "gpt-test".to_string(),
                    None,
                )
                .unwrap(),
                pending_request_ids: Vec::new(),
                context_usage: None,
                panel_log: Value::Null,
                context_state: Default::default(),
                task_state: Default::default(),
                autonomous_run: None,
                team_tasks: Vec::new(),
            })
            .unwrap();

        let stored_target = SelectionMask::canonical(
            "stored-target",
            "stored-target",
            view.revision_key.clone(),
            crate::map_model::SelectionRole::Target,
            [MapLayer::Terrain].into_iter().collect(),
            crate::map_model::MaskGrid {
                width: view.baseline.width,
                height: view.baseline.height,
                rows: vec![RowSpan {
                    y: view.baseline.height - 1,
                    spans: vec![(view.baseline.width - 1, view.baseline.width)],
                }],
            },
        )
        .unwrap();
        candidates
            .save_selection("project", "map-session", stored_target)
            .unwrap();
        let protect = SelectionMask::canonical(
            "protect",
            "protect",
            view.revision_key.clone(),
            crate::map_model::SelectionRole::Protect,
            Default::default(),
            crate::map_model::MaskGrid {
                width: view.baseline.width,
                height: view.baseline.height,
                rows: (0..view.baseline.height)
                    .map(|y| RowSpan {
                        y,
                        spans: vec![(0, view.baseline.width)],
                    })
                    .collect(),
            },
        )
        .unwrap();
        candidates
            .save_selection("project", "map-session", protect)
            .unwrap();

        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png, 8, 4);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            let mut rgba = Vec::with_capacity(8 * 4 * 4);
            for y in 0_u8..4 {
                for x in 0_u8..8 {
                    rgba.extend_from_slice(&[
                        x.saturating_mul(31),
                        y.saturating_mul(63),
                        255_u8.saturating_sub(x.saturating_mul(19)),
                        255,
                    ]);
                }
            }
            writer.write_image_data(&rgba).unwrap();
        }
        let attachment = service
            .attachments
            .stage("terrain.png", "image/png", &png)
            .unwrap();
        let placement = MapImagePlacement {
            x: 0,
            y: 0,
            width: 8,
            height: 4,
        };
        let preview_command = MapImagePreviewCommand {
            session_id: "map-session".to_string(),
            attachment_id: attachment.id.clone(),
            revision_key: view.revision_key.clone(),
            placement,
            preview_sequence: 1,
        };
        let (blocked_preview, blocked_png) = service.image_preview(&preview_command).unwrap();
        assert_eq!(&blocked_png[..8], b"\x89PNG\r\n\x1a\n");
        assert!(blocked_preview.report.changed_cells > 0);
        assert_eq!(
            blocked_preview.report.protected_conflicts,
            blocked_preview.report.changed_cells
        );
        let current_before = candidates
            .current_map("project", "map-session")
            .and_then(|path| std::fs::read(path).map_err(|error| error.to_string()))
            .unwrap();
        let blocked = service
            .image_confirm(&MapImageConfirmCommand {
                session_id: "map-session".to_string(),
                attachment_id: attachment.id.clone(),
                revision_key: view.revision_key.clone(),
                placement,
                preview_digest: blocked_preview.report.tile_grid_sha256,
                preview_sequence: 1,
            })
            .unwrap_err();
        assert!(blocked.contains("persistently protected"));
        assert_eq!(
            std::fs::read(candidates.current_map("project", "map-session").unwrap()).unwrap(),
            current_before
        );
        assert_eq!(
            candidates
                .state("project", "map-session")
                .unwrap()
                .current_revision,
            0
        );

        candidates
            .delete_selection("project", "map-session", "protect")
            .unwrap();
        let preview_command = MapImagePreviewCommand {
            preview_sequence: 2,
            ..preview_command
        };
        let (preview, _) = service.image_preview(&preview_command).unwrap();
        assert_eq!(preview.report.protected_conflicts, 0);
        assert_eq!(preview.report.outside_authority_conflicts, 0);
        let stale_digest = service
            .image_confirm(&MapImageConfirmCommand {
                session_id: "map-session".to_string(),
                attachment_id: attachment.id.clone(),
                revision_key: view.revision_key.clone(),
                placement,
                preview_digest: "stale-preview-digest".to_string(),
                preview_sequence: 2,
            })
            .unwrap_err();
        assert!(stale_digest.contains("preview is stale"));
        assert_eq!(
            candidates
                .state("project", "map-session")
                .unwrap()
                .current_revision,
            0
        );
        let confirmed = service
            .image_confirm(&MapImageConfirmCommand {
                session_id: "map-session".to_string(),
                attachment_id: attachment.id,
                revision_key: view.revision_key,
                placement,
                preview_digest: preview.report.tile_grid_sha256.clone(),
                preview_sequence: 2,
            })
            .unwrap();
        assert_eq!(confirmed.candidate.current_revision, 1);
        assert_eq!(
            confirmed.report.tile_grid_sha256,
            preview.report.tile_grid_sha256
        );
        assert_eq!(std::fs::read(&source).unwrap(), source_before);
        let manifest = dirs
            .map_candidates_dir()
            .join("project")
            .join("map-session")
            .join("revisions")
            .join("r0001.json");
        let manifest: Value = serde_json::from_slice(&std::fs::read(manifest).unwrap()).unwrap();
        assert_eq!(manifest["imageConversions"][0]["kind"], "image_conversion");
        assert!(manifest["imageConversions"][0]
            .get("attachmentId")
            .is_none());
        assert!(manifest["imageConversions"][0].get("path").is_none());
        let candidate_path = candidates.current_map("project", "map-session").unwrap();
        let candidate_chk = isom::chk_extract(&candidate_path).unwrap();
        let sections = crate::chk::assemble_sections(&crate::chk::walk_sections(&candidate_chk));
        assert_eq!(sections.get("MTXM"), sections.get("TILE"));

        service.attachments.delete_session("map-session").unwrap();
        candidates.revert("project", "map-session", 0).unwrap();
        let replayed = candidates.revert("project", "map-session", 1).unwrap();
        assert_eq!(replayed.current_hash, confirmed.candidate.current_hash);
        std::fs::remove_dir_all(root).ok();
    }
}

#[cfg(test)]
mod team_handoff_tests {
    use super::*;
    use crate::map_candidate::{CandidateRevisionView, CandidateStateView, SelectionView};
    use crate::map_model::{LayerDiffCount, MapDiff, MapLayer, SelectionRole, TileRect};

    fn selection(id: &str, role: SelectionRole) -> SelectionView {
        SelectionView {
            selection: SelectionMask {
                id: id.to_string(),
                label: id.to_string(),
                source_revision: "r0:hash".to_string(),
                role,
                layers: [MapLayer::Terrain].into_iter().collect(),
                bounds: TileRect {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                },
                selected_cells: 1,
                rows: vec![RowSpan {
                    y: 0,
                    spans: vec![(0, 0)],
                }],
            },
            snapshot_hash: format!("snap-{id}"),
        }
    }

    fn state(revision: u32, diff: Option<MapDiff>) -> CandidateStateView {
        CandidateStateView {
            session_id: "map-team".to_string(),
            baseline: crate::map_model::MapRevision {
                project_id: "project".to_string(),
                source_path: std::path::PathBuf::from("maps/a.scx"),
                file_sha256: "a".repeat(64),
                chk_sha256: "c".repeat(64),
                mtime_ns: 0,
                tileset: crate::map_model::Tileset::Platform,
                width: 64,
                height: 64,
            },
            current_revision: revision,
            current_hash: "b".repeat(64),
            revision_key: format!("r{revision}:hash"),
            revisions: diff
                .map(|diff| {
                    vec![CandidateRevisionView {
                        revision,
                        parent: revision.saturating_sub(1),
                        request_id: "map-x".to_string(),
                        map_sha256: "b".repeat(64),
                        diff: diff.clone(),
                        verification: crate::map_model::VerificationReport {
                            valid: true,
                            errors: Vec::new(),
                            warnings: Vec::new(),
                            diff,
                            candidate_sha256: "b".repeat(64),
                            canonical_digest: String::new(),
                            extra_assets_digest: String::new(),
                        },
                    }]
                })
                .unwrap_or_default(),
            selections: vec![
                selection("target", SelectionRole::Target),
                selection("protect", SelectionRole::Protect),
            ],
            stale: false,
            source_diverged: false,
            can_apply: revision > 0,
            can_undo: false,
        }
    }

    fn task() -> crate::team::TeamTask {
        crate::team::TeamTask {
            id: "task-1".to_string(),
            parent_request_id: "req".to_string(),
            map_session_id: "map-team".to_string(),
            map_request_id: Some("map-1".to_string()),
            goal: "전부 공허로".to_string(),
            layers: vec![MapLayer::Terrain, MapLayer::Units],
            selection_ids: vec!["target".to_string()],
            location_ids: vec![3, 7],
            source_map_sha256_at_create: "a".repeat(64),
            status: crate::team::TeamTaskStatus::Queued,
            candidate: None,
            applied_source_sha256: None,
            applied_by: None,
            created_at: 1,
            updated_at: 1,
        }
    }

    #[test]
    fn team_mentions_project_target_selections_and_locations_onto_the_candidate() {
        let state = state(0, None);
        let mentions =
            MapAgentService::team_mentions(&state, &["target".to_string()], &[3]).unwrap();
        assert_eq!(
            mentions,
            vec![
                MapMentionSnapshot::Region {
                    selection_id: "target".to_string(),
                    snapshot_hash: "snap-target".to_string(),
                    source_revision: "r0:hash".to_string(),
                },
                MapMentionSnapshot::Location {
                    location_id: 3,
                    revision_key: "r0:hash".to_string(),
                    baseline_hash: "a".repeat(64),
                },
            ]
        );
        let protect =
            MapAgentService::team_mentions(&state, &["protect".to_string()], &[]).unwrap_err();
        assert!(protect.contains("not a target"), "{protect}");
        let missing =
            MapAgentService::team_mentions(&state, &["nope".to_string()], &[]).unwrap_err();
        assert!(missing.contains("does not exist"), "{missing}");
    }

    #[test]
    fn team_request_text_names_the_parent_layers_and_goal() {
        let text = MapAgentService::team_request_text(
            "메인 세션",
            "a3f9c2d1-7b1e-4c2a-9d0f-1234567890ab",
            &task(),
            None,
            &json!([{"kind": "region"}]),
        );
        assert!(text.starts_with("[map mention snapshots]\n[{\"kind\":\"region\"}]"));
        assert!(
            text.contains("EPS session \"메인 세션\" (session id a3f9c2d1) as team task task-1")
        );
        assert!(text.contains("Change only these layers: terrain, units."));
        assert!(text.contains("Location context: #3, #7."));
        // The team session keeps the Map window's iterate-then-finalize way of
        // working and owns the design; the goal's constraints alone bind it.
        assert!(text.contains("exactly as you would a request the user typed in this window"));
        assert!(text.contains("the choice of tiles, doodads, objects, and layout is yours"));
        assert!(text.ends_with("[goal]\n전부 공허로"));
        assert!(!text.contains("It revises team task"));

        let revising = MapAgentService::team_request_text(
            "메인 세션",
            "a3f9c2d1-7b1e-4c2a-9d0f-1234567890ab",
            &task(),
            Some("task-0"),
            &json!([]),
        );
        assert!(revising.contains(
            "It revises team task task-0, which you worked on earlier in this conversation"
        ));
    }

    #[test]
    fn team_scope_is_the_target_union_minus_protect() {
        let rect = |x, y, width, height| crate::team::TeamTileRect {
            x,
            y,
            width,
            height,
        };
        assert_eq!(
            MapAgentService::team_scope_rows(8, 8, &[], &[]).unwrap(),
            None,
            "no scope leaves the task unscoped"
        );
        let rows = MapAgentService::team_scope_rows(
            8,
            8,
            &[rect(0, 0, 4, 2), rect(2, 1, 4, 2)],
            &[rect(1, 0, 1, 3)],
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            rows,
            vec![
                RowSpan {
                    y: 0,
                    spans: vec![(0, 1), (2, 4)],
                },
                RowSpan {
                    y: 1,
                    spans: vec![(0, 1), (2, 6)],
                },
                RowSpan {
                    y: 2,
                    spans: vec![(2, 6)],
                },
            ]
        );
        // Protect alone cuts cells out of the whole map.
        let rows = MapAgentService::team_scope_rows(4, 2, &[], &[rect(0, 0, 4, 1)])
            .unwrap()
            .unwrap();
        assert_eq!(
            rows,
            vec![RowSpan {
                y: 1,
                spans: vec![(0, 4)],
            }]
        );
        let outside = MapAgentService::team_scope_rows(8, 8, &[rect(6, 0, 3, 1)], &[]).unwrap_err();
        assert!(outside.contains("leaves the 8x8 map"), "{outside}");
        let empty = MapAgentService::team_scope_rows(8, 8, &[rect(0, 0, 0, 1)], &[]).unwrap_err();
        assert!(empty.contains("is empty"), "{empty}");
        let covered =
            MapAgentService::team_scope_rows(8, 8, &[rect(1, 1, 2, 2)], &[rect(0, 0, 4, 4)])
                .unwrap_err();
        assert!(
            covered.contains("protect covers the whole target"),
            "{covered}"
        );
    }

    /// The scope becomes a target selection on the team session's candidate,
    /// so the request authority refuses a change outside it or on a layer
    /// the task was not granted, exactly as for a region the user selected.
    #[test]
    fn team_scope_mention_scopes_the_team_request_authority() {
        let fixture = team_fixture();
        let TeamFixture {
            map_project_id,
            context,
            service,
            ..
        } = &fixture;
        let parent = eps_parent("rpg");
        service.sessions.save(&parent).unwrap();
        let (team, state, _) = service
            .fresh_team_session_in(&parent, context, "rpg")
            .unwrap();
        let rows = MapAgentService::team_scope_rows(
            state.baseline.width,
            state.baseline.height,
            &[crate::team::TeamTileRect {
                x: 2,
                y: 3,
                width: 4,
                height: 2,
            }],
            &[],
        )
        .unwrap()
        .unwrap();
        let (selection_id, mention) = service
            .team_scope_mention("eps-1", "메인", &team, &state, &[MapLayer::Terrain], rows)
            .unwrap();
        assert_eq!(selection_id, "team-eps-1");
        assert!(matches!(
            &mention,
            MapMentionSnapshot::Region { selection_id, .. } if selection_id == "team-eps-1"
        ));
        let state = service
            .candidates
            .state(map_project_id, &team.meta.id)
            .unwrap();
        assert!(state
            .selections
            .iter()
            .any(|view| view.selection.id == selection_id
                && view.selection.role == crate::map_model::SelectionRole::Target));

        let authority = service
            .candidates
            .prepare_request(map_project_id, &team.meta.id, "req-scope", 0, &[mention])
            .unwrap();
        assert!(authority.allows(MapLayer::Terrain, 2, 3));
        assert!(authority.allows(MapLayer::Terrain, 5, 4));
        assert!(!authority.allows(MapLayer::Terrain, 6, 4));
        assert!(!authority.allows(MapLayer::Terrain, 2, 5));
        assert!(!authority.allows(MapLayer::Units, 2, 3));
        service
            .candidates
            .finish_request(&team.meta.id, "req-scope")
            .unwrap();
    }

    /// A revision shares its task's team session, so withdrawing the ready
    /// candidate must not rewrite an earlier task whose apply still stands,
    /// and an undo withdraws only the one apply it reverted.
    #[test]
    fn a_withdrawal_ends_only_the_tasks_it_undid_in_a_shared_team_session() {
        use crate::team::TeamTaskStatus;
        let fixture = team_fixture();
        let service = &fixture.service;
        let tasks = |ready: TeamTaskStatus| {
            let mut parent = eps_parent("rpg");
            parent.team_tasks = [
                ("task-a", TeamTaskStatus::Applied, 1),
                ("task-b", TeamTaskStatus::Applied, 2),
                ("task-c", ready, 3),
            ]
            .into_iter()
            .map(|(id, status, updated_at)| crate::team::TeamTask {
                id: id.to_string(),
                status,
                applied_source_sha256: Some(format!("{}-output", &id[5..])),
                updated_at,
                ..task()
            })
            .collect();
            service.sessions.save(&parent).unwrap();
        };
        let statuses = || {
            service
                .sessions
                .load("eps-1")
                .unwrap()
                .team_tasks
                .into_iter()
                .map(|task| (task.id, task.status.label()))
                .collect::<Vec<_>>()
        };

        tasks(TeamTaskStatus::CandidateReady);
        service.team_tasks_withdrawn("map-team", TeamWithdrawal::Discard);
        assert_eq!(
            statuses(),
            vec![
                ("task-a".to_string(), "applied"),
                ("task-b".to_string(), "applied"),
                ("task-c".to_string(), "discarded"),
            ]
        );

        tasks(TeamTaskStatus::Superseded);
        service.team_tasks_withdrawn(
            "map-team",
            TeamWithdrawal::Undo {
                applied_sha256: Some("b-output".to_string()),
            },
        );
        assert_eq!(
            statuses(),
            vec![
                ("task-a".to_string(), "applied"),
                ("task-b".to_string(), "discarded"),
                ("task-c".to_string(), "superseded"),
            ]
        );
        // Undoing the user's own apply (no task produced that output)
        // leaves every task's apply standing.
        tasks(TeamTaskStatus::Superseded);
        service.team_tasks_withdrawn(
            "map-team",
            TeamWithdrawal::Undo {
                applied_sha256: Some("user-output".to_string()),
            },
        );
        assert!(statuses()
            .iter()
            .take(2)
            .all(|(_, status)| *status == "applied"));

        tasks(TeamTaskStatus::CandidateReady);
        service.team_tasks_withdrawn("map-team", TeamWithdrawal::SessionDeleted);
        assert!(statuses().iter().all(|(_, status)| *status == "discarded"));
    }

    /// A follow-up edit continues the revised task's own team session; a
    /// task with nothing to build on, or whose session is gone, is refused
    /// with the fresh-request recovery.
    #[test]
    fn a_revising_task_continues_the_revised_task_team_session() {
        let fixture = team_fixture();
        let TeamFixture {
            context, service, ..
        } = &fixture;
        let parent = eps_parent("rpg");
        service.sessions.save(&parent).unwrap();
        let (team, _, _) = service
            .fresh_team_session_in(&parent, context, "rpg")
            .unwrap();
        let revised = |status| crate::team::TeamTask {
            map_session_id: team.meta.id.clone(),
            status,
            ..task()
        };

        for status in [
            crate::team::TeamTaskStatus::CandidateReady,
            crate::team::TeamTaskStatus::Applied,
        ] {
            let (record, state) = service
                .continued_team_session(&parent, &revised(status))
                .unwrap();
            assert_eq!(record.meta.id, team.meta.id);
            assert_eq!(state.current_revision, 0);
        }
        let failed = service
            .continued_team_session(&parent, &revised(crate::team::TeamTaskStatus::Discarded))
            .unwrap_err();
        assert!(failed.contains("without revisesTaskId"), "{failed}");

        let mut stranger = eps_parent("rpg");
        stranger.meta.id = "eps-2".to_string();
        let foreign = service
            .continued_team_session(
                &stranger,
                &revised(crate::team::TeamTaskStatus::CandidateReady),
            )
            .unwrap_err();
        assert!(foreign.contains("was retired"), "{foreign}");

        service.sessions.delete(&team.meta.id).unwrap();
        let retired = service
            .continued_team_session(&parent, &revised(crate::team::TeamTaskStatus::Applied))
            .unwrap_err();
        assert!(retired.contains("was retired"), "{retired}");
    }

    fn eps_parent(project: &str) -> crate::session::SessionRecord {
        crate::session::SessionRecord {
            meta: crate::session::SessionMeta {
                id: "eps-1".to_string(),
                name: "메인".to_string(),
                project: project.to_string(),
                kind: crate::session::SessionKind::Eps,
                provider: crate::provider::ProviderId::Codex,
                model: "gpt-test".to_string(),
                created_at: 1,
                last_conversation_at: 1,
                team_parent: None,
            },
            provider_binding: crate::provider::ProviderBinding::new(
                crate::provider::ProviderId::Codex,
                "gpt-test".to_string(),
                None,
            )
            .unwrap(),
            pending_request_ids: Vec::new(),
            context_usage: None,
            panel_log: Value::Null,
            context_state: Default::default(),
            task_state: Default::default(),
            autonomous_run: None,
            team_tasks: Vec::new(),
        }
    }

    /// EPS sessions are keyed by manifest name and Map sessions by the
    /// project-root hash: the live `rpg` project failed with "the current
    /// source map belongs to another project" when the two were compared.
    /// A temp-root Map service bound to a copy of the rich fixture map, with
    /// the Map project id derived from the root like the live app does.
    struct TeamFixture {
        root: PathBuf,
        source: PathBuf,
        map_project_id: String,
        context: crate::map_context::MapContextSnapshot,
        service: MapAgentService,
    }

    impl Drop for TeamFixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).ok();
        }
    }

    fn team_fixture() -> TeamFixture {
        let root = std::env::temp_dir().join(format!("map-agent-team-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let dirs = DataDirs::from_bases(&root.join("roaming"), &root.join("local"));
        dirs.ensure_dirs().unwrap();
        let source = root.join("rpg.scx");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("crates")
            .join("isom")
            .join("tests")
            .join("fixtures")
            .join("map_agent_rich.scx");
        std::fs::copy(fixture, &source).unwrap();
        let map_project_id = crate::map_context::project_id_for_path(root.clone());
        let context_service = crate::map_context::MapContextService::new(dirs.clone());
        let revision = context_service
            .revision_for_path(map_project_id.clone(), &source)
            .unwrap();
        let chk = isom::chk_extract(&source).unwrap();
        let context = crate::map_context::MapContextSnapshot {
            revision,
            saved_source_notice: "saved".to_string(),
            source_file_size: std::fs::metadata(&source).unwrap().len(),
            starcraft_path: PathBuf::from(r"C:\Program Files (x86)\StarCraft"),
            digest: crate::chk::digest_chk(&chk),
        };
        let candidates = CandidateStore::new(
            dirs.clone(),
            crate::map_import::MapImportStore::new(dirs.clone()),
        );
        let service = MapAgentService::new(
            dirs,
            candidates,
            crate::write_coordinator::ProjectWriteCoordinator::silent(),
        );
        TeamFixture {
            root,
            source,
            map_project_id,
            context,
            service,
        }
    }

    #[test]
    fn team_session_is_keyed_by_the_map_project_id_not_the_eps_manifest_name() {
        let fixture = team_fixture();
        let TeamFixture {
            source,
            map_project_id,
            context,
            service,
            ..
        } = &fixture;
        let (source, map_project_id, context, service) =
            (source.clone(), map_project_id.clone(), context, service);
        assert_ne!(map_project_id, "rpg");
        let parent = eps_parent("rpg");

        let other = service
            .fresh_team_session_in(&parent, context, "other")
            .unwrap_err();
        assert!(other.contains("not the open project 'other'"), "{other}");
        assert!(service.sessions.team_session_of("eps-1").unwrap().is_none());

        let (record, state, retired) = service
            .fresh_team_session_in(&parent, context, "rpg")
            .unwrap();
        assert!(retired.is_empty());
        assert_eq!(record.meta.kind, crate::session::SessionKind::Map);
        assert_eq!(record.meta.project, map_project_id);
        assert_eq!(record.meta.team_parent.as_deref(), Some("eps-1"));
        assert_eq!(record.meta.name, "메인 · 맵 작업");
        assert_eq!(state.current_revision, 0);
        assert_eq!(state.baseline.source_path, source);
    }

    /// Every map task that revises nothing runs in a fresh team session on
    /// the saved source map:
    /// the earlier session is retired (its candidate would otherwise stack
    /// under the next request) unless the Map window can still undo its
    /// Apply against the current source.
    #[test]
    fn each_map_task_gets_a_fresh_team_session_and_retires_the_earlier_one() {
        let fixture = team_fixture();
        let TeamFixture {
            source,
            map_project_id,
            context,
            service,
            root,
        } = &fixture;
        let parent = eps_parent("rpg");
        service.sessions.save(&parent).unwrap();

        let (first, _, retired) = service
            .fresh_team_session_in(&parent, context, "rpg")
            .unwrap();
        assert!(retired.is_empty());
        let (second, second_state, retired) = service
            .fresh_team_session_in(&parent, context, "rpg")
            .unwrap();
        assert_ne!(
            second.meta.id, first.meta.id,
            "a task never reuses a team session"
        );
        assert_eq!(second_state.current_revision, 0);
        assert_eq!(
            retired
                .iter()
                .map(|meta| meta.id.as_str())
                .collect::<Vec<_>>(),
            vec![first.meta.id.as_str()],
            "an earlier session without an undoable apply is retired"
        );
        assert_eq!(
            service
                .sessions
                .team_session_of("eps-1")
                .unwrap()
                .map(|meta| meta.id),
            Some(second.meta.id.clone()),
            "the newest team session is the parent's current one"
        );
        // The engine manager deletes retired sessions; until it has, they are
        // reported again so a failed retirement is retried by the next task.
        service.sessions.delete(&first.meta.id).unwrap();

        // The second session applies its candidate: the saved source is now
        // that Apply's output, so the window can still undo it.
        let backup = root.join("second.backup.scx");
        std::fs::copy(source, &backup).unwrap();
        let source_sha256 = context.revision.file_sha256.clone();
        service
            .candidates
            .complete_apply(
                map_project_id,
                &second.meta.id,
                &crate::mapsafe::CandidateApplyRecord {
                    source_path: source.clone(),
                    backup_path: backup,
                    before_sha256: "b".repeat(64),
                    applied_sha256: source_sha256.clone(),
                },
            )
            .unwrap();
        assert!(service
            .candidates
            .apply_undoable_against(map_project_id, &second.meta.id, &source_sha256)
            .unwrap());
        let (third, _, retired) = service
            .fresh_team_session_in(&parent, context, "rpg")
            .unwrap();
        assert_ne!(third.meta.id, second.meta.id);
        assert!(
            retired.is_empty(),
            "a session whose Apply is still undoable survives: {retired:?}"
        );

        // Once the source moves on, that undo is retired with its session.
        let mut moved = context.clone();
        moved.revision.file_sha256 = "c".repeat(64);
        let (_, _, retired) = service
            .fresh_team_session_in(&parent, &moved, "rpg")
            .unwrap();
        let mut retired_ids = retired
            .iter()
            .map(|meta| meta.id.as_str())
            .collect::<Vec<_>>();
        retired_ids.sort();
        let mut expected = vec![second.meta.id.as_str(), third.meta.id.as_str()];
        expected.sort();
        assert_eq!(retired_ids, expected);
    }

    /// The Map window opens on the team session the moment its task runs: a
    /// queued or running task is not a reason to fall back to another session.
    #[test]
    fn a_team_session_with_a_running_task_is_a_valid_window_target() {
        let fixture = team_fixture();
        let service = &fixture.service;
        let parent = eps_parent("rpg");
        service.sessions.save(&parent).unwrap();
        let (team, _, _) = service
            .fresh_team_session_in(&parent, &fixture.context, "rpg")
            .unwrap();
        for status in [
            crate::team::TeamTaskStatus::Queued,
            crate::team::TeamTaskStatus::Running,
        ] {
            let mut task = task();
            task.map_session_id = team.meta.id.clone();
            task.status = status.clone();
            service
                .sessions
                .upsert_team_task(&parent.meta.id, task)
                .unwrap();
            let resolution = service
                .session_for_context(&team.meta.id, &fixture.context)
                .unwrap_or_else(|error| panic!("{status:?} task blocked the window: {error}"));
            assert_eq!(resolution.session.meta.id, team.meta.id);
            assert!(matches!(
                resolution.candidate_action,
                CandidateSessionAction::Open
            ));
        }
        *service.pending_open_session.lock() = Some(team.meta.id.clone());
        let bootstrapped = service.bootstrap_session(&fixture.context).unwrap();
        assert_eq!(
            bootstrapped.session.meta.id, team.meta.id,
            "the requested running team session is bootstrapped, not the default one"
        );
        assert!(service.pending_open_session.lock().is_none());
    }

    /// A team run that died with the process leaves a pending native receipt
    /// that would refuse every later turn on the team session. Startup marks
    /// the task interrupted and resets that session's conversation for the
    /// user, so the EPS agent's next map request runs without a manual reset.
    #[test]
    fn startup_resets_the_team_session_whose_run_died_with_the_process() {
        let fixture = team_fixture();
        let service = &fixture.service;
        let parent = eps_parent("rpg");
        service.sessions.save(&parent).unwrap();
        let (mut team, _, _) = service
            .fresh_team_session_in(&parent, &fixture.context, "rpg")
            .unwrap();
        team.provider_binding.conversation = crate::provider::ProviderConversationState::Codex {
            thread_id: Some("thread-before".to_string()),
        };
        service.sessions.save(&team).unwrap();
        let mut running = task();
        running.map_session_id = team.meta.id.clone();
        running.status = crate::team::TeamTaskStatus::Running;
        service
            .sessions
            .upsert_team_task(&parent.meta.id, running)
            .unwrap();
        let journal = service.dirs.journal_dir();
        crate::provider_tool_loop::leave_pending_native_receipt_for_tests(
            &journal,
            &crate::provider_runtime::RunIdentity {
                session_id: team.meta.id.clone(),
                run_id: crate::provider_runtime::RunId::new(7),
                request_id: "map-1".to_string(),
                session_kind: crate::session::SessionKind::Map,
                cancellation_generation: 0,
            },
            crate::provider::ProviderId::Codex,
            Some("thread-before"),
        )
        .unwrap();
        assert_eq!(
            crate::provider_tool_loop::unresolved_native_runs(&journal, &team.meta.id)
                .unwrap()
                .len(),
            1
        );

        assert_eq!(service.recover_team_tasks().unwrap(), 1);

        let tasks = service.sessions.load(&parent.meta.id).unwrap().team_tasks;
        assert_eq!(tasks[0].status, crate::team::TeamTaskStatus::Interrupted);
        assert!(
            crate::provider_tool_loop::unresolved_native_runs(&journal, &team.meta.id)
                .unwrap()
                .is_empty(),
            "the dead run's receipt no longer refuses the next turn"
        );
        let reset = service.sessions.load(&team.meta.id).unwrap();
        assert_eq!(
            reset.provider_binding.conversation,
            crate::provider::ProviderConversationState::empty(team.provider_binding.provider)
        );
        assert_eq!(
            reset.meta.team_parent.as_deref(),
            Some(parent.meta.id.as_str())
        );
        // A second startup finds nothing to interrupt and resets nothing.
        assert_eq!(service.recover_team_tasks().unwrap(), 0);
    }

    /// A revision the restart interrupted produced nothing: the task it
    /// revised gets back the candidate its team session still shows.
    #[test]
    fn startup_hands_an_interrupted_revision_candidate_back_to_its_task() {
        use crate::team::TeamTaskStatus;
        let fixture = team_fixture();
        let service = &fixture.service;
        let parent = eps_parent("rpg");
        service.sessions.save(&parent).unwrap();
        let (team, state, _) = service
            .fresh_team_session_in(&parent, &fixture.context, "rpg")
            .unwrap();
        let summary = |revision| crate::team::TeamCandidateSummary {
            revision,
            revision_key: "r:x".to_string(),
            map_sha256: "c".repeat(64),
            summary: "지형 4칸".to_string(),
            terrain_cells: 4,
            units: 0,
            buildings: 0,
            doodads: 0,
            sprites: 0,
            locations: 0,
        };
        for (id, status, updated_at) in [
            ("task-a", TeamTaskStatus::Superseded, 1),
            ("task-b", TeamTaskStatus::Running, 2),
        ] {
            service
                .sessions
                .upsert_team_task(
                    &parent.meta.id,
                    crate::team::TeamTask {
                        id: id.to_string(),
                        map_session_id: team.meta.id.clone(),
                        status,
                        candidate: (id == "task-a").then(|| summary(state.current_revision)),
                        updated_at,
                        ..task()
                    },
                )
                .unwrap();
        }

        assert_eq!(service.recover_team_tasks().unwrap(), 2);
        let statuses = service
            .sessions
            .load(&parent.meta.id)
            .unwrap()
            .team_tasks
            .into_iter()
            .map(|task| (task.id, task.status))
            .collect::<Vec<_>>();
        assert_eq!(
            statuses,
            vec![
                ("task-a".to_string(), TeamTaskStatus::CandidateReady),
                ("task-b".to_string(), TeamTaskStatus::Interrupted),
            ]
        );
    }

    /// A `candidate_ready` task left over from the previous run used to hang
    /// startup: the candidate check re-read the Map session through the
    /// session store while the recovery pass already held its lock, so the
    /// main thread never returned from `setup` ("응답 없음"). Recovery must
    /// settle within a bound and keep exactly the candidate that still exists.
    #[test]
    fn startup_with_a_ready_team_candidate_settles_instead_of_deadlocking() {
        let fixture = team_fixture();
        let service = &fixture.service;
        let parent = eps_parent("rpg");
        service.sessions.save(&parent).unwrap();
        let (team, state, _) = service
            .fresh_team_session_in(&parent, &fixture.context, "rpg")
            .unwrap();
        let ready = |id: &str, revision: u32| {
            let mut task = task();
            task.id = id.to_string();
            task.map_session_id = team.meta.id.clone();
            task.status = crate::team::TeamTaskStatus::CandidateReady;
            task.candidate = Some(crate::team::TeamCandidateSummary {
                revision,
                revision_key: format!("r{revision}"),
                map_sha256: "b".repeat(64),
                summary: "candidate".to_string(),
                terrain_cells: 1,
                units: 0,
                buildings: 0,
                doodads: 0,
                sprites: 0,
                locations: 0,
            });
            task
        };
        service
            .sessions
            .upsert_team_task(&parent.meta.id, ready("live", state.current_revision))
            .unwrap();
        service
            .sessions
            .upsert_team_task(&parent.meta.id, ready("gone", state.current_revision + 1))
            .unwrap();

        let (done, recovered) = std::sync::mpsc::channel();
        let startup = service.clone();
        std::thread::spawn(move || done.send(startup.recover_team_tasks()).ok());
        let changed = recovered
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("team task recovery deadlocked on the session store lock")
            .unwrap();
        assert_eq!(changed, 1);

        let tasks = service.sessions.load(&parent.meta.id).unwrap().team_tasks;
        let status = |id: &str| {
            tasks
                .iter()
                .find(|task| task.id == id)
                .unwrap()
                .status
                .clone()
        };
        assert_eq!(status("live"), crate::team::TeamTaskStatus::CandidateReady);
        assert_eq!(status("gone"), crate::team::TeamTaskStatus::Discarded);
    }

    /// `map_task_apply` / `map_task_discard` act only on the exact candidate
    /// the task announced; discard settles the task and leaves the map alone.
    #[test]
    fn team_task_action_validates_the_live_candidate_and_discards_it() {
        let fixture = team_fixture();
        let service = &fixture.service;
        let parent = eps_parent("rpg");
        service.sessions.save(&parent).unwrap();
        let (team, state, _) = service
            .fresh_team_session_in(&parent, &fixture.context, "rpg")
            .unwrap();
        let mut task = task();
        task.map_session_id = team.meta.id.clone();
        task.status = crate::team::TeamTaskStatus::CandidateReady;
        task.candidate = Some(crate::team::TeamCandidateSummary {
            revision: 1,
            revision_key: "r1:stale".to_string(),
            map_sha256: "f".repeat(64),
            summary: "지형 1칸".to_string(),
            terrain_cells: 1,
            units: 0,
            buildings: 0,
            doodads: 0,
            sprites: 0,
            locations: 0,
        });
        service
            .sessions
            .upsert_team_task(&parent.meta.id, task.clone())
            .unwrap();
        let action = crate::tool_exec::TeamTaskAction {
            session_id: parent.meta.id.clone(),
            request_id: "req".to_string(),
            task_id: task.id.clone(),
            kind: crate::tool_exec::TeamTaskActionKind::Discard,
        };

        // The team session is at r0, not the announced r1: refuse.
        let changed = service.team_task_action(&action).unwrap_err();
        assert!(
            changed.contains("changed since it was announced"),
            "{changed}"
        );

        // Announce the live candidate exactly and discard it.
        task.candidate = Some(crate::team::TeamCandidateSummary {
            revision: state.current_revision,
            revision_key: state.revision_key.clone(),
            map_sha256: state.current_hash.clone(),
            summary: "변경 없음".to_string(),
            terrain_cells: 0,
            units: 0,
            buildings: 0,
            doodads: 0,
            sprites: 0,
            locations: 0,
        });
        service
            .sessions
            .upsert_team_task(&parent.meta.id, task.clone())
            .unwrap();
        let before = std::fs::read(&fixture.source).unwrap();
        let (updated, settled) = service.team_task_action(&action).unwrap();
        assert_eq!(updated.status, crate::team::TeamTaskStatus::Discarded);
        assert_eq!(settled.len(), 1);
        assert_eq!(settled[0].0, parent.meta.id);
        assert_eq!(std::fs::read(&fixture.source).unwrap(), before);
        let stored = service.sessions.load(&parent.meta.id).unwrap();
        assert_eq!(
            stored.team_tasks[0].status,
            crate::team::TeamTaskStatus::Discarded
        );

        // A settled task can no longer be applied.
        let apply = crate::tool_exec::TeamTaskAction {
            kind: crate::tool_exec::TeamTaskActionKind::Apply,
            ..action
        };
        let settled = service.team_task_action(&apply).unwrap_err();
        assert!(settled.contains("not candidate_ready"), "{settled}");
        assert_eq!(fixture.map_project_id, team.meta.project);
    }

    #[test]
    fn team_candidate_summary_reports_only_a_new_revision() {
        let diff = MapDiff {
            terrain_cells: 40,
            terrain_bounds: None,
            units: LayerDiffCount {
                added: 2,
                removed: 0,
                moved: 1,
                changed: 0,
            },
            buildings: LayerDiffCount::default(),
            doodads: LayerDiffCount::default(),
            sprites: LayerDiffCount::default(),
            locations: LayerDiffCount {
                added: 1,
                removed: 0,
                moved: 0,
                changed: 0,
            },
            outside_target: 0,
            protected: 0,
            unsupported_section_changes: Vec::new(),
            properties: 0,
        };
        let summary = MapAgentService::team_candidate_summary(0, &state(1, Some(diff))).unwrap();
        assert_eq!(summary.revision, 1);
        assert_eq!(summary.summary, "지형 40칸, 유닛 3건, 로케이션 1건");
        assert_eq!(summary.units, 3);
        assert_eq!(summary.map_sha256, "b".repeat(64));
        assert!(MapAgentService::team_candidate_summary(1, &state(1, None)).is_none());
        assert_eq!(
            MapAgentService::team_candidate_summary(0, &state(1, Some(MapDiff::default())))
                .unwrap()
                .summary,
            "변경 없음"
        );
    }
}
