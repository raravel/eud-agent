//! Tool-layer safety rails and per-request evidence state.
//!
//! The functions here are small, deterministic backstops for crash-critical
//! first principles and the EUD-090 evidence requirement.

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use encoding_rs::EUC_KR;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;
use thiserror::Error;

/// Build verification tool name, exempt from the evidence gate.
pub const BUILD_RUN_TOOL: &str = "build_run";
/// Isolated runtime trace test tool name.
pub const TRACE_TEST_RUN_TOOL: &str = "trace_test_run";
/// Source-controlled persistent runtime regression suite tool name.
pub const TRACE_SUITE_RUN_TOOL: &str = "trace_suite_run";

/// Documentation search tool name.
pub const SEARCH_DOCS_TOOL: &str = "search_docs";
/// Exact documentation-chunk lookup tool name.
pub const DOCS_GET_TOOL: &str = "docs_get";
/// Bounded source search tool name.
pub const SOURCE_SEARCH_TOOL: &str = "source_search";
/// Resolve one complete exact direct-Python dependency list into a bounded candidate.
pub const PYTHON_DEPENDENCIES_PREPARE_TOOL: &str = "python_dependencies_prepare";
/// Commit a previously prepared direct-Python dependency candidate.
pub const PYTHON_DEPENDENCIES_SET_TOOL: &str = "python_dependencies_set";
/// Flow-control tool that pauses the current turn for structured user input.
pub const ASK_TOOL: &str = "ask";
/// Bounded wait for one `ask` before it expires into a plain-text handoff.
///
/// Native Codex/Claude CLIs abort a silent MCP call after 300 seconds and
/// progress notifications do not extend it (measured 2026-09-17, `verify.md`),
/// so no single tool call may wait longer than this.
pub const ASK_WAIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(240);

/// Soft action threshold for one foreground iteration.
pub const ITERATION_TOOL_ACTION_THRESHOLD: usize = 300;
const BUILD_PROGRESS_HISTORY: usize = 8;

/// Connected source-map digest tool name.
pub const MAP_INFO_TOOL: &str = "map_info";
/// Connected source-map minimap image tool name.
pub const MAP_MINIMAP_TOOL: &str = "map_minimap";
/// In-place switch rename tool name.
pub const SWITCH_WRITE_TOOL: &str = "switch_write";
/// Read-only registered map sound inventory for main EPS sessions.
pub const MAP_SOUND_LIST_TOOL: &str = "map_sound_list";
/// Request-local canonical audio import into the connected source SCX.
pub const MAP_SOUND_IMPORT_TOOL: &str = "map_sound_import";
/// Re-render one project-managed sound from its immutable source and replace it in the SCX.
pub const MAP_SOUND_EDIT_TOOL: &str = "map_sound_edit";

const MAP_PALETTE_CATALOG_KINDS: [&str; 6] = [
    "brushes",
    "tiles",
    "units",
    "buildings",
    "doodads",
    "sprites",
];

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

/// Stable post-execution build observation used for progress detection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildProgress {
    pub input_revision: String,
    pub diagnostics_fingerprint: String,
    pub error_count: usize,
    pub success: bool,
    pub consecutive_no_progress: u32,
}

/// Result of recording one completed build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildProgressOutcome {
    Progress(BuildProgress),
    NoProgress {
        progress: BuildProgress,
        reason: String,
    },
}

/// Mutable state carried for one agent request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestState {
    /// Stable id for the currently admitted request.
    pub request_id: String,

    /// Set once a `search_docs` call has run successfully, even with zero hits.
    pub docs_searched: bool,

    /// Number of admitted tool actions across the request.
    pub action_count: usize,

    /// Number of admitted actions in the current foreground iteration.
    pub iteration_action_count: usize,
    /// Total admitted read/project-operation calls for lifecycle reporting.
    pub read_action_count: u64,

    /// Total admitted canonical authoring calls for lifecycle reporting.
    pub write_action_count: u64,

    /// Number of admitted `search_docs` calls in this request.
    pub search_docs_count: usize,

    /// Number of documentation hits returned across discovery calls.
    pub search_docs_returned_hits: usize,

    /// Number of first-seen documentation hits in this request.
    pub search_docs_unique_hits: usize,

    /// Number of already-seen documentation hits returned again.
    pub search_docs_repeated_hits: usize,

    /// Serialized discovery-result bytes returned to the provider.
    pub search_docs_result_bytes: usize,

    /// Number of exact `docs_get` calls in this request.
    pub docs_get_count: usize,

    /// Number of exact documentation chunks returned by `docs_get`.
    pub docs_get_documents: usize,

    /// Serialized exact-document bytes returned to the provider.
    pub docs_get_result_bytes: usize,

    seen_doc_ids: BTreeSet<u64>,
    completed_searches: BTreeMap<String, BTreeSet<u64>>,
    build_history: VecDeque<BuildProgress>,
    blocked_build_revision: Option<String>,
}

impl RequestState {
    /// Create request state with the evidence search flag unset.
    pub fn new() -> Self {
        Self::for_request("")
    }

    /// Create clean state for a specific request id.
    pub fn for_request(id: &str) -> Self {
        Self {
            request_id: id.to_string(),
            docs_searched: false,
            action_count: 0,
            iteration_action_count: 0,
            read_action_count: 0,
            write_action_count: 0,
            search_docs_count: 0,
            search_docs_returned_hits: 0,
            search_docs_unique_hits: 0,
            search_docs_repeated_hits: 0,
            search_docs_result_bytes: 0,
            docs_get_count: 0,
            docs_get_documents: 0,
            docs_get_result_bytes: 0,
            seen_doc_ids: BTreeSet::new(),
            completed_searches: BTreeMap::new(),
            build_history: VecDeque::with_capacity(BUILD_PROGRESS_HISTORY),
            blocked_build_revision: None,
        }
    }

    /// Start a fresh request, resetting all request-scoped authority and progress.
    pub fn start_request(&mut self, id: &str) {
        *self = Self::for_request(id);
    }

    /// Start one soft-bounded foreground iteration without resetting request state.
    pub fn begin_iteration(&mut self) {
        self.iteration_action_count = 0;
    }

    pub fn iteration_action_boundary_reached(&self) -> bool {
        self.iteration_action_count >= ITERATION_TOOL_ACTION_THRESHOLD
    }

    pub fn restore_autonomous_progress(
        &mut self,
        read_actions: u64,
        write_actions: u64,
        latest_build: Option<BuildProgress>,
    ) {
        self.read_action_count = read_actions;
        self.write_action_count = write_actions;
        self.action_count =
            usize::try_from(read_actions.saturating_add(write_actions)).unwrap_or(usize::MAX);
        if let Some(build) = latest_build {
            if build.consecutive_no_progress > 0 && !build.success {
                self.blocked_build_revision = Some(build.input_revision.clone());
            }
            self.build_history.push_back(build);
        }
    }

    /// Record that `search_docs` ran successfully for this request.
    pub fn record_search_docs(&mut self) {
        self.docs_searched = true;
    }

    pub fn search_identity(args: &Value) -> String {
        let query = args
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_lowercase)
            .collect::<Vec<_>>()
            .join(" ");
        let k = args.get("k").and_then(Value::as_i64).unwrap_or(5);
        format!("{query}\u{1f}{k}")
    }

    pub fn repeated_search_ids(&self, args: &Value) -> Option<&BTreeSet<u64>> {
        self.completed_searches.get(&Self::search_identity(args))
    }

    /// Record discovery hits and return one repeated flag per id.
    pub fn record_search_docs_hits(&mut self, args: &Value, ids: &[u64]) -> Vec<bool> {
        self.record_search_docs();
        self.search_docs_count = self.search_docs_count.saturating_add(1);
        self.search_docs_returned_hits = self.search_docs_returned_hits.saturating_add(ids.len());
        let repeated = ids
            .iter()
            .map(|id| {
                let repeated = !self.seen_doc_ids.insert(*id);
                if repeated {
                    self.search_docs_repeated_hits =
                        self.search_docs_repeated_hits.saturating_add(1);
                } else {
                    self.search_docs_unique_hits = self.search_docs_unique_hits.saturating_add(1);
                }
                repeated
            })
            .collect();
        self.completed_searches
            .insert(Self::search_identity(args), ids.iter().copied().collect());
        repeated
    }

    /// Record the final serialized discovery payload size.
    pub fn record_search_docs_payload(&mut self, bytes: usize) {
        self.search_docs_result_bytes = self.search_docs_result_bytes.saturating_add(bytes);
    }

    /// Record an exact-document response.
    pub fn record_docs_get(&mut self, documents: usize, bytes: usize) {
        self.docs_get_count = self.docs_get_count.saturating_add(1);
        self.docs_get_documents = self.docs_get_documents.saturating_add(documents);
        self.docs_get_result_bytes = self.docs_get_result_bytes.saturating_add(bytes);
    }

    pub fn build_blocked_for_revision(&self, revision: &str) -> bool {
        self.blocked_build_revision.as_deref() == Some(revision)
    }

    pub fn latest_build(&self) -> Option<&BuildProgress> {
        self.build_history.back()
    }

    pub fn record_build(
        &mut self,
        input_revision: String,
        result: &crate::native_build::NativeBuildResult,
    ) -> BuildProgressOutcome {
        let diagnostics_fingerprint = build_diagnostics_fingerprint(&result.errors);
        let error_count = result.errors.len();
        let pair_repeated = self.build_history.back().is_some_and(|previous| {
            previous.input_revision == input_revision
                && previous.diagnostics_fingerprint == diagnostics_fingerprint
                && !result.ok
        });
        let oscillating = self.build_history.len() >= 3
            && self.build_history[self.build_history.len() - 3].input_revision
                == self.build_history[self.build_history.len() - 1].input_revision
            && self.build_history[self.build_history.len() - 3].diagnostics_fingerprint
                == self.build_history[self.build_history.len() - 1].diagnostics_fingerprint
            && self.build_history[self.build_history.len() - 2].input_revision == input_revision
            && self.build_history[self.build_history.len() - 2].diagnostics_fingerprint
                == diagnostics_fingerprint
            && !result.ok;
        let previous_no_progress = self
            .build_history
            .back()
            .map_or(0, |previous| previous.consecutive_no_progress);
        let no_progress = pair_repeated || oscillating;
        let progress = BuildProgress {
            input_revision: input_revision.clone(),
            diagnostics_fingerprint,
            error_count,
            success: result.ok,
            consecutive_no_progress: if result.ok {
                0
            } else if no_progress {
                previous_no_progress.saturating_add(1)
            } else {
                0
            },
        };
        if self.build_history.len() == BUILD_PROGRESS_HISTORY {
            self.build_history.pop_front();
        }
        self.build_history.push_back(progress.clone());
        if no_progress {
            self.blocked_build_revision = Some(input_revision);
            BuildProgressOutcome::NoProgress {
                progress,
                reason: if oscillating {
                    "최근 빌드 상태가 A → B → A → B 형태로 반복되었습니다. 입력을 실제로 진전시킨 뒤 다시 빌드하세요."
                        .to_string()
                } else {
                    "같은 프로젝트 revision에서 같은 빌드 진단이 반복되었습니다. 진단을 확인하고 입력을 변경한 뒤 다시 빌드하세요."
                        .to_string()
                },
            }
        } else {
            self.blocked_build_revision = None;
            BuildProgressOutcome::Progress(progress)
        }
    }
}

impl Default for RequestState {
    fn default() -> Self {
        Self::new()
    }
}

/// Minimal tool metadata needed by workspace and project-operation admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub requires_write_workspace: bool,
    pub requires_project_transaction: bool,
    pub input_schema: Value,
}

impl ToolSpec {
    pub fn canonical_mutation(name: &'static str) -> Self {
        Self {
            name,
            description: "",
            requires_write_workspace: true,
            requires_project_transaction: true,
            input_schema: empty_schema(),
        }
    }

    pub fn read_only(name: &'static str) -> Self {
        Self {
            name,
            description: "",
            requires_write_workspace: false,
            requires_project_transaction: false,
            input_schema: empty_schema(),
        }
    }
}

fn read_tool(name: &'static str, description: &'static str, input_schema: Value) -> ToolSpec {
    ToolSpec {
        name,
        description,
        requires_write_workspace: false,
        requires_project_transaction: false,
        input_schema,
    }
}

fn canonical_tool(name: &'static str, description: &'static str, input_schema: Value) -> ToolSpec {
    ToolSpec {
        name,
        description,
        requires_write_workspace: true,
        requires_project_transaction: true,
        input_schema,
    }
}

fn project_operation_tool(
    name: &'static str,
    description: &'static str,
    input_schema: Value,
) -> ToolSpec {
    ToolSpec {
        name,
        description,
        requires_write_workspace: false,
        requires_project_transaction: true,
        input_schema,
    }
}

