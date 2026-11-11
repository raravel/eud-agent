//! The browser window and its tab webviews (Windows).
//!
//! The window is created on the first tool call that needs it, without taking
//! focus. The chrome webview takes the top strip; every tab webview covers the
//! rest. Inactive tabs stay at the same rect BEHIND the active one: a hidden
//! or zero-sized WebView2 reports `visibilityState=hidden` and
//! `Page.captureScreenshot` then never answers (P0), while an occluded one
//! keeps rendering.
//!
//! Every tab pauses its document and script (`fetch`/XHR) requests
//! (`Fetch`, in the page target and in every out-of-process iframe target it
//! auto-attaches) and decides each one with [`policy::fetch_decision`]: the
//! app's own origins never load, and a form POST (or a data-changing script
//! request) an agent call caused to a domain its session has not approved
//! waits for the user. Every other request continues at once, so the user's
//! own browsing is never held. External protocol launches are cancelled, and
//! a page's popups become tabs only for a person's gesture or an agent call.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use base64::Engine as _;
use parking_lot::Mutex;
use serde_json::{json, Value};
use tauri::webview::{DownloadEvent, NewWindowResponse, PageLoadEvent, WebviewBuilder};
use tauri::window::WindowBuilder;
use tauri::{
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, PhysicalPosition, PhysicalSize,
    Rect, Webview, WebviewUrl, Window, WindowEvent,
};

use super::downloads::{self, DownloadState, DownloadStore};
use super::policy::{self, ElementFacts, FetchDecision};
use super::registry::{TabRecord, TabRegistry};
use super::snapshot::{self, SNAPSHOT_BUDGET};
use super::{
    cdp, BrowserCall, BrowserConfirm, BrowserConfirmation, BrowserRequest, BrowserState,
    ConfirmAnswer, DownloadTarget, NavTarget, BROWSER_STATE_EVENT, BROWSER_WINDOW_LABEL,
    CONFIRM_WAIT, MAX_SCREENSHOT_BYTES, NAVIGATION_WAIT,
};

/// The chrome strip's height, in logical pixels.
const CHROME_HEIGHT: f64 = 76.0;
/// wry's own WebView2 arguments, restated: a webview with its own data
/// directory gets exactly these, so every tab shares one environment.
const BROWSER_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection";
/// Every tool call ends well within the 240 s a tool call may take.
const TOOL_DEADLINE: Duration = Duration::from_secs(150);
/// A click, typing or key press may wait for the user's confirmation
/// ([`CONFIRM_WAIT`]) and still end within the 240 s.
const INTERACTION_DEADLINE: Duration = Duration::from_secs(230);
/// What a confirmation leaves of the deadline for the action itself.
const AFTER_CONFIRM: Duration = Duration::from_secs(20);
/// After a click or key press, how long a navigation it starts may take to
/// load, and how long to watch for one to start at all.
const ACTION_NAVIGATION_WAIT: Duration = Duration::from_secs(10);
const ACTION_SETTLE: Duration = Duration::from_millis(1_000);
/// Every document request a tab makes is paused and decided here; script
/// requests (`fetch`/XHR/`sendBeacon`) too, but only while an agent
/// interaction (or its grace) runs on the tab ([`fetch_patterns`]): the only
/// script requests the policy ever holds are those, so the user's and a
/// reading agent's page traffic never makes a round trip through the app.
const DOCUMENT_PATTERNS: &str =
    r#"{"patterns":[{"urlPattern":"*","resourceType":"Document","requestStage":"Request"}]}"#;
const SCRIPT_PATTERNS: &str = r#"{"patterns":[
  {"urlPattern":"*","resourceType":"Document","requestStage":"Request"},
  {"urlPattern":"*","resourceType":"XHR","requestStage":"Request"},
  {"urlPattern":"*","resourceType":"Fetch","requestStage":"Request"},
  {"urlPattern":"*","resourceType":"Ping","requestStage":"Request"}
]}"#;

/// The `Fetch.enable` parameters: documents only, or with script requests.
fn fetch_patterns(scripts: bool) -> Value {
    serde_json::from_str(if scripts {
        SCRIPT_PATTERNS
    } else {
        DOCUMENT_PATTERNS
    })
    .expect("patterns are JSON")
}

/// The `Network.enable` parameters: events only, no payload buffering.
fn network_enable() -> Value {
    json!({"maxTotalBufferSize": 0, "maxResourceBufferSize": 0, "maxPostDataSize": 0})
}

/// How often the chrome is told the browser state at most; a page that
/// changes its title in a loop cannot flood the UI thread.
const EMIT_INTERVAL: Duration = Duration::from_millis(150);
/// How long the session titles the chrome shows are reused before the
/// session list is read again.
const TITLES_TTL: Duration = Duration::from_secs(2);
/// Bounds of the per-tab bookkeeping of agent document loads.
const MAX_TRACKED_DOCUMENTS: usize = 512;
const MAX_LOCAL_PAGES: usize = 64;
/// How long a name lookup before an agent's page load may take; a slow or
/// failing lookup lets the load go on (its response address is still
/// checked).
const LOOKUP_WAIT: Duration = Duration::from_secs(2);
/// Longest screenshot side handed to the model.
const MAX_SCREENSHOT_SIDE: f64 = 7_900.0;
const SCREENSHOT_TIMEOUT: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(250);

#[derive(Clone)]
struct TabRuntime {
    webview: Webview,
    /// Bumped on every main-frame navigation (document or same-document).
    navigations: tokio::sync::watch::Receiver<u64>,
    guard: Arc<TabGuard>,
}

/// What the tab's request policy needs to know while requests are paused.
#[derive(Default)]
struct TabGuard {
    /// The agent calls running on this tab: a request they cause asks the
    /// acting call's user.
    agent: Mutex<AgentCalls>,
    main_frame: Mutex<String>,
    submissions: tokio::sync::watch::Sender<Submissions>,
    /// Downloads the page announced (`Page.downloadWillBegin`): guid → URL.
    download_urls: Mutex<HashMap<String, String>>,
    /// Popups the page asked for (`NewWindowRequested`), by URL: whether a
    /// person's gesture (or an agent's dispatched input) opened each.
    popups: Mutex<HashMap<String, bool>>,
    /// The host of the tab's current load when it was refused to the agent
    /// (this computer or the local network): reads of the tab refuse until
    /// the next main-frame load starts.
    internal: Mutex<Option<String>>,
    /// Document requests an agent call drove, by (DevTools session, network
    /// id), until their response arrives.
    agent_documents: Mutex<HashSet<(Option<String>, String)>>,
    /// Document URLs whose response came from a local address, newest
    /// last: the agent's back/forward never returns to one of them.
    local_pages: Mutex<VecDeque<String>>,
    /// Script requests are paused ([`SCRIPT_PATTERNS`]) in the page and every
    /// guarded child session.
    scripts_armed: AtomicBool,
    /// Serializes switching the patterns and guarding a new child session.
    arming: tokio::sync::Mutex<()>,
    /// The child sessions (iframes, workers) whose requests are paused.
    sessions: Mutex<HashSet<String>>,
    /// The isolated world element facts are read in: (navigation count it
    /// was made for, execution context id).
    facts_world: Mutex<Option<(u64, i64)>>,
    /// A navigation the user started from the chrome (address bar, back,
    /// forward, reload): its main-frame document request, and that
    /// request's redirect hops, are the user's within an interaction's
    /// grace. While any agent call runs on the tab no request is the
    /// user's. Cleared when the main frame commits, and when starting the
    /// navigation fails.
    user_navigation: Mutex<Option<UserNavigation>>,
    /// The file URLs `browser_download` is fetching through the page now,
    /// with the address their response came from once it arrived
    /// ([`BrowserHost::follow_fetch_responses`]).
    fetches: Mutex<HashMap<String, Option<String>>>,
    /// How many page fetches are watched; script responses are read only
    /// while one is.
    fetch_watching: std::sync::atomic::AtomicUsize,
}

/// How long a user navigation whose loader is not known yet waits to be
/// matched to its document request (a backstop: the main frame's commit and
/// a failed start clear it sooner).
const USER_NAVIGATION_WAIT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
enum UserNavigation {
    /// Started, its loader not known yet (an address before `Page.navigate`
    /// answered, a reload, back/forward): the first main-frame GET document
    /// request (until then) is it.
    Pending(Instant),
    /// Its document request, by network id: the loader `Page.navigate`
    /// reported, or the request matched while pending (redirect hops keep
    /// the id).
    Request(String),
}

impl UserNavigation {
    /// Whether the main-frame document request `network_id` (`method`) is
    /// this navigation's, at `now`; a pending navigation then follows that
    /// request's id.
    fn claims(
        navigation: &mut Option<Self>,
        method: &str,
        network_id: Option<&str>,
        now: Instant,
    ) -> bool {
        match navigation.clone() {
            Some(Self::Pending(until)) if now < until && method.eq_ignore_ascii_case("GET") => {
                *navigation = network_id.map(|id| Self::Request(id.to_string()));
                true
            }
            Some(Self::Request(id)) => network_id == Some(id.as_str()),
            _ => false,
        }
    }

    /// Whether the main-frame document request `network_id` (`method`) is
    /// the user's navigation, while `driver` drives the tab at `now`: only
    /// while the user does, an agent interaction's grace included, so a
    /// navigation a page script starts after an agent click is never taken
    /// for the user's reload. A request it does not claim leaves a pending
    /// navigation waiting.
    fn claims_while(
        navigation: &mut Option<Self>,
        driver: policy::Driver,
        method: &str,
        network_id: Option<&str>,
        now: Instant,
    ) -> bool {
        driver == policy::Driver::User && Self::claims(navigation, method, network_id, now)
    }

    /// The main frame committed the load `loader` (`None`: a same-document
    /// navigation): the user's navigation is over, whichever load it was.
    fn committed(navigation: &mut Option<Self>, loader: Option<&str>) {
        match navigation.as_ref() {
            Some(Self::Request(id)) if loader.is_some_and(|loader| loader != id) => {}
            _ => *navigation = None,
        }
    }
}

impl TabGuard {
    /// Whether an agent call runs on the tab now (or an interaction's grace
    /// has not ended).
    fn agent_driven(&self) -> bool {
        self.agent.lock().acting(Instant::now()).0 != policy::Driver::User
    }

    /// The refusal of a read of this tab while its load was refused to the
    /// agent.
    fn require_public_load(&self) -> Result<(), String> {
        match self.internal.lock().as_deref() {
            Some(host) => Err(format!(
                "{} (이 탭은 그 페이지를 불러오지 않았거나 빈 페이지로 되돌렸습니다.)",
                super::internal_refusal(host)
            )),
            None => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Submissions {
    /// Main-frame document requests seen.
    documents: u64,
}

#[derive(Clone)]
struct AgentContext {
    session_id: String,
    confirm: Option<BrowserConfirm>,
    deadline: Instant,
}

/// How long a tab stays agent-driven after an agent interaction returned,
/// so a submit the page delays (a debounced button, a `setTimeout` POST)
/// still asks.
const AGENT_GRACE: Duration = Duration::from_secs(3);
/// How long a confirmation raised inside [`AGENT_GRACE`] (after its call
/// returned) waits for the user.
const GRACE_CONFIRM_WAIT: Duration = Duration::from_secs(60);

/// The agent calls on one tab: every call running now, and the last
/// interaction for [`AGENT_GRACE`] after it returned.
///
/// Residual risk: requests are attributed by time, not by cause. The user's
/// own submission while an agent call runs on the same tab (or within the
/// grace) is held as the agent's, and a page that waits longer than the
/// grace before submitting is treated as the user's.
#[derive(Default)]
struct AgentCalls {
    /// `(scope id, interacts, context)`, oldest first.
    running: Vec<(u64, bool, AgentContext)>,
    /// When the grace of the latest finished interaction ends, and its
    /// context.
    grace: Option<(Instant, AgentContext)>,
}

impl AgentCalls {
    fn enter(&mut self, id: u64, interacts: bool, context: AgentContext) {
        self.running.push((id, interacts, context));
    }

    /// Whether an interaction is running now (not only in its grace).
    fn interaction_running(&self) -> bool {
        self.running.iter().any(|(_, interacts, _)| *interacts)
    }

    fn leave(&mut self, id: u64, now: Instant) {
        if let Some(index) = self.running.iter().position(|(scope, ..)| *scope == id) {
            let (_, interacts, context) = self.running.remove(index);
            if interacts {
                self.grace = Some((now + AGENT_GRACE, context));
            }
        }
    }

    /// Who drives the tab at `now`, and the context of the call a
    /// confirmation asks: the latest running interaction, else the
    /// interaction still in its grace (a reading call beside it never ends
    /// the grace early), else the latest running call.
    fn acting(&self, now: Instant) -> (policy::Driver, Option<AgentContext>) {
        if let Some((_, _, context)) = self
            .running
            .iter()
            .rev()
            .find(|(_, interacts, _)| *interacts)
        {
            return (policy::Driver::Interacting, Some(context.clone()));
        }
        if let Some((until, context)) = &self.grace {
            if now < *until {
                let mut context = context.clone();
                context.deadline = context
                    .deadline
                    .max(now + AFTER_CONFIRM + GRACE_CONFIRM_WAIT);
                return (policy::Driver::Interacting, Some(context));
            }
        }
        match self.running.last() {
            Some((_, _, context)) => (policy::Driver::Reading, Some(context.clone())),
            None => (policy::Driver::User, None),
        }
    }

    /// When the latest interaction's grace ends, if it has not yet.
    fn grace_until(&self, now: Instant) -> Option<Instant> {
        self.grace
            .as_ref()
            .map(|(until, _)| *until)
            .filter(|until| now < *until)
    }
}

static AGENT_SCOPES: AtomicU64 = AtomicU64::new(0);

/// Marks one agent call running on a tab, for its request policy.
struct AgentScope {
    guard: Arc<TabGuard>,
    id: u64,
}

impl AgentScope {
    fn enter(tab: &TabRuntime, context: AgentContext, interacts: bool) -> Self {
        let id = AGENT_SCOPES.fetch_add(1, Ordering::Relaxed) + 1;
        tab.guard.agent.lock().enter(id, interacts, context);
        Self {
            guard: Arc::clone(&tab.guard),
            id,
        }
    }
}

impl Drop for AgentScope {
    fn drop(&mut self) {
        self.guard.agent.lock().leave(self.id, Instant::now());
    }
}

/// One open browser confirmation of a session, counted in
/// [`Inner::confirmations`] and shown on its tab ("확인 대기") until dropped,
/// however the confirmation ends: answered, expired, or its call dropped
/// mid-wait.
struct ConfirmationSlot {
    host: BrowserHost,
    session_id: String,
    tab_id: String,
}

impl Drop for ConfirmationSlot {
    fn drop(&mut self) {
        self.host.inner.confirmations.lock().close(&self.session_id);
        self.host
            .inner
            .registry
            .lock()
            .mark_confirming(&self.tab_id, false);
        self.host.emit_state();
    }
}

/// An interaction's hold on its tab: the agent call is running, and the
/// tab's script requests are paused. Dropped however the call ends (also
/// when its future is dropped mid-call), it ends the call first and then
/// schedules the release of the script pause, so no interrupted call leaves
/// the tab's XHR/fetch paused.
struct Interaction {
    host: BrowserHost,
    tab: TabRuntime,
    agent: Option<AgentScope>,
}

impl Interaction {
    fn enter(host: &BrowserHost, tab: &TabRuntime, context: AgentContext) -> Self {
        Self {
            host: host.clone(),
            tab: tab.clone(),
            agent: Some(AgentScope::enter(tab, context, true)),
        }
    }
}

impl Drop for Interaction {
    fn drop(&mut self) {
        drop(self.agent.take());
        self.host.schedule_disarm(&self.tab);
    }
}

/// What a click or key press is compared against afterwards.
#[derive(Clone, Copy)]
struct BeforeAction {
    navigations: u64,
    submissions: Submissions,
    /// The latest download number before the action.
    downloads: u64,
}

struct Inner {
    app: AppHandle,
    profile_dir: PathBuf,
    sessions: crate::session::SessionStore,
    registry: Mutex<TabRegistry>,
    tabs: Mutex<HashMap<String, TabRuntime>>,
    /// Which page popups become tabs.
    popups: Mutex<policy::PopupGate>,
    /// Serializes creating the window.
    window: tokio::sync::Mutex<()>,
    downloads: DownloadStore,
    /// The chrome covers the tab area while its downloads list is open.
    chrome_expanded: AtomicBool,
    /// Browser confirmations open now, by session.
    confirmations: Mutex<policy::OpenConfirmations>,
    /// A state emit is scheduled ([`EMIT_INTERVAL`]).
    emit_pending: AtomicBool,
    last_emit: Mutex<Option<Instant>>,
    /// Session titles and when they were read ([`TITLES_TTL`]).
    titles: Mutex<Option<(Instant, BTreeMap<String, String>)>>,
    /// A refresh of [`Inner::titles`] runs.
    titles_refreshing: AtomicBool,
}

#[derive(Clone)]
pub struct BrowserHost {
    inner: Arc<Inner>,
}

/// Marks a tab busy for the chrome while an agent call runs on it.
struct Busy {
    host: BrowserHost,
    tab_id: String,
}

impl Drop for Busy {
    fn drop(&mut self) {
        self.host
            .inner
            .registry
            .lock()
            .mark_busy(&self.tab_id, false);
        self.host.emit_state();
    }
}

impl BrowserHost {
    pub fn new(
        app: AppHandle,
        profile_dir: PathBuf,
        sessions: crate::session::SessionStore,
        downloads: DownloadStore,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                app,
                profile_dir,
                sessions,
                registry: Mutex::new(TabRegistry::default()),
                tabs: Mutex::new(HashMap::new()),
                popups: Mutex::new(policy::PopupGate::default()),
                window: tokio::sync::Mutex::new(()),
                downloads,
                chrome_expanded: AtomicBool::new(false),
                confirmations: Mutex::new(policy::OpenConfirmations::default()),
                emit_pending: AtomicBool::new(false),
                last_emit: Mutex::new(None),
                titles: Mutex::new(None),
                titles_refreshing: AtomicBool::new(false),
            }),
        }
    }

