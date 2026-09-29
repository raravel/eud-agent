//! Project open requests from the OS (file association, a second launch) and
//! from "새 창에서 열기" wait here, per window, until that window's panel
//! takes them. Receiving a request never changes the project of a window
//! underneath a turn: a project that is open already is focused, a request
//! goes to the launcher window only while it has no project and nothing
//! queued, and otherwise to a new project window.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;
use serde::Serialize;
use tauri::{Emitter, Manager};

use crate::project_id::ProjectId;
use crate::project_registry::{ProjectRegistry, MAIN_WINDOW_LABEL};

/// The prefix of every project window's label but the launcher's own `main`.
pub const PROJECT_WINDOW_PREFIX: &str = "main-";

/// What a caller is told when the project it asked for is open in another
/// window, which is focused instead.
pub const ALREADY_OPEN_ELSEWHERE: &str = "이미 다른 창에서 열려 있습니다. 그 창으로 이동했습니다.";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectLaunchRequest {
    pub path: String,
}

/// The open requests waiting for each window, by window label.
#[derive(Default)]
pub struct PendingProjectLaunch {
    requests: Mutex<HashMap<String, VecDeque<ProjectLaunchRequest>>>,
}

impl PendingProjectLaunch {
    /// The requests of the process's own command line, for the launcher.
    pub fn from_args(args: impl IntoIterator<Item = String>, cwd: &Path) -> Self {
        let pending = Self::default();
        for request in parse_args(args, cwd) {
            pending.enqueue(MAIN_WINDOW_LABEL, request);
        }
        pending
    }

    fn enqueue(&self, label: &str, request: ProjectLaunchRequest) {
        self.requests
            .lock()
            .entry(label.to_string())
            .or_default()
            .push_back(request);
    }

    fn take(&self, label: &str) -> Option<ProjectLaunchRequest> {
        let mut requests = self.requests.lock();
        let queue = requests.get_mut(label)?;
        let request = queue.pop_front();
        if queue.is_empty() {
            requests.remove(label);
        }
        request
    }

    /// Take back the request `deliver` just queued for `label`.
    fn withdraw_last(&self, label: &str) {
        let mut requests = self.requests.lock();
        if let Some(queue) = requests.get_mut(label) {
            queue.pop_back();
            if queue.is_empty() {
                requests.remove(label);
            }
        }
    }

    /// Drop the requests of a window that closed before taking them.
    pub fn forget(&self, label: &str) {
        self.requests.lock().remove(label);
    }

    fn is_waiting(&self, label: &str) -> bool {
        self.requests
            .lock()
            .get(label)
            .is_some_and(|queue| !queue.is_empty())
    }
}

/// The project paths of a command line: argv[0] is the executable, flags are
/// skipped, and `--` makes every later argument a path.
fn parse_args(args: impl IntoIterator<Item = String>, cwd: &Path) -> Vec<ProjectLaunchRequest> {
    let mut requests = Vec::new();
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
        requests.push(ProjectLaunchRequest {
            path: absolute.to_string_lossy().into_owned(),
        });
    }
    requests
}

/// Where one open request goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LaunchTarget {
    /// The project is open: its window is focused and nothing is queued.
    Focus(String),
    /// The request waits for this existing window.
    Window(String),
    /// A new project window with this label takes the request.
    NewWindow(String),
}

/// Decide where a request for `project` (`None` when the path names no
/// native project yet: a legacy one to convert, or nothing openable) goes.
/// `window_exists` tells which windows are open; `waiting` which have
/// requests queued; `fresh` names a new window for an unknown project.
pub(crate) fn launch_target(
    registry: &ProjectRegistry,
    project: Option<&ProjectId>,
    window_exists: impl Fn(&str) -> bool,
    waiting: impl Fn(&str) -> bool,
    fresh: impl FnOnce() -> String,
) -> LaunchTarget {
    if let Some(id) = project.filter(|id| registry.get(id).is_some()) {
        return LaunchTarget::Focus(main_window_of(registry, id));
    }
    // The launcher takes a request only while it works on no project and
    // has no other request queued: two requests there would open one
    // project and then switch it to the other.
    if window_exists(MAIN_WINDOW_LABEL)
        && registry.for_window(MAIN_WINDOW_LABEL).is_err()
        && !waiting(MAIN_WINDOW_LABEL)
    {
        return LaunchTarget::Window(MAIN_WINDOW_LABEL.to_string());
    }
    let label = match project {
        Some(id) => format!("{PROJECT_WINDOW_PREFIX}{}", registry.window_suffix(id)),
        None => return LaunchTarget::NewWindow(fresh()),
    };
    if !window_exists(&label) {
        return LaunchTarget::NewWindow(label);
    }
    // A window keeps its label for life: one created for this project may
    // have switched to another since, and never takes a request that would
    // switch it back.
    if registry.for_window(&label).is_err() {
        LaunchTarget::Window(label)
    } else {
        LaunchTarget::NewWindow(fresh())
    }
}

