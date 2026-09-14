use super::*;

pub(super) type FixtureResponse = (axum::http::StatusCode, String);

pub(super) fn accepted(body: String) -> FixtureResponse {
    (axum::http::StatusCode::OK, body)
}

pub(super) fn validate_tool_contract(
    body: &Value,
    wire: OpenCodeGoWire,
    structured: bool,
) -> Result<(), FixtureResponse> {
    let Some(tools) = body.get("tools").and_then(Value::as_array) else {
        return Err(rejected());
    };
    let valid = if structured {
        tools.len() == 1 && structured_tool_is_valid(&tools[0], wire)
    } else {
        let expected = [
            (
                "read_file",
                json!({
                    "type":"object",
                    "properties":{
                        "path":{"type":"string"},
                        "range":{
                            "type":"object",
                            "properties":{
                                "start":{"type":"integer"},
                                "end":{"type":"integer"}
                            },
                            "required":["start"],
                            "additionalProperties":false
                        }
                    },
                    "required":["path"],
                    "additionalProperties":false
                }),
            ),
            (
                "search_docs",
                json!({
                    "type":"object",
                    "properties":{"query":{"type":"string"}},
                    "required":["query"],
                    "additionalProperties":false
                }),
            ),
        ];
        tools.len() == expected.len()
            && tools
                .iter()
                .zip(expected)
                .all(|(tool, expected)| ordinary_tool_is_valid(tool, wire, expected.0, &expected.1))
    };
    valid.then_some(()).ok_or_else(rejected)
}

fn ordinary_tool_is_valid(tool: &Value, wire: OpenCodeGoWire, name: &str, schema: &Value) -> bool {
    let function = function_definition(tool, wire);
    function["name"] == name
        && function[schema_key(wire)] == *schema
        && match wire {
            OpenCodeGoWire::Responses | OpenCodeGoWire::ChatCompletions => {
                function.get("strict") == Some(&Value::Bool(false))
            }
            OpenCodeGoWire::AnthropicMessages => function.get("strict").is_none(),
        }
}

fn structured_tool_is_valid(tool: &Value, wire: OpenCodeGoWire) -> bool {
    let function = function_definition(tool, wire);
    function["name"] == STRUCTURED_TOOL
        && function[schema_key(wire)]
            == json!({
                "type":"object",
                "required":["ok"],
                "properties":{"ok":{"type":"boolean"}},
                "additionalProperties":false
            })
        && match wire {
            OpenCodeGoWire::Responses | OpenCodeGoWire::ChatCompletions => {
                function.get("strict") == Some(&Value::Bool(true))
            }
            OpenCodeGoWire::AnthropicMessages => function.get("strict").is_none(),
        }
}

fn function_definition(tool: &Value, wire: OpenCodeGoWire) -> &Value {
    match wire {
        OpenCodeGoWire::ChatCompletions => &tool["function"],
        OpenCodeGoWire::Responses | OpenCodeGoWire::AnthropicMessages => tool,
    }
}

const fn schema_key(wire: OpenCodeGoWire) -> &'static str {
    match wire {
        OpenCodeGoWire::AnthropicMessages => "input_schema",
        OpenCodeGoWire::Responses | OpenCodeGoWire::ChatCompletions => "parameters",
    }
}

fn rejected() -> FixtureResponse {
    (
        axum::http::StatusCode::BAD_REQUEST,
        json!({
            "error": {
                "type": "invalid_request_error",
                "param": "tools[0].parameters"
            }
        })
        .to_string(),
    )
}
