use super::schema_contract::{accepted, validate_tool_contract, FixtureResponse};
use super::*;

pub(super) async fn responses(
    headers: axum::http::HeaderMap,
    axum::Json(body): axum::Json<Value>,
) -> FixtureResponse {
    assert_eq!(
        headers
            .get("authorization")
            .and_then(|value| value.to_str().ok()),
        Some("Bearer fixture-key")
    );
    assert_eq!(
        headers
            .get("x-opencode-session")
            .and_then(|value| value.to_str().ok()),
        Some("fixture-session")
    );
    if body.pointer("/tool_choice/name").and_then(Value::as_str) == Some(STRUCTURED_TOOL) {
        if let Err(response) = validate_tool_contract(&body, OpenCodeGoWire::Responses, true) {
            return response;
        }
        return accepted(concat!(
            "event: response.output_item.added\n",
            "data: {\"item\":{\"type\":\"function_call\",\"call_id\":\"structured\",\"name\":\"submit_structured_result\",\"arguments\":\"{\\\"ok\\\":true}\"}}\n\n",
            "event: response.completed\n",
            "data: {\"response\":{\"usage\":{\"input_tokens\":4,\"output_tokens\":1}}}\n\n"
        )
        .to_string());
    }
    if let Err(response) = validate_tool_contract(&body, OpenCodeGoWire::Responses, false) {
        return response;
    }
    if body["input"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item["type"] == "function_call_output")
    }) {
        let input = body["input"].as_array().unwrap();
        assert_eq!(input.len(), 6);
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[1]["type"], "reasoning");
        assert_eq!(input[1]["encrypted_content"], "encrypted-reasoning");
        assert_eq!(input[2]["type"], "function_call");
        assert_eq!(input[2]["call_id"], "z");
        assert_eq!(input[3]["type"], "function_call");
        assert_eq!(input[3]["call_id"], "a");
        assert_eq!(input[4]["type"], "function_call_output");
        assert_eq!(input[4]["call_id"], "z");
        assert_eq!(input[5]["type"], "function_call_output");
        assert_eq!(input[5]["call_id"], "a");
        return accepted(
            concat!(
                "event: response.output_text.delta\n",
                "data: {\"delta\":\"done\"}\n\n",
                "event: response.completed\n",
                "data: {\"response\":{\"usage\":{\"input_tokens\":4,\"output_tokens\":1}}}\n\n"
            )
            .to_string(),
        );
    }
    accepted(concat!(
        "event: response.output_item.added\n",
        "data: {\"item\":{\"type\":\"reasoning\",\"id\":\"reasoning-1\",\"encrypted_content\":\"encrypted-reasoning\"}}\n\n",
        "event: response.output_item.added\n",
        "data: {\"item\":{\"type\":\"function_call\",\"call_id\":\"z\",\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\":\\\"src/main.eps\\\"}\"}}\n\n",
        "event: response.output_item.added\n",
        "data: {\"item\":{\"type\":\"function_call\",\"call_id\":\"a\",\"name\":\"search_docs\",\"arguments\":\"{\\\"query\\\":\\\"eps\\\"}\"}}\n\n",
        "event: response.completed\n",
        "data: {\"response\":{\"usage\":{\"input_tokens\":4,\"output_tokens\":1}}}\n\n"
    )
    .to_string())
}

