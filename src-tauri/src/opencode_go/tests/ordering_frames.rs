use super::{fixture::*, *};

pub(super) fn interleaved(wire: OpenCodeGoWire) -> String {
    text(wire, 0, "A")
        + &fragmented_call(wire, 0, "z", "a")
        + &reasoning(wire)
        + &text(wire, 3, "B")
        + &fragmented_call(wire, 1, "a", "b")
        + &text(wire, 5, "C")
        + &terminal(wire, true, false)
}

fn text(wire: OpenCodeGoWire, index: u64, text: &str) -> String {
    if wire != OpenCodeGoWire::AnthropicMessages {
        return text_frame(wire, text);
    }
    frame(
        Some("content_block_start"),
        json!({"index":index,"content_block":{"type":"text","text":""}}),
    ) + &frame(
        Some("content_block_delta"),
        json!({"index":index,"delta":{"type":"text_delta","text":text}}),
    ) + &frame(Some("content_block_stop"), json!({"index":index}))
}

fn fragmented_call(wire: OpenCodeGoWire, index: u64, id: &str, path: &str) -> String {
    let arguments = json!({"path":path}).to_string();
    let (first, last) = arguments.split_at(5);
    match wire {
        OpenCodeGoWire::Responses => {
            let item_id = format!("item-{id}");
            frame(
                Some("response.output_item.added"),
                json!({"item":{
                    "id":item_id,"type":"function_call","call_id":id,"name":"read_file","arguments":""
                }}),
            ) + &frame(
                Some("response.function_call_arguments.delta"),
                json!({"item_id":item_id,"delta":first}),
            ) + &frame(
                Some("response.function_call_arguments.delta"),
                json!({"item_id":item_id,"delta":last}),
            ) + &frame(
                Some("response.output_item.done"),
                json!({"item":{
                    "id":item_id,"type":"function_call","call_id":id,"name":"read_file","arguments":arguments
                }}),
            )
        }
        OpenCodeGoWire::AnthropicMessages => {
            let index = index * 3 + 1;
            frame(
                Some("content_block_start"),
                json!({"index":index,"content_block":{
                    "type":"tool_use","id":id,"name":"read_file","input":{}
                }}),
            ) + &frame(
                Some("content_block_delta"),
                json!({"index":index,"delta":{"type":"input_json_delta","partial_json":first}}),
            ) + &frame(
                Some("content_block_delta"),
                json!({"index":index,"delta":{"type":"input_json_delta","partial_json":last}}),
            ) + &frame(Some("content_block_stop"), json!({"index":index}))
        }
        OpenCodeGoWire::ChatCompletions => {
            frame(
                None,
                json!({"choices":[{"delta":{"tool_calls":[{
                    "index":index,"id":id,"function":{"name":"read_file","arguments":first}
                }]}}]}),
            ) + &frame(
                None,
                json!({"choices":[{"delta":{"tool_calls":[{
                    "index":index,"function":{"arguments":last}
                }]}}]}),
            )
        }
    }
}

fn reasoning(wire: OpenCodeGoWire) -> String {
    match wire {
        OpenCodeGoWire::Responses => {
            frame(
                Some("response.reasoning_summary_text.delta"),
                json!({"delta":"R"}),
            ) + &frame(
                Some("response.output_item.done"),
                json!({"item":{"type":"reasoning","id":"reason","encrypted_content":"encrypted-R"}}),
            )
        }
        OpenCodeGoWire::AnthropicMessages => {
            frame(
                Some("content_block_delta"),
                json!({"index":2,"delta":{"type":"thinking_delta","thinking":"R"}}),
            ) + &frame(
                Some("content_block_delta"),
                json!({"index":2,"delta":{"type":"signature_delta","signature":"signed-R"}}),
            )
        }
        OpenCodeGoWire::ChatCompletions => frame(
            None,
            json!({"choices":[{"delta":{"reasoning_content":"R"}}]}),
        ),
    }
}
