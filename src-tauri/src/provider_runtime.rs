use crate::provider::ProviderConversationState;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;
use thiserror::Error;

mod adapter;
mod compiler_workspace;
mod conversation;
mod factory;
mod identity;
mod requests;
mod runtime;

#[cfg(test)]
mod contract_tests;

pub use adapter::{
    AdapterEvent, AdapterEventKind, AdapterFuture, AdapterLoopKind, AdapterOutput,
    AdapterRequestKind, AdapterStepOutcome, AdapterStepRequest, ProviderAdapter,
};
pub use compiler_workspace::CompilerInputWorkspace;
pub use conversation::{
    ConversationImage, ConversationItem, NormalizedBlock, NormalizedUsage, ProviderContinuation,
};
pub(crate) use factory::production_adapter;
pub use identity::{BindingSnapshot, RunId, RunIdentity};
pub use requests::{
    AgentTurnInput, CompactionRequest,
    ForegroundRequest, JobBase, RunPolicy, StructuredJobKind, StructuredJobRequest,
    WorkspaceAccess,
};
pub use runtime::{ProviderRuntime, RuntimeEventSink, StructuredJobExecutor};
pub const TASK_STATE_COMPILER_DEADLINE: Duration = Duration::from_secs(60);
pub const TASK_STATE_COMPILER_OUTPUT_TOKENS: u64 = 8_192;
pub const HARNESS_DEADLINE: Duration = Duration::from_secs(300);
pub const DEFAULT_MAX_TOOL_ROUNDS: usize = 64;
pub const MAX_PROVIDER_CONTINUATION_BYTES: usize = 64 * 1024;

/// Typed reason why a foreground request stopped at a resumable iteration boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IterationBoundaryReason {
    ToolActions,
    ToolRounds,
    ContextPressure,
    ProviderContinuation,
}
#[derive(Debug, Clone, PartialEq)]
pub enum RunOutcome {
    Completed {
        text: String,
        conversation: ProviderConversationState,
    },
    Structured {
        value: Value,
        base: JobBase,
    },
    IterationBoundary {
        reason: IterationBoundaryReason,
        conversation: ProviderConversationState,
    },
    Cancelled,
    WriteTransition,
    Failed(ProviderRuntimeError),
}

pub trait RuntimeExecutor: Send {
    fn run_foreground(&mut self, request: ForegroundRequest) -> AdapterFuture<'_, RunOutcome>;
    fn run_structured(&mut self, request: StructuredJobRequest) -> AdapterFuture<'_, RunOutcome>;
    fn reset(&mut self) -> AdapterFuture<'_, Result<(), ProviderRuntimeError>>;
    fn compact(
        &mut self,
        request: CompactionRequest,
    ) -> AdapterFuture<'_, Result<ProviderConversationState, ProviderRuntimeError>>;
    fn seed(
        &mut self,
        state: ProviderConversationState,
    ) -> AdapterFuture<'_, Result<(), ProviderRuntimeError>>;
    fn acknowledge_persisted(&mut self) -> AdapterFuture<'_, Result<(), ProviderRuntimeError>> {
        Box::pin(async { Ok(()) })
    }
    fn conversation_state(&self) -> ProviderConversationState;
    fn current_workspace(&self) -> Option<crate::workspace::PreparedWorkspace>;
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ProviderRuntimeError {
    #[error("invalid provider binding: {0}")]
    InvalidBinding(String),
    #[error("provider continuation belongs to another provider")]
    ContinuationProviderMismatch,
    #[error("provider continuation is invalid")]
    ContinuationInvalid,
    #[error("provider continuation exceeds its size limit")]
    ContinuationTooLarge,
    #[error("provider request was cancelled")]
    Cancelled,
    #[error("provider request timed out")]
    TimedOut,
    #[error("provider transport failed: {0}")]
    Transport(String),
    #[error("provider protocol failed: {0}")]
    Protocol(String),
    #[error("provider structured output is invalid")]
    StructuredOutputInvalid,
    #[error("provider iteration boundary was not resumable")]
    IterationBoundaryNotResumable,
    #[error("provider event belongs to another run")]
    StaleEvent,
}
