//! Shared tool services and one request runtime per conversation session.
//!
//! [`ToolServices`] owns app-wide immutable/shared services. Each
//! [`SessionToolRuntime`] is cloned only by one session engine and its MCP handler,
//! so request ids, evidence/plan/budget gates, and write tickets cannot overwrite another session.
//!
//! [`SessionToolRuntime::execute`] is the single tool entry point. It verifies
//! concurrent write registration, serializes each shared-state operation, applies
//! validation and safety gates, and journals every project write for review.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};

use crate::bootstrap::process_tree::ProcessCancellation;
use crate::config::DataDirs;
use crate::journal::{DatTable, JournalEntry, JournalStore, JournalTarget, Snapshot, WriteTool};
use crate::mapsafe::{CompilingStatus, IsomEngine, MapSafe, WindowsLockProbe};
use crate::native_project::{DatScalar, DatTarget, NativeDatChange, NativeDatPatch};
use crate::rag::Rag;
use crate::tools::{self, RequestState};
use crate::workspace::{apply_exact_text_edits, ExactTextEdit};

struct ProcessCancellationBridge {
    token: ProcessCancellation,
    stop: mpsc::Sender<()>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl ProcessCancellationBridge {
    fn new(mut receiver: tokio::sync::watch::Receiver<u64>) -> Self {
        let generation = *receiver.borrow_and_update();
        let token = ProcessCancellation::default();
        let watched = token.clone();
        let (stop, stopped) = mpsc::channel();
        let worker = std::thread::spawn(move || loop {
            match stopped.recv_timeout(Duration::from_millis(10)) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if *receiver.borrow() != generation {
                        watched.cancel();
                        break;
                    }
                }
            }
        });
        Self {
            token,
            stop,
            worker: Some(worker),
        }
    }

    fn token(&self) -> &ProcessCancellation {
        &self.token
    }
}

impl Drop for ProcessCancellationBridge {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Maximum `search_docs` top-k (mirrors the registry/feature 11 clamp).
const SEARCH_DOCS_MAX_K: i64 = 10;
const SEARCH_DOCS_DEFAULT_K: i64 = 5;
const SEARCH_DOCS_PREVIEW_CHARS: usize = 480;
const DOCS_GET_MAX_IDS: usize = 10;
const READ_FILE_DEFAULT_LINES: usize = 400;
const SOURCE_SEARCH_DEFAULT_LIMIT: usize = 20;
const SOURCE_SEARCH_MAX_LIMIT: usize = 100;
const SOURCE_SEARCH_MAX_CONTEXT_LINES: usize = 20;
const SOURCE_SEARCH_MAX_QUERY_CHARS: usize = 256;
const MAP_PALETTE_QUERY_MAX_MATCHES: usize = 256;

/// Native euddraft build-state probe used by map-write safety rails.
#[derive(Clone)]
pub struct NativeCompilingStatus {
    dirs: DataDirs,
}

impl CompilingStatus for NativeCompilingStatus {
    fn is_compiling(&self) -> bool {
        crate::native_runtime::NativeProjectManager::new(self.dirs.clone()).is_building()
    }
}

/// Production map-write service: native build guard, Windows share probe, and
/// the isom static-library engine.
pub type ProductionMapSafe = MapSafe<NativeCompilingStatus, WindowsLockProbe, IsomEngine>;

/// Shared, immutable production services. Session workers clone these handles,
/// while every request gate, plan, and write ticket remains inside a
/// [`SessionToolRuntime`].
#[derive(Clone)]
pub struct ToolServices {
    dirs: DataDirs,
    journal: JournalStore,
    rag: Arc<Rag>,
    map_safe: Arc<ProductionMapSafe>,
    writes: crate::write_coordinator::ProjectWriteCoordinator,
    map_candidates: crate::map_candidate::CandidateStore,
    map_images: crate::map_image::MapImageService,
    audio: crate::audio::AudioService,
    mentions: crate::mentions::MentionService,
    native: crate::native_runtime::NativeProjectManager,
}

impl ToolServices {
    pub fn new(
        dirs: DataDirs,
        map_candidates: crate::map_candidate::CandidateStore,
        writes: crate::write_coordinator::ProjectWriteCoordinator,
    ) -> Self {
        let journal = JournalStore::new(dirs.app_data());
        let rag = Arc::new(load_rag(&dirs));
        let map_safe = Arc::new(MapSafe::new(
            dirs.app_data().to_path_buf(),
            NativeCompilingStatus { dirs: dirs.clone() },
            WindowsLockProbe,
            IsomEngine,
        ));
        let mentions = crate::mentions::MentionService::new(
            map_candidates.clone(),
            crate::map_context::MapContextService::new(dirs.clone()),
        );
        let audio = crate::audio::AudioService::new(dirs.clone());
        let native = crate::native_runtime::NativeProjectManager::new(dirs.clone());
        Self {
            dirs,
            journal,
            rag,
            map_safe,
            map_candidates,
            audio,
            map_images: crate::map_image::MapImageService::new(),
            mentions,
            native,
            writes,
        }
    }

    pub fn session(&self, session_id: impl Into<String>) -> SessionToolRuntime {
        SessionToolRuntime::new(self.clone(), session_id.into())
    }
    pub fn map_session(&self, session_id: impl Into<String>) -> SessionToolRuntime {
        SessionToolRuntime::new_kind(
            self.clone(),
            session_id.into(),
            crate::session::SessionKind::Map,
        )
    }

    pub fn map_candidates(&self) -> crate::map_candidate::CandidateStore {
        self.map_candidates.clone()
    }

    pub fn mentions(&self) -> crate::mentions::MentionService {
        self.mentions.clone()
    }

    pub fn rag(&self) -> Arc<Rag> {
        Arc::clone(&self.rag)
    }

    pub fn journal(&self) -> &JournalStore {
        &self.journal
    }

    pub fn writes(&self) -> &crate::write_coordinator::ProjectWriteCoordinator {
        &self.writes
    }
    pub fn native(&self) -> crate::native_runtime::NativeProjectManager {
        self.native.clone()
    }
}

#[derive(Debug, Clone)]
struct SessionRequest {
    request_id: String,
    project_id: String,
    source_baseline_root: Option<PathBuf>,
    image_refs: BTreeMap<String, crate::map_image::MapImageBinding>,
    sound_results: usize,
    audio_refs: BTreeMap<String, crate::audio::AudioBinding>,
    audio_temp: Option<Arc<crate::audio::RequestAudioTemp>>,
}

#[derive(Debug, Default)]
struct SessionWriteState {
    ticket: Option<crate::write_coordinator::WriteTicket>,
    reason: Option<String>,
    hazard: Option<String>,
}
type AskEmitter = Arc<dyn Fn(crate::ipc::AskEvent) -> Result<(), String> + Send + Sync>;
type ProgressEmitter = Arc<dyn Fn(crate::ipc::ProgressEvent) -> Result<(), String> + Send + Sync>;
type AutonomousEmitter =
    Arc<dyn Fn(crate::autonomous::AutonomousRunState) -> Result<(), String> + Send + Sync>;

struct PendingAsk {
    owner_request_id: String,
    questions: Vec<crate::ipc::AskQuestion>,
    /// When the bounded wait started, so a restored card shows the real remainder.
    started_at: std::time::Instant,
    response: tokio::sync::oneshot::Sender<Result<BTreeMap<String, crate::ipc::AskAnswer>, String>>,
}

/// The one ask of the current foreground run whose bounded wait elapsed.
struct ExpiredAsk {
    ask_request_id: String,
    owner_request_id: String,
}

struct AskState {
    next_id: u64,
    emitter: Option<AskEmitter>,
    pending: HashMap<String, PendingAsk>,
    /// Bounded wait before a pending ask expires; tests inject shorter values.
    wait_timeout: Duration,
    expired: Option<ExpiredAsk>,
}

impl Default for AskState {
    fn default() -> Self {
        Self {
            next_id: 0,
            emitter: None,
            pending: HashMap::new(),
            wait_timeout: tools::ASK_WAIT_TIMEOUT,
            expired: None,
        }
    }
}

#[derive(serde::Deserialize)]
struct AskToolInput {
    questions: Vec<crate::ipc::AskQuestion>,
}

fn validate_ask_questions(questions: &[crate::ipc::AskQuestion]) -> Result<(), String> {
    if !(1..=4).contains(&questions.len()) {
        return Err("ask requires between 1 and 4 related questions".to_string());
    }

    let mut ids = HashSet::new();
    for question in questions {
        let id = question.id.trim();
        if id.is_empty() || id.len() > 64 {
            return Err("each ask question id must contain 1 to 64 characters".to_string());
        }
        if !ids.insert(id) {
            return Err(format!("ask question id `{id}` is duplicated"));
        }
        if question.question.trim().is_empty() || question.question.len() > 1_000 {
            return Err(format!(
                "ask question `{id}` must contain 1 to 1000 characters"
            ));
        }
        if question
            .header
            .as_ref()
            .is_some_and(|header| header.len() > 80)
        {
            return Err(format!("ask question `{id}` header exceeds 80 characters"));
        }
        if question.options.is_empty() {
            if question.multi {
                return Err(format!(
                    "ask question `{id}` cannot enable multi without selectable options"
                ));
            }
            continue;
        }
        if !(2..=5).contains(&question.options.len()) {
            return Err(format!(
                "ask question `{id}` requires between 2 and 5 selectable options"
            ));
        }
        let mut labels = HashSet::new();
        for option in &question.options {
            let label = option.label.trim();
            if label.is_empty() || option.label.len() > 120 {
                return Err(format!(
                    "ask question `{id}` option labels must contain 1 to 120 characters"
                ));
            }
            if !labels.insert(label) {
                return Err(format!(
                    "ask question `{id}` option label `{label}` is duplicated"
                ));
            }
            if option
                .description
                .as_ref()
                .is_some_and(|description| description.len() > 500)
            {
                return Err(format!(
                    "ask question `{id}` option description exceeds 500 characters"
                ));
            }
        }
    }
    Ok(())
}

fn validate_ask_answers(
    questions: &[crate::ipc::AskQuestion],
    answers: BTreeMap<String, crate::ipc::AskAnswer>,
) -> Result<BTreeMap<String, crate::ipc::AskAnswer>, String> {
    if answers.len() != questions.len() {
        return Err("ask response must answer every question exactly once".to_string());
    }

    let expected = questions
        .iter()
        .map(|question| question.id.as_str())
        .collect::<HashSet<_>>();
    if answers.keys().any(|id| !expected.contains(id.as_str())) {
        return Err("ask response contains an unknown question id".to_string());
    }

    let mut normalized = BTreeMap::new();
    for question in questions {
        let answer = answers
            .get(&question.id)
            .ok_or_else(|| format!("ask question `{}` has no answer", question.id))?;
        if answer.answers.is_empty() {
            return Err(format!(
                "ask question `{}` requires at least one answer",
                question.id
            ));
        }
        let max_answers = if question.multi {
            question.options.len() + 1
        } else {
            1
        };
        if answer.answers.len() > max_answers {
            return Err(format!(
                "ask question `{}` accepts at most {max_answers} answer(s)",
                question.id
            ));
        }
        let mut values = Vec::with_capacity(answer.answers.len());
        let mut seen = HashSet::new();
        for value in &answer.answers {
            let value = value.trim();
            if value.is_empty() || value.len() > 2_000 {
                return Err(format!(
                    "ask question `{}` answers must contain 1 to 2000 characters",
                    question.id
                ));
            }
            if !seen.insert(value) {
                return Err(format!(
                    "ask question `{}` contains a duplicated answer",
                    question.id
                ));
            }
            values.push(value.to_string());
        }
        normalized.insert(
            question.id.clone(),
            crate::ipc::AskAnswer { answers: values },
        );
    }
    Ok(normalized)
}

/// One session's MCP request state. Clones are shared only by that session's
/// engine and loopback MCP handler.
#[derive(Clone)]
pub struct SessionToolRuntime {
    services: ToolServices,
    session_id: String,
    kind: crate::session::SessionKind,
    request: Arc<Mutex<Option<SessionRequest>>>,
    request_state: Arc<Mutex<Option<RequestState>>>,
    pending_plan: Arc<Mutex<Option<(String, String)>>>,
    write_state: Arc<Mutex<SessionWriteState>>,
    execution_lock: Arc<Mutex<()>>,
    #[cfg(test)]
    completion_barrier: Arc<Mutex<Option<ToolCompletionBarrier>>>,
    ask: Arc<Mutex<AskState>>,
    ask_waiting: tokio::sync::watch::Sender<bool>,
    cancellation: Arc<Mutex<Option<tokio::sync::watch::Receiver<u64>>>>,
    progress_emitter: Arc<Mutex<Option<ProgressEmitter>>>,
    autonomous_emitter: Arc<Mutex<Option<AutonomousEmitter>>>,
    provider_identity: Arc<Mutex<Option<(crate::provider::ProviderId, String)>>>,
    last_build: Arc<Mutex<Option<crate::harness::BuildEvidence>>>,
    /// The complete JSON of the request's latest `build_run` and trace results,
    /// retained as verifier evidence (file/line diagnostics, not only counts).
    last_build_result: Arc<Mutex<Option<Value>>>,
    last_trace_result: Arc<Mutex<Option<Value>>>,
    sound_build_required: Arc<Mutex<bool>>,
    autonomous_pause_requested: Arc<AtomicBool>,
}

struct PendingAskLease {
    runtime: SessionToolRuntime,
    request_id: String,
}

impl Drop for PendingAskLease {
    fn drop(&mut self) {
        if self.runtime.remove_pending_ask(&self.request_id).is_some() {
            self.runtime.emit_activity_after_ask();
        }
    }
}

impl SessionToolRuntime {
    pub fn new(services: ToolServices, session_id: String) -> Self {
        Self::new_kind(services, session_id, crate::session::SessionKind::Eps)
    }

    pub fn new_kind(
        services: ToolServices,
        session_id: String,
        kind: crate::session::SessionKind,
    ) -> Self {
        let (ask_waiting, _) = tokio::sync::watch::channel(false);
        Self {
            services,
            session_id,
            kind,
            request: Arc::new(Mutex::new(None)),
            request_state: Arc::new(Mutex::new(None)),
            pending_plan: Arc::new(Mutex::new(None)),
            write_state: Arc::new(Mutex::new(SessionWriteState::default())),
            execution_lock: Arc::new(Mutex::new(())),
            #[cfg(test)]
            completion_barrier: Arc::new(Mutex::new(None)),
            ask: Arc::new(Mutex::new(AskState::default())),
            ask_waiting,
            cancellation: Arc::new(Mutex::new(None)),
            progress_emitter: Arc::new(Mutex::new(None)),
            autonomous_emitter: Arc::new(Mutex::new(None)),
            provider_identity: Arc::new(Mutex::new(None)),
            last_build: Arc::new(Mutex::new(None)),
            last_build_result: Arc::new(Mutex::new(None)),
            last_trace_result: Arc::new(Mutex::new(None)),
            sound_build_required: Arc::new(Mutex::new(false)),
            autonomous_pause_requested: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    pub fn kind(&self) -> crate::session::SessionKind {
        self.kind
    }
    pub fn autonomous_pause_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.autonomous_pause_requested)
    }

    pub fn autonomous_pause_requested(&self) -> bool {
        self.autonomous_pause_requested.load(Ordering::SeqCst)
    }

    pub fn set_autonomous_pause_requested(&self, requested: bool) {
        self.autonomous_pause_requested
            .store(requested, Ordering::SeqCst);
    }

    pub(crate) fn tool_descriptors(&self) -> Vec<Value> {
        if self.kind == crate::session::SessionKind::Map {
            crate::tools::map_mcp_tool_descriptors()
        } else {
            crate::tools::mcp_tool_descriptors()
        }
    }

    pub(crate) fn matches_run_scope(
        &self,
        identity: &crate::provider_runtime::RunIdentity,
    ) -> bool {
        self.matches_request_scope(identity)
            && self
                .cancellation
                .lock()
                .as_ref()
                .is_some_and(|receiver| *receiver.borrow() == identity.cancellation_generation)
    }

    pub fn journal(&self) -> &JournalStore {
        &self.services.journal
    }

    pub fn app_data_dir(&self) -> std::path::PathBuf {
        self.services.dirs.app_data().to_path_buf()
    }

    pub fn data_dirs(&self) -> DataDirs {
        self.services.dirs.clone()
    }

    pub fn mentions(&self) -> crate::mentions::MentionService {
        self.services.mentions()
    }

    pub fn set_ask_emitter(
        &self,
        emitter: impl Fn(crate::ipc::AskEvent) -> Result<(), String> + Send + Sync + 'static,
    ) {
        self.ask.lock().emitter = Some(Arc::new(emitter));
    }

    pub fn set_progress_emitter(
        &self,
        emitter: impl Fn(crate::ipc::ProgressEvent) -> Result<(), String> + Send + Sync + 'static,
    ) {
        *self.progress_emitter.lock() = Some(Arc::new(emitter));
    }
    pub fn set_autonomous_emitter(
        &self,
        emitter: impl Fn(crate::autonomous::AutonomousRunState) -> Result<(), String>
            + Send
            + Sync
            + 'static,
    ) {
        *self.autonomous_emitter.lock() = Some(Arc::new(emitter));
    }

    pub fn set_cancellation(&self, cancellation: tokio::sync::watch::Receiver<u64>) {
        *self.cancellation.lock() = Some(cancellation);
    }
    pub fn set_provider_identity(
        &self,
        provider: crate::provider::ProviderId,
        model: impl Into<String>,
    ) {
        *self.provider_identity.lock() = Some((provider, model.into()));
    }
    pub fn subscribe_ask_waiting(&self) -> tokio::sync::watch::Receiver<bool> {
        self.ask_waiting.subscribe()
    }

    /// Override the bounded ask wait (tests only; production keeps
    /// [`tools::ASK_WAIT_TIMEOUT`]).
    #[cfg(test)]
    pub(crate) fn set_ask_wait_timeout(&self, timeout: Duration) {
        self.ask.lock().wait_timeout = timeout;
    }

    /// Whether an ask of `request_id` expired in the current foreground run, so
    /// the turn is ending with the question restated as text.
    pub(crate) fn ask_expired_for_request(&self, request_id: &str) -> bool {
        self.ask
            .lock()
            .expired
            .as_ref()
            .is_some_and(|expired| expired.owner_request_id == request_id)
    }

    /// Forget an expired ask. The engine calls this whenever it starts a new
    /// foreground run for the same request (continuation, plan feedback, write
    /// transition), so the "do not ask again" rule covers exactly one run.
    pub(crate) fn clear_expired_ask(&self) {
        self.ask.lock().expired = None;
    }

    fn emit_progress(&self, stage: crate::ipc::ProgressStage, detail: &str) {
        if let Some(emitter) = self.progress_emitter.lock().clone() {
            let identity = self.provider_identity.lock().clone();
            let _ = emitter(crate::ipc::ProgressEvent {
                stage,
                detail: Some(detail.to_string()),
                provider: identity.as_ref().map(|(provider, _)| *provider),
                model: identity.map(|(_, model)| model),
            });
        }
    }

    pub async fn ask(&self, args: &Value) -> Result<Value, String> {
        let owner_request_id = self.current_request_id().ok_or_else(|| {
            "no agent request is open; ask is only valid during a turn".to_string()
        })?;
        self.ask_scoped(&owner_request_id, None, args).await
    }

    pub(crate) async fn ask_for_run(
        &self,
        identity: &crate::provider_runtime::RunIdentity,
        args: &Value,
    ) -> Result<Value, String> {
        if self.session_id != identity.session_id || self.kind != identity.session_kind {
            return Err("stale provider run cannot create an ask request".to_string());
        }
        self.ask_scoped(
            &identity.request_id,
            Some(identity.cancellation_generation),
            args,
        )
        .await
    }

    /// Ask the user structured questions on behalf of the engine (not a model
    /// tool call), for example a triage clarification. Waits through the same
    /// pending-ask path as the `ask` tool and returns the validated answers.
    pub(crate) async fn ask_for_request(
        &self,
        request_id: &str,
        questions: Vec<crate::ipc::AskQuestion>,
    ) -> Result<BTreeMap<String, crate::ipc::AskAnswer>, String> {
        let outcome = self
            .ask_scoped(request_id, None, &json!({ "questions": questions }))
            .await?;
        if outcome["status"] == "unanswered" {
            return Err(format!(
                "ask_unanswered: 질문 답변 대기 시간({}초)이 지났습니다.",
                outcome["waitedSeconds"]
            ));
        }
        serde_json::from_value(outcome["answers"].clone())
            .map_err(|error| format!("ask answers are malformed: {error}"))
    }

    async fn ask_scoped(
        &self,
        expected_request_id: &str,
        expected_generation: Option<u64>,
        args: &Value,
    ) -> Result<Value, String> {
        let input: AskToolInput = serde_json::from_value(args.clone())
            .map_err(|error| format!("invalid ask arguments: {error}"))?;
        validate_ask_questions(&input.questions)?;

        let request_guard = self.execution_lock.lock();
        let generation_matches = expected_generation.map_or(true, |expected| {
            self.cancellation
                .lock()
                .as_ref()
                .is_some_and(|receiver| *receiver.borrow() == expected)
        });
        if self.current_request_id().as_deref() != Some(expected_request_id) || !generation_matches
        {
            return Err("stale provider run cannot create an ask request".to_string());
        }
        let (request_id, response, emitter, wait_timeout) = {
            let mut ask = self.ask.lock();
            if !ask.pending.is_empty() {
                return Err("another ask request is already waiting for this session".to_string());
            }
            if ask
                .expired
                .as_ref()
                .is_some_and(|expired| expired.owner_request_id == expected_request_id)
            {
                return Err(
                    "a previous ask in this turn went unanswered; restate the questions as your final text answer and end the turn instead of calling ask again"
                        .to_string(),
                );
            }
            let emitter = ask
                .emitter
                .clone()
                .ok_or_else(|| "ask UI is unavailable for this session".to_string())?;
            ask.next_id = ask
                .next_id
                .checked_add(1)
                .ok_or_else(|| "ask request id overflow".to_string())?;
            let request_id = format!("ask-{}", ask.next_id);
            let (send, response) = tokio::sync::oneshot::channel();
            ask.pending.insert(
                request_id.clone(),
                PendingAsk {
                    owner_request_id: expected_request_id.to_string(),
                    questions: input.questions.clone(),
                    started_at: std::time::Instant::now(),
                    response: send,
                },
            );
            self.ask_waiting.send_replace(true);
            let wait_timeout = ask.wait_timeout;
            (request_id, response, emitter, wait_timeout)
        };
        drop(request_guard);
        let _lease = PendingAskLease {
            runtime: self.clone(),
            request_id: request_id.clone(),
        };

        self.emit_activity(crate::write_coordinator::SessionActivity::WaitingInput);
        self.update_autonomous_status(
            crate::autonomous::AutonomousRunStatus::WaitingInput,
            Some(crate::autonomous::AutonomousPauseReason::WaitingInput),
        )?;
        let wait_seconds = wait_timeout.as_secs();
        emitter(crate::ipc::AskEvent {
            request_id: request_id.clone(),
            status: crate::ipc::AskEventStatus::Pending,
            wait_seconds: Some(wait_seconds),
            questions: input.questions.clone(),
        })?;

        tokio::pin!(response);
        let deadline = tokio::time::sleep(wait_timeout);
        tokio::pin!(deadline);
        let answers = tokio::select! {
            biased;
            answered = &mut response => Some(answered),
            _ = &mut deadline => None,
        };
        let answers = match answers {
            Some(answered) => answered,
            None => {
                // The wait elapsed. If `answer_ask` removed the pending entry in
                // the meantime its answer already sits in the channel and wins;
                // otherwise this ask expires and the turn continues as text.
                let expired = {
                    let mut ask = self.ask.lock();
                    let removed = ask.pending.remove(&request_id).is_some();
                    if removed {
                        self.ask_waiting.send_replace(!ask.pending.is_empty());
                        ask.expired = Some(ExpiredAsk {
                            ask_request_id: request_id.clone(),
                            owner_request_id: expected_request_id.to_string(),
                        });
                    }
                    removed
                };
                if !expired {
                    response.await
                } else {
                    // The expiry is already recorded; status/UI publication
                    // failures must not turn it into a tool error.
                    self.emit_activity_after_ask();
                    if let Err(error) = self.update_autonomous_status(
                        crate::autonomous::AutonomousRunStatus::Running,
                        None,
                    ) {
                        eprintln!("eud-agent: ask expiry status update failed: {error}");
                    }
                    if let Err(error) = emitter(crate::ipc::AskEvent {
                        request_id,
                        status: crate::ipc::AskEventStatus::Expired,
                        wait_seconds: Some(wait_seconds),
                        questions: input.questions.clone(),
                    }) {
                        eprintln!("eud-agent: ask expiry event failed: {error}");
                    }
                    return Ok(json!({
                        "status": "unanswered",
                        "questionIds": input
                            .questions
                            .iter()
                            .map(|question| question.id.as_str())
                            .collect::<Vec<_>>(),
                        "waitedSeconds": wait_seconds,
                    }));
                }
            }
        };
        let answers = answers.map_err(|_| "ask response channel closed".to_string())??;
        self.update_autonomous_status(crate::autonomous::AutonomousRunStatus::Running, None)?;
        Ok(json!({ "answers": answers }))
    }