    fn weak(&self) -> Weak<Inner> {
        Arc::downgrade(&self.inner)
    }

    fn from_weak(weak: &Weak<Inner>) -> Option<Self> {
        weak.upgrade().map(|inner| Self { inner })
    }

    pub async fn execute(&self, call: BrowserCall) -> Result<Value, String> {
        let request = super::parse_request(&call.tool, &call.args)?;
        let limit = if request.interacts() {
            INTERACTION_DEADLINE
        } else {
            TOOL_DEADLINE
        };
        let deadline = Instant::now() + limit;
        match tokio::time::timeout(limit, self.run(&call, request, deadline)).await {
            Ok(result) => result,
            Err(_) => Err(format!(
                "{} 이 {}초 안에 끝나지 않았습니다. browser_snapshot 으로 탭 상태를 확인한 뒤 다시 시도하세요.",
                call.tool,
                limit.as_secs()
            )),
        }
    }

    async fn run(
        &self,
        call: &BrowserCall,
        request: BrowserRequest,
        deadline: Instant,
    ) -> Result<Value, String> {
        let context = AgentContext {
            session_id: call.session_id.clone(),
            confirm: call.confirm.clone(),
            deadline,
        };
        if let BrowserRequest::Open { url } = &request {
            return self
                .open(&call.session_id, &call.project_id, url, context)
                .await;
        }
        if request == BrowserRequest::Tabs {
            return Ok(self.tabs_of(&call.session_id));
        }
        let tab_id = request
            .tab_id()
            .expect("every other request names a tab")
            .to_string();
        let record = self
            .inner
            .registry
            .lock()
            .owned(&call.session_id, &tab_id)?
            .clone();
        let tab = self.runtime(&tab_id)?;
        let _busy = self.busy(&tab_id);
        let interacts = request.interacts();
        let closes = matches!(request, BrowserRequest::Close { .. });
        // A navigation starts a new load; every other call reads the
        // current one.
        if !closes && !matches!(request, BrowserRequest::Navigate { .. }) {
            tab.guard.require_public_load()?;
        }
        // Ends the call (and, for an interaction, releases the script pause)
        // however the call ends, also when this future is dropped.
        let (agent, interaction) = if interacts {
            let interaction = Interaction::enter(self, &tab, context.clone());
            self.arm_scripts(&tab).await?;
            (None, Some(interaction))
        } else {
            (Some(AgentScope::enter(&tab, context.clone(), false)), None)
        };
        let result = self
            .dispatch(request, &record, &tab, &tab_id, &context)
            .await;
        drop(agent);
        drop(interaction);
        // What the call read or showed may have come from a load refused
        // to the agent meanwhile; that refusal is also the clearer reason
        // when the call itself failed on it.
        if !closes {
            tab.guard.require_public_load()?;
        }
        result
    }

    async fn dispatch(
        &self,
        request: BrowserRequest,
        record: &TabRecord,
        tab: &TabRuntime,
        tab_id: &str,
        context: &AgentContext,
    ) -> Result<Value, String> {
        match request {
            BrowserRequest::Navigate { target, .. } => {
                let loaded = self.navigate(tab, &target).await?;
                self.page_result(record, tab, loaded).await
            }
            BrowserRequest::Snapshot { offset, .. } => self.snapshot(record, tab, offset).await,
            BrowserRequest::Screenshot { full_page, .. } => {
                self.screenshot(record, tab, full_page).await
            }
            BrowserRequest::Close { .. } => {
                self.close_tab(tab_id, false);
                Ok(json!({"ok": true, "closed": tab_id}))
            }
            BrowserRequest::Wait { text, ms, .. } => self.wait(record, tab, text, ms).await,
            BrowserRequest::Click { element, .. } => {
                self.click(record, tab, &element, context).await
            }
            BrowserRequest::Type {
                element,
                text,
                submit,
                clear,
                ..
            } => {
                self.type_text(record, tab, &element, &text, submit, clear, context)
                    .await
            }
            BrowserRequest::Press { key, .. } => self.press(record, tab, key, context).await,
            BrowserRequest::Download {
                target, timeout_ms, ..
            } => {
                self.download(record, tab, target, timeout_ms, context)
                    .await
            }
            BrowserRequest::Open { .. } | BrowserRequest::Tabs => unreachable!("handled above"),
        }
    }

    fn runtime(&self, tab_id: &str) -> Result<TabRuntime, String> {
        self.inner
            .tabs
            .lock()
            .get(tab_id)
            .cloned()
            .ok_or_else(|| super::registry::TAB_NOT_FOUND.to_string())
    }

    fn busy(&self, tab_id: &str) -> Busy {
        self.inner.registry.lock().mark_busy(tab_id, true);
        self.emit_state();
        Busy {
            host: self.clone(),
            tab_id: tab_id.to_string(),
        }
    }

    // ---------------------------------------------------------------- window

    async fn ensure_window(&self) -> Result<Window, String> {
        let _creating = self.inner.window.lock().await;
        if let Some(window) = self.inner.app.get_window(BROWSER_WINDOW_LABEL) {
            return Ok(window);
        }
        let app = self.inner.app.clone();
        let window = tokio::task::spawn_blocking(move || -> Result<Window, String> {
            let window = WindowBuilder::new(&app, BROWSER_WINDOW_LABEL)
                .title("브라우저")
                .inner_size(1280.0, 860.0)
                .min_inner_size(640.0, 420.0)
                .focused(false)
                .build()
                .map_err(|error| {
                    format!("브라우저 창을 만들 수 없습니다({error}). 다시 시도하세요.")
                })?;
            window
                .add_child(
                    WebviewBuilder::new(
                        BROWSER_WINDOW_LABEL,
                        WebviewUrl::App("browser.html".into()),
                    )
                    .focused(false),
                    LogicalPosition::new(0.0, 0.0),
                    LogicalSize::new(1280.0, CHROME_HEIGHT),
                )
                .map_err(|error| {
                    let _ = window.destroy();
                    format!("브라우저 창을 만들 수 없습니다({error}). 다시 시도하세요.")
                })?;
            Ok(window)
        })
        .await
        .map_err(|error| format!("브라우저 창을 만들 수 없습니다({error})"))??;
        let weak = self.weak();
        window.on_window_event(move |event| {
            let Some(host) = Self::from_weak(&weak) else {
                return;
            };
            match event {
                WindowEvent::Resized(size) => host.layout(*size),
                WindowEvent::ScaleFactorChanged { new_inner_size, .. } => {
                    host.layout(*new_inner_size)
                }
                WindowEvent::Destroyed => host.window_destroyed(),
                _ => {}
            }
        });
        if let Ok(size) = window.inner_size() {
            self.layout(size);
        }
        Ok(window)
    }

    fn scale(&self) -> f64 {
        self.inner
            .app
            .get_window(BROWSER_WINDOW_LABEL)
            .and_then(|window| window.scale_factor().ok())
            .unwrap_or(1.0)
    }

    /// The tab area of a window whose client area is `size`.
    fn tab_rect(&self, size: PhysicalSize<u32>) -> Rect {
        let chrome = (CHROME_HEIGHT * self.scale()).round() as u32;
        Rect {
            position: PhysicalPosition::new(0, chrome.min(size.height) as i32).into(),
            size: PhysicalSize::new(size.width, size.height.saturating_sub(chrome)).into(),
        }
    }

    fn layout(&self, size: PhysicalSize<u32>) {
        let chrome = if self.inner.chrome_expanded.load(Ordering::SeqCst) {
            size.height
        } else {
            (CHROME_HEIGHT * self.scale()).round() as u32
        };
        if let Some(webview) = self.inner.app.get_webview(BROWSER_WINDOW_LABEL) {
            let _ = webview.set_bounds(Rect {
                position: PhysicalPosition::new(0, 0).into(),
                size: PhysicalSize::new(size.width, chrome.min(size.height)).into(),
            });
        }
        let rect = self.tab_rect(size);
        for tab in self.inner.tabs.lock().values() {
            let _ = tab.webview.set_bounds(rect);
        }
    }

    /// The user closed the browser window: every tab went with it, and so
    /// did a downloads list the chrome had open (the next window starts with
    /// its tab area uncovered).
    fn window_destroyed(&self) {
        self.inner.chrome_expanded.store(false, Ordering::SeqCst);
        let tabs = std::mem::take(&mut *self.inner.tabs.lock());
        {
            let mut registry = self.inner.registry.lock();
            let ids = registry
                .all()
                .iter()
                .map(|tab| tab.tab_id.clone())
                .collect::<Vec<_>>();
            for id in ids {
                registry.remove(&id, true);
            }
        }
        // Destroyed is delivered on the UI thread, where the receivers live.
        for tab in tabs.values() {
            cdp::forget(tab.webview.label());
        }
    }

    // ---------------------------------------------------------------- tabs

    async fn open(
        &self,
        session_id: &str,
        project_id: &str,
        url: &tauri::Url,
        context: AgentContext,
    ) -> Result<Value, String> {
        let (record, tab) = self.create_tab(session_id, project_id).await?;
        let _busy = self.busy(&record.tab_id);
        let _agent = AgentScope::enter(&tab, context, false);
        let loaded = self.navigate(&tab, &NavTarget::Url(url.clone())).await;
        tab.guard.require_public_load()?;
        self.page_result(&record, &tab, loaded?).await
    }

    async fn create_tab(
        &self,
        session_id: &str,
        project_id: &str,
    ) -> Result<(TabRecord, TabRuntime), String> {
        if self.inner.registry.lock().of_session(session_id).len() >= policy::MAX_TABS_PER_SESSION {
            return Err(format!(
                "이 대화의 브라우저 탭이 이미 {}개입니다. browser_tabs 로 확인하고 쓰지 않는 탭을 browser_close 로 닫은 뒤 다시 여세요.",
                policy::MAX_TABS_PER_SESSION
            ));
        }
        let window = self.ensure_window().await?;
        let record = self.inner.registry.lock().insert(session_id, project_id);
        let builder = self.tab_builder(&record.label);
        let rect = window
            .inner_size()
            .map(|size| self.tab_rect(size))
            .map_err(|error| format!("브라우저 창 크기를 읽을 수 없습니다({error})"));
        let added = match rect {
            Ok(rect) => {
                let window = window.clone();
                tokio::task::spawn_blocking(move || {
                    window
                        .add_child(builder, rect.position, rect.size)
                        .map_err(|error| {
                            format!("탭을 만들 수 없습니다({error}). 다시 시도하세요.")
                        })
                })
                .await
                .map_err(|error| format!("탭을 만들 수 없습니다({error})"))
                .and_then(|result| result)
            }
            Err(error) => Err(error),
        };
        let webview = match added {
            Ok(webview) => webview,
            Err(error) => {
                self.inner.registry.lock().remove(&record.tab_id, false);
                self.emit_state();
                return Err(error);
            }
        };
        match self.attach(&record, webview.clone()).await {
            Ok(tab) => {
                if self.inner.chrome_expanded.load(Ordering::SeqCst) {
                    self.raise_chrome();
                }
                self.emit_state();
                Ok((record, tab))
            }
            Err(error) => {
                self.inner.registry.lock().remove(&record.tab_id, false);
                cdp::unsubscribe_all(&webview);
                let _ = webview.close();
                self.emit_state();
                Err(error)
            }
        }
    }

    /// Follow `webview`'s main-frame navigations and register it.
    async fn attach(&self, record: &TabRecord, webview: Webview) -> Result<TabRuntime, String> {
        let (bump, navigations) = tokio::sync::watch::channel(0_u64);
        let bump = Arc::new(bump);
        let guard = Arc::new(TabGuard::default());
        let popups = Arc::clone(&guard);
        cdp::install_page_guards(&webview, move |uri, user_initiated| {
            let mut asked = popups.popups.lock();
            // A page that keeps asking cannot grow the map without bound.
            if asked.len() >= 64 {
                asked.clear();
            }
            // Keyed as the new-window handler will see it: a parsed URL.
            let key = tauri::Url::parse(&uri).map_or(uri, |url| url.to_string());
            asked.insert(key, user_initiated);
        })
        .await?;
        for event in ["Page.frameNavigated", "Page.navigatedWithinDocument"] {
            let mut events = cdp::subscribe(&webview, event).await?;
            let bump = Arc::clone(&bump);
            let weak = self.weak();
            let tab_id = record.tab_id.clone();
            let guard = Arc::clone(&guard);
            tauri::async_runtime::spawn(async move {
                while let Some(event) = events.recv().await {
                    let (main_frame, url) = match event.get("frame") {
                        // Page.frameNavigated: a document committed.
                        Some(frame) => {
                            let main = frame.get("parentId").is_none();
                            if main {
                                if let Some(id) = frame.get("id").and_then(Value::as_str) {
                                    *guard.main_frame.lock() = id.to_string();
                                }
                                UserNavigation::committed(
                                    &mut guard.user_navigation.lock(),
                                    frame.get("loaderId").and_then(Value::as_str),
                                );
                            }
                            (
                                main,
                                frame.get("url").and_then(Value::as_str).map(str::to_string),
                            )
                        }
                        // Page.navigatedWithinDocument: frameId + url.
                        None => (
                            event
                                .get("frameId")
                                .and_then(Value::as_str)
                                .map_or(true, |id| {
                                    let main = guard.main_frame.lock();
                                    main.is_empty() || *main == id
                                }),
                            event.get("url").and_then(Value::as_str).map(str::to_string),
                        ),
                    };
                    if !main_frame {
                        continue;
                    }
                    if event.get("frame").is_none() {
                        UserNavigation::committed(&mut guard.user_navigation.lock(), None);
                    }
                    bump.send_modify(|seq| *seq += 1);
                    if let Some(host) = BrowserHost::from_weak(&weak) {
                        {
                            let mut registry = host.inner.registry.lock();
                            registry.invalidate_refs(&tab_id);
                            registry.set_page(&tab_id, url, None);
                        }
                        host.emit_state();
                    }
                }
            });
        }
        self.follow_download_progress(record, &webview, &guard)
            .await?;
        cdp::call(&webview, "Page.enable", json!({})).await?;
        if let Ok(tree) = cdp::call(&webview, "Page.getFrameTree", json!({})).await {
            if let Some(id) = tree.pointer("/frameTree/frame/id").and_then(Value::as_str) {
                *guard.main_frame.lock() = id.to_string();
            }
        }
        let tab = TabRuntime {
            webview,
            navigations,
            guard,
        };
        // Requests are paused in the page target and in every out-of-process
        // iframe target (a cross-site frame): the page target's Fetch never
        // sees a frame target's own navigations.
        let mut paused = cdp::subscribe_sessions(&tab.webview, "Fetch.requestPaused").await?;
        let weak = self.weak();
        let paused_tab = tab.clone();
        tauri::async_runtime::spawn(async move {
            while let Some((session, event)) = paused.recv().await {
                let Some(host) = BrowserHost::from_weak(&weak) else {
                    break;
                };
                let tab = paused_tab.clone();
                // Each request is decided on its own: one waiting on a DNS
                // lookup never stalls the tab's other loads.
                tauri::async_runtime::spawn(async move {
                    host.decide_paused(&tab, session, event).await;
                });
            }
        });
        let mut attached = cdp::subscribe_sessions(&tab.webview, "Target.attachedToTarget").await?;
        let attached_tab = tab.clone();
        tauri::async_runtime::spawn(async move {
            while let Some((_, event)) = attached.recv().await {
                let Some(session) = event.get("sessionId").and_then(Value::as_str) else {
                    continue;
                };
                let session = session.to_string();
                let kind = event
                    .pointer("/targetInfo/type")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let tab = attached_tab.clone();
                tauri::async_runtime::spawn(async move {
                    guard_frame_target(&tab, &session, &kind).await;
                });
            }
        });
        let mut detached =
            cdp::subscribe_sessions(&tab.webview, "Target.detachedFromTarget").await?;
        let detached_guard = Arc::clone(&tab.guard);
        tauri::async_runtime::spawn(async move {
            while let Some((_, event)) = detached.recv().await {
                if let Some(session) = event.get("sessionId").and_then(Value::as_str) {
                    detached_guard.sessions.lock().remove(session);
                }
            }
        });
        self.follow_document_responses(&tab).await?;
        follow_fetch_responses(&tab).await?;
        cdp::call(&tab.webview, "Network.enable", network_enable()).await?;
        // A service worker could answer the page's requests itself, where
        // the page's Fetch never sees them.
        cdp::call(
            &tab.webview,
            "Network.setBypassServiceWorker",
            json!({"bypass": true}),
        )
        .await?;
        cdp::call(&tab.webview, "Fetch.enable", fetch_patterns(false)).await?;
        cdp::call(&tab.webview, "Target.setAutoAttach", auto_attach()).await?;
        self.inner
            .tabs
            .lock()
            .insert(record.tab_id.clone(), tab.clone());
        Ok(tab)
    }

