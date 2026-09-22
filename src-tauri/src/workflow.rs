//! Staged agent workflow: durable per-request stage state, the delegated-run
//! contracts of each stage (tool profile, output schema, round budget), and
//! the deterministic renderers that turn stage results into workspace files.
//!
//! The engine owns the transitions (see `engine.rs`); this module owns the
//! data. Every stage result is whole-schema validated by the delegated run
//! before it reaches these types, so renderers assume schema-shaped JSON and
//! degrade to "(none)" only for optional fields.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;

use crate::provider_runtime::{DelegatedRunKind, RunPolicy};

pub const WORKFLOW_SCHEMA_VERSION: u32 = 1;
/// Clarify rounds before triage gives up and answers with what is missing.
pub const MAX_CLARIFY_ROUNDS: u8 = 2;
/// Planner revisions driven by the critic in the default depth.
pub const MAX_CRITIQUE_ROUNDS_DEFAULT: u8 = 1;
/// Planner → architect → critic iterations in deep planning.
pub const MAX_CRITIQUE_ROUNDS_DEEP: u8 = 3;
/// Executing turns re-entered by a failing verification.
pub const MAX_VERIFY_ATTEMPTS: u8 = 2;
/// Every stage job shares the harness deadline class.
pub const STAGE_DEADLINE: Duration = Duration::from_secs(300);
pub const STAGE_MAX_OUTPUT_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowStage {
    Triage,
    Clarify,
    Research,
    Planning,
    Critique,
    PlanReview,
    Executing,
    Verifying,
    ChangesetReview,
    /// A stage job was in flight at shutdown; explicit resume/restart only.
    Interrupted,
    /// The user cancelled the request during a stage; artifacts are retained.
    Cancelled,
    Done,
    Failed,
}

impl WorkflowStage {
    /// Stages whose in-flight work is lost at shutdown.
    pub fn is_in_flight_job(self) -> bool {
        matches!(
            self,
            Self::Triage
                | Self::Clarify
                | Self::Research
                | Self::Planning
                | Self::Critique
                | Self::Verifying
        )
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowRoute {
    Answer,
    Direct,
    Pipeline,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TriageResult {
    pub route: WorkflowRoute,
    pub goal: String,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    pub rationale: String,
    #[serde(default)]
    pub clarify_rounds: u8,
}

/// A workspace-relative artifact with the hash of its rendered bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRef {
    pub path: String,
    pub sha256: String,
    pub summary: String,
}

/// One review round of a plan revision: the architect verdict only in deep
/// planning, the critic verdict always.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanReviewRecord {
    pub iteration: u8,
    pub revision: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub architect_verdict: Option<String>,
    pub critic_verdict: String,
    pub summary: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanArtifact {
    pub path: String,
    pub revision: u32,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_sha256: Option<String>,
    pub title: String,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub critic_verdict: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub critic_summary: Option<String>,
    pub deep: bool,
    pub iterations: u8,
    /// Every review round in order; empty when the revision was not reviewed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reviews: Vec<PlanReviewRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyResult {
    pub verdict: String,
    pub summary: String,
    pub path: String,
    pub sha256: String,
    #[serde(default)]
    pub unmet: Vec<String>,
}

/// Durable stage state of one user request on an EPS session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowState {
    #[serde(default = "schema_version")]
    pub schema_version: u32,
    pub request_id: String,
    pub client_turn_id: String,
    pub project_revision_at_start: String,
    pub stage: WorkflowStage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interrupted_stage: Option<WorkflowStage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<WorkflowRoute>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triage: Option<TriageResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub research: Option<ArtifactRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<PlanArtifact>,
    #[serde(default)]
    pub critique_rounds: u8,
    #[serde(default)]
    pub verify_attempts: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<VerifyResult>,
    pub deep_planning: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub user_text: String,
    #[serde(default)]
    pub clarifications: Vec<String>,
    pub started_at: u64,
    pub updated_at: u64,
}

const fn schema_version() -> u32 {
    WORKFLOW_SCHEMA_VERSION
}

impl WorkflowState {
    pub fn new(
        request_id: String,
        client_turn_id: String,
        project_revision: String,
        user_text: String,
        deep_planning: bool,
        now: u64,
    ) -> Self {
        Self {
            schema_version: WORKFLOW_SCHEMA_VERSION,
            request_id,
            client_turn_id,
            project_revision_at_start: project_revision,
            stage: WorkflowStage::Triage,
            interrupted_stage: None,
            route: None,
            triage: None,
            research: None,
            plan: None,
            critique_rounds: 0,
            verify_attempts: 0,
            verdict: None,
            deep_planning,
            error: None,
            user_text,
            clarifications: Vec::new(),
            started_at: now,
            updated_at: now,
        }
    }

    pub fn max_critique_rounds(&self) -> u8 {
        if self.deep_planning {
            MAX_CRITIQUE_ROUNDS_DEEP
        } else {
            MAX_CRITIQUE_ROUNDS_DEFAULT
        }
    }

