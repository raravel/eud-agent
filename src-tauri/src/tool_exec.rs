//! Shared tool services and one request runtime per conversation session.
//!
//! [`ToolServices`] owns app-wide immutable/shared services. Each
//! [`SessionToolRuntime`] is cloned only by one session engine and its MCP
//! handler, so request ids, evidence/plan/budget gates, preflight state, and
//! write tickets cannot overwrite another session.
//!
//! [`SessionToolRuntime::execute`] is the single tool entry point. It verifies
//! concurrent write registration, serializes each shared-state operation, applies
//! validation and safety gates, and journals every project write for review.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};

use crate::bridge_io::{BridgeIo, SendOpts, HEARTBEAT_STALE_AFTER};
use crate::config::DataDirs;
use crate::edd_runner;
use crate::eps_preflight::{EpsAnalyzer, EpsCandidateInput, EpsPreflight};
use crate::journal::{DatTable, JournalEntry, JournalStore, JournalTarget, Snapshot, WriteTool};
use crate::mapsafe::{CompilingStatus, IsomEngine, MapSafe, WindowsLockProbe};
use crate::rag::Rag;
use crate::tools::{self, RequestState};
use crate::workspace::{apply_exact_text_edits, ExactTextEdit};

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

/// Editor build-state probe backed by the editor `status.txt`, resolved from
/// `config.json` on each read (the editor path can change at runtime, and the
/// editor may be down). A read failure reports NOT compiling — the map-write
/// lock probe and the bridge's own compiling guard remain as independent rails.
#[derive(Clone)]
pub struct BridgeCompilingStatus {
    dirs: DataDirs,
}

impl CompilingStatus for BridgeCompilingStatus {
    fn is_compiling(&self) -> bool {
        crate::ipc::bridge_from_config(&self.dirs)
            .ok()
            .and_then(|bridge| bridge.read_status_snapshot(HEARTBEAT_STALE_AFTER).ok())
            .map(|snapshot| snapshot.compiling)
            .unwrap_or(false)
    }
}

/// Production map-write service: bridge-backed compiling guard, Windows share
/// probe, isom static-lib engine.
pub type ProductionMapSafe = MapSafe<BridgeCompilingStatus, WindowsLockProbe, IsomEngine>;

/// Shared, immutable production services. Session workers clone these handles,
/// while every request gate, plan, preflight snapshot, and write ticket remains
/// inside a [`SessionToolRuntime`].
#[derive(Clone)]
pub struct ToolServices {
    dirs: DataDirs,
    journal: JournalStore,
    rag: Arc<Rag>,
    map_safe: Arc<ProductionMapSafe>,
    analyzer: Arc<dyn EpsAnalyzer>,
    writes: crate::write_coordinator::ProjectWriteCoordinator,
    map_candidates: crate::map_candidate::CandidateStore,
    map_images: crate::map_image::MapImageService,
    audio: crate::audio::AudioService,
    mentions: crate::mentions::MentionService,
}

