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
//! Phase 2 attaches only to a project that is already open on the desktop;
//! the binding goes when the socket closes or detaches, and a project that
//! closes on the desktop drops it with every other binding.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use serde::Serialize;

use crate::project_id::ProjectId;
use crate::project_registry::{ProjectHandle, ProjectRegistry};

pub const REMOTE_LABEL_PREFIX: &str = "remote-";

pub const PROJECT_NOT_OPEN: &str =
    "이 프로젝트는 PC에서 열려 있지 않습니다. PC에서 프로젝트를 연 뒤 다시 선택해 주세요.";
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

    /// Unbind socket `conn_id` from its project.
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

    /// Forget socket `conn_id`, dropping its binding.
    pub fn close(&self, conn_id: &str) {
        if let Some(connection) = self.connections.lock().remove(conn_id) {
            if connection.attached.is_some() {
                self.registry.unbind_window(&connection.caller.label);
            }
        }
    }

    /// The projects open on the desktop, by name.
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
