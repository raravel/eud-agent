//! The engine side of mid-turn steering ([`crate::steer`]): accepting a
//! message the user sent while a turn runs, and what happens to the messages
//! the turn never received.
//!
//! Accepting a steer never takes the engine mutex the running turn holds: the
//! message is resolved (attachments, mentions) against the session's tool
//! runtime and pushed into the session's inbox, which the turn drains at its
//! tool boundaries. A turn that ended first answers `NoTurn` and the panel
//! sends the message as an ordinary turn instead.

use super::{team::TeamCommand, AgentEngineError, Phase, SessionEngineManager, SessionWorker};
use crate::{
    ipc,
    steer::{SteerEvent, SteerEventKind, SteerMessage, SteerPush},
};
use std::sync::Arc;

/// The mentions a steer carries, in the form its session kind resolves.
pub(crate) enum SteerMentions {
    Eps(Vec<crate::mentions::MentionInstance>),
    /// Map mention snapshots plus their compact model form, which the Map
    /// window's command resolves against its candidate before the push.
    Map {
        snapshots: Vec<crate::map_model::MapMentionSnapshot>,
        compact: Option<serde_json::Value>,
    },
}

/// One message the user sent while a turn ran.
pub(crate) struct SteerInput {
    pub steer_id: String,
    pub text: String,
    pub attachments: Vec<String>,
    pub mentions: SteerMentions,
}

impl SessionEngineManager {
    /// Deliver `input` to the turn running in `session_id`, or answer
    /// [`SteerPush::NoTurn`] when none runs there.
    pub(crate) async fn steer(
        &self,
        session_id: &str,
        kind: crate::session::SessionKind,
        input: SteerInput,
    ) -> Result<SteerPush, AgentEngineError> {
        uuid::Uuid::parse_str(&input.steer_id)
            .map_err(|_| AgentEngineError::new("steerId must be a UUID"))?;
        let Some(worker) = self.registered_worker(session_id).await else {
            return Ok(SteerPush::NoTurn);
        };
        if worker.runtime.kind() != kind {
            return Err(AgentEngineError::new(match kind {
                crate::session::SessionKind::Eps => {
                    "Map sessions accept steering only through map_agent_chat_steer."
                }
                crate::session::SessionKind::Map => "the requested session is not a Map session",
            }));
        }
        let inbox = worker.runtime.steer_inbox();
        // The turn the steer is resolved for: its refs bind to that turn's
        // request, so a push after another turn opened is refused.
        let generation = inbox.generation();
        let Some(request_id) = worker.runtime.current_request_id() else {
            return Ok(SteerPush::NoTurn);
        };
        let message = self
            .prepare_steer(&worker, session_id, &request_id, input)
            .await?;
        Ok(inbox.push_for(generation, message))
    }

