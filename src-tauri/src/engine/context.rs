use super::{
    condense_transcript, project_memory_section, project_state_section, prompts,
    reference_context_section, wiki_facts_section, AgentEngine, AgentEngineError, EventSink,
};
use crate::provider_runtime::RuntimeExecutor;

impl<R: RuntimeExecutor, S: EventSink> AgentEngine<R, S> {
    pub(super) fn session_prompt_baseline(&self) -> String {
        match self.worker_origin {
            Some(_) => prompts::worker_prompt_baseline(self.runtime.worker_profile()),
            None => prompts::static_prompt_baseline(self.provider_binding.provider),
        }
    }

    pub(super) async fn prepare_eps_context(
        &mut self,
        user_text: &str,
        resolved_mentions: Option<&str>,
        replay_transcript: Option<&str>,
        mut force_full: bool,
    ) -> Result<String, AgentEngineError> {
        let static_baseline = self.session_prompt_baseline();
        let luck_defense_offered = !crate::skills::is_hidden(crate::skills::LUCK_DEFENSE);
        let worker_skill = self
            .worker_origin
            .is_some()
            .then(|| prompts::selected_worker_skill(user_text))
            .flatten()
            .filter(|_| luck_defense_offered);
        let project_state = project_state_section(&self.config.project_state_for_prompt());
        let mut project_map = self.config.project_map_for_prompt();
        let memory = self
            .config
            .project_memory_for_prompt()
            .and_then(|memory| project_memory_section(Some(&memory)));
        let wiki = self
            .config
            .wiki_section_for_prompt(user_text)
            .and_then(|wiki| wiki_facts_section(Some(&wiki)));
        let mut record = self
            .session_store
            .load(&self.session_id)
            .map_err(|error| AgentEngineError::new(error.to_string()))?;
        let legacy_luck_defense_marker = prompts::selected_luck_defense_skill(user_text).is_some();
        let durable_workflow = (record
            .task_state
            .explicit_skill_selection(crate::skills::LUCK_DEFENSE)
            .map_err(AgentEngineError::new)?
            .is_some()
            || record.task_state.projection.workflow.mode.is_some())
        .then_some(&record.task_state.projection.workflow);
        let luck_defense_guidance = (self.worker_origin.is_none() && luck_defense_offered)
            .then(|| {
                prompts::selected_luck_defense_guidance(
                    durable_workflow,
                    legacy_luck_defense_marker,
                )
            })
            .flatten();
        let main_skills = match self.worker_origin {
            Some(_) => None,
            None => {
                let mut selections = std::collections::BTreeMap::new();
                for skill in crate::skills::registry() {
                    if let Some(enabled) = record
                        .task_state
                        .explicit_skill_selection(skill.id)
                        .map_err(AgentEngineError::new)?
                    {
                        selections.insert(skill.id, enabled);
                    }
                }
                prompts::selected_active_skills(luck_defense_guidance.as_deref(), |skill| {
                    selections.get(skill).copied()
                })
            }
        };
        let active_skill = worker_skill.or(main_skills.as_deref());
        if record.context_state.static_prompt_fingerprint.is_empty() {
            self.session_store
                .initialize_context_state(
                    &self.session_id,
                    &static_baseline,
                    memory.as_deref(),
                    wiki.as_deref(),
                )
                .map_err(|error| AgentEngineError::new(error.to_string()))?;
            record = self
                .session_store
                .load(&self.session_id)
                .map_err(|error| AgentEngineError::new(error.to_string()))?;
        }

        let mut replay = replay_transcript.map(str::to_string);
        if !record.context_state.baseline_matches(&static_baseline) {
            self.executor
                .reset()
                .await
                .map_err(|error| AgentEngineError::new(error.to_string()))?;
            self.thread_active = false;
            self.pending_resume_transcript = None;
            if replay.is_none() {
                let transcript = condense_transcript(&record.panel_log);
                replay = (!transcript.is_empty()).then_some(transcript);
            }
            self.session_store
                .reset_context_epoch(&self.session_id, &static_baseline, true)
                .map_err(|error| AgentEngineError::new(error.to_string()))?;
            record = self
                .session_store
                .load(&self.session_id)
                .map_err(|error| AgentEngineError::new(error.to_string()))?;
            force_full = true;
        }

        if self.worker_origin.is_some()
            && replay.as_deref().is_some_and(|text| {
                text.strip_prefix("[prior conversation]\nuser: ") == Some(user_text.trim())
            })
        {
            replay = None;
        }

        let current_conversation_key = if self.thread_active {
            self.executor.conversation_state().conversation_key()
        } else {
            None
        };
        let task_snapshot = record
            .task_state
            .render_full(record.context_state.instruction_epoch)
            .map_err(AgentEngineError::new)?;
        let task_delta = record
            .task_state
            .render_delta(
                record.context_state.instruction_epoch,
                record.context_state.delivered.task_revision,
            )
            .map_err(AgentEngineError::new)?;
        if self.worker_origin.is_none() && self.runtime.project_root().is_ok() {
            let checkpoint = (|| -> anyhow::Result<Option<String>> {
                let workspace = self
                    .runtime
                    .workspace_manager()
                    .prepare_session_current(&self.session_id)
                    .map_err(anyhow::Error::msg)?;
                let revision = self
                    .runtime
                    .current_project_revision()
                    .map_err(anyhow::Error::msg)?;
                crate::work_checkpoint::refresh_session(&workspace.root, &workspace.id, &record)?;
                crate::work_checkpoint::resume_context(
                    &workspace.root,
                    &workspace.id,
                    &self.session_id,
                    &revision,
                )
            })();
            match checkpoint {
                Ok(Some(note)) => {
                    let section = project_map.get_or_insert_with(String::new);
                    section.push_str("\n\n");
                    section.push_str(&note);
                }
                Ok(None) => {}
                Err(error) => eprintln!("eud-agent: project work checkpoint read failed: {error}"),
            }
        }
        let reference_context = (!self.config.rag_hits.is_empty())
            .then(|| reference_context_section(&self.config.rag_hits));
        let selected_guidance =
            if self.worker_origin.is_none() || self.runtime.worker_profile().can_write() {
                prompts::selected_task_guidance(&[
                    user_text,
                    &project_state,
                    project_map.as_deref().unwrap_or(""),
                    memory.as_deref().unwrap_or(""),
                    &task_snapshot,
                    resolved_mentions.unwrap_or(""),
                    replay.as_deref().unwrap_or(""),
                ])
            } else {
                None
            };
        let mut assembly = crate::context_state::assemble_context(
            &record.context_state,
            crate::context_state::ContextAssemblyInput {
                task_guidance: selected_guidance.as_deref(),
                static_baseline: &static_baseline,
                project_state: &project_state,
                project_memory: memory.as_deref(),
                wiki_facts: wiki.as_deref(),
                project_map: project_map.as_deref(),
                reference_context: reference_context.as_deref(),
                task_revision: record.task_state.projection.revision,
                task_snapshot: &task_snapshot,
                task_delta: task_delta.as_deref(),
                replay_transcript: replay.as_deref(),
                resolved_mentions,
                active_skill,
                user_text,
                provider: record.provider_binding.provider,
                current_conversation_key: current_conversation_key.as_deref(),
                force_full: force_full || !self.thread_active,
            },
        )
        .map_err(AgentEngineError::new)?;
        if let Some((note, shown)) = crate::worker::report_note(&record.workers, assembly.mode) {
            assembly.text.push_str("\n\n");
            assembly.text.push_str(&note);
            self.shown_worker_reports.extend(shown);
        }
        eprintln!(
            "eud-agent: context session={} epoch={} task_revision={} delivery={} bytes={}",
            self.session_id,
            assembly.cursor.epoch,
            assembly.cursor.task_revision,
            match assembly.mode {
                crate::context_state::ContextDeliveryMode::Full => "full",
                crate::context_state::ContextDeliveryMode::Delta => "delta",
            },
            assembly.text.len()
        );
        self.pending_context_delivery = Some(assembly.cursor);
        Ok(assembly.text)
    }
}
