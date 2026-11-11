//! The rules the app enforces on an agent's clicks, typing and form
//! submissions, whatever the model was told: kept pure here so every rule is
//! tested without a window.
//!
//! - Credential fields (passwords, one-time codes, card data) are never typed
//!   into by the agent: the user enters them in the browser window.
//! - A click on a control named like a payment, purchase, subscription or
//!   account change waits for the user's confirmation.
//! - A form POST the agent's action caused, to a domain the user has not
//!   approved for that session, waits for the user's confirmation. The user's
//!   own submissions in the window are never held. A data-changing script
//!   request (POST/PUT/PATCH/DELETE) an agent interaction caused is held the
//!   same way, and so is the click an Enter or Space key press amounts to.
//! - No document may load from the app's own origins, in a tab or an iframe.

use std::collections::HashMap;

use serde::Deserialize;

/// The refusal for typing into a credential field.
pub const CREDENTIAL_REFUSED: &str = "이 입력란은 비밀번호·인증번호·카드 정보 입력란이라 에이전트가 입력할 수 없습니다. 사용자에게 브라우저 창에서 직접 입력해 달라고 요청하고, 입력이 끝났다고 답하면 이어서 진행하세요.";
/// The tool error after the user refused (or did not answer) a sensitive click.
pub const CLICK_DENIED: &str = "사용자가 이 클릭을 허용하지 않았습니다. 클릭하지 않았습니다; 같은 동작을 다시 시도하지 말고 사용자에게 어떻게 할지 물어보세요.";
/// The tool error when a sensitive click could not be put to the user.
pub const CLICK_UNASKED: &str = "이 클릭을 사용자에게 확인받지 못해 누르지 않았습니다(다른 확인이나 질문이 답을 기다리고 있었거나 시간이 부족했습니다). 사용자가 거부한 것이 아닙니다; 진행 중인 확인이 끝난 뒤 한 번 다시 시도하거나 사용자에게 물어보세요.";

/// Words that name a payment, purchase, subscription or account change.
/// Korean terms match anywhere in the label (Korean has no word boundary);
/// English terms match whole words (and whole word sequences).
pub const SENSITIVE_TERMS: &[&str] = &[
    "결제",
    "구매",
    "주문",
    "구독",
    "정기결제",
    "충전",
    "결제하기",
    "탈퇴",
    "회원탈퇴",
    "계정 삭제",
    "삭제",
    "비밀번호 변경",
    "이메일 변경",
    "해지",
    "pay",
    "purchase",
    "buy",
    "checkout",
    "order",
    "subscribe",
    "upgrade plan",
    "delete account",
    "remove account",
    "change password",
    "cancel subscription",
    "top up",
];

/// A label that only shows or looks up a record ("주문 내역", "결제 내역
/// 조회", "order history") changes nothing.
const VIEW_ONLY_TERMS: &[&str] = &["내역", "조회", "history"];

/// autocomplete tokens of credential and card fields.
const CREDENTIAL_AUTOCOMPLETE: &[&str] = &[
    "current-password",
    "new-password",
    "one-time-code",
    "cc-number",
    "cc-csc",
    "cc-name",
];

/// Accessible names of credential and card fields.
const CREDENTIAL_NAMES: &[&str] = &[
    "password",
    "passwd",
    "passcode",
    "비밀번호",
    "패스워드",
    "card number",
    "카드번호",
    "카드 번호",
    "cvc",
    "cvv",
    "보안코드",
    "security code",
];

/// The longest label text the sensitive matcher reads; a control's label is
/// short, and a whole paragraph says nothing about one button.
const MAX_LABEL_CHARS: usize = 200;

/// What the page says about one element: read from the DOM (tag,
/// attributes, the text of the control it belongs to) and the accessibility
/// tree (role and name).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ElementFacts {
    /// Lowercase tag name (`input`, `button`, ...).
    pub tag: String,
    /// Lowercase `type` attribute of an input.
    pub input_type: String,
    pub autocomplete: String,
    /// The accessibility role and name.
    pub role: String,
    pub name: String,
    /// aria-label, title, value and placeholder.
    pub labels: Vec<String>,
    /// The visible text of the nearest button, link or label around the
    /// element.
    pub text: String,
    /// The element takes typed text (text-like input, textarea,
    /// contenteditable).
    pub editable: bool,
}