    /// Resolve a steer the way `chat` resolves a turn's message: text files
    /// inline, images as the run's images, audio bound to the running request,
    /// Map images as request image refs, mentions resolved.
    async fn prepare_steer(
        &self,
        worker: &Arc<SessionWorker>,
        session_id: &str,
        request_id: &str,
        input: SteerInput,
    ) -> Result<SteerMessage, AgentEngineError> {
        let runtime = worker.runtime.clone();
        let mut context = if input.attachments.is_empty() {
            crate::attachment::AttachmentContext {
                image_paths: Vec::new(),
                images: Vec::new(),
                text_files: Vec::new(),
                audio_files: Vec::new(),
            }
        } else {
            self.inner
                .attachments
                .bind_and_resolve(&input.attachments, session_id)
                .map_err(AgentEngineError::new)?
        };
        let audio_files = std::mem::take(&mut context.audio_files);
        let audio_refs = if audio_files.is_empty() {
            Vec::new()
        } else {
            let binder = runtime.clone();
            let request = request_id.to_string();
            tokio::task::spawn_blocking(move || {
                binder.bind_audio_attachments(&request, audio_files)
            })
            .await
            .map_err(|error| {
                AgentEngineError::new(format!("audio attachment probe task failed: {error}"))
            })?
            .map_err(AgentEngineError::new)?
        };
        let map_image_refs = if runtime.kind() == crate::session::SessionKind::Map {
            runtime
                .bind_map_images(request_id, &context.images)
                .map_err(AgentEngineError::new)?
        } else {
            Vec::new()
        };
        let has_mentions = match &input.mentions {
            SteerMentions::Eps(mentions) => !mentions.is_empty(),
            SteerMentions::Map { snapshots, .. } => !snapshots.is_empty(),
        };
        let plain = if input.text.trim().is_empty() && !input.attachments.is_empty() {
            "첨부한 파일을 분석해 주세요."
        } else if input.text.trim().is_empty() && has_mentions {
            "참조한 리소스를 바탕으로 요청을 수행해 주세요."
        } else {
            input.text.as_str()
        };
        let mut body = context.append_text_files(plain);
        let mentions = match &input.mentions {
            SteerMentions::Eps(mentions) => {
                let project = runtime.map_context().map_err(AgentEngineError::new)?;
                let resolved = runtime
                    .mentions()
                    .resolve_all(&project, mentions, runtime.project_root().ok().as_deref())
                    .map_err(AgentEngineError::new)?;
                if let Some(resolved) = resolved {
                    body.push_str("\n\n");
                    body.push_str(&resolved);
                }
                serde_json::to_value(mentions)
            }
            SteerMentions::Map { snapshots, compact } => {
                if let Some(compact) = compact {
                    body.push_str(
                        "\n\n[map mention snapshots]\nFor reference: mentions sent mid-request do not change this request's edit authority.\n",
                    );
                    body.push_str(&compact.to_string());
                }
                serde_json::to_value(snapshots)
            }
        }
        .map_err(|error| AgentEngineError::new(format!("steer mentions: {error}")))?;
        if !map_image_refs.is_empty() {
            body.push_str("\n\n[map image refs]\n");
            body.push_str(&serde_json::to_string(&map_image_refs).map_err(|error| {
                AgentEngineError::new(format!("map image references: {error}"))
            })?);
        }
        if !audio_refs.is_empty() {
            body.push_str("\n\n[audio attachments]\n");
            body.push_str(&serde_json::to_string(&audio_refs).map_err(|error| {
                AgentEngineError::new(format!("trusted audio references: {error}"))
            })?);
        }
        // An image the provider cannot take refuses this message alone, here,
        // instead of failing the running turn at its next tool boundary.
        crate::provider_transcript::conversation_images(&context.image_paths).map_err(|error| {
            AgentEngineError::new(format!("첨부 이미지를 전달할 수 없습니다: {error}"))
        })?;
        Ok(SteerMessage {
            steer_id: input.steer_id,
            text: input.text,
            attachments: input.attachments,
            mentions,
            model_text: crate::steer::frame_for_model(&body),
            image_paths: context.image_paths,
        })
    }

    /// The steers the session's last turn never received, once that turn
    /// closed its inbox (a turn still running keeps them).
    pub(crate) async fn take_unconsumed_steers(&self, session_id: &str) -> Vec<SteerMessage> {
        match self.registered_worker(session_id).await {
            Some(worker) => worker.runtime.steer_inbox().take_unconsumed(),
            None => Vec::new(),
        }
    }

    /// Tell the panels `steers` go back to the composer.
    pub(crate) async fn return_steers(&self, session_id: &str, steers: &[SteerMessage]) {
        if steers.is_empty() {
            return;
        }
        if let Some(worker) = self.registered_worker(session_id).await {
            worker
                .runtime
                .steer_inbox()
                .emit(SteerEvent::new(SteerEventKind::Returned, steers));
        }
    }

    /// Tell the panels an ordinary turn starts now with `steers`.
    pub(crate) async fn announce_steer_turn(
        &self,
        session_id: &str,
        steers: &[SteerMessage],
        client_turn_id: Option<String>,
    ) {
        if let Some(worker) = self.registered_worker(session_id).await {
            let mut event = SteerEvent::new(SteerEventKind::Turn, steers);
            event.client_turn_id = client_turn_id;
            worker.runtime.steer_inbox().emit(event);
        }
    }

