use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::chk::Location;
use crate::map_candidate::CandidateStore;
use crate::map_context::{MapContextService, MapContextSnapshot};
use crate::map_model::{MapLayer, SelectionRole, TileRect};
use crate::map_stamp::PersistentSelection;

pub const MAX_MENTIONS_PER_TURN: usize = 16;
const DEFAULT_SEARCH_LIMIT: usize = 20;
const MAX_SEARCH_LIMIT: usize = 50;
const MAX_QUERY_BYTES: usize = 256;
const MAX_INSTANCE_ID_BYTES: usize = 128;
const MAX_LABEL_BYTES: usize = 256;
const MAX_DETAIL_BYTES: usize = 512;
const SEARCH_CONTEXT_REVALIDATE_AFTER: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum MentionKind {
    #[serde(rename = "map.region")]
    MapRegion,
    #[serde(rename = "map.location")]
    MapLocation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum MentionSnapshot {
    #[serde(rename = "map.region")]
    MapRegion(MapRegionMentionV1),
    #[serde(rename = "map.location")]
    MapLocation(MapLocationMentionV1),
}

impl MentionSnapshot {
    pub fn kind(&self) -> MentionKind {
        match self {
            Self::MapRegion(_) => MentionKind::MapRegion,
            Self::MapLocation(_) => MentionKind::MapLocation,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapRegionMentionV1 {
    pub version: u8,
    pub project_id: String,
    pub source_file_sha256: String,
    pub map_width: u16,
    pub map_height: u16,
    pub selection_id: String,
    pub selection_snapshot_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MapLocationMentionV1 {
    pub version: u8,
    pub project_id: String,
    pub source_file_sha256: String,
    pub location_id: u16,
    pub location_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MentionInstance {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub mention: MentionSnapshot,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stale: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MentionSearchRequest {
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<MentionKind>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MentionSuggestion {
    pub resource_key: String,
    pub kind: MentionKind,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub mention: MentionSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MentionSearchResponse {
    pub schema: String,
    pub results: Vec<MentionSuggestion>,
    pub truncated: bool,
}

struct CachedMentionContext {
    snapshot: Arc<MapContextSnapshot>,
    validated_at: Instant,
}

#[derive(Clone)]
pub struct MentionService {
    candidates: CandidateStore,
    context: MapContextService,
    search_cache: Arc<parking_lot::RwLock<Option<CachedMentionContext>>>,
    refresh_lock: Arc<parking_lot::Mutex<()>>,
    refresh_scheduled: Arc<std::sync::atomic::AtomicBool>,
    #[cfg(test)]
    context_override: Arc<parking_lot::Mutex<Option<MapContextSnapshot>>>,
    #[cfg(test)]
    context_loads: Arc<std::sync::atomic::AtomicUsize>,
}

impl MentionService {
    pub fn new(candidates: CandidateStore, context: MapContextService) -> Self {
        Self {
            candidates,
            context,
            search_cache: Arc::new(parking_lot::RwLock::new(None)),
            refresh_lock: Arc::new(parking_lot::Mutex::new(())),
            refresh_scheduled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            #[cfg(test)]
            context_override: Arc::new(parking_lot::Mutex::new(None)),
            #[cfg(test)]
            context_loads: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    pub fn search(&self, request: MentionSearchRequest) -> Result<MentionSearchResponse, String> {
        validate_search_request(&request)?;
        let context = match self.cached_search_context()? {
            Some(context) => {
                self.refresh_search_context_in_background();
                context
            }
            None => {
                self.refresh_search_context()?;
                self.cached_search_context()?
                    .ok_or_else(|| "mention search context cache is unavailable".to_string())?
            }
        };
        let selections = if kind_enabled(&request, MentionKind::MapRegion) {
            self.candidates
                .persistent_selections(&context.revision.project_id)?
        } else {
            Vec::new()
        };
        search_for_context(&request, context.as_ref(), &selections)
    }

    pub fn warmup(&self) -> Result<(), String> {
        self.refresh_search_context()
    }

    pub fn resolve_all(&self, instances: &[MentionInstance]) -> Result<Option<String>, String> {
        validate_instance_envelope(instances)?;
        if instances.is_empty() {
            return Ok(None);
        }
        let context = self.current_context().map_err(|_| {
            "멘션을 확인할 현재 저장 소스 맵을 읽지 못했습니다. 프로젝트와 OpenMapName을 확인해 주세요."
                .to_string()
        })?;
        let selections = if instances
            .iter()
            .any(|instance| instance.mention.kind() == MentionKind::MapRegion)
        {
            self.candidates
                .persistent_selections(&context.revision.project_id)
                .map_err(|_| {
                    "저장된 영역 목록을 읽지 못했습니다. Map Agent에서 영역을 다시 확인해 주세요."
                        .to_string()
                })?
        } else {
            Vec::new()
        };
        resolve_for_context(&context, &selections, instances)
    }

    fn cached_search_context(&self) -> Result<Option<Arc<MapContextSnapshot>>, String> {
        let cached = self
            .search_cache
            .read()
            .as_ref()
            .map(|cached| Arc::clone(&cached.snapshot));
        let Some(context) = cached else {
            return Ok(None);
        };
        #[cfg(test)]
        return Ok(Some(context));
        #[cfg(not(test))]
        {
            if self.context.snapshot_binding_is_current(&context)? {
                return Ok(Some(context));
            }
            *self.search_cache.write() = None;
            Ok(None)
        }
    }

    fn cache_needs_revalidation(&self) -> bool {
        let cache = self.search_cache.read();
        match cache.as_ref() {
            Some(cached) => cached.validated_at.elapsed() >= SEARCH_CONTEXT_REVALIDATE_AFTER,
            None => true,
        }
    }

    fn refresh_search_context_in_background(&self) {
        if !self.cache_needs_revalidation()
            || self
                .refresh_scheduled
                .compare_exchange(
                    false,
                    true,
                    std::sync::atomic::Ordering::AcqRel,
                    std::sync::atomic::Ordering::Acquire,
                )
                .is_err()
        {
            return;
        }
        let service = self.clone();
        let _refresh = tauri::async_runtime::spawn_blocking(move || {
            let result = service.refresh_search_context();
            service
                .refresh_scheduled
                .store(false, std::sync::atomic::Ordering::Release);
            if let Err(error) = result {
                eprintln!("eud-agent: mention context refresh skipped: {error}");
            }
        });
    }

    fn refresh_search_context(&self) -> Result<(), String> {
        let _refresh = self.refresh_lock.lock();
        if !self.cache_needs_revalidation() {
            return Ok(());
        }
        let snapshot = self.load_search_context()?;
        *self.search_cache.write() = Some(CachedMentionContext {
            snapshot,
            validated_at: Instant::now(),
        });
        Ok(())
    }

    fn load_search_context(&self) -> Result<Arc<MapContextSnapshot>, String> {
        #[cfg(test)]
        {
            self.current_context().map(Arc::new)
        }
        #[cfg(not(test))]
        {
            let probe = self.context.probe_current()?;
            if let Some(cached) = self.search_cache.read().as_ref() {
                let revision = &cached.snapshot.revision;
                if revision.project_id == probe.project_id
                    && revision.source_path == probe.source_path
                    && revision.mtime_ns == probe.mtime_ns
                    && cached.snapshot.source_file_size == probe.file_size
                {
                    return Ok(Arc::clone(&cached.snapshot));
                }
            }
            self.context.snapshot_for_probe(probe).map(Arc::new)
        }
    }

    fn current_context(&self) -> Result<MapContextSnapshot, String> {
        #[cfg(test)]
        self.context_loads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        #[cfg(test)]
        if let Some(context) = self.context_override.lock().clone() {
            return Ok(context);
        }
        self.context.current()
    }

    #[cfg(test)]
    pub(crate) fn set_context_for_tests(&self, context: MapContextSnapshot) {
        *self.context_override.lock() = Some(context);
        *self.search_cache.write() = None;
    }

    #[cfg(test)]
    pub(crate) fn context_loads_for_tests(&self) -> usize {
        self.context_loads
            .load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[tauri::command]
pub async fn mention_search(
    state: tauri::State<'_, MentionService>,
    request: MentionSearchRequest,
) -> Result<MentionSearchResponse, String> {
    let service = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || service.search(request))
        .await
        .map_err(|error| format!("mention search task failed: {error}"))?
}

pub(crate) fn search_for_context(
    request: &MentionSearchRequest,
    context: &MapContextSnapshot,
    selections: &[PersistentSelection],
) -> Result<MentionSearchResponse, String> {
    let limit = validate_search_request(request)?;
    let query = request.query.trim().to_lowercase();
    let mut matches = Vec::new();

    if kind_enabled(request, MentionKind::MapRegion) {
        let mut regions = selections.iter().collect::<Vec<_>>();
        regions.sort_by(|left, right| {
            left.label
                .cmp(&right.label)
                .then_with(|| left.id.cmp(&right.id))
        });
        for selection in regions {
            let bound = validated_selection(selection, context)?;
            if !matches_query(&query, [selection.label.as_str(), selection.id.as_str()]) {
                continue;
            }
            let rectangular = is_exact_rectangle(selection);
            let label = display_region_label(selection);
            matches.push(MentionSuggestion {
                resource_key: format!("map.region:{}", selection.id),
                kind: MentionKind::MapRegion,
                label,
                detail: Some(format!(
                    "저장된 영역 · {} · ({}, {})–({}, {}) · {}",
                    selection_role_label(selection.role),
                    bound.bounds.left,
                    bound.bounds.top,
                    bound.bounds.right,
                    bound.bounds.bottom,
                    if rectangular {
                        "사각형"
                    } else {
                        "자유형"
                    }
                )),
                mention: MentionSnapshot::MapRegion(MapRegionMentionV1 {
                    version: 1,
                    project_id: context.revision.project_id.clone(),
                    source_file_sha256: context.revision.file_sha256.clone(),
                    map_width: context.revision.width,
                    map_height: context.revision.height,
                    selection_id: selection.id.clone(),
                    selection_snapshot_hash: selection.snapshot_hash(),
                }),
            });
            if matches.len() > limit {
                break;
            }
        }
    }

    if matches.len() <= limit && kind_enabled(request, MentionKind::MapLocation) {
        let mut locations = context.digest.locations.iter().collect::<Vec<_>>();
        locations.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.id.cmp(&right.id))
        });
        for location in locations {
            let id_alias = format!("#{}", location.id);
            let numeric_alias = location.id.to_string();
            if !matches_query(
                &query,
                [
                    location.name.as_str(),
                    id_alias.as_str(),
                    numeric_alias.as_str(),
                ],
            ) {
                continue;
            }
            let location_id = u16::try_from(location.id)
                .map_err(|_| "saved source map contains an invalid location id".to_string())?;
            matches.push(MentionSuggestion {
                resource_key: format!("map.location:{}", location.id),
                kind: MentionKind::MapLocation,
                label: display_location_label(location),
                detail: Some(format!(
                    "저장된 소스 맵 · #{} · 타일 ({}, {})–({}, {}){}",
                    location.id,
                    location.tile_rect[0],
                    location.tile_rect[1],
                    location.tile_rect[2],
                    location.tile_rect[3],
                    if location.anywhere == Some(true) {
                        " · Anywhere"
                    } else {
                        ""
                    }
                )),
                mention: MentionSnapshot::MapLocation(MapLocationMentionV1 {
                    version: 1,
                    project_id: context.revision.project_id.clone(),
                    source_file_sha256: context.revision.file_sha256.clone(),
                    location_id,
                    location_fingerprint: location_fingerprint(location),
                }),
            });
            if matches.len() > limit {
                break;
            }
        }
    }

    let truncated = matches.len() > limit;
    matches.truncate(limit);
    Ok(MentionSearchResponse {
        schema: "eud-mention-search/1".to_string(),
        results: matches,
        truncated,
    })
}

pub(crate) fn resolve_for_context(
    context: &MapContextSnapshot,
    selections: &[PersistentSelection],
    instances: &[MentionInstance],
) -> Result<Option<String>, String> {
    validate_instance_envelope(instances)?;
    if instances.is_empty() {
        return Ok(None);
    }

    let mut items = Vec::with_capacity(instances.len());
    for instance in instances {
        match &instance.mention {
            MentionSnapshot::MapRegion(snapshot) => {
                validate_version(instance, snapshot.version)?;
                validate_map_binding(
                    instance,
                    &snapshot.project_id,
                    &snapshot.source_file_sha256,
                    Some((snapshot.map_width, snapshot.map_height)),
                    context,
                )?;
                let selection = selections
                    .iter()
                    .find(|selection| selection.id == snapshot.selection_id)
                    .ok_or_else(|| mention_error(instance, "저장 영역이 삭제되었습니다"))?;
                let bound = validated_selection(selection, context)
                    .map_err(|_| mention_error(instance, "저장 영역 데이터가 유효하지 않습니다"))?;
                if selection.snapshot_hash() != snapshot.selection_snapshot_hash {
                    return Err(mention_error(instance, "저장 영역이 변경되었습니다"));
                }
                items.push(ResolvedMention::Region(ResolvedRegion {
                    id: instance.id.clone(),
                    kind: "map.region",
                    selection_id: selection.id.clone(),
                    label: selection.label.clone(),
                    role: selection.role,
                    layers: selection.layers.iter().copied().collect(),
                    bounds: bound.bounds,
                    selected_cells: bound.selected_cells,
                    rectangular: is_exact_rectangle(selection),
                }));
            }
            MentionSnapshot::MapLocation(snapshot) => {
                validate_version(instance, snapshot.version)?;
                validate_map_binding(
                    instance,
                    &snapshot.project_id,
                    &snapshot.source_file_sha256,
                    None,
                    context,
                )?;
                let location = context
                    .digest
                    .locations
                    .iter()
                    .find(|location| location.id == usize::from(snapshot.location_id))
                    .ok_or_else(|| mention_error(instance, "저장된 로케이션이 삭제되었습니다"))?;
                if location_fingerprint(location) != snapshot.location_fingerprint {
                    return Err(mention_error(instance, "저장된 로케이션이 변경되었습니다"));
                }
                items.push(ResolvedMention::Location(ResolvedLocation {
                    id: instance.id.clone(),
                    kind: "map.location",
                    location_id: snapshot.location_id,
                    name: location.name.clone(),
                    pixel_bounds: BoundsI32::new(
                        location.left,
                        location.top,
                        location.right,
                        location.bottom,
                    ),
                    tile_bounds: BoundsI32::new(
                        location.tile_rect[0],
                        location.tile_rect[1],
                        location.tile_rect[2],
                        location.tile_rect[3],
                    ),
                    elevation_flags: location.elevation_flags,
                    inverted: location.inverted.clone(),
                    anywhere: location.anywhere == Some(true),
                }));
            }
        }
    }

    let json = serde_json::to_string(&ResolvedEnvelope {
        schema: "eud-resolved-mentions/1",
        items,
    })
    .map_err(|error| format!("resolved mentions could not be serialized: {error}"))?;
    Ok(Some(format!("[resolved mentions]\n{json}")))
}

fn validate_search_request(request: &MentionSearchRequest) -> Result<usize, String> {
    if request.query.len() > MAX_QUERY_BYTES {
        return Err(format!(
            "mention query must be at most {MAX_QUERY_BYTES} UTF-8 bytes"
        ));
    }
    if let Some(kinds) = &request.kinds {
        let unique = kinds.iter().copied().collect::<BTreeSet<_>>();
        if unique.len() != kinds.len() {
            return Err("mention search kinds must be unique".to_string());
        }
    }
    let limit = request.limit.unwrap_or(DEFAULT_SEARCH_LIMIT);
    if limit == 0 || limit > MAX_SEARCH_LIMIT {
        return Err(format!(
            "mention search limit must be between 1 and {MAX_SEARCH_LIMIT}"
        ));
    }
    Ok(limit)
}

fn validate_instance_envelope(instances: &[MentionInstance]) -> Result<(), String> {
    if instances.len() > MAX_MENTIONS_PER_TURN {
        return Err(format!(
            "한 메시지에는 멘션을 최대 {MAX_MENTIONS_PER_TURN}개까지 사용할 수 있습니다."
        ));
    }
    let mut ids = HashSet::with_capacity(instances.len());
    for instance in instances {
        if instance.id.is_empty() || instance.id.len() > MAX_INSTANCE_ID_BYTES {
            return Err("멘션 instance id가 비어 있거나 너무 깁니다.".to_string());
        }
        if !ids.insert(instance.id.as_str()) {
            return Err(format!(
                "멘션 instance id '{}'가 중복되었습니다. 전체 요청을 다시 보내 주세요.",
                instance.id
            ));
        }
        if instance.label.is_empty() || instance.label.len() > MAX_LABEL_BYTES {
            return Err(format!(
                "멘션 '{}'의 표시 라벨이 비어 있거나 너무 깁니다.",
                instance.id
            ));
        }
        if instance
            .detail
            .as_ref()
            .is_some_and(|detail| detail.len() > MAX_DETAIL_BYTES)
        {
            return Err(format!("멘션 '{}'의 표시 설명이 너무 깁니다.", instance.id));
        }
        if instance.stale {
            return Err(mention_error(
                instance,
                "현재 프로젝트에서 더 이상 유효하지 않습니다",
            ));
        }
        match &instance.mention {
            MentionSnapshot::MapRegion(snapshot) => validate_version(instance, snapshot.version)?,
            MentionSnapshot::MapLocation(snapshot) => validate_version(instance, snapshot.version)?,
        }
    }
    Ok(())
}

fn validate_version(instance: &MentionInstance, version: u8) -> Result<(), String> {
    if version != 1 {
        return Err(mention_error(
            instance,
            &format!("지원하지 않는 멘션 버전 {version}입니다"),
        ));
    }
    Ok(())
}

fn validate_map_binding(
    instance: &MentionInstance,
    project_id: &str,
    source_file_sha256: &str,
    dimensions: Option<(u16, u16)>,
    context: &MapContextSnapshot,
) -> Result<(), String> {
    if project_id != context.revision.project_id {
        return Err(mention_error(instance, "현재 EUD 프로젝트와 다릅니다"));
    }
    if source_file_sha256 != context.revision.file_sha256 {
        return Err(mention_error(instance, "저장된 소스 맵이 변경되었습니다"));
    }
    if dimensions.is_some_and(|(width, height)| {
        width != context.revision.width || height != context.revision.height
    }) {
        return Err(mention_error(instance, "저장된 맵 크기가 변경되었습니다"));
    }
    Ok(())
}

fn validated_selection(
    selection: &PersistentSelection,
    context: &MapContextSnapshot,
) -> Result<crate::map_model::SelectionMask, String> {
    let bound = selection.bind(
        "main-resource-mention",
        context.revision.width,
        context.revision.height,
    )?;
    if bound.id != selection.id
        || bound.label != selection.label
        || bound.role != selection.role
        || bound.layers != selection.layers
        || bound.bounds != selection.bounds
        || bound.selected_cells != selection.selected_cells
        || bound.rows != selection.rows
    {
        return Err("persistent selection is not canonical".to_string());
    }
    Ok(bound)
}

pub(crate) fn is_exact_rectangle(selection: &PersistentSelection) -> bool {
    let bounds = selection.bounds;
    let expected_rows = usize::from(bounds.bottom.saturating_sub(bounds.top));
    if expected_rows == 0 || selection.rows.len() != expected_rows {
        return false;
    }
    selection.rows.iter().enumerate().all(|(offset, row)| {
        row.y == bounds.top + u16::try_from(offset).unwrap_or(u16::MAX)
            && row.spans.as_slice() == [(bounds.left, bounds.right)]
    })
}

pub(crate) fn location_fingerprint(location: &Location) -> String {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Fingerprint<'a> {
        id: usize,
        name: &'a str,
        pixel_bounds: BoundsI32,
        tile_bounds: BoundsI32,
        elevation_flags: u16,
        inverted: Option<&'a str>,
        anywhere: bool,
    }

    let record = Fingerprint {
        id: location.id,
        name: &location.name,
        pixel_bounds: BoundsI32::new(location.left, location.top, location.right, location.bottom),
        tile_bounds: BoundsI32::new(
            location.tile_rect[0],
            location.tile_rect[1],
            location.tile_rect[2],
            location.tile_rect[3],
        ),
        elevation_flags: location.elevation_flags,
        inverted: location.inverted.as_deref(),
        anywhere: location.anywhere == Some(true),
    };
    let bytes = serde_json::to_vec(&record).expect("location fingerprint record is serializable");
    crate::map_model::hex_sha256(&bytes)
}

fn kind_enabled(request: &MentionSearchRequest, kind: MentionKind) -> bool {
    request
        .kinds
        .as_ref()
        .map_or(true, |kinds| kinds.contains(&kind))
}

fn matches_query<'a>(query: &str, fields: impl IntoIterator<Item = &'a str>) -> bool {
    query.is_empty()
        || fields
            .into_iter()
            .any(|field| field.to_lowercase().contains(query))
}

fn display_region_label(selection: &PersistentSelection) -> String {
    if selection.label.trim().is_empty() {
        selection.id.clone()
    } else {
        selection.label.clone()
    }
}

fn display_location_label(location: &Location) -> String {
    if location.name.trim().is_empty() {
        format!("#{}", location.id)
    } else {
        location.name.clone()
    }
}

fn selection_role_label(role: SelectionRole) -> &'static str {
    match role {
        SelectionRole::Target => "target",
        SelectionRole::Reference => "reference",
        SelectionRole::Protect => "protect",
        SelectionRole::Anchor => "anchor",
    }
}

fn mention_error(instance: &MentionInstance, reason: &str) -> String {
    format!(
        "멘션 '@{}' ({})을 확인할 수 없습니다: {reason}. 다시 검색해 주세요.",
        instance.label, instance.id
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct BoundsI32 {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl BoundsI32 {
    fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }
}

#[derive(Serialize)]
struct ResolvedEnvelope {
    schema: &'static str,
    items: Vec<ResolvedMention>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum ResolvedMention {
    Region(ResolvedRegion),
    Location(ResolvedLocation),
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ResolvedRegion {
    id: String,
    kind: &'static str,
    selection_id: String,
    label: String,
    role: SelectionRole,
    layers: Vec<MapLayer>,
    bounds: TileRect,
    selected_cells: u32,
    rectangular: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ResolvedLocation {
    id: String,
    kind: &'static str,
    location_id: u16,
    name: String,
    pixel_bounds: BoundsI32,
    tile_bounds: BoundsI32,
    elevation_flags: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    inverted: Option<String>,
    anywhere: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chk::{Digest, Location, MapHeader};
    use crate::map_context::MapContextSnapshot;
    use crate::map_model::{MapRevision, RowSpan, Tileset};
    use serde_json::json;
    use std::path::PathBuf;

    fn location(id: usize, name: &str) -> Location {
        Location {
            id,
            name: name.to_string(),
            left: 32,
            top: 64,
            right: 160,
            bottom: 192,
            tile_rect: [1, 2, 5, 6],
            elevation_flags: 3,
            inverted: None,
            anywhere: None,
        }
    }

    fn context(locations: Vec<Location>) -> MapContextSnapshot {
        MapContextSnapshot {
            revision: MapRevision {
                project_id: "project-a".to_string(),
                source_path: PathBuf::from("C:/private/source.scx"),
                file_sha256: "a".repeat(64),
                chk_sha256: "b".repeat(64),
                mtime_ns: 1,
                tileset: Tileset::Jungle,
                width: 64,
                height: 64,
            },
            saved_source_notice: "saved".to_string(),
            source_file_size: 100,
            starcraft_path: PathBuf::from("C:/private/StarCraft"),
            digest: Digest {
                map: MapHeader {
                    width: 64,
                    height: 64,
                    tileset: "Jungle".to_string(),
                },
                players: Vec::new(),
                forces: Vec::new(),
                locations,
                units: Vec::new(),
                doodads: Vec::new(),
                sprites: Vec::new(),
                start_locations: Vec::new(),
                tiles: Vec::new(),
                switches: Vec::new(),
                switch_usages: Vec::new(),
            },
        }
    }

    fn rectangle(id: &str, label: &str) -> PersistentSelection {
        PersistentSelection {
            id: id.to_string(),
            label: label.to_string(),
            role: SelectionRole::Target,
            layers: [MapLayer::Units, MapLayer::Locations].into_iter().collect(),
            bounds: TileRect {
                left: 2,
                top: 3,
                right: 5,
                bottom: 5,
            },
            selected_cells: 6,
            rows: vec![
                RowSpan {
                    y: 3,
                    spans: vec![(2, 5)],
                },
                RowSpan {
                    y: 4,
                    spans: vec![(2, 5)],
                },
            ],
        }
    }

    fn region_instance(selection: &PersistentSelection) -> MentionInstance {
        MentionInstance {
            id: "mention-region".to_string(),
            label: selection.label.clone(),
            detail: None,
            mention: MentionSnapshot::MapRegion(MapRegionMentionV1 {
                version: 1,
                project_id: "project-a".to_string(),
                source_file_sha256: "a".repeat(64),
                map_width: 64,
                map_height: 64,
                selection_id: selection.id.clone(),
                selection_snapshot_hash: selection.snapshot_hash(),
            }),
            stale: false,
        }
    }

    fn location_instance(value: &Location) -> MentionInstance {
        MentionInstance {
            id: "mention-location".to_string(),
            label: value.name.clone(),
            detail: Some(format!("#{}", value.id)),
            mention: MentionSnapshot::MapLocation(MapLocationMentionV1 {
                version: 1,
                project_id: "project-a".to_string(),
                source_file_sha256: "a".repeat(64),
                location_id: u16::try_from(value.id).unwrap(),
                location_fingerprint: location_fingerprint(value),
            }),
            stale: false,
        }
    }

    fn request(query: &str, limit: Option<usize>) -> MentionSearchRequest {
        MentionSearchRequest {
            query: query.to_string(),
            kinds: None,
            limit,
        }
    }

    fn service_with_context(value: MapContextSnapshot) -> MentionService {
        let root = std::env::temp_dir().join(format!("mention-cache-{}", uuid::Uuid::new_v4()));
        let dirs = crate::config::DataDirs::from_bases(&root.join("roaming"), &root.join("local"));
        let imports = crate::map_import::MapImportStore::new(dirs.clone());
        let candidates = CandidateStore::new(dirs.clone(), imports);
        let service = MentionService::new(candidates, MapContextService::new(dirs));
        service.set_context_for_tests(value);
        service
    }

    #[test]
    fn repeated_searches_reuse_the_loaded_map_context() {
        let service = service_with_context(context(vec![location(17, "회복 지점")]));

        let first = service.search(request("", None)).unwrap();
        let second = service.search(request("회복", None)).unwrap();

        assert_eq!(first.results.len(), 1);
        assert_eq!(second.results.len(), 1);
        assert_eq!(service.context_loads_for_tests(), 1);
    }

    #[test]
    #[ignore = "requires the live EUD Editor bridge and current OpenMapName"]
    fn live_warmed_search_avoids_editor_roundtrip() {
        let roaming = PathBuf::from(std::env::var_os("APPDATA").unwrap());
        let local = PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap());
        let dirs = crate::config::DataDirs::from_bases(&roaming, &local);
        let imports = crate::map_import::MapImportStore::new(dirs.clone());
        let candidates = CandidateStore::new(dirs.clone(), imports);
        let service = MentionService::new(candidates, MapContextService::new(dirs));
        let search_request = MentionSearchRequest {
            query: String::new(),
            kinds: Some(vec![MentionKind::MapLocation]),
            limit: Some(DEFAULT_SEARCH_LIMIT),
        };

        let warm_started = Instant::now();
        service.warmup().unwrap();
        let warm_elapsed = warm_started.elapsed();
        let first_started = Instant::now();
        service.search(search_request.clone()).unwrap();
        let first_elapsed = first_started.elapsed();
        let second_started = Instant::now();
        service.search(search_request).unwrap();
        let second_elapsed = second_started.elapsed();

        eprintln!(
            "mention warmup={warm_elapsed:?} first={first_elapsed:?} second={second_elapsed:?}"
        );
        assert_eq!(service.context_loads_for_tests(), 1);
        assert!(first_elapsed < Duration::from_millis(500));
        assert!(second_elapsed < Duration::from_millis(500));
    }

    #[test]
    fn strict_serde_rejects_unknown_kind_field_and_missing_authority() {
        let good = json!({
            "kind": "map.region",
            "version": 1,
            "projectId": "project-a",
            "sourceFileSha256": "a",
            "mapWidth": 64,
            "mapHeight": 64,
            "selectionId": "region-a",
            "selectionSnapshotHash": "b"
        });
        let parsed: MentionSnapshot = serde_json::from_value(good.clone()).unwrap();
        assert!(matches!(parsed, MentionSnapshot::MapRegion(_)));

        let mut unknown_kind = good.clone();
        unknown_kind["kind"] = json!("eps.file");
        assert!(serde_json::from_value::<MentionSnapshot>(unknown_kind).is_err());

        let mut unknown_field = good.clone();
        unknown_field["path"] = json!("secret.eps");
        assert!(serde_json::from_value::<MentionSnapshot>(unknown_field).is_err());

        let mut missing = good;
        missing.as_object_mut().unwrap().remove("projectId");
        assert!(serde_json::from_value::<MentionSnapshot>(missing).is_err());
    }

    #[test]
    fn unsupported_version_fails_before_resolution() {
        let selection = rectangle("region-a", "영역 A");
        let mut instance = region_instance(&selection);
        let MentionSnapshot::MapRegion(snapshot) = &mut instance.mention else {
            unreachable!()
        };
        snapshot.version = 2;
        let error =
            resolve_for_context(&context(Vec::new()), &[selection], &[instance]).unwrap_err();
        assert!(error.contains("버전 2"));
    }

    #[test]
    fn search_is_bounded_deterministic_and_provider_ordered() {
        let locations = vec![location(2, "Zulu"), location(1, "Alpha")];
        let selections = vec![
            rectangle("region-z", "나 영역"),
            rectangle("region-a", "가 영역"),
        ];
        let response =
            search_for_context(&request("", Some(3)), &context(locations), &selections).unwrap();
        assert!(response.truncated);
        assert_eq!(response.results.len(), 3);
        assert_eq!(response.results[0].resource_key, "map.region:region-a");
        assert_eq!(response.results[1].resource_key, "map.region:region-z");
        assert_eq!(response.results[2].resource_key, "map.location:1");
    }

    #[test]
    fn korean_region_location_and_location_id_filtering_work() {
        let locations = vec![location(17, "회복 지점"), location(2, "출발")];
        let selections = vec![rectangle("region-a", "영역 A")];
        let region = search_for_context(
            &request("영역 A", None),
            &context(locations.clone()),
            &selections,
        )
        .unwrap();
        assert_eq!(region.results.len(), 1);
        assert_eq!(region.results[0].kind, MentionKind::MapRegion);

        let named = search_for_context(
            &request("회복 지점", None),
            &context(locations.clone()),
            &selections,
        )
        .unwrap();
        assert_eq!(named.results.len(), 1);
        assert_eq!(named.results[0].kind, MentionKind::MapLocation);

        let by_id =
            search_for_context(&request("#17", None), &context(locations), &selections).unwrap();
        assert_eq!(by_id.results[0].resource_key, "map.location:17");
    }

    #[test]
    fn search_validates_query_kinds_and_limit() {
        assert!(
            search_for_context(&request(&"가".repeat(86), None), &context(Vec::new()), &[])
                .is_err()
        );
        let duplicate_kinds = MentionSearchRequest {
            query: String::new(),
            kinds: Some(vec![MentionKind::MapRegion, MentionKind::MapRegion]),
            limit: None,
        };
        assert!(search_for_context(&duplicate_kinds, &context(Vec::new()), &[]).is_err());
        assert!(search_for_context(&request("", Some(0)), &context(Vec::new()), &[]).is_err());
        assert!(search_for_context(&request("", Some(51)), &context(Vec::new()), &[]).is_err());
    }

    #[test]
    fn persistent_selection_hash_is_sensitive_to_every_authoritative_field() {
        let base = rectangle("region-a", "영역 A");
        let mut variants = Vec::new();
        let mut changed = base.clone();
        changed.id.push('x');
        variants.push(changed);
        let mut changed = base.clone();
        changed.label.push('x');
        variants.push(changed);
        let mut changed = base.clone();
        changed.role = SelectionRole::Reference;
        variants.push(changed);
        let mut changed = base.clone();
        changed.layers.insert(MapLayer::Terrain);
        variants.push(changed);
        let mut changed = base.clone();
        changed.bounds.right += 1;
        variants.push(changed);
        let mut changed = base.clone();
        changed.selected_cells += 1;
        variants.push(changed);
        let mut changed = base.clone();
        changed.rows[0].spans[0].1 -= 1;
        variants.push(changed);
        for changed in variants {
            assert_ne!(base.snapshot_hash(), changed.snapshot_hash());
        }
    }

    #[test]
    fn location_fingerprint_is_sensitive_to_complete_record() {
        let base = location(17, "회복 지점");
        let mut variants = Vec::new();
        let mut changed = base.clone();
        changed.id += 1;
        variants.push(changed);
        let mut changed = base.clone();
        changed.name.push('x');
        variants.push(changed);
        let mut changed = base.clone();
        changed.left += 1;
        variants.push(changed);
        let mut changed = base.clone();
        changed.tile_rect[0] += 1;
        variants.push(changed);
        let mut changed = base.clone();
        changed.elevation_flags += 1;
        variants.push(changed);
        let mut changed = base.clone();
        changed.inverted = Some("x".to_string());
        variants.push(changed);
        let mut changed = base.clone();
        changed.anywhere = Some(true);
        variants.push(changed);
        for changed in variants {
            assert_ne!(location_fingerprint(&base), location_fingerprint(&changed));
        }
    }

    #[test]
    fn rectangle_classification_uses_canonical_rows_not_cell_count() {
        let rectangle = rectangle("region-a", "영역 A");
        assert!(is_exact_rectangle(&rectangle));
        let mut free_form = rectangle.clone();
        free_form.rows[1].spans = vec![(2, 3), (4, 5)];
        assert!(!is_exact_rectangle(&free_form));
    }

    #[test]
    fn mixed_mentions_preserve_instance_order_and_compact_output() {
        let selection = rectangle("region-a", "영역 A");
        let loc = location(17, "회복 지점");
        let instances = vec![location_instance(&loc), region_instance(&selection)];
        let section = resolve_for_context(&context(vec![loc]), &[selection], &instances)
            .unwrap()
            .unwrap();
        assert!(
            section.find("mention-location").unwrap() < section.find("mention-region").unwrap()
        );
        assert!(section.contains("\"schema\":\"eud-resolved-mentions/1\""));
        assert!(!section.contains("private"));
        assert!(!section.contains("source.scx"));
        assert!(!section.contains("rows"));
        assert!(!section.contains("SnapshotHash"));
    }

    #[test]
    fn duplicate_instance_id_and_count_cap_refuse_complete_request() {
        let loc = location(17, "회복 지점");
        let first = location_instance(&loc);
        let duplicate = first.clone();
        assert!(resolve_for_context(
            &context(vec![loc.clone()]),
            &[],
            &[first.clone(), duplicate]
        )
        .unwrap_err()
        .contains("중복"));
        let over_cap = (0..=MAX_MENTIONS_PER_TURN)
            .map(|index| {
                let mut instance = first.clone();
                instance.id = format!("mention-{index}");
                instance
            })
            .collect::<Vec<_>>();
        assert!(resolve_for_context(&context(vec![loc]), &[], &over_cap)
            .unwrap_err()
            .contains("최대"));
    }

    #[test]
    fn project_source_dimension_and_hash_mismatches_fail_closed() {
        let selection = rectangle("region-a", "영역 A");
        let base = region_instance(&selection);
        for mutate in ["project", "source", "width", "height", "selection"] {
            let mut instance = base.clone();
            let MentionSnapshot::MapRegion(snapshot) = &mut instance.mention else {
                unreachable!()
            };
            match mutate {
                "project" => snapshot.project_id = "project-b".to_string(),
                "source" => snapshot.source_file_sha256 = "c".repeat(64),
                "width" => snapshot.map_width = 32,
                "height" => snapshot.map_height = 32,
                "selection" => snapshot.selection_snapshot_hash = "d".repeat(64),
                _ => unreachable!(),
            }
            assert!(resolve_for_context(
                &context(Vec::new()),
                std::slice::from_ref(&selection),
                &[instance]
            )
            .is_err());
        }
    }

    #[test]
    fn changed_or_deleted_region_and_location_are_rejected() {
        let selection = rectangle("region-a", "영역 A");
        let region = region_instance(&selection);
        assert!(
            resolve_for_context(&context(Vec::new()), &[], std::slice::from_ref(&region))
                .unwrap_err()
                .contains("삭제")
        );
        let mut changed_selection = selection.clone();
        changed_selection.label = "영역 B".to_string();
        assert!(
            resolve_for_context(&context(Vec::new()), &[changed_selection], &[region])
                .unwrap_err()
                .contains("변경")
        );

        let loc = location(17, "회복 지점");
        let location_mention = location_instance(&loc);
        assert!(resolve_for_context(
            &context(Vec::new()),
            &[],
            std::slice::from_ref(&location_mention),
        )
        .unwrap_err()
        .contains("삭제"));
        let mut changed_location = loc;
        changed_location.name = "다른 이름".to_string();
        assert!(
            resolve_for_context(&context(vec![changed_location]), &[], &[location_mention])
                .unwrap_err()
                .contains("변경")
        );
    }

    #[test]
    fn candidate_only_location_is_not_a_search_input() {
        let source = location(1, "저장 위치");
        let response = search_for_context(
            &request("후보 전용", None),
            &context(vec![source]),
            &[rectangle("region-a", "영역 A")],
        )
        .unwrap();
        assert!(response.results.is_empty());
    }
}
