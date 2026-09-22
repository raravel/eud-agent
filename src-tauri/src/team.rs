//! EPS → Map team handoff records.
//!
//! An EPS session hands placement work to its own team Map session with
//! `map_task_request`. The durable [`TeamTask`] lives on the EPS session record;
//! the Map session runs an ordinary candidate request. The EPS agent then
//! inspects the candidate and applies, replaces (a new request in a fresh
//! team session from the saved map), or discards it, and the user can do the same in the Map
//! window or undo an apply from either window. Every status change goes
//! through the engine's own apply/discard paths: a tool result never advances
//! a status by itself.
//!
//! See `features/subagent-delegation-and-team-handoff-plan.md` Phases 2–2b.

use serde::{Deserialize, Serialize};

use crate::map_model::MapLayer;

/// Bounded wait for one `map_task_request` call before it returns `running`
/// and the turn continues without the candidate. Shares the native MCP call
/// ceiling with `ask` and `delegate_read`.
pub const TEAM_TASK_WAIT_TIMEOUT: std::time::Duration = crate::tools::ASK_WAIT_TIMEOUT;

/// Tasks kept in the `[map tasks]` prompt note, newest first.
pub const PROMPT_TASK_LIMIT: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum TeamTaskStatus {
    /// Persisted before the Map request is submitted.
    Queued,
    /// The Map session is working on the request.
    Running,
    /// The Map session committed candidate revision `candidate.revision`; the
    /// user has not applied or discarded it yet.
    CandidateReady,
    /// The candidate was applied to the source map (`TeamTask::applied_by`
    /// says by whom).
    Applied,
    /// The candidate was discarded, its apply was undone, or it was stale
    /// after a restart.
    Discarded,
    /// A later `map_task_request` replaced this ready candidate, which was
    /// dropped with its retired team session.
    Superseded,
    Failed {
        reason: String,
    },
    Cancelled,
    /// The app restarted while the Map request was running.
    Interrupted,
}

impl TeamTaskStatus {
    /// A task that still excludes the EPS session's own map writes.
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Queued | Self::Running | Self::CandidateReady)
    }

    pub fn is_terminal(&self) -> bool {
        !self.is_active()
    }

    /// A settlement the EPS conversation should pick up on its own when the
    /// `map_task_request` that started the task had already returned
    /// `running`: a candidate to inspect, or a failure to report. A cancelled
    /// task was stopped deliberately and waits for the user instead.
    pub fn continues_interactively(&self) -> bool {
        matches!(self, Self::CandidateReady | Self::Failed { .. })
    }

    /// The stable wire label the tool result and the panel use.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::CandidateReady => "candidate_ready",
            Self::Applied => "applied",
            Self::Discarded => "discarded",
            Self::Failed { .. } => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
            Self::Superseded => "superseded",
        }
    }
}

/// Who applied a team candidate to the source map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamApplyActor {
    /// Trusted Apply in the Map window.
    User,
    /// The EPS agent's `map_task_apply` after inspecting the candidate.
    Agent,
}

/// What the Map session produced: the candidate revision the user will review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamCandidateSummary {
    pub revision: u32,
    pub revision_key: String,
    pub map_sha256: String,
    /// One-line diff summary rendered by the engine.
    pub summary: String,
    pub terrain_cells: u32,
    pub units: u32,
    pub buildings: u32,
    pub doodads: u32,
    pub sprites: u32,
    pub locations: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamTask {
    pub id: String,
    /// The EPS request that created the task.
    pub parent_request_id: String,
    pub map_session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map_request_id: Option<String>,
    pub goal: String,
    pub layers: Vec<MapLayer>,
    /// Persistent selection ids handed to the Map session as `target` scope.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selection_ids: Vec<String>,
    /// Exact location ids handed to the Map session as context.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub location_ids: Vec<u16>,
    pub source_map_sha256_at_create: String,
    pub status: TeamTaskStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate: Option<TeamCandidateSummary>,
    /// The source map hash after Apply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_source_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_by: Option<TeamApplyActor>,
    pub created_at: u64,
    pub updated_at: u64,
}

