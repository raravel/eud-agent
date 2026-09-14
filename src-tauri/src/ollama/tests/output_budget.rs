use super::*;
use crate::provider_runtime::{
    JobBase, RunOutcome, StructuredJobExecutor, StructuredJobKind, StructuredJobRequest,
};

#[tokio::test]
async fn production_http_applies_compiler_budget_without_capping_unbounded_jobs() {
    for (kind, budget) in [
        (Some(StructuredJobKind::TaskStateCompiler), Some(8_192)),
        (Some(StructuredJobKind::HarnessGenerator), None),
        (None, None),
    ] {
        let payload = format!(
            "data: {}\n\ndata: [DONE]\n\n",
            json!({"choices":[{
                "delta":{"content":"{\"ok\":true}"},"finish_reason":"stop"
            }]})
        );
        let (base_url, requests, server) = fixture(sse_response(&payload));
        let (_cancel, mut step) = request(&base_url, Vec::new(), Vec::new());
        step.policy.max_output_tokens = budget;
        let mut adapter = ProductionOllamaAdapter::with_client(reqwest::Client::new(), None);
        if let Some(kind) = kind {
            let job = StructuredJobRequest {
                identity: step.identity,
                binding: step.binding,
                kind,
                prompt: "compile".into(),
                workspace_root: std::env::temp_dir(),
                output_schema: json!({"type":"object","properties":{"ok":{"type":"boolean"}},"required":["ok"]}),
                base: JobBase {
                    revision: 0,
                    instruction_epoch: 0,
                    branch: None,
                },
                policy: step.policy,
            };
            let mut executor = StructuredJobExecutor::new(Box::new(adapter), step.cancellation);
            assert!(matches!(
                executor.run(job).await,
                RunOutcome::Structured { .. }
            ));
        } else {
            let (events, _received) = tokio::sync::mpsc::channel(8);
            assert!(matches!(
                adapter.run_step(step, events).await.unwrap(),
                AdapterStepOutcome::Completed { .. }
            ));
        }
        server.join().unwrap();
        let request = requests.recv().unwrap();
        let (_, body) = request.split_once("\r\n\r\n").unwrap();
        let body: Value = serde_json::from_str(body).unwrap();
        assert_eq!(
            body.get("max_tokens").and_then(Value::as_u64),
            budget,
            "{kind:?}"
        );
    }
}