    fn tab_builder(&self, label: &str) -> WebviewBuilder<tauri::Wry> {
        let (on_popup, on_title, on_load, on_download) =
            (self.weak(), self.weak(), self.weak(), self.weak());
        let popup_label = label.to_string();
        WebviewBuilder::new(
            label,
            WebviewUrl::External("about:blank".parse().expect("about:blank parses")),
        )
        .data_directory(self.inner.profile_dir.clone())
        .additional_browser_args(BROWSER_ARGS)
        .focused(false)
        .on_navigation(super::page_navigation_allowed)
        .on_download(move |webview, event| {
            Self::from_weak(&on_download)
                .is_some_and(|host| host.on_download(webview.label(), event))
        })
        .on_new_window(move |url, _features| {
            if let Some(host) = Self::from_weak(&on_popup) {
                host.open_popup(&popup_label, url);
            }
            NewWindowResponse::Deny
        })
        .on_document_title_changed(move |webview, title| {
            if let Some(host) = Self::from_weak(&on_title) {
                if let Some(tab) = host.inner.registry.lock().by_label_mut(webview.label()) {
                    tab.title = title;
                }
                host.emit_state();
            }
        })
        .on_page_load(move |webview, payload| {
            if payload.event() != PageLoadEvent::Finished {
                return;
            }
            if let Some(host) = Self::from_weak(&on_load) {
                if let Some(tab) = host.inner.registry.lock().by_label_mut(webview.label()) {
                    tab.url = payload.url().to_string();
                }
                host.emit_state();
            }
        })
    }

    /// A page opened a window: it becomes a new tab of the same session,
    /// when a person's gesture or an agent call opened it ([`policy::PopupGate`]:
    /// a few at a time, within the session's tab limit). Any other popup is
    /// dropped.
    fn open_popup(&self, opener_label: &str, url: tauri::Url) {
        let owner = {
            let registry = self.inner.registry.lock();
            registry
                .all()
                .iter()
                .find(|tab| tab.label == opener_label)
                .map(|tab| {
                    (
                        tab.tab_id.clone(),
                        tab.session_id.clone(),
                        tab.project_id.clone(),
                        registry.of_session(&tab.session_id).len(),
                    )
                })
        };
        let Some((tab_id, session_id, project_id, open_tabs)) = owner else {
            return;
        };
        let Ok(opener) = self.runtime(&tab_id) else {
            return;
        };
        let user_initiated = opener
            .guard
            .popups
            .lock()
            .remove(url.as_str())
            .unwrap_or(false);
        // Only an interaction opens popups for the agent; a page that opens
        // one while the agent merely reads it gets none without a gesture.
        let (driver, opener_context) = opener.guard.agent.lock().acting(Instant::now());
        let agent_driven = driver == policy::Driver::Interacting;
        let checked = if agent_driven {
            super::validate_agent_url(url.as_str())
        } else {
            super::validate_url(url.as_str())
        };
        let Ok(url) = checked else {
            eprintln!("eud-agent: browser popup refused (address): {url}");
            return;
        };
        if let Err(refusal) = self.inner.popups.lock().admit(
            &session_id,
            user_initiated,
            agent_driven,
            open_tabs,
            Instant::now(),
        ) {
            eprintln!("eud-agent: browser popup refused ({refusal:?}): {url}");
            return;
        }
        // A popup the agent's interaction opened is the agent's load: it
        // runs under that interaction (and leaves it a grace), so its
        // requests are checked like the opener's.
        let inherited = opener_context.filter(|_| agent_driven);
        let host = self.clone();
        tauri::async_runtime::spawn(async move {
            match host.create_tab(&session_id, &project_id).await {
                Ok((record, tab)) => {
                    let interaction =
                        inherited.map(|context| Interaction::enter(&host, &tab, context));
                    if interaction.is_some() {
                        if let Err(error) = host.arm_scripts(&tab).await {
                            eprintln!("eud-agent: browser popup tab was not guarded: {error}");
                            drop(interaction);
                            host.close_tab(&record.tab_id, false);
                            return;
                        }
                    }
                    if let Err(error) = host.navigate(&tab, &NavTarget::Url(url)).await {
                        eprintln!("eud-agent: browser popup tab did not load: {error}");
                    }
                    drop(interaction);
                }
                Err(error) => eprintln!("eud-agent: browser popup tab was not opened: {error}"),
            }
        });
    }

    fn tabs_of(&self, session_id: &str) -> Value {
        let registry = self.inner.registry.lock();
        let active = registry.active().map(str::to_string);
        let tabs = registry
            .of_session(session_id)
            .into_iter()
            .map(|tab| {
                json!({
                    "tabId": tab.tab_id,
                    "url": tab.url,
                    "title": tab.title,
                    "active": active.as_deref() == Some(tab.tab_id.as_str()),
                })
            })
            .collect::<Vec<_>>();
        json!({"tabs": tabs, "untrusted": true})
    }

    fn close_tab(&self, tab_id: &str, by_user: bool) {
        let removed = self.inner.registry.lock().remove(tab_id, by_user);
        let runtime = self.inner.tabs.lock().remove(tab_id);
        if let Some(tab) = runtime {
            cdp::unsubscribe_all(&tab.webview);
            if let Err(error) = tab.webview.close() {
                eprintln!("eud-agent: browser tab {tab_id} could not be closed: {error}");
            }
        }
        if removed.is_some() {
            if self.inner.registry.lock().all().is_empty() {
                // The last tab went, whoever closed it: the window goes
                // too, and the next browser_open makes it again.
                if let Some(window) = self.inner.app.get_window(BROWSER_WINDOW_LABEL) {
                    let _ = window.close();
                }
                return;
            }
            self.raise_active();
            self.emit_state();
        }
    }

    /// Delete the profile and downloads folders while no window or tab
    /// exists; holding the window lock keeps a tool call from creating the
    /// window meanwhile.
    pub async fn reset_profile(&self) -> Result<(), String> {
        let _creating = self.inner.window.lock().await;
        let open = self.inner.app.get_window(BROWSER_WINDOW_LABEL).is_some()
            || !self.inner.registry.lock().all().is_empty();
        if !open {
            self.inner.downloads.forget_all();
        }
        let profile = self.inner.profile_dir.clone();
        let downloads = self.inner.downloads.root().to_path_buf();
        tokio::task::spawn_blocking(move || super::reset_profile_dirs(open, &profile, &downloads))
            .await
            .map_err(|error| {
                format!("로그인 데이터를 지우지 못했습니다({error}). 다시 시도하세요.")
            })?
    }

    pub fn close_tabs_where(&self, filter: impl Fn(&TabRecord) -> bool) {
        let ids = self
            .inner
            .registry
            .lock()
            .all()
            .iter()
            .filter(|tab| filter(tab))
            .map(|tab| tab.tab_id.clone())
            .collect::<Vec<_>>();
        for id in ids {
            self.close_tab(&id, false);
        }
    }

    /// The session was deleted: close its tabs.
    pub fn close_session(&self, session_id: &str) {
        self.close_tabs_where(|tab| tab.session_id == session_id);
    }

    pub fn close_project_tabs(&self, project_id: &str) {
        self.close_tabs_where(|tab| tab.project_id == project_id);
    }

    fn raise_active(&self) {
        let active = self.inner.registry.lock().active().map(str::to_string);
        if let Some(tab) = active.and_then(|id| self.inner.tabs.lock().get(&id).cloned()) {
            if let Err(error) = cdp::bring_to_front(&tab.webview) {
                eprintln!("eud-agent: browser tab could not be raised: {error}");
            }
        }
    }

    // ---------------------------------------------------------------- chrome

    /// Session titles by id, as last read. The session list (a directory
    /// read) is never read on the caller's thread, which may be the UI
    /// thread: a cache older than [`TITLES_TTL`] (or none yet) is refreshed
    /// on a blocking worker, which then tells the chrome again.
    fn session_titles(&self) -> BTreeMap<String, String> {
        let (titles, stale) = match self.inner.titles.lock().as_ref() {
            Some((read, titles)) => (titles.clone(), read.elapsed() >= TITLES_TTL),
            None => (BTreeMap::new(), true),
        };
        if stale && !self.inner.titles_refreshing.swap(true, Ordering::SeqCst) {
            let host = self.clone();
            tauri::async_runtime::spawn_blocking(move || {
                let titles = host
                    .inner
                    .sessions
                    .list()
                    .map(|sessions| {
                        sessions
                            .into_iter()
                            .map(|meta| (meta.id, meta.name))
                            .collect::<BTreeMap<_, _>>()
                    })
                    .unwrap_or_default();
                let changed = host
                    .inner
                    .titles
                    .lock()
                    .replace((Instant::now(), titles.clone()))
                    .map_or(true, |(_, before)| before != titles);
                host.inner.titles_refreshing.store(false, Ordering::SeqCst);
                if changed {
                    host.emit_state();
                }
            });
        }
        titles
    }

    pub fn state(&self) -> BrowserState {
        let titles = self.session_titles();
        let mut state = self
            .inner
            .registry
            .lock()
            .view(|session| titles.get(session).cloned());
        state.downloads = self
            .inner
            .downloads
            .views(|session| titles.get(session).cloned());
        state
    }

    /// Tell the chrome the browser state: at most once per
    /// [`EMIT_INTERVAL`], with the state as it is when the emit runs
    /// (trailing edge), and never on the caller's thread (page events
    /// arrive on the UI thread).
    fn emit_state(&self) {
        if self.inner.emit_pending.swap(true, Ordering::SeqCst) {
            return;
        }
        let host = self.clone();
        tauri::async_runtime::spawn(async move {
            let since = host.inner.last_emit.lock().map(|at| at.elapsed());
            if let Some(since) = since.filter(|since| *since < EMIT_INTERVAL) {
                tokio::time::sleep(EMIT_INTERVAL - since).await;
            }
            // Cleared before the state is read: a change from here on
            // schedules the next emit.
            host.inner.emit_pending.store(false, Ordering::SeqCst);
            *host.inner.last_emit.lock() = Some(Instant::now());
            host.emit_now();
        });
    }

    fn emit_now(&self) {
        if self.inner.app.get_webview(BROWSER_WINDOW_LABEL).is_none() {
            return;
        }
        let state = self.state();
        if let Err(error) = self
            .inner
            .app
            .emit_to(BROWSER_WINDOW_LABEL, BROWSER_STATE_EVENT, state)
        {
            eprintln!("eud-agent: browser state emit failed: {error}");
        }
    }

    pub fn activate(&self, tab_id: &str) -> Result<(), String> {
        if !self.inner.registry.lock().activate(tab_id) {
            return Err("탭을 찾을 수 없습니다. 이미 닫힌 탭입니다.".to_string());
        }
        self.raise_active();
        self.emit_state();
        Ok(())
    }

    pub fn user_close(&self, tab_id: &str) -> Result<(), String> {
        if self.inner.registry.lock().get(tab_id).is_none() {
            return Err("탭을 찾을 수 없습니다. 이미 닫힌 탭입니다.".to_string());
        }
        self.close_tab(tab_id, true);
        Ok(())
    }

    pub async fn user_navigate(
        &self,
        tab_id: &str,
        url: Option<String>,
        action: Option<String>,
    ) -> Result<(), String> {
        let target = match (url, action.as_deref()) {
            (Some(url), None) => NavTarget::Url(super::validate_url(&url)?),
            (None, Some("back")) => NavTarget::Back,
            (None, Some("forward")) => NavTarget::Forward,
            (None, Some("reload")) => NavTarget::Reload,
            _ => return Err("주소나 뒤로/앞으로/새로고침 중 하나를 고르세요.".to_string()),
        };
        let tab = self
            .runtime(tab_id)
            .map_err(|_| "탭을 찾을 수 없습니다. 이미 닫힌 탭입니다.".to_string())?;
        *tab.guard.user_navigation.lock() = Some(UserNavigation::Pending(
            Instant::now() + USER_NAVIGATION_WAIT,
        ));
        let started = self.start_navigation(&tab, &target).await;
        let mut navigation = tab.guard.user_navigation.lock();
        match &started {
            // Its document request is the one `Page.navigate` started.
            Ok((true, Some(loader))) => {
                *navigation = Some(UserNavigation::Request(loader.clone()));
            }
            Ok((true, None)) => {}
            // A failed start, or a same-document move, loads nothing.
            Ok((false, _)) | Err(_) => *navigation = None,
        }
        started.map(|_| ())
    }

    // ---------------------------------------------------------------- page

    /// Start `target` on `tab`; `true` when it is a document load that
    /// commits a new main frame (and so bumps `navigations`), with its
    /// loader when `Page.navigate` reported one.
    async fn start_navigation(
        &self,
        tab: &TabRuntime,
        target: &NavTarget,
    ) -> Result<(bool, Option<String>), String> {
        let webview = &tab.webview;
        match target {
            NavTarget::Url(url) => {
                let result =
                    cdp::call(webview, "Page.navigate", json!({"url": url.as_str()})).await?;
                if let Some(error) = result.get("errorText").and_then(Value::as_str) {
                    return Err(format!(
                        "페이지를 열 수 없습니다: {error}. 주소를 확인하거나 잠시 뒤 다시 시도하세요."
                    ));
                }
                // A same-document navigation (only the fragment changed) has
                // no loader.
                let loader = result
                    .get("loaderId")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                Ok((loader.is_some(), loader))
            }
            NavTarget::Reload => {
                cdp::call(webview, "Page.reload", json!({})).await?;
                Ok((true, None))
            }
            NavTarget::Back | NavTarget::Forward => {
                let history = cdp::call(webview, "Page.getNavigationHistory", json!({})).await?;
                let current = history
                    .get("currentIndex")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                let entries = history
                    .get("entries")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let index = if *target == NavTarget::Back {
                    current - 1
                } else {
                    current + 1
                };
                let entry = usize::try_from(index)
                    .ok()
                    .and_then(|index| entries.get(index))
                    .ok_or_else(|| {
                        if *target == NavTarget::Back {
                            "뒤로 갈 페이지가 없습니다.".to_string()
                        } else {
                            "앞으로 갈 페이지가 없습니다.".to_string()
                        }
                    })?;
                cdp::call(
                    webview,
                    "Page.navigateToHistoryEntry",
                    json!({"entryId": entry.get("id")}),
                )
                .await?;
                Ok((true, None))
            }
        }
    }