/// Whether the agent may not type into this field.
pub fn is_credential_field(facts: &ElementFacts) -> bool {
    if facts.tag == "input" && facts.input_type == "password" {
        return true;
    }
    let autocomplete = facts.autocomplete.to_ascii_lowercase();
    if autocomplete
        .split_whitespace()
        .any(|token| CREDENTIAL_AUTOCOMPLETE.contains(&token) || token.starts_with("cc-exp"))
    {
        return true;
    }
    let names = std::iter::once(facts.name.as_str())
        .chain(facts.labels.iter().map(String::as_str))
        .map(normalize)
        .collect::<Vec<_>>();
    names.iter().any(|name| {
        CREDENTIAL_NAMES.iter().any(|term| {
            if term.is_ascii() {
                contains_words(name, term)
            } else {
                name.contains(term)
            }
        })
    })
}

/// The sensitive term a click on this element matches, if any.
pub fn sensitive_click(facts: &ElementFacts) -> Option<&'static str> {
    std::iter::once(facts.name.as_str())
        .chain(facts.labels.iter().map(String::as_str))
        .chain(std::iter::once(facts.text.as_str()))
        .find_map(sensitive_term)
}

/// The sensitive term `label` matches, if any.
pub fn sensitive_term(label: &str) -> Option<&'static str> {
    let text = normalize(label);
    if text.is_empty() {
        return None;
    }
    let text: String = text.chars().take(MAX_LABEL_CHARS).collect();
    if VIEW_ONLY_TERMS.iter().any(|term| {
        if term.is_ascii() {
            contains_words(&text, term)
        } else {
            text.contains(term)
        }
    }) {
        return None;
    }
    SENSITIVE_TERMS.iter().copied().find(|term| {
        if term.is_ascii() {
            contains_words(&text, term)
        } else {
            text.contains(term) || text.replace(' ', "").contains(&term.replace(' ', ""))
        }
    })
}

/// Lowercase with every whitespace run made one space.
fn normalize(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Whether `text` holds the words of `phrase` in order, each a whole word.
fn contains_words(text: &str, phrase: &str) -> bool {
    let words = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    let wanted = phrase.split(' ').collect::<Vec<_>>();
    !wanted.is_empty() && words.windows(wanted.len()).any(|window| window == wanted)
}

/// Most browser confirmations one session holds at once: the one shown and
/// the one waiting for it to settle (the session shows one card at a time).
/// A page that keeps raising more (a request per second, several tabs) has
/// the rest refused as not asked instead of filling the conversation with
/// questions.
pub const MAX_OPEN_CONFIRMATIONS: usize = 2;

/// The browser confirmations open now, by session.
#[derive(Debug, Default)]
pub struct OpenConfirmations {
    by_session: HashMap<String, usize>,
}

impl OpenConfirmations {
    /// Count one more open question for `session_id`; `false` (and nothing
    /// counted) when it already has [`MAX_OPEN_CONFIRMATIONS`].
    pub fn try_open(&mut self, session_id: &str) -> bool {
        let open = self.by_session.entry(session_id.to_string()).or_default();
        if *open >= MAX_OPEN_CONFIRMATIONS {
            return false;
        }
        *open += 1;
        true
    }

    pub fn close(&mut self, session_id: &str) {
        if let Some(open) = self.by_session.get_mut(session_id) {
            *open = open.saturating_sub(1);
            if *open == 0 {
                self.by_session.remove(session_id);
            }
        }
    }
}

/// What to do with one paused request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchDecision {
    /// Let it through now.
    Continue,
    /// Fail it: no page may load the app's own origins.
    Block,
    /// Fail it: an agent call may not load a page from `host`, this
    /// computer or the local network ([`super::agent_host_is_local`]).
    Internal { host: String },
}

