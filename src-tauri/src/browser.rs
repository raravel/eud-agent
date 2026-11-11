//! The built-in browser: one app-owned "브라우저" window shared by every
//! project, with one tab per page an EPS session (or a research worker) opened.
//!
//! The window is process-wide. A chrome webview (`browser`, `browser.html`)
//! draws the tab strip; every tab is its own child webview
//! (`browser-tab-<n>`) on a separate WebView2 user data folder
//! (`browser/profile`), so sign-ins are shared by every tab and never by the
//! app windows. Tab webviews are in no capability: remote pages reach no app
//! command (Tauri refuses every IPC call from a remote origin without one).
//!
//! The agent drives a tab through the DevTools Protocol called in-process
//! (`cdp.rs`, Windows only): no debugging port is ever opened. A session sees
//! and uses only its own tabs ([`registry::TabRegistry`]).

use serde::Serialize;
use serde_json::Value;

pub mod downloads;
pub mod policy;
pub mod registry;
pub mod snapshot;

#[cfg(windows)]
mod cdp;
#[cfg(windows)]
mod host;

/// Label of the browser window, and of its chrome webview.
pub const BROWSER_WINDOW_LABEL: &str = "browser";
/// Prefix of every tab webview's label.
pub const TAB_LABEL_PREFIX: &str = "browser-tab-";
/// The chrome page's state event, emitted to the browser window only.
pub const BROWSER_STATE_EVENT: &str = "browser-state-changed";
/// Every navigation tool waits at most this long for the page to load, then
/// returns the page as it stands.
pub const NAVIGATION_WAIT: std::time::Duration = std::time::Duration::from_secs(30);
/// Longest `browser_wait`.
pub const MAX_WAIT_MS: u64 = 30_000;
/// Largest screenshot PNG handed to the model.
pub const MAX_SCREENSHOT_BYTES: usize = 5 * 1024 * 1024;
/// Longest URL a tool accepts.
pub const MAX_URL_CHARS: usize = 4096;
/// Longest `browser_wait` text.
pub const MAX_WAIT_TEXT_CHARS: usize = 500;
/// Longest `browser_type` text.
pub const MAX_TYPE_TEXT_CHARS: usize = 10_000;
/// Longest wait for the user to allow or refuse one browser action; the
/// tool call itself ends within the 240 s every tool call keeps to.
pub const CONFIRM_WAIT: std::time::Duration = std::time::Duration::from_secs(200);

pub const UNSUPPORTED_PLATFORM: &str =
    "이 플랫폼에서는 브라우저를 쓸 수 없습니다. Windows에서 실행해 주세요.";

/// One admitted browser tool call, handed by the tool runtime to the
/// [`BrowserService`] the session manager injected.
#[derive(Debug, Clone)]
pub struct BrowserCall {
    pub session_id: String,
    pub project_id: String,
    pub tool: String,
    pub args: Value,
    /// Asks the session's user to allow one sensitive click. `None`: nobody
    /// can be asked, so no sensitive click is allowed.
    pub confirm: Option<BrowserConfirm>,
}

/// One question to the user: allow this browser action?
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserConfirmation {
    pub question: String,
    /// Longest wait for the answer; no answer is a refusal.
    pub wait: std::time::Duration,
}

/// How one confirmation the app wanted from the user ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmAnswer {
    /// The user chose 허용.
    Allowed,
    /// The user chose 거부 (or another answer), or let the shown card expire.
    Denied,
    /// The question never reached the user: nobody can be asked (a worker),
    /// another question was waiting the whole time, or the call had no time
    /// left. Not the user's refusal.
    Unasked,
}

pub type BrowserConfirmFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<ConfirmAnswer, String>> + Send>>;

/// The tool runtime's confirmation channel for one browser call: it shows
/// the question as an ask card (one at a time per session, the next waiting
/// for the shown one to settle) and resolves [`ConfirmAnswer::Allowed`] only
/// for 허용.
#[derive(Clone)]
pub struct BrowserConfirm(
    std::sync::Arc<dyn Fn(BrowserConfirmation) -> BrowserConfirmFuture + Send + Sync>,
);

impl BrowserConfirm {
    pub fn new(
        confirm: impl Fn(BrowserConfirmation) -> BrowserConfirmFuture + Send + Sync + 'static,
    ) -> Self {
        Self(std::sync::Arc::new(confirm))
    }

    pub fn ask(&self, confirmation: BrowserConfirmation) -> BrowserConfirmFuture {
        (self.0)(confirmation)
    }
}

impl std::fmt::Debug for BrowserConfirm {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("BrowserConfirm")
    }
}

/// What the chrome page shows: every tab with its session's name.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BrowserState {
    pub tabs: Vec<BrowserTabView>,
    pub active_tab_id: Option<String>,
    /// Every download of every tab, newest first.
    pub downloads: Vec<downloads::BrowserDownloadView>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BrowserTabView {
    pub tab_id: String,
    pub session_id: String,
    pub session_title: String,
    pub url: String,
    pub title: String,
    pub active: bool,
    /// An agent tool call is running on this tab.
    pub agent_busy: bool,
    /// An agent action on this tab waits for the user's 허용/거부.
    pub awaiting_confirmation: bool,
}

/// Where `browser_navigate` goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavTarget {
    Url(tauri::Url),
    Back,
    Forward,
    Reload,
}

/// What `browser_download` downloads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadTarget {
    /// Click the element `ref` of the tab's latest snapshot.
    Element(String),
    /// Fetch this http(s) URL in the tab.
    Url(tauri::Url),
    /// Wait for a download the tab already started.
    Started,
}

/// One browser tool call, validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserRequest {
    Open {
        url: tauri::Url,
    },
    Navigate {
        tab_id: String,
        target: NavTarget,
    },
    Snapshot {
        tab_id: String,
        offset: usize,
    },
    Screenshot {
        tab_id: String,
        full_page: bool,
    },
    Tabs,
    Close {
        tab_id: String,
    },
    Wait {
        tab_id: String,
        text: Option<String>,
        ms: u64,
    },
    Click {
        tab_id: String,
        element: String,
    },
    Type {
        tab_id: String,
        element: String,
        text: String,
        submit: bool,
        clear: bool,
    },
    Press {
        tab_id: String,
        key: policy::KeyInfo,
    },
    Download {
        tab_id: String,
        target: DownloadTarget,
        timeout_ms: u64,
    },
}

impl BrowserRequest {
    pub fn tab_id(&self) -> Option<&str> {
        match self {
            Self::Navigate { tab_id, .. }
            | Self::Snapshot { tab_id, .. }
            | Self::Screenshot { tab_id, .. }
            | Self::Close { tab_id }
            | Self::Wait { tab_id, .. }
            | Self::Click { tab_id, .. }
            | Self::Type { tab_id, .. }
            | Self::Press { tab_id, .. }
            | Self::Download { tab_id, .. } => Some(tab_id),
            Self::Open { .. } | Self::Tabs => None,
        }
    }