    /// Navigate and wait (≤ [`NAVIGATION_WAIT`]) for the page to load;
    /// `false` when it did not finish in time.
    async fn navigate(&self, tab: &TabRuntime, target: &NavTarget) -> Result<bool, String> {
        if matches!(target, NavTarget::Back | NavTarget::Forward) {
            self.require_public_history_entry(tab, target).await?;
        }
        let deadline = Instant::now() + NAVIGATION_WAIT;
        let before = *tab.navigations.borrow();
        let (committing, _) = self.start_navigation(tab, target).await?;
        if committing {
            let mut navigations = tab.navigations.clone();
            while *navigations.borrow_and_update() == before {
                let wait = deadline.saturating_duration_since(Instant::now());
                match tokio::time::timeout(wait, navigations.changed()).await {
                    Ok(Ok(())) => {}
                    Ok(Err(_)) => return Err(super::registry::TAB_NOT_FOUND.to_string()),
                    Err(_) => return Ok(false),
                }
            }
        }
        loop {
            if evaluate(&tab.webview, "document.readyState").await.ok() == Some(json!("complete")) {
                return Ok(true);
            }
            if Instant::now() + POLL > deadline {
                return Ok(false);
            }
            tokio::time::sleep(POLL).await;
        }
    }

    /// An agent's back/forward may restore a page without loading it (the
    /// back/forward cache), where neither the request policy nor the
    /// response check sees it: the entry must not be a local address, nor a
    /// page a local address answered.
    async fn require_public_history_entry(
        &self,
        tab: &TabRuntime,
        target: &NavTarget,
    ) -> Result<(), String> {
        let history = cdp::call(&tab.webview, "Page.getNavigationHistory", json!({})).await?;
        let current = history
            .get("currentIndex")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let index = if *target == NavTarget::Back {
            current - 1
        } else {
            current + 1
        };
        let Some(url) = usize::try_from(index)
            .ok()
            .and_then(|index| history.pointer(&format!("/entries/{index}/url")))
            .and_then(Value::as_str)
        else {
            // No such entry: the navigation itself says so.
            return Ok(());
        };
        let parsed = tauri::Url::parse(url).ok();
        let local_host = parsed
            .as_ref()
            .and_then(super::agent_host_is_local)
            .or_else(|| {
                tab.guard
                    .local_pages
                    .lock()
                    .iter()
                    .any(|page| page == url)
                    .then(|| {
                        parsed
                            .as_ref()
                            .and_then(|url| url.host_str().map(str::to_string))
                            .unwrap_or_else(|| url.to_string())
                    })
            });
        match local_host {
            Some(host) => Err(super::internal_refusal(&host)),
            None => Ok(()),
        }
    }

    /// The live URL and title of `tab`, falling back to what the registry saw.
    async fn page_info(&self, record: &TabRecord, tab: &TabRuntime) -> (String, String) {
        if let Ok(Value::Array(values)) =
            evaluate(&tab.webview, "[location.href, document.title]").await
        {
            if let [Value::String(url), Value::String(title)] = values.as_slice() {
                self.inner.registry.lock().set_page(
                    &record.tab_id,
                    Some(url.clone()),
                    Some(title.clone()),
                );
                return (url.clone(), title.clone());
            }
        }
        let registry = self.inner.registry.lock();
        let tab = registry.get(&record.tab_id).unwrap_or(record);
        (tab.url.clone(), tab.title.clone())
    }

    async fn page_result(
        &self,
        record: &TabRecord,
        tab: &TabRuntime,
        loaded: bool,
    ) -> Result<Value, String> {
        let (url, title) = self.page_info(record, tab).await;
        self.emit_state();
        let mut value = json!({
            "tabId": record.tab_id,
            "url": url,
            "title": title,
            "loaded": loaded,
            "untrusted": true,
        });
        if !loaded {
            value["note"] = json!(
                "The page did not finish loading within 30 s; this is its current state. Use browser_wait or browser_snapshot."
            );
        }
        Ok(value)
    }

    async fn snapshot(
        &self,
        record: &TabRecord,
        tab: &TabRuntime,
        offset: usize,
    ) -> Result<Value, String> {
        let tree = cdp::call_with(
            &tab.webview,
            "Accessibility.getFullAXTree",
            json!({}),
            Duration::from_secs(30),
        )
        .await?;
        let formatted = snapshot::format_tree(&tree);
        let (url, title) = self.page_info(record, tab).await;
        let total = formatted.lines.len();
        self.inner
            .registry
            .lock()
            .set_refs(&record.tab_id, formatted.refs);
        Ok(snapshot::page(
            &formatted.lines,
            offset,
            SNAPSHOT_BUDGET,
            |text, next| {
                json!({
                    "tabId": record.tab_id,
                    "url": url,
                    "title": title,
                    "untrusted": true,
                    "totalLines": total,
                    "offset": offset.min(total),
                    "nextOffset": next,
                    "snapshot": text,
                })
            },
        ))
    }

    async fn screenshot(
        &self,
        record: &TabRecord,
        tab: &TabRuntime,
        full_page: bool,
    ) -> Result<Value, String> {
        if self
            .inner
            .app
            .get_window(BROWSER_WINDOW_LABEL)
            .and_then(|window| window.is_minimized().ok())
            == Some(true)
        {
            return Err(
                "브라우저 창이 최소화되어 있어 화면을 캡처할 수 없습니다. 사용자에게 창을 복원해 달라고 요청한 뒤 다시 시도하세요."
                    .to_string(),
            );
        }
        let mut truncated = false;
        let params = if full_page {
            let metrics = cdp::call(&tab.webview, "Page.getLayoutMetrics", json!({})).await?;
            let content = metrics
                .get("cssContentSize")
                .or_else(|| metrics.get("contentSize"))
                .cloned()
                .unwrap_or(Value::Null);
            let ratio = evaluate(&tab.webview, "window.devicePixelRatio")
                .await
                .ok()
                .and_then(|ratio| ratio.as_f64())
                .filter(|ratio| *ratio > 0.0)
                .unwrap_or(1.0);
            let limit = (MAX_SCREENSHOT_SIDE / ratio).floor();
            let width = content.get("width").and_then(Value::as_f64).unwrap_or(0.0);
            let height = content.get("height").and_then(Value::as_f64).unwrap_or(0.0);
            truncated = width > limit || height > limit;
            json!({
                "format": "png",
                "captureBeyondViewport": true,
                "clip": {
                    "x": 0, "y": 0,
                    "width": width.clamp(1.0, limit),
                    "height": height.clamp(1.0, limit),
                    "scale": 1,
                },
            })
        } else {
            json!({"format": "png"})
        };
        let result = cdp::call_with(
            &tab.webview,
            "Page.captureScreenshot",
            params,
            SCREENSHOT_TIMEOUT,
        )
        .await?;
        let data = result
            .get("data")
            .and_then(Value::as_str)
            .ok_or_else(|| "스크린샷 응답에 이미지가 없습니다. 다시 시도하세요.".to_string())?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|error| format!("스크린샷을 해석할 수 없습니다({error})"))?;
        if bytes.len() > MAX_SCREENSHOT_BYTES {
            return Err(format!(
                "스크린샷이 {} MiB로 상한 5 MiB를 넘습니다. fullPage 없이 화면에 보이는 부분만 캡처하거나 browser_snapshot 을 쓰세요.",
                bytes.len() / (1024 * 1024)
            ));
        }
        let (width, height) = png_size(&bytes)
            .ok_or_else(|| "스크린샷이 PNG가 아닙니다. 다시 시도하세요.".to_string())?;
        let (url, title) = self.page_info(record, tab).await;
        Ok(json!({
            "tabId": record.tab_id,
            "url": url,
            "title": title,
            "untrusted": true,
            "fullPage": full_page,
            "truncated": truncated,
            "image": {
                "mimeType": "image/png",
                "width": width,
                "height": height,
                "data": data,
            },
        }))
    }

    async fn wait(
        &self,
        record: &TabRecord,
        tab: &TabRuntime,
        text: Option<String>,
        ms: u64,
    ) -> Result<Value, String> {
        let started = Instant::now();
        let deadline = started + Duration::from_millis(ms);
        let found = match &text {
            Some(text) => {
                let expression = format!(
                    "!!(document.body && document.body.innerText.includes({}))",
                    serde_json::to_string(text).expect("a string serializes")
                );
                loop {
                    if evaluate(&tab.webview, &expression).await.ok() == Some(json!(true)) {
                        break Some(true);
                    }
                    if Instant::now() + POLL > deadline {
                        break Some(false);
                    }
                    tokio::time::sleep(POLL).await;
                }
            }
            None => {
                tokio::time::sleep_until(deadline.into()).await;
                None
            }
        };
        let (url, title) = self.page_info(record, tab).await;
        let mut value = json!({
            "tabId": record.tab_id,
            "url": url,
            "title": title,
            "waitedMs": started.elapsed().as_millis() as u64,
            "untrusted": true,
        });
        if let Some(found) = found {
            value["found"] = json!(found);
        }
        Ok(value)
    }
}

impl BrowserHost {
    // ---------------------------------------------------------------- policy

    /// Decide one paused request ([`policy::fetch_decision`]). `session` is
    /// the DevTools session of the frame target it was paused in (an
    /// out-of-process iframe), `None` for the tab's own page target.
    async fn decide_paused(&self, tab: &TabRuntime, session: Option<String>, event: Value) {
        let Some(request_id) = event.get("requestId").cloned() else {
            return;
        };
        let request = event.get("request").cloned().unwrap_or(Value::Null);
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("GET");
        let url = request.get("url").and_then(Value::as_str).unwrap_or("");
        let kind = event
            .get("resourceType")
            .and_then(Value::as_str)
            .unwrap_or("Document");
        let frame = event.get("frameId").and_then(Value::as_str).unwrap_or("");
        let main_frame = session.is_none() && {
            let main = tab.guard.main_frame.lock();
            main.is_empty() || *main == frame
        };
        if main_frame && kind == "Document" {
            tab.guard
                .submissions
                .send_modify(|state| state.documents += 1);
        }
        let network_id = event.get("networkId").and_then(Value::as_str);
        // While an agent call runs on the tab, or an interaction's grace has
        // not ended, every request is checked as the agent's: a user
        // navigation that failed or is still unmatched never exempts one of
        // the agent's loads (a page script's navigation after its click).
        let now = Instant::now();
        let (driver, _) = tab.guard.agent.lock().acting(now);
        let by_user = main_frame
            && kind == "Document"
            && UserNavigation::claims_while(
                &mut tab.guard.user_navigation.lock(),
                driver,
                method,
                network_id,
                now,
            );
        let driver = if by_user {
            policy::Driver::User
        } else {
            driver
        };
        let mut decision = policy::fetch_decision(url, kind, driver);
        let agent_document = kind == "Document" && driver != policy::Driver::User;
        if agent_document && decision == FetchDecision::Continue {
            // A public name may resolve into the local network.
            if let Some(host) = resolves_to_local(url).await {
                decision = FetchDecision::Internal { host };
            }
        }
        if agent_document {
            // Its response address is checked when it arrives.
            if let Some(id) = network_id {
                let mut tracked = tab.guard.agent_documents.lock();
                if tracked.len() >= MAX_TRACKED_DOCUMENTS {
                    tracked.clear();
                }
                tracked.insert((session.clone(), id.to_string()));
            }
        }
        let reason = match decision {
            FetchDecision::Continue => None,
            FetchDecision::Block => {
                eprintln!("eud-agent: browser refused a page load of the app's own origin: {url}");
                Some("BlockedByClient")
            }
            FetchDecision::Internal { host } => {
                eprintln!(
                    "eud-agent: browser refused an agent page load of a local address: {url}"
                );
                if main_frame {
                    *tab.guard.internal.lock() = Some(host);
                }
                Some("BlockedByClient")
            }
        };
        if reason.is_none() && main_frame && kind == "Document" {
            // A new main-frame load starts: the refused one is gone.
            *tab.guard.internal.lock() = None;
        }
        let result = match reason {
            None => {
                cdp::call_in(
                    &tab.webview,
                    session.as_deref(),
                    "Fetch.continueRequest",
                    json!({"requestId": request_id}),
                )
                .await
            }
            Some(reason) => {
                cdp::call_in(
                    &tab.webview,
                    session.as_deref(),
                    "Fetch.failRequest",
                    json!({"requestId": request_id, "errorReason": reason}),
                )
                .await
            }
        };
        if let Err(error) = result {
            eprintln!("eud-agent: browser request decision was not applied: {error}");
        }
    }

    /// Watch the address every document response came from, in the page
    /// and in every guarded iframe: a page an agent call loaded from this
    /// computer or the local network (a public name that resolves there) is
    /// replaced by `about:blank`, and the tab's reads refuse until its next
    /// load. Every local page is also remembered, so the agent's back/forward
    /// never returns to one.
    async fn follow_document_responses(&self, tab: &TabRuntime) -> Result<(), String> {
        let mut responses = cdp::subscribe_sessions_matching(
            &tab.webview,
            "Network.responseReceived",
            &["\"type\":\"Document\"", "\"type\": \"Document\""],
        )
        .await?;
        let tab = tab.clone();
        tauri::async_runtime::spawn(async move {
            while let Some((session, event)) = responses.recv().await {
                if event.get("type").and_then(Value::as_str) != Some("Document") {
                    continue;
                }
                let request = event.get("requestId").and_then(Value::as_str).unwrap_or("");
                let by_agent = tab
                    .guard
                    .agent_documents
                    .lock()
                    .remove(&(session.clone(), request.to_string()));
                let response = event.get("response").cloned().unwrap_or(Value::Null);
                let address = response
                    .get("remoteIPAddress")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if !super::remote_address_is_local(address) {
                    continue;
                }
                let url = response.get("url").and_then(Value::as_str).unwrap_or("");
                {
                    let mut local = tab.guard.local_pages.lock();
                    if local.len() >= MAX_LOCAL_PAGES {
                        local.pop_front();
                    }
                    local.push_back(url.to_string());
                }
                if !by_agent {
                    continue;
                }
                let host = tauri::Url::parse(url)
                    .ok()
                    .and_then(|url| url.host_str().map(str::to_string))
                    .unwrap_or_else(|| address.to_string());
                eprintln!(
                    "eud-agent: browser replaced an agent page load answered from a local address ({address}): {url}"
                );
                *tab.guard.internal.lock() = Some(host);
                if let Err(error) =
                    cdp::call(&tab.webview, "Page.navigate", json!({"url": "about:blank"})).await
                {
                    eprintln!("eud-agent: browser tab was not blanked: {error}");
                }
            }
        });
        Ok(())
    }

    /// Pause script requests too ([`SCRIPT_PATTERNS`]), in the page and in
    /// every guarded child session, before an interaction acts. Failing in
    /// the page or in a child session that is still attached fails the call
    /// (fail closed): nothing is dispatched unprotected. The caller's
    /// [`Interaction`] releases whatever was armed.
    async fn arm_scripts(&self, tab: &TabRuntime) -> Result<(), String> {
        let _arming = tab.guard.arming.lock().await;
        if tab.guard.scripts_armed.load(Ordering::SeqCst) {
            return Ok(());
        }
        cdp::call(&tab.webview, "Fetch.enable", fetch_patterns(true))
            .await
            .map_err(|error| {
                format!(
                    "탭의 요청 감시를 켤 수 없어 동작하지 않았습니다({error}). 다시 시도하세요."
                )
            })?;
        tab.guard.scripts_armed.store(true, Ordering::SeqCst);
        let sessions = tab
            .guard
            .sessions
            .lock()
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        for session in sessions {
            if let Err(error) = cdp::call_in(
                &tab.webview,
                Some(&session),
                "Fetch.enable",
                fetch_patterns(true),
            )
            .await
            {
                // A frame that detached meanwhile makes no more requests.
                if !tab.guard.sessions.lock().contains(&session) {
                    continue;
                }
                eprintln!("eud-agent: browser child target script guard failed: {error}");
                return Err(format!(
                    "탭 안의 프레임 요청 감시를 켤 수 없어 동작하지 않았습니다({error}). browser_snapshot 으로 페이지를 다시 읽은 뒤 다시 시도하세요."
                ));
            }
        }
        Ok(())
    }

