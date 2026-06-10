//! Tool-layer safety rails and per-request evidence state.
//!
//! The functions here are small, deterministic backstops for crash-critical
//! first principles and the EUD-090 evidence requirement.

use encoding_rs::EUC_KR;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

/// Durable project-memory write tool name, exempt from the evidence gate.
pub const MEMORY_WRITE_TOOL: &str = "memory_write";

/// Build verification tool name, exempt from the evidence gate.
pub const BUILD_RUN_TOOL: &str = "build_run";

/// Documentation search tool name.
pub const SEARCH_DOCS_TOOL: &str = "search_docs";

/// Connected source-map digest tool name.
pub const MAP_INFO_TOOL: &str = "map_info";

/// Result type used by tool-layer validation and gate checks.
pub type ToolResult<T> = Result<T, ToolError>;

/// Tool-layer errors surfaced as correctable tool-call failures.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ToolError {
    /// A mutating tool was called before `search_docs` ran in this request.
    #[error("{message}")]
    EvidenceRequired { message: String },

    /// A `btn_set` CSV contains a disableable button with `disstr == 0`.
    #[error("{message}")]
    ButtonDisableStringRequired { message: String },

    /// An `xdat_set` attempts to reassign a unit's ButtonSet to a different id.
    #[error("{message}")]
    ButtonSetReassign { message: String },

    /// A tool call failed registry, budget, or admission validation.
    #[error("{message}")]
    AdmissionRejected { message: String },
}

/// Mutable state carried for one agent request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestState {
    /// Stable id for the currently admitted request.
    pub request_id: String,

    /// Set once a `search_docs` call has run successfully, even with zero hits.
    pub docs_searched: bool,

    /// Set once a proposed plan has been approved for this request.
    pub plan_approved: bool,

    /// Number of admitted tool actions in this request.
    pub action_count: usize,

    /// Number of admitted mutating tool actions in this request.
    pub mutation_count: usize,

    /// Number of admitted build self-fix attempts in this request.
    pub build_fix_attempts: usize,
}

impl RequestState {
    /// Create request state with the evidence search flag unset.
    pub fn new() -> Self {
        Self::for_request("")
    }

    /// Create clean request state for a specific request id.
    pub fn for_request(id: &str) -> Self {
        Self {
            request_id: id.to_string(),
            docs_searched: false,
            plan_approved: false,
            action_count: 0,
            mutation_count: 0,
            build_fix_attempts: 0,
        }
    }

    /// Start a fresh request, resetting all per-request gates and budgets.
    pub fn start_request(&mut self, id: &str) {
        *self = Self::for_request(id);
    }

    /// Record that `search_docs` ran successfully for this request.
    ///
    /// The execution layer calls this after a successful search; admission only
    /// validates the call and must not mark the evidence gate satisfied.
    pub fn record_search_docs(&mut self) {
        self.docs_searched = true;
    }

    /// Approve the current request plan, lifting the mutation gate.
    pub fn approve_plan(&mut self) {
        self.plan_approved = true;
    }
}

impl Default for RequestState {
    fn default() -> Self {
        Self::new()
    }
}

/// Minimal tool metadata needed by the evidence gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub mutating: bool,
    pub input_schema: Value,
}

impl ToolSpec {
    /// Construct a tool spec for a mutating tool.
    pub fn mutating(name: &'static str) -> Self {
        Self {
            name,
            description: "",
            mutating: true,
            input_schema: empty_schema(),
        }
    }

    /// Construct a tool spec for a read-only tool.
    pub fn read_only(name: &'static str) -> Self {
        Self {
            name,
            description: "",
            mutating: false,
            input_schema: empty_schema(),
        }
    }
}

fn tool_spec(
    name: &'static str,
    description: &'static str,
    mutating: bool,
    input_schema: Value,
) -> ToolSpec {
    ToolSpec {
        name,
        description,
        mutating,
        input_schema,
    }
}

fn empty_schema() -> Value {
    schema(json!({}), &[])
}