impl TeamTask {
    /// The tool result / status payload shared by `map_task_request` and
    /// `map_task_status`.
    pub fn to_tool_value(&self) -> serde_json::Value {
        let mut value = serde_json::json!({
            "taskId": self.id,
            "status": self.status.label(),
            "mapSessionId": self.map_session_id,
            "goal": self.goal,
            "layers": self.layers,
        });
        if let TeamTaskStatus::Failed { reason } = &self.status {
            value["reason"] = serde_json::Value::String(reason.clone());
        }
        if let Some(candidate) = &self.candidate {
            value["candidate"] = serde_json::json!({
                "revision": candidate.revision,
                "summary": candidate.summary,
                "terrainCells": candidate.terrain_cells,
                "objectCounts": {
                    "units": candidate.units,
                    "buildings": candidate.buildings,
                    "doodads": candidate.doodads,
                    "sprites": candidate.sprites,
                    "locations": candidate.locations,
                },
            });
        }
        if let Some(hash) = &self.applied_source_sha256 {
            value["appliedSourceSha256"] = serde_json::Value::String(hash.clone());
        }
        if let Some(actor) = self.applied_by {
            value["appliedBy"] = serde_json::json!(actor);
        }
        value["applied"] = serde_json::Value::Bool(matches!(self.status, TeamTaskStatus::Applied));
        value
    }

    /// One line for the `[map tasks]` prompt note.
    pub fn prompt_line(&self) -> String {
        let candidate = self
            .candidate
            .as_ref()
            .map(|candidate| format!(" candidate=r{} ({})", candidate.revision, candidate.summary))
            .unwrap_or_default();
        let reason = match &self.status {
            TeamTaskStatus::Failed { reason } => format!(" reason={reason}"),
            _ => String::new(),
        };
        format!(
            "- {} status={}{}{} goal={}",
            self.id,
            self.status.label(),
            candidate,
            reason,
            self.goal
        )
    }
}

/// Render the `[map tasks]` prompt note for the EPS foreground: the newest
/// tasks with what the model must do next. `None` when there are no tasks.
pub fn prompt_note(tasks: &[TeamTask]) -> Option<String> {
    if tasks.is_empty() {
        return None;
    }
    let mut ordered = tasks.iter().collect::<Vec<_>>();
    ordered.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then(b.id.cmp(&a.id)));
    let lines = ordered
        .into_iter()
        .take(PROMPT_TASK_LIMIT)
        .map(TeamTask::prompt_line)
        .collect::<Vec<_>>()
        .join("\n");
    let guidance = if tasks
        .iter()
        .any(|task| task.status == TeamTaskStatus::CandidateReady)
    {
        "A candidate_ready task is waiting for your decision and is not in the source map yet: inspect it with map_task_diff, map_task_objects, or map_task_render, then map_task_apply it when it meets the goal, send a corrected complete map_task_request to replace it (a fresh team session from the saved map, never a delta on this candidate), or map_task_discard it. Until it is applied do not call location_write, switch_write, player_setup, or map sound tools, and do not build code that references objects it creates."
    } else if tasks.iter().any(|task| task.status.is_active()) {
        "The Map Agent is still working on a task: read map_task_status once; if it is still running, end the turn and continue when the next message reports it, without calling location_write, switch_write, player_setup, or map sound tools."
    } else {
        "Applied tasks changed the source map: re-read map_info before referencing what they created. Discarded, superseded, failed, or interrupted tasks made no change."
    };
    Some(format!("[map tasks]\n{lines}\n{guidance}"))
}

/// The fixed user message an EPS session continues with after a team task
/// settled in the background; the panel's "이어서 진행" button sends the same
/// text.
pub const TEAM_TASK_CONTINUE_TEXT: &str =
    "맵 작업 결과를 확인했습니다. [map tasks] 상태를 기준으로 이어서 진행해 주세요.";

/// The `team_task` core-to-panel event: the task as the panel shows it, and,
/// when the engine starts the EPS session's continuation turn itself, the
/// user message that turn runs on so the panel records it as sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamTaskEvent {
    pub task: TeamTask,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation: Option<TeamTaskContinuation>,
}

