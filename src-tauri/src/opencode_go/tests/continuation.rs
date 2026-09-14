use super::fixture::{frame, serve, terminal, text_frame};
use super::*;

#[tokio::test]
async fn encrypted_items_survive_reconstruction_without_summaries_and_keep_distinct_ids() {
    let stream = frame(
        Some("response.output_item.added"),
        json!({"item":{
            "type":"reasoning","id":"r1","encrypted_content":"initial"
        }}),
    ) + &frame(
        Some("response.output_item.done"),
        json!({"item":{
            "type":"reasoning","id":"r1","encrypted_content":"first"
        }}),
    ) + &frame(
        Some("response.output_item.done"),
        json!({"item":{
            "type":"reasoning","id":"r2","encrypted_content":"second"
        }}),
    ) + &text_frame(OpenCodeGoWire::Responses, "answer")
        + &terminal(OpenCodeGoWire::Responses, false, false);
    let mut fixture = serve(OpenCodeGoWire::Responses, stream, None).await;
    let (_cancel, cancellation) = tokio::sync::watch::channel(0);
    let (events, mut received) = tokio::sync::mpsc::channel(16);
    let outcome = fixture
        .adapter
        .run_step(
            fixture_request("fixture-wire", Vec::new(), cancellation),
            events,
        )
        .await
        .unwrap();
    assert!(
        matches!(outcome, AdapterStepOutcome::Completed { output: AdapterOutput::Text(ref answer), .. } if answer == "answer")
    );
    let history: Vec<_> = std::iter::from_fn(|| received.try_recv().ok())
        .filter_map(|event| match event.kind {
            AdapterEventKind::Block(block) => Some(ConversationItem::Assistant(block)),
            _ => None,
        })
        .collect();
    let bytes = serde_json::to_vec(&history).unwrap();
    let restored: Vec<ConversationItem> = serde_json::from_slice(&bytes).unwrap();
    let history: Vec<_> = restored.iter().map(wire_history_entry).collect();
    let input = responses_runtime_history(&history);
    assert_eq!(input.len(), 3);
    assert_eq!(
        input[0],
        json!({"type":"reasoning","id":"r1","encrypted_content":"first"})
    );
    assert_eq!(
        input[1],
        json!({"type":"reasoning","id":"r2","encrypted_content":"second"})
    );
    assert_eq!(input[2]["content"][0]["text"], "answer");
}

#[tokio::test]
async fn thinking_fragments_preserve_distinct_block_identities_and_signature_fragments() {
    let mut stream = String::new();
    for (index, kind, field, value) in [
        (0, "thinking_delta", "thinking", "th"),
        (0, "thinking_delta", "thinking", "ink"),
        (0, "signature_delta", "signature", "si"),
        (0, "signature_delta", "signature", "gned"),
        (1, "thinking_delta", "thinking", "distinct"),
        (1, "signature_delta", "signature", "signed"),
    ] {
        stream.push_str(&frame(
            Some("content_block_delta"),
            json!({"index":index,"delta":{"type":kind,field:value}}),
        ));
    }
    stream.push_str(&text_frame(OpenCodeGoWire::AnthropicMessages, "answer"));
    stream.push_str(&terminal(OpenCodeGoWire::AnthropicMessages, false, false));
    let mut fixture = serve(OpenCodeGoWire::AnthropicMessages, stream, None).await;
    let (_cancel, cancellation) = tokio::sync::watch::channel(0);
    let (events, mut received) = tokio::sync::mpsc::channel(16);
    fixture
        .adapter
        .run_step(
            fixture_request("fixture-wire", Vec::new(), cancellation),
            events,
        )
        .await
        .unwrap();
    let history: Vec<_> = std::iter::from_fn(|| received.try_recv().ok())
        .filter_map(|event| match event.kind {
            AdapterEventKind::Block(block) => {
                Some(wire_history_entry(&ConversationItem::Assistant(block)))
            }
            _ => None,
        })
        .collect();
    let messages = anthropic_runtime_history(&history);
    assert_eq!(
        messages[0]["content"],
        json!([
            {"type":"thinking","thinking":"think","signature":"signed"},
            {"type":"thinking","thinking":"distinct","signature":"signed"},
            {"type":"text","text":"answer"}
        ])
    );
}