    /// Once no interaction runs on the tab and its grace is over, stop
    /// pausing script requests.
    fn schedule_disarm(&self, tab: &TabRuntime) {
        let tab = tab.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                let wait = {
                    let calls = tab.guard.agent.lock();
                    if calls.interaction_running() {
                        // That interaction schedules its own disarm.
                        return;
                    }
                    calls.grace_until(Instant::now())
                };
                match wait {
                    Some(until) => tokio::time::sleep_until(until.into()).await,
                    None => break,
                }
            }
            let _arming = tab.guard.arming.lock().await;
            if !tab.guard.scripts_armed.load(Ordering::SeqCst)
                || tab.guard.agent.lock().acting(Instant::now()).0 == policy::Driver::Interacting
            {
                return;
            }
            tab.guard.scripts_armed.store(false, Ordering::SeqCst);
            let sessions = tab
                .guard
                .sessions
                .lock()
                .iter()
                .cloned()
                .collect::<Vec<_>>();
            let targets = std::iter::once(None).chain(sessions.into_iter().map(Some));
            for session in targets {
                if let Err(error) = cdp::call_in(
                    &tab.webview,
                    session.as_deref(),
                    "Fetch.enable",
                    fetch_patterns(false),
                )
                .await
                {
                    eprintln!("eud-agent: browser script guard was not released: {error}");
                }
            }
        });
    }

    /// Ask the agent call's user to allow one action, within its deadline.
    /// The session's confirmations are shown one at a time, the next waiting
    /// for the shown one to settle ([`policy::MAX_OPEN_CONFIRMATIONS`] open
    /// or waiting at once). Only 허용 allows; the user's 거부 or a shown card
    /// left unanswered is [`ConfirmAnswer::Denied`]. A question that never
    /// reached the user (nobody to ask, no time left, too many waiting, an
    /// error) is [`ConfirmAnswer::Unasked`]: also refused, but reported as
    /// such, never as the user's decision.
    async fn confirm(&self, tab_id: &str, agent: &AgentContext, question: String) -> ConfirmAnswer {
        let wait = agent
            .deadline
            .saturating_duration_since(Instant::now())
            .saturating_sub(AFTER_CONFIRM)
            .min(CONFIRM_WAIT);
        let Some(confirm) = agent.confirm.as_ref().filter(|_| !wait.is_zero()) else {
            return ConfirmAnswer::Unasked;
        };
        if !self.inner.confirmations.lock().try_open(&agent.session_id) {
            eprintln!(
                "eud-agent: browser confirmation not asked: {} already open or waiting for this session",
                policy::MAX_OPEN_CONFIRMATIONS
            );
            return ConfirmAnswer::Unasked;
        }
        self.inner.registry.lock().mark_confirming(tab_id, true);
        // Closed and unmarked however this ends, also when the call is
        // dropped mid-wait.
        let _slot = ConfirmationSlot {
            host: self.clone(),
            session_id: agent.session_id.clone(),
            tab_id: tab_id.to_string(),
        };
        self.emit_state();
        confirm
            .ask(BrowserConfirmation { question, wait })
            .await
            .unwrap_or_else(|error| {
                eprintln!("eud-agent: browser confirmation failed: {error}");
                ConfirmAnswer::Unasked
            })
    }

    // ---------------------------------------------------------------- input

    /// The page must be an ordinary web page before the agent acts on it.
    async fn require_web_page(&self, record: &TabRecord, tab: &TabRuntime) -> Result<(), String> {
        let (url, _) = self.page_info(record, tab).await;
        policy::interaction_allowed_on(&url)
    }

    /// The backend node `element` names in the tab's latest snapshot.
    fn backend_node(&self, record: &TabRecord, element: &str) -> Result<i64, String> {
        self.inner
            .registry
            .lock()
            .get(&record.tab_id)
            .and_then(|tab| tab.refs.get(element).copied())
            .ok_or_else(|| stale_ref(element))
    }

    /// The execution context of an isolated world in the tab's main frame,
    /// made once per navigation: the facts scripts run there, where the
    /// page's own script cannot replace the DOM methods they call. `None`
    /// when it cannot be made.
    async fn facts_context(&self, tab: &TabRuntime) -> Option<i64> {
        let navigation = *tab.navigations.borrow();
        if let Some((made_for, context)) = *tab.guard.facts_world.lock() {
            if made_for == navigation {
                return Some(context);
            }
        }
        let frame = tab.guard.main_frame.lock().clone();
        if frame.is_empty() {
            return None;
        }
        let world = cdp::call(
            &tab.webview,
            "Page.createIsolatedWorld",
            json!({"frameId": frame, "worldName": "eud-agent-facts"}),
        )
        .await
        .map_err(|error| eprintln!("eud-agent: browser isolated world was not made: {error}"))
        .ok()?;
        let context = world.get("executionContextId")?.as_i64()?;
        *tab.guard.facts_world.lock() = Some((navigation, context));
        Some(context)
    }

    /// A remote object id of DOM node `backend`, in the facts world when it
    /// can be made (a stale world is made again once), else in the page's
    /// own world.
    async fn resolve_node(&self, tab: &TabRuntime, backend: i64) -> Option<Value> {
        for _ in 0..2 {
            let Some(context) = self.facts_context(tab).await else {
                break;
            };
            match cdp::call(
                &tab.webview,
                "DOM.resolveNode",
                json!({"backendNodeId": backend, "executionContextId": context}),
            )
            .await
            {
                Ok(object) => return object.pointer("/object/objectId").cloned(),
                // The world went with a navigation the tab has not counted yet.
                Err(_) => *tab.guard.facts_world.lock() = None,
            }
        }
        cdp::call(
            &tab.webview,
            "DOM.resolveNode",
            json!({"backendNodeId": backend}),
        )
        .await
        .ok()?
        .pointer("/object/objectId")
        .cloned()
    }

    async fn element_facts(
        &self,
        tab: &TabRuntime,
        element: &str,
        backend: i64,
    ) -> Result<ElementFacts, String> {
        let object_id = self
            .resolve_node(tab, backend)
            .await
            .ok_or_else(|| stale_ref(element))?;
        let described = cdp::call(
            &tab.webview,
            "Runtime.callFunctionOn",
            json!({
                "objectId": object_id,
                "functionDeclaration": ELEMENT_FACTS_JS,
                "returnByValue": true,
            }),
        )
        .await;
        let _ = cdp::call(
            &tab.webview,
            "Runtime.releaseObject",
            json!({"objectId": object_id}),
        )
        .await;
        let value = described?
            .pointer("/result/value")
            .cloned()
            .filter(|value| value.is_object())
            .ok_or_else(|| stale_ref(element))?;
        let mut facts: ElementFacts = serde_json::from_value(value)
            .map_err(|error| format!("요소 정보를 해석할 수 없습니다({error})"))?;
        if let Ok(tree) = cdp::call(
            &tab.webview,
            "Accessibility.getPartialAXTree",
            json!({"backendNodeId": backend, "fetchRelatives": false}),
        )
        .await
        {
            let nodes = tree
                .get("nodes")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let node = nodes
                .iter()
                .find(|node| node.get("backendDOMNodeId").and_then(Value::as_i64) == Some(backend))
                .or_else(|| nodes.first());
            if let Some(node) = node {
                let text = |field: &str| {
                    node.pointer(&format!("/{field}/value"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string()
                };
                facts.role = text("role");
                facts.name = text("name");
            }
        }
        Ok(facts)
    }

    /// Scroll `backend` into view and return its centre in viewport CSS
    /// pixels.
    async fn element_center(&self, tab: &TabRuntime, backend: i64) -> Result<(f64, f64), String> {
        let not_drawn = || {
            "요소가 화면에 그려져 있지 않아 클릭할 수 없습니다(숨겨졌거나 크기가 없습니다). browser_snapshot 으로 다른 요소를 고르세요.".to_string()
        };
        let _ = cdp::call(
            &tab.webview,
            "DOM.scrollIntoViewIfNeeded",
            json!({"backendNodeId": backend}),
        )
        .await;
        let model = cdp::call(
            &tab.webview,
            "DOM.getBoxModel",
            json!({"backendNodeId": backend}),
        )
        .await
        .map_err(|_| not_drawn())?;
        ["content", "border"]
            .iter()
            .find_map(|quad| quad_center(model.pointer(&format!("/model/{quad}"))?))
            .ok_or_else(not_drawn)
    }

    /// Ask the user before an action that amounts to clicking `facts`, when
    /// it is named like a payment, purchase, subscription or account change.
    async fn confirm_sensitive(
        &self,
        record: &TabRecord,
        tab: &TabRuntime,
        facts: &ElementFacts,
        context: &AgentContext,
    ) -> Result<(), String> {
        let Some(term) = policy::sensitive_click(facts) else {
            return Ok(());
        };
        let (url, _) = self.page_info(record, tab).await;
        let question = format!(
            "에이전트가 브라우저에서 \"{}\" 을(를) 누르려고 합니다. 결제·구매·구독·계정 변경 같은 동작일 수 있습니다(`{term}`).\n페이지: {}\n허용할까요?",
            shorten(&element_label(facts), 120),
            shorten(&url, 300)
        );
        match self.confirm(&record.tab_id, context, question).await {
            ConfirmAnswer::Allowed => Ok(()),
            ConfirmAnswer::Denied => Err(policy::CLICK_DENIED.to_string()),
            ConfirmAnswer::Unasked => Err(policy::CLICK_UNASKED.to_string()),
        }
    }

    /// The backend node a remote object is, releasing the object.
    async fn object_backend(&self, tab: &TabRuntime, result: &Value) -> Option<i64> {
        let object_id = result.pointer("/result/objectId")?.clone();
        let described = cdp::call(
            &tab.webview,
            "DOM.describeNode",
            json!({"objectId": object_id}),
        )
        .await;
        let _ = cdp::call(
            &tab.webview,
            "Runtime.releaseObject",
            json!({"objectId": object_id}),
        )
        .await;
        described.ok()?.pointer("/node/backendNodeId")?.as_i64()
    }

    /// The element that has focus (through open shadow roots and
    /// same-origin frames, read in the facts world); `None` when nothing but
    /// the page itself has it. Focus that cannot be read is an error, never
    /// "nothing focused".
    async fn focused_backend(&self, tab: &TabRuntime) -> Result<Option<i64>, String> {
        let unreadable = |detail: &str| format!("{} ({detail})", policy::KEY_TARGET_UNKNOWN);
        let mut params = json!({"expression": FOCUSED_JS, "returnByValue": false});
        if let Some(context) = self.facts_context(tab).await {
            params["contextId"] = json!(context);
        }
        let mut result = cdp::call(&tab.webview, "Runtime.evaluate", params.clone()).await;
        if result.is_err() && params.get("contextId").is_some() {
            // The world went with a navigation the tab has not counted yet.
            *tab.guard.facts_world.lock() = None;
            if let Some(context) = self.facts_context(tab).await {
                params["contextId"] = json!(context);
                result = cdp::call(&tab.webview, "Runtime.evaluate", params).await;
            }
        }
        let result = result.map_err(|error| unreadable(&error))?;
        if result.get("exceptionDetails").is_some() {
            return Err(unreadable("script error"));
        }
        if result.pointer("/result/objectId").is_none() {
            return Ok(None);
        }
        self.object_backend(tab, &result)
            .await
            .map(Some)
            .ok_or_else(|| unreadable("node"))
    }

    /// The default button of the form text field `backend` belongs to: what
    /// Enter in that field clicks.
    async fn form_default_backend(&self, tab: &TabRuntime, backend: i64) -> Option<i64> {
        let object_id = self.resolve_node(tab, backend).await?;
        let result = cdp::call(
            &tab.webview,
            "Runtime.callFunctionOn",
            json!({
                "objectId": object_id,
                "functionDeclaration": FORM_DEFAULT_JS,
                "returnByValue": false,
            }),
        )
        .await;
        let _ = cdp::call(
            &tab.webview,
            "Runtime.releaseObject",
            json!({"objectId": object_id}),
        )
        .await;
        self.object_backend(tab, &result.ok()?).await
    }

    /// Before `key` is pressed: the click it amounts to on the focused
    /// element (a button, a link, a form's default button) is checked like a
    /// `browser_click` of that element.
    async fn confirm_key(
        &self,
        record: &TabRecord,
        tab: &TabRuntime,
        key: policy::KeyInfo,
        context: &AgentContext,
    ) -> Result<(), String> {
        if !matches!(key.code, "Enter" | "NumpadEnter" | "Space") {
            return Ok(());
        }
        let unreadable = || {
            "초점이 있는 요소를 확인할 수 없어 키를 누르지 않았습니다. browser_snapshot 으로 페이지를 다시 읽은 뒤 browser_click 으로 요소를 직접 누르세요.".to_string()
        };
        let Some(focused) = self.focused_backend(tab).await? else {
            return Ok(());
        };
        let facts = self
            .element_facts(tab, "focused", focused)
            .await
            .map_err(|_| unreadable())?;
        let activated = match policy::key_activation(key, &facts) {
            policy::KeyActivation::Nothing => return Ok(()),
            policy::KeyActivation::Unknown => return Err(policy::KEY_TARGET_UNKNOWN.to_string()),
            policy::KeyActivation::Element => facts,
            policy::KeyActivation::FormDefault => {
                let Some(button) = self.form_default_backend(tab, focused).await else {
                    return Ok(());
                };
                self.element_facts(tab, "focused", button)
                    .await
                    .map_err(|_| unreadable())?
            }
        };
        self.confirm_sensitive(record, tab, &activated, context)
            .await
    }

    /// The point the mouse would press must hit `backend` itself (or
    /// something inside it, or one of its labels), not an element drawn over
    /// it.
    async fn require_hit(
        &self,
        tab: &TabRuntime,
        element: &str,
        backend: i64,
        (x, y): (f64, f64),
    ) -> Result<(), String> {
        let covered = || {
            format!(
                "ref `{element}` 위에 다른 요소(팝업·배너·오버레이 등)가 덮여 있어 클릭하지 않았습니다. browser_snapshot 으로 다시 읽어 덮고 있는 요소를 닫거나(예: 닫기·동의 버튼) 스크롤한 뒤 다시 시도하세요. (The element is covered by another element; nothing was clicked.)"
            )
        };
        // `getNodeForLocation` takes document coordinates: add the scroll
        // offset the browser (not the page's script) reports.
        let metrics = cdp::call(&tab.webview, "Page.getLayoutMetrics", json!({}))
            .await
            .map_err(|_| covered())?;
        let viewport = metrics
            .get("cssLayoutViewport")
            .or_else(|| metrics.get("layoutViewport"));
        let offset = |field: &str| {
            viewport
                .and_then(|viewport| viewport.get(field))
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
        };
        let hit = cdp::call(
            &tab.webview,
            "DOM.getNodeForLocation",
            json!({
                "x": (x + offset("pageX")).round() as i64,
                "y": (y + offset("pageY")).round() as i64,
                "includeUserAgentShadowDOM": true,
                "ignorePointerEventsNone": false,
            }),
        )
        .await
        .map_err(|_| covered())?;
        let Some(hit) = hit.get("backendNodeId").and_then(Value::as_i64) else {
            return Err(covered());
        };
        if hit == backend {
            return Ok(());
        }
        // Both in the same (facts) world, so one function can compare them.
        let (Some(target), Some(other)) = (
            self.resolve_node(tab, backend).await,
            self.resolve_node(tab, hit).await,
        ) else {
            return Err(covered());
        };
        let inside = cdp::call(
            &tab.webview,
            "Runtime.callFunctionOn",
            json!({
                "objectId": target,
                "functionDeclaration": HIT_INSIDE_JS,
                "arguments": [{"objectId": other}],
                "returnByValue": true,
            }),
        )
        .await;
        for object in [&target, &other] {
            let _ = cdp::call(
                &tab.webview,
                "Runtime.releaseObject",
                json!({"objectId": object}),
            )
            .await;
        }
        match inside {
            Ok(result) if result.pointer("/result/value") == Some(&json!(true)) => Ok(()),
            _ => Err(covered()),
        }
    }

    async fn click(
        &self,
        record: &TabRecord,
        tab: &TabRuntime,
        element: &str,
        context: &AgentContext,
    ) -> Result<Value, String> {
        self.require_web_page(record, tab).await?;
        let backend = self.backend_node(record, element)?;
        let facts = self.element_facts(tab, element, backend).await?;
        if policy::sensitive_click(&facts).is_some() {
            self.confirm_sensitive(record, tab, &facts, context).await?;
            // The page may have moved on while the user decided.
            self.backend_node(record, element)?;
        }
        let point = self.element_center(tab, backend).await?;
        self.require_hit(tab, element, backend, point).await?;
        let (x, y) = point;
        let before = self.before_action(tab);
        for (kind, buttons) in [("mouseMoved", 0), ("mousePressed", 1), ("mouseReleased", 0)] {
            let mut params = json!({"type": kind, "x": x, "y": y});
            if kind != "mouseMoved" {
                params["button"] = json!("left");
                params["buttons"] = json!(buttons);
                params["clickCount"] = json!(1);
            }
            cdp::call(&tab.webview, "Input.dispatchMouseEvent", params).await?;
            if kind == "mouseMoved" {
                // Hovering can open a menu or an overlay over the target.
                self.require_hit(tab, element, backend, point).await?;
            }
        }
        self.action_result(record, tab, before, context, json!({}))
            .await
    }

    /// The field `backend` still has focus and is still not a credential
    /// field: the page cannot swap what the text goes into between the
    /// checks and the typing.
    async fn ensure_typing_target(
        &self,
        tab: &TabRuntime,
        element: &str,
        backend: i64,
    ) -> Result<(), String> {
        if self.focused_backend(tab).await.ok().flatten() != Some(backend) {
            return Err(format!(
                "ref `{element}` 에 초점이 머물지 않아 입력하지 않았습니다(페이지가 초점을 다른 곳으로 옮겼습니다). browser_snapshot 으로 다시 읽은 뒤 다시 시도하세요."
            ));
        }
        let facts = self.element_facts(tab, element, backend).await?;
        if policy::is_credential_field(&facts) {
            return Err(policy::CREDENTIAL_REFUSED.to_string());
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn type_text(
        &self,
        record: &TabRecord,
        tab: &TabRuntime,
        element: &str,
        text: &str,
        submit: bool,
        clear: bool,
        context: &AgentContext,
    ) -> Result<Value, String> {
        self.require_web_page(record, tab).await?;
        let backend = self.backend_node(record, element)?;
        let facts = self.element_facts(tab, element, backend).await?;
        if policy::is_credential_field(&facts) {
            return Err(policy::CREDENTIAL_REFUSED.to_string());
        }
        if !facts.editable {
            return Err(format!(
                "ref `{element}` 은 글자를 입력하는 칸이 아닙니다({}). 입력란의 ref 를 고르거나, 버튼·체크박스는 browser_click 을 쓰세요.",
                if facts.role.is_empty() { &facts.tag } else { &facts.role }
            ));
        }
        let _ = cdp::call(
            &tab.webview,
            "DOM.scrollIntoViewIfNeeded",
            json!({"backendNodeId": backend}),
        )
        .await;
        cdp::call(&tab.webview, "DOM.focus", json!({"backendNodeId": backend}))
            .await
            .map_err(|_| {
                format!("ref `{element}` 에 초점을 둘 수 없습니다(비활성 입력란일 수 있습니다). browser_snapshot 으로 상태를 확인하세요.")
            })?;
        self.ensure_typing_target(tab, element, backend).await?;
        if clear {
            let select_all = json!({
                "type": "rawKeyDown", "key": "a", "code": "KeyA",
                "windowsVirtualKeyCode": 65, "modifiers": 2, "commands": ["selectAll"],
            });
            cdp::call(&tab.webview, "Input.dispatchKeyEvent", select_all).await?;
            cdp::call(
                &tab.webview,
                "Input.dispatchKeyEvent",
                json!({"type": "keyUp", "key": "a", "code": "KeyA", "windowsVirtualKeyCode": 65, "modifiers": 2}),
            )
            .await?;
            self.dispatch_key(tab, policy::key_info("Backspace").expect("a known key"))
                .await?;
            self.ensure_typing_target(tab, element, backend).await?;
        }
        cdp::call(&tab.webview, "Input.insertText", json!({"text": text})).await?;
        let typed = json!({"typed": text.chars().count(), "cleared": clear});
        if !submit {
            let (url, title) = self.page_info(record, tab).await;
            let mut value = json!({
                "tabId": record.tab_id,
                "url": url,
                "title": title,
                "submitted": false,
                "untrusted": true,
            });
            merge(&mut value, typed);
            return Ok(value);
        }
        let enter = policy::key_info("Enter").expect("a known key");
        self.confirm_key(record, tab, enter, context).await?;
        let before = self.before_action(tab);
        self.dispatch_key(tab, enter).await?;
        let mut extra = typed;
        extra["submitted"] = json!(true);
        self.action_result(record, tab, before, context, extra)
            .await
    }

    async fn press(
        &self,
        record: &TabRecord,
        tab: &TabRuntime,
        key: policy::KeyInfo,
        context: &AgentContext,
    ) -> Result<Value, String> {
        self.require_web_page(record, tab).await?;
        self.confirm_key(record, tab, key, context).await?;
        let before = self.before_action(tab);
        self.dispatch_key(tab, key).await?;
        self.action_result(record, tab, before, context, json!({"key": key.code}))
            .await
    }

    async fn dispatch_key(&self, tab: &TabRuntime, key: policy::KeyInfo) -> Result<(), String> {
        for event in policy::key_events(key, 0) {
            cdp::call(&tab.webview, "Input.dispatchKeyEvent", event).await?;
        }
        Ok(())
    }

    fn before_action(&self, tab: &TabRuntime) -> BeforeAction {
        BeforeAction {
            navigations: *tab.navigations.borrow(),
            submissions: *tab.guard.submissions.borrow(),
            downloads: self.inner.downloads.last_number(),
        }
    }

    /// Wait for what an action started: a navigation (≤ 10 s to load), a
    /// download (until it starts), or nothing (1 s).
    async fn settle(
        &self,
        tab: &TabRuntime,
        before: BeforeAction,
        context: &AgentContext,
    ) -> Result<(bool, bool), String> {
        let mut submissions = tab.guard.submissions.subscribe();
        let mut navigations = tab.navigations.clone();
        let mut downloads = self.inner.downloads.subscribe();
        let quiet_until = Instant::now() + ACTION_SETTLE;
        let load_by = (Instant::now() + ACTION_NAVIGATION_WAIT).min(context.deadline);
        let mut started = false;
        loop {
            let state = *submissions.borrow_and_update();
            let navigated = *navigations.borrow_and_update() != before.navigations;
            if navigated {
                break;
            }
            // A document request that became a download commits nothing.
            downloads.borrow_and_update();
            if self.inner.downloads.last_number() > before.downloads {
                return Ok((false, false));
            }
            started |= state.documents != before.submissions.documents;
            let now = Instant::now();
            if (!started && now >= quiet_until) || now >= load_by {
                return Ok((false, false));
            }
            let until = if started { load_by } else { quiet_until };
            tokio::select! {
                _ = submissions.changed() => {}
                _ = navigations.changed() => {}
                _ = downloads.changed() => {}
                _ = tokio::time::sleep_until(until.into()) => {}
            }
        }
        loop {
            if evaluate(&tab.webview, "document.readyState").await.ok() == Some(json!("complete")) {
                return Ok((true, true));
            }
            if Instant::now() + POLL > load_by {
                return Ok((true, false));
            }
            tokio::time::sleep(POLL).await;
        }
    }

    async fn action_result(
        &self,
        record: &TabRecord,
        tab: &TabRuntime,
        before: BeforeAction,
        context: &AgentContext,
        extra: Value,
    ) -> Result<Value, String> {
        let (navigated, loaded) = self.settle(tab, before, context).await?;
        let (url, title) = self.page_info(record, tab).await;
        self.emit_state();
        let mut value = json!({
            "tabId": record.tab_id,
            "url": url,
            "title": title,
            "navigated": navigated,
            "untrusted": true,
        });
        if navigated {
            value["loaded"] = json!(loaded);
            value["note"] = json!("The page changed: refs from earlier snapshots are no longer valid. Use browser_snapshot.");
        }
        let started = self
            .inner
            .downloads
            .of_session(&record.session_id)
            .into_iter()
            .filter(|download| {
                download.tab_id == record.tab_id && download.number > before.downloads
            })
            .map(|download| download.tool_value())
            .collect::<Vec<_>>();
        if !started.is_empty() {
            value["downloads"] = json!(started);
            value["downloadNote"] = json!(
                "This action started a download. Call browser_download with this tabId alone to wait for it and get the file (audio is bound as an audioRef)."
            );
        }
        merge(&mut value, extra);
        Ok(value)
    }
}

impl BrowserHost {
    // ---------------------------------------------------------------- downloads

    /// A tab's download starts or ends. Every download (an agent call's or
    /// the user's own) lands in its own managed folder; one that cannot is
    /// cancelled.
    fn on_download(&self, label: &str, event: DownloadEvent<'_>) -> bool {
        match event {
            DownloadEvent::Requested { url, destination } => {
                let owner = self
                    .inner
                    .registry
                    .lock()
                    .all()
                    .iter()
                    .find(|tab| tab.label == label)
                    .map(|tab| (tab.tab_id.clone(), tab.session_id.clone()));
                let Some((tab_id, session_id)) = owner else {
                    return false;
                };
                let by_agent = self
                    .inner
                    .tabs
                    .lock()
                    .get(&tab_id)
                    .is_some_and(|tab| tab.guard.agent_driven());
                let suggested = destination
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                match self.inner.downloads.begin(
                    &session_id,
                    &tab_id,
                    label,
                    url.as_str(),
                    &suggested,
                    by_agent,
                ) {
                    Ok((_, path)) => {
                        *destination = path;
                        self.emit_state();
                        true
                    }
                    Err(error) => {
                        eprintln!("eud-agent: browser download refused: {error}");
                        false
                    }
                }
            }
            DownloadEvent::Finished { url, path, success } => {
                if let Some(id) = self.inner.downloads.pending_for(label, url.as_str()) {
                    let host = self.clone();
                    tauri::async_runtime::spawn_blocking(move || {
                        host.inner.downloads.finalize(&id, path, success);
                        host.emit_state();
                    });
                }
                true
            }
            _ => true,
        }
    }

    /// Cancel a download the page reports will exceed the size limit, so it
    /// is never written out in full.
    async fn follow_download_progress(
        &self,
        record: &TabRecord,
        webview: &Webview,
        guard: &Arc<TabGuard>,
    ) -> Result<(), String> {
        // WebView2 announces these before its own download event, and
        // `Browser.cancelDownload` stops the transfer.
        {
            let mut begins = cdp::subscribe(webview, "Page.downloadWillBegin").await?;
            let urls = Arc::clone(guard);
            tauri::async_runtime::spawn(async move {
                while let Some(event) = begins.recv().await {
                    if let (Some(guid), Some(url)) = (
                        event.get("guid").and_then(Value::as_str),
                        event.get("url").and_then(Value::as_str),
                    ) {
                        urls.download_urls
                            .lock()
                            .insert(guid.to_string(), url.to_string());
                    }
                }
            });
            let mut progress = cdp::subscribe(webview, "Page.downloadProgress").await?;
            let (weak, guard, label, webview) = (
                self.weak(),
                Arc::clone(guard),
                record.label.clone(),
                webview.clone(),
            );
            tauri::async_runtime::spawn(async move {
                while let Some(event) = progress.recv().await {
                    let Some(guid) = event.get("guid").and_then(Value::as_str) else {
                        continue;
                    };
                    let size = ["totalBytes", "receivedBytes"]
                        .iter()
                        .filter_map(|field| event.get(*field).and_then(Value::as_f64))
                        .fold(0.0_f64, f64::max);
                    let state = event.get("state").and_then(Value::as_str).unwrap_or("");
                    if state != "inProgress" {
                        guard.download_urls.lock().remove(guid);
                        continue;
                    }
                    if size <= downloads::MAX_DOWNLOAD_BYTES as f64 {
                        continue;
                    }
                    let Some(url) = guard.download_urls.lock().remove(guid) else {
                        continue;
                    };
                    let Some(host) = BrowserHost::from_weak(&weak) else {
                        break;
                    };
                    host.inner.downloads.mark_oversize(&label, &url);
                    if let Err(error) =
                        cdp::call(&webview, "Browser.cancelDownload", json!({"guid": guid})).await
                    {
                        eprintln!(
                            "eud-agent: an oversize browser download was not cancelled: {error}"
                        );
                    }
                }
            });
        }
        Ok(())
    }

    /// `browser_download`: start the download (click, fetch) or not, then
    /// wait for the tab's download to settle.
    async fn download(
        &self,
        record: &TabRecord,
        tab: &TabRuntime,
        target: DownloadTarget,
        timeout_ms: u64,
        context: &AgentContext,
    ) -> Result<Value, String> {
        let store = &self.inner.downloads;
        let after = match target {
            DownloadTarget::Started => None,
            _ => Some(store.last_number()),
        };
        let mut note = None;
        match &target {
            DownloadTarget::Element(element) => {
                self.click(record, tab, element, context).await?;
            }
            DownloadTarget::Url(url) => {
                note = self
                    .fetch_download(record, tab, url, after.unwrap_or(0), context)
                    .await?;
            }
            DownloadTarget::Started => {}
        }
        let deadline = (Instant::now() + Duration::from_millis(timeout_ms))
            .min(context.deadline - Duration::from_secs(5));
        let (session, tab_id) = (record.session_id.clone(), record.tab_id.clone());
        let settled = store
            .wait_for(deadline, |store| {
                store
                    .awaited(&session, &tab_id, after)
                    .filter(|download| download.state != DownloadState::InProgress)
            })
            .await;
        let Some(download) = settled else {
            return Err(match store.awaited(&session, &tab_id, after) {
                Some(running) => format!(
                    "다운로드 {} ({}) 를 아직 받는 중입니다. browser_download 를 tabId 만 주고 다시 호출해 이어서 기다리세요.",
                    running.download_id, running.display_name
                ),
                None => match target {
                    DownloadTarget::Started => "이 탭에서 기다릴 다운로드가 없습니다. 다운로드 버튼의 ref 를 browser_snapshot 으로 찾아 browser_download {tabId, ref} 로 받으세요.".to_string(),
                    _ => "다운로드가 시작되지 않았습니다. browser_snapshot 으로 페이지를 확인하세요(생성 중이면 browser_wait 뒤 다운로드 버튼의 ref 로 다시 시도).".to_string(),
                },
            });
        };
        store.mark_reported(&download.download_id);
        self.emit_state();
        if download.state == DownloadState::Failed {
            return Err(format!(
                "다운로드 {} ({}) 실패: {}",
                download.download_id,
                download.display_name,
                download.error.as_deref().unwrap_or("알 수 없는 오류")
            ));
        }
        let mut value = download.tool_value();
        value["untrusted"] = json!(true);
        if let Some(note) = note {
            value["note"] = json!(note);
        }
        Ok(value)
    }

    /// Download `url` in `tab`: fetched by the page (its cookies apply) and
    /// read out of it into a managed download, or, when the page cannot
    /// fetch it (cross-origin), by navigating the tab to it. A navigation
    /// that shows the file instead of downloading it is fetched again from
    /// there; the returned note says the tab moved.
    async fn fetch_download(
        &self,
        record: &TabRecord,
        tab: &TabRuntime,
        url: &tauri::Url,
        after: u64,
        context: &AgentContext,
    ) -> Result<Option<String>, String> {
        let deadline = context.deadline - Duration::from_secs(10);
        // The page's fetch passes no request check of its own.
        if let Some(host) = resolves_to_local(url.as_str()).await {
            return Err(super::internal_refusal(&host));
        }
        if self.page_fetch(record, tab, url.as_str(), deadline).await? {
            return Ok(None);
        }
        let before = *tab.navigations.borrow();
        let result = cdp::call(&tab.webview, "Page.navigate", json!({"url": url.as_str()})).await?;
        if let Some(error) = result.get("errorText").and_then(Value::as_str) {
            // A response that becomes a download aborts the navigation.
            if error != "net::ERR_ABORTED" {
                return Err(format!(
                    "파일 주소를 열 수 없습니다: {error}. 주소를 확인하거나 페이지의 다운로드 버튼 ref 로 받으세요."
                ));
            }
        }
        let started = |host: &BrowserHost| {
            host.inner
                .downloads
                .awaited(&record.session_id, &record.tab_id, Some(after))
                .is_some()
        };
        let settle_by = Instant::now() + NAVIGATION_WAIT;
        let mut navigations = tab.navigations.clone();
        loop {
            if started(self) {
                return Ok(None);
            }
            if *navigations.borrow_and_update() != before {
                break;
            }
            if Instant::now() >= settle_by {
                return Ok(None);
            }
            let _ = tokio::time::timeout(POLL, navigations.changed()).await;
        }
        // The tab now shows the file: let it load, then read it from there.
        let loaded_by = Instant::now() + NAVIGATION_WAIT;
        while evaluate(&tab.webview, "document.readyState").await.ok() != Some(json!("complete"))
            && Instant::now() < loaded_by
        {
            tokio::time::sleep(POLL).await;
        }
        if started(self) {
            return Ok(None);
        }
        // Read it from where the navigation (and its redirects) ended.
        tab.guard.require_public_load()?;
        let (shown, _) = self.page_info(record, tab).await;
        let shown = if shown.starts_with("http") {
            shown
        } else {
            url.to_string()
        };
        if !self.page_fetch(record, tab, &shown, deadline).await? {
            return Err("파일을 받을 수 없습니다(페이지가 그 주소를 읽지 못했습니다). 페이지의 다운로드 버튼 ref 로 browser_download 를 호출하세요.".to_string());
        }
        Ok(Some(
            "The tab navigated to the file's address to download it; use browser_navigate with action back to return to the previous page.".to_string(),
        ))
    }

    /// Have the page fetch `url` with its own cookies, then read the bytes
    /// out of the page in chunks into a managed download of the tab (the
    /// webview's own download path would hold a script-started download for
    /// a permission nobody can grant). `false` when the page could not read
    /// it (network or cross-origin refusal).
    async fn page_fetch(
        &self,
        record: &TabRecord,
        tab: &TabRuntime,
        url: &str,
        deadline: Instant,
    ) -> Result<bool, String> {
        let left = || {
            deadline
                .saturating_duration_since(Instant::now())
                .max(Duration::from_secs(1))
        };
        let key = format!("__eudDownload_{}", uuid::Uuid::new_v4().simple());
        // The name was checked before the fetch, but it may resolve again
        // (DNS rebinding): the address the response came from decides.
        let watch = FetchWatch::start(tab, url);
        let fetched = page_call(
            tab,
            PAGE_FETCH_JS,
            &[json!(url), json!(key), json!(downloads::MAX_DOWNLOAD_BYTES)],
            left(),
        )
        .await?;
        if fetched.get("tooLarge").is_some() {
            return Err(downloads::DOWNLOAD_TOO_LARGE.to_string());
        }
        if let Some(status) = fetched.get("status").and_then(Value::as_u64) {
            return Err(format!(
                "파일 주소가 HTTP {status} 로 응답했습니다. 주소가 맞는지, 로그인이 필요한지 확인하세요(로그인은 사용자에게 부탁하세요)."
            ));
        }
        if fetched.get("fetched") != Some(&json!(true)) {
            return Ok(false);
        }
        if let Err(refusal) = watch.public_response(url).await {
            let _ = page_call(tab, PAGE_FORGET_JS, &[json!(key)], left()).await;
            return Err(refusal);
        }
        let size = match downloads::page_blob_size(fetched.get("size").and_then(Value::as_u64)) {
            Ok(size) => size,
            Err(error) => {
                let _ = page_call(tab, PAGE_FORGET_JS, &[json!(key)], left()).await;
                return Err(error);
            }
        };
        let name = fetched.get("name").and_then(Value::as_str).unwrap_or("");
        let store = self.inner.downloads.clone();
        let (id, path) = store.begin(
            &record.session_id,
            &record.tab_id,
            &record.label,
            url,
            name,
            true,
        )?;
        self.emit_state();
        let copied = self.read_page_blob(tab, &key, size, &path, deadline).await;
        let _ = page_call(tab, PAGE_FORGET_JS, &[json!(key)], left()).await;
        let success = match copied {
            Ok(()) => true,
            Err(error) => {
                eprintln!("eud-agent: a fetched browser download was not read: {error}");
                false
            }
        };
        // A failed read removes the partial file with its download folder.
        let host = self.clone();
        tauri::async_runtime::spawn_blocking(move || {
            store.finalize(&id, Some(path), success);
            host.emit_state();
        })
        .await
        .map_err(|error| format!("다운로드를 마무리하지 못했습니다({error})"))?;
        Ok(true)
    }

    /// Append the page blob `window[key]` (`size` bytes, already checked
    /// against [`downloads::MAX_DOWNLOAD_BYTES`]) to `path`, one base64 chunk
    /// per call. A chunk of any other length than asked ends the read, so
    /// the page can never make it write more than `size`.
    async fn read_page_blob(
        &self,
        tab: &TabRuntime,
        key: &str,
        size: u64,
        path: &std::path::Path,
        deadline: Instant,
    ) -> Result<(), String> {
        use std::io::Write as _;
        let file = {
            let path = path.to_path_buf();
            tokio::task::spawn_blocking(move || std::fs::File::create(path))
                .await
                .map_err(|error| error.to_string())?
                .map_err(|error| error.to_string())?
        };
        let mut file = Some(file);
        let mut offset = 0_u64;
        while offset < size {
            let end = (offset + PAGE_READ_CHUNK).min(size);
            let chunk = page_call(
                tab,
                PAGE_READ_JS,
                &[json!(key), json!(offset), json!(end)],
                deadline
                    .saturating_duration_since(Instant::now())
                    .max(Duration::from_secs(1)),
            )
            .await?;
            let text = chunk.as_str().unwrap_or_default();
            let expected = end - offset;
            if text.len() as u64 > expected.div_ceil(3) * 4 {
                return Err("the page returned a chunk longer than asked".to_string());
            }
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(text)
                .map_err(|error| error.to_string())?;
            if bytes.len() as u64 != expected {
                return Err("the page returned a chunk of the wrong length".to_string());
            }
            // Written off the async workers: a chunk is up to 2 MiB.
            let mut writing = file.take().expect("the file is back after every chunk");
            file = Some(
                tokio::task::spawn_blocking(move || writing.write_all(&bytes).map(|()| writing))
                    .await
                    .map_err(|error| error.to_string())?
                    .map_err(|error| error.to_string())?,
            );
            offset = end;
        }
        let mut file = file.expect("the file is back after every chunk");
        tokio::task::spawn_blocking(move || file.flush())
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())
    }

    // ---------------------------------------------------------------- chrome

    pub fn set_chrome_expanded(&self, expanded: bool) {
        self.inner.chrome_expanded.store(expanded, Ordering::SeqCst);
        if let Some(size) = self
            .inner
            .app
            .get_window(BROWSER_WINDOW_LABEL)
            .and_then(|window| window.inner_size().ok())
        {
            self.layout(size);
        }
        if expanded {
            self.raise_chrome();
        } else {
            self.raise_active();
        }
    }

    fn raise_chrome(&self) {
        if let Some(chrome) = self.inner.app.get_webview(BROWSER_WINDOW_LABEL) {
            if let Err(error) = cdp::bring_to_front(&chrome) {
                eprintln!("eud-agent: browser chrome could not be raised: {error}");
            }
        }
    }
}

/// The host of `url` when its name resolves to an address of this computer
/// or the local network, looked up within [`LOOKUP_WAIT`]. An IP literal is
/// left to [`super::agent_host_is_local`]; a lookup that fails or takes
/// too long answers `None` (the response address is still checked).
async fn resolves_to_local(url: &str) -> Option<String> {
    let parsed = tauri::Url::parse(url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let host = parsed.domain()?.to_string();
    let port = parsed.port_or_known_default().unwrap_or(80);
    let local = tokio::time::timeout(LOOKUP_WAIT, tokio::net::lookup_host((host.clone(), port)))
        .await
        .ok()?
        .ok()?
        .map(|address| address.ip())
        .any(super::is_local_address);
    local.then_some(host)
}

/// How long the response address of a page fetch may take to arrive after
/// the fetch itself returned.
const FETCH_ADDRESS_WAIT: Duration = Duration::from_secs(3);

/// The key a fetched URL is watched under: as the response reports it.
fn fetch_key(url: &str) -> String {
    tauri::Url::parse(url).map_or_else(|_| url.to_string(), |url| url.to_string())
}

/// Record the address every script (`fetch`) response of the tab came from,
/// for the URLs a page fetch watches ([`FetchWatch`]). Responses are parsed
/// only while one is watched.
async fn follow_fetch_responses(tab: &TabRuntime) -> Result<(), String> {
    let watching = Arc::clone(&tab.guard);
    let mut responses =
        cdp::subscribe_sessions_filtered(&tab.webview, "Network.responseReceived", move |text| {
            watching.fetch_watching.load(Ordering::SeqCst) > 0
                && (text.contains("\"type\":\"Fetch\"") || text.contains("\"type\": \"Fetch\""))
        })
        .await?;
    let guard = Arc::clone(&tab.guard);
    tauri::async_runtime::spawn(async move {
        while let Some((_, event)) = responses.recv().await {
            let Some(response) = event.get("response") else {
                continue;
            };
            let url = fetch_key(response.get("url").and_then(Value::as_str).unwrap_or(""));
            let address = response
                .get("remoteIPAddress")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if let Some(seen) = guard.fetches.lock().get_mut(&url) {
                // A local answer is never replaced by a later public one.
                if !seen.as_deref().is_some_and(super::remote_address_is_local) {
                    *seen = Some(address);
                }
            }
        }
    });
    Ok(())
}

/// One page fetch whose response address is watched, until dropped.
struct FetchWatch {
    guard: Arc<TabGuard>,
    key: String,
}

impl FetchWatch {
    fn start(tab: &TabRuntime, url: &str) -> Self {
        let key = fetch_key(url);
        tab.guard.fetches.lock().insert(key.clone(), None);
        tab.guard.fetch_watching.fetch_add(1, Ordering::SeqCst);
        Self {
            guard: Arc::clone(&tab.guard),
            key,
        }
    }

    /// The fetch of `url` returned: its response must have come from a
    /// public address. One from this computer or the local network is
    /// refused, and so is one whose address never arrived (fail closed).
    async fn public_response(&self, url: &str) -> Result<(), String> {
        let until = Instant::now() + FETCH_ADDRESS_WAIT;
        loop {
            let seen = self.guard.fetches.lock().get(&self.key).cloned().flatten();
            match seen {
                Some(address) if super::remote_address_is_local(&address) => {
                    eprintln!(
                        "eud-agent: browser refused a page fetch answered from a local address ({address}): {url}"
                    );
                    let host = tauri::Url::parse(url)
                        .ok()
                        .and_then(|url| url.host_str().map(str::to_string))
                        .unwrap_or(address);
                    return Err(super::internal_refusal(&host));
                }
                Some(_) => return Ok(()),
                None if Instant::now() >= until => {
                    return Err("파일이 온 주소를 확인하지 못해 받지 않았습니다. 페이지의 다운로드 버튼 ref 로 browser_download 를 다시 호출하세요.".to_string());
                }
                None => tokio::time::sleep(Duration::from_millis(25)).await,
            }
        }
    }
}

impl Drop for FetchWatch {
    fn drop(&mut self) {
        self.guard.fetches.lock().remove(&self.key);
        self.guard.fetch_watching.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Attach every child target (out-of-process iframes, workers) as a
/// flattened session, paused until [`guard_frame_target`] has set it up, so
/// none of its requests slips past the request policy.
fn auto_attach() -> Value {
    json!({"autoAttach": true, "waitForDebuggerOnStart": true, "flatten": true})
}

/// Give an attached child target the tab's request policy before it runs:
/// an iframe pauses its own requests (`Fetch.enable`, decided by
/// `decide_paused` with this session, with the patterns the tab uses now),
/// reports its document responses, bypasses service workers and attaches
/// its own children; a dedicated, shared or service worker pauses its
/// requests (and attaches nested workers where it can). A frame or worker
/// whose guard cannot be set up is never resumed: it stays paused before
/// its first instruction (fail closed), so none of its requests goes out
/// unchecked. Other targets (worklets) make no requests of their own and
/// resume at once.
async fn guard_frame_target(tab: &TabRuntime, session: &str, kind: &str) {
    let webview = &tab.webview;
    let frame = matches!(kind, "iframe" | "page");
    let worker = matches!(kind, "worker" | "shared_worker" | "service_worker");
    if frame || worker {
        // Registered first, then the patterns read under the arming lock:
        // a concurrent switch either sees this session or is seen by it.
        tab.guard.sessions.lock().insert(session.to_string());
        let _arming = tab.guard.arming.lock().await;
        let scripts = tab.guard.scripts_armed.load(Ordering::SeqCst);
        let mut steps = Vec::new();
        if frame {
            steps.push(("Network.enable", network_enable(), true));
            steps.push((
                "Network.setBypassServiceWorker",
                json!({"bypass": true}),
                true,
            ));
        }
        steps.push(("Fetch.enable", fetch_patterns(scripts), true));
        steps.push(("Target.setAutoAttach", auto_attach(), frame));
        for (method, params, required) in steps {
            if let Err(error) = cdp::call_in(webview, Some(session), method, params).await {
                // A dedicated worker has no Fetch domain of its own: its
                // requests pause in the page (or frame) that owns it
                // (verified live), so it runs guarded by that.
                if kind == "worker" && method == "Fetch.enable" && error.contains("-32601") {
                    tab.guard.sessions.lock().remove(session);
                    continue;
                }
                if required {
                    eprintln!(
                        "eud-agent: browser {kind} target left paused, its guard failed ({method}): {error}"
                    );
                    tab.guard.sessions.lock().remove(session);
                    return;
                }
                eprintln!("eud-agent: browser {kind} target: {method} failed: {error}");
            }
        }
    }
    if let Err(error) = cdp::call_in(
        webview,
        Some(session),
        "Runtime.runIfWaitingForDebugger",
        json!({}),
    )
    .await
    {
        eprintln!("eud-agent: browser child target did not resume: {error}");
    }
}

/// Bytes read out of the page per call of [`PAGE_READ_JS`].
const PAGE_READ_CHUNK: u64 = 2 * 1024 * 1024;

/// Fetch a file with the page's own cookies and keep it as `window[key]`
/// for [`PAGE_READ_JS`]; `cap` is the byte limit. Its values arrive as
/// call arguments, never as source text.
const PAGE_FETCH_JS: &str = r#"async function (url, key, cap) {
  let response;
  // A redirect is not followed here, where no hop is checked: the tab's
  // navigation fallback follows it, hop by hop under the request policy.
  try { response = await fetch(url, { credentials: "include", redirect: "manual" }); }
  catch (error) { return { fetched: false, error: String(error) }; }
  if (response.type === "opaqueredirect") return { fetched: false, redirected: true };
  if (!response.ok) return { fetched: false, status: response.status };
  const length = Number(response.headers.get("content-length") || 0);
  if (length > cap) return { tooLarge: length };
  const blob = await response.blob();
  if (blob.size > cap) return { tooLarge: blob.size };
  const disposition = response.headers.get("content-disposition") || "";
  const star = /filename\*\s*=\s*[^']*''([^;]+)/i.exec(disposition);
  const plain = /filename\s*=\s*"?([^";]+)"?/i.exec(disposition);
  let name = "";
  try { name = star ? decodeURIComponent(star[1]) : (plain ? plain[1] : ""); } catch (_) { name = plain ? plain[1] : ""; }
  if (!name) {
    try { name = decodeURIComponent(new URL(response.url || url).pathname.split("/").pop() || ""); } catch (_) { name = ""; }
  }
  window[key] = blob;
  return { fetched: true, name, size: blob.size };
}"#;

/// One base64 chunk `[start, end)` of the blob `window[key]`.
const PAGE_READ_JS: &str = r#"async function (key, start, end) {
  const bytes = new Uint8Array(await window[key].slice(start, end).arrayBuffer());
  let text = "";
  for (let i = 0; i < bytes.length; i += 0x8000) {
    text += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
  }
  return btoa(text);
}"#;

/// Drop the blob `window[key]`.
const PAGE_FORGET_JS: &str = r#"function (key) { delete window[key]; return true; }"#;

/// Call the async `function` in the page's global scope with `arguments`
/// (passed as protocol values, never spliced into source), awaited within
/// `wait`; its value, or the page's exception.
async fn page_call(
    tab: &TabRuntime,
    function: &str,
    arguments: &[Value],
    wait: Duration,
) -> Result<Value, String> {
    let global = cdp::call(
        &tab.webview,
        "Runtime.evaluate",
        json!({"expression": "globalThis", "returnByValue": false}),
    )
    .await?;
    let object_id = global
        .pointer("/result/objectId")
        .cloned()
        .ok_or_else(|| "페이지에 접근할 수 없습니다. 다시 시도하세요.".to_string())?;
    let result = cdp::call_with(
        &tab.webview,
        "Runtime.callFunctionOn",
        json!({
            "objectId": object_id,
            "functionDeclaration": function,
            "arguments": arguments.iter().map(|value| json!({"value": value})).collect::<Vec<_>>(),
            "awaitPromise": true,
            "returnByValue": true,
        }),
        wait,
    )
    .await;
    let _ = cdp::call(
        &tab.webview,
        "Runtime.releaseObject",
        json!({"objectId": object_id}),
    )
    .await;
    let result = result?;
    if let Some(exception) = result.get("exceptionDetails") {
        return Err(format!("페이지 스크립트 오류: {exception}"));
    }
    Ok(result
        .pointer("/result/value")
        .cloned()
        .unwrap_or(Value::Null))
}

// Gathers what the request policy reads about one element: its tag and
/// type, autocomplete, its labels, and the text of the control it is part of.
const ELEMENT_FACTS_JS: &str = r#"function () {
  const el = this.nodeType === 1 ? this : this.parentElement;
  if (!el) return null;
  const tag = el.tagName.toLowerCase();
  const attr = (name) => el.getAttribute(name) || "";
  const type = (attr("type") || (tag === "input" ? "text" : "")).toLowerCase();
  const control = el.closest("button,a,summary,label,[role=button],[role=link],[role=menuitem],[role=tab],[role=option]") || el;
  const text = String(control.innerText || control.textContent || "").trim().slice(0, 400);
  const labels = [attr("aria-label"), attr("title"), attr("placeholder")];
  if (tag === "input" && ["submit", "button", "reset"].includes(type)) labels.push(String(el.value || ""));
  if (control !== el) labels.push(control.getAttribute("aria-label") || "", control.getAttribute("title") || "");
  if (el.labels) for (const label of el.labels) labels.push(String(label.innerText || "").trim().slice(0, 200));
  // A click on a label clicks the control it labels: that control's names count too.
  const label = el.closest("label");
  const labelled = label ? label.control : null;
  if (labelled && labelled !== el) {
    for (const name of ["aria-label", "title"]) labels.push(labelled.getAttribute(name) || "");
    const kind = (labelled.getAttribute("type") || "").toLowerCase();
    if (labelled.tagName === "INPUT" && ["submit", "button", "reset", "image"].includes(kind)) labels.push(String(labelled.value || labelled.getAttribute("alt") || ""));
    if (labelled.tagName === "BUTTON") labels.push(String(labelled.innerText || labelled.textContent || "").trim().slice(0, 200));
  }
  const textTypes = ["text", "search", "email", "url", "tel", "number", "password"];
  const editable = !!(el.isContentEditable || tag === "textarea" || (tag === "input" && textTypes.includes(type)));
  return { tag, inputType: type, autocomplete: attr("autocomplete"), labels: labels.filter(Boolean), text, editable };
}"#;

/// The element that has focus, followed into open shadow roots and
/// same-origin frames; `null` when only the page itself has focus.
const FOCUSED_JS: &str = r#"(() => {
  let el = document.activeElement;
  for (let depth = 0; el && depth < 32; depth++) {
    if (el.shadowRoot && el.shadowRoot.activeElement) { el = el.shadowRoot.activeElement; continue; }
    if (el.tagName === "IFRAME" || el.tagName === "FRAME") {
      let inner = null;
      try { inner = el.contentDocument && el.contentDocument.activeElement; } catch (_) { inner = null; }
      if (inner && inner !== el.contentDocument.body && inner !== el.contentDocument.documentElement) { el = inner; continue; }
    }
    break;
  }
  if (!el || el === document.body || el === document.documentElement) return null;
  return el;
})()"#;

