//! Strict, sparse JSON authoring model for DAT changes.
//!
//! This spike keeps the current editor bridge as the apply backend. Codex can edit one
//! coherent JSON document, then Rust validates the complete document and compiles its semantic
//! delta into the existing journaled DAT tool calls.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const DAT_PROJECT_SCHEMA_VERSION: u32 = 1;

const DAT_TABLES: &[&str] = &[
    "units", "weapons", "flingy", "sprites", "images", "upgrades", "techdata", "orders",
    "portdata", "sfxdata",
];
const XDAT_FIELDS: &[(&str, &[&str])] = &[
    ("statusinfor", &["Status", "Display", "Joint"]),
    ("wireframe", &["wire", "grp", "tran"]),
    ("ButtonSet", &["ButtonSet"]),
];
const REQUIREMENT_TABLES: &[&str] = &["units", "upgrades", "techdata", "Stechdata", "orders"];

pub type NumericFields = BTreeMap<String, i64>;
pub type NumericObjects = BTreeMap<u32, NumericFields>;
pub type NumericTables = BTreeMap<String, NumericObjects>;
pub type TextObjects = BTreeMap<u32, String>;
pub type TextTables = BTreeMap<String, TextObjects>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatProject {
    pub schema_version: u32,
    #[serde(default)]
    pub dat: NumericTables,
    #[serde(default)]
    pub xdat: NumericTables,
    #[serde(default)]
    pub tbl: TextObjects,
    #[serde(default)]
    pub requirements: TextTables,
    #[serde(default)]
    pub buttons: TextObjects,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolInvocation {
    pub tool: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatChange {
    pub pointer: String,
    pub before: Option<Value>,
    pub after: Option<Value>,
    pub invocation: ToolInvocation,
}

impl DatProject {
    pub fn parse_json(source: &str) -> Result<Self, String> {
        let project: Self = serde_json::from_str(source)
            .map_err(|error| format!("invalid DAT project JSON: {error}"))?;
        project.validate()?;
        Ok(project)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != DAT_PROJECT_SCHEMA_VERSION {
            return Err(format!(
                "unsupported DAT project schemaVersion {} (expected {DAT_PROJECT_SCHEMA_VERSION})",
                self.schema_version
            ));
        }

        validate_numeric_tables("dat", &self.dat, DAT_TABLES, |_| None)?;
        validate_numeric_tables("xdat", &self.xdat, &xdat_table_names(), |table| {
            xdat_fields(table)
        })?;
        validate_text_tables("requirements", &self.requirements, REQUIREMENT_TABLES)?;

        for (table, objects) in &self.requirements {
            for (obj_id, payload) in objects {
                validate_requirement_payload(payload)
                    .map_err(|error| format!("requirements/{table}/{obj_id}: {error}"))?;
            }
        }
        for (set_id, csv) in &self.buttons {
            validate_button_csv(csv).map_err(|error| format!("buttons/{set_id}: {error}"))?;
        }
        Ok(())
    }

    /// Compile a validated sparse-document delta into the existing model-facing tool contract.
    ///
    /// DAT, XDAT, and TBL removals become `dat_reset`. Requirements and button defaults do not
    /// have a safe reset command today, so deleting those entries fails closed.
    pub fn changes_from(&self, before: &Self) -> Result<Vec<DatChange>, String> {
        before.validate()?;
        self.validate()?;

        let mut changes = Vec::new();
        compile_numeric_changes("dat", &before.dat, &self.dat, &mut changes);
        compile_numeric_changes("xdat", &before.xdat, &self.xdat, &mut changes);
        compile_tbl_changes(&before.tbl, &self.tbl, &mut changes);
        compile_requirement_changes(&before.requirements, &self.requirements, &mut changes)?;
        compile_button_changes(&before.buttons, &self.buttons, &mut changes)?;
        Ok(changes)
    }
}

fn validate_numeric_tables<F>(
    section: &str,
    tables: &NumericTables,
    allowed_tables: &[&str],
    allowed_fields: F,
) -> Result<(), String>
where
    F: Fn(&str) -> Option<&'static [&'static str]>,
{
    for (table, objects) in tables {
        if !allowed_tables.contains(&table.as_str()) {
            return Err(format!("{section}: unknown table '{table}'"));
        }
        let fields = allowed_fields(table);
        for (obj_id, values) in objects {
            if values.is_empty() {
                return Err(format!("{section}/{table}/{obj_id}: object has no fields"));
            }
            for property in values.keys() {
                validate_component(property, "property")
                    .map_err(|error| format!("{section}/{table}/{obj_id}: {error}"))?;
                if let Some(fields) = fields {
                    if !fields.contains(&property.as_str()) {
                        return Err(format!(
                            "{section}/{table}/{obj_id}: unknown field '{property}'"
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

fn validate_text_tables(
    section: &str,
    tables: &TextTables,
    allowed_tables: &[&str],
) -> Result<(), String> {
    for table in tables.keys() {
        if !allowed_tables.contains(&table.as_str()) {
            return Err(format!("{section}: unknown table '{table}'"));
        }
    }
    Ok(())
}

fn validate_component(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{label} must not be empty"));
    }
    if value.contains(['|', '\r', '\n']) {
        return Err(format!("{label} contains a bridge command delimiter"));
    }
    Ok(())
}

fn validate_requirement_payload(payload: &str) -> Result<(), String> {
    let first = payload.split('.').next().unwrap_or_default();
    let mode = first
        .parse::<u8>()
        .map_err(|_| "payload first segment must be numeric (0-4)".to_string())?;
    if mode > 4 {
        return Err("payload first segment must be numeric (0-4)".to_string());
    }
    Ok(())
}

fn validate_button_csv(csv: &str) -> Result<(), String> {
    if csv.is_empty() {
        return Err("CSV must not be empty".to_string());
    }
    for (group_index, group) in csv.split('.').enumerate() {
        let fields: Vec<_> = group.split(',').collect();
        if fields.len() < 8 {
            return Err(format!(
                "button {} needs at least 8 numeric fields",
                group_index + 1
            ));
        }
        for (field_index, field) in fields.iter().take(8).enumerate() {
            field.parse::<i64>().map_err(|_| {
                format!(
                    "button {} field {} is not an integer",
                    group_index + 1,
                    field_index + 1
                )
            })?;
        }
    }
    Ok(())
}

fn xdat_table_names() -> Vec<&'static str> {
    XDAT_FIELDS.iter().map(|(table, _)| *table).collect()
}

fn xdat_fields(table: &str) -> Option<&'static [&'static str]> {
    XDAT_FIELDS
        .iter()
        .find_map(|(candidate, fields)| (*candidate == table).then_some(*fields))
}

fn compile_numeric_changes(
    section: &str,
    before: &NumericTables,
    after: &NumericTables,
    changes: &mut Vec<DatChange>,
) {
    for table in union_keys(before, after) {
        let before_objects = before.get(&table);
        let after_objects = after.get(&table);
        for obj_id in union_optional_keys(before_objects, after_objects) {
            let before_fields = before_objects.and_then(|objects| objects.get(&obj_id));
            let after_fields = after_objects.and_then(|objects| objects.get(&obj_id));
            for property in union_optional_keys(before_fields, after_fields) {
                let old = before_fields
                    .and_then(|fields| fields.get(&property))
                    .copied();
                let new = after_fields
                    .and_then(|fields| fields.get(&property))
                    .copied();
                if old == new {
                    continue;
                }
                let pointer = format!(
                    "/{}/{}/{}/{}",
                    section,
                    escape_pointer(&table),
                    obj_id,
                    escape_pointer(&property)
                );
                let invocation = match new {
                    Some(value) if section == "dat" => ToolInvocation {
                        tool: "dat_set".to_string(),
                        arguments: json!({
                            "dat": table,
                            "param": property,
                            "objId": obj_id,
                            "value": value,
                        }),
                    },
                    Some(value) => ToolInvocation {
                        tool: "xdat_set".to_string(),
                        arguments: json!({
                            "dat": table,
                            "name": property,
                            "objId": obj_id,
                            "value": value,
                        }),
                    },
                    None => ToolInvocation {
                        tool: "dat_reset".to_string(),
                        arguments: json!({
                            "kind": section,
                            "dat": table,
                            "param": property,
                            "objId": obj_id,
                        }),
                    },
                };
                changes.push(DatChange {
                    pointer,
                    before: old.map(Value::from),
                    after: new.map(Value::from),
                    invocation,
                });
            }
        }
    }
}

fn compile_tbl_changes(before: &TextObjects, after: &TextObjects, changes: &mut Vec<DatChange>) {
    for index in union_keys(before, after) {
        let old = before.get(&index);
        let new = after.get(&index);
        if old == new {
            continue;
        }
        let invocation = match new {
            Some(value) => ToolInvocation {
                tool: "tbl_set".to_string(),
                arguments: json!({"index": index, "value": value}),
            },
            None => ToolInvocation {
                tool: "dat_reset".to_string(),
                arguments: json!({"kind": "tbl", "objId": index}),
            },
        };
        changes.push(DatChange {
            pointer: format!("/tbl/{index}"),
            before: old.cloned().map(Value::String),
            after: new.cloned().map(Value::String),
            invocation,
        });
    }
}

fn compile_requirement_changes(
    before: &TextTables,
    after: &TextTables,
    changes: &mut Vec<DatChange>,
) -> Result<(), String> {
    for table in union_keys(before, after) {
        let before_objects = before.get(&table);
        let after_objects = after.get(&table);
        for obj_id in union_optional_keys(before_objects, after_objects) {
            let old = before_objects.and_then(|objects| objects.get(&obj_id));
            let new = after_objects.and_then(|objects| objects.get(&obj_id));
            if old == new {
                continue;
            }
            let Some(payload) = new else {
                return Err(format!(
                    "/requirements/{}/{obj_id}: deletion has no safe reset operation",
                    escape_pointer(&table)
                ));
            };
            changes.push(DatChange {
                pointer: format!("/requirements/{}/{}", escape_pointer(&table), obj_id),
                before: old.cloned().map(Value::String),
                after: Some(Value::String(payload.clone())),
                invocation: ToolInvocation {
                    tool: "req_set".to_string(),
                    arguments: json!({"dat": table, "objId": obj_id, "payload": payload}),
                },
            });
        }
    }
    Ok(())
}

fn compile_button_changes(
    before: &TextObjects,
    after: &TextObjects,
    changes: &mut Vec<DatChange>,
) -> Result<(), String> {
    for set_id in union_keys(before, after) {
        let old = before.get(&set_id);
        let new = after.get(&set_id);
        if old == new {
            continue;
        }
        let Some(csv) = new else {
            return Err(format!(
                "/buttons/{set_id}: deletion has no safe reset operation"
            ));
        };
        changes.push(DatChange {
            pointer: format!("/buttons/{set_id}"),
            before: old.cloned().map(Value::String),
            after: Some(Value::String(csv.clone())),
            invocation: ToolInvocation {
                tool: "btn_set".to_string(),
                arguments: json!({"setId": set_id, "csv": csv}),
            },
        });
    }
    Ok(())
}

fn union_keys<K, V>(left: &BTreeMap<K, V>, right: &BTreeMap<K, V>) -> BTreeSet<K>
where
    K: Ord + Clone,
{
    left.keys().chain(right.keys()).cloned().collect()
}

fn union_optional_keys<K, V>(
    left: Option<&BTreeMap<K, V>>,
    right: Option<&BTreeMap<K, V>>,
) -> BTreeSet<K>
where
    K: Ord + Clone,
{
    left.into_iter()
        .flat_map(BTreeMap::keys)
        .chain(right.into_iter().flat_map(BTreeMap::keys))
        .cloned()
        .collect()
}

fn escape_pointer(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
mod tests {
    use super::DatProject;

    const BEFORE: &str = r#"{
      "schemaVersion": 1,
      "dat": {
        "units": {"0": {"Hit Points": 10240}},
        "weapons": {"0": {"Damage Amount": 6}}
      },
      "xdat": {"wireframe": {"0": {"wire": 0}}},
      "tbl": {"0": "Terran Marine"},
      "requirements": {"units": {"0": "0"}},
      "buttons": {"0": "1,2,3,4,5,6,7,8"}
    }"#;

    const AFTER: &str = r#"{
      "schemaVersion": 1,
      "dat": {
        "units": {"0": {"Hit Points": 20480}},
        "weapons": {"0": {"Damage Amount": 12}}
      },
      "xdat": {"wireframe": {"0": {"wire": 1}}},
      "tbl": {"0": "정예 해병"},
      "requirements": {"units": {"0": "3"}},
      "buttons": {"0": "1,2,3,4,5,6,9,10"}
    }"#;

    #[test]
    fn parses_risk_representative_sparse_document() {
        let project = DatProject::parse_json(BEFORE).unwrap();
        assert_eq!(project.dat["units"][&0]["Hit Points"], 10240);
        assert_eq!(project.tbl[&0], "Terran Marine");
    }

    #[test]
    fn compiles_semantic_delta_to_existing_tool_contracts() {
        let before = DatProject::parse_json(BEFORE).unwrap();
        let after = DatProject::parse_json(AFTER).unwrap();
        let changes = after.changes_from(&before).unwrap();

        assert_eq!(changes.len(), 6);
        assert_eq!(
            changes
                .iter()
                .map(|change| change.invocation.tool.as_str())
                .collect::<Vec<_>>(),
            vec!["dat_set", "dat_set", "xdat_set", "tbl_set", "req_set", "btn_set"]
        );
        assert_eq!(changes[0].pointer, "/dat/units/0/Hit Points");
        assert_eq!(changes[0].invocation.arguments["value"], 20480);
        assert_eq!(changes[3].invocation.arguments["value"], "정예 해병");
    }

    #[test]
    fn deletion_uses_reset_only_where_bridge_supports_it() {
        let before = DatProject::parse_json(BEFORE).unwrap();
        let mut after = before.clone();
        after
            .dat
            .get_mut("units")
            .unwrap()
            .get_mut(&0)
            .unwrap()
            .clear();
        assert!(after
            .validate()
            .unwrap_err()
            .contains("object has no fields"));

        let mut after = before.clone();
        after.tbl.remove(&0);
        let changes = after.changes_from(&before).unwrap();
        let reset = changes
            .iter()
            .find(|change| change.pointer == "/tbl/0")
            .unwrap();
        assert_eq!(reset.invocation.tool, "dat_reset");

        let mut after = before.clone();
        after.buttons.remove(&0);
        assert!(after
            .changes_from(&before)
            .unwrap_err()
            .contains("no safe reset operation"));
    }

    #[test]
    fn rejects_unknown_or_dangerous_shapes() {
        assert!(
            DatProject::parse_json(r#"{"schemaVersion":1,"dat":{"unknown":{"0":{"x":1}}}}"#)
                .unwrap_err()
                .contains("unknown table")
        );
        assert!(DatProject::parse_json(
            r#"{"schemaVersion":1,"requirements":{"units":{"0":"Default"}}}"#
        )
        .unwrap_err()
        .contains("numeric (0-4)"));
        assert!(
            DatProject::parse_json(r#"{"schemaVersion":1,"buttons":{"0":"1,2,3"}}"#)
                .unwrap_err()
                .contains("at least 8")
        );
        assert!(
            DatProject::parse_json(r#"{"schemaVersion":1,"extra":true}"#)
                .unwrap_err()
                .contains("unknown field")
        );
    }
}
