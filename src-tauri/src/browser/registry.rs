//! Which tabs exist, who owns them, and what the chrome shows — kept apart
//! from the webviews so the ownership rules are testable without a window.

use std::collections::{BTreeMap, BTreeSet};

use super::{BrowserState, BrowserTabView, TAB_LABEL_PREFIX};

/// The refusal for a tab this session cannot see.
pub const TAB_NOT_FOUND: &str =
    "탭을 찾을 수 없습니다. browser_tabs 로 이 세션의 탭을 확인하거나 browser_open 으로 새 탭을 여세요.";
/// The refusal for a tab the user closed in the browser window.
pub const TAB_CLOSED_BY_USER: &str =
    "사용자가 탭을 닫았습니다. 필요하면 browser_open 으로 다시 여세요.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabRecord {
    pub tab_id: String,
    pub label: String,
    pub session_id: String,
    pub project_id: String,
    pub url: String,
    pub title: String,
    /// `browser_snapshot` refs (`e1`...) → backendDOMNodeId, valid until the
    /// tab navigates.
    pub refs: BTreeMap<String, i64>,
    /// Agent tool calls currently running on this tab.
    pub busy: u32,
    /// Agent actions on this tab waiting for the user's confirmation.
    pub confirming: u32,
}

#[derive(Debug, Default)]
pub struct TabRegistry {
    next: u64,
    /// Creation order: the tab strip's order.
    tabs: Vec<TabRecord>,
    active: Option<String>,
    /// `(session, tab)` pairs the user closed, so the agent hears why.
    closed_by_user: BTreeSet<(String, String)>,
}

impl TabRegistry {
    /// Register a new tab of `session_id`; it becomes the active tab.
    pub fn insert(&mut self, session_id: &str, project_id: &str) -> TabRecord {
        self.next += 1;
        let record = TabRecord {
            tab_id: format!("t{}", self.next),
            label: format!("{TAB_LABEL_PREFIX}{}", self.next),
            session_id: session_id.to_string(),
            project_id: project_id.to_string(),
            url: "about:blank".to_string(),
            title: String::new(),
            refs: BTreeMap::new(),
            busy: 0,
            confirming: 0,
        };
        self.tabs.push(record.clone());
        self.active = Some(record.tab_id.clone());
        record
    }

    /// Tab `tab_id` of `session_id`. Another session's tab is reported
    /// exactly like a tab that does not exist.
    pub fn owned(&self, session_id: &str, tab_id: &str) -> Result<&TabRecord, String> {
        let tab_id = tab_id.trim();
        if let Some(tab) = self
            .tabs
            .iter()
            .find(|tab| tab.tab_id == tab_id && tab.session_id == session_id)
        {
            return Ok(tab);
        }
        if self
            .closed_by_user
            .contains(&(session_id.to_string(), tab_id.to_string()))
        {
            Err(TAB_CLOSED_BY_USER.to_string())
        } else {
            Err(TAB_NOT_FOUND.to_string())
        }
    }

    pub fn get(&self, tab_id: &str) -> Option<&TabRecord> {
        self.tabs.iter().find(|tab| tab.tab_id == tab_id)
    }

    pub fn by_label_mut(&mut self, label: &str) -> Option<&mut TabRecord> {
        self.tabs.iter_mut().find(|tab| tab.label == label)
    }

    fn get_mut(&mut self, tab_id: &str) -> Option<&mut TabRecord> {
        self.tabs.iter_mut().find(|tab| tab.tab_id == tab_id)
    }

    /// `session_id`'s tabs, in strip order.
    pub fn of_session(&self, session_id: &str) -> Vec<&TabRecord> {
        self.tabs
            .iter()
            .filter(|tab| tab.session_id == session_id)
            .collect()
    }

    pub fn all(&self) -> &[TabRecord] {
        &self.tabs
    }

    pub fn active(&self) -> Option<&str> {
        self.active.as_deref()
    }

