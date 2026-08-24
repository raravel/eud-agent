//! Session-local active task state.
//!
//! The model may propose bounded semantic deltas, but this module owns every
//! durable type, transition, provenance check, branch operation, and rendering
//! bound. Active state is background context only and never grants editor,
//! workspace, map, journal, or tool authority.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const TASK_STATE_SCHEMA_VERSION: u32 = 1;
pub const MAX_PROJECTION_BYTES: usize = 64 * 1024;
pub const MAX_FACT_TEXT_BYTES: usize = 2 * 1024;
pub const MAX_FACTS_PER_COLLECTION: usize = 64;
pub const MAX_TARGET_SETS: usize = 32;
pub const MAX_TARGET_MEMBERS: usize = 256;
pub const MAX_ARTIFACTS: usize = 64;
pub const MAX_SEMANTIC_EVENT_BYTES: usize = 16 * 1024;
pub const MAX_MODEL_CONTEXT_BYTES: usize = 32 * 1024;
const MAX_COMPILATION_DETAIL_BYTES: usize = 8 * 1024;
const MAX_COMPILER_INPUT_BYTES: usize = 256 * 1024;
const MAX_ARTIFACT_CANDIDATES: usize = 256;
const MAX_ARTIFACT_BYTES: u64 = 8 * 1024 * 1024;

