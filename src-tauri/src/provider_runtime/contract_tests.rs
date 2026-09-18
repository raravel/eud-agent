mod claude_size;
mod codex_live;
mod concurrency;
mod delegated_runs;
mod direct_size;
mod direct_write_transition;
mod fixtures;
mod harness_retry;
mod live;
mod live_smoke;
mod native_ask;
mod native_mcp;
mod native_pending_mutation;
mod native_recovery;
mod native_usage;
mod opencode_delta_coalescing;
mod opencode_live;
mod opencode_schema;
mod structured_failures;

use std::{future::pending, sync::Arc, time::Duration};

use serde_json::json;

use crate::{
    ollama::ProductionOllamaAdapter,
    provider::{ProviderConversationState, ProviderId},
    provider_runtime::{
        AdapterEvent, AdapterEventKind, AdapterFuture, AdapterLoopKind, AdapterOutput,
        AdapterRequestKind, AdapterStepOutcome, AdapterStepRequest, CompactionRequest,
        NormalizedBlock, ProviderAdapter, ProviderRuntime, ProviderRuntimeError, RunOutcome,
        RuntimeExecutor,
    },
    provider_transcript::{ProviderTranscriptStore, TranscriptBlock, TranscriptBranch},
};

use fixtures::{request_body, sse, EventSummary, HttpFixture, RuntimeFixture};

fn adapter_request(
    fixture: &RuntimeFixture,
    binding: crate::provider_runtime::BindingSnapshot,
    run: u64,
    deadline: Option<Duration>,
) -> AdapterStepRequest {
    AdapterStepRequest {
        identity: fixture.identity(run),
        binding,
        kind: AdapterRequestKind::Foreground(crate::provider_runtime::AgentTurnInput::text(
            "runtime contract",
        )),
        policy: fixtures::policy(deadline),
        continuation: None,
        history: Arc::from([]),
        prior_tool_results: Arc::from([]),
        tool_descriptors: Arc::from([]),
        native_mcp_endpoint: None,
        cancellation: fixture.cancellation.subscribe(),
    }
}

fn runtime(
    fixture: &RuntimeFixture,
    binding: crate::provider_runtime::BindingSnapshot,
) -> ProviderRuntime {
    ProviderRuntime::new(
        Box::new(ProductionOllamaAdapter::new(None).expect("construct Ollama adapter")),
        binding,
        fixture.dirs.clone(),
        fixture.root.clone(),
        fixture.tools.clone(),
        fixture.cancellation.subscribe(),
        fixture.events.clone(),
    )
    .expect("construct provider runtime")
}

