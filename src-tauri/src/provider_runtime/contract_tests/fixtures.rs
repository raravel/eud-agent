use std::{
    collections::VecDeque,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    sync::{mpsc, Arc},
    thread,
};

use parking_lot::Mutex;
use serde_json::Value;

use crate::{
    config::DataDirs,
    eps_preflight::{NodeEpsAnalyzer, SkipReason},
    map_candidate::CandidateStore,
    map_import::MapImportStore,
    native_project::{NativeProject, ProjectManifest, PROJECT_SCHEMA_VERSION},
    native_runtime::NativeProjectManager,
    provider::{ModelCapabilities, ProviderConversationState, ProviderId, ReasoningSelection},
    provider_runtime::{
        AdapterEventKind, AgentTurnInput, BindingSnapshot, ForegroundRequest, JobBase, RunId,
        RunIdentity, RunPolicy, RuntimeEventSink, StructuredJobKind, StructuredJobRequest,
        WorkspaceAccess,
    },
    tool_exec::{SessionToolRuntime, ToolServices},
    write_coordinator::ProjectWriteCoordinator,
};

pub(super) struct HttpFixture {
    pub base_url: String,
    pub requests: mpsc::Receiver<String>,
    server: Option<thread::JoinHandle<()>>,
}

impl HttpFixture {
    pub fn scripted(responses: impl IntoIterator<Item = String>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind HTTP fixture");
        let address = listener.local_addr().expect("HTTP fixture address");
        let (send, requests) = mpsc::channel();
        let mut responses = responses.into_iter().collect::<VecDeque<_>>();
        let server = thread::spawn(move || {
            while let Some(response) = responses.pop_front() {
                let (mut stream, _) = listener.accept().expect("accept fixture request");
                let request = read_request(&mut stream);
                send.send(request).expect("capture fixture request");
                stream
                    .write_all(response.as_bytes())
                    .expect("write fixture response");
            }
        });
        Self {
            base_url: format!("http://{address}/v1"),
            requests,
            server: Some(server),
        }
    }

    pub fn join(mut self) {
        self.server
            .take()
            .expect("fixture server handle")
            .join()
            .expect("fixture server thread");
    }
}

pub(super) fn read_request(stream: &mut impl Read) -> String {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let count = stream.read(&mut buffer).expect("read fixture request");
        assert_ne!(count, 0, "fixture request closed before headers");
        bytes.extend_from_slice(&buffer[..count]);
        let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let headers = String::from_utf8_lossy(&bytes[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().expect("content length"))
            })
            .unwrap_or_default();
        if bytes.len() >= header_end + 4 + content_length {
            return String::from_utf8(bytes).expect("UTF-8 fixture request");
        }
    }
}

pub(super) fn sse(events: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{events}",
        events.len()
    )
}

#[derive(Default)]
pub(super) struct EventCollector {
    events: Mutex<Vec<EventSummary>>,
    cancel_on_start: Mutex<Option<tokio::sync::watch::Sender<u64>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum EventSummary {
    Started(String),
    Text(String),
    Reasoning(String),
    Usage(Option<u64>),
    Finished(String, bool),
}

impl EventCollector {
    pub fn snapshot(&self) -> Vec<EventSummary> {
        self.events.lock().clone()
    }

