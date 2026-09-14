use super::{
    AgentTurnInput, BindingSnapshot, ConversationItem, NormalizedBlock, NormalizedUsage,
    ProviderContinuation, ProviderRuntimeError, RunIdentity, RunPolicy, StructuredJobKind,
};
use crate::{
    provider::{ProviderConversationState, ProviderId},
    provider_tool_loop::{DirectToolCall, DirectToolResult},
};
use serde_json::Value;
use std::{future::Future, path::PathBuf, pin::Pin, sync::Arc};

#[derive(Debug)]
pub enum AdapterEventKind {
    ResponseStarted {
        response_id: String,
    },
    Block(NormalizedBlock),
    Usage(NormalizedUsage),
    ResponseFinished {
        response_id: String,
        finish_reason: Option<String>,
        complete: bool,
    },
    NativeToolObservation {
        call_id: Option<String>,
        mcp_server: Option<String>,
        name: String,
        arguments: Option<Value>,
        result: Option<Value>,
        status: Option<String>,
    },
    TransportClosed,
}

#[derive(Debug)]
pub struct AdapterEvent {
    pub identity: RunIdentity,
    pub kind: AdapterEventKind,
}

#[derive(Debug, Clone)]
pub enum AdapterRequestKind {
    Foreground(AgentTurnInput),
    Structured {
        kind: StructuredJobKind,
        prompt: String,
        workspace_root: PathBuf,
        output_schema: Value,
    },
}

#[derive(Debug, Clone)]
pub struct AdapterStepRequest {
    pub identity: RunIdentity,
    pub binding: BindingSnapshot,
    pub kind: AdapterRequestKind,
    pub policy: RunPolicy,
    pub continuation: Option<ProviderContinuation>,
    pub history: Arc<[ConversationItem]>,
    pub prior_tool_results: Arc<[DirectToolResult]>,
    pub tool_descriptors: Arc<[Value]>,
    pub native_mcp_endpoint: Option<String>,
    pub cancellation: tokio::sync::watch::Receiver<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterLoopKind {
    DirectSteps,
    NativeSession,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AdapterOutput {
    Text(String),
    Structured(Value),
}

#[derive(Debug, Clone, PartialEq)]
pub enum AdapterStepOutcome {
    Completed {
        output: AdapterOutput,
        continuation: Option<ProviderContinuation>,
        native_conversation: Option<ProviderConversationState>,
    },
    NeedsTools {
        calls: Vec<DirectToolCall>,
        continuation: Option<ProviderContinuation>,
    },
    Cancelled,
}

pub type AdapterFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait ProviderAdapter: Send + Sync {
    fn provider(&self) -> ProviderId;
    fn loop_kind(&self) -> AdapterLoopKind;
    fn run_step<'a>(
        &'a mut self,
        request: AdapterStepRequest,
        events: tokio::sync::mpsc::Sender<AdapterEvent>,
    ) -> AdapterFuture<'a, Result<AdapterStepOutcome, ProviderRuntimeError>>;
    fn interrupt(
        &mut self,
        identity: &RunIdentity,
    ) -> AdapterFuture<'_, Result<(), ProviderRuntimeError>>;
    fn reset(&mut self) -> AdapterFuture<'_, Result<(), ProviderRuntimeError>>;
    fn prepare_compaction<'a>(
        &'a mut self,
        _request: &'a super::CompactionRequest,
    ) -> AdapterFuture<'a, Result<(), ProviderRuntimeError>> {
        Box::pin(async { Ok(()) })
    }
    fn compact<'a>(
        &'a mut self,
        identity: RunIdentity,
        cancellation: tokio::sync::watch::Receiver<u64>,
        events: tokio::sync::mpsc::Sender<AdapterEvent>,
    ) -> AdapterFuture<'a, Result<ProviderConversationState, ProviderRuntimeError>>;
    fn seed(
        &mut self,
        state: ProviderConversationState,
    ) -> AdapterFuture<'_, Result<(), ProviderRuntimeError>>;
}