    /// Clicking, typing, pressing keys and downloading act on the page (and
    /// may wait for the user's confirmation); the rest read it.
    pub fn interacts(&self) -> bool {
        matches!(
            self,
            Self::Click { .. } | Self::Type { .. } | Self::Press { .. } | Self::Download { .. }
        )
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ToolInput {
    url: Option<String>,
    tab_id: Option<String>,
    action: Option<String>,
    offset: Option<u64>,
    full_page: Option<bool>,
    text: Option<String>,
    ms: Option<u64>,
    #[serde(rename = "ref")]
    element: Option<String>,
    submit: Option<bool>,
    clear: Option<bool>,
    key: Option<String>,
    timeout_ms: Option<u64>,
}

/// Parse and validate the arguments of browser tool `tool`.
pub fn parse_request(tool: &str, args: &Value) -> Result<BrowserRequest, String> {
    use crate::tools::{
        BROWSER_CLICK_TOOL, BROWSER_CLOSE_TOOL, BROWSER_DOWNLOAD_TOOL, BROWSER_NAVIGATE_TOOL,
        BROWSER_OPEN_TOOL, BROWSER_PRESS_TOOL, BROWSER_SCREENSHOT_TOOL, BROWSER_SNAPSHOT_TOOL,
        BROWSER_TABS_TOOL, BROWSER_TYPE_TOOL, BROWSER_WAIT_TOOL,
    };
    let input: ToolInput = serde_json::from_value(args.clone()).map_err(|error| {
        format!(
            "{tool} 인자가 올바르지 않습니다({error}). 도구 설명의 인자 이름과 형식을 따르세요."
        )
    })?;
    let allowed: &[&str] = match tool {
        BROWSER_OPEN_TOOL => &["url"],
        BROWSER_NAVIGATE_TOOL => &["tabId", "url", "action"],
        BROWSER_SNAPSHOT_TOOL => &["tabId", "offset"],
        BROWSER_SCREENSHOT_TOOL => &["tabId", "fullPage"],
        BROWSER_TABS_TOOL => &[],
        BROWSER_CLOSE_TOOL => &["tabId"],
        BROWSER_WAIT_TOOL => &["tabId", "text", "ms"],
        BROWSER_CLICK_TOOL => &["tabId", "ref"],
        BROWSER_TYPE_TOOL => &["tabId", "ref", "text", "submit", "clear"],
        BROWSER_PRESS_TOOL => &["tabId", "key"],
        BROWSER_DOWNLOAD_TOOL => &["tabId", "ref", "url", "timeoutMs"],
        other => return Err(format!("`{other}` 은 브라우저 도구가 아닙니다.")),
    };
    if let Some(extra) = args
        .as_object()
        .into_iter()
        .flat_map(|object| object.keys())
        .find(|key| !allowed.contains(&key.as_str()))
    {
        return Err(format!(
            "{tool} 은 `{extra}` 인자를 받지 않습니다. 받는 인자: {}",
            allowed.join(", ")
        ));
    }
    let tab_id = || {
        input
            .tab_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .ok_or_else(|| {
                format!(
                    "{tool} 에는 tabId 가 필요합니다. browser_tabs 로 이 세션의 탭을 확인하세요."
                )
            })
    };
    let element = || {
        input
            .element
            .as_deref()
            .map(str::trim)
            .filter(|element| !element.is_empty())
            .map(str::to_string)
            .ok_or_else(|| {
                format!("{tool} 에는 ref 가 필요합니다. browser_snapshot 으로 요소의 ref(e1, e2…)를 확인하세요.")
            })
    };
    Ok(match tool {
        BROWSER_CLICK_TOOL => BrowserRequest::Click {
            tab_id: tab_id()?,
            element: element()?,
        },
        BROWSER_TYPE_TOOL => {
            let text = input.text.clone().unwrap_or_default();
            if text.is_empty() {
                return Err("browser_type text 가 비어 있습니다. 입력할 글자를 주세요(지우기만 하려면 clear 와 함께 browser_press Backspace 를 쓰세요).".into());
            }
            if text.chars().count() > MAX_TYPE_TEXT_CHARS {
                return Err(format!(
                    "browser_type text 는 {MAX_TYPE_TEXT_CHARS}자 이하여야 합니다."
                ));
            }
            BrowserRequest::Type {
                tab_id: tab_id()?,
                element: element()?,
                text,
                submit: input.submit.unwrap_or(false),
                clear: input.clear.unwrap_or(false),
            }
        }
        BROWSER_PRESS_TOOL => {
            let name = input.key.as_deref().unwrap_or("");
            let key = policy::key_info(name).ok_or_else(|| {
                format!(
                    "browser_press key `{name}` 은 누를 수 없습니다. 다음 중 하나를 주세요: {}",
                    policy::key_names().join(", ")
                )
            })?;
            BrowserRequest::Press {
                tab_id: tab_id()?,
                key,
            }
        }
        BROWSER_DOWNLOAD_TOOL => {
            let target = match (input.element.as_deref(), input.url.as_deref()) {
                (Some(_), Some(_)) => {
                    return Err("browser_download 에는 ref 와 url 중 하나만 주세요(둘 다 빼면 이 탭에서 이미 시작된 다운로드를 기다립니다).".to_string())
                }
                (Some(_), None) => DownloadTarget::Element(element()?),
                (None, Some(url)) => {
                    let url = validate_agent_url(url)?;
                    if url.scheme() == "about" {
                        return Err("browser_download url 은 http:// 또는 https:// 파일 주소여야 합니다.".to_string());
                    }
                    DownloadTarget::Url(url)
                }
                (None, None) => DownloadTarget::Started,
            };
            let timeout_ms = input
                .timeout_ms
                .unwrap_or(downloads::DEFAULT_DOWNLOAD_WAIT_MS);
            if !(1..=downloads::MAX_DOWNLOAD_WAIT_MS).contains(&timeout_ms) {
                return Err(format!(
                    "browser_download timeoutMs 는 1~{} 이어야 합니다. 더 오래 걸리면 tabId 만 주고 다시 호출해 이어서 기다리세요.",
                    downloads::MAX_DOWNLOAD_WAIT_MS
                ));
            }
            BrowserRequest::Download {
                tab_id: tab_id()?,
                target,
                timeout_ms,
            }
        }
        BROWSER_OPEN_TOOL => BrowserRequest::Open {
            url: validate_agent_url(input.url.as_deref().unwrap_or(""))?,
        },
        BROWSER_NAVIGATE_TOOL => {
            let target = match (input.url.as_deref(), input.action.as_deref()) {
                (Some(url), None) => NavTarget::Url(validate_agent_url(url)?),
                (None, Some("back")) => NavTarget::Back,
                (None, Some("forward")) => NavTarget::Forward,
                (None, Some("reload")) => NavTarget::Reload,
                (None, Some(other)) => {
                    return Err(format!(
                        "action `{other}` 은 없습니다. back, forward, reload 중 하나를 주세요."
                    ))
                }
                _ => {
                    return Err(
                        "browser_navigate 에는 url 과 action(back|forward|reload) 중 정확히 하나를 주세요."
                            .to_string(),
                    )
                }
            };
            BrowserRequest::Navigate {
                tab_id: tab_id()?,
                target,
            }
        }
        BROWSER_SNAPSHOT_TOOL => BrowserRequest::Snapshot {
            tab_id: tab_id()?,
            offset: usize::try_from(input.offset.unwrap_or(0)).unwrap_or(usize::MAX),
        },
        BROWSER_SCREENSHOT_TOOL => BrowserRequest::Screenshot {
            tab_id: tab_id()?,
            full_page: input.full_page.unwrap_or(false),
        },
        BROWSER_TABS_TOOL => BrowserRequest::Tabs,
        BROWSER_CLOSE_TOOL => BrowserRequest::Close { tab_id: tab_id()? },
        _ => {
            let text = input.text.map(|text| text.trim().to_string());
            if text.as_ref().is_some_and(String::is_empty) {
                return Err("browser_wait text 가 비어 있습니다. 기다릴 글자를 주거나 text 를 빼고 ms 만 주세요.".into());
            }
            if text
                .as_ref()
                .is_some_and(|text| text.chars().count() > MAX_WAIT_TEXT_CHARS)
            {
                return Err(format!(
                    "browser_wait text 는 {MAX_WAIT_TEXT_CHARS}자 이하여야 합니다."
                ));
            }
            if input.ms.is_some_and(|ms| ms > MAX_WAIT_MS) {
                return Err(format!(
                    "browser_wait ms 는 {MAX_WAIT_MS} 이하여야 합니다. 더 오래 기다려야 하면 다시 호출하세요."
                ));
            }
            let ms = match (&text, input.ms) {
                (_, Some(ms)) => ms,
                (Some(_), None) => 10_000,
                (None, None) => {
                    return Err("browser_wait 에는 text 나 ms 중 하나 이상이 필요합니다.".into())
                }
            };
            BrowserRequest::Wait {
                tab_id: tab_id()?,
                text,
                ms,
            }
        }
    })
}

/// A URL a tool or the address bar may open: http(s), or `about:blank`.
/// `file:`, `javascript:`, `data:`, browser-internal schemes and the app's own
/// `*.localhost` origins are refused, with the reason.
pub fn validate_url(raw: &str) -> Result<tauri::Url, String> {
    let text = raw.trim();
    if text.is_empty() {
        return Err("URL이 비어 있습니다. http:// 또는 https:// 로 시작하는 주소를 주세요.".into());
    }
    if text.chars().count() > MAX_URL_CHARS {
        return Err(format!(
            "URL이 {MAX_URL_CHARS}자를 넘습니다. 더 짧은 주소를 주세요."
        ));
    }
    if text.eq_ignore_ascii_case("about:blank") {
        return Ok(tauri::Url::parse("about:blank").expect("about:blank parses"));
    }
    let parsed = tauri::Url::parse(text).map_err(|error| {
        format!("URL을 해석할 수 없습니다({error}). http:// 또는 https:// 로 시작하는 전체 주소를 주세요.")
    })?;
    match parsed.scheme() {
        "http" | "https" => {}
        scheme => {
            return Err(format!(
                "`{scheme}:` 주소는 열 수 없습니다. 브라우저는 http:// 와 https:// 주소(및 about:blank)만 엽니다."
            ))
        }
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| "URL에 호스트가 없습니다. 전체 주소를 주세요.".to_string())?
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if host.ends_with(".localhost") {
        return Err(format!(
            "`{host}` 은 앱 내부 주소라 열 수 없습니다. 다른 사이트 주소를 주세요."
        ));
    }
    Ok(parsed)
}

/// A URL an agent tool (or a popup an agent call caused) may open:
/// [`validate_url`], and never a loopback, private, link-local, CGNAT or
/// unspecified address, nor a single-label or `.local`-style name, so the
/// agent cannot reach the user's router, NAS, local dev servers, the cloud
/// metadata address or this app's own remote origin. The user may still type
/// such an address into the address bar.
///
/// This is the URL check only; the tab's request policy repeats it at every
/// redirect hop of an agent-driven page load, and refuses a page whose
/// response came from such an address (a public name that resolves there).
pub fn validate_agent_url(raw: &str) -> Result<tauri::Url, String> {
    let parsed = validate_url(raw)?;
    if let Some(host) = agent_host_is_local(&parsed) {
        return Err(internal_refusal(&host));
    }
    Ok(parsed)
}

/// The refusal for an agent load of `host`, an address of this computer or
/// the local network.
pub fn internal_refusal(host: &str) -> String {
    format!(
        "`{host}` 은 이 컴퓨터나 내부망 주소라 에이전트가 열 수 없습니다. 공개 사이트 주소를 주거나, 꼭 필요하면 사용자에게 브라우저 주소창에서 직접 열어 달라고 요청하세요."
    )
}

/// Public DNS names that resolve every name under them to the address it
/// spells (`127.0.0.1.nip.io`) or to loopback (`localtest.me`).
const WILDCARD_LOOPBACK_DOMAINS: &[&str] = &[
    "nip.io",
    "sslip.io",
    "xip.io",
    "localtest.me",
    "lvh.me",
    "localho.st",
    "vcap.me",
    "traefik.me",
];

/// The host of `url` (lowercase, without brackets) when an agent may not
/// load it: an IP literal of this computer or the local network
/// ([`is_local_address`]), a single-label or local-only name, or a
/// [`WILDCARD_LOOPBACK_DOMAINS`] name.
pub fn agent_host_is_local(url: &tauri::Url) -> Option<String> {
    let host = url
        .host_str()?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let local = match host.parse::<std::net::IpAddr>() {
        Ok(address) => is_local_address(address),
        Err(_) => {
            !host.contains('.')
                || [
                    ".localhost",
                    ".local",
                    ".lan",
                    ".internal",
                    ".home.arpa",
                    ".localdomain",
                    ".ts.net",
                ]
                .iter()
                .any(|suffix| host.ends_with(suffix))
                || WILDCARD_LOOPBACK_DOMAINS
                    .iter()
                    .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
        }
    };
    local.then_some(host)
}

/// Whether a page response's `remoteIPAddress` (as DevTools reports it,
/// `[::1]` or `127.0.0.1`) is this computer or the local network.
pub fn remote_address_is_local(address: &str) -> bool {
    address
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<std::net::IpAddr>()
        .is_ok_and(is_local_address)
}

/// Whether `address` is this computer, the local network, or no public
/// unicast host at all. An IPv6 address that carries an IPv4 one (mapped
/// `::ffff:a.b.c.d`, SIIT `::ffff:0:a.b.c.d`, compatible `::a.b.c.d`, NAT64
/// `64:ff9b::/96`, 6to4 `2002::/16`) is judged by that IPv4 address; one
/// whose IPv4 address cannot be told or is local by definition (local-use
/// NAT64 `64:ff9b:1::/48`, Teredo `2001::/32`) is never public.
pub fn is_local_address(address: std::net::IpAddr) -> bool {
    match address {
        std::net::IpAddr::V4(v4) => {
            let [a, b, c, _] = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || a == 0
                || (a == 100 && (64..128).contains(&b))
                // Benchmarking (198.18.0.0/15) and IETF protocol
                // assignments (192.0.0.0/24).
                || (a == 198 && (b == 18 || b == 19))
                || (a == 192 && b == 0 && c == 0)
                // Reserved 240.0.0.0/4 (with the broadcast address).
                || a >= 240
        }
        std::net::IpAddr::V6(v6) => {
            if let Some(v4) = embedded_ipv4(v6) {
                return is_local_address(std::net::IpAddr::V4(v4));
            }
            let [first, second, third, ..] = v6.segments();
            v6.is_loopback()
                || v6.is_unspecified()
                || (first == 0x64 && second == 0xff9b && third == 1)
                || (first == 0x2001 && second == 0)
                || v6.is_multicast()
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80
                // Deprecated site-local `fec0::/10` (RFC 3879), still routed
                // to the local site by stacks that never dropped it.
                || (first & 0xffc0) == 0xfec0
        }
    }
}

/// The IPv4 address an IPv6 address stands for, if it carries one.
fn embedded_ipv4(v6: std::net::Ipv6Addr) -> Option<std::net::Ipv4Addr> {
    let segments = v6.segments();
    let low = |high: u16, low: u16| {
        std::net::Ipv4Addr::new((high >> 8) as u8, high as u8, (low >> 8) as u8, low as u8)
    };
    if let Some(v4) = v6.to_ipv4_mapped() {
        return Some(v4);
    }
    match segments {
        // SIIT IPv4-translated `::ffff:0:a.b.c.d`.
        [0, 0, 0, 0, 0xffff, 0, high, low_bits] => Some(low(high, low_bits)),
        // IPv4-compatible `::a.b.c.d` (`::` and `::1` are themselves local).
        [0, 0, 0, 0, 0, 0, high, low_bits] => Some(low(high, low_bits)),
        // NAT64 well-known prefix `64:ff9b::/96`.
        [0x64, 0xff9b, 0, 0, 0, 0, high, low_bits] => Some(low(high, low_bits)),
        // 6to4 `2002:aabb:ccdd::/48`.
        [0x2002, high, low_bits, ..] => Some(low(high, low_bits)),
        _ => None,
    }
}

/// Whether a page may navigate a tab (or load a request) to `url` on its
/// own: http(s) outside the app's internal origins, and `about:`/`data:`/
/// `blob:`. The app's schemes, browser-internal pages and every external
/// protocol (which would start an OS handler) are refused.
pub fn page_navigation_allowed(url: &tauri::Url) -> bool {
    navigation_allowed_besides(url, dev_origin())
}

/// The dev server the app's own pages come from in a debug build
/// (`build.devUrl`); recorded once the browser starts.
static DEV_ORIGIN: std::sync::OnceLock<Option<tauri::Url>> = std::sync::OnceLock::new();

fn dev_origin() -> Option<&'static tauri::Url> {
    DEV_ORIGIN.get().and_then(Option::as_ref)
}

/// [`page_navigation_allowed`], and never the origin of `dev` (the debug
/// build's dev server, which serves the app's own pages).
fn navigation_allowed_besides(url: &tauri::Url, dev: Option<&tauri::Url>) -> bool {
    if dev.is_some_and(|dev| dev.origin() == url.origin()) {
        return false;
    }
    match url.scheme() {
        "http" | "https" => !url.host_str().is_some_and(|host| {
            host.trim_end_matches('.')
                .to_ascii_lowercase()
                .ends_with(".localhost")
        }),
        "about" | "data" | "blob" => true,
        // tauri/ipc/asset/file, browser-internal pages, and every external
        // protocol an OS handler would take (`mailto:`, `ms-settings:`, ...).
        _ => false,
    }
}

/// The app's browser. Cheap to clone; every clone is the same browser.
#[derive(Clone)]
pub struct BrowserService {
    #[cfg(windows)]
    host: host::BrowserHost,
    downloads: downloads::DownloadStore,
}

impl BrowserService {
    /// Downloads older than a day are removed from `downloads_dir` first.
    #[cfg_attr(not(windows), allow(unused_variables))]
    pub fn new(
        app: tauri::AppHandle,
        profile_dir: std::path::PathBuf,
        downloads_dir: std::path::PathBuf,
        sessions: crate::session::SessionStore,
    ) -> Self {
        if cfg!(debug_assertions) {
            let _ = DEV_ORIGIN.set(app.config().build.dev_url.clone());
        }
        let cleanup = downloads_dir.clone();
        std::thread::spawn(move || {
            downloads::cleanup_stale(
                &cleanup,
                downloads::DOWNLOAD_RETENTION,
                std::time::SystemTime::now(),
            );
        });
        let downloads = downloads::DownloadStore::new(downloads_dir);
        Self {
            #[cfg(windows)]
            host: host::BrowserHost::new(app, profile_dir, sessions, downloads.clone()),
            downloads,
        }
    }