    pub fn activate(&mut self, tab_id: &str) -> bool {
        if self.get(tab_id).is_none() {
            return false;
        }
        self.active = Some(tab_id.to_string());
        true
    }

    /// Remove tab `tab_id`. When it was active, the tab next to it (the one
    /// after it, else the one before) becomes active.
    pub fn remove(&mut self, tab_id: &str, by_user: bool) -> Option<TabRecord> {
        let index = self.tabs.iter().position(|tab| tab.tab_id == tab_id)?;
        let removed = self.tabs.remove(index);
        if self.active.as_deref() == Some(tab_id) {
            self.active = self
                .tabs
                .get(index)
                .or_else(|| {
                    index
                        .checked_sub(1)
                        .and_then(|before| self.tabs.get(before))
                })
                .map(|tab| tab.tab_id.clone());
        }
        if by_user {
            self.closed_by_user
                .insert((removed.session_id.clone(), removed.tab_id.clone()));
        }
        Some(removed)
    }

    /// Remove every tab matching `filter`; returns them.
    pub fn remove_where(&mut self, filter: impl Fn(&TabRecord) -> bool) -> Vec<TabRecord> {
        let ids = self
            .tabs
            .iter()
            .filter(|tab| filter(tab))
            .map(|tab| tab.tab_id.clone())
            .collect::<Vec<_>>();
        ids.iter().filter_map(|id| self.remove(id, false)).collect()
    }

    pub fn set_page(&mut self, tab_id: &str, url: Option<String>, title: Option<String>) {
        if let Some(tab) = self.get_mut(tab_id) {
            if let Some(url) = url {
                tab.url = url;
            }
            if let Some(title) = title {
                tab.title = title;
            }
        }
    }

    pub fn set_refs(&mut self, tab_id: &str, refs: BTreeMap<String, i64>) {
        if let Some(tab) = self.get_mut(tab_id) {
            tab.refs = refs;
        }
    }

    /// The tab navigated: its snapshot refs no longer name its elements.
    pub fn invalidate_refs(&mut self, tab_id: &str) {
        if let Some(tab) = self.get_mut(tab_id) {
            tab.refs.clear();
        }
    }

    pub fn mark_busy(&mut self, tab_id: &str, busy: bool) {
        if let Some(tab) = self.get_mut(tab_id) {
            tab.busy = if busy {
                tab.busy.saturating_add(1)
            } else {
                tab.busy.saturating_sub(1)
            };
        }
    }

    pub fn mark_confirming(&mut self, tab_id: &str, confirming: bool) {
        if let Some(tab) = self.get_mut(tab_id) {
            tab.confirming = if confirming {
                tab.confirming.saturating_add(1)
            } else {
                tab.confirming.saturating_sub(1)
            };
        }
    }