#[tokio::test]
async fn c01_actual_ollama_keeps_reasoning_answer_usage_and_completion_distinct() {
    // Given: the production Ollama codec connected to a controlled SSE endpoint.
    let events = concat!(
        "data: {\"id\":\"response-1\",\"choices\":[{\"delta\":{\"reasoning_content\":\"because\"}}]}\n\n",
        "data: {\"id\":\"response-1\",\"choices\":[{\"delta\":{\"content\":\"answer\"},\"finish_reason\":\"stop\"}],",
        "\"usage\":{\"prompt_tokens\":4,\"completion_tokens\":3,\"total_tokens\":7}}\n\n",
        "data: [DONE]\n\n"
    );
    let http = HttpFixture::scripted([sse(events)]);
    let fixture = RuntimeFixture::new("c01");
    let binding = fixture.binding(&http.base_url);
    let mut adapter = ProductionOllamaAdapter::new(None).expect("construct Ollama adapter");

    // When: the common lifecycle runner drives the actual adapter step.
    let result = ProviderRuntime::run_adapter_step(
        &mut adapter,
        adapter_request(&fixture, binding, 1, None),
        None,
        Some(fixture.events.as_ref()),
        fixture.tools.subscribe_ask_waiting(),
    )
    .await
    .expect("complete actual adapter step");

    // Then: reasoning remains a reasoning block and completion/usage are attributed once.
    assert!(matches!(
        result.outcome,
        AdapterStepOutcome::Completed {
            output: AdapterOutput::Text(ref text),
            ..
        } if text == "answer"
    ));
    assert_eq!(
        fixture.events.snapshot(),
        vec![
            EventSummary::Started("ollama-1-0".to_string()),
            EventSummary::Reasoning("because".to_string()),
            EventSummary::Text("answer".to_string()),
            EventSummary::Usage(Some(7)),
            EventSummary::Finished("ollama-1-0".to_string(), true),
        ]
    );
    assert!(matches!(
        &result.blocks[..],
        [NormalizedBlock::Reasoning { text, .. }, NormalizedBlock::Text { text: answer, .. }]
            if text == "because" && answer == "answer"
    ));
    let request = http.requests.recv().expect("captured Ollama request");
    assert!(request.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"));
    let body = request_body(&request);
    assert_eq!(body["model"], "fixture-model");
    assert_eq!(body["reasoning_effort"], "high");
    http.join();
}

#[tokio::test]
async fn c02_actual_ollama_runs_ordered_tools_and_checkpoints_results_before_final_answer() {
    // Given: an actual adapter response containing two ordered tool calls, then a final answer.
    let tools = concat!(
        "data: {\"id\":\"response-tools\",\"choices\":[{\"delta\":{\"reasoning_content\":\"inspect both\",\"tool_calls\":[",
        "{\"index\":0,\"id\":\"call-1\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\":\\\"src/main.eps\\\"}\"}},",
        "{\"index\":1,\"id\":\"call-2\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\":\\\"src/main.eps\\\"}\"}}]} ,\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n"
    );
    let answer = concat!(
        "data: {\"id\":\"response-final\",\"choices\":[{\"delta\":{\"content\":\"both inspected\"},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n"
    );
    let http = HttpFixture::scripted([sse(tools), sse(answer)]);
    let fixture = RuntimeFixture::new("c02");
    let binding = fixture.binding(&http.base_url);
    let mut runtime = runtime(&fixture, binding.clone());

    // When: the common runtime owns the model -> tool -> model loop.
    let outcome = runtime.run_foreground(fixture.foreground(binding, 2)).await;

    // Then: it reaches the final answer and the second wire request carries both results in order.
    assert!(matches!(
        outcome,
        RunOutcome::Completed { ref text, .. } if text == "both inspected"
    ));
    let _first = http.requests.recv().expect("captured tool request");
    let second = request_body(&http.requests.recv().expect("captured continuation request"));
    let messages = second["messages"].as_array().expect("Ollama messages");
    let tool_ids = messages
        .iter()
        .filter(|message| message["role"] == "tool")
        .map(|message| message["tool_call_id"].as_str().expect("tool call id"))
        .collect::<Vec<_>>();
    assert_eq!(tool_ids, ["call-1", "call-2"]);
    assert!(messages.iter().any(|message| {
        message["role"] == "assistant" && message["reasoning_content"] == "inspect both"
    }));

    let store = ProviderTranscriptStore::new(&fixture.dirs);
    let revision = store
        .current_revision(ProviderId::Ollama, &fixture.session_id)
        .expect("read transcript revision");
    let restored = store
        .restore(
            ProviderId::Ollama,
            &fixture.session_id,
            revision,
            &TranscriptBranch {
                instruction_epoch: 3,
                task_leaf_id: Some("leaf-a".to_string()),
            },
        )
        .expect("restore transcript")
        .expect("transcript generation");
    let completed_ids = restored
        .generation
        .checkpoint
        .blocks
        .iter()
        .filter_map(|block| match block {
            TranscriptBlock::ToolResult { id, .. } => Some(id.as_str()),
            TranscriptBlock::User { .. }
            | TranscriptBlock::AssistantText { .. }
            | TranscriptBlock::AssistantReasoning { .. }
            | TranscriptBlock::ToolCall { .. }
            | TranscriptBlock::Compaction { .. } => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(completed_ids, ["call-1", "call-2"]);
    fixtures::assert_path_exists(&fixtures::transcript_root(
        &fixture.dirs,
        &fixture.session_id,
    ));
    http.join();
}

#[tokio::test]
async fn c12_actual_first_mutation_transitions_and_resumes_once_in_write_context() {
    // Given: the production adapter directly requests a mutation during a read turn.
    let transition = concat!(
        "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"transition reasoning\",\"tool_calls\":[",
        "{\"index\":0,\"id\":\"read-call\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\":\\\"src/main.eps\\\"}\"}},",
        "{\"index\":1,\"id\":\"write-call\",\"function\":{\"name\":\"file_create\",\"arguments\":\"{\\\"path\\\":\\\"src/automatic-transition.eps\\\",\\\"ftype\\\":\\\"CUIEps\\\",\\\"code\\\":\\\"const automatic_transition = 1;\\\\n\\\"}\"}},",
        "{\"index\":2,\"id\":\"never-call\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\":\\\"src/main.eps\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n"
    );
    let completed = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"write workspace ready\"},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n"
    );
    let http = HttpFixture::scripted([sse(transition), sse(completed)]);
    let fixture = RuntimeFixture::new("c12");
    let binding = fixture.binding(&http.base_url);
    let mut runtime = runtime(&fixture, binding.clone());

    // When: the runtime parks that mutation, then the same request resumes with write authority.
    let first = runtime
        .run_foreground(fixture.foreground(binding.clone(), 121))
        .await;
    assert_eq!(first, RunOutcome::WriteTransition);
    assert!(fixture.tools.owns_write_registration());
    let transition_state = runtime.conversation_state();
    let transition_revision = match transition_state {
        ProviderConversationState::Ollama {
            transcript_revision,
        } => transcript_revision,
        other => panic!("expected Ollama transition state, got {other:?}"),
    };
    let audit = ProviderTranscriptStore::new(&fixture.dirs)
        .restore(
            ProviderId::Ollama,
            &fixture.session_id,
            transition_revision,
            &TranscriptBranch {
                instruction_epoch: 3,
                task_leaf_id: Some("leaf-a".to_string()),
            },
        )
        .expect("restore transition audit")
        .expect("transition audit generation");
    let audited_results = audit
        .generation
        .checkpoint
        .blocks
        .iter()
        .filter_map(|block| match block {
            TranscriptBlock::ToolResult { id, .. } => Some(id.as_str()),
            TranscriptBlock::User { .. }
            | TranscriptBlock::AssistantText { .. }
            | TranscriptBlock::AssistantReasoning { .. }
            | TranscriptBlock::ToolCall { .. }
            | TranscriptBlock::Compaction { .. } => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(audited_results, ["read-call", "write-call"]);
    let transition_result = audit
        .generation
        .checkpoint
        .blocks
        .iter()
        .find_map(|block| match block {
            TranscriptBlock::ToolResult {
                id,
                result,
                is_error,
                ..
            } if id == "write-call" => Some((result, is_error)),
            _ => None,
        })
        .expect("transition result");
    assert!(*transition_result.1);
    assert!(transition_result
        .0
        .as_str()
        .is_some_and(|message| message.starts_with("WriteWorkspaceTransition:")));
    let audited_calls = audit
        .generation
        .checkpoint
        .blocks
        .iter()
        .filter_map(|block| match block {
            TranscriptBlock::ToolCall { id, .. } => Some(id.as_str()),
            TranscriptBlock::User { .. }
            | TranscriptBlock::AssistantText { .. }
            | TranscriptBlock::AssistantReasoning { .. }
            | TranscriptBlock::ToolResult { .. }
            | TranscriptBlock::Compaction { .. } => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(audited_calls, ["read-call", "write-call", "never-call"]);
    let mut resumed_binding = binding;
    resumed_binding.conversation = ProviderConversationState::Ollama {
        transcript_revision: transition_revision,
    };
    let mut resumed = fixture.foreground(resumed_binding.clone(), 122);
    resumed.binding = resumed_binding;
    resumed.turn.workspace_access = crate::provider_runtime::WorkspaceAccess::Write;
    let outcome = runtime.run_foreground(resumed).await;

    // Then: audit and resumed wire keep only the executed prefix with exact correlation.
    assert!(matches!(
        outcome,
        RunOutcome::Completed { ref text, .. } if text == "write workspace ready"
    ));
    let _ = http.requests.recv().expect("transition request");
    let resumed_wire = request_body(&http.requests.recv().expect("resumed request"));
    let messages = resumed_wire["messages"]
        .as_array()
        .expect("resumed history");
    let resumed_results = messages
        .iter()
        .filter_map(|message| message["tool_call_id"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(resumed_results, ["read-call", "write-call"]);
    let resumed_calls = messages
        .iter()
        .filter_map(|message| message["tool_calls"].as_array())
        .flatten()
        .map(|call| call["id"].as_str().expect("resumed tool-call id"))
        .collect::<Vec<_>>();
    assert_eq!(resumed_calls, ["read-call", "write-call"]);
    assert!(messages
        .iter()
        .any(|message| { message["reasoning_content"] == "transition reasoning" }));
    http.join();
}

#[tokio::test]
async fn c04_actual_ollama_error_after_tools_preserves_durable_completed_results() {
    // Given: two tools complete before the following SSE response reports an error.
    let tools = concat!(
        "data: {\"id\":\"response-tools\",\"choices\":[{\"delta\":{\"tool_calls\":[",
        "{\"index\":0,\"id\":\"call-a\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\":\\\"src/main.eps\\\"}\"}},",
        "{\"index\":1,\"id\":\"call-b\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\":\\\"src/main.eps\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n"
    );
    let failure = concat!(
        "data: {\"id\":\"response-failed\",\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n",
        "data: {\"error\":{\"message\":\"fixture transport failed\"}}\n\n"
    );
    let http = HttpFixture::scripted([sse(tools), sse(failure)]);
    let fixture = RuntimeFixture::new("c04");
    let binding = fixture.binding(&http.base_url);
    let mut runtime = runtime(&fixture, binding.clone());

    // When: the continuation transport fails after both admitted tools finish.
    let outcome = runtime.run_foreground(fixture.foreground(binding, 4)).await;

    // Then: partial text is not success and both tool results remain on disk.
    assert!(matches!(outcome, RunOutcome::Failed(_)));
    let store = ProviderTranscriptStore::new(&fixture.dirs);
    let revision = store
        .current_revision(ProviderId::Ollama, &fixture.session_id)
        .expect("read durable tool revision");
    let restored = store
        .restore(
            ProviderId::Ollama,
            &fixture.session_id,
            revision,
            &TranscriptBranch {
                instruction_epoch: 3,
                task_leaf_id: Some("leaf-a".to_string()),
            },
        )
        .expect("restore durable tool generation")
        .expect("durable tool generation");
    let tool_results = restored
        .generation
        .checkpoint
        .blocks
        .iter()
        .filter(|block| matches!(block, TranscriptBlock::ToolResult { .. }))
        .count();
    assert_eq!(tool_results, 2);
    assert!(!restored.generation.checkpoint.blocks.iter().any(|block| {
        matches!(block, TranscriptBlock::AssistantText { text, .. } if text == "partial")
    }));
    let _ = http.requests.recv().expect("captured first request");
    let _ = http.requests.recv().expect("captured failed request");
    http.join();
}

#[tokio::test]
async fn c07_c10_actual_structured_job_isolated_between_two_foreground_turns() {
    // Given: one runtime and three actual Ollama wire responses: foreground, compiler, foreground.
    let first = "data: {\"id\":\"main-1\",\"choices\":[{\"delta\":{\"content\":\"first\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    let structured = "data: {\"id\":\"compiler\",\"choices\":[{\"delta\":{\"content\":\"{\\\"state\\\":\\\"ready\\\"}\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    let second = "data: {\"id\":\"main-2\",\"choices\":[{\"delta\":{\"content\":\"second\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    let http = HttpFixture::scripted([sse(first), sse(structured), sse(second)]);
    let fixture = RuntimeFixture::new("c07-c10");
    let binding = fixture.binding(&http.base_url);
    let mut runtime = runtime(&fixture, binding.clone());

    // When: a compiler job runs between two main turns.
    let first_outcome = runtime
        .run_foreground(fixture.foreground(binding.clone(), 71))
        .await;
    let first_conversation = match first_outcome {
        RunOutcome::Completed { conversation, .. } => conversation,
        other => panic!("expected first foreground completion, got {other:?}"),
    };
    let event_count_before_job = fixture.events.snapshot().len();
    let job = fixture.structured(
        binding.clone(),
        72,
        json!({
            "type": "object",
            "properties": {"state": {"type": "string"}},
            "required": ["state"],
            "additionalProperties": false
        }),
    );
    let job_outcome = runtime.run_structured(job).await;
    assert!(matches!(
        job_outcome,
        RunOutcome::Structured { ref value, ref base }
            if value == &json!({"state":"ready"}) && base.revision == 41
    ));
    assert_eq!(runtime.conversation_state(), first_conversation);
    assert_eq!(fixture.events.snapshot().len(), event_count_before_job);

    fixture
        .tools
        .begin_request("runtime-request-2", "fixture-project")
        .expect("begin second foreground request");
    let mut next_binding = binding;
    next_binding.conversation = first_conversation;
    let mut next = fixture.foreground(next_binding.clone(), 73);
    next.identity.request_id = "runtime-request-2".to_string();
    next.binding = next_binding;
    next.turn.text = "follow up".to_string();
    let second_outcome = runtime.run_foreground(next).await;

    // Then: the compiler response never enters main history, while the first answer does.
    assert!(matches!(
        second_outcome,
        RunOutcome::Completed { ref text, .. } if text == "second"
    ));
    let first_wire = request_body(&http.requests.recv().expect("first foreground wire"));
    let compiler_wire = request_body(&http.requests.recv().expect("compiler wire"));
    let second_wire = request_body(&http.requests.recv().expect("second foreground wire"));
    assert_eq!(first_wire["messages"][0]["content"], "inspect the project");
    assert!(compiler_wire["response_format"].is_object());
    let second_messages = second_wire["messages"].as_array().expect("second history");
    assert!(second_messages
        .iter()
        .any(|message| message["content"] == "first"));
    assert!(!second_messages.iter().any(|message| message["content"]
        .as_str()
        .is_some_and(|text| text.contains("ready"))));
    http.join();
}

#[tokio::test]
async fn c15_actual_ollama_compaction_publishes_new_epoch_and_followup_uses_summary() {
    // Given: a completed direct-provider conversation and an actual structured summary response.
    let first = "data: {\"id\":\"before-compact\",\"choices\":[{\"delta\":{\"content\":\"old answer\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    let compacted = "data: {\"id\":\"compact\",\"choices\":[{\"delta\":{\"content\":\"{\\\"summary\\\":\\\"durable summary\\\"}\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    let followup = "data: {\"id\":\"after-compact\",\"choices\":[{\"delta\":{\"content\":\"continued\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    let http = HttpFixture::scripted([sse(first), sse(compacted), sse(followup)]);
    let fixture = RuntimeFixture::new("c15");
    let binding = fixture.binding(&http.base_url);
    let mut runtime = runtime(&fixture, binding.clone());
    let mut initial = fixture.foreground(binding.clone(), 151);
    initial.checkpoint.branch = None;
    let first_state = match runtime.run_foreground(initial).await {
        RunOutcome::Completed { conversation, .. } => conversation,
        other => panic!("expected initial completion, got {other:?}"),
    };

    // When: compaction advances the instruction epoch, followed by another foreground request.
    let mut compact_binding = binding.clone();
    compact_binding.conversation = first_state;
    let compact_state = runtime
        .compact(CompactionRequest {
            identity: fixture.identity(152),
            binding: compact_binding,
            workspace_root: fixture.root.clone(),
            next_instruction_epoch: 4,
            policy: fixtures::policy(Some(Duration::from_secs(5))),
        })
        .await
        .expect("compact direct transcript");
    let compact_revision = match compact_state {
        ProviderConversationState::Ollama {
            transcript_revision,
        } => transcript_revision,
        other => panic!("expected Ollama compaction state, got {other:?}"),
    };
    let store = ProviderTranscriptStore::new(&fixture.dirs);
    let compacted_generation = store
        .restore(
            ProviderId::Ollama,
            &fixture.session_id,
            compact_revision,
            &TranscriptBranch {
                instruction_epoch: 4,
                task_leaf_id: None,
            },
        )
        .expect("restore compacted epoch before followup")
        .expect("compacted generation");
    assert!(
        matches!(compacted_generation.generation.checkpoint.blocks.as_slice(),
        [TranscriptBlock::Compaction { summary, .. }] if summary == "durable summary")
    );
    fixture
        .tools
        .begin_request("runtime-request-after-compact", "fixture-project")
        .expect("begin post-compaction request");
    let mut followup_binding = binding;
    followup_binding.conversation = ProviderConversationState::Ollama {
        transcript_revision: compact_revision,
    };
    let mut next = fixture.foreground(followup_binding.clone(), 153);
    next.identity.request_id = "runtime-request-after-compact".to_string();
    next.binding = followup_binding;
    next.checkpoint.instruction_epoch = 4;
    next.checkpoint.branch = None;
    next.turn.text = "continue".to_string();
    let outcome = runtime.run_foreground(next).await;

    // Then: only the compacted summary is restored at the new epoch and continuation succeeds.
    assert!(matches!(
        outcome,
        RunOutcome::Completed { ref text, .. } if text == "continued"
    ));
    let _ = http.requests.recv().expect("initial request");
    let _ = http.requests.recv().expect("compaction request");
    let followup_wire = request_body(&http.requests.recv().expect("followup request"));
    let messages = followup_wire["messages"]
        .as_array()
        .expect("post-compaction messages");
    assert!(messages.iter().any(|message| message["content"]
        .as_str()
        .is_some_and(|text| text.contains("durable summary"))));
    assert!(!messages
        .iter()
        .any(|message| message["content"] == "old answer"));
    let restored = store
        .restore(
            ProviderId::Ollama,
            &fixture.session_id,
            compact_revision,
            &TranscriptBranch {
                instruction_epoch: 4,
                task_leaf_id: None,
            },
        )
        .expect("restore compacted epoch")
        .expect("compacted generation");
    assert!(matches!(
        restored.generation.checkpoint.blocks.as_slice(),
        [TranscriptBlock::Compaction { summary, .. }, ..] if summary == "durable summary"
    ));
    assert!(restored.metadata_was_stale);
    assert!(restored.generation.checkpoint.blocks.iter().any(
        |block| matches!(block, TranscriptBlock::AssistantText { text, .. } if text == "continued")
    ));
    http.join();
}

#[tokio::test]
async fn c15_actual_reset_clears_head_preserves_generation_and_starts_new_epoch() {
    // Given: a direct-provider conversation with a durable generation on disk.
    let first = "data: {\"choices\":[{\"delta\":{\"content\":\"before reset\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    let second = "data: {\"choices\":[{\"delta\":{\"content\":\"after reset\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    let http = HttpFixture::scripted([sse(first), sse(second)]);
    let fixture = RuntimeFixture::new("reset");
    let binding = fixture.binding(&http.base_url);
    let mut runtime = runtime(&fixture, binding.clone());
    let mut initial = fixture.foreground(binding.clone(), 161);
    initial.checkpoint.branch = None;
    let first_state = runtime.run_foreground(initial).await;
    assert!(matches!(first_state, RunOutcome::Completed { .. }));
    let store = ProviderTranscriptStore::new(&fixture.dirs);
    let old_revision = store
        .current_revision(ProviderId::Ollama, &fixture.session_id)
        .expect("read pre-reset head");
    let old_generation = fixtures::transcript_root(&fixture.dirs, &fixture.session_id)
        .join("generations")
        .join(format!("{old_revision}.json"));
    fixtures::assert_path_exists(&old_generation);

    // When: runtime reset is followed by a fresh foreground turn at the advanced epoch.
    runtime.reset().await.expect("reset direct runtime");
    assert_eq!(
        store
            .current_revision(ProviderId::Ollama, &fixture.session_id)
            .expect("read cleared head"),
        0
    );
    fixtures::assert_path_exists(&old_generation);
    fixture
        .tools
        .begin_request("runtime-request-after-reset", "fixture-project")
        .expect("begin post-reset request");
    let mut fresh = fixture.foreground(binding, 162);
    fresh.identity.request_id = "runtime-request-after-reset".to_string();
    fresh.checkpoint.instruction_epoch = 4;
    fresh.checkpoint.branch = None;
    fresh.turn.text = "fresh turn".to_string();
    let outcome = runtime.run_foreground(fresh).await;

    // Then: the fresh request omits old content and publishes without overwriting history.
    assert!(matches!(
        outcome,
        RunOutcome::Completed { ref text, .. } if text == "after reset"
    ));
    let _ = http.requests.recv().expect("pre-reset request");
    let fresh_wire = request_body(&http.requests.recv().expect("post-reset request"));
    let messages = fresh_wire["messages"].as_array().expect("fresh messages");
    assert!(!messages
        .iter()
        .any(|message| message["content"] == "before reset"));
    let new_revision = store
        .current_revision(ProviderId::Ollama, &fixture.session_id)
        .expect("read post-reset head");
    assert!(new_revision > old_revision);
    fixtures::assert_path_exists(&old_generation);
    http.join();
}

#[tokio::test]
async fn c14_corrupt_generation_fails_closed_without_rewriting_head() {
    // Given: an actual completed Ollama turn whose immutable generation is later corrupted.
    let response = "data: {\"choices\":[{\"delta\":{\"content\":\"durable answer\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    let http = HttpFixture::scripted([sse(response)]);
    let fixture = RuntimeFixture::new("c14");
    let binding = fixture.binding(&http.base_url);
    let mut runtime = runtime(&fixture, binding.clone());
    let mut initial = fixture.foreground(binding.clone(), 171);
    initial.checkpoint.branch = None;
    let conversation = match runtime.run_foreground(initial).await {
        RunOutcome::Completed { conversation, .. } => conversation,
        other => panic!("expected durable completion, got {other:?}"),
    };
    let revision = match conversation {
        ProviderConversationState::Ollama {
            transcript_revision,
        } => transcript_revision,
        other => panic!("expected Ollama state, got {other:?}"),
    };
    let transcript = fixtures::transcript_root(&fixture.dirs, &fixture.session_id);
    let pointer_path = transcript.join("current.json");
    let pointer_before = std::fs::read(&pointer_path).expect("read transcript pointer");
    std::fs::write(
        transcript
            .join("generations")
            .join(format!("{revision}.json")),
        b"{}",
    )
    .expect("corrupt generation fixture");

    // When: another foreground run attempts to restore that verified head.
    fixture
        .tools
        .begin_request("runtime-request-corrupt", "fixture-project")
        .expect("begin corrupt-head request");
    let mut next_binding = binding;
    next_binding.conversation = ProviderConversationState::Ollama {
        transcript_revision: revision,
    };
    let mut next = fixture.foreground(next_binding.clone(), 172);
    next.identity.request_id = "runtime-request-corrupt".to_string();
    next.binding = next_binding;
    next.checkpoint.branch = None;
    let outcome = runtime.run_foreground(next).await;

    // Then: no empty conversation is substituted and the original head bytes remain untouched.
    assert!(matches!(
        outcome,
        RunOutcome::Failed(ProviderRuntimeError::Protocol(_))
    ));
    assert_eq!(
        std::fs::read(&pointer_path).expect("reread transcript pointer"),
        pointer_before
    );
    let _ = http.requests.recv().expect("initial Ollama request");
    http.join();
}

struct ClosedEventAdapter;

impl ProviderAdapter for ClosedEventAdapter {
    fn provider(&self) -> ProviderId {
        ProviderId::Ollama
    }

    fn loop_kind(&self) -> AdapterLoopKind {
        AdapterLoopKind::DirectSteps
    }

    fn run_step<'a>(
        &'a mut self,
        _request: AdapterStepRequest,
        events: tokio::sync::mpsc::Sender<AdapterEvent>,
    ) -> AdapterFuture<'a, Result<AdapterStepOutcome, ProviderRuntimeError>> {
        Box::pin(async move {
            drop(events);
            tokio::task::yield_now().await;
            Ok(AdapterStepOutcome::Completed {
                output: AdapterOutput::Text("closed cleanly".to_string()),
                continuation: None,
                native_conversation: None,
            })
        })
    }

    fn interrupt(
        &mut self,
        _identity: &crate::provider_runtime::RunIdentity,
    ) -> AdapterFuture<'_, Result<(), ProviderRuntimeError>> {
        Box::pin(async { Ok(()) })
    }

    fn reset(&mut self) -> AdapterFuture<'_, Result<(), ProviderRuntimeError>> {
        Box::pin(async { Ok(()) })
    }

    fn compact<'a>(
        &'a mut self,
        _identity: crate::provider_runtime::RunIdentity,
        _cancellation: tokio::sync::watch::Receiver<u64>,
        _events: tokio::sync::mpsc::Sender<AdapterEvent>,
    ) -> AdapterFuture<'a, Result<ProviderConversationState, ProviderRuntimeError>> {
        Box::pin(async {
            Ok(ProviderConversationState::Ollama {
                transcript_revision: 0,
            })
        })
    }

    fn seed(
        &mut self,
        _state: ProviderConversationState,
    ) -> AdapterFuture<'_, Result<(), ProviderRuntimeError>> {
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn lifecycle_closed_event_channel_with_no_deadline_completes_without_spin_or_overflow() {
    // Given: a transport that closes its event stream before resolving and has no deadline.
    let fixture = RuntimeFixture::new("closed-channel");
    let binding = fixture.binding("http://127.0.0.1:9/v1");
    let mut adapter = ClosedEventAdapter;

    // When: the common runner observes channel closure.
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        ProviderRuntime::run_adapter_step(
            &mut adapter,
            adapter_request(&fixture, binding, 81, None),
            None,
            None,
            fixture.tools.subscribe_ask_waiting(),
        ),
    )
    .await
    .expect("closed event channel must not spin")
    .expect("closed channel is not itself an adapter error");

    // Then: the transport outcome is returned without constructing Duration::MAX.
    assert!(matches!(
        result.outcome,
        AdapterStepOutcome::Completed {
            output: AdapterOutput::Text(ref text),
            ..
        } if text == "closed cleanly"
    ));
}

struct StaleEventAdapter;

impl ProviderAdapter for StaleEventAdapter {
    fn provider(&self) -> ProviderId {
        ProviderId::Ollama
    }

    fn loop_kind(&self) -> AdapterLoopKind {
        AdapterLoopKind::DirectSteps
    }

    fn run_step<'a>(
        &'a mut self,
        request: AdapterStepRequest,
        events: tokio::sync::mpsc::Sender<AdapterEvent>,
    ) -> AdapterFuture<'a, Result<AdapterStepOutcome, ProviderRuntimeError>> {
        Box::pin(async move {
            let mut stale = request.identity.clone();
            stale.run_id = crate::provider_runtime::RunId::new(stale.run_id.get() + 1);
            events
                .send(AdapterEvent {
                    identity: stale,
                    kind: AdapterEventKind::ResponseStarted {
                        response_id: "stale".to_string(),
                    },
                })
                .await
                .map_err(|_| ProviderRuntimeError::Transport("event receiver closed".into()))?;
            for kind in [
                AdapterEventKind::ResponseStarted {
                    response_id: "current".to_string(),
                },
                AdapterEventKind::Block(NormalizedBlock::Text {
                    response_id: "current".to_string(),
                    text: "current answer".to_string(),
                }),
                AdapterEventKind::ResponseFinished {
                    response_id: "current".to_string(),
                    finish_reason: Some("stop".to_string()),
                    complete: true,
                },
            ] {
                events
                    .send(AdapterEvent {
                        identity: request.identity.clone(),
                        kind,
                    })
                    .await
                    .map_err(|_| ProviderRuntimeError::Transport("event receiver closed".into()))?;
            }
            Ok(AdapterStepOutcome::Completed {
                output: AdapterOutput::Text("current answer".to_string()),
                continuation: None,
                native_conversation: None,
            })
        })
    }

    fn interrupt(
        &mut self,
        _identity: &crate::provider_runtime::RunIdentity,
    ) -> AdapterFuture<'_, Result<(), ProviderRuntimeError>> {
        Box::pin(async { Ok(()) })
    }

    fn reset(&mut self) -> AdapterFuture<'_, Result<(), ProviderRuntimeError>> {
        Box::pin(async { Ok(()) })
    }

    fn compact<'a>(
        &'a mut self,
        _identity: crate::provider_runtime::RunIdentity,
        _cancellation: tokio::sync::watch::Receiver<u64>,
        _events: tokio::sync::mpsc::Sender<AdapterEvent>,
    ) -> AdapterFuture<'a, Result<ProviderConversationState, ProviderRuntimeError>> {
        Box::pin(async {
            Ok(ProviderConversationState::Ollama {
                transcript_revision: 0,
            })
        })
    }

    fn seed(
        &mut self,
        _state: ProviderConversationState,
    ) -> AdapterFuture<'_, Result<(), ProviderRuntimeError>> {
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn c05_late_event_from_another_run_is_rejected_before_ui_delivery() {
    // Given: a controlled transport that emits an event stamped with a different run ID.
    let fixture = RuntimeFixture::new("stale-event");
    let binding = fixture.binding("http://127.0.0.1:9/v1");
    let mut adapter = StaleEventAdapter;

    // When: the common lifecycle runner receives the stale event.
    let result = ProviderRuntime::run_adapter_step(
        &mut adapter,
        adapter_request(&fixture, binding, 91, None),
        None,
        Some(fixture.events.as_ref()),
        fixture.tools.subscribe_ask_waiting(),
    )
    .await;

    // Then: only the current run reaches the product sink and completes.
    let result = result.expect("old event cannot terminate the current run");
    assert!(
        matches!(result.outcome, AdapterStepOutcome::Completed { output: AdapterOutput::Text(ref text), .. } if text == "current answer")
    );
    assert_eq!(
        fixture.events.snapshot(),
        vec![
            EventSummary::Started("current".to_string()),
            EventSummary::Text("current answer".to_string()),
            EventSummary::Finished("current".to_string(), true)
        ]
    );
}

struct CancellationAdapter {
    started: Option<tokio::sync::oneshot::Sender<()>>,
    interrupted: Arc<std::sync::atomic::AtomicBool>,
}

struct FutureDropSignal(Arc<std::sync::atomic::AtomicBool>);

impl Drop for FutureDropSignal {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

struct DropAwareAdapter {
    started: Option<tokio::sync::oneshot::Sender<()>>,
    dropped: Arc<std::sync::atomic::AtomicBool>,
}

impl ProviderAdapter for DropAwareAdapter {
    fn provider(&self) -> ProviderId {
        ProviderId::Ollama
    }

    fn loop_kind(&self) -> AdapterLoopKind {
        AdapterLoopKind::DirectSteps
    }

    fn run_step<'a>(
        &'a mut self,
        _request: AdapterStepRequest,
        _events: tokio::sync::mpsc::Sender<AdapterEvent>,
    ) -> AdapterFuture<'a, Result<AdapterStepOutcome, ProviderRuntimeError>> {
        let started = self.started.take();
        let signal = FutureDropSignal(Arc::clone(&self.dropped));
        Box::pin(async move {
            let _signal = signal;
            if let Some(started) = started {
                let _ = started.send(());
            }
            pending::<Result<AdapterStepOutcome, ProviderRuntimeError>>().await
        })
    }

    fn interrupt(
        &mut self,
        _identity: &crate::provider_runtime::RunIdentity,
    ) -> AdapterFuture<'_, Result<(), ProviderRuntimeError>> {
        Box::pin(async { Ok(()) })
    }

    fn reset(&mut self) -> AdapterFuture<'_, Result<(), ProviderRuntimeError>> {
        Box::pin(async { Ok(()) })
    }

    fn compact<'a>(
        &'a mut self,
        _identity: crate::provider_runtime::RunIdentity,
        _cancellation: tokio::sync::watch::Receiver<u64>,
        _events: tokio::sync::mpsc::Sender<AdapterEvent>,
    ) -> AdapterFuture<'a, Result<ProviderConversationState, ProviderRuntimeError>> {
        Box::pin(async {
            Ok(ProviderConversationState::Ollama {
                transcript_revision: 0,
            })
        })
    }

    fn seed(
        &mut self,
        _state: ProviderConversationState,
    ) -> AdapterFuture<'_, Result<(), ProviderRuntimeError>> {
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn c07_forced_foreground_future_drop_releases_transport_and_request_scope() {
    // Given: a foreground adapter future paused after transport start.
    let fixture = RuntimeFixture::new("future-drop");
    let binding = fixture.binding("http://127.0.0.1:9/v1");
    let (started, running) = tokio::sync::oneshot::channel();
    let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let adapter = DropAwareAdapter {
        started: Some(started),
        dropped: Arc::clone(&dropped),
    };
    let mut runtime = ProviderRuntime::new(
        Box::new(adapter),
        binding.clone(),
        fixture.dirs.clone(),
        fixture.root.clone(),
        fixture.tools.clone(),
        fixture.cancellation.subscribe(),
        fixture.events.clone(),
    )
    .expect("construct future-drop runtime");
    let mut execution = Box::pin(runtime.run_foreground(fixture.foreground(binding, 98)));
    tokio::select! {
        result = &mut execution => panic!("adapter unexpectedly completed: {result:?}"),
        result = running => result.expect("adapter entered transport"),
    }

    // When: the caller forcibly drops the in-flight common-runtime future.
    drop(execution);

    // Then: transport ownership is released and a new request scope can open immediately.
    assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));
    fixture
        .tools
        .begin_request("after-forced-drop", "fixture-project")
        .expect("forced drop must not strand request admission");
}