fn schema(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn string_schema() -> Value {
    json!({"type": "string"})
}

fn integer_schema() -> Value {
    json!({"type": "integer"})
}

fn numeric_value_schema() -> Value {
    json!({"type": ["integer", "string"]})
}

fn integer_or_string_schema() -> Value {
    json!({"type": ["integer", "string"], "x-eud-allowAnyString": true})
}

fn enum_string_schema(values: &[&str]) -> Value {
    json!({"type": "string", "enum": values})
}

fn dat_names_schema() -> Value {
    enum_string_schema(&[
        "units", "weapons", "flingy", "sprites", "images", "upgrades", "techdata", "orders",
        "portdata", "sfxdata",
    ])
}

fn xdat_kinds_schema() -> Value {
    enum_string_schema(&["statusinfor", "wireframe", "ButtonSet"])
}

fn req_dats_schema() -> Value {
    enum_string_schema(&["units", "upgrades", "techdata", "Stechdata", "orders"])
}

fn settings_scopes_schema() -> Value {
    enum_string_schema(&["project", "program"])
}

fn map_info_owner_schema() -> Value {
    enum_string_schema(&[
        "P1", "P2", "P3", "P4", "P5", "P6", "P7", "P8", "P9", "P10", "P11", "P12", "neutral",
    ])
}

/// Registry of the EUD tool API exposed to Codex and MCP.
pub fn tool_registry() -> Vec<ToolSpec> {
    vec![
        tool_spec(
            "project_status",
            "Read current project status.",
            false,
            empty_schema(),
        ),
        tool_spec(
            "list_files",
            "List editable project files.",
            false,
            empty_schema(),
        ),
        tool_spec(
            "read_file",
            "Read an editable project file.",
            false,
            schema(json!({"path": string_schema()}), &["path"]),
        ),
        tool_spec(
            "dat_get",
            "Read a DAT field value.",
            false,
            schema(
                json!({
                    "dat": dat_names_schema(),
                    "param": string_schema(),
                    "objId": integer_schema(),
                }),
                &["dat", "param", "objId"],
            ),
        ),
        tool_spec(
            "xdat_get",
            "Read an extended DAT field value.",
            false,
            schema(
                json!({
                    "dat": xdat_kinds_schema(),
                    "name": string_schema(),
                    "objId": integer_schema(),
                }),
                &["dat", "name", "objId"],
            ),
        ),
        tool_spec(
            "tbl_get",
            "Read a TBL string by index.",
            false,
            schema(json!({"index": integer_schema()}), &["index"]),
        ),
        tool_spec(
            "req_get",
            "Read a requirements payload.",
            false,
            schema(
                json!({
                    "dat": req_dats_schema(),
                    "objId": integer_schema(),
                }),
                &["dat", "objId"],
            ),
        ),
        tool_spec(
            "btn_get",
            "Read a button set CSV payload.",
            false,
            schema(json!({"setId": integer_schema()}), &["setId"]),
        ),
        tool_spec(
            "settings_get",
            "Read an agent setting.",
            false,
            schema(
                json!({
                    "scope": settings_scopes_schema(),
                    "key": string_schema(),
                }),
                &["scope", "key"],
            ),
        ),
        tool_spec(
            MAP_INFO_TOOL,
            "Read connected source-map locations, units, players, and forces.",
            false,
            schema(
                json!({
                    "mode": enum_string_schema(&["summary", "locations", "units", "players"]),
                    "owner": map_info_owner_schema(),
                    "unitType": integer_or_string_schema(),
                }),
                &[],
            ),
        ),
        tool_spec(
            "plugins_list",
            "List configured plugins.",
            false,
            empty_schema(),
        ),
        tool_spec(
            "build_errors",
            "Read the latest build errors.",
            false,
            empty_schema(),
        ),
        tool_spec(
            SEARCH_DOCS_TOOL,
            "Search the project reference corpus.",
            false,
            schema(
                json!({
                    "query": string_schema(),
                    "k": integer_schema(),
                }),
                &["query"],
            ),
        ),
        tool_spec(
            "dat_set",
            "Write a DAT field value.",
            true,
            schema(
                json!({
                    "dat": dat_names_schema(),
                    "param": string_schema(),
                    "objId": integer_schema(),
                    "value": numeric_value_schema(),
                }),
                &["dat", "param", "objId", "value"],
            ),
        ),
        tool_spec(
            "xdat_set",
            "Write an extended DAT field value.",
            true,
            schema(
                json!({
                    "dat": xdat_kinds_schema(),
                    "name": string_schema(),
                    "objId": integer_schema(),
                    "value": numeric_value_schema(),
                }),
                &["dat", "name", "objId", "value"],
            ),
        ),
        tool_spec(
            "tbl_set",
            "Write a TBL string value.",
            true,
            schema(
                json!({
                    "index": integer_schema(),
                    "value": string_schema(),
                }),
                &["index", "value"],
            ),
        ),
        tool_spec(
            "req_set",
            "Write a requirements payload.",
            true,
            schema(
                json!({
                    "dat": req_dats_schema(),
                    "objId": integer_schema(),
                    "payload": string_schema(),
                }),
                &["dat", "objId", "payload"],
            ),
        ),
        tool_spec(
            "btn_set",
            "Write a button set CSV payload.",
            true,
            schema(
                json!({
                    "setId": integer_schema(),
                    "csv": string_schema(),
                }),
                &["setId", "csv"],
            ),
        ),
        tool_spec(
            "dat_reset",
            "Reset a DAT, XDAT, or TBL value.",
            true,
            schema(
                json!({
                    "kind": enum_string_schema(&["dat", "xdat", "tbl"]),
                    "dat": string_schema(),
                    "param": string_schema(),
                    "objId": integer_schema(),
                }),
                &["kind", "objId"],
            ),
        ),
        tool_spec(
            "file_create",
            "Create a project file.",
            true,
            schema(
                json!({
                    "path": string_schema(),
                    "ftype": enum_string_schema(&["CUIEps", "CUIPy", "RawText"]),
                    "code": string_schema(),
                }),
                &["path", "ftype"],
            ),
        ),
        tool_spec(
            "file_write",
            "Overwrite a project file.",
            true,
            schema(
                json!({
                    "path": string_schema(),
                    "code": string_schema(),
                }),
                &["path", "code"],
            ),
        ),
        tool_spec(
            "file_rename",
            "Rename a project file.",
            true,
            schema(
                json!({
                    "path": string_schema(),
                    "newname": string_schema(),
                }),
                &["path", "newname"],
            ),
        ),
        tool_spec(
            "file_delete",
            "Delete a project file.",
            true,
            schema(json!({"path": string_schema()}), &["path"]),
        ),
        tool_spec(
            "file_move",
            "Move a project file to another folder.",
            true,
            schema(
                json!({
                    "path": string_schema(),
                    "destFolder": string_schema(),
                }),
                &["path"],
            ),
        ),
        tool_spec(
            "mkdir",
            "Create a project folder.",
            true,
            schema(json!({"path": string_schema()}), &["path"]),
        ),
        tool_spec(
            "set_main",
            "Set the main project file.",
            true,
            schema(json!({"path": string_schema()}), &["path"]),
        ),
        tool_spec(
            "settings_set",
            "Write an agent setting.",
            true,
            schema(
                json!({
                    "scope": settings_scopes_schema(),
                    "key": string_schema(),
                    "value": string_schema(),
                }),
                &["scope", "key", "value"],
            ),
        ),
        tool_spec(
            "plugin_add",
            "Add a plugin entry.",
            true,
            schema(
                json!({
                    "index": integer_schema(),
                    "texts": string_schema(),
                }),
                &[],
            ),
        ),
        tool_spec(
            "plugin_edit",
            "Edit a plugin entry.",
            true,
            schema(
                json!({
                    "index": integer_schema(),
                    "texts": string_schema(),
                }),
                &["index"],
            ),
        ),
        tool_spec(
            "plugin_remove",
            "Remove a plugin entry.",
            true,
            schema(json!({"index": integer_schema()}), &["index"]),
        ),
        tool_spec(
            "plugin_move",
            "Move a plugin entry.",
            true,
            schema(
                json!({
                    "from": integer_schema(),
                    "to": integer_schema(),
                }),
                &["from", "to"],
            ),
        ),
        tool_spec(
            BUILD_RUN_TOOL,
            "Run the project build.",
            true,
            empty_schema(),
        ),
        tool_spec(
            "location_write",
            "Write map location data.",
            true,
            schema(
                json!({
                    "action": enum_string_schema(&["add", "set", "rename", "delete"]),
                    "name": string_schema(),
                    "locationId": integer_schema(),
                    "tileLeft": integer_schema(),
                    "tileTop": integer_schema(),
                    "tileRight": integer_schema(),
                    "tileBottom": integer_schema(),
                    "invertX": {"type": "boolean"},
                    "invertY": {"type": "boolean"},
                }),
                &["action"],
            ),
        ),
        tool_spec(
            "player_setup",
            "Write player start or controller data.",
            true,
            schema(
                json!({
                    "action": enum_string_schema(&[
                        "start",
                        "delstart",
                        "controller",
                    ]),
                    "player": integer_schema(),
                    "tileX": integer_schema(),
                    "tileY": integer_schema(),
                    "controller": enum_string_schema(&[
                        "human",
                        "computer",
                        "rescuable",
                        "neutral",
                        "inactive",
                        "closed",
                    ]),
                }),
                &["action", "player"],
            ),
        ),
        tool_spec(
            MEMORY_WRITE_TOOL,
            "Write durable agent memory.",
            true,
            schema(
                json!({
                    "file": enum_string_schema(&[
                        "resources",
                        "structure",
                        "conventions",
                        "lessons",
                    ]),
                    "content": string_schema(),
                }),
                &["file", "content"],
            ),
        ),
        tool_spec(
            "propose_plan",
            "Propose a plan for approval.",
            false,
            schema(json!({"markdown": string_schema()}), &["markdown"]),
        ),
    ]
}

/// Return MCP tool descriptors using each registry tool's verbatim inputSchema.
pub fn mcp_tool_descriptors() -> Vec<Value> {
    tool_registry()
        .into_iter()
        .map(|spec| {
            json!({
                "name": spec.name,
                "description": spec.description,
                "inputSchema": spec.input_schema,
            })
        })
        .collect()
}

/// Return whether a tool is exempt from the EUD-090 evidence gate.
pub fn is_evidence_gate_exempt(tool_name: &str) -> bool {
    matches!(tool_name, MEMORY_WRITE_TOOL | BUILD_RUN_TOOL)
}

/// Check whether a tool call passes the EUD-090 evidence gate.
///
/// Mutating tools are blocked on RAG-wired layers until `search_docs` has run
/// once in the request. A search with zero hits still lifts the gate.
pub fn check_evidence_gate(
    state: &RequestState,
    tool: &ToolSpec,
    rag_wired: bool,
) -> ToolResult<()> {
    if tool.mutating && !is_evidence_gate_exempt(tool.name) && rag_wired && !state.docs_searched {
        return Err(ToolError::EvidenceRequired {
            message: "evidence gate: no search_docs has run in this request. Ground the change \
first by calling search_docs with a Korean query, cite each work item's reason with its source \
link, then retry this call. A search with zero hits still lifts the gate; mark such items as \
근거 없음 instead of fabricating a source."
                .to_string(),
        });
    }

    Ok(())
}

/// Admit one tool call through argument, evidence, mutation, and budget gates.
///
/// Admission does not execute tools. In particular, successful `search_docs`
/// execution is recorded by the execution layer, not here.
pub fn admit_tool_call(state: &mut RequestState, tool: &str, args: &Value) -> ToolResult<()> {
    let spec = lookup_tool(tool)?;

    validate_tool_args(&spec, args)?;

    if state.action_count >= 30 {
        return admission_error(
            "action budget exhausted: this request is limited to 30 tool calls. Wrap up with the \
current findings instead of continuing to call tools.",
        );
    }

    check_evidence_gate(state, &spec, true)?;

    if counts_against_mutation_gate(&spec) && !state.plan_approved && state.mutation_count >= 2 {
        return admission_error(
            "mutation gate: direct changes are limited to 2 before plan approval. Call \
propose_plan with sourced steps, wait for approval, then retry the mutating tool call.",
        );
    }

    if spec.name == BUILD_RUN_TOOL && state.build_fix_attempts >= 3 {
        return admission_error(
            "build_run budget exhausted: this request is limited to 3 build self-fix attempts. \
Summarize the remaining build issue instead of running build again.",
        );
    }

    validate_first_principles(&spec, args)?;

    state.action_count += 1;
    if counts_against_mutation_gate(&spec) {
        state.mutation_count += 1;
    }
    if spec.name == BUILD_RUN_TOOL {
        state.build_fix_attempts += 1;
    }

    Ok(())
}

fn counts_against_mutation_gate(spec: &ToolSpec) -> bool {
    spec.mutating && spec.name != MEMORY_WRITE_TOOL
}

fn lookup_tool(tool: &str) -> ToolResult<ToolSpec> {
    tool_registry()
        .into_iter()
        .find(|spec| spec.name == tool)
        .ok_or_else(|| ToolError::AdmissionRejected {
            message: format!("unknown tool '{tool}'"),
        })
}

fn admission_error<T>(message: &str) -> ToolResult<T> {
    Err(ToolError::AdmissionRejected {
        message: message.to_string(),
    })
}

fn validate_tool_args(spec: &ToolSpec, args: &Value) -> ToolResult<()> {
    let Some(object) = args.as_object() else {
        return usage_error(
            spec,
            &required_args(spec),
            "arguments must be a JSON object",
        );
    };

    let required = required_args(spec);
    let missing: Vec<&str> = required
        .iter()
        .copied()
        .filter(|name| !object.contains_key(*name))
        .collect();
    if !missing.is_empty() {
        return usage_error(spec, &missing, "missing required argument(s)");
    }

    let Some(properties) = spec
        .input_schema
        .get("properties")
        .and_then(Value::as_object)
    else {
        return admission_error("tool schema is invalid: missing properties object");
    };

    for (name, value) in object {
        let Some(property_schema) = properties.get(name) else {
            return usage_error(
                spec,
                &[name.as_str()],
                "unexpected argument; use the documented parameter names",
            );
        };
        validate_arg_value(spec, name, value, property_schema)?;
    }

    Ok(())
}

fn required_args(spec: &ToolSpec) -> Vec<&str> {
    spec.input_schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

fn usage_error<T>(spec: &ToolSpec, names: &[&str], reason: &str) -> ToolResult<T> {
    let required = required_args(spec);
    let usage = format!("Usage: {}({})", spec.name, required.join(", "));
    let quoted_names = names
        .iter()
        .map(|name| format!("'{name}'"))
        .collect::<Vec<_>>()
        .join(", ");

    Err(ToolError::AdmissionRejected {
        message: format!("{usage}. {reason}: {quoted_names}"),
    })
}

fn validate_arg_value(
    spec: &ToolSpec,
    name: &str,
    value: &Value,
    property_schema: &Value,
) -> ToolResult<()> {
    if let Some(values) = property_schema.get("enum").and_then(Value::as_array) {
        validate_string(spec, name, value)?;
        let Some(actual) = value.as_str() else {
            return usage_error(spec, &[name], "invalid argument type");
        };
        let allowed = values
            .iter()
            .filter_map(Value::as_str)
            .any(|item| item == actual);
        if !allowed {
            return admission_error(&format!(
                "invalid value for '{name}': '{actual}' is not allowed for {}",
                spec.name
            ));
        }
        return Ok(());
    }

    match property_schema.get("type") {
        Some(Value::String(kind)) if kind == "string" => validate_string(spec, name, value),
        Some(Value::String(kind)) if kind == "integer" => validate_integer(spec, name, value),
        Some(Value::String(kind)) if kind == "boolean" => validate_boolean(spec, name, value),
        Some(Value::Array(kinds)) => validate_union_type(spec, name, value, kinds, property_schema),
        _ => admission_error(&format!(
            "tool schema for {}.{} has an unsupported type",
            spec.name, name
        )),
    }
}

fn validate_string(spec: &ToolSpec, name: &str, value: &Value) -> ToolResult<()> {
    if value.is_string() {
        Ok(())
    } else {
        usage_error(spec, &[name], "invalid argument type; expected string")
    }
}

fn validate_integer(spec: &ToolSpec, name: &str, value: &Value) -> ToolResult<()> {
    let Some(integer) = value.as_i64() else {
        return usage_error(spec, &[name], "invalid argument type; expected integer");
    };
    let minimum = minimum_integer_value(spec, name);
    if integer < minimum {
        let bound = if minimum == 0 {
            "a non-negative value".to_string()
        } else {
            format!("a value >= {minimum}")
        };
        return admission_error(&format!(
            "invalid value for '{name}': integer bounds require {bound}"
        ));
    }
    Ok(())
}

fn minimum_integer_value(spec: &ToolSpec, name: &str) -> i64 {
    if spec.name == "plugin_add" && name == "index" {
        -1
    } else {
        0
    }
}

fn validate_boolean(spec: &ToolSpec, name: &str, value: &Value) -> ToolResult<()> {
    if value.is_boolean() {
        Ok(())
    } else {
        usage_error(spec, &[name], "invalid argument type; expected boolean")
    }
}

fn validate_union_type(
    spec: &ToolSpec,
    name: &str,
    value: &Value,
    kinds: &[Value],
    property_schema: &Value,
) -> ToolResult<()> {
    let accepts_integer = kinds.iter().any(|kind| kind.as_str() == Some("integer"));
    let accepts_string = kinds.iter().any(|kind| kind.as_str() == Some("string"));

    if accepts_integer && value.as_i64().is_some() {
        return Ok(());
    }

    if accepts_string {
        if let Some(text) = value.as_str() {
            if property_schema
                .get("x-eud-allowAnyString")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                return Ok(());
            }
            if text.parse::<i64>().is_ok() {
                return Ok(());
            }
        }
    }

    usage_error(
        spec,
        &[name],
        "invalid argument type; expected integer or numeric string",
    )
}

fn validate_first_principles(spec: &ToolSpec, args: &Value) -> ToolResult<()> {
    match spec.name {
        "btn_set" => {
            if let Some(csv) = args.get("csv").and_then(Value::as_str) {
                validate_btn_csv(csv)?;
            }
        }
        "xdat_set" => {
            let dat = args.get("dat").and_then(Value::as_str);
            let name = args.get("name").and_then(Value::as_str);
            let obj_id = args.get("objId").and_then(Value::as_i64);
            let value = args.get("value").and_then(parse_numeric_arg);

            if let (Some(dat), Some(name), Some(obj_id), Some(value)) = (dat, name, obj_id, value) {
                validate_buttonset_xdat(dat, name, obj_id, value)?;
            }
        }
        _ => {}
    }

    Ok(())
}

fn parse_numeric_arg(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|text| text.parse::<i64>().ok()))
}

