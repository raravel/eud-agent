//! Mid-turn steering: a message the user sends while a turn runs reaches that
//! turn at its next tool boundary instead of waiting for the turn to end.
//!
//! Each session owns one [`SteerInbox`], shared like the cancellation watch and
//! never behind the engine mutex, so a steer is accepted while the turn holds
//! the engine. The engine opens the inbox for the length of a turn and closes
//! it when the turn ends; a push after the close answers [`SteerPush::NoTurn`]
//! under the same lock, and the caller sends the message as an ordinary turn.
//! What the turn never consumed stays in the inbox for the engine to start the
//! next turn with (or to hand back to the composer after a cancel).

use std::{collections::VecDeque, path::PathBuf, sync::Arc};

use parking_lot::Mutex;
use serde::Serialize;

/// One message the user sent while a turn ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteerMessage {
    /// The panel's id for the message (a UUID), echoed in every steer event.
    pub steer_id: String,
    /// The text as the user typed it: the panel row, and the text of the
    /// ordinary turn an unconsumed steer starts.
    pub text: String,
    /// Staged attachment ids, kept for the ordinary turn an unconsumed steer
    /// starts (binding them again to the same session is idempotent).
    pub attachments: Vec<String>,
    /// The mention snapshots the user attached, for the same reason.
    pub mentions: serde_json::Value,
    /// What the model receives: the text with its resolved attachments and
    /// mentions, framed as the user's own message.
    pub model_text: String,
    pub image_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SteerPush {
    /// The running turn will receive the message at its next tool boundary.
    Accepted,
    /// No turn is running; the caller sends the message as an ordinary turn.
    NoTurn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SteerEventKind {
    /// The running turn received these steers at this point of its stream.
    Applied,
    /// The turn ended without receiving them and was cancelled: they go back
    /// to the composer.
    Returned,
    /// The turn ended without receiving them: an ordinary turn starts with
    /// them now (`client_turn_id` names it).
    Turn,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SteerEventItem {
    pub steer_id: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SteerEvent {
    pub kind: SteerEventKind,
    pub steers: Vec<SteerEventItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_turn_id: Option<String>,
}

impl SteerEvent {
    pub fn new(kind: SteerEventKind, steers: &[SteerMessage]) -> Self {
        Self {
            kind,
            steers: steers
                .iter()
                .map(|steer| SteerEventItem {
                    steer_id: steer.steer_id.clone(),
                    text: steer.text.clone(),
                })
                .collect(),
            client_turn_id: None,
        }
    }
}

type SteerEmitter = Arc<dyn Fn(SteerEvent) + Send + Sync>;

#[derive(Default)]
struct State {
    open: bool,
    pending: VecDeque<SteerMessage>,
}

#[derive(Default)]
struct Inner {
    state: Mutex<State>,
    notify: tokio::sync::Notify,
    emitter: Mutex<Option<SteerEmitter>>,
}

/// The per-session steer inbox; cloning shares it.
#[derive(Clone, Default)]
pub struct SteerInbox {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for SteerInbox {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.inner.state.lock();
        formatter
            .debug_struct("SteerInbox")
            .field("open", &state.open)
            .field("pending", &state.pending.len())
            .finish()
    }
}

impl SteerInbox {
    pub fn set_emitter(&self, emitter: impl Fn(SteerEvent) + Send + Sync + 'static) {
        *self.inner.emitter.lock() = Some(Arc::new(emitter));
    }

    pub fn emit(&self, event: SteerEvent) {
        let emitter = self.inner.emitter.lock().clone();
        if let Some(emitter) = emitter {
            emitter(event);
        }
    }

    /// A turn starts: steers are accepted until [`Self::close`]. Steers a
    /// previous turn left behind stay queued for this one.
    pub fn open(&self) {
        let mut state = self.inner.state.lock();
        state.open = true;
        if !state.pending.is_empty() {
            self.inner.notify.notify_one();
        }
    }

    /// The turn ended: later pushes answer [`SteerPush::NoTurn`].
    pub fn close(&self) {
        self.inner.state.lock().open = false;
    }

    pub fn is_open(&self) -> bool {
        self.inner.state.lock().open
    }

    pub fn push(&self, message: SteerMessage) -> SteerPush {
        let mut state = self.inner.state.lock();
        if !state.open {
            return SteerPush::NoTurn;
        }
        state.pending.push_back(message);
        self.inner.notify.notify_one();
        SteerPush::Accepted
    }

    /// Take every queued steer for delivery.
    pub fn drain(&self) -> Vec<SteerMessage> {
        self.inner.state.lock().pending.drain(..).collect()
    }

    /// Put steers a delivery could not complete back in front of the queue,
    /// in their order.
    pub fn requeue(&self, messages: Vec<SteerMessage>) {
        if messages.is_empty() {
            return;
        }
        let mut state = self.inner.state.lock();
        for message in messages.into_iter().rev() {
            state.pending.push_front(message);
        }
        if state.open {
            self.inner.notify.notify_one();
        }
    }

    /// The steers a closed turn never consumed. An open inbox belongs to a
    /// running turn, which still delivers them, so it yields nothing.
    pub fn take_unconsumed(&self) -> Vec<SteerMessage> {
        let mut state = self.inner.state.lock();
        if state.open {
            return Vec::new();
        }
        state.pending.drain(..).collect()
    }

    /// Resolves when a steer may be waiting. Spurious wakes are possible, so
    /// the caller drains and handles an empty result.
    pub async fn wait(&self) {
        self.inner.notify.notified().await;
    }

    /// The running turn received `messages`; tell the panels.
    pub fn applied(&self, messages: &[SteerMessage]) {
        if messages.is_empty() {
            return;
        }
        self.emit(SteerEvent::new(SteerEventKind::Applied, messages));
    }
}

/// Wait on an optional inbox; without one the future never resolves, so a
/// `select!` branch over it stays disabled.
pub async fn wait_optional(inbox: Option<&SteerInbox>) {
    match inbox {
        Some(inbox) => inbox.wait().await,
        None => std::future::pending().await,
    }
}

/// The text the model receives for one steer.
pub fn frame_for_model(text: &str) -> String {
    format!("[user message during this turn]\n{text}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(id: &str) -> SteerMessage {
        SteerMessage {
            steer_id: id.to_string(),
            text: format!("text {id}"),
            attachments: Vec::new(),
            mentions: serde_json::Value::Null,
            model_text: frame_for_model(id),
            image_paths: Vec::new(),
        }
    }

    #[test]
    fn a_push_without_a_running_turn_is_refused() {
        let inbox = SteerInbox::default();
        assert_eq!(inbox.push(message("a")), SteerPush::NoTurn);
        assert!(inbox.drain().is_empty());
    }

    #[test]
    fn a_push_after_the_turn_closed_is_refused_and_never_queued() {
        let inbox = SteerInbox::default();
        inbox.open();
        assert_eq!(inbox.push(message("a")), SteerPush::Accepted);
        inbox.close();
        assert_eq!(inbox.push(message("b")), SteerPush::NoTurn);
        let unconsumed = inbox.take_unconsumed();
        assert_eq!(
            unconsumed
                .iter()
                .map(|m| m.steer_id.as_str())
                .collect::<Vec<_>>(),
            vec!["a"]
        );
        assert!(inbox.take_unconsumed().is_empty());
    }

    #[test]
    fn a_running_turn_keeps_its_steers_from_the_unconsumed_take() {
        let inbox = SteerInbox::default();
        inbox.open();
        inbox.push(message("a"));
        assert!(inbox.take_unconsumed().is_empty());
        assert_eq!(inbox.drain().len(), 1);
    }

    #[test]
    fn a_requeued_steer_keeps_its_place_before_later_ones() {
        let inbox = SteerInbox::default();
        inbox.open();
        inbox.push(message("a"));
        inbox.push(message("b"));
        let taken = inbox.drain();
        inbox.push(message("c"));
        inbox.requeue(taken);
        let order = inbox
            .drain()
            .into_iter()
            .map(|m| m.steer_id)
            .collect::<Vec<_>>();
        assert_eq!(order, vec!["a", "b", "c"]);
    }

    /// Pushes racing the turn end are either delivered to the turn (taken as
    /// unconsumed after it closed) or refused; none is lost or doubled.
    #[test]
    fn concurrent_pushes_racing_the_close_are_never_lost() {
        for _ in 0..50 {
            let inbox = SteerInbox::default();
            inbox.open();
            let handles = (0..8)
                .map(|index| {
                    let inbox = inbox.clone();
                    std::thread::spawn(move || (index, inbox.push(message(&index.to_string()))))
                })
                .collect::<Vec<_>>();
            inbox.close();
            let results = handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect::<Vec<_>>();
            let accepted = results
                .iter()
                .filter(|(_, push)| *push == SteerPush::Accepted)
                .map(|(index, _)| index.to_string())
                .collect::<std::collections::BTreeSet<_>>();
            let kept = inbox
                .take_unconsumed()
                .into_iter()
                .map(|m| m.steer_id)
                .collect::<std::collections::BTreeSet<_>>();
            assert_eq!(accepted, kept);
        }
    }

    #[tokio::test]
    async fn a_push_wakes_a_waiting_delivery() {
        let inbox = SteerInbox::default();
        inbox.open();
        let waiter = {
            let inbox = inbox.clone();
            tokio::spawn(async move {
                inbox.wait().await;
                inbox.drain()
            })
        };
        tokio::task::yield_now().await;
        inbox.push(message("a"));
        let delivered = tokio::time::timeout(std::time::Duration::from_secs(2), waiter)
            .await
            .expect("the push wakes the waiter")
            .unwrap();
        assert_eq!(delivered.len(), 1);
    }

    #[test]
    fn applied_steers_are_announced_with_their_text() {
        let inbox = SteerInbox::default();
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        inbox.set_emitter(move |event| sink.lock().push(event));
        inbox.applied(&[message("a")]);
        inbox.applied(&[]);
        let events = events.lock();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, SteerEventKind::Applied);
        assert_eq!(events[0].steers[0].steer_id, "a");
        assert_eq!(events[0].steers[0].text, "text a");
    }
}
