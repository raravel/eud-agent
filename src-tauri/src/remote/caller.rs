//! Who a remote request comes from, and which project it works on.
//!
//! Every desktop command resolves its project from the calling window's label
//! through [`ProjectRegistry::for_window`]. A phone has no window: each event
//! socket gets a virtual label `remote-<device>-<conn>`, and attaching the
//! socket to a project binds that label in the registry like a window, so the
//! same commands resolve the same project. A remote label is never a project
//! window: it has no webview, starts with neither `main` nor a companion
//! prefix, and is skipped by the per-window event fanout.
//!
//! A project has at most one desktop window plus any number of remote
//! observers. A phone attaches to an open project, or opens one of the
//! recent projects itself (see `remote_open_project`); the binding goes when
//! the socket closes or detaches, while the project stays open so a phone
//! that dropped can attach again. A project that closes drops every binding.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use serde::Serialize;

use crate::project_id::ProjectId;
use crate::project_registry::{ProjectHandle, ProjectRegistry};

pub const REMOTE_LABEL_PREFIX: &str = "remote-";

pub const PROJECT_NOT_OPEN: &str =
    "이 프로젝트는 열려 있지 않습니다. 프로젝트 목록을 새로 고친 뒤 다시 열어 주세요.";
pub const PROJECT_CLOSED: &str = "프로젝트가 닫혔습니다. 프로젝트 목록에서 다시 열어 주세요.";
pub const NOT_RECENT: &str =
    "최근 프로젝트 목록에 없는 경로입니다. 프로젝트 목록을 새로 고친 뒤 다시 선택해 주세요.";
pub const UNKNOWN_CONNECTION: &str =
    "PC와의 연결을 찾을 수 없습니다. 페이지를 새로 고친 뒤 다시 시도해 주세요.";

/// Whether `label` names a remote connection rather than a window.
pub fn is_remote_label(label: &str) -> bool {
    label.starts_with(REMOTE_LABEL_PREFIX)
}

/// One remote event socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteCaller {
    pub conn_id: String,
    pub device_id: String,
    /// The virtual window label commands resolve the project from.
    pub label: String,
}

/// An open project a remote client can attach to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteProject {
    pub project_id: String,
    pub name: String,
    pub root: String,
}

impl RemoteProject {
    fn of(handle: &ProjectHandle) -> Self {
        let root = dunce::simplified(handle.root()).to_path_buf();
        let name = handle.live_name().unwrap_or_else(|_| {
            root.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
        Self {
            project_id: handle.id().as_str().to_string(),
            name,
            root: root.to_string_lossy().into_owned(),
        }
    }
}

/// One row of the phone's project screen: an open project, a recent one, or
/// both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteProjectEntry {
    /// The recent-list path when the project is recent, else its root.
    pub path: String,
    pub name: String,
    pub open: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// A desktop window shows the project.
    pub desktop_window: bool,
    /// The project can be opened (its manifest is readable).
    pub available: bool,
}

/// The phone's project list: the open projects by name, then the recent
/// projects that are not open, newest first. `id_of` names the native
/// project at a recent path (`None` when it cannot be opened).
pub fn project_entries(
    registry: &ProjectRegistry,
    recents: &[crate::setup::RecentProject],
    id_of: impl Fn(&str) -> Option<ProjectId>,
) -> Vec<RemoteProjectEntry> {
    let recent_ids = recents
        .iter()
        .map(|recent| {
            if recent.available {
                id_of(&recent.path)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    let mut open = registry
        .handles()
        .into_iter()
        .filter(|handle| !handle.is_closed())
        .map(|handle| {
            let project = RemoteProject::of(&handle);
            let path = recents
                .iter()
                .zip(&recent_ids)
                .find(|(_, id)| id.as_ref() == Some(handle.id()))
                .map_or_else(|| project.root.clone(), |(recent, _)| recent.path.clone());
            RemoteProjectEntry {
                path,
                name: project.name,
                open: true,
                project_id: Some(project.project_id),
                desktop_window: registry.desktop_window(handle.id()).is_some(),
                available: true,
            }
        })
        .collect::<Vec<_>>();
    open.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.project_id.cmp(&right.project_id))
    });
    let closed = recents
        .iter()
        .zip(recent_ids)
        .filter(|(_, id)| !id.as_ref().is_some_and(|id| registry.get(id).is_some()))
        .map(|(recent, _)| RemoteProjectEntry {
            path: recent.path.clone(),
            name: recent.name.clone(),
            open: false,
            project_id: None,
            desktop_window: false,
            available: recent.available,
        })
        .collect::<Vec<_>>();
    open.into_iter().chain(closed).collect()
}

/// The stored recent path `path` names. A phone opens only a project the PC
/// already opened explicitly, never an arbitrary path.
pub fn recent_path(config: &crate::config::Config, path: &str) -> Result<String, String> {
    let key = crate::config::RecentProjectRecord::key(path);
    config
        .project_recents
        .iter()
        .find(|recent| crate::config::RecentProjectRecord::key(&recent.path) == key)
        .map(|recent| recent.path.clone())
        .ok_or_else(|| NOT_RECENT.to_string())
}

struct Connection {
    caller: RemoteCaller,
    attached: Option<ProjectId>,
}

/// The live remote sockets and their project bindings.
pub struct RemoteConnections {
    registry: ProjectRegistry,
    connections: Mutex<HashMap<String, Connection>>,
}

fn random_hex(bytes: usize) -> String {
    let mut buffer = vec![0u8; bytes];
    getrandom::fill(&mut buffer).expect("OS random source");
    buffer.iter().map(|byte| format!("{byte:02x}")).collect()
}

impl RemoteConnections {
    pub fn new(registry: ProjectRegistry) -> Arc<Self> {
        Arc::new(Self {
            registry,
            connections: Mutex::new(HashMap::new()),
        })
    }