    pub fn pending_ask(&self) -> Option<crate::ipc::AskEvent> {
        let owner_request_id = self.current_request_id()?;
        let ask = self.ask.lock();
        ask.pending.iter().find_map(|(request_id, pending)| {
            (pending.owner_request_id == owner_request_id).then(|| crate::ipc::AskEvent {
                request_id: request_id.clone(),
                status: crate::ipc::AskEventStatus::Pending,
                wait_seconds: Some(
                    ask.wait_timeout
                        .saturating_sub(pending.started_at.elapsed())
                        .as_secs(),
                ),
                questions: pending.questions.clone(),
            })
        })
    }

    pub(crate) fn matches_request_scope(
        &self,
        identity: &crate::provider_runtime::RunIdentity,
    ) -> bool {
        self.session_id == identity.session_id
            && self.kind == identity.session_kind
            && self.current_request_id().as_deref() == Some(identity.request_id.as_str())
    }

    fn remove_pending_ask(&self, request_id: &str) -> Option<PendingAsk> {
        let mut ask = self.ask.lock();
        let pending = ask.pending.remove(request_id);
        if pending.is_some() {
            self.ask_waiting.send_replace(!ask.pending.is_empty());
        }
        pending
    }
    pub fn answer_ask(
        &self,
        request_id: &str,
        answers: BTreeMap<String, crate::ipc::AskAnswer>,
    ) -> Result<(), String> {
        let mut ask = self.ask.lock();
        if ask
            .expired
            .as_ref()
            .is_some_and(|expired| expired.ask_request_id == request_id)
        {
            return Err(format!(
                "ask_expired: 답변 대기 시간({}초)이 지나 접수되지 않았습니다. 대화에 답을 입력해 주세요.",
                ask.wait_timeout.as_secs()
            ));
        }
        let pending = ask
            .pending
            .get(request_id)
            .ok_or_else(|| format!("ask request `{request_id}` is not pending"))?;
        let answers = validate_ask_answers(&pending.questions, answers)?;
        let pending = ask
            .pending
            .remove(request_id)
            .ok_or_else(|| format!("ask request `{request_id}` is not pending"))?;
        self.ask_waiting.send_replace(!ask.pending.is_empty());
        drop(ask);
        pending
            .response
            .send(Ok(answers))
            .map_err(|_| format!("ask request `{request_id}` is no longer active"))?;
        self.emit_activity_after_ask();
        Ok(())
    }

    pub fn cancel_pending_ask(&self) {
        let pending = {
            let mut ask = self.ask.lock();
            let pending = std::mem::take(&mut ask.pending);
            self.ask_waiting.send_replace(false);
            pending
        };
        for (_, pending) in pending {
            let _ = pending
                .response
                .send(Err("ask request cancelled".to_string()));
        }
    }

    fn update_autonomous_status(
        &self,
        status: crate::autonomous::AutonomousRunStatus,
        pause_reason: Option<crate::autonomous::AutonomousPauseReason>,
    ) -> Result<(), String> {
        let sessions = crate::session::SessionStore::new(&self.services.dirs);
        let Ok(record) = sessions.load(&self.session_id) else {
            return Ok(());
        };
        if record.autonomous_run.is_none() {
            return Ok(());
        }
        let updated = sessions
            .update_autonomous_status(&self.session_id, status, pause_reason, None)
            .map_err(|error| format!("장시간 작업 상태를 저장하지 못했습니다: {error}"))?;
        if let (Some(run), Some(emitter)) = (updated, self.autonomous_emitter.lock().clone()) {
            emitter(run)?;
        }
        Ok(())
    }

    fn emit_activity_after_ask(&self) {
        let activity = if self.write_ticket().is_some() {
            crate::write_coordinator::SessionActivity::RunningWrite
        } else {
            crate::write_coordinator::SessionActivity::RunningRead
        };
        self.emit_activity(activity);
    }
    pub fn begin_request(&self, request_id: &str, project_id: &str) -> Result<(), String> {
        self.set_autonomous_pause_requested(false);
        let _execution = self.execution_lock.try_lock().ok_or_else(|| {
            "a previously admitted tool is still running; wait for its completion before opening a new request"
                .to_string()
        })?;
        if let Some(ticket) = self.write_state.lock().ticket.as_ref() {
            return Err(format!(
                "previous write ticket {} is still active; settle or abort it before opening {request_id}",
                ticket.request_id()
            ));
        }
        if self
            .current_request_id()
            .as_deref()
            .is_some_and(|current| current != request_id)
        {
            self.cancel_pending_ask();
        }
        *self.request.lock() = Some(SessionRequest {
            request_id: request_id.to_owned(),
            project_id: project_id.to_owned(),
            sound_results: 0,
            source_baseline_root: None,
            image_refs: BTreeMap::new(),
            audio_refs: BTreeMap::new(),
            audio_temp: None,
        });
        *self.request_state.lock() = Some(RequestState::for_request(request_id));
        self.ask.lock().expired = None;
        *self.pending_plan.lock() = None;
        *self.last_build.lock() = None;
        *self.last_build_result.lock() = None;
        *self.sound_build_required.lock() = false;
        Ok(())
    }

    /// Reset only the soft iteration action counter for a continuing request.
    pub fn begin_iteration(&self, request_id: &str) -> Result<(), String> {
        {
            let mut state = self.request_state.lock();
            let state = state
                .as_mut()
                .filter(|state| state.request_id == request_id)
                .ok_or_else(|| format!("request state for {request_id} is missing"))?;
            state.begin_iteration();
        }
        self.ask.lock().expired = None;
        Ok(())
    }

    pub fn iteration_action_boundary_reached(&self, request_id: &str) -> bool {
        self.request_state
            .lock()
            .as_ref()
            .filter(|state| state.request_id == request_id)
            .is_some_and(RequestState::iteration_action_boundary_reached)
    }

    pub fn latest_build_progress(&self) -> Option<tools::BuildProgress> {
        self.request_state
            .lock()
            .as_ref()
            .and_then(RequestState::latest_build)
            .cloned()
    }

    pub fn request_action_counts(&self) -> (u64, u64) {
        self.request_state.lock().as_ref().map_or((0, 0), |state| {
            (state.read_action_count, state.write_action_count)
        })
    }

    pub fn current_project_revision(&self) -> Result<String, String> {
        self.services.native().open()?.revision()
    }

    pub fn restore_autonomous_progress(
        &self,
        request_id: &str,
        progress: &crate::autonomous::AutonomousRunProgress,
    ) -> Result<(), String> {
        let mut state = self.request_state.lock();
        let state = state
            .as_mut()
            .filter(|state| state.request_id == request_id)
            .ok_or_else(|| format!("request state for {request_id} is missing"))?;
        state.restore_autonomous_progress(
            progress.read_actions,
            progress.write_actions,
            progress.latest_build.clone().map(Into::into),
        );
        Ok(())
    }

    pub fn bind_map_images(
        &self,
        request_id: &str,
        attachments: &[crate::attachment::ResolvedImageAttachment],
    ) -> Result<Vec<crate::map_image::MapImageRequestRef>, String> {
        if self.kind != crate::session::SessionKind::Map {
            return Err(
                "request-local imageRef bindings are available only to Map Agent".to_string(),
            );
        }
        let project_id = self
            .request
            .lock()
            .as_ref()
            .filter(|request| request.request_id == request_id)
            .map(|request| request.project_id.clone())
            .ok_or_else(|| format!("request {request_id} is not active"))?;
        let state = self
            .services
            .map_candidates
            .state(&project_id, &self.session_id)?;
        let bindings = self.services.map_images.bind_request_images(
            &self.session_id,
            request_id,
            attachments,
            &state.revision_key,
            &state.baseline.file_sha256,
        )?;
        let refs = bindings
            .iter()
            .map(crate::map_image::MapImageBinding::request_ref)
            .collect();
        let mut request = self.request.lock();
        let request = request
            .as_mut()
            .filter(|request| request.request_id == request_id)
            .ok_or_else(|| format!("request {request_id} ended while images were binding"))?;
        request.image_refs = bindings
            .into_iter()
            .map(|binding| (binding.image_ref.clone(), binding))
            .collect();
        Ok(refs)
    }

    pub fn bind_audio_attachments(
        &self,
        request_id: &str,
        attachments: Vec<crate::attachment::ResolvedAudioAttachment>,
    ) -> Result<Vec<crate::audio::TrustedAudioRef>, String> {
        if self.kind != crate::session::SessionKind::Eps {
            return Err("오디오 첨부는 메인 EPS 대화에서만 사용할 수 있습니다.".to_string());
        }
        if attachments.is_empty() {
            return Ok(Vec::new());
        }
        let (start, request_temp, existing_ids) = {
            let mut request = self.request.lock();
            let request = request
                .as_mut()
                .filter(|request| request.request_id == request_id)
                .ok_or_else(|| format!("request {request_id} is not active"))?;
            let request_temp = match request.audio_temp.clone() {
                Some(temp) => temp,
                None => {
                    let temp = self.services.audio.request_temp()?;
                    request.audio_temp = Some(temp.clone());
                    temp
                }
            };
            let existing_ids = request
                .audio_refs
                .values()
                .map(|binding| binding.descriptor.id.clone())
                .collect::<HashSet<_>>();
            (request.audio_refs.len(), request_temp, existing_ids)
        };
        let cancellation = self.cancellation.lock().clone();
        let mut bindings = Vec::with_capacity(attachments.len());
        for (offset, attachment) in attachments.into_iter().enumerate() {
            if existing_ids.contains(&attachment.descriptor.id)
                || bindings.iter().any(|binding: &crate::audio::AudioBinding| {
                    binding.descriptor.id == attachment.descriptor.id
                })
            {
                return Err("같은 오디오 첨부를 한 요청에 두 번 바인딩할 수 없습니다.".to_string());
            }
            let audio_ref = format!("audio-{}", start + offset + 1);
            self.emit_progress(
                crate::ipc::ProgressStage::AudioProbe,
                "첨부 오디오 스트림을 확인하고 있습니다.",
            );
            let binding = self.services.audio.bind(
                attachment,
                audio_ref,
                request_temp.clone(),
                cancellation.as_ref(),
            )?;
            bindings.push(binding);
        }
        let refs = bindings
            .iter()
            .map(crate::audio::AudioBinding::trusted_ref)
            .collect::<Vec<_>>();
        let mut request = self.request.lock();
        let request = request
            .as_mut()
            .filter(|request| request.request_id == request_id)
            .ok_or_else(|| format!("request {request_id} ended while audio was binding"))?;
        for binding in bindings {
            request
                .audio_refs
                .insert(binding.audio_ref.clone(), binding);
        }
        Ok(refs)
    }

    fn audio_binding(
        &self,
        request_id: &str,
        audio_ref: &str,
    ) -> Result<crate::audio::AudioBinding, String> {
        self.request
            .lock()
            .as_ref()
            .filter(|request| request.request_id == request_id)
            .and_then(|request| request.audio_refs.get(audio_ref))
            .cloned()
            .ok_or_else(|| {
                format!("audioRef '{audio_ref}' is not bound to the current session/request")
            })
    }

    pub fn clear_audio_cache(&self) {
        if let Some(request) = self.request.lock().as_mut() {
            request.audio_refs.clear();
            request.audio_temp = None;
        }
    }

    fn next_sound_ref(&self, request_id: &str) -> Result<String, String> {
        let mut request = self.request.lock();
        let request = request
            .as_mut()
            .filter(|request| request.request_id == request_id)
            .ok_or_else(|| format!("request {request_id} is not active"))?;
        request.sound_results = request
            .sound_results
            .checked_add(1)
            .ok_or_else(|| "soundRef sequence overflow".to_string())?;
        Ok(format!("sound-{}", request.sound_results))
    }

    fn map_image_binding(
        &self,
        request_id: &str,
        image_ref: &str,
    ) -> Result<crate::map_image::MapImageBinding, String> {
        self.request
            .lock()
            .as_ref()
            .filter(|request| request.request_id == request_id)
            .and_then(|request| request.image_refs.get(image_ref))
            .cloned()
            .ok_or_else(|| {
                format!("imageRef '{image_ref}' is not bound to the current Map Agent request")
            })
    }

    pub fn current_request_id(&self) -> Option<String> {
        self.request
            .lock()
            .as_ref()
            .map(|request| request.request_id.clone())
    }

    pub fn current_project_id(&self) -> Option<String> {
        self.request
            .lock()
            .as_ref()
            .map(|request| request.project_id.clone())
    }

    pub fn last_build_evidence(&self) -> Option<crate::harness::BuildEvidence> {
        self.last_build.lock().clone()
    }

    /// The latest complete `build_run` result JSON of this request.
    pub fn last_build_result(&self) -> Option<Value> {
        self.last_build_result.lock().clone()
    }

    pub fn sound_build_required(&self) -> bool {
        *self.sound_build_required.lock()
    }

    /// Bind the trusted turn baseline captured by [`WorkspaceManager::begin_turn`].
    ///
    /// Only write turns capture a baseline; mutating tools run exclusively in
    /// write turns, so the stale-check reference is always present when needed.
    pub fn bind_source_baseline(
        &self,
        request_id: &str,
        baseline_root: PathBuf,
    ) -> Result<(), String> {
        let mut request = self.request.lock();
        let active = request
            .as_mut()
            .filter(|request| request.request_id == request_id)
            .ok_or_else(|| format!("request {request_id} is not active"))?;
        active.source_baseline_root = Some(baseline_root);
        Ok(())
    }

    fn source_baseline(&self, path: &str) -> Result<Option<String>, String> {
        let baseline_root = self
            .request
            .lock()
            .as_ref()
            .and_then(|request| request.source_baseline_root.clone())
            .ok_or_else(|| "the current request has no captured source baseline".to_string())?;
        crate::workspace::read_source_baseline(&baseline_root, path)
            .map_err(|error| error.to_string())
    }

    fn source_created_by_request(&self, request_id: &str, path: &str) -> bool {
        self.services
            .journal
            .selected_entries(request_id, &crate::journal::DecisionIds::All)
            .is_ok_and(|entries| {
                entries.iter().any(|entry| {
                    if entry.tool != WriteTool::FileCreate {
                        return false;
                    }
                    let JournalTarget::Path { path: created } = &entry.target else {
                        return false;
                    };
                    created == path
                        || path.strip_prefix(created.as_str()).is_some_and(|suffix| {
                            suffix.starts_with('.') && !suffix[1..].contains('/')
                        })
                })
            })
    }

    pub fn clear_current(&self) {
        let _execution = self.execution_lock.lock();
        self.cancel_pending_ask();
        *self.request.lock() = None;
        if let Some(state) = self.request_state.lock().take() {
            eprintln!(
                "eud-agent: retrieval session={} request={} searches={} hits={} unique={} repeated={} search_bytes={} docs_get={} documents={} docs_bytes={}",
                self.session_id,
                state.request_id,
                state.search_docs_count,
                state.search_docs_returned_hits,
                state.search_docs_unique_hits,
                state.search_docs_repeated_hits,
                state.search_docs_result_bytes,
                state.docs_get_count,
                state.docs_get_documents,
                state.docs_get_result_bytes,
            );
        }
        *self.pending_plan.lock() = None;
    }

    pub fn take_pending_plan(&self, request_id: &str) -> Option<String> {
        let mut pending = self.pending_plan.lock();
        match pending.as_ref() {
            Some((id, _)) if id == request_id => pending.take().map(|(_, markdown)| markdown),
            _ => None,
        }
    }

    pub(crate) fn register_write_request(
        &self,
        reason: impl Into<String>,
    ) -> Result<crate::write_coordinator::WriteTicket, String> {
        let request = self
            .request
            .lock()
            .clone()
            .ok_or_else(|| "no agent request is open".to_string())?;
        let mut write = self.write_state.lock();
        if let Some(ticket) = &write.ticket {
            return Ok(ticket.clone());
        }
        let ticket = self.services.writes.request(
            &request.project_id,
            &self.session_id,
            &request.request_id,
        )?;
        write.reason = Some(reason.into());
        write.ticket = Some(ticket.clone());
        Ok(ticket)
    }

    pub fn restore_review(
        &self,
        project_id: &str,
        request_id: &str,
    ) -> Result<crate::write_coordinator::WriteTicket, String> {
        let ticket =
            self.services
                .writes
                .restore_review(project_id, &self.session_id, request_id)?;
        *self.write_state.lock() = SessionWriteState {
            ticket: Some(ticket.clone()),
            reason: Some("restored pending review".to_string()),
            hazard: None,
        };
        Ok(ticket)
    }

    pub fn write_ticket(&self) -> Option<crate::write_coordinator::WriteTicket> {
        self.write_state.lock().ticket.clone()
    }

    pub fn write_reason(&self) -> Option<String> {
        self.write_state.lock().reason.clone()
    }

    pub fn owns_write_registration(&self) -> bool {
        let Some(ticket) = self.write_ticket() else {
            return false;
        };
        self.services.writes.owns(
            ticket.project_id(),
            ticket.session_id(),
            ticket.request_id(),
        )
    }

    fn mark_write_hazard(&self, detail: impl Into<String>) {
        self.write_state.lock().hazard = Some(detail.into());
    }

    pub fn release_write_registration(&self) -> Result<bool, String> {
        if let Some(hazard) = self.write_state.lock().hazard.clone() {
            return Err(format!(
                "write lease retained because map rollback did not settle: {hazard}"
            ));
        }
        let ticket = self.write_state.lock().ticket.clone();
        let Some(ticket) = ticket else {
            return Ok(false);
        };
        let released = self.services.writes.release(ticket.request_id())?;
        if released {
            *self.write_state.lock() = SessionWriteState::default();
        }
        Ok(released)
    }

    /// Release a read turn's write intent after the turn itself failed. Read mode
    /// cannot mutate the session workspace or call mutating MCP tools, so a
    /// journal entry here is an invariant violation and must retain registration.
    pub fn abort_unmutated_write_intent(&self) -> Result<(), String> {
        let Some(ticket) = self.write_ticket() else {
            return Ok(());
        };
        if let Some(request_id) = self.current_request_id() {
            if self.services.journal.entry_count(&request_id) > 0 {
                return Err(format!(
                    "cannot abort write ticket {}; request has journaled mutations",
                    ticket.request_id()
                ));
            }
        }
        if self.release_write_registration()? {
            return Ok(());
        }
        if ticket.state() == crate::write_coordinator::TicketState::Cancelled {
            *self.write_state.lock() = SessionWriteState::default();
            return Ok(());
        }
        Err(format!(
            "failed to abort stale write ticket {}",
            ticket.request_id()
        ))
    }

    pub fn emit_activity(&self, activity: crate::write_coordinator::SessionActivity) {
        self.services
            .writes
            .emit_activity(self.session_id.clone(), activity);
    }

    pub fn project_transaction<T>(&self, operation: impl FnOnce() -> T) -> Result<T, String> {
        let project_id = self
            .current_project_id()
            .ok_or_else(|| "no agent project is open".to_string())?;
        self.services.writes.transaction(&project_id, operation)
    }

    pub fn execute(&self, tool: &str, args: &Value) -> Result<Value, String> {
        let request_id = self.current_request_id().ok_or_else(|| {
            "no agent request is open; tool calls are only valid during a turn".to_string()
        })?;
        self.execute_scoped(&request_id, None, tool, args)
    }

    pub(crate) fn execute_for_run(
        &self,
        identity: &crate::provider_runtime::RunIdentity,
        tool: &str,
        args: &Value,
    ) -> Result<Value, String> {
        if self.session_id != identity.session_id || self.kind != identity.session_kind {
            return Err("stale provider run cannot execute tools".to_string());
        }
        self.execute_scoped(
            &identity.request_id,
            Some(identity.cancellation_generation),
            tool,
            args,
        )
    }

    fn execute_scoped(
        &self,
        expected_request_id: &str,
        expected_generation: Option<u64>,
        tool: &str,
        args: &Value,
    ) -> Result<Value, String> {
        if !self.ask.lock().pending.is_empty() {
            return Err(
                "a user answer is pending; wait for the ask tool to complete before calling another tool"
                    .to_string(),
            );
        }
        let _execution = self.execution_lock.lock();
        let generation_matches = expected_generation.map_or(true, |expected| {
            self.cancellation
                .lock()
                .as_ref()
                .is_some_and(|receiver| *receiver.borrow() == expected)
        });
        let request_id = self
            .current_request_id()
            .filter(|request_id| request_id == expected_request_id && generation_matches)
            .ok_or_else(|| "stale provider run cannot execute tools".to_string())?;
        if self.kind == crate::session::SessionKind::Map {
            tools::validate_map_tool_call(tool, args).map_err(|error| error.to_string())?;
            return self.dispatch_map(&request_id, tool, args);
        }

        if tool == tools::PYTHON_DEPENDENCIES_PREPARE_TOOL && self.owns_write_registration() {
            return Err(
                "Python 의존성 준비는 프로젝트 쓰기 등록을 보유하지 않은 상태에서 실행해야 합니다."
                    .to_string(),
            );
        }

        let effects = tools::tool_spec(tool).ok_or_else(|| format!("Unknown tool `{tool}`."))?;
        if effects.requires_write_workspace && !self.owns_write_registration() {
            return Err(
                "WriteRegistrationRequired: canonical authoring requires runtime-managed write admission."
                    .to_string(),
            );
        }

        {
            let mut state = self.request_state.lock();
            let state = state
                .as_mut()
                .filter(|state| state.request_id == request_id)
                .ok_or_else(|| format!("request state for {request_id} is missing"))?;
            tools::admit_tool_call(state, tool, args).map_err(|error| error.to_string())?;
        }

        let mut result = if tool == tools::PYTHON_DEPENDENCIES_SET_TOOL {
            let token = str_arg(args, "candidateToken")?;
            let project_id = self
                .current_project_id()
                .ok_or_else(|| "현재 에이전트 프로젝트가 열려 있지 않습니다.".to_string())?;
            let claimed = self.services.native().claim_python_dependencies(
                token,
                &self.session_id,
                &project_id,
            )?;
            self.project_transaction(|| {
                self.python_dependencies_set(&request_id, &project_id, claimed)
            })?
        } else if matches!(
            tool,
            tools::MAP_SOUND_IMPORT_TOOL | tools::MAP_SOUND_EDIT_TOOL
        ) {
            if tool == tools::MAP_SOUND_IMPORT_TOOL {
                self.map_sound_import(&request_id, args)
            } else {
                self.map_sound_edit(&request_id, args)
            }
        } else if effects.requires_project_transaction {
            self.project_transaction(|| self.dispatch(&request_id, tool, args))?
        } else {
            self.dispatch(&request_id, tool, args)
        };
        if let Ok(value) = result.as_mut() {
            if tool == tools::SEARCH_DOCS_TOOL {
                self.record_search_docs_result(&request_id, args, value)?;
            } else if tool == tools::DOCS_GET_TOOL {
                self.record_docs_get_result(&request_id, value)?;
            }
        }
        #[cfg(test)]
        if result.is_ok() && effects.requires_write_workspace {
            if let Some(barrier) = self.completion_barrier.lock().take() {
                barrier
                    .reached
                    .send(())
                    .expect("completion observer is waiting");
                barrier
                    .released
                    .recv_timeout(std::time::Duration::from_secs(15))
                    .expect("test releases the completed mutation");
            }
        }
        result
    }

