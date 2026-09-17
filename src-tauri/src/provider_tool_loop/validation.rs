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

pub(super) fn validate_call_shape(
    descriptors: &[Value],
    call_id: Option<&str>,
    name: &str,
    arguments: &Value,
) -> Result<CallAdmission, String> {
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
    match crate::tools::schema_violation_message(name, schema, arguments) {
        Ok(None) => Ok(CallAdmission::Valid),
        Ok(Some(message)) => Ok(CallAdmission::SchemaViolation(message)),
        // An uncompilable registry schema is our defect, not model-correctable.
        Err(_) => Err(format!("tool '{name}' has an invalid input schema")),
    }
}
