use serde_json::Value;

/// Outcome of the pre-dispatch shape/schema check for one tool call.
pub(super) enum CallAdmission {
    /// Arguments satisfy the advertised schema; the call may execute.
    Valid,
    /// Arguments violate the advertised schema. This is model-correctable: the
    /// gate completes the call with this usage message instead of executing it,
    /// so the model can retry within the run's bounded tool rounds.
    SchemaViolation(String),
}

/// Checks one call before it is admitted. Only a call the gate cannot answer —
/// a malformed id or a missing name — is fatal. A name that is not offered
/// (`hidden_refusal` explains a deliberately withheld one) and arguments that
/// are not an object are slips the model corrects from the usage message.
pub(super) fn validate_call_shape(
    descriptors: &[Value],
    call_id: Option<&str>,
    name: &str,
    arguments: &Value,
    hidden_refusal: impl FnOnce(&str) -> Option<String>,
) -> Result<CallAdmission, String> {
    if call_id.is_some_and(|id| id.trim().is_empty() || id.len() > 256) {
        return Err("provider returned an invalid tool-call id".to_string());
    }
    if name.trim().is_empty() || name.len() > 128 {
        return Err("provider returned an invalid tool name".to_string());
    }
    let Some(schema) = descriptors
        .iter()
        .find(|descriptor| descriptor.get("name").and_then(Value::as_str) == Some(name))
        .and_then(|descriptor| descriptor.get("inputSchema"))
    else {
        return Ok(CallAdmission::SchemaViolation(
            hidden_refusal(name).unwrap_or_else(|| unknown_tool_message(descriptors, name)),
        ));
    };
    if !arguments.is_object() {
        return Ok(CallAdmission::SchemaViolation(format!(
            "invalid_arguments: `{name}` takes its arguments as a JSON object matching its input schema, not as {}. Nothing was executed; call it again with an object.",
            json_kind(arguments)
        )));
    }
    match crate::tools::schema_violation_message(name, schema, arguments) {
        Ok(None) => Ok(CallAdmission::Valid),
        Ok(Some(message)) => Ok(CallAdmission::SchemaViolation(message)),
        // An uncompilable registry schema is our defect, not model-correctable.
        Err(_) => Err(format!("tool '{name}' has an invalid input schema")),
    }
}

/// The usage message for a name no descriptor carries. A model that wraps an
/// offered name in a namespace (`mcp__eud-tools__search_docs`,
/// `functions.search_docs`) or changes its case is told the exact name.
fn unknown_tool_message(descriptors: &[Value], name: &str) -> String {
    let bare = name
        .rsplit(['.', '/', ':'])
        .next()
        .and_then(|name| name.rsplit("__").next())
        .unwrap_or(name);
    let suggestion = descriptors
        .iter()
        .filter_map(|descriptor| descriptor.get("name").and_then(Value::as_str))
        .find(|offered| offered.eq_ignore_ascii_case(bare))
        .map(|offered| format!(" Did you mean `{offered}`? Call it by that exact name."))
        .unwrap_or_else(|| {
            " Call only a tool from the advertised list, by its exact name.".to_string()
        });
    format!(
        "unknown_tool: `{name}` is not a tool in this session. Nothing was executed.{suggestion}"
    )
}

fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}
