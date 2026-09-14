use serde_json::{json, Value};

pub(super) fn normalize_cca_parameters(schema: &Value) -> Value {
    let mut normalized = normalize_cca_schema(schema);
    let Some(object) = normalized.as_object_mut() else {
        return json!({"type":"object","properties":{}});
    };
    if object.get("type").and_then(Value::as_str) != Some("object") {
        return json!({"type":"object","properties":{}});
    }
    object
        .entry("properties")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    normalized
}

fn normalize_cca_schema(schema: &Value) -> Value {
    let branches = normalize_cca_branches(schema)
        .into_iter()
        .map(finalize_cca_object)
        .collect::<Vec<_>>();
    if let [branch] = branches.as_slice() {
        return Value::Object(branch.clone());
    }
    let object_union = branches
        .iter()
        .all(|branch| branch.get("type").and_then(Value::as_str) == Some("object"));
    let mut normalized = serde_json::Map::new();
    if object_union {
        normalized.insert("type".to_string(), Value::String("object".to_string()));
        normalized.insert(
            "properties".to_string(),
            Value::Object(serde_json::Map::new()),
        );
    }
    normalized.insert(
        "anyOf".to_string(),
        Value::Array(branches.into_iter().map(Value::Object).collect()),
    );
    Value::Object(normalized)
}

fn normalize_cca_branches(schema: &Value) -> Vec<serde_json::Map<String, Value>> {
    let Some(source) = schema.as_object() else {
        return vec![serde_json::Map::new()];
    };
    let mut normalized = serde_json::Map::new();
    if let Some(schema_type) = normalized_cca_type(source.get("type")) {
        normalized.insert("type".to_string(), Value::String(schema_type.to_string()));
    }
    if let Some(description) = source.get("description").and_then(Value::as_str) {
        normalized.insert(
            "description".to_string(),
            Value::String(description.to_string()),
        );
    }
    if let Some(reference) = source.get("$ref").and_then(Value::as_str) {
        let reference = reference
            .strip_prefix("#/definitions/")
            .or_else(|| reference.strip_prefix("#/$defs/"))
            .map_or_else(|| reference.to_string(), |name| format!("#/defs/{name}"));
        normalized.insert("ref".to_string(), Value::String(reference));
    }
    if let Some(values) = source.get("enum").and_then(Value::as_array) {
        let strings = values
            .iter()
            .filter(|value| value.is_string())
            .cloned()
            .collect::<Vec<_>>();
        if !strings.is_empty() {
            normalized.insert("enum".to_string(), Value::Array(strings));
        }
    } else if let Some(value) = source.get("const").filter(|value| value.is_string()) {
        normalized.insert("enum".to_string(), Value::Array(vec![value.clone()]));
    }
    if let Some(properties) = source.get("properties").and_then(Value::as_object) {
        normalized.insert(
            "properties".to_string(),
            Value::Object(
                properties
                    .iter()
                    .map(|(name, property)| (name.clone(), normalize_cca_schema(property)))
                    .collect(),
            ),
        );
    }
    if let Some(definitions) = source
        .get("$defs")
        .or_else(|| source.get("definitions"))
        .and_then(Value::as_object)
    {
        normalized.insert(
            "defs".to_string(),
            Value::Object(
                definitions
                    .iter()
                    .map(|(name, definition)| (name.clone(), normalize_cca_schema(definition)))
                    .collect(),
            ),
        );
    }
    if let Some(required) = source.get("required").and_then(Value::as_array) {
        normalized.insert(
            "required".to_string(),
            Value::Array(
                required
                    .iter()
                    .filter(|name| name.is_string())
                    .cloned()
                    .collect(),
            ),
        );
    }
    if let Some(items) = source.get("items") {
        normalized.insert("items".to_string(), normalize_cca_schema(items));
    }
    let mut branches = vec![normalized];
    if let Some(variants) = source
        .get("oneOf")
        .or_else(|| source.get("anyOf"))
        .and_then(Value::as_array)
    {
        let alternatives = variants
            .iter()
            .flat_map(normalize_cca_branches)
            .collect::<Vec<_>>();
        branches = intersect_cca_branches(&branches, &alternatives);
    }
    if let Some(members) = source.get("allOf").and_then(Value::as_array) {
        for member in members {
            branches = intersect_cca_branches(&branches, &normalize_cca_branches(member));
        }
    }
    branches
}

fn finalize_cca_object(
    mut normalized: serde_json::Map<String, Value>,
) -> serde_json::Map<String, Value> {
    if normalized.get("type").and_then(Value::as_str) == Some("object") {
        normalized
            .entry("properties")
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        if let Some(Value::Array(mut required)) = normalized.remove("required") {
            let Some(properties) = normalized.get("properties").and_then(Value::as_object) else {
                return normalized;
            };
            required.retain(|name| {
                name.as_str()
                    .is_some_and(|name| properties.contains_key(name))
            });
            required.dedup();
            if !required.is_empty() {
                normalized.insert("required".to_string(), Value::Array(required));
            }
        }
    }
    normalized
}

fn normalized_cca_type(value: Option<&Value>) -> Option<&str> {
    let supported = |value| {
        matches!(
            value,
            "object" | "array" | "string" | "number" | "integer" | "boolean"
        )
    };
    match value {
        Some(Value::String(value)) if supported(value) => Some(value),
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .find(|value| *value != "null" && supported(value)),
        _ => None,
    }
}

fn intersect_cca_branches(
    left: &[serde_json::Map<String, Value>],
    right: &[serde_json::Map<String, Value>],
) -> Vec<serde_json::Map<String, Value>> {
    let mut intersections = Vec::new();
    for left_branch in left {
        for right_branch in right {
            let mut intersection = left_branch.clone();
            if merge_cca_objects(&mut intersection, right_branch) {
                intersections.push(intersection);
            }
        }
    }
    intersections
}

fn merge_cca_objects(
    target: &mut serde_json::Map<String, Value>,
    source: &serde_json::Map<String, Value>,
) -> bool {
    for (name, source_value) in source {
        let Some(target_value) = target.get_mut(name) else {
            target.insert(name.clone(), source_value.clone());
            continue;
        };
        if target_value == source_value {
            continue;
        }
        match (name.as_str(), target_value, source_value) {
            ("required", Value::Array(target), Value::Array(source)) => {
                for value in source {
                    if !target.contains(value) {
                        target.push(value.clone());
                    }
                }
            }
            ("enum", Value::Array(target), Value::Array(source)) => {
                target.retain(|value| source.contains(value));
                if target.is_empty() {
                    return false;
                }
            }
            (_, Value::Object(target), Value::Object(source)) => {
                if !merge_cca_objects(target, source) {
                    return false;
                }
            }
            ("description", Value::String(target), Value::String(source)) => {
                target.push('\n');
                target.push_str(source);
            }
            _ => return false,
        }
    }
    true
}