    pub fn arm_cancellation(&self, sender: tokio::sync::watch::Sender<u64>) {
        *self.cancel_on_start.lock() = Some(sender);
    }
}

impl RuntimeEventSink for EventCollector {
    fn emit(
        &self,
        event: &AdapterEventKind,
    ) -> Result<(), crate::provider_runtime::ProviderRuntimeError> {
        let summary = match event {
            AdapterEventKind::ResponseStarted { response_id } => {
                if let Some(sender) = self.cancel_on_start.lock().take() {
                    let next = sender.borrow().saturating_add(1);
                    let _ = sender.send(next);
                }
                Some(EventSummary::Started(response_id.clone()))
            }
            AdapterEventKind::Block(crate::provider_runtime::NormalizedBlock::Text {
                text,
                ..
            }) => Some(EventSummary::Text(text.clone())),
            AdapterEventKind::Block(crate::provider_runtime::NormalizedBlock::Reasoning {
                text,
                ..
            }) => Some(EventSummary::Reasoning(text.clone())),
            AdapterEventKind::Usage(usage) => Some(EventSummary::Usage(usage.total_tokens)),
            AdapterEventKind::ResponseFinished {
                response_id,
                complete,
                ..
            } => Some(EventSummary::Finished(response_id.clone(), *complete)),
            AdapterEventKind::TransportClosed => None,
            AdapterEventKind::Block(crate::provider_runtime::NormalizedBlock::ToolCall {
                ..
            })
            | AdapterEventKind::Block(crate::provider_runtime::NormalizedBlock::ToolResult {
                ..
            })
            | AdapterEventKind::NativeToolObservation { .. } => None,
        };
        if let Some(summary) = summary {
            self.events.lock().push(summary);
        }
        Ok(())
    }
}

pub(super) struct RuntimeFixture {
    pub root: PathBuf,
    pub dirs: DataDirs,
    pub tools: SessionToolRuntime,
    pub cancellation: tokio::sync::watch::Sender<u64>,
    pub events: Arc<EventCollector>,
    pub session_id: String,
    pub request_id: String,
    services: ToolServices,
    owns_root: bool,
}

impl RuntimeFixture {
    pub fn new(tag: &str) -> Self {
        Self::new_kind(tag, crate::session::SessionKind::Eps)
    }

