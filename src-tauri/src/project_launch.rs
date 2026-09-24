//! File-association requests wait here until the main panel registers its listener.
//! Receiving an OS request never changes the configured project underneath a turn.

use std::collections::VecDeque;
use std::path::Path;

use parking_lot::Mutex;
use serde::Serialize;
use tauri::{Emitter, Manager};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectLaunchRequest {
    pub path: String,
}

#[derive(Default)]
pub struct PendingProjectLaunch {
    requests: Mutex<VecDeque<ProjectLaunchRequest>>,
}

impl PendingProjectLaunch {
    pub fn from_args(args: impl IntoIterator<Item = String>, cwd: &Path) -> Self {
        let pending = Self::default();
        pending.enqueue_args(args, cwd);
        pending
    }

    fn enqueue_args(&self, args: impl IntoIterator<Item = String>, cwd: &Path) -> bool {
        let mut requests = self.requests.lock();
        let mut added = false;
        // argv[0] is the executable, never a project. `--` allows dash-prefixed paths.
        let mut positional_only = false;
        for arg in args.into_iter().skip(1) {
            if !positional_only && arg == "--" {
                positional_only = true;
                continue;
            }
            if arg.is_empty() || (!positional_only && arg.starts_with('-')) {
                continue;
            }
            let path = Path::new(&arg);
            let absolute = if path.is_absolute() {
                path.to_path_buf()
            } else {
                cwd.join(path)
            };
            requests.push_back(ProjectLaunchRequest {
                path: absolute.to_string_lossy().into_owned(),
            });
            added = true;
        }
        added
    }

    fn take(&self) -> Option<ProjectLaunchRequest> {
        self.requests.lock().pop_front()
    }
}

pub fn receive_launch(app: &tauri::AppHandle, args: Vec<String>, cwd: String) {
    let pending = app.state::<PendingProjectLaunch>();
    if pending.enqueue_args(args, Path::new(&cwd)) {
        // A lost notification is harmless: the initial panel handshake drains the queue.
        if let Err(error) = app.emit_to("main", "project-open-requested", ()) {
            eprintln!("eud-agent: project launch notification failed: {error}");
        }
    }
    if let Some(window) = app.get_webview_window("main") {
        for result in [window.show(), window.unminimize(), window.set_focus()] {
            if let Err(error) = result {
                eprintln!("eud-agent: cannot focus project window: {error}");
            }
        }
    }
}

#[tauri::command]
pub fn project_take_launch_request(
    state: tauri::State<'_, PendingProjectLaunch>,
) -> Option<ProjectLaunchRequest> {
    state.take()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_requests_survive_missing_listener_and_resolve_sender_cwd() {
        let cwd = std::env::temp_dir().join("프로젝트 작업 폴더");
        let pending = PendingProjectLaunch::from_args(
            ["eud-agent.exe", "첫 프로젝트/project.eap"].map(str::to_string),
            &cwd,
        );
        let second_cwd = cwd.join("다른 폴더");
        pending.enqueue_args(
            ["eud-agent.exe", "다른 이름.eap"].map(str::to_string),
            &second_cwd,
        );
        assert_eq!(
            pending.take().unwrap().path,
            cwd.join("첫 프로젝트/project.eap").to_string_lossy()
        );
        assert_eq!(
            pending.take().unwrap().path,
            second_cwd.join("다른 이름.eap").to_string_lossy()
        );
        assert!(pending.take().is_none());
    }

    #[test]
    fn normal_start_does_not_open_executable_or_flags_as_project() {
        let pending = PendingProjectLaunch::from_args(
            ["eud-agent.exe", "--", "-project.eap"].map(str::to_string),
            &std::env::temp_dir(),
        );
        assert!(pending.take().unwrap().path.ends_with("-project.eap"));
        pending.enqueue_args(
            ["eud-agent.exe", "--verbose"].map(str::to_string),
            &std::env::temp_dir(),
        );
        assert!(pending.take().is_none());
    }
}