    fn record_search_docs_result(
        &self,
        request_id: &str,
        args: &Value,
        value: &mut Value,
    ) -> Result<(), String> {
        let ids = value
            .get("hits")
            .and_then(Value::as_array)
            .ok_or_else(|| "search_docs returned no hits array".to_string())?
            .iter()
            .map(|hit| {
                hit.get("id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "search_docs hit omitted its id".to_string())
                    .and_then(parse_doc_id)
            })
            .collect::<Result<Vec<_>, _>>()?;

        let mut states = self.request_state.lock();
        let state = states
            .as_mut()
            .filter(|state| state.request_id == request_id)
            .ok_or_else(|| format!("request state for {request_id} is missing"))?;
        let repeated = state.record_search_docs_hits(args, &ids);
        let repeated_count = repeated.iter().filter(|flag| **flag).count();
        let new_count = repeated.len() - repeated_count;

        let hits = value
            .get_mut("hits")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| "search_docs returned no mutable hits array".to_string())?;
        for (hit, repeated) in hits.iter_mut().zip(repeated) {
            hit.as_object_mut()
                .ok_or_else(|| "search_docs returned a non-object hit".to_string())?
                .insert("repeated".to_string(), Value::Bool(repeated));
        }
        let object = value
            .as_object_mut()
            .ok_or_else(|| "search_docs returned a non-object result".to_string())?;
        object.insert("newCount".to_string(), Value::from(new_count));
        object.insert("repeatedCount".to_string(), Value::from(repeated_count));

        let bytes = serde_json::to_vec(value)
            .map_err(|error| format!("failed to measure search_docs result: {error}"))?
            .len();
        state.record_search_docs_payload(bytes);
        eprintln!(
            "eud-agent: search_docs session={} request={} call={} hits={} new={} repeated={} bytes={}",
            self.session_id,
            request_id,
            state.search_docs_count,
            ids.len(),
            new_count,
            repeated_count,
            bytes,
        );
        if state.search_docs_count > 1 && new_count == 0 {
            return Err(
                "search_docs no-progress: this normalized search produced no new stable document ids. Reuse the existing evidence or change the query and filters."
                    .to_string(),
            );
        }
        Ok(())
    }

    fn record_docs_get_result(&self, request_id: &str, value: &Value) -> Result<(), String> {
        let documents = value
            .get("documents")
            .and_then(Value::as_array)
            .ok_or_else(|| "docs_get returned no documents array".to_string())?
            .len();
        let bytes = serde_json::to_vec(value)
            .map_err(|error| format!("failed to measure docs_get result: {error}"))?
            .len();
        let mut states = self.request_state.lock();
        let state = states
            .as_mut()
            .filter(|state| state.request_id == request_id)
            .ok_or_else(|| format!("request state for {request_id} is missing"))?;
        state.record_docs_get(documents, bytes);
        eprintln!(
            "eud-agent: docs_get session={} request={} call={} documents={} bytes={}",
            self.session_id, request_id, state.docs_get_count, documents, bytes,
        );
        Ok(())
    }

    #[cfg(test)]
    fn request_state_snapshot(&self) -> Option<RequestState> {
        self.request_state.lock().clone()
    }

    fn dispatch_map(&self, request_id: &str, tool: &str, args: &Value) -> Result<Value, String> {
        let project_id = self
            .current_project_id()
            .ok_or_else(|| "no Map Agent project is open".to_string())?;
        let candidates = self.services.map_candidates();
        match tool {
            "map_status" => serde_json::to_value(candidates.state(&project_id, &self.session_id)?)
                .map_err(|error| error.to_string()),
            "map_selection_read" => {
                let selection_id = str_arg(args, "selectionId")?;
                let state = candidates.state(&project_id, &self.session_id)?;
                let selection = state
                    .selections
                    .into_iter()
                    .find(|selection| selection.selection.id == selection_id)
                    .ok_or_else(|| format!("selection '{selection_id}' does not exist"))?;
                serde_json::to_value(selection).map_err(|error| error.to_string())
            }
            "map_objects_read" => {
                let layer = str_arg(args, "layer")?;
                let offset = usize_arg_default(args, "offset", 0)?;
                let limit = usize_arg_default(args, "limit", 100)?.min(500);
                let state = candidates.state(&project_id, &self.session_id)?;
                let map = candidates.current_map(&project_id, &self.session_id)?;
                let page = map_objects_page(
                    &map,
                    candidates.context().starcraft_path()?.as_path(),
                    &state.revision_key,
                    &state.baseline.file_sha256,
                    layer,
                    offset,
                    limit,
                )?;
                candidates.annotate_object_page(&project_id, &self.session_id, page)
            }
            "map_render" => {
                let state = candidates.state(&project_id, &self.session_id)?;
                let map = candidates.current_map(&project_id, &self.session_id)?;
                render_map_tool(
                    &map,
                    &state,
                    args,
                    candidates.context().starcraft_path()?.as_path(),
                )
            }
            "map_palette_query" => {
                let state = candidates.state(&project_id, &self.session_id)?;
                let request = map_palette_catalog_request(args, state.baseline.tileset.era())?;
                let result = isom::catalog_query(
                    &candidates.context().starcraft_path()?,
                    request.to_string().as_bytes(),
                )
                .map_err(|error| error.to_string())?;
                let value: Value =
                    serde_json::from_str(&result).map_err(|error| error.to_string())?;
                enforce_map_palette_result_bound(value)
            }
            "map_tile_info" => {
                let state = candidates.state(&project_id, &self.session_id)?;
                let tile_id = usize_arg_default(args, "tileId", usize::MAX)?;
                let request = json!({
                    "schema": "eud-map-catalog/1",
                    "kind": "tiles",
                    "tileset": state.baseline.tileset.era(),
                    "offset": tile_id,
                    "limit": 1,
                });
                let result = isom::catalog_query(
                    &candidates.context().starcraft_path()?,
                    request.to_string().as_bytes(),
                )
                .map_err(|error| error.to_string())?;
                let value: Value =
                    serde_json::from_str(&result).map_err(|error| error.to_string())?;
                value["entries"]
                    .as_array()
                    .and_then(|entries| entries.first())
                    .cloned()
                    .ok_or_else(|| format!("tile {tile_id} does not exist in this tileset"))
            }
            "map_analyze" | "map_candidate_diff" => {
                let state = candidates.state(&project_id, &self.session_id)?;
                let revision = state
                    .revisions
                    .iter()
                    .find(|revision| revision.revision == state.current_revision);
                Ok(json!({
                    "candidateRevision": state.current_revision,
                    "stale": state.stale,
                    "diff": revision.map(|revision| &revision.diff),
                    "verification": revision.map(|revision| &revision.verification),
                }))
            }
            "map_draft_begin" => candidates.draft_begin(&project_id, &self.session_id, request_id),
            "map_stamp_preview" => {
                let input: crate::map_stamp::StampPreviewInput =
                    serde_json::from_value(args.clone())
                        .map_err(|error| format!("invalid map_stamp_preview arguments: {error}"))?;
                let source = candidates.normalize_stamp_tool_source(
                    &project_id,
                    &self.session_id,
                    request_id,
                    &input.source,
                )?;
                serde_json::to_value(candidates.draft_stamp_preview(
                    &project_id,
                    &self.session_id,
                    request_id,
                    &source,
                    &input.destinations,
                )?)
                .map_err(|error| error.to_string())
            }
            "map_stamp_place" => {
                let input: crate::map_stamp::StampPlaceInput = serde_json::from_value(args.clone())
                    .map_err(|error| format!("invalid map_stamp_place arguments: {error}"))?;
                let source = candidates.normalize_stamp_tool_source(
                    &project_id,
                    &self.session_id,
                    request_id,
                    &input.source,
                )?;
                serde_json::to_value(candidates.draft_stamp_place(
                    &project_id,
                    &self.session_id,
                    request_id,
                    &source,
                    &input.destinations,
                    input.collision_policy,
                )?)
                .map_err(|error| error.to_string())
            }
            "map_draft_patch" => {
                let operations: Vec<crate::map_model::MapOperation> = serde_json::from_value(
                    args.get("operations")
                        .cloned()
                        .ok_or_else(|| "map_draft_patch requires operations".to_string())?,
                )
                .map_err(|error| format!("invalid map draft operations: {error}"))?;
                candidates.draft_patch(&project_id, &self.session_id, request_id, operations)
            }
            "map_image_place" => {
                let input: crate::map_image::MapImagePlaceInput =
                    serde_json::from_value(args.clone())
                        .map_err(|error| format!("invalid map_image_place arguments: {error}"))?;
                let binding = self.map_image_binding(request_id, &input.image_ref)?;
                if binding.session_id != self.session_id || binding.request_id != request_id {
                    return Err(
                        "imageRef belongs to another Map Agent session or request".to_string()
                    );
                }
                let state = candidates.state(&project_id, &self.session_id)?;
                if binding.candidate_revision_key != state.revision_key
                    || binding.baseline_hash != state.baseline.file_sha256
                {
                    return Err("imageRef belongs to another candidate revision".to_string());
                }
                if !candidates.request_has_draft(&self.session_id, request_id)? {
                    candidates.draft_begin(&project_id, &self.session_id, request_id)?;
                }
                let (authority, expected_revision, draft) =
                    candidates.image_request_context(&project_id, &self.session_id, request_id)?;
                let starcraft_path = candidates.context().starcraft_path()?;
                let conversion = self.services.map_images.convert(
                    &self.session_id,
                    &binding.attachment,
                    input.placement(),
                    crate::map_image::MapImageMapContext {
                        map_path: &draft,
                        revision: &expected_revision,
                        authority: &authority,
                        starcraft_path: &starcraft_path,
                    },
                )?;
                if conversion.report.protected_conflicts != 0 {
                    return Err(format!(
                        "map_image_place changes {} protected terrain cell(s)",
                        conversion.report.protected_conflicts
                    ));
                }
                if conversion.report.outside_authority_conflicts != 0 {
                    return Err(format!(
                        "map_image_place changes {} cell(s) outside the current terrain authority",
                        conversion.report.outside_authority_conflicts
                    ));
                }
                let report = conversion.report;
                let patch = candidates.draft_patch_image(
                    &project_id,
                    &self.session_id,
                    request_id,
                    conversion.operation,
                    conversion.metadata,
                )?;
                Ok(json!({
                    "ok": true,
                    "imageRef": input.image_ref,
                    "report": report,
                    "draft": patch,
                }))
            }
            "map_draft_render" => {
                let state = candidates.state(&project_id, &self.session_id)?;
                let map = candidates.draft_map(&self.session_id, request_id)?;
                render_map_tool(
                    &map,
                    &state,
                    args,
                    candidates.context().starcraft_path()?.as_path(),
                )
            }
            "map_draft_analyze" => serde_json::to_value(candidates.draft_analyze(
                &project_id,
                &self.session_id,
                request_id,
            )?)
            .map_err(|error| error.to_string()),
            "map_draft_reset" => candidates.draft_reset(&project_id, &self.session_id, request_id),
            "map_candidate_finalize" => serde_json::to_value(candidates.finalize(
                &project_id,
                &self.session_id,
                request_id,
            )?)
            .map_err(|error| error.to_string()),
            _ => Err(format!("unknown Map Agent tool '{tool}'")),
        }
    }