/// The main window `id` is shown in.
pub(crate) fn main_window_of(registry: &ProjectRegistry, id: &ProjectId) -> String {
    registry
        .window_labels(id)
        .into_iter()
        .find(|label| is_project_window(label))
        .unwrap_or_else(|| MAIN_WINDOW_LABEL.to_string())
}

/// Whether `label` is a main (project) window: the launcher `main` or a
/// `main-*` window, never a Map or import window.
pub(crate) fn is_project_window(label: &str) -> bool {
    label == MAIN_WINDOW_LABEL || label.starts_with(PROJECT_WINDOW_PREFIX)
}

/// A label for a new window whose project is not known yet.
fn fresh_label(app: &tauri::AppHandle) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    loop {
        let label = format!(
            "{PROJECT_WINDOW_PREFIX}new{}",
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        if app.get_webview_window(&label).is_none() {
            return label;
        }
    }
}

/// Deliver one request: focus the project's window, queue it for an
/// existing window, or open a new project window that takes it.
pub(crate) fn deliver(
    app: &tauri::AppHandle,
    request: ProjectLaunchRequest,
) -> Result<LaunchTarget, String> {
    // One delivery at a time: two requests deciding together could both
    // take the launcher, or both create one project's window.
    static DELIVERY: Mutex<()> = Mutex::new(());
    let _delivering = DELIVERY.lock();
    let registry = app.state::<ProjectRegistry>();
    let pending = app.state::<PendingProjectLaunch>();
    let project = crate::native_runtime::project_id_at(Path::new(&request.path));
    let target = launch_target(
        &registry,
        project.as_ref(),
        |label| app.get_webview_window(label).is_some(),
        |label| pending.is_waiting(label),
        || fresh_label(app),
    );
    match &target {
        LaunchTarget::Focus(label) => focus_window(app, label),
        LaunchTarget::Window(label) => {
            pending.enqueue(label, request);
            // A lost notification is harmless: the panel's handshake drains
            // its queue.
            if let Err(error) = app.emit_to(label.as_str(), "project-open-requested", ()) {
                eprintln!("eud-agent: project launch notification failed: {error}");
            }
            focus_window(app, label);
        }
        LaunchTarget::NewWindow(label) => {
            pending.enqueue(label, request);
            if let Err(error) = open_project_window(app, label) {
                pending.withdraw_last(label);
                return Err(error);
            }
        }
    }
    Ok(target)
}

/// Open a new project window. Its panel starts at the launcher and opens
/// the request queued for it.
pub(crate) fn open_project_window(app: &tauri::AppHandle, label: &str) -> Result<(), String> {
    let window =
        tauri::WebviewWindowBuilder::new(app, label, tauri::WebviewUrl::App("index.html".into()))
            .title("eud-agent")
            .inner_size(1280.0, 832.0)
            .min_inner_size(960.0, 640.0)
            .resizable(true)
            .disable_drag_drop_handler()
            .build()
            .map_err(|error| {
                format!("새 프로젝트 창을 열지 못했습니다. 앱을 다시 시작해 주세요. ({error})")
            })?;
    if let Err(error) = window.set_focus() {
        eprintln!("eud-agent: cannot focus project window: {error}");
    }
    Ok(())
}

/// Show, restore, and focus window `label` when it exists.
pub(crate) fn focus_window(app: &tauri::AppHandle, label: &str) {
    if let Some(window) = app.get_webview_window(label) {
        for result in [window.show(), window.unminimize(), window.set_focus()] {
            if let Err(error) = result {
                eprintln!("eud-agent: cannot focus project window: {error}");
            }
        }
    }
}