    /// Every download of the browser; tool calls read only their session's.
    pub fn downloads(&self) -> downloads::DownloadStore {
        self.downloads.clone()
    }

    /// Run one browser tool for its session.
    pub async fn execute(&self, call: BrowserCall) -> Result<Value, String> {
        #[cfg(windows)]
        {
            self.host.execute(call).await
        }
        #[cfg(not(windows))]
        {
            let _ = call;
            Err(UNSUPPORTED_PLATFORM.to_string())
        }
    }

    /// Close every tab session `session_id` owns and forget the domains its
    /// user approved (the session was deleted).
    pub fn close_session_tabs(&self, session_id: &str) {
        #[cfg(windows)]
        self.host.close_session(session_id);
        self.downloads.forget_session(session_id);
    }

    /// Close every tab of project `project_id` (the project closed). With no
    /// tab left the window closes too, so it never keeps the app running.
    pub fn close_project_tabs(&self, project_id: &str) {
        #[cfg(windows)]
        self.host.close_project_tabs(project_id);
        #[cfg(not(windows))]
        let _ = project_id;
    }

    pub fn state(&self) -> Result<BrowserState, String> {
        #[cfg(windows)]
        {
            Ok(self.host.state())
        }
        #[cfg(not(windows))]
        {
            Err(UNSUPPORTED_PLATFORM.to_string())
        }
    }