    /// After an EPS command: steers its turn never received go back to the
    /// composer or start the next ordinary turn (on the dispatcher, which
    /// awaits the engine); see [`unconsumed_disposition`].
    pub(super) fn settle_unconsumed_steers(&self, worker: &Arc<SessionWorker>, cancelled: bool) {
        if worker.runtime.kind() != crate::session::SessionKind::Eps {
            return;
        }
        let inbox = worker.runtime.steer_inbox();
        let steers = inbox.take_unconsumed();
        if steers.is_empty() {
            return;
        }
        let session_id = worker.runtime.session_id().to_string();
        let disposition = unconsumed_disposition(
            cancelled,
            worker.runtime.worker_parent().is_some(),
            self.autonomous_run_active(&session_id),
        );
        if disposition == UnconsumedSteers::Return {
            inbox.emit(SteerEvent::new(SteerEventKind::Returned, &steers));
            return;
        }
        if let Err(error) = self
            .inner
            .team_dispatcher
            .send(TeamCommand::SteerFollowUp { session_id, steers })
        {
            let TeamCommand::SteerFollowUp { steers, .. } = error.0 else {
                return;
            };
            inbox.emit(SteerEvent::new(SteerEventKind::Returned, &steers));
        }
    }

    /// Whether the session has an autonomous run that is not over: a turn
    /// the user did not start must not run beside it.
    fn autonomous_run_active(&self, session_id: &str) -> bool {
        self.inner
            .sessions
            .load(session_id)
            .ok()
            .and_then(|record| record.autonomous_run)
            .is_some_and(|run| !run.status.is_terminal())
    }

    /// Start the ordinary turn for steers the previous turn never received.
    /// A session that is no longer idle (a plan awaits review, another turn
    /// took it) hands them back to the composer instead.
    pub(crate) async fn steer_follow_up(
        &self,
        session_id: &str,
        steers: Vec<SteerMessage>,
    ) -> Result<(), AgentEngineError> {
        let worker = self.worker(session_id).await?;
        let _provider_busy = self.inner.provider_service.enter_busy(worker.provider);
        let inbox = worker.runtime.steer_inbox();
        let mut engine = worker.engine.lock().await;
        if engine.phase != Phase::Idle
            || unconsumed_disposition(
                false,
                worker.runtime.worker_parent().is_some(),
                self.autonomous_run_active(session_id),
            ) == UnconsumedSteers::Return
        {
            inbox.emit(SteerEvent::new(SteerEventKind::Returned, &steers));
            return Ok(());
        }
        let client_turn_id = ipc::new_client_turn_id();
        let mut event = SteerEvent::new(SteerEventKind::Turn, &steers);
        event.client_turn_id = Some(client_turn_id.clone());
        inbox.emit(event);
        let request = follow_up_request(client_turn_id, &steers);
        if let Err(error) = self.inner.sessions.touch_conversation(session_id) {
            eprintln!("eud-agent: conversation timestamp update failed: {error}");
        }
        worker
            .runtime
            .emit_activity(crate::write_coordinator::SessionActivity::RunningRead);
        let result = engine.chat(request).await;
        drop(engine);
        self.finish_read_command(&worker, result).await
    }
}

/// What happens to steers a finished turn never received.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnconsumedSteers {
    /// Back to the composer, still the user's to send.
    Return,
    /// The next ordinary turn carries them.
    FollowUp,
}

/// A cancelled turn hands its steers back. So does a worker session: a turn
/// started there after its worker settled would be one the parent never sees,
/// cannot revert and does not cancel. And so does a session with a live
/// autonomous run, which an interactive turn must not run beside. Only an
/// ordinary EPS conversation starts the next turn with them.
pub(crate) fn unconsumed_disposition(
    cancelled: bool,
    worker_session: bool,
    autonomous_run_active: bool,
) -> UnconsumedSteers {
    if cancelled || worker_session || autonomous_run_active {
        UnconsumedSteers::Return
    } else {
        UnconsumedSteers::FollowUp
    }
}