    fn dispatch(&self, request_id: &str, tool: &str, args: &Value) -> Result<Value, String> {
        match tool {
            // ---- read tools (no journal) ----
            "project_status" => {
                let project = self.services.native().open()?;
                let status = project.status()?;
                Ok(json!({
                    "status": {
                        "name": status.name,
                        "root": status.root,
                        "sourceMap": status.source_map,
                        "outputMap": status.output_map,
                        "revision": status.revision,
                    },
                    "mainFile": status.main_file,
                    "pythonEntrypoints": project.manifest().python_entrypoints.clone(),
                    "pythonDependencies": project.manifest().python_dependencies.clone(),
                    "pythonLock": project.manifest().python_lock.clone(),
                }))
            }
            "list_files" => {
                let files = self.services.native().list_files()?;
                let items: Vec<Value> = files
                    .into_iter()
                    .map(|file| {
                        json!({ "path": file.path, "ftype": file.file_type, "settable": file.settable })
                    })
                    .collect();
                Ok(json!({ "count": items.len(), "files": items }))
            }
            "read_file" => {
                let path = str_arg(args, "path")?;
                let content = self.services.native().read_source(path)?;
                ranged_file_result(path, &content, args)
            }
            tools::SOURCE_SEARCH_TOOL => self.source_search(args),
            tools::PYTHON_DEPENDENCIES_PREPARE_TOOL => {
                let values = array_arg(args, "dependencies")?;
                let dependencies = values
                    .iter()
                    .map(|value| {
                        value.as_str().map(str::to_string).ok_or_else(|| {
                            "dependencies에는 문자열만 사용할 수 있습니다.".to_string()
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let project_id = self
                    .current_project_id()
                    .ok_or_else(|| "현재 에이전트 프로젝트가 열려 있지 않습니다.".to_string())?;
                let bridge = self
                    .cancellation
                    .lock()
                    .clone()
                    .map(ProcessCancellationBridge::new);
                serde_json::to_value(
                    self.services
                        .native()
                        .prepare_python_dependencies_with_cancellation(
                            &self.session_id,
                            &project_id,
                            dependencies,
                            bridge.as_ref().map(ProcessCancellationBridge::token),
                        )?,
                )
                .map_err(|error| {
                    format!("Python 의존성 준비 결과를 직렬화하지 못했습니다: {error}")
                })
            }
            "dat_get" => {
                let items = array_arg(args, "items")?;
                let mut targets = Vec::with_capacity(items.len());
                let mut metadata = Vec::with_capacity(items.len());
                for item in items {
                    let (dat, param, obj_id) = (
                        str_arg(item, "dat")?,
                        str_arg(item, "param")?,
                        i64_arg(item, "objId")?,
                    );
                    let object_id = u32::try_from(obj_id)
                        .map_err(|_| "objId must be a non-negative integer".to_string())?;
                    targets.push(DatTarget::Dat {
                        dat: dat.to_string(),
                        object_id,
                        field: param.to_string(),
                    });
                    metadata.push((dat, param, obj_id));
                }
                let values = self.services.native().dat_values(&targets)?;
                let results: Vec<_> = targets
                    .iter()
                    .zip(metadata)
                    .map(|(target, (dat, param, obj_id))| {
                        json!({
                            "dat": dat,
                            "param": param,
                            "objId": obj_id,
                            "ok": true,
                            "value": dat_scalar_json(&values[target]),
                        })
                    })
                    .collect();
                Ok(json!({"count": results.len(), "results": results}))
            }
            "xdat_get" => {
                let items = array_arg(args, "items")?;
                let mut targets = Vec::with_capacity(items.len());
                let mut metadata = Vec::with_capacity(items.len());
                for item in items {
                    let (dat, name, obj_id) = (
                        str_arg(item, "dat")?,
                        str_arg(item, "name")?,
                        i64_arg(item, "objId")?,
                    );
                    let object_id = u32::try_from(obj_id)
                        .map_err(|_| "objId must be a non-negative integer".to_string())?;
                    targets.push(DatTarget::Xdat {
                        dat: dat.to_string(),
                        object_id,
                        field: name.to_string(),
                    });
                    metadata.push((dat, name, obj_id));
                }
                let values = self.services.native().dat_values(&targets)?;
                let results: Vec<_> = targets
                    .iter()
                    .zip(metadata)
                    .map(|(target, (dat, name, obj_id))| {
                        json!({
                            "dat": dat,
                            "name": name,
                            "objId": obj_id,
                            "ok": true,
                            "value": dat_scalar_json(&values[target]),
                        })
                    })
                    .collect();
                Ok(json!({"count": results.len(), "results": results}))
            }
            "tbl_get" => {
                let items = array_arg(args, "items")?;
                let targets: Result<Vec<_>, String> = items
                    .iter()
                    .map(|item| {
                        u32::try_from(i64_arg(item, "index")?)
                            .map(DatTarget::Tbl)
                            .map_err(|_| "index must be a non-negative integer".to_string())
                    })
                    .collect();
                let targets = targets?;
                let values = self.services.native().dat_values(&targets)?;
                let results: Vec<_> = targets
                    .iter()
                    .map(|target| match target {
                        DatTarget::Tbl(index) => json!({
                            "index": index,
                            "ok": true,
                            "value": dat_scalar_json(&values[target]),
                        }),
                        _ => unreachable!(),
                    })
                    .collect();
                Ok(json!({"count": results.len(), "results": results}))
            }
            "req_get" => {
                let items = array_arg(args, "items")?;
                let mut targets = Vec::with_capacity(items.len());
                for item in items {
                    let dat = str_arg(item, "dat")?;
                    let object_id = u32::try_from(i64_arg(item, "objId")?)
                        .map_err(|_| "objId must be a non-negative integer".to_string())?;
                    targets.push(DatTarget::Requirement {
                        dat: dat.to_string(),
                        object_id,
                    });
                }
                let values = self.services.native().dat_values(&targets)?;
                let results: Vec<_> = targets
                    .iter()
                    .map(|target| match target {
                        DatTarget::Requirement { dat, object_id } => json!({
                            "dat": dat,
                            "objId": object_id,
                            "ok": true,
                            "value": dat_scalar_json(&values[target]),
                        }),
                        _ => unreachable!(),
                    })
                    .collect();
                Ok(json!({"count": results.len(), "results": results}))
            }
            "btn_get" => {
                let items = array_arg(args, "items")?;
                let targets: Result<Vec<_>, String> = items
                    .iter()
                    .map(|item| {
                        u32::try_from(i64_arg(item, "setId")?)
                            .map(DatTarget::Button)
                            .map_err(|_| "setId must be a non-negative integer".to_string())
                    })
                    .collect();
                let targets = targets?;
                let values = self.services.native().dat_values(&targets)?;
                let results: Vec<_> = targets
                    .iter()
                    .map(|target| match target {
                        DatTarget::Button(set_id) => json!({
                            "setId": set_id,
                            "ok": true,
                            "csv": dat_scalar_json(&values[target]),
                        }),
                        _ => unreachable!(),
                    })
                    .collect();
                Ok(json!({"count": results.len(), "results": results}))
            }
            "settings_get" => {
                let (scope, key) = (str_arg(args, "scope")?, str_arg(args, "key")?);
                let config = self
                    .services
                    .dirs
                    .load_config()
                    .map_err(|error| error.to_string())?;
                let project = self.services.native().open()?;
                let value = match (scope, key) {
                    ("project", "OpenMapName") => project.manifest().source_map.clone(),
                    ("project", "SaveMapName") => project.manifest().output_map.clone(),
                    ("project", "UseCustomtbl") => {
                        project.manifest().settings.use_custom_tbl.to_string()
                    }
                    ("program", "euddraft") => config.euddraft_path,
                    ("program", "starcraft") => config.starcraft_path,
                    _ => return Err(format!("unsupported native setting {scope}.{key}")),
                };
                Ok(json!({ "value": value }))
            }
            "plugins_list" => {
                let plugins = self.services.native().open()?.manifest().plugins.clone();
                Ok(json!({ "plugins": plugins }))
            }
            tools::MAP_INFO_TOOL => {
                let map_path = self.services.native().source_map_path()?;
                tools::map_info_path(&map_path, args).map_err(stringify)
            }
            tools::MAP_MINIMAP_TOOL => {
                let map_path = self.services.native().source_map_path()?;
                let configured = args
                    .get("starcraftPath")
                    .and_then(Value::as_str)
                    .map(PathBuf::from)
                    .unwrap_or_else(|| {
                        PathBuf::from(
                            self.services
                                .dirs
                                .load_config()
                                .map(|config| config.starcraft_path)
                                .unwrap_or_default(),
                        )
                    });
                tools::map_minimap_path(&map_path, &configured, args).map_err(stringify)
            }
            tools::MAP_SOUND_LIST_TOOL => self.map_sound_list(request_id),
            tools::SEARCH_DOCS_TOOL => Ok(self.search_docs(args)),
            tools::DOCS_GET_TOOL => self.docs_get(args),

            // ---- write tools (journaled) ----
            "dat_patch" => {
                let changes: Vec<NativeDatChange> = serde_json::from_value(
                    args.get("changes")
                        .cloned()
                        .ok_or_else(|| "missing argument 'changes'".to_string())?,
                )
                .map_err(|error| format!("invalid dat_patch changes: {error}"))?;
                let patch = NativeDatPatch {
                    changes: changes.clone(),
                };
                let revision = self.services.native().apply_dat_patch(&patch)?;
                for change in changes {
                    self.record_native_dat_change(request_id, &change)?;
                }
                Ok(json!({
                    "ok": true,
                    "changeCount": patch.changes.len(),
                    "revision": revision,
                }))
            }
            "file_create" => self.file_create(request_id, args),
            "file_write" => self.file_write(request_id, args),
            "file_edit" => self.file_edit(request_id, args),
            "file_rename" => self.file_rename(request_id, args),
            "file_delete" => self.file_delete(request_id, args),
            "file_move" => self.file_move(request_id, args),
            "mkdir" => self.mkdir(request_id, args),
            "set_main" => self.set_main(request_id, args),
            "settings_set" => self.settings_set(request_id, args),
            "plugin_add" => self.plugin_add(request_id, args),
            "plugin_edit" => self.plugin_edit(request_id, args),
            "plugin_remove" => self.plugin_remove(request_id, args),
            "plugin_move" => self.plugin_move(request_id, args),
            tools::BUILD_RUN_TOOL => {
                let project_id = self
                    .current_project_id()
                    .ok_or_else(|| "현재 에이전트 프로젝트가 열려 있지 않습니다.".to_string())?;
                let input_revision = self.services.native().open()?.revision()?;
                {
                    let state = self.request_state.lock();
                    let state = state
                        .as_ref()
                        .filter(|state| state.request_id == request_id)
                        .ok_or_else(|| format!("request state for {request_id} is missing"))?;
                    if state.build_blocked_for_revision(&input_revision) {
                        return Err(format!(
                            "build_run no-progress: revision `{input_revision}` is blocked until a canonical input change produces a new revision."
                        ));
                    }
                }
                let bridge = self
                    .cancellation
                    .lock()
                    .clone()
                    .map(ProcessCancellationBridge::new);
                let result = self.services.native().build_with_cancellation(
                    &project_id,
                    bridge.as_ref().map(ProcessCancellationBridge::token),
                )?;
                *self.sound_build_required.lock() = false;
                *self.last_build.lock() = Some(crate::harness::BuildEvidence {
                    ok: result.ok,
                    error_count: result.errors.len(),
                });
                let progress = {
                    let mut state = self.request_state.lock();
                    let state = state
                        .as_mut()
                        .filter(|state| state.request_id == request_id)
                        .ok_or_else(|| format!("request state for {request_id} is missing"))?;
                    state.record_build(input_revision, &result)
                };
                let mut value = serde_json::to_value(result)
                    .map_err(|error| format!("failed to serialize build result: {error}"))?;
                let (progress, no_progress_reason) = match progress {
                    tools::BuildProgressOutcome::Progress(progress) => (progress, None),
                    tools::BuildProgressOutcome::NoProgress { progress, reason } => {
                        (progress, Some(reason))
                    }
                };
                value
                    .as_object_mut()
                    .ok_or_else(|| "failed to serialize build result object".to_string())?
                    .insert(
                        "buildProgress".to_string(),
                        json!({
                            "inputRevision": progress.input_revision,
                            "diagnosticsFingerprint": progress.diagnostics_fingerprint,
                            "errorCount": progress.error_count,
                            "success": progress.success,
                            "consecutiveNoProgress": progress.consecutive_no_progress,
                        }),
                    );
                *self.last_build_result.lock() = Some(value.clone());
                if let Some(reason) = no_progress_reason {
                    return Err(reason);
                }
                Ok(value)
            }
            "location_write" => {
                let map_path = self.services.native().source_map_path()?;
                let chk = isom::chk_extract(&map_path).map_err(|error| error.to_string())?;
                tools::location_write_apply(
                    &self.services.map_safe,
                    &self.services.journal,
                    request_id,
                    &map_path,
                    &chk,
                    args,
                    epoch_secs(),
                )
                .map_err(stringify)
            }
            "player_setup" => {
                let map_path = self.services.native().source_map_path()?;
                tools::player_setup_apply(
                    &self.services.map_safe,
                    &self.services.journal,
                    request_id,
                    &map_path,
                    args,
                    epoch_secs(),
                )
                .map_err(stringify)
            }
            tools::SWITCH_WRITE_TOOL => {
                let map_path = self.services.native().source_map_path()?;
                let chk = isom::chk_extract(&map_path).map_err(|error| error.to_string())?;
                tools::switch_write_apply(
                    &self.services.map_safe,
                    &self.services.journal,
                    request_id,
                    &map_path,
                    &chk,
                    args,
                    epoch_secs(),
                )
                .map_err(stringify)
            }
            "propose_plan" => {
                let markdown = str_arg(args, "markdown")?.to_string();
                *self.pending_plan.lock() = Some((request_id.to_owned(), markdown));
                Ok(json!({
                    "ok": true,
                    "note": "Plan recorded for user review. Stop this turn now and wait for the user to approve before applying any change."
                }))
            }
            other => Err(format!("unknown tool '{other}'")),
        }
    }

    // ---- write-tool helpers ----
    fn map_sound_list(&self, request_id: &str) -> Result<Value, String> {
        let project_id = self
            .request
            .lock()
            .as_ref()
            .filter(|request| request.request_id == request_id)
            .map(|request| request.project_id.clone())
            .ok_or_else(|| format!("request {request_id} is not active"))?;
        let map_path = self.services.native().source_map_path()?;
        let chk = isom::chk_extract(&map_path)
            .map_err(|_| "저장된 맵의 사운드 목록을 읽을 수 없습니다.".to_string())?;
        let assets = map_asset_inventory(&map_path)?;
        let sounds = crate::chk::parse_sounds(&chk)
            .into_iter()
            .take(512)
            .map(|sound| {
                let asset_sha256 = assets.get(&sound.mpq_path.to_ascii_lowercase()).cloned();
                let managed = managed_sound_hash(&sound.mpq_path).is_some();
                let source = if managed {
                    self.services
                        .audio
                        .source_record(&project_id, &sound.mpq_path)?
                } else {
                    None
                };
                Ok(json!({
                    "soundIndex": sound.sound_index,
                    "mpqPath": sound.mpq_path,
                    "assetPresent": asset_sha256.is_some(),
                    "assetSha256": asset_sha256,
                    "managed": managed,
                    "sourceAvailable": source.is_some(),
                    "sourceName": source.as_ref().map(|source| source.source_display_name.clone()),
                    "originalDurationMs": source.as_ref().map(|source| source.source_duration_ms),
                    "volumePercent": source.as_ref().map(|source| source.effects.volume_percent),
                    "fadeInMs": source.as_ref().map(|source| source.effects.fade_in_ms),
                    "fadeOutMs": source.as_ref().map(|source| source.effects.fade_out_ms),
                }))
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(json!({"sounds": sounds}))
    }

    fn map_sound_import(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let project_id = self
            .request
            .lock()
            .as_ref()
            .filter(|request| request.request_id == request_id)
            .map(|request| request.project_id.clone())
            .ok_or_else(|| format!("request {request_id} is not active"))?;
        let audio_ref = str_arg(args, "audioRef")?;
        let binding = self.audio_binding(request_id, audio_ref)?;
        let map_path = self.services.native().source_map_path()?;
        let expected_map_sha256 = crate::bootstrap::sha256_file(&map_path)
            .map_err(|_| "저장된 원본 맵을 읽을 수 없습니다.".to_string())?;
        self.emit_progress(
            crate::ipc::ProgressStage::AudioTranscode,
            "canonical OGG Vorbis로 변환하고 있습니다.",
        );
        let cancellation = self.cancellation.lock().clone();
        let normalized = self
            .services
            .audio
            .normalize(&binding, cancellation.as_ref())?;
        self.emit_progress(
            crate::ipc::ProgressStage::AudioValidate,
            "canonical OGG profile을 검증했습니다.",
        );
        let ogg_bytes = std::fs::read(&normalized.path)
            .map_err(|_| "검증된 OGG cache를 읽을 수 없습니다.".to_string())?;
        if ogg_bytes.len() as u64 != normalized.bytes
            || !ogg_bytes.starts_with(b"OggS")
            || format!("{:x}", Sha256::digest(&ogg_bytes)) != normalized.sha256
        {
            return Err("검증된 OGG cache invariant가 변경되었습니다.".to_string());
        }
        let mpq_path = select_managed_sound_path(&map_path, &normalized.sha256)?;
        self.services
            .audio
            .remember_import(&project_id, &mpq_path, &binding)?;
        self.emit_progress(
            crate::ipc::ProgressStage::MapSoundWrite,
            "저장된 SCX에 MPQ asset, game string, WAV slot을 등록하고 있습니다.",
        );

        let operation = self.project_transaction(|| {
            let write = match self.services.map_safe.write_sound(
                &map_path,
                &expected_map_sha256,
                &mpq_path,
                &ogg_bytes,
            ) {
                Ok(write) => write,
                Err(crate::mapsafe::MapSafeError::Compiling) => {
                    return Err("euddraft 빌드 중이므로 맵 사운드를 추가할 수 없습니다.".to_string())
                }
                Err(crate::mapsafe::MapSafeError::MapLocked(_)) => {
                    self.emit_progress(
                        crate::ipc::ProgressStage::WaitingMapClose,
                        "SCMDraft에서 현재 맵을 저장하고 닫은 뒤 다시 시도해 주세요.",
                    );
                    return Err(
                        "SCMDraft에서 현재 맵을 저장하고 닫은 뒤 다시 시도해 주세요.".to_string(),
                    );
                }
                Err(crate::mapsafe::MapSafeError::StaleSource { .. }) => {
                    return Err("저장된 원본 맵이 오디오 변환 중 변경되었습니다.".to_string())
                }
                Err(crate::mapsafe::MapSafeError::PostVerifyRestored { .. }) => {
                    return Err(
                        "맵 사운드 저장 후 검증에 실패해 원본 backup을 복원했습니다.".to_string(),
                    )
                }
                Err(crate::mapsafe::MapSafeError::Rollback { .. }) => {
                    self.mark_write_hazard("map sound post-verify rollback failed");
                    return Err(
                        "맵 사운드 rollback에 실패했습니다. write lease와 backup을 유지합니다."
                            .to_string(),
                    );
                }
                Err(crate::mapsafe::MapSafeError::Apply(detail)) => {
                    let message = if detail.contains("512 WAV") {
                        "맵의 WAV 슬롯 512개가 모두 사용 중입니다."
                    } else if detail.contains("different bytes") {
                        "기존 MPQ sound path에 다른 bytes가 있습니다."
                    } else if detail.contains("partial state") {
                        "기존 맵 사운드의 MPQ/string/WAV 상태가 불완전합니다."
                    } else {
                        "native 맵 사운드 등록에 실패했습니다."
                    };
                    return Err(message.to_string());
                }
                Err(crate::mapsafe::MapSafeError::Verify { .. }) => {
                    return Err("맵 사운드 저장 후 검증에 실패했습니다.".to_string())
                }
                Err(crate::mapsafe::MapSafeError::InsufficientDisk { .. }) => {
                    return Err("맵 사운드 등록에 필요한 디스크 공간이 부족합니다.".to_string())
                }
                Err(crate::mapsafe::MapSafeError::Io(_))
                | Err(crate::mapsafe::MapSafeError::BackupNotFound(_)) => {
                    return Err("맵 backup 또는 atomic replace에 실패했습니다.".to_string())
                }
            };
            if write.report.reused {
                return Ok(write);
            }
            let seq = self.services.journal.entry_count(request_id) as u64 + 1;
            let native_report = serde_json::to_vec(&write.report)
                .map_err(|_| "native sound report를 직렬화할 수 없습니다.".to_string())?;
            let entry = JournalEntry {
                id: format!("sound-{seq}"),
                seq,
                tool: WriteTool::MapSound,
                target: JournalTarget::MapSound {
                    source_map: map_path.clone(),
                    mpq_path: mpq_path.clone(),
                    normalized_sha256: normalized.sha256.clone(),
                },
                before: Snapshot::MapBackup {
                    map_path: map_path.to_string_lossy().into_owned(),
                    backup_path: write.backup_path.to_string_lossy().into_owned(),
                },
                after: Snapshot::MapSound {
                    source_sha256: binding.source_sha256.clone(),
                    source_codec: binding.probe.codec.clone(),
                    duration_ms: normalized.duration_ms,
                    channels: binding.probe.channels,
                    sample_rate: binding.probe.sample_rate,
                    normalization_profile: format!(
                        "{};ogg/vorbis/44100/stereo/q4",
                        normalized.profile_version
                    ),
                    normalized_sha256: normalized.sha256.clone(),
                    normalized_bytes: normalized.bytes,
                    mpq_path: mpq_path.clone(),
                    wav_index: write.report.sound_index,
                    string_id: write.report.sound_string_id,
                    map_sha256_before: write.report.input_sha256.clone(),
                    map_sha256_after: write.report.output_sha256.clone(),
                    backup_path: write.backup_path.clone(),
                    native_report_sha256: format!("{:x}", Sha256::digest(&native_report)),
                    map_bytes_before: write.map_bytes_before,
                    map_bytes_after: write.map_bytes_after,
                    source_display_name: binding.descriptor.name.clone(),
                    edit: None,
                },
                ts: epoch_secs(),
            };
            if let Err(error) = self.services.journal.record(request_id, entry) {
                let restore = self
                    .services
                    .map_safe
                    .restore(&crate::mapsafe::JournalEntry {
                        map_path: map_path.clone(),
                        backup_path: write.backup_path.clone(),
                    });
                let restored_exactly = crate::bootstrap::sha256_file(&map_path)
                    .is_ok_and(|hash| hash == expected_map_sha256);
                if restore.is_err() || !restored_exactly {
                    self.mark_write_hazard(
                        "map sound journal record failed and rollback did not settle",
                    );
                }
                return Err(format!("맵 사운드 journal 기록에 실패했습니다: {error}"));
            }
            *self.sound_build_required.lock() = true;
            Ok(write)
        })?;
        let write = operation?;
        let sound_ref = self.next_sound_ref(request_id)?;
        self.emit_progress(
            crate::ipc::ProgressStage::MapSoundVerify,
            "SCX sound asset과 WAV slot 저장 검증을 완료했습니다.",
        );
        let map_size_delta = i128::from(write.map_bytes_after) - i128::from(write.map_bytes_before);
        Ok(json!({
            "soundRef": sound_ref,
            "mpqPath": write.report.mpq_path,
            "durationMs": normalized.duration_ms,
            "normalizedBytes": normalized.bytes,
            "sourceCodec": normalized.source_codec,
            "outputCodec": "vorbis",
            "reused": write.report.reused,
            "mapSha256Before": write.report.input_sha256,
            "mapSha256After": write.report.output_sha256,
            "mapSizeDelta": map_size_delta,
        }))
    }

    fn map_sound_edit(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let project_id = self
            .request
            .lock()
            .as_ref()
            .filter(|request| request.request_id == request_id)
            .map(|request| request.project_id.clone())
            .ok_or_else(|| format!("request {request_id} is not active"))?;
        let old_mpq_path = str_arg(args, "mpqPath")?;
        if managed_sound_hash(old_mpq_path).is_none() {
            return Err("eud-agent가 관리하는 MPQ 사운드 경로만 편집할 수 있습니다.".to_string());
        }
        let volume_percent = optional_u64_arg(args, "volumePercent")?;
        if volume_percent.is_some_and(|value| value > u64::from(crate::audio::MAX_VOLUME_PERCENT)) {
            return Err(format!(
                "volumePercent는 0~{} 범위여야 합니다.",
                crate::audio::MAX_VOLUME_PERCENT
            ));
        }
        let fade_in_ms = optional_u64_arg(args, "fadeInMs")?;
        let fade_out_ms = optional_u64_arg(args, "fadeOutMs")?;
        if fade_in_ms
            .into_iter()
            .chain(fade_out_ms)
            .any(|value| value > crate::audio::MAX_AUDIO_DURATION_MS)
        {
            return Err("fade 시간은 오디오 최대 길이를 초과할 수 없습니다.".to_string());
        }
        let patch = crate::audio::AudioEditPatch {
            volume_percent: volume_percent.map(|value| value as u16),
            fade_in_ms,
            fade_out_ms,
        };
        if patch.is_empty() {
            return Err(
                "volumePercent, fadeInMs, fadeOutMs 중 하나를 지정해야 합니다.".to_string(),
            );
        }

        let map_path = self.services.native().source_map_path()?;
        let expected_map_sha256 = crate::bootstrap::sha256_file(&map_path)
            .map_err(|_| "저장된 원본 맵을 읽을 수 없습니다.".to_string())?;
        let inventory = map_asset_inventory(&map_path)?;
        let old_asset_sha256 = inventory
            .get(&old_mpq_path.to_ascii_lowercase())
            .cloned()
            .ok_or_else(|| "편집할 관리형 MPQ 사운드 자산이 맵에 없습니다.".to_string())?;
        let chk = isom::chk_extract(&map_path)
            .map_err(|_| "저장된 맵의 사운드 목록을 읽을 수 없습니다.".to_string())?;
        if crate::chk::parse_sounds(&chk)
            .iter()
            .filter(|sound| sound.mpq_path == old_mpq_path)
            .count()
            != 1
        {
            return Err("편집할 MPQ 사운드의 WAV 등록이 하나가 아닙니다.".to_string());
        }

        let supplied_audio_ref = args.get("audioRef").and_then(Value::as_str);
        let existing_source = self
            .services
            .audio
            .source_record(&project_id, old_mpq_path)?;
        if existing_source.is_some() && supplied_audio_ref.is_some() {
            return Err(
                "이미 프로젝트 원본이 있는 사운드에는 audioRef를 다시 지정할 수 없습니다."
                    .to_string(),
            );
        }
        if existing_source.is_none() {
            let audio_ref = supplied_audio_ref.ok_or_else(|| {
                "이 사운드의 프로젝트 원본이 없습니다. 기존 등록에 사용한 원본을 다시 첨부해 주세요."
                    .to_string()
            })?;
            let binding = self.audio_binding(request_id, audio_ref)?;
            let cancellation = self.cancellation.lock().clone();
            let normalized = self
                .services
                .audio
                .normalize(&binding, cancellation.as_ref())?;
            if normalized.sha256 != old_asset_sha256 {
                return Err(
                    "첨부한 원본을 기본 변환한 결과가 기존 등록 사운드와 일치하지 않습니다."
                        .to_string(),
                );
            }
            self.services
                .audio
                .remember_import(&project_id, old_mpq_path, &binding)?;
        }

        self.emit_progress(
            crate::ipc::ProgressStage::AudioTranscode,
            "프로젝트 원본에 볼륨과 페이드 설정을 적용하고 있습니다.",
        );
        let request_temp = self.services.audio.request_temp()?;
        let cancellation = self.cancellation.lock().clone();
        let edited = self.services.audio.render_edit(
            &project_id,
            old_mpq_path,
            patch,
            request_temp.as_ref(),
            cancellation.as_ref(),
        )?;
        self.emit_progress(
            crate::ipc::ProgressStage::AudioValidate,
            "편집된 canonical OGG profile을 검증했습니다.",
        );
        let ogg_bytes = std::fs::read(&edited.normalized.path)
            .map_err(|_| "검증된 편집 OGG cache를 읽을 수 없습니다.".to_string())?;
        if ogg_bytes.len() as u64 != edited.normalized.bytes
            || !ogg_bytes.starts_with(b"OggS")
            || format!("{:x}", Sha256::digest(&ogg_bytes)) != edited.normalized.sha256
        {
            return Err("검증된 편집 OGG cache invariant가 변경되었습니다.".to_string());
        }
        if edited.normalized.sha256 == old_asset_sha256 {
            return Err("편집 결과가 현재 맵 사운드 bytes와 같습니다.".to_string());
        }
        let new_mpq_path = select_managed_sound_replacement_path_from_inventory(
            &inventory,
            old_mpq_path,
            &edited.normalized.sha256,
        )?;
        self.services
            .audio
            .remember_edit(&project_id, &new_mpq_path, &edited)?;
        self.emit_progress(
            crate::ipc::ProgressStage::MapSoundWrite,
            "SCX의 기존 MPQ asset, game string, WAV 등록을 편집본으로 교체하고 있습니다.",
        );

        let operation = self.project_transaction(|| {
            let write = match self.services.map_safe.replace_sound(
                &map_path,
                &expected_map_sha256,
                old_mpq_path,
                &new_mpq_path,
                &ogg_bytes,
            ) {
                Ok(write) => write,
                Err(crate::mapsafe::MapSafeError::Compiling) => {
                    return Err("euddraft 빌드 중이므로 맵 사운드를 교체할 수 없습니다.".to_string())
                }
                Err(crate::mapsafe::MapSafeError::MapLocked(_)) => {
                    self.emit_progress(
                        crate::ipc::ProgressStage::WaitingMapClose,
                        "SCMDraft에서 현재 맵을 저장하고 닫은 뒤 다시 시도해 주세요.",
                    );
                    return Err(
                        "SCMDraft에서 현재 맵을 저장하고 닫은 뒤 다시 시도해 주세요.".to_string(),
                    );
                }
                Err(crate::mapsafe::MapSafeError::StaleSource { .. }) => {
                    return Err("저장된 원본 맵이 오디오 편집 중 변경되었습니다.".to_string())
                }
                Err(crate::mapsafe::MapSafeError::PostVerifyRestored { .. }) => {
                    return Err(
                        "맵 사운드 교체 검증에 실패해 원본 backup을 복원했습니다.".to_string()
                    )
                }
                Err(crate::mapsafe::MapSafeError::Rollback { .. }) => {
                    self.mark_write_hazard("map sound replacement rollback failed");
                    return Err(
                        "맵 사운드 교체 rollback에 실패했습니다. write lease와 backup을 유지합니다."
                            .to_string(),
                    );
                }
                Err(crate::mapsafe::MapSafeError::Apply(detail)) => {
                    let message = if detail.contains("source") {
                        "기존 맵 사운드의 MPQ/string/WAV 등록이 완전하지 않습니다."
                    } else if detail.contains("destination") {
                        "편집본 MPQ sound path가 이미 사용 중입니다."
                    } else {
                        "native 맵 사운드 교체에 실패했습니다."
                    };
                    return Err(message.to_string());
                }
                Err(crate::mapsafe::MapSafeError::Verify { .. }) => {
                    return Err("맵 사운드 교체 후 검증에 실패했습니다.".to_string())
                }
                Err(crate::mapsafe::MapSafeError::InsufficientDisk { .. }) => {
                    return Err("맵 사운드 교체에 필요한 디스크 공간이 부족합니다.".to_string())
                }
                Err(crate::mapsafe::MapSafeError::Io(_))
                | Err(crate::mapsafe::MapSafeError::BackupNotFound(_)) => {
                    return Err("맵 backup 또는 atomic replace에 실패했습니다.".to_string())
                }
            };
            let seq = self.services.journal.entry_count(request_id) as u64 + 1;
            let native_report = serde_json::to_vec(&write.report).map_err(|_| {
                "native sound replacement report를 직렬화할 수 없습니다.".to_string()
            })?;
            let entry = JournalEntry {
                id: format!("sound-{seq}"),
                seq,
                tool: WriteTool::MapSound,
                target: JournalTarget::MapSound {
                    source_map: map_path.clone(),
                    mpq_path: new_mpq_path.clone(),
                    normalized_sha256: edited.normalized.sha256.clone(),
                },
                before: Snapshot::MapBackup {
                    map_path: map_path.to_string_lossy().into_owned(),
                    backup_path: write.backup_path.to_string_lossy().into_owned(),
                },
                after: Snapshot::MapSound {
                    source_sha256: edited.source.source_sha256.clone(),
                    source_codec: edited.source.source_codec.clone(),
                    duration_ms: edited.normalized.duration_ms,
                    channels: edited.source.source_channels,
                    sample_rate: edited.source.source_sample_rate,
                    normalization_profile: format!(
                        "{};ogg/vorbis/44100/stereo/q4;volume={}%;fadeInMs={};fadeOutMs={}",
                        edited.normalized.profile_version,
                        edited.effects.volume_percent,
                        edited.effects.fade_in_ms,
                        edited.effects.fade_out_ms,
                    ),
                    normalized_sha256: edited.normalized.sha256.clone(),
                    normalized_bytes: edited.normalized.bytes,
                    mpq_path: new_mpq_path.clone(),
                    wav_index: write.report.sound_index,
                    string_id: write.report.sound_string_id,
                    map_sha256_before: write.report.input_sha256.clone(),
                    map_sha256_after: write.report.output_sha256.clone(),
                    backup_path: write.backup_path.clone(),
                    native_report_sha256: format!("{:x}", Sha256::digest(&native_report)),
                    map_bytes_before: write.map_bytes_before,
                    map_bytes_after: write.map_bytes_after,
                    source_display_name: edited.source.source_display_name.clone(),
                    edit: Some(crate::journal::MapSoundEditChange {
                        previous_mpq_path: old_mpq_path.to_string(),
                        before: crate::journal::MapSoundEffects {
                            volume_percent: edited.previous_effects.volume_percent,
                            fade_in_ms: edited.previous_effects.fade_in_ms,
                            fade_out_ms: edited.previous_effects.fade_out_ms,
                        },
                        after: crate::journal::MapSoundEffects {
                            volume_percent: edited.effects.volume_percent,
                            fade_in_ms: edited.effects.fade_in_ms,
                            fade_out_ms: edited.effects.fade_out_ms,
                        },
                    }),
                },
                ts: epoch_secs(),
            };
            if let Err(error) = self.services.journal.record(request_id, entry) {
                let restore = self
                    .services
                    .map_safe
                    .restore(&crate::mapsafe::JournalEntry {
                        map_path: map_path.clone(),
                        backup_path: write.backup_path.clone(),
                    });
                let restored_exactly = crate::bootstrap::sha256_file(&map_path)
                    .is_ok_and(|hash| hash == expected_map_sha256);
                if restore.is_err() || !restored_exactly {
                    self.mark_write_hazard(
                        "map sound replacement journal record failed and rollback did not settle",
                    );
                }
                return Err(format!(
                    "맵 사운드 교체 journal 기록에 실패했습니다: {error}"
                ));
            }
            *self.sound_build_required.lock() = true;
            Ok(write)
        })?;
        let write = operation?;
        self.emit_progress(
            crate::ipc::ProgressStage::MapSoundVerify,
            "기존 사운드 제거와 편집본 MPQ/WAV 등록 검증을 완료했습니다.",
        );
        let map_size_delta = i128::from(write.map_bytes_after) - i128::from(write.map_bytes_before);
        Ok(json!({
            "oldMpqPath": old_mpq_path,
            "mpqPath": write.report.mpq_path,
            "durationMs": edited.normalized.duration_ms,
            "normalizedBytes": edited.normalized.bytes,
            "outputCodec": "vorbis",
            "volumePercent": edited.effects.volume_percent,
            "fadeInMs": edited.effects.fade_in_ms,
            "fadeOutMs": edited.effects.fade_out_ms,
            "mapSha256Before": write.report.input_sha256,
            "mapSha256After": write.report.output_sha256,
            "mapSizeDelta": map_size_delta,
            "requiresCodeMigration": true,
        }))
    }

    fn record_native_dat_change(
        &self,
        request_id: &str,
        change: &NativeDatChange,
    ) -> Result<(), String> {
        match change {
            NativeDatChange::Dat {
                dat,
                object_id,
                field,
                before,
                after,
            } => self.record_dat(
                request_id,
                WriteTool::DatSet,
                DatTable::Dat,
                dat,
                i64::from(*object_id),
                field,
                Value::from(*before),
                Value::from(*after),
            ),
            NativeDatChange::Xdat {
                dat,
                object_id,
                field,
                before,
                after,
            } => self.record_dat(
                request_id,
                WriteTool::XdatSet,
                DatTable::Xdat,
                dat,
                i64::from(*object_id),
                field,
                Value::from(*before),
                Value::from(*after),
            ),
            NativeDatChange::Tbl {
                index,
                before,
                after,
            } => self.record_dat(
                request_id,
                WriteTool::TblSet,
                DatTable::Tbl,
                "",
                i64::from(*index),
                "text",
                Value::String(before.clone()),
                Value::String(after.clone()),
            ),
            NativeDatChange::Requirement {
                dat,
                object_id,
                before,
                after,
            } => self.record_dat(
                request_id,
                WriteTool::ReqSet,
                DatTable::Req,
                dat,
                i64::from(*object_id),
                "payload",
                Value::String(before.clone()),
                Value::String(after.clone()),
            ),
            NativeDatChange::Button {
                set_id,
                before,
                after,
            } => self.record_dat(
                request_id,
                WriteTool::BtnSet,
                DatTable::Btn,
                "",
                i64::from(*set_id),
                "csv",
                Value::String(before.clone()),
                Value::String(after.clone()),
            ),
        }
    }

    // An internal journal-entry builder: the dat/xdat/tbl/req/btn writes all
    // share this exact shape (target coordinates + before/after value), so the
    // argument count is inherent rather than a sign of a missing abstraction.
    #[allow(clippy::too_many_arguments)]
    fn record_dat(
        &self,
        request_id: &str,
        tool: WriteTool,
        table: DatTable,
        dat: &str,
        obj_id: i64,
        property: &str,
        old: Value,
        new: Value,
    ) -> Result<(), String> {
        let obj_id = u32::try_from(obj_id)
            .map_err(|_| "objId must be a non-negative integer".to_string())?;
        let seq = self.next_seq(request_id);
        self.record(JournalEntry {
            id: format!("dat-{seq}"),
            seq,
            tool,
            target: JournalTarget::Dat {
                table,
                dat: dat.to_owned(),
                obj_id,
                property: property.to_owned(),
            },
            before: Snapshot::DatValue {
                value: old,
                was_default: false,
            },
            after: Snapshot::DatValue {
                value: new,
                was_default: false,
            },
            ts: epoch_secs(),
        })
    }

    fn file_create(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let (requested_path, ftype) = (str_arg(args, "path")?, str_arg(args, "ftype")?);
        if !matches!(ftype, "CUIEps" | "CUIPy") {
            return Err("파일 형식은 CUIEps 또는 CUIPy여야 합니다.".to_string());
        }
        let code = args.get("code").and_then(Value::as_str).unwrap_or("");
        let path = native_source_path(requested_path);
        self.services.native().create_source(&path, code)?;
        self.record_file(
            request_id,
            WriteTool::FileCreate,
            &path,
            Snapshot::Created,
            Snapshot::Created,
        )?;
        Ok(json!({ "ok": true, "path": path }))
    }

    fn file_write(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let requested_path = str_arg(args, "path")?;
        let path = native_source_path(requested_path);
        let code = str_arg(args, "code")?;
        let old = self.services.native().read_source(&path)?;
        let merged = match self.source_baseline(&path)? {
            Some(base) => crate::workspace::merge_concurrent_text(&path, &base, code, &old)
                .map_err(|error| error.to_string())?,
            None if self.source_created_by_request(request_id, &path) || old == code => {
                code.to_string()
            }
            None => {
                return Err(concurrent_source_conflict(
                    &path,
                    "the file was absent from this session's source baseline",
                ))
            }
        };
        self.services.native().write_source(&path, &merged)?;
        self.record_file(
            request_id,
            WriteTool::FileWrite,
            &path,
            Snapshot::FileContent { content: old },
            Snapshot::FileContent { content: merged },
        )?;
        Ok(json!({ "ok": true, "path": path }))
    }

    fn file_edit(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let requested_path = str_arg(args, "path")?;
        let path = native_source_path(requested_path);
        let edits: Vec<ExactTextEdit> = serde_json::from_value(
            args.get("edits")
                .cloned()
                .ok_or_else(|| "missing argument 'edits'".to_string())?,
        )
        .map_err(|error| format!("invalid file_edit edits: {error}"))?;
        let old = self.services.native().read_source(&path)?;
        let merged = match self.source_baseline(&path)? {
            Some(base) => {
                let edit_base = self
                    .latest_file_content(request_id, &path)
                    .unwrap_or_else(|| base.clone());
                let ours = apply_exact_text_edits(&path, &edit_base, &edits)
                    .map_err(|error| error.to_string())?;
                crate::workspace::merge_concurrent_text(&path, &edit_base, &ours, &old)
                    .map_err(|error| error.to_string())?
            }
            None if self.source_created_by_request(request_id, &path) => {
                apply_exact_text_edits(&path, &old, &edits).map_err(|error| error.to_string())?
            }
            None => {
                return Err(concurrent_source_conflict(
                    &path,
                    "the file was absent from this session's source baseline",
                ))
            }
        };
        self.services.native().write_source(&path, &merged)?;
        self.record_file(
            request_id,
            WriteTool::FileWrite,
            &path,
            Snapshot::FileContent { content: old },
            Snapshot::FileContent { content: merged },
        )?;
        Ok(json!({
            "ok": true,
            "path": path,
            "editsApplied": edits.len(),
        }))
    }

    fn file_delete(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let requested_path = str_arg(args, "path")?;
        let path = native_source_path(requested_path);
        let old = self.services.native().read_source(&path)?;
        if let Some(base) = self.source_baseline(&path)? {
            if old != base {
                return Err(concurrent_source_conflict(
                    &path,
                    "the live file changed after this session read it",
                ));
            }
        }
        self.services.native().delete_source(&path)?;
        self.record_file(
            request_id,
            WriteTool::FileDelete,
            &path,
            Snapshot::DeletedFile {
                content: old,
                position: None,
            },
            Snapshot::Deleted,
        )?;
        Ok(json!({ "ok": true, "path": path }))
    }

    fn mkdir(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let requested_path = str_arg(args, "path")?;
        let path = native_source_dir(requested_path);
        self.services.native().create_source_dir(&path)?;
        self.record_file(
            request_id,
            WriteTool::Mkdir,
            &path,
            Snapshot::Created,
            Snapshot::Created,
        )?;
        Ok(json!({ "ok": true, "path": path }))
    }

    fn file_rename(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let path = native_source_path(str_arg(args, "path")?);
        let newname = str_arg(args, "newname")?;
        if let Some(base) = self.source_baseline(&path)? {
            let current = self.services.native().read_source(&path)?;
            if current != base {
                return Err(concurrent_source_conflict(
                    &path,
                    "the live file changed after this session read it",
                ));
            }
        }
        let to = sibling_path(&path, newname);
        self.services.native().move_source(&path, &to)?;
        self.record_rename(request_id, WriteTool::FileRename, &path, &to)?;
        Ok(json!({ "ok": true, "from": path, "to": to }))
    }

    fn file_move(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let path = native_source_path(str_arg(args, "path")?);
        if let Some(base) = self.source_baseline(&path)? {
            let current = self.services.native().read_source(&path)?;
            if current != base {
                return Err(concurrent_source_conflict(
                    &path,
                    "the live file changed after this session read it",
                ));
            }
        }
        let requested_dest = args.get("destFolder").and_then(Value::as_str).unwrap_or("");
        let dest = native_source_dir(requested_dest);
        let to = moved_path(&path, &dest);
        self.services.native().move_source(&path, &to)?;
        self.record_rename(request_id, WriteTool::FileMove, &path, &to)?;
        Ok(json!({ "ok": true, "from": path, "to": to }))
    }

    fn set_main(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let path = native_source_path(str_arg(args, "path")?);
        let old = self.services.native().status()?.main_file;
        self.services.native().set_main_file(&path)?;
        let seq = self.next_seq(request_id);
        self.record(JournalEntry {
            id: format!("main-{seq}"),
            seq,
            tool: WriteTool::SetMain,
            target: JournalTarget::Path { path: path.clone() },
            before: Snapshot::MainPath { path: Some(old) },
            after: Snapshot::MainPath {
                path: Some(path.clone()),
            },
            ts: epoch_secs(),
        })?;
        Ok(json!({ "ok": true, "mainFile": path }))
    }

    fn settings_set(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let (scope, key, value) = (
            str_arg(args, "scope")?,
            str_arg(args, "key")?,
            str_arg(args, "value")?,
        );
        let old = if scope == "project" {
            let project = self.services.native().open()?;
            match key {
                "OpenMapName" => project.manifest().source_map.clone(),
                "SaveMapName" => project.manifest().output_map.clone(),
                "UseCustomtbl" => project.manifest().settings.use_custom_tbl.to_string(),
                "sectorSize" => project.manifest().settings.sector_size.to_string(),
                _ => return Err(format!("unsupported native project setting {key}")),
            }
        } else {
            let config = self
                .services
                .dirs
                .load_config()
                .map_err(|error| error.to_string())?;
            match key {
                "euddraft" => config.euddraft_path,
                "starcraft" => config.starcraft_path,
                _ => return Err(format!("unsupported native program setting {key}")),
            }
        };
        if scope == "project" {
            self.services.native().set_project_setting(key, value)?;
        } else {
            let mut config = self
                .services
                .dirs
                .load_config()
                .map_err(|error| error.to_string())?;
            match key {
                "euddraft" => config.euddraft_path = value.to_string(),
                "starcraft" => config.starcraft_path = value.to_string(),
                _ => unreachable!(),
            }
            self.services
                .dirs
                .save_config(&config)
                .map_err(|error| error.to_string())?;
        }
        let seq = self.next_seq(request_id);
        self.record(JournalEntry {
            id: format!("set-{seq}"),
            seq,
            tool: WriteTool::SettingsSet,
            target: JournalTarget::Setting {
                key: format!("{scope}|{key}"),
            },
            before: Snapshot::SettingValue {
                value: Value::String(old),
            },
            after: Snapshot::SettingValue {
                value: Value::String(value.to_string()),
            },
            ts: epoch_secs(),
        })?;
        Ok(json!({ "ok": true, "scope": scope, "key": key, "value": value }))
    }

    fn python_dependencies_set(
        &self,
        request_id: &str,
        project_id: &str,
        claimed: crate::native_runtime::ClaimedPythonDependencies,
    ) -> Result<Value, String> {
        let commit = self.services.native().commit_python_dependencies(
            claimed,
            &self.session_id,
            project_id,
        )?;
        let seq = self.next_seq(request_id);
        let entry = JournalEntry {
            id: format!("python-dependencies-{seq}"),
            seq,
            tool: WriteTool::PythonDependenciesSet,
            target: JournalTarget::ProjectManifest {
                path: crate::native_project::PROJECT_MANIFEST_FILE.to_string(),
            },
            before: Snapshot::ManifestBytes {
                bytes: commit.before_manifest.clone(),
                manifest_sha256: commit.before_manifest_sha256,
            },
            after: Snapshot::ManifestBytes {
                bytes: commit.after_manifest,
                manifest_sha256: commit.after_manifest_sha256.clone(),
            },
            ts: epoch_secs(),
        };
        let entry_id = entry.id.clone();
        if let Err(error) = self.record(entry) {
            self.services
                .native()
                .restore_manifest_bytes(
                    &commit.after_manifest_sha256,
                    &commit.before_manifest,
                )
                .map_err(|restore_error| {
                    format!(
                        "Python 의존성 저널 기록과 매니페스트 복원이 모두 실패했습니다: {error}; {restore_error}"
                    )
                })?;
            return Err(format!(
                "Python 의존성 저널을 기록하지 못해 매니페스트를 복원했습니다: {error}"
            ));
        }
        if let Err(error) = self.services.journal.persist(request_id) {
            self.services
                .native()
                .restore_manifest_bytes(
                    &commit.after_manifest_sha256,
                    &commit.before_manifest,
                )
                .map_err(|restore_error| {
                    format!(
                        "Python 의존성 저널 저장과 매니페스트 복원이 모두 실패했습니다: {error}; {restore_error}"
                    )
                })?;
            self.services
                .journal
                .forget_unpersisted_entry(request_id, &entry_id)
                .map_err(|cleanup_error| {
                    format!(
                        "Python 의존성 매니페스트는 복원했지만 미저장 저널을 정리하지 못했습니다: {cleanup_error}"
                    )
                })?;
            return Err(format!(
                "Python 의존성 저널을 저장하지 못해 매니페스트를 복원했습니다: {error}"
            ));
        }
        Ok(json!({
            "ok": true,
            "revision": commit.revision,
            "normalizedDependencies": commit.normalized_dependencies,
            "lockDigest": commit.lock_digest,
        }))
    }

    fn plugin_add(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let index = args.get("index").and_then(Value::as_i64).unwrap_or(-1);
        let texts = args.get("texts").and_then(Value::as_str).unwrap_or("");
        let at = self.services.native().plugin_add(index, texts)?;
        let seq = self.next_seq(request_id);
        self.record(JournalEntry {
            id: format!("plug-{seq}"),
            seq,
            tool: WriteTool::PluginAdd,
            target: JournalTarget::Plugin {
                plugin_id: at.to_string(),
            },
            before: Snapshot::PluginAbsent,
            after: Snapshot::PluginTexts {
                texts: vec![texts.to_string()],
                index: at,
            },
            ts: epoch_secs(),
        })?;
        Ok(json!({ "ok": true, "index": at }))
    }

    fn plugin_edit(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let index = usize::try_from(i64_arg(args, "index")?)
            .map_err(|_| "plugin index must be non-negative".to_string())?;
        let texts = args.get("texts").and_then(Value::as_str).unwrap_or("");
        let old = self
            .services
            .native()
            .open()?
            .manifest()
            .plugins
            .get(index)
            .map(plugin_text)
            .ok_or_else(|| format!("plugin index {index} is out of range"))?;
        self.services.native().plugin_edit(index, texts)?;
        let seq = self.next_seq(request_id);
        self.record(JournalEntry {
            id: format!("plug-{seq}"),
            seq,
            tool: WriteTool::PluginEdit,
            target: JournalTarget::Plugin {
                plugin_id: index.to_string(),
            },
            before: Snapshot::PluginTexts {
                texts: vec![old],
                index,
            },
            after: Snapshot::PluginTexts {
                texts: vec![texts.to_string()],
                index,
            },
            ts: epoch_secs(),
        })?;
        Ok(json!({ "ok": true, "index": index }))
    }

    fn plugin_remove(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let index = usize::try_from(i64_arg(args, "index")?)
            .map_err(|_| "plugin index must be non-negative".to_string())?;
        let removed = self.services.native().plugin_remove(index)?;
        let old = plugin_text(&removed);
        let seq = self.next_seq(request_id);
        self.record(JournalEntry {
            id: format!("plug-{seq}"),
            seq,
            tool: WriteTool::PluginRemove,
            target: JournalTarget::Plugin {
                plugin_id: index.to_string(),
            },
            before: Snapshot::PluginTexts {
                texts: vec![old],
                index,
            },
            after: Snapshot::PluginAbsent,
            ts: epoch_secs(),
        })?;
        Ok(json!({ "ok": true, "index": index }))
    }

    fn plugin_move(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let from = usize::try_from(i64_arg(args, "from")?)
            .map_err(|_| "plugin from index must be non-negative".to_string())?;
        let to = usize::try_from(i64_arg(args, "to")?)
            .map_err(|_| "plugin to index must be non-negative".to_string())?;
        self.services.native().plugin_move(from, to)?;
        let seq = self.next_seq(request_id);
        self.record(JournalEntry {
            id: format!("plug-{seq}"),
            seq,
            tool: WriteTool::PluginMove,
            target: JournalTarget::Plugin {
                plugin_id: from.to_string(),
            },
            before: Snapshot::PluginTexts {
                texts: Vec::new(),
                index: from,
            },
            after: Snapshot::PluginTexts {
                texts: Vec::new(),
                index: to,
            },
            ts: epoch_secs(),
        })?;
        Ok(json!({ "ok": true, "from": from, "to": to }))
    }

    fn record_file(
        &self,
        request_id: &str,
        tool: WriteTool,
        path: &str,
        before: Snapshot,
        after: Snapshot,
    ) -> Result<(), String> {
        let seq = self.next_seq(request_id);
        self.record(JournalEntry {
            id: format!("file-{seq}"),
            seq,
            tool,
            target: JournalTarget::Path {
                path: path.to_string(),
            },
            before,
            after,
            ts: epoch_secs(),
        })
    }

    fn record_rename(
        &self,
        request_id: &str,
        tool: WriteTool,
        from: &str,
        to: &str,
    ) -> Result<(), String> {
        let seq = self.next_seq(request_id);
        self.record(JournalEntry {
            id: format!("file-{seq}"),
            seq,
            tool,
            target: JournalTarget::Rename {
                from: from.to_string(),
                to: to.to_string(),
            },
            before: Snapshot::Path {
                path: from.to_string(),
            },
            after: Snapshot::Path {
                path: to.to_string(),
            },
            ts: epoch_secs(),
        })
    }

    fn source_search(&self, args: &Value) -> Result<Value, String> {
        let query = str_arg(args, "query")?.trim();
        if query.is_empty() {
            return Err("source_search query must not be empty".to_string());
        }
        if query.chars().count() > SOURCE_SEARCH_MAX_QUERY_CHARS {
            return Err(format!(
                "source_search query exceeds {SOURCE_SEARCH_MAX_QUERY_CHARS} characters"
            ));
        }
        let context_lines =
            usize_arg_default(args, "contextLines", 3)?.min(SOURCE_SEARCH_MAX_CONTEXT_LINES);
        let offset = usize_arg_default(args, "offset", 0)?;
        let limit = usize_arg_default(args, "limit", SOURCE_SEARCH_DEFAULT_LIMIT)?
            .clamp(1, SOURCE_SEARCH_MAX_LIMIT);
        let requested_paths = optional_string_array_arg(args, "paths")?;

        let files = self.services.native().list_files()?;
        for requested in &requested_paths {
            if !files
                .iter()
                .any(|file| file.settable && file.path.eq_ignore_ascii_case(requested.as_str()))
            {
                return Err(format!(
                    "source_search path '{requested}' is not an editable project file"
                ));
            }
        }

        let mut matches = Vec::new();
        let mut total = 0usize;
        for file in files {
            if !file.settable
                || (!requested_paths.is_empty()
                    && !requested_paths
                        .iter()
                        .any(|path| file.path.eq_ignore_ascii_case(path)))
            {
                continue;
            }
            let content = self.services.native().read_source(&file.path)?;
            let lines: Vec<&str> = content.lines().collect();
            for (start, end) in source_match_regions(&lines, query, context_lines) {
                if total >= offset && matches.len() < limit {
                    matches.push(json!({
                        "path": file.path,
                        "startLine": start + 1,
                        "endLine": end,
                        "text": lines[start..end].join("\n"),
                    }));
                }
                total += 1;
            }
        }

        let next_offset = offset.saturating_add(matches.len());
        let has_more = next_offset < total;
        Ok(json!({
            "query": query,
            "offset": offset,
            "limit": limit,
            "total": total,
            "count": matches.len(),
            "hasMore": has_more,
            "nextOffset": has_more.then_some(next_offset),
            "matches": matches,
        }))
    }

    fn search_docs(&self, args: &Value) -> Value {
        let query = args.get("query").and_then(Value::as_str).unwrap_or("");
        let k = args
            .get("k")
            .and_then(Value::as_i64)
            .unwrap_or(SEARCH_DOCS_DEFAULT_K)
            .clamp(1, SEARCH_DOCS_MAX_K) as usize;

        // Empty index (no asset yet) returns zero hits. Otherwise hybrid search:
        // lexical substring hits (exact identifiers/Korean terms, no model needed)
        // first, then dense semantic hits fill the rest. A model still warming
        // yields no semantic hits but lexical still works; zero hits either way
        // still lift the evidence gate.
        let hits = if self.services.rag.is_empty() {
            Vec::new()
        } else {
            self.services.rag.search_hybrid(query, k)
        };

        let items: Vec<Value> = hits
            .iter()
            .map(|hit| {
                let (preview, preview_start_char, preview_truncated) =
                    search_docs_preview(&hit.text, query);
                json!({
                    "id": format_doc_id(hit.id),
                    "source": hit.source,
                    "tier": tier_label(hit.tier_level),
                    "match": hit.match_kind.as_str(),
                    "score": hit.score,
                    "preview": preview,
                    "previewStartChar": preview_start_char,
                    "previewTruncated": preview_truncated,
                })
            })
            .collect();
        let note = if items.is_empty() {
            "no reference document matched; treat affected items as 근거 없음 (일반 EUD 지식) — never fabricate a source"
        } else {
            "previews are exact excerpts, not summaries; call docs_get with promising ids before relying on omitted details"
        };
        json!({ "query": query, "count": items.len(), "hits": items, "note": note })
    }

    fn docs_get(&self, args: &Value) -> Result<Value, String> {
        let raw_ids = array_arg(args, "ids")?;
        if raw_ids.len() > DOCS_GET_MAX_IDS {
            return Err(format!(
                "docs_get accepts at most {DOCS_GET_MAX_IDS} ids per call; continue in another call"
            ));
        }
        let mut seen = HashSet::with_capacity(raw_ids.len());
        let mut documents = Vec::with_capacity(raw_ids.len());
        let mut missing_ids = Vec::new();
        for raw_id in raw_ids {
            let text = raw_id
                .as_str()
                .ok_or_else(|| "docs_get ids must be strings".to_string())?;
            let id = parse_doc_id(text)?;
            if !seen.insert(id) {
                return Err(format!("docs_get id '{text}' is duplicated"));
            }
            let Some(entry) = self.services.rag.document(id) else {
                missing_ids.push(text.to_string());
                continue;
            };
            documents.push(json!({
                "id": format_doc_id(entry.id),
                "source": entry.source,
                "tier": tier_label(entry.tier_level),
                "text": entry.text,
            }));
        }
        Ok(json!({
            "count": documents.len(),
            "documents": documents,
            "missingIds": missing_ids,
        }))
    }

    fn latest_file_content(&self, request_id: &str, path: &str) -> Option<String> {
        self.services
            .journal
            .selected_entries(request_id, &crate::journal::DecisionIds::All)
            .ok()?
            .into_iter()
            .rev()
            .find_map(|entry| {
                if entry.tool != WriteTool::FileWrite {
                    return None;
                }
                let JournalTarget::Path { path: target } = entry.target else {
                    return None;
                };
                if target != path {
                    return None;
                }
                match entry.after {
                    Snapshot::FileContent { content } => Some(content),
                    _ => None,
                }
            })
    }

    fn next_seq(&self, request_id: &str) -> u64 {
        self.services.journal.entry_count(request_id) as u64 + 1
    }

    fn record(&self, entry: JournalEntry) -> Result<(), String> {
        let request_id = self
            .current_request_id()
            .ok_or_else(|| "no open request to journal against".to_string())?;
        self.services
            .journal
            .record(&request_id, entry)
            .map_err(stringify)
    }
}

impl crate::journal::JournalRollbackTarget for SessionToolRuntime {
    type Error = String;

    fn set_dat_value(
        &self,
        table: DatTable,
        dat: &str,
        obj_id: u32,
        property: &str,
        value: Value,
    ) -> Result<(), Self::Error> {
        let target = match table {
            DatTable::Dat => DatTarget::Dat {
                dat: dat.to_string(),
                object_id: obj_id,
                field: property.to_string(),
            },
            DatTable::Xdat => DatTarget::Xdat {
                dat: dat.to_string(),
                object_id: obj_id,
                field: property.to_string(),
            },
            DatTable::Tbl => DatTarget::Tbl(obj_id),
            DatTable::Req => DatTarget::Requirement {
                dat: dat.to_string(),
                object_id: obj_id,
            },
            DatTable::Btn => DatTarget::Button(obj_id),
        };
        let current = self
            .services
            .native()
            .dat_values(std::slice::from_ref(&target))?;
        let before = current[&target].clone();
        let change = match (table, before, value) {
            (DatTable::Dat, DatScalar::Number(before), value) => NativeDatChange::Dat {
                dat: dat.to_string(),
                object_id: obj_id,
                field: property.to_string(),
                before,
                after: numeric_json_value(&value)?,
            },
            (DatTable::Xdat, DatScalar::Number(before), value) => NativeDatChange::Xdat {
                dat: dat.to_string(),
                object_id: obj_id,
                field: property.to_string(),
                before,
                after: numeric_json_value(&value)?,
            },
            (DatTable::Tbl, DatScalar::Text(before), value) => NativeDatChange::Tbl {
                index: obj_id,
                before,
                after: value_to_text(&value),
            },
            (DatTable::Req, DatScalar::Text(before), value) => NativeDatChange::Requirement {
                dat: dat.to_string(),
                object_id: obj_id,
                before,
                after: value_to_text(&value),
            },
            (DatTable::Btn, DatScalar::Text(before), value) => NativeDatChange::Button {
                set_id: obj_id,
                before,
                after: value_to_text(&value),
            },
            _ => return Err("journal DAT value type mismatch".to_string()),
        };
        self.services
            .native()
            .apply_dat_patch(&NativeDatPatch {
                changes: vec![change],
            })
            .map(|_| ())
    }

    fn reset_dat_value(
        &self,
        table: DatTable,
        dat: &str,
        obj_id: u32,
        property: &str,
    ) -> Result<(), Self::Error> {
        let target = match table {
            DatTable::Dat => DatTarget::Dat {
                dat: dat.to_string(),
                object_id: obj_id,
                field: property.to_string(),
            },
            DatTable::Xdat => DatTarget::Xdat {
                dat: dat.to_string(),
                object_id: obj_id,
                field: property.to_string(),
            },
            DatTable::Tbl => DatTarget::Tbl(obj_id),
            DatTable::Req => DatTarget::Requirement {
                dat: dat.to_string(),
                object_id: obj_id,
            },
            DatTable::Btn => DatTarget::Button(obj_id),
        };
        let original = self
            .services
            .native()
            .open()?
            .original_dat_value(&target)
            .ok_or_else(|| format!("target {target:?} has no sparse override to reset"))?;
        self.set_dat_value(table, dat, obj_id, property, dat_scalar_json(&original))
    }

    fn write_file(&self, path: &str, content: &str) -> Result<(), Self::Error> {
        self.services
            .native()
            .write_source(&native_source_path(path), content)
    }

    fn delete_file(&self, path: &str) -> Result<(), Self::Error> {
        self.services
            .native()
            .delete_source(&native_source_path(path))
    }

    fn write_workspace_file(
        &self,
        workspace_id: &str,
        path: &str,
        content: &str,
    ) -> Result<(), Self::Error> {
        crate::workspace::WorkspaceManager::new(self.data_dirs())
            .restore_file(workspace_id, path, Some(content))
            .map_err(stringify)
    }

    fn delete_workspace_file(&self, workspace_id: &str, path: &str) -> Result<(), Self::Error> {
        crate::workspace::WorkspaceManager::new(self.data_dirs())
            .restore_file(workspace_id, path, None)
            .map_err(stringify)
    }

    fn create_file(
        &self,
        path: &str,
        content: &str,
        _position: Option<usize>,
    ) -> Result<(), Self::Error> {
        self.services
            .native()
            .create_source(&native_source_path(path), content)
    }

    fn rename_path(&self, from: &str, to: &str) -> Result<(), Self::Error> {
        if from == to {
            return Ok(());
        }
        self.services
            .native()
            .move_source(&native_source_path(from), &native_source_path(to))
    }

    fn set_main(&self, path: Option<&str>) -> Result<(), Self::Error> {
        let path = path.ok_or_else(|| "native MainFile cannot be empty".to_string())?;
        self.services
            .native()
            .set_main_file(&native_source_path(path))
    }

    fn set_setting(&self, key: &str, value: Value) -> Result<(), Self::Error> {
        let (scope, name) = key
            .split_once('|')
            .ok_or_else(|| format!("invalid journal setting key '{key}'"))?;
        let value = value_to_text(&value);
        if scope == "project" {
            self.services.native().set_project_setting(name, &value)
        } else {
            let mut config = self
                .services
                .dirs
                .load_config()
                .map_err(|error| error.to_string())?;
            match name {
                "euddraft" => config.euddraft_path = value,
                "starcraft" => config.starcraft_path = value,
                _ => return Err(format!("unsupported native program setting {name}")),
            }
            self.services
                .dirs
                .save_config(&config)
                .map_err(|error| error.to_string())
        }
    }

    fn plugin_add(
        &self,
        _plugin_id: &str,
        texts: Vec<String>,
        index: usize,
    ) -> Result<(), Self::Error> {
        self.services
            .native()
            .plugin_add(index as i64, &texts.join("\n"))
            .map(|_| ())
    }

    fn plugin_edit(
        &self,
        _plugin_id: &str,
        texts: Vec<String>,
        index: usize,
    ) -> Result<(), Self::Error> {
        self.services.native().plugin_edit(index, &texts.join("\n"))
    }

    fn plugin_remove(&self, plugin_id: &str) -> Result<(), Self::Error> {
        let index = rollback_plugin_index(plugin_id)?;
        self.services.native().plugin_remove(index).map(|_| ())
    }

    fn plugin_move(&self, from_index: usize, to_index: usize) -> Result<(), Self::Error> {
        self.services.native().plugin_move(from_index, to_index)
    }

    fn restore_project_manifest(
        &self,
        expected_revision: &str,
        bytes: &[u8],
    ) -> Result<(), Self::Error> {
        self.services
            .native()
            .restore_manifest_bytes(expected_revision, bytes)
            .map(|_| ())
    }

    fn restore_map_backup(
        &self,
        map_path: &str,
        backup_path: &str,
        expected_sha256: Option<&str>,
    ) -> Result<(), Self::Error> {
        let map_path = PathBuf::from(map_path);
        self.services
            .map_safe
            .restore(&crate::mapsafe::JournalEntry {
                map_path: map_path.clone(),
                backup_path: PathBuf::from(backup_path),
            })
            .map_err(stringify)?;
        if let Some(expected_sha256) = expected_sha256 {
            let actual = crate::bootstrap::sha256_file(&map_path)
                .map_err(|error| format!("restored map hash failed: {error}"))?;
            if actual != expected_sha256 {
                return Err("restored map SHA-256 does not match exact before state".to_string());
            }
        }
        Ok(())
    }
}

fn rollback_plugin_index(plugin_id: &str) -> Result<usize, String> {
    plugin_id
        .parse()
        .map_err(|_| format!("invalid plugin journal index '{plugin_id}'"))
}

#[cfg(test)]
impl ToolServices {
    pub fn for_tests() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        // Nanoseconds alone collide when parallel tests construct runtimes in
        // the same tick; the UUID keeps every runtime's data dirs private.
        let base = std::env::temp_dir().join(format!(
            "eud-agent-runtime-test-{nanos}-{}",
            uuid::Uuid::new_v4()
        ));
        let dirs = DataDirs::from_bases(&base, &base);
        let candidates = crate::map_candidate::CandidateStore::new(
            (dirs.clone()).clone(),
            crate::map_import::MapImportStore::new(dirs.clone()),
        );
        Self::new(
            dirs,
            candidates,
            crate::write_coordinator::ProjectWriteCoordinator::silent(),
        )
    }
}

#[cfg(test)]
impl SessionToolRuntime {
    pub fn for_tests() -> Self {
        ToolServices::for_tests().session("test-session")
    }

    pub(crate) fn execution_lock_for_tests(&self) -> Arc<Mutex<()>> {
        Arc::clone(&self.execution_lock)
    }

    pub fn require_sound_build_for_tests(&self) {
        *self.sound_build_required.lock() = true;
    }
}

fn map_palette_catalog_request(args: &Value, tileset: u16) -> Result<Value, String> {
    if args.get("offset").is_some() || args.get("limit").is_some() {
        return Err(
            "map_palette_query does not accept pagination; refine query/filter instead".to_string(),
        );
    }
    let kind = str_arg(args, "kind")?;
    let query = match args.get("query") {
        Some(value) => {
            let query = value
                .as_str()
                .ok_or_else(|| "argument 'query' must be a string".to_string())?
                .trim();
            if query.is_empty() {
                return Err("map_palette_query query must not be blank".to_string());
            }
            Some(query)
        }
        None => None,
    };
    let filter = match args.get("filter") {
        Some(value) => {
            let object = value
                .as_object()
                .ok_or_else(|| "argument 'filter' must be an object".to_string())?;
            if object.is_empty() {
                return Err("map_palette_query filter must not be empty".to_string());
            }
            Some(value)
        }
        None => None,
    };
    if query.is_none() && filter.is_none() {
        return Err(
            "map_palette_query requires a non-blank name query or structured filter".to_string(),
        );
    }

    let mut request = json!({
        "schema": "eud-map-catalog/1",
        "kind": kind,
        "tileset": tileset,
        "offset": 0,
        "limit": MAP_PALETTE_QUERY_MAX_MATCHES + 1,
    });
    if let Some(query) = query {
        request["query"] = json!(query);
    }
    if let Some(filter) = filter {
        request["filter"] = filter.clone();
    }
    Ok(request)
}

fn enforce_map_palette_result_bound(value: Value) -> Result<Value, String> {
    let total = value
        .get("total")
        .and_then(Value::as_u64)
        .ok_or_else(|| "map palette catalog response is missing integer total".to_string())?;
    if total > MAP_PALETTE_QUERY_MAX_MATCHES as u64 {
        return Err(format!(
            "map_palette_query matched {total} entries; refine query/filter to at most {MAP_PALETTE_QUERY_MAX_MATCHES} matches (for exact tiles, search brushes first and filter by terrainType, group, or tile metadata)"
        ));
    }
    let returned = value
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| "map palette catalog response is missing entries".to_string())?
        .len() as u64;
    if returned != total {
        return Err(format!(
            "map palette catalog returned {returned} of {total} bounded matches"
        ));
    }
    Ok(value)
}

fn usize_arg_default(args: &Value, name: &str, default: usize) -> Result<usize, String> {
    let Some(value) = args.get(name) else {
        return Ok(default);
    };
    let value = value
        .as_u64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        .ok_or_else(|| format!("argument '{name}' must be a non-negative integer"))?;
    usize::try_from(value).map_err(|_| format!("argument '{name}' is too large"))
}

fn optional_string_array_arg(args: &Value, name: &str) -> Result<Vec<String>, String> {
    let Some(value) = args.get(name) else {
        return Ok(Vec::new());
    };
    let values = value
        .as_array()
        .ok_or_else(|| format!("argument '{name}' must be an array of strings"))?;
    let mut seen = HashSet::with_capacity(values.len());
    let mut result = Vec::with_capacity(values.len());
    for value in values {
        let text = value
            .as_str()
            .ok_or_else(|| format!("argument '{name}' must contain only strings"))?
            .trim();
        if text.is_empty() {
            return Err(format!("argument '{name}' contains an empty string"));
        }
        let key = text.to_lowercase();
        if !seen.insert(key) {
            return Err(format!("argument '{name}' contains duplicate '{text}'"));
        }
        result.push(text.to_string());
    }
    Ok(result)
}

fn ranged_file_result(path: &str, content: &str, args: &Value) -> Result<Value, String> {
    let total_lines = content.lines().count();
    if args.get("startLine").is_none() && args.get("endLine").is_none() {
        return Ok(json!({
            "path": path,
            "content": content,
            "startLine": (total_lines > 0).then_some(1),
            "endLine": total_lines,
            "totalLines": total_lines,
            "hasMore": false,
        }));
    }

    let start = usize_arg_default(args, "startLine", 1)?;
    if start == 0 {
        return Err("read_file startLine is 1-based and must be at least 1".to_string());
    }
    if total_lines == 0 {
        return Ok(json!({
            "path": path,
            "content": "",
            "startLine": Value::Null,
            "endLine": 0,
            "totalLines": 0,
            "hasMore": false,
        }));
    }
    if start > total_lines {
        return Err(format!(
            "read_file startLine {start} exceeds {total_lines} total lines"
        ));
    }
    let default_end = start
        .saturating_add(READ_FILE_DEFAULT_LINES - 1)
        .min(total_lines);
    let end = usize_arg_default(args, "endLine", default_end)?.min(total_lines);
    if end < start {
        return Err(format!(
            "read_file endLine {end} precedes startLine {start}"
        ));
    }
    let selected = content
        .lines()
        .skip(start - 1)
        .take(end - start + 1)
        .collect::<Vec<_>>()
        .join("\n");
    Ok(json!({
        "path": path,
        "content": selected,
        "startLine": start,
        "endLine": end,
        "totalLines": total_lines,
        "hasMore": end < total_lines,
    }))
}

fn source_match_regions(lines: &[&str], query: &str, context_lines: usize) -> Vec<(usize, usize)> {
    let query = query.to_lowercase();
    let mut regions: Vec<(usize, usize)> = Vec::new();
    for (line_index, line) in lines.iter().enumerate() {
        if !line.to_lowercase().contains(&query) {
            continue;
        }
        let start = line_index.saturating_sub(context_lines);
        let end = line_index
            .saturating_add(context_lines + 1)
            .min(lines.len());
        if let Some(last) = regions.last_mut() {
            if start <= last.1 {
                last.1 = last.1.max(end);
                continue;
            }
        }
        regions.push((start, end));
    }
    regions
}

fn search_docs_preview(text: &str, query: &str) -> (String, usize, bool) {
    let total_chars = text.chars().count();
    if total_chars <= SEARCH_DOCS_PREVIEW_CHARS {
        return (text.to_string(), 0, false);
    }

    let lower = text.to_lowercase();
    let matched_char = crate::rag::tokenize_lexical(query)
        .iter()
        .filter_map(|term| {
            lower
                .find(term)
                .map(|byte_index| lower[..byte_index].chars().count())
        })
        .min()
        .unwrap_or(0);
    let start = matched_char
        .saturating_sub(SEARCH_DOCS_PREVIEW_CHARS / 3)
        .min(total_chars - SEARCH_DOCS_PREVIEW_CHARS);
    let preview = text
        .chars()
        .skip(start)
        .take(SEARCH_DOCS_PREVIEW_CHARS)
        .collect();
    (preview, start, true)
}

fn format_doc_id(id: u64) -> String {
    format!("{id:016x}")
}

fn parse_doc_id(text: &str) -> Result<u64, String> {
    let text = text.trim();
    let text = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .unwrap_or(text);
    if text.is_empty() || text.len() > 16 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!(
            "documentation id '{text}' must contain 1 to 16 hexadecimal digits"
        ));
    }
    u64::from_str_radix(text, 16)
        .map_err(|error| format!("invalid documentation id '{text}': {error}"))
}

fn tier_label(tier_level: u8) -> &'static str {
    match tier_level {
        3 => "primary",
        2 => "lecture",
        1 => "general",
        _ => "qa",
    }
}
fn render_scale_arg(args: &Value) -> Result<usize, String> {
    let scale = usize_arg_default(args, "scale", 4)?;
    if !matches!(scale, 1 | 2 | 4 | 8) {
        return Err("map render scale must be 1, 2, 4, or 8".to_string());
    }
    Ok(scale)
}

fn render_map_tool(
    map: &std::path::Path,
    state: &crate::map_candidate::CandidateStateView,
    args: &Value,
    starcraft_path: &std::path::Path,
) -> Result<Value, String> {
    let x = usize_arg_default(args, "x", 0)?;
    let y = usize_arg_default(args, "y", 0)?;
    let width = usize_arg_default(args, "width", usize::from(state.baseline.width))?;
    let height = usize_arg_default(args, "height", usize::from(state.baseline.height))?;
    let scale = render_scale_arg(args)?;
    if width == 0
        || height == 0
        || x + width > usize::from(state.baseline.width)
        || y + height > usize::from(state.baseline.height)
    {
        return Err("map render crop is outside candidate dimensions".to_string());
    }
    let layers = args
        .get("layers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            vec![
                json!("terrain"),
                json!("doodads"),
                json!("sprites"),
                json!("units"),
                json!("buildings"),
            ]
        });
    let request = json!({
        "schema": "eud-map-render/1",
        "mode": "region",
        "x": x,
        "y": y,
        "width": width,
        "height": height,
        "scale": scale,
        "layers": layers,
    });
    let image = isom::render_region(map, starcraft_path, request.to_string().as_bytes())
        .map_err(|error| format!("map render failed: {error}"))?;
    crate::map_agent::mcp_image(&image)
}