    pub fn registry(&self) -> &ProjectRegistry {
        &self.registry
    }

    /// Register a new socket of `device_id`.
    pub fn open(&self, device_id: &str) -> RemoteCaller {
        let conn_id = random_hex(8);
        let device: String = device_id
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .take(8)
            .collect();
        let caller = RemoteCaller {
            label: format!("{REMOTE_LABEL_PREFIX}{device}-{conn_id}"),
            conn_id: conn_id.clone(),
            device_id: device_id.to_string(),
        };
        self.connections.lock().insert(
            conn_id,
            Connection {
                caller: caller.clone(),
                attached: None,
            },
        );
        caller
    }

    /// The socket `conn_id`, when it belongs to `device_id`: a request never
    /// borrows another device's socket.
    pub fn caller(&self, conn_id: &str, device_id: &str) -> Option<RemoteCaller> {
        self.connections
            .lock()
            .get(conn_id)
            .filter(|connection| connection.caller.device_id == device_id)
            .map(|connection| connection.caller.clone())
    }

    /// Bind socket `conn_id` to the open project `project_id`.
    pub fn attach(&self, conn_id: &str, project_id: &str) -> Result<RemoteProject, String> {
        let id = ProjectId::from_stored(project_id);
        let handle = self
            .registry
            .get(&id)
            .filter(|handle| !handle.is_closed())
            .ok_or_else(|| PROJECT_NOT_OPEN.to_string())?;
        let mut connections = self.connections.lock();
        let connection = connections
            .get_mut(conn_id)
            .ok_or_else(|| UNKNOWN_CONNECTION.to_string())?;
        self.registry.bind_window(&connection.caller.label, &id);
        connection.attached = Some(id);
        Ok(RemoteProject::of(&handle))
    }

    /// Unbind socket `conn_id` from project `id`, when it is attached to it.
    pub fn detach_from(&self, conn_id: &str, id: &ProjectId) {
        if self.attached(conn_id).as_ref() == Some(id) {
            self.detach(conn_id);
        }
    }

    /// Unbind socket `conn_id` from its project. The project stays open.
    pub fn detach(&self, conn_id: &str) {
        if let Some(connection) = self.connections.lock().get_mut(conn_id) {
            if connection.attached.take().is_some() {
                self.registry.unbind_window(&connection.caller.label);
            }
        }
    }

    /// The project socket `conn_id` works on. A project closed on the desktop
    /// dropped the binding: the socket is detached from it here.
    pub fn attached(&self, conn_id: &str) -> Option<ProjectId> {
        let mut connections = self.connections.lock();
        let connection = connections.get_mut(conn_id)?;
        let attached = connection.attached.clone()?;
        match self.registry.for_window(&connection.caller.label) {
            Ok(handle) if handle.id() == &attached => Some(attached),
            _ => {
                connection.attached = None;
                None
            }
        }
    }

    /// Forget socket `conn_id`, dropping its binding. The project stays open,
    /// even with no window or client left, so the phone can attach again.
    pub fn close(&self, conn_id: &str) {
        if let Some(connection) = self.connections.lock().remove(conn_id) {
            if connection.attached.is_some() {
                self.registry.unbind_window(&connection.caller.label);
            }
        }
    }