/// The default button of the form `this` (a text field) belongs to: the
/// first submit button among the form's controls, which implicit submission
/// (Enter) clicks; `null` without one.
const FORM_DEFAULT_JS: &str = r#"function () {
  const form = this.form;
  if (!form) return null;
  for (const control of form.elements) {
    const type = (control.getAttribute("type") || "").toLowerCase();
    if (control.tagName === "BUTTON" && (type === "" || type === "submit")) return control;
    if (control.tagName === "INPUT" && (type === "submit" || type === "image")) return control;
  }
  return null;
}"#;

/// Whether the hit-tested node `other` is `this` element, inside it (also
/// through its shadow tree) or inside one of its labels.
const HIT_INSIDE_JS: &str = r#"function (other) {
  const target = this.nodeType === 1 ? this : this.parentElement;
  if (!target || !other) return false;
  const within = (node, root) => {
    for (let n = node, depth = 0; n && depth < 4096; depth++) {
      if (n === root) return true;
      n = n.parentNode || n.host || null;
    }
    return false;
  };
  if (within(other, target)) return true;
  if (target.labels) for (const label of target.labels) if (within(other, label)) return true;
  return false;
}"#;

fn stale_ref(element: &str) -> String {
    format!(
        "ref `{element}` 는 이 탭의 최신 스냅샷에 없습니다(페이지가 바뀌었거나 아직 스냅샷을 읽지 않았습니다). browser_snapshot 으로 다시 읽은 뒤 새 ref 를 쓰세요."
    )
}

