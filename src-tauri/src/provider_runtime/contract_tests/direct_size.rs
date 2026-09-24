use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::json;

use crate::{
    antigravity_auth::{AntigravityAuthHandle, AntigravityCredential},
    antigravity_client::AntigravityAdapter,
    ollama::ProductionOllamaAdapter,
    provider::{ProviderConversationState, ProviderId},
    provider_runtime::{
        AdapterEventKind, NormalizedBlock, ProviderAdapter, ProviderRuntime, ProviderRuntimeError,
        RunOutcome, RuntimeEventSink, RuntimeExecutor,
    },
};

use super::fixtures::{sse, HttpFixture, RuntimeFixture};

const POLICY_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
enum ObservedEvent {
    Text(String),
    Finished(bool),
}

#[derive(Default)]
struct EventSink {
    events: Mutex<Vec<ObservedEvent>>,
}

impl RuntimeEventSink for EventSink {
    fn emit(&self, event: &AdapterEventKind) -> Result<(), ProviderRuntimeError> {
        let observed = match event {
            AdapterEventKind::NativeSessionStarted { .. } => None,
            AdapterEventKind::Block(NormalizedBlock::Text { text, .. }) => {
                Some(ObservedEvent::Text(text.clone()))
            }
            AdapterEventKind::ResponseFinished { complete, .. } => {
                Some(ObservedEvent::Finished(*complete))
            }
            AdapterEventKind::ResponseStarted { .. }
            | AdapterEventKind::Block(NormalizedBlock::Reasoning { .. })
            | AdapterEventKind::Block(NormalizedBlock::ToolCall { .. })
            | AdapterEventKind::Block(NormalizedBlock::ToolResult { .. })
            | AdapterEventKind::Usage(_)
            | AdapterEventKind::NativeToolObservation { .. }
            | AdapterEventKind::TransportClosed => None,
        };
        if let Some(observed) = observed {
            self.events.lock().push(observed);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
enum DirectProvider {
    Ollama,
    Antigravity,
}

struct ContractRun {
    outcome: RunOutcome,
    events: Vec<ObservedEvent>,
    tool_ran: bool,
}

async fn run_contract(provider: DirectProvider, stream: String) -> ContractRun {
    let fixture = RuntimeFixture::new("direct-size");
    let (http, binding, adapter): (_, _, Box<dyn ProviderAdapter>) = match provider {
        DirectProvider::Ollama => {
            let http = HttpFixture::scripted([sse(&stream)]);
            let binding = fixture.binding(&http.base_url);
            let adapter = ProductionOllamaAdapter::new(None).expect("construct Ollama adapter");
            (http, binding, Box::new(adapter))
        }
        DirectProvider::Antigravity => {
            let catalog = json_response(
                &json!({
                    "models": {
                        "fixture-model": {
                            "displayName": "Fixture",
                            "supportsImages": true,
                            "supportsThinking": true,
                            "thinkingBudget": 64,
                            "maxOutputTokens": 16_384
                        }
                    }
                })
                .to_string(),
            );
            let http = HttpFixture::scripted([catalog, sse(&stream)]);
            let mut binding = fixture.binding(&http.base_url);
            binding.provider = ProviderId::Antigravity;
            binding.reasoning = None;
            binding.base_url = None;
            binding.conversation = ProviderConversationState::Antigravity {
                transcript_revision: 0,
            };
            let credential = AntigravityCredential {
                access_token: "fixture-token".into(),
                refresh_token: "fixture-refresh".into(),
                expires_at: u64::MAX,
                granted_scopes: Vec::new(),
                project_id: "fixture-project".into(),
            };
            let adapter = AntigravityAdapter::with_transport(
                "fixture-model".into(),
                AntigravityAuthHandle::fixed(credential),
                reqwest::Client::new(),
                http.base_url.clone(),
            )
            .expect("construct Antigravity adapter");
            (http, binding, Box::new(adapter))
        }
    };
    let sink = Arc::new(EventSink::default());
    let mut runtime = ProviderRuntime::new(
        adapter,
        binding.clone(),
        fixture.dirs.clone(),
        fixture.root.clone(),
        fixture.tools.clone(),
        fixture.cancellation.subscribe(),
        sink.clone(),
    )
    .expect("construct provider runtime");
    let outcome = runtime
        .run_foreground(fixture.foreground(binding, 2301))
        .await;
    let tool_ran = fixture.tools.owns_write_registration();
    let events = sink.events.lock().clone();
    http.join();
    eprintln!("{provider:?}: {outcome:?}");
    ContractRun {
        outcome,
        events,
        tool_ran,
    }
}

fn json_response(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn padded_stream(provider: DirectProvider) -> String {
    let ignored = format!(":{}\n\n", " ".repeat(POLICY_BYTES));
    let terminal = match provider {
        DirectProvider::Ollama => concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n"
        ),
        DirectProvider::Antigravity => concat!(
            "data: {\"response\":{\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"ok\"}]},",
            "\"finishReason\":\"STOP\"}]}}\n\n"
        ),
    };
    format!("{ignored}{terminal}")
}

fn normalized_over_limit_stream(provider: DirectProvider) -> String {
    let oversized = "x".repeat(POLICY_BYTES);
    match provider {
        DirectProvider::Ollama => format!(
            concat!(
                "data: {{\"choices\":[{{\"delta\":{{\"content\":\"partial\"}}}}]}}\n\n",
                "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{}\",\"tool_calls\":[",
                "{{\"index\":0,\"id\":\"read-call\",\"function\":{{\"name\":\"list_files\",",
                "\"arguments\":\"{{}}\"}}}}]}},\"finish_reason\":\"tool_calls\"}}]}}\n\n",
                "data: [DONE]\n\n"
            ),
            oversized
        ),
        DirectProvider::Antigravity => format!(
            concat!(
                "data: {{\"response\":{{\"candidates\":[{{\"content\":{{\"parts\":[{{\"text\":\"partial\"}}]}}}}]}}}}\n\n",
                "data: {{\"response\":{{\"candidates\":[{{\"content\":{{\"parts\":[",
                "{{\"text\":\"{}\"}},{{\"functionCall\":{{\"id\":\"read-call\",",
                "\"name\":\"list_files\",\"args\":{{}}}}}}]}} ,",
                "\"finishReason\":\"STOP\"}}]}}}}\n\n"
            ),
            oversized
        ),
    }
}

#[tokio::test]
async fn direct_adapters_complete_when_wire_overhead_exceeds_normalized_output_policy() {
    // Given: each production adapter receives a valid stream over the 64 KiB policy but under its transport ceiling.
    let mut runs = Vec::new();
    for provider in [DirectProvider::Ollama, DirectProvider::Antigravity] {
        let stream = padded_stream(provider);
        assert!(stream.len() > POLICY_BYTES, "{provider:?}");
        assert!(stream.len() < 16 * 1024 * 1024, "{provider:?}");

        // When: the common runtime applies the 64 KiB normalized-output policy.
        runs.push((provider, run_contract(provider, stream).await));
    }

    // Then: raw envelope padding does not reject either tiny normalized answer.
    for (provider, run) in runs {
        assert!(
            matches!(&run.outcome, RunOutcome::Completed { text, .. } if text == "ok"),
            "{provider:?}: {:?}",
            run.outcome
        );
        assert_eq!(
            run.events,
            [
                ObservedEvent::Text("ok".into()),
                ObservedEvent::Finished(true)
            ],
            "{provider:?}"
        );
        assert!(!run.tool_ran, "{provider:?}");
    }
}

#[tokio::test]
async fn direct_adapters_fail_normalized_output_over_policy_without_success_or_tools() {
    // Given: each production adapter emits a partial followed by normalized text over the 64 KiB policy and a tool request.
    let mut runs = Vec::new();
    for provider in [DirectProvider::Ollama, DirectProvider::Antigravity] {
        let stream = normalized_over_limit_stream(provider);

        // When: the common runtime accounts for normalized blocks.
        runs.push((provider, run_contract(provider, stream).await));
    }

    // Then: both partials remain visible, while completion and the later tools are rejected.
    for (provider, run) in runs {
        assert_eq!(
            run.outcome,
            RunOutcome::Failed(ProviderRuntimeError::Protocol(
                "provider output exceeded its byte limit".into()
            )),
            "{provider:?}"
        );
        assert_eq!(
            run.events,
            [ObservedEvent::Text("partial".into())],
            "{provider:?}"
        );
        assert!(!run.tool_ran, "{provider:?}");
    }
}