/// Who is driving a tab while one of its requests is paused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Driver {
    /// No agent call: the user's own browsing.
    User,
    /// Only agent calls that read the page (snapshot, screenshot, wait,
    /// navigate) are running.
    Reading,
    /// An agent click, typing, key press or download is running, or one
    /// ended moments ago.
    Interacting,
}

/// Decide a paused request of `resource_type` (the DevTools `ResourceType`:
/// `Document`, `XHR`, `Fetch`, ...).
///
/// Submissions and data-sending requests are never held for the user (user
/// decision 2026-11-11: pages send analytics and CDN traffic all the time).
/// Only the app's own origins and, for an agent-driven document, local
/// addresses are refused.
pub fn fetch_decision(url: &str, resource_type: &str, driver: Driver) -> FetchDecision {
    let Ok(parsed) = tauri::Url::parse(url) else {
        return FetchDecision::Continue;
    };
    if !super::page_navigation_allowed(&parsed) {
        return FetchDecision::Block;
    }
    // Every redirect hop pauses again, so a public page that redirects an
    // agent's load into the local network is refused at that hop.
    if resource_type == "Document" && driver != Driver::User {
        if let Some(host) = super::agent_host_is_local(&parsed) {
            return FetchDecision::Internal { host };
        }
    }
    FetchDecision::Continue
}

/// The agent may click or type only on an ordinary web page: not on
/// `about:blank`, a browser error page, or anything internal.
pub fn interaction_allowed_on(url: &str) -> Result<(), String> {
    let refusal = || {
        format!(
            "이 탭은 지금 웹 페이지가 아닌 `{}` 을 보여 주고 있어 클릭·입력할 수 없습니다. browser_navigate 로 http(s) 페이지를 연 뒤 다시 시도하세요.",
            url.chars().take(80).collect::<String>()
        )
    };
    let parsed = tauri::Url::parse(url).map_err(|_| refusal())?;
    if matches!(parsed.scheme(), "http" | "https") && super::page_navigation_allowed(&parsed) {
        Ok(())
    } else {
        Err(refusal())
    }
}

/// What pressing a key on the focused element activates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyActivation {
    /// Nothing a click would: typing, moving, or no focused control.
    Nothing,
    /// The focused element itself, as a click would (button, link, checkbox).
    Element,
    /// The form of the focused text field, through its default button.
    FormDefault,
    /// A frame or plug-in has focus: the element inside it (another,
    /// possibly cross-origin document) cannot be checked, so the key is not
    /// pressed.
    Unknown,
}

/// The refusal of a key press whose target cannot be checked.
pub const KEY_TARGET_UNKNOWN: &str = "초점이 iframe 안(다른 문서)에 있거나 초점 요소를 확인할 수 없어 키를 누르지 않았습니다. browser_snapshot 으로 누를 요소의 ref 를 찾아 browser_click 으로 직접 누르세요.";

/// What `key` pressed on `focused` activates, so a key press is checked
/// like the click it amounts to ([`sensitive_click`]).
pub fn key_activation(key: KeyInfo, focused: &ElementFacts) -> KeyActivation {
    let enter = key.code == "Enter" || key.code == "NumpadEnter";
    if !enter && key.code != "Space" {
        return KeyActivation::Nothing;
    }
    if matches!(focused.tag.as_str(), "" | "body" | "html") {
        return KeyActivation::Nothing;
    }
    if matches!(
        focused.tag.as_str(),
        "iframe" | "frame" | "object" | "embed"
    ) {
        return KeyActivation::Unknown;
    }
    if focused.editable {
        return if enter && focused.tag == "input" {
            KeyActivation::FormDefault
        } else {
            KeyActivation::Nothing
        };
    }
    KeyActivation::Element
}

/// Longest run of popups [`PopupGate`] admits for one session within
/// [`POPUP_WINDOW`].
pub const MAX_POPUPS_PER_WINDOW: usize = 3;
pub const POPUP_WINDOW: std::time::Duration = std::time::Duration::from_secs(10);
/// Most tabs one session may have open; a popup beyond it is not opened and
/// `browser_open` is refused.
pub const MAX_TABS_PER_SESSION: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupRefusal {
    /// Neither a person's gesture nor an agent call opened it.
    NotInitiated,
    /// The session opened [`MAX_POPUPS_PER_WINDOW`] popups just now.
    TooFast,
    /// The session already has [`MAX_TABS_PER_SESSION`] tabs.
    TooManyTabs,
}