    pub fn activate(&self, tab_id: &str) -> Result<(), String> {
        #[cfg(windows)]
        {
            self.host.activate(tab_id)
        }
        #[cfg(not(windows))]
        {
            let _ = tab_id;
            Err(UNSUPPORTED_PLATFORM.to_string())
        }
    }

    pub fn user_close(&self, tab_id: &str) -> Result<(), String> {
        #[cfg(windows)]
        {
            self.host.user_close(tab_id)
        }
        #[cfg(not(windows))]
        {
            let _ = tab_id;
            Err(UNSUPPORTED_PLATFORM.to_string())
        }
    }

    pub async fn user_navigate(
        &self,
        tab_id: &str,
        url: Option<String>,
        action: Option<String>,
    ) -> Result<(), String> {
        #[cfg(windows)]
        {
            self.host.user_navigate(tab_id, url, action).await
        }
        #[cfg(not(windows))]
        {
            let _ = (tab_id, url, action);
            Err(UNSUPPORTED_PLATFORM.to_string())
        }
    }

    /// The chrome's downloads list is open: let the chrome cover the tab
    /// area so the list is not cut off, or give the area back to the tab.
    pub fn set_chrome_expanded(&self, expanded: bool) -> Result<(), String> {
        #[cfg(windows)]
        {
            self.host.set_chrome_expanded(expanded);
            Ok(())
        }
        #[cfg(not(windows))]
        {
            let _ = expanded;
            Err(UNSUPPORTED_PLATFORM.to_string())
        }
    }

