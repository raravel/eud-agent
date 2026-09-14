use std::{sync::Arc, time::Duration};

use serde_json::json;
use zeroize::Zeroizing;

use crate::{
    provider::{ProviderConversationState, ProviderId},
    provider_runtime::{BindingSnapshot, ProviderRuntime, RunOutcome, RuntimeExecutor},
    provider_secrets::ProviderSecretStore,
    provider_transcript::{ProviderTranscriptStore, TranscriptBlock, TranscriptBranch},
};

use super::fixtures::RuntimeFixture;
use selection::{select_rows, wire_name, LiveRow};

mod selection;

fn binding(row: &LiveRow) -> BindingSnapshot {
    BindingSnapshot {
        provider: ProviderId::OpencodeGo,
        model: row.model.model.clone(),
        reasoning: None,
        base_url: None,
        capabilities: Some(row.model.capabilities.clone()),
        conversation: ProviderConversationState::empty(ProviderId::OpencodeGo),
    }
}

async fn run_row(row: &LiveRow) -> Result<bool, String> {
    let fixture = RuntimeFixture::new(&format!("live-opencode-{}", wire_name(row.wire)));
    let initial_binding = binding(row);
    let adapter = super::super::production_adapter(
        &initial_binding.to_binding(),
        &fixture.dirs,
        fixture.root.clone(),
    )
    .map_err(|error| error.to_string())?;
    let events: Arc<dyn crate::provider_runtime::RuntimeEventSink> = fixture.events.clone();
    let mut runtime = ProviderRuntime::new(
        adapter,
        initial_binding.clone(),
        fixture.dirs.clone(),
        fixture.root.clone(),
        fixture.tools.clone(),
        fixture.cancellation.subscribe(),
        events,
    )
    .map_err(|error| error.to_string())?;
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let mut first = fixture.foreground(initial_binding.clone(), 40_001);
    first.policy.active_deadline = Some(Duration::from_secs(120));
    first.policy.max_output_tokens = Some(2_048);
    first.turn.text = format!(
        "In this exact order, call list_files, project_status, read_file for src/main.eps, then read_file for src/main.eps again with a distinct call ID. Remember token {nonce}. Reply briefly after all four calls finish."
    );
    let first_outcome = runtime.run_foreground(first).await;
    let first_state = match first_outcome {
        RunOutcome::Completed { text, conversation } if !text.trim().is_empty() => conversation,
        other => return Err(format!("foreground:{other:?}")),
    };
    let revision = match first_state {
        ProviderConversationState::OpencodeGo {
            transcript_revision,
        } => transcript_revision,
        other => return Err(format!("conversation:{other:?}")),
    };
    let restored = ProviderTranscriptStore::new(&fixture.dirs)
        .restore(
            ProviderId::OpencodeGo,
            &fixture.session_id,
            revision,
            &TranscriptBranch {
                instruction_epoch: 3,
                task_leaf_id: Some("leaf-a".to_string()),
            },
        )?
        .ok_or_else(|| "transcript:missing_generation".to_string())?;
    let completed = restored
        .generation
        .checkpoint
        .blocks
        .iter()
        .filter_map(|block| match block {
            TranscriptBlock::ToolResult {
                name,
                is_error: false,
                ..
            } => Some(name.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if completed != ["list_files", "project_status", "read_file", "read_file"] {
        return Err(format!("ordered_tools:{completed:?}"));
    }
    fixture.tools.begin_request(
        "opencode-live-resume",
        &fixture.root.join("project").to_string_lossy(),
    )?;
    let mut resumed_binding = initial_binding.clone();
    resumed_binding.conversation = ProviderConversationState::OpencodeGo {
        transcript_revision: revision,
    };
    let mut followup = fixture.foreground(resumed_binding.clone(), 40_002);
    followup.identity.request_id = "opencode-live-resume".to_string();
    followup.binding = resumed_binding.clone();
    followup.policy.active_deadline = Some(Duration::from_secs(120));
    followup.policy.max_output_tokens = Some(256);
    followup.turn.text = "Return only the token you were asked to remember.".to_string();
    let followup_outcome = runtime.run_foreground(followup).await;
    match followup_outcome {
        RunOutcome::Completed { text, .. } if text.contains(&nonce) => {}
        other => return Err(format!("resume:{other:?}")),
    }
    if row.model.capabilities.strict_structured_output {
        let conversation_before = runtime.conversation_state();
        let mut job = fixture.structured(
            resumed_binding,
            40_003,
            json!({
                "type": "object",
                "properties": {"ok": {"type": "boolean"}},
                "required": ["ok"],
                "additionalProperties": false
            }),
        );
        job.prompt = "Submit one structured result with ok set to true.".to_string();
        job.policy.active_deadline = Some(crate::provider_runtime::TASK_STATE_COMPILER_DEADLINE);
        job.policy.max_output_tokens = Some(512);
        let structured_outcome = runtime.run_structured(job).await;
        match structured_outcome {
            RunOutcome::Structured { value, .. } if value == json!({"ok": true}) => {}
            other => return Err(format!("structured:{other:?}")),
        }
        if runtime.conversation_state() != conversation_before {
            return Err("structured:foreground_isolation_changed".to_string());
        }
    }
    Ok(row.model.capabilities.strict_structured_output)
}

#[tokio::test]
#[ignore = "requires the product OpenCode Go credential and remote service access"]
async fn live_opencode_go_catalog_wires_runtime_contract() {
    // Given: the exact product credential target and catalog, with all app state isolated.
    let catalog_fixture = RuntimeFixture::new("live-opencode-catalog");
    let store = ProviderSecretStore::new(catalog_fixture.dirs.clone())
        .expect("construct product credential store");
    let key = Zeroizing::new(
        store
            .read_secret(ProviderId::OpencodeGo, "api-key")
            .expect("read product OpenCode Go credential target")
            .expect("product OpenCode Go credential is missing"),
    );
    let client = reqwest::Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(120))
        .user_agent(concat!("eud-agent/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("construct live catalog client");
    let catalog = crate::opencode_go::fetch_live_contract_catalog(&client, &key)
        .await
        .expect("read authenticated OpenCode Go catalog");
    let selected = select_rows(&catalog);
    assert!(
        !selected.is_empty(),
        "no tool-capable catalog rows selected"
    );
    for row in &selected {
        eprintln!(
            "OPENCODE_LIVE wire={} model={} tool_calls={} structured={} outcome=selected",
            wire_name(row.wire),
            row.model.model,
            row.model.capabilities.tool_calls,
            row.model.capabilities.strict_structured_output
        );
    }

    // When: each explicitly listed row runs once through the production runtime.
    let mut failures = Vec::new();
    for row in &selected {
        match run_row(row).await {
            Ok(structured) => {
                eprintln!(
                    "OPENCODE_LIVE wire={} model={} capability=tools_resume outcome=passed",
                    wire_name(row.wire),
                    row.model.model
                );
                eprintln!(
                    "OPENCODE_LIVE wire={} model={} capability=structured outcome={}",
                    wire_name(row.wire),
                    row.model.model,
                    if structured {
                        "passed"
                    } else {
                        "provider_capability_unsupported"
                    }
                );
            }
            Err(code) => {
                eprintln!(
                    "OPENCODE_LIVE wire={} model={} capability=tools_resume_structured outcome=failed code={}",
                    wire_name(row.wire),
                    row.model.model,
                    code
                );
                failures.push(format!(
                    "{}:{}:{code}",
                    wire_name(row.wire),
                    row.model.model
                ));
            }
        }
    }

    // Then: no failed row is retried with another model or reported as supported.
    assert!(
        failures.is_empty(),
        "OpenCode Go live rows failed: {failures:?}"
    );
}