/// Which page popups become tabs: only those a person's gesture or an agent
/// call opened, a few at a time, up to the session's tab limit. A page that
/// opens windows on its own gets none.
#[derive(Debug, Default)]
pub struct PopupGate {
    recent: HashMap<String, std::collections::VecDeque<std::time::Instant>>,
}

impl PopupGate {
    pub fn admit(
        &mut self,
        session_id: &str,
        user_initiated: bool,
        agent_driven: bool,
        open_tabs: usize,
        now: std::time::Instant,
    ) -> Result<(), PopupRefusal> {
        if !user_initiated && !agent_driven {
            return Err(PopupRefusal::NotInitiated);
        }
        if open_tabs >= MAX_TABS_PER_SESSION {
            return Err(PopupRefusal::TooManyTabs);
        }
        let recent = self.recent.entry(session_id.to_string()).or_default();
        while recent
            .front()
            .is_some_and(|opened| now.saturating_duration_since(*opened) >= POPUP_WINDOW)
        {
            recent.pop_front();
        }
        if recent.len() >= MAX_POPUPS_PER_WINDOW {
            return Err(PopupRefusal::TooFast);
        }
        recent.push_back(now);
        Ok(())
    }
}

/// A key `browser_press` may press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyInfo {
    pub key: &'static str,
    pub code: &'static str,
    pub windows_virtual_key_code: u32,
    /// The text the key produces (`keyDown`); `None` sends a `rawKeyDown`.
    pub text: Option<&'static str>,
}

/// Every key `browser_press` accepts, by its name.
pub const KEYS: &[(&str, KeyInfo)] = &[
    ("Enter", key("Enter", "Enter", 13, Some("\r"))),
    ("Tab", key("Tab", "Tab", 9, None)),
    ("Escape", key("Escape", "Escape", 27, None)),
    ("Backspace", key("Backspace", "Backspace", 8, None)),
    ("Delete", key("Delete", "Delete", 46, None)),
    ("ArrowUp", key("ArrowUp", "ArrowUp", 38, None)),
    ("ArrowDown", key("ArrowDown", "ArrowDown", 40, None)),
    ("ArrowLeft", key("ArrowLeft", "ArrowLeft", 37, None)),
    ("ArrowRight", key("ArrowRight", "ArrowRight", 39, None)),
    ("PageUp", key("PageUp", "PageUp", 33, None)),
    ("PageDown", key("PageDown", "PageDown", 34, None)),
    ("Home", key("Home", "Home", 36, None)),
    ("End", key("End", "End", 35, None)),
    ("Space", key(" ", "Space", 32, Some(" "))),
];

const fn key(
    key: &'static str,
    code: &'static str,
    windows_virtual_key_code: u32,
    text: Option<&'static str>,
) -> KeyInfo {
    KeyInfo {
        key,
        code,
        windows_virtual_key_code,
        text,
    }
}

pub fn key_info(name: &str) -> Option<KeyInfo> {
    KEYS.iter()
        .find(|(known, _)| *known == name)
        .map(|(_, info)| *info)
}

/// The names [`KEYS`] accepts, for schemas and errors.
pub fn key_names() -> Vec<&'static str> {
    KEYS.iter().map(|(name, _)| *name).collect()
}

