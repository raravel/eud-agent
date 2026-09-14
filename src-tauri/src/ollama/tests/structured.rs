use super::*;
use crate::provider_runtime::{
    JobBase, RunOutcome, StructuredJobExecutor, StructuredJobKind, StructuredJobRequest,
};

#[tokio::test]
async fn production_structured_jobs_reject_forbidden_duplicate_truncated_and_invalid_json() {
    for failure in ["forbidden", "duplicate", "truncated", "schema"] {
        let delta = match failure {
            "forbidden" => {
                json!({"tool_calls":[{"index":0,"id":"forbidden","function":{"name":"read_file","arguments":"{}"}}]})
            }
            "duplicate" => json!({"content":"{\"ok\":true}{\"ok\":true}"}),
            "truncated" => json!({"content":"{\"ok\":"}),
            "schema" => json!({"content":"{\"ok\":\"wrong-type\"}"}),
            _ => unreachable!(),
        };
        let reason = if failure == "forbidden" {
            "tool_calls"
        } else {
            "stop"
        };
        let payload = format!(
            "data: {}\n\ndata: [DONE]\n\n",
            json!({"choices":[{"delta":delta,"finish_reason":reason}]})
        );
        let (base_url, requests, server) = fixture(sse_response(&payload));
        let (cancel, step) = request(&base_url, Vec::new(), Vec::new());
        let job = StructuredJobRequest {
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
        let adapter = ProductionOllamaAdapter::with_client(reqwest::Client::new(), None);
        let mut executor = StructuredJobExecutor::new(Box::new(adapter), step.cancellation);
        let outcome = executor.run(job).await;
        server.join().unwrap();
        assert!(
            matches!(
                outcome,
                RunOutcome::Failed(ProviderRuntimeError::StructuredOutputInvalid)
            ),
            "{failure}: {outcome:?}"
        );
        let request = requests.recv().unwrap();
        let (_, body) = request.split_once("\r\n\r\n").unwrap();
        let body: Value = serde_json::from_str(body).unwrap();
        assert!(
            body.get("tools").is_none(),
            "structured jobs must not advertise project tools"
        );
        assert_eq!(body["response_format"]["type"], "json_schema");
        drop(cancel);
    }
}
