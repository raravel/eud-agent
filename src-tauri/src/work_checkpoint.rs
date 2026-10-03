use std::{fs, io::Read, path::Path};

use serde::{Deserialize, Serialize};

use crate::{session::SessionRecord, task_state::ActiveTaskProjection, worker::WorkerRecord};

const MAX_BYTES: u64 = 512 * 1024;
const MAX_WORKERS: usize = 64;
const WORK_CHECKPOINT_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct WorkCheckpoint {
    schema_version: u32,
    workspace_id: String,
    session_id: String,
    updated_at: u64,
    project_revision: String,
    compilation_stale: bool,
    task: ActiveTaskProjection,
    request_goal: Option<String>,
    requests: Vec<String>,
    workers: Vec<WorkerRecord>,
    plan: Option<String>,
    build: Option<crate::tool_exec::BuildEvidence>,
}

impl WorkCheckpoint {
    pub(crate) fn capture(
        workspace_id: &str,
        record: &SessionRecord,
        project_revision: String,
        request_goal: Option<&str>,
        plan: Option<&str>,
        build: Option<crate::tool_exec::BuildEvidence>,
    ) -> Self {
        let mut workers = record.workers.clone();
        workers.sort_by_key(|worker| {
            (
                !worker.status.is_active(),
                std::cmp::Reverse(worker.updated_at),
            )
        });
        workers.truncate(MAX_WORKERS);
        Self {
            schema_version: WORK_CHECKPOINT_SCHEMA_VERSION,
            workspace_id: workspace_id.into(),
            session_id: record.meta.id.clone(),
            updated_at: record.meta.last_conversation_at.max(
                record
                    .workers
                    .iter()
                    .map(|worker| worker.updated_at)
                    .max()
                    .unwrap_or(0),
            ),
            project_revision,
            compilation_stale: record.task_state.compilation_stale,
            task: record.task_state.projection.clone(),
            request_goal: request_goal.map(|text| {
                let mut excerpt = text.chars().take(4096).collect::<String>();
                if text.chars().count() > 4096 {
                    excerpt.push_str("\n[request excerpt; full text remains in session history]");
                }
                excerpt
            }),
            requests: Vec::new(),
            workers,
            plan: plan.map(str::to_owned),
            build,
        }
    }

    pub(crate) fn save(mut self, root: &Path) -> anyhow::Result<()> {
        self.schema_version = WORK_CHECKPOINT_SCHEMA_VERSION;
        validate_id(&self.session_id)?;
        let state_path = root
            .join(".eud-agent/state/tasks")
            .join(format!("{}.json", self.session_id));
        if state_path.try_exists()? {
            ensure_plain_chain(root, &root.join(".eud-agent/state/tasks"))?;
            let previous = load(&state_path, &self.workspace_id)?;
            self.plan = self.plan.or(previous.plan);
            self.request_goal = self.request_goal.or(previous.request_goal);
            self.requests = previous.requests;
            if self.project_revision == previous.project_revision {
                self.build = self.build.or(previous.build);
            }
        }
        if let Some(request) = &self.request_goal {
            if self.requests.last() != Some(request) {
                self.requests.push(request.clone());
            }
        }
        if self.requests.len() > 8 {
            self.requests = self.requests.split_off(self.requests.len() - 8);
        }
        let bytes = serde_json::to_vec_pretty(&self)?;
        anyhow::ensure!(
            bytes.len() as u64 <= MAX_BYTES,
            "work checkpoint exceeds size limit"
        );
        let documents = root
            .join(".eud-agent/workspace/tasks")
            .join(&self.session_id);
        crate::memory::write_atomic_bytes(
            &documents.join("plan.md"),
            self.plan_document().as_bytes(),
        )?;
        let evidence = serde_json::to_vec_pretty(&serde_json::json!({
            "projectRevision": self.project_revision,
            "build": self.build,
            "workers": self.workers,
            "verification": "requires acceptance checks; worker settlement and build success are not gameplay verification"
        }))?;
        crate::memory::write_atomic_bytes(&documents.join("evidence.json"), &evidence)?;
        crate::memory::write_atomic_bytes(&state_path, &bytes)
    }

