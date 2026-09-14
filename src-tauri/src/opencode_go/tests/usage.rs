use super::{fixture::*, *};

#[tokio::test]
async fn production_http_preserves_complete_usage_snapshots_across_wires() {
    for wire in WIRES {
        let stream = match wire {
            OpenCodeGoWire::Responses => {
                text_frame(wire, "done")
                    + &frame(
                        Some("response.completed"),
                        json!({"response":{"status":"completed","usage":{
                            "input_tokens":4,"output_tokens":3,"total_tokens":7,"input_tokens_details":{"cached_tokens":2},"output_tokens_details":{"reasoning_tokens":1}
                        }}}),
                    )
            }
            OpenCodeGoWire::ChatCompletions => {
                text_frame(wire, "done")
                    + &frame(
                        None,
                        json!({"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{
                            "prompt_tokens":4,"completion_tokens":3,"total_tokens":7,"prompt_tokens_details":{"cached_tokens":2},"completion_tokens_details":{"reasoning_tokens":1}
                        }}),
                    )
                    + "data: [DONE]\n\n"
            }
            OpenCodeGoWire::AnthropicMessages => {
                frame(
                    Some("message_start"),
                    json!({"message":{"usage":{
                        "input_tokens":4,"cache_read_input_tokens":2,"cache_creation_input_tokens":1
                    }}}),
                ) + &text_frame(wire, "done")
                    + &frame(
                        Some("message_delta"),
                        json!({"delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}),
                    )
                    + &frame(Some("message_stop"), json!({}))
            }
        };
        let mut fixture = serve(wire, stream, None).await;
        let (_cancel, cancellation) = tokio::sync::watch::channel(0);
        let (events, mut received) = tokio::sync::mpsc::channel(8);
        fixture
            .adapter
            .run_step(
                fixture_request("fixture-wire", Vec::new(), cancellation),
                events,
            )
            .await
            .unwrap();
        let usage = std::iter::from_fn(|| received.try_recv().ok())
            .find_map(|event| match event.kind {
                AdapterEventKind::Usage(usage) => Some(usage),
                _ => None,
            })
            .expect("normalized usage event");
        let context = usage.context_usage.expect("full context usage snapshot");
        assert_eq!(context.last.input_tokens, 4, "{wire:?}");
        assert_eq!(context.last.cached_input_tokens, 2, "{wire:?}");
        assert_eq!(
            context.last.cache_write_input_tokens,
            i64::from(wire == OpenCodeGoWire::AnthropicMessages)
        );
        assert_eq!(context.last.output_tokens, 3);
        assert_eq!(
            context.last.reasoning_output_tokens,
            i64::from(wire != OpenCodeGoWire::AnthropicMessages)
        );
        assert_eq!(context.last.total_tokens, 7);
        assert_eq!(context.total, context.last);
        assert_eq!(context.model_context_window, Some(128_000));
    }
}