pub(crate) struct MapObjectSnapshot {
    layers: std::collections::BTreeMap<&'static str, Vec<Value>>,
}

impl MapObjectSnapshot {
    pub(crate) fn page(&self, layer: &str, offset: usize, limit: usize) -> Result<Value, String> {
        let items = self
            .layers
            .get(layer)
            .ok_or_else(|| format!("unsupported map object layer '{layer}'"))?;
        let total = items.len();
        let items = items
            .iter()
            .skip(offset)
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        Ok(json!({"layer": layer, "offset": offset, "total": total, "items": items}))
    }
}

pub(crate) fn map_object_snapshot(
    map: &std::path::Path,
    starcraft_path: &std::path::Path,
    revision_key: &str,
    baseline_hash: &str,
) -> Result<MapObjectSnapshot, String> {
    let chk = isom::chk_extract(map).map_err(|error| error.to_string())?;
    let digest = crate::chk::digest_chk(&chk);
    let sections = crate::chk::assemble_sections(&crate::chk::walk_sections(&chk));
    let buildings = map_building_ids(starcraft_path, &digest.map.tileset)?;
    let mut layers = std::collections::BTreeMap::from([
        ("units", Vec::new()),
        ("buildings", Vec::new()),
        ("doodads", Vec::new()),
        ("sprites", Vec::new()),
        ("locations", Vec::new()),
    ]);

    let raw_units = sections.get("UNIT").map(Vec::as_slice).unwrap_or(&[]);
    for (ordinal, (unit, bytes)) in digest
        .units
        .iter()
        .zip(raw_units.chunks_exact(crate::chk::UNIT_ENTRY_SIZE))
        .enumerate()
    {
        let building = buildings.contains(&unit.type_id);
        let layer = if building { "buildings" } else { "units" };
        layers
            .get_mut(layer)
            .expect("object layer exists")
            .push(json!({
                "object": unit,
                "objectRef": {
                    "kind": if building { "building" } else { "unit" },
                    "ordinal": ordinal,
                    "semanticFingerprint": crate::map_model::hex_sha256(bytes),
                    "revisionKey": revision_key,
                    "baselineHash": baseline_hash,
                }
            }));
    }

    let raw_doodads = sections.get("DD2 ").map(Vec::as_slice).unwrap_or(&[]);
    for doodad in &digest.doodads {
        let start = doodad.ordinal.saturating_mul(crate::chk::DD2_ENTRY_SIZE);
        let Some(bytes) = raw_doodads.get(start..start.saturating_add(crate::chk::DD2_ENTRY_SIZE))
        else {
            continue;
        };
        layers
            .get_mut("doodads")
            .expect("object layer exists")
            .push(json!({
                "object": doodad,
                "objectRef": {
                    "kind": "doodad",
                    "ordinal": doodad.ordinal,
                    "semanticFingerprint": crate::map_model::hex_sha256(bytes),
                    "revisionKey": revision_key,
                    "baselineHash": baseline_hash,
                }
            }));
    }

    let raw_sprites = sections.get("THG2").map(Vec::as_slice).unwrap_or(&[]);
    for sprite in &digest.sprites {
        let start = sprite.ordinal.saturating_mul(crate::chk::THG2_ENTRY_SIZE);
        let Some(bytes) = raw_sprites.get(start..start.saturating_add(crate::chk::THG2_ENTRY_SIZE))
        else {
            continue;
        };
        layers
            .get_mut("sprites")
            .expect("object layer exists")
            .push(json!({
                "object": sprite,
                "objectRef": {
                    "kind": "sprite",
                    "ordinal": sprite.ordinal,
                    "semanticFingerprint": crate::map_model::hex_sha256(bytes),
                    "revisionKey": revision_key,
                    "baselineHash": baseline_hash,
                }
            }));
    }

    layers
        .get_mut("locations")
        .expect("object layer exists")
        .extend(digest.locations.iter().map(|location| {
            json!({
                "location": location,
                "revisionKey": revision_key,
                "baselineHash": baseline_hash,
            })
        }));

    Ok(MapObjectSnapshot { layers })
}