    fn plan_document(&self) -> String {
        let mut text = format!(
            "# Work checkpoint\n\nSession: {}\nProject revision: {}\nTask revision: {}\n\nGenerated from session state. This records progress, not permission or verified completion.\n",
            self.session_id, self.project_revision, self.task.revision
        );
        if let Some(goal) = &self.request_goal {
            text.push_str(&format!("\n## Latest request\n\n{goal}\n"));
        }
        text.push_str("\n## Recent user requests\n\n");
        for request in &self.requests {
            text.push_str(request);
            text.push_str("\n\n");
        }
        for (label, facts) in [
            ("Goals", &self.task.goals),
            ("Constraints", &self.task.constraints),
            ("Decisions", &self.task.decisions),
            ("Blockers", &self.task.blockers),
            ("Acceptance criteria", &self.task.acceptance_criteria),
        ] {
            text.push_str(&format!("\n## {label}\n\n"));
            for fact in facts {
                text.push_str(&format!("- [{:?}] {}\n", fact.status, fact.text));
            }
        }
        if let Some(plan) = &self.plan {
            text.push_str(&format!("\n## Recorded plan\n\n{plan}\n"));
        }
        text.push_str("\n## Workers\n\nSuccessful results wait for active peers with the same request id. Failed results need recovery.\n\n");
        for worker in &self.workers {
            text.push_str(&format!(
                "- {}: {} / {} / request={} / owner={}\n",
                worker.id,
                worker.title,
                worker.status.label(),
                worker.parent_request_id,
                worker.child_session_id
            ));
        }
        text
    }

    fn resume_note(&self, current_revision: &str) -> String {
        let active = self
            .workers
            .iter()
            .filter(|worker| worker.status.is_active())
            .count();
        let ready = self
            .workers
            .iter()
            .filter(|worker| worker.status.continues_interactively() && !worker.reported)
            .count();
        let workflow = &self.task.workflow;
        let workflow_detail = match (workflow.mode, workflow.phase) {
            (Some(mode), Some(phase)) => format!(
                " workflowMode={mode:?} workflowPhase={phase:?} lastCompleted={:?} prerequisites=[{}] nextAction={} pendingPlaytests=[{}] workflowEvidence={}",
                workflow.last_completed,
                workflow.prerequisites.join(" | "),
                workflow.next_action,
                workflow.pending_playtests.join(" | "),
                if workflow.completed_evidence.is_empty() {
                    "none"
                } else if self.project_revision == current_revision {
                    "same revision; still advisory"
                } else {
                    "stale; revalidate"
                }
            ),
            _ => String::new(),
        };
        format!(
            "session={} projectRevision={} evidence={} activeWorkers={} unreportedResults={} semanticState={}{}\nRead .eud-agent/workspace/tasks/{}/plan.md and evidence.json for details. Next: {}.\n",
            self.session_id,
            self.project_revision,
            if self.project_revision == current_revision {
                "same revision; check acceptance separately"
            } else {
                "stale; revalidate before reuse"
            },
            active,
            ready,
            if self.compilation_stale {
                "stale"
            } else {
                "recorded"
            },
            workflow_detail,
            self.session_id,
            if active > 0 {
                "wait for the request batch or handle a failure"
            } else {
                "review evidence and remaining acceptance criteria"
            }
        )
    }
}

pub(crate) fn refresh_session(
    root: &Path,
    workspace_id: &str,
    record: &SessionRecord,
) -> anyhow::Result<()> {
    validate_id(&record.meta.id)?;
    let directory = root.join(".eud-agent/state/tasks");
    let path = directory.join(format!("{}.json", record.meta.id));
    if !path.try_exists()? {
        return Ok(());
    }
    ensure_plain_chain(root, &directory)?;
    let mut saved = load(&path, workspace_id)?;
    let fresh = WorkCheckpoint::capture(
        workspace_id,
        record,
        saved.project_revision.clone(),
        None,
        None,
        None,
    );
    if saved.workers != fresh.workers
        || saved.task != fresh.task
        || saved.compilation_stale != fresh.compilation_stale
    {
        saved.workers = fresh.workers;
        saved.task = fresh.task;
        saved.compilation_stale = fresh.compilation_stale;
        saved.updated_at = fresh.updated_at;
        saved.save(root)?;
    }
    Ok(())
}

