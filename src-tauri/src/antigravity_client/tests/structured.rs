use super::*;
use crate::provider_runtime::{JobBase, RunOutcome, StructuredJobExecutor, StructuredJobRequest};

#[tokio::test]
async fn production_structured_jobs_reject_forbidden_duplicate_and_truncated_payloads() {
    for failure in ["forbidden", "duplicate", "malformed", "truncated"] {
        let name = if failure == "forbidden" {
            "read_file"
        } else {
            "submit_structured_result"
        };
        let mut parts = vec![json!({"functionCall":{"id":"a","name":name,"args":{"ok":true}}})];
        if failure == "duplicate" {
            parts.push(json!({"functionCall":{"id":"b","name":name,"args":{"ok":true}}}));
        }
        let payload = if failure == "malformed" {
            "data: {\"response\":{\"candidates\":[\n\n".to_string()
        } else {
            sse(json!({"response":{"candidates":[{"content":{"parts":parts},
                "finishReason":if failure == "truncated" { "MAX_TOKENS" } else { "STOP" }}]}}))
        };
        let (endpoint, requests) = fixture(vec![payload]).await;
        let step = structured_request();
        let (cancel, cancellation) =
            tokio::sync::watch::channel(step.identity.cancellation_generation);
        let request = StructuredJobRequest {
            identity: step.identity,
            binding: step.binding,
            kind: StructuredJobKind::TaskStateCompiler,
            prompt: "compile".into(),
            workspace_root: std::env::temp_dir(),
            output_schema: json!({"type":"object","properties":{"ok":{"type":"boolean"}},"required":["ok"]}),
            base: JobBase {
                revision: 4,
                instruction_epoch: 2,
                branch: None,
            },
            policy: step.policy,
        };
        let mut executor = StructuredJobExecutor::new(Box::new(adapter(endpoint)), cancellation);
        let outcome = executor.run(request).await;
        assert!(
            matches!(outcome, RunOutcome::Failed(_)),
            "{failure}: {outcome:?}"
        );
        assert_eq!(
            requests.lock().len(),
            1,
            "{failure} must not retry a rejected result"
        );
        drop(cancel);
    }
}

#[tokio::test]
async fn isolates_structured_prompt_and_validates_the_complete_result() {
    let result = sse(json!({"response":{"candidates":[{"content":{"parts":[{
        "functionCall":{"id":"structured","name":"submit_structured_result","args":{"ok":true}}
    }]},"finishReason":"STOP"}]}}));
    let (endpoint, requests) = fixture(vec![result]).await;
    let mut adapter = adapter(endpoint);
    let (events_tx, _events_rx) = tokio::sync::mpsc::channel(8);

    let outcome = adapter
        .run_step(structured_request(), events_tx)
        .await
        .unwrap();

    assert!(
        matches!(outcome, AdapterStepOutcome::Completed { output: AdapterOutput::Structured(ref value), .. } if value == &json!({"ok":true}))
    );
    let sent = requests.lock();
    assert_eq!(
        sent[0]["request"]["contents"][0]["parts"][0]["text"],
        "compile input"
    );
    assert_eq!(
        sent[0]["request"]["generationConfig"]["maxOutputTokens"],
        8_192
    );
}

#[tokio::test]
async fn rejects_structured_result_that_fails_the_full_schema() {
    let result = sse(json!({"response":{"candidates":[{"content":{"parts":[{
        "functionCall":{"id":"structured","name":"submit_structured_result","args":{"ok":"yes"}}
    }]},"finishReason":"STOP"}]}}));
    let (endpoint, _) = fixture(vec![result]).await;
    let mut adapter = adapter(endpoint);
    let (events_tx, _events_rx) = tokio::sync::mpsc::channel(8);

    let error = adapter
        .run_step(structured_request(), events_tx)
        .await
        .unwrap_err();

    assert_eq!(error, ProviderRuntimeError::StructuredOutputInvalid);
}
