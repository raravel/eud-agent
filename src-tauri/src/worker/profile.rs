use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerProfile {
    Research,
    MapResearch,
    #[default]
    Implementation,
}

impl WorkerProfile {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Research => "research",
            Self::MapResearch => "map_research",
            Self::Implementation => "implementation",
        }
    }

    pub const fn can_write(self) -> bool {
        match self {
            Self::Research | Self::MapResearch => false,
            Self::Implementation => true,
        }
    }

    pub fn permits(self, tool: &str) -> bool {
        if crate::tools::hidden_from_workers(tool) {
            return false;
        }
        if crate::tools::is_fs_tool(tool) {
            return true;
        }
        match self {
            Self::Implementation => true,
            Self::Research => matches!(
                tool,
                "project_status"
                    | "list_files"
                    | "read_file"
                    | "source_search"
                    | "fs_read"
                    | "fs_glob"
                    | "fs_grep"
                    | "search_docs"
                    | "docs_get"
                    | "dat_get"
                    | "iscript_info"
                    | "build_log_read"
            ),
            Self::MapResearch => matches!(
                tool,
                "project_status"
                    | "map_info"
                    | "map_minimap"
                    | "map_sound_list"
                    | "map_selection_list"
                    | "map_selection_read"
                    | "dat_get"
                    | "search_docs"
                    | "docs_get"
            ),
        }
    }

    pub const fn prompt(self) -> &'static str {
        match self {
            Self::Implementation => super::WORKER_PROMPT,
            Self::Research => "[background worker]\n[profile: research]\nInvestigate the self-contained goal in this isolated session. You may freely read, create, overwrite and edit project .md/.txt documents with fs_* without approval. You cannot change source or map binaries, build, ask the user, or delegate. Use scoped source and documentation searches. Return a concise answer with exact file/line or documentation references, relevant definitions, assumptions and unresolved questions. Keep raw search results and exploration here; the parent receives only your bounded final summary. Read project_status and report the observed project revision when available.",
            Self::MapResearch => "[background worker]\n[profile: map_research]\nInvestigate only the map facts needed by the self-contained goal in this isolated session. You may freely read, create, overwrite and edit project .md/.txt documents with fs_* without approval. You cannot change source or map binaries, build, ask the user, or delegate. Query relevant areas and layers instead of dumping the map. Return a concise answer with exact object/location ids, coordinates, constraints, evidence tools and the observed map revision or source hash when available. Distinguish observed facts from assumptions. Keep raw map data and intermediate analysis here; the parent receives only your bounded final summary.",
        }
    }
}

pub const PARENT_PROMPT: &str = r#"[role delegation]
Use worker_start when a substantial independent task benefits from a separate context. Group related API/documentation questions into one research goal and reuse its evidence while the observed revision still matches. Small exact reads and editing-target re-reads belong here. Research workers have project/document tools, not shell or web access; do not assign them upstream browsing or runtime execution. Use profile=implementation for independent changes with explicit file ownership. A worker has a fresh context and opens in the existing split chat panels; give it a self-contained goal, needed user constraints and a short role-specific title.

Write the goal as readable sentences in the user's language. For complex tasks, use actual line breaks and short labeled sections for the goal, files/scope, requirements, prohibitions, validation and expected return. Include only useful sections and put one requirement per bullet. Simple goals may use one sentence. Preserve normal word spacing and exact code/API identifiers, paths, numeric limits and URLs. Be concise by removing repetition, never by running words together, dropping separators or omitting constraints.

Do useful independent work while it runs. Do not poll, duplicate its exploration, or claim the dependent work is complete: finish this turn with a brief pending-work handoff if its answer is needed. The app resumes you once that request's workers have settled, or earlier on failure for recovery. A settlement or successful build is implementation evidence; verify the user's acceptance criteria separately. Consult the project work checkpoint before repeating research or builds; invalidate evidence when its revision changes. At most three workers run at once; autonomous runs cannot start workers. Map placement still goes through map_task_request, not investigation workers."#;

#[cfg(test)]
mod tests {
    use super::WorkerProfile;

    #[test]
    fn investigation_profiles_offer_registered_reads_and_document_io() {
        for profile in [WorkerProfile::Research, WorkerProfile::MapResearch] {
            let tools = crate::tools::tool_registry()
                .into_iter()
                .filter(|spec| profile.permits(spec.name))
                .collect::<Vec<_>>();
            assert!(!tools.is_empty());
            for spec in tools {
                assert!(!spec.requires_write_workspace, "{}", spec.name);
                assert!(!spec.requires_project_transaction, "{}", spec.name);
                assert_ne!(spec.name, "worker_start");
            }
        }
        assert!(WorkerProfile::Research.permits("source_search"));
        assert!(!WorkerProfile::Research.permits("map_info"));
        assert!(WorkerProfile::MapResearch.permits("map_info"));
        assert!(!WorkerProfile::MapResearch.permits("source_search"));
    }

    #[test]
    fn saved_workers_keep_their_role_and_legacy_workers_keep_implementation() {
        let mut worker = crate::worker::WorkerRecord::new(
            "child".into(),
            "request".into(),
            "research".into(),
            "goal".into(),
        );
        worker.profile = WorkerProfile::MapResearch;
        let mut value = serde_json::to_value(&worker).unwrap();
        let restored: crate::worker::WorkerRecord = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(restored.profile, WorkerProfile::MapResearch);
        value.as_object_mut().unwrap().remove("profile");
        let legacy: crate::worker::WorkerRecord = serde_json::from_value(value).unwrap();
        assert_eq!(legacy.profile, WorkerProfile::Implementation);
    }
}