/// The ordinary turn that carries unconsumed steers: their texts in order,
/// with every attachment and mention they had.
pub(crate) fn follow_up_request(
    client_turn_id: String,
    steers: &[SteerMessage],
) -> ipc::ChatRequest {
    ipc::ChatRequest {
        client_turn_id,
        text: steers
            .iter()
            .map(|steer| steer.text.as_str())
            .filter(|text| !text.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"),
        attachments: steers
            .iter()
            .flat_map(|steer| steer.attachments.iter().cloned())
            .collect(),
        mentions: steers
            .iter()
            .flat_map(|steer| {
                serde_json::from_value::<Vec<crate::mentions::MentionInstance>>(
                    steer.mentions.clone(),
                )
                .unwrap_or_default()
            })
            .collect(),
        execution_mode: crate::autonomous::ExecutionMode::Interactive,
        autonomous_policy: None,
        clarify: false,
        skill_selections: Default::default(),
    }
}

/// The arguments of the `chat_steer` command.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChatSteerCommand {
    pub session_id: String,
    pub steer_id: String,
    pub text: String,
    pub attachments: Vec<String>,
    pub mentions: Option<Vec<crate::mentions::MentionInstance>>,
}

pub(crate) async fn chat_steer_inner(
    state: &SessionEngineManager,
    command: ChatSteerCommand,
) -> Result<SteerPush, String> {
    state
        .steer(
            &command.session_id,
            crate::session::SessionKind::Eps,
            SteerInput {
                steer_id: command.steer_id,
                text: command.text,
                attachments: command.attachments,
                mentions: SteerMentions::Eps(command.mentions.unwrap_or_default()),
            },
        )
        .await
        .map_err(|error| error.message)
}

#[tauri::command(rename = "chat_steer")]
pub(crate) async fn engine_chat_steer(
    state: tauri::State<'_, SessionEngineManager>,
    session_id: String,
    steer_id: String,
    text: String,
    attachments: Vec<String>,
    mentions: Option<Vec<crate::mentions::MentionInstance>>,
) -> Result<SteerPush, String> {
    chat_steer_inner(
        &state,
        ChatSteerCommand {
            session_id,
            steer_id,
            text,
            attachments,
            mentions,
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steer(id: &str, text: &str, attachments: &[&str]) -> SteerMessage {
        SteerMessage {
            steer_id: id.to_string(),
            text: text.to_string(),
            attachments: attachments.iter().map(|id| id.to_string()).collect(),
            mentions: serde_json::json!([]),
            model_text: crate::steer::frame_for_model(text),
            image_paths: Vec::new(),
        }
    }

    #[test]
    fn only_an_ordinary_conversation_starts_a_turn_for_unconsumed_steers() {
        assert_eq!(
            unconsumed_disposition(false, false, false),
            UnconsumedSteers::FollowUp
        );
        // A cancelled turn, a worker session (a hidden turn on a settled
        // worker), and a live autonomous run all hand them back.
        assert_eq!(
            unconsumed_disposition(true, false, false),
            UnconsumedSteers::Return
        );
        assert_eq!(
            unconsumed_disposition(false, true, false),
            UnconsumedSteers::Return
        );
        assert_eq!(
            unconsumed_disposition(false, false, true),
            UnconsumedSteers::Return
        );
    }

    #[test]
    fn unconsumed_steers_become_one_ordinary_turn_in_their_order() {
        let request = follow_up_request(
            "turn".to_string(),
            &[
                steer("a", "make it red", &["att-1"]),
                steer("b", "  ", &["att-2"]),
                steer("c", "and faster", &[]),
            ],
        );
        assert_eq!(request.client_turn_id, "turn");
        assert_eq!(request.text, "make it red\n\nand faster");
        assert_eq!(request.attachments, vec!["att-1", "att-2"]);
        assert!(!request.clarify);
        assert!(request.skill_selections.is_empty());
        assert_eq!(
            request.execution_mode,
            crate::autonomous::ExecutionMode::Interactive
        );
    }
}