    /// Startup maps an in-flight stage job to `Interrupted`, retaining the
    /// last completed artifact. Plan review and changeset review survive as
    /// they are.
    pub fn interrupt_if_in_flight(&mut self, now: u64) -> bool {
        if self.stage.is_in_flight_job() {
            self.interrupted_stage = Some(self.stage);
            self.stage = WorkflowStage::Interrupted;
            self.updated_at = now;
            true
        } else {
            false
        }
    }
}

/// The panel-facing projection emitted on every transition and on hydrate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowEvent {
    pub request_id: String,
    pub stage: WorkflowStage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interrupted_stage: Option<WorkflowStage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<WorkflowRoute>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub research: Option<ArtifactRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<PlanArtifact>,
    pub critique_rounds: u8,
    pub verify_attempts: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<VerifyResult>,
    pub deep_planning: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl WorkflowEvent {
    pub fn from_state(state: &WorkflowState) -> Self {
        Self {
            request_id: state.request_id.clone(),
            stage: state.stage,
            interrupted_stage: state.interrupted_stage,
            route: state.route,
            goal: state.triage.as_ref().map(|triage| triage.goal.clone()),
            acceptance_criteria: state
                .triage
                .as_ref()
                .map(|triage| triage.acceptance_criteria.clone())
                .unwrap_or_default(),
            research: state.research.clone(),
            plan: state.plan.clone(),
            critique_rounds: state.critique_rounds,
            verify_attempts: state.verify_attempts,
            verdict: state.verdict.clone(),
            deep_planning: state.deep_planning,
            error: state.error.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Stage contracts: tool profile, schema, budget
// ---------------------------------------------------------------------------

const TRIAGE_TOOLS: &[&str] = &["project_status", "list_files", "read_file", "source_search"];
const RESEARCH_TOOLS: &[&str] = &[
    "project_status",
    "list_files",
    "read_file",
    "source_search",
    "search_docs",
    "docs_get",
    "dat_get",
    "xdat_get",
    "tbl_get",
    "req_get",
    "btn_get",
    "settings_get",
    "plugins_list",
    "map_info",
    "map_sound_list",
];
const VERIFIER_TOOLS: &[&str] = &[
    "project_status",
    "list_files",
    "read_file",
    "source_search",
    "search_docs",
    "docs_get",
    "dat_get",
    "xdat_get",
    "tbl_get",
    "req_get",
    "btn_get",
    "settings_get",
    "plugins_list",
    "map_info",
    "map_sound_list",
    "build_run",
    "build_log_read",
];

/// Tool names a stage may execute. Every name is a registered EPS read tool
/// (see `stage_profiles_are_registered_read_tools`).
pub fn stage_tools(kind: DelegatedRunKind) -> &'static [&'static str] {
    match kind {
        DelegatedRunKind::Triage => TRIAGE_TOOLS,
        DelegatedRunKind::Research
        | DelegatedRunKind::Planner
        | DelegatedRunKind::Architect
        | DelegatedRunKind::Critic
        | DelegatedRunKind::Read => RESEARCH_TOOLS,
        DelegatedRunKind::Verifier => VERIFIER_TOOLS,
    }
}

pub fn stage_policy(kind: DelegatedRunKind) -> RunPolicy {
    let max_tool_rounds = match kind {
        DelegatedRunKind::Triage => 8,
        DelegatedRunKind::Research => 40,
        DelegatedRunKind::Planner => 24,
        DelegatedRunKind::Architect | DelegatedRunKind::Critic | DelegatedRunKind::Read => 16,
        DelegatedRunKind::Verifier => 24,
    };
    RunPolicy {
        active_deadline: Some(STAGE_DEADLINE),
        shutdown_grace: Duration::from_secs(2),
        max_output_bytes: STAGE_MAX_OUTPUT_BYTES,
        max_output_tokens: None,
        max_tool_rounds,
        allow_resume: false,
    }
}

fn string_array() -> Value {
    json!({ "type": "array", "items": { "type": "string" } })
}

pub fn triage_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "route": { "type": "string", "enum": ["answer", "direct", "pipeline", "clarify"] },
            "goal": { "type": "string" },
            "acceptanceCriteria": string_array(),
            "rationale": { "type": "string" },
            "questions": {
                "type": "array",
                "minItems": 1,
                "maxItems": 4,
                "items": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "minLength": 1, "maxLength": 64 },
                        "question": { "type": "string", "minLength": 1, "maxLength": 1000 },
                        "options": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "label": { "type": "string", "minLength": 1, "maxLength": 200 },
                                    "description": { "type": "string", "maxLength": 1000 }
                                },
                                "required": ["label"],
                                "additionalProperties": false
                            }
                        },
                        "multi": { "type": "boolean" }
                    },
                    "required": ["id", "question"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["route", "goal", "acceptanceCriteria", "rationale"],
        "additionalProperties": false
    })
}

pub fn research_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "summary": { "type": "string" },
            "relevantFiles": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "why": { "type": "string" },
                        "symbols": string_array()
                    },
                    "required": ["path", "why"],
                    "additionalProperties": false
                }
            },
            "datTargets": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "family": { "type": "string" },
                        "dat": { "type": "string" },
                        "objId": { "type": "integer" },
                        "why": { "type": "string" }
                    },
                    "required": ["family", "why"],
                    "additionalProperties": false
                }
            },
            "docEvidence": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "title": { "type": "string" },
                        "url": { "type": "string" },
                        "claim": { "type": "string" }
                    },
                    "required": ["title", "url", "claim"],
                    "additionalProperties": false
                }
            },
            "constraints": string_array(),
            "risks": string_array(),
            "openQuestions": string_array()
        },
        "required": ["summary", "relevantFiles", "docEvidence", "constraints", "risks"],
        "additionalProperties": false
    })
}