    /// Settings → 브라우저 → 로그인 데이터 초기화: delete the profile (every
    /// sign-in) and the managed downloads, only while the 브라우저 window is
    /// closed and no tab exists, so no WebView2 holds the folder.
    pub async fn reset_profile(&self) -> Result<(), String> {
        #[cfg(windows)]
        {
            self.host.reset_profile().await
        }
        #[cfg(not(windows))]
        {
            Err(UNSUPPORTED_PLATFORM.to_string())
        }
    }

    /// Close this project's tabs whenever the registry closes a project.
    pub fn follow_project_closes(&self, registry: &crate::project_registry::ProjectRegistry) {
        let mut closed = registry.subscribe_closed();
        let browser = self.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                match closed.recv().await {
                    Ok(id) => browser.close_project_tabs(id.as_str()),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
}

pub const PROFILE_RESET_WINDOW_OPEN: &str =
    "브라우저 창이 열려 있어 로그인 데이터를 지울 수 없습니다. 브라우저 창을 닫은 뒤 다시 시도하세요.";

/// Delete the browser profile and downloads folders unless the window (or a
/// tab) is `open`. `profile_dir` must end in `browser/profile` and
/// `downloads_dir` in `browser/downloads`; a missing folder is already reset.
pub fn reset_profile_dirs(
    open: bool,
    profile_dir: &std::path::Path,
    downloads_dir: &std::path::Path,
) -> Result<(), String> {
    if open {
        return Err(PROFILE_RESET_WINDOW_OPEN.to_string());
    }
    remove_managed_dir(profile_dir, "profile")?;
    remove_managed_dir(downloads_dir, "downloads")
}

/// Remove `dir`, a real directory named `name` inside a real `browser`
/// directory, with everything below it. A link or junction in its place, or
/// in place of `browser`, is refused; links inside it are removed as links
/// and never followed (`remove_dir_all` opens every entry without following
/// reparse points).
fn remove_managed_dir(dir: &std::path::Path, name: &str) -> Result<(), String> {
    let unexpected = || {
        format!(
            "{} 은(는) 앱의 브라우저 폴더가 아니어서 지우지 않았습니다. 앱을 다시 시작한 뒤 다시 시도하세요.",
            dir.display()
        )
    };
    let parent = dir.parent().ok_or_else(unexpected)?;
    if dir.file_name().and_then(|value| value.to_str()) != Some(name)
        || parent.file_name().and_then(|value| value.to_str()) != Some("browser")
    {
        return Err(unexpected());
    }
    let real_dir = |path: &std::path::Path| match std::fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(
            metadata.is_dir()
                && !metadata.file_type().is_symlink()
                && !crate::memory::is_reparse_point(&metadata),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "브라우저 폴더를 확인할 수 없습니다({error}). 다시 시도하세요."
        )),
    };
    match real_dir(parent)? {
        None => return Ok(()),
        Some(false) => return Err(unexpected()),
        Some(true) => {}
    }
    match real_dir(dir)? {
        None => Ok(()),
        Some(false) => Err(unexpected()),
        Some(true) => std::fs::remove_dir_all(dir).map_err(|error| {
            format!(
                "로그인 데이터를 모두 지우지 못했습니다({error}). 브라우저 창이 완전히 닫힐 때까지 잠시 기다린 뒤 다시 시도하세요."
            )
        }),
    }
}

/// Settings commands run only in a main (project) window.
fn require_main_window(webview: &tauri::Webview) -> Result<(), String> {
    if crate::project_launch::is_project_window(webview.label()) {
        Ok(())
    } else {
        Err("메인 창의 설정에서만 쓸 수 있는 명령입니다.".to_string())
    }
}

/// Only the browser window's chrome webview drives the browser from the UI.
fn require_chrome(webview: &tauri::Webview) -> Result<(), String> {
    if webview.label() == BROWSER_WINDOW_LABEL {
        Ok(())
    } else {
        Err("브라우저 창에서만 쓸 수 있는 명령입니다.".to_string())
    }
}

#[tauri::command]
pub fn browser_state(
    webview: tauri::Webview,
    browser: tauri::State<'_, BrowserService>,
) -> Result<BrowserState, String> {
    require_chrome(&webview)?;
    browser.state()
}

#[tauri::command]
pub fn browser_activate(
    webview: tauri::Webview,
    browser: tauri::State<'_, BrowserService>,
    tab_id: String,
) -> Result<(), String> {
    require_chrome(&webview)?;
    browser.activate(&tab_id)
}

#[tauri::command]
pub fn browser_tab_close(
    webview: tauri::Webview,
    browser: tauri::State<'_, BrowserService>,
    tab_id: String,
) -> Result<(), String> {
    require_chrome(&webview)?;
    browser.user_close(&tab_id)
}

/// The user's own navigation from the chrome: a typed URL, or back /
/// forward / reload.
#[tauri::command]
pub async fn browser_user_navigate(
    webview: tauri::Webview,
    browser: tauri::State<'_, BrowserService>,
    tab_id: String,
    url: Option<String>,
    action: Option<String>,
) -> Result<(), String> {
    require_chrome(&webview)?;
    let browser = browser.inner().clone();
    browser.user_navigate(&tab_id, url, action).await
}

/// The chrome opened (or closed) its downloads list.
#[tauri::command]
pub fn browser_chrome_expand(
    webview: tauri::Webview,
    browser: tauri::State<'_, BrowserService>,
    expanded: bool,
) -> Result<(), String> {
    require_chrome(&webview)?;
    browser.set_chrome_expanded(expanded)
}

/// Settings → 브라우저 → 로그인 데이터 초기화.
#[tauri::command]
pub async fn browser_profile_reset(
    webview: tauri::Webview,
    browser: tauri::State<'_, BrowserService>,
) -> Result<(), String> {
    require_main_window(&webview)?;
    let browser = browser.inner().clone();
    browser.reset_profile().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_that_carry_or_are_no_public_host_are_local() {
        for address in [
            "127.0.0.1",
            "10.0.0.1",
            "198.18.0.1",
            "198.19.255.255",
            "192.0.0.8",
            "224.0.0.251",
            "239.255.255.250",
            "::1",
            "::",
            "::ffff:127.0.0.1",
            "::127.0.0.1",
            "::10.1.2.3",
            "64:ff9b::7f00:1",
            "64:ff9b::192.168.0.1",
            "2002:7f00:1::",
            "2002:c0a8:101::1",
            "ff02::1",
            "ff05::2",
            "fd00::1",
            "fe80::1",
            // Deprecated site-local `fec0::/10`.
            "fec0::1",
            "feff:ffff::1",
            // Reserved 240.0.0.0/4.
            "240.0.0.1",
            "255.255.255.254",
            // Local-use NAT64 `64:ff9b:1::/48`, whatever it carries.
            "64:ff9b:1::808:808",
            "64:ff9b:1:ffff::1",
            // Teredo `2001::/32` (its client address is obfuscated).
            "2001:0:4136:e378:8000:63bf:3fff:fdd2",
            "2001::1",
            // SIIT `::ffff:0:a.b.c.d`.
            "::ffff:0:127.0.0.1",
            "::ffff:0:192.168.1.1",
        ] {
            assert!(
                is_local_address(address.parse().unwrap()),
                "{address} must be local"
            );
        }
        for address in [
            "8.8.8.8",
            "198.20.0.1",
            "192.0.1.1",
            "223.255.255.255",
            "2606:4700::1111",
            "64:ff9b::808:808",
            "2002:808:808::",
            "::8.8.8.8",
            "::ffff:0:8.8.8.8",
            "2001:4860:4860::8888",
        ] {
            assert!(
                !is_local_address(address.parse().unwrap()),
                "{address} is public"
            );
        }
        assert!(remote_address_is_local("[64:ff9b::a00:1]"));
    }

    fn reset_fixture(name: &str) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
        let root =
            std::env::temp_dir().join(format!("eud-browser-reset-{name}-{}", uuid::Uuid::new_v4()));
        let profile = root.join("browser").join("profile");
        let downloads = root.join("browser").join("downloads");
        std::fs::create_dir_all(profile.join("Default")).unwrap();
        std::fs::write(profile.join("Default").join("Cookies"), b"cookie").unwrap();
        std::fs::create_dir_all(downloads.join("d1")).unwrap();
        std::fs::write(downloads.join("d1").join("a.ogg"), b"ogg").unwrap();
        (root, profile, downloads)
    }

