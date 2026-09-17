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
