use super::super::definitions::E2Definition;
use super::super::{E2RawValue, E2TriggerNode};

pub(super) fn value_expression(raw_value: &E2RawValue) -> String {
    value("", Some(raw_value))
}

pub(super) fn raw(node: &E2TriggerNode, index: usize) -> &str {
    match node.values.get(index) {
        Some(E2RawValue::String(value)) => value,
        _ => "0",
    }
}

pub(super) fn arg(node: &E2TriggerNode, definition: &E2Definition, index: usize) -> String {
    let kind = definition.value_kinds.get(index).copied().unwrap_or("");
    value(kind, node.values.get(index))
}

fn value(kind: &str, input: Option<&E2RawValue>) -> String {
    let source = match input {
        None | Some(E2RawValue::Null) => return "0".to_string(),
        Some(E2RawValue::Bool(value)) => return if *value { "true" } else { "false" }.to_string(),
        Some(E2RawValue::Integer(value)) => value.to_string(),
        Some(E2RawValue::String(value)) => value.clone(),
        Some(E2RawValue::List(items)) => {
            return format!(
                "[{}]",
                items
                    .iter()
                    .map(|item| value("", Some(item)))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        Some(E2RawValue::Object(fields)) => {
            return serde_json::to_string(fields).unwrap_or_else(|_| "{}".to_string())
        }
    };
    match (kind, source.as_str()) {
        ("PlayerX", "13") => "getcurpl()".to_string(),
        ("VariableModifier", "0") => "=".to_string(),
        ("VariableModifier", "1") => "+=".to_string(),
        ("VariableModifier", "2") => "-=".to_string(),
        ("VariableModifier", "3") => "*=".to_string(),
        ("VariableModifier", "4") => "/=".to_string(),
        ("VariableComparison", "0") => "==".to_string(),
        ("VariableComparison", "1") => "!=".to_string(),
        ("VariableComparison", "2") => ">=".to_string(),
        ("VariableComparison", "3") => "<=".to_string(),
        ("VariableComparison", "4") => ">".to_string(),
        ("VariableComparison", "5") => "<".to_string(),
        ("AlwaysDisplay", "1") => "4".to_string(),
        ("Comparison", "2") => "10".to_string(),
        ("Modifier" | "TimeModifier", "0") => "7".to_string(),
        ("Modifier" | "TimeModifier", "1") => "8".to_string(),
        ("Modifier" | "TimeModifier", "2") => "9".to_string(),
        ("SState", "0") | ("State", "0") => "4".to_string(),
        ("SState", "1") | ("State", "1") => "5".to_string(),
        ("SState", "2") | ("State", "2") => "6".to_string(),
        ("SState", "3") => "11".to_string(),
        ("CState", "0") => "2".to_string(),
        ("CState", "1") => "3".to_string(),
        ("ScoreOffset", value) => score_offset(value).unwrap_or(value).to_string(),
        ("DatFile", value) => dat_file(value).unwrap_or(value).to_string(),
        ("Text" | "WAVName" | "Label" | "ScenarioName" | "CText" | "DBText", value) => {
            serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
        }
        _ => source,
    }
}

fn score_offset(value: &str) -> Option<&'static str> {
    const VALUES: [&str; 19] = [
        "0x581DE4", "0x581E14", "0x581E44", "0x581E74", "0x581EA4", "0x581ED4", "0x581F04",
        "0x581F34", "0x581F64", "0x581F94", "0x581FC4", "0x581FF4", "0x582024", "0x582054",
        "0x582084", "0x5820B4", "0x5820E4", "0x582114", "0x5822F4",
    ];
    value
        .parse::<usize>()
        .ok()
        .and_then(|index| VALUES.get(index).copied())
}

fn dat_file(value: &str) -> Option<&'static str> {
    const VALUES: [&str; 8] = [
        "0x65FD00", "0x6564E0", "0x6C9858", "0x665AC0", "0x666778", "0x655700", "0x656198",
        "0x664A40",
    ];
    value
        .parse::<usize>()
        .ok()
        .and_then(|index| VALUES.get(index).copied())
}