impl ProviderAdapter for CancellationAdapter {
    fn provider(&self) -> ProviderId {
        ProviderId::Ollama
    }

    fn loop_kind(&self) -> AdapterLoopKind {
        AdapterLoopKind::DirectSteps
    }

    fn run_step<'a>(
        &'a mut self,
        _request: AdapterStepRequest,
        _events: tokio::sync::mpsc::Sender<AdapterEvent>,
    ) -> AdapterFuture<'a, Result<AdapterStepOutcome, ProviderRuntimeError>> {
        let started = self.started.take();
        Box::pin(async move {
            if let Some(started) = started {
                let _ = started.send(());
            }
            pending::<Result<AdapterStepOutcome, ProviderRuntimeError>>().await
        })
    }

    fn interrupt(
        &mut self,
        _identity: &crate::provider_runtime::RunIdentity,
    ) -> AdapterFuture<'_, Result<(), ProviderRuntimeError>> {
        self.interrupted
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }

    fn reset(&mut self) -> AdapterFuture<'_, Result<(), ProviderRuntimeError>> {
        Box::pin(async { Ok(()) })
    }

    fn compact<'a>(
        &'a mut self,
        _identity: crate::provider_runtime::RunIdentity,
        _cancellation: tokio::sync::watch::Receiver<u64>,
        _events: tokio::sync::mpsc::Sender<AdapterEvent>,
    ) -> AdapterFuture<'a, Result<ProviderConversationState, ProviderRuntimeError>> {
        Box::pin(async {
            Ok(ProviderConversationState::Ollama {
                transcript_revision: 0,
            })
        })
    }

    fn seed(
        &mut self,
        _state: ProviderConversationState,
    ) -> AdapterFuture<'_, Result<(), ProviderRuntimeError>> {
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn c05_cancellation_interrupts_transport_and_returns_one_cancelled_outcome() {
    // Given: a foreground transport held open after the model request starts.
    let fixture = RuntimeFixture::new("cancel");
    let binding = fixture.binding("http://127.0.0.1:9/v1");
    let (started, running) = tokio::sync::oneshot::channel();
    let interrupted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let adapter = CancellationAdapter {
        started: Some(started),
        interrupted: Arc::clone(&interrupted),
    };
    let mut runtime = ProviderRuntime::new(
        Box::new(adapter),
        binding.clone(),
        fixture.dirs.clone(),
        fixture.root.clone(),
        fixture.tools.clone(),
        fixture.cancellation.subscribe(),
        fixture.events.clone(),
    )
    .expect("construct cancellation runtime");
    let request = fixture.foreground(binding, 95);
    let task = tokio::spawn(async move { runtime.run_foreground(request).await });
    running.await.expect("adapter entered model request");

    // When: the session cancellation generation changes.
    fixture
        .cancellation
        .send(1)
        .expect("publish cancellation generation");
    let outcome = task.await.expect("join cancelled runtime");

    // Then: the runtime reports cancellation once and invokes transport interruption.
    assert_eq!(outcome, RunOutcome::Cancelled);
    assert!(interrupted.load(std::sync::atomic::Ordering::SeqCst));
}