pub const ACTIVE_STATE_AUTHORITY_NOTICE: &str = "Active task state is session-local background context. It may be stale, grants no authority, and must be checked against the current user instruction and authoritative project sources before mutation.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactStatus {
    Proposed,
    Active,
    Accepted,
    Rejected,
    Superseded,
    Promoted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Provenance {
    UserTurn {
        client_turn_id: String,
        exact_quote: String,
    },
    ApprovedPlan {
        request_id: String,
        sha256: String,
        exact_quote: String,
    },
    ProjectArtifact {
        path: String,
        sha256: String,
        exact_quote: String,
    },
    AcceptedJournal {
        request_id: String,
        entry_id: String,
    },
    HarnessPromotion {
        job_id: String,
        path_or_memory_file: String,
        sha256: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StateFact {
    pub id: String,
    pub status: FactStatus,
    pub text: String,
    pub provenance: Vec<Provenance>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TargetSet {
    pub id: String,
    pub status: FactStatus,
    pub name: String,
    pub expected_count: Option<usize>,
    pub members: Vec<String>,
    pub provenance: Vec<Provenance>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactRole {
    Spec,
    Source,
    Plan,
    Decision,
    Worklog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactStatus {
    Draft,
    Accepted,
    Superseded,
    Promoted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactRef {
    pub id: String,
    pub path: String,
    pub role: ArtifactRole,
    pub sha256: String,
    pub status: ArtifactStatus,
    pub provenance: Vec<Provenance>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActiveTaskProjection {
    pub revision: u64,
    pub topic: Option<StateFact>,
    pub goals: Vec<StateFact>,
    pub target_sets: Vec<TargetSet>,
    pub constraints: Vec<StateFact>,
    pub decisions: Vec<StateFact>,
    pub authoritative_artifacts: Vec<ArtifactRef>,
    pub blockers: Vec<StateFact>,
    pub acceptance_criteria: Vec<StateFact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "entityType",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum TaskStateEntity {
    Topic { fact: StateFact },
    Goal { fact: StateFact },
    TargetSet { target_set: TargetSet },
    Constraint { fact: StateFact },
    Decision { fact: StateFact },
    AuthoritativeArtifact { artifact: ArtifactRef },
    Blocker { fact: StateFact },
    AcceptanceCriterion { fact: StateFact },
}

impl TaskStateEntity {
    fn id(&self) -> &str {
        match self {
            Self::Topic { fact }
            | Self::Goal { fact }
            | Self::Constraint { fact }
            | Self::Decision { fact }
            | Self::Blocker { fact }
            | Self::AcceptanceCriterion { fact } => &fact.id,
            Self::TargetSet { target_set } => &target_set.id,
            Self::AuthoritativeArtifact { artifact } => &artifact.id,
        }
    }

    fn fact_status(&self) -> FactStatus {
        match self {
            Self::Topic { fact }
            | Self::Goal { fact }
            | Self::Constraint { fact }
            | Self::Decision { fact }
            | Self::Blocker { fact }
            | Self::AcceptanceCriterion { fact } => fact.status,
            Self::TargetSet { target_set } => target_set.status,
            Self::AuthoritativeArtifact { artifact } => match artifact.status {
                ArtifactStatus::Draft => FactStatus::Active,
                ArtifactStatus::Accepted => FactStatus::Accepted,
                ArtifactStatus::Superseded => FactStatus::Superseded,
                ArtifactStatus::Promoted => FactStatus::Promoted,
            },
        }
    }

    fn provenance(&self) -> &[Provenance] {
        match self {
            Self::Topic { fact }
            | Self::Goal { fact }
            | Self::Constraint { fact }
            | Self::Decision { fact }
            | Self::Blocker { fact }
            | Self::AcceptanceCriterion { fact } => &fact.provenance,
            Self::TargetSet { target_set } => &target_set.provenance,
            Self::AuthoritativeArtifact { artifact } => &artifact.provenance,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "op",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum TaskStateOperation {
    Upsert {
        entity: TaskStateEntity,
    },
    Supersede {
        fact_id: String,
        replacement_id: Option<String>,
    },
    Remove {
        fact_id: String,
    },
    CloseRequest {
        request_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskStateDelta {
    pub base_revision: u64,
    pub operations: Vec<TaskStateOperation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum TaskStateEventKind {
    SemanticDelta {
        delta: TaskStateDelta,
    },
    TurnCancelled,
    RequestAccepted {
        journal_entry_ids: Vec<String>,
        harness_job_id: Option<String>,
    },
    RequestRejected {
        journal_entry_ids: Vec<String>,
    },
    PromotionAccepted {
        harness_job_id: String,
        fact_ids: Vec<String>,
        document_refs: Vec<PromotedRef>,
        memory_refs: Vec<PromotedRef>,
    },
    PromotionRejected {
        harness_job_id: String,
    },
    StateCompilationFailed {
        reason_code: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    CompactionBoundary {
        instruction_epoch: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskStateEvent {
    pub id: String,
    pub parent_id: Option<String>,
    pub revision: u64,
    pub client_turn_id: Option<String>,
    pub request_id: Option<String>,
    pub timestamp: u64,
    pub kind: TaskStateEventKind,
}

impl TaskStateEvent {
    pub fn new(
        client_turn_id: Option<String>,
        request_id: Option<String>,
        kind: TaskStateEventKind,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            parent_id: None,
            revision: 0,
            client_turn_id,
            request_id,
            timestamp: crate::session::now_unix_seconds(),
            kind,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PromotedRef {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskStatePromotionInput {
    pub source_task_revision: u64,
    pub source_event_id: String,
    pub fact_ids: Vec<String>,
    pub candidates: Vec<PromotionCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PromotionCandidate {
    pub id: String,
    pub category: String,
    pub text: String,
    pub provenance: Vec<Provenance>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskStatePromotionAudit {
    pub harness_job_id: String,
    pub source_event_id: String,
    pub fact_ids: Vec<String>,
    pub accepted: bool,
    pub document_refs: Vec<PromotedRef>,
    pub memory_refs: Vec<PromotedRef>,
    pub timestamp: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTaskState {
    pub schema_version: u32,
    pub events: Vec<TaskStateEvent>,
    pub leaf_id: Option<String>,
    pub projection: ActiveTaskProjection,
    pub projection_checksum: String,
    #[serde(default)]
    pub promotion_audits: Vec<TaskStatePromotionAudit>,
    #[serde(default)]
    pub compilation_stale: bool,
}

impl Default for SessionTaskState {
    fn default() -> Self {
        let projection = ActiveTaskProjection::default();
        Self {
            schema_version: TASK_STATE_SCHEMA_VERSION,
            events: Vec::new(),
            leaf_id: None,
            projection_checksum: projection_checksum(&projection),
            projection,
            promotion_audits: Vec::new(),
            compilation_stale: false,
        }
    }
}

impl SessionTaskState {
    pub fn repair_cache(&mut self) -> Result<bool, String> {
        let expected_revision = self
            .leaf_id
            .as_deref()
            .and_then(|leaf| self.events.iter().find(|event| event.id == leaf))
            .map_or(0, |event| event.revision);
        let checksum_matches = self.projection.revision == expected_revision
            && self.projection_checksum == projection_checksum(&self.projection);
        if checksum_matches {
            return Ok(false);
        }
        self.rebuild_projection()?;
        Ok(true)
    }

    pub fn rebuild_projection(&mut self) -> Result<(), String> {
        let path = self.branch_path()?;
        let mut projection = ActiveTaskProjection::default();
        let mut stale = false;
        for (index, event) in path.iter().enumerate() {
            apply_event(&mut projection, event, &path[..=index])?;
            stale = match event.kind {
                TaskStateEventKind::SemanticDelta { .. } => false,
                TaskStateEventKind::StateCompilationFailed { .. } => true,
                _ => stale,
            };
            projection.revision = event.revision;
        }
        validate_projection_bounds(&projection)?;
        self.projection_checksum = projection_checksum(&projection);
        self.projection = projection;
        self.compilation_stale = stale;
        Ok(())
    }

    pub fn append_event(
        &mut self,
        expected_leaf: Option<&str>,
        mut event: TaskStateEvent,
    ) -> Result<(), String> {
        if self.leaf_id.as_deref() != expected_leaf {
            return Err("task state leaf changed concurrently".to_string());
        }
        if self.events.iter().any(|existing| existing.id == event.id) {
            return Err("task state event id already exists".to_string());
        }
        if event.id.trim().is_empty() {
            return Err("task state event id is empty".to_string());
        }
        if event
            .client_turn_id
            .as_deref()
            .is_some_and(|id| uuid::Uuid::parse_str(id).is_err())
        {
            return Err("task state client turn id is not a UUID".to_string());
        }
        event.parent_id = self.leaf_id.clone();
        event.revision = self
            .events
            .iter()
            .map(|existing| existing.revision)
            .max()
            .unwrap_or_default()
            .checked_add(1)
            .ok_or_else(|| "task state revision overflow".to_string())?;
        if let TaskStateEventKind::SemanticDelta { delta } = &event.kind {
            if delta.base_revision != self.projection.revision {
                return Err(format!(
                    "task state base revision {} does not match current revision {}",
                    delta.base_revision, self.projection.revision
                ));
            }
            let size = serde_json::to_vec(delta)
                .map_err(|error| error.to_string())?
                .len();
            if size > MAX_SEMANTIC_EVENT_BYTES {
                return Err(format!(
                    "semantic task state event is {size} bytes, over the {MAX_SEMANTIC_EVENT_BYTES}-byte limit"
                ));
            }
        }
        if let TaskStateEventKind::StateCompilationFailed {
            reason_code,
            detail,
        } = &event.kind
        {
            if reason_code.is_empty()
                || reason_code.len() > 64
                || !reason_code
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
            {
                return Err("task state compilation reason code is invalid".to_string());
            }
            if let Some(detail) = detail {
                if detail.is_empty() || detail.len() > MAX_COMPILATION_DETAIL_BYTES {
                    return Err("task state compilation detail is invalid".to_string());
                }
            }
        }

        self.events.push(event.clone());
        self.leaf_id = Some(event.id);
        if let Err(error) = self.rebuild_projection() {
            self.events.pop();
            self.leaf_id = expected_leaf.map(str::to_string);
            let _ = self.rebuild_projection();
            return Err(error);
        }
        Ok(())
    }

    pub fn move_leaf_to_client_turn(&mut self, client_turn_id: Option<&str>) -> Result<(), String> {
        let Some(client_turn_id) = client_turn_id else {
            self.leaf_id = None;
            return self.rebuild_projection();
        };
        let target = self
            .branch_path()?
            .into_iter()
            .rev()
            .find(|event| event.client_turn_id.as_deref() == Some(client_turn_id))
            .map(|event| event.id.clone());
        self.leaf_id = target;
        self.rebuild_projection()
    }

    pub fn is_current_ancestor(&self, event_id: &str) -> bool {
        self.branch_path()
            .map(|path| path.iter().any(|event| event.id == event_id))
            .unwrap_or(false)
    }

    pub fn render_full(&self, instruction_epoch: u64) -> Result<String, String> {
        let payload = serde_json::json!({
            "revision": self.projection.revision,
            "leafId": self.leaf_id,
            "compilationStale": self.compilation_stale,
            "projection": self.projection,
        });
        render_model_state("snapshot", instruction_epoch, &payload)
    }

    pub fn render_delta(
        &self,
        instruction_epoch: u64,
        delivered_revision: u64,
    ) -> Result<Option<String>, String> {
        if delivered_revision > self.projection.revision {
            return self.render_full(instruction_epoch).map(Some);
        }
        let events = self
            .branch_path()?
            .into_iter()
            .filter(|event| event.revision > delivered_revision)
            .collect::<Vec<_>>();
        if events.is_empty() {
            return Ok(None);
        }
        let payload = serde_json::json!({
            "fromRevision": delivered_revision,
            "toRevision": self.projection.revision,
            "events": events,
        });
        render_model_state("delta", instruction_epoch, &payload).map(Some)
    }

    pub fn promotion_input_for_request(&self, request_id: &str) -> Option<TaskStatePromotionInput> {
        let source_event = self.branch_path().ok()?.into_iter().rev().find(|event| {
            event.request_id.as_deref() == Some(request_id)
                && matches!(event.kind, TaskStateEventKind::RequestAccepted { .. })
        })?;
        let ids = fact_ids_for_request(&self.branch_path().ok()?, request_id);
        let candidates = projection_candidates(&self.projection, &ids);
        let encoded = serde_json::to_vec(&candidates).ok()?;
        if encoded.len() > MAX_SEMANTIC_EVENT_BYTES {
            return None;
        }
        Some(TaskStatePromotionInput {
            source_task_revision: self.projection.revision,
            source_event_id: source_event.id.clone(),
            fact_ids: candidates
                .iter()
                .map(|candidate| candidate.id.clone())
                .collect(),
            candidates,
        })
    }

    fn branch_path(&self) -> Result<Vec<&TaskStateEvent>, String> {
        let Some(mut current) = self.leaf_id.as_deref() else {
            return Ok(Vec::new());
        };
        let by_id = self
            .events
            .iter()
            .map(|event| (event.id.as_str(), event))
            .collect::<HashMap<_, _>>();
        let mut seen = HashSet::new();
        let mut reversed = Vec::new();
        loop {
            if !seen.insert(current.to_string()) {
                return Err("task state event graph contains a cycle".to_string());
            }
            let event = by_id
                .get(current)
                .copied()
                .ok_or_else(|| format!("task state event `{current}` is missing"))?;
            reversed.push(event);
            match event.parent_id.as_deref() {
                Some(parent) => current = parent,
                None => break,
            }
        }
        reversed.reverse();
        for pair in reversed.windows(2) {
            if pair[1].parent_id.as_deref() != Some(pair[0].id.as_str())
                || pair[1].revision <= pair[0].revision
            {
                return Err("task state event branch ordering is invalid".to_string());
            }
        }
        Ok(reversed)
    }
}

fn render_model_state(
    delivery: &str,
    instruction_epoch: u64,
    payload: &serde_json::Value,
) -> Result<String, String> {
    let json = serde_json::to_string(payload).map_err(|error| error.to_string())?;
    let rendered = format!(
        "[active task state delivery={delivery} instructionEpoch={instruction_epoch}]\n{ACTIVE_STATE_AUTHORITY_NOTICE}\n{json}"
    );
    if rendered.len() > MAX_MODEL_CONTEXT_BYTES {
        return Err(format!(
            "rendered active task state is {} bytes, over the {MAX_MODEL_CONTEXT_BYTES}-byte limit",
            rendered.len()
        ));
    }
    Ok(rendered)
}

fn apply_event(
    projection: &mut ActiveTaskProjection,
    event: &TaskStateEvent,
    branch_prefix: &[&TaskStateEvent],
) -> Result<(), String> {
    match &event.kind {
        TaskStateEventKind::SemanticDelta { delta } => {
            if delta.base_revision != projection.revision {
                return Err(format!(
                    "semantic event base revision {} does not match replay revision {}",
                    delta.base_revision, projection.revision
                ));
            }
            apply_operations(projection, &delta.operations)?;
        }
        TaskStateEventKind::RequestAccepted { .. } => {
            if let Some(request_id) = event.request_id.as_deref() {
                let ids = fact_ids_for_request(branch_prefix, request_id);
                set_facts_status(projection, &ids, FactStatus::Accepted);
            }
        }
        TaskStateEventKind::RequestRejected { .. } => {
            if let Some(request_id) = event.request_id.as_deref() {
                let ids = fact_ids_for_request(branch_prefix, request_id);
                set_facts_status(projection, &ids, FactStatus::Rejected);
            }
        }
        TaskStateEventKind::PromotionAccepted {
            harness_job_id,
            fact_ids,
            document_refs,
            memory_refs,
        } => {
            mark_promoted(
                projection,
                fact_ids,
                harness_job_id,
                document_refs,
                memory_refs,
            )?;
        }
        TaskStateEventKind::TurnCancelled
        | TaskStateEventKind::PromotionRejected { .. }
        | TaskStateEventKind::StateCompilationFailed { .. }
        | TaskStateEventKind::CompactionBoundary { .. } => {}
    }
    validate_projection_bounds(projection)
}

fn apply_operations(
    projection: &mut ActiveTaskProjection,
    operations: &[TaskStateOperation],
) -> Result<(), String> {
    for operation in operations {
        match operation {
            TaskStateOperation::Upsert { entity } => upsert_entity(projection, entity.clone())?,
            TaskStateOperation::Supersede {
                fact_id,
                replacement_id,
            } => {
                if replacement_id
                    .as_deref()
                    .is_some_and(|replacement| !projection_contains_id(projection, replacement))
                {
                    return Err(format!(
                        "replacement fact `{}` does not exist",
                        replacement_id.as_deref().unwrap_or_default()
                    ));
                }
                update_status(projection, fact_id, FactStatus::Superseded)?;
            }
            TaskStateOperation::Remove { fact_id } => remove_entity(projection, fact_id)?,
            TaskStateOperation::CloseRequest { request_id } => {
                for blocker in &mut projection.blockers {
                    if blocker
                        .provenance
                        .iter()
                        .any(|provenance| match provenance {
                            Provenance::ApprovedPlan {
                                request_id: source, ..
                            }
                            | Provenance::AcceptedJournal {
                                request_id: source, ..
                            } => source == request_id,
                            _ => false,
                        })
                        && matches!(blocker.status, FactStatus::Proposed | FactStatus::Active)
                    {
                        blocker.status = FactStatus::Superseded;
                    }
                }
            }
        }
    }
    validate_projection_bounds(projection)
}

fn upsert_entity(
    projection: &mut ActiveTaskProjection,
    entity: TaskStateEntity,
) -> Result<(), String> {
    validate_entity_shape(&entity)?;
    let id = entity.id().to_string();
    let existing_status = projection_status(projection, &id);
    if let Some(status) = existing_status {
        validate_transition(status, entity.fact_status())?;
    } else if entity.fact_status() == FactStatus::Promoted {
        return Err("compiler cannot introduce promoted state".to_string());
    }
    if projection_contains_id(projection, &id) {
        remove_entity_raw(projection, &id);
    }
    match entity {
        TaskStateEntity::Topic { fact } => projection.topic = Some(fact),
        TaskStateEntity::Goal { fact } => projection.goals.push(fact),
        TaskStateEntity::TargetSet { target_set } => projection.target_sets.push(target_set),
        TaskStateEntity::Constraint { fact } => projection.constraints.push(fact),
        TaskStateEntity::Decision { fact } => projection.decisions.push(fact),
        TaskStateEntity::AuthoritativeArtifact { artifact } => {
            projection.authoritative_artifacts.push(artifact)
        }
        TaskStateEntity::Blocker { fact } => projection.blockers.push(fact),
        TaskStateEntity::AcceptanceCriterion { fact } => projection.acceptance_criteria.push(fact),
    }
    Ok(())
}

fn validate_transition(from: FactStatus, to: FactStatus) -> Result<(), String> {
    let valid = from == to
        || matches!(
            (from, to),
            (FactStatus::Proposed, FactStatus::Active)
                | (FactStatus::Proposed, FactStatus::Accepted)
                | (FactStatus::Proposed, FactStatus::Rejected)
                | (FactStatus::Proposed, FactStatus::Superseded)
                | (FactStatus::Active, FactStatus::Accepted)
                | (FactStatus::Active, FactStatus::Rejected)
                | (FactStatus::Active, FactStatus::Superseded)
                | (FactStatus::Accepted, FactStatus::Promoted)
                | (FactStatus::Accepted, FactStatus::Superseded)
        );
    valid
        .then_some(())
        .ok_or_else(|| format!("invalid task state transition {from:?} -> {to:?}"))
}

fn validate_entity_shape(entity: &TaskStateEntity) -> Result<(), String> {
    if entity.id().trim().is_empty() || entity.id().len() > 128 {
        return Err("task state fact id must contain 1 to 128 bytes".to_string());
    }
    if entity.provenance().is_empty() || entity.provenance().len() > 8 {
        return Err("task state entity must have 1 to 8 provenance records".to_string());
    }
    match entity {
        TaskStateEntity::Topic { fact }
        | TaskStateEntity::Goal { fact }
        | TaskStateEntity::Constraint { fact }
        | TaskStateEntity::Decision { fact }
        | TaskStateEntity::Blocker { fact }
        | TaskStateEntity::AcceptanceCriterion { fact } => validate_fact(fact),
        TaskStateEntity::TargetSet { target_set } => {
            if target_set.name.trim().is_empty() || target_set.name.len() > MAX_FACT_TEXT_BYTES {
                return Err("target set name is empty or too large".to_string());
            }
            if target_set.members.len() > MAX_TARGET_MEMBERS {
                return Err(format!(
                    "target set has {} members, over the {MAX_TARGET_MEMBERS}-member limit",
                    target_set.members.len()
                ));
            }
            if target_set
                .expected_count
                .is_some_and(|count| count != target_set.members.len())
            {
                return Err("target set expected count does not match explicit members".to_string());
            }
            let mut members = BTreeSet::new();
            for member in &target_set.members {
                if member.trim().is_empty() || member.len() > MAX_FACT_TEXT_BYTES {
                    return Err("target set member is empty or too large".to_string());
                }
                if !members.insert(member) {
                    return Err("target set contains duplicate members".to_string());
                }
            }
            Ok(())
        }
        TaskStateEntity::AuthoritativeArtifact { artifact } => {
            if artifact.sha256.len() != 64
                || !artifact.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err("artifact sha256 is invalid".to_string());
            }
            validate_artifact_role_path(artifact.role, &artifact.path)
        }
    }
}

fn validate_fact(fact: &StateFact) -> Result<(), String> {
    if fact.text.trim().is_empty() || fact.text.len() > MAX_FACT_TEXT_BYTES {
        return Err(format!(
            "fact text must contain 1 to {MAX_FACT_TEXT_BYTES} bytes"
        ));
    }
    Ok(())
}

fn validate_projection_bounds(projection: &ActiveTaskProjection) -> Result<(), String> {
    for (name, count) in [
        ("goals", projection.goals.len()),
        ("constraints", projection.constraints.len()),
        ("decisions", projection.decisions.len()),
        ("blockers", projection.blockers.len()),
        ("acceptance criteria", projection.acceptance_criteria.len()),
    ] {
        if count > MAX_FACTS_PER_COLLECTION {
            return Err(format!(
                "task state {name} has {count} entries, over the {MAX_FACTS_PER_COLLECTION}-entry limit"
            ));
        }
    }
    if projection.target_sets.len() > MAX_TARGET_SETS {
        return Err(format!(
            "task state has {} target sets, over the {MAX_TARGET_SETS}-set limit",
            projection.target_sets.len()
        ));
    }
    if projection.authoritative_artifacts.len() > MAX_ARTIFACTS {
        return Err(format!(
            "task state has {} artifacts, over the {MAX_ARTIFACTS}-artifact limit",
            projection.authoritative_artifacts.len()
        ));
    }
    let mut ids = HashSet::new();
    for id in projection_ids(projection) {
        if !ids.insert(id) {
            return Err("task state projection contains duplicate fact ids".to_string());
        }
    }
    let encoded = serde_json::to_vec(projection).map_err(|error| error.to_string())?;
    if encoded.len() > MAX_PROJECTION_BYTES {
        return Err(format!(
            "task state projection is {} bytes, over the {MAX_PROJECTION_BYTES}-byte limit",
            encoded.len()
        ));
    }
    Ok(())
}

fn projection_ids(projection: &ActiveTaskProjection) -> Vec<&str> {
    let mut ids = Vec::new();
    if let Some(topic) = &projection.topic {
        ids.push(topic.id.as_str());
    }
    ids.extend(projection.goals.iter().map(|fact| fact.id.as_str()));
    ids.extend(projection.target_sets.iter().map(|set| set.id.as_str()));
    ids.extend(projection.constraints.iter().map(|fact| fact.id.as_str()));
    ids.extend(projection.decisions.iter().map(|fact| fact.id.as_str()));
    ids.extend(
        projection
            .authoritative_artifacts
            .iter()
            .map(|artifact| artifact.id.as_str()),
    );
    ids.extend(projection.blockers.iter().map(|fact| fact.id.as_str()));
    ids.extend(
        projection
            .acceptance_criteria
            .iter()
            .map(|fact| fact.id.as_str()),
    );
    ids
}

fn projection_contains_id(projection: &ActiveTaskProjection, id: &str) -> bool {
    projection_ids(projection).contains(&id)
}

fn projection_status(projection: &ActiveTaskProjection, id: &str) -> Option<FactStatus> {
    projection
        .topic
        .iter()
        .chain(projection.goals.iter())
        .chain(projection.constraints.iter())
        .chain(projection.decisions.iter())
        .chain(projection.blockers.iter())
        .chain(projection.acceptance_criteria.iter())
        .find(|fact| fact.id == id)
        .map(|fact| fact.status)
        .or_else(|| {
            projection
                .target_sets
                .iter()
                .find(|set| set.id == id)
                .map(|set| set.status)
        })
        .or_else(|| {
            projection
                .authoritative_artifacts
                .iter()
                .find(|artifact| artifact.id == id)
                .map(|artifact| match artifact.status {
                    ArtifactStatus::Draft => FactStatus::Active,
                    ArtifactStatus::Accepted => FactStatus::Accepted,
                    ArtifactStatus::Superseded => FactStatus::Superseded,
                    ArtifactStatus::Promoted => FactStatus::Promoted,
                })
        })
}

fn update_status(
    projection: &mut ActiveTaskProjection,
    id: &str,
    status: FactStatus,
) -> Result<(), String> {
    let from = projection_status(projection, id)
        .ok_or_else(|| format!("task state fact `{id}` does not exist"))?;
    validate_transition(from, status)?;
    set_facts_status(projection, &[id.to_string()], status);
    Ok(())
}

fn set_facts_status(projection: &mut ActiveTaskProjection, ids: &[String], status: FactStatus) {
    let ids = ids.iter().map(String::as_str).collect::<HashSet<_>>();
    for fact in projection
        .topic
        .iter_mut()
        .chain(projection.goals.iter_mut())
        .chain(projection.constraints.iter_mut())
        .chain(projection.decisions.iter_mut())
        .chain(projection.blockers.iter_mut())
        .chain(projection.acceptance_criteria.iter_mut())
    {
        if ids.contains(fact.id.as_str()) && validate_transition(fact.status, status).is_ok() {
            fact.status = status;
        }
    }
    for set in &mut projection.target_sets {
        if ids.contains(set.id.as_str()) && validate_transition(set.status, status).is_ok() {
            set.status = status;
        }
    }
    for artifact in &mut projection.authoritative_artifacts {
        if !ids.contains(artifact.id.as_str()) {
            continue;
        }
        artifact.status = match status {
            FactStatus::Proposed | FactStatus::Active => ArtifactStatus::Draft,
            FactStatus::Accepted => ArtifactStatus::Accepted,
            FactStatus::Rejected | FactStatus::Superseded => ArtifactStatus::Superseded,
            FactStatus::Promoted => ArtifactStatus::Promoted,
        };
    }
}

fn mark_promoted(
    projection: &mut ActiveTaskProjection,
    ids: &[String],
    harness_job_id: &str,
    document_refs: &[PromotedRef],
    memory_refs: &[PromotedRef],
) -> Result<(), String> {
    let promoted_ref = document_refs.first().or_else(|| memory_refs.first());
    if !ids.is_empty() && promoted_ref.is_none() {
        return Err(
            "promoted task-state facts require an accepted document or memory ref".to_string(),
        );
    }
    set_facts_status(projection, ids, FactStatus::Promoted);
    let Some(promoted_ref) = promoted_ref else {
        return Ok(());
    };
    let provenance = Provenance::HarnessPromotion {
        job_id: harness_job_id.to_string(),
        path_or_memory_file: promoted_ref.path.clone(),
        sha256: promoted_ref.sha256.clone(),
    };
    let ids = ids.iter().map(String::as_str).collect::<HashSet<_>>();
    for fact in projection
        .topic
        .iter_mut()
        .chain(projection.goals.iter_mut())
        .chain(projection.constraints.iter_mut())
        .chain(projection.decisions.iter_mut())
        .chain(projection.blockers.iter_mut())
        .chain(projection.acceptance_criteria.iter_mut())
    {
        if ids.contains(fact.id.as_str()) && !fact.provenance.contains(&provenance) {
            fact.provenance.push(provenance.clone());
        }
    }
    for set in &mut projection.target_sets {
        if ids.contains(set.id.as_str()) && !set.provenance.contains(&provenance) {
            set.provenance.push(provenance.clone());
        }
    }
    for artifact in &mut projection.authoritative_artifacts {
        if ids.contains(artifact.id.as_str()) && !artifact.provenance.contains(&provenance) {
            artifact.provenance.push(provenance.clone());
        }
    }
    Ok(())
}

fn remove_entity(projection: &mut ActiveTaskProjection, id: &str) -> Result<(), String> {
    let status = projection_status(projection, id)
        .ok_or_else(|| format!("task state fact `{id}` does not exist"))?;
    if matches!(status, FactStatus::Accepted | FactStatus::Promoted) {
        return Err("accepted or promoted facts must be superseded, not removed".to_string());
    }
    remove_entity_raw(projection, id);
    Ok(())
}

fn remove_entity_raw(projection: &mut ActiveTaskProjection, id: &str) {
    if projection.topic.as_ref().is_some_and(|fact| fact.id == id) {
        projection.topic = None;
    }
    projection.goals.retain(|fact| fact.id != id);
    projection.target_sets.retain(|set| set.id != id);
    projection.constraints.retain(|fact| fact.id != id);
    projection.decisions.retain(|fact| fact.id != id);
    projection
        .authoritative_artifacts
        .retain(|artifact| artifact.id != id);
    projection.blockers.retain(|fact| fact.id != id);
    projection.acceptance_criteria.retain(|fact| fact.id != id);
}

fn fact_ids_for_request(path: &[&TaskStateEvent], request_id: &str) -> Vec<String> {
    let mut ids = BTreeSet::new();
    for event in path {
        if event.request_id.as_deref() != Some(request_id) {
            continue;
        }
        if let TaskStateEventKind::SemanticDelta { delta } = &event.kind {
            for operation in &delta.operations {
                match operation {
                    TaskStateOperation::Upsert { entity } => {
                        ids.insert(entity.id().to_string());
                    }
                    TaskStateOperation::Supersede { fact_id, .. }
                    | TaskStateOperation::Remove { fact_id } => {
                        ids.insert(fact_id.clone());
                    }
                    TaskStateOperation::CloseRequest { .. } => {}
                }
            }
        }
    }
    ids.into_iter().collect()
}

fn projection_candidates(
    projection: &ActiveTaskProjection,
    ids: &[String],
) -> Vec<PromotionCandidate> {
    let ids = ids.iter().map(String::as_str).collect::<HashSet<_>>();
    let mut candidates = Vec::new();
    let mut push_fact = |category: &str, fact: &StateFact| {
        if ids.contains(fact.id.as_str()) && fact.status == FactStatus::Accepted {
            candidates.push(PromotionCandidate {
                id: fact.id.clone(),
                category: category.to_string(),
                text: fact.text.clone(),
                provenance: fact.provenance.clone(),
            });
        }
    };
    if let Some(topic) = &projection.topic {
        push_fact("topic", topic);
    }
    for fact in &projection.goals {
        push_fact("goal", fact);
    }
    for fact in &projection.constraints {
        push_fact("constraint", fact);
    }
    for fact in &projection.decisions {
        push_fact("decision", fact);
    }
    for fact in &projection.blockers {
        push_fact("blocker", fact);
    }
    for fact in &projection.acceptance_criteria {
        push_fact("acceptance_criterion", fact);
    }
    for set in &projection.target_sets {
        if ids.contains(set.id.as_str()) && set.status == FactStatus::Accepted {
            candidates.push(PromotionCandidate {
                id: set.id.clone(),
                category: "target_set".to_string(),
                text: format!(
                    "{} (expected_count={}): {}",
                    set.name,
                    set.expected_count.unwrap_or(set.members.len()),
                    set.members.join(", ")
                ),
                provenance: set.provenance.clone(),
            });
        }
    }
    for artifact in &projection.authoritative_artifacts {
        if ids.contains(artifact.id.as_str()) && artifact.status == ArtifactStatus::Accepted {
            candidates.push(PromotionCandidate {
                id: artifact.id.clone(),
                category: "authoritative_artifact".to_string(),
                text: format!("{} ({})", artifact.path, artifact.sha256),
                provenance: artifact.provenance.clone(),
            });
        }
    }
    candidates.sort_by(|left, right| left.id.cmp(&right.id));
    candidates
}

fn projection_checksum(projection: &ActiveTaskProjection) -> String {
    let bytes = serde_json::to_vec(projection).unwrap_or_default();
    sha256_bytes(&bytes)
}

pub struct ProvenanceValidationContext<'a> {
    pub client_turn_id: &'a str,
    pub user_text: &'a str,
    pub request_id: &'a str,
    pub approved_plan: Option<ApprovedPlanEvidence<'a>>,
    pub workspace_root: Option<&'a Path>,
    pub accepted_journal_entry_ids: &'a HashSet<String>,
}

#[derive(Clone, Copy)]
pub struct ApprovedPlanEvidence<'a> {
    pub request_id: &'a str,
    pub markdown: &'a str,
    pub sha256: &'a str,
}

pub fn validate_compiler_delta(
    current: &ActiveTaskProjection,
    delta: &TaskStateDelta,
    context: &ProvenanceValidationContext<'_>,
) -> Result<ActiveTaskProjection, String> {
    if delta.base_revision != current.revision {
        return Err(format!(
            "compiler base revision {} does not match current revision {}",
            delta.base_revision, current.revision
        ));
    }
    let encoded = serde_json::to_vec(delta).map_err(|error| error.to_string())?;
    if encoded.len() > MAX_SEMANTIC_EVENT_BYTES {
        return Err(format!(
            "compiler delta is {} bytes, over the {MAX_SEMANTIC_EVENT_BYTES}-byte limit",
            encoded.len()
        ));
    }
    for operation in &delta.operations {
        match operation {
            TaskStateOperation::Upsert { entity } => {
                validate_entity_shape(entity)?;
                for provenance in entity.provenance() {
                    validate_provenance(provenance, context)?;
                }
                if matches!(
                    entity.fact_status(),
                    FactStatus::Accepted | FactStatus::Promoted
                ) && !entity.provenance().iter().any(|provenance| {
                    matches!(
                        provenance,
                        Provenance::ApprovedPlan { .. }
                            | Provenance::AcceptedJournal { .. }
                            | Provenance::HarnessPromotion { .. }
                    )
                }) {
                    return Err("accepted facts require approved provenance".to_string());
                }
                if let TaskStateEntity::AuthoritativeArtifact { artifact } = entity {
                    validate_artifact_ref(artifact, context.workspace_root)?;
                }
            }
            TaskStateOperation::CloseRequest { request_id } if request_id != context.request_id => {
                return Err("compiler attempted to close a different request".to_string());
            }
            TaskStateOperation::Supersede { fact_id, .. }
            | TaskStateOperation::Remove { fact_id }
                if !projection_contains_id(current, fact_id) =>
            {
                return Err(format!("compiler referenced unknown fact `{fact_id}`"));
            }
            _ => {}
        }
    }
    let mut next = current.clone();
    apply_operations(&mut next, &delta.operations)?;
    next.revision = current.revision;
    validate_projection_bounds(&next)?;
    Ok(next)
}

fn validate_provenance(
    provenance: &Provenance,
    context: &ProvenanceValidationContext<'_>,
) -> Result<(), String> {
    match provenance {
        Provenance::UserTurn {
            client_turn_id,
            exact_quote,
        } => {
            if client_turn_id != context.client_turn_id
                || exact_quote.trim().is_empty()
                || !context.user_text.contains(exact_quote)
            {
                return Err("user-turn provenance does not match compiler input".to_string());
            }
        }
        Provenance::ApprovedPlan {
            request_id,
            sha256,
            exact_quote,
        } => {
            let evidence = context
                .approved_plan
                .ok_or_else(|| "approved-plan provenance has no approved plan input".to_string())?;
            if request_id != evidence.request_id
                || request_id != context.request_id
                || sha256 != evidence.sha256
                || sha256_bytes(evidence.markdown.as_bytes()) != *sha256
                || exact_quote.trim().is_empty()
                || !evidence.markdown.contains(exact_quote)
            {
                return Err("approved-plan provenance does not match compiler input".to_string());
            }
        }
        Provenance::ProjectArtifact {
            path,
            sha256,
            exact_quote,
        } => {
            let root = context
                .workspace_root
                .ok_or_else(|| "artifact provenance has no workspace".to_string())?;
            let bytes = read_verified_artifact(root, path, sha256)?;
            let content = std::str::from_utf8(&bytes)
                .map_err(|_| "artifact provenance path is not UTF-8 text".to_string())?;
            if exact_quote.trim().is_empty() || !content.contains(exact_quote) {
                return Err("artifact provenance quote is not present in the artifact".to_string());
            }
        }
        Provenance::AcceptedJournal {
            request_id,
            entry_id,
        } => {
            if request_id != context.request_id
                || !context.accepted_journal_entry_ids.contains(entry_id)
            {
                return Err("accepted-journal provenance is not in compiler evidence".to_string());
            }
        }
        Provenance::HarnessPromotion { .. } => {
            return Err(
                "foreground compiler cannot claim harness-promotion provenance".to_string(),
            );
        }
    }
    Ok(())
}

fn validate_artifact_ref(
    artifact: &ArtifactRef,
    workspace_root: Option<&Path>,
) -> Result<(), String> {
    validate_artifact_role_path(artifact.role, &artifact.path)?;
    let root = workspace_root.ok_or_else(|| "artifact reference has no workspace".to_string())?;
    read_verified_artifact(root, &artifact.path, &artifact.sha256).map(|_| ())
}

fn validate_artifact_role_path(role: ArtifactRole, path: &str) -> Result<(), String> {
    validate_project_relative_path(path)?;
    let prefix = match role {
        ArtifactRole::Spec => "specs/",
        ArtifactRole::Source => "source/",
        ArtifactRole::Plan => "plans/",
        ArtifactRole::Decision => "decisions/",
        ArtifactRole::Worklog => "worklog/",
    };
    if !path.starts_with(prefix) {
        return Err(format!(
            "artifact role {role:?} does not match path `{path}`"
        ));
    }
    Ok(())
}

fn validate_project_relative_path(path: &str) -> Result<(), String> {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains(':')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err("artifact path is not a confined project-relative path".to_string());
    }
    let allowed = ["specs/", "plans/", "decisions/", "worklog/", "source/"];
    if !allowed.iter().any(|prefix| path.starts_with(prefix)) {
        return Err(
            "artifact path is outside the allowed project document/source roots".to_string(),
        );
    }
    Ok(())
}

fn read_verified_artifact(
    root: &Path,
    relative: &str,
    expected_sha: &str,
) -> Result<Vec<u8>, String> {
    validate_project_relative_path(relative)?;
    let path = root.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
    let metadata =
        fs::symlink_metadata(&path).map_err(|_| format!("artifact `{relative}` does not exist"))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(format!("artifact `{relative}` is not a regular file"));
    }
    if metadata.len() > MAX_ARTIFACT_BYTES {
        return Err(format!(
            "artifact `{relative}` is too large for provenance validation"
        ));
    }
    let bytes =
        fs::read(&path).map_err(|error| format!("artifact `{relative}` read failed: {error}"))?;
    let actual = sha256_bytes(&bytes);
    if actual != expected_sha {
        return Err(format!(
            "artifact `{relative}` hash does not match current content"
        ));
    }
    Ok(bytes)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactCandidate {
    pub path: String,
    pub sha256: String,
}

pub fn collect_artifact_candidates(root: &Path) -> Result<Vec<ArtifactCandidate>, String> {
    let mut paths = Vec::new();
    for directory in ["specs", "plans", "decisions", "worklog", "source"] {
        collect_regular_files(root, &root.join(directory), &mut paths)?;
    }
    paths.sort();
    paths.truncate(MAX_ARTIFACT_CANDIDATES);
    let mut candidates = Vec::with_capacity(paths.len());
    for path in paths {
        let relative = path
            .strip_prefix(root)
            .map_err(|_| "artifact candidate escaped the workspace".to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        validate_project_relative_path(&relative)?;
        candidates.push(ArtifactCandidate {
            path: relative,
            sha256: sha256_file(&path)?,
        });
    }
    Ok(candidates)
}

fn collect_regular_files(
    root: &Path,
    path: &Path,
    output: &mut Vec<PathBuf>,
) -> Result<(), String> {
    if output.len() >= MAX_ARTIFACT_CANDIDATES {
        return Ok(());
    }
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    if metadata.is_file() {
        if metadata.len() <= MAX_ARTIFACT_BYTES && path.starts_with(root) {
            output.push(path.to_path_buf());
        }
        return Ok(());
    }
    if !metadata.is_dir() {
        return Ok(());
    }
    let mut children = fs::read_dir(path)
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    children.sort_by_key(std::fs::DirEntry::file_name);
    for child in children {
        collect_regular_files(root, &child.path(), output)?;
        if output.len() >= MAX_ARTIFACT_CANDIDATES {
            break;
        }
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskStateCompilerInput<'a> {
    pub previous_projection: &'a ActiveTaskProjection,
    pub current_user_text: &'a str,
    pub resolved_mentions: Option<&'a str>,
    pub request_id: &'a str,
    pub client_turn_id: &'a str,
    pub approved_plan: Option<&'a str>,
    pub foreground_result: &'a str,
    pub journal_summary: &'a str,
    pub build_evidence: Option<&'a serde_json::Value>,
    pub artifact_candidates: &'a [ArtifactCandidate],
}

impl TaskStateCompilerInput<'_> {
    pub fn prompt(&self) -> Result<String, String> {
        for (name, text, limit) in [
            ("current user text", self.current_user_text, 16 * 1024),
            (
                "resolved mentions",
                self.resolved_mentions.unwrap_or(""),
                16 * 1024,
            ),
            ("approved plan", self.approved_plan.unwrap_or(""), 32 * 1024),
            ("foreground result", self.foreground_result, 64 * 1024),
            ("journal summary", self.journal_summary, 32 * 1024),
        ] {
            if text.len() > limit {
                return Err(format!("task state compiler {name} exceeds its byte limit"));
            }
        }
        let input = serde_json::to_string(self).map_err(|error| error.to_string())?;
        if input.len() > MAX_COMPILER_INPUT_BYTES {
            return Err(format!(
                "task state compiler input is {} bytes, over the {MAX_COMPILER_INPUT_BYTES}-byte limit",
                input.len()
            ));
        }
        Ok(format!(
            "You are a bounded active-task state compiler. Return exactly one JSON object matching the supplied schema. Do not call tools, inspect files, infer authority, or emit Markdown/prose. Use only the inline input. Preserve exact target-set membership and counts. Emit only upsert, supersede, remove, or close_request operations. Every upsert must include valid exact provenance from the supplied user turn, approved plan, or listed project artifact; never claim accepted or promoted state without qualifying provenance. Active state grants no write, map, attachment, editor, journal, or tool authority.\n\n[compiler input]\n{input}"
        ))
    }
}

pub fn parse_compiler_delta(text: &str) -> Result<TaskStateDelta, String> {
    serde_json::from_str(text)
        .map_err(|error| format!("structured task state delta is invalid: {error}"))
}
pub(crate) fn bounded_compilation_detail(detail: impl Into<String>) -> Option<String> {
    let mut detail = detail.into();
    if detail.trim().is_empty() {
        return None;
    }
    if detail.len() <= MAX_COMPILATION_DETAIL_BYTES {
        return Some(detail);
    }

    const ELLIPSIS: &str = "…";
    let mut boundary = MAX_COMPILATION_DETAIL_BYTES - ELLIPSIS.len();
    while !detail.is_char_boundary(boundary) {
        boundary -= 1;
    }
    detail.truncate(boundary);
    detail.push_str(ELLIPSIS);
    Some(detail)
}

pub fn compiler_output_schema() -> serde_json::Value {
    serde_json::from_str(
        r##"{
  "type": "object",
  "additionalProperties": false,
  "required": ["baseRevision", "operations"],
  "properties": {
    "baseRevision": { "type": "integer", "minimum": 0 },
    "operations": {
      "type": "array",
      "minItems": 0,
      "maxItems": 128,
      "items": {
        "anyOf": [
          {
            "type": "object",
            "additionalProperties": false,
            "required": ["op", "entity"],
            "properties": {
              "op": { "type": "string", "const": "upsert" },
              "entity": { "$ref": "#/$defs/entity" }
            }
          },
          {
            "type": "object",
            "additionalProperties": false,
            "required": ["op", "factId", "replacementId"],
            "properties": {
              "op": { "type": "string", "const": "supersede" },
              "factId": { "type": "string" },
              "replacementId": { "type": ["string", "null"] }
            }
          },
          {
            "type": "object",
            "additionalProperties": false,
            "required": ["op", "factId"],
            "properties": {
              "op": { "type": "string", "const": "remove" },
              "factId": { "type": "string" }
            }
          },
          {
            "type": "object",
            "additionalProperties": false,
            "required": ["op", "requestId"],
            "properties": {
              "op": { "type": "string", "const": "close_request" },
              "requestId": { "type": "string" }
            }
          }
        ]
      }
    }
  },
  "$defs": {
    "status": {
      "type": "string",
      "enum": ["proposed", "active", "accepted", "rejected", "superseded"]
    },
    "provenance": {
      "anyOf": [
        {
          "type": "object",
          "additionalProperties": false,
          "required": ["kind", "clientTurnId", "exactQuote"],
          "properties": {
            "kind": { "type": "string", "const": "user_turn" },
            "clientTurnId": { "type": "string" },
            "exactQuote": { "type": "string", "minLength": 1, "maxLength": 2048 }
          }
        },
        {
          "type": "object",
          "additionalProperties": false,
          "required": ["kind", "requestId", "sha256", "exactQuote"],
          "properties": {
            "kind": { "type": "string", "const": "approved_plan" },
            "requestId": { "type": "string" },
            "sha256": { "type": "string", "pattern": "^[0-9a-fA-F]{64}$" },
            "exactQuote": { "type": "string", "minLength": 1, "maxLength": 2048 }
          }
        },
        {
          "type": "object",
          "additionalProperties": false,
          "required": ["kind", "path", "sha256", "exactQuote"],
          "properties": {
            "kind": { "type": "string", "const": "project_artifact" },
            "path": { "type": "string" },
            "sha256": { "type": "string", "pattern": "^[0-9a-fA-F]{64}$" },
            "exactQuote": { "type": "string", "minLength": 1, "maxLength": 2048 }
          }
        },
        {
          "type": "object",
          "additionalProperties": false,
          "required": ["kind", "requestId", "entryId"],
          "properties": {
            "kind": { "type": "string", "const": "accepted_journal" },
            "requestId": { "type": "string" },
            "entryId": { "type": "string" }
          }
        }
      ]
    },
    "provenanceList": {
      "type": "array",
      "minItems": 1,
      "maxItems": 8,
      "items": { "$ref": "#/$defs/provenance" }
    },
    "fact": {
      "type": "object",
      "additionalProperties": false,
      "required": ["id", "status", "text", "provenance"],
      "properties": {
        "id": { "type": "string", "minLength": 1, "maxLength": 128 },
        "status": { "$ref": "#/$defs/status" },
        "text": { "type": "string", "minLength": 1, "maxLength": 2048 },
        "provenance": { "$ref": "#/$defs/provenanceList" }
      }
    },
    "targetSet": {
      "type": "object",
      "additionalProperties": false,
      "required": ["id", "status", "name", "expectedCount", "members", "provenance"],
      "properties": {
        "id": { "type": "string", "minLength": 1, "maxLength": 128 },
        "status": { "$ref": "#/$defs/status" },
        "name": { "type": "string", "minLength": 1, "maxLength": 2048 },
        "expectedCount": { "type": ["integer", "null"], "minimum": 0, "maximum": 256 },
        "members": {
          "type": "array",
          "maxItems": 256,
          "items": { "type": "string", "minLength": 1, "maxLength": 2048 }
        },
        "provenance": { "$ref": "#/$defs/provenanceList" }
      }
    },
    "artifact": {
      "type": "object",
      "additionalProperties": false,
      "required": ["id", "path", "role", "sha256", "status", "provenance"],
      "properties": {
        "id": { "type": "string", "minLength": 1, "maxLength": 128 },
        "path": { "type": "string" },
        "role": { "type": "string", "enum": ["spec", "source", "plan", "decision", "worklog"] },
        "sha256": { "type": "string", "pattern": "^[0-9a-fA-F]{64}$" },
        "status": { "type": "string", "enum": ["draft", "accepted", "superseded"] },
        "provenance": { "$ref": "#/$defs/provenanceList" }
      }
    },
    "entity": {
      "anyOf": [
        { "$ref": "#/$defs/topicEntity" },
        { "$ref": "#/$defs/goalEntity" },
        { "$ref": "#/$defs/targetSetEntity" },
        { "$ref": "#/$defs/constraintEntity" },
        { "$ref": "#/$defs/decisionEntity" },
        { "$ref": "#/$defs/artifactEntity" },
        { "$ref": "#/$defs/blockerEntity" },
        { "$ref": "#/$defs/criterionEntity" }
      ]
    },
    "topicEntity": {
      "type": "object", "additionalProperties": false,
      "required": ["entityType", "fact"],
      "properties": { "entityType": { "type": "string", "const": "topic" }, "fact": { "$ref": "#/$defs/fact" } }
    },
    "goalEntity": {
      "type": "object", "additionalProperties": false,
      "required": ["entityType", "fact"],
      "properties": { "entityType": { "type": "string", "const": "goal" }, "fact": { "$ref": "#/$defs/fact" } }
    },
    "targetSetEntity": {
      "type": "object", "additionalProperties": false,
      "required": ["entityType", "targetSet"],
      "properties": { "entityType": { "type": "string", "const": "target_set" }, "targetSet": { "$ref": "#/$defs/targetSet" } }
    },
    "constraintEntity": {
      "type": "object", "additionalProperties": false,
      "required": ["entityType", "fact"],
      "properties": { "entityType": { "type": "string", "const": "constraint" }, "fact": { "$ref": "#/$defs/fact" } }
    },
    "decisionEntity": {
      "type": "object", "additionalProperties": false,
      "required": ["entityType", "fact"],
      "properties": { "entityType": { "type": "string", "const": "decision" }, "fact": { "$ref": "#/$defs/fact" } }
    },
    "artifactEntity": {
      "type": "object", "additionalProperties": false,
      "required": ["entityType", "artifact"],
      "properties": { "entityType": { "type": "string", "const": "authoritative_artifact" }, "artifact": { "$ref": "#/$defs/artifact" } }
    },
    "blockerEntity": {
      "type": "object", "additionalProperties": false,
      "required": ["entityType", "fact"],
      "properties": { "entityType": { "type": "string", "const": "blocker" }, "fact": { "$ref": "#/$defs/fact" } }
    },
    "criterionEntity": {
      "type": "object", "additionalProperties": false,
      "required": ["entityType", "fact"],
      "properties": { "entityType": { "type": "string", "const": "acceptance_criterion" }, "fact": { "$ref": "#/$defs/fact" } }
    }
  }
}"##,
    )
    .expect("task-state compiler output schema is valid JSON")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn_id(seed: u128) -> String {
        uuid::Uuid::from_u128(seed).to_string()
    }

    fn user_provenance(id: &str, quote: &str) -> Vec<Provenance> {
        vec![Provenance::UserTurn {
            client_turn_id: id.to_string(),
            exact_quote: quote.to_string(),
        }]
    }

    fn goal(id: &str, turn: &str, text: &str) -> TaskStateEntity {
        TaskStateEntity::Goal {
            fact: StateFact {
                id: id.to_string(),
                status: FactStatus::Active,
                text: text.to_string(),
                provenance: user_provenance(turn, text),
            },
        }
    }

    fn semantic_event(
        base_revision: u64,
        request_id: &str,
        turn: &str,
        operations: Vec<TaskStateOperation>,
    ) -> TaskStateEvent {
        TaskStateEvent::new(
            Some(turn.to_string()),
            Some(request_id.to_string()),
            TaskStateEventKind::SemanticDelta {
                delta: TaskStateDelta {
                    base_revision,
                    operations,
                },
            },
        )
    }

    #[test]
    fn compiler_output_schema_uses_supported_union_and_constant_shapes() {
        fn assert_typed_constants(value: &serde_json::Value) -> usize {
            match value {
                serde_json::Value::Object(schema) => {
                    assert!(
                        !schema.contains_key("oneOf"),
                        "compiler schema must use the supported anyOf composition"
                    );
                    let current = if let Some(constant) = schema.get("const") {
                        assert!(
                            constant.is_string(),
                            "compiler schema constant must be a string: {constant}"
                        );
                        assert_eq!(
                            schema.get("type").and_then(|value| value.as_str()),
                            Some("string"),
                            "compiler schema constant lacks an explicit string type: {constant}"
                        );
                        1
                    } else {
                        0
                    };
                    current + schema.values().map(assert_typed_constants).sum::<usize>()
                }
                serde_json::Value::Array(values) => values.iter().map(assert_typed_constants).sum(),
                _ => 0,
            }
        }

        assert_eq!(assert_typed_constants(&compiler_output_schema()), 16);
    }
    #[test]
    fn compilation_failure_detail_is_bounded_and_legacy_compatible() {
        let legacy: TaskStateEventKind = serde_json::from_str(
            r#"{"kind":"state_compilation_failed","reasonCode":"driver_error"}"#,
        )
        .unwrap();
        assert_eq!(
            legacy,
            TaskStateEventKind::StateCompilationFailed {
                reason_code: "driver_error".to_string(),
                detail: None,
            }
        );

        let detail = bounded_compilation_detail("가".repeat(MAX_COMPILATION_DETAIL_BYTES))
            .expect("non-empty diagnostic");
        assert!(detail.len() <= MAX_COMPILATION_DETAIL_BYTES);
        assert!(detail.ends_with('…'));

        let mut state = SessionTaskState::default();
        state
            .append_event(
                None,
                TaskStateEvent::new(
                    None,
                    None,
                    TaskStateEventKind::StateCompilationFailed {
                        reason_code: "driver_error".to_string(),
                        detail: Some(detail),
                    },
                ),
            )
            .unwrap();
        assert!(state.compilation_stale);
    }

    #[test]
    fn append_replay_is_deterministic_and_duplicate_upsert_is_idempotent() {
        let turn = turn_id(1);
        let operation = TaskStateOperation::Upsert {
            entity: goal("goal-1", &turn, "Keep all enemies in scope"),
        };
        let mut state = SessionTaskState::default();
        state
            .append_event(
                None,
                semantic_event(0, "req-1", &turn, vec![operation.clone()]),
            )
            .unwrap();
        let first = state.projection.clone();
        let leaf = state.leaf_id.clone();
        state
            .append_event(
                leaf.as_deref(),
                semantic_event(1, "req-1", &turn, vec![operation]),
            )
            .unwrap();
        assert_eq!(state.projection.goals.len(), 1);
        let checksum = state.projection_checksum.clone();
        state.projection = first;
        state.projection_checksum = "corrupt".to_string();
        assert!(state.repair_cache().unwrap());
        assert_eq!(state.projection.goals.len(), 1);
        assert_eq!(state.projection_checksum, checksum);
    }

    #[test]
    fn invalid_base_transition_and_target_count_are_rejected() {
        let turn = turn_id(2);
        let mut state = SessionTaskState::default();
        let invalid_base = semantic_event(
            9,
            "req-1",
            &turn,
            vec![TaskStateOperation::Upsert {
                entity: goal("goal-1", &turn, "goal"),
            }],
        );
        assert!(state.append_event(None, invalid_base).is_err());

        let target = TaskStateEntity::TargetSet {
            target_set: TargetSet {
                id: "enemies".to_string(),
                status: FactStatus::Active,
                name: "Enemy roster".to_string(),
                expected_count: Some(10),
                members: vec!["one".to_string()],
                provenance: user_provenance(&turn, "all enemies"),
            },
        };
        assert!(state
            .append_event(
                None,
                semantic_event(
                    0,
                    "req-1",
                    &turn,
                    vec![TaskStateOperation::Upsert { entity: target }],
                ),
            )
            .is_err());
    }

    #[test]
    fn rewind_moves_only_the_leaf_and_excludes_abandoned_branch() {
        let first_turn = turn_id(3);
        let second_turn = turn_id(4);
        let branch_turn = turn_id(5);
        let mut state = SessionTaskState::default();
        state
            .append_event(
                None,
                semantic_event(
                    0,
                    "req-1",
                    &first_turn,
                    vec![TaskStateOperation::Upsert {
                        entity: goal("shared", &first_turn, "shared goal"),
                    }],
                ),
            )
            .unwrap();
        let first_leaf = state.leaf_id.clone();
        state
            .append_event(
                first_leaf.as_deref(),
                semantic_event(
                    1,
                    "req-2",
                    &second_turn,
                    vec![TaskStateOperation::Upsert {
                        entity: goal("discarded", &second_turn, "linearize old branch"),
                    }],
                ),
            )
            .unwrap();
        let abandoned_event_count = state.events.len();
        state.move_leaf_to_client_turn(Some(&first_turn)).unwrap();
        assert!(state
            .projection
            .goals
            .iter()
            .all(|fact| fact.id != "discarded"));
        let rewound_leaf = state.leaf_id.clone();
        state
            .append_event(
                rewound_leaf.as_deref(),
                semantic_event(
                    state.projection.revision,
                    "req-3",
                    &branch_turn,
                    vec![TaskStateOperation::Upsert {
                        entity: goal("replacement", &branch_turn, "new branch"),
                    }],
                ),
            )
            .unwrap();
        assert_eq!(state.events.len(), abandoned_event_count + 1);
        assert!(state
            .projection
            .goals
            .iter()
            .any(|fact| fact.id == "replacement"));
        assert!(state
            .projection
            .goals
            .iter()
            .all(|fact| fact.id != "discarded"));
    }

    #[test]
    fn legacy_rewind_without_anchor_fails_closed_to_empty_projection() {
        let turn = turn_id(6);
        let mut state = SessionTaskState::default();
        state
            .append_event(
                None,
                semantic_event(
                    0,
                    "req-1",
                    &turn,
                    vec![TaskStateOperation::Upsert {
                        entity: goal("goal", &turn, "goal"),
                    }],
                ),
            )
            .unwrap();
        state.move_leaf_to_client_turn(None).unwrap();
        assert!(state.leaf_id.is_none());
        assert_eq!(state.projection, ActiveTaskProjection::default());
        assert_eq!(state.events.len(), 1);
    }

    #[test]
    fn detached_promotion_audit_does_not_change_current_projection() {
        let first_turn = turn_id(7);
        let second_turn = turn_id(8);
        let mut state = SessionTaskState::default();
        state
            .append_event(
                None,
                semantic_event(
                    0,
                    "req-1",
                    &first_turn,
                    vec![TaskStateOperation::Upsert {
                        entity: goal("old", &first_turn, "old branch"),
                    }],
                ),
            )
            .unwrap();
        let detached_source = state.leaf_id.clone().unwrap();
        state.move_leaf_to_client_turn(None).unwrap();
        state
            .append_event(
                None,
                semantic_event(
                    0,
                    "req-2",
                    &second_turn,
                    vec![TaskStateOperation::Upsert {
                        entity: goal("current", &second_turn, "current branch"),
                    }],
                ),
            )
            .unwrap();
        assert!(!state.is_current_ancestor(&detached_source));
        state.promotion_audits.push(TaskStatePromotionAudit {
            harness_job_id: "job-1".to_string(),
            source_event_id: detached_source,
            fact_ids: vec!["old".to_string()],
            accepted: true,
            document_refs: Vec::new(),
            memory_refs: Vec::new(),
            timestamp: 1,
        });
        assert_eq!(state.projection.goals[0].id, "current");
        assert_eq!(state.projection.goals[0].status, FactStatus::Active);
    }

    #[test]
    fn provenance_confines_artifacts_and_checks_exact_hash_and_quote() {
        let root = std::env::temp_dir().join(format!("eud-task-state-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("specs")).unwrap();
        fs::write(
            root.join("specs/enemy.md"),
            "Ten enemies are authoritative.",
        )
        .unwrap();
        let hash = sha256_bytes(b"Ten enemies are authoritative.");
        let turn = turn_id(9);
        let delta = TaskStateDelta {
            base_revision: 0,
            operations: vec![TaskStateOperation::Upsert {
                entity: TaskStateEntity::AuthoritativeArtifact {
                    artifact: ArtifactRef {
                        id: "enemy-spec".to_string(),
                        path: "specs/enemy.md".to_string(),
                        role: ArtifactRole::Spec,
                        sha256: hash.clone(),
                        status: ArtifactStatus::Draft,
                        provenance: vec![Provenance::ProjectArtifact {
                            path: "specs/enemy.md".to_string(),
                            sha256: hash,
                            exact_quote: "Ten enemies".to_string(),
                        }],
                    },
                },
            }],
        };
        let accepted = HashSet::new();
        let context = ProvenanceValidationContext {
            client_turn_id: &turn,
            user_text: "use the enemy spec",
            request_id: "req-1",
            approved_plan: None,
            workspace_root: Some(&root),
            accepted_journal_entry_ids: &accepted,
        };
        assert!(
            validate_compiler_delta(&ActiveTaskProjection::default(), &delta, &context).is_ok()
        );
        let mut escaped = delta;
        if let TaskStateOperation::Upsert {
            entity: TaskStateEntity::AuthoritativeArtifact { artifact },
        } = &mut escaped.operations[0]
        {
            artifact.path = "../enemy.md".to_string();
        }
        assert!(
            validate_compiler_delta(&ActiveTaskProjection::default(), &escaped, &context).is_err()
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ten_enemy_smoke_preserves_roster_authority_rewind_and_promotion_boundary() {
        let root =
            std::env::temp_dir().join(format!("eud-task-state-smoke-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("specs")).unwrap();
        let artifact_body = "All ten enemies use the agreed roster.";
        fs::write(root.join("specs/enemy.md"), artifact_body).unwrap();
        let artifact_hash = sha256_bytes(artifact_body.as_bytes());
        let roster_turn = turn_id(10);
        let linear_turn = turn_id(11);
        let members = [
            "hunger-tracker",
            "lurker",
            "charger",
            "spitter",
            "guardian",
            "stalker",
            "burrower",
            "swarm-host",
            "brute",
            "queen",
        ]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
        let roster_delta = TaskStateDelta {
            base_revision: 0,
            operations: vec![
                TaskStateOperation::Upsert {
                    entity: TaskStateEntity::TargetSet {
                        target_set: TargetSet {
                            id: "enemy-roster".to_string(),
                            status: FactStatus::Active,
                            name: "All enemies".to_string(),
                            expected_count: Some(10),
                            members: members.clone(),
                            provenance: user_provenance(&roster_turn, "all ten enemies"),
                        },
                    },
                },
                TaskStateOperation::Upsert {
                    entity: TaskStateEntity::AuthoritativeArtifact {
                        artifact: ArtifactRef {
                            id: "enemy-spec".to_string(),
                            path: "specs/enemy.md".to_string(),
                            role: ArtifactRole::Spec,
                            sha256: artifact_hash.clone(),
                            status: ArtifactStatus::Draft,
                            provenance: vec![Provenance::ProjectArtifact {
                                path: "specs/enemy.md".to_string(),
                                sha256: artifact_hash.clone(),
                                exact_quote: "All ten enemies".to_string(),
                            }],
                        },
                    },
                },
            ],
        };
        let accepted_journal_entry_ids = HashSet::new();
        let validation = ProvenanceValidationContext {
            client_turn_id: &roster_turn,
            user_text: "all ten enemies",
            request_id: "req-roster",
            approved_plan: None,
            workspace_root: Some(&root),
            accepted_journal_entry_ids: &accepted_journal_entry_ids,
        };
        validate_compiler_delta(&ActiveTaskProjection::default(), &roster_delta, &validation)
            .unwrap();

        let mut state = SessionTaskState::default();
        state
            .append_event(
                None,
                semantic_event(
                    0,
                    "req-roster",
                    &roster_turn,
                    roster_delta.operations.clone(),
                ),
            )
            .unwrap();
        let roster_leaf = state.leaf_id.clone();
        let snapshot = state.render_full(1).unwrap();
        assert!(snapshot.contains("\"expectedCount\":10"));
        for member in &members {
            assert!(snapshot.contains(member));
        }
        assert!(snapshot.contains("specs/enemy.md"));
        assert!(snapshot.contains(&artifact_hash));

        state
            .append_event(
                roster_leaf.as_deref(),
                semantic_event(
                    1,
                    "req-linear",
                    &linear_turn,
                    vec![TaskStateOperation::Upsert {
                        entity: TaskStateEntity::Constraint {
                            fact: StateFact {
                                id: "linear-scaling".to_string(),
                                status: FactStatus::Active,
                                text: "Apply linear scaling like the hunger tracker".to_string(),
                                provenance: user_provenance(
                                    &linear_turn,
                                    "linear scaling like the hunger tracker",
                                ),
                            },
                        },
                    }],
                ),
            )
            .unwrap();
        assert!(state
            .projection
            .constraints
            .iter()
            .any(|fact| fact.id == "linear-scaling"));
        state.move_leaf_to_client_turn(Some(&roster_turn)).unwrap();
        assert!(state.projection.constraints.is_empty());
        assert_eq!(state.events.len(), 2);
        assert_eq!(state.projection.target_sets[0].members, members);

        let accepted_leaf = state.leaf_id.clone();
        state
            .append_event(
                accepted_leaf.as_deref(),
                TaskStateEvent::new(
                    Some(roster_turn.clone()),
                    Some("req-roster".to_string()),
                    TaskStateEventKind::RequestAccepted {
                        journal_entry_ids: vec!["journal-1".to_string()],
                        harness_job_id: Some("harness-1".to_string()),
                    },
                ),
            )
            .unwrap();
        assert_eq!(state.projection.target_sets[0].status, FactStatus::Accepted);
        assert_eq!(
            state.projection.authoritative_artifacts[0].status,
            ArtifactStatus::Accepted
        );
        let promotion = state
            .promotion_input_for_request("req-roster")
            .expect("accepted roster has bounded promotion input");
        assert_eq!(promotion.fact_ids.len(), 2);

        let promotion_leaf = state.leaf_id.clone();
        state
            .append_event(
                promotion_leaf.as_deref(),
                TaskStateEvent::new(
                    None,
                    None,
                    TaskStateEventKind::PromotionAccepted {
                        harness_job_id: "harness-1".to_string(),
                        fact_ids: promotion.fact_ids,
                        document_refs: vec![PromotedRef {
                            path: "specs/enemy.md".to_string(),
                            sha256: artifact_hash,
                        }],
                        memory_refs: Vec::new(),
                    },
                ),
            )
            .unwrap();
        assert_eq!(state.projection.target_sets[0].status, FactStatus::Promoted);
        assert!(state.projection.target_sets[0]
            .provenance
            .iter()
            .any(|provenance| matches!(
                provenance,
                Provenance::HarnessPromotion { job_id, .. } if job_id == "harness-1"
            )));
        let _ = fs::remove_dir_all(root);
    }
}