pub(crate) fn map_objects_page(
    map: &std::path::Path,
    starcraft_path: &std::path::Path,
    revision_key: &str,
    baseline_hash: &str,
    layer: &str,
    offset: usize,
    limit: usize,
) -> Result<Value, String> {
    map_object_snapshot(map, starcraft_path, revision_key, baseline_hash)?
        .page(layer, offset, limit)
}

pub(crate) fn map_building_ids(
    starcraft_path: &std::path::Path,
    tileset_name: &str,
) -> Result<std::collections::BTreeSet<u16>, String> {
    let tileset = [
        "badlands",
        "platform",
        "installation",
        "ashworld",
        "jungle",
        "desert",
        "arctic",
        "twilight",
    ]
    .iter()
    .position(|candidate| *candidate == tileset_name)
    .ok_or_else(|| format!("unknown map tileset: {tileset_name}"))?;
    let request = json!({
        "schema": "eud-map-catalog/1",
        "kind": "buildings",
        "tileset": tileset,
        "offset": 0,
        "limit": 512,
    });
    let result = isom::catalog_query(starcraft_path, request.to_string().as_bytes())
        .map_err(|error| format!("building DAT catalog is unavailable: {error}"))?;
    let value: Value = serde_json::from_str(&result)
        .map_err(|error| format!("building DAT catalog response is invalid: {error}"))?;
    let entries = value["entries"]
        .as_array()
        .ok_or_else(|| "building DAT catalog has no entries array".to_string())?;
    if entries.is_empty() {
        return Err("building DAT catalog is empty".to_string());
    }
    entries
        .iter()
        .map(|entry| {
            entry["id"]
                .as_u64()
                .and_then(|id| u16::try_from(id).ok())
                .ok_or_else(|| "building DAT catalog contains an invalid id".to_string())
        })
        .collect()
}

