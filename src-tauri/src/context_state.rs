//! Instruction epochs and model-facing baseline/delta assembly.
//!
//! The durable cursor advances only after a successful primary Codex turn. Every
//! rendered dynamic section is revision-labelled, so a failed delivery can be
//! retried without inventing state or duplicating an unversioned instruction.

use serde::{Deserialize, Serialize};

use crate::task_state::sha256_bytes;

pub const CONTEXT_STATE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelContextCursor {
    pub thread_id: Option<String>,
    pub epoch: u64,
    pub memory_sha256: Option<String>,
    pub wiki_sha256: Option<String>,
    pub task_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionContextState {
    pub schema_version: u32,
    pub instruction_epoch: u64,
    pub static_prompt_fingerprint: String,
    pub delivered: ModelContextCursor,
}

impl Default for SessionContextState {
    fn default() -> Self {
        Self {
            schema_version: CONTEXT_STATE_SCHEMA_VERSION,
            instruction_epoch: 0,
            static_prompt_fingerprint: String::new(),
            delivered: ModelContextCursor::default(),
        }
    }
}

impl SessionContextState {
    pub fn initialize_baseline(&mut self, static_baseline: &str) {
        if self.instruction_epoch == 0 {
            self.instruction_epoch = 1;
        }
        if self.static_prompt_fingerprint.is_empty() {
            self.static_prompt_fingerprint = static_prompt_fingerprint(static_baseline);
        }
    }

    pub fn baseline_matches(&self, static_baseline: &str) -> bool {
        self.static_prompt_fingerprint.is_empty()
            || self.static_prompt_fingerprint == static_prompt_fingerprint(static_baseline)
    }

    pub fn reset_epoch(&mut self, static_baseline: &str) -> u64 {
        self.instruction_epoch = self.instruction_epoch.max(1).saturating_add(1);
        self.static_prompt_fingerprint = static_prompt_fingerprint(static_baseline);
        self.delivered = ModelContextCursor::default();
        self.instruction_epoch
    }

    pub fn adopt_legacy_thread(
        &mut self,
        static_baseline: &str,
        thread_id: String,
        memory: Option<&str>,
        wiki: Option<&str>,
        task_revision: u64,
    ) {
        self.initialize_baseline(static_baseline);
        if self.delivered.epoch == 0 {
            self.delivered = ModelContextCursor {
                thread_id: Some(thread_id),
                epoch: self.instruction_epoch,
                memory_sha256: section_hash(memory),
                wiki_sha256: section_hash(wiki),
                task_revision,
            };
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextDeliveryMode {
    Full,
    Delta,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextAssembly {
    pub text: String,
    pub mode: ContextDeliveryMode,
    pub cursor: ModelContextCursor,
}

pub struct ContextAssemblyInput<'a> {
    pub static_baseline: &'a str,
    pub project_state: &'a str,
    pub project_memory: Option<&'a str>,
    pub wiki_facts: Option<&'a str>,
    pub reference_context: Option<&'a str>,
    pub task_revision: u64,
    pub task_snapshot: &'a str,
    pub task_delta: Option<&'a str>,
    pub replay_transcript: Option<&'a str>,
    pub resolved_mentions: Option<&'a str>,
    pub user_text: &'a str,
    pub current_thread_id: Option<&'a str>,
    pub force_full: bool,
}

pub fn static_prompt_fingerprint(static_baseline: &str) -> String {
    sha256_bytes(static_baseline.as_bytes())
}

pub fn assemble_context(
    state: &SessionContextState,
    input: ContextAssemblyInput<'_>,
) -> Result<ContextAssembly, String> {
    if state.instruction_epoch == 0 {
        return Err("instruction epoch is not initialized".to_string());
    }
    let fingerprint = static_prompt_fingerprint(input.static_baseline);
    if state.static_prompt_fingerprint != fingerprint {
        return Err("static_prompt_fingerprint_changed".to_string());
    }

    let full = input.force_full
        || state.delivered.epoch != state.instruction_epoch
        || state.delivered.thread_id.as_deref() != input.current_thread_id;
    let memory_hash = section_hash(input.project_memory);
    let wiki_hash = section_hash(input.wiki_facts);
    let mut parts = Vec::new();

    if full {
        push_nonempty(&mut parts, input.static_baseline);
        push_nonempty(&mut parts, input.project_state);
        if let Some(memory) = normalized(input.project_memory) {
            parts.push(memory.to_string());
        }
        if let Some(wiki) = normalized(input.wiki_facts) {
            parts.push(wiki.to_string());
        }
        if let Some(reference) = normalized(input.reference_context) {
            parts.push(reference.to_string());
        }
        push_nonempty(&mut parts, input.task_snapshot);
        if let Some(transcript) = normalized(input.replay_transcript) {
            parts.push(transcript.to_string());
        }
    } else {
        push_nonempty(&mut parts, input.project_state);
        if state.delivered.memory_sha256 != memory_hash {
            parts.push(replacement_section(
                "project memory",
                state.instruction_epoch,
                memory_hash.as_deref(),
                input.project_memory,
            ));
        }
        if state.delivered.wiki_sha256 != wiki_hash {
            parts.push(replacement_section(
                "wiki facts",
                state.instruction_epoch,
                wiki_hash.as_deref(),
                input.wiki_facts,
            ));
        }
        if state.delivered.task_revision != input.task_revision {
            let delta = input.task_delta.ok_or_else(|| {
                "task revision changed without a model-facing task delta".to_string()
            })?;
            push_nonempty(&mut parts, delta);
        }
    }

    if let Some(mentions) = normalized(input.resolved_mentions) {
        parts.push(mentions.to_string());
    }
    parts.push(format!("[user message]\n{}", input.user_text));

    Ok(ContextAssembly {
        text: parts.join("\n\n"),
        mode: if full {
            ContextDeliveryMode::Full
        } else {
            ContextDeliveryMode::Delta
        },
        cursor: ModelContextCursor {
            thread_id: input.current_thread_id.map(str::to_string),
            epoch: state.instruction_epoch,
            memory_sha256: memory_hash,
            wiki_sha256: wiki_hash,
            task_revision: input.task_revision,
        },
    })
}

fn normalized(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn push_nonempty(parts: &mut Vec<String>, value: &str) {
    let value = value.trim();
    if !value.is_empty() {
        parts.push(value.to_string());
    }
}

fn section_hash(value: Option<&str>) -> Option<String> {
    normalized(value).map(|value| sha256_bytes(value.as_bytes()))
}

fn replacement_section(
    name: &str,
    instruction_epoch: u64,
    revision: Option<&str>,
    value: Option<&str>,
) -> String {
    format!(
        "[{name} delta instructionEpoch={instruction_epoch} replaces revision={}]\n{}",
        revision.unwrap_or("none"),
        normalized(value).unwrap_or("(cleared)")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn initialized() -> SessionContextState {
        let mut state = SessionContextState::default();
        state.initialize_baseline("[static]\nguide");
        state
    }

    fn input<'a>(
        current_thread_id: Option<&'a str>,
        memory: Option<&'a str>,
        wiki: Option<&'a str>,
        task_revision: u64,
        task_delta: Option<&'a str>,
    ) -> ContextAssemblyInput<'a> {
        ContextAssemblyInput {
            static_baseline: "[static]\nguide",
            project_state: "[project state]\nproject=Sample",
            project_memory: memory,
            wiki_facts: wiki,
            reference_context: Some("[reference context]\nsource hit"),
            task_revision,
            task_snapshot: "[active task state]\nfull",
            task_delta,
            replay_transcript: None,
            resolved_mentions: None,
            user_text: "do work",
            current_thread_id,
            force_full: false,
        }
    }

    #[test]
    fn cold_start_sends_full_baseline_once_and_omits_empty_reference() {
        let state = initialized();
        let mut request = input(None, Some("[project memory]\nknown"), None, 0, None);
        request.reference_context = None;
        let assembled = assemble_context(&state, request).unwrap();
        assert_eq!(assembled.mode, ContextDeliveryMode::Full);
        assert_eq!(assembled.text.matches("[static]").count(), 1);
        assert!(assembled.text.contains("[project memory]"));
        assert!(!assembled.text.contains("[reference context]"));
        assert!(assembled.text.contains("[active task state]"));
        assert!(assembled.text.ends_with("[user message]\ndo work"));
    }

    #[test]
    fn unchanged_follow_up_omits_static_memory_wiki_and_task_state() {
        let mut state = initialized();
        let first = assemble_context(
            &state,
            input(
                Some("thread-1"),
                Some("[project memory]\nknown"),
                Some("[wiki facts]\nknown"),
                4,
                None,
            ),
        )
        .unwrap();
        state.delivered = first.cursor;
        let follow = assemble_context(
            &state,
            input(
                Some("thread-1"),
                Some("[project memory]\nknown"),
                Some("[wiki facts]\nknown"),
                4,
                None,
            ),
        )
        .unwrap();
        assert_eq!(follow.mode, ContextDeliveryMode::Delta);
        assert!(!follow.text.contains("[static]"));
        assert!(!follow.text.contains("project memory"));
        assert!(!follow.text.contains("wiki facts"));
        assert!(!follow.text.contains("active task state"));
        assert!(follow.text.contains("[project state]"));
    }

    #[test]
    fn changed_memory_wiki_and_task_revision_send_one_replacement_delta() {
        let mut state = initialized();
        let first = assemble_context(
            &state,
            input(
                Some("thread-1"),
                Some("[project memory]\nold"),
                Some("[wiki facts]\nold"),
                2,
                None,
            ),
        )
        .unwrap();
        state.delivered = first.cursor;
        let follow = assemble_context(
            &state,
            input(
                Some("thread-1"),
                Some("[project memory]\nnew"),
                Some("[wiki facts]\nnew"),
                3,
                Some("[active task state delivery=delta]\nchange"),
            ),
        )
        .unwrap();
        assert_eq!(follow.text.matches("project memory delta").count(), 1);
        assert_eq!(follow.text.matches("wiki facts delta").count(), 1);
        assert_eq!(
            follow
                .text
                .matches("active task state delivery=delta")
                .count(),
            1
        );
    }

    #[test]
    fn failed_or_cancelled_turn_does_not_advance_cursor() {
        let state = initialized();
        let assembled = assemble_context(
            &state,
            input(Some("thread-1"), Some("memory"), None, 1, None),
        )
        .unwrap();
        assert_ne!(state.delivered, assembled.cursor);
        let retried = assemble_context(
            &state,
            input(Some("thread-1"), Some("memory"), None, 1, None),
        )
        .unwrap();
        assert_eq!(retried.mode, ContextDeliveryMode::Full);
    }

    #[test]
    fn compact_rewind_or_fallback_reset_increments_epoch_and_forces_full() {
        let mut state = initialized();
        let first = assemble_context(&state, input(Some("thread-1"), None, None, 0, None)).unwrap();
        state.delivered = first.cursor;
        let prior_epoch = state.instruction_epoch;
        state.reset_epoch("[static]\nguide");
        assert_eq!(state.instruction_epoch, prior_epoch + 1);
        let after = assemble_context(&state, input(Some("thread-1"), None, None, 0, None)).unwrap();
        assert_eq!(after.mode, ContextDeliveryMode::Full);
    }

    #[test]
    fn changed_static_fingerprint_requires_fresh_epoch_cutover() {
        let state = initialized();
        let mut request = input(Some("thread-1"), None, None, 0, None);
        request.static_baseline = "[static]\nchanged";
        assert_eq!(
            assemble_context(&state, request).unwrap_err(),
            "static_prompt_fingerprint_changed"
        );
    }

    #[test]
    fn legacy_thread_can_be_adopted_without_duplicate_baseline() {
        let mut state = SessionContextState::default();
        state.adopt_legacy_thread(
            "[static]\nguide",
            "thread-1".to_string(),
            Some("memory"),
            None,
            0,
        );
        let assembled = assemble_context(
            &state,
            input(Some("thread-1"), Some("memory"), None, 0, None),
        )
        .unwrap();
        assert_eq!(assembled.mode, ContextDeliveryMode::Delta);
        assert!(!assembled.text.contains("[static]"));
    }
}
