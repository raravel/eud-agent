use serde_json::Value;

pub(super) fn validate_call_shape(
    descriptors: &[Value],
    call_id: Option<&str>,
    name: &str,
    arguments: &Value,
) -> Result<(), String> {
    if call_id.is_some_and(|id| id.trim().is_empty() || id.len() > 256) {
        return Err("provider returned an invalid tool-call id".to_string());
    }
    if name.trim().is_empty() || name.len() > 128 {
        return Err("provider returned an invalid tool name".to_string());
    }
    if !arguments.is_object() {
        return Err("provider returned non-object tool arguments".to_string());
    }
    let schema = descriptors
        .iter()
        .find(|descriptor| descriptor.get("name").and_then(Value::as_str) == Some(name))
        .and_then(|descriptor| descriptor.get("inputSchema"))
        .ok_or_else(|| format!("unknown tool '{name}'"))?;
    let validator = jsonschema::JSONSchema::options()
        .with_draft(jsonschema::Draft::Draft7)
        .compile(schema)
        .map_err(|_| format!("tool '{name}' has an invalid input schema"))?;
    validator
        .validate(arguments)
        .map_err(|_| format!("tool '{name}' arguments do not match its input schema"))
}