impl ToolServices {
    pub fn new(
        dirs: DataDirs,
        analyzer: Arc<dyn EpsAnalyzer>,
        map_candidates: crate::map_candidate::CandidateStore,
        writes: crate::write_coordinator::ProjectWriteCoordinator,
    ) -> Self {
        let journal = JournalStore::new(dirs.app_data());
        let rag = Arc::new(load_rag(&dirs));
        let map_safe = Arc::new(MapSafe::new(
            dirs.app_data().to_path_buf(),
            BridgeCompilingStatus { dirs: dirs.clone() },
            WindowsLockProbe,
            IsomEngine,
        ));
        let mentions = crate::mentions::MentionService::new(
            map_candidates.clone(),
            crate::map_context::MapContextService::new(dirs.clone()),
        );
        let audio = crate::audio::AudioService::new(dirs.clone());
        Self {
            dirs,
            journal,
            rag,
            map_safe,
            analyzer,
            map_candidates,
            audio,
            map_images: crate::map_image::MapImageService::new(),
            mentions,
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
}

#[derive(Debug, Clone)]
struct SessionRequest {
    request_id: String,
    project_id: String,
    workspace_root: Option<PathBuf>,
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

struct PendingAsk {
    owner_request_id: String,
    questions: Vec<crate::ipc::AskQuestion>,
    response: tokio::sync::oneshot::Sender<Result<BTreeMap<String, crate::ipc::AskAnswer>, String>>,
}

#[derive(Default)]
struct AskState {
    next_id: u64,
    emitter: Option<AskEmitter>,
    pending: HashMap<String, PendingAsk>,
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
    eps_preflight: Arc<EpsPreflight>,
    request: Arc<Mutex<Option<SessionRequest>>>,
    request_state: Arc<Mutex<Option<RequestState>>>,
    pending_plan: Arc<Mutex<Option<(String, String)>>>,
    write_state: Arc<Mutex<SessionWriteState>>,
    execution_lock: Arc<Mutex<()>>,
    ask: Arc<Mutex<AskState>>,
    ask_waiting: tokio::sync::watch::Sender<bool>,
    cancellation: Arc<Mutex<Option<tokio::sync::watch::Receiver<u64>>>>,
    progress_emitter: Arc<Mutex<Option<ProgressEmitter>>>,
    last_build: Arc<Mutex<Option<crate::harness::BuildEvidence>>>,
    sound_build_required: Arc<Mutex<bool>>,
    sound_preflight_required: Arc<Mutex<bool>>,
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
        let eps_preflight = Arc::new(EpsPreflight::new(
            services.dirs.clone(),
            Arc::clone(&services.analyzer),
        ));
        let (ask_waiting, _) = tokio::sync::watch::channel(false);
        Self {
            services,
            session_id,
            kind,
            eps_preflight,
            request: Arc::new(Mutex::new(None)),
            request_state: Arc::new(Mutex::new(None)),
            pending_plan: Arc::new(Mutex::new(None)),
            write_state: Arc::new(Mutex::new(SessionWriteState::default())),
            execution_lock: Arc::new(Mutex::new(())),
            ask: Arc::new(Mutex::new(AskState::default())),
            ask_waiting,
            cancellation: Arc::new(Mutex::new(None)),
            progress_emitter: Arc::new(Mutex::new(None)),
            last_build: Arc::new(Mutex::new(None)),
            sound_build_required: Arc::new(Mutex::new(false)),
            sound_preflight_required: Arc::new(Mutex::new(false)),
        }
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    pub fn kind(&self) -> crate::session::SessionKind {
        self.kind
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

    pub fn set_cancellation(&self, cancellation: tokio::sync::watch::Receiver<u64>) {
        *self.cancellation.lock() = Some(cancellation);
    }
    pub fn subscribe_ask_waiting(&self) -> tokio::sync::watch::Receiver<bool> {
        self.ask_waiting.subscribe()
    }

    fn emit_audio_progress(&self, stage: crate::ipc::ProgressStage, detail: &str) {
        if let Some(emitter) = self.progress_emitter.lock().clone() {
            let _ = emitter(crate::ipc::ProgressEvent {
                stage,
                detail: Some(detail.to_string()),
            });
        }
    }

    pub async fn ask(&self, args: &Value) -> Result<Value, String> {
        let owner_request_id = self.current_request_id().ok_or_else(|| {
            "no agent request is open; ask is only valid during a turn".to_string()
        })?;
        let input: AskToolInput = serde_json::from_value(args.clone())
            .map_err(|error| format!("invalid ask arguments: {error}"))?;
        validate_ask_questions(&input.questions)?;

        let (request_id, response, emitter) = {
            let mut ask = self.ask.lock();
            if !ask.pending.is_empty() {
                return Err("another ask request is already waiting for this session".to_string());
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
                    owner_request_id,
                    questions: input.questions.clone(),
                    response: send,
                },
            );
            self.ask_waiting.send_replace(true);
            (request_id, response, emitter)
        };
        let _lease = PendingAskLease {
            runtime: self.clone(),
            request_id: request_id.clone(),
        };

        self.emit_activity(crate::write_coordinator::SessionActivity::WaitingInput);
        emitter(crate::ipc::AskEvent {
            request_id,
            questions: input.questions,
        })?;

        let answers = response
            .await
            .map_err(|_| "ask response channel closed".to_string())??;
        Ok(json!({ "answers": answers }))
    }

    pub fn pending_ask(&self) -> Option<crate::ipc::AskEvent> {
        let owner_request_id = self.current_request_id()?;
        let ask = self.ask.lock();
        ask.pending.iter().find_map(|(request_id, pending)| {
            (pending.owner_request_id == owner_request_id).then(|| crate::ipc::AskEvent {
                request_id: request_id.clone(),
                questions: pending.questions.clone(),
            })
        })
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

    fn emit_activity_after_ask(&self) {
        let activity = if self.write_ticket().is_some() {
            crate::write_coordinator::SessionActivity::RunningWrite
        } else {
            crate::write_coordinator::SessionActivity::RunningRead
        };
        self.emit_activity(activity);
    }
    pub fn begin_request(&self, request_id: &str, project_id: &str) -> Result<(), String> {
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
            workspace_root: None,
            image_refs: BTreeMap::new(),
            audio_refs: BTreeMap::new(),
            audio_temp: None,
        });
        *self.request_state.lock() = Some(RequestState::for_request(request_id));
        *self.pending_plan.lock() = None;
        *self.last_build.lock() = None;
        *self.sound_build_required.lock() = false;
        *self.sound_preflight_required.lock() = false;
        self.eps_preflight.begin_request(request_id);
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
            self.emit_audio_progress(
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

    pub fn sound_build_required(&self) -> bool {
        *self.sound_build_required.lock()
    }

    pub fn bind_workspace_root(
        &self,
        request_id: &str,
        workspace_root: PathBuf,
    ) -> Result<(), String> {
        let mut request = self.request.lock();
        let active = request
            .as_mut()
            .filter(|request| request.request_id == request_id)
            .ok_or_else(|| format!("request {request_id} is not active"))?;
        active.workspace_root = Some(workspace_root);
        Ok(())
    }

    fn source_baseline(&self, path: &str) -> Result<Option<String>, String> {
        let workspace_root = self
            .request
            .lock()
            .as_ref()
            .and_then(|request| request.workspace_root.clone())
            .ok_or_else(|| "the current request has no prepared session workspace".to_string())?;
        crate::workspace::read_source_baseline(&workspace_root, path)
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

    pub fn request_write_workspace(
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
        if !self.ask.lock().pending.is_empty() {
            return Err(
                "a user answer is pending; wait for the ask tool to complete before calling another tool"
                    .to_string(),
            );
        }
        let _execution = self.execution_lock.lock();
        let request_id = self.current_request_id().ok_or_else(|| {
            "no agent request is open; tool calls are only valid during a turn".to_string()
        })?;
        if self.kind == crate::session::SessionKind::Map {
            tools::validate_map_tool_call(tool, args).map_err(|error| error.to_string())?;
            return self.dispatch_map(&request_id, tool, args);
        }

        if tools::is_mutating_tool(tool) && !self.owns_write_registration() {
            return Err(
                "WriteRegistrationRequired: call request_write_workspace with the reason for the change, \
stop this turn so the backend can resume the same thread in its isolated writable workspace."
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

        let mut result = if matches!(
            tool,
            tools::MAP_SOUND_IMPORT_TOOL | tools::MAP_SOUND_EDIT_TOOL
        ) {
            if tool == tools::MAP_SOUND_IMPORT_TOOL {
                self.map_sound_import(&request_id, args)
            } else {
                self.map_sound_edit(&request_id, args)
            }
        } else if tools::is_mutating_tool(tool) {
            self.project_transaction(|| self.dispatch(&request_id, tool, args))?
        } else {
            self.dispatch(&request_id, tool, args)
        };
        if let Ok(value) = result.as_mut() {
            if tool == tools::SEARCH_DOCS_TOOL {
                self.record_search_docs_result(&request_id, value)?;
            } else if tool == tools::DOCS_GET_TOOL {
                self.record_docs_get_result(&request_id, value)?;
            }
        }
        result
    }

    fn record_search_docs_result(&self, request_id: &str, value: &mut Value) -> Result<(), String> {
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
        let repeated = state.record_search_docs_hits(&ids);
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

    fn bridge(&self) -> Result<BridgeIo, String> {
        crate::ipc::bridge_from_config(&self.services.dirs)
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
        let opts = SendOpts::default();
        match tool {
            // ---- read tools (no journal) ----
            "project_status" => {
                let bridge = self.bridge()?;
                let status = bridge.status(&opts, None).map_err(stringify)?;
                let main_file = bridge.get_main(&opts, None).map_err(stringify)?;
                Ok(json!({ "status": status.trim(), "mainFile": main_file }))
            }
            "list_files" => {
                let files = self.bridge()?.list(&opts, None).map_err(stringify)?;
                let items: Vec<Value> = files
                    .into_iter()
                    .map(|file| {
                        json!({ "path": file.path, "ftype": file.ftype, "settable": file.settable })
                    })
                    .collect();
                Ok(json!({ "count": items.len(), "files": items }))
            }
            "read_file" => self.read_file(args, &opts),
            tools::SOURCE_SEARCH_TOOL => self.source_search(args, &opts),
            tools::EPS_CHECK_TOOL => {
                let files: Vec<EpsCandidateInput> = serde_json::from_value(
                    args.get("files")
                        .cloned()
                        .ok_or_else(|| "missing argument 'files'".to_string())?,
                )
                .map_err(|error| format!("invalid eps_check files: {error}"))?;
                let result = self.eps_preflight.check_inputs(request_id, files)?;
                if *self.sound_build_required.lock() {
                    *self.sound_preflight_required.lock() = false;
                }
                serde_json::to_value(result)
                    .map_err(|error| format!("failed to serialize eps_check result: {error}"))
            }
            "dat_get" => {
                let bridge = self.bridge()?;
                let items = array_arg(args, "items")?;
                let mut results = Vec::with_capacity(items.len());
                for item in items {
                    let (dat, param, obj_id) = (
                        str_arg(item, "dat")?,
                        str_arg(item, "param")?,
                        i64_arg(item, "objId")?,
                    );
                    let result = match bridge.getdat(dat, param, obj_id, &opts, None) {
                        Ok(reply) => {
                            json!({"dat": dat, "param": param, "objId": obj_id, "ok": true, "value": reply_value(&reply)})
                        }
                        Err(error) => {
                            json!({"dat": dat, "param": param, "objId": obj_id, "ok": false, "error": error.to_string()})
                        }
                    };
                    results.push(result);
                }
                Ok(json!({"count": results.len(), "results": results}))
            }
            "xdat_get" => {
                let bridge = self.bridge()?;
                let items = array_arg(args, "items")?;
                let mut results = Vec::with_capacity(items.len());
                for item in items {
                    let (dat, name, obj_id) = (
                        str_arg(item, "dat")?,
                        str_arg(item, "name")?,
                        i64_arg(item, "objId")?,
                    );
                    let command = format!("GETXDAT {dat}|{name}|{obj_id}");
                    let result = match bridge.send(&command, &opts, None) {
                        Ok(reply) => {
                            json!({"dat": dat, "name": name, "objId": obj_id, "ok": true, "value": reply_value(&reply)})
                        }
                        Err(error) => {
                            json!({"dat": dat, "name": name, "objId": obj_id, "ok": false, "error": error.to_string()})
                        }
                    };
                    results.push(result);
                }
                Ok(json!({"count": results.len(), "results": results}))
            }
            "tbl_get" => {
                let bridge = self.bridge()?;
                let items = array_arg(args, "items")?;
                let mut results = Vec::with_capacity(items.len());
                for item in items {
                    let index = i64_arg(item, "index")?;
                    let command = format!("GETTBL {index}");
                    let result = match bridge.send(&command, &opts, None) {
                        Ok(reply) => {
                            json!({"index": index, "ok": true, "value": reply_value(&reply)})
                        }
                        Err(error) => {
                            json!({"index": index, "ok": false, "error": error.to_string()})
                        }
                    };
                    results.push(result);
                }
                Ok(json!({"count": results.len(), "results": results}))
            }
            "req_get" => {
                let bridge = self.bridge()?;
                let items = array_arg(args, "items")?;
                let mut results = Vec::with_capacity(items.len());
                for item in items {
                    let (dat, obj_id) = (str_arg(item, "dat")?, i64_arg(item, "objId")?);
                    let command = format!("GETREQ {dat}|{obj_id}");
                    let result = match bridge.send(&command, &opts, None) {
                        Ok(reply) => {
                            json!({"dat": dat, "objId": obj_id, "ok": true, "value": reply_value(&reply)})
                        }
                        Err(error) => {
                            json!({"dat": dat, "objId": obj_id, "ok": false, "error": error.to_string()})
                        }
                    };
                    results.push(result);
                }
                Ok(json!({"count": results.len(), "results": results}))
            }
            "btn_get" => {
                let bridge = self.bridge()?;
                let items = array_arg(args, "items")?;
                let mut results = Vec::with_capacity(items.len());
                for item in items {
                    let set_id = i64_arg(item, "setId")?;
                    let command = format!("GETBTN {set_id}");
                    let result = match bridge.send(&command, &opts, None) {
                        Ok(reply) => {
                            json!({"setId": set_id, "ok": true, "csv": reply_value(&reply)})
                        }
                        Err(error) => {
                            json!({"setId": set_id, "ok": false, "error": error.to_string()})
                        }
                    };
                    results.push(result);
                }
                Ok(json!({"count": results.len(), "results": results}))
            }
            "settings_get" => {
                let (scope, key) = (str_arg(args, "scope")?, str_arg(args, "key")?);
                let reply = self.send(&format!("GETSET {scope}|{key}"))?;
                Ok(json!({ "value": reply_value(&reply) }))
            }
            "plugins_list" => {
                let reply = self.send("PLUGLIST")?;
                Ok(json!({ "plugins": reply.trim() }))
            }
            tools::MAP_INFO_TOOL => {
                let bridge = self.bridge()?;
                tools::map_info(&bridge, args).map_err(stringify)
            }
            tools::MAP_MINIMAP_TOOL => {
                let bridge = self.bridge()?;
                tools::map_minimap(&bridge, args).map_err(stringify)
            }
            tools::MAP_SOUND_LIST_TOOL => self.map_sound_list(request_id),
            tools::SEARCH_DOCS_TOOL => Ok(self.search_docs(args)),
            tools::DOCS_GET_TOOL => self.docs_get(args),
            tools::REQUEST_WRITE_WORKSPACE_TOOL => {
                let reason = str_arg(args, "reason")?;
                self.request_write_workspace(reason)?;
                Ok(json!({
                    "ok": true,
                    "status": "granted",
                    "note": "Write intent recorded. Stop this turn now; the backend will resume this thread immediately in its isolated writable workspace."
                }))
            }

            // ---- write tools (journaled) ----
            "dat_set" => self.dat_family_set(
                request_id,
                WriteTool::DatSet,
                DatTable::Dat,
                "dat",
                str_arg(args, "param")?,
                args,
            ),
            "xdat_set" => self.dat_family_set(
                request_id,
                WriteTool::XdatSet,
                DatTable::Xdat,
                "xdat",
                str_arg(args, "name")?,
                args,
            ),
            "tbl_set" => self.tbl_set(request_id, args),
            "req_set" => self.req_set(request_id, args),
            "btn_set" => self.btn_set(request_id, args),
            "dat_reset" => self.dat_reset(request_id, args),
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
                if *self.sound_preflight_required.lock() {
                    return Err(
                        "map sound import 이후 modified/created EPS 전체를 한 eps_check batch로 검사해야 합니다."
                            .to_string(),
                    );
                }
                let bridge = self.bridge()?;
                let result = edd_runner::build_run(&bridge)?;
                *self.sound_build_required.lock() = false;
                *self.last_build.lock() = Some(crate::harness::BuildEvidence {
                    ok: result.ok,
                    error_count: result.errors.len(),
                });
                serde_json::to_value(result)
                    .map_err(|error| format!("failed to serialize build result: {error}"))
            }
            "location_write" => {
                let bridge = self.bridge()?;
                tools::location_write(
                    &bridge,
                    &self.services.map_safe,
                    &self.services.journal,
                    request_id,
                    args,
                )
                .map_err(stringify)
            }
            "player_setup" => {
                let bridge = self.bridge()?;
                tools::player_setup(
                    &bridge,
                    &self.services.map_safe,
                    &self.services.journal,
                    request_id,
                    args,
                )
                .map_err(stringify)
            }
            tools::SWITCH_WRITE_TOOL => {
                let bridge = self.bridge()?;
                tools::switch_write(
                    &bridge,
                    &self.services.map_safe,
                    &self.services.journal,
                    request_id,
                    args,
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
        let bridge = self.bridge()?;
        let (map_path, _) = tools::connected_map_metadata(&bridge, tools::MAP_SOUND_LIST_TOOL)
            .map_err(stringify)?;
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
        let bridge = self.bridge()?;
        let (map_path, _) = tools::connected_map_metadata(&bridge, tools::MAP_SOUND_IMPORT_TOOL)
            .map_err(stringify)?;
        let expected_map_sha256 = crate::bootstrap::sha256_file(&map_path)
            .map_err(|_| "저장된 원본 맵을 읽을 수 없습니다.".to_string())?;
        self.emit_audio_progress(
            crate::ipc::ProgressStage::AudioTranscode,
            "canonical OGG Vorbis로 변환하고 있습니다.",
        );
        let cancellation = self.cancellation.lock().clone();
        let normalized = self
            .services
            .audio
            .normalize(&binding, cancellation.as_ref())?;
        self.emit_audio_progress(
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
        self.emit_audio_progress(
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
                    return Err(
                        "EUD Editor가 빌드 중이므로 맵 사운드를 추가할 수 없습니다.".to_string()
                    )
                }
                Err(crate::mapsafe::MapSafeError::MapLocked(_)) => {
                    self.emit_audio_progress(
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
            *self.sound_preflight_required.lock() = true;
            Ok(write)
        })?;
        let write = operation?;
        let sound_ref = self.next_sound_ref(request_id)?;
        self.emit_audio_progress(
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

        let bridge = self.bridge()?;
        let (map_path, _) = tools::connected_map_metadata(&bridge, tools::MAP_SOUND_EDIT_TOOL)
            .map_err(stringify)?;
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

        self.emit_audio_progress(
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
        self.emit_audio_progress(
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
        self.emit_audio_progress(
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
                    return Err(
                        "EUD Editor가 빌드 중이므로 맵 사운드를 교체할 수 없습니다.".to_string()
                    )
                }
                Err(crate::mapsafe::MapSafeError::MapLocked(_)) => {
                    self.emit_audio_progress(
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
            *self.sound_preflight_required.lock() = true;
            Ok(write)
        })?;
        let write = operation?;
        self.emit_audio_progress(
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

    fn dat_family_set(
        &self,
        request_id: &str,
        tool: WriteTool,
        table: DatTable,
        kind: &str,
        property: &str,
        args: &Value,
    ) -> Result<Value, String> {
        let obj_id = i64_arg(args, "objId")?;
        let dat = str_arg(args, "dat")?;
        let value = args.get("value").cloned().unwrap_or(Value::Null);
        let value_text = value_to_text(&value);

        // `property` is the param (dat) or field name (xdat); both commands share
        // the `<dat>|<property>|<objId>[|<value>]` shape, only the verb differs.
        let (get_cmd, set_cmd) = if kind == "dat" {
            (
                format!("GETDAT {dat}|{property}|{obj_id}"),
                format!("SETDAT {dat}|{property}|{obj_id}|{value_text}"),
            )
        } else {
            (
                format!("GETXDAT {dat}|{property}|{obj_id}"),
                format!("SETXDAT {dat}|{property}|{obj_id}|{value_text}"),
            )
        };

        let old = json_num_or_str(&reply_value(&self.send(&get_cmd)?));
        let reply = self.send(&set_cmd)?;
        self.record_dat(request_id, tool, table, dat, obj_id, property, old, value)?;
        Ok(json!({ "ok": true, "result": reply.trim() }))
    }

    fn tbl_set(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let index = i64_arg(args, "index")?;
        let value = str_arg(args, "value")?;
        let old = json_num_or_str(&reply_value(&self.send(&format!("GETTBL {index}"))?));
        let reply = self.send(&format!("SETTBL {index}\n{value}"))?;
        self.record_dat(
            request_id,
            WriteTool::TblSet,
            DatTable::Tbl,
            "",
            index,
            "text",
            old,
            Value::String(value.to_string()),
        )?;
        Ok(json!({ "ok": true, "result": reply.trim() }))
    }

    fn req_set(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let (dat, obj_id, payload) = (
            str_arg(args, "dat")?,
            i64_arg(args, "objId")?,
            str_arg(args, "payload")?,
        );
        let old = json_num_or_str(&reply_value(&self.send(&format!("GETREQ {dat}|{obj_id}"))?));
        let reply = self.send(&format!("SETREQ {dat}|{obj_id}\n{payload}"))?;
        self.record_dat(
            request_id,
            WriteTool::ReqSet,
            DatTable::Req,
            dat,
            obj_id,
            "payload",
            old,
            Value::String(payload.to_string()),
        )?;
        Ok(json!({ "ok": true, "result": reply.trim() }))
    }

    fn btn_set(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let (set_id, csv) = (i64_arg(args, "setId")?, str_arg(args, "csv")?);
        let old = json_num_or_str(&reply_value(&self.send(&format!("GETBTN {set_id}"))?));
        let reply = self.send(&format!("SETBTN {set_id}\n{csv}"))?;
        self.record_dat(
            request_id,
            WriteTool::BtnSet,
            DatTable::Btn,
            "",
            set_id,
            "csv",
            old,
            Value::String(csv.to_string()),
        )?;
        Ok(json!({ "ok": true, "result": reply.trim() }))
    }

    fn dat_reset(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let kind = str_arg(args, "kind")?;
        let obj_id = i64_arg(args, "objId")?;
        let dat = args.get("dat").and_then(Value::as_str).unwrap_or("");
        let param = args.get("param").and_then(Value::as_str).unwrap_or("");

        let (get_cmd, table, tool) = match kind {
            "dat" => (
                format!("GETDAT {dat}|{param}|{obj_id}"),
                DatTable::Dat,
                WriteTool::DatSet,
            ),
            "xdat" => (
                format!("GETXDAT {dat}|{param}|{obj_id}"),
                DatTable::Xdat,
                WriteTool::XdatSet,
            ),
            "tbl" => (format!("GETTBL {obj_id}"), DatTable::Tbl, WriteTool::TblSet),
            other => return Err(format!("invalid reset kind '{other}' (dat/xdat/tbl)")),
        };

        let old = json_num_or_str(&reply_value(&self.send(&get_cmd)?));
        let reply = self.send(&format!("RESETDAT {kind}|{dat}|{param}|{obj_id}"))?;
        // The inverse restores the captured old value (was_default:false), so a
        // later real rollback re-sets it rather than resetting again.
        let property = if kind == "tbl" { "text" } else { param };
        let dat = if kind == "tbl" { "" } else { dat };
        self.record_dat(
            request_id,
            tool,
            table,
            dat,
            obj_id,
            property,
            old,
            Value::Null,
        )?;
        Ok(json!({ "ok": true, "result": reply.trim() }))
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
        let code = args.get("code").and_then(Value::as_str).unwrap_or("");
        // The editor stores a script's name WITHOUT its type extension (a
        // natively-created CUIEps `test` has FileName `test`; the editor adds
        // `.eps` for display and at build time). The model, mirroring the
        // LIST/GET paths it reads (e.g. main.eps), passes a path WITH the
        // extension, which would persist as FileName `main.eps` and build to
        // `main.eps.eps`. Strip it so the stored name matches a native file.
        let path = normalize_create_path(requested_path, ftype);
        let mirror_path = created_project_path(requested_path, ftype);
        if self.source_baseline(&mirror_path)?.is_some()
            || self
                .bridge()?
                .get(&mirror_path, &SendOpts::default(), None)
                .is_ok()
        {
            return Err(concurrent_source_conflict(
                &mirror_path,
                "the create target already exists",
            ));
        }
        let reply = self.send(&format!("NEWFILE {path}|{ftype}\n{code}"))?;
        self.eps_preflight
            .write_applied(request_id, &mirror_path, code);
        self.record_file(
            request_id,
            WriteTool::FileCreate,
            &path,
            Snapshot::Created,
            Snapshot::Created,
        )?;
        Ok(json!({ "ok": true, "result": reply.trim() }))
    }

    fn file_write(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let (path, code) = (str_arg(args, "path")?, str_arg(args, "code")?);
        let old = self
            .bridge()?
            .get(path, &SendOpts::default(), None)
            .map_err(stringify)?;
        let merged = match self.source_baseline(path)? {
            Some(base) => crate::workspace::merge_concurrent_text(path, &base, code, &old)
                .map_err(|error| error.to_string())?,
            None if self.source_created_by_request(request_id, path) || old == code => {
                code.to_string()
            }
            None => {
                return Err(concurrent_source_conflict(
                    path,
                    "the file was absent from this session's source baseline",
                ))
            }
        };
        let reply = self
            .bridge()?
            .set(path, &merged, &SendOpts::default(), None)
            .map_err(stringify)?;
        self.eps_preflight.write_applied(request_id, path, &merged);
        self.record_file(
            request_id,
            WriteTool::FileWrite,
            path,
            Snapshot::FileContent { content: old },
            Snapshot::FileContent { content: merged },
        )?;
        Ok(json!({ "ok": true, "result": reply.trim() }))
    }

    fn file_edit(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let path = str_arg(args, "path")?;
        let edits: Vec<ExactTextEdit> = serde_json::from_value(
            args.get("edits")
                .cloned()
                .ok_or_else(|| "missing argument 'edits'".to_string())?,
        )
        .map_err(|error| format!("invalid file_edit edits: {error}"))?;
        let bridge = self.bridge()?;
        let old = bridge
            .get(path, &SendOpts::default(), None)
            .map_err(stringify)?;
        let merged = match self.source_baseline(path)? {
            Some(base) => {
                let edit_base = self
                    .latest_file_content(request_id, path)
                    .unwrap_or_else(|| base.clone());
                let ours = apply_exact_text_edits(path, &edit_base, &edits)
                    .map_err(|error| error.to_string())?;
                crate::workspace::merge_concurrent_text(path, &edit_base, &ours, &old)
                    .map_err(|error| error.to_string())?
            }
            None if self.source_created_by_request(request_id, path) => {
                apply_exact_text_edits(path, &old, &edits).map_err(|error| error.to_string())?
            }
            None => {
                return Err(concurrent_source_conflict(
                    path,
                    "the file was absent from this session's source baseline",
                ))
            }
        };
        let reply = bridge
            .set(path, &merged, &SendOpts::default(), None)
            .map_err(stringify)?;
        self.eps_preflight.write_applied(request_id, path, &merged);
        self.record_file(
            request_id,
            WriteTool::FileWrite,
            path,
            Snapshot::FileContent { content: old },
            Snapshot::FileContent { content: merged },
        )?;
        Ok(json!({
            "ok": true,
            "result": reply.trim(),
            "editsApplied": edits.len(),
        }))
    }

    fn file_delete(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let path = str_arg(args, "path")?;
        // Best-effort content snapshot for folder deletion. Text files are checked
        // against this session's coherent source baseline before shared mutation.
        let old = self
            .bridge()?
            .get(path, &SendOpts::default(), None)
            .unwrap_or_default();
        if let Some(base) = self.source_baseline(path)? {
            if old != base {
                return Err(concurrent_source_conflict(
                    path,
                    "the live file changed after this session read it",
                ));
            }
        }
        let reply = self.send(&format!("DELFILE {path}"))?;
        self.eps_preflight.delete_applied(request_id, path);
        self.record_file(
            request_id,
            WriteTool::FileDelete,
            path,
            Snapshot::DeletedFile {
                content: old,
                position: None,
            },
            Snapshot::Deleted,
        )?;
        Ok(json!({ "ok": true, "result": reply.trim() }))
    }

    fn mkdir(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let path = str_arg(args, "path")?;
        let reply = self.send(&format!("MKDIR {path}"))?;
        self.record_file(
            request_id,
            WriteTool::Mkdir,
            path,
            Snapshot::Created,
            Snapshot::Created,
        )?;
        Ok(json!({ "ok": true, "result": reply.trim() }))
    }

    fn file_rename(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let (path, newname) = (str_arg(args, "path")?, str_arg(args, "newname")?);
        if let Some(base) = self.source_baseline(path)? {
            let current = self
                .bridge()?
                .get(path, &SendOpts::default(), None)
                .map_err(stringify)?;
            if current != base {
                return Err(concurrent_source_conflict(
                    path,
                    "the live file changed after this session read it",
                ));
            }
        }
        let to = sibling_path(path, newname);
        let reply = self.send(&format!("RENAME {path}\n{newname}"))?;
        self.eps_preflight.rename_applied(request_id, path, &to);
        self.record_rename(request_id, WriteTool::FileRename, path, &to)?;
        Ok(json!({ "ok": true, "result": reply.trim() }))
    }

    fn file_move(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let path = str_arg(args, "path")?;
        if let Some(base) = self.source_baseline(path)? {
            let current = self
                .bridge()?
                .get(path, &SendOpts::default(), None)
                .map_err(stringify)?;
            if current != base {
                return Err(concurrent_source_conflict(
                    path,
                    "the live file changed after this session read it",
                ));
            }
        }
        let dest = args.get("destFolder").and_then(Value::as_str).unwrap_or("");
        let to = moved_path(path, dest);
        let reply = self.send(&format!("MOVEFILE {path}\n{dest}"))?;
        self.eps_preflight.rename_applied(request_id, path, &to);
        self.record_rename(request_id, WriteTool::FileMove, path, &to)?;
        Ok(json!({ "ok": true, "result": reply.trim() }))
    }

    fn set_main(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let path = str_arg(args, "path")?;
        let opts = SendOpts::default();
        let bridge = self.bridge()?;
        let old = bridge.get_main(&opts, None).map_err(stringify)?;
        let reply = bridge
            .send(&format!("SETMAIN {path}"), &opts, None)
            .map_err(stringify)?;
        let before = Snapshot::MainPath { path: old };
        let after = Snapshot::MainPath {
            path: Some(path.to_string()),
        };
        let seq = self.next_seq(request_id);
        self.record(JournalEntry {
            id: format!("main-{seq}"),
            seq,
            tool: WriteTool::SetMain,
            target: JournalTarget::Path {
                path: path.to_string(),
            },
            before,
            after,
            ts: epoch_secs(),
        })?;
        Ok(json!({ "ok": true, "result": reply.trim() }))
    }

    fn settings_set(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let (scope, key, value) = (
            str_arg(args, "scope")?,
            str_arg(args, "key")?,
            str_arg(args, "value")?,
        );
        let old = reply_value(&self.send(&format!("GETSET {scope}|{key}"))?);
        let reply = self.send(&format!("SETSET {scope}|{key}\n{value}"))?;
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
        Ok(json!({ "ok": true, "result": reply.trim() }))
    }

    fn plugin_add(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let index = args.get("index").and_then(Value::as_i64).unwrap_or(-1);
        let texts = args.get("texts").and_then(Value::as_str).unwrap_or("");
        let reply = self.send(&format!("PLUGADD {index}\n{texts}"))?;
        let at = parse_trailing_index(&reply, "plugadd at ").unwrap_or(index.max(0));
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
                index: at as usize,
            },
            ts: epoch_secs(),
        })?;
        Ok(json!({ "ok": true, "result": reply.trim() }))
    }

    fn plugin_edit(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let index = i64_arg(args, "index")?;
        let texts = args.get("texts").and_then(Value::as_str).unwrap_or("");
        let reply = self.send(&format!("PLUGSET {index}\n{texts}"))?;
        let index_usize = index.max(0) as usize;
        // The bridge exposes only a plugin's FIRST line (PLUGLIST), so the old
        // Texts cannot be fully snapshotted here — the before keeps the index for
        // tail-reject targeting, with empty texts (a documented rollback limit).
        let seq = self.next_seq(request_id);
        self.record(JournalEntry {
            id: format!("plug-{seq}"),
            seq,
            tool: WriteTool::PluginEdit,
            target: JournalTarget::Plugin {
                plugin_id: index.to_string(),
            },
            before: Snapshot::PluginTexts {
                texts: Vec::new(),
                index: index_usize,
            },
            after: Snapshot::PluginTexts {
                texts: vec![texts.to_string()],
                index: index_usize,
            },
            ts: epoch_secs(),
        })?;
        Ok(json!({ "ok": true, "result": reply.trim() }))
    }

    fn plugin_remove(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let index = i64_arg(args, "index")?;
        let reply = self.send(&format!("PLUGDEL {index}"))?;
        let index_usize = index.max(0) as usize;
        let seq = self.next_seq(request_id);
        self.record(JournalEntry {
            id: format!("plug-{seq}"),
            seq,
            tool: WriteTool::PluginRemove,
            target: JournalTarget::Plugin {
                plugin_id: index.to_string(),
            },
            before: Snapshot::PluginTexts {
                texts: Vec::new(),
                index: index_usize,
            },
            after: Snapshot::PluginAbsent,
            ts: epoch_secs(),
        })?;
        Ok(json!({ "ok": true, "result": reply.trim() }))
    }

    fn plugin_move(&self, request_id: &str, args: &Value) -> Result<Value, String> {
        let (from, to) = (i64_arg(args, "from")?, i64_arg(args, "to")?);
        let reply = self.send(&format!("PLUGMOVE {from}|{to}"))?;
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
                index: from.max(0) as usize,
            },
            after: Snapshot::PluginTexts {
                texts: Vec::new(),
                index: to.max(0) as usize,
            },
            ts: epoch_secs(),
        })?;
        Ok(json!({ "ok": true, "result": reply.trim() }))
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

    fn read_file(&self, args: &Value, opts: &SendOpts) -> Result<Value, String> {
        let path = str_arg(args, "path")?;
        let content = self.bridge()?.get(path, opts, None).map_err(stringify)?;
        ranged_file_result(path, &content, args)
    }

    fn source_search(&self, args: &Value, opts: &SendOpts) -> Result<Value, String> {
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

        let bridge = self.bridge()?;
        let files = bridge.list(opts, None).map_err(stringify)?;
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
            let content = bridge.get(&file.path, opts, None).map_err(stringify)?;
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

    fn send(&self, command: &str) -> Result<String, String> {
        self.bridge()?
            .send(command, &SendOpts::default(), None)
            .map_err(stringify)
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

impl crate::journal::JournalBridge for SessionToolRuntime {
    type Error = String;

    fn set_dat_value(
        &self,
        table: DatTable,
        dat: &str,
        obj_id: u32,
        property: &str,
        value: Value,
    ) -> Result<(), Self::Error> {
        let value = value_to_text(&value);
        let command = match table {
            DatTable::Dat => format!("SETDAT {dat}|{property}|{obj_id}|{value}"),
            DatTable::Xdat => format!("SETXDAT {dat}|{property}|{obj_id}|{value}"),
            DatTable::Tbl => format!("SETTBL {obj_id}\n{value}"),
            DatTable::Req => format!("SETREQ {dat}|{obj_id}\n{value}"),
            DatTable::Btn => format!("SETBTN {obj_id}\n{value}"),
        };
        self.send(&command).map(|_| ())
    }

    fn reset_dat_value(
        &self,
        table: DatTable,
        dat: &str,
        obj_id: u32,
        property: &str,
    ) -> Result<(), Self::Error> {
        let kind = match table {
            DatTable::Dat => "dat",
            DatTable::Xdat => "xdat",
            DatTable::Tbl => "tbl",
            DatTable::Req | DatTable::Btn => {
                return Err(format!("{table} values do not support default reset"));
            }
        };
        self.send(&format!("RESETDAT {kind}|{dat}|{property}|{obj_id}"))
            .map(|_| ())
    }

    fn write_file(&self, path: &str, content: &str) -> Result<(), Self::Error> {
        self.bridge()?
            .set(path, content, &SendOpts::default(), None)
            .map(|_| ())
            .map_err(stringify)
    }

    fn delete_file(&self, path: &str) -> Result<(), Self::Error> {
        self.send(&format!("DELFILE {path}")).map(|_| ())
    }

    fn write_workspace_file(
        &self,
        workspace_id: &str,
        session_id: Option<&str>,
        path: &str,
        content: &str,
    ) -> Result<(), Self::Error> {
        crate::workspace::WorkspaceManager::new(self.data_dirs())
            .restore_file(workspace_id, session_id, path, Some(content))
            .map_err(stringify)
    }

    fn delete_workspace_file(
        &self,
        workspace_id: &str,
        session_id: Option<&str>,
        path: &str,
    ) -> Result<(), Self::Error> {
        crate::workspace::WorkspaceManager::new(self.data_dirs())
            .restore_file(workspace_id, session_id, path, None)
            .map_err(stringify)
    }

    fn create_file(
        &self,
        path: &str,
        content: &str,
        _position: Option<usize>,
    ) -> Result<(), Self::Error> {
        let file_type = rollback_file_type(path);
        let create_path = normalize_create_path(path, file_type);
        self.send(&format!("NEWFILE {create_path}|{file_type}\n{content}"))
            .map(|_| ())
    }

    fn rename_path(&self, from: &str, to: &str) -> Result<(), Self::Error> {
        if from == to {
            return Ok(());
        }
        let (from_parent, from_name) = rollback_path_parts(from)?;
        let (to_parent, to_name) = rollback_path_parts(to)?;
        if from_parent == to_parent {
            return self.send(&format!("RENAME {from}\n{to_name}")).map(|_| ());
        }
        if from_name == to_name {
            return self
                .send(&format!("MOVEFILE {from}\n{to_parent}"))
                .map(|_| ());
        }
        Err(format!(
            "cannot rollback a combined move and rename from '{from}' to '{to}'"
        ))
    }

    fn set_main(&self, path: Option<&str>) -> Result<(), Self::Error> {
        let path = path.ok_or_else(|| {
            "cannot rollback MainFile because the editor bridge has no clear-main command"
                .to_string()
        })?;
        self.send(&format!("SETMAIN {path}")).map(|_| ())
    }

    fn set_setting(&self, key: &str, value: Value) -> Result<(), Self::Error> {
        let (scope, name) = key
            .split_once('|')
            .ok_or_else(|| format!("invalid journal setting key '{key}'"))?;
        self.send(&format!("SETSET {scope}|{name}\n{}", value_to_text(&value)))
            .map(|_| ())
    }

    fn plugin_add(
        &self,
        _plugin_id: &str,
        texts: Vec<String>,
        index: usize,
    ) -> Result<(), Self::Error> {
        self.send(&format!("PLUGADD {index}\n{}", texts.join("\n")))
            .map(|_| ())
    }

    fn plugin_edit(
        &self,
        _plugin_id: &str,
        texts: Vec<String>,
        index: usize,
    ) -> Result<(), Self::Error> {
        self.send(&format!("PLUGSET {index}\n{}", texts.join("\n")))
            .map(|_| ())
    }

    fn plugin_remove(&self, plugin_id: &str) -> Result<(), Self::Error> {
        let index = rollback_plugin_index(plugin_id)?;
        self.send(&format!("PLUGDEL {index}")).map(|_| ())
    }

    fn plugin_move(&self, from_index: usize, to_index: usize) -> Result<(), Self::Error> {
        self.send(&format!("PLUGMOVE {from_index}|{to_index}"))
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

fn rollback_file_type(path: &str) -> &'static str {
    let path = path.to_ascii_lowercase();
    if path.ends_with(".eps") {
        "CUIEps"
    } else if path.ends_with(".py") {
        "CUIPy"
    } else {
        "RawText"
    }
}

fn rollback_path_parts(path: &str) -> Result<(&str, &str), String> {
    let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
    if name.is_empty() {
        return Err(format!("invalid empty rollback path leaf in '{path}'"));
    }
    Ok((parent, name))
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
        let base = std::env::temp_dir().join(format!("eud-agent-runtime-test-{nanos}"));
        let dirs = DataDirs::from_bases(&base, &base);
        let analyzer = Arc::new(crate::eps_preflight::NodeEpsAnalyzer::unavailable(
            crate::eps_preflight::SkipReason::AdapterMissing,
            "test runtime has no adapter resource",
        ));
        let candidates = crate::map_candidate::CandidateStore::new(
            (dirs.clone()).clone(),
            crate::map_import::MapImportStore::new(dirs.clone()),
        );
        Self::new(
            dirs,
            analyzer,
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

    pub fn require_sound_build_for_tests(&self) {
        *self.sound_build_required.lock() = true;
        *self.sound_preflight_required.lock() = true;
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

/// Render a JSON arg as the bare bridge token: a string passes through, a number
/// stringifies. (Tool-arg validation already rails the accepted shapes.)
fn value_to_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Extract the value after the bridge `OK: ... = <value>` separator (the reply
/// shape every GET* command shares); falls back to the trimmed reply.
fn reply_value(reply: &str) -> String {
    reply
        .split_once(" = ")
        .map(|(_, value)| value.trim().to_string())
        .unwrap_or_else(|| reply.trim().to_string())
}

fn json_num_or_str(text: &str) -> Value {
    match text.parse::<i64>() {
        Ok(number) => Value::from(number),
        Err(_) => Value::String(text.to_string()),
    }
}

/// New full path for a rename: keep the source's parent folder, swap the leaf.
// Type extension the editor appends to a script's base name for display and at
// build time, so file_create must strip it from the leaf to avoid a doubled
// name (CUIEps `main.eps` -> stored FileName `main.eps` -> built `main.eps.eps`;
// a native `test` stores FileName `test`). Only CUIEps is confirmed against an
// editor-native file. CUIPy is almost certainly symmetric (`.py`) but the
// editor exposes no native-create path to verify it, so it stays untouched
// until a build check confirms. RawText keeps its extension — it is part of the
// user-chosen name (e.g. notes.txt), not a fixed type marker.
fn auto_extension(ftype: &str) -> Option<&'static str> {
    match ftype {
        "CUIEps" => Some(".eps"),
        _ => None,
    }
}

// Drop the type's extension from the path's leaf when present, keeping any
// parent folders. Idempotent: a leaf without the extension passes through. A
// leaf that is only the extension (".eps", "folder/.eps") is left intact.
fn normalize_create_path(path: &str, ftype: &str) -> String {
    let Some(ext) = auto_extension(ftype) else {
        return path.to_string();
    };
    match path.strip_suffix(ext) {
        Some(stripped) if !stripped.is_empty() && !stripped.ends_with('/') => stripped.to_string(),
        _ => path.to_string(),
    }
}

fn created_project_path(path: &str, ftype: &str) -> String {
    let Some(extension) = auto_extension(ftype) else {
        return path.to_string();
    };
    if path.ends_with(extension) {
        path.to_string()
    } else {
        format!("{path}{extension}")
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

/// Parse the index out of a bridge reply like `OK: plugadd at 3 (12B)`.
fn parse_trailing_index(reply: &str, marker: &str) -> Option<i64> {
    let rest = reply.split_once(marker)?.1;
    let token: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    token.parse().ok()
}

fn epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eps_preflight::{
        AnalyzerError, AnalyzerRequest, AnalyzerSuccess, EpsDiagnostic, EpsImport,
    };
    use crate::journal::JournalBridge;
    use base64::Engine;
    use serde_json::json;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};

    fn open_runtime(request_id: &str) -> SessionToolRuntime {
        let runtime = SessionToolRuntime::for_tests();
        runtime.begin_request(request_id, "test-project").unwrap();
        runtime.request_write_workspace("test mutation").unwrap();
        runtime
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
        runtime.begin_request("req-ask", "project").unwrap();
        let (events, mut emitted) = tokio::sync::mpsc::unbounded_channel();
        runtime.set_ask_emitter(move |event| {
            events
                .send(event)
                .map_err(|_| "ask event receiver closed".to_string())
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
        assert_eq!(runtime.pending_ask(), Some(event.clone()));

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

    #[test]
    fn failed_read_write_intent_cannot_become_a_ghost_owner() {
        let services = ToolServices::for_tests();
        let runtime = services.session("only-session");
        runtime.begin_request("request-old", "project").unwrap();
        let old = runtime.request_write_workspace("write after read").unwrap();
        assert_eq!(old.state(), crate::write_coordinator::TicketState::Granted);

        let error = runtime
            .begin_request("request-new", "project")
            .expect_err("an active ticket must never be silently discarded");
        assert!(error.contains("request-old"));

        runtime.abort_unmutated_write_intent().unwrap();
        runtime.clear_current();
        runtime.begin_request("request-new", "project").unwrap();
        let next = runtime.request_write_workspace("retry write").unwrap();
        assert_eq!(
            next.state(),
            crate::write_coordinator::TicketState::Granted,
            "the only session must not queue behind its failed stale request"
        );
    }

    #[test]
    fn every_mutating_tool_requires_the_exact_session_write_lease() {
        let runtime = SessionToolRuntime::for_tests();
        runtime.begin_request("req-read-only", "project").unwrap();

        for spec in tools::tool_registry()
            .into_iter()
            .filter(|spec| spec.mutating)
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
            state.build_fix_attempts = 1;
        }

        session_b.begin_request("request-b", "project").unwrap();

        let state_a = session_a
            .request_state_snapshot()
            .expect("session B must not clear session A");
        assert!(state_a.docs_searched);
        assert_eq!(state_a.build_fix_attempts, 1);
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
                "dat_set",
                &json!({"dat": "units", "param": "HP", "objId": 0, "value": 100}),
            )
            .expect_err("dat_set before search must hit the evidence gate");
        assert!(before.contains("evidence gate"), "got: {before}");

        // search_docs runs (zero hits on the empty test index) and lifts the gate.
        let result = runtime
            .execute("search_docs", &json!({"query": "마린 생성"}))
            .expect("search_docs should succeed even with an empty index");
        assert_eq!(result["count"], 0);

        // The SAME mutating call now passes admission and reaches execution,
        // failing only because the test runtime has no connected editor — never
        // again on the evidence gate.
        let after = runtime
            .execute(
                "dat_set",
                &json!({"dat": "units", "param": "HP", "objId": 0, "value": 100}),
            )
            .expect_err("no editor is connected in the test runtime");
        assert!(
            !after.contains("evidence gate"),
            "the gate must be lifted after search_docs, got: {after}"
        );
    }

    #[test]
    fn progressive_docs_discovery_preserves_exact_reads_and_reports_repeats() {
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

        let second = runtime
            .execute(
                tools::SEARCH_DOCS_TOOL,
                &json!({"query": "SelectionCircle", "k": 1}),
            )
            .unwrap();
        assert_eq!(second["newCount"], 0);
        assert_eq!(second["repeatedCount"], 1);
        assert_eq!(second["hits"][0]["repeated"], true);

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

    struct ReturningAnalyzer {
        calls: AtomicUsize,
    }

    impl EpsAnalyzer for ReturningAnalyzer {
        fn analyze(&self, request: &AnalyzerRequest) -> Result<AnalyzerSuccess, AnalyzerError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(request.candidates.len(), 1);
            assert_eq!(request.candidates[0].path, "main.eps");
            Ok(AnalyzerSuccess {
                checked_files: vec!["main.eps".to_string()],
                diagnostics: Vec::<EpsDiagnostic>::new(),
                imports: Vec::<EpsImport>::new(),
                truncated: false,
                omitted_diagnostics: 0,
                omitted_message_bytes: 0,
            })
        }

        fn reset_project(&self) {}
    }

    fn bridge_runtime(
        tag: &str,
        request_id: &str,
    ) -> (PathBuf, PathBuf, PathBuf, SessionToolRuntime) {
        let base =
            std::env::temp_dir().join(format!("eud-agent-runtime-{tag}-{}", uuid::Uuid::new_v4()));
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        dirs.ensure_dirs().unwrap();
        let editor = base.join("editor");
        let agent_dir = editor.join("Data").join("agent");
        let inbox = agent_dir.join("inbox");
        let outbox = agent_dir.join("outbox");
        fs::create_dir_all(&inbox).unwrap();
        fs::create_dir_all(&outbox).unwrap();
        dirs.save_config(&crate::config::Config {
            editor_path: editor.to_string_lossy().to_string(),
            ..Default::default()
        })
        .unwrap();
        let candidates = crate::map_candidate::CandidateStore::new(
            (dirs.clone()).clone(),
            crate::map_import::MapImportStore::new(dirs.clone()),
        );
        let services = ToolServices::new(
            dirs,
            Arc::new(ReturningAnalyzer {
                calls: AtomicUsize::new(0),
            }),
            candidates,
            crate::write_coordinator::ProjectWriteCoordinator::silent(),
        );
        let runtime = services.session(format!("{tag}-session"));
        runtime.begin_request(request_id, &"a".repeat(64)).unwrap();
        (base, inbox, outbox, runtime)
    }

    fn spawn_bridge_responder(
        inbox: PathBuf,
        outbox: PathBuf,
        replies: Vec<(&'static str, &'static str)>,
    ) -> thread::JoinHandle<Vec<String>> {
        thread::spawn(move || {
            let mut seen = Vec::with_capacity(replies.len());
            for (expected, reply) in replies {
                let deadline = Instant::now() + Duration::from_secs(5);
                loop {
                    let command = fs::read_dir(&inbox)
                        .unwrap()
                        .filter_map(Result::ok)
                        .find_map(|entry| {
                            let file_name = entry.file_name().to_string_lossy().to_string();
                            if !file_name.starts_with("srv-") || !file_name.ends_with(".cmd") {
                                return None;
                            }
                            Some((entry.path(), file_name))
                        });
                    if let Some((path, file_name)) = command {
                        let command = fs::read_to_string(&path).unwrap();
                        assert_eq!(command, expected);
                        seen.push(command);
                        fs::remove_file(path).unwrap();
                        let stem = file_name.trim_end_matches(".cmd");
                        fs::write(outbox.join(format!("{stem}.result")), reply.as_bytes()).unwrap();
                        break;
                    }
                    assert!(Instant::now() < deadline, "bridge command did not arrive");
                    thread::sleep(Duration::from_millis(5));
                }
            }
            seen
        })
    }

    fn spawn_owned_bridge_responder(
        inbox: PathBuf,
        outbox: PathBuf,
        expected: String,
        reply: String,
    ) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let command = fs::read_dir(&inbox)
                    .unwrap()
                    .filter_map(Result::ok)
                    .find_map(|entry| {
                        let file_name = entry.file_name().to_string_lossy().to_string();
                        (file_name.starts_with("srv-") && file_name.ends_with(".cmd"))
                            .then_some((entry.path(), file_name))
                    });
                if let Some((path, file_name)) = command {
                    assert_eq!(fs::read_to_string(&path).unwrap(), expected);
                    fs::remove_file(path).unwrap();
                    let stem = file_name.trim_end_matches(".cmd");
                    fs::write(outbox.join(format!("{stem}.result")), reply.as_bytes()).unwrap();
                    return;
                }
                assert!(Instant::now() < deadline, "bridge command did not arrive");
                thread::sleep(Duration::from_millis(5));
            }
        })
    }

    #[test]
    fn production_journal_bridge_translates_every_editor_inverse_command() {
        let (base, inbox, outbox, runtime) =
            bridge_runtime("journal-rollback-commands", "req-journal-rollback-commands");
        let expected = vec![
            ("SETDAT units|HP|0|40", "OK"),
            ("SETXDAT weapons|Splash|1|2", "OK"),
            ("SETTBL 2\nName", "OK"),
            ("SETREQ units|3\npayload", "OK"),
            ("SETBTN 4\ncsv", "OK"),
            ("RESETDAT xdat|units|ButtonSet|5", "OK"),
            ("SET scripts/main.eps\nold", "OK"),
            ("DELFILE scripts/new.eps", "OK"),
            ("NEWFILE scripts/deleted|CUIEps\nrestored", "OK"),
            ("RENAME scripts/new.eps\nold.eps", "OK"),
            ("MOVEFILE lib/moved.eps\nscripts", "OK"),
            ("SETMAIN scripts/old-main.eps", "OK"),
            ("SETSET project|output\nold", "OK"),
            ("PLUGADD 2\nline1\nline2", "OK"),
            ("PLUGSET 3\nold plugin", "OK"),
            ("PLUGDEL 4", "OK"),
            ("PLUGMOVE 5|1", "OK"),
        ];
        let responder = spawn_bridge_responder(inbox, outbox, expected.clone());

        runtime
            .set_dat_value(DatTable::Dat, "units", 0, "HP", json!(40))
            .unwrap();
        runtime
            .set_dat_value(DatTable::Xdat, "weapons", 1, "Splash", json!(2))
            .unwrap();
        runtime
            .set_dat_value(DatTable::Tbl, "", 2, "text", json!("Name"))
            .unwrap();
        runtime
            .set_dat_value(DatTable::Req, "units", 3, "payload", json!("payload"))
            .unwrap();
        runtime
            .set_dat_value(DatTable::Btn, "", 4, "csv", json!("csv"))
            .unwrap();
        runtime
            .reset_dat_value(DatTable::Xdat, "units", 5, "ButtonSet")
            .unwrap();
        runtime.write_file("scripts/main.eps", "old").unwrap();
        runtime.delete_file("scripts/new.eps").unwrap();
        runtime
            .create_file("scripts/deleted.eps", "restored", Some(4))
            .unwrap();
        runtime
            .rename_path("scripts/new.eps", "scripts/old.eps")
            .unwrap();
        runtime
            .rename_path("lib/moved.eps", "scripts/moved.eps")
            .unwrap();
        JournalBridge::set_main(&runtime, Some("scripts/old-main.eps")).unwrap();
        runtime.set_setting("project|output", json!("old")).unwrap();
        JournalBridge::plugin_add(&runtime, "2", vec!["line1\nline2".to_string()], 2).unwrap();
        JournalBridge::plugin_edit(&runtime, "3", vec!["old plugin".to_string()], 3).unwrap();
        JournalBridge::plugin_remove(&runtime, "4").unwrap();
        JournalBridge::plugin_move(&runtime, 5, 1).unwrap();

        assert_eq!(
            responder.join().unwrap(),
            expected
                .into_iter()
                .map(|(command, _)| command.to_string())
                .collect::<Vec<_>>()
        );
        assert!(
            JournalBridge::set_main(&runtime, None)
                .unwrap_err()
                .contains("clear-main"),
            "an unset previous MainFile must refuse rather than corrupt state"
        );
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn project_status_adds_configured_main_without_list_or_write_lease() {
        let (base, inbox, outbox, runtime) =
            bridge_runtime("project-status-main", "req-project-status-main");
        let raw_status = "compiling=False\r\nproject='E:\\maps\\demo.e3s'\r\nversion=0.19.6.0";
        let responder = spawn_bridge_responder(
            inbox,
            outbox,
            vec![
                ("STATUS", raw_status),
                ("GETMAIN", "survivor_mvp"),
                ("LIST", "main\tClassicTrigger\r\nsurvivor_mvp\tCUIEps"),
            ],
        );

        let status = runtime.execute("project_status", &json!({})).unwrap();
        assert_eq!(status["status"], raw_status);
        assert_eq!(status["mainFile"], "survivor_mvp");

        let files = runtime.execute("list_files", &json!({})).unwrap();
        assert_eq!(files["count"], 2);
        assert_eq!(files["files"][0]["path"], "main");
        assert_eq!(files["files"][0]["ftype"], "ClassicTrigger");
        assert_eq!(files["files"][1]["path"], "survivor_mvp");
        assert_eq!(files["files"][1]["ftype"], "CUIEps");
        assert_eq!(
            responder.join().unwrap(),
            vec!["STATUS", "GETMAIN", "LIST"],
            "project_status must read MainFile only through GETMAIN; LIST remains separate"
        );
        assert_eq!(runtime.request_state_snapshot().unwrap().action_count, 2);
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn ranged_read_and_source_search_page_exact_excerpts() {
        let (base, inbox, outbox, runtime) = bridge_runtime("source-search", "req-source-search");
        let responder = spawn_bridge_responder(
            inbox,
            outbox,
            vec![
                ("GET boss", "alpha\nbeta\ngamma\ndelta"),
                ("LIST", "boss\tCUIEps\r\nhero\tCUIEps"),
                ("GET boss", "a\nneedle one\nb\nc\nd\nneedle two\ne"),
                ("GET hero", "x\ny"),
                ("LIST", "boss\tCUIEps\r\nhero\tCUIEps"),
                ("GET boss", "a\nneedle one\nb\nc\nd\nneedle two\ne"),
                ("GET hero", "x\ny"),
            ],
        );

        let ranged = runtime
            .execute(
                "read_file",
                &json!({"path": "boss", "startLine": 2, "endLine": 3}),
            )
            .unwrap();
        assert_eq!(ranged["content"], "beta\ngamma");
        assert_eq!(ranged["startLine"], 2);
        assert_eq!(ranged["endLine"], 3);
        assert_eq!(ranged["totalLines"], 4);
        assert_eq!(ranged["hasMore"], true);

        let first = runtime
            .execute(
                tools::SOURCE_SEARCH_TOOL,
                &json!({"query": "NEEDLE", "contextLines": 1, "offset": 0, "limit": 1}),
            )
            .unwrap();
        assert_eq!(first["total"], 2);
        assert_eq!(first["count"], 1);
        assert_eq!(first["hasMore"], true);
        assert_eq!(first["nextOffset"], 1);
        assert_eq!(first["matches"][0]["path"], "boss");
        assert_eq!(first["matches"][0]["startLine"], 1);
        assert_eq!(first["matches"][0]["endLine"], 3);
        assert_eq!(first["matches"][0]["text"], "a\nneedle one\nb");

        let second = runtime
            .execute(
                tools::SOURCE_SEARCH_TOOL,
                &json!({"query": "needle", "contextLines": 1, "offset": 1, "limit": 1}),
            )
            .unwrap();
        assert_eq!(second["total"], 2);
        assert_eq!(second["count"], 1);
        assert_eq!(second["hasMore"], false);
        assert!(second["nextOffset"].is_null());
        assert_eq!(second["matches"][0]["startLine"], 5);
        assert_eq!(second["matches"][0]["endLine"], 7);
        assert_eq!(second["matches"][0]["text"], "d\nneedle two\ne");

        responder.join().unwrap();
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn project_status_maps_empty_getmain_reply_to_json_null() {
        let (base, inbox, outbox, runtime) =
            bridge_runtime("project-status-empty-main", "req-project-status-empty-main");
        let responder = spawn_bridge_responder(
            inbox,
            outbox,
            vec![("STATUS", "compiling=False\nproject=''"), ("GETMAIN", "")],
        );

        let result = runtime.execute("project_status", &json!({})).unwrap();
        responder.join().unwrap();

        assert_eq!(result["status"], "compiling=False\nproject=''");
        assert!(result["mainFile"].is_null());
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn set_main_journals_configured_and_unset_old_values_through_getmain() {
        for (tag, request_id, old_reply, expected_old) in [
            (
                "set-main-configured",
                "req-set-main-configured",
                "systems/old_root",
                Some("systems/old_root"),
            ),
            ("set-main-unset", "req-set-main-unset", "", None),
        ] {
            let (base, inbox, outbox, runtime) = bridge_runtime(tag, request_id);
            runtime
                .request_write_workspace("set the requested composition root")
                .unwrap();
            runtime
                .execute("search_docs", &json!({"query": "MainFile 설정"}))
                .unwrap();
            let responder = spawn_bridge_responder(
                inbox,
                outbox,
                vec![
                    ("GETMAIN", old_reply),
                    ("SETMAIN survivor_mvp", "OK: main 'survivor_mvp'"),
                ],
            );

            runtime
                .execute("set_main", &json!({"path": "survivor_mvp"}))
                .unwrap();
            assert_eq!(
                responder.join().unwrap(),
                vec!["GETMAIN", "SETMAIN survivor_mvp"]
            );

            let entries = runtime
                .services
                .journal
                .selected_entries(request_id, &crate::journal::DecisionIds::All)
                .unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(
                entries[0].before,
                Snapshot::MainPath {
                    path: expected_old.map(str::to_string),
                }
            );
            fs::remove_dir_all(base).ok();
        }
    }

    #[test]
    fn set_main_surfaces_getmain_errors_before_mutating_or_journaling() {
        let request_id = "req-set-main-read-error";
        let (base, inbox, outbox, runtime) = bridge_runtime("set-main-read-error", request_id);
        runtime
            .request_write_workspace("set the requested composition root")
            .unwrap();
        runtime
            .execute("search_docs", &json!({"query": "MainFile 설정"}))
            .unwrap();
        let responder = spawn_bridge_responder(
            inbox,
            outbox,
            vec![("GETMAIN", "ERROR: unexpected GETMAIN failure")],
        );

        let error = runtime
            .execute("set_main", &json!({"path": "survivor_mvp"}))
            .unwrap_err();
        assert!(error.contains("unexpected GETMAIN failure"), "got: {error}");
        assert_eq!(responder.join().unwrap(), vec!["GETMAIN"]);
        assert_eq!(runtime.services.journal.entry_count(request_id), 0);
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn batched_tbl_get_preserves_order_and_reports_per_item_errors() {
        let (base, inbox, outbox, runtime) =
            bridge_runtime("batched-tbl-get", "req-batched-tbl-get");
        let responder = spawn_bridge_responder(
            inbox,
            outbox,
            vec![
                ("GETTBL 1", "OK: tbl|1 = First"),
                ("GETTBL 2", "ERROR: missing TBL string"),
            ],
        );

        let result = runtime
            .execute("tbl_get", &json!({"items": [{"index": 1}, {"index": 2}]}))
            .unwrap();
        responder.join().unwrap();

        assert_eq!(result["count"], 2);
        assert_eq!(result["results"][0]["index"], 1);
        assert_eq!(result["results"][0]["ok"], true);
        assert_eq!(result["results"][0]["value"], "First");
        assert_eq!(result["results"][1]["index"], 2);
        assert_eq!(result["results"][1]["ok"], false);
        assert!(result["results"][1]["error"]
            .as_str()
            .unwrap()
            .contains("missing TBL string"));
        assert_eq!(runtime.request_state_snapshot().unwrap().action_count, 1);
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn file_edit_merges_non_overlapping_live_changes_and_journals_full_content() {
        let (base, inbox, outbox, runtime) = bridge_runtime("file-edit", "req-file-edit");
        let workspace = base.join("workspace");
        fs::create_dir_all(workspace.join("source")).unwrap();
        fs::write(
            workspace.join("source/survivor_mvp"),
            "alpha: old\nbeta: old\n",
        )
        .unwrap();
        runtime
            .bind_workspace_root("req-file-edit", workspace)
            .unwrap();
        runtime
            .request_write_workspace("edit survivor_mvp")
            .unwrap();
        runtime
            .execute("search_docs", &json!({"query": "테스트"}))
            .unwrap();
        let responder = spawn_bridge_responder(
            inbox,
            outbox,
            vec![
                ("GET survivor_mvp", "alpha: old\nbeta: external\n"),
                (
                    "SET survivor_mvp\nalpha: agent\nbeta: external\n",
                    "OK: saved",
                ),
                ("GET survivor_mvp", "alpha: agent\nbeta: external\n"),
                (
                    "SET survivor_mvp\nalpha: final\nbeta: external\n",
                    "OK: saved",
                ),
            ],
        );

        let result = runtime
            .execute(
                "file_edit",
                &json!({
                    "path": "survivor_mvp",
                    "edits": [{"old_text": "alpha: old", "new_text": "alpha: agent"}],
                }),
            )
            .unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["editsApplied"], 1);
        let second = runtime
            .execute(
                "file_edit",
                &json!({
                    "path": "survivor_mvp",
                    "edits": [{"old_text": "alpha: agent", "new_text": "alpha: final"}],
                }),
            )
            .unwrap();
        responder.join().unwrap();

        assert_eq!(second["ok"], true);
        assert_eq!(second["editsApplied"], 1);
        let entries = runtime
            .journal()
            .selected_entries("req-file-edit", &crate::journal::DecisionIds::All)
            .unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[1].after,
            Snapshot::FileContent {
                content: "alpha: final\nbeta: external\n".into(),
            }
        );
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn file_write_accepts_extensionless_cuieps_source_baseline() {
        let (base, inbox, outbox, runtime) =
            bridge_runtime("file-write-extensionless", "req-file-write-extensionless");
        let workspace = base.join("workspace");
        fs::create_dir_all(workspace.join("source")).unwrap();
        fs::write(
            workspace.join("source/survivor_mvp"),
            "function onPluginStart() {}\n",
        )
        .unwrap();
        runtime
            .bind_workspace_root("req-file-write-extensionless", workspace)
            .unwrap();
        runtime
            .request_write_workspace("write survivor_mvp")
            .unwrap();
        runtime
            .execute("search_docs", &json!({"query": "테스트"}))
            .unwrap();
        let responder = spawn_bridge_responder(
            inbox,
            outbox,
            vec![
                ("GET survivor_mvp", "function onPluginStart() {}\n"),
                (
                    "SET survivor_mvp\nfunction onPluginStart() {\n    init();\n}\n",
                    "OK: saved",
                ),
            ],
        );

        let result = runtime
            .execute(
                "file_write",
                &json!({
                    "path": "survivor_mvp",
                    "code": "function onPluginStart() {\n    init();\n}\n",
                }),
            )
            .unwrap();
        responder.join().unwrap();

        assert_eq!(result["ok"], true);
        let entries = runtime
            .journal()
            .selected_entries(
                "req-file-write-extensionless",
                &crate::journal::DecisionIds::All,
            )
            .unwrap();
        assert_eq!(
            entries[0].after,
            Snapshot::FileContent {
                content: "function onPluginStart() {\n    init();\n}\n".into(),
            }
        );
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn eps_check_returns_fake_analyzer_result_without_journal_or_budget_changes() {
        let base = std::env::temp_dir().join(format!(
            "eud-agent-runtime-eps-check-{}",
            uuid::Uuid::new_v4()
        ));
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        dirs.ensure_dirs().unwrap();
        let editor = base.join("editor");
        let agent_dir = editor.join("Data").join("agent");
        let inbox = agent_dir.join("inbox");
        let outbox = agent_dir.join("outbox");
        fs::create_dir_all(&inbox).unwrap();
        fs::create_dir_all(&outbox).unwrap();
        let config = crate::config::Config {
            editor_path: editor.to_string_lossy().to_string(),
            ..Default::default()
        };
        dirs.save_config(&config).unwrap();

        let responder_outbox = outbox.clone();
        let responder = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                for entry in fs::read_dir(&inbox).unwrap().filter_map(Result::ok) {
                    let file_name = entry.file_name().to_string_lossy().to_string();
                    if !file_name.starts_with("srv-") || !file_name.ends_with(".cmd") {
                        continue;
                    }
                    let command = fs::read_to_string(entry.path()).unwrap();
                    let Some(token) = command.strip_prefix("EPSNAPSHOT ") else {
                        continue;
                    };
                    let token = token.trim();
                    let snapshot_dir = responder_outbox.join(format!("epsnapshot-{token}"));
                    fs::create_dir_all(&snapshot_dir).unwrap();
                    let code = "function onPluginStart() {}";
                    fs::write(snapshot_dir.join("000001.eps"), code.as_bytes()).unwrap();
                    let manifest = [
                        "EUD-EPSNAPSHOT\t1".to_string(),
                        format!("token\t{token}"),
                        format!(
                            "project\t{}",
                            base64::engine::general_purpose::STANDARD.encode("Project".as_bytes())
                        ),
                        format!(
                            "identity\t{}",
                            base64::engine::general_purpose::STANDARD
                                .encode("Project\nmap.scx".as_bytes())
                        ),
                        format!(
                            "file\t1\tCUIEps\t{}\t{}\tok",
                            base64::engine::general_purpose::STANDARD.encode("main.eps".as_bytes()),
                            code.len()
                        ),
                    ]
                    .join("\n");
                    fs::write(snapshot_dir.join("manifest.tsv"), manifest.as_bytes()).unwrap();
                    let stem = file_name.trim_end_matches(".cmd").to_string();
                    fs::remove_file(entry.path()).unwrap();
                    fs::write(
                        responder_outbox.join(format!("{stem}.result")),
                        b"OK: epsnapshot 1 files",
                    )
                    .unwrap();
                    return;
                }
                assert!(
                    Instant::now() < deadline,
                    "eps snapshot command did not arrive"
                );
                thread::sleep(Duration::from_millis(5));
            }
        });

        let analyzer = Arc::new(ReturningAnalyzer {
            calls: AtomicUsize::new(0),
        });
        let candidates = crate::map_candidate::CandidateStore::new(
            (dirs.clone()).clone(),
            crate::map_import::MapImportStore::new(dirs.clone()),
        );
        let services = ToolServices::new(
            dirs,
            analyzer.clone(),
            candidates,
            crate::write_coordinator::ProjectWriteCoordinator::silent(),
        );
        let runtime = services.session("eps-session");
        runtime.begin_request("req-eps", "project").unwrap();
        let result = runtime
            .execute(
                tools::EPS_CHECK_TOOL,
                &json!({
                    "files": [{"path": "main.eps", "code": "function onPluginStart() {}"}]
                }),
            )
            .unwrap();
        responder.join().unwrap();

        assert_eq!(result["status"], "diagnosed");
        assert_eq!(result["checkedFiles"], json!(["main.eps"]));
        assert_eq!(analyzer.calls.load(Ordering::SeqCst), 1);
        assert_eq!(runtime.journal().entry_count("req-eps"), 0);
        let state = runtime.request_state_snapshot().unwrap();
        assert_eq!(state.action_count, 0);
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn unknown_tool_is_rejected_with_a_clear_message() {
        let runtime = open_runtime("req-unknown");
        let error = runtime
            .execute("teleport", &json!({}))
            .expect_err("an unregistered tool must be rejected");
        assert!(error.contains("unknown tool"), "got: {error}");
    }

    #[test]
    fn reply_value_extracts_the_bridge_ok_payload() {
        assert_eq!(reply_value("OK: units|HP|0 = 80"), "80");
        assert_eq!(
            reply_value("OK: project|OpenMapName = C:/maps/x.scx"),
            "C:/maps/x.scx"
        );
        assert_eq!(reply_value("no separator here"), "no separator here");
    }

    #[test]
    fn moved_and_sibling_paths_keep_the_leaf() {
        assert_eq!(sibling_path("folder/a.eps", "b.eps"), "folder/b.eps");
        assert_eq!(sibling_path("a.eps", "b.eps"), "b.eps");
        assert_eq!(moved_path("folder/a.eps", "dest"), "dest/a.eps");
        assert_eq!(moved_path("folder/a.eps", ""), "a.eps");
    }

    #[test]
    fn normalize_create_path_strips_a_doubled_eps_extension() {
        // CUIEps leaf the model suffixed: strip so FileName matches a native
        // file (`test`), which the editor displays/builds as `test.eps`.
        assert_eq!(normalize_create_path("main.eps", "CUIEps"), "main");
        assert_eq!(
            normalize_create_path("folder/main.eps", "CUIEps"),
            "folder/main"
        );
        // Idempotent: no extension to strip passes through unchanged.
        assert_eq!(normalize_create_path("main", "CUIEps"), "main");
        // A leaf that is only the extension is left intact.
        assert_eq!(normalize_create_path(".eps", "CUIEps"), ".eps");
        assert_eq!(
            normalize_create_path("folder/.eps", "CUIEps"),
            "folder/.eps"
        );
        // RawText keeps its extension (part of the user-chosen name); CUIPy is
        // left untouched until verified.
        assert_eq!(normalize_create_path("notes.txt", "RawText"), "notes.txt");
        assert_eq!(normalize_create_path("raw.eps", "RawText"), "raw.eps");
        assert_eq!(normalize_create_path("script.py", "CUIPy"), "script.py");
    }

    #[test]
    fn parse_trailing_index_reads_the_plugadd_slot() {
        assert_eq!(
            parse_trailing_index("OK: plugadd at 3 (12B)", "plugadd at "),
            Some(3)
        );
        assert_eq!(parse_trailing_index("OK: nothing", "plugadd at "), None);
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
        let analyzer = Arc::new(crate::eps_preflight::NodeEpsAnalyzer::unavailable(
            crate::eps_preflight::SkipReason::AdapterMissing,
            "map palette test has no analyzer",
        ));
        let services = ToolServices::new(
            dirs,
            analyzer,
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
        let analyzer = Arc::new(crate::eps_preflight::NodeEpsAnalyzer::unavailable(
            crate::eps_preflight::SkipReason::AdapterMissing,
            "map image test has no analyzer",
        ));
        let services = ToolServices::new(
            dirs.clone(),
            analyzer,
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
    #[test]
    fn map_sound_requires_post_import_preflight_before_complete_build() {
        let runtime = SessionToolRuntime::for_tests();
        runtime.begin_request("sound-build", "project").unwrap();
        *runtime.sound_build_required.lock() = true;
        *runtime.sound_preflight_required.lock() = true;
        let error = runtime
            .dispatch("sound-build", tools::BUILD_RUN_TOOL, &json!({}))
            .unwrap_err();
        assert!(error.contains("eps_check batch"));
        assert!(runtime.sound_build_required());

        *runtime.sound_preflight_required.lock() = false;
        let bridge_error = runtime
            .dispatch("sound-build", tools::BUILD_RUN_TOOL, &json!({}))
            .unwrap_err();
        assert!(!bridge_error.contains("eps_check batch"));
        assert!(runtime.sound_build_required());
    }
    #[test]
    #[ignore = "requires checksum-pinned managed FFmpeg/FFprobe in LocalAppData"]
    fn map_sound_import_is_one_lease_journal_and_exact_reject_transaction() {
        let (base, inbox, outbox, runtime) = bridge_runtime("sound-import", "sound-import-request");
        let dirs = runtime.data_dirs();
        let installed = DataDirs::from_bases(
            std::path::Path::new(&std::env::var("APPDATA").unwrap()),
            std::path::Path::new(&std::env::var("LOCALAPPDATA").unwrap()),
        );
        for name in ["ffmpeg.exe", "ffprobe.exe"] {
            std::fs::hard_link(installed.bin_dir().join(name), dirs.bin_dir().join(name)).unwrap();
        }
        let map = base.join("sound-source.scx");
        std::fs::copy(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("crates")
                .join("isom")
                .join("tests")
                .join("fixtures")
                .join("map_agent_rich.scx"),
            &map,
        )
        .unwrap();
        let before = std::fs::read(&map).unwrap();
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
        let attachments = crate::attachment::AttachmentStore::new(dirs.attachments_dir());
        let descriptor = attachments
            .stage("battle-theme.ogg", "audio/ogg", &tone)
            .unwrap();
        let context = attachments
            .bind_and_resolve(std::slice::from_ref(&descriptor.id), runtime.session_id())
            .unwrap();
        let refs = runtime
            .bind_audio_attachments("sound-import-request", context.audio_files)
            .unwrap();
        assert_eq!(refs[0].audio_ref, "audio-1");
        runtime
            .execute(tools::SEARCH_DOCS_TOOL, &json!({"query": "PlayWAV"}))
            .unwrap();
        let ticket = runtime
            .request_write_workspace("import request-local map sound")
            .unwrap();
        assert_eq!(
            ticket.state(),
            crate::write_coordinator::TicketState::Granted
        );
        let responder = spawn_owned_bridge_responder(
            inbox,
            outbox,
            "GETSET project|OpenMapName".to_string(),
            format!("OK: project|OpenMapName = {}", map.display()),
        );
        let result = runtime
            .execute(
                tools::MAP_SOUND_IMPORT_TOOL,
                &json!({"audioRef": "audio-1"}),
            )
            .unwrap();
        responder.join().unwrap();
        assert_eq!(result["soundRef"], "sound-1");
        assert_eq!(result["outputCodec"], "vorbis");
        assert_eq!(result["reused"], false);
        assert!(result["mpqPath"]
            .as_str()
            .is_some_and(|path| managed_sound_hash(path).is_some()));
        let visible = result.to_string();
        assert!(!visible.contains(&descriptor.id));
        assert!(!visible.contains(&descriptor.name));
        assert!(!visible.contains("audio_temp"));
        assert_ne!(std::fs::read(&map).unwrap(), before);
        let changeset = runtime.journal().changeset("sound-import-request").unwrap();
        assert_eq!(changeset.items.len(), 1);
        assert_eq!(
            changeset.items[0].kind,
            crate::journal::ChangesetItemKind::MapSound
        );
        assert!(runtime.sound_build_required());
        let list_responder = spawn_owned_bridge_responder(
            base.join("editor").join("Data").join("agent").join("inbox"),
            base.join("editor")
                .join("Data")
                .join("agent")
                .join("outbox"),
            "GETSET project|OpenMapName".to_string(),
            format!("OK: project|OpenMapName = {}", map.display()),
        );
        let listed = runtime
            .execute(tools::MAP_SOUND_LIST_TOOL, &json!({}))
            .unwrap();
        list_responder.join().unwrap();
        assert!(listed["sounds"].as_array().unwrap().iter().any(|sound| {
            sound["mpqPath"] == result["mpqPath"]
                && sound["assetPresent"] == true
                && sound["managed"] == true
                && sound["sourceAvailable"] == true
                && sound["volumePercent"] == 100
        }));

        let old_mpq_path = result["mpqPath"].as_str().unwrap().to_string();
        let edit_responder = spawn_owned_bridge_responder(
            base.join("editor").join("Data").join("agent").join("inbox"),
            base.join("editor")
                .join("Data")
                .join("agent")
                .join("outbox"),
            "GETSET project|OpenMapName".to_string(),
            format!("OK: project|OpenMapName = {}", map.display()),
        );
        let edited = runtime
            .execute(
                tools::MAP_SOUND_EDIT_TOOL,
                &json!({
                    "mpqPath": old_mpq_path,
                    "volumePercent": 50,
                    "fadeInMs": 100,
                    "fadeOutMs": 100,
                }),
            )
            .unwrap();
        edit_responder.join().unwrap();
        let new_mpq_path = edited["mpqPath"].as_str().unwrap();
        assert_ne!(new_mpq_path, old_mpq_path);
        assert_eq!(edited["oldMpqPath"], old_mpq_path);
        assert_eq!(edited["volumePercent"], 50);
        assert_eq!(edited["fadeInMs"], 100);
        assert_eq!(edited["fadeOutMs"], 100);
        assert_eq!(edited["requiresCodeMigration"], true);
        let registered = crate::chk::parse_sounds(&isom::chk_extract(&map).unwrap());
        assert_eq!(
            registered
                .iter()
                .filter(|sound| sound.mpq_path == old_mpq_path)
                .count(),
            0
        );
        assert_eq!(
            registered
                .iter()
                .filter(|sound| sound.mpq_path == new_mpq_path)
                .count(),
            1
        );
        let changeset = runtime.journal().changeset("sound-import-request").unwrap();
        assert_eq!(changeset.items.len(), 2);
        assert!(changeset.items.iter().any(|item| {
            item.properties.iter().any(|property| {
                property.property == "volumePercent"
                    && property.old == json!(100)
                    && property.new == json!(50)
            })
        }));

        attachments.delete_session(runtime.session_id()).unwrap();
        assert_eq!(
            crate::chk::parse_sounds(&isom::chk_extract(&map).unwrap())
                .iter()
                .filter(|sound| sound.mpq_path == new_mpq_path)
                .count(),
            1
        );
        runtime
            .project_transaction(|| {
                runtime.journal().decide(
                    "sound-import-request",
                    crate::journal::ChangesetDecision::reject(crate::journal::DecisionIds::All),
                    &runtime,
                )
            })
            .unwrap()
            .unwrap();
        assert_eq!(std::fs::read(&map).unwrap(), before);
        runtime.release_write_registration().unwrap();
        std::fs::remove_dir_all(base).ok();
    }
}