#[tokio::test]
async fn c05_dropped_cancellation_owner_fails_closed_independent_of_adapter_result() {
    // Given: an open adapter future whose cancellation owner disappears before polling.
    let fixture = RuntimeFixture::new("cancel-owner-drop");
    let binding = fixture.binding("http://127.0.0.1:9/v1");
    let interrupted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let adapter = CancellationAdapter {
        started: None,
        interrupted: Arc::clone(&interrupted),
    };
    let (owner, cancellation) = tokio::sync::watch::channel(0);
    fixture.tools.set_cancellation(cancellation.clone());
    let mut runtime = ProviderRuntime::new(
        Box::new(adapter),
        binding.clone(),
        fixture.dirs.clone(),
        fixture.root.clone(),
        fixture.tools.clone(),
        cancellation,
        fixture.events.clone(),
    )
    .expect("construct owner-drop runtime");
    drop(owner);

    // When: the foreground lifecycle observes the closed cancellation channel.
    let outcome = runtime
        .run_foreground(fixture.foreground(binding, 97))
        .await;

    // Then: it fails closed as cancellation and still interrupts the transport.
    assert_eq!(outcome, RunOutcome::Cancelled);
    assert!(interrupted.load(std::sync::atomic::Ordering::SeqCst));
}

struct WaitingAdapter {
    started: Option<tokio::sync::oneshot::Sender<()>>,
    release: Arc<tokio::sync::Notify>,
}