pub(super) async fn chat(
    headers: axum::http::HeaderMap,
    axum::Json(body): axum::Json<Value>,
) -> FixtureResponse {
    assert_eq!(
        headers
            .get("authorization")
            .and_then(|value| value.to_str().ok()),
        Some("Bearer fixture-key")
    );
    if body
        .pointer("/tool_choice/function/name")
        .and_then(Value::as_str)
        == Some(STRUCTURED_TOOL)
    {
        if let Err(response) = validate_tool_contract(&body, OpenCodeGoWire::ChatCompletions, true)
        {
            return response;
        }
        return accepted(concat!(
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"structured\",\"function\":{\"name\":\"submit_structured_result\",\"arguments\":\"{\\\"ok\\\":true}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
            "data: [DONE]\n\n"
        )
        .to_string());
    }
    if let Err(response) = validate_tool_contract(&body, OpenCodeGoWire::ChatCompletions, false) {
        return response;
    }
    if body["messages"]
        .as_array()
        .is_some_and(|items| items.iter().any(|item| item["role"] == "tool"))
    {
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[1]["reasoning_content"], "think");
        assert_eq!(
            messages[1]["reasoning_details"],
            json!([{"signature":"signed-chat"}])
        );
        let calls = messages[1]["tool_calls"].as_array().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0]["id"], "z");
        assert_eq!(calls[1]["id"], "a");
        assert_eq!(messages[2]["role"], "tool");
        assert_eq!(messages[2]["tool_call_id"], "z");
        assert_eq!(messages[3]["role"], "tool");
        assert_eq!(messages[3]["tool_call_id"], "a");
        return accepted(concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"done\"},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n"
        )
        .to_string());
    }
    accepted(concat!(
        "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"think\",\"reasoning_details\":[{\"signature\":\"signed-chat\"}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[",
        "{\"index\":0,\"id\":\"z\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\":\\\"src/main.eps\\\"}\"}},",
        "{\"index\":1,\"id\":\"a\",\"function\":{\"name\":\"search_docs\",\"arguments\":\"{\\\"query\\\":\\\"eps\\\"}\"}}",
        "]},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n"
    )
    .to_string())
}

pub(super) async fn messages(
    headers: axum::http::HeaderMap,
    axum::Json(body): axum::Json<Value>,
) -> FixtureResponse {
    assert_eq!(
        headers
            .get("x-api-key")
            .and_then(|value| value.to_str().ok()),
        Some("fixture-key")
    );
    assert!(!headers.contains_key("authorization"));
    if body.pointer("/tool_choice/name").and_then(Value::as_str) == Some(STRUCTURED_TOOL) {
        if let Err(response) =
            validate_tool_contract(&body, OpenCodeGoWire::AnthropicMessages, true)
        {
            return response;
        }
        return accepted(concat!(
            "event: content_block_start\n",
            "data: {\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"structured\",\"name\":\"submit_structured_result\",\"input\":{\"ok\":true}}}\n\n",
            "event: message_delta\n",
            "data: {\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":1}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n"
        )
        .to_string());
    }
    if let Err(response) = validate_tool_contract(&body, OpenCodeGoWire::AnthropicMessages, false) {
        return response;
    }
    if body["messages"].as_array().is_some_and(|items| {
        items.iter().any(|item| {
            item["content"]
                .as_array()
                .is_some_and(|content| content.iter().any(|block| block["type"] == "tool_result"))
        })
    }) {
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[1]["role"], "assistant");
        let assistant = messages[1]["content"].as_array().unwrap();
        assert_eq!(assistant.len(), 3);
        assert_eq!(assistant[0]["type"], "thinking");
        assert_eq!(assistant[0]["thinking"], "think");
        assert_eq!(assistant[0]["signature"], "signed");
        assert_eq!(assistant[1]["type"], "tool_use");
        assert_eq!(assistant[1]["id"], "z");
        assert_eq!(assistant[2]["type"], "tool_use");
        assert_eq!(assistant[2]["id"], "a");
        assert_eq!(messages[2]["role"], "user");
        let results = messages[2]["content"].as_array().unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0]["tool_use_id"], "z");
        assert_eq!(results[1]["tool_use_id"], "a");
        return accepted(
            concat!(
            "event: content_block_delta\n",
            "data: {\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"done\"}}\n\n",
            "event: message_delta\n",
            "data: {\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n"
        )
            .to_string(),
        );
    }
    accepted(concat!(
        "event: content_block_delta\n",
        "data: {\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"th\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"ink\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"signed\"}}\n\n",
        "event: content_block_start\n",
        "data: {\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"z\",\"name\":\"read_file\",\"input\":{\"path\":\"src/main.eps\"}}}\n\n",
        "event: content_block_start\n",
        "data: {\"index\":2,\"content_block\":{\"type\":\"tool_use\",\"id\":\"a\",\"name\":\"search_docs\",\"input\":{\"query\":\"eps\"}}}\n\n",
        "event: message_delta\n",
        "data: {\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":1}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n"
    )
    .to_string())
}
