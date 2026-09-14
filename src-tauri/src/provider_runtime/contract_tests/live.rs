use serde_json::json;

use crate::{
    provider::ProviderConversationState,
    provider_runtime::{RunOutcome, RuntimeExecutor},
    provider_transcript::{ProviderTranscriptStore, TranscriptBlock, TranscriptBranch},
};

use super::{fixtures::RuntimeFixture, runtime};

#[tokio::test]
#[ignore = "requires a live local Ollama tool/structured-capable model"]
async fn live_ollama_runtime_main_tools_structured_and_resume() {
    // Given: an isolated native project and an explicitly selected live Ollama model.
    let model = std::env::var("EUD_AGENT_OLLAMA_LIVE_MODEL")
        .expect("set EUD_AGENT_OLLAMA_LIVE_MODEL to a tool/structured-capable model");
    let base_url = std::env::var("EUD_AGENT_OLLAMA_LIVE_BASE_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:11434/v1".to_string());
    let fixture = RuntimeFixture::new("live-ollama");
    fixture.install_live_map_from_env();
    let mut binding = fixture.binding(&base_url);
    binding.model = model;
    let mut runtime = runtime(&fixture, binding.clone());
    let mut main = fixture.foreground(binding.clone(), 20_001);
    main.turn.text = "Call list_files and project_status, then read_file for src/main.eps twice with two distinct tool call IDs, then explain briefly what file was read.".to_string();

    // When: foreground tools, an isolated structured job, and a follow-up run in sequence.
    let conversation = match runtime.run_foreground(main).await {
        RunOutcome::Completed { conversation, .. } => conversation,
        other => panic!("live Ollama foreground failed: {other:?}"),
    };
    let revision = match conversation {
        ProviderConversationState::Ollama {
            transcript_revision,
        } => transcript_revision,
        other => panic!("live Ollama returned wrong conversation state: {other:?}"),
    };
    let stored = ProviderTranscriptStore::new(&fixture.dirs)
        .restore(
            crate::provider::ProviderId::Ollama,
            &fixture.session_id,
            revision,
            &TranscriptBranch {
                instruction_epoch: 3,
                task_leaf_id: Some("leaf-a".to_string()),
            },
        )
        .expect("restore live Ollama transcript")
        .expect("live Ollama transcript generation");
    for (tool, minimum) in [("list_files", 1), ("project_status", 1), ("read_file", 2)] {
        let count = stored.generation.checkpoint.blocks.iter().filter(|block| {
            matches!(block, TranscriptBlock::ToolResult { name, is_error: false, .. } if name == tool)
        }).count();
        assert!(
            count >= minimum,
            "live Ollama did not complete required tool {tool}"
        );
    }
    let event_count = fixture.events.snapshot().len();
    let mut job_binding = binding.clone();
    job_binding.conversation = ProviderConversationState::Ollama {
        transcript_revision: revision,
    };
    let mut job = fixture.structured(
        job_binding.clone(),
        20_002,
        json!({
            "type": "object",
            "properties": {"ok": {"type": "boolean"}},
            "required": ["ok"],
            "additionalProperties": false
        }),
    );
    job.prompt = "Return exactly one JSON object with ok set to true.".to_string();
    job.policy.active_deadline = Some(crate::provider_runtime::TASK_STATE_COMPILER_DEADLINE);
    assert!(matches!(
        runtime.run_structured(job).await,
        RunOutcome::Structured { ref value, .. } if value == &json!({"ok": true})
    ));
    assert_eq!(fixture.events.snapshot().len(), event_count);
    assert_eq!(
        runtime.conversation_state(),
        ProviderConversationState::Ollama {
            transcript_revision: revision
        }
    );
    fixture
        .tools
        .begin_request(
            "live-ollama-followup",
            &fixture.root.join("project").to_string_lossy(),
        )
        .expect("begin live follow-up");
    let mut followup = fixture.foreground(job_binding.clone(), 20_003);
    followup.identity.request_id = "live-ollama-followup".to_string();
    followup.binding = job_binding;
    followup.turn.text = "Continue from the main conversation with one short sentence.".to_string();
    let outcome = runtime.run_foreground(followup).await;

    // Then: the follow-up succeeds from main history and the compiler never changed main state.
    assert!(matches!(
        outcome,
        RunOutcome::Completed { ref text, .. } if !text.trim().is_empty()
    ));
}

#[tokio::test]
#[ignore = "requires a live local Ollama embedding-only model"]
async fn live_ollama_embedding_model_rejects_chat_without_fallback() {
    // Given: the known embedding-only model is explicitly bound to one isolated runtime.
    let model = std::env::var("EUD_AGENT_OLLAMA_EMBEDDING_MODEL")
        .unwrap_or_else(|_| "bge-m3:latest".to_string());
    let base_url = std::env::var("EUD_AGENT_OLLAMA_LIVE_BASE_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:11434/v1".to_string());
    let fixture = RuntimeFixture::new("live-ollama-embedding");
    let mut binding = fixture.binding(&base_url);
    binding.model = model;
    binding.capabilities = None;
    let mut runtime = runtime(&fixture, binding.clone());
    let mut request = fixture.foreground(binding, 20_011);
    request.policy.active_deadline = Some(std::time::Duration::from_secs(30));
    request.turn.text = "Respond with one word.".to_string();

    // When: the common runtime sends a foreground chat to that exact bound model.
    let outcome = runtime.run_foreground(request).await;

    // Then: the provider rejection is terminal and no alternate provider/model completes it.
    assert!(matches!(outcome, RunOutcome::Failed(_)));
    assert!(matches!(
        runtime.conversation_state(),
        ProviderConversationState::Ollama {
            transcript_revision: 0
        }
    ));
}