/// A second launch of the app: each project path it names is delivered.
pub fn receive_launch(app: &tauri::AppHandle, args: Vec<String>, cwd: String) {
    let requests = parse_args(args, Path::new(&cwd));
    if requests.is_empty() {
        // A plain second launch brings the app forward: the launcher, or
        // else a project window.
        if let Some(label) = app
            .webview_windows()
            .into_keys()
            .filter(|label| is_project_window(label))
            .min()
        {
            focus_window(app, &label);
        }
        return;
    }
    // Off the single-instance callback, which runs on the main thread
    // while the second process waits: reading manifests and creating a
    // window there can stall or deadlock it.
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        for request in requests {
            if let Err(error) = deliver(&app, request) {
                eprintln!("eud-agent: project launch failed: {error}");
            }
        }
    });
}

#[tauri::command]
pub fn project_take_launch_request(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, PendingProjectLaunch>,
) -> Option<ProjectLaunchRequest> {
    state.take(window.label())
}

/// What "새 창에서 열기" did.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum NewWindowOutcome {
    /// The picker was closed without a choice.
    Cancelled,
    /// A project window takes the chosen project.
    Opened,
    /// The project is open in another window, which was focused.
    Focused { message: String },
}

/// Pick a project and open it in its own window, next to the ones open.
#[tauri::command]
pub async fn project_open_new_window(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
) -> Result<NewWindowOutcome, String> {
    use tauri_plugin_dialog::DialogExt;
    tauri::async_runtime::spawn_blocking(move || {
        let Some(picked) = app
            .dialog()
            .file()
            .set_parent(&window)
            .add_filter("EUD Agent 프로젝트 (.eap)", &["eap"])
            .blocking_pick_file()
        else {
            return Ok(NewWindowOutcome::Cancelled);
        };
        let path = picked.into_path().map_err(|error| error.to_string())?;
        let request = ProjectLaunchRequest {
            path: path.to_string_lossy().into_owned(),
        };
        Ok(match deliver(&app, request)? {
            LaunchTarget::Focus(_) => NewWindowOutcome::Focused {
                message: ALREADY_OPEN_ELSEWHERE.to_string(),
            },
            LaunchTarget::Window(_) | LaunchTarget::NewWindow(_) => NewWindowOutcome::Opened,
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DataDirs;
    use crate::project_id::test_support::{create_project, temp_root};

    fn registry(label: &str) -> (std::path::PathBuf, ProjectRegistry) {
        let base = temp_root(label);
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        dirs.ensure_dirs().unwrap();
        (base, ProjectRegistry::new(dirs))
    }

    #[test]
    fn launch_requests_survive_missing_listener_and_resolve_sender_cwd() {
        let cwd = std::env::temp_dir().join("프로젝트 작업 폴더");
        let pending = PendingProjectLaunch::from_args(
            ["eud-agent.exe", "첫 프로젝트/project.eap"].map(str::to_string),
            &cwd,
        );
        let second_cwd = cwd.join("다른 폴더");
        for request in parse_args(
            ["eud-agent.exe", "다른 이름.eap"].map(str::to_string),
            &second_cwd,
        ) {
            pending.enqueue(MAIN_WINDOW_LABEL, request);
        }
        assert_eq!(
            pending.take(MAIN_WINDOW_LABEL).unwrap().path,
            cwd.join("첫 프로젝트/project.eap").to_string_lossy()
        );
        assert_eq!(
            pending.take(MAIN_WINDOW_LABEL).unwrap().path,
            second_cwd.join("다른 이름.eap").to_string_lossy()
        );
        assert!(pending.take(MAIN_WINDOW_LABEL).is_none());
    }

    #[test]
    fn normal_start_does_not_open_executable_or_flags_as_project() {
        let pending = PendingProjectLaunch::from_args(
            ["eud-agent.exe", "--", "-project.eap"].map(str::to_string),
            &std::env::temp_dir(),
        );
        assert!(pending
            .take(MAIN_WINDOW_LABEL)
            .unwrap()
            .path
            .ends_with("-project.eap"));
        assert!(parse_args(
            ["eud-agent.exe", "--verbose"].map(str::to_string),
            &std::env::temp_dir(),
        )
        .is_empty());
    }

    #[test]
    fn a_closed_windows_requests_are_forgotten() {
        let pending = PendingProjectLaunch::default();
        pending.enqueue(
            "main-aaaaaaaaaaaa",
            ProjectLaunchRequest { path: "a".into() },
        );
        pending.enqueue(
            "main-aaaaaaaaaaaa",
            ProjectLaunchRequest { path: "b".into() },
        );
        pending.withdraw_last("main-aaaaaaaaaaaa");
        assert_eq!(pending.take("main-aaaaaaaaaaaa").unwrap().path, "a");
        pending.enqueue(
            "main-aaaaaaaaaaaa",
            ProjectLaunchRequest { path: "c".into() },
        );
        pending.forget("main-aaaaaaaaaaaa");
        assert!(pending.take("main-aaaaaaaaaaaa").is_none());
    }

    #[test]
    fn each_window_takes_only_its_own_requests() {
        let pending = PendingProjectLaunch::default();
        pending.enqueue(
            "main-aaaaaaaaaaaa",
            ProjectLaunchRequest { path: "a".into() },
        );
        pending.enqueue(MAIN_WINDOW_LABEL, ProjectLaunchRequest { path: "m".into() });
        assert!(pending.is_waiting("main-aaaaaaaaaaaa"));
        assert_eq!(pending.take(MAIN_WINDOW_LABEL).unwrap().path, "m");
        assert!(pending.take(MAIN_WINDOW_LABEL).is_none());
        assert!(pending.take("main-bbbbbbbbbbbb").is_none());
        assert_eq!(pending.take("main-aaaaaaaaaaaa").unwrap().path, "a");
        assert!(!pending.is_waiting("main-aaaaaaaaaaaa"));
    }

    #[test]
    fn an_open_project_is_focused_and_another_gets_its_own_window() {
        let (base, registry) = registry("launch-target");
        create_project(&base.join("a"), "A");
        create_project(&base.join("b"), "B");
        let a_id = crate::native_runtime::project_id_at(&base.join("a")).unwrap();
        let b_id = crate::native_runtime::project_id_at(&base.join("b")).unwrap();
        let fresh = || "main-new1".to_string();

        // No project open: the launcher takes the first request, but not a
        // second one while the first waits.
        let only_main = |label: &str| label == MAIN_WINDOW_LABEL;
        assert_eq!(
            launch_target(&registry, Some(&a_id), only_main, |_| false, fresh),
            LaunchTarget::Window(MAIN_WINDOW_LABEL.to_string())
        );
        let queued = |label: &str| label == MAIN_WINDOW_LABEL;
        let b_window = format!("{PROJECT_WINDOW_PREFIX}{}", &b_id.as_str()[..12]);
        assert_eq!(
            launch_target(&registry, Some(&b_id), only_main, queued, fresh),
            LaunchTarget::NewWindow(b_window.clone())
        );

        // A is open in the launcher: asking for A again focuses it; B gets a
        // new window, or the one already opened for it.
        let a = registry.open(&base.join("a")).unwrap();
        registry.bind_window(MAIN_WINDOW_LABEL, a.id());
        assert_eq!(
            launch_target(&registry, Some(&a_id), only_main, |_| false, fresh),
            LaunchTarget::Focus(MAIN_WINDOW_LABEL.to_string())
        );
        assert_eq!(
            launch_target(&registry, Some(&b_id), only_main, |_| false, fresh),
            LaunchTarget::NewWindow(b_window.clone())
        );
        let b_exists = |label: &str| label == MAIN_WINDOW_LABEL || label == b_window;
        assert_eq!(
            launch_target(&registry, Some(&b_id), b_exists, |_| false, fresh),
            LaunchTarget::Window(b_window.clone())
        );

        // B open in its window is focused there, whatever window asks.
        let b = registry.open(&base.join("b")).unwrap();
        registry.bind_window(&b_window, b.id());
        registry.bind_window(&registry.map_window_label(b.id()), b.id());
        assert_eq!(
            launch_target(&registry, Some(&b_id), b_exists, |_| false, fresh),
            LaunchTarget::Focus(b_window.clone())
        );

        // B's window switched to another project C: a request for B (closed
        // now) never switches that window back, it gets a fresh one.
        registry.close(b.id());
        create_project(&base.join("c"), "C");
        let c = registry.open(&base.join("c")).unwrap();
        registry.bind_window(&b_window, c.id());
        assert_eq!(
            launch_target(&registry, Some(&b_id), b_exists, |_| false, fresh),
            LaunchTarget::NewWindow("main-new1".to_string())
        );

        // A path that names no native project yet gets a fresh window once
        // the launcher works on a project.
        assert_eq!(
            launch_target(&registry, None, only_main, |_| false, fresh),
            LaunchTarget::NewWindow("main-new1".to_string())
        );
    }

    #[test]
    fn only_main_windows_are_project_windows() {
        assert!(is_project_window(MAIN_WINDOW_LABEL));
        assert!(is_project_window("main-0123456789ab"));
        assert!(!is_project_window("map-agent-0123456789ab"));
        assert!(!is_project_window("map-import-0123456789ab"));
    }
}
