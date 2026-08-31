use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine as _;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{Emitter, Manager};

use crate::attachment::AttachmentStore;
use crate::bridge_io::HEARTBEAT_STALE_AFTER;
use crate::config::DataDirs;
use crate::map_candidate::{CandidateStateView, CandidateStore};
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

const MAP_WINDOW_LABEL: &str = "map-agent";
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
        crate::ipc::bridge_from_config(&self.dirs)
            .ok()
            .and_then(|bridge| bridge.read_status_snapshot(HEARTBEAT_STALE_AFTER).ok())
            .is_some_and(|status| status.compiling)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapBootstrapResponse {
    pub context: crate::map_context::MapContextSnapshot,
    pub candidate: CandidateStateView,
    pub session: crate::session::SessionRecord,
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
            },
            provider_binding,
            pending_request_ids: Vec::new(),
            context_usage: None,
            panel_log: serde_json::Value::Null,
            context_state: Default::default(),
            task_state: Default::default(),
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
            "filter": if command.kind == "tiles" {
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
            let cache_key = format!("{}|{}", command.session_id, source.revision_key);
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
        let bridge = crate::ipc::bridge_from_config(&self.dirs)?;
        let status = bridge
            .read_status_snapshot(HEARTBEAT_STALE_AFTER)
            .map_err(|error| format!("editor status is unavailable; Apply is blocked: {error}"))?;
        if status.compiling {
            return Err("the editor is compiling; Apply is blocked".to_string());
        }
        Ok(())
    }

    fn apply(&self, session_id: &str) -> Result<CandidateStateView, String> {
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

    fn undo(&self, session_id: &str) -> Result<CandidateStateView, String> {
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
    let candidate = match resolution.candidate_action {
        CandidateSessionAction::Create => service
            .candidates
            .create_session(&resolution.session.meta.id, &context)?,
        CandidateSessionAction::Open => service
            .candidates
            .open_session(&resolution.session.meta.id, &context)?,
    };
    engines
        .open_map_session(&resolution.session.meta.id)
        .await?;
    Ok(MapBootstrapResponse {
        context,
        candidate,
        session: resolution.session,
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

#[tauri::command]
pub async fn map_agent_open(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(MAP_WINDOW_LABEL) {
        window.show().map_err(|error| error.to_string())?;
        window.set_focus().map_err(|error| error.to_string())?;
        return Ok(());
    }
    tauri::WebviewWindowBuilder::new(
        &app,
        MAP_WINDOW_LABEL,
        tauri::WebviewUrl::App("map-agent.html".into()),
    )
    .title("Map Agent Workbench")
    .inner_size(1600.0, 960.0)
    .min_inner_size(1100.0, 700.0)
    .resizable(true)
    .drag_and_drop(false)
    .build()
    .map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
pub(crate) async fn map_agent_bootstrap(
    service: tauri::State<'_, MapAgentService>,
    engines: tauri::State<'_, crate::engine::SessionEngineManager>,
) -> Result<MapBootstrapResponse, String> {
    let context = service.candidates.context().current()?;
    let resolution = service.map_session(&context)?;
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
    service: tauri::State<'_, MapAgentService>,
    session_id: String,
    revision: u32,
) -> Result<CandidateStateView, String> {
    let session = service.session_record(&session_id)?;
    service
        .candidates
        .revert(&session.meta.project, &session_id, revision)
}

#[tauri::command]
pub fn map_agent_candidate_discard(
    service: tauri::State<'_, MapAgentService>,
    session_id: String,
) -> Result<(), String> {
    let session = service.session_record(&session_id)?;
    service
        .candidates
        .discard(&session.meta.project, &session_id)
}

#[tauri::command]
pub fn map_agent_candidate_apply(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, MapAgentService>,
    session_id: String,
) -> Result<CandidateStateView, String> {
    require_map_window(&window)?;
    let state = service.apply(&session_id)?;
    let _ = window.emit("map_apply_result", &state);
    Ok(state)
}

#[tauri::command]
pub fn map_agent_apply_undo(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, MapAgentService>,
    session_id: String,
) -> Result<CandidateStateView, String> {
    require_map_window(&window)?;
    let state = service.undo(&session_id)?;
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