/// Validate a SETBTN CSV payload against first principles #15.
///
/// Rejects disableable train/tech buttons whose disabled-state requirement
/// string is `0`, while leaving malformed or non-numeric groups to the bridge.
pub fn validate_btn_csv(csv: &str) -> ToolResult<()> {
    for (position, group) in csv.split('.').enumerate() {
        let fields: Vec<&str> = group.split(',').collect();
        if fields.len() < 8 {
            continue;
        }

        let actval = match fields[5].trim().parse::<i64>() {
            Ok(value) => value,
            Err(_) => continue,
        };
        let disstr = match fields[7].trim().parse::<i64>() {
            Ok(value) => value,
            Err(_) => continue,
        };

        if actval != 0 && disstr == 0 {
            return Err(ToolError::ButtonDisableStringRequired {
                message: format!(
                    "first principles #15: button group {position} is disableable \
(actval={actval}, a train/tech button) but its disabled-state requirement string \
(disstr, field index 7) is 0/None. Rendering that disabled state crashes 64-bit \
StarCraft on selection. Set disstr to a valid TBL string id, for example reuse enastr \
(field index 6 = {}).",
                    fields[6].trim()
                ),
            });
        }
    }

    Ok(())
}

/// Validate an xdat ButtonSet write against the reassignment crash rail.
///
/// A unit's ButtonSet may be edited only in place; assigning another set id is
/// a measured hard crash when that unit is selected.
pub fn validate_buttonset_xdat(dat: &str, name: &str, obj_id: i64, value: i64) -> ToolResult<()> {
    if dat == "ButtonSet" && name == "ButtonSet" && value != obj_id {
        return Err(ToolError::ButtonSetReassign {
            message: format!(
                "measured hard-crash (2026-06-07): reassigning unit {obj_id}'s ButtonSet \
to a different set id ({value}) crashes StarCraft on unit selection in both 32-bit and \
64-bit. Edit the unit's OWN button set in place with btn_set instead; its set id equals \
the unit id ({obj_id})."
            ),
        });
    }

    Ok(())
}

/// Parsed `location_write` operation. Coordinates are tile units until encoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocWrite {
    Add {
        left: i64,
        top: i64,
        right: i64,
        bottom: i64,
        name: String,
        invert_x: bool,
        invert_y: bool,
    },
    Set {
        id: i64,
        left: i64,
        top: i64,
        right: i64,
        bottom: i64,
        invert_x: bool,
        invert_y: bool,
    },
    Rename {
        id: i64,
        name: String,
    },
    Delete {
        id: i64,
    },
}

pub fn parse_location_write(args: &Value) -> ToolResult<LocWrite> {
    let Some(object) = args.as_object() else {
        return Err(location_write_error(
            "arguments must be a JSON object with action add|set|rename|delete",
        ));
    };
    let action = object
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| location_write_error("missing required field action"))?;

    match action {
        "add" => {
            let name = required_location_name(object, "name")?;
            let (left, top, right, bottom) = required_tile_rect(object)?;
            validate_tile_rect(left, top, right, bottom)?;
            Ok(LocWrite::Add {
                left,
                top,
                right,
                bottom,
                name,
                invert_x: optional_bool(object, "invertX"),
                invert_y: optional_bool(object, "invertY"),
            })
        }
        "set" => {
            let id = required_location_id(object)?;
            reject_anywhere(id)?;
            let (left, top, right, bottom) = required_tile_rect(object)?;
            validate_tile_rect(left, top, right, bottom)?;
            Ok(LocWrite::Set {
                id,
                left,
                top,
                right,
                bottom,
                invert_x: optional_bool(object, "invertX"),
                invert_y: optional_bool(object, "invertY"),
            })
        }
        "rename" => {
            let id = required_location_id(object)?;
            reject_anywhere(id)?;
            let name = required_location_name(object, "name")?;
            Ok(LocWrite::Rename { id, name })
        }
        "delete" => {
            let id = required_location_id(object)?;
            reject_anywhere(id)?;
            Ok(LocWrite::Delete { id })
        }
        other => Err(location_write_error(format!(
            "invalid action {other:?}; expected add, set, rename, or delete"
        ))),
    }
}

pub fn encode_locedit_ops(op: &LocWrite, name_bytes: &[u8]) -> Vec<u8> {
    match op {
        LocWrite::Add {
            left,
            top,
            right,
            bottom,
            invert_x,
            invert_y,
            ..
        } => {
            let (left, top, right, bottom) =
                pixel_rect(*left, *top, *right, *bottom, *invert_x, *invert_y);
            let mut ops = format!("add|{left}|{top}|{right}|{bottom}|").into_bytes();
            ops.extend_from_slice(name_bytes);
            ops
        }
        LocWrite::Set {
            id,
            left,
            top,
            right,
            bottom,
            invert_x,
            invert_y,
        } => {
            let (left, top, right, bottom) =
                pixel_rect(*left, *top, *right, *bottom, *invert_x, *invert_y);
            format!("set|{id}|{left}|{top}|{right}|{bottom}").into_bytes()
        }
        LocWrite::Rename { id, .. } => {
            let mut ops = format!("rename|{id}|").into_bytes();
            ops.extend_from_slice(name_bytes);
            ops
        }
        LocWrite::Delete { id } => format!("del|{id}").into_bytes(),
    }
}

pub fn encode_location_name(name: &str, chk: &[u8]) -> Vec<u8> {
    if name.is_ascii() {
        return name.as_bytes().to_vec();
    }
    if chk.windows(4).any(|window| window == b"STRx") {
        return name.as_bytes().to_vec();
    }

    EUC_KR.encode(name).0.into_owned()
}

pub fn location_write_apply<S, L, E>(
    map_safe: &crate::mapsafe::MapSafe<S, L, E>,
    journal: &crate::journal::JournalStore,
    request_id: &str,
    map_path: &Path,
    chk: &[u8],
    args: &Value,
    ts: u64,
) -> ToolResult<Value>
where
    S: crate::mapsafe::CompilingStatus,
    L: crate::mapsafe::LockProbe,
    E: crate::mapsafe::MapEngine,
{
    let op = parse_location_write(args)?;
    let name_bytes = op
        .name()
        .map(|name| encode_location_name(name, chk))
        .unwrap_or_default();
    let ops = encode_locedit_ops(&op, &name_bytes);
    let backup = match map_safe.write(map_path, crate::mapsafe::OpKind::Locedit, &ops) {
        Ok(entry) => entry,
        Err(error) => {
            return Err(location_write_mapsafe_error(map_safe, map_path, error));
        }
    };

    let post_chk = match isom::chk_extract(map_path) {
        Ok(chk) => chk,
        Err(isom_error) => std::fs::read(map_path).map_err(|read_error| {
            location_write_error(format!(
                "post-edit CHK extraction failed for {}: {isom_error}; raw CHK fallback failed: {read_error}",
                map_path.display()
            ))
        })?,
    };
    let pre_digest = crate::chk::digest_chk(chk);
    let post_digest = crate::chk::digest_chk(&post_chk);
    let location_id = assigned_location_id(&op, &pre_digest.locations, &post_digest.locations);

    let existing = match journal.changeset(request_id) {
        Ok(changeset) => changeset.items.len() as u64,
        Err(crate::journal::JournalError::MissingJournal { .. }) => 0,
        Err(error) => return Err(location_write_error(error.to_string())),
    };
    let seq = existing + 1;
    let entry = crate::journal::JournalEntry {
        id: format!("loc-{seq}"),
        seq,
        tool: crate::journal::WriteTool::LocationWrite,
        target: crate::journal::JournalTarget::Map {
            path: map_path.to_string_lossy().to_string(),
            summary: location_write_summary(&op),
        },
        before: crate::journal::Snapshot::MapBackup {
            map_path: backup.map_path.to_string_lossy().to_string(),
            backup_path: backup.backup_path.to_string_lossy().to_string(),
        },
        after: crate::journal::Snapshot::MapEdit {
            action: op.action().to_string(),
            location_id,
            name: op.name().map(str::to_owned),
        },
        ts,
    };
    journal
        .record(request_id, entry)
        .map_err(|error| location_write_error(error.to_string()))?;

    Ok(json!({
        "ok": true,
        "action": op.action(),
        "locationId": location_id,
        "mapPath": map_path.to_string_lossy().to_string(),
        "backupPath": backup.backup_path.to_string_lossy().to_string(),
        "locations": post_digest.locations,
    }))
}

pub fn location_write<S, L, E>(
    bridge: &crate::bridge_io::BridgeIo,
    map_safe: &crate::mapsafe::MapSafe<S, L, E>,
    journal: &crate::journal::JournalStore,
    request_id: &str,
    args: &Value,
) -> ToolResult<Value>
where
    S: crate::mapsafe::CompilingStatus,
    L: crate::mapsafe::LockProbe,
    E: crate::mapsafe::MapEngine,
{
    let map_path_reply = bridge
        .send(
            "GETSET project|OpenMapName",
            &crate::bridge_io::SendOpts::default(),
            None,
        )
        .map_err(|error| {
            location_write_error(format!("bridge GETSET OpenMapName failed: {error}"))
        })?;
    let map_path = parse_open_map_name_reply(&map_path_reply);
    if map_path.is_empty() {
        return Err(location_write_error(
            "bridge returned an empty project OpenMapName; open or configure a source map",
        ));
    }

    let path = Path::new(map_path);
    let metadata = std::fs::metadata(path).map_err(|error| {
        location_write_error(format!(
            "source map file is missing or unreadable: {map_path} ({error})"
        ))
    })?;
    if !metadata.is_file() {
        return Err(location_write_error(format!(
            "source map path is not a file: {map_path}"
        )));
    }

    let chk = isom::chk_extract(path).map_err(|error| {
        location_write_error(format!("CHK extraction failed for {map_path}: {error}"))
    })?;
    let ts = saved_at_epoch_seconds(SystemTime::now());
    location_write_apply(map_safe, journal, request_id, path, &chk, args, ts)
}

impl LocWrite {
    fn action(&self) -> &'static str {
        match self {
            Self::Add { .. } => "add",
            Self::Set { .. } => "set",
            Self::Rename { .. } => "rename",
            Self::Delete { .. } => "delete",
        }
    }

    fn name(&self) -> Option<&str> {
        match self {
            Self::Add { name, .. } | Self::Rename { name, .. } => Some(name),
            Self::Set { .. } | Self::Delete { .. } => None,
        }
    }

    fn explicit_id(&self) -> Option<i64> {
        match self {
            Self::Add { .. } => None,
            Self::Set { id, .. } | Self::Rename { id, .. } | Self::Delete { id } => Some(*id),
        }
    }
}

fn required_location_name(object: &Map<String, Value>, field: &str) -> ToolResult<String> {
    let name = object
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| location_write_error(format!("missing required field {field}")))?;
    if name.is_empty() {
        return Err(location_write_error(format!("{field} must be non-empty")));
    }
    if name.contains('|') || name.contains('\n') || name.contains('\r') {
        return Err(location_write_error(format!(
            "{field} must not contain '|', newline, or carriage return"
        )));
    }
    Ok(name.to_string())
}

fn required_location_id(object: &Map<String, Value>) -> ToolResult<i64> {
    let id = required_i64(object, "locationId")?;
    if id < 1 {
        return Err(location_write_error(
            "locationId must be an integer greater than or equal to 1",
        ));
    }
    Ok(id)
}

