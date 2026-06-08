//! Tool-layer safety rails and per-request evidence state.
//!
//! The functions here are small, deterministic backstops for crash-critical
//! first principles and the EUD-090 evidence requirement.

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
}

/// Mutable state carried for one agent request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestState {
    /// Set once a `search_docs` call has run successfully, even with zero hits.
    pub docs_searched: bool,
}

impl RequestState {
    /// Create request state with the evidence search flag unset.
    pub fn new() -> Self {
        Self {
            docs_searched: false,
        }
    }

    /// Record that `search_docs` ran successfully for this request.
    pub fn record_search_docs(&mut self) {
        self.docs_searched = true;
    }
}

impl Default for RequestState {
    fn default() -> Self {
        Self::new()
    }
}

/// Minimal tool metadata needed by the evidence gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolSpec {
    pub name: &'static str,
    pub mutating: bool,
}

impl ToolSpec {
    /// Construct a tool spec for a mutating tool.
    pub const fn mutating(name: &'static str) -> Self {
        Self {
            name,
            mutating: true,
        }
    }

    /// Construct a tool spec for a read-only tool.
    pub const fn read_only(name: &'static str) -> Self {
        Self {
            name,
            mutating: false,
        }
    }
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
}