    #[test]
    fn the_profile_is_never_reset_while_the_window_is_open() {
        let (root, profile, downloads) = reset_fixture("open");
        assert_eq!(
            reset_profile_dirs(true, &profile, &downloads),
            Err(PROFILE_RESET_WINDOW_OPEN.to_string())
        );
        assert!(profile.join("Default").join("Cookies").is_file());
        assert!(downloads.join("d1").join("a.ogg").is_file());

        reset_profile_dirs(false, &profile, &downloads).unwrap();
        assert!(!profile.exists() && !downloads.exists());
        assert!(root.join("browser").is_dir(), "only the two folders go");
        reset_profile_dirs(false, &profile, &downloads).expect("already reset is fine");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_reset_deletes_only_the_expected_folders_and_never_follows_links() {
        let (root, profile, downloads) = reset_fixture("links");
        // Wrong names are refused before anything is touched.
        let elsewhere = root.join("browser").join("other");
        std::fs::create_dir_all(&elsewhere).unwrap();
        assert!(reset_profile_dirs(false, &elsewhere, &downloads).is_err());
        assert!(reset_profile_dirs(false, &root.join("profile"), &downloads).is_err());
        assert!(elsewhere.is_dir() && profile.is_dir() && downloads.is_dir());

        let outside = root.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("keep.txt"), b"keep").unwrap();
        #[cfg(windows)]
        let link = |link: &std::path::Path, target: &std::path::Path| {
            let command = format!("mklink /J {} {}", link.display(), target.display());
            let status = std::process::Command::new("cmd.exe")
                .args(["/d", "/s", "/c", &command])
                .stdout(std::process::Stdio::null())
                .status()
                .unwrap();
            assert!(status.success(), "junction must be created");
        };
        #[cfg(unix)]
        let link = |link: &std::path::Path, target: &std::path::Path| {
            std::os::unix::fs::symlink(target, link).unwrap();
        };
        // A link inside the profile is removed as a link, never followed.
        link(&profile.join("Default").join("escape"), &outside);
        reset_profile_dirs(false, &profile, &downloads).unwrap();
        assert!(!profile.exists());
        assert!(
            outside.join("keep.txt").is_file(),
            "the link target survives"
        );

        // A link in the profile's place is refused, and its target kept.
        std::fs::create_dir_all(profile.parent().unwrap()).unwrap();
        link(&profile, &outside);
        assert!(reset_profile_dirs(false, &profile, &downloads).is_err());
        assert!(outside.join("keep.txt").is_file());
        let _ = std::fs::remove_dir(&profile);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn download_arguments_pick_one_target_and_bound_the_wait() {
        use crate::tools::BROWSER_DOWNLOAD_TOOL;
        use serde_json::json;
        let parse = |args: Value| parse_request(BROWSER_DOWNLOAD_TOOL, &args);
        assert_eq!(
            parse(json!({"tabId": "t1", "ref": " e3 "})).unwrap(),
            BrowserRequest::Download {
                tab_id: "t1".into(),
                target: DownloadTarget::Element("e3".into()),
                timeout_ms: downloads::DEFAULT_DOWNLOAD_WAIT_MS,
            }
        );
        assert_eq!(
            parse(json!({"tabId": "t1", "url": "https://example.com/a.ogg", "timeoutMs": 5000}))
                .unwrap(),
            BrowserRequest::Download {
                tab_id: "t1".into(),
                target: DownloadTarget::Url(
                    tauri::Url::parse("https://example.com/a.ogg").unwrap()
                ),
                timeout_ms: 5000,
            }
        );
        let started = parse(json!({"tabId": "t1"})).unwrap();
        assert!(matches!(
            started,
            BrowserRequest::Download {
                target: DownloadTarget::Started,
                ..
            }
        ));
        assert!(started.interacts());
        for invalid in [
            json!({"tabId": "t1", "ref": "e1", "url": "https://a.b/x"}),
            json!({"tabId": "t1", "url": "about:blank"}),
            json!({"tabId": "t1", "url": "file:///C:/x.ogg"}),
            json!({"tabId": "t1", "timeoutMs": 0}),
            json!({"tabId": "t1", "timeoutMs": 120_001}),
            json!({"ref": "e1"}),
            json!({"tabId": "t1", "path": "x"}),
        ] {
            assert!(parse(invalid.clone()).is_err(), "{invalid}");
        }
    }

    #[test]
    fn only_http_https_and_about_blank_open() {
        for ok in [
            "https://example.com",
            "http://example.com/a?b=c#d",
            "  https://example.com  ",
            "about:blank",
            "ABOUT:BLANK",
            "http://localhost:3000/",
            "http://127.0.0.1:8080/",
        ] {
            assert!(validate_url(ok).is_ok(), "{ok}");
        }
        for refused in [
            "",
            "file:///C:/Windows/win.ini",
            "javascript:alert(1)",
            "data:text/html,<b>x</b>",
            "chrome://settings",
            "edge://settings",
            "about:settings",
            "ftp://example.com",
            "example.com",
            "http://tauri.localhost/index.html",
            "https://ipc.localhost/",
            "http://asset.LOCALHOST./x",
        ] {
            let error = validate_url(refused).unwrap_err();
            assert!(!error.is_empty(), "{refused}");
        }
        let long = format!("https://example.com/{}", "a".repeat(MAX_URL_CHARS));
        assert!(validate_url(&long).unwrap_err().contains("4096"));
    }

    #[test]
    fn the_agent_never_opens_wildcard_loopback_names() {
        for url in [
            "http://127.0.0.1.nip.io/",
            "http://anything.192.168.1.1.nip.io/",
            "https://10.0.0.1.sslip.io/",
            "http://localtest.me/",
            "http://app.localtest.me:3000/",
            "http://lvh.me/",
            "http://x.localho.st/",
        ] {
            let error = validate_agent_url(url).unwrap_err();
            assert!(error.contains("내부망"), "{url}: {error}");
        }
        assert!(validate_agent_url("https://notnip.io/").is_ok());
        assert!(validate_agent_url("https://mylocaltest.me/").is_ok());
    }

    #[test]
    fn the_agent_never_opens_local_or_private_hosts() {
        for refused in [
            "http://localhost:3000/",
            "http://LOCALHOST./",
            "http://127.0.0.1:8080/",
            "http://127.1/",
            "http://0x7f.0.0.1/",
            "http://0.0.0.0/",
            "http://10.1.2.3/",
            "http://172.16.0.1/",
            "http://172.31.255.255/",
            "http://192.168.0.1/admin",
            "http://169.254.169.254/latest/meta-data",
            "http://100.64.0.1/",
            "http://100.127.255.254/",
            "http://[::1]/",
            "http://[::]/",
            "http://[fd00::1]/",
            "http://[fc00::1]/",
            "http://[fe80::1]/",
            "http://[::ffff:192.168.0.1]/",
            "http://router/",
            "http://printer.local/",
            "http://my-pc.ts.net./",
        ] {
            let error = validate_agent_url(refused).expect_err(refused);
            assert!(error.contains("열 수 없습니다"), "{refused}: {error}");
            // The user may still type them into the address bar.
            assert!(validate_url(refused).is_ok(), "{refused}");
        }
        for ok in [
            "https://example.com/",
            "http://172.32.0.1/",
            "http://100.128.0.1/",
            "http://8.8.8.8/",
            "http://[2606:4700::1111]/",
            "about:blank",
        ] {
            assert!(validate_agent_url(ok).is_ok(), "{ok}");
        }
        // Every agent tool that takes a URL goes through it.
        use crate::tools::*;
        use serde_json::json;
        let local = json!({"url": "http://192.168.0.1/", "tabId": "t1"});
        assert!(parse_request(BROWSER_OPEN_TOOL, &json!({"url": "http://192.168.0.1/"})).is_err());
        assert!(parse_request(BROWSER_NAVIGATE_TOOL, &local).is_err());
        assert!(parse_request(BROWSER_DOWNLOAD_TOOL, &local).is_err());
    }

    #[test]
    fn tool_arguments_are_parsed_and_bounded() {
        use crate::tools::*;
        use serde_json::json;
        assert_eq!(
            parse_request(BROWSER_OPEN_TOOL, &json!({"url": "https://example.com"})).unwrap(),
            BrowserRequest::Open {
                url: tauri::Url::parse("https://example.com").unwrap()
            }
        );
        assert!(parse_request(BROWSER_OPEN_TOOL, &json!({"url": "file:///x"})).is_err());
        assert!(parse_request(BROWSER_OPEN_TOOL, &json!({})).is_err());
        assert!(parse_request(
            BROWSER_OPEN_TOOL,
            &json!({"url": "https://a.b", "tabId": "t1"})
        )
        .is_err());

        assert_eq!(
            parse_request(
                BROWSER_NAVIGATE_TOOL,
                &json!({"tabId": " t1 ", "action": "back"})
            )
            .unwrap(),
            BrowserRequest::Navigate {
                tab_id: "t1".into(),
                target: NavTarget::Back
            }
        );
        for invalid in [
            json!({"tabId": "t1"}),
            json!({"tabId": "t1", "url": "https://a.b", "action": "reload"}),
            json!({"tabId": "t1", "action": "home"}),
            json!({"url": "https://a.b"}),
            json!({"tabId": "", "url": "https://a.b"}),
        ] {
            assert!(
                parse_request(BROWSER_NAVIGATE_TOOL, &invalid).is_err(),
                "{invalid}"
            );
        }

        assert_eq!(
            parse_request(BROWSER_SNAPSHOT_TOOL, &json!({"tabId": "t2", "offset": 40})).unwrap(),
            BrowserRequest::Snapshot {
                tab_id: "t2".into(),
                offset: 40
            }
        );
        assert!(
            parse_request(BROWSER_SNAPSHOT_TOOL, &json!({"tabId": "t2", "offset": -1})).is_err()
        );
        assert_eq!(
            parse_request(BROWSER_SCREENSHOT_TOOL, &json!({"tabId": "t2"})).unwrap(),
            BrowserRequest::Screenshot {
                tab_id: "t2".into(),
                full_page: false
            }
        );
        assert_eq!(
            parse_request(BROWSER_TABS_TOOL, &json!({})).unwrap(),
            BrowserRequest::Tabs
        );
        assert!(parse_request(BROWSER_TABS_TOOL, &json!({"tabId": "t1"})).is_err());
        assert_eq!(
            parse_request(BROWSER_CLOSE_TOOL, &json!({"tabId": "t3"})).unwrap(),
            BrowserRequest::Close {
                tab_id: "t3".into()
            }
        );

        assert_eq!(
            parse_request(BROWSER_WAIT_TOOL, &json!({"tabId": "t1", "text": " 완료 "})).unwrap(),
            BrowserRequest::Wait {
                tab_id: "t1".into(),
                text: Some("완료".into()),
                ms: 10_000
            }
        );
        assert_eq!(
            parse_request(BROWSER_WAIT_TOOL, &json!({"tabId": "t1", "ms": 500})).unwrap(),
            BrowserRequest::Wait {
                tab_id: "t1".into(),
                text: None,
                ms: 500
            }
        );
        for invalid in [
            json!({"tabId": "t1"}),
            json!({"tabId": "t1", "ms": 30_001}),
            json!({"tabId": "t1", "text": "  "}),
            json!({"tabId": "t1", "text": "x".repeat(501)}),
        ] {
            assert!(
                parse_request(BROWSER_WAIT_TOOL, &invalid).is_err(),
                "{invalid}"
            );
        }
        assert!(parse_request("fs_read", &json!({})).is_err());
    }

    #[test]
    fn interaction_arguments_are_parsed_and_bounded() {
        use crate::tools::*;
        use serde_json::json;
        let click =
            parse_request(BROWSER_CLICK_TOOL, &json!({"tabId": "t1", "ref": " e4 "})).unwrap();
        assert_eq!(
            click,
            BrowserRequest::Click {
                tab_id: "t1".into(),
                element: "e4".into()
            }
        );
        assert!(click.interacts());
        assert!(!BrowserRequest::Tabs.interacts());
        for invalid in [
            json!({"tabId": "t1"}),
            json!({"ref": "e1"}),
            json!({"tabId": "t1", "ref": ""}),
            json!({"tabId": "t1", "ref": "e1", "text": "x"}),
        ] {
            assert!(
                parse_request(BROWSER_CLICK_TOOL, &invalid).is_err(),
                "{invalid}"
            );
        }

        assert_eq!(
            parse_request(
                BROWSER_TYPE_TOOL,
                &json!({"tabId": "t1", "ref": "e2", "text": " 안녕 효과음 ", "submit": true})
            )
            .unwrap(),
            BrowserRequest::Type {
                tab_id: "t1".into(),
                element: "e2".into(),
                text: " 안녕 효과음 ".into(),
                submit: true,
                clear: false,
            },
            "typed text is kept exactly"
        );
        for invalid in [
            json!({"tabId": "t1", "ref": "e2"}),
            json!({"tabId": "t1", "ref": "e2", "text": ""}),
            json!({"tabId": "t1", "ref": "e2", "text": "x".repeat(MAX_TYPE_TEXT_CHARS + 1)}),
            json!({"tabId": "t1", "ref": "e2", "text": "x", "key": "Enter"}),
        ] {
            assert!(
                parse_request(BROWSER_TYPE_TOOL, &invalid).is_err(),
                "{invalid}"
            );
        }

        let BrowserRequest::Press { key, .. } = parse_request(
            BROWSER_PRESS_TOOL,
            &json!({"tabId": "t1", "key": "PageDown"}),
        )
        .unwrap() else {
            panic!("a press request");
        };
        assert_eq!(key.windows_virtual_key_code, 34);
        let unknown =
            parse_request(BROWSER_PRESS_TOOL, &json!({"tabId": "t1", "key": "F12"})).unwrap_err();
        assert!(unknown.contains("Enter, Tab"), "{unknown}");
        assert!(parse_request(BROWSER_PRESS_TOOL, &json!({"tabId": "t1"})).is_err());
    }

    #[test]
    fn pages_never_navigate_to_app_origins() {
        let parse = |text: &str| tauri::Url::parse(text).unwrap();
        assert!(page_navigation_allowed(&parse("https://example.com/")));
        assert!(page_navigation_allowed(&parse("about:blank")));
        assert!(page_navigation_allowed(&parse("http://localhost:5173/")));
        assert!(!page_navigation_allowed(&parse("http://tauri.localhost/")));
        assert!(!page_navigation_allowed(&parse("https://ipc.localhost/x")));
        assert!(!page_navigation_allowed(&parse("tauri://localhost/")));
        assert!(!page_navigation_allowed(&parse("file:///C:/x")));
        // Pages keep data:/blob: documents; no other scheme reaches an OS
        // handler or a browser-internal page.
        assert!(page_navigation_allowed(&parse("data:text/html,x")));
        assert!(page_navigation_allowed(&parse(
            "blob:https://example.com/0f0a2b1c-0000-4000-8000-000000000000"
        )));
        for external in [
            "mailto:a@example.com",
            "ms-settings:privacy",
            "steam://run/1",
            "edge://settings",
            "chrome://version",
            "javascript:alert(1)",
            "ftp://example.com/",
        ] {
            assert!(!page_navigation_allowed(&parse(external)), "{external}");
        }
    }

    #[test]
    fn a_debug_build_keeps_tabs_off_its_dev_server() {
        let parse = |text: &str| tauri::Url::parse(text).unwrap();
        let dev = Some(parse("http://localhost:6173"));
        for url in [
            "http://localhost:6173/",
            "http://LOCALHOST:6173/browser.html",
            "http://localhost:6173/src/main.tsx?x=1",
        ] {
            assert!(
                !navigation_allowed_besides(&parse(url), dev.as_ref()),
                "{url}"
            );
        }
        for url in [
            "http://localhost:6174/",
            "https://localhost:6173/",
            "http://127.0.0.1:6173/",
            "https://example.com/",
        ] {
            assert!(
                navigation_allowed_besides(&parse(url), dev.as_ref()),
                "{url}"
            );
        }
        assert!(navigation_allowed_besides(
            &parse("http://localhost:6173/"),
            None
        ));
    }
}