pub fn plan_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "title": { "type": "string" },
            "goal": { "type": "string" },
            "acceptanceCriteria": string_array(),
            "steps": {
                "type": "array",
                "minItems": 1,
                "items": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "title": { "type": "string" },
                        "files": string_array(),
                        "change": { "type": "string" },
                        "verification": { "type": "string" },
                        "dependsOn": string_array()
                    },
                    "required": ["id", "title", "files", "change", "verification"],
                    "additionalProperties": false
                }
            },
            "buildRequired": { "type": "boolean" },
            "risks": string_array(),
            "outOfScope": string_array()
        },
        "required": ["title", "goal", "acceptanceCriteria", "steps", "buildRequired", "risks", "outOfScope"],
        "additionalProperties": false
    })
}

pub fn critic_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "verdict": { "type": "string", "enum": ["approve", "revise"] },
            "issues": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "severity": { "type": "string", "enum": ["blocking", "major", "minor"] },
                        "stepId": { "type": "string" },
                        "text": { "type": "string" }
                    },
                    "required": ["severity", "text"],
                    "additionalProperties": false
                }
            },
            "summary": { "type": "string" }
        },
        "required": ["verdict", "issues", "summary"],
        "additionalProperties": false
    })
}

pub fn architect_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "verdict": { "type": "string", "enum": ["approve", "revise"] },
            "structureIssues": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "stepId": { "type": "string" },
                        "text": { "type": "string" }
                    },
                    "required": ["text"],
                    "additionalProperties": false
                }
            },
            "suggestedModules": string_array(),
            "summary": { "type": "string" }
        },
        "required": ["verdict", "structureIssues", "suggestedModules", "summary"],
        "additionalProperties": false
    })
}

pub fn verifier_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "verdict": { "type": "string", "enum": ["pass", "fail"] },
            "criteria": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "text": { "type": "string" },
                        "status": { "type": "string", "enum": ["met", "unmet", "unverifiable"] },
                        "evidence": { "type": "string" }
                    },
                    "required": ["text", "status", "evidence"],
                    "additionalProperties": false
                }
            },
            "stepStatus": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "status": { "type": "string", "enum": ["done", "partial", "missing"] },
                        "note": { "type": "string" }
                    },
                    "required": ["id", "status"],
                    "additionalProperties": false
                }
            },
            "build": {
                "type": "object",
                "properties": {
                    "ok": { "type": "boolean" },
                    "revision": { "type": "string" }
                },
                "required": ["ok"],
                "additionalProperties": false
            },
            "summary": { "type": "string" }
        },
        "required": ["verdict", "criteria", "stepStatus", "build", "summary"],
        "additionalProperties": false
    })
}

/// The fixed `delegate_read` result: a summary plus located findings. It is
/// what the parent receives as its tool result, verbatim.
pub fn read_delegation_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "summary": { "type": "string" },
            "findings": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "line": { "type": "integer" },
                        "excerpt": { "type": "string" },
                        "note": { "type": "string" }
                    },
                    "required": ["note"],
                    "additionalProperties": false
                }
            },
            "openQuestions": string_array(),
            "toolCalls": { "type": "integer" }
        },
        "required": ["summary", "findings", "openQuestions", "toolCalls"],
        "additionalProperties": false
    })
}

pub fn stage_schema(kind: DelegatedRunKind) -> Value {
    match kind {
        DelegatedRunKind::Triage => triage_schema(),
        DelegatedRunKind::Research | DelegatedRunKind::Read => research_schema(),
        DelegatedRunKind::Planner => plan_schema(),
        DelegatedRunKind::Architect => architect_schema(),
        DelegatedRunKind::Critic => critic_schema(),
        DelegatedRunKind::Verifier => verifier_schema(),
    }
}

// ---------------------------------------------------------------------------
// Deterministic renderers
// ---------------------------------------------------------------------------

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn strings(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

fn objects<'a>(value: &'a Value, key: &str) -> Vec<&'a Value> {
    value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.is_object())
        .collect()
}

fn bullet_section(out: &mut String, heading: &str, items: &[String]) {
    out.push_str(&format!("\n## {heading}\n\n"));
    if items.is_empty() {
        out.push_str("(none)\n");
        return;
    }
    for item in items {
        out.push_str(&format!("- {item}\n"));
    }
}

pub fn render_research(request_id: &str, value: &Value) -> String {
    let mut out = format!("# 조사 — {request_id}\n\n{}\n", text(value, "summary"));
    out.push_str("\n## 관련 파일\n\n");
    let files = objects(value, "relevantFiles");
    if files.is_empty() {
        out.push_str("(none)\n");
    }
    for file in files {
        let symbols = strings(file, "symbols");
        out.push_str(&format!(
            "- `{}` — {}",
            text(file, "path"),
            text(file, "why")
        ));
        if !symbols.is_empty() {
            out.push_str(&format!(" (symbols: {})", symbols.join(", ")));
        }
        out.push('\n');
    }
    let dat = objects(value, "datTargets");
    if !dat.is_empty() {
        out.push_str("\n## DAT 대상\n\n");
        for target in dat {
            let obj = target
                .get("objId")
                .and_then(Value::as_u64)
                .map(|id| format!("#{id}"))
                .unwrap_or_default();
            out.push_str(&format!(
                "- {}/{}{} — {}\n",
                text(target, "family"),
                text(target, "dat"),
                obj,
                text(target, "why")
            ));
        }
    }
    out.push_str("\n## 문서 근거\n\n");
    let docs = objects(value, "docEvidence");
    if docs.is_empty() {
        out.push_str("(none — 근거 없음, 일반 EUD 지식)\n");
    }
    for doc in docs {
        out.push_str(&format!(
            "- {} (근거: [{}]({}))\n",
            text(doc, "claim"),
            text(doc, "title"),
            text(doc, "url")
        ));
    }
    bullet_section(&mut out, "제약", &strings(value, "constraints"));
    bullet_section(&mut out, "위험", &strings(value, "risks"));
    bullet_section(&mut out, "열린 질문", &strings(value, "openQuestions"));
    out
}