/// The `Input.dispatchKeyEvent` parameters pressing `info` sends: the down
/// event, then the up event.
pub fn key_events(info: KeyInfo, modifiers: u32) -> [serde_json::Value; 2] {
    let mut down = serde_json::json!({
        "type": if info.text.is_some() { "keyDown" } else { "rawKeyDown" },
        "key": info.key,
        "code": info.code,
        "windowsVirtualKeyCode": info.windows_virtual_key_code,
        "nativeVirtualKeyCode": info.windows_virtual_key_code,
        "modifiers": modifiers,
    });
    if let Some(text) = info.text {
        down["text"] = text.into();
        down["unmodifiedText"] = text.into();
    }
    let up = serde_json::json!({
        "type": "keyUp",
        "key": info.key,
        "code": info.code,
        "windowsVirtualKeyCode": info.windows_virtual_key_code,
        "nativeVirtualKeyCode": info.windows_virtual_key_code,
        "modifiers": modifiers,
    });
    [down, up]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(tag: &str, input_type: &str) -> ElementFacts {
        ElementFacts {
            tag: tag.into(),
            input_type: input_type.into(),
            role: "textbox".into(),
            editable: true,
            ..ElementFacts::default()
        }
    }

    #[test]
    fn credential_fields_are_recognized_by_type_autocomplete_and_name() {
        assert!(is_credential_field(&field("input", "password")));
        for autocomplete in [
            "current-password",
            "new-password",
            "one-time-code",
            "cc-number",
            "cc-csc",
            "cc-name",
            "cc-exp",
            "cc-exp-month",
            "section-pay billing CC-NUMBER",
        ] {
            let mut facts = field("input", "text");
            facts.autocomplete = autocomplete.into();
            assert!(is_credential_field(&facts), "{autocomplete}");
        }
        for name in [
            "비밀번호",
            "비밀번호 확인",
            "Password",
            "Enter your password",
            "카드 번호",
            "카드번호 16자리",
            "CVC",
            "보안코드 3자리",
        ] {
            let mut facts = field("input", "text");
            facts.name = name.into();
            assert!(is_credential_field(&facts), "{name}");
            let mut facts = field("input", "text");
            facts.labels = vec![name.into()];
            assert!(is_credential_field(&facts), "label {name}");
        }
        for ordinary in [
            "검색어",
            "이메일",
            "Username",
            "프롬프트",
            "효과음 설명",
            "",
        ] {
            let mut facts = field("input", "text");
            facts.name = ordinary.into();
            facts.autocomplete = "username email".into();
            assert!(!is_credential_field(&facts), "{ordinary}");
        }
        let mut facts = field("textarea", "");
        facts.name = "passwordless sign-in tips".into();
        assert!(!is_credential_field(&facts), "whole words only");
    }

    #[test]
    fn sensitive_clicks_match_the_explicit_list_and_nothing_ordinary() {
        for label in [
            "결제하기",
            "지금 결제",
            "구매",
            "바로 구매하기",
            "주문하기",
            "구독 시작",
            "정기결제 신청",
            "포인트 충전",
            "회원탈퇴",
            "회원 탈퇴",
            "계정 삭제",
            "삭제",
            "비밀번호 변경",
            "이메일 변경",
            "서비스 해지",
            "Pay now",
            "Purchase",
            "Buy credits",
            "Checkout",
            "Place order",
            "Subscribe",
            "Upgrade plan",
            "Delete account",
            "Remove account",
            "Change password",
            "Cancel subscription",
            "Top up",
            "  PAY  ",
        ] {
            assert!(sensitive_term(label).is_some(), "{label}");
        }
        for label in [
            "생성",
            "효과음 생성하기",
            "다운로드",
            "검색",
            "로그인",
            "Log in",
            "Sign in",
            "Generate",
            "Download",
            "Search",
            "다음",
            "확인",
            "Border color",
            "Buyer guide",
            "Payment methods help",
            "주문 내역",
            "결제 내역 조회",
            "Order history",
            "",
        ] {
            assert_eq!(sensitive_term(label), None, "{label}");
        }
    }

    #[test]
    fn a_click_is_sensitive_by_name_label_or_surrounding_text() {
        let mut facts = ElementFacts {
            tag: "span".into(),
            ..ElementFacts::default()
        };
        assert_eq!(sensitive_click(&facts), None);
        facts.text = "결제하기".into();
        assert_eq!(sensitive_click(&facts), Some("결제"));
        let facts = ElementFacts {
            tag: "input".into(),
            input_type: "submit".into(),
            labels: vec!["Buy now".into()],
            ..ElementFacts::default()
        };
        assert_eq!(sensitive_click(&facts), Some("buy"));
        let facts = ElementFacts {
            role: "button".into(),
            name: "생성".into(),
            text: "생성".into(),
            ..ElementFacts::default()
        };
        assert_eq!(sensitive_click(&facts), None);
    }

    #[test]
    fn fetch_decisions_never_hold_submissions() {
        // Submissions and data-sending requests continue, whoever drives the
        // tab, whatever the method or domain.
        for url in [
            "https://forms.unknown.org/submit",
            "https://www.example.com/api",
        ] {
            for driver in [Driver::User, Driver::Reading, Driver::Interacting] {
                for kind in ["Document", "XHR", "Fetch", "Ping", "Image"] {
                    assert_eq!(
                        fetch_decision(url, kind, driver),
                        FetchDecision::Continue,
                        "{driver:?} {kind} {url}"
                    );
                }
            }
        }
        // The app's own origins never load, for anyone.
        for url in [
            "http://tauri.localhost/index.html",
            "https://ipc.localhost/x",
            "tauri://localhost/",
        ] {
            for driver in [Driver::User, Driver::Reading, Driver::Interacting] {
                for kind in ["Document", "XHR", "Fetch"] {
                    assert_eq!(
                        fetch_decision(url, kind, driver),
                        FetchDecision::Block,
                        "{url}"
                    );
                }
            }
        }
        assert_eq!(
            fetch_decision("not a url", "Document", Driver::Interacting),
            FetchDecision::Continue
        );
    }

    #[test]
    fn agent_driven_documents_never_load_internal_hosts() {
        for url in [
            "http://192.168.0.1/admin",
            "http://127.0.0.1:8080/",
            "http://[::1]/",
            "http://router/",
            "http://nas.local/",
            "http://127.0.0.1.nip.io/",
            "http://app.10.0.0.1.sslip.io/",
            "http://localtest.me:3000/",
            "http://x.lvh.me/",
        ] {
            for driver in [Driver::Reading, Driver::Interacting] {
                // Every redirect hop pauses again, so a public page's 302
                // into the network is refused the same way.
                assert!(
                    matches!(
                        fetch_decision(url, "Document", driver),
                        FetchDecision::Internal { .. }
                    ),
                    "{url} {driver:?}"
                );
            }
            // The user's own browsing is never restricted.
            assert_eq!(
                fetch_decision(url, "Document", Driver::User),
                FetchDecision::Continue,
                "{url}"
            );
            // A page's own script traffic is not a page the agent opened.
            assert_eq!(
                fetch_decision(url, "Fetch", Driver::Interacting),
                FetchDecision::Continue,
                "{url}"
            );
        }
        assert_eq!(
            fetch_decision("https://example.com/", "Document", Driver::Reading),
            FetchDecision::Continue
        );
    }

    fn facts(tag: &str, input_type: &str, editable: bool) -> ElementFacts {
        ElementFacts {
            tag: tag.into(),
            input_type: input_type.into(),
            editable,
            ..ElementFacts::default()
        }
    }

    #[test]
    fn enter_and_space_activate_what_has_focus() {
        let enter = key_info("Enter").unwrap();
        let space = key_info("Space").unwrap();
        let tab = key_info("Tab").unwrap();
        let button = facts("button", "", false);
        let link = facts("a", "", false);
        let checkbox = facts("input", "checkbox", false);
        let text = facts("input", "text", true);
        let area = facts("textarea", "", true);
        let editable = facts("div", "", true);
        for element in [&button, &link, &checkbox] {
            assert_eq!(key_activation(enter, element), KeyActivation::Element);
            assert_eq!(key_activation(space, element), KeyActivation::Element);
        }
        // Enter in a text field submits its form through the default button.
        assert_eq!(key_activation(enter, &text), KeyActivation::FormDefault);
        assert_eq!(key_activation(space, &text), KeyActivation::Nothing);
        for element in [&area, &editable] {
            assert_eq!(key_activation(enter, element), KeyActivation::Nothing);
            assert_eq!(key_activation(space, element), KeyActivation::Nothing);
        }
        // Nothing focused: the page body takes the key and activates nothing.
        for tag in ["body", "html", ""] {
            assert_eq!(
                key_activation(enter, &facts(tag, "", false)),
                KeyActivation::Nothing
            );
        }
        assert_eq!(key_activation(tab, &button), KeyActivation::Nothing);
        // A frame or plug-in holds focus for a document the app cannot read
        // (cross-origin): what the key would activate is unknown.
        for tag in ["iframe", "frame", "object", "embed"] {
            for key in [enter, space] {
                assert_eq!(
                    key_activation(key, &facts(tag, "", false)),
                    KeyActivation::Unknown,
                    "{tag}"
                );
            }
            assert_eq!(
                key_activation(tab, &facts(tag, "", false)),
                KeyActivation::Nothing
            );
        }
    }

    #[test]
    fn a_session_has_at_most_two_confirmations_open() {
        let mut open = OpenConfirmations::default();
        assert!(open.try_open("s1"));
        assert!(open.try_open("s1"));
        assert!(!open.try_open("s1"), "a third question is refused at once");
        assert!(open.try_open("s2"), "per session");
        open.close("s1");
        assert!(open.try_open("s1"));
        open.close("s1");
        open.close("s1");
        open.close("s1");
        assert!(open.try_open("s1"));
        assert!(open.try_open("s1"));
        assert!(!open.try_open("s1"));
    }

    #[test]
    fn popups_open_only_for_a_person_or_an_agent_and_never_flood() {
        let start = std::time::Instant::now();
        let mut gate = PopupGate::default();
        assert_eq!(
            gate.admit("s1", false, false, 0, start),
            Err(PopupRefusal::NotInitiated)
        );
        for (index, user) in [true, false, true].into_iter().enumerate() {
            assert_eq!(gate.admit("s1", user, !user, index, start), Ok(()));
        }
        assert_eq!(
            gate.admit("s1", true, false, 3, start),
            Err(PopupRefusal::TooFast)
        );
        // Another session has its own budget.
        assert_eq!(gate.admit("s2", true, false, 0, start), Ok(()));
        let later = start + POPUP_WINDOW;
        assert_eq!(gate.admit("s1", true, false, 3, later), Ok(()));
        assert_eq!(
            gate.admit(
                "s1",
                true,
                false,
                MAX_TABS_PER_SESSION,
                later + POPUP_WINDOW
            ),
            Err(PopupRefusal::TooManyTabs)
        );
    }

    #[test]
    fn only_web_pages_take_clicks_and_typing() {
        assert!(interaction_allowed_on("https://example.com/a").is_ok());
        assert!(interaction_allowed_on("http://127.0.0.1:8080/").is_ok());
        for url in [
            "about:blank",
            "chrome-error://chromewebdata/",
            "edge://settings",
            "http://tauri.localhost/",
            "file:///C:/x",
            "",
        ] {
            let error = interaction_allowed_on(url).unwrap_err();
            assert!(error.contains("browser_navigate"), "{url}: {error}");
        }
    }

    #[test]
    fn keys_carry_their_codes_and_text() {
        assert_eq!(key_names().len(), 14);
        let enter = key_info("Enter").unwrap();
        let [down, up] = key_events(enter, 0);
        assert_eq!(down["type"], "keyDown");
        assert_eq!(down["text"], "\r");
        assert_eq!(down["windowsVirtualKeyCode"], 13);
        assert_eq!(up["type"], "keyUp");
        assert!(up.get("text").is_none());
        let [down, _] = key_events(key_info("ArrowDown").unwrap(), 0);
        assert_eq!(down["type"], "rawKeyDown");
        assert_eq!(down["code"], "ArrowDown");
        assert_eq!(down["windowsVirtualKeyCode"], 40);
        let space = key_info("Space").unwrap();
        assert_eq!((space.key, space.text), (" ", Some(" ")));
        assert_eq!(key_info("F5"), None);
        assert_eq!(key_info("enter"), None, "names are exact");
    }
}