fn build_diagnostics_fingerprint(errors: &[crate::native_build::NativeBuildError]) -> String {
    let mut diagnostics = errors
        .iter()
        .map(|error| {
            format!(
                "{}\u{1f}{}\u{1f}{}\u{1f}{}",
                normalize_build_path(&error.source),
                normalize_build_path(&error.file),
                error.line,
                normalize_build_message(&error.message),
            )
        })
        .collect::<Vec<_>>();
    diagnostics.sort_unstable();
    let digest = Sha256::digest(diagnostics.join("\u{1e}").as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn normalize_build_path(path: &str) -> String {
    let normalized = path.trim().replace('\\', "/");
    let parts = normalized
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect::<Vec<_>>();
    let logical_start = parts
        .iter()
        .position(|part| matches!(part.to_ascii_lowercase().as_str(), "src" | "tests"));
    let logical = logical_start
        .map(|index| parts[index..].join("/"))
        .or_else(|| parts.last().map(|part| (*part).to_string()))
        .unwrap_or_default();
    logical.to_ascii_lowercase()
}

fn normalize_build_message(message: &str) -> String {
    message
        .split_whitespace()
        .map(|token| {
            let slash = token.replace('\\', "/");
            let unwrapped = slash.trim_matches(|character: char| {
                matches!(character, '"' | '\'' | '(' | ')' | '[' | ']' | ',' | ';')
            });
            let lower = unwrapped.to_ascii_lowercase();
            let digit_count = lower.bytes().filter(u8::is_ascii_digit).count();
            if lower.starts_with('/')
                || lower.get(1..3) == Some(":/")
                || lower.contains("/appdata/local/temp/")
            {
                "<path>".to_string()
            } else if digit_count >= 6
                && lower.contains(':')
                && (lower.contains('-') || lower.contains('t'))
            {
                "<time>".to_string()
            } else {
                lower
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn empty_schema() -> Value {
    schema(json!({}), &[])
}

fn schema(properties: Value, required: &[&str]) -> Value {
    object_schema(properties, required)
}

fn object_schema(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn object_array_schema(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "array",
        "minItems": 1,
        "items": object_schema(properties, required),
    })
}

fn optional_string_array_schema(max_items: usize) -> Value {
    json!({
        "type": "array",
        "minItems": 0,
        "maxItems": max_items,
        "items": string_schema(),
    })
}

fn string_array_schema(max_items: usize) -> Value {
    json!({
        "type": "array",
        "minItems": 1,
        "maxItems": max_items,
        "items": string_schema(),
    })
}

fn string_schema() -> Value {
    json!({"type": "string"})
}

fn integer_schema() -> Value {
    json!({"type": "integer"})
}
fn render_scale_schema() -> Value {
    json!({
        "type": "integer",
        "enum": [1, 2, 4, 8],
        "description": "Render scale must be one of 1, 2, 4, or 8.",
    })
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

fn map_palette_catalog_kind_schema() -> Value {
    let mut kind = enum_string_schema(&MAP_PALETTE_CATALOG_KINDS);
    kind["description"] = json!(
        "Catalog family, not PaletteRef.kind. Use brushes for semanticTerrain, tiles for exactTile, and the plural object family for unit/building/doodad/sprite."
    );
    kind
}

fn exact_text_edits_schema() -> Value {
    object_array_schema(
        json!({
            "old_text": string_schema(),
            "new_text": string_schema(),
        }),
        &["old_text", "new_text"],
    )
}

fn ask_questions_schema() -> Value {
    let options = json!({
        "type": "array",
        "minItems": 2,
        "maxItems": 5,
        "items": object_schema(
            json!({
                "label": string_schema(),
                "description": string_schema(),
            }),
            &["label"],
        ),
    });
    json!({
        "type": "array",
        "minItems": 1,
        "maxItems": 4,
        "items": object_schema(
            json!({
                "id": string_schema(),
                "header": string_schema(),
                "question": string_schema(),
                "options": options,
                "multi": {"type": "boolean"},
            }),
            &["id", "question"],
        ),
    })
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

fn dat_patch_changes_schema() -> Value {
    let numeric = |kind: &str, dat: Value| {
        object_schema(
            json!({
                "kind": {"const": kind},
                "dat": dat,
                "objectId": integer_schema(),
                "field": string_schema(),
                "before": numeric_value_schema(),
                "after": numeric_value_schema(),
            }),
            &["kind", "dat", "objectId", "field", "before", "after"],
        )
    };
    json!({
        "type": "array",
        "minItems": 1,
        "maxItems": crate::native_project::MAX_DAT_PATCH_CHANGES,
        "items": {
            "oneOf": [
                numeric("dat", dat_names_schema()),
                numeric("xdat", xdat_kinds_schema()),
                object_schema(
                    json!({
                        "kind": {"const": "tbl"},
                        "index": integer_schema(),
                        "before": string_schema(),
                        "after": string_schema(),
                    }),
                    &["kind", "index", "before", "after"],
                ),
                object_schema(
                    json!({
                        "kind": {"const": "requirement"},
                        "dat": req_dats_schema(),
                        "objectId": integer_schema(),
                        "before": string_schema(),
                        "after": string_schema(),
                    }),
                    &["kind", "dat", "objectId", "before", "after"],
                ),
                object_schema(
                    json!({
                        "kind": {"const": "button"},
                        "setId": integer_schema(),
                        "before": string_schema(),
                        "after": string_schema(),
                    }),
                    &["kind", "setId", "before", "after"],
                ),
            ]
        }
    })
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
        read_tool(
            "project_status",
            "Read current native project status and the exact configured mainFile path.",
            empty_schema(),
        ),
        read_tool(
            "list_files",
            "List editable project files.",
            empty_schema(),
        ),
        read_tool(
            "read_file",
            "Read an editable project file, optionally by inclusive 1-based line range.",
            schema(
                json!({
                    "path": string_schema(),
                    "startLine": integer_schema(),
                    "endLine": integer_schema(),
                }),
                &["path"],
            ),
        ),
        read_tool(
            SOURCE_SEARCH_TOOL,
            "Search editable project source with exact paged excerpts; native read-only shell remains available as a fallback.",
            schema(
                json!({
                    "query": string_schema(),
                    "paths": string_array_schema(64),
                    "contextLines": integer_schema(),
                    "offset": integer_schema(),
                    "limit": integer_schema(),
                }),
                &["query"],
            ),
        ),
        read_tool(
            PYTHON_DEPENDENCIES_PREPARE_TOOL,
            "Resolve a complete exact direct-Python dependency list without changing project state.",
            schema(
                json!({"dependencies": optional_string_array_schema(128)}),
                &["dependencies"],
            ),
        ),
        read_tool(
            "dat_get",
            "Read one or more DAT field values.",
            schema(
                json!({
                    "items": object_array_schema(
                        json!({
                            "dat": dat_names_schema(),
                            "param": string_schema(),
                            "objId": integer_schema(),
                        }),
                        &["dat", "param", "objId"],
                    ),
                }),
                &["items"],
            ),
        ),
        read_tool(
            "xdat_get",
            "Read one or more extended DAT field values.",
            schema(
                json!({
                    "items": object_array_schema(
                        json!({
                            "dat": xdat_kinds_schema(),
                            "name": string_schema(),
                            "objId": integer_schema(),
                        }),
                        &["dat", "name", "objId"],
                    ),
                }),
                &["items"],
            ),
        ),
        read_tool(
            "tbl_get",
            "Read one or more TBL strings by index.",
            schema(
                json!({
                    "items": object_array_schema(
                        json!({"index": integer_schema()}),
                        &["index"],
                    ),
                }),
                &["items"],
            ),
        ),
        read_tool(
            "req_get",
            "Read one or more requirements payloads.",
            schema(
                json!({
                    "items": object_array_schema(
                        json!({
                            "dat": req_dats_schema(),
                            "objId": integer_schema(),
                        }),
                        &["dat", "objId"],
                    ),
                }),
                &["items"],
            ),
        ),
        read_tool(
            "btn_get",
            "Read one or more button set CSV payloads.",
            schema(
                json!({
                    "items": object_array_schema(
                        json!({"setId": integer_schema()}),
                        &["setId"],
                    ),
                }),
                &["items"],
            ),
        ),
        read_tool(
            "settings_get",
            "Read an agent setting.",
            schema(
                json!({
                    "scope": settings_scopes_schema(),
                    "key": string_schema(),
                }),
                &["scope", "key"],
            ),
        ),
        read_tool(
            MAP_INFO_TOOL,
            "Read paged connected-map terrain, placements, switches, players, and forces.",
            schema(
                json!({
                    "mode": enum_string_schema(&[
                        "summary",
                        "terrain",
                        "locations",
                        "units",
                        "players",
                        "switches",
                    ]),
                    "owner": map_info_owner_schema(),
                    "unitType": integer_or_string_schema(),
                    "switch": integer_or_string_schema(),
                    "x": integer_schema(),
                    "y": integer_schema(),
                    "width": integer_schema(),
                    "height": integer_schema(),
                    "offset": integer_schema(),
                    "limit": integer_schema(),
                }),
                &[],
            ),
        ),
        read_tool(
            MAP_SOUND_LIST_TOOL,
            "List registered map WAV slots and exact MPQ paths from the connected saved SCX.",
            empty_schema(),
        ),
        read_tool(
            MAP_MINIMAP_TOOL,
            "Render the connected map as PNG terrain with an optional unit overlay.",
            schema(
                json!({
                    "maxSize": integer_schema(),
                    "showUnits": {"type": "boolean"},
                    "starcraftPath": string_schema(),
                }),
                &[],
            ),
        ),
        read_tool(
            "plugins_list",
            "List configured plugins.",
            empty_schema(),
        ),
        read_tool(
            SEARCH_DOCS_TOOL,
            "Discover ranked reference chunks as compact exact previews. Inspect promising ids with docs_get; repeat only when the normalized query or returned stable ids can add new evidence.",
            schema(
                json!({
                    "query": string_schema(),
                    "k": integer_schema(),
                }),
                &["query"],
            ),
        ),
        read_tool(
            DOCS_GET_TOOL,
            "Read selected search_docs chunks exactly by stable hexadecimal id.",
            schema(json!({"ids": string_array_schema(10)}), &["ids"]),
        ),
        read_tool(
            ASK_TOOL,
            "Pause this turn to ask the user up to four related questions. Each question supports single or multiple choice and always allows direct input. The user has 240 seconds to answer in the panel. A result with status \"unanswered\" means no answer arrived in time: restate the same questions as your final plain-text answer, end the turn, and treat the user's next message as the answer. Never call ask again after an unanswered result in the same turn.",
            schema(json!({"questions": ask_questions_schema()}), &["questions"]),
        ),
        canonical_tool(
            PYTHON_DEPENDENCIES_SET_TOOL,
            "Commit one prepared direct-Python dependency candidate by its opaque token.",
            schema(
                json!({"candidateToken": string_schema()}),
                &["candidateToken"],
            ),
        ),
        canonical_tool(
            MAP_SOUND_IMPORT_TOOL,
            "Import one request-local audioRef as canonical OGG into the connected saved SCX.",
            schema(json!({"audioRef": string_schema()}), &["audioRef"]),
        ),
        canonical_tool(
            MAP_SOUND_EDIT_TOOL,
            "Apply persistent volume/fade settings to one managed sound, replace its SCX registration, and return the new exact MPQ path.",
            schema(
                json!({
                    "mpqPath": string_schema(),
                    "audioRef": string_schema(),
                    "volumePercent": {"type": "integer", "minimum": 0, "maximum": 400},
                    "fadeInMs": {"type": "integer", "minimum": 0, "maximum": 3_600_000},
                    "fadeOutMs": {"type": "integer", "minimum": 0, "maximum": 3_600_000},
                }),
                &["mpqPath"],
            ),
        ),
        canonical_tool(
            "dat_patch",
            "Atomically validate and stage one complete native DAT/XDAT/TBL/requirements/buttons changeset.",
            schema(json!({"changes": dat_patch_changes_schema()}), &["changes"]),
        ),
        canonical_tool(
            "file_create",
            "Create a project file.",
            schema(
                json!({
                    "path": string_schema(),
                    "ftype": enum_string_schema(&["CUIEps", "CUIPy"]),
                    "code": string_schema(),
                }),
                &["path", "ftype"],
            ),
        ),
        canonical_tool(
            "file_write",
            "Overwrite a project file.",
            schema(
                json!({
                    "path": string_schema(),
                    "code": string_schema(),
                }),
                &["path", "code"],
            ),
        ),
        canonical_tool(
            "file_edit",
            "Apply ordered exact-text edits to an existing project file.",
            schema(
                json!({
                    "path": string_schema(),
                    "edits": exact_text_edits_schema(),
                }),
                &["path", "edits"],
            ),
        ),
        canonical_tool(
            "file_rename",
            "Rename a project file.",
            schema(
                json!({
                    "path": string_schema(),
                    "newname": string_schema(),
                }),
                &["path", "newname"],
            ),
        ),
        canonical_tool(
            "file_delete",
            "Delete a project file.",
            schema(json!({"path": string_schema()}), &["path"]),
        ),
        canonical_tool(
            "file_move",
            "Move a project file to another folder.",
            schema(
                json!({
                    "path": string_schema(),
                    "destFolder": string_schema(),
                }),
                &["path"],
            ),
        ),
        canonical_tool(
            "mkdir",
            "Create a project folder.",
            schema(json!({"path": string_schema()}), &["path"]),
        ),
        canonical_tool(
            "set_main",
            "Set the main project file.",
            schema(json!({"path": string_schema()}), &["path"]),
        ),
        canonical_tool(
            "settings_set",
            "Write an agent setting.",
            schema(
                json!({
                    "scope": settings_scopes_schema(),
                    "key": string_schema(),
                    "value": string_schema(),
                }),
                &["scope", "key", "value"],
            ),
        ),
        canonical_tool(
            "plugin_add",
            "Add a plugin entry.",
            schema(
                json!({
                    "index": integer_schema(),
                    "texts": string_schema(),
                }),
                &[],
            ),
        ),
        canonical_tool(
            "plugin_edit",
            "Edit a plugin entry.",
            schema(
                json!({
                    "index": integer_schema(),
                    "texts": string_schema(),
                }),
                &["index"],
            ),
        ),
        canonical_tool(
            "plugin_remove",
            "Remove a plugin entry.",
            schema(json!({"index": integer_schema()}), &["index"]),
        ),
        canonical_tool(
            "plugin_move",
            "Move a plugin entry.",
            schema(
                json!({
                    "from": integer_schema(),
                    "to": integer_schema(),
                }),
                &["from", "to"],
            ),
        ),
        project_operation_tool(
            BUILD_RUN_TOOL,
            "Generate deterministic native build artifacts, run configured euddraft, require a fresh output map, and return structured diagnostics.",
            empty_schema(),
        ),
        read_tool(
            TRACE_TEST_RUN_TOOL,
            "Build and run one isolated epScript runtime test after a successful build_run. The owned 32-bit client is created suspended; a bounded x86 helper validates StarCraft.exe and neutralizes only its foreground/focus/cursor user32 calls before resume. The minimized off-screen client then receives LAN/UDP CreateGame and Alt+O through background window messages, with no global input or focus fallback. The source project/map remain unchanged.",
            schema(
                json!({
                    "name": {"type": "string", "minLength": 1, "maxLength": 512},
                    "code": {"type": "string", "minLength": 1, "maxLength": 131072},
                    "symbols": {
                        "type": "array",
                        "maxItems": 256,
                        "items": {
                            "type": "object",
                            "additionalProperties": false,
                            "properties": {
                                "eventId": {"type": "integer", "minimum": 0, "maximum": 4294967295_u64},
                                "name": {"type": "string", "minLength": 1, "maxLength": 128},
                                "source": {"type": "string", "maxLength": 512},
                                "line": {"type": "integer", "minimum": 1, "maximum": 4294967295_u64}
                            },
                            "required": ["eventId", "name"]
                        }
                    },
                    "timeoutMs": {"type": "integer", "minimum": 1000, "maximum": 120000}
                }),
                &["name", "code"],
            ),
        ),
        read_tool(
            TRACE_SUITE_RUN_TOOL,
            "Discover and run up to 256 source-controlled src/tests/**/*.tests.eps after a successful build_run. Omit tests for the complete suite or provide exact logical project paths. Every owned 32-bit client is suspended until a bounded x86 helper neutralizes its foreground/focus/cursor user32 calls, then remains minimized and uses only background messages for LAN/UDP CreateGame and Alt+O. Results are diagnostic and the source project/map remain unchanged.",
            schema(
                json!({
                    "tests": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 256,
                        "items": {"type": "string", "minLength": 1, "maxLength": 512}
                    },
                    "timeoutMs": {"type": "integer", "minimum": 1000, "maximum": 120000}
                }),
                &[],
            ),
        ),
        canonical_tool(
            "location_write",
            "Write map location data.",
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
        canonical_tool(
            "player_setup",
            "Write player start or controller data.",
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
        canonical_tool(
            SWITCH_WRITE_TOOL,
            "Rename one connected-map switch in place without changing numeric trigger references.",
            schema(
                json!({
                    "action": enum_string_schema(&["rename"]),
                    "switchId": integer_schema(),
                    "name": string_schema(),
                }),
                &["action", "switchId", "name"],
            ),
        ),
        read_tool(
            "propose_plan",
            "Propose a plan for approval only when the user explicitly requested a plan in this turn; staged requests plan through their own stage.",
            schema(json!({"markdown": string_schema()}), &["markdown"]),
        ),
    ]
}
fn u8_schema() -> Value {
    json!({"type": "integer", "minimum": 0, "maximum": 255})
}

fn u16_schema() -> Value {
    json!({"type": "integer", "minimum": 0, "maximum": 65_535})
}

fn positive_u16_schema() -> Value {
    json!({"type": "integer", "minimum": 1, "maximum": 65_535})
}

fn u32_schema() -> Value {
    json!({"type": "integer", "minimum": 0, "maximum": 4_294_967_295_u64})
}

fn i32_schema() -> Value {
    json!({
        "type": "integer",
        "minimum": -2_147_483_648_i64,
        "maximum": 2_147_483_647
    })
}

fn defaulted(mut schema: Value, default: Value) -> Value {
    schema["default"] = default;
    schema
}

fn tile_rows_schema() -> Value {
    json!({
        "type": "array",
        "minItems": 1,
        "items": {
            "type": "array",
            "minItems": 1,
            "items": u16_schema(),
        },
    })
}

fn unit_properties_schema(state_defaults: bool) -> Value {
    let optional = |schema, default| {
        if state_defaults {
            defaulted(schema, default)
        } else {
            schema
        }
    };
    json!({
        "typeId": u16_schema(),
        "owner": u8_schema(),
        "x": u16_schema(),
        "y": u16_schema(),
        "classId": optional(u32_schema(), json!(0)),
        "relationFlags": optional(u16_schema(), json!(0)),
        "validStateFlags": optional(u16_schema(), json!(0)),
        "validFieldFlags": optional(u16_schema(), json!(0)),
        "hpPercent": optional(u8_schema(), json!(100)),
        "shieldPercent": optional(u8_schema(), json!(100)),
        "energyPercent": optional(u8_schema(), json!(100)),
        "resourceAmount": optional(u32_schema(), json!(0)),
        "hangarAmount": optional(u16_schema(), json!(0)),
        "stateFlags": optional(u16_schema(), json!(0)),
        "unused": optional(u32_schema(), json!(0)),
        "relationClassId": optional(u32_schema(), json!(0)),
    })
}

fn unit_state_schema() -> Value {
    object_schema(unit_properties_schema(true), &["typeId", "owner", "x", "y"])
}

fn unit_patch_schema() -> Value {
    object_schema(unit_properties_schema(false), &[])
}

fn doodad_state_schema() -> Value {
    object_schema(
        json!({
            "doodadId": u16_schema(),
            "x": u16_schema(),
            "y": u16_schema(),
            "owner": defaulted(u8_schema(), json!(11)),
            "disabled": defaulted(json!({"type": "boolean"}), json!(false)),
        }),
        &["doodadId", "x", "y"],
    )
}

fn sprite_state_schema() -> Value {
    object_schema(
        json!({
            "spriteId": u16_schema(),
            "x": u16_schema(),
            "y": u16_schema(),
            "owner": defaulted(u8_schema(), json!(11)),
            "flags": defaulted(u16_schema(), json!(0)),
        }),
        &["spriteId", "x", "y"],
    )
}

fn location_state_schema() -> Value {
    object_schema(
        json!({
            "locationId": u16_schema(),
            "left": i32_schema(),
            "top": i32_schema(),
            "right": i32_schema(),
            "bottom": i32_schema(),
            "elevationFlags": defaulted(u16_schema(), json!(0)),
            "nameBytesHex": string_schema(),
        }),
        &["locationId", "left", "top", "right", "bottom"],
    )
}

fn replace_closed_objects_with_property_names(value: &mut Value) {
    match value {
        Value::Array(values) => values
            .iter_mut()
            .for_each(replace_closed_objects_with_property_names),
        Value::Object(map) => {
            for value in map.values_mut() {
                replace_closed_objects_with_property_names(value);
            }
            if map.get("additionalProperties") == Some(&Value::Bool(false)) {
                let property_names = map
                    .get("properties")
                    .and_then(Value::as_object)
                    .map(|properties| properties.keys().cloned().collect::<Vec<_>>())
                    .unwrap_or_default();
                map.remove("additionalProperties");
                map.insert("propertyNames".to_string(), json!({"enum": property_names}));
            }
        }
        _ => {}
    }
}

fn map_operation_leaf(
    names: &[&str],
    mut properties: Value,
    required: &[&str],
    inherited_properties: &[&str],
) -> Value {
    properties
        .as_object_mut()
        .expect("map operation properties must be an object")
        .insert(
            "op".to_string(),
            if names.len() == 1 {
                json!({"const": names[0]})
            } else {
                enum_string_schema(names)
            },
        );
    let mut leaf = object_schema(properties, required);
    replace_closed_objects_with_property_names(&mut leaf);
    let mut allowed = schema_string_set_from_properties(&leaf);
    allowed.extend(inherited_properties.iter().map(|name| (*name).to_string()));
    leaf["propertyNames"] = json!({"enum": allowed});
    leaf
}

fn schema_string_set_from_properties(schema: &Value) -> Vec<String> {
    schema["properties"]
        .as_object()
        .expect("map operation properties must be an object")
        .keys()
        .cloned()
        .collect()
}

fn map_operation_schema() -> Value {
    let terrain_xy = json!({
        "type": "object",
        "properties": {"x": u16_schema(), "y": u16_schema()},
        "required": ["x", "y"],
    });
    let terrain = json!({"allOf": [terrain_xy, {"oneOf": [
        map_operation_leaf(
            &["terrain.set"],
            json!({"before": u16_schema(), "after": u16_schema()}),
            &["before", "after"],
            &["x", "y"],
        ),
        map_operation_leaf(
            &["terrain.rect"],
            json!({"width": u16_schema(), "height": u16_schema(), "after": u16_schema()}),
            &["width", "height", "after"],
            &["x", "y"],
        ),
        map_operation_leaf(
            &["terrain.blit"],
            json!({"tiles": {"$ref": "#/$defs/t"}}),
            &["tiles"],
            &["x", "y"],
        ),
    ]}]});

    let target_identity = json!({
        "type": "object",
        "properties": {
            "ordinal": u32_schema(),
            "beforeFingerprint": string_schema(),
        },
        "required": ["ordinal", "beforeFingerprint"],
    });
    let target_operations = json!({"allOf": [target_identity, {"oneOf": [
        map_operation_leaf(
            &["unit.set"],
            json!({"state": unit_patch_schema()}),
            &["state"],
            &["ordinal", "beforeFingerprint"],
        ),
        map_operation_leaf(
            &["unit.delete", "sprite.delete"],
            json!({}),
            &[],
            &["ordinal", "beforeFingerprint"],
        ),
        map_operation_leaf(
            &["unit.move", "sprite.move"],
            json!({"x": u16_schema(), "y": u16_schema()}),
            &["x", "y"],
            &["ordinal", "beforeFingerprint"],
        ),
        map_operation_leaf(
            &["doodad.set"],
            json!({
                "state": {"$ref": "#/$defs/d"},
                "replacementTiles": {"$ref": "#/$defs/t"},
            }),
            &["state", "replacementTiles"],
            &["ordinal", "beforeFingerprint"],
        ),
        map_operation_leaf(
            &["doodad.delete"],
            json!({"replacementTiles": {"$ref": "#/$defs/t"}}),
            &["replacementTiles"],
            &["ordinal", "beforeFingerprint"],
        ),
        map_operation_leaf(
            &["doodad.move"],
            json!({
                "x": u16_schema(),
                "y": u16_schema(),
                "replacementTiles": {"$ref": "#/$defs/t"},
            }),
            &["x", "y", "replacementTiles"],
            &["ordinal", "beforeFingerprint"],
        ),
        map_operation_leaf(
            &["sprite.set"],
            json!({"state": {"$ref": "#/$defs/s"}}),
            &["state"],
            &["ordinal", "beforeFingerprint"],
        ),
    ]}]});

    json!({"allOf": [
        {
            "type": "object",
            "properties": {"op": string_schema()},
            "required": ["op"],
        },
        {"oneOf": [
            terrain,
            map_operation_leaf(
                &["terrain.isom_brush"],
                json!({
                    "isomX": u16_schema(),
                    "isomY": u16_schema(),
                    "brush": u16_schema(),
                    "extent": defaulted(u16_schema(), json!(1)),
                }),
                &["isomX", "isomY", "brush"],
                &[],
            ),
            map_operation_leaf(
                &["unit.add"],
                json!({"state": unit_state_schema()}),
                &["state"],
                &[],
            ),
            map_operation_leaf(
                &["doodad.add"],
                json!({"state": {"$ref": "#/$defs/d"}}),
                &["state"],
                &[],
            ),
            map_operation_leaf(
                &["sprite.add"],
                json!({"state": {"$ref": "#/$defs/s"}}),
                &["state"],
                &[],
            ),
            target_operations,
            map_operation_leaf(
                &["location.add", "location.set"],
                json!({"state": location_state_schema()}),
                &["state"],
                &[],
            ),
            map_operation_leaf(
                &["location.rename"],
                json!({"locationId": u16_schema(), "nameBytesHex": string_schema()}),
                &["locationId", "nameBytesHex"],
                &[],
            ),
            map_operation_leaf(
                &["location.delete"],
                json!({"locationId": u16_schema()}),
                &["locationId"],
                &[],
            ),
        ]},
    ]})
}

fn map_operations_schema() -> Value {
    json!({
        "type": "array",
        "minItems": 1,
        "maxItems": 4096,
        "items": map_operation_schema(),
    })
}

/// One documented `map_draft_patch` operation family: its discriminator
/// values and the exact top-level keys the closed leaf schema admits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MapOperationShape {
    pub ops: Vec<String>,
    pub fields: Vec<MapOperationField>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MapOperationField {
    pub name: String,
    pub required: bool,
    /// Advertised JSON type plus integer bounds when the schema declares them.
    pub kind: Option<String>,
    pub minimum: Option<i64>,
    pub maximum: Option<i64>,
    /// Required and optional keys of a closed nested object such as `state`.
    pub nested: Option<(Vec<String>, Vec<String>)>,
}

impl MapOperationField {
    /// Describe a value that cannot satisfy this field's advertised type or
    /// integer bounds; `None` when the value is acceptable at this level.
    fn mismatch(&self, value: &Value) -> Option<String> {
        let kind = self.kind.as_deref()?;
        let actual = match value {
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        };
        let expected = match (kind, self.minimum, self.maximum) {
            ("integer", Some(minimum), Some(maximum)) => {
                format!("integer {minimum}..={maximum}")
            }
            ("integer", Some(minimum), None) => format!("integer >= {minimum}"),
            _ => kind.to_string(),
        };
        let acceptable = match kind {
            "integer" => value.as_i64().is_some_and(|integer| {
                self.minimum.is_none_or(|minimum| integer >= minimum)
                    && self.maximum.is_none_or(|maximum| integer <= maximum)
            }),
            "string" => value.is_string(),
            "boolean" => value.is_boolean(),
            "array" => value.is_array(),
            "object" => value.is_object(),
            _ => true,
        };
        if acceptable {
            return None;
        }
        Some(format!(
            "{} expects {expected}, got {actual} {}",
            self.name,
            bounded_schema_value(value)
        ))
    }
}

impl MapOperationShape {
    fn required_names(&self) -> Vec<&str> {
        self.fields
            .iter()
            .filter(|field| field.required)
            .map(|field| field.name.as_str())
            .collect()
    }

    fn allowed_names(&self) -> Vec<&str> {
        self.fields
            .iter()
            .map(|field| field.name.as_str())
            .collect()
    }

    fn describe_fields(&self) -> String {
        self.fields
            .iter()
            .map(|field| {
                let mut text = field.name.clone();
                if let Some((required, optional)) = &field.nested {
                    text.push('{');
                    text.push_str(&required.join(", "));
                    if !optional.is_empty() {
                        if !required.is_empty() {
                            text.push(' ');
                        }
                        text.push_str(&format!("[{}]", optional.join(", ")));
                    }
                    text.push('}');
                }
                if field.required {
                    text
                } else {
                    format!("[{text}]")
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn describe(&self) -> String {
        format!("{}: {}", self.ops.join(" | "), self.describe_fields())
    }
}

fn schema_string_list(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

fn resolve_schema_ref<'a>(root: &'a Value, schema: &'a Value) -> &'a Value {
    schema
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|reference| reference.strip_prefix("#/$defs/"))
        .and_then(|name| root.get("$defs").and_then(|defs| defs.get(name)))
        .unwrap_or(schema)
}

fn nested_object_shape(root: &Value, schema: &Value) -> Option<(Vec<String>, Vec<String>)> {
    let schema = resolve_schema_ref(root, schema);
    let properties = schema.get("properties")?.as_object()?;
    let required = schema_string_list(&schema["required"]);
    let optional = properties
        .keys()
        .filter(|name| !required.contains(name))
        .cloned()
        .collect();
    Some((required, optional))
}

/// Enumerate every documented operation family from the advertised
/// `map_draft_patch` schema so prompts and usage errors never drift from it.
pub(crate) fn map_operation_shapes() -> Vec<MapOperationShape> {
    fn collect(
        root: &Value,
        schema: &Value,
        inherited_required: &[String],
        shapes: &mut Vec<MapOperationShape>,
    ) {
        if let Some(parts) = schema["allOf"].as_array() {
            let mut required = inherited_required.to_vec();
            let mut composition = None;
            for part in parts {
                if part["oneOf"].is_array() || part["allOf"].is_array() {
                    composition = Some(part);
                } else {
                    required.extend(schema_string_list(&part["required"]));
                }
            }
            if let Some(composition) = composition {
                collect(root, composition, &required, shapes);
            }
            return;
        }
        if let Some(alternatives) = schema["oneOf"].as_array() {
            for alternative in alternatives {
                collect(root, alternative, inherited_required, shapes);
            }
            return;
        }
        let Some(properties) = schema["properties"].as_object() else {
            return;
        };
        let ops = properties
            .get("op")
            .map(|op| match op.get("const") {
                Some(Value::String(name)) => vec![name.clone()],
                _ => schema_string_list(&op["enum"]),
            })
            .unwrap_or_default();
        if ops.is_empty() {
            return;
        }
        let required = inherited_required
            .iter()
            .cloned()
            .chain(schema_string_list(&schema["required"]))
            .filter(|name| name != "op")
            .collect::<Vec<_>>();
        let mut order = required.clone();
        for name in schema_string_list(&schema["propertyNames"]["enum"]) {
            if name != "op" && !order.contains(&name) {
                order.push(name);
            }
        }
        let fields = order
            .into_iter()
            .map(|name| {
                let property = properties
                    .get(&name)
                    .map(|property| resolve_schema_ref(root, property));
                MapOperationField {
                    required: required.contains(&name),
                    kind: property
                        .and_then(|property| property.get("type"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    minimum: property
                        .and_then(|property| property.get("minimum"))
                        .and_then(Value::as_i64),
                    maximum: property
                        .and_then(|property| property.get("maximum"))
                        .and_then(Value::as_i64),
                    nested: property.and_then(|property| nested_object_shape(root, property)),
                    name,
                }
            })
            .collect();
        shapes.push(MapOperationShape { ops, fields });
    }

    let root = map_draft_patch_schema();
    let mut shapes = Vec::new();
    collect(
        &root,
        &root["properties"]["operations"]["items"],
        &[],
        &mut shapes,
    );
    shapes
}

/// Model-facing list of every `map_draft_patch` operation and its exact keys.
/// Optional keys are bracketed; `state{...}` lists the nested object keys.
pub fn map_draft_patch_operations_guide() -> String {
    map_operation_shapes()
        .iter()
        .map(|shape| format!("- {}", shape.describe()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Explain why one `operations[index]` value matches no documented operation:
/// the unknown `op`, or the missing/unexpected keys for the named `op`.
fn map_operation_shape_hint(instance: &Value, index: usize) -> Option<String> {
    let shapes = map_operation_shapes();
    let object = instance.as_object()?;
    let all_ops = shapes
        .iter()
        .flat_map(|shape| shape.ops.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(", ");
    let Some(op) = object.get("op").and_then(Value::as_str) else {
        return Some(format!(
            "operations[{index}] must declare a string 'op'; documented ops: {all_ops}"
        ));
    };
    let Some(shape) = shapes
        .iter()
        .find(|shape| shape.ops.iter().any(|name| name == op))
    else {
        return Some(format!(
            "operations[{index}].op '{op}' is not a documented op; documented ops: {all_ops}"
        ));
    };
    let allowed = shape.allowed_names();
    let missing = shape
        .required_names()
        .into_iter()
        .filter(|name| !object.contains_key(*name))
        .collect::<Vec<_>>();
    let unexpected = object
        .keys()
        .filter(|key| key.as_str() != "op" && !allowed.contains(&key.as_str()))
        .map(String::as_str)
        .collect::<Vec<_>>();
    let mut hint = format!(
        "operations[{index}] with op '{op}' takes exactly {{op, {}}}",
        shape.describe_fields()
    );
    if !missing.is_empty() {
        hint.push_str(&format!(
            "; missing required key(s): {}",
            missing.join(", ")
        ));
    }
    if !unexpected.is_empty() {
        hint.push_str(&format!("; unexpected key(s): {}", unexpected.join(", ")));
    }
    if missing.is_empty() && unexpected.is_empty() {
        let mismatches = shape
            .fields
            .iter()
            .filter_map(|field| {
                object
                    .get(&field.name)
                    .and_then(|value| field.mismatch(value))
            })
            .collect::<Vec<_>>();
        if mismatches.is_empty() {
            hint.push_str("; check each nested value's type and range");
        } else {
            hint.push_str(&format!("; invalid value(s): {}", mismatches.join(", ")));
        }
    }
    Some(hint)
}

fn map_draft_patch_schema() -> Value {
    let mut doodad = doodad_state_schema();
    let mut sprite = sprite_state_schema();
    replace_closed_objects_with_property_names(&mut doodad);
    replace_closed_objects_with_property_names(&mut sprite);
    let mut patch = schema(
        json!({"operations": map_operations_schema()}),
        &["operations"],
    );
    replace_closed_objects_with_property_names(&mut patch);
    patch["$defs"] = json!({"t": tile_rows_schema(), "d": doodad, "s": sprite});
    patch
}

fn map_palette_filter_schema() -> Value {
    let mut filter = object_schema(
        json!({
            "id": u16_schema(),
            "terrainType": u16_schema(),
            "group": {"type": "integer", "minimum": 0, "maximum": 1_023},
            "variant": {"type": "integer", "minimum": 0, "maximum": 15},
            "graphicsValid": {"type": "boolean"},
            "walkability": enum_string_schema(&["all", "any", "none"]),
            "groundHeight": u16_schema(),
            "buildability": u16_schema(),
            "ramp": {"type": "boolean"},
            "blocksView": {"type": "boolean"},
            "overlay": {"type": "boolean"},
            "visible": {"type": "boolean"},
            "width": u16_schema(),
            "height": u16_schema(),
            "placementWidth": u16_schema(),
            "placementHeight": u16_schema(),
        }),
        &[],
    );
    filter["minProperties"] = json!(1);
    filter["description"] = json!(
        "Exact AND filters. Tiles: id/terrainType/group/variant/graphicsValid/walkability/groundHeight/buildability/ramp/blocksView; brushes: id/terrainType and preview metadata; units/buildings: id/placementWidth/placementHeight; doodads: id/graphicsValid/overlay/width/height/buildability; sprites: id/visible."
    );
    filter
}

fn map_palette_query_schema() -> Value {
    let properties = json!({
        "kind": map_palette_catalog_kind_schema(),
        "query": {
            "type": "string",
            "minLength": 1,
            "description": "Case-insensitive name substring. Tile names contain only their numeric id; use filter for tile metadata.",
        },
        "filter": map_palette_filter_schema(),
    });
    let mut query = object_schema(properties.clone(), &["kind"]);
    query["anyOf"] = json!([
        object_schema(properties.clone(), &["kind", "query"]),
        object_schema(properties, &["kind", "filter"]),
    ]);
    query
}

fn map_stamp_destinations_schema() -> Value {
    json!({
        "type": "array",
        "minItems": 1,
        "maxItems": 64,
        "items": object_schema(
            json!({"x": u16_schema(), "y": u16_schema()}),
            &["x", "y"],
        ),
    })
}

fn map_stamp_source_schema() -> Value {
    json!({
        "oneOf": [
            object_schema(
                json!({
                    "kind": enum_string_schema(&["candidateSelection"]),
                    "selectionId": string_schema(),
                }),
                &["kind", "selectionId"],
            ),
            object_schema(
                json!({
                    "kind": enum_string_schema(&["imported"]),
                    "importId": string_schema(),
                }),
                &["kind", "importId"],
            ),
        ],
    })
}

pub fn map_tool_registry() -> Vec<ToolSpec> {
    vec![
        read_tool(
            "map_status",
            "Read the saved source and visible candidate revision.",
            empty_schema(),
        ),
        read_tool(
            "map_selection_read",
            "Read exact canonical row spans, role, and layer capabilities for one saved selection.",
            schema(json!({"selectionId": string_schema()}), &["selectionId"]),
        ),
        read_tool(
            "map_objects_read",
            "Read structured candidate objects and locations; object identity is revision-bound.",
            schema(
                json!({
                    "layer": enum_string_schema(&["units", "buildings", "doodads", "sprites", "locations"]),
                    "offset": integer_schema(),
                    "limit": integer_schema(),
                }),
                &["layer"],
            ),
        ),
        read_tool(
            "map_render",
            "Render a bounded candidate crop using actual terrain and GRP assets.",
            schema(
                json!({
                    "x": integer_schema(), "y": integer_schema(),
                    "width": integer_schema(), "height": integer_schema(),
                    "scale": render_scale_schema(),
                    "layers": {"type": "array", "items": string_schema()},
                }),
                &["x", "y", "width", "height"],
            ),
        ),
        read_tool(
            "map_palette_query",
            "Search one bounded current-tileset catalog family. kind must be brushes, tiles, units, buildings, doodads, or sprites; semanticTerrain palette mentions use brushes and exactTile mentions use tiles. Results are complete up to 256 matches; refine query/filter when broader. Never enumerate exact-tile pages.",
            map_palette_query_schema(),
        ),
        read_tool(
            "map_tile_info",
            "Read exact CV5/VF4 metadata for one current-tileset tile.",
            schema(json!({"tileId": integer_schema()}), &["tileId"]),
        ),
        read_tool(
            "map_analyze",
            "Analyze the visible candidate and its verification state.",
            empty_schema(),
        ),
        read_tool(
            "map_candidate_diff",
            "Read the visible revision's layer diff and validation report.",
            empty_schema(),
        ),
        read_tool(
            "map_draft_begin",
            "Create the request-owned draft from the visible candidate.",
            empty_schema(),
        ),
        read_tool(
            "map_stamp_preview",
            "Inspect an exact current-candidate or current-request imported stamp at one or more top-left destinations. Reports object/location collisions without mutating the draft.",
            schema(
                json!({
                    "source": map_stamp_source_schema(),
                    "destinations": map_stamp_destinations_schema(),
                }),
                &["source", "destinations"],
            ),
        ),
        read_tool(
            "map_stamp_place",
            "Place an exact current-candidate or current-request imported stamp on the request draft. Never substitutes semantic ISOM. Preview first and ask the user before choosing merge or replace when collisions exist.",
            schema(
                json!({
                    "source": map_stamp_source_schema(),
                    "destinations": map_stamp_destinations_schema(),
                    "collisionPolicy": enum_string_schema(&["merge", "replace"]),
                }),
                &["source", "destinations", "collisionPolicy"],
            ),
        ),
        read_tool(
            "map_draft_patch",
            "Apply one strict all-or-nothing operation batch to the request draft only. Each operation is a flat object discriminated by op with exactly the keys listed under [draft patch operations] in the system prompt, e.g. {\"op\":\"terrain.set\",\"x\":0,\"y\":0,\"before\":<current tile id>,\"after\":<tile id>} or {\"op\":\"terrain.rect\",\"x\":0,\"y\":0,\"width\":1,\"height\":1,\"after\":<tile id>}.",
            map_draft_patch_schema(),
        ),
        read_tool(
            "map_image_place",
            "Convert one current-request imageRef into a server-generated TerrainBlit on the request draft.",
            schema(
                json!({
                    "imageRef": string_schema(),
                    "x": u16_schema(),
                    "y": u16_schema(),
                    "width": positive_u16_schema(),
                    "height": positive_u16_schema(),
                }),
                &["imageRef", "x", "y", "width", "height"],
            ),
        ),
        read_tool(
            "map_draft_render",
            "Render the request draft with actual map assets.",
            schema(
                json!({
                    "x": integer_schema(), "y": integer_schema(),
                    "width": integer_schema(), "height": integer_schema(),
                    "scale": render_scale_schema(),
                    "layers": {"type": "array", "items": string_schema()},
                }),
                &["x", "y", "width", "height"],
            ),
        ),
        read_tool(
            "map_draft_analyze",
            "Verify draft masks, protections, layers, CHK semantics, and MPQ assets.",
            empty_schema(),
        ),
        read_tool(
            "map_draft_reset",
            "Reset only this request's draft to its parent candidate.",
            empty_schema(),
        ),
        read_tool(
            "map_candidate_finalize",
            "Finalize at most one verified visible revision for this request.",
            empty_schema(),
        ),
        read_tool(
            ASK_TOOL,
            "Pause this map turn for materially ambiguous owner, count, state, or authority input.",
            schema(json!({"questions": ask_questions_schema()}), &["questions"]),
        ),
    ]
}

fn descriptors(registry: Vec<ToolSpec>) -> Vec<Value> {
    registry
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

pub fn map_mcp_tool_descriptors() -> Vec<Value> {
    descriptors(map_tool_registry())
}

fn bounded_schema_value(value: &Value) -> String {
    match value {
        Value::String(text) => {
            let mut chars = text.chars();
            let mut preview = chars.by_ref().take(80).collect::<String>();
            if chars.next().is_some() {
                preview.push('…');
            }
            format!("{preview:?}")
        }
        Value::Number(_) | Value::Bool(_) | Value::Null => value.to_string(),
        Value::Array(_) => "[array]".to_string(),
        Value::Object(_) => "{object}".to_string(),
    }
}

fn map_validation_error_detail(error: &jsonschema::ValidationError<'_>) -> String {
    let instance_path = error.instance_path.to_string();
    let instance_path = if instance_path.is_empty() {
        "$".to_string()
    } else {
        instance_path
    };
    match &error.kind {
        jsonschema::error::ValidationErrorKind::Enum { options } => format!(
            "invalid value at {instance_path}: {} is not one of {options}",
            bounded_schema_value(error.instance.as_ref())
        ),
        _ => format!(
            "invalid value at {instance_path} (schema {})",
            error.schema_path
        ),
    }
}

/// Validate a Map Agent tool against the same strict schema advertised over MCP.
pub fn validate_map_tool_call(tool_name: &str, args: &Value) -> ToolResult<()> {
    let spec = map_tool_registry()
        .into_iter()
        .find(|spec| spec.name == tool_name)
        .ok_or_else(|| ToolError::AdmissionRejected {
            message: format!(
                "tool '{tool_name}' is not available to Map Agent; original Apply is intentionally absent"
            ),
        })?;
    match schema_violation_message(spec.name, &spec.input_schema, args) {
        Ok(None) => Ok(()),
        Ok(Some(message)) => Err(ToolError::AdmissionRejected { message }),
        Err(error) => Err(ToolError::AdmissionRejected {
            message: format!("tool schema for {} is invalid: {error}", spec.name),
        }),
    }
}

/// Build the model-facing usage message for arguments that violate a tool's
/// advertised input schema. `Ok(None)` means the arguments are valid; `Err`
/// means the schema itself failed to compile.
pub(crate) fn schema_violation_message(
    tool_name: &str,
    schema: &Value,
    args: &Value,
) -> Result<Option<String>, String> {
    let validator = jsonschema::JSONSchema::options()
        .with_draft(jsonschema::Draft::Draft7)
        .compile(schema)
        .map_err(|error| error.to_string())?;
    let violation = match validator.validate(args) {
        Ok(()) => None,
        Err(errors) => {
            let mut errors = errors.take(3).collect::<Vec<_>>();
            errors.sort_by_key(|error| {
                !matches!(
                    &error.kind,
                    jsonschema::error::ValidationErrorKind::Enum { .. }
                )
            });
            let mut details = errors
                .iter()
                .map(map_validation_error_detail)
                .collect::<Vec<_>>();
            if let Some(hint) = errors
                .iter()
                .find_map(|error| map_operation_violation_hint(schema, args, error))
            {
                details.push(hint);
            }
            let details = details.join("; ");
            Some(format!(
                "Usage: {}({}). arguments do not match the documented input schema: {details}",
                tool_name,
                required_arg_names(schema).join(", ")
            ))
        }
    };
    Ok(violation)
}

/// For a `map_draft_patch`-shaped schema, name the exact documented keys of the
/// operation the model attempted so it can correct the call on the next round.
fn map_operation_violation_hint(
    schema: &Value,
    args: &Value,
    error: &jsonschema::ValidationError<'_>,
) -> Option<String> {
    schema["properties"]["operations"]["items"]["allOf"].as_array()?;
    let instance_path = error.instance_path.to_string();
    let rest = instance_path.strip_prefix("/operations/")?;
    let index = rest
        .split('/')
        .next()
        .and_then(|segment| segment.parse::<usize>().ok())?;
    let operation = args.get("operations")?.get(index)?;
    map_operation_shape_hint(operation, index)
}

/// Return MCP tool descriptors using each registry tool's verbatim inputSchema.
pub fn mcp_tool_descriptors() -> Vec<Value> {
    descriptors(tool_registry())
}

/// Look up the registered effects for one tool.
pub fn tool_spec(tool_name: &str) -> Option<ToolSpec> {
    tool_registry()
        .into_iter()
        .find(|spec| spec.name == tool_name)
}

/// Whether a registered tool requires the canonical write workspace.
pub fn requires_write_workspace(tool_name: &str) -> bool {
    tool_spec(tool_name).is_some_and(|spec| spec.requires_write_workspace)
}

/// Whether a registered tool requires serialized project transaction authority.
pub fn requires_project_transaction(tool_name: &str) -> bool {
    tool_spec(tool_name).is_some_and(|spec| spec.requires_project_transaction)
}

/// Return whether a tool is exempt from the EUD-090 evidence gate.
pub fn is_evidence_gate_exempt(tool_name: &str) -> bool {
    tool_name == BUILD_RUN_TOOL
}

/// Check whether a tool call passes the EUD-090 evidence gate.
///
/// Canonical authoring tools are blocked on RAG-wired layers until
/// `search_docs` has run once in the request. A zero-hit search still lifts the gate.
pub fn check_evidence_gate(
    state: &RequestState,
    tool: &ToolSpec,
    rag_wired: bool,
) -> ToolResult<()> {
    if tool.requires_write_workspace
        && !is_evidence_gate_exempt(tool.name)
        && rag_wired
        && !state.docs_searched
    {
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

/// Admit one tool call through argument, evidence, and budget gates.
///
/// Admission does not execute tools. In particular, successful `search_docs`
/// execution is recorded by the execution layer, not here.
pub fn admit_tool_call(state: &mut RequestState, tool: &str, args: &Value) -> ToolResult<()> {
    let spec = lookup_tool(tool)?;

    validate_tool_args(&spec, args)?;

    if spec.name == SEARCH_DOCS_TOOL {
        if let Some(ids) = state.repeated_search_ids(args) {
            let ids = ids
                .iter()
                .map(|id| format!("{id:016x}"))
                .collect::<Vec<_>>()
                .join(", ");
            return admission_error(&format!(
                "search_docs no-progress: the normalized query and filters already returned the same stable ids [{ids}]. Change the query or inspect an existing id with docs_get."
            ));
        }
    }

    check_evidence_gate(state, &spec, true)?;

    validate_first_principles(&spec, args)?;
    if spec.requires_write_workspace {
        state.write_action_count = state.write_action_count.saturating_add(1);
    } else {
        state.read_action_count = state.read_action_count.saturating_add(1);
    }

    if !matches!(
        spec.name,
        SEARCH_DOCS_TOOL | PYTHON_DEPENDENCIES_PREPARE_TOOL
    ) {
        state.action_count = state.action_count.saturating_add(1);
        state.iteration_action_count = state.iteration_action_count.saturating_add(1);
    }

    Ok(())
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

    validate_tool_arg_semantics(spec, object)
}

fn validate_tool_arg_semantics(spec: &ToolSpec, args: &Map<String, Value>) -> ToolResult<()> {
    if spec.name == "file_edit" {
        let edits = args
            .get("edits")
            .expect("generic schema validation guarantees file_edit.edits");
        validate_nonempty_old_texts(spec, "edits", edits)?;
    }
    Ok(())
}

fn validate_nonempty_old_texts(spec: &ToolSpec, name: &str, edits: &Value) -> ToolResult<()> {
    let edits = edits
        .as_array()
        .expect("generic schema validation guarantees exact edit arrays");
    for (index, edit) in edits.iter().enumerate() {
        let old_text = edit
            .get("old_text")
            .and_then(Value::as_str)
            .expect("generic schema validation guarantees old_text strings");
        if old_text.is_empty() {
            return usage_error(
                spec,
                &[name],
                &format!("{name}[{index}].old_text must not be empty"),
            );
        }
    }
    Ok(())
}

fn required_args(spec: &ToolSpec) -> Vec<&str> {
    required_arg_names(&spec.input_schema)
}

fn required_arg_names(schema: &Value) -> Vec<&str> {
    schema
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
    if let Some(alternatives) = property_schema
        .get("oneOf")
        .and_then(Value::as_array)
        .filter(|alternatives| {
            alternatives
                .iter()
                .all(|alternative| alternative.get("type").is_some())
        })
    {
        let matches = alternatives
            .iter()
            .filter(|alternative| validate_arg_value(spec, name, value, alternative).is_ok())
            .count();
        if matches == 1 {
            return Ok(());
        }
        return admission_error(&format!(
            "invalid value for '{name}': expected exactly one documented source shape"
        ));
    }

    if let Some(expected) = property_schema.get("const") {
        if value != expected {
            return admission_error(&format!(
                "invalid value for '{name}': expected constant {expected}"
            ));
        }
        return Ok(());
    }

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
            let expected = values
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ");
            return admission_error(&format!(
                "invalid value for '{name}': '{actual}' is not allowed for {}; expected one of {expected}",
                spec.name
            ));
        }
        return Ok(());
    }

    match property_schema.get("type") {
        Some(Value::String(kind)) if kind == "string" => validate_string(spec, name, value),
        Some(Value::String(kind)) if kind == "integer" => validate_integer(spec, name, value),
        Some(Value::String(kind)) if kind == "boolean" => validate_boolean(spec, name, value),
        Some(Value::String(kind)) if kind == "array" => {
            validate_array(spec, name, value, property_schema)
        }
        Some(Value::String(kind)) if kind == "object" => {
            validate_object(spec, name, value, property_schema)
        }
        Some(Value::Array(kinds)) => validate_union_type(spec, name, value, kinds, property_schema),
        _ => admission_error(&format!(
            "tool schema for {}.{} has an unsupported type",
            spec.name, name
        )),
    }
}

fn validate_array(
    spec: &ToolSpec,
    name: &str,
    value: &Value,
    property_schema: &Value,
) -> ToolResult<()> {
    let Some(items) = value.as_array() else {
        return usage_error(spec, &[name], "invalid argument type; expected array");
    };
    let minimum = property_schema
        .get("minItems")
        .and_then(Value::as_u64)
        .unwrap_or_default() as usize;
    if items.len() < minimum {
        return admission_error(&format!(
            "invalid value for '{name}': expected at least {minimum} item(s)"
        ));
    }
    let item_schema = property_schema
        .get("items")
        .ok_or_else(|| ToolError::AdmissionRejected {
            message: format!(
                "tool schema for {}.{name} is missing array items",
                spec.name
            ),
        })?;
    for (index, item) in items.iter().enumerate() {
        validate_arg_value(spec, &format!("{name}[{index}]"), item, item_schema)?;
    }
    Ok(())
}

fn validate_object(
    spec: &ToolSpec,
    name: &str,
    value: &Value,
    property_schema: &Value,
) -> ToolResult<()> {
    let Some(object) = value.as_object() else {
        return usage_error(spec, &[name], "invalid argument type; expected object");
    };
    let properties = property_schema
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| ToolError::AdmissionRejected {
            message: format!(
                "tool schema for {}.{name} is missing object properties",
                spec.name
            ),
        })?;
    let required = property_schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str);
    for field in required {
        if !object.contains_key(field) {
            return usage_error(
                spec,
                &[name],
                &format!("missing required nested argument '{name}.{field}'"),
            );
        }
    }
    for (field, nested) in object {
        let Some(nested_schema) = properties.get(field) else {
            return usage_error(
                spec,
                &[name],
                &format!("unexpected nested argument '{name}.{field}'"),
            );
        };
        validate_arg_value(spec, &format!("{name}.{field}"), nested, nested_schema)?;
    }
    Ok(())
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
        "dat_patch" => {
            if let Some(changes) = args.get("changes").and_then(Value::as_array) {
                for change in changes {
                    match change.get("kind").and_then(Value::as_str) {
                        Some("button") => {
                            if let Some(csv) = change.get("after").and_then(Value::as_str) {
                                validate_btn_csv(csv)?;
                            }
                        }
                        Some("xdat") => {
                            let dat = change.get("dat").and_then(Value::as_str);
                            let name = change.get("field").and_then(Value::as_str);
                            let obj_id = change.get("objectId").and_then(Value::as_i64);
                            let value = change.get("after").and_then(parse_numeric_arg);
                            if let (Some(dat), Some(name), Some(obj_id), Some(value)) =
                                (dat, name, obj_id, value)
                            {
                                validate_buttonset_xdat(dat, name, obj_id, value)?;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
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
64-bit. Keep the unit's own set id and edit the matching `button` variant in the same \
dat_patch; its set id equals the unit id ({obj_id})."
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

/// Parsed `player_setup` operation. Players are 1-based P1..P8 until encoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayerEdit {
    Start {
        player: i64,
        tile_x: i64,
        tile_y: i64,
    },
    DelStart {
        player: i64,
    },
    Controller {
        player: i64,
        controller: String,
    },
}

pub fn parse_player_setup(args: &Value) -> ToolResult<PlayerEdit> {
    let Some(object) = args.as_object() else {
        return Err(player_setup_error(
            "arguments must be a JSON object with action start|delstart|controller",
        ));
    };
    let action = object
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| player_setup_error("missing required field action"))?;
    let player = required_player_setup_player(object)?;

    match action {
        "start" => {
            let tile_x = required_player_setup_i64(object, "tileX")?;
            let tile_y = required_player_setup_i64(object, "tileY")?;
            if tile_x < 0 || tile_y < 0 {
                return Err(player_setup_error("tileX and tileY must be >= 0"));
            }
            Ok(PlayerEdit::Start {
                player,
                tile_x,
                tile_y,
            })
        }
        "delstart" => Ok(PlayerEdit::DelStart { player }),
        "controller" => {
            let controller = object
                .get("controller")
                .and_then(Value::as_str)
                .ok_or_else(|| player_setup_error("missing required field controller"))?;
            if !matches!(
                controller,
                "human" | "computer" | "rescuable" | "neutral" | "inactive" | "closed"
            ) {
                return Err(player_setup_error(
                    "controller must be one of human, computer, rescuable, neutral, inactive, or closed",
                ));
            }
            Ok(PlayerEdit::Controller {
                player,
                controller: controller.to_owned(),
            })
        }
        other => Err(player_setup_error(format!(
            "invalid action {other:?}; expected start, delstart, or controller"
        ))),
    }
}

pub fn encode_playeredit_ops(op: &PlayerEdit) -> Vec<u8> {
    match op {
        PlayerEdit::Start {
            player,
            tile_x,
            tile_y,
        } => {
            let slot = player - 1;
            let x = tile_x * 32 + 16;
            let y = tile_y * 32 + 16;
            format!("start|{slot}|{x}|{y}").into_bytes()
        }
        PlayerEdit::DelStart { player } => {
            let slot = player - 1;
            format!("delstart|{slot}").into_bytes()
        }
        PlayerEdit::Controller { player, controller } => {
            let slot = player - 1;
            format!("controller|{slot}|{controller}").into_bytes()
        }
    }
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
            switch_id: None,
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

pub fn player_setup_apply<S, L, E>(
    map_safe: &crate::mapsafe::MapSafe<S, L, E>,
    journal: &crate::journal::JournalStore,
    request_id: &str,
    map_path: &Path,
    args: &Value,
    ts: u64,
) -> ToolResult<Value>
where
    S: crate::mapsafe::CompilingStatus,
    L: crate::mapsafe::LockProbe,
    E: crate::mapsafe::MapEngine,
{
    let op = parse_player_setup(args)?;
    let ops = encode_playeredit_ops(&op);
    let backup = match map_safe.write(map_path, crate::mapsafe::OpKind::PlayerEdit, &ops) {
        Ok(entry) => entry,
        Err(error) => {
            return Err(player_setup_mapsafe_error(map_safe, map_path, error));
        }
    };

    let post_chk = match isom::chk_extract(map_path) {
        Ok(chk) => chk,
        Err(isom_error) => std::fs::read(map_path).map_err(|read_error| {
            player_setup_error(format!(
                "post-edit CHK extraction failed for {}: {isom_error}; raw CHK fallback failed: {read_error}",
                map_path.display()
            ))
        })?,
    };
    let post_digest = crate::chk::digest_chk(&post_chk);

    let existing = match journal.changeset(request_id) {
        Ok(changeset) => changeset.items.len() as u64,
        Err(crate::journal::JournalError::MissingJournal { .. }) => 0,
        Err(error) => return Err(player_setup_error(error.to_string())),
    };
    let seq = existing + 1;
    let entry = crate::journal::JournalEntry {
        id: format!("plr-{seq}"),
        seq,
        tool: crate::journal::WriteTool::PlayerSetup,
        target: crate::journal::JournalTarget::Map {
            path: map_path.to_string_lossy().to_string(),
            summary: player_setup_summary(&op),
        },
        before: crate::journal::Snapshot::MapBackup {
            map_path: backup.map_path.to_string_lossy().to_string(),
            backup_path: backup.backup_path.to_string_lossy().to_string(),
        },
        after: crate::journal::Snapshot::MapEdit {
            action: op.action().to_string(),
            location_id: None,
            switch_id: None,
            name: None,
        },
        ts,
    };
    journal
        .record(request_id, entry)
        .map_err(|error| player_setup_error(error.to_string()))?;

    Ok(json!({
        "ok": true,
        "action": op.action(),
        "player": op.player(),
        "mapPath": map_path.to_string_lossy().to_string(),
        "backupPath": backup.backup_path.to_string_lossy().to_string(),
        "players": post_digest.players,
        "startLocations": post_digest.start_locations,
    }))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchWrite {
    id: usize,
    name: String,
}

pub fn parse_switch_write(args: &Value) -> ToolResult<SwitchWrite> {
    let Some(object) = args.as_object() else {
        return Err(switch_write_error(
            "arguments must be a JSON object with action rename, switchId, and name",
        ));
    };
    if object.get("action").and_then(Value::as_str) != Some("rename") {
        return Err(switch_write_error("action must be rename"));
    }
    let id = object
        .get("switchId")
        .and_then(Value::as_i64)
        .ok_or_else(|| switch_write_error("switchId must be an integer from 1 through 256"))?;
    if !(1..=256).contains(&id) {
        return Err(switch_write_error(
            "switchId must be an integer from 1 through 256",
        ));
    }
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| switch_write_error("name must be a non-empty string"))?
        .trim();
    if name.is_empty() {
        return Err(switch_write_error("name must be a non-empty string"));
    }
    if name
        .bytes()
        .any(|byte| matches!(byte, 0 | b'\r' | b'\n' | b'|'))
    {
        return Err(switch_write_error(
            "name must not contain NUL, a line break, or '|'; those bytes delimit the native op format",
        ));
    }

    Ok(SwitchWrite {
        id: id as usize,
        name: name.to_owned(),
    })
}

pub fn switch_write_apply<S, L, E>(
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
    let op = parse_switch_write(args)?;
    let mut ops = format!("rename|{}|", op.id).into_bytes();
    ops.extend_from_slice(&encode_location_name(&op.name, chk));
    let backup = map_safe
        .write(map_path, crate::mapsafe::OpKind::SwitchEdit, &ops)
        .map_err(|error| switch_write_mapsafe_error(map_safe, map_path, error))?;

    let post_chk = match isom::chk_extract(map_path) {
        Ok(chk) => chk,
        Err(isom_error) => std::fs::read(map_path).map_err(|read_error| {
            switch_write_error(format!(
                "post-edit CHK extraction failed for {}: {isom_error}; raw CHK fallback failed: {read_error}",
                map_path.display()
            ))
        })?,
    };
    let post_digest = crate::chk::digest_chk(&post_chk);
    let saved_switch = &post_digest.switches[op.id - 1];
    if saved_switch.name != op.name {
        let restore_entry = crate::mapsafe::JournalEntry {
            map_path: backup.map_path.clone(),
            backup_path: backup.backup_path.clone(),
        };
        let restore = map_safe.restore(&restore_entry);
        return Err(switch_write_error(match restore {
            Ok(()) => format!(
                "post-edit verification returned switch #{} as {:?}; the map was restored",
                op.id, saved_switch.name
            ),
            Err(error) => format!(
                "post-edit verification returned switch #{} as {:?}; restore from {} also failed: {error}",
                op.id,
                saved_switch.name,
                backup.backup_path.display()
            ),
        }));
    }

    let existing = match journal.changeset(request_id) {
        Ok(changeset) => changeset.items.len() as u64,
        Err(crate::journal::JournalError::MissingJournal { .. }) => 0,
        Err(error) => return Err(switch_write_error(error.to_string())),
    };
    let seq = existing + 1;
    let entry = crate::journal::JournalEntry {
        id: format!("switch-{seq}"),
        seq,
        tool: crate::journal::WriteTool::SwitchWrite,
        target: crate::journal::JournalTarget::Map {
            path: map_path.to_string_lossy().to_string(),
            summary: format!("Switch #{} renamed to {:?}", op.id, op.name),
        },
        before: crate::journal::Snapshot::MapBackup {
            map_path: backup.map_path.to_string_lossy().to_string(),
            backup_path: backup.backup_path.to_string_lossy().to_string(),
        },
        after: crate::journal::Snapshot::MapEdit {
            action: "rename".to_owned(),
            location_id: None,
            switch_id: Some(op.id as i64),
            name: Some(op.name.clone()),
        },
        ts,
    };
    journal
        .record(request_id, entry)
        .map_err(|error| switch_write_error(error.to_string()))?;

    Ok(json!({
        "ok": true,
        "action": "rename",
        "switch": saved_switch,
        "mapPath": map_path.to_string_lossy().to_string(),
        "backupPath": backup.backup_path.to_string_lossy().to_string(),
    }))
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

impl PlayerEdit {
    fn action(&self) -> &'static str {
        match self {
            Self::Start { .. } => "start",
            Self::DelStart { .. } => "delstart",
            Self::Controller { .. } => "controller",
        }
    }

    fn player(&self) -> i64 {
        match self {
            Self::Start { player, .. }
            | Self::DelStart { player }
            | Self::Controller { player, .. } => *player,
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

fn required_player_setup_i64(object: &Map<String, Value>, field: &str) -> ToolResult<i64> {
    let value = object
        .get(field)
        .ok_or_else(|| player_setup_error(format!("missing required field {field}")))?;
    if let Some(value) = value.as_i64() {
        return Ok(value);
    }
    if let Some(value) = value.as_u64().and_then(|value| i64::try_from(value).ok()) {
        return Ok(value);
    }
    Err(player_setup_error(format!("{field} must be an integer")))
}

fn required_player_setup_player(object: &Map<String, Value>) -> ToolResult<i64> {
    let player = required_player_setup_i64(object, "player")?;
    if !(1..=8).contains(&player) {
        return Err(player_setup_error("player must be 1..8 (P1..P8)"));
    }
    Ok(player)
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

fn player_setup_summary(op: &PlayerEdit) -> String {
    match op {
        PlayerEdit::Start {
            player,
            tile_x,
            tile_y,
        } => format!("P{player} start at tile ({tile_x},{tile_y})"),
        PlayerEdit::DelStart { player } => format!("P{player} start removed"),
        PlayerEdit::Controller { player, controller } => {
            format!("P{player} controller = {controller}")
        }
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

fn player_setup_mapsafe_error<S, L, E>(
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
                Ok(()) => player_setup_error(format!(
                    "post-edit verification failed ({detail}); the map was restored from backup {}",
                    backup.display()
                )),
                Err(restore_error) => player_setup_error(format!(
                    "post-edit verification failed ({detail}); restore from backup {} also failed: {restore_error}. Recover manually from this backup.",
                    backup.display()
                )),
            }
        }
        crate::mapsafe::MapSafeError::Compiling => player_setup_error(
            "compiling guard refused: the editor is building right now; retry after the build finishes",
        ),
        _ => player_setup_error(error.to_string()),
    }
}

pub fn map_info_path(map_path: &Path, args: &Value) -> ToolResult<Value> {
    let metadata = std::fs::metadata(map_path)
        .map_err(|error| map_info_error(format!("source map metadata failed: {error}")))?;
    let saved_at = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|value| value.as_secs())
        .unwrap_or_default();
    let chk = isom::chk_extract(map_path).map_err(|error| {
        map_info_error(format!(
            "CHK extraction failed for {}: {error}",
            map_path.display()
        ))
    })?;
    let digest = crate::chk::digest_chk(&chk);
    map_info_view(&digest, args, &map_path.to_string_lossy(), saved_at)
}

/// Pure view builder for `map_info`: applies mode-specific filters and bounded paging.
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
        "summary" => {
            let used_switches = switch_usage_counts(digest);
            Ok(json!({
                "map": map,
                "mode": "summary",
                "summary": {
                    "header": &digest.map,
                    "terrain": {
                        "tileCount": usize::from(digest.map.width) * usize::from(digest.map.height),
                        "availableTileCount": digest.tiles.len(),
                        "tileGroups": tile_group_counts(&digest.tiles),
                    },
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
                    "switches": {
                        "capacity": digest.switches.len(),
                        "named": digest.switches.iter().filter(|switch| !switch.name.is_empty()).count(),
                        "used": used_switches.len(),
                        "usages": digest.switch_usages.len(),
                    },
                },
            }))
        }
        "terrain" => terrain_view(digest, object, map),
        "locations" => locations_view(digest, object, map),
        "units" => units_view(digest, object, map),
        "players" => Ok(json!({
            "map": map,
            "mode": "players",
            "players": &digest.players,
            "forces": &digest.forces,
        })),
        "switches" => switches_view(digest, object, map),
        other => Err(map_info_error(format!(
            "invalid map_info mode {other:?}; expected summary, terrain, locations, units, players, or switches"
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

fn tile_group_counts(tiles: &[u16]) -> BTreeMap<u16, usize> {
    let mut groups = BTreeMap::new();
    for tile in tiles {
        *groups.entry(tile / 16).or_default() += 1;
    }
    groups
}

fn terrain_view(
    digest: &crate::chk::Digest,
    args: &Map<String, Value>,
    map: Value,
) -> ToolResult<Value> {
    let map_width = usize::from(digest.map.width);
    let map_height = usize::from(digest.map.height);
    if map_width == 0 || map_height == 0 {
        return Err(map_info_error(
            "terrain is unavailable because DIM is missing or empty",
        ));
    }

    let x = optional_nonnegative(args, "x")?.unwrap_or(0);
    let y = optional_nonnegative(args, "y")?.unwrap_or(0);
    if x >= map_width || y >= map_height {
        return Err(map_info_error(format!(
            "terrain origin ({x},{y}) is outside map bounds {map_width}x{map_height}"
        )));
    }
    let width = optional_nonnegative(args, "width")?.unwrap_or(map_width - x);
    let height = optional_nonnegative(args, "height")?.unwrap_or(map_height - y);
    if width == 0 || height == 0 || x + width > map_width || y + height > map_height {
        return Err(map_info_error(format!(
            "terrain rectangle ({x},{y},{width},{height}) must be non-empty and within {map_width}x{map_height}"
        )));
    }

    let total = width.saturating_mul(height);
    let (offset, limit) = page_args(args, 256, 1_024)?;
    let end = offset.saturating_add(limit).min(total);
    let mut tiles = Vec::with_capacity(end.saturating_sub(offset));
    for position in offset.min(total)..end {
        let tile_x = x + position % width;
        let tile_y = y + position / width;
        let value = digest.tiles.get(tile_y * map_width + tile_x).copied();
        tiles.push(json!({
            "x": tile_x,
            "y": tile_y,
            "value": value,
            "group": value.map(|tile| tile / 16),
            "variant": value.map(|tile| tile % 16),
        }));
    }

    Ok(json!({
        "map": map,
        "mode": "terrain",
        "rect": {"x": x, "y": y, "width": width, "height": height},
        "count": total,
        "availableTileCount": digest.tiles.len(),
        "offset": offset,
        "limit": limit,
        "hasMore": end < total,
        "tiles": tiles,
    }))
}

fn locations_view(
    digest: &crate::chk::Digest,
    args: &Map<String, Value>,
    map: Value,
) -> ToolResult<Value> {
    let (offset, limit) = page_args(args, 255, 255)?;
    let total = digest.locations.len();
    let end = offset.saturating_add(limit).min(total);
    let locations = digest
        .locations
        .iter()
        .skip(offset.min(total))
        .take(end.saturating_sub(offset.min(total)))
        .collect::<Vec<_>>();
    Ok(json!({
        "map": map,
        "mode": "locations",
        "count": total,
        "offset": offset,
        "limit": limit,
        "hasMore": end < total,
        "locations": locations,
    }))
}

fn units_view(
    digest: &crate::chk::Digest,
    args: &Map<String, Value>,
    map: Value,
) -> ToolResult<Value> {
    let owner_filter = args.get("owner").and_then(Value::as_str);
    let unit_type_filter = args.get("unitType").map(parse_unit_type_filter);
    let filtered = digest
        .units
        .iter()
        .filter(|unit| {
            owner_filter.map_or(true, |owner| unit_owner_matches_filter(&unit.owner, owner))
                && unit_type_filter
                    .as_ref()
                    .map_or(true, |filter| unit_matches_type_filter(unit, filter))
        })
        .collect::<Vec<_>>();
    let total = filtered.len();
    let (offset, limit) = page_args(args, 200, 200)?;
    let end = offset.saturating_add(limit).min(total);
    let units = filtered
        .into_iter()
        .skip(offset.min(total))
        .take(end.saturating_sub(offset.min(total)))
        .collect::<Vec<_>>();

    Ok(json!({
        "map": map,
        "mode": "units",
        "filters": {
            "owner": args.get("owner").cloned().unwrap_or(Value::Null),
            "unitType": args.get("unitType").cloned().unwrap_or(Value::Null),
        },
        "count": total,
        "offset": offset,
        "limit": limit,
        "hasMore": end < total,
        "units": units,
    }))
}

fn switches_view(
    digest: &crate::chk::Digest,
    args: &Map<String, Value>,
    map: Value,
) -> ToolResult<Value> {
    let filter = args.get("switch").map(parse_switch_filter);
    let counts = switch_usage_counts(digest);
    let switches = digest
        .switches
        .iter()
        .filter(|switch| {
            (filter.is_some() || !switch.name.is_empty() || counts.contains_key(&switch.id))
                && filter
                    .as_ref()
                    .map_or(true, |filter| switch_matches_filter(switch, filter))
        })
        .map(|switch| {
            let (conditions, actions) = counts.get(&switch.id).copied().unwrap_or_default();
            json!({
                "id": switch.id,
                "name": switch.name,
                "conditionCount": conditions,
                "actionCount": actions,
                "usageCount": conditions + actions,
            })
        })
        .collect::<Vec<_>>();
    let usages = digest
        .switch_usages
        .iter()
        .filter(|usage| {
            filter.as_ref().map_or(true, |filter| {
                digest
                    .switches
                    .get(usage.switch_id - 1)
                    .is_some_and(|switch| switch_matches_filter(switch, filter))
            })
        })
        .collect::<Vec<_>>();
    let total_usages = usages.len();
    let (offset, limit) = page_args(args, 100, 200)?;
    let end = offset.saturating_add(limit).min(total_usages);
    let usages = usages
        .into_iter()
        .skip(offset.min(total_usages))
        .take(end.saturating_sub(offset.min(total_usages)))
        .collect::<Vec<_>>();

    Ok(json!({
        "map": map,
        "mode": "switches",
        "filter": args.get("switch").cloned().unwrap_or(Value::Null),
        "capacity": digest.switches.len(),
        "switches": switches,
        "usageCount": total_usages,
        "offset": offset,
        "limit": limit,
        "hasMore": end < total_usages,
        "usages": usages,
    }))
}

fn switch_usage_counts(digest: &crate::chk::Digest) -> BTreeMap<usize, (usize, usize)> {
    let mut counts = BTreeMap::<usize, (usize, usize)>::new();
    for usage in &digest.switch_usages {
        let count = counts.entry(usage.switch_id).or_default();
        match usage.kind {
            crate::chk::SwitchUsageKind::Condition => count.0 += 1,
            crate::chk::SwitchUsageKind::Action => count.1 += 1,
        }
    }
    counts
}

fn page_args(
    args: &Map<String, Value>,
    default_limit: usize,
    max_limit: usize,
) -> ToolResult<(usize, usize)> {
    let offset = optional_nonnegative(args, "offset")?.unwrap_or(0);
    let limit = optional_nonnegative(args, "limit")?.unwrap_or(default_limit);
    if limit == 0 || limit > max_limit {
        return Err(map_info_error(format!(
            "limit must be from 1 through {max_limit}"
        )));
    }
    Ok((offset, limit))
}

fn optional_nonnegative(args: &Map<String, Value>, name: &str) -> ToolResult<Option<usize>> {
    let Some(value) = args.get(name) else {
        return Ok(None);
    };
    let value = value
        .as_i64()
        .ok_or_else(|| map_info_error(format!("{name} must be an integer")))?;
    if value < 0 {
        return Err(map_info_error(format!("{name} must be >= 0")));
    }
    Ok(Some(value as usize))
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

enum SwitchFilter {
    Id(usize),
    Name(String),
}

fn parse_switch_filter(value: &Value) -> SwitchFilter {
    if let Some(id) = value.as_u64() {
        return SwitchFilter::Id(id as usize);
    }
    if let Some(text) = value.as_str() {
        if let Ok(id) = text.trim().parse::<usize>() {
            return SwitchFilter::Id(id);
        }
        return SwitchFilter::Name(text.to_lowercase());
    }
    SwitchFilter::Id(usize::MAX)
}

fn switch_matches_filter(switch: &crate::chk::MapSwitch, filter: &SwitchFilter) -> bool {
    match filter {
        SwitchFilter::Id(id) => switch.id == *id,
        SwitchFilter::Name(text) => switch.name.to_lowercase().contains(text),
    }
}

pub fn map_minimap_path(map_path: &Path, starcraft_path: &Path, args: &Value) -> ToolResult<Value> {
    let Some(object) = args.as_object() else {
        return Err(map_minimap_error(
            "map_minimap arguments must be a JSON object",
        ));
    };
    let max_size = object.get("maxSize").and_then(Value::as_i64).unwrap_or(512);
    if !(128..=2_048).contains(&max_size) {
        return Err(map_minimap_error(
            "maxSize must be an integer from 128 through 2048",
        ));
    }
    let show_units = object
        .get("showUnits")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let metadata = std::fs::metadata(map_path)
        .map_err(|error| map_minimap_error(format!("source map metadata failed: {error}")))?;
    let saved_at = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|value| value.as_secs())
        .unwrap_or_default();
    let chk = isom::chk_extract(map_path).map_err(|error| {
        map_minimap_error(format!(
            "CHK extraction failed for {}: {error}",
            map_path.display()
        ))
    })?;
    let digest = crate::chk::digest_chk(&chk);
    let bmp = isom::render_map(map_path, starcraft_path, 8)
        .map_err(|error| map_minimap_error(format!("native terrain render failed: {error}")))?;
    let (source_width, source_height, source_rgb) = decode_bmp24(&bmp)?;
    let (width, height, mut rgb) =
        resize_rgb_to_fit(source_width, source_height, &source_rgb, max_size as usize);
    if show_units {
        overlay_units(&mut rgb, width, height, &digest);
    }
    let png = encode_png(width, height, &rgb)?;
    Ok(json!({
        "map": {
            "path": map_path.to_string_lossy().to_string(),
            "savedAt": saved_at,
        },
        "layers": {"terrain": true, "units": show_units},
        "unitCount": if show_units { digest.units.len() } else { 0 },
        "renderer": {"starcraftPath": starcraft_path.to_string_lossy().to_string()},
        "image": {
            "mimeType": "image/png",
            "width": width,
            "height": height,
            "data": BASE64_STANDARD.encode(png),
        },
    }))
}

fn decode_bmp24(bmp: &[u8]) -> ToolResult<(usize, usize, Vec<u8>)> {
    if bmp.len() < 54 || &bmp[0..2] != b"BM" {
        return Err(map_minimap_error(
            "native renderer returned an invalid BMP header",
        ));
    }
    let pixel_offset = u32::from_le_bytes(bmp[10..14].try_into().unwrap()) as usize;
    let width = i32::from_le_bytes(bmp[18..22].try_into().unwrap());
    let signed_height = i32::from_le_bytes(bmp[22..26].try_into().unwrap());
    let bits_per_pixel = u16::from_le_bytes(bmp[28..30].try_into().unwrap());
    let compression = u32::from_le_bytes(bmp[30..34].try_into().unwrap());
    if width <= 0 || signed_height == 0 || bits_per_pixel != 24 || compression != 0 {
        return Err(map_minimap_error(
            "native renderer returned an unsupported BMP layout",
        ));
    }
    let width = width as usize;
    let height = signed_height.unsigned_abs() as usize;
    let row_bytes = width
        .checked_mul(3)
        .and_then(|bytes| bytes.checked_add(3))
        .map(|bytes| bytes & !3)
        .ok_or_else(|| map_minimap_error("rendered BMP dimensions overflow"))?;
    let pixel_bytes = row_bytes
        .checked_mul(height)
        .and_then(|bytes| pixel_offset.checked_add(bytes))
        .ok_or_else(|| map_minimap_error("rendered BMP dimensions overflow"))?;
    if pixel_bytes > bmp.len() {
        return Err(map_minimap_error(
            "native renderer returned truncated BMP pixels",
        ));
    }

    let mut rgb = vec![0; width * height * 3];
    let bottom_up = signed_height > 0;
    for y in 0..height {
        let source_y = if bottom_up { height - 1 - y } else { y };
        let source_row = pixel_offset + source_y * row_bytes;
        for x in 0..width {
            let source = source_row + x * 3;
            let target = (y * width + x) * 3;
            rgb[target] = bmp[source + 2];
            rgb[target + 1] = bmp[source + 1];
            rgb[target + 2] = bmp[source];
        }
    }
    Ok((width, height, rgb))
}

fn resize_rgb_to_fit(
    source_width: usize,
    source_height: usize,
    source: &[u8],
    max_size: usize,
) -> (usize, usize, Vec<u8>) {
    if source_width <= max_size && source_height <= max_size {
        return (source_width, source_height, source.to_vec());
    }
    let scale = (max_size as f64 / source_width as f64).min(max_size as f64 / source_height as f64);
    let width = (source_width as f64 * scale).round().max(1.0) as usize;
    let height = (source_height as f64 * scale).round().max(1.0) as usize;
    let mut resized = vec![0; width * height * 3];
    for y in 0..height {
        let source_y = y * source_height / height;
        for x in 0..width {
            let source_x = x * source_width / width;
            let from = (source_y * source_width + source_x) * 3;
            let to = (y * width + x) * 3;
            resized[to..to + 3].copy_from_slice(&source[from..from + 3]);
        }
    }
    (width, height, resized)
}

fn overlay_units(rgb: &mut [u8], width: usize, height: usize, digest: &crate::chk::Digest) {
    let map_width = usize::from(digest.map.width) * 32;
    let map_height = usize::from(digest.map.height) * 32;
    if map_width == 0 || map_height == 0 {
        return;
    }
    let radius = (width.max(height) / 256).clamp(1, 4);
    for unit in &digest.units {
        let x = (usize::from(unit.x) * width / map_width).min(width.saturating_sub(1));
        let y = (usize::from(unit.y) * height / map_height).min(height.saturating_sub(1));
        let color = owner_minimap_color(&unit.owner);
        let marker_radius = if unit.type_id == crate::chk::_START_LOCATION_TYPE {
            (radius + 1).min(5)
        } else {
            radius
        };
        for py in y.saturating_sub(marker_radius)..=(y + marker_radius).min(height - 1) {
            for px in x.saturating_sub(marker_radius)..=(x + marker_radius).min(width - 1) {
                let target = (py * width + px) * 3;
                rgb[target..target + 3].copy_from_slice(&color);
            }
        }
    }
}

fn owner_minimap_color(owner: &str) -> [u8; 3] {
    const COLORS: [[u8; 3]; 12] = [
        [244, 4, 4],
        [12, 72, 204],
        [44, 180, 148],
        [136, 64, 156],
        [248, 140, 20],
        [112, 48, 20],
        [204, 224, 208],
        [252, 252, 56],
        [8, 128, 8],
        [252, 252, 124],
        [0, 228, 252],
        [116, 20, 20],
    ];
    let slot = owner
        .strip_prefix('P')
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|number| number.parse::<usize>().ok())
        .and_then(|number| number.checked_sub(1));
    slot.and_then(|slot| COLORS.get(slot).copied())
        .unwrap_or([224, 224, 224])
}

fn encode_png(width: usize, height: usize, rgb: &[u8]) -> ToolResult<Vec<u8>> {
    let width =
        u32::try_from(width).map_err(|_| map_minimap_error("minimap width exceeds PNG limits"))?;
    let height = u32::try_from(height)
        .map_err(|_| map_minimap_error("minimap height exceeds PNG limits"))?;
    let mut output = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut output, width, height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|error| map_minimap_error(format!("PNG header failed: {error}")))?;
        writer
            .write_image_data(rgb)
            .map_err(|error| map_minimap_error(format!("PNG encoding failed: {error}")))?;
    }
    Ok(output)
}

fn map_tool_error(tool: &str, message: impl Into<String>) -> ToolError {
    ToolError::AdmissionRejected {
        message: format!("{tool}: {}", message.into()),
    }
}

fn map_info_error(message: impl Into<String>) -> ToolError {
    map_tool_error(MAP_INFO_TOOL, message)
}

fn location_write_error(message: impl Into<String>) -> ToolError {
    ToolError::AdmissionRejected {
        message: format!("location_write: {}", message.into()),
    }
}

fn player_setup_error(message: impl Into<String>) -> ToolError {
    ToolError::AdmissionRejected {
        message: format!("player_setup: {}", message.into()),
    }
}

fn map_minimap_error(message: impl Into<String>) -> ToolError {
    map_tool_error(MAP_MINIMAP_TOOL, message)
}

fn switch_write_error(message: impl Into<String>) -> ToolError {
    map_tool_error(SWITCH_WRITE_TOOL, message)
}

fn switch_write_mapsafe_error<S, L, E>(
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
                Ok(()) => switch_write_error(format!(
                    "post-edit verification failed ({detail}); the map was restored from backup {}",
                    backup.display()
                )),
                Err(restore_error) => switch_write_error(format!(
                    "post-edit verification failed ({detail}); restore from backup {} also failed: {restore_error}. Recover manually from this backup.",
                    backup.display()
                )),
            }
        }
        crate::mapsafe::MapSafeError::Compiling => switch_write_error(
            "compiling guard refused: the editor is building right now; retry after the build finishes",
        ),
        _ => switch_write_error(error.to_string()),
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

    struct PlayerSetupFakeStatus(bool);

    impl crate::mapsafe::CompilingStatus for PlayerSetupFakeStatus {
        fn is_compiling(&self) -> bool {
            self.0
        }
    }

    struct PlayerSetupFakeLock(bool);

    impl crate::mapsafe::LockProbe for PlayerSetupFakeLock {
        fn is_locked(&self, _path: &Path) -> bool {
            self.0
        }
    }

    struct PlayerSetupFakeEngine {
        applied_bytes: Vec<u8>,
        digest_result: Result<Vec<u8>, String>,
        apply_called: Cell<bool>,
    }

    impl PlayerSetupFakeEngine {
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

    impl crate::mapsafe::MapEngine for PlayerSetupFakeEngine {
        fn apply(
            &self,
            map: &Path,
            kind: crate::mapsafe::OpKind,
            ops: &[u8],
        ) -> Result<(), String> {
            assert_eq!(kind, crate::mapsafe::OpKind::PlayerEdit);
            assert!(
                ops.starts_with(b"controller|"),
                "player_setup controller should encode a playeredit controller op"
            );
            self.apply_called.set(true);
            fs::write(map, &self.applied_bytes).map_err(|error| error.to_string())
        }

        fn digest(&self, _map: &Path) -> Result<Vec<u8>, String> {
            self.digest_result.clone()
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

    struct SwitchWriteFakeEngine {
        applied_bytes: Vec<u8>,
    }

    impl crate::mapsafe::MapEngine for SwitchWriteFakeEngine {
        fn apply(
            &self,
            map: &Path,
            kind: crate::mapsafe::OpKind,
            ops: &[u8],
        ) -> Result<(), String> {
            assert_eq!(kind, crate::mapsafe::OpKind::SwitchEdit);
            assert!(
                ops.starts_with(b"rename|1|"),
                "switch_write should encode a switchedit rename op"
            );
            fs::write(map, &self.applied_bytes).map_err(|error| error.to_string())
        }

        fn digest(&self, _map: &Path) -> Result<Vec<u8>, String> {
            Ok(self.applied_bytes.clone())
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

    fn tool_test_chk_with_switch(name: &[u8]) -> Vec<u8> {
        let strx = tool_test_strx(&[name]);
        let mut swnm = vec![0; 256 * crate::chk::SWNM_ENTRY_SIZE];
        swnm[0..4].copy_from_slice(&1u32.to_le_bytes());
        [
            tool_test_section("STRx", &strx),
            tool_test_section("SWNM", &swnm),
        ]
        .concat()
    }

    fn tool_test_unit_entry(
        x: u16,
        y: u16,
        type_id: u16,
        owner: u8,
    ) -> [u8; crate::chk::UNIT_ENTRY_SIZE] {
        let mut out = [0u8; crate::chk::UNIT_ENTRY_SIZE];
        out[4..6].copy_from_slice(&x.to_le_bytes());
        out[6..8].copy_from_slice(&y.to_le_bytes());
        out[8..10].copy_from_slice(&type_id.to_le_bytes());
        out[16] = owner;
        out[17] = 100;
        out[18] = 100;
        out[19] = 100;
        out
    }

    fn tool_test_chk_with_player(controller: u8, start_x: u16, start_y: u16) -> Vec<u8> {
        let dim = [64u16.to_le_bytes(), 128u16.to_le_bytes()].concat();
        let era = 3u16.to_le_bytes();
        let mut ownr = vec![0u8; 12];
        ownr[0] = controller;
        let mut side = vec![7u8; 12];
        side[0] = 1;
        let forc = vec![0u8; 20];
        let units = tool_test_unit_entry(start_x, start_y, crate::chk::_START_LOCATION_TYPE, 0);

        let mut chk = Vec::new();
        chk.extend_from_slice(&tool_test_section("DIM ", &dim));
        chk.extend_from_slice(&tool_test_section("ERA ", &era));
        chk.extend_from_slice(&tool_test_section("OWNR", &ownr));
        chk.extend_from_slice(&tool_test_section("SIDE", &side));
        chk.extend_from_slice(&tool_test_section("FORC", &forc));
        chk.extend_from_slice(&tool_test_section("UNIT", &units));
        chk
    }

    fn write_tool(name: &'static str) -> ToolSpec {
        ToolSpec::canonical_mutation(name)
    }

    fn native_build_result(
        ok: bool,
        errors: Vec<crate::native_build::NativeBuildError>,
    ) -> crate::native_build::NativeBuildResult {
        crate::native_build::NativeBuildResult {
            ok,
            errors,
            raw_status: u32::from(!ok),
            stdout: String::new(),
            stderr: String::new(),
            artifacts: crate::native_build::NativeBuildArtifacts {
                build_dir: "build".to_string(),
                wireframe_editor: None,
                requirement_file: None,
                eds_path: "build/main.eds".to_string(),
                output_map: "build/output.scx".to_string(),
                data_editor: None,
                extra_data_editor: None,
                custom_tbl: None,
                python_path_bootstrap: None,
            },
        }
    }

    fn native_build_error(raw: &str) -> crate::native_build::NativeBuildError {
        crate::native_build::NativeBuildError {
            source: "eudplib".to_string(),
            file: "main.eps".to_string(),
            line: 7,
            message: "undefined symbol".to_string(),
            raw: raw.to_string(),
        }
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
    fn evidence_gate_never_blocks_build_run() {
        let state = RequestState::new();

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

    #[test]
    fn switch_write_parse_rejects_bad_ids_and_op_delimiters() {
        assert_eq!(
            parse_switch_write(&serde_json::json!({
                "action": "rename",
                "switchId": 1,
                "name": "Door Control",
            }))
            .unwrap(),
            SwitchWrite {
                id: 1,
                name: "Door Control".to_owned(),
            }
        );
        for args in [
            serde_json::json!({"action": "rename", "switchId": 0, "name": "x"}),
            serde_json::json!({"action": "rename", "switchId": 257, "name": "x"}),
            serde_json::json!({"action": "rename", "switchId": 1, "name": "a|b"}),
            serde_json::json!({"action": "rename", "switchId": 1, "name": ""}),
        ] {
            assert!(parse_switch_write(&args).is_err(), "must reject {args}");
        }
    }

    #[test]
    fn switch_write_apply_uses_mapsafe_and_records_verified_rename() {
        let data_dir = tool_test_temp_dir("switch-write-apply");
        let map_path = data_dir.join("demo.scx");
        let pre_edit_chk = tool_test_chk_with_switch(b"Old Name");
        let post_edit_chk = tool_test_chk_with_switch(b"Door Control");
        fs::write(&map_path, &pre_edit_chk).expect("temp map should be writable");
        let map_safe = crate::mapsafe::MapSafe::new(
            data_dir.clone(),
            LocationWriteFakeStatus(false),
            LocationWriteFakeLock(false),
            SwitchWriteFakeEngine {
                applied_bytes: post_edit_chk.clone(),
            },
        );
        let journal = crate::journal::JournalStore::new(&data_dir);
        let request_id = "req-switch-write";

        let result = switch_write_apply(
            &map_safe,
            &journal,
            request_id,
            &map_path,
            &pre_edit_chk,
            &serde_json::json!({
                "action": "rename",
                "switchId": 1,
                "name": "Door Control",
            }),
            1_781_000_003,
        )
        .expect("switch rename should apply through mapsafe");

        assert_eq!(result["ok"], true);
        assert_eq!(result["switch"]["id"], 1);
        assert_eq!(result["switch"]["name"], "Door Control");
        assert_eq!(fs::read(&map_path).unwrap(), post_edit_chk);
        assert_eq!(journal.changeset(request_id).unwrap().items.len(), 1);
    }

    #[test]
    #[ignore = "requires the native isom engine and real sample.scx"]
    fn switch_write_real_map_roundtrip_verifies_exact_name() {
        let data_dir = tool_test_temp_dir("switch-write-real");
        let map_path = data_dir.join("demo.scx");
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("crates")
            .join("isom")
            .join("tests")
            .join("fixtures")
            .join("sample.scx");
        fs::copy(&fixture, &map_path).expect("sample map copy should succeed");
        let pre_edit_chk = isom::chk_extract(&map_path).expect("sample CHK should extract");
        let map_safe = crate::mapsafe::MapSafe::new(
            data_dir.clone(),
            LocationWriteFakeStatus(false),
            LocationWriteFakeLock(false),
            crate::mapsafe::IsomEngine,
        );
        let journal = crate::journal::JournalStore::new(&data_dir);

        let result = switch_write_apply(
            &map_safe,
            &journal,
            "req-switch-write-real",
            &map_path,
            &pre_edit_chk,
            &serde_json::json!({
                "action": "rename",
                "switchId": 1,
                "name": "EUD Agent Real Smoke",
            }),
            1_781_000_004,
        )
        .expect("real switch rename should round-trip through native save");

        assert_eq!(result["switch"]["id"], 1);
        assert_eq!(result["switch"]["name"], "EUD Agent Real Smoke");
        fs::remove_dir_all(data_dir).ok();
    }

    #[test]
    fn player_setup_parse_accepts_valid_action_shapes() {
        assert_eq!(
            parse_player_setup(&serde_json::json!({
                "action": "start",
                "player": 1,
                "tileX": 4,
                "tileY": 8,
            }))
            .unwrap(),
            PlayerEdit::Start {
                player: 1,
                tile_x: 4,
                tile_y: 8,
            }
        );
        assert_eq!(
            parse_player_setup(&serde_json::json!({
                "action": "delstart",
                "player": 3,
            }))
            .unwrap(),
            PlayerEdit::DelStart { player: 3 }
        );
        assert_eq!(
            parse_player_setup(&serde_json::json!({
                "action": "controller",
                "player": 2,
                "controller": "human",
            }))
            .unwrap(),
            PlayerEdit::Controller {
                player: 2,
                controller: "human".to_owned(),
            }
        );
    }

    #[test]
    fn player_setup_parse_rejects_missing_or_invalid_fields() {
        for args in [
            serde_json::json!({}),
            serde_json::json!({"action": "bogus", "player": 1}),
            serde_json::json!({"action": "start", "player": 0, "tileX": 4, "tileY": 8}),
            serde_json::json!({"action": "start", "player": 9, "tileX": 4, "tileY": 8}),
            serde_json::json!({"action": "start", "player": 1, "tileY": 8}),
            serde_json::json!({"action": "start", "player": 1, "tileX": 4}),
            serde_json::json!({"action": "controller", "player": 1}),
            serde_json::json!({"action": "controller", "player": 1, "controller": "bogus"}),
        ] {
            assert!(
                matches!(
                    parse_player_setup(&args),
                    Err(ToolError::AdmissionRejected { .. })
                ),
                "expected player_setup parse rejection for {args}"
            );
        }
    }

    #[test]
    fn encode_playeredit_ops_renders_zero_based_slots_and_tile_center_pixels() {
        assert_eq!(
            encode_playeredit_ops(&PlayerEdit::Start {
                player: 1,
                tile_x: 4,
                tile_y: 8,
            }),
            b"start|0|144|272".to_vec()
        );
        assert_eq!(
            encode_playeredit_ops(&PlayerEdit::DelStart { player: 3 }),
            b"delstart|2".to_vec()
        );
        assert_eq!(
            encode_playeredit_ops(&PlayerEdit::Controller {
                player: 2,
                controller: "human".to_owned(),
            }),
            b"controller|1|human".to_vec()
        );
    }

    #[test]
    fn player_setup_apply_records_journal_and_returns_post_edit_digest() {
        let data_dir = tool_test_temp_dir("player-setup-apply");
        let map_path = data_dir.join("demo.scx");
        let pre_edit_chk = tool_test_chk_with_player(0, 80, 80);
        let post_edit_chk = tool_test_chk_with_player(6, 80, 80);
        fs::write(&map_path, &pre_edit_chk).expect("temp map should be writable");

        let map_safe = crate::mapsafe::MapSafe::new(
            data_dir.clone(),
            PlayerSetupFakeStatus(false),
            PlayerSetupFakeLock(false),
            PlayerSetupFakeEngine::ok(post_edit_chk),
        );
        let journal = crate::journal::JournalStore::new(&data_dir);
        let request_id = "req-player-setup";

        let result = player_setup_apply(
            &map_safe,
            &journal,
            request_id,
            &map_path,
            &serde_json::json!({
                "action": "controller",
                "player": 1,
                "controller": "human",
            }),
            1_781_000_003,
        )
        .expect("player_setup controller should apply through mapsafe and journal");

        let expected_map_path = map_path.to_string_lossy().to_string();
        assert_eq!(result["ok"], true);
        assert_eq!(result["action"], "controller");
        assert_eq!(result["player"], 1);
        assert_eq!(result["mapPath"].as_str(), Some(expected_map_path.as_str()));
        assert!(result["backupPath"]
            .as_str()
            .is_some_and(|path| !path.is_empty()));
        assert!(result["players"].is_array());
        assert!(result["startLocations"].is_array());
        assert_eq!(journal.changeset(request_id).unwrap().items.len(), 1);
    }

    #[test]
    fn player_setup_apply_refuses_while_compiling() {
        let data_dir = tool_test_temp_dir("player-setup-compiling");
        let map_path = data_dir.join("demo.scx");
        let pre_edit_chk = tool_test_chk_with_player(0, 80, 80);
        fs::write(&map_path, &pre_edit_chk).expect("temp map should be writable");

        let map_safe = crate::mapsafe::MapSafe::new(
            data_dir.clone(),
            PlayerSetupFakeStatus(true),
            PlayerSetupFakeLock(false),
            PlayerSetupFakeEngine::ok(pre_edit_chk.clone()),
        );
        let journal = crate::journal::JournalStore::new(&data_dir);

        let error = player_setup_apply(
            &map_safe,
            &journal,
            "req-player-setup-compiling",
            &map_path,
            &serde_json::json!({
                "action": "controller",
                "player": 1,
                "controller": "human",
            }),
            1_781_000_004,
        )
        .expect_err("player_setup must reuse mapsafe compiling guard");

        assert!(error.to_string().to_lowercase().contains("compil"));
    }

    #[test]
    fn player_setup_apply_restores_backup_on_verify_failure() {
        let data_dir = tool_test_temp_dir("player-setup-verify-fails");
        let map_path = data_dir.join("demo.scx");
        let pre_edit_chk = tool_test_chk_with_player(0, 80, 80);
        let post_edit_chk = tool_test_chk_with_player(6, 80, 80);
        fs::write(&map_path, &pre_edit_chk).expect("temp map should be writable");

        let map_safe = crate::mapsafe::MapSafe::new(
            data_dir.clone(),
            PlayerSetupFakeStatus(false),
            PlayerSetupFakeLock(false),
            PlayerSetupFakeEngine::verify_fails(post_edit_chk),
        );
        let journal = crate::journal::JournalStore::new(&data_dir);
        let request_id = "req-player-setup-verify-fails";

        let error = player_setup_apply(
            &map_safe,
            &journal,
            request_id,
            &map_path,
            &serde_json::json!({
                "action": "controller",
                "player": 1,
                "controller": "human",
            }),
            1_781_000_005,
        )
        .expect_err("verify failure must reject the player_setup call");

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
            doodads: Vec::new(),
            sprites: Vec::new(),
            tiles: (0..64 * 128).map(|index| (index % 32) as u16).collect(),
            switches: (1..=256)
                .map(|id| crate::chk::MapSwitch {
                    id,
                    name: if id == 1 {
                        "Door Control".to_owned()
                    } else {
                        String::new()
                    },
                })
                .collect(),
            switch_usages: vec![crate::chk::SwitchUsage {
                switch_id: 1,
                trigger_id: 3,
                kind: crate::chk::SwitchUsageKind::Condition,
                index: 2,
                operation: crate::chk::SwitchOperation::Set,
                raw_operation: None,
                disabled: false,
            }],
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
            class_id: 0,
            relation_flags: 0,
            valid_state_flags: 0,
            valid_field_flags: 0,
            hp_percent: 100,
            shield_percent: 100,
            energy_percent: 100,
            resources: 0,
            hangar_amount: 0,
            state_flags: 0,
            cloaked: false,
            burrowed: false,
            in_transit: false,
            hallucinated: false,
            invincible: false,
            relation_class_id: 0,
        }
    }

    fn expected_registry_contract() -> Vec<(&'static str, bool, serde_json::Value)> {
        vec![
            ("project_status", false, schema(serde_json::json!({}), &[])),
            ("list_files", false, schema(serde_json::json!({}), &[])),
            (
                "read_file",
                false,
                schema(
                    serde_json::json!({
                        "path": string_schema(),
                        "startLine": integer_schema(),
                        "endLine": integer_schema(),
                    }),
                    &["path"],
                ),
            ),
            (
                SOURCE_SEARCH_TOOL,
                false,
                schema(
                    serde_json::json!({
                        "query": string_schema(),
                        "paths": string_array_schema(64),
                        "contextLines": integer_schema(),
                        "offset": integer_schema(),
                        "limit": integer_schema(),
                    }),
                    &["query"],
                ),
            ),
            (
                PYTHON_DEPENDENCIES_PREPARE_TOOL,
                false,
                schema(
                    serde_json::json!({"dependencies": optional_string_array_schema(128)}),
                    &["dependencies"],
                ),
            ),
            (
                "dat_get",
                false,
                schema(
                    serde_json::json!({
                        "items": object_array_schema(
                            serde_json::json!({
                                "dat": dat_names_schema(),
                                "param": string_schema(),
                                "objId": integer_schema(),
                            }),
                            &["dat", "param", "objId"],
                        ),
                    }),
                    &["items"],
                ),
            ),
            (
                "xdat_get",
                false,
                schema(
                    serde_json::json!({
                        "items": object_array_schema(
                            serde_json::json!({
                                "dat": xdat_kinds_schema(),
                                "name": string_schema(),
                                "objId": integer_schema(),
                            }),
                            &["dat", "name", "objId"],
                        ),
                    }),
                    &["items"],
                ),
            ),
            (
                "tbl_get",
                false,
                schema(
                    serde_json::json!({
                        "items": object_array_schema(
                            serde_json::json!({"index": integer_schema()}),
                            &["index"],
                        ),
                    }),
                    &["items"],
                ),
            ),
            (
                "req_get",
                false,
                schema(
                    serde_json::json!({
                        "items": object_array_schema(
                            serde_json::json!({
                                "dat": req_dats_schema(),
                                "objId": integer_schema(),
                            }),
                            &["dat", "objId"],
                        ),
                    }),
                    &["items"],
                ),
            ),
            (
                "btn_get",
                false,
                schema(
                    serde_json::json!({
                        "items": object_array_schema(
                            serde_json::json!({"setId": integer_schema()}),
                            &["setId"],
                        ),
                    }),
                    &["items"],
                ),
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
                        "mode": enum_string_schema(&[
                            "summary",
                            "terrain",
                            "locations",
                            "units",
                            "players",
                            "switches",
                        ]),
                        "owner": map_info_owner_schema(),
                        "unitType": integer_or_string_schema(),
                        "switch": integer_or_string_schema(),
                        "x": integer_schema(),
                        "y": integer_schema(),
                        "width": integer_schema(),
                        "height": integer_schema(),
                        "offset": integer_schema(),
                        "limit": integer_schema(),
                    }),
                    &[],
                ),
            ),
            (
                MAP_MINIMAP_TOOL,
                false,
                schema(
                    serde_json::json!({
                        "maxSize": integer_schema(),
                        "showUnits": {"type": "boolean"},
                        "starcraftPath": string_schema(),
                    }),
                    &[],
                ),
            ),
            ("plugins_list", false, schema(serde_json::json!({}), &[])),
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
                DOCS_GET_TOOL,
                false,
                schema(
                    serde_json::json!({"ids": string_array_schema(10)}),
                    &["ids"],
                ),
            ),
            (
                ASK_TOOL,
                false,
                schema(
                    serde_json::json!({"questions": ask_questions_schema()}),
                    &["questions"],
                ),
            ),
            (
                PYTHON_DEPENDENCIES_SET_TOOL,
                true,
                schema(
                    serde_json::json!({"candidateToken": string_schema()}),
                    &["candidateToken"],
                ),
            ),
            (
                "dat_patch",
                true,
                schema(
                    serde_json::json!({"changes": dat_patch_changes_schema()}),
                    &["changes"],
                ),
            ),
            (
                "file_create",
                true,
                schema(
                    serde_json::json!({
                        "path": string_schema(),
                        "ftype": enum_string_schema(&["CUIEps", "CUIPy"]),
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
                "file_edit",
                true,
                schema(
                    serde_json::json!({
                        "path": string_schema(),
                        "edits": exact_text_edits_schema(),
                    }),
                    &["path", "edits"],
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
            ("build_run", false, schema(serde_json::json!({}), &[])),
            (
                "trace_test_run",
                false,
                schema(
                    serde_json::json!({
                        "name": {"type": "string", "minLength": 1, "maxLength": 512},
                        "code": {"type": "string", "minLength": 1, "maxLength": 131072},
                        "symbols": {
                            "type": "array",
                            "maxItems": 256,
                            "items": {
                                "type": "object",
                                "additionalProperties": false,
                                "properties": {
                                    "eventId": {"type": "integer", "minimum": 0, "maximum": 4294967295_u64},
                                    "name": {"type": "string", "minLength": 1, "maxLength": 128},
                                    "source": {"type": "string", "maxLength": 512},
                                    "line": {"type": "integer", "minimum": 1, "maximum": 4294967295_u64}
                                },
                                "required": ["eventId", "name"]
                            }
                        },
                        "timeoutMs": {"type": "integer", "minimum": 1000, "maximum": 120000}
                    }),
                    &["name", "code"],
                ),
            ),
            (
                "trace_suite_run",
                false,
                schema(
                    serde_json::json!({
                        "tests": {
                            "type": "array",
                            "minItems": 1,
                            "maxItems": 256,
                            "items": {"type": "string", "minLength": 1, "maxLength": 512}
                        },
                        "timeoutMs": {"type": "integer", "minimum": 1000, "maximum": 120000}
                    }),
                    &[],
                ),
            ),
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
                SWITCH_WRITE_TOOL,
                true,
                schema(
                    serde_json::json!({
                        "action": enum_string_schema(&["rename"]),
                        "switchId": integer_schema(),
                        "name": string_schema(),
                    }),
                    &["action", "switchId", "name"],
                ),
            ),
            (MAP_SOUND_LIST_TOOL, false, empty_schema()),
            (
                MAP_SOUND_IMPORT_TOOL,
                true,
                schema(
                    serde_json::json!({"audioRef": string_schema()}),
                    &["audioRef"],
                ),
            ),
            (
                MAP_SOUND_EDIT_TOOL,
                true,
                schema(
                    serde_json::json!({
                        "mpqPath": string_schema(),
                        "audioRef": string_schema(),
                        "volumePercent": {"type": "integer", "minimum": 0, "maximum": 400},
                        "fadeInMs": {"type": "integer", "minimum": 0, "maximum": 3_600_000},
                        "fadeOutMs": {"type": "integer", "minimum": 0, "maximum": 3_600_000},
                    }),
                    &["mpqPath"],
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

        for (name, requires_write_workspace, input_schema) in expected {
            let spec = registry
                .iter()
                .find(|spec| spec.name == name)
                .unwrap_or_else(|| panic!("missing tool {name}"));

            assert_eq!(
                spec.requires_write_workspace, requires_write_workspace,
                "{name} write-workspace effect mismatch"
            );
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
    fn project_status_is_read_only_and_describes_the_configured_start_file() {
        let spec = tool_registry()
            .into_iter()
            .find(|spec| spec.name == "project_status")
            .expect("project_status must be registered");

        assert!(spec.description.contains("native project"));
        assert!(spec.description.contains("mainFile path"));
    }

    #[test]
    fn build_is_a_read_workspace_project_operation() {
        let build = tool_spec(BUILD_RUN_TOOL).expect("build_run registry entry");
        assert!(!build.requires_write_workspace);
        assert!(build.requires_project_transaction);

        let read = tool_spec("read_file").expect("read_file registry entry");
        assert!(!read.requires_write_workspace);
        assert!(!read.requires_project_transaction);

        let write = tool_spec("file_write").expect("file_write registry entry");
        assert!(write.requires_write_workspace);
        assert!(write.requires_project_transaction);
    }

    #[test]
    fn map_info_is_registered_read_only_and_invalid_mode_rejects_before_counting() {
        let spec = tool_registry()
            .into_iter()
            .find(|spec| spec.name == MAP_INFO_TOOL)
            .expect("map_info must be registered");
        assert!(
            !spec.requires_write_workspace && !spec.requires_project_transaction,
            "map_info must be read-only"
        );

        let mut state = RequestState::for_request("req-map-info");
        let error = admit_tool_call(
            &mut state,
            MAP_INFO_TOOL,
            &serde_json::json!({"mode": "fog"}),
        )
        .unwrap_err();

        assert!(error.to_string().contains("invalid value for 'mode'"));
        assert_eq!(
            state.action_count, 0,
            "invalid map_info mode must be rejected before counting"
        );
    }

    #[test]
    fn python_dependency_tools_have_closed_schemas_and_correct_admission_lanes() {
        let registry = tool_registry();
        let prepare = registry
            .iter()
            .find(|spec| spec.name == PYTHON_DEPENDENCIES_PREPARE_TOOL)
            .expect("python dependency preparation must be registered");
        assert!(!prepare.requires_write_workspace);
        assert!(!prepare.requires_project_transaction);
        assert_eq!(prepare.input_schema["additionalProperties"], json!(false));
        assert_eq!(
            prepare.input_schema["properties"]["dependencies"]["maxItems"],
            json!(128)
        );

        let set = registry
            .iter()
            .find(|spec| spec.name == PYTHON_DEPENDENCIES_SET_TOOL)
            .expect("python dependency commit must be registered");
        assert!(set.requires_write_workspace);
        assert!(set.requires_project_transaction);
        assert_eq!(set.input_schema["additionalProperties"], json!(false));
        assert_eq!(set.input_schema["required"], json!(["candidateToken"]));

        let mut prepare_state = RequestState::for_request("req-python-prepare");
        for index in 0..5 {
            admit_tool_call(
                &mut prepare_state,
                PYTHON_DEPENDENCIES_PREPARE_TOOL,
                &json!({"dependencies": [format!("package-{index}==1.0")]}),
            )
            .unwrap();
        }
        assert_eq!(
            prepare_state.action_count, 0,
            "dependency preparation is request-local analysis, not a project action"
        );
        assert!(!prepare_state.docs_searched);

        for invalid in [
            json!({"dependencies": ["eudplib==0.80.6"], "extra": true}),
            json!({"dependencies": [7]}),
        ] {
            let mut state = RequestState::for_request("req-invalid-python-prepare");
            assert!(
                admit_tool_call(&mut state, PYTHON_DEPENDENCIES_PREPARE_TOOL, &invalid).is_err()
            );
            assert_eq!(state.action_count, 0);
        }

        let mut set_state = RequestState::for_request("req-python-set");
        assert_evidence_required(admit_tool_call(
            &mut set_state,
            PYTHON_DEPENDENCIES_SET_TOOL,
            &json!({"candidateToken": "opaque"}),
        ));
        set_state.record_search_docs();
        admit_tool_call(
            &mut set_state,
            PYTHON_DEPENDENCIES_SET_TOOL,
            &json!({"candidateToken": "opaque"}),
        )
        .unwrap();
        assert_eq!(set_state.action_count, 1);
        assert!(admit_tool_call(
            &mut set_state,
            PYTHON_DEPENDENCIES_SET_TOOL,
            &json!({"candidateToken": "opaque", "dependencies": []}),
        )
        .is_err());
    }
    #[test]
    fn removed_eps_check_is_not_registered_or_admitted() {
        assert!(tool_registry().iter().all(|spec| spec.name != "eps_check"));
        assert!(mcp_tool_descriptors()
            .iter()
            .all(|descriptor| descriptor["name"] != "eps_check"));

        let mut state = RequestState::for_request("req-removed-eps-check");
        let error = admit_tool_call(
            &mut state,
            "eps_check",
            &serde_json::json!({"files": [{"path": "main.eps", "code": ""}]}),
        )
        .unwrap_err();
        assert!(error.to_string().contains("unknown tool 'eps_check'"));
        assert_eq!(state.action_count, 0);
    }

    #[test]
    fn batched_get_schemas_require_nonempty_well_typed_items() {
        for (tool, valid) in [
            (
                "dat_get",
                serde_json::json!({"items": [
                    {"dat": "units", "param": "HitPoints", "objId": 0},
                    {"dat": "weapons", "param": "DamageAmount", "objId": 1},
                ]}),
            ),
            (
                "xdat_get",
                serde_json::json!({"items": [
                    {"dat": "wireframe", "name": "wirefram", "objId": 0},
                ]}),
            ),
            ("tbl_get", serde_json::json!({"items": [{"index": 1}]})),
            (
                "req_get",
                serde_json::json!({"items": [{"dat": "units", "objId": 0}]}),
            ),
            ("btn_get", serde_json::json!({"items": [{"setId": 0}]})),
        ] {
            let mut state = RequestState::for_request("req-batched-get");
            admit_tool_call(&mut state, tool, &valid).unwrap();
            assert_eq!(
                state.action_count, 1,
                "{tool} batch must count as one action"
            );
            assert!(admit_tool_call(
                &mut RequestState::for_request("req-empty-batch"),
                tool,
                &serde_json::json!({"items": []}),
            )
            .is_err());
        }

        assert!(admit_tool_call(
            &mut RequestState::for_request("req-old-scalar-contract"),
            "dat_get",
            &serde_json::json!({"dat": "units", "param": "HitPoints", "objId": 0}),
        )
        .is_err());
    }

    #[test]
    fn file_edit_requires_nonempty_exact_matches_before_counting() {
        let mut invalid = RequestState::for_request("req-invalid-file-edit");
        let error = admit_tool_call(
            &mut invalid,
            "file_edit",
            &serde_json::json!({
                "path": "main.eps",
                "edits": [{"old_text": "", "new_text": "newCall();"}],
            }),
        )
        .unwrap_err();
        assert!(error.to_string().contains("must not be empty"));
        assert_eq!(invalid.action_count, 0);
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
        assert_eq!(value["summary"]["terrain"]["tileCount"], 64 * 128);
        assert_eq!(value["summary"]["terrain"]["availableTileCount"], 64 * 128);
        assert_eq!(value["summary"]["switches"]["named"], 1);
        assert_eq!(value["summary"]["switches"]["used"], 1);
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
        assert_eq!(units["hasMore"], false);

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
    fn map_info_units_pages_after_filters() {
        let units = (0..205)
            .map(|idx| unit(0, "Terran Marine", "P1", idx, 160))
            .collect();
        let digest = sample_digest(units);

        let first = map_info_view(
            &digest,
            &serde_json::json!({"mode": "units", "owner": "P1", "unitType": "Marine"}),
            "demo.scx",
            10,
        )
        .unwrap();
        assert_eq!(first["count"], 205);
        assert_eq!(first["units"].as_array().unwrap().len(), 200);
        assert_eq!(first["offset"], 0);
        assert_eq!(first["limit"], 200);
        assert_eq!(first["hasMore"], true);
        assert_eq!(first["filters"]["owner"], "P1");
        assert_eq!(first["filters"]["unitType"], "Marine");

        let second = map_info_view(
            &digest,
            &serde_json::json!({"mode": "units", "owner": "P1", "offset": 200, "limit": 5}),
            "demo.scx",
            10,
        )
        .unwrap();
        assert_eq!(second["units"].as_array().unwrap().len(), 5);
        assert_eq!(second["units"][0]["x"], 200);
        assert_eq!(second["hasMore"], false);
    }

    #[test]
    fn map_info_terrain_filters_rectangle_and_pages_tiles() {
        let digest = sample_digest(Vec::new());
        let value = map_info_view(
            &digest,
            &serde_json::json!({
                "mode": "terrain",
                "x": 1,
                "y": 2,
                "width": 3,
                "height": 2,
                "offset": 2,
                "limit": 3,
            }),
            "demo.scx",
            10,
        )
        .unwrap();

        assert_eq!(value["count"], 6);
        assert_eq!(value["offset"], 2);
        assert_eq!(value["hasMore"], true);
        assert_eq!(
            value["tiles"][0],
            serde_json::json!({
                "x": 3,
                "y": 2,
                "value": 3,
                "group": 0,
                "variant": 3,
            })
        );
        assert_eq!(value["tiles"][2]["x"], 2);
        assert_eq!(value["tiles"][2]["y"], 3);
    }
    #[test]
    fn minimap_bmp_decode_resize_overlay_and_png_encode_are_consistent() {
        let mut bmp = vec![0u8; 54 + 16];
        bmp[0..2].copy_from_slice(b"BM");
        bmp[2..6].copy_from_slice(&70u32.to_le_bytes());
        bmp[10..14].copy_from_slice(&54u32.to_le_bytes());
        bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
        bmp[18..22].copy_from_slice(&2i32.to_le_bytes());
        bmp[22..26].copy_from_slice(&2i32.to_le_bytes());
        bmp[26..28].copy_from_slice(&1u16.to_le_bytes());
        bmp[28..30].copy_from_slice(&24u16.to_le_bytes());
        // Bottom row: blue, white. Top row: red, green. Each row has 2 bytes padding.
        bmp[54..60].copy_from_slice(&[255, 0, 0, 255, 255, 255]);
        bmp[62..68].copy_from_slice(&[0, 0, 255, 0, 255, 0]);

        let (width, height, rgb) = decode_bmp24(&bmp).unwrap();
        assert_eq!((width, height), (2, 2));
        assert_eq!(rgb, vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255,]);

        let (width, height, mut resized) = resize_rgb_to_fit(width, height, &rgb, 1);
        assert_eq!((width, height), (1, 1));
        let digest = sample_digest(vec![unit(0, "Terran Marine", "P1", 0, 0)]);
        overlay_units(&mut resized, width, height, &digest);
        assert_eq!(resized, vec![244, 4, 4], "P1 overlay should be red");

        let png = encode_png(width, height, &resized).unwrap();
        assert_eq!(&png[0..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn map_info_switches_filters_names_and_returns_trigger_usages() {
        let digest = sample_digest(Vec::new());
        let value = map_info_view(
            &digest,
            &serde_json::json!({"mode": "switches", "switch": "door", "limit": 1}),
            "demo.scx",
            10,
        )
        .unwrap();

        assert_eq!(value["switches"].as_array().unwrap().len(), 1);
        assert_eq!(value["switches"][0]["id"], 1);
        assert_eq!(value["switches"][0]["usageCount"], 1);
        assert_eq!(value["usageCount"], 1);
        assert_eq!(value["usages"][0]["triggerId"], 3);
        assert_eq!(value["usages"][0]["kind"], "condition");
        assert_eq!(value["usages"][0]["operation"], "set");
    }

    fn resolve_map_schema_refs(root: &Value, schema: &Value) -> Value {
        if let Some(reference) = schema["$ref"].as_str() {
            let pointer = reference
                .strip_prefix('#')
                .expect("Map schema refs must be local");
            return resolve_map_schema_refs(
                root,
                root.pointer(pointer)
                    .unwrap_or_else(|| panic!("missing Map schema ref {reference}")),
            );
        }
        match schema {
            Value::Array(values) => Value::Array(
                values
                    .iter()
                    .map(|value| resolve_map_schema_refs(root, value))
                    .collect(),
            ),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(name, value)| (name.clone(), resolve_map_schema_refs(root, value)))
                    .collect(),
            ),
            _ => schema.clone(),
        }
    }

    fn materialized_map_operations(root: &Value) -> Vec<Value> {
        fn collect(
            root: &Value,
            schema: &Value,
            inherited_properties: &Map<String, Value>,
            inherited_required: &std::collections::BTreeSet<String>,
            operations: &mut Vec<Value>,
        ) {
            if let Some(parts) = schema["allOf"].as_array() {
                let mut properties = inherited_properties.clone();
                let mut required = inherited_required.clone();
                let mut composition = None;
                for part in parts {
                    if part["oneOf"].is_array() || part["allOf"].is_array() {
                        assert!(composition.replace(part).is_none());
                    } else {
                        if let Some(part_properties) = part["properties"].as_object() {
                            properties.extend(part_properties.clone());
                        }
                        if let Some(part_required) = part["required"].as_array() {
                            required.extend(part_required.iter().map(|name| {
                                name.as_str()
                                    .expect("required names are strings")
                                    .to_string()
                            }));
                        }
                    }
                }
                collect(
                    root,
                    composition.expect("Map allOf must contain one composition"),
                    &properties,
                    &required,
                    operations,
                );
                return;
            }
            if let Some(alternatives) = schema["oneOf"].as_array() {
                for alternative in alternatives {
                    collect(
                        root,
                        alternative,
                        inherited_properties,
                        inherited_required,
                        operations,
                    );
                }
                return;
            }

            let mut properties = inherited_properties.clone();
            properties.extend(
                schema["properties"]
                    .as_object()
                    .expect("Map operation leaf must have properties")
                    .clone(),
            );
            let mut required = inherited_required.clone();
            required.extend(
                schema["required"]
                    .as_array()
                    .expect("Map operation leaf must have required names")
                    .iter()
                    .map(|name| {
                        name.as_str()
                            .expect("required names are strings")
                            .to_string()
                    }),
            );
            assert_eq!(
                schema_string_set(&schema["propertyNames"], "enum"),
                properties.keys().cloned().collect(),
                "Map operation leaf closure must include inherited properties"
            );
            let discriminators = properties["op"]
                .get("enum")
                .and_then(Value::as_array)
                .cloned()
                .or_else(|| {
                    properties["op"]
                        .get("const")
                        .cloned()
                        .map(|name| vec![name])
                })
                .expect("Map operation leaf must have a discriminator");
            for discriminator in discriminators {
                let mut operation_properties = properties.clone();
                operation_properties.insert("op".to_string(), json!({"const": discriminator}));
                let mut operation = object_schema(
                    Value::Object(operation_properties),
                    &required.iter().map(String::as_str).collect::<Vec<_>>(),
                );
                operation = resolve_map_schema_refs(root, &operation);
                operations.push(operation);
            }
        }

        let mut operations = Vec::new();
        collect(
            root,
            &root["properties"]["operations"]["items"],
            &Map::new(),
            &std::collections::BTreeSet::new(),
            &mut operations,
        );
        operations
    }

    fn map_operation<'a>(alternatives: &'a [Value], name: &str) -> &'a Value {
        alternatives
            .iter()
            .find(|alternative| alternative["properties"]["op"]["const"] == name)
            .unwrap_or_else(|| panic!("missing map operation schema for {name}"))
    }

    fn operation_property<'a>(
        alternatives: &'a [Value],
        operation: &str,
        property: &str,
    ) -> &'a Value {
        &map_operation(alternatives, operation)["properties"][property]
    }

    fn schema_string_set(schema: &Value, property: &str) -> std::collections::BTreeSet<String> {
        schema[property]
            .as_array()
            .unwrap_or_else(|| panic!("{property} must be an array"))
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .unwrap_or_else(|| panic!("{property} values must be strings"))
                    .to_string()
            })
            .collect()
    }

    fn schema_property_set(schema: &Value) -> std::collections::BTreeSet<String> {
        schema["properties"]
            .as_object()
            .expect("schema properties must be an object")
            .keys()
            .cloned()
            .collect()
    }

    fn assert_object_contract(schema: &Value, properties: &[&str], required: &[&str]) {
        assert_eq!(schema["type"], "object");
        if schema["additionalProperties"] != false {
            assert!(schema["additionalProperties"].is_null());
            assert_eq!(
                schema_string_set(&schema["propertyNames"], "enum"),
                schema_property_set(schema),
                "closed object must enumerate exactly its properties"
            );
        }
        assert_eq!(
            schema_property_set(schema),
            properties
                .iter()
                .map(|property| (*property).to_string())
                .collect()
        );
        assert_eq!(
            schema_string_set(schema, "required"),
            required
                .iter()
                .map(|property| (*property).to_string())
                .collect()
        );
    }

    fn assert_integer_bounds(schema: &Value, minimum: i64, maximum: i64) {
        assert_eq!(schema["type"], "integer");
        assert_eq!(schema["minimum"].as_i64(), Some(minimum));
        assert_eq!(schema["maximum"].as_i64(), Some(maximum));
    }

    fn assert_tile_rows(schema: &Value) {
        assert_eq!(schema["type"], "array");
        assert_eq!(schema["minItems"], 1);
        assert_eq!(schema["items"]["type"], "array");
        assert_eq!(schema["items"]["minItems"], 1);
        assert_integer_bounds(&schema["items"]["items"], 0, 65_535);
    }

    #[test]
    fn map_draft_patch_schema_exhaustively_mirrors_map_operation() {
        let registry = map_tool_registry();
        let patch = registry
            .iter()
            .find(|tool| tool.name == "map_draft_patch")
            .expect("map_draft_patch must be registered");
        assert_object_contract(&patch.input_schema, &["operations"], &["operations"]);
        let operations = &patch.input_schema["properties"]["operations"];
        assert_eq!(operations["type"], "array");
        assert_eq!(operations["minItems"], 1);
        assert_eq!(operations["maxItems"], 4096);
        let alternatives = materialized_map_operations(&patch.input_schema);
        let expected: &[(&str, &[&str], &[&str])] = &[
            (
                "terrain.set",
                &["op", "x", "y", "before", "after"],
                &["op", "x", "y", "before", "after"],
            ),
            (
                "terrain.rect",
                &["op", "x", "y", "width", "height", "after"],
                &["op", "x", "y", "width", "height", "after"],
            ),
            (
                "terrain.blit",
                &["op", "x", "y", "tiles"],
                &["op", "x", "y", "tiles"],
            ),
            (
                "terrain.isom_brush",
                &["op", "isomX", "isomY", "brush", "extent"],
                &["op", "isomX", "isomY", "brush"],
            ),
            ("unit.add", &["op", "state"], &["op", "state"]),
            (
                "unit.set",
                &["op", "ordinal", "beforeFingerprint", "state"],
                &["op", "ordinal", "beforeFingerprint", "state"],
            ),
            (
                "unit.delete",
                &["op", "ordinal", "beforeFingerprint"],
                &["op", "ordinal", "beforeFingerprint"],
            ),
            (
                "unit.move",
                &["op", "ordinal", "beforeFingerprint", "x", "y"],
                &["op", "ordinal", "beforeFingerprint", "x", "y"],
            ),
            ("doodad.add", &["op", "state"], &["op", "state"]),
            (
                "doodad.set",
                &[
                    "op",
                    "ordinal",
                    "beforeFingerprint",
                    "state",
                    "replacementTiles",
                ],
                &[
                    "op",
                    "ordinal",
                    "beforeFingerprint",
                    "state",
                    "replacementTiles",
                ],
            ),
            (
                "doodad.delete",
                &["op", "ordinal", "beforeFingerprint", "replacementTiles"],
                &["op", "ordinal", "beforeFingerprint", "replacementTiles"],
            ),
            (
                "doodad.move",
                &[
                    "op",
                    "ordinal",
                    "beforeFingerprint",
                    "x",
                    "y",
                    "replacementTiles",
                ],
                &[
                    "op",
                    "ordinal",
                    "beforeFingerprint",
                    "x",
                    "y",
                    "replacementTiles",
                ],
            ),
            ("sprite.add", &["op", "state"], &["op", "state"]),
            (
                "sprite.set",
                &["op", "ordinal", "beforeFingerprint", "state"],
                &["op", "ordinal", "beforeFingerprint", "state"],
            ),
            (
                "sprite.delete",
                &["op", "ordinal", "beforeFingerprint"],
                &["op", "ordinal", "beforeFingerprint"],
            ),
            (
                "sprite.move",
                &["op", "ordinal", "beforeFingerprint", "x", "y"],
                &["op", "ordinal", "beforeFingerprint", "x", "y"],
            ),
            ("location.add", &["op", "state"], &["op", "state"]),
            ("location.set", &["op", "state"], &["op", "state"]),
            (
                "location.rename",
                &["op", "locationId", "nameBytesHex"],
                &["op", "locationId", "nameBytesHex"],
            ),
            (
                "location.delete",
                &["op", "locationId"],
                &["op", "locationId"],
            ),
        ];
        assert_eq!(alternatives.len(), expected.len());
        for (name, properties, required) in expected {
            let alternative = map_operation(&alternatives, name);
            assert_eq!(alternative["properties"]["op"]["const"], *name);
            assert_object_contract(alternative, properties, required);
        }

        for (operation, property) in [
            ("terrain.set", "x"),
            ("terrain.set", "y"),
            ("terrain.set", "before"),
            ("terrain.set", "after"),
            ("terrain.rect", "x"),
            ("terrain.rect", "y"),
            ("terrain.rect", "width"),
            ("terrain.rect", "height"),
            ("terrain.rect", "after"),
            ("terrain.blit", "x"),
            ("terrain.blit", "y"),
            ("terrain.isom_brush", "isomX"),
            ("terrain.isom_brush", "isomY"),
            ("terrain.isom_brush", "brush"),
            ("terrain.isom_brush", "extent"),
            ("unit.move", "x"),
            ("unit.move", "y"),
            ("doodad.move", "x"),
            ("doodad.move", "y"),
            ("sprite.move", "x"),
            ("sprite.move", "y"),
            ("location.rename", "locationId"),
            ("location.delete", "locationId"),
        ] {
            assert_integer_bounds(
                operation_property(&alternatives, operation, property),
                0,
                65_535,
            );
        }
        for (operation, property) in [
            ("unit.set", "ordinal"),
            ("unit.delete", "ordinal"),
            ("unit.move", "ordinal"),
            ("doodad.set", "ordinal"),
            ("doodad.delete", "ordinal"),
            ("doodad.move", "ordinal"),
            ("sprite.set", "ordinal"),
            ("sprite.delete", "ordinal"),
            ("sprite.move", "ordinal"),
        ] {
            assert_integer_bounds(
                operation_property(&alternatives, operation, property),
                0,
                4_294_967_295,
            );
        }
        for (operation, property) in [
            ("unit.set", "beforeFingerprint"),
            ("unit.delete", "beforeFingerprint"),
            ("unit.move", "beforeFingerprint"),
            ("doodad.set", "beforeFingerprint"),
            ("doodad.delete", "beforeFingerprint"),
            ("doodad.move", "beforeFingerprint"),
            ("sprite.set", "beforeFingerprint"),
            ("sprite.delete", "beforeFingerprint"),
            ("sprite.move", "beforeFingerprint"),
            ("location.rename", "nameBytesHex"),
        ] {
            assert_eq!(
                operation_property(&alternatives, operation, property)["type"],
                "string"
            );
        }
        assert_eq!(
            operation_property(&alternatives, "terrain.isom_brush", "extent")["default"],
            1
        );

        let tiles = operation_property(&alternatives, "terrain.blit", "tiles");
        assert_tile_rows(tiles);
        for (operation, property) in [
            ("doodad.set", "replacementTiles"),
            ("doodad.delete", "replacementTiles"),
            ("doodad.move", "replacementTiles"),
        ] {
            assert_eq!(
                operation_property(&alternatives, operation, property),
                tiles
            );
        }

        let unit_state = operation_property(&alternatives, "unit.add", "state");
        let unit_properties = [
            "typeId",
            "owner",
            "x",
            "y",
            "classId",
            "relationFlags",
            "validStateFlags",
            "validFieldFlags",
            "hpPercent",
            "shieldPercent",
            "energyPercent",
            "resourceAmount",
            "hangarAmount",
            "stateFlags",
            "unused",
            "relationClassId",
        ];
        assert_object_contract(unit_state, &unit_properties, &["typeId", "owner", "x", "y"]);
        for property in ["owner", "hpPercent", "shieldPercent", "energyPercent"] {
            assert_integer_bounds(&unit_state["properties"][property], 0, 255);
        }
        for property in [
            "typeId",
            "x",
            "y",
            "relationFlags",
            "validStateFlags",
            "validFieldFlags",
            "hangarAmount",
            "stateFlags",
        ] {
            assert_integer_bounds(&unit_state["properties"][property], 0, 65_535);
        }
        for property in ["classId", "resourceAmount", "unused", "relationClassId"] {
            assert_integer_bounds(&unit_state["properties"][property], 0, 4_294_967_295);
        }
        for (property, default) in [
            ("classId", 0),
            ("relationFlags", 0),
            ("validStateFlags", 0),
            ("validFieldFlags", 0),
            ("hpPercent", 100),
            ("shieldPercent", 100),
            ("energyPercent", 100),
            ("resourceAmount", 0),
            ("hangarAmount", 0),
            ("stateFlags", 0),
            ("unused", 0),
            ("relationClassId", 0),
        ] {
            assert_eq!(unit_state["properties"][property]["default"], default);
        }

        let unit_patch = operation_property(&alternatives, "unit.set", "state");
        assert_object_contract(unit_patch, &unit_properties, &[]);
        for property in ["owner", "hpPercent", "shieldPercent", "energyPercent"] {
            assert_integer_bounds(&unit_patch["properties"][property], 0, 255);
        }
        for property in [
            "typeId",
            "x",
            "y",
            "relationFlags",
            "validStateFlags",
            "validFieldFlags",
            "hangarAmount",
            "stateFlags",
        ] {
            assert_integer_bounds(&unit_patch["properties"][property], 0, 65_535);
        }
        for property in ["classId", "resourceAmount", "unused", "relationClassId"] {
            assert_integer_bounds(&unit_patch["properties"][property], 0, 4_294_967_295);
        }

        let doodad_state = operation_property(&alternatives, "doodad.add", "state");
        assert_object_contract(
            doodad_state,
            &["doodadId", "x", "y", "owner", "disabled"],
            &["doodadId", "x", "y"],
        );
        for property in ["doodadId", "x", "y"] {
            assert_integer_bounds(&doodad_state["properties"][property], 0, 65_535);
        }
        assert_integer_bounds(&doodad_state["properties"]["owner"], 0, 255);
        assert_eq!(doodad_state["properties"]["owner"]["default"], 11);
        assert_eq!(doodad_state["properties"]["disabled"]["type"], "boolean");
        assert_eq!(doodad_state["properties"]["disabled"]["default"], false);
        assert_eq!(
            operation_property(&alternatives, "doodad.set", "state"),
            doodad_state
        );

        let sprite_state = operation_property(&alternatives, "sprite.add", "state");
        assert_object_contract(
            sprite_state,
            &["spriteId", "x", "y", "owner", "flags"],
            &["spriteId", "x", "y"],
        );
        for property in ["spriteId", "x", "y", "flags"] {
            assert_integer_bounds(&sprite_state["properties"][property], 0, 65_535);
        }
        assert_integer_bounds(&sprite_state["properties"]["owner"], 0, 255);
        assert_eq!(sprite_state["properties"]["owner"]["default"], 11);
        assert_eq!(sprite_state["properties"]["flags"]["default"], 0);
        assert_eq!(
            operation_property(&alternatives, "sprite.set", "state"),
            sprite_state
        );

        let location_state = operation_property(&alternatives, "location.add", "state");
        assert_object_contract(
            location_state,
            &[
                "locationId",
                "left",
                "top",
                "right",
                "bottom",
                "elevationFlags",
                "nameBytesHex",
            ],
            &["locationId", "left", "top", "right", "bottom"],
        );
        for property in ["locationId", "elevationFlags"] {
            assert_integer_bounds(&location_state["properties"][property], 0, 65_535);
        }
        for property in ["left", "top", "right", "bottom"] {
            assert_integer_bounds(
                &location_state["properties"][property],
                -2_147_483_648,
                2_147_483_647,
            );
        }
        assert_eq!(
            location_state["properties"]["nameBytesHex"]["type"],
            "string"
        );
        assert_eq!(location_state["properties"]["elevationFlags"]["default"], 0);
        assert_eq!(
            operation_property(&alternatives, "location.set", "state"),
            location_state
        );
    }

    fn codex_normalized_map_schema(value: &Value) -> Value {
        if let Some(values) = value.as_array() {
            return Value::Array(values.iter().map(codex_normalized_map_schema).collect());
        }
        let Some(source) = value.as_object() else {
            return value.clone();
        };
        let mut normalized = Map::new();
        for key in [
            "$ref",
            "type",
            "description",
            "encrypted",
            "enum",
            "required",
            "items",
            "additionalProperties",
            "properties",
            "$defs",
            "definitions",
            "anyOf",
            "oneOf",
            "allOf",
        ] {
            let Some(child) = source.get(key) else {
                continue;
            };
            let normalized_child = if matches!(key, "properties" | "$defs" | "definitions") {
                Value::Object(
                    child
                        .as_object()
                        .expect("Map schema table must be an object")
                        .iter()
                        .map(|(name, schema)| (name.clone(), codex_normalized_map_schema(schema)))
                        .collect(),
                )
            } else {
                codex_normalized_map_schema(child)
            };
            normalized.insert(key.to_string(), normalized_child);
        }
        if let Some(constant) = source.get("const") {
            normalized.insert("type".to_string(), json!("string"));
            normalized.insert("enum".to_string(), json!([constant]));
        }
        Value::Object(normalized)
    }

    fn required_map_schema_example(schema: &Value) -> Value {
        if let Some(constant) = schema.get("const") {
            return constant.clone();
        }
        if let Some(value) = schema.get("enum").and_then(Value::as_array) {
            return value
                .first()
                .expect("schema enum must not be empty")
                .clone();
        }
        match schema["type"].as_str() {
            Some("object") => Value::Object(
                schema["required"]
                    .as_array()
                    .expect("object schema must declare required")
                    .iter()
                    .map(|name| {
                        let name = name.as_str().expect("required name must be a string");
                        (
                            name.to_string(),
                            required_map_schema_example(&schema["properties"][name]),
                        )
                    })
                    .collect(),
            ),
            Some("array") => json!([required_map_schema_example(&schema["items"])]),
            Some("integer" | "number") => json!(schema["minimum"].as_i64().unwrap_or(0)),
            Some("string") => json!("x"),
            Some("boolean") => json!(false),
            _ => panic!("unsupported Map example schema {schema}"),
        }
    }

    #[test]
    fn map_draft_patch_schema_stays_within_codex_normalized_budget() {
        let schema = map_tool_registry()
            .into_iter()
            .find(|tool| tool.name == "map_draft_patch")
            .expect("map_draft_patch must be registered")
            .input_schema;
        let normalized_bytes = serde_json::to_vec(&codex_normalized_map_schema(&schema))
            .expect("normalized Map schema must serialize")
            .len();
        assert!(
            normalized_bytes <= 5_000,
            "Map schema exceeds the Codex normalized budget: {normalized_bytes}B"
        );
        let operations = materialized_map_operations(&schema);
        assert_eq!(operations.len(), 20);
        let validator = jsonschema::JSONSchema::options()
            .with_draft(jsonschema::Draft::Draft7)
            .compile(&schema)
            .expect("Map schema must compile as Draft 7");
        let mut canonical = Vec::new();
        for operation in &operations {
            let example = required_map_schema_example(operation);
            assert!(validator.is_valid(&json!({"operations": [example.clone()]})));
            for required in operation["required"]
                .as_array()
                .expect("operation must declare required")
            {
                let mut missing = example.clone();
                missing
                    .as_object_mut()
                    .expect("operation example must be an object")
                    .remove(required.as_str().expect("required name must be a string"));
                assert!(!validator.is_valid(&json!({"operations": [missing]})));
            }
            let mut extra = example.clone();
            extra["unexpected"] = json!(true);
            assert!(!validator.is_valid(&json!({"operations": [extra]})));
            canonical.push(example);
        }
        assert!(validator.is_valid(&json!({"operations": canonical})));
        assert!(!validator.is_valid(&json!({
            "operations": [{"type": "setTile", "x": 64, "y": 64, "tileId": 1}],
        })));
    }

    #[test]
    fn map_render_schemas_advertise_supported_scales() {
        let registry = map_tool_registry();
        for name in ["map_render", "map_draft_render"] {
            let render = registry
                .iter()
                .find(|tool| tool.name == name)
                .unwrap_or_else(|| panic!("{name} must be registered"));
            assert_eq!(
                render.input_schema["properties"]["scale"],
                render_scale_schema()
            );
        }
    }

    #[test]
    fn map_render_admission_accepts_only_advertised_numeric_scales() {
        let crop = json!({"x": 0, "y": 0, "width": 8, "height": 8});
        for name in ["map_render", "map_draft_render"] {
            for scale in [1, 2, 4, 8] {
                let mut args = crop.clone();
                args["scale"] = json!(scale);
                assert_eq!(validate_map_tool_call(name, &args), Ok(()));
            }
            for scale in [json!(3), json!("1"), json!(1.5), json!(true)] {
                let mut args = crop.clone();
                args["scale"] = scale;
                assert!(matches!(
                    validate_map_tool_call(name, &args),
                    Err(ToolError::AdmissionRejected { .. })
                ));
            }
        }

        assert_eq!(
            validate_map_tool_call("map_objects_read", &json!({"layer": "units"})),
            Ok(())
        );
        for layer in [json!("terrain"), json!(1)] {
            assert!(matches!(
                validate_map_tool_call("map_objects_read", &json!({"layer": layer})),
                Err(ToolError::AdmissionRejected { .. })
            ));
        }
    }

    #[test]
    fn map_draft_patch_operations_guide_mirrors_every_documented_operation() {
        let schema = map_tool_registry()
            .into_iter()
            .find(|tool| tool.name == "map_draft_patch")
            .expect("map_draft_patch must be registered")
            .input_schema;
        let documented_ops = materialized_map_operations(&schema)
            .iter()
            .map(|operation| {
                operation["properties"]["op"]["const"]
                    .as_str()
                    .expect("materialized operations pin one op")
                    .to_string()
            })
            .collect::<BTreeSet<_>>();
        let shapes = map_operation_shapes();
        let guided_ops = shapes
            .iter()
            .flat_map(|shape| shape.ops.iter().cloned())
            .collect::<BTreeSet<_>>();
        assert_eq!(guided_ops, documented_ops);
        assert_eq!(guided_ops.len(), 20);

        let guide = map_draft_patch_operations_guide();
        for line in [
            "- terrain.set: x, y, before, after",
            "- terrain.rect: x, y, width, height, after",
            "- terrain.blit: x, y, tiles",
            "- terrain.isom_brush: isomX, isomY, brush, [extent]",
            "- unit.add: state{typeId, owner, x, y [",
            "- unit.set: ordinal, beforeFingerprint, state{[",
            "- unit.delete | sprite.delete: ordinal, beforeFingerprint",
            "- unit.move | sprite.move: ordinal, beforeFingerprint, x, y",
            "- doodad.add: state{doodadId, x, y [disabled, owner]}",
            "- doodad.set: ordinal, beforeFingerprint, state{doodadId, x, y [disabled, owner]}, replacementTiles",
            "- doodad.delete: ordinal, beforeFingerprint, replacementTiles",
            "- doodad.move: ordinal, beforeFingerprint, x, y, replacementTiles",
            "- sprite.add: state{spriteId, x, y [flags, owner]}",
            "- sprite.set: ordinal, beforeFingerprint, state{spriteId, x, y [flags, owner]}",
            "- location.add | location.set: state{locationId, left, top, right, bottom [elevationFlags, nameBytesHex]}",
            "- location.rename: locationId, nameBytesHex",
            "- location.delete: locationId",
        ] {
            assert!(guide.contains(line), "guide is missing {line:?}:\n{guide}");
        }
        for shape in &shapes {
            for field in &shape.fields {
                let example = required_map_schema_example(
                    materialized_map_operations(&schema)
                        .iter()
                        .find(|operation| {
                            operation["properties"]["op"]["const"].as_str() == Some(&shape.ops[0])
                        })
                        .expect("every guided op is materialized"),
                );
                assert_eq!(
                    example.get(&field.name).is_some(),
                    field.required,
                    "{}.{} required flag must match the schema",
                    shape.ops[0],
                    field.name
                );
            }
        }
    }

    #[test]
    fn map_draft_patch_usage_error_names_the_documented_keys_of_the_attempted_op() {
        let invented_key = validate_map_tool_call(
            "map_draft_patch",
            &json!({"operations": [{"op": "terrain.set", "tileId": 128, "x": 20, "y": 248}]}),
        )
        .expect_err("terrain.set without before/after must be rejected");
        let ToolError::AdmissionRejected { message } = &invented_key else {
            panic!("expected an admission rejection, got {invented_key:?}");
        };
        assert!(
            message.starts_with("Usage: map_draft_patch(operations)."),
            "{message}"
        );
        assert!(
            message.contains(
                "operations[0] with op 'terrain.set' takes exactly {op, x, y, before, after}"
            ),
            "{message}"
        );
        assert!(
            message.contains("missing required key(s): before, after"),
            "{message}"
        );
        assert!(message.contains("unexpected key(s): tileId"), "{message}");

        let unknown_op = validate_map_tool_call(
            "map_draft_patch",
            &json!({"operations": [
                {"op": "terrain.rect", "x": 0, "y": 0, "width": 1, "height": 1, "after": 1},
                {"op": "terrain.paint", "x": 0, "y": 0},
            ]}),
        )
        .expect_err("an undocumented op must be rejected");
        let ToolError::AdmissionRejected { message } = &unknown_op else {
            panic!("expected an admission rejection, got {unknown_op:?}");
        };
        assert!(
            message.contains("operations[1].op 'terrain.paint' is not a documented op"),
            "{message}"
        );
        assert!(
            message.contains("documented ops: terrain.set, terrain.rect"),
            "{message}"
        );

        let missing_op = validate_map_tool_call(
            "map_draft_patch",
            &json!({"operations": [{"x": 0, "y": 0, "before": 0, "after": 1}]}),
        )
        .expect_err("an operation without op must be rejected");
        let ToolError::AdmissionRejected { message } = &missing_op else {
            panic!("expected an admission rejection, got {missing_op:?}");
        };
        assert!(
            message.contains("operations[0] must declare a string 'op'"),
            "{message}"
        );

        let bad_value = validate_map_tool_call(
            "map_draft_patch",
            &json!({"operations": [{"op": "terrain.set", "x": 0, "y": 0, "before": -1, "after": 1}]}),
        )
        .expect_err("an out-of-range value must be rejected");
        let ToolError::AdmissionRejected { message } = &bad_value else {
            panic!("expected an admission rejection, got {bad_value:?}");
        };
        assert!(
            message.contains("invalid value(s): before expects integer 0..=65535, got number -1"),
            "{message}"
        );

        assert_eq!(
            validate_map_tool_call(
                "map_draft_patch",
                &json!({"operations": [
                    {"op": "terrain.isom_brush", "isomX": 7, "isomY": 249, "brush": 2},
                    {"op": "terrain.rect", "x": 20, "y": 248, "width": 1, "height": 1, "after": 128},
                ]}),
            ),
            Ok(())
        );

        let named_brush = validate_map_tool_call(
            "map_draft_patch",
            &json!({"operations": [
                {"brush": "Dirt", "isomX": 7, "isomY": 249, "op": "terrain.isom_brush"},
            ]}),
        )
        .expect_err("a brush name instead of its numeric id must be rejected");
        let ToolError::AdmissionRejected { message } = &named_brush else {
            panic!("expected an admission rejection, got {named_brush:?}");
        };
        assert!(
            message.contains("operations[0] with op 'terrain.isom_brush' takes exactly {op, isomX, isomY, brush, [extent]}"),
            "{message}"
        );
        assert!(
            message
                .contains("invalid value(s): brush expects integer 0..=65535, got string \"Dirt\""),
            "{message}"
        );

        let nested_value = validate_map_tool_call(
            "map_draft_patch",
            &json!({"operations": [
                {"op": "unit.add", "state": {"typeId": "marine", "owner": 0, "x": 1, "y": 1}},
            ]}),
        )
        .expect_err("a nested string typeId must be rejected");
        let ToolError::AdmissionRejected { message } = &nested_value else {
            panic!("expected an admission rejection, got {nested_value:?}");
        };
        assert!(
            message.contains("check each nested value's type and range"),
            "{message}"
        );

        let other_tool = validate_map_tool_call("map_tile_info", &json!({"tileId": "1"}))
            .expect_err("map_tile_info with a string id must be rejected");
        let ToolError::AdmissionRejected { message } = &other_tool else {
            panic!("expected an admission rejection, got {other_tool:?}");
        };
        assert!(!message.contains("documented ops"), "{message}");
    }

    #[test]
    fn map_draft_patch_public_admission_accepts_all_documented_operations() {
        let schema = map_tool_registry()
            .into_iter()
            .find(|tool| tool.name == "map_draft_patch")
            .expect("map_draft_patch must be registered")
            .input_schema;
        let operations = materialized_map_operations(&schema)
            .iter()
            .map(required_map_schema_example)
            .collect::<Vec<_>>();
        assert_eq!(operations.len(), 20);
        for operation in &operations {
            assert_eq!(
                validate_map_tool_call("map_draft_patch", &json!({"operations": [operation]})),
                Ok(())
            );
        }
        assert_eq!(
            validate_map_tool_call("map_draft_patch", &json!({"operations": operations})),
            Ok(())
        );

        let valid_terrain_set = json!({
            "op": "terrain.set",
            "x": 0,
            "y": 0,
            "before": 0,
            "after": 1,
        });
        assert_eq!(
            validate_map_tool_call(
                "map_draft_patch",
                &json!({"operations": [valid_terrain_set.clone()]}),
            ),
            Ok(())
        );
        let mut extra_terrain_set = valid_terrain_set;
        extra_terrain_set["unexpected"] = json!(true);
        for args in [
            json!({"operations": [extra_terrain_set]}),
            json!({"operations": [{"type": "setTile", "x": 64, "y": 64, "tileId": 1}]}),
            json!({"operations": [{}]}),
        ] {
            assert!(matches!(
                validate_map_tool_call("map_draft_patch", &args),
                Err(ToolError::AdmissionRejected { .. })
            ));
        }
        assert!(matches!(
            validate_map_tool_call("map_apply", &json!({})),
            Err(ToolError::AdmissionRejected { message })
                if message.contains("original Apply is intentionally absent")
        ));
    }

    #[test]
    fn map_palette_query_schema_requires_a_bounded_structured_search() {
        let registry = map_tool_registry();
        let tool = registry
            .iter()
            .find(|tool| tool.name == "map_palette_query")
            .expect("map_palette_query must be registered");
        assert_object_contract(&tool.input_schema, &["kind", "query", "filter"], &["kind"]);
        let alternatives = tool.input_schema["anyOf"]
            .as_array()
            .expect("map_palette_query must advertise search alternatives");
        assert_eq!(alternatives.len(), 2);
        assert_object_contract(
            &alternatives[0],
            &["kind", "query", "filter"],
            &["kind", "query"],
        );
        assert_object_contract(
            &alternatives[1],
            &["kind", "query", "filter"],
            &["kind", "filter"],
        );
        assert_eq!(
            alternatives[0]["properties"],
            tool.input_schema["properties"]
        );
        assert_eq!(
            alternatives[1]["properties"],
            tool.input_schema["properties"]
        );
        assert_eq!(tool.input_schema["properties"]["query"]["minLength"], 1);
        assert_eq!(
            tool.input_schema["properties"]["kind"]["enum"],
            json!(MAP_PALETTE_CATALOG_KINDS)
        );
        assert!(tool.input_schema["properties"]["kind"]["description"]
            .as_str()
            .is_some_and(|description| {
                description.contains("semanticTerrain") && description.contains("brushes")
            }));

        let filter = &tool.input_schema["properties"]["filter"];
        assert_object_contract(
            filter,
            &[
                "id",
                "terrainType",
                "group",
                "variant",
                "graphicsValid",
                "walkability",
                "groundHeight",
                "buildability",
                "ramp",
                "blocksView",
                "overlay",
                "visible",
                "width",
                "height",
                "placementWidth",
                "placementHeight",
            ],
            &[],
        );
        assert_eq!(filter["minProperties"], 1);
        assert_integer_bounds(&filter["properties"]["group"], 0, 1_023);
        assert_integer_bounds(&filter["properties"]["variant"], 0, 15);
        assert_eq!(
            filter["properties"]["walkability"]["enum"],
            json!(["all", "any", "none"])
        );
        assert!(tool.input_schema["properties"].get("offset").is_none());
        assert!(tool.input_schema["properties"].get("limit").is_none());
    }

    #[test]
    fn map_palette_query_mcp_union_arms_preserve_the_complete_contract() {
        let descriptor = map_mcp_tool_descriptors()
            .into_iter()
            .find(|descriptor| descriptor["name"] == "map_palette_query")
            .expect("map_palette_query must be advertised");
        let schema = &descriptor["inputSchema"];
        let branches = schema["anyOf"]
            .as_array()
            .expect("map_palette_query must advertise search alternatives");
        assert_eq!(branches.len(), 2);

        let query = branches
            .iter()
            .find(|branch| {
                branch["required"]
                    .as_array()
                    .is_some_and(|required| required.contains(&json!("query")))
            })
            .expect("one query alternative must be advertised");
        let filter = branches
            .iter()
            .find(|branch| {
                branch["required"]
                    .as_array()
                    .is_some_and(|required| required.contains(&json!("filter")))
            })
            .expect("one filter alternative must be advertised");

        let query_validator = jsonschema::JSONSchema::options()
            .with_draft(jsonschema::Draft::Draft7)
            .compile(query)
            .expect("query alternative must be valid JSON Schema");
        let filter_validator = jsonschema::JSONSchema::options()
            .with_draft(jsonschema::Draft::Draft7)
            .compile(filter)
            .expect("filter alternative must be valid JSON Schema");

        assert!(query_validator
            .validate(&json!({"kind": "tiles", "query": "floor"}))
            .is_ok());
        assert!(filter_validator
            .validate(&json!({"kind": "tiles", "filter": {"id": 0}}))
            .is_ok());
        let captured_invalid = json!({"kind": "tiles", "filter": {"tileId": 0}});
        assert!(
            filter_validator.validate(&captured_invalid).is_err(),
            "standalone filter alternative accepted the captured invalid tileId call"
        );
        assert!(
            query_validator
                .validate(&json!({"query": "floor"}))
                .is_err(),
            "standalone query alternative accepted a call without kind"
        );
        assert!(query_validator.validate(&captured_invalid).is_err());

        let properties = ["kind", "query", "filter"];
        assert_object_contract(query, &properties, &["kind", "query"]);
        assert_object_contract(filter, &properties, &["kind", "filter"]);
        assert_eq!(query["properties"], schema["properties"]);
        assert_eq!(filter["properties"], schema["properties"]);

        let complete_validator = jsonschema::JSONSchema::options()
            .with_draft(jsonschema::Draft::Draft7)
            .compile(schema)
            .expect("complete map palette input must be valid JSON Schema");
        for accepted in [
            json!({"kind": "tiles", "query": "floor"}),
            json!({"kind": "tiles", "filter": {"id": 0}}),
            json!({"kind": "tiles", "query": "floor", "filter": {"id": 0}}),
        ] {
            assert!(
                complete_validator.validate(&accepted).is_ok(),
                "complete schema rejected valid palette query: {accepted}"
            );
        }
        for rejected in [
            json!({"kind": "tiles"}),
            json!({"kind": "tiles", "query": ""}),
            json!({"kind": "tiles", "filter": {}}),
            json!({"kind": "tiles", "query": "floor", "offset": 1}),
            json!({"kind": "semanticTerrain", "query": "floor"}),
            captured_invalid,
        ] {
            assert!(
                complete_validator.validate(&rejected).is_err(),
                "complete schema accepted invalid palette query: {rejected}"
            );
        }
    }

    #[test]
    fn map_stamp_tools_require_strict_source_union_destinations_and_collision_policy() {
        let registry = map_tool_registry();
        for (name, properties, required) in [
            (
                "map_stamp_preview",
                vec!["source", "destinations"],
                vec!["source", "destinations"],
            ),
            (
                "map_stamp_place",
                vec!["source", "destinations", "collisionPolicy"],
                vec!["source", "destinations", "collisionPolicy"],
            ),
        ] {
            let tool = registry
                .iter()
                .find(|tool| tool.name == name)
                .unwrap_or_else(|| panic!("{name} must be registered"));
            assert_object_contract(&tool.input_schema, &properties, &required);
            let source = &tool.input_schema["properties"]["source"];
            let alternatives = source["oneOf"].as_array().unwrap();
            assert_eq!(alternatives.len(), 2);
            assert_object_contract(
                &alternatives[0],
                &["kind", "selectionId"],
                &["kind", "selectionId"],
            );
            assert_object_contract(
                &alternatives[1],
                &["kind", "importId"],
                &["kind", "importId"],
            );
            assert_eq!(
                alternatives[0]["properties"]["kind"]["enum"],
                json!(["candidateSelection"])
            );
            assert_eq!(
                alternatives[1]["properties"]["kind"]["enum"],
                json!(["imported"])
            );
            let serialized = source.to_string().to_ascii_lowercase();
            for forbidden in ["path", "picker", "blob", "chk", "mtxm"] {
                assert!(!serialized.contains(forbidden));
            }
            let destinations = &tool.input_schema["properties"]["destinations"];
            assert_eq!(destinations["type"], "array");
            assert_eq!(destinations["minItems"], 1);
            assert_eq!(destinations["maxItems"], 64);
            assert_object_contract(&destinations["items"], &["x", "y"], &["x", "y"]);
            assert_integer_bounds(&destinations["items"]["properties"]["x"], 0, 65_535);
            assert_integer_bounds(&destinations["items"]["properties"]["y"], 0, 65_535);
            let descriptor = map_mcp_tool_descriptors()
                .into_iter()
                .find(|descriptor| descriptor["name"] == name)
                .unwrap_or_else(|| panic!("{name} must be advertised"));
            assert_eq!(descriptor["inputSchema"], tool.input_schema);
            assert!(descriptor.get("parameters").is_none());
        }
        let place = registry
            .iter()
            .find(|tool| tool.name == "map_stamp_place")
            .unwrap();
        assert_eq!(
            place.input_schema["properties"]["collisionPolicy"]["enum"],
            json!(["merge", "replace"])
        );
        assert!(
            serde_json::from_value::<crate::map_stamp::StampPreviewInput>(json!({
                "source": {"kind": "candidateSelection", "selectionId": "selection"},
                "destinations": [{"x": 1, "y": 2}]
            }))
            .is_ok()
        );
        assert!(
            serde_json::from_value::<crate::map_stamp::StampPreviewInput>(json!({
                "source": {"kind": "imported", "importId": "import"},
                "destinations": [{"x": 1, "y": 2}]
            }))
            .is_ok()
        );
        assert!(
            serde_json::from_value::<crate::map_stamp::StampPreviewInput>(json!({
                "source": {
                    "kind": "imported",
                    "importId": "import",
                    "selectionId": "selection"
                },
                "destinations": [{"x": 1, "y": 2}]
            }))
            .is_err()
        );
    }

    #[test]
    fn map_image_place_schema_exposes_only_request_ref_and_tile_transform() {
        let registry = map_tool_registry();
        let tool = registry
            .iter()
            .find(|tool| tool.name == "map_image_place")
            .expect("map_image_place must be registered");
        assert_object_contract(
            &tool.input_schema,
            &["imageRef", "x", "y", "width", "height"],
            &["imageRef", "x", "y", "width", "height"],
        );
        assert_eq!(
            tool.input_schema["properties"]["imageRef"]["type"],
            "string"
        );
        for property in ["x", "y"] {
            assert_integer_bounds(&tool.input_schema["properties"][property], 0, 65_535);
        }
        for property in ["width", "height"] {
            assert_integer_bounds(&tool.input_schema["properties"][property], 1, 65_535);
        }
        for forbidden in ["path", "tiles", "tileId", "mtxm", "palette", "permission"] {
            assert!(tool.input_schema["properties"].get(forbidden).is_none());
        }
        let descriptor = map_mcp_tool_descriptors()
            .into_iter()
            .find(|descriptor| descriptor["name"] == "map_image_place")
            .expect("map_image_place must be advertised");
        assert_eq!(descriptor["inputSchema"], tool.input_schema);
        assert!(descriptor.get("parameters").is_none());
    }

    #[test]
    fn map_mcp_descriptor_advertises_registry_input_schema_verbatim() {
        let registry = map_tool_registry();
        let registry_patch = registry
            .iter()
            .find(|tool| tool.name == "map_draft_patch")
            .expect("map_draft_patch must be registered");
        let descriptors = map_mcp_tool_descriptors();
        let descriptor = descriptors
            .iter()
            .find(|descriptor| descriptor["name"] == "map_draft_patch")
            .expect("map_draft_patch must be advertised");

        assert_eq!(descriptor["inputSchema"], registry_patch.input_schema);
        assert!(descriptor.get("parameters").is_none());
    }

    #[test]
    fn mcp_advertisement_uses_real_input_schema_names_verbatim() {
        let descriptors = mcp_tool_descriptors();
        let dat_patch = descriptors
            .iter()
            .find(|descriptor| descriptor["name"] == "dat_patch")
            .expect("dat_patch must be advertised to MCP");

        assert_eq!(
            dat_patch["inputSchema"],
            schema(
                serde_json::json!({"changes": dat_patch_changes_schema()}),
                &["changes"],
            )
        );
        assert!(
            dat_patch.get("parameters").is_none(),
            "MCP advertisement must use inputSchema, not a derived generic parameters wrapper"
        );
        assert!(
            dat_patch["description"]
                .as_str()
                .is_some_and(|description| !description.is_empty()),
            "MCP descriptor must carry the registry description"
        );
    }

    #[test]
    fn admission_allows_repeated_file_writes_and_build_without_plan_approval() {
        let mut state = RequestState::for_request("req-mutate");
        state.record_search_docs();

        for (path, code) in [("a.eps", "1"), ("b.eps", "2"), ("c.eps", "3")] {
            admit_tool_call(
                &mut state,
                "file_write",
                &serde_json::json!({"path": path, "code": code}),
            )
            .unwrap();
        }
        admit_tool_call(&mut state, BUILD_RUN_TOOL, &serde_json::json!({})).unwrap();
        assert_eq!(state.iteration_action_count, 4);
        assert_eq!(state.write_action_count, 3);
        assert_eq!(state.read_action_count, 1);

        assert_eq!(state.action_count, 4);
    }

    #[test]
    fn admission_tracks_search_budget_without_recording_evidence_or_general_actions() {
        let mut state = RequestState::for_request("req-search");

        admit_tool_call(
            &mut state,
            SEARCH_DOCS_TOOL,
            &serde_json::json!({"query": "button set"}),
        )
        .unwrap();

        assert!(!state.docs_searched);
        assert_eq!(state.search_docs_count, 0);
        assert_eq!(state.action_count, 0);
        assert_eq!(state.read_action_count, 1);
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
    fn action_threshold_requests_an_iteration_boundary_without_refusing_future_work() {
        let mut state = RequestState::for_request("req-boundary");

        for _ in 0..ITERATION_TOOL_ACTION_THRESHOLD {
            admit_tool_call(&mut state, "project_status", &serde_json::json!({})).unwrap();
        }
        assert!(state.iteration_action_boundary_reached());
        assert_eq!(state.action_count, ITERATION_TOOL_ACTION_THRESHOLD);

        state.begin_iteration();
        assert!(!state.iteration_action_boundary_reached());
        admit_tool_call(&mut state, "project_status", &serde_json::json!({})).unwrap();
        assert_eq!(state.action_count, ITERATION_TOOL_ACTION_THRESHOLD + 1);
        assert_eq!(state.iteration_action_count, 1);
    }

    #[test]
    fn search_progress_allows_more_than_120_queries_and_rejects_exact_repeats() {
        let mut state = RequestState::for_request("req-search-progress");

        for index in 0_u64..121 {
            let args = serde_json::json!({"query": format!("wave growth {index}"), "k": 5});
            admit_tool_call(&mut state, SEARCH_DOCS_TOOL, &args).unwrap();
            state.record_search_docs_hits(&args, &[index]);
        }
        assert_eq!(state.search_docs_count, 121);
        assert_eq!(state.search_docs_unique_hits, 121);
        assert_eq!(state.action_count, 0);

        let repeated = serde_json::json!({"query": "  WAVE   GROWTH 120 ", "k": 5});
        let error = admit_tool_call(&mut state, SEARCH_DOCS_TOOL, &repeated).unwrap_err();
        assert!(error.to_string().contains("no-progress"));

        let novel = serde_json::json!({"query": "wave growth next", "k": 5});
        admit_tool_call(&mut state, SEARCH_DOCS_TOOL, &novel).unwrap();
    }

    #[test]
    fn build_admission_allows_ten_iterations() {
        let mut state = RequestState::for_request("req-build-admission");
        for _ in 0..10 {
            admit_tool_call(&mut state, BUILD_RUN_TOOL, &serde_json::json!({})).unwrap();
        }
        assert_eq!(state.action_count, 10);
    }
    #[test]
    fn dependency_preparation_admits_more_than_four_distinct_sets() {
        let mut state = RequestState::for_request("req-dependency-admission");
        for index in 0..5 {
            admit_tool_call(
                &mut state,
                PYTHON_DEPENDENCIES_PREPARE_TOOL,
                &serde_json::json!({
                    "dependencies": [format!("example-{index}==1.0.0")]
                }),
            )
            .unwrap();
        }
        assert_eq!(state.action_count, 0);
    }

    #[test]
    fn build_progress_uses_revision_and_stable_diagnostics() {
        let mut state = RequestState::for_request("req-build-progress");
        let first = native_build_result(
            false,
            vec![native_build_error(
                "C:\\temp\\one\\main.eps:7 timestamp=100",
            )],
        );
        let noisy_repeat = native_build_result(
            false,
            vec![native_build_error("D:\\other\\main.eps:7 timestamp=999")],
        );
        assert_eq!(
            build_diagnostics_fingerprint(&first.errors),
            build_diagnostics_fingerprint(&noisy_repeat.errors),
            "raw output paths and timestamps must not affect progress identity"
        );

        assert!(matches!(
            state.record_build("revision-a".to_string(), &first),
            BuildProgressOutcome::Progress(_)
        ));
        let repeated = state.record_build("revision-a".to_string(), &noisy_repeat);
        assert!(matches!(repeated, BuildProgressOutcome::NoProgress { .. }));
        assert!(state.build_blocked_for_revision("revision-a"));

        assert!(matches!(
            state.record_build("revision-b".to_string(), &noisy_repeat),
            BuildProgressOutcome::Progress(_)
        ));
        let success = native_build_result(true, Vec::new());
        assert!(matches!(
            state.record_build("revision-b".to_string(), &success),
            BuildProgressOutcome::Progress(BuildProgress { success: true, .. })
        ));
        assert!(matches!(
            state.record_build("revision-b".to_string(), &success),
            BuildProgressOutcome::Progress(BuildProgress { success: true, .. })
        ));
        assert!(!state.build_blocked_for_revision("revision-b"));
    }

    #[test]
    fn build_progress_accepts_improving_diagnostics() {
        let mut state = RequestState::for_request("req-build-improving");
        let mut second_error = native_build_error("raw-second");
        second_error.line = 12;
        second_error.message = "second error".to_string();
        let two_errors =
            native_build_result(false, vec![native_build_error("raw-first"), second_error]);
        let one_error = native_build_result(false, vec![native_build_error("raw-first")]);

        assert!(matches!(
            state.record_build("revision-a".to_string(), &two_errors),
            BuildProgressOutcome::Progress(_)
        ));
        assert!(matches!(
            state.record_build("revision-a".to_string(), &one_error),
            BuildProgressOutcome::Progress(BuildProgress { error_count: 1, .. })
        ));
        assert!(!state.build_blocked_for_revision("revision-a"));
    }

    #[test]
    fn build_progress_detects_short_abab_cycles() {
        let mut state = RequestState::for_request("req-build-cycle");
        let failed = native_build_result(false, vec![native_build_error("raw")]);
        for revision in ["a", "b", "a"] {
            assert!(matches!(
                state.record_build(revision.to_string(), &failed),
                BuildProgressOutcome::Progress(_)
            ));
        }
        let outcome = state.record_build("b".to_string(), &failed);
        assert!(matches!(
            outcome,
            BuildProgressOutcome::NoProgress { ref reason, .. }
                if reason.contains("A → B → A → B")
        ));
        assert!(state.build_blocked_for_revision("b"));
    }
    #[test]
    fn missing_required_arg_error_carries_self_correcting_usage_line() {
        let mut state = RequestState::for_request("req-args");

        let error = admit_tool_call(&mut state, "xdat_get", &serde_json::json!({})).unwrap_err();
        let message = error.to_string();

        assert!(message.contains("Usage: xdat_get(items)"));
        assert!(message.contains("'items'"));
        assert_eq!(
            state.action_count, 0,
            "calls rejected by arg validation must not consume an action"
        );
    }

    #[test]
    fn fresh_request_id_resets_per_request_evidence_and_progress() {
        let mut state = RequestState::for_request("req-A");
        admit_tool_call(
            &mut state,
            SEARCH_DOCS_TOOL,
            &serde_json::json!({"query": "request-scoped evidence"}),
        )
        .unwrap();
        state.record_search_docs();
        admit_tool_call(
            &mut state,
            "file_write",
            &serde_json::json!({"path": "a.eps", "code": "1"}),
        )
        .unwrap();
        admit_tool_call(&mut state, BUILD_RUN_TOOL, &serde_json::json!({})).unwrap();

        assert_eq!(state.request_id, "req-A");
        assert!(state.docs_searched);
        assert_eq!(state.search_docs_count, 0);
        assert_eq!(state.action_count, 2);
        assert_eq!(state.iteration_action_count, 2);
        assert_eq!(state.read_action_count, 2);
        assert_eq!(state.write_action_count, 1);

        state.start_request("req-B");

        assert_eq!(state.request_id, "req-B");
        assert!(!state.docs_searched, "evidence gate is per-request");
        assert_eq!(state.search_docs_count, 0);
        assert_eq!(state.action_count, 0);
        assert_eq!(state.iteration_action_count, 0);
        assert_eq!(state.read_action_count, 0);
        assert_eq!(state.write_action_count, 0);

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
    #[test]
    fn sound_tools_are_main_eps_only_and_expose_bounded_offline_edits() {
        let registry = tool_registry();
        let list = registry
            .iter()
            .find(|spec| spec.name == MAP_SOUND_LIST_TOOL)
            .expect("main EPS registry must expose map_sound_list");
        assert!(!list.requires_write_workspace);
        assert!(!list.requires_project_transaction);
        assert_eq!(list.input_schema["properties"], json!({}));

        let import = registry
            .iter()
            .find(|spec| spec.name == MAP_SOUND_IMPORT_TOOL)
            .expect("main EPS registry must expose map_sound_import");
        assert!(import.requires_write_workspace);
        assert!(import.requires_project_transaction);
        assert_eq!(import.input_schema["required"], json!(["audioRef"]));
        assert_eq!(
            import.input_schema["properties"]
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            vec!["audioRef".to_string()]
        );
        assert_eq!(import.input_schema["additionalProperties"], json!(false));
        let edit = registry
            .iter()
            .find(|spec| spec.name == MAP_SOUND_EDIT_TOOL)
            .expect("main EPS registry must expose map_sound_edit");
        assert!(edit.requires_write_workspace);
        assert!(edit.requires_project_transaction);
        assert_eq!(edit.input_schema["required"], json!(["mpqPath"]));
        assert_eq!(
            edit.input_schema["properties"]["volumePercent"]["maximum"],
            json!(400)
        );
        assert_eq!(
            edit.input_schema["properties"]["fadeInMs"]["maximum"],
            json!(3_600_000)
        );
        assert_eq!(edit.input_schema["additionalProperties"], json!(false));
        assert!(map_tool_registry().iter().all(|spec| !matches!(
            spec.name,
            MAP_SOUND_LIST_TOOL | MAP_SOUND_IMPORT_TOOL | MAP_SOUND_EDIT_TOOL
        )));
    }
}