pub fn render_plan(request_id: &str, revision: u32, value: &Value) -> String {
    let mut out = format!(
        "# {}\n\n요청: `{request_id}` · 개정 {revision}\n\n## 목표\n\n{}\n",
        text(value, "title"),
        text(value, "goal")
    );
    bullet_section(&mut out, "수용 기준", &strings(value, "acceptanceCriteria"));
    out.push_str("\n## 단계\n\n");
    for step in objects(value, "steps") {
        let files = strings(step, "files");
        let depends = strings(step, "dependsOn");
        out.push_str(&format!(
            "### {} — {}\n\n",
            text(step, "id"),
            text(step, "title")
        ));
        if !files.is_empty() {
            out.push_str(&format!(
                "- 파일: {}\n",
                files
                    .iter()
                    .map(|file| format!("`{file}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !depends.is_empty() {
            out.push_str(&format!("- 선행: {}\n", depends.join(", ")));
        }
        out.push_str(&format!("- 변경: {}\n", text(step, "change")));
        out.push_str(&format!("- 검증: {}\n\n", text(step, "verification")));
    }
    let build_required = value
        .get("buildRequired")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    out.push_str(&format!(
        "## 빌드\n\n- build_run 필요: {}\n",
        if build_required { "예" } else { "아니오" }
    ));
    bullet_section(&mut out, "위험", &strings(value, "risks"));
    bullet_section(&mut out, "범위 밖", &strings(value, "outOfScope"));
    out
}

pub fn render_critique(iteration: u8, architect: Option<&Value>, critic: &Value) -> String {
    let mut out = format!("## 계획 검토 — {iteration}회차\n\n");
    if let Some(architect) = architect {
        out.push_str(&format!(
            "### 구조 검토: {}\n\n{}\n",
            text(architect, "verdict"),
            text(architect, "summary")
        ));
        for issue in objects(architect, "structureIssues") {
            let step = text(issue, "stepId");
            out.push_str(&format!(
                "- {}{}\n",
                if step.is_empty() {
                    String::new()
                } else {
                    format!("[{step}] ")
                },
                text(issue, "text")
            ));
        }
        let modules = strings(architect, "suggestedModules");
        if !modules.is_empty() {
            out.push_str(&format!("- 제안 모듈: {}\n", modules.join(", ")));
        }
        out.push('\n');
    }
    out.push_str(&format!(
        "### 비평: {}\n\n{}\n",
        text(critic, "verdict"),
        text(critic, "summary")
    ));
    for issue in objects(critic, "issues") {
        let step = text(issue, "stepId");
        out.push_str(&format!(
            "- ({}) {}{}\n",
            text(issue, "severity"),
            if step.is_empty() {
                String::new()
            } else {
                format!("[{step}] ")
            },
            text(issue, "text")
        ));
    }
    out
}

pub fn render_verdict(request_id: &str, attempt: u8, value: &Value) -> String {
    let mut out = format!(
        "# 검증 — {request_id} ({attempt}차)\n\n판정: **{}**\n\n{}\n",
        text(value, "verdict"),
        text(value, "summary")
    );
    out.push_str("\n## 수용 기준\n\n");
    for criterion in objects(value, "criteria") {
        out.push_str(&format!(
            "- [{}] {} — {}\n",
            text(criterion, "status"),
            text(criterion, "text"),
            text(criterion, "evidence")
        ));
    }
    out.push_str("\n## 단계\n\n");
    for step in objects(value, "stepStatus") {
        let note = text(step, "note");
        out.push_str(&format!(
            "- {} — {}{}\n",
            text(step, "id"),
            text(step, "status"),
            if note.is_empty() {
                String::new()
            } else {
                format!(" ({note})")
            }
        ));
    }
    let build = value.get("build").cloned().unwrap_or(Value::Null);
    out.push_str(&format!(
        "\n## 빌드\n\n- 빌드 성공: {}{}\n",
        build.get("ok").and_then(Value::as_bool).unwrap_or(false),
        build
            .get("revision")
            .and_then(Value::as_str)
            .map(|revision| format!(" (revision {revision})"))
            .unwrap_or_default(),
    ));
    out
}

/// Unmet criteria plus missing/partial steps, for the `[verification]`
/// continuation instruction. Unverifiable criteria are reported in the
/// verdict but cannot be resolved by another executing turn.
pub fn verdict_unmet(value: &Value) -> Vec<String> {
    let mut unmet = objects(value, "criteria")
        .into_iter()
        .filter(|criterion| text(criterion, "status") == "unmet")
        .map(|criterion| {
            format!(
                "{} ({}: {})",
                text(criterion, "text"),
                text(criterion, "status"),
                text(criterion, "evidence")
            )
        })
        .collect::<Vec<_>>();
    unmet.extend(
        objects(value, "stepStatus")
            .into_iter()
            .filter(|step| text(step, "status") != "done")
            .map(|step| {
                let note = text(step, "note");
                format!(
                    "step {} is {}{}",
                    text(step, "id"),
                    text(step, "status"),
                    if note.is_empty() {
                        String::new()
                    } else {
                        format!(": {note}")
                    }
                )
            }),
    );
    unmet
}

// ---------------------------------------------------------------------------
// Stage prompts
// ---------------------------------------------------------------------------

/// Context shared by every stage prompt. `guides` is the static authoring
/// guidance (first principles, eps idioms, epScript guide); `project` is the
/// project map plus memory and wiki sections; both may be empty for tests.
pub struct StageContext<'a> {
    pub guides: &'a str,
    pub project: &'a str,
    pub user_text: &'a str,
    pub clarifications: &'a [String],
}

const SUBMIT_RULE: &str = "Finish by calling `submit_result` exactly once with a value that matches its schema. Write every user-facing text field in Korean. Only the tools offered to this run exist: `build_run` and every write tool are absent unless listed; never call a tool that is not offered; never edit files.";

fn clarification_section(clarifications: &[String]) -> String {
    if clarifications.is_empty() {
        return String::new();
    }
    let mut out = String::from("\n[clarification]\n");
    for (index, answer) in clarifications.iter().enumerate() {
        out.push_str(&format!("round {}: {}\n", index + 1, answer));
    }
    out
}

pub fn triage_prompt(context: &StageContext<'_>, clarify_rounds_left: u8) -> String {
    format!(
        "[role]\nYou triage one user request for the native EUD project agent. Decide how it must be handled; do not do the work.\n\n\
[routes]\n\
- answer: a question or explanation that changes nothing.\n\
- direct: one clearly specified single-site change whose target and value are explicit in the message and the project (rename one thing, change one value, add one line).\n\
- pipeline: any other change: new behavior, multiple files, unclear placement, a bug to diagnose, anything needing design or investigation.\n\
- clarify: the goal, target, or acceptance is materially ambiguous and a short question would change the work. {} clarify round(s) remain; when none remain, choose answer and state in `goal` what is missing.\n\n\
[output]\n\
`goal` restates the request as one verifiable sentence. `acceptanceCriteria` lists what must be true when done (empty for answer). `rationale` explains the route in one or two sentences. For clarify, `questions` holds 1–4 questions with 2–5 options each where a choice exists.\n\
Read the project only as far as the route decision needs (MainFile and the files the request names). {SUBMIT_RULE}\n\n\
{}\n{}\n[user message]\n{}",
        clarify_rounds_left,
        context.project,
        clarification_section(context.clarifications),
        context.user_text
    )
}

pub fn research_prompt(context: &StageContext<'_>, triage: &TriageResult) -> String {
    format!(
        "[role]\nYou research the native EUD project for one approved goal so a separate planner can write an exact plan. Do not propose the plan and do not change anything.\n\n\
[goal]\n{}\n\n[acceptance criteria]\n{}\n\n\
[method]\n\
- Read MainFile and every module the goal touches; list each relevant file with why it matters and the functions/constants involved.\n\
- For DAT-affecting goals, read the exact catalog entries with the dat tools and list them under datTargets.\n\
- Search the documentation (Korean queries) for every unit of work and read the exact chunks; cite each claim with title and url. If nothing relevant exists, leave docEvidence empty rather than inventing a source.\n\
- List constraints from [first principles] that apply, risks, and open questions the planner must decide.\n\
{SUBMIT_RULE}\n\n{}\n\n{}\n{}\n[user message]\n{}",
        triage.goal,
        bullets(&triage.acceptance_criteria),
        context.guides,
        context.project,
        clarification_section(context.clarifications),
        context.user_text
    )
}

pub fn planner_prompt(
    context: &StageContext<'_>,
    triage: &TriageResult,
    research_markdown: &str,
    previous_plan: Option<&str>,
    feedback: Option<&str>,
    critique: Option<&str>,
) -> String {
    let mut out = format!(
        "[role]\nYou write the implementation plan for one goal in the native EUD project. A separate executor will follow it step by step and a separate verifier will judge the result against its acceptance criteria. Do not implement anything.\n\n\
[goal]\n{}\n\n[acceptance criteria from triage]\n{}\n\n\
[plan rules]\n\
- Every step names its files, the exact change, and how that step is verified. Order steps by dependency.\n\
- Keep MainFile as the composition root; place cohesive logic in focused modules; keep imports acyclic; batch mutually dependent edits into one step.\n\
- Set buildRequired to true whenever source, DAT, plugins, or Python change.\n\
- acceptanceCriteria must be checkable from project state or build output.\n\
- Cite documentation from the research; do not invent sources.\n\
{SUBMIT_RULE}\n\n[research]\n{}\n",
        triage.goal,
        bullets(&triage.acceptance_criteria),
        research_markdown
    );
    if let Some(previous) = previous_plan {
        out.push_str(&format!("\n[previous plan]\n{previous}\n"));
    }
    if let Some(feedback) = feedback {
        out.push_str(&format!(
            "\n[user feedback]\nRevise the previous plan to satisfy this feedback:\n{feedback}\n"
        ));
    }
    if let Some(critique) = critique {
        out.push_str(&format!(
            "\n[critique]\nRevise the previous plan to resolve every blocking and major issue:\n{critique}\n"
        ));
    }
    out.push_str(&format!(
        "\n{}\n\n{}\n{}\n[user message]\n{}",
        context.guides,
        context.project,
        clarification_section(context.clarifications),
        context.user_text
    ));
    out
}

pub fn critic_prompt(
    context: &StageContext<'_>,
    triage: &TriageResult,
    research_markdown: &str,
    plan_markdown: &str,
) -> String {
    format!(
        "[role]\nYou review one implementation plan for the native EUD project before the user sees it. Find what would make execution fail or leave the goal unmet. You may read the project to check claims. Do not rewrite the plan.\n\n\
[goal]\n{}\n\n[acceptance criteria]\n{}\n\n\
[check]\n\
- missing steps, wrong or nonexistent files, steps that cannot be verified, acceptance criteria without a verifying step;\n\
- violations of [first principles] and eps idioms;\n\
- unsafe assumptions about locations, players, units, or DAT values that the research did not confirm.\n\
Verdict `approve` only when no blocking or major issue remains. {SUBMIT_RULE}\n\n[plan]\n{}\n\n[research]\n{}\n\n{}\n\n{}",
        triage.goal,
        bullets(&triage.acceptance_criteria),
        plan_markdown,
        research_markdown,
        context.guides,
        context.project
    )
}

pub fn architect_prompt(
    context: &StageContext<'_>,
    triage: &TriageResult,
    research_markdown: &str,
    plan_markdown: &str,
) -> String {
    format!(
        "[role]\nYou review the structure of one implementation plan for the native EUD project: module placement, MainFile composition-root policy, import cycles, lifecycle hooks, and state ownership. You may read the project. Do not rewrite the plan.\n\n\
[goal]\n{}\n\n[acceptance criteria]\n{}\n\n\
Verdict `approve` only when the structure is sound; otherwise list each structure issue with the step it affects and suggest the module split. {SUBMIT_RULE}\n\n[plan]\n{}\n\n[research]\n{}\n\n{}\n\n{}",
        triage.goal,
        bullets(&triage.acceptance_criteria),
        plan_markdown,
        research_markdown,
        context.guides,
        context.project
    )
}

pub struct VerifierInputs<'a> {
    pub plan_markdown: &'a str,
    pub changeset_summary: &'a str,
    pub build_result: &'a str,
    pub executor_answer: &'a str,
    pub revision: &'a str,
}

pub fn verifier_prompt(
    context: &StageContext<'_>,
    triage: &TriageResult,
    inputs: &VerifierInputs<'_>,
    attempt: u8,
) -> String {
    format!(
        "[role]\nYou verify one implemented change in the native EUD project against its approved plan and acceptance criteria. This is attempt {attempt}. Read the changed files, and run `build_run` when the plan requires a build. Do not change anything.\n\n\
[goal]\n{}\n\n[acceptance criteria]\n{}\n\n\
[judgement]\n\
- Mark each criterion met only with concrete evidence from files or build output; unverifiable when no evidence can exist.\n\
- Mark each plan step done, partial, or missing from the changeset and files.\n\
- Verdict `pass` only when every criterion is met, every step is done, and the build succeeded for revision {} when required.\n\
{SUBMIT_RULE}\n\n[approved plan]\n{}\n\n[changeset]\n{}\n\n[last build_run]\n{}\n\n[executor answer]\n{}\n\n{}\n\n{}",
        triage.goal,
        bullets(&triage.acceptance_criteria),
        inputs.revision,
        inputs.plan_markdown,
        inputs.changeset_summary,
        inputs.build_result,
        inputs.executor_answer,
        context.guides,
        context.project
    )
}

fn bullets(items: &[String]) -> String {
    if items.is_empty() {
        return "(none)".to_string();
    }
    items
        .iter()
        .map(|item| format!("- {item}"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn parse_triage(value: &Value) -> Result<(TriageResult, Vec<crate::ipc::AskQuestion>), String> {
    let route = match text(value, "route").as_str() {
        "answer" => Some(WorkflowRoute::Answer),
        "direct" => Some(WorkflowRoute::Direct),
        "pipeline" => Some(WorkflowRoute::Pipeline),
        "clarify" => None,
        other => return Err(format!("triage returned unknown route '{other}'")),
    };
    let questions = objects(value, "questions")
        .into_iter()
        .map(|question| crate::ipc::AskQuestion {
            id: text(question, "id"),
            header: None,
            question: text(question, "question"),
            options: objects(question, "options")
                .into_iter()
                .map(|option| crate::ipc::AskOption {
                    label: text(option, "label"),
                    description: {
                        let description = text(option, "description");
                        (!description.is_empty()).then_some(description)
                    },
                })
                .collect(),
            multi: question
                .get("multi")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
        .collect::<Vec<_>>();
    if route.is_none() && questions.is_empty() {
        return Err("triage asked to clarify without any question".to_string());
    }
    Ok((
        TriageResult {
            // A clarify result carries no route yet; the caller re-triages.
            route: route.unwrap_or(WorkflowRoute::Answer),
            goal: text(value, "goal"),
            acceptance_criteria: strings(value, "acceptanceCriteria"),
            rationale: text(value, "rationale"),
            clarify_rounds: 0,
        },
        if route.is_none() {
            questions
        } else {
            Vec::new()
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_profiles_are_registered_read_tools() {
        for kind in [
            DelegatedRunKind::Triage,
            DelegatedRunKind::Research,
            DelegatedRunKind::Planner,
            DelegatedRunKind::Architect,
            DelegatedRunKind::Critic,
            DelegatedRunKind::Verifier,
            DelegatedRunKind::Read,
        ] {
            crate::provider_tool_loop::DelegatedToolProfile::new(
                stage_tools(kind).iter().copied(),
                &stage_schema(kind),
            )
            .unwrap_or_else(|error| panic!("{kind:?}: {error}"));
        }
    }

    #[test]
    fn stage_schemas_accept_their_minimal_results() {
        let cases = [
            (
                DelegatedRunKind::Triage,
                json!({"route": "pipeline", "goal": "g", "acceptanceCriteria": ["a"], "rationale": "r"}),
            ),
            (
                DelegatedRunKind::Research,
                json!({"summary": "s", "relevantFiles": [], "docEvidence": [], "constraints": [], "risks": []}),
            ),
            (
                DelegatedRunKind::Planner,
                json!({"title": "t", "goal": "g", "acceptanceCriteria": ["a"], "steps": [{"id": "S1", "title": "x", "files": ["src/main.eps"], "change": "c", "verification": "v"}], "buildRequired": true, "risks": [], "outOfScope": []}),
            ),
            (
                DelegatedRunKind::Critic,
                json!({"verdict": "approve", "issues": [], "summary": "ok"}),
            ),
            (
                DelegatedRunKind::Architect,
                json!({"verdict": "revise", "structureIssues": [{"text": "cycle"}], "suggestedModules": [], "summary": "s"}),
            ),
            (
                DelegatedRunKind::Verifier,
                json!({"verdict": "fail", "criteria": [{"text": "a", "status": "unmet", "evidence": "e"}], "stepStatus": [{"id": "S1", "status": "partial"}], "build": {"ok": false}, "summary": "s"}),
            ),
            (
                DelegatedRunKind::Read,
                json!({"summary": "s", "findings": [{"path": "src/main.eps", "line": 3, "excerpt": "x", "note": "n"}, {"note": "unlocated"}], "openQuestions": [], "toolCalls": 2}),
            ),
        ];
        for (kind, value) in cases {
            crate::provider_tool_loop::validate_structured_output(&stage_schema(kind), &value)
                .unwrap_or_else(|error| panic!("{kind:?}: {error}"));
        }
        assert!(crate::provider_tool_loop::validate_structured_output(
            &plan_schema(),
            &json!({"title": "t", "goal": "g", "acceptanceCriteria": [], "steps": [], "buildRequired": true, "risks": [], "outOfScope": []})
        )
        .is_err(), "a plan needs at least one step");
    }

    #[test]
    fn triage_routes_map_placement_requests_direct_to_the_map_handoff() {
        let state = WorkflowState::new(
            "req".into(),
            "turn".into(),
            "rev".into(),
            "전부 공허 지형으로 만들어줘".into(),
            false,
            0,
        );
        let context = StageContext {
            guides: "",
            project: "[project state]",
            user_text: &state.user_text,
            clarifications: &state.clarifications,
        };
        let prompt = triage_prompt(&context, 2);
        assert!(prompt.contains("map placement only"));
        assert!(prompt.contains("map_task_request"));
        assert!(prompt.contains("choose pipeline only when it also needs EPS code"));
    }

    #[test]
    fn read_delegation_is_one_parent_tool_call_over_exploration_reads() {
        // The child settles inside the native MCP call ceiling and never sees
        // build, dependency, ask, plan, or nested delegation tools.
        let policy = stage_policy(DelegatedRunKind::Read);
        assert_eq!(
            policy.active_deadline,
            Some(crate::tools::DELEGATE_READ_TIMEOUT)
        );
        assert!(policy.active_deadline < Some(STAGE_DEADLINE));
        assert!(!policy.allow_resume);
        let tools = stage_tools(DelegatedRunKind::Read);
        for absent in [
            "build_run",
            "python_dependencies_prepare",
            "ask",
            "propose_plan",
            crate::tools::DELEGATE_READ_TOOL,
        ] {
            assert!(
                !tools.contains(&absent),
                "{absent} must stay out of the child"
            );
        }
        for present in [
            "source_search",
            "read_file",
            "docs_get",
            "map_info",
            "map_minimap",
        ] {
            assert!(tools.contains(&present), "{present} is exploration");
        }
        assert!(crate::provider_tool_loop::validate_structured_output(
            &read_delegation_schema(),
            &json!({"summary": "s", "findings": [{"path": "p"}], "openQuestions": [], "toolCalls": 1})
        )
        .is_err(), "every finding carries a note");
        let prompt = read_delegation_prompt("where is P1 hp", &["src/main.eps".to_string()]);
        assert!(prompt.contains("[goal]\nwhere is P1 hp"));
        assert!(prompt.contains("[focus]\n- src/main.eps"));
        assert!(prompt.contains("not mutation evidence"));
        assert!(prompt.contains("submit_result"));
        assert!(read_delegation_prompt("g", &[]).contains("[focus]\n(none)"));
    }

    #[test]
    fn renderers_are_deterministic_and_complete() {
        let research = json!({
            "summary": "요약",
            "relevantFiles": [{"path": "src/kills.eps", "why": "킬 카운트", "symbols": ["killsUpdate"]}],
            "datTargets": [{"family": "units", "dat": "units", "objId": 0, "why": "체력"}],
            "docEvidence": [{"title": "Kills", "url": "https://x", "claim": "Kills 조건"}],
            "constraints": ["c"], "risks": [], "openQuestions": ["q"]
        });
        let first = render_research("req-1", &research);
        assert_eq!(first, render_research("req-1", &research));
        assert!(first.contains("`src/kills.eps` — 킬 카운트 (symbols: killsUpdate)"));
        assert!(first.contains("units/units#0 — 체력"));
        assert!(first.contains("(근거: [Kills](https://x))"));
        assert!(first.contains("## 위험\n\n(none)"));

        let plan = json!({
            "title": "웨이브", "goal": "g", "acceptanceCriteria": ["a1"],
            "steps": [{"id": "S1", "title": "모듈", "files": ["src/wave.eps"], "change": "c", "verification": "v", "dependsOn": []},
                      {"id": "S2", "title": "import", "files": ["src/main.eps"], "change": "c2", "verification": "v2", "dependsOn": ["S1"]}],
            "buildRequired": true,
            "risks": ["r"], "outOfScope": []
        });
        let rendered = render_plan("req-1", 2, &plan);
        assert!(rendered.starts_with("# 웨이브\n\n요청: `req-1` · 개정 2"));
        assert!(rendered.contains("### S2 — import\n\n- 파일: `src/main.eps`\n- 선행: S1\n"));
        assert!(rendered.contains("## 빌드\n\n- build_run 필요: 예\n"));
        assert!(!rendered.contains("테스트"));

        let verdict = json!({
            "verdict": "fail", "summary": "요약",
            "criteria": [{"text": "a1", "status": "met", "evidence": "e"}, {"text": "a2", "status": "unmet", "evidence": "missing"}],
            "stepStatus": [{"id": "S1", "status": "done"}, {"id": "S2", "status": "missing", "note": "no import"}],
            "build": {"ok": true, "revision": "abc"}
        });
        let rendered = render_verdict("req-1", 1, &verdict);
        assert!(rendered.contains("판정: **fail**"));
        assert!(rendered.contains("- [unmet] a2 — missing"));
        assert!(rendered.contains("- 빌드 성공: true (revision abc)"));
        // Unverifiable criteria are never handed to a fix turn.
        let unverifiable = json!({
            "verdict": "fail", "summary": "s",
            "criteria": [{"text": "gameplay", "status": "unverifiable", "evidence": "needs a game"}],
            "stepStatus": [{"id": "S1", "status": "done"}],
            "build": {"ok": true}
        });
        assert!(verdict_unmet(&unverifiable).is_empty());
        assert_eq!(
            verdict_unmet(&verdict),
            ["a2 (unmet: missing)", "step S2 is missing: no import"]
        );
    }

    #[test]
    fn triage_parse_distinguishes_clarify_from_routes() {
        let (triage, questions) = parse_triage(&json!({
            "route": "clarify", "goal": "g", "acceptanceCriteria": [], "rationale": "r",
            "questions": [{"id": "what", "question": "무엇을?", "options": [{"label": "A", "description": "d"}, {"label": "B"}]}]
        }))
        .unwrap();
        assert_eq!(questions.len(), 1);
        assert_eq!(questions[0].options[1].description, None);
        assert_eq!(triage.goal, "g");
        let (triage, questions) = parse_triage(&json!({
            "route": "direct", "goal": "g", "acceptanceCriteria": ["a"], "rationale": "r",
            "questions": [{"id": "ignored", "question": "x"}]
        }))
        .unwrap();
        assert_eq!(triage.route, WorkflowRoute::Direct);
        assert!(questions.is_empty());
        assert!(parse_triage(
            &json!({"route": "clarify", "goal": "", "acceptanceCriteria": [], "rationale": ""})
        )
        .is_err());
        // The triage schema itself bounds questions to what the ASK panel accepts.
        let five = (0..5)
            .map(|index| json!({"id": format!("q{index}"), "question": "?"}))
            .collect::<Vec<_>>();
        assert!(crate::provider_tool_loop::validate_structured_output(
            &triage_schema(),
            &json!({"route": "clarify", "goal": "g", "acceptanceCriteria": [], "rationale": "r", "questions": five})
        )
        .is_err());
        assert!(crate::provider_tool_loop::validate_structured_output(
            &triage_schema(),
            &json!({"route": "clarify", "goal": "g", "acceptanceCriteria": [], "rationale": "r", "questions": [{"id": "", "question": "?"}]})
        )
        .is_err());
    }

    #[test]
    fn interrupt_maps_only_in_flight_jobs() {
        let mut state = WorkflowState::new(
            "req".into(),
            "turn".into(),
            "rev".into(),
            "text".into(),
            false,
            1,
        );
        state.stage = WorkflowStage::PlanReview;
        assert!(!state.interrupt_if_in_flight(2));
        state.stage = WorkflowStage::Research;
        assert!(state.interrupt_if_in_flight(3));
        assert_eq!(state.stage, WorkflowStage::Interrupted);
        assert_eq!(state.interrupted_stage, Some(WorkflowStage::Research));
        assert_eq!(state.updated_at, 3);
        let event = WorkflowEvent::from_state(&state);
        assert_eq!(event.stage, WorkflowStage::Interrupted);
        assert_eq!(
            serde_json::to_value(&event).unwrap()["interruptedStage"],
            json!("research")
        );
    }
}