/// The label the user is shown for a clicked element.
fn element_label(facts: &ElementFacts) -> String {
    [facts.name.as_str(), facts.text.as_str()]
        .into_iter()
        .chain(facts.labels.iter().map(String::as_str))
        .map(str::trim)
        .find(|label| !label.is_empty())
        .unwrap_or("이름 없는 요소")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn shorten(text: &str, chars: usize) -> String {
    if text.chars().count() <= chars {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(chars).collect::<String>())
    }
}

fn merge(value: &mut Value, extra: Value) {
    if let (Some(value), Value::Object(extra)) = (value.as_object_mut(), extra) {
        value.extend(extra);
    }
}

/// The centre of a box-model quad `[x1, y1, ..., x4, y4]`, when it has an area.
fn quad_center(quad: &Value) -> Option<(f64, f64)> {
    let points = quad
        .as_array()?
        .iter()
        .map(Value::as_f64)
        .collect::<Option<Vec<_>>>()?;
    if points.len() != 8 {
        return None;
    }
    let xs = points.iter().step_by(2).copied().collect::<Vec<_>>();
    let ys = points
        .iter()
        .skip(1)
        .step_by(2)
        .copied()
        .collect::<Vec<_>>();
    let span = |values: &[f64]| {
        values.iter().copied().fold(f64::MIN, f64::max)
            - values.iter().copied().fold(f64::MAX, f64::min)
    };
    if span(&xs) < 1.0 || span(&ys) < 1.0 {
        return None;
    }
    Some((xs.iter().sum::<f64>() / 4.0, ys.iter().sum::<f64>() / 4.0))
}