pub(crate) fn resume_context(
    root: &Path,
    workspace_id: &str,
    session_id: &str,
    revision: &str,
) -> anyhow::Result<Option<String>> {
    let directory = root.join(".eud-agent/state/tasks");
    if !directory.try_exists()? {
        return Ok(None);
    }
    ensure_plain_chain(root, &directory)?;
    validate_id(session_id)?;
    let own = directory.join(format!("{session_id}.json"));
    let mut checkpoints = Vec::new();
    if own.try_exists()? {
        checkpoints.push(load(&own, workspace_id)?);
    } else {
        for entry in fs::read_dir(&directory)?.take(128) {
            let path = entry?.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                match load(&path, workspace_id) {
                    Ok(checkpoint) => checkpoints.push(checkpoint),
                    Err(error) => eprintln!("eud-agent: ignored invalid work checkpoint: {error}"),
                }
            }
        }
        checkpoints.sort_by_key(|checkpoint| std::cmp::Reverse(checkpoint.updated_at));
        checkpoints.truncate(3);
    }
    if checkpoints.is_empty() {
        return Ok(None);
    }
    Ok(Some(format!(
        "[project work checkpoint]\nSaved progress is advisory; current user instructions and session task state take precedence. Do not restart workers or claim verified completion from this record. Read only the evidence entries needed for the next step.\n{}",
        checkpoints
            .iter()
            .map(|checkpoint| checkpoint.resume_note(revision))
            .collect::<String>()
    )))
}

fn load(path: &Path, workspace_id: &str) -> anyhow::Result<WorkCheckpoint> {
    let metadata = fs::symlink_metadata(path)?;
    anyhow::ensure!(
        metadata.is_file()
            && !metadata.file_type().is_symlink()
            && !crate::memory::is_reparse_point(&metadata),
        "checkpoint is not a regular file"
    );
    anyhow::ensure!(
        metadata.len() <= MAX_BYTES,
        "work checkpoint exceeds size limit"
    );
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_BYTES,
        "work checkpoint exceeds size limit"
    );
    let mut checkpoint: WorkCheckpoint = serde_json::from_slice(&bytes)?;
    validate_id(&checkpoint.session_id)?;
    anyhow::ensure!(
        (1..=WORK_CHECKPOINT_SCHEMA_VERSION).contains(&checkpoint.schema_version)
            && checkpoint.workspace_id == workspace_id,
        "work checkpoint belongs to another workspace or schema"
    );
    anyhow::ensure!(
        path.file_stem().and_then(|stem| stem.to_str()) == Some(&checkpoint.session_id),
        "checkpoint session identity mismatch"
    );
    anyhow::ensure!(
        checkpoint.workers.len() <= MAX_WORKERS,
        "too many checkpoint workers"
    );
    checkpoint.schema_version = WORK_CHECKPOINT_SCHEMA_VERSION;
    Ok(checkpoint)
}

fn ensure_plain_chain(root: &Path, target: &Path) -> anyhow::Result<()> {
    let mut path = root.to_path_buf();
    for component in target.strip_prefix(root)?.components() {
        path.push(component);
        let metadata = fs::symlink_metadata(&path)?;
        anyhow::ensure!(
            metadata.is_dir()
                && !metadata.file_type().is_symlink()
                && !crate::memory::is_reparse_point(&metadata),
            "unsafe checkpoint directory"
        );
    }
    Ok(())
}

