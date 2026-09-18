use crate::{
    provider::ProviderConversationState, provider_runtime::IterationBoundaryReason,
    tools::BuildProgress,
};
use serde::{Deserialize, Serialize};

pub const AUTONOMOUS_STATE_SCHEMA_VERSION: u32 = 1;
pub const MAX_RECENT_PROGRESS_FINGERPRINTS: usize = 16;

/// Explicit opt-in execution mode selected for a user turn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    #[default]
    Interactive,
    Autonomous,
}

/// Explicit run-level budget policy. Iteration and no-progress limits are deliberately not
/// included: an unset budget means the run continues until completion, review, ASK, or user stop.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutonomousRunPolicy {
    #[serde(default)]
    pub max_wall_time_millis: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_observed_tokens: Option<u64>,
}

impl AutonomousRunPolicy {
    pub fn validate(&self) -> Result<(), String> {
        if self.max_wall_time_millis == Some(0) {
            return Err("장시간 작업의 최대 실행 시간은 0보다 커야 합니다.".to_string());
        }
        if self.max_observed_tokens == Some(0) {
            return Err("장시간 작업의 관찰 토큰 한도는 0보다 커야 합니다.".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutonomousRunStatus {
    Running,
    Pausing,
    Paused,
    PausedAfterRestart,
    WaitingInput,
    Review,
    SafetyStopped,
    Cancelled,
    Failed,
    Completed,
}

impl AutonomousRunStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::SafetyStopped | Self::Cancelled | Self::Failed | Self::Completed
        )
    }

    pub fn can_resume(self) -> bool {
        matches!(self, Self::Paused | Self::PausedAfterRestart)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutonomousPauseReason {
    User,
    Restart,
    WaitingInput,
    Review,
    /// An `ask` expired and the turn ended with the question as plain text;
    /// the user's reply arrives as an ordinary message.
    UnansweredAsk,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutonomousBuildStatus {
    pub input_revision: String,
    pub diagnostics_fingerprint: String,
    pub error_count: usize,
    pub success: bool,
    pub consecutive_no_progress: u32,
}

impl From<BuildProgress> for AutonomousBuildStatus {
    fn from(value: BuildProgress) -> Self {
        Self {
            input_revision: value.input_revision,
            diagnostics_fingerprint: value.diagnostics_fingerprint,
            error_count: value.error_count,
            success: value.success,
            consecutive_no_progress: value.consecutive_no_progress,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutonomousRunProgress {
    pub elapsed_active_millis: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_tokens: Option<u64>,
    pub read_actions: u64,
    pub write_actions: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_build: Option<AutonomousBuildStatus>,
    pub consecutive_no_progress: u32,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_fingerprints: Vec<String>,
}
impl From<AutonomousBuildStatus> for BuildProgress {
    fn from(value: AutonomousBuildStatus) -> Self {
        Self {
            input_revision: value.input_revision,
            diagnostics_fingerprint: value.diagnostics_fingerprint,
            error_count: value.error_count,
            success: value.success,
            consecutive_no_progress: value.consecutive_no_progress,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutonomousRunState {
    pub schema_version: u32,
    pub id: String,
    pub status: AutonomousRunStatus,
    pub started_at: u64,
    pub updated_at: u64,
    pub iteration: u64,
    pub goal: String,
    pub request_id: String,
    pub project_id: String,
    pub client_turn_id: String,
    pub project_revision: String,
    pub policy: AutonomousRunPolicy,
    pub progress: AutonomousRunProgress,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_started_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_checkpoint: Option<ProviderConversationState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boundary_reason: Option<IterationBoundaryReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pause_reason: Option<AutonomousPauseReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocker: Option<String>,
}

impl AutonomousRunState {
    pub fn new(
        goal: String,
        client_turn_id: String,
        request_id: String,
        project_id: String,
        project_revision: String,
        policy: AutonomousRunPolicy,
        now: u64,
    ) -> Self {
        Self {
            schema_version: AUTONOMOUS_STATE_SCHEMA_VERSION,
            id: format!("auto-{}", uuid::Uuid::new_v4().simple()),
            status: AutonomousRunStatus::Running,
            started_at: now,
            updated_at: now,
            client_turn_id,
            iteration: 1,
            goal,
            request_id,
            project_id,
            project_revision,
            policy,
            progress: AutonomousRunProgress::default(),
            last_checkpoint: None,
            boundary_reason: None,
            pause_reason: None,
            active_started_at: Some(now),
            blocker: None,
        }
    }

    pub fn record_progress_fingerprint(&mut self, fingerprint: String) -> bool {
        let progressed = self.progress.recent_fingerprints.last() != Some(&fingerprint);
        if progressed {
            self.progress.consecutive_no_progress = 0;
        } else {
            self.progress.consecutive_no_progress =
                self.progress.consecutive_no_progress.saturating_add(1);
        }
        self.progress.recent_fingerprints.push(fingerprint);
        let extra = self
            .progress
            .recent_fingerprints
            .len()
            .saturating_sub(MAX_RECENT_PROGRESS_FINGERPRINTS);
        if extra > 0 {
            self.progress.recent_fingerprints.drain(0..extra);
        }
        progressed
    }

    pub fn run_limit_reason(&self) -> Option<String> {
        if self
            .policy
            .max_wall_time_millis
            .is_some_and(|limit| self.progress.elapsed_active_millis >= limit)
        {
            return Some("설정한 최대 실행 시간에 도달했습니다.".to_string());
        }
        if let (Some(observed), Some(limit)) = (
            self.progress.observed_tokens,
            self.policy.max_observed_tokens,
        ) {
            if observed >= limit {
                return Some("공급자가 보고한 토큰 사용량이 설정 한도에 도달했습니다.".to_string());
            }
        }
        None
    }
}

/// Common controller outcome, independent of provider transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutonomousRunOutcome {
    Completed,
    IterationBoundary(IterationBoundaryReason),
    WriteTransition,
    WaitingInput,
    Review,
    SafetyStopped(String),
    Cancelled,
    Failed(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_with_policy(policy: AutonomousRunPolicy) -> AutonomousRunState {
        AutonomousRunState::new(
            "goal".to_string(),
            "turn".to_string(),
            "req".to_string(),
            "project".to_string(),
            "rev".to_string(),
            policy,
            0,
        )
    }

    #[test]
    fn default_policy_has_no_wall_time_or_token_limit() {
        let policy = AutonomousRunPolicy::default();
        assert_eq!(policy.max_wall_time_millis, None);
        assert_eq!(policy.max_observed_tokens, None);
        policy.validate().unwrap();
        let parsed: AutonomousRunPolicy = serde_json::from_str("{}").unwrap();
        assert_eq!(parsed, policy);
    }

    #[test]
    fn repeated_fingerprints_are_tracked_but_never_stop_the_run() {
        let mut run = run_with_policy(AutonomousRunPolicy::default());
        run.progress.elapsed_active_millis = 24 * 60 * 60 * 1_000;
        for _ in 0..MAX_RECENT_PROGRESS_FINGERPRINTS * 2 {
            run.record_progress_fingerprint("same".to_string());
        }
        assert!(run.progress.consecutive_no_progress >= MAX_RECENT_PROGRESS_FINGERPRINTS as u32);
        assert_eq!(
            run.progress.recent_fingerprints.len(),
            MAX_RECENT_PROGRESS_FINGERPRINTS
        );
        assert_eq!(run.run_limit_reason(), None);
    }

    #[test]
    fn explicit_budgets_still_stop_the_run() {
        let mut run = run_with_policy(AutonomousRunPolicy {
            max_wall_time_millis: Some(1_000),
            max_observed_tokens: Some(10),
        });
        assert_eq!(run.run_limit_reason(), None);
        run.progress.elapsed_active_millis = 1_000;
        assert!(run.run_limit_reason().unwrap().contains("최대 실행 시간"));
        run.progress.elapsed_active_millis = 0;
        run.progress.observed_tokens = Some(10);
        assert!(run.run_limit_reason().unwrap().contains("토큰"));
    }
}