fn map_asset_inventory(map_path: &std::path::Path) -> Result<BTreeMap<String, String>, String> {
    let digest = isom::map_digest(map_path)
        .map_err(|_| "저장된 맵의 MPQ inventory를 읽을 수 없습니다.".to_string())?;
    let value: Value = serde_json::from_str(&digest)
        .map_err(|_| "맵 MPQ inventory 응답이 올바르지 않습니다.".to_string())?;
    let assets = value["extraAssets"]["assets"]
        .as_array()
        .ok_or_else(|| "맵 MPQ inventory가 없습니다.".to_string())?;
    let mut inventory = BTreeMap::new();
    for asset in assets {
        let path = asset["path"]
            .as_str()
            .filter(|path| !path.is_empty())
            .ok_or_else(|| "맵 MPQ asset path가 올바르지 않습니다.".to_string())?;
        let sha256 = asset["sha256"]
            .as_str()
            .filter(|sha256| sha256.len() == 64)
            .ok_or_else(|| "맵 MPQ asset checksum이 올바르지 않습니다.".to_string())?;
        if inventory
            .insert(path.to_ascii_lowercase(), sha256.to_string())
            .is_some()
        {
            return Err("맵 MPQ inventory에 중복 path가 있습니다.".to_string());
        }
    }
    Ok(inventory)
}

fn managed_sound_hash(path: &str) -> Option<&str> {
    let hash = path
        .strip_prefix("staredit\\wav\\ea_")?
        .strip_suffix(".ogg")?;
    if matches!(hash.len(), 16 | 24 | 32 | 64)
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Some(hash)
    } else {
        None
    }
}

fn select_managed_sound_path(
    map_path: &std::path::Path,
    normalized_sha256: &str,
) -> Result<String, String> {
    let inventory = map_asset_inventory(map_path)?;
    select_managed_sound_path_from_inventory(&inventory, normalized_sha256)
}

fn select_managed_sound_path_from_inventory(
    inventory: &BTreeMap<String, String>,
    normalized_sha256: &str,
) -> Result<String, String> {
    if normalized_sha256.len() != 64
        || !normalized_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("정규화된 OGG checksum이 올바르지 않습니다.".to_string());
    }
    for length in [16usize, 24, 32, 64] {
        let path = format!("staredit\\wav\\ea_{}.ogg", &normalized_sha256[..length]);
        match inventory.get(&path) {
            None => return Ok(path),
            Some(existing) if existing == normalized_sha256 => return Ok(path),
            Some(_) => continue,
        }
    }
    Err("관리형 MPQ sound path checksum prefix가 모두 충돌합니다.".to_string())
}

fn select_managed_sound_replacement_path_from_inventory(
    inventory: &BTreeMap<String, String>,
    old_mpq_path: &str,
    normalized_sha256: &str,
) -> Result<String, String> {
    if managed_sound_hash(old_mpq_path).is_none()
        || normalized_sha256.len() != 64
        || !normalized_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("편집된 OGG 경로 또는 checksum이 올바르지 않습니다.".to_string());
    }
    for length in [16usize, 24, 32, 64] {
        let path = format!("staredit\\wav\\ea_{}.ogg", &normalized_sha256[..length]);
        if path != old_mpq_path && !inventory.contains_key(&path) {
            return Ok(path);
        }
    }
    Err("편집본용 관리형 MPQ sound path checksum prefix가 모두 사용 중입니다.".to_string())
}

fn numeric_json_value(value: &Value) -> Result<i64, String> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        .ok_or_else(|| "DAT value must be an integer or numeric string".to_string())
}

fn load_rag(dirs: &DataDirs) -> Rag {
    let index_path = dirs.rag_dir().join(crate::bootstrap::RAG_INDEX_FILENAME);
    let cache_dir = Some(dirs.models_dir());
    match Rag::from_index_file(&index_path, cache_dir.clone()) {
        Ok(rag) => rag,
        Err(_) => Rag::new(Vec::new(), cache_dir),
    }
}

fn stringify(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn concurrent_source_conflict(path: &str, detail: &str) -> String {
    format!("ConcurrentWriteConflict: `{path}` {detail}")
}

fn str_arg<'a>(args: &'a Value, name: &str) -> Result<&'a str, String> {
    args.get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing or non-string argument '{name}'"))
}

fn optional_u64_arg(args: &Value, name: &str) -> Result<Option<u64>, String> {
    match args.get(name) {
        None => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| format!("non-integer argument '{name}'")),
    }
}

fn array_arg<'a>(args: &'a Value, name: &str) -> Result<&'a [Value], String> {
    args.get(name)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("missing or non-array argument '{name}'"))
}

fn i64_arg(args: &Value, name: &str) -> Result<i64, String> {
    args.get(name)
        .and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        })
        .ok_or_else(|| format!("missing or non-integer argument '{name}'"))
}

fn dat_scalar_json(value: &DatScalar) -> Value {
    match value {
        DatScalar::Number(value) => Value::from(*value),
        DatScalar::Text(value) => Value::String(value.clone()),
    }
}

/// Convert a validated DAT JSON scalar to its canonical text representation.
fn value_to_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// New full path for a rename: keep the source's parent folder, swap the leaf.
// Native source paths keep their declared extension. Generated euddraft plugin
// paths are materialized exactly as stored in the project manifest.
fn plugin_text(plugin: &crate::native_project::EdsPlugin) -> String {
    if let Some(raw_text) = &plugin.raw_text {
        return raw_text.clone();
    }
    let mut text = format!("[{}]\n", plugin.section);
    for entry in &plugin.entries {
        match &entry.value {
            Some(value) => text.push_str(&format!("{}: {}\n", entry.key, value)),
            None => text.push_str(&format!("{}\n", entry.key)),
        }
    }
    text
}

fn native_source_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    if normalized == "src" || normalized.starts_with("src/") {
        normalized
    } else {
        format!("src/{}", normalized.trim_start_matches('/'))
    }
}

fn native_source_dir(path: &str) -> String {
    if path.trim().is_empty() {
        "src".to_string()
    } else {
        native_source_path(path).trim_end_matches('/').to_string()
    }
}

fn sibling_path(path: &str, newname: &str) -> String {
    match path.rsplit_once('/') {
        Some((parent, _)) => format!("{parent}/{newname}"),
        None => newname.to_string(),
    }
}

/// New full path for a move: the leaf under `dest` (an empty dest = project root).
fn moved_path(path: &str, dest: &str) -> String {
    let leaf = path.rsplit_once('/').map(|(_, leaf)| leaf).unwrap_or(path);
    if dest.is_empty() {
        leaf.to_string()
    } else {
        format!("{dest}/{leaf}")
    }
}

fn epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
struct ToolCompletionBarrier {
    reached: tokio::sync::oneshot::Sender<()>,
    released: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
impl SessionToolRuntime {
    pub(crate) fn pause_mutation_completion(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        std::sync::mpsc::Sender<()>,
    ) {
        let (reached, observed) = tokio::sync::oneshot::channel();
        let (release, released) = std::sync::mpsc::channel();
        *self.completion_barrier.lock() = Some(ToolCompletionBarrier { reached, released });
        (observed, release)
    }
}

#[cfg(test)]
#[path = "tool_exec/blocking_completion_tests.rs"]
mod blocking_completion_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    fn open_runtime(request_id: &str) -> SessionToolRuntime {
        let runtime = SessionToolRuntime::for_tests();
        runtime.begin_request(request_id, "test-project").unwrap();
        runtime.register_write_request("test mutation").unwrap();
        runtime
    }

