//! Tool-layer safety rails and per-request evidence state.
//!
//! The functions here are small, deterministic backstops for crash-critical
//! first principles and the EUD-090 evidence requirement.

use serde_json::{json, Value};
use thiserror::Error;

/// Durable project-memory write tool name, exempt from the evidence gate.
pub const MEMORY_WRITE_TOOL: &str = "memory_write";

/// Build verification tool name, exempt from the evidence gate.
pub const BUILD_RUN_TOOL: &str = "build_run";

/// Documentation search tool name.
pub const SEARCH_DOCS_TOOL: &str = "search_docs";

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
        Some(Value::Array(kinds)) => validate_union_type(spec, name, value, kinds),
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
) -> ToolResult<()> {
    let accepts_integer = kinds.iter().any(|kind| kind.as_str() == Some("integer"));
    let accepts_string = kinds.iter().any(|kind| kind.as_str() == Some("string"));

    if accepts_integer && value.as_i64().is_some() {
        return Ok(());
    }

    if accepts_string {
        if let Some(text) = value.as_str() {
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

#[cfg(test)]
mod tests {
    use super::*;

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