fn reject_anywhere(id: i64) -> ToolResult<()> {
    if id == 64 {
        return Err(location_write_error(
            "locationId 64 is Anywhere and is protected by hivemind/docs/rules.md; refusing set/rename/delete",
        ));
    }
    Ok(())
}

fn required_tile_rect(object: &Map<String, Value>) -> ToolResult<(i64, i64, i64, i64)> {
    Ok((
        required_i64(object, "tileLeft")?,
        required_i64(object, "tileTop")?,
        required_i64(object, "tileRight")?,
        required_i64(object, "tileBottom")?,
    ))
}

fn required_i64(object: &Map<String, Value>, field: &str) -> ToolResult<i64> {
    let value = object
        .get(field)
        .ok_or_else(|| location_write_error(format!("missing required field {field}")))?;
    if let Some(value) = value.as_i64() {
        return Ok(value);
    }
    if let Some(value) = value.as_u64().and_then(|value| i64::try_from(value).ok()) {
        return Ok(value);
    }
    Err(location_write_error(format!("{field} must be an integer")))
}

fn optional_bool(object: &Map<String, Value>, field: &str) -> bool {
    object.get(field).and_then(Value::as_bool).unwrap_or(false)
}

fn validate_tile_rect(left: i64, top: i64, right: i64, bottom: i64) -> ToolResult<()> {
    if right <= left || bottom <= top {
        return Err(location_write_error(
            "tile rect must be normal before inversion: tileRight > tileLeft and tileBottom > tileTop",
        ));
    }
    Ok(())
}

fn pixel_rect(
    left: i64,
    top: i64,
    right: i64,
    bottom: i64,
    invert_x: bool,
    invert_y: bool,
) -> (i64, i64, i64, i64) {
    let (mut left, mut top, mut right, mut bottom) = (left * 32, top * 32, right * 32, bottom * 32);
    if invert_x {
        std::mem::swap(&mut left, &mut right);
    }
    if invert_y {
        std::mem::swap(&mut top, &mut bottom);
    }
    (left, top, right, bottom)
}

fn assigned_location_id(
    op: &LocWrite,
    pre_locations: &[crate::chk::Location],
    post_locations: &[crate::chk::Location],
) -> Option<i64> {
    match op {
        LocWrite::Add { name, .. } => {
            let pre_ids_for_name = pre_locations
                .iter()
                .filter(|location| location.name == *name)
                .map(|location| location.id)
                .collect::<Vec<_>>();
            post_locations
                .iter()
                .find(|location| location.name == *name && !pre_ids_for_name.contains(&location.id))
                .or_else(|| {
                    post_locations
                        .iter()
                        .find(|location| location.name == *name)
                })
                .and_then(|location| i64::try_from(location.id).ok())
        }
        _ => op.explicit_id(),
    }
}

fn location_write_summary(op: &LocWrite) -> String {
    match op {
        LocWrite::Add { name, .. } => format!("add {name}"),
        LocWrite::Set { id, .. } => format!("set #{id}"),
        LocWrite::Rename { id, name } => format!("rename #{id} -> {name}"),
        LocWrite::Delete { id } => format!("delete #{id}"),
    }
}

fn location_write_mapsafe_error<S, L, E>(
    map_safe: &crate::mapsafe::MapSafe<S, L, E>,
    map_path: &Path,
    error: crate::mapsafe::MapSafeError,
) -> ToolError
where
    S: crate::mapsafe::CompilingStatus,
    L: crate::mapsafe::LockProbe,
    E: crate::mapsafe::MapEngine,
{
    match error {
        crate::mapsafe::MapSafeError::Verify { detail, backup } => {
            let entry = crate::mapsafe::JournalEntry {
                map_path: map_path.to_path_buf(),
                backup_path: backup.clone(),
            };
            match map_safe.restore(&entry) {
                Ok(()) => location_write_error(format!(
                    "post-edit verification failed ({detail}); the map was restored from backup {}",
                    backup.display()
                )),
                Err(restore_error) => location_write_error(format!(
                    "post-edit verification failed ({detail}); restore from backup {} also failed: {restore_error}. Recover manually from this backup.",
                    backup.display()
                )),
            }
        }
        crate::mapsafe::MapSafeError::Compiling => location_write_error(
            "compiling guard refused: the editor is building right now; retry after the build finishes",
        ),
        _ => location_write_error(error.to_string()),
    }
}

/// Resolve the connected source map through the bridge, extract its CHK, and return a
/// sliced JSON view of the digest. This is intentionally thin; map parsing, filtering,
/// and truncation are in [`map_info_view`] for headless tests.
pub fn map_info(bridge: &crate::bridge_io::BridgeIo, args: &Value) -> ToolResult<Value> {
    let map_path_reply = bridge
        .send(
            "GETSET project|OpenMapName",
            &crate::bridge_io::SendOpts::default(),
            None,
        )
        .map_err(|error| map_info_error(format!("bridge GETSET OpenMapName failed: {error}")))?;
    let map_path = parse_open_map_name_reply(&map_path_reply);
    if map_path.is_empty() {
        return Err(map_info_error(
            "bridge returned an empty project OpenMapName; open or configure a source map",
        ));
    }

    let path = Path::new(map_path);
    let metadata = std::fs::metadata(path).map_err(|error| {
        map_info_error(format!(
            "source map file is missing or unreadable: {map_path} ({error})"
        ))
    })?;
    if !metadata.is_file() {
        return Err(map_info_error(format!(
            "source map path is not a file: {map_path}"
        )));
    }
    let saved_at = metadata
        .modified()
        .map(saved_at_epoch_seconds)
        .map_err(|error| map_info_error(format!("could not read map mtime: {error}")))?;

    let chk = isom::chk_extract(path).map_err(|error| {
        map_info_error(format!("CHK extraction failed for {map_path}: {error}"))
    })?;
    let digest = crate::chk::digest_chk(&chk);
    map_info_view(&digest, args, map_path, saved_at)
}

/// Pure view builder for `map_info`: slices a precomputed CHK digest by mode, applies
/// filters, caps unit output, and attaches the map path/mtime envelope.
pub fn map_info_view(
    digest: &crate::chk::Digest,
    args: &Value,
    map_path: &str,
    saved_at: u64,
) -> ToolResult<Value> {
    let Some(object) = args.as_object() else {
        return Err(map_info_error("map_info arguments must be a JSON object"));
    };

    let mode = object
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("summary");
    let map = json!({
        "path": map_path,
        "savedAt": saved_at,
    });

    match mode {
        "summary" => Ok(json!({
            "map": map,
            "mode": "summary",
            "summary": {
                "header": &digest.map,
                "activePlayers": active_players(digest),
                "forces": &digest.forces,
                "startLocations": {
                    "count": digest.start_locations.len(),
                    "items": &digest.start_locations,
                },
                "locations": {
                    "count": digest.locations.len(),
                    "names": digest.locations.iter().map(|location| location.name.clone()).collect::<Vec<_>>(),
                },
                "unitsByOwner": units_by_owner(digest),
            },
        })),
        "locations" => Ok(json!({
            "map": map,
            "mode": "locations",
            "locations": &digest.locations,
        })),
        "units" => units_view(digest, object, map),
        "players" => Ok(json!({
            "map": map,
            "mode": "players",
            "players": &digest.players,
            "forces": &digest.forces,
        })),
        other => Err(map_info_error(format!(
            "invalid map_info mode {other:?}; expected summary, locations, units, or players"
        ))),
    }
}

fn active_players(digest: &crate::chk::Digest) -> Vec<crate::chk::Player> {
    digest
        .players
        .iter()
        .filter(|player| is_active_controller(&player.controller))
        .cloned()
        .collect()
}

fn is_active_controller(controller: &str) -> bool {
    matches!(
        controller,
        "Computer (game)"
            | "Occupied by Human"
            | "Rescue Passive"
            | "Computer"
            | "Human (Open Slot)"
    )
}

fn units_by_owner(digest: &crate::chk::Digest) -> BTreeMap<String, BTreeMap<String, usize>> {
    let mut owners = BTreeMap::<String, BTreeMap<String, usize>>::new();
    for unit in &digest.units {
        *owners
            .entry(unit.owner.clone())
            .or_default()
            .entry(unit.type_name.clone())
            .or_default() += 1;
    }
    owners
}

fn units_view(
    digest: &crate::chk::Digest,
    args: &Map<String, Value>,
    map: Value,
) -> ToolResult<Value> {
    const UNIT_LIMIT: usize = 200;

    let owner_filter = args.get("owner").and_then(Value::as_str);
    let unit_type_filter = args.get("unitType").map(parse_unit_type_filter);

    let mut filtered = Vec::new();
    for unit in &digest.units {
        if let Some(owner) = &owner_filter {
            if !unit_owner_matches_filter(&unit.owner, owner) {
                continue;
            }
        }
        if let Some(filter) = &unit_type_filter {
            if !unit_matches_type_filter(unit, filter) {
                continue;
            }
        }
        filtered.push(unit);
    }

    let total = filtered.len();
    let units = filtered
        .into_iter()
        .take(UNIT_LIMIT)
        .cloned()
        .collect::<Vec<_>>();
    let mut value = json!({
        "map": map,
        "mode": "units",
        "filters": {
            "owner": args.get("owner").cloned().unwrap_or(Value::Null),
            "unitType": args.get("unitType").cloned().unwrap_or(Value::Null),
        },
        "count": total,
        "units": units,
    });

    if total > UNIT_LIMIT {
        value["truncated"] = Value::Bool(true);
        value["dropped"] = json!(total - UNIT_LIMIT);
        value["hint"] = json!(format!(
            "showing first {UNIT_LIMIT} units after filters owner={:?}, unitType={:?}",
            args.get("owner"),
            args.get("unitType")
        ));
    }

    Ok(value)
}

fn unit_owner_matches_filter(unit_owner: &str, filter: &str) -> bool {
    unit_owner == filter
        || unit_owner.starts_with(&format!("{filter} "))
        || (filter.eq_ignore_ascii_case("neutral") && unit_owner.contains("(neutral)"))
}

enum UnitTypeFilter {
    Id(u64),
    Name(String),
}

fn parse_unit_type_filter(value: &Value) -> UnitTypeFilter {
    if let Some(id) = value.as_u64() {
        return UnitTypeFilter::Id(id);
    }
    if value.as_i64().is_some() {
        return UnitTypeFilter::Id(u64::MAX);
    }
    if let Some(text) = value.as_str() {
        if let Ok(id) = text.trim().parse::<u64>() {
            return UnitTypeFilter::Id(id);
        }
        return UnitTypeFilter::Name(text.to_lowercase());
    }
    UnitTypeFilter::Name(String::new())
}

fn unit_matches_type_filter(unit: &crate::chk::Unit, filter: &UnitTypeFilter) -> bool {
    match filter {
        UnitTypeFilter::Id(id) => u64::from(unit.type_id) == *id,
        UnitTypeFilter::Name(text) => unit.type_name.to_lowercase().contains(text),
    }
}

fn saved_at_epoch_seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn parse_open_map_name_reply(reply: &str) -> &str {
    let trimmed = reply.trim();
    let Some((prefix, value)) = trimmed.split_once(" = ") else {
        return trimmed;
    };
    if prefix.trim() == "OK: project|OpenMapName" {
        value.trim()
    } else {
        trimmed
    }
}