    pub fn new_kind(tag: &str, kind: crate::session::SessionKind) -> Self {
        let root = std::env::temp_dir().join(format!(
            "eud-agent-provider-runtime-{tag}-{}",
            uuid::Uuid::new_v4()
        ));
        let dirs = DataDirs::from_bases(&root.join("roaming"), &root.join("local"));
        dirs.ensure_dirs().expect("create runtime fixture dirs");
        let project_root = root.join("project");
        std::fs::create_dir_all(project_root.join("maps")).expect("create fixture map dir");
        std::fs::write(project_root.join("maps/source.scx"), b"fixture-map")
            .expect("write fixture map");
        let project = NativeProject::create(
            &project_root,
            ProjectManifest {
                schema_version: PROJECT_SCHEMA_VERSION,
                name: "Runtime Fixture".to_string(),
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
        .expect("create native fixture project");
        project
            .write_source("src/main.eps", "function onPluginStart() {}")
            .expect("write fixture source");
        NativeProjectManager::new(dirs.clone())
            .activate_project(&project)
            .expect("activate fixture project");

        let analyzer = Arc::new(NodeEpsAnalyzer::unavailable(
            SkipReason::AdapterMissing,
            "provider runtime contract fixture",
        ));
        let candidates = CandidateStore::new(dirs.clone(), MapImportStore::new(dirs.clone()));
        let services = ToolServices::new(
            dirs.clone(),
            analyzer,
            candidates,
            ProjectWriteCoordinator::silent(),
        );
        let session_id = "runtime-session".to_string();
        let request_id = "runtime-request".to_string();
        let tools = match kind {
            crate::session::SessionKind::Eps => services.session(session_id.clone()),
            crate::session::SessionKind::Map => services.map_session(session_id.clone()),
        };
        tools
            .begin_request(&request_id, &project.root().to_string_lossy())
            .expect("begin fixture tool request");
        let (cancellation, receiver) = tokio::sync::watch::channel(0);
        tools.set_cancellation(receiver);
        Self {
            root,
            dirs,
            tools,
            cancellation,
            events: Arc::new(EventCollector::default()),
            session_id,
            request_id,
            services,
            owns_root: true,
        }
    }

    pub fn sibling_session(&self, session_id: &str, kind: crate::session::SessionKind) -> Self {
        let tools = match kind {
            crate::session::SessionKind::Eps => self.services.session(session_id),
            crate::session::SessionKind::Map => self.services.map_session(session_id),
        };
        let request_id = format!("{session_id}-request");
        tools
            .begin_request(&request_id, &self.root.join("project").to_string_lossy())
            .expect("begin sibling fixture request");
        let (cancellation, receiver) = tokio::sync::watch::channel(0);
        tools.set_cancellation(receiver);
        Self {
            root: self.root.clone(),
            dirs: self.dirs.clone(),
            tools,
            cancellation,
            events: Arc::new(EventCollector::default()),
            session_id: session_id.to_string(),
            request_id,
            services: self.services.clone(),
            owns_root: false,
        }
    }

    pub fn binding(&self, base_url: &str) -> BindingSnapshot {
        BindingSnapshot {
            provider: ProviderId::Ollama,
            model: "fixture-model".to_string(),
            reasoning: Some(ReasoningSelection {
                level: "high".to_string(),
            }),
            base_url: Some(base_url.to_string()),
            capabilities: Some(ModelCapabilities {
                tool_calls: true,
                strict_structured_output: true,
                ..ModelCapabilities::default()
            }),
            conversation: ProviderConversationState::Ollama {
                transcript_revision: 0,
            },
        }
    }

    pub fn install_live_map_from_env(&self) {
        let source = std::env::var_os("EUD_AGENT_BUILD_MAP")
            .map(PathBuf::from)
            .expect("set EUD_AGENT_BUILD_MAP to an isolated-test source SCX");
        assert!(source.is_file(), "live source map is unavailable");
        std::fs::copy(source, self.root.join("project/maps/source.scx"))
            .expect("copy live source map into disposable project");
    }

    pub fn identity(&self, run: u64) -> RunIdentity {
        RunIdentity {
            session_id: self.session_id.clone(),
            run_id: RunId::new(run),
            request_id: self.request_id.clone(),
            session_kind: self.tools.kind(),
            cancellation_generation: 0,
        }
    }

    pub fn foreground(&self, binding: BindingSnapshot, run: u64) -> ForegroundRequest {
        ForegroundRequest {
            identity: self.identity(run),
            binding,
            turn: AgentTurnInput::text("inspect the project").with_access(WorkspaceAccess::Read),
            checkpoint: JobBase {
                revision: 7,
                instruction_epoch: 3,
                branch: Some("leaf-a".to_string()),
            },
            policy: policy(None),
        }
    }

    pub fn structured(
        &self,
        binding: BindingSnapshot,
        run: u64,
        schema: Value,
    ) -> StructuredJobRequest {
        StructuredJobRequest {
            identity: self.identity(run),
            binding,
            kind: StructuredJobKind::TaskStateCompiler,
            prompt: "compile state".to_string(),
            workspace_root: self.root.join("structured-input"),
            output_schema: schema,
            base: JobBase {
                revision: 41,
                instruction_epoch: 8,
                branch: Some("compiler-leaf".to_string()),
            },
            policy: policy(Some(std::time::Duration::from_secs(5))),
        }
    }
}

impl Drop for RuntimeFixture {
    fn drop(&mut self) {
        if self.owns_root {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}

pub(super) fn policy(active_deadline: Option<std::time::Duration>) -> RunPolicy {
    RunPolicy {
        active_deadline,
        shutdown_grace: std::time::Duration::from_secs(1),
        max_output_bytes: 64 * 1024,
        max_output_tokens: Some(8_192),
        max_tool_rounds: 8,
        allow_resume: true,
    }
}

pub(super) fn request_body(request: &str) -> Value {
    serde_json::from_str(
        request
            .split_once("\r\n\r\n")
            .expect("fixture request body")
            .1,
    )
    .expect("JSON fixture request")
}

pub(super) fn transcript_root(dirs: &DataDirs, session_id: &str) -> PathBuf {
    dirs.provider_sessions_dir().join(session_id)
}

pub(super) fn assert_path_exists(path: &Path) {
    assert!(
        path.exists(),
        "expected durable artifact at {}",
        path.display()
    );
}