/// The engine-started continuation turn announced with a `team_task` event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamTaskContinuation {
    pub client_turn_id: String,
    pub text: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, status: TeamTaskStatus, updated_at: u64) -> TeamTask {
        TeamTask {
            id: id.to_string(),
            parent_request_id: "req".to_string(),
            map_session_id: "map".to_string(),
            map_request_id: None,
            goal: format!("goal {id}"),
            layers: vec![MapLayer::Terrain],
            selection_ids: Vec::new(),
            location_ids: Vec::new(),
            source_map_sha256_at_create: "a".repeat(64),
            status,
            candidate: None,
            applied_source_sha256: None,
            applied_by: None,
            created_at: updated_at,
            updated_at,
        }
    }

    #[test]
    fn a_background_settlement_continues_the_conversation_only_for_a_candidate_or_failure() {
        assert!(TeamTaskStatus::CandidateReady.continues_interactively());
        assert!(TeamTaskStatus::Failed { reason: "x".into() }.continues_interactively());
        for waiting in [
            TeamTaskStatus::Queued,
            TeamTaskStatus::Running,
            TeamTaskStatus::Cancelled,
            TeamTaskStatus::Applied,
            TeamTaskStatus::Discarded,
            TeamTaskStatus::Interrupted,
            TeamTaskStatus::Superseded,
        ] {
            assert!(!waiting.continues_interactively(), "{waiting:?}");
        }
    }

    #[test]
    fn team_task_event_carries_the_engine_started_continuation_turn() {
        let plain = serde_json::to_value(TeamTaskEvent {
            task: task("t1", TeamTaskStatus::Running, 1),
            continuation: None,
        })
        .unwrap();
        assert!(plain.get("continuation").is_none());
        let continued = serde_json::to_value(TeamTaskEvent {
            task: task("t1", TeamTaskStatus::CandidateReady, 2),
            continuation: Some(TeamTaskContinuation {
                client_turn_id: "turn-1".to_string(),
                text: TEAM_TASK_CONTINUE_TEXT.to_string(),
            }),
        })
        .unwrap();
        assert_eq!(continued["continuation"]["clientTurnId"], "turn-1");
        assert_eq!(continued["continuation"]["text"], TEAM_TASK_CONTINUE_TEXT);
    }

    #[test]
    fn status_activity_and_labels_are_stable() {
        assert!(TeamTaskStatus::Queued.is_active());
        assert!(TeamTaskStatus::Running.is_active());
        assert!(TeamTaskStatus::CandidateReady.is_active());
        for terminal in [
            TeamTaskStatus::Applied,
            TeamTaskStatus::Discarded,
            TeamTaskStatus::Failed { reason: "x".into() },
            TeamTaskStatus::Cancelled,
            TeamTaskStatus::Interrupted,
            TeamTaskStatus::Superseded,
        ] {
            assert!(terminal.is_terminal(), "{terminal:?}");
        }
        assert_eq!(TeamTaskStatus::Superseded.label(), "superseded");
        assert_eq!(TeamTaskStatus::CandidateReady.label(), "candidate_ready");
        let json = serde_json::to_value(TeamTaskStatus::Failed {
            reason: "boom".into(),
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({"kind": "failed", "reason": "boom"})
        );
    }

    #[test]
    fn tool_value_carries_candidate_and_failure_reason() {
        let mut ready = task("t1", TeamTaskStatus::CandidateReady, 2);
        ready.candidate = Some(TeamCandidateSummary {
            revision: 3,
            revision_key: "r3:abc".into(),
            map_sha256: "b".repeat(64),
            summary: "지형 40칸".into(),
            terrain_cells: 40,
            units: 0,
            buildings: 0,
            doodads: 0,
            sprites: 0,
            locations: 1,
        });
        let value = ready.to_tool_value();
        assert_eq!(value["status"], "candidate_ready");
        assert_eq!(value["candidate"]["revision"], 3);
        assert_eq!(value["candidate"]["objectCounts"]["locations"], 1);
        assert_eq!(value["applied"], false);
        assert!(value.get("appliedBy").is_none());
        let mut applied = task("t3", TeamTaskStatus::Applied, 3);
        applied.applied_by = Some(TeamApplyActor::Agent);
        let applied = applied.to_tool_value();
        assert_eq!(applied["applied"], true);
        assert_eq!(applied["appliedBy"], "agent");
        let failed = task(
            "t2",
            TeamTaskStatus::Failed {
                reason: "stale".into(),
            },
            1,
        )
        .to_tool_value();
        assert_eq!(failed["reason"], "stale");
        assert!(failed.get("candidate").is_none());
    }

    #[test]
    fn prompt_note_lists_newest_first_and_warns_while_active() {
        assert!(prompt_note(&[]).is_none());
        let tasks = (0..7)
            .map(|index| task(&format!("t{index}"), TeamTaskStatus::Applied, index as u64))
            .collect::<Vec<_>>();
        let note = prompt_note(&tasks).unwrap();
        assert!(note.starts_with("[map tasks]\n- t6 status=applied"));
        assert!(!note.contains("- t1 "), "only the newest five are listed");
        assert!(note.contains("re-read map_info"));
        let ready = vec![task("a", TeamTaskStatus::CandidateReady, 9)];
        let note = prompt_note(&ready).unwrap();
        assert!(note.contains("map_task_apply"));
        assert!(note.contains("do not call location_write"));
        let running = vec![task("b", TeamTaskStatus::Running, 9)];
        let note = prompt_note(&running).unwrap();
        assert!(note.contains("still working"));
        assert!(!note.contains("map_task_apply"));
    }
}