fn map_info_error(message: impl Into<String>) -> ToolError {
    ToolError::AdmissionRejected {
        message: format!("map_info: {}", message.into()),
    }
}

fn location_write_error(message: impl Into<String>) -> ToolError {
    ToolError::AdmissionRejected {
        message: format!("location_write: {}", message.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use encoding_rs::EUC_KR;
    use std::cell::Cell;
    use std::fs;
    use std::path::{Path, PathBuf};

    struct LocationWriteFakeStatus(bool);

    impl crate::mapsafe::CompilingStatus for LocationWriteFakeStatus {
        fn is_compiling(&self) -> bool {
            self.0
        }
    }

    struct LocationWriteFakeLock(bool);

    impl crate::mapsafe::LockProbe for LocationWriteFakeLock {
        fn is_locked(&self, _path: &Path) -> bool {
            self.0
        }
    }

    struct LocationWriteFakeEngine {
        applied_bytes: Vec<u8>,
        digest_result: Result<Vec<u8>, String>,
        apply_called: Cell<bool>,
    }

    impl LocationWriteFakeEngine {
        fn ok(chk_after_apply: Vec<u8>) -> Self {
            Self {
                applied_bytes: chk_after_apply.clone(),
                digest_result: Ok(chk_after_apply),
                apply_called: Cell::new(false),
            }
        }

        fn verify_fails(applied_bytes: Vec<u8>) -> Self {
            Self {
                applied_bytes,
                digest_result: Err("unreadable post-edit CHK".to_owned()),
                apply_called: Cell::new(false),
            }
        }
    }

    impl crate::mapsafe::MapEngine for LocationWriteFakeEngine {
        fn apply(
            &self,
            map: &Path,
            kind: crate::mapsafe::OpKind,
            ops: &[u8],
        ) -> Result<(), String> {
            assert_eq!(kind, crate::mapsafe::OpKind::Locedit);
            assert!(
                ops.starts_with(b"add|"),
                "location_write add should encode a locedit add op"
            );
            self.apply_called.set(true);
            fs::write(map, &self.applied_bytes).map_err(|error| error.to_string())
        }

        fn digest(&self, _map: &Path) -> Result<Vec<u8>, String> {
            self.digest_result.clone()
        }
    }

    const TOOL_TEST_MRGN_ENTRY_SIZE: usize = 20;

    fn tool_test_temp_dir(test_name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("eud-agent-tools-{test_name}-{nanos}"));
        fs::create_dir_all(&dir).expect("temp data dir should be creatable");
        dir
    }

    fn tool_test_section(name: &str, body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&(body.len() as i32).to_le_bytes());
        out.extend_from_slice(body);
        out
    }

    fn tool_test_strx(values: &[&[u8]]) -> Vec<u8> {
        let count = values.len();
        let table_len = 4 * (count + 1);
        let mut out = vec![0; table_len];
        out[0..4].copy_from_slice(&(count as u32).to_le_bytes());

        let mut cursor = table_len;
        for (idx, value) in values.iter().enumerate() {
            out[4 * (idx + 1)..4 * (idx + 2)].copy_from_slice(&(cursor as u32).to_le_bytes());
            out.extend_from_slice(value);
            out.push(0);
            cursor = out.len();
        }
        out
    }

    fn tool_test_mrgn_entry(
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
        string_id: u16,
    ) -> [u8; TOOL_TEST_MRGN_ENTRY_SIZE] {
        let mut out = [0u8; TOOL_TEST_MRGN_ENTRY_SIZE];
        out[0..4].copy_from_slice(&left.to_le_bytes());
        out[4..8].copy_from_slice(&top.to_le_bytes());
        out[8..12].copy_from_slice(&right.to_le_bytes());
        out[12..16].copy_from_slice(&bottom.to_le_bytes());
        out[16..18].copy_from_slice(&string_id.to_le_bytes());
        out
    }

    fn tool_test_chk_with_location(name: &[u8]) -> Vec<u8> {
        let dim = [64u16.to_le_bytes(), 128u16.to_le_bytes()].concat();
        let era = 3u16.to_le_bytes();
        let strx = tool_test_strx(&[name, b"Anywhere"]);

        let mut mrgn = Vec::new();
        mrgn.extend_from_slice(&tool_test_mrgn_entry(32, 64, 96, 128, 1));
        while mrgn.len() < crate::chk::_ANYWHERE_INDEX * TOOL_TEST_MRGN_ENTRY_SIZE {
            mrgn.extend_from_slice(&tool_test_mrgn_entry(0, 0, 0, 0, 0));
        }
        mrgn.extend_from_slice(&tool_test_mrgn_entry(0, 0, 2048, 4096, 2));

        let mut chk = Vec::new();
        chk.extend_from_slice(&tool_test_section("DIM ", &dim));
        chk.extend_from_slice(&tool_test_section("ERA ", &era));
        chk.extend_from_slice(&tool_test_section("STRx", &strx));
        chk.extend_from_slice(&tool_test_section("MRGN", &mrgn));
        chk
    }

    fn write_tool(name: &'static str) -> ToolSpec {
        ToolSpec::mutating(name)
    }

    fn assert_evidence_required(result: ToolResult<()>) {
        match result {
            Err(ToolError::EvidenceRequired { message }) => {
                assert!(
                    message.contains(SEARCH_DOCS_TOOL),
                    "EvidenceRequired must direct the model to call search_docs first"
                );
            }
            other => panic!("expected EvidenceRequired, got {other:?}"),
        }
    }

    #[test]
    fn evidence_gate_blocks_mutating_rag_wired_call_before_search() {
        let state = RequestState::new();
        let result = check_evidence_gate(&state, &write_tool("btn_set"), true);

        assert_evidence_required(result);
    }

    #[test]
    fn evidence_gate_allows_same_mutating_call_after_search_even_with_zero_hits() {
        let mut state = RequestState::new();
        state.record_search_docs();

        assert!(
            state.docs_searched,
            "search_docs must lift the evidence gate"
        );
        assert_eq!(
            check_evidence_gate(&state, &write_tool("btn_set"), true),
            Ok(())
        );
    }

    #[test]
    fn evidence_gate_never_blocks_memory_write_or_build_run() {
        let state = RequestState::new();

        assert_eq!(
            check_evidence_gate(&state, &write_tool(MEMORY_WRITE_TOOL), true),
            Ok(())
        );
        assert_eq!(
            check_evidence_gate(&state, &write_tool(BUILD_RUN_TOOL), true),
            Ok(())
        );
    }

    #[test]
    fn evidence_gate_degrades_open_when_rag_is_not_wired() {
        let state = RequestState::new();

        assert_eq!(
            check_evidence_gate(&state, &write_tool("btn_set"), false),
            Ok(())
        );
    }

    #[test]
    fn btn_csv_rejects_disableable_button_with_zero_disabled_string() {
        let csv = "1,2,3,4,5,65,200,0";

        assert!(
            matches!(
                validate_btn_csv(csv),
                Err(ToolError::ButtonDisableStringRequired { .. })
            ),
            "actval != 0 and disstr == 0 must be rejected"
        );
    }

    #[test]
    fn btn_csv_allows_always_enabled_button_with_zero_disabled_string() {
        let csv = "1,2,3,4,5,0,200,0";

        assert_eq!(validate_btn_csv(csv), Ok(()));
    }

    #[test]
    fn btn_csv_allows_disableable_button_with_nonzero_disabled_string() {
        let csv = "1,2,3,4,5,65,200,201";

        assert_eq!(validate_btn_csv(csv), Ok(()));
    }

    #[test]
    fn btn_csv_skips_short_groups() {
        let csv = "1,2,3,4,5,65,200";

        assert_eq!(validate_btn_csv(csv), Ok(()));
    }

    #[test]
    fn btn_csv_checks_each_dot_separated_group() {
        let csv = "1,2,3,4,5,0,200,0.2,2,3,4,5,65,200,0";

        assert!(
            matches!(
                validate_btn_csv(csv),
                Err(ToolError::ButtonDisableStringRequired { .. })
            ),
            "any invalid button group in a dot-separated SETBTN CSV must reject"
        );
    }

    #[test]
    fn xdat_buttonset_reassignment_to_different_set_is_rejected() {
        let result = validate_buttonset_xdat("ButtonSet", "ButtonSet", 65, 66);

        assert!(
            matches!(result, Err(ToolError::ButtonSetReassign { .. })),
            "ButtonSet/ButtonSet value != obj_id must be rejected"
        );
    }

    #[test]
    fn xdat_buttonset_in_place_edit_of_own_set_is_allowed() {
        assert_eq!(
            validate_buttonset_xdat("ButtonSet", "ButtonSet", 65, 65),
            Ok(())
        );
    }

    #[test]
    fn xdat_other_dat_or_name_is_unaffected() {
        assert_eq!(validate_buttonset_xdat("Unit", "ButtonSet", 65, 66), Ok(()));
        assert_eq!(
            validate_buttonset_xdat("ButtonSet", "Other", 65, 66),
            Ok(())
        );
    }

    #[test]
    fn location_write_parse_accepts_valid_action_shapes() {
        assert_eq!(
            parse_location_write(&serde_json::json!({
                "action": "add",
                "name": "spot",
                "tileLeft": 1,
                "tileTop": 2,
                "tileRight": 3,
                "tileBottom": 4,
                "invertX": true,
            }))
            .unwrap(),
            LocWrite::Add {
                left: 1,
                top: 2,
                right: 3,
                bottom: 4,
                name: "spot".to_string(),
                invert_x: true,
                invert_y: false,
            }
        );
        assert_eq!(
            parse_location_write(&serde_json::json!({
                "action": "set",
                "locationId": 5,
                "tileLeft": 1,
                "tileTop": 2,
                "tileRight": 3,
                "tileBottom": 4,
                "invertY": true,
            }))
            .unwrap(),
            LocWrite::Set {
                id: 5,
                left: 1,
                top: 2,
                right: 3,
                bottom: 4,
                invert_x: false,
                invert_y: true,
            }
        );
        assert_eq!(
            parse_location_write(&serde_json::json!({
                "action": "rename",
                "locationId": 5,
                "name": "new spot",
            }))
            .unwrap(),
            LocWrite::Rename {
                id: 5,
                name: "new spot".to_string(),
            }
        );
        assert_eq!(
            parse_location_write(&serde_json::json!({
                "action": "delete",
                "locationId": 7,
            }))
            .unwrap(),
            LocWrite::Delete { id: 7 }
        );
    }

    #[test]
    fn location_write_parse_rejects_missing_or_invalid_fields() {
        for args in [
            serde_json::json!({}),
            serde_json::json!({"action": "copy"}),
            serde_json::json!({"action": "add", "tileLeft": 1, "tileTop": 2, "tileRight": 3, "tileBottom": 4}),
            serde_json::json!({"action": "set", "locationId": 1, "tileLeft": 1, "tileTop": 2, "tileRight": 3}),
            serde_json::json!({"action": "rename", "locationId": 1}),
            serde_json::json!({"action": "delete"}),
        ] {
            assert!(
                matches!(
                    parse_location_write(&args),
                    Err(ToolError::AdmissionRejected { .. })
                ),
                "expected location_write parse rejection for {args}"
            );
        }
    }

    #[test]
    fn location_write_parse_rejects_bad_names_ids_anywhere_and_rects() {
        for args in [
            serde_json::json!({
                "action": "add",
                "name": "bad|name",
                "tileLeft": 1,
                "tileTop": 2,
                "tileRight": 3,
                "tileBottom": 4,
            }),
            serde_json::json!({"action": "rename", "locationId": 1, "name": ""}),
            serde_json::json!({"action": "delete", "locationId": 0}),
            serde_json::json!({"action": "delete", "locationId": 64}),
            serde_json::json!({
                "action": "set",
                "locationId": 64,
                "tileLeft": 1,
                "tileTop": 2,
                "tileRight": 3,
                "tileBottom": 4,
            }),
            serde_json::json!({"action": "rename", "locationId": 64, "name": "Anywhere2"}),
            serde_json::json!({
                "action": "add",
                "name": "bad rect",
                "tileLeft": 3,
                "tileTop": 2,
                "tileRight": 3,
                "tileBottom": 4,
            }),
            serde_json::json!({
                "action": "add",
                "name": "bad rect",
                "tileLeft": 1,
                "tileTop": 4,
                "tileRight": 3,
                "tileBottom": 4,
            }),
        ] {
            assert!(
                matches!(
                    parse_location_write(&args),
                    Err(ToolError::AdmissionRejected { .. })
                ),
                "expected location_write parse rejection for {args}"
            );
        }
    }

    #[test]
    fn encode_locedit_ops_renders_pixels_and_inverted_axes_without_newline() {
        assert_eq!(
            encode_locedit_ops(
                &LocWrite::Add {
                    left: 1,
                    top: 2,
                    right: 3,
                    bottom: 4,
                    name: "spot".to_string(),
                    invert_x: false,
                    invert_y: false,
                },
                b"spot",
            ),
            b"add|32|64|96|128|spot".to_vec()
        );
        assert_eq!(
            encode_locedit_ops(
                &LocWrite::Add {
                    left: 1,
                    top: 2,
                    right: 3,
                    bottom: 4,
                    name: "spot".to_string(),
                    invert_x: true,
                    invert_y: false,
                },
                b"spot",
            ),
            b"add|96|64|32|128|spot".to_vec()
        );
        assert_eq!(
            encode_locedit_ops(
                &LocWrite::Add {
                    left: 1,
                    top: 2,
                    right: 3,
                    bottom: 4,
                    name: "spot".to_string(),
                    invert_x: false,
                    invert_y: true,
                },
                b"spot",
            ),
            b"add|32|128|96|64|spot".to_vec()
        );
        assert_eq!(
            encode_locedit_ops(
                &LocWrite::Set {
                    id: 5,
                    left: 1,
                    top: 2,
                    right: 3,
                    bottom: 4,
                    invert_x: false,
                    invert_y: false,
                },
                b"",
            ),
            b"set|5|32|64|96|128".to_vec()
        );
        assert_eq!(
            encode_locedit_ops(
                &LocWrite::Rename {
                    id: 5,
                    name: "n".to_string(),
                },
                b"n",
            ),
            b"rename|5|n".to_vec()
        );
        assert_eq!(
            encode_locedit_ops(&LocWrite::Delete { id: 7 }, b""),
            b"del|7".to_vec()
        );
    }

    #[test]
    fn encode_location_name_matches_ascii_strx_utf8_and_legacy_cp949_rules() {
        let korean = "공격지점";
        let strx_chk = tool_test_section("STRx", &[]);
        let str_chk = tool_test_section("STR ", &[]);
        let (cp949, _, had_errors) = EUC_KR.encode(korean);
        assert!(!had_errors);

        assert_eq!(encode_location_name("spot", &strx_chk), b"spot".to_vec());
        assert_eq!(
            encode_location_name(korean, &strx_chk),
            korean.as_bytes().to_vec()
        );
        assert_eq!(encode_location_name(korean, &str_chk), cp949.to_vec());
    }

    #[test]
    fn location_write_apply_records_journal_and_returns_post_edit_digest() {
        let data_dir = tool_test_temp_dir("location-write-apply");
        let map_path = data_dir.join("demo.scx");
        let pre_edit_chk = tool_test_chk_with_location(b"Existing");
        let post_edit_chk = tool_test_chk_with_location(b"spot");
        fs::write(&map_path, &pre_edit_chk).expect("temp map should be writable");

        let map_safe = crate::mapsafe::MapSafe::new(
            data_dir.clone(),
            LocationWriteFakeStatus(false),
            LocationWriteFakeLock(false),
            LocationWriteFakeEngine::ok(post_edit_chk),
        );
        let journal = crate::journal::JournalStore::new(&data_dir);
        let request_id = "req-location-write";

        let result = location_write_apply(
            &map_safe,
            &journal,
            request_id,
            &map_path,
            &pre_edit_chk,
            &serde_json::json!({
                "action": "add",
                "name": "spot",
                "tileLeft": 1,
                "tileTop": 2,
                "tileRight": 3,
                "tileBottom": 4,
            }),
            1_781_000_000,
        )
        .expect("location_write add should apply through mapsafe and journal");

        let expected_map_path = map_path.to_string_lossy().to_string();
        assert_eq!(result["ok"], true);
        assert_eq!(result["action"], "add");
        assert_eq!(result["mapPath"].as_str(), Some(expected_map_path.as_str()));
        assert!(result["backupPath"]
            .as_str()
            .is_some_and(|path| !path.is_empty()));
        assert!(result["locations"].is_array());
        assert_eq!(journal.changeset(request_id).unwrap().items.len(), 1);
    }

    #[test]
    fn location_write_apply_refuses_while_compiling() {
        let data_dir = tool_test_temp_dir("location-write-compiling");
        let map_path = data_dir.join("demo.scx");
        let pre_edit_chk = tool_test_chk_with_location(b"Existing");
        fs::write(&map_path, &pre_edit_chk).expect("temp map should be writable");

        let map_safe = crate::mapsafe::MapSafe::new(
            data_dir.clone(),
            LocationWriteFakeStatus(true),
            LocationWriteFakeLock(false),
            LocationWriteFakeEngine::ok(pre_edit_chk.clone()),
        );
        let journal = crate::journal::JournalStore::new(&data_dir);

        let error = location_write_apply(
            &map_safe,
            &journal,
            "req-location-write-compiling",
            &map_path,
            &pre_edit_chk,
            &serde_json::json!({
                "action": "add",
                "name": "spot",
                "tileLeft": 1,
                "tileTop": 2,
                "tileRight": 3,
                "tileBottom": 4,
            }),
            1_781_000_001,
        )
        .expect_err("location_write must reuse mapsafe compiling guard");

        assert!(error.to_string().to_lowercase().contains("compil"));
    }

    #[test]
    fn location_write_apply_restores_backup_on_verify_failure() {
        let data_dir = tool_test_temp_dir("location-write-verify-fails");
        let map_path = data_dir.join("demo.scx");
        let pre_edit_chk = tool_test_chk_with_location(b"Existing");
        let post_edit_chk = tool_test_chk_with_location(b"spot");
        fs::write(&map_path, &pre_edit_chk).expect("temp map should be writable");

        let map_safe = crate::mapsafe::MapSafe::new(
            data_dir.clone(),
            LocationWriteFakeStatus(false),
            LocationWriteFakeLock(false),
            LocationWriteFakeEngine::verify_fails(post_edit_chk),
        );
        let journal = crate::journal::JournalStore::new(&data_dir);
        let request_id = "req-location-write-verify-fails";

        let error = location_write_apply(
            &map_safe,
            &journal,
            request_id,
            &map_path,
            &pre_edit_chk,
            &serde_json::json!({
                "action": "add",
                "name": "spot",
                "tileLeft": 1,
                "tileTop": 2,
                "tileRight": 3,
                "tileBottom": 4,
            }),
            1_781_000_002,
        )
        .expect_err("verify failure must reject the location_write call");

        let message = error.to_string();
        assert!(message.contains("post-edit verification failed"));
        assert!(message.contains("restored from backup"));
        assert_eq!(
            fs::read(&map_path).expect("map should remain readable"),
            pre_edit_chk,
            "verify failure must restore the pre-edit map bytes"
        );
        assert!(
            matches!(
                journal.changeset(request_id),
                Err(crate::journal::JournalError::MissingJournal { .. })
            ),
            "reverted verify failures must not record a journal entry"
        );
    }

    fn schema(properties: serde_json::Value, required: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        })
    }

    fn string_schema() -> serde_json::Value {
        serde_json::json!({"type": "string"})
    }

    fn integer_schema() -> serde_json::Value {
        serde_json::json!({"type": "integer"})
    }

    fn numeric_value_schema() -> serde_json::Value {
        serde_json::json!({"type": ["integer", "string"]})
    }

    fn integer_or_string_schema() -> serde_json::Value {
        serde_json::json!({"type": ["integer", "string"], "x-eud-allowAnyString": true})
    }

    fn enum_string_schema(values: &[&str]) -> serde_json::Value {
        serde_json::json!({"type": "string", "enum": values})
    }

    fn dat_names_schema() -> serde_json::Value {
        enum_string_schema(&[
            "units", "weapons", "flingy", "sprites", "images", "upgrades", "techdata", "orders",
            "portdata", "sfxdata",
        ])
    }

    fn xdat_kinds_schema() -> serde_json::Value {
        enum_string_schema(&["statusinfor", "wireframe", "ButtonSet"])
    }

    fn req_dats_schema() -> serde_json::Value {
        enum_string_schema(&["units", "upgrades", "techdata", "Stechdata", "orders"])
    }

    fn settings_scopes_schema() -> serde_json::Value {
        enum_string_schema(&["project", "program"])
    }

    fn map_info_owner_schema() -> serde_json::Value {
        enum_string_schema(&[
            "P1", "P2", "P3", "P4", "P5", "P6", "P7", "P8", "P9", "P10", "P11", "P12", "neutral",
        ])
    }

    fn sample_digest(units: Vec<crate::chk::Unit>) -> crate::chk::Digest {
        crate::chk::Digest {
            map: crate::chk::MapHeader {
                width: 64,
                height: 128,
                tileset: "jungle".to_string(),
            },
            players: vec![
                crate::chk::Player {
                    player: "P1".to_string(),
                    controller: "Occupied by Human".to_string(),
                    race: "Terran".to_string(),
                    force: Some(1),
                },
                crate::chk::Player {
                    player: "P2".to_string(),
                    controller: "Computer".to_string(),
                    race: "Protoss".to_string(),
                    force: Some(1),
                },
                crate::chk::Player {
                    player: "P3".to_string(),
                    controller: "Inactive".to_string(),
                    race: "Zerg".to_string(),
                    force: Some(2),
                },
            ],
            forces: vec![crate::chk::Force {
                force: 1,
                name: "Allies".to_string(),
                players: vec!["P1".to_string(), "P2".to_string()],
                flags: crate::chk::ForceFlags {
                    random_start_location: false,
                    allies: true,
                    allied_victory: true,
                    shared_vision: false,
                },
            }],
            locations: vec![
                crate::chk::Location {
                    id: 1,
                    name: "Main".to_string(),
                    left: 64,
                    top: 96,
                    right: 160,
                    bottom: 224,
                    tile_rect: [2, 3, 5, 7],
                    elevation_flags: 3,
                    inverted: None,
                    anywhere: None,
                },
                crate::chk::Location {
                    id: 64,
                    name: "Anywhere".to_string(),
                    left: 0,
                    top: 0,
                    right: 2048,
                    bottom: 4096,
                    tile_rect: [0, 0, 64, 128],
                    elevation_flags: 0,
                    inverted: None,
                    anywhere: Some(true),
                },
            ],
            start_locations: vec![crate::chk::StartLocation {
                player: "P1".to_string(),
                x: 96,
                y: 160,
                tile_x: 3,
                tile_y: 5,
            }],
            units,
        }
    }

    fn unit(type_id: u16, type_name: &str, owner: &str, x: u16, y: u16) -> crate::chk::Unit {
        crate::chk::Unit {
            type_name: type_name.to_string(),
            type_id,
            owner: owner.to_string(),
            x,
            y,
            tile_x: x / 32,
            tile_y: y / 32,
            hp_percent: 100,
            shield_percent: 100,
            energy_percent: 100,
            resources: 0,
        }
    }

    fn expected_registry_contract() -> Vec<(&'static str, bool, serde_json::Value)> {
        vec![
            ("project_status", false, schema(serde_json::json!({}), &[])),
            ("list_files", false, schema(serde_json::json!({}), &[])),
            (
                "read_file",
                false,
                schema(serde_json::json!({"path": string_schema()}), &["path"]),
            ),
            (
                "dat_get",
                false,
                schema(
                    serde_json::json!({
                        "dat": dat_names_schema(),
                        "param": string_schema(),
                        "objId": integer_schema(),
                    }),
                    &["dat", "param", "objId"],
                ),
            ),
            (
                "xdat_get",
                false,
                schema(
                    serde_json::json!({
                        "dat": xdat_kinds_schema(),
                        "name": string_schema(),
                        "objId": integer_schema(),
                    }),
                    &["dat", "name", "objId"],
                ),
            ),
            (
                "tbl_get",
                false,
                schema(serde_json::json!({"index": integer_schema()}), &["index"]),
            ),
            (
                "req_get",
                false,
                schema(
                    serde_json::json!({
                        "dat": req_dats_schema(),
                        "objId": integer_schema(),
                    }),
                    &["dat", "objId"],
                ),
            ),
            (
                "btn_get",
                false,
                schema(serde_json::json!({"setId": integer_schema()}), &["setId"]),
            ),
            (
                "settings_get",
                false,
                schema(
                    serde_json::json!({
                        "scope": settings_scopes_schema(),
                        "key": string_schema(),
                    }),
                    &["scope", "key"],
                ),
            ),
            (
                MAP_INFO_TOOL,
                false,
                schema(
                    serde_json::json!({
                        "mode": enum_string_schema(&["summary", "locations", "units", "players"]),
                        "owner": map_info_owner_schema(),
                        "unitType": integer_or_string_schema(),
                    }),
                    &[],
                ),
            ),
            ("plugins_list", false, schema(serde_json::json!({}), &[])),
            ("build_errors", false, schema(serde_json::json!({}), &[])),
            (
                SEARCH_DOCS_TOOL,
                false,
                schema(
                    serde_json::json!({
                        "query": string_schema(),
                        "k": integer_schema(),
                    }),
                    &["query"],
                ),
            ),
            (
                "dat_set",
                true,
                schema(
                    serde_json::json!({
                        "dat": dat_names_schema(),
                        "param": string_schema(),
                        "objId": integer_schema(),
                        "value": numeric_value_schema(),
                    }),
                    &["dat", "param", "objId", "value"],
                ),
            ),
            (
                "xdat_set",
                true,
                schema(
                    serde_json::json!({
                        "dat": xdat_kinds_schema(),
                        "name": string_schema(),
                        "objId": integer_schema(),
                        "value": numeric_value_schema(),
                    }),
                    &["dat", "name", "objId", "value"],
                ),
            ),
            (
                "tbl_set",
                true,
                schema(
                    serde_json::json!({
                        "index": integer_schema(),
                        "value": string_schema(),
                    }),
                    &["index", "value"],
                ),
            ),
            (
                "req_set",
                true,
                schema(
                    serde_json::json!({
                        "dat": req_dats_schema(),
                        "objId": integer_schema(),
                        "payload": string_schema(),
                    }),
                    &["dat", "objId", "payload"],
                ),
            ),
            (
                "btn_set",
                true,
                schema(
                    serde_json::json!({
                        "setId": integer_schema(),
                        "csv": string_schema(),
                    }),
                    &["setId", "csv"],
                ),
            ),
            (
                "dat_reset",
                true,
                schema(
                    serde_json::json!({
                        "kind": enum_string_schema(&["dat", "xdat", "tbl"]),
                        "dat": string_schema(),
                        "param": string_schema(),
                        "objId": integer_schema(),
                    }),
                    &["kind", "objId"],
                ),
            ),
            (
                "file_create",
                true,
                schema(
                    serde_json::json!({
                        "path": string_schema(),
                        "ftype": enum_string_schema(&["CUIEps", "CUIPy", "RawText"]),
                        "code": string_schema(),
                    }),
                    &["path", "ftype"],
                ),
            ),
            (
                "file_write",
                true,
                schema(
                    serde_json::json!({
                        "path": string_schema(),
                        "code": string_schema(),
                    }),
                    &["path", "code"],
                ),
            ),
            (
                "file_rename",
                true,
                schema(
                    serde_json::json!({
                        "path": string_schema(),
                        "newname": string_schema(),
                    }),
                    &["path", "newname"],
                ),
            ),
            (
                "file_delete",
                true,
                schema(serde_json::json!({"path": string_schema()}), &["path"]),
            ),
            (
                "file_move",
                true,
                schema(
                    serde_json::json!({
                        "path": string_schema(),
                        "destFolder": string_schema(),
                    }),
                    &["path"],
                ),
            ),
            (
                "mkdir",
                true,
                schema(serde_json::json!({"path": string_schema()}), &["path"]),
            ),
            (
                "set_main",
                true,
                schema(serde_json::json!({"path": string_schema()}), &["path"]),
            ),
            (
                "settings_set",
                true,
                schema(
                    serde_json::json!({
                        "scope": settings_scopes_schema(),
                        "key": string_schema(),
                        "value": string_schema(),
                    }),
                    &["scope", "key", "value"],
                ),
            ),
            (
                "plugin_add",
                true,
                schema(
                    serde_json::json!({
                        "index": integer_schema(),
                        "texts": string_schema(),
                    }),
                    &[],
                ),
            ),
            (
                "plugin_edit",
                true,
                schema(
                    serde_json::json!({
                        "index": integer_schema(),
                        "texts": string_schema(),
                    }),
                    &["index"],
                ),
            ),
            (
                "plugin_remove",
                true,
                schema(serde_json::json!({"index": integer_schema()}), &["index"]),
            ),
            (
                "plugin_move",
                true,
                schema(
                    serde_json::json!({
                        "from": integer_schema(),
                        "to": integer_schema(),
                    }),
                    &["from", "to"],
                ),
            ),
            ("build_run", true, schema(serde_json::json!({}), &[])),
            (
                "location_write",
                true,
                schema(
                    serde_json::json!({
                        "action": enum_string_schema(&["add", "set", "rename", "delete"]),
                        "name": string_schema(),
                        "locationId": integer_schema(),
                        "tileLeft": integer_schema(),
                        "tileTop": integer_schema(),
                        "tileRight": integer_schema(),
                        "tileBottom": integer_schema(),
                        "invertX": {"type": "boolean"},
                        "invertY": {"type": "boolean"},
                    }),
                    &["action"],
                ),
            ),
            (
                "player_setup",
                true,
                schema(
                    serde_json::json!({
                        "action": enum_string_schema(&[
                            "start",
                            "delstart",
                            "controller",
                        ]),
                        "player": integer_schema(),
                        "tileX": integer_schema(),
                        "tileY": integer_schema(),
                        "controller": enum_string_schema(&[
                            "human",
                            "computer",
                            "rescuable",
                            "neutral",
                            "inactive",
                            "closed",
                        ]),
                    }),
                    &["action", "player"],
                ),
            ),
            (
                MEMORY_WRITE_TOOL,
                true,
                schema(
                    serde_json::json!({
                        "file": enum_string_schema(&[
                            "resources",
                            "structure",
                            "conventions",
                            "lessons",
                        ]),
                        "content": string_schema(),
                    }),
                    &["file", "content"],
                ),
            ),
            (
                "propose_plan",
                false,
                schema(
                    serde_json::json!({"markdown": string_schema()}),
                    &["markdown"],
                ),
            ),
        ]
    }

    #[test]
    fn registry_contains_every_eud_tool_with_verbatim_schemas() {
        let registry = tool_registry();
        let expected = expected_registry_contract();

        assert_eq!(
            registry.len(),
            expected.len(),
            "registry must expose exactly the EUD-124 target tools"
        );

        for (name, mutating, input_schema) in expected {
            let spec = registry
                .iter()
                .find(|spec| spec.name == name)
                .unwrap_or_else(|| panic!("missing tool {name}"));

            assert_eq!(spec.mutating, mutating, "{name} mutating flag mismatch");
            assert!(
                !spec.description.trim().is_empty() && !spec.description.contains('\n'),
                "{name} must have a one-line description"
            );
            assert_eq!(
                &spec.input_schema, &input_schema,
                "{name} must advertise the exact parameter schema"
            );
        }
    }

    #[test]
    fn map_info_is_registered_read_only_and_invalid_mode_rejects_before_counting() {
        let spec = tool_registry()
            .into_iter()
            .find(|spec| spec.name == MAP_INFO_TOOL)
            .expect("map_info must be registered");
        assert!(!spec.mutating, "map_info must be read-only");

        let mut state = RequestState::for_request("req-map-info");
        let error = admit_tool_call(
            &mut state,
            MAP_INFO_TOOL,
            &serde_json::json!({"mode": "terrain"}),
        )
        .unwrap_err();

        assert!(error.to_string().contains("invalid value for 'mode'"));
        assert_eq!(
            state.action_count, 0,
            "invalid map_info mode must be rejected before counting"
        );
        assert_eq!(state.mutation_count, 0);
    }

    #[test]
    fn map_info_summary_returns_aggregates_without_raw_units() {
        let digest = sample_digest(vec![
            unit(0, "Terran Marine", "P1", 96, 160),
            unit(0, "Terran Marine", "P1", 128, 160),
            unit(65, "Protoss Zealot", "P2", 320, 160),
        ]);

        let value = map_info_view(
            &digest,
            &serde_json::json!({}),
            "C:/maps/demo.scx",
            1_781_000_000,
        )
        .unwrap();

        assert_eq!(value["map"]["path"], "C:/maps/demo.scx");
        assert_eq!(value["map"]["savedAt"], 1_781_000_000u64);
        assert_eq!(value["mode"], "summary");
        assert!(
            value.get("units").is_none(),
            "summary must not return raw units"
        );
        assert_eq!(
            value["summary"]["activePlayers"].as_array().unwrap().len(),
            2
        );
        assert_eq!(value["summary"]["locations"]["count"], 2);
        assert_eq!(
            value["summary"]["locations"]["names"],
            serde_json::json!(["Main", "Anywhere"])
        );
        assert_eq!(value["summary"]["unitsByOwner"]["P1"]["Terran Marine"], 2);
        assert_eq!(value["summary"]["unitsByOwner"]["P2"]["Protoss Zealot"], 1);
    }

    #[test]
    fn map_info_locations_units_and_players_shapes() {
        let digest = sample_digest(vec![
            unit(0, "Terran Marine", "P1", 96, 160),
            unit(65, "Protoss Zealot", "P2", 320, 160),
        ]);

        let locations = map_info_view(
            &digest,
            &serde_json::json!({"mode": "locations"}),
            "demo.scx",
            10,
        )
        .unwrap();
        assert_eq!(locations["map"]["savedAt"], 10);
        assert_eq!(locations["mode"], "locations");
        assert_eq!(locations["locations"].as_array().unwrap().len(), 2);
        assert_eq!(
            locations["locations"][0]["tileRect"],
            serde_json::json!([2, 3, 5, 7])
        );

        let units = map_info_view(
            &digest,
            &serde_json::json!({"mode": "units"}),
            "demo.scx",
            10,
        )
        .unwrap();
        assert_eq!(units["mode"], "units");
        assert_eq!(units["count"], 2);
        assert_eq!(units["units"][0]["type"], "Terran Marine");
        assert!(units.get("truncated").is_none());

        let players = map_info_view(
            &digest,
            &serde_json::json!({"mode": "players"}),
            "demo.scx",
            10,
        )
        .unwrap();
        assert_eq!(players["mode"], "players");
        assert_eq!(players["players"].as_array().unwrap().len(), 3);
        assert_eq!(players["forces"][0]["name"], "Allies");
    }

    #[test]
    fn map_info_units_filters_owner_numeric_id_and_name_substring() {
        let digest = sample_digest(vec![
            unit(0, "Terran Marine", "P1", 96, 160),
            unit(65, "Protoss Zealot", "P2", 320, 160),
            unit(214, "Start Location", "P12 (neutral)", 32, 32),
        ]);

        let owner = map_info_view(
            &digest,
            &serde_json::json!({"mode": "units", "owner": "P2"}),
            "demo.scx",
            10,
        )
        .unwrap();
        assert_eq!(owner["count"], 1);
        assert_eq!(owner["units"][0]["owner"], "P2");

        let neutral = map_info_view(
            &digest,
            &serde_json::json!({"mode": "units", "owner": "neutral"}),
            "demo.scx",
            10,
        )
        .unwrap();
        assert_eq!(neutral["count"], 1);
        assert_eq!(neutral["units"][0]["typeId"], 214);

        let p12 = map_info_view(
            &digest,
            &serde_json::json!({"mode": "units", "owner": "P12"}),
            "demo.scx",
            10,
        )
        .unwrap();
        assert_eq!(p12["count"], 1);
        assert_eq!(p12["units"][0]["owner"], "P12 (neutral)");

        let p1 = map_info_view(
            &digest,
            &serde_json::json!({"mode": "units", "owner": "P1"}),
            "demo.scx",
            10,
        )
        .unwrap();
        assert_eq!(p1["count"], 1);
        assert_eq!(p1["units"][0]["owner"], "P1");

        let numeric = map_info_view(
            &digest,
            &serde_json::json!({"mode": "units", "unitType": "65"}),
            "demo.scx",
            10,
        )
        .unwrap();
        assert_eq!(numeric["count"], 1);
        assert_eq!(numeric["units"][0]["type"], "Protoss Zealot");

        let substring = map_info_view(
            &digest,
            &serde_json::json!({"mode": "units", "unitType": "marine"}),
            "demo.scx",
            10,
        )
        .unwrap();
        assert_eq!(substring["count"], 1);
        assert_eq!(substring["units"][0]["typeId"], 0);
    }

    #[test]
    fn map_info_units_caps_at_200_and_reports_truncation() {
        let units = (0..205)
            .map(|idx| unit(0, "Terran Marine", "P1", idx, 160))
            .collect();
        let digest = sample_digest(units);

        let value = map_info_view(
            &digest,
            &serde_json::json!({"mode": "units", "owner": "P1", "unitType": "Marine"}),
            "demo.scx",
            10,
        )
        .unwrap();

        assert_eq!(value["count"], 205);
        assert_eq!(value["units"].as_array().unwrap().len(), 200);
        assert_eq!(value["truncated"], true);
        assert_eq!(value["dropped"], 5);
        assert!(value["hint"].as_str().unwrap().contains("owner"));
        assert_eq!(value["filters"]["owner"], "P1");
        assert_eq!(value["filters"]["unitType"], "Marine");
    }

    #[test]
    fn map_info_open_map_reply_accepts_bridge_ok_line_and_raw_path() {
        assert_eq!(
            parse_open_map_name_reply("OK: project|OpenMapName = C:/maps/demo.scx\r\n"),
            "C:/maps/demo.scx"
        );
        assert_eq!(
            parse_open_map_name_reply("C:/maps/demo.scx\n"),
            "C:/maps/demo.scx"
        );
    }

    #[test]
    fn mcp_advertisement_uses_real_input_schema_names_verbatim() {
        let descriptors = mcp_tool_descriptors();
        let xdat_set = descriptors
            .iter()
            .find(|descriptor| descriptor["name"] == "xdat_set")
            .expect("xdat_set must be advertised to MCP");

        assert_eq!(
            xdat_set["inputSchema"],
            schema(
                serde_json::json!({
                    "dat": xdat_kinds_schema(),
                    "name": string_schema(),
                    "objId": integer_schema(),
                    "value": numeric_value_schema(),
                }),
                &["dat", "name", "objId", "value"],
            )
        );
        assert!(
            xdat_set.get("parameters").is_none(),
            "MCP advertisement must use inputSchema, not a derived generic parameters wrapper"
        );
        assert!(
            xdat_set["description"]
                .as_str()
                .is_some_and(|description| !description.is_empty()),
            "MCP descriptor must carry the registry description"
        );
    }

    #[test]
    fn admission_blocks_third_mutation_until_plan_is_approved() {
        let mut state = RequestState::for_request("req-mutate");
        state.record_search_docs();

        admit_tool_call(
            &mut state,
            "file_write",
            &serde_json::json!({"path": "a.eps", "code": "1"}),
        )
        .unwrap();
        admit_tool_call(
            &mut state,
            "file_write",
            &serde_json::json!({"path": "b.eps", "code": "2"}),
        )
        .unwrap();

        let error = admit_tool_call(
            &mut state,
            "file_write",
            &serde_json::json!({"path": "c.eps", "code": "3"}),
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("propose_plan"),
            "3rd mutation without a plan must direct codex to propose_plan"
        );
        assert_eq!(state.mutation_count, 2, "rejected mutation must not count");

        state.approve_plan();
        admit_tool_call(
            &mut state,
            "file_write",
            &serde_json::json!({"path": "c.eps", "code": "3"}),
        )
        .unwrap();
        assert_eq!(state.mutation_count, 3);
    }

    #[test]
    fn admission_does_not_record_search_docs_before_execution() {
        let mut state = RequestState::for_request("req-search");

        admit_tool_call(
            &mut state,
            SEARCH_DOCS_TOOL,
            &serde_json::json!({"query": "button set"}),
        )
        .unwrap();

        assert!(!state.docs_searched);
        assert_eq!(state.action_count, 1);
    }

    #[test]
    fn memory_write_skips_mutation_gate_and_counter() {
        let mut state = RequestState::for_request("req-memory");
        state.mutation_count = 2;

        admit_tool_call(
            &mut state,
            MEMORY_WRITE_TOOL,
            &serde_json::json!({"file": "lessons", "content": "remember this"}),
        )
        .unwrap();

        assert_eq!(state.action_count, 1);
        assert_eq!(state.mutation_count, 2);
    }

    #[test]
    fn plugin_add_accepts_append_sentinel_but_remove_rejects_negative_index() {
        let mut state = RequestState::for_request("req-plugin");
        state.record_search_docs();

        admit_tool_call(
            &mut state,
            "plugin_add",
            &serde_json::json!({"index": -1, "texts": "Plugin entry"}),
        )
        .unwrap();

        let error = admit_tool_call(
            &mut state,
            "plugin_remove",
            &serde_json::json!({"index": -1}),
        )
        .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("bounds"));
        assert!(message.contains("non-negative"));
    }

    #[test]
    fn admission_rejects_thirty_first_action_with_wrapup_message() {
        let mut state = RequestState::for_request("req-budget");

        for _ in 0..30 {
            admit_tool_call(&mut state, "project_status", &serde_json::json!({})).unwrap();
        }

        let error =
            admit_tool_call(&mut state, "project_status", &serde_json::json!({})).unwrap_err();
        let message = error.to_string().to_lowercase();
        assert!(
            message.contains("30"),
            "budget error should state the limit"
        );
        assert!(
            message.contains("wrap"),
            "31st action should tell codex to wrap up"
        );
        assert_eq!(state.action_count, 30, "rejected action must not count");
    }

    #[test]
    fn admission_rejects_fourth_build_run_attempt() {
        let mut state = RequestState::for_request("req-build");
        state.approve_plan();

        for _ in 0..3 {
            admit_tool_call(&mut state, BUILD_RUN_TOOL, &serde_json::json!({})).unwrap();
        }

        let error =
            admit_tool_call(&mut state, BUILD_RUN_TOOL, &serde_json::json!({})).unwrap_err();
        let message = error.to_string().to_lowercase();
        assert!(message.contains("build_run") || message.contains("build"));
        assert!(message.contains("3"), "build self-fix budget is 3 attempts");
        assert_eq!(state.build_fix_attempts, 3);
    }

    #[test]
    fn missing_required_arg_error_carries_self_correcting_usage_line() {
        let mut state = RequestState::for_request("req-args");

        let error = admit_tool_call(
            &mut state,
            "xdat_get",
            &serde_json::json!({"table": "units", "field": "ButtonSet", "id": 65}),
        )
        .unwrap_err();
        let message = error.to_string();

        assert!(message.contains("Usage: xdat_get(dat, name, objId)"));
        assert!(message.contains("'dat'"));
        assert!(message.contains("'name'"));
        assert!(message.contains("'objId'"));
        assert_eq!(
            state.action_count, 0,
            "calls rejected by arg validation must not consume an action"
        );
    }

    #[test]
    fn fresh_request_id_resets_per_request_gate_evidence_and_budgets() {
        let mut state = RequestState::for_request("req-A");
        state.record_search_docs();
        state.approve_plan();
        admit_tool_call(
            &mut state,
            "file_write",
            &serde_json::json!({"path": "a.eps", "code": "1"}),
        )
        .unwrap();
        admit_tool_call(&mut state, BUILD_RUN_TOOL, &serde_json::json!({})).unwrap();

        assert_eq!(state.request_id, "req-A");
        assert!(state.docs_searched);
        assert!(state.plan_approved);
        assert_eq!(state.action_count, 2);
        assert_eq!(state.mutation_count, 2);
        assert_eq!(state.build_fix_attempts, 1);

        state.start_request("req-B");

        assert_eq!(state.request_id, "req-B");
        assert!(!state.docs_searched, "evidence gate is per-request");
        assert!(!state.plan_approved, "plan approval is per-request");
        assert_eq!(state.action_count, 0);
        assert_eq!(state.mutation_count, 0);
        assert_eq!(state.build_fix_attempts, 0);

        let error = admit_tool_call(
            &mut state,
            "file_write",
            &serde_json::json!({"path": "b.eps", "code": "2"}),
        )
        .unwrap_err();
        assert!(
            matches!(error, ToolError::EvidenceRequired { .. }),
            "new request must require fresh search_docs evidence before writes"
        );
    }
}
