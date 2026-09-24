mod budget_policy;
mod cancellation;
mod catalog;
mod continuation;
mod contracts;
mod fixture;
mod ordering;
mod ordering_frames;
mod protocol;
mod schema_contract;
mod size_limits;
mod successful_replies;
mod three_wires;
mod usage;
use super::*;
use std::sync::Arc;

use crate::provider_runtime::{AgentTurnInput, BindingSnapshot, RunId, RunPolicy, WorkspaceAccess};
use crate::session::SessionKind;

fn fixture_request(
    model: &str,
    history: Vec<ConversationItem>,
    cancellation: tokio::sync::watch::Receiver<u64>,
) -> AdapterStepRequest {
    AdapterStepRequest {
        identity: RunIdentity {
            session_id: "fixture-session".to_string(),
            run_id: RunId::new(7),
            request_id: "fixture-request".to_string(),
            session_kind: SessionKind::Eps,
            cancellation_generation: 0,
        },
        binding: BindingSnapshot {
            provider: ProviderId::OpencodeGo,
            model: model.to_string(),
            reasoning: None,
            base_url: None,
            capabilities: None,
            conversation: ProviderConversationState::OpencodeGo {
                transcript_revision: 0,
            },
        },
        kind: AdapterRequestKind::Foreground(AgentTurnInput {
            text: "inspect".to_string(),
            image_paths: Vec::new(),
            workspace_root: None,
            workspace_temp: None,
            workspace_access: WorkspaceAccess::Read,
            output_schema: None,
            forbid_tools: false,
        }),
        policy: RunPolicy {
            active_deadline: None,
            shutdown_grace: std::time::Duration::from_secs(1),
            max_output_bytes: MAX_RESPONSE_BYTES,
            max_output_tokens: Some(1024),
            max_tool_rounds: 64,
            allow_resume: true,
        },
        continuation: None,
        history: Arc::from(history),
        prior_tool_results: Arc::from([]),
        tool_descriptors: Arc::from([
            json!({
                "name":"read_file",
                "description":"read",
                "inputSchema":{
                    "type":"object",
                    "properties":{
                        "path":{"type":"string"},
                        "range":{
                            "type":"object",
                            "properties":{
                                "start":{"type":"integer"},
                                "end":{"type":"integer"}
                            },
                            "required":["start"],
                            "additionalProperties":false
                        }
                    },
                    "required":["path"],
                    "additionalProperties":false
                }
            }),
            json!({
                "name":"search_docs",
                "description":"search",
                "inputSchema":{
                    "type":"object",
                    "properties":{"query":{"type":"string"}},
                    "required":["query"],
                    "additionalProperties":false
                }
            }),
        ]),
        native_mcp_endpoint: None,
        cancellation,
    }
}

fn sse(lines: &[&str]) -> Vec<u8> {
    format!("{}\n\n", lines.join("\n\n")).into_bytes()
}