fn validate_id(id: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !id.is_empty()
            && id.len() <= 128
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')),
        "invalid checkpoint session id"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker::{WorkerRecord, WorkerStatus};

    fn fixture() -> (std::path::PathBuf, WorkCheckpoint) {
        let root = std::env::temp_dir().join(format!("work-checkpoint-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let mut worker = WorkerRecord::new(
            "child".into(),
            "request".into(),
            "source change".into(),
            "implement source".into(),
        );
        worker.status = WorkerStatus::Settled;
        worker.summary = Some("observed implementation evidence".into());
        worker.changed_files = vec!["src/main.eps".into()];
        (
            root,
            WorkCheckpoint {
                schema_version: 1,
                workspace_id: "workspace".into(),
                session_id: "session".into(),
                updated_at: 1,
                project_revision: "revision-one".into(),
                compilation_stale: false,
                task: Default::default(),
                request_goal: Some("fixed races and separated lanes".into()),
                requests: Vec::new(),
                workers: vec![worker],
                plan: Some("1. Set races\n2. Verify actual gameplay".into()),
                build: Some(crate::tool_exec::BuildEvidence {
                    ok: true,
                    error_count: 0,
                }),
            },
        )
    }

    #[test]
    fn checkpoint_survives_move_and_new_session_without_claiming_completion() {
        let (root, checkpoint) = fixture();
        checkpoint.save(&root).unwrap();
        let moved = root.with_extension("moved");
        fs::rename(&root, &moved).unwrap();
        let note = resume_context(&moved, "workspace", "fresh-session", "revision-one")
            .unwrap()
            .unwrap();
        assert!(note.contains("tasks/session/plan.md"));
        assert!(note.contains("review evidence and remaining acceptance criteria"));
        assert!(!note.contains("observed implementation evidence"));
        let document =
            fs::read_to_string(moved.join(".eud-agent/workspace/tasks/session/plan.md")).unwrap();
        assert!(document.contains("fixed races and separated lanes"));
        let evidence =
            fs::read_to_string(moved.join(".eud-agent/workspace/tasks/session/evidence.json"))
                .unwrap();
        assert!(evidence.contains("src/main.eps"));
        fs::remove_dir_all(moved).unwrap();
    }

    #[test]
    fn checkpoint_schema_one_loads_and_next_save_writes_schema_two() {
        let (root, mut checkpoint) = fixture();
        checkpoint.schema_version = 1;
        checkpoint.task.workflow = crate::task_state::ScopedWorkflowState {
            skill_enabled: true,
            mode: Some(crate::task_state::WorkflowMode::ContinueExisting),
            phase: Some(crate::task_state::WorkflowPhase::CoreLoop),
            last_completed: Some(crate::task_state::WorkflowPhase::Scope),
            completed_evidence: vec![
                crate::task_state::WorkflowCompletedEvidence::SuccessfulBuild {
                    project_revision: "revision-one".to_string(),
                    request_id: "request".to_string(),
                },
            ],
            prerequisites: vec!["Confirm the summon roster".to_string()],
            next_action: "Implement movement and combat".to_string(),
            pending_playtests: vec!["Verify the closed route in game".to_string()],
            provenance: Some(crate::task_state::WorkflowProvenance::local()),
        };
        let directory = root.join(".eud-agent/state/tasks");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("session.json");
        fs::write(&path, serde_json::to_vec_pretty(&checkpoint).unwrap()).unwrap();

        let loaded = load(&path, "workspace").unwrap();
        assert_eq!(loaded.schema_version, WORK_CHECKPOINT_SCHEMA_VERSION);
        let note = loaded.resume_note("revision-two");
        assert!(note.contains("Confirm the summon roster"));
        assert!(note.contains("Implement movement and combat"));
        assert!(note.contains("Verify the closed route in game"));
        assert!(note.contains("stale; revalidate"));
        loaded.save(&root).unwrap();
        let mut saved: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["schemaVersion"], serde_json::json!(2));
        println!(
            "persistedCheckpoint={}",
            serde_json::to_string(&saved).unwrap()
        );
        saved["schemaVersion"] = serde_json::json!(WORK_CHECKPOINT_SCHEMA_VERSION + 1);
        fs::write(&path, serde_json::to_vec_pretty(&saved).unwrap()).unwrap();
        assert!(load(&path, "workspace").is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn evidence_and_recorded_plan_are_retained_only_at_their_valid_revision() {
        let (root, checkpoint) = fixture();
        checkpoint.clone().save(&root).unwrap();
        let mut next = checkpoint;
        next.plan = None;
        next.build = None;
        next.clone().save(&root).unwrap();
        let path = root.join(".eud-agent/state/tasks/session.json");
        assert!(load(&path, "workspace").unwrap().build.unwrap().ok);
        next.project_revision = "revision-two".into();
        next.save(&root).unwrap();
        let saved = load(&path, "workspace").unwrap();
        assert!(saved.build.is_none());
        assert!(saved.plan.unwrap().contains("Verify actual gameplay"));
        let note = resume_context(&root, "workspace", "session", "revision-three")
            .unwrap()
            .unwrap();
        assert!(note.contains("stale; revalidate"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_workspace_identity_and_oversized_files_do_not_resume() {
        let (root, checkpoint) = fixture();
        checkpoint.save(&root).unwrap();
        assert!(resume_context(&root, "other-workspace", "session", "revision-one").is_err());
        assert!(resume_context(&root, "workspace", "../outside", "revision-one").is_err());
        let path = root.join(".eud-agent/state/tasks/session.json");
        fs::write(&path, vec![b' '; MAX_BYTES as usize + 1]).unwrap();
        assert!(resume_context(&root, "workspace", "session", "revision-one").is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