    #[test]
    fn prepared_native_source_baseline_admits_an_exact_file_edit() {
        // Given: a real native project whose canonical src/ is captured into the
        // trusted turn baseline that write tools use for stale checks.
        let services = ToolServices::for_tests();
        let root = services.dirs.app_data().join("source-baseline-project");
        std::fs::create_dir_all(root.join("maps")).unwrap();
        std::fs::write(root.join("maps/source.scx"), b"fixture map").unwrap();
        let project = crate::native_project::NativeProject::create(
            &root,
            crate::native_project::ProjectManifest {
                schema_version: crate::native_project::PROJECT_SCHEMA_VERSION,
                name: "Source baseline project".to_string(),
                source_map: "maps/source.scx".to_string(),
                output_map: "build/output.scx".to_string(),
                main_file: "src/main.eps".to_string(),
                settings: Default::default(),
                plugins: Vec::new(),
                python_entrypoints: Vec::new(),
                python_dependencies: Vec::new(),
                python_lock: None,
                editor_compatibility: None,
            },
        )
        .unwrap();
        services.native().activate_project(&project).unwrap();
        services
            .native()
            .write_source("src/main.eps", "// baseline\n")
            .unwrap();
        services.native().create_source_dir("src/nested").unwrap();
        services
            .native()
            .write_source("src/nested/helper.eps", "// nested\n")
            .unwrap();
        let manager = crate::workspace::WorkspaceManager::new(services.dirs.clone());
        let workspace = manager
            .prepare_session_current("source-baseline-session")
            .unwrap();
        let baseline = manager
            .begin_turn(&workspace, "source-baseline-request")
            .unwrap();

        let runtime = services.session("source-baseline-session");
        runtime
            .begin_request("source-baseline-request", "source-baseline-project")
            .unwrap();
        runtime
            .register_write_request("edit the existing source")
            .unwrap();
        runtime
            .bind_source_baseline("source-baseline-request", baseline.baseline_root.clone())
            .unwrap();
        runtime
            .execute("search_docs", &json!({"query": "epScript comment"}))
            .unwrap();

        // When: the production file-edit path resolves the native src/ path against that mirror.
        runtime
            .execute(
                "file_edit",
                &json!({
                    "path": "src/main.eps",
                    "edits": [{"old_text": "// baseline\n", "new_text": "// baseline\n// edited\n"}],
                }),
            )
            .unwrap();

        // Then: the existing source is edited, while a genuinely absent source has no baseline.
        assert_eq!(
            services.native().read_source("src/main.eps").unwrap(),
            "// baseline\n// edited\n"
        );
        let entries = services
            .journal
            .selected_entries("source-baseline-request", &crate::journal::DecisionIds::All)
            .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].tool, WriteTool::FileWrite);
        assert_eq!(
            entries[0].target,
            JournalTarget::Path {
                path: "src/main.eps".to_string()
            }
        );
        assert_eq!(
            crate::workspace::read_source_baseline(&baseline.baseline_root, "src/missing.eps")
                .unwrap(),
            None
        );
        assert_eq!(
            crate::workspace::read_source_baseline(
                &baseline.baseline_root,
                "src/nested/helper.eps"
            )
            .unwrap()
            .as_deref(),
            Some("// nested\n")
        );
        assert_eq!(
            crate::workspace::read_source_baseline(&baseline.baseline_root, "nested/helper.eps")
                .unwrap()
                .as_deref(),
            Some("// nested\n")
        );
        manager.finish_turn(&baseline).unwrap();
    }

    #[test]
    fn execute_without_open_request_is_rejected() {
        // No begin_request -> no live request id to resolve against.
        let runtime = SessionToolRuntime::for_tests();
        let error = runtime
            .execute("project_status", &json!({}))
            .expect_err("a tool call outside a turn must be rejected");
        assert!(error.contains("no agent request is open"), "got: {error}");
    }

    #[tokio::test]
    async fn ask_waits_for_all_answers_and_resumes_the_same_tool_call() {
        let runtime = SessionToolRuntime::for_tests();
        let sessions = crate::session::SessionStore::new(&runtime.data_dirs());
        let now = crate::session::now_unix_millis();
        let mut autonomous = crate::autonomous::AutonomousRunState::new(
            "사용자 선택이 필요한 작업".to_string(),
            "turn-ask".to_string(),
            "req-ask".to_string(),
            "project".to_string(),
            "revision".to_string(),
            Default::default(),
            now,
        );
        let binding = crate::provider::ProviderBinding::new(
            crate::provider::ProviderId::Codex,
            "gpt-test".to_string(),
            None,
        )
        .unwrap();
        autonomous.last_checkpoint = Some(binding.conversation.clone());
        sessions
            .save(&crate::session::SessionRecord {
                meta: crate::session::SessionMeta {
                    id: runtime.session_id().to_string(),
                    name: "ask lifecycle".to_string(),
                    project: "project".to_string(),
                    kind: crate::session::SessionKind::Eps,
                    provider: binding.provider,
                    model: binding.model.clone(),
                    created_at: now / 1_000,
                    last_conversation_at: now,
                },
                provider_binding: binding,
                pending_request_ids: Vec::new(),
                context_usage: None,
                panel_log: Value::Null,
                context_state: Default::default(),
                task_state: Default::default(),
                autonomous_run: Some(autonomous),
                workflow: None,
            })
            .unwrap();
        runtime.begin_request("req-ask", "project").unwrap();
        let (events, mut emitted) = tokio::sync::mpsc::unbounded_channel();
        runtime.set_ask_emitter(move |event| {
            events
                .send(event)
                .map_err(|_| "ask event receiver closed".to_string())
        });
        let (autonomous_events, mut emitted_autonomous) = tokio::sync::mpsc::unbounded_channel();
        runtime.set_autonomous_emitter(move |event| {
            autonomous_events
                .send(event)
                .map_err(|_| "autonomous event receiver closed".to_string())
        });

        let asking = runtime.clone();
        let task = tokio::spawn(async move {
            asking
                .ask(&json!({
                    "questions": [
                        {
                            "id": "mode",
                            "header": "방식",
                            "question": "어떤 방식을 사용할까요?",
                            "options": [
                                {"label": "A", "description": "첫 번째"},
                                {"label": "B", "description": "두 번째"}
                            ]
                        },
                        {
                            "id": "features",
                            "question": "필요한 항목을 고르세요.",
                            "multi": true,
                            "options": [
                                {"label": "로그"},
                                {"label": "알림"}
                            ]
                        }
                    ]
                }))
                .await
        });

        let event = emitted.recv().await.expect("ask event must be emitted");
        assert_eq!(event.questions.len(), 2);
        assert_eq!(event.questions[0].id, "mode");
        let restored = runtime.pending_ask().expect("ask is pending");
        assert_eq!(restored.request_id, event.request_id);
        assert_eq!(restored.questions, event.questions);
        assert_eq!(restored.status, crate::ipc::AskEventStatus::Pending);
        assert!(restored.wait_seconds.unwrap() <= event.wait_seconds.unwrap());
        assert_eq!(
            sessions
                .load(runtime.session_id())
                .unwrap()
                .autonomous_run
                .unwrap()
                .status,
            crate::autonomous::AutonomousRunStatus::WaitingInput
        );
        assert_eq!(
            emitted_autonomous.recv().await.unwrap().status,
            crate::autonomous::AutonomousRunStatus::WaitingInput
        );

        let incomplete = runtime
            .answer_ask(
                &event.request_id,
                BTreeMap::from([(
                    "mode".to_string(),
                    crate::ipc::AskAnswer {
                        answers: vec!["A".to_string()],
                    },
                )]),
            )
            .expect_err("every related question must be answered");
        assert!(incomplete.contains("every question"));

        runtime
            .answer_ask(
                &event.request_id,
                BTreeMap::from([
                    (
                        "features".to_string(),
                        crate::ipc::AskAnswer {
                            answers: vec!["로그".to_string(), "직접 입력".to_string()],
                        },
                    ),
                    (
                        "mode".to_string(),
                        crate::ipc::AskAnswer {
                            answers: vec!["A".to_string()],
                        },
                    ),
                ]),
            )
            .unwrap();

        let result = task.await.unwrap().unwrap();
        assert_eq!(result["answers"]["mode"]["answers"], json!(["A"]));
        assert_eq!(
            result["answers"]["features"]["answers"],
            json!(["로그", "직접 입력"])
        );
        assert!(runtime.pending_ask().is_none());
        assert_eq!(
            sessions
                .load(runtime.session_id())
                .unwrap()
                .autonomous_run
                .unwrap()
                .status,
            crate::autonomous::AutonomousRunStatus::Running
        );
        assert_eq!(
            emitted_autonomous.recv().await.unwrap().status,
            crate::autonomous::AutonomousRunStatus::Running
        );
    }

    #[tokio::test]
    async fn dropped_ask_future_releases_the_session_slot() {
        let runtime = SessionToolRuntime::for_tests();
        runtime.begin_request("req-ask-drop", "project").unwrap();
        let (events, mut emitted) = tokio::sync::mpsc::unbounded_channel();
        runtime.set_ask_emitter(move |event| {
            events
                .send(event)
                .map_err(|_| "ask event receiver closed".to_string())
        });
        let args = json!({
            "questions": [{
                "id": "mode",
                "question": "방식을 고르세요.",
                "options": [{"label": "A"}, {"label": "B"}]
            }]
        });

        let first_runtime = runtime.clone();
        let first_args = args.clone();
        let first = tokio::spawn(async move { first_runtime.ask(&first_args).await });
        let first_event = emitted.recv().await.expect("first ask event");
        assert_eq!(first_event.request_id, "ask-1");
        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());

        let second_runtime = runtime.clone();
        let second_args = args.clone();
        let second = tokio::spawn(async move { second_runtime.ask(&second_args).await });
        let second_event = tokio::time::timeout(Duration::from_secs(1), emitted.recv())
            .await
            .expect("a dropped ask must release the session slot")
            .expect("second ask event");
        assert_eq!(second_event.request_id, "ask-2");

        runtime.cancel_pending_ask();
        assert_eq!(second.await.unwrap().unwrap_err(), "ask request cancelled");
    }

    fn ask_args() -> Value {
        json!({
            "questions": [{
                "id": "mode",
                "question": "방식을 고르세요.",
                "options": [{"label": "A"}, {"label": "B"}]
            }]
        })
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unanswered_ask_expires_into_a_text_handoff() {
        // Native CLIs abort a silent MCP call after 300s, so the wait is bounded
        // and the model continues by restating the question as plain text.
        let runtime = SessionToolRuntime::for_tests();
        runtime.set_ask_wait_timeout(Duration::from_millis(50));
        runtime.begin_request("req-ask-expire", "project").unwrap();
        let (events, mut emitted) = tokio::sync::mpsc::unbounded_channel();
        runtime.set_ask_emitter(move |event| {
            events
                .send(event)
                .map_err(|_| "ask event receiver closed".to_string())
        });
        let identity = crate::provider_runtime::RunIdentity {
            session_id: runtime.session_id().to_string(),
            run_id: crate::provider_runtime::RunId::new(1),
            request_id: "req-ask-expire".to_string(),
            session_kind: crate::session::SessionKind::Eps,
            cancellation_generation: 0,
        };
        let (_cancel, receiver) = tokio::sync::watch::channel(0_u64);
        runtime.set_cancellation(receiver);
        let mut waiting = runtime.subscribe_ask_waiting();

        let outcome = runtime.ask_for_run(&identity, &ask_args()).await.unwrap();
        assert_eq!(outcome["status"], "unanswered");
        assert_eq!(outcome["questionIds"], json!(["mode"]));
        assert_eq!(outcome["waitedSeconds"], json!(0));
        let pending = emitted.recv().await.unwrap();
        assert_eq!(pending.status, crate::ipc::AskEventStatus::Pending);
        assert_eq!(pending.wait_seconds, Some(0));
        assert!(pending.questions[0].id == "mode");
        let expired = emitted.recv().await.unwrap();
        assert_eq!(expired.request_id, pending.request_id);
        assert_eq!(expired.status, crate::ipc::AskEventStatus::Expired);
        assert_eq!(expired.questions, pending.questions);
        assert!(runtime.pending_ask().is_none());
        assert!(!*waiting.borrow_and_update());
        assert!(runtime.ask_expired_for_request("req-ask-expire"));

        let late = runtime
            .answer_ask(&pending.request_id, BTreeMap::new())
            .expect_err("an expired ask cannot be answered");
        assert!(late.starts_with("ask_expired"), "{late}");

        let again = runtime
            .ask_for_run(&identity, &ask_args())
            .await
            .expect_err("the same run cannot ask again after an expiry");
        assert!(again.contains("unanswered"), "{again}");

        runtime.begin_iteration("req-ask-expire").unwrap();
        assert!(!runtime.ask_expired_for_request("req-ask-expire"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn restored_pending_ask_reports_the_remaining_wait() {
        let runtime = SessionToolRuntime::for_tests();
        runtime.set_ask_wait_timeout(Duration::from_secs(30));
        runtime.begin_request("req-ask-restore", "project").unwrap();
        runtime.set_ask_emitter(|_| Ok(()));
        let asking = runtime.clone();
        let task = tokio::spawn(async move { asking.ask(&ask_args()).await });
        tokio::time::sleep(Duration::from_millis(1_100)).await;
        let restored = runtime.pending_ask().expect("ask is still pending");
        let remaining = restored.wait_seconds.unwrap();
        assert!(
            (25..=29).contains(&remaining),
            "restored wait must be the remainder, got {remaining}"
        );
        runtime.clear_expired_ask();
        runtime.cancel_pending_ask();
        assert_eq!(task.await.unwrap().unwrap_err(), "ask request cancelled");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ask_answered_before_expiry_keeps_the_answer_and_emits_no_expiry() {
        let runtime = SessionToolRuntime::for_tests();
        runtime.set_ask_wait_timeout(Duration::from_millis(80));
        runtime.begin_request("req-ask-race", "project").unwrap();
        let (events, mut emitted) = tokio::sync::mpsc::unbounded_channel();
        runtime.set_ask_emitter(move |event| {
            events
                .send(event)
                .map_err(|_| "ask event receiver closed".to_string())
        });
        let asking = runtime.clone();
        let task = tokio::spawn(async move { asking.ask(&ask_args()).await });
        let event = emitted.recv().await.unwrap();
        runtime
            .answer_ask(
                &event.request_id,
                BTreeMap::from([(
                    "mode".to_string(),
                    crate::ipc::AskAnswer {
                        answers: vec!["B".to_string()],
                    },
                )]),
            )
            .unwrap();
        let outcome = task.await.unwrap().unwrap();
        assert_eq!(outcome["answers"]["mode"]["answers"], json!(["B"]));
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(
            emitted.try_recv().is_err(),
            "an answered ask must not also expire"
        );
        assert!(!runtime.ask_expired_for_request("req-ask-race"));
    }

    #[test]
    fn failed_read_write_intent_cannot_become_a_ghost_owner() {
        let services = ToolServices::for_tests();
        let runtime = services.session("only-session");
        runtime.begin_request("request-old", "project").unwrap();
        let old = runtime.register_write_request("write after read").unwrap();
        assert_eq!(old.state(), crate::write_coordinator::TicketState::Granted);

        let error = runtime
            .begin_request("request-new", "project")
            .expect_err("an active ticket must never be silently discarded");
        assert!(error.contains("request-old"));

        runtime.abort_unmutated_write_intent().unwrap();
        runtime.clear_current();
        runtime.begin_request("request-new", "project").unwrap();
        let next = runtime.register_write_request("retry write").unwrap();
        assert_eq!(
            next.state(),
            crate::write_coordinator::TicketState::Granted,
            "the only session must not queue behind its failed stale request"
        );
    }

    #[test]
    fn every_write_workspace_tool_requires_the_exact_session_write_lease() {
        let runtime = SessionToolRuntime::for_tests();
        runtime.begin_request("req-read-only", "project").unwrap();

        for spec in tools::tool_registry()
            .into_iter()
            .filter(|spec| spec.requires_write_workspace)
        {
            let error = runtime
                .execute(spec.name, &json!({}))
                .expect_err("read mode must reject every project mutation");
            assert!(
                error.starts_with("WriteRegistrationRequired:"),
                "{} bypassed write registration: {error}",
                spec.name
            );
        }
    }

    #[test]
    fn opening_one_session_request_does_not_clear_another_sessions_state() {
        let services = ToolServices::for_tests();
        let session_a = services.session("session-a");
        let session_b = services.session("session-b");
        session_a.begin_request("request-a", "project").unwrap();
        {
            let mut state = session_a.request_state.lock();
            let state = state.as_mut().expect("session A state");
            state.record_search_docs();
            state.iteration_action_count = 7;
        }

        session_b.begin_request("request-b", "project").unwrap();

        let state_a = session_a
            .request_state_snapshot()
            .expect("session B must not clear session A");
        assert!(state_a.docs_searched);
        assert_eq!(state_a.iteration_action_count, 7);
        assert_eq!(
            session_b.request_state_snapshot().unwrap().request_id,
            "request-b"
        );
    }

    #[test]
    fn search_docs_with_empty_index_returns_zero_hits_and_lifts_the_evidence_gate() {
        let runtime = open_runtime("req-search");

        // A mutating call BEFORE any search is blocked by the evidence gate.
        let before = runtime
            .execute(
                "dat_patch",
                &json!({"changes": [{
                    "kind": "dat", "dat": "units", "objectId": 0,
                    "field": "Hit Points", "before": 10240, "after": 20480
                }]}),
            )
            .expect_err("dat_patch before search must hit the evidence gate");
        assert!(before.contains("evidence gate"), "got: {before}");

        // search_docs runs (zero hits on the empty test index) and lifts the gate.
        let result = runtime
            .execute("search_docs", &json!({"query": "마린 생성"}))
            .expect("search_docs should succeed even with an empty index");
        assert_eq!(result["count"], 0);

        // The same mutation now passes admission and reaches native project resolution.
        let after = runtime
            .execute(
                "dat_patch",
                &json!({"changes": [{
                    "kind": "dat", "dat": "units", "objectId": 0,
                    "field": "Hit Points", "before": 10240, "after": 20480
                }]}),
            )
            .expect_err("no native project is configured in the test runtime");
        assert!(
            !after.contains("evidence gate"),
            "the gate must be lifted after search_docs, got: {after}"
        );
    }

    #[test]
    fn progressive_docs_discovery_preserves_exact_reads_and_rejects_no_progress() {
        let full_text = format!("{} SelectionCircle {}", "앞".repeat(600), "뒤".repeat(600));
        let mut services = ToolServices::for_tests();
        services.rag = Arc::new(Rag::new(
            vec![crate::rag::IndexEntry {
                id: 0x123,
                vector: vec![0.0; crate::rag::EMBED_DIM],
                tier_level: 3,
                text: full_text.clone(),
                source: "[원문](https://example.test/doc)".to_string(),
            }],
            None,
        ));
        let runtime = services.session("progressive-docs-session");
        runtime
            .begin_request("req-progressive-docs", "project")
            .unwrap();

        let first = runtime
            .execute(
                tools::SEARCH_DOCS_TOOL,
                &json!({"query": "SelectionCircle", "k": 1}),
            )
            .unwrap();
        assert_eq!(first["count"], 1);
        assert_eq!(first["newCount"], 1);
        assert_eq!(first["repeatedCount"], 0);
        assert_eq!(first["hits"][0]["id"], "0000000000000123");
        assert_eq!(first["hits"][0]["tier"], "primary");
        assert_eq!(first["hits"][0]["match"], "lexical");
        assert_eq!(first["hits"][0]["repeated"], false);
        assert_eq!(first["hits"][0]["previewTruncated"], true);
        assert!(first["hits"][0]["preview"]
            .as_str()
            .unwrap()
            .contains("SelectionCircle"));
        assert!(
            first["hits"][0].get("text").is_none(),
            "discovery must not inject the complete chunk"
        );

        let exact_repeat = runtime
            .execute(
                tools::SEARCH_DOCS_TOOL,
                &json!({"query": "  selectioncircle  ", "k": 1}),
            )
            .unwrap_err();
        assert!(exact_repeat.contains("no-progress"));

        let no_novel_ids = runtime
            .execute(
                tools::SEARCH_DOCS_TOOL,
                &json!({"query": "Circle Selection", "k": 1}),
            )
            .unwrap_err();
        assert!(no_novel_ids.contains("no new stable"));

        let exact = runtime
            .execute(tools::DOCS_GET_TOOL, &json!({"ids": ["0000000000000123"]}))
            .unwrap();
        assert_eq!(exact["count"], 1);
        assert_eq!(exact["documents"][0]["text"], full_text);
        assert_eq!(
            exact["documents"][0]["source"],
            "[원문](https://example.test/doc)"
        );

        let state = runtime.request_state_snapshot().unwrap();
        assert_eq!(state.search_docs_count, 2);
        assert_eq!(state.search_docs_returned_hits, 2);
        assert_eq!(state.search_docs_unique_hits, 1);
        assert_eq!(state.search_docs_repeated_hits, 1);
        assert!(state.search_docs_result_bytes > 0);
        assert_eq!(state.docs_get_count, 1);
        assert_eq!(state.docs_get_documents, 1);
        assert!(state.docs_get_result_bytes > full_text.len());
    }

    #[test]
    fn propose_plan_parks_markdown_for_the_engine_to_pick_up() {
        let runtime = open_runtime("req-plan");
        let result = runtime
            .execute("propose_plan", &json!({"markdown": "# Plan\n1. do it"}))
            .expect("propose_plan should record the plan");
        assert_eq!(result["ok"], true);

        // The engine reads this after the turn to end as a plan review; it is a
        // one-shot take keyed by the open request id.
        assert_eq!(
            runtime.take_pending_plan("req-plan").as_deref(),
            Some("# Plan\n1. do it")
        );
        assert_eq!(runtime.take_pending_plan("req-plan"), None);
    }

    #[test]
    fn unknown_tool_is_rejected_with_a_clear_message() {
        let runtime = open_runtime("req-unknown");
        let error = runtime
            .execute("teleport", &json!({}))
            .expect_err("an unregistered tool must be rejected");
        assert!(error.contains("Unknown tool"), "got: {error}");
    }

    #[test]
    fn moved_and_sibling_paths_keep_the_leaf() {
        assert_eq!(sibling_path("folder/a.eps", "b.eps"), "folder/b.eps");
        assert_eq!(sibling_path("a.eps", "b.eps"), "b.eps");
        assert_eq!(moved_path("folder/a.eps", "dest"), "dest/a.eps");
        assert_eq!(moved_path("folder/a.eps", ""), "a.eps");
    }

    #[test]
    #[ignore = "requires installed StarCraft terrain assets"]
    fn map_palette_query_rejects_catalog_walks_and_returns_complete_filtered_tiles() {
        let root = std::env::temp_dir().join(format!("map-palette-tool-{}", uuid::Uuid::new_v4()));
        let dirs = DataDirs::from_bases(&root.join("roaming"), &root.join("local"));
        dirs.ensure_dirs().unwrap();
        let source = root.join("source.scx");
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("crates")
            .join("isom")
            .join("tests")
            .join("fixtures")
            .join("map_agent_rich.scx");
        std::fs::copy(fixture, &source).unwrap();
        let context_service = crate::map_context::MapContextService::new(dirs.clone());
        let revision = context_service
            .revision_for_path("project".to_string(), &source)
            .unwrap();
        let chk = isom::chk_extract(&source).unwrap();
        let context = crate::map_context::MapContextSnapshot {
            revision,
            saved_source_notice: "saved".to_string(),
            source_file_size: std::fs::metadata(&source).unwrap().len(),
            starcraft_path: PathBuf::from(r"C:\Program Files (x86)\StarCraft"),
            digest: crate::chk::digest_chk(&chk),
        };
        let candidates = crate::map_candidate::CandidateStore::new(
            (dirs.clone()).clone(),
            crate::map_import::MapImportStore::new(dirs.clone()),
        );
        candidates.create_session("map-session", &context).unwrap();
        candidates
            .prepare_request("project", "map-session", "request", 0, &[])
            .unwrap();
        let services = ToolServices::new(
            dirs,
            candidates.clone(),
            crate::write_coordinator::ProjectWriteCoordinator::silent(),
        );
        let runtime = services.map_session("map-session");
        runtime.begin_request("request", "project").unwrap();

        let broad = runtime
            .execute(
                "map_palette_query",
                &json!({"kind": "tiles", "query": "Tile"}),
            )
            .unwrap_err();
        assert!(broad.contains("refine query/filter"), "got: {broad}");

        let filtered = runtime
            .execute(
                "map_palette_query",
                &json!({"kind": "tiles", "filter": {"group": 0}}),
            )
            .unwrap();
        assert_eq!(filtered["total"], 16);
        assert_eq!(filtered["entries"].as_array().unwrap().len(), 16);
        assert!(filtered["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["group"] == 0));

        let paginated = runtime
            .execute(
                "map_palette_query",
                &json!({"kind": "tiles", "query": "Tile", "offset": 100}),
            )
            .unwrap_err();
        assert!(paginated.contains("does not accept pagination"));

        candidates.finish_request("map-session", "request").unwrap();
        runtime.clear_current();
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    #[ignore = "requires installed StarCraft terrain assets"]
    fn request_local_image_refs_support_multiple_images_and_terrain_patches_in_one_draft() {
        let root = std::env::temp_dir().join(format!("map-image-tool-{}", uuid::Uuid::new_v4()));
        let dirs = DataDirs::from_bases(&root.join("roaming"), &root.join("local"));
        dirs.ensure_dirs().unwrap();
        let source = root.join("source.scx");
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("crates")
            .join("isom")
            .join("tests")
            .join("fixtures")
            .join("map_agent_rich.scx");
        std::fs::copy(fixture, &source).unwrap();
        let context_service = crate::map_context::MapContextService::new(dirs.clone());
        let revision = context_service
            .revision_for_path("project".to_string(), &source)
            .unwrap();
        let chk = isom::chk_extract(&source).unwrap();
        let context = crate::map_context::MapContextSnapshot {
            revision,
            saved_source_notice: "saved".to_string(),
            source_file_size: std::fs::metadata(&source).unwrap().len(),
            starcraft_path: PathBuf::from(r"C:\Program Files (x86)\StarCraft"),
            digest: crate::chk::digest_chk(&chk),
        };
        let candidates = crate::map_candidate::CandidateStore::new(
            (dirs.clone()).clone(),
            crate::map_import::MapImportStore::new(dirs.clone()),
        );
        candidates.create_session("map-session", &context).unwrap();
        candidates
            .prepare_request("project", "map-session", "request", 0, &[])
            .unwrap();
        let services = ToolServices::new(
            dirs.clone(),
            candidates.clone(),
            crate::write_coordinator::ProjectWriteCoordinator::silent(),
        );
        let runtime = services.map_session("map-session");
        runtime.begin_request("request", "project").unwrap();

        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png, 2, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer
                .write_image_data(&[255, 0, 0, 255, 0, 0, 255, 255])
                .unwrap();
        }
        let attachments = crate::attachment::AttachmentStore::new(dirs.attachments_dir());
        let first = attachments.stage("first.png", "image/png", &png).unwrap();
        let second = attachments.stage("second.png", "image/png", &png).unwrap();
        let attachment_context = attachments
            .bind_and_resolve(&[first.id.clone(), second.id.clone()], "map-session")
            .unwrap();
        let refs = runtime
            .bind_map_images("request", &attachment_context.images)
            .unwrap();
        assert_eq!(
            refs.iter()
                .map(|reference| reference.image_ref.as_str())
                .collect::<Vec<_>>(),
            ["image-1", "image-2"]
        );
        assert!(serde_json::to_value(&refs).unwrap()[0]
            .get("attachmentId")
            .is_none());
        assert!(attachments
            .bind_and_resolve(&[first.id], "other-session")
            .unwrap_err()
            .contains("다른 대화"));

        let first_result = runtime
            .execute(
                "map_image_place",
                &json!({"imageRef": "image-1", "x": 0, "y": 0, "width": 2, "height": 1}),
            )
            .unwrap();
        assert_eq!(first_result["report"]["placement"]["width"], 2);
        let draft = candidates.draft_map("map-session", "request").unwrap();
        let draft_chk = isom::chk_extract(&draft).unwrap();
        let draft_digest = crate::chk::digest_chk(&draft_chk);
        let patch_x = 10_u16;
        let patch_y = 10_u16;
        let before = draft_digest.tiles
            [usize::from(patch_y) * usize::from(context.revision.width) + usize::from(patch_x)];
        let catalog: Value = serde_json::from_str(
            &isom::catalog_query(
                &context.starcraft_path,
                json!({
                    "schema": "eud-map-catalog/1",
                    "kind": "tiles",
                    "tileset": context.revision.tileset.era(),
                    "offset": 0,
                    "limit": 512,
                })
                .to_string()
                .as_bytes(),
            )
            .unwrap(),
        )
        .unwrap();
        let after = catalog["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["graphicsValid"] == true && entry["id"] != before)
            .and_then(|entry| entry["id"].as_u64())
            .unwrap() as u16;
        runtime
            .execute(
                "map_draft_patch",
                &json!({
                    "operations": [{
                        "op": "terrain.set",
                        "x": patch_x,
                        "y": patch_y,
                        "before": before,
                        "after": after,
                    }]
                }),
            )
            .unwrap();
        runtime
            .execute(
                "map_image_place",
                &json!({"imageRef": "image-2", "x": 0, "y": 2, "width": 2, "height": 1}),
            )
            .unwrap();
        runtime
            .execute("map_candidate_finalize", &json!({}))
            .unwrap();
        let committed = candidates
            .commit_request("project", "map-session", "request")
            .unwrap();
        assert_eq!(committed.current_revision, 1);
        let manifest = dirs
            .map_candidates_dir()
            .join("project")
            .join("map-session")
            .join("revisions")
            .join("r0001.json");
        let manifest: Value = serde_json::from_slice(&std::fs::read(manifest).unwrap()).unwrap();
        assert_eq!(manifest["imageConversions"].as_array().unwrap().len(), 2);
        assert_eq!(manifest["batches"].as_array().unwrap().len(), 3);

        candidates.finish_request("map-session", "request").unwrap();
        runtime.clear_current();
        candidates
            .prepare_request("project", "map-session", "request-2", 1, &[])
            .unwrap();
        runtime.begin_request("request-2", "project").unwrap();
        let stale_ref = runtime
            .execute(
                "map_image_place",
                &json!({"imageRef": "image-1", "x": 0, "y": 0, "width": 2, "height": 1}),
            )
            .unwrap_err();
        assert!(stale_ref.contains("not bound to the current Map Agent request"));
        candidates
            .finish_request("map-session", "request-2")
            .unwrap();
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn eps_and_map_runtimes_keep_requests_and_tool_surfaces_isolated() {
        let services = ToolServices::for_tests();
        let eps = services.session("eps-session");
        let map = services.map_session("map-session");
        eps.begin_request("eps-request", "project").unwrap();
        map.begin_request("map-request", "project").unwrap();
        assert_eq!(eps.kind(), crate::session::SessionKind::Eps);
        assert_eq!(map.kind(), crate::session::SessionKind::Map);
        assert_eq!(eps.current_request_id().as_deref(), Some("eps-request"));
        assert_eq!(map.current_request_id().as_deref(), Some("map-request"));
        assert!(eps.execute("map_status", &json!({})).is_err());
        let error = map
            .execute("file_write", &json!({"path": "main", "code": ""}))
            .unwrap_err();
        assert!(error.contains("not available to Map Agent"));
        assert!(!crate::tools::map_tool_registry()
            .iter()
            .any(|tool| tool.name.contains("apply")));
    }

    #[test]
    fn map_runtime_rejects_palette_mention_kind_before_native_dispatch() {
        let services = ToolServices::for_tests();
        let runtime = services.map_session("map-session");
        runtime.begin_request("map-request", "project").unwrap();

        let error = runtime
            .execute(
                "map_palette_query",
                &json!({"kind": "semanticTerrain", "query": "Space"}),
            )
            .unwrap_err();

        assert!(error.contains("semanticTerrain"), "got: {error}");
        assert!(error.contains("brushes"), "got: {error}");
        assert!(
            !error.contains("unsupported catalog kind"),
            "invalid model input reached the native catalog: {error}"
        );
    }
    #[test]
    fn map_palette_query_builds_one_complete_bounded_search() {
        let request = map_palette_catalog_request(
            &json!({
                "kind": "tiles",
                "query": "  Tile 12 ",
                "filter": {
                    "terrainType": 3,
                    "graphicsValid": true,
                    "walkability": "all",
                },
            }),
            4,
        )
        .unwrap();
        assert_eq!(request["kind"], "tiles");
        assert_eq!(request["tileset"], 4);
        assert_eq!(request["query"], "Tile 12");
        assert_eq!(request["offset"], 0);
        assert_eq!(request["limit"], (MAP_PALETTE_QUERY_MAX_MATCHES + 1) as u64);
        assert_eq!(request["filter"]["terrainType"], 3);

        for args in [
            json!({"kind": "tiles"}),
            json!({"kind": "tiles", "query": " "}),
            json!({"kind": "tiles", "filter": {}}),
            json!({"kind": "tiles", "query": "Tile", "offset": 100}),
            json!({"kind": "tiles", "query": "Tile", "limit": 10}),
        ] {
            assert!(
                map_palette_catalog_request(&args, 0).is_err(),
                "broad or paginated search must fail: {args}"
            );
        }

        let complete = json!({
            "total": MAP_PALETTE_QUERY_MAX_MATCHES,
            "entries": vec![Value::Null; MAP_PALETTE_QUERY_MAX_MATCHES],
        });
        assert!(enforce_map_palette_result_bound(complete).is_ok());
        let broad = json!({
            "total": MAP_PALETTE_QUERY_MAX_MATCHES + 1,
            "entries": vec![Value::Null; MAP_PALETTE_QUERY_MAX_MATCHES + 1],
        });
        let error = enforce_map_palette_result_bound(broad).unwrap_err();
        assert!(error.contains("refine query/filter"));
    }

    #[test]
    fn render_scale_rejects_unsupported_values_with_actionable_error() {
        assert_eq!(render_scale_arg(&json!({})), Ok(4));
        for scale in [1, 2, 4, 8] {
            assert_eq!(render_scale_arg(&json!({"scale": scale})), Ok(scale));
        }
        assert_eq!(
            render_scale_arg(&json!({"scale": 3})),
            Err("map render scale must be 1, 2, 4, or 8".to_string())
        );
    }
    #[test]
    #[ignore = "requires checksum-pinned managed FFmpeg/FFprobe in LocalAppData"]
    fn audio_refs_are_exactly_session_and_request_bound_without_prompt_secrets() {
        let services = ToolServices::for_tests();
        let dirs = services.dirs.clone();
        dirs.ensure_dirs().unwrap();
        let installed = DataDirs::from_bases(
            std::path::Path::new(&std::env::var("APPDATA").unwrap()),
            std::path::Path::new(&std::env::var("LOCALAPPDATA").unwrap()),
        );
        for name in ["ffmpeg.exe", "ffprobe.exe"] {
            std::fs::hard_link(installed.bin_dir().join(name), dirs.bin_dir().join(name)).unwrap();
        }
        let attachment_store = crate::attachment::AttachmentStore::new(dirs.attachments_dir());
        let tone = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("crates")
                .join("isom")
                .join("tests")
                .join("fixtures")
                .join("tone.ogg"),
        )
        .unwrap();
        let descriptor = attachment_store
            .stage("테마.ogg", "application/octet-stream", &tone)
            .unwrap();
        let context = attachment_store
            .bind_and_resolve(std::slice::from_ref(&descriptor.id), "session-a")
            .unwrap();

        let runtime_a = services.session("session-a");
        runtime_a.begin_request("request-a", "project").unwrap();
        let refs = runtime_a
            .bind_audio_attachments("request-a", context.audio_files)
            .unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].audio_ref, "audio-1");
        let visible = serde_json::to_string(&refs).unwrap();
        assert!(visible.contains("audio-1"));
        assert!(!visible.contains(&descriptor.id));
        assert!(!visible.contains("sha256"));
        assert!(!visible.contains("audio_temp"));
        assert!(runtime_a.audio_binding("request-a", "audio-1").is_ok());

        let runtime_b = services.session("session-b");
        runtime_b.begin_request("request-b", "project").unwrap();
        assert!(runtime_b.audio_binding("request-b", "audio-1").is_err());

        runtime_a.begin_request("request-a-2", "project").unwrap();
        assert!(runtime_a.audio_binding("request-a-2", "audio-1").is_err());
        let base = dirs.app_data().parent().unwrap().to_path_buf();
        std::fs::remove_dir_all(base).ok();
    }
    #[test]
    fn managed_sound_path_is_ascii_content_addressed_and_extends_on_collision() {
        let normalized = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let empty = BTreeMap::new();
        assert_eq!(
            select_managed_sound_path_from_inventory(&empty, normalized).unwrap(),
            "staredit\\wav\\ea_0123456789abcdef.ogg"
        );

        let mut collision = BTreeMap::new();
        collision.insert(
            "staredit\\wav\\ea_0123456789abcdef.ogg".to_string(),
            "f".repeat(64),
        );
        assert_eq!(
            select_managed_sound_path_from_inventory(&collision, normalized).unwrap(),
            "staredit\\wav\\ea_0123456789abcdef01234567.ogg"
        );

        collision.insert(
            "staredit\\wav\\ea_0123456789abcdef01234567.ogg".to_string(),
            normalized.to_string(),
        );
        assert_eq!(
            select_managed_sound_path_from_inventory(&collision, normalized).unwrap(),
            "staredit\\wav\\ea_0123456789abcdef01234567.ogg"
        );
        assert!(managed_sound_hash("staredit\\wav\\ea_ABCDEF0123456789.ogg").is_none());
        assert!(select_managed_sound_path_from_inventory(&empty, "bad").is_err());

        let old_path = "staredit\\wav\\ea_aaaaaaaaaaaaaaaa.ogg";
        let mut replacement_inventory = BTreeMap::new();
        replacement_inventory.insert(old_path.to_string(), "a".repeat(64));
        replacement_inventory.insert(
            "staredit\\wav\\ea_0123456789abcdef.ogg".to_string(),
            normalized.to_string(),
        );
        assert_eq!(
            select_managed_sound_replacement_path_from_inventory(
                &replacement_inventory,
                old_path,
                normalized,
            )
            .unwrap(),
            "staredit\\wav\\ea_0123456789abcdef01234567.ogg"
        );
    }
}