/// `Runtime.evaluate` returning the value, or the page's exception.
async fn evaluate(webview: &Webview, expression: &str) -> Result<Value, String> {
    let result = cdp::call(
        webview,
        "Runtime.evaluate",
        json!({"expression": expression, "returnByValue": true}),
    )
    .await?;
    if let Some(exception) = result.get("exceptionDetails") {
        return Err(format!("페이지 스크립트 오류: {exception}"));
    }
    Ok(result
        .get("result")
        .and_then(|result| result.get("value"))
        .cloned()
        .unwrap_or(Value::Null))
}

/// Width and height from a PNG's IHDR.
fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 24 || &bytes[..8] != SIGNATURE || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((width, height))
}

#[cfg(test)]
mod tests {
    #[test]
    fn png_size_reads_the_header_and_refuses_other_bytes() {
        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png, 3, 2);
            encoder.set_color(png::ColorType::Rgb);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[0; 18]).unwrap();
        }
        assert_eq!(super::png_size(&png), Some((3, 2)));
        assert_eq!(super::png_size(b"GIF89a................"), None);
        assert_eq!(super::png_size(&png[..10]), None);
    }

    #[test]
    fn a_user_navigation_claims_only_its_own_document_request() {
        use super::UserNavigation;
        use std::time::{Duration, Instant};
        let now = Instant::now();
        let until = now + Duration::from_secs(5);

        // A pending navigation (reload, back/forward) takes the first GET
        // document request and then only that request's hops.
        let mut navigation = Some(UserNavigation::Pending(until));
        assert!(!UserNavigation::claims(
            &mut navigation,
            "POST",
            Some("n1"),
            now
        ));
        assert!(UserNavigation::claims(
            &mut navigation,
            "GET",
            Some("n1"),
            now
        ));
        assert!(UserNavigation::claims(
            &mut navigation,
            "GET",
            Some("n1"),
            now
        ));
        assert!(!UserNavigation::claims(
            &mut navigation,
            "GET",
            Some("n2"),
            now
        ));

        // An address navigation is its loader's request only.
        let mut navigation = Some(UserNavigation::Request("loader".to_string()));
        assert!(!UserNavigation::claims(
            &mut navigation,
            "GET",
            Some("other"),
            now
        ));
        assert!(UserNavigation::claims(
            &mut navigation,
            "GET",
            Some("loader"),
            now
        ));

        // The commit of that load ends it; another load's commit does not.
        UserNavigation::committed(&mut navigation, Some("other"));
        assert!(navigation.is_some());
        UserNavigation::committed(&mut navigation, Some("loader"));
        assert_eq!(navigation, None);
        assert!(!UserNavigation::claims(
            &mut navigation,
            "GET",
            Some("loader"),
            now
        ));

        // A pending navigation that committed (or moved within the document)
        // without its request being seen claims nothing afterwards.
        let mut navigation = Some(UserNavigation::Pending(until));
        UserNavigation::committed(&mut navigation, None);
        assert!(!UserNavigation::claims(
            &mut navigation,
            "GET",
            Some("n3"),
            now
        ));
        // And an unmatched one lapses.
        let mut navigation = Some(UserNavigation::Pending(now));
        assert!(!UserNavigation::claims(
            &mut navigation,
            "GET",
            Some("n4"),
            now
        ));
    }

    #[test]
    fn a_user_navigation_is_never_claimed_while_the_agent_drives_the_tab() {
        use super::{policy::Driver, AgentCalls, UserNavigation};
        use std::time::{Duration, Instant};
        let now = Instant::now();
        let until = now + Duration::from_secs(5);

        // Given: the user's reload is pending, and an agent click on the
        // same tab just returned (its grace runs).
        let mut calls = AgentCalls::default();
        calls.enter(1, true, context("s1"));
        calls.leave(1, now);
        let mut navigation = Some(UserNavigation::Pending(until));

        // When: a page script navigates within the grace.
        let (driver, _) = calls.acting(now + Duration::from_millis(500));
        assert_eq!(driver, Driver::Interacting);
        // Then: that load is the agent's, and the reload still waits.
        assert!(!UserNavigation::claims_while(
            &mut navigation,
            driver,
            "GET",
            Some("script"),
            now
        ));
        assert_eq!(navigation, Some(UserNavigation::Pending(until)));
        // A reading call running on the tab counts too.
        calls.enter(2, false, context("s1"));
        let (driver, _) = calls.acting(now + Duration::from_secs(4));
        assert_eq!(driver, Driver::Reading);
        assert!(!UserNavigation::claims_while(
            &mut navigation,
            driver,
            "GET",
            Some("read"),
            now
        ));
        calls.leave(2, now + Duration::from_secs(4));

        // Once the grace is over, the user's own request is claimed.
        let (driver, _) = calls.acting(now + Duration::from_secs(4));
        assert_eq!(driver, Driver::User);
        assert!(UserNavigation::claims_while(
            &mut navigation,
            driver,
            "GET",
            Some("reload"),
            now
        ));
    }

    fn context(session: &str) -> super::AgentContext {
        super::AgentContext {
            session_id: session.into(),
            confirm: None,
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(100),
        }
    }

    #[test]
    fn a_reading_call_never_hides_the_interaction_running_beside_it() {
        use super::policy::Driver;
        let now = std::time::Instant::now();
        let mut calls = super::AgentCalls::default();
        assert_eq!(calls.acting(now).0, Driver::User);
        calls.enter(1, true, context("clicker"));
        // A snapshot starts and ends on the same tab while the click runs.
        calls.enter(2, false, context("reader"));
        let (driver, acting) = calls.acting(now);
        assert_eq!(driver, Driver::Interacting);
        assert_eq!(acting.unwrap().session_id, "clicker");
        calls.leave(2, now);
        assert_eq!(calls.acting(now).0, Driver::Interacting);
        // A reading call that runs inside the interaction's grace never
        // ends that grace early: a delayed submit still asks the clicker.
        calls.enter(3, false, context("reader"));
        calls.leave(1, now);
        let (driver, acting) = calls.acting(now);
        assert_eq!(driver, Driver::Interacting);
        assert_eq!(acting.unwrap().session_id, "clicker");
        // Once the grace is over only the reading call is left: its
        // requests are the agent's, but the page's own script traffic is
        // not held.
        let (driver, acting) = calls.acting(now + super::AGENT_GRACE);
        assert_eq!(driver, Driver::Reading);
        assert_eq!(acting.unwrap().session_id, "reader");
        calls.leave(3, now);
    }

    #[test]
    fn an_interaction_keeps_the_tab_agent_driven_for_its_grace() {
        use super::policy::Driver;
        let now = std::time::Instant::now();
        let mut calls = super::AgentCalls::default();
        calls.enter(1, true, context("clicker"));
        calls.leave(1, now);
        let (driver, acting) = calls.acting(now + super::AGENT_GRACE / 2);
        assert_eq!(driver, Driver::Interacting);
        // A confirmation raised after the call returned still has time to ask.
        let acting = acting.unwrap();
        assert_eq!(acting.session_id, "clicker");
        assert!(acting.deadline >= now + super::AFTER_CONFIRM + super::GRACE_CONFIRM_WAIT);
        assert_eq!(calls.acting(now + super::AGENT_GRACE).0, Driver::User);
        // A reading call leaves no grace behind.
        let mut calls = super::AgentCalls::default();
        calls.enter(2, false, context("reader"));
        calls.leave(2, now);
        assert_eq!(calls.acting(now).0, Driver::User);
    }

    #[test]
    fn quad_centers_need_an_area() {
        use serde_json::json;
        assert_eq!(
            super::quad_center(&json!([10, 20, 30, 20, 30, 60, 10, 60])),
            Some((20.0, 40.0))
        );
        assert_eq!(super::quad_center(&json!([5, 5, 5, 5, 5, 5, 5, 5])), None);
        assert_eq!(super::quad_center(&json!([1, 2, 3])), None);
        assert_eq!(super::quad_center(&json!("x")), None);
    }
}