    /// The chrome's view; `title_of` names each tab's session.
    pub fn view(&self, title_of: impl Fn(&str) -> Option<String>) -> BrowserState {
        BrowserState {
            tabs: self
                .tabs
                .iter()
                .map(|tab| BrowserTabView {
                    tab_id: tab.tab_id.clone(),
                    session_id: tab.session_id.clone(),
                    session_title: title_of(&tab.session_id).unwrap_or_else(|| "세션".to_string()),
                    url: tab.url.clone(),
                    title: tab.title.clone(),
                    active: self.active.as_deref() == Some(tab.tab_id.as_str()),
                    agent_busy: tab.busy > 0,
                    awaiting_confirmation: tab.confirming > 0,
                })
                .collect(),
            active_tab_id: self.active.clone(),
            downloads: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_sees_only_its_own_tabs() {
        let mut registry = TabRegistry::default();
        let a = registry.insert("session-a", "project-1");
        let b = registry.insert("session-b", "project-1");
        assert_eq!(a.tab_id, "t1");
        assert_eq!(a.label, "browser-tab-1");
        assert_eq!(b.tab_id, "t2");

        assert_eq!(registry.owned("session-a", "t1").unwrap().tab_id, "t1");
        assert_eq!(registry.owned("session-a", " t1 ").unwrap().tab_id, "t1");
        assert_eq!(
            registry.owned("session-a", "t2").unwrap_err(),
            TAB_NOT_FOUND,
            "another session's tab looks like no tab"
        );
        assert_eq!(
            registry.owned("session-a", "t9").unwrap_err(),
            TAB_NOT_FOUND
        );
        let mine = registry
            .of_session("session-b")
            .into_iter()
            .map(|tab| tab.tab_id.clone())
            .collect::<Vec<_>>();
        assert_eq!(mine, ["t2"]);
    }

    #[test]
    fn a_tab_the_user_closed_says_so_to_its_session_only() {
        let mut registry = TabRegistry::default();
        registry.insert("session-a", "p");
        registry.remove("t1", true);
        assert_eq!(
            registry.owned("session-a", "t1").unwrap_err(),
            TAB_CLOSED_BY_USER
        );
        assert_eq!(
            registry.owned("session-b", "t1").unwrap_err(),
            TAB_NOT_FOUND
        );
        registry.insert("session-a", "p");
        registry.remove("t2", false);
        assert_eq!(
            registry.owned("session-a", "t2").unwrap_err(),
            TAB_NOT_FOUND
        );
    }

    #[test]
    fn closing_the_active_tab_activates_its_neighbour() {
        let mut registry = TabRegistry::default();
        for _ in 0..3 {
            registry.insert("s", "p");
        }
        assert_eq!(registry.active(), Some("t3"));
        assert!(registry.activate("t2"));
        registry.remove("t2", false);
        assert_eq!(registry.active(), Some("t3"));
        registry.remove("t3", false);
        assert_eq!(registry.active(), Some("t1"));
        registry.remove("t1", false);
        assert_eq!(registry.active(), None);
        assert!(!registry.activate("t1"));
    }

    #[test]
    fn project_and_session_closes_remove_only_their_tabs() {
        let mut registry = TabRegistry::default();
        registry.insert("a", "p1");
        registry.insert("b", "p2");
        registry.insert("a", "p1");
        let removed = registry.remove_where(|tab| tab.project_id == "p1");
        assert_eq!(removed.len(), 2);
        assert_eq!(registry.all().len(), 1);
        assert_eq!(registry.active(), Some("t2"));
        let removed = registry.remove_where(|tab| tab.session_id == "b");
        assert_eq!(removed[0].tab_id, "t2");
        assert!(registry.all().is_empty());
    }

    #[test]
    fn navigation_invalidates_refs_and_busy_shows_in_the_view() {
        let mut registry = TabRegistry::default();
        registry.insert("s", "p");
        registry.set_refs("t1", BTreeMap::from([("e1".to_string(), 7)]));
        registry.set_page(
            "t1",
            Some("https://example.com/".into()),
            Some("Example".into()),
        );
        registry.mark_busy("t1", true);
        let view = registry.view(|session| (session == "s").then(|| "내 세션".to_string()));
        assert_eq!(view.tabs[0].session_title, "내 세션");
        assert_eq!(view.tabs[0].title, "Example");
        assert!(view.tabs[0].agent_busy);
        assert!(view.tabs[0].active);
        assert_eq!(view.active_tab_id.as_deref(), Some("t1"));
        registry.mark_busy("t1", false);
        registry.mark_busy("t1", false);
        assert!(!registry.view(|_| None).tabs[0].agent_busy);
        assert!(!registry.view(|_| None).tabs[0].awaiting_confirmation);
        registry.mark_confirming("t1", true);
        assert!(registry.view(|_| None).tabs[0].awaiting_confirmation);
        registry.mark_confirming("t1", false);
        registry.mark_confirming("t1", false);
        assert!(!registry.view(|_| None).tabs[0].awaiting_confirmation);
        assert_eq!(registry.get("t1").unwrap().refs.len(), 1);
        registry.invalidate_refs("t1");
        assert!(registry.get("t1").unwrap().refs.is_empty());
    }
}