    /// The open projects, by name.
    pub fn open_projects(&self) -> Vec<RemoteProject> {
        let mut projects = self
            .registry
            .handles()
            .iter()
            .filter(|handle| !handle.is_closed())
            .map(|handle| RemoteProject::of(handle))
            .collect::<Vec<_>>();
        projects.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.project_id.cmp(&right.project_id))
        });
        projects
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DataDirs;
    use crate::project_id::test_support::{create_project, temp_root};

    fn registry(label: &str) -> (std::path::PathBuf, ProjectRegistry) {
        let temp = temp_root(label);
        let dirs = DataDirs::from_bases(&temp.join("roaming"), &temp.join("local"));
        (temp, ProjectRegistry::new(dirs))
    }

    #[test]
    fn attaching_binds_the_virtual_label_like_a_window() {
        let (temp, registry) = registry("remote-attach");
        let project = create_project(&temp.join("p"), "Sample");
        let handle = registry.register(&project).unwrap();
        let connections = RemoteConnections::new(registry.clone());
        let caller = connections.open("abcdef0123456789");
        assert!(caller.label.starts_with("remote-abcdef01-"));
        assert!(is_remote_label(&caller.label));
        assert!(!crate::project_launch::is_project_window(&caller.label));

        assert!(registry.for_window(&caller.label).is_err());
        let attached = connections
            .attach(&caller.conn_id, handle.id().as_str())
            .unwrap();
        assert_eq!(attached.name, "Sample");
        assert_eq!(
            registry.for_window(&caller.label).unwrap().id(),
            handle.id()
        );
        assert_eq!(
            connections.attached(&caller.conn_id).as_ref(),
            Some(handle.id())
        );
        // The desktop main window of the project is still found without it.
        assert_eq!(
            crate::project_launch::main_window_of(&registry, handle.id()),
            crate::project_registry::MAIN_WINDOW_LABEL
        );

        connections.detach(&caller.conn_id);
        assert!(registry.for_window(&caller.label).is_err());
        assert_eq!(connections.attached(&caller.conn_id), None);

        connections
            .attach(&caller.conn_id, handle.id().as_str())
            .unwrap();
        connections.close(&caller.conn_id);
        assert!(registry.window_labels(handle.id()).is_empty());
        assert!(connections
            .caller(&caller.conn_id, &caller.device_id)
            .is_none());
        std::fs::remove_dir_all(temp).ok();
    }

    #[test]
    fn only_open_projects_can_be_attached() {
        let (temp, registry) = registry("remote-attach-closed");
        let project = create_project(&temp.join("p"), "Sample");
        let handle = registry.register(&project).unwrap();
        let connections = RemoteConnections::new(registry.clone());
        let caller = connections.open("device");

        assert_eq!(
            connections.attach(&caller.conn_id, "not-open").unwrap_err(),
            PROJECT_NOT_OPEN
        );
        assert_eq!(
            connections
                .attach("missing", handle.id().as_str())
                .unwrap_err(),
            UNKNOWN_CONNECTION
        );
        assert_eq!(connections.open_projects().len(), 1);

        connections
            .attach(&caller.conn_id, handle.id().as_str())
            .unwrap();
        // The desktop closes the project: the remote binding goes with it.
        registry.close(handle.id());
        assert_eq!(connections.attached(&caller.conn_id), None);
        assert!(connections.open_projects().is_empty());
        std::fs::remove_dir_all(temp).ok();
    }

    fn writes() -> crate::write_coordinator::ProjectWriteCoordinator {
        crate::write_coordinator::ProjectWriteCoordinator::silent()
    }

    #[test]
    fn a_desktop_open_attaches_to_a_remote_held_project_but_never_shares_a_window() {
        let (temp, registry) = registry("remote-desktop-attach");
        let project = create_project(&temp.join("p"), "Sample");
        let writes = writes();
        let connections = RemoteConnections::new(registry.clone());
        let phone = connections.open("phone");

        // The phone opened it: no desktop window shows it.
        let held = crate::engine::register_open(&registry, &project).unwrap();
        connections
            .attach(&phone.conn_id, held.id().as_str())
            .unwrap();
        assert_eq!(registry.desktop_window(held.id()), None);

        // The launcher attaches instead of refusing: the same open project.
        let shown =
            crate::engine::commit_switch(&registry, &writes, None, &project, "main").unwrap();
        assert!(Arc::ptr_eq(&held, &shown));
        assert_eq!(
            registry.window_labels(held.id()),
            ["main", phone.label.as_str()]
        );
        assert_eq!(registry.desktop_window(held.id()).as_deref(), Some("main"));

        // A second desktop window is still refused, however it asks.
        assert_eq!(
            crate::engine::commit_switch(&registry, &writes, None, &project, "main-other")
                .unwrap_err(),
            crate::project_launch::ALREADY_OPEN_ELSEWHERE
        );
        let other = create_project(&temp.join("q"), "Other");
        let other =
            crate::engine::commit_switch(&registry, &writes, None, &other, "main-other").unwrap();
        assert_eq!(
            crate::engine::commit_switch(&registry, &writes, Some(&other), &project, "main-other")
                .unwrap_err(),
            crate::project_launch::ALREADY_OPEN_ELSEWHERE
        );
        assert_eq!(registry.for_window("main-other").unwrap().id(), other.id());
        std::fs::remove_dir_all(temp).ok();
    }

    #[test]
    fn a_window_leaving_a_project_with_a_remote_observer_keeps_it_and_its_work() {
        let (temp, registry) = registry("remote-window-leaves");
        let project = create_project(&temp.join("p"), "Sample");
        let next = create_project(&temp.join("q"), "Next");
        let writes = writes();
        let connections = RemoteConnections::new(registry.clone());
        let phone = connections.open("phone");
        let shown =
            crate::engine::commit_switch(&registry, &writes, None, &project, "main").unwrap();
        connections
            .attach(&phone.conn_id, shown.id().as_str())
            .unwrap();
        // A turn in flight holds a claim on the project.
        let key = crate::write_coordinator::TxnKey::eps(shown.id());
        writes.claim(&key, "s1", "req-1", ["src/main.eps".to_string()]);

        // The window switches away: only it leaves the project.
        crate::engine::commit_switch(&registry, &writes, Some(&shown), &next, "main").unwrap();
        assert!(!shown.is_closed());
        assert_eq!(registry.window_labels(shown.id()), [phone.label.as_str()]);
        assert!(writes.claimed_in_flight(&key).contains("src/main.eps"));
        assert_eq!(
            connections.attached(&phone.conn_id).as_ref(),
            Some(shown.id())
        );

        // A window closing works the same way.
        let window = "main-0123456789ab";
        let reshown =
            crate::engine::commit_switch(&registry, &writes, None, &project, window).unwrap();
        assert!(!crate::engine::leave_project(
            &registry,
            &writes,
            reshown.id(),
            window
        ));
        assert!(!shown.is_closed());
        assert!(writes.claimed_in_flight(&key).contains("src/main.eps"));

        // Without an observer, the window's project closes with it.
        connections.detach(&phone.conn_id);
        let reshown =
            crate::engine::commit_switch(&registry, &writes, None, &project, window).unwrap();
        assert!(crate::engine::leave_project(
            &registry,
            &writes,
            reshown.id(),
            window
        ));
        assert!(shown.is_closed());
        assert!(writes.claimed_in_flight(&key).is_empty());
        std::fs::remove_dir_all(temp).ok();
    }

    #[test]
    fn a_dropped_socket_leaves_its_project_open_for_the_next_open() {
        let (temp, registry) = registry("remote-socket-drop");
        let project = create_project(&temp.join("p"), "Sample");
        let writes = writes();
        let connections = RemoteConnections::new(registry.clone());
        let phone = connections.open("phone");
        let held = crate::engine::register_open(&registry, &project).unwrap();
        connections
            .attach(&phone.conn_id, held.id().as_str())
            .unwrap();

        connections.close(&phone.conn_id);
        assert!(registry.window_labels(held.id()).is_empty());
        assert!(registry.get(held.id()).is_some() && !held.is_closed());

        // A reconnecting phone attaches again.
        let again = connections.open("phone");
        connections
            .attach(&again.conn_id, held.id().as_str())
            .unwrap();
        connections.close(&again.conn_id);

        // A desktop request is routed to a window, not focused on nothing,
        // and that window's open attaches to the project.
        let id = held.id().clone();
        assert_eq!(
            crate::project_launch::launch_target(
                &registry,
                Some(&id),
                |label| label == crate::project_registry::MAIN_WINDOW_LABEL,
                |_| false,
                || "main-new1".to_string(),
            ),
            crate::project_launch::LaunchTarget::Window("main".to_string())
        );
        let shown =
            crate::engine::commit_switch(&registry, &writes, None, &project, "main").unwrap();
        assert!(Arc::ptr_eq(&held, &shown));
        std::fs::remove_dir_all(temp).ok();
    }

    #[test]
    fn a_remote_close_refuses_work_closes_idle_and_only_detaches_from_a_window() {
        let (temp, registry) = registry("remote-close");
        let project = create_project(&temp.join("p"), "Sample");
        let writes = writes();
        let handle = crate::engine::register_open(&registry, &project).unwrap();
        registry.bind_window("remote-phone-1", handle.id());

        // Admitted work holds the close gate: refused, still open.
        let working = handle.close_gate().try_read().unwrap();
        let refusal =
            crate::engine::close_remote_project(&registry, &writes, &handle, || false).unwrap_err();
        assert!(refusal.contains("닫지 않았습니다"), "{refusal}");
        assert!(!handle.is_closed());
        drop(working);

        // A Map or import window still open on the PC refuses it too.
        let refusal =
            crate::engine::close_remote_project(&registry, &writes, &handle, || true).unwrap_err();
        assert!(refusal.contains("Map Agent"), "{refusal}");
        assert!(!handle.is_closed());

        // A desktop window shows it: the phone only leaves.
        registry.bind_window("main", handle.id());
        assert_eq!(
            crate::engine::close_remote_project(&registry, &writes, &handle, || false),
            Ok(crate::engine::RemoteClose::Detached)
        );
        assert!(!handle.is_closed());

        // Idle with no desktop window: it closes, with every binding.
        registry.unbind_window("main");
        assert_eq!(
            crate::engine::close_remote_project(&registry, &writes, &handle, || false),
            Ok(crate::engine::RemoteClose::Closed)
        );
        assert!(handle.is_closed());
        assert!(registry.get(handle.id()).is_none());
        assert!(registry.window_labels(handle.id()).is_empty());
        std::fs::remove_dir_all(temp).ok();
    }

    #[test]
    fn a_phone_opens_only_a_recent_project() {
        let mut config = crate::config::Config::default();
        config.record_recent_project("Sample".to_string(), r"C:\Maps\Sample".to_string(), 1);
        assert_eq!(
            recent_path(&config, r"C:\Maps\Sample").as_deref(),
            Ok(r"C:\Maps\Sample")
        );
        // The same entry however the separators and case are spelled.
        assert_eq!(
            recent_path(&config, "c:/maps/sample").as_deref(),
            Ok(r"C:\Maps\Sample")
        );
        for path in [r"C:\Maps\Other", r"C:\Maps", r"C:\Maps\Sample\..\Other", ""] {
            assert_eq!(
                recent_path(&config, path).unwrap_err(),
                NOT_RECENT,
                "{path}"
            );
        }
    }

    #[test]
    fn the_project_list_marks_open_recent_and_desktop_projects() {
        let (temp, registry) = registry("remote-project-list");
        let shown = create_project(&temp.join("a"), "Alpha");
        let held = create_project(&temp.join("b"), "Beta");
        create_project(&temp.join("c"), "Gamma");
        let recent = |name: &str, dir: &str, available: bool| crate::setup::RecentProject {
            name: name.to_string(),
            path: temp.join(dir).to_string_lossy().into_owned(),
            last_opened_at: 1,
            available,
        };
        let recents = [
            recent("Gamma", "c", true),
            recent("Beta", "b", true),
            recent("Gone", "gone", false),
        ];
        let shown = registry.register(&shown).unwrap();
        registry.bind_window("main", shown.id());
        let held = registry.register(&held).unwrap();
        registry.bind_window("remote-phone-1", held.id());

        let entries = project_entries(&registry, &recents, |path| {
            crate::native_runtime::project_id_at(std::path::Path::new(path))
        });
        let rows = entries
            .iter()
            .map(|entry| {
                (
                    entry.name.as_str(),
                    entry.open,
                    entry.desktop_window,
                    entry.available,
                    entry.project_id.is_some(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            rows,
            [
                ("Alpha", true, true, true, true),
                ("Beta", true, false, true, true),
                ("Gamma", false, false, true, false),
                ("Gone", false, false, false, false),
            ]
        );
        // An open recent project keeps its recent path, so it can be reopened.
        assert_eq!(entries[1].path, recents[1].path);
        assert_eq!(entries[1].project_id.as_deref(), Some(held.id().as_str()));
        std::fs::remove_dir_all(temp).ok();
    }

    #[test]
    fn a_socket_is_never_borrowed_by_another_device() {
        let (temp, registry) = registry("remote-caller-device");
        let connections = RemoteConnections::new(registry);
        let caller = connections.open("device-a");
        assert_eq!(
            connections.caller(&caller.conn_id, "device-a"),
            Some(caller.clone())
        );
        assert_eq!(connections.caller(&caller.conn_id, "device-b"), None);
        std::fs::remove_dir_all(temp).ok();
    }
}
