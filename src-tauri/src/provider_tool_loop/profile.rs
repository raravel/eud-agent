use std::collections::BTreeSet;

use serde_json::{json, Value};

/// The result-submission function every delegated run must call exactly once.
/// It is an encoding channel for the run's schema result, never an EUD tool:
/// the gate captures it and it is not dispatched to the tool runtime.
pub const SUBMIT_RESULT_TOOL: &str = "submit_result";

/// Which tools one run may see and execute.
#[derive(Debug, Clone)]
pub enum ToolProfile {
    /// The ordinary session foreground: the full registry for the session kind,
    /// with read-to-write transition on the first canonical mutation.
    Foreground,
    /// An isolated read-only run that ends by calling [`SUBMIT_RESULT_TOOL`].
    Delegated(DelegatedToolProfile),
}

#[derive(Debug, Clone)]
pub struct DelegatedToolProfile {
    allowed: BTreeSet<String>,
    result_descriptor: Value,
}

impl DelegatedToolProfile {
    /// `allowed` names must be registered EPS read tools. Unknown names, write
    /// tools, `ask`, and `submit_result` are refused here so a profile can never
    /// advertise a mutation or silently advertise nothing. Map candidate tools
    /// are registered as reads with candidate authority, so Map sessions have no
    /// delegated profile until a Map-specific classification exists.
    pub fn new(
        allowed: impl IntoIterator<Item = impl Into<String>>,
        output_schema: &Value,
    ) -> Result<Self, String> {
        let allowed = allowed
            .into_iter()
            .map(Into::into)
            .collect::<BTreeSet<String>>();
        for name in &allowed {
            if name == SUBMIT_RESULT_TOOL
                || name == crate::tools::ASK_TOOL
                || name == crate::tools::DELEGATE_READ_TOOL
                || name == crate::tools::MAP_TASK_REQUEST_TOOL
            {
                return Err(format!("delegated tool profile cannot include '{name}'"));
            }
            match crate::tools::tool_spec(name) {
                None => {
                    return Err(format!(
                        "delegated tool profile names unregistered EPS tool '{name}'"
                    ))
                }
                Some(spec) if spec.requires_write_workspace => {
                    return Err(format!(
                        "delegated tool profile cannot include write tool '{name}'"
                    ))
                }
                Some(_) => {}
            }
        }
        if !output_schema.is_object() {
            return Err("delegated result schema must be a JSON object schema".to_string());
        }
        Ok(Self {
            allowed,
            result_descriptor: json!({
                "name": SUBMIT_RESULT_TOOL,
                "description": "Submit the final structured result of this run. Call it exactly once, after every needed read; the run ends when it is accepted.",
                "inputSchema": output_schema,
            }),
        })
    }

    pub fn allows(&self, name: &str) -> bool {
        self.allowed.contains(name)
    }

    pub fn allowed(&self) -> impl Iterator<Item = &str> {
        self.allowed.iter().map(String::as_str)
    }

    /// The submission descriptor alone: what the model sees on the final
    /// tool round, when reads are no longer executed.
    pub(super) fn submission_descriptors(&self) -> Vec<Value> {
        vec![self.result_descriptor.clone()]
    }

    /// Filter the session registry to this profile and append the submission
    /// descriptor, so unknown-tool admission rejects everything else.
    pub(super) fn descriptors(&self, registry: Vec<Value>) -> Vec<Value> {
        let mut descriptors = registry
            .into_iter()
            .filter(|descriptor| {
                descriptor
                    .get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|name| self.allows(name))
            })
            .collect::<Vec<_>>();
        descriptors.push(self.result_descriptor.clone());
        descriptors
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": { "summary": { "type": "string" } },
            "required": ["summary"],
            "additionalProperties": false
        })
    }

    #[test]
    fn profile_refuses_write_tools_ask_submit_result_and_unknown_names() {
        assert!(DelegatedToolProfile::new(["file_write"], &schema()).is_err());
        assert!(DelegatedToolProfile::new(["dat_patch"], &schema()).is_err());
        assert!(DelegatedToolProfile::new([crate::tools::ASK_TOOL], &schema()).is_err());
        // Delegation never nests.
        assert!(DelegatedToolProfile::new([crate::tools::DELEGATE_READ_TOOL], &schema()).is_err());
        assert!(DelegatedToolProfile::new([SUBMIT_RESULT_TOOL], &schema()).is_err());
        assert!(DelegatedToolProfile::new(["read_file"], &json!("not-a-schema")).is_err());
        // A typo and Map-only candidate tools are all unregistered EPS tools.
        assert!(DelegatedToolProfile::new(["read_files"], &schema()).is_err());
        assert!(DelegatedToolProfile::new(["map_candidate_finalize"], &schema()).is_err());
        assert!(DelegatedToolProfile::new(["map_draft_patch"], &schema()).is_err());
    }

    #[test]
    fn descriptors_keep_only_the_profile_plus_submit_result() {
        let profile = DelegatedToolProfile::new(["read_file", "list_files"], &schema()).unwrap();
        let descriptors = profile.descriptors(crate::tools::mcp_tool_descriptors());
        let names = descriptors
            .iter()
            .map(|descriptor| descriptor["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(names, ["list_files", "read_file", SUBMIT_RESULT_TOOL]);
        assert_eq!(descriptors[2]["inputSchema"], schema());
    }
}
