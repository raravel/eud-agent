use super::{BindingSnapshot, RunIdentity};
use serde_json::Value;
use std::{path::PathBuf, time::Duration};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceAccess {
    Read,
    Write,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTurnInput {
    pub text: String,
    pub image_paths: Vec<PathBuf>,
    /// The native project root used as the provider CLI cwd.
    pub workspace_root: Option<PathBuf>,
    /// The session-private scratch directory under the project workspace. It is
    /// the only filesystem location a write-profile turn may create, and it is
    /// exported to the CLI as `TEMP`/`TMP`.
    pub workspace_temp: Option<PathBuf>,
    pub workspace_access: WorkspaceAccess,
    pub output_schema: Option<Value>,
    pub forbid_tools: bool,
}

impl AgentTurnInput {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            image_paths: Vec::new(),
            workspace_root: None,
            workspace_temp: None,
            workspace_access: WorkspaceAccess::Read,
            output_schema: None,
            forbid_tools: false,
        }
    }

    pub fn with_access(mut self, access: WorkspaceAccess) -> Self {
        self.workspace_access = access;
        self
    }

    pub fn with_output_schema(mut self, schema: Value) -> Self {
        self.output_schema = Some(schema);
        self
    }

    pub fn without_tools(mut self) -> Self {
        self.forbid_tools = true;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunPolicy {
    pub active_deadline: Option<Duration>,
    pub shutdown_grace: Duration,
    /// Maximum serialized normalized output admitted by the common runtime.
    /// Provider transports enforce their own independent raw response limits.
    pub max_output_bytes: usize,
    pub max_output_tokens: Option<u64>,
    pub max_tool_rounds: usize,
    pub allow_resume: bool,
}

#[derive(Debug, Clone)]
pub struct ForegroundRequest {
    pub identity: RunIdentity,
    pub binding: BindingSnapshot,
    pub turn: AgentTurnInput,
    pub checkpoint: JobBase,
    pub policy: RunPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructuredJobKind {
    TaskStateCompiler,
    HarnessGenerator,
    ConversationCompaction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobBase {
    pub revision: u64,
    pub instruction_epoch: u64,
    pub branch: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StructuredJobRequest {
    pub identity: RunIdentity,
    pub binding: BindingSnapshot,
    pub kind: StructuredJobKind,
    pub prompt: String,
    pub workspace_root: PathBuf,
    pub output_schema: Value,
    pub base: JobBase,
    pub policy: RunPolicy,
}

#[derive(Debug, Clone)]
pub struct CompactionRequest {
    pub identity: RunIdentity,
    pub binding: BindingSnapshot,
    pub workspace_root: PathBuf,
    pub next_instruction_epoch: u64,
    pub policy: RunPolicy,
}

/// The purpose of one isolated read-only run. Each kind owns a tool profile and
/// a result schema; the executor is shared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelegatedRunKind {
    Triage,
    Research,
    Planner,
    Architect,
    Critic,
    Verifier,
    /// A model-invoked exploration child of a live foreground run.
    Read,
}

/// One isolated model context over the session's read tools that must end by
/// calling `submit_result` with a value matching `output_schema`.
#[derive(Debug, Clone)]
pub struct DelegatedRunRequest {
    pub identity: RunIdentity,
    /// The foreground run that delegated this work, when model-invoked. The
    /// executor does not read it; call sites label events and state with it.
    pub parent_run_id: Option<super::RunId>,
    pub binding: BindingSnapshot,
    /// Labels the run for call sites and persisted state; the executor does
    /// not branch on it. Profiles and budgets are chosen by the caller.
    pub kind: DelegatedRunKind,
    pub prompt: String,
    /// The native project root: the provider CLI cwd and the tool scope.
    pub workspace_root: PathBuf,
    /// The session-private scratch directory exported to native CLIs.
    pub workspace_temp: Option<PathBuf>,
    pub output_schema: Value,
    pub profile: crate::provider_tool_loop::DelegatedToolProfile,
    /// Only the engine-owned verifier sets this: it runs after the executing
    /// turn has settled while the request still holds its write ticket for
    /// pending review. Model-invoked delegation must leave it false.
    pub allow_live_write_ticket: bool,
    pub policy: RunPolicy,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DelegatedRunOutcome {
    Result {
        value: Value,
        /// Every admitted completion of the run, including usage-error
        /// completions and the accepted submission itself.
        completions: usize,
        usage: Option<crate::ipc::ContextUsage>,
    },
    Cancelled,
    Failed(super::ProviderRuntimeError),
}