impl ProviderAdapter for WaitingAdapter {
    fn provider(&self) -> ProviderId {
        ProviderId::Ollama
    }

    fn loop_kind(&self) -> AdapterLoopKind {
        AdapterLoopKind::DirectSteps
    }

    fn run_step<'a>(
        &'a mut self,
        _request: AdapterStepRequest,
        _events: tokio::sync::mpsc::Sender<AdapterEvent>,
    ) -> AdapterFuture<'a, Result<AdapterStepOutcome, ProviderRuntimeError>> {
        let started = self.started.take();
        let release = Arc::clone(&self.release);
        Box::pin(async move {
            if let Some(started) = started {
                let _ = started.send(());
            }
            release.notified().await;
            Ok(AdapterStepOutcome::Completed {
                output: AdapterOutput::Text("released".to_string()),
                continuation: None,
                native_conversation: None,
            })
        })
    }

    fn interrupt(
        &mut self,
        _identity: &crate::provider_runtime::RunIdentity,
    ) -> AdapterFuture<'_, Result<(), ProviderRuntimeError>> {
        Box::pin(async { Ok(()) })
    }

    fn reset(&mut self) -> AdapterFuture<'_, Result<(), ProviderRuntimeError>> {
        Box::pin(async { Ok(()) })
    }

    fn compact<'a>(
        &'a mut self,
        _identity: crate::provider_runtime::RunIdentity,
        _cancellation: tokio::sync::watch::Receiver<u64>,
        _events: tokio::sync::mpsc::Sender<AdapterEvent>,
    ) -> AdapterFuture<'a, Result<ProviderConversationState, ProviderRuntimeError>> {
        Box::pin(async {
            Ok(ProviderConversationState::Ollama {
                transcript_revision: 0,
            })
        })
    }

    fn seed(
        &mut self,
        _state: ProviderConversationState,
    ) -> AdapterFuture<'_, Result<(), ProviderRuntimeError>> {
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test(start_paused = true)]
async fn c11_ask_wait_is_excluded_from_active_deadline() {
    // Given: a model request with one second of active time and a controlled ASK wait signal.
    let fixture = RuntimeFixture::new("ask-deadline");
    let binding = fixture.binding("http://127.0.0.1:9/v1");
    let (started, running) = tokio::sync::oneshot::channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let adapter = WaitingAdapter {
        started: Some(started),
        release: Arc::clone(&release),
    };
    let (ask, ask_waiting) = tokio::sync::watch::channel(false);
    let request = adapter_request(&fixture, binding, 96, Some(Duration::from_secs(1)));
    let mut adapter = adapter;
    let mut task = Box::pin(ProviderRuntime::run_adapter_step(
        &mut adapter,
        request,
        None,
        None,
        ask_waiting,
    ));
    std::future::poll_fn(|context| {
        assert!(std::future::Future::poll(task.as_mut(), context).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    running.await.expect("adapter entered model request");

    // When: five seconds pass while ASK is pending, then the user wait ends.
    ask.send(true).expect("mark ASK pending");
    std::future::poll_fn(|context| {
        assert!(std::future::Future::poll(task.as_mut(), context).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    tokio::time::advance(Duration::from_secs(5)).await;
    std::future::poll_fn(|context| {
        assert!(
            std::future::Future::poll(task.as_mut(), context).is_pending(),
            "ASK time consumed the active deadline"
        );
        std::task::Poll::Ready(())
    })
    .await;
    ask.send(false).expect("mark ASK answered");
    release.notify_one();
    let result = task
        .await
        .expect("ASK wait must preserve remaining active time");

    // Then: the original model request resumes and completes.
    assert!(matches!(
        result.outcome,
        AdapterStepOutcome::Completed {
            output: AdapterOutput::Text(ref text),
            ..
        } if text == "released"
    ));
}
