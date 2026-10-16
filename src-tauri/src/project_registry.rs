//! The runtime authority over open projects.
//!
//! A [`ProjectHandle`] is one open project: its id, its canonical root, the
//! root-bound native manager every tool, build, and prompt section of that
//! project uses, and the runtime state that belongs to that project (its
//! session engines, running harness jobs, review recovery, the Map window's
//! pending open request, and the close gate). Nothing here reads the
//! configured project path: `Config.project_path` only remembers the last
//! opened project for the launcher.
//!
//! Any number of projects are open at once, each in its own windows. The
//! one place that could limit that again is
//! [`ProjectRegistry::can_open_additional`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};

use crate::config::DataDirs;
use crate::native_project::NativeProject;
use crate::native_runtime::NativeProjectManager;
use crate::project_id::ProjectId;

/// The launcher window: bound to a project once it opens one.
pub const MAIN_WINDOW_LABEL: &str = "main";
/// Label prefixes of a project's own windows. The binding, not the label, is
/// the authority on which project a window works on.
pub const MAP_WINDOW_PREFIX: &str = "map-agent-";
pub const IMPORT_WINDOW_PREFIX: &str = "map-import-";

/// The error of every call that needs an open project and has none.
pub const NO_PROJECT_OPEN: &str = "열린 프로젝트가 없습니다. 프로젝트를 먼저 열어 주세요.";
/// The error of a call on a project that was closed meanwhile.
pub const PROJECT_CLOSED: &str = "프로젝트가 닫혔습니다. 프로젝트를 다시 열어 주세요.";
/// The error of a call on a project that is being closed right now.
pub const PROJECT_CLOSING: &str = "프로젝트를 닫는 중입니다. 잠시 후 다시 시도해 주세요.";

/// Runtime state a subsystem keeps per open project (the engine's session
/// workers, harness jobs, and review recovery). The handle holds it type
/// erased and creates it on first use, so a handle alone never pulls in the
/// subsystem (the engine's workers hold the Tauri app handle).
pub(crate) trait ProjectRuntime: Send + Sync + 'static {
    /// Drop what the project owns; called when it closes.
    fn clear(&self);
    fn as_any(&self) -> &dyn std::any::Any;
}

pub struct ProjectHandle {
    id: ProjectId,
    root: PathBuf,
    dirs: DataDirs,
    native: NativeProjectManager,
    closed: AtomicBool,
    /// Work on this project holds it for reading; closing takes it for
    /// writing, so a close never lands in the middle of admitted work.
    close_gate: tokio::sync::RwLock<()>,
    /// This project's session engines, running harness jobs, and review
    /// recovery (see [`ProjectHandle::runtime`]).
    runtime: std::sync::OnceLock<Box<dyn ProjectRuntime>>,
    /// The project's pending-review recovery, run once per open: `Ok` holds
    /// the sessions whose review could not be restored, by session id.
    recovery: Mutex<Option<Result<HashMap<String, String>, String>>>,
    /// The Map session the next Map window bootstrap of this project loads.
    pending_open_session: Mutex<Option<String>>,
    /// Whether the next Map window bootstrap opens the properties dialog.
    pending_open_properties: Mutex<bool>,
}

impl ProjectHandle {
    fn new(dirs: DataDirs, project: &NativeProject) -> Self {
        let root = project.root().to_path_buf();
        Self {
            id: ProjectId::of(project),
            native: NativeProjectManager::for_root(dirs.clone(), root.clone()),
            root,
            dirs,
            closed: AtomicBool::new(false),
            close_gate: tokio::sync::RwLock::new(()),
            runtime: std::sync::OnceLock::new(),
            recovery: Mutex::new(None),
            pending_open_session: Mutex::new(None),
            pending_open_properties: Mutex::new(false),
        }
    }

    pub fn id(&self) -> &ProjectId {
        &self.id
    }

    /// The canonical root the project was opened at.
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Fail when the project was closed: nothing may reach a closed
    /// project's files through a handle someone kept.
    pub fn ensure_open(&self) -> Result<(), String> {
        if self.is_closed() {
            Err(PROJECT_CLOSED.to_string())
        } else {
            Ok(())
        }
    }

    /// The project's root-bound native manager.
    pub fn native(&self) -> Result<NativeProjectManager, String> {
        self.ensure_open()?;
        Ok(self.native.clone())
    }

    /// The project as it is on disk now.
    pub fn open_project(&self) -> Result<NativeProject, String> {
        self.ensure_open()?;
        self.native.open()
    }

    /// The live manifest name.
    pub fn live_name(&self) -> Result<String, String> {
        Ok(self.open_project()?.manifest().name.clone())
    }

    /// Whether a build of this project holds its marker.
    pub fn is_building(&self) -> bool {
        self.native.is_building()
    }

    /// This project's Map context (its saved source map).
    pub fn map_context(&self) -> Result<crate::map_context::MapContextService, String> {
        self.ensure_open()?;
        Ok(crate::map_context::MapContextService::for_root(
            self.dirs.clone(),
            self.root.clone(),
        ))
    }

    /// This project's memory store.
    pub fn memory(&self) -> Result<crate::memory::ProjectMemory, String> {
        Ok(crate::memory::ProjectMemory::for_project(
            &self.open_project()?,
        ))
    }

    /// The project's runtime state of type `T`, created on first use.
    pub(crate) fn runtime<T: ProjectRuntime + Default>(&self) -> &T {
        self.runtime
            .get_or_init(|| Box::new(T::default()))
            .as_any()
            .downcast_ref::<T>()
            .expect("one runtime state type per project handle")
    }

    pub fn close_gate(&self) -> &tokio::sync::RwLock<()> {
        &self.close_gate
    }

    pub(crate) fn recovery(&self) -> &Mutex<Option<Result<HashMap<String, String>, String>>> {
        &self.recovery
    }

    /// Admit one piece of work on this project: it holds the close gate for
    /// reading while it runs, so the project is not closed underneath it,
    /// and a project being closed or already closed refuses it.
    pub fn admit(&self) -> Result<tokio::sync::RwLockReadGuard<'_, ()>, String> {
        let admitted = self
            .close_gate
            .try_read()
            .map_err(|_| PROJECT_CLOSING.to_string())?;
        self.ensure_open()?;
        Ok(admitted)
    }

    pub(crate) fn pending_open_session(&self) -> &Mutex<Option<String>> {
        &self.pending_open_session
    }

    pub(crate) fn pending_open_properties(&self) -> &Mutex<bool> {
        &self.pending_open_properties
    }

    /// Mark the project closed and drop the runtime state it owns.
    fn mark_closed(&self) {
        self.closed.store(true, Ordering::SeqCst);
        *self.pending_open_session.lock() = None;
        *self.pending_open_properties.lock() = false;
        *self.recovery.lock() = None;
        if let Some(runtime) = self.runtime.get() {
            runtime.clear();
        }
    }
}

/// A window label suffix for `id` that none of `taken` is: its first 12 hex
/// digits, or the whole id when those are taken.
fn unique_suffix<'a>(id: &ProjectId, mut taken: impl Iterator<Item = &'a String>) -> String {
    let short = &id.as_str()[..id.as_str().len().min(12)];
    if taken.any(|other| other == short) {
        id.as_str().to_string()
    } else {
        short.to_string()
    }
}

/// Why a project could not be registered.
#[derive(Debug)]
pub enum RegisterError {
    /// The project is already open; the handle is the open one.
    AlreadyOpen(Arc<ProjectHandle>),
    /// Another project is open and only one may be.
    LimitReached,
    /// The folder is not an openable project.
    Invalid(String),
}

impl std::fmt::Display for RegisterError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyOpen(_) => {
                formatter.write_str("이 프로젝트는 이미 다른 창에서 열려 있습니다.")
            }
            Self::LimitReached => formatter
                .write_str("다른 프로젝트가 열려 있습니다. 그 프로젝트를 닫은 뒤 열어 주세요."),
            Self::Invalid(error) => formatter.write_str(error),
        }
    }
}

impl std::fmt::Debug for ProjectHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProjectHandle")
            .field("id", &self.id)
            .field("root", &self.root)
            .field("closed", &self.is_closed())
            .finish()
    }
}

#[derive(Clone)]
pub struct ProjectRegistry {
    inner: Arc<RegistryInner>,
}

struct RegistryInner {
    dirs: DataDirs,
    /// Which open project each window works on, by window label.
    windows: RwLock<HashMap<String, ProjectId>>,
    /// Held for one open, switch, or close at a time.
    switching: Mutex<()>,
    projects: RwLock<HashMap<ProjectId, Arc<ProjectHandle>>>,
    /// The label suffix of each open project's windows, fixed when it opens
    /// so a project opened later never renames the windows already open.
    suffixes: RwLock<HashMap<ProjectId, String>>,
    /// How many projects may be open at once.
    limit: usize,
}

impl ProjectRegistry {
    /// The product registry: projects open side by side without a limit.
    pub fn new(dirs: DataDirs) -> Self {
        Self::with_limit(dirs, usize::MAX)
    }

    /// A registry that holds up to `limit` projects.
    pub(crate) fn with_limit(dirs: DataDirs, limit: usize) -> Self {
        Self {
            inner: Arc::new(RegistryInner {
                dirs,
                switching: Mutex::new(()),
                windows: RwLock::new(HashMap::new()),
                projects: RwLock::new(HashMap::new()),
                suffixes: RwLock::new(HashMap::new()),
                limit,
            }),
        }
    }

    pub fn dirs(&self) -> &DataDirs {
        &self.inner.dirs
    }

    /// Begin an open, switch, or close; `None` while another one runs.
    pub fn try_begin_switch(&self) -> Option<parking_lot::MutexGuard<'_, ()>> {
        self.inner.switching.try_lock()
    }

    /// Whether another project may be opened next to the open ones. This is
    /// the one switch that would keep the product at one open project again.
    pub fn can_open_additional(&self) -> bool {
        self.admits(self.inner.projects.read().len())
    }

    fn admits(&self, open: usize) -> bool {
        open < self.inner.limit
    }

    /// Open the project at `root` and register it.
    pub fn open(&self, root: &Path) -> Result<Arc<ProjectHandle>, RegisterError> {
        let project = NativeProject::open(root).map_err(RegisterError::Invalid)?;
        self.register(&project)
    }

    /// Register an opened project. The same project (by id, whatever the
    /// spelling it was opened by) is never registered twice.
    pub fn register(&self, project: &NativeProject) -> Result<Arc<ProjectHandle>, RegisterError> {
        let id = ProjectId::of(project);
        let mut projects = self.inner.projects.write();
        if let Some(open) = projects.get(&id) {
            return Err(RegisterError::AlreadyOpen(Arc::clone(open)));
        }
        if !self.admits(projects.len()) {
            return Err(RegisterError::LimitReached);
        }
        let handle = Arc::new(ProjectHandle::new(self.inner.dirs.clone(), project));
        let mut suffixes = self.inner.suffixes.write();
        let suffix = unique_suffix(&id, suffixes.values());
        suffixes.insert(id.clone(), suffix);
        projects.insert(id, Arc::clone(&handle));
        Ok(handle)
    }

    pub fn get(&self, id: &ProjectId) -> Option<Arc<ProjectHandle>> {
        self.inner.projects.read().get(id).cloned()
    }

    /// Close `id`: it leaves the registry, its windows lose their binding,
    /// it is marked closed, and it drops the runtime state it owns. Returns
    /// the closed handle.
    pub fn close(&self, id: &ProjectId) -> Option<Arc<ProjectHandle>> {
        let handle = self.inner.projects.write().remove(id)?;
        self.inner.suffixes.write().remove(id);
        self.inner
            .windows
            .write()
            .retain(|_, project| project != id);
        handle.mark_closed();
        Some(handle)
    }

    /// Bind window `label` to open project `id`; a window works on one
    /// project at a time.
    pub fn bind_window(&self, label: &str, id: &ProjectId) {
        self.inner
            .windows
            .write()
            .insert(label.to_string(), id.clone());
    }

    /// Drop the binding of a window that closed.
    pub fn unbind_window(&self, label: &str) -> Option<ProjectId> {
        self.inner.windows.write().remove(label)
    }

    /// The open project window `label` works on.
    pub fn for_window(&self, label: &str) -> Result<Arc<ProjectHandle>, String> {
        let id = self
            .inner
            .windows
            .read()
            .get(label)
            .cloned()
            .ok_or_else(|| NO_PROJECT_OPEN.to_string())?;
        self.get(&id).ok_or_else(|| PROJECT_CLOSED.to_string())
    }

    /// The windows bound to `id`, sorted: exactly the windows a project's
    /// events are delivered to.
    pub fn window_labels(&self, id: &ProjectId) -> Vec<String> {
        let mut labels = self
            .inner
            .windows
            .read()
            .iter()
            .filter(|(_, project)| *project == id)
            .map(|(label, _)| label.clone())
            .collect::<Vec<_>>();
        labels.sort();
        labels
    }

    /// The desktop main window that shows `id`, if any. A project is shown
    /// in at most one; remote observers and companion windows are not it.
    pub fn desktop_window(&self, id: &ProjectId) -> Option<String> {
        self.window_labels(id)
            .into_iter()
            .find(|label| crate::project_launch::is_project_window(label))
    }

    /// Whether a remote client is attached to `id`.
    pub fn has_remote_observers(&self, id: &ProjectId) -> bool {
        self.window_labels(id)
            .iter()
            .any(|label| crate::remote::caller::is_remote_label(label))
    }

    /// The label suffix of `id`'s windows, fixed when it opened: its first
    /// 12 hex digits, or the whole id when an open project already had them.
    pub fn window_suffix(&self, id: &ProjectId) -> String {
        let suffixes = self.inner.suffixes.read();
        suffixes
            .get(id)
            .cloned()
            .unwrap_or_else(|| unique_suffix(id, suffixes.values()))
    }

    /// The label of `id`'s Map window.
    pub fn map_window_label(&self, id: &ProjectId) -> String {
        format!("{MAP_WINDOW_PREFIX}{}", self.window_suffix(id))
    }

    /// The label of `id`'s Map Importer window.
    pub fn import_window_label(&self, id: &ProjectId) -> String {
        format!("{IMPORT_WINDOW_PREFIX}{}", self.window_suffix(id))
    }

    pub fn handles(&self) -> Vec<Arc<ProjectHandle>> {
        self.inner.projects.read().values().cloned().collect()
    }

    /// The one open project, for the calls that cannot yet tell which
    /// project they are for. None or several open is an error.
    pub fn sole(&self) -> Result<Arc<ProjectHandle>, String> {
        let projects = self.inner.projects.read();
        let mut handles = projects.values();
        match (handles.next(), handles.next()) {
            (Some(handle), None) => Ok(Arc::clone(handle)),
            _ => Err(NO_PROJECT_OPEN.to_string()),
        }
    }

    /// The open project whose live manifest name is `name`, when exactly one
    /// open project has it. This is the only name match a session written
    /// before project ids ever gets.
    pub fn unique_by_live_name(&self, name: &str) -> Option<Arc<ProjectHandle>> {
        let mut matches = self
            .handles()
            .into_iter()
            .filter(|handle| handle.live_name().is_ok_and(|live| live == name));
        match (matches.next(), matches.next()) {
            (Some(handle), None) => Some(handle),
            _ => None,
        }
    }

    pub fn any_building(&self) -> bool {
        self.handles().iter().any(|handle| handle.is_building())
    }

    /// The open project whose root contains `path`.
    pub fn owner_of(&self, path: &Path) -> Option<Arc<ProjectHandle>> {
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        self.handles()
            .into_iter()
            .find(|handle| crate::project_rebind::is_within(&path, handle.root()))
    }
}

/// The prompt sections of one project, read from its handle every turn. A
/// provider holds its handle weakly: a closed project renders nothing.
#[derive(Clone)]
pub(crate) struct HandleProviders {
    handle: std::sync::Weak<ProjectHandle>,
}

impl HandleProviders {
    pub(crate) fn new(handle: &Arc<ProjectHandle>) -> Self {
        Self {
            handle: Arc::downgrade(handle),
        }
    }

    fn live(&self) -> Option<Arc<ProjectHandle>> {
        self.handle.upgrade().filter(|handle| !handle.is_closed())
    }

    fn wiki_store(&self) -> crate::wiki::WikiStore {
        let dir = self
            .live()
            .and_then(|handle| handle.memory().ok())
            .and_then(|memory| memory.store_dir())
            .map(|dir| dir.join("wiki"));
        crate::wiki::WikiStore::load(dir)
    }
}

impl crate::engine::MemoryProvider for HandleProviders {
    fn render_section(&self) -> String {
        self.live()
            .and_then(|handle| handle.memory().ok())
            .unwrap_or_else(crate::memory::ProjectMemory::disabled)
            .render_section(None)
    }
}

impl crate::engine::WikiProvider for HandleProviders {
    fn render_section(&self, query: &str) -> Option<String> {
        self.wiki_store().render_section(query)
    }

    fn record_accepted(
        &self,
        entries: Vec<crate::wiki::LedgerEntry>,
    ) -> Option<crate::ipc::WikiResponse> {
        let mut store = self.wiki_store();
        if !store.enabled() || entries.is_empty() {
            return None;
        }
        for entry in entries {
            store.upsert(entry);
        }
        if let Err(error) = store.save() {
            eprintln!("eud-agent: wiki ledger save failed: {error}");
            return None;
        }
        Some(crate::ipc::WikiResponse::from(store.ledger()))
    }
}

impl crate::engine::ProjectStateProvider for HandleProviders {
    fn render_section(&self) -> String {
        let Some(handle) = self.live() else {
            return format!("[project state]\n(native project unavailable: {PROJECT_CLOSED})");
        };
        let native = &handle.native;
        match native.status() {
            Ok(status) => format!(
                "[project state]\nproject={}\nroot={}\nsourceMap={}\nmainFile={}\nbuilding={}",
                status.name,
                status.root,
                status.source_map,
                status.main_file,
                native.is_building(),
            ),
            Err(error) => format!("[project state]\n(native project unavailable: {error})"),
        }
    }
}

impl crate::engine::ProjectMapProvider for HandleProviders {
    fn render_section(&self) -> Option<String> {
        match self.live()?.native.render_project_map() {
            Ok(section) => Some(section),
            Err(error) => {
                eprintln!("eud-agent: project map unavailable: {error}");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project_id::test_support::{create_project, temp_root};

    fn registry(label: &str, limit: usize) -> (PathBuf, ProjectRegistry) {
        let base = temp_root(label);
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        dirs.ensure_dirs().unwrap();
        (base.clone(), ProjectRegistry::with_limit(dirs, limit))
    }

    #[test]
    fn one_project_is_registered_once_whatever_its_spelling() {
        let (base, registry) = registry("registry-once", 2);
        let root = base.join("Proj");
        create_project(&root, "Sample");
        let first = registry.open(&root).unwrap();

        let spelled = PathBuf::from(format!("{}/", root.to_string_lossy().to_uppercase()));
        match registry.open(&spelled) {
            Err(RegisterError::AlreadyOpen(open)) => assert_eq!(open.id(), first.id()),
            other => panic!("expected AlreadyOpen, got {other:?}"),
        }
        assert_eq!(registry.handles().len(), 1);
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn the_product_registry_opens_projects_side_by_side() {
        let (base, _) = registry("registry-limit", 1);
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        let registry = ProjectRegistry::new(dirs.clone());
        create_project(&base.join("a"), "A");
        create_project(&base.join("b"), "B");
        let a = registry.open(&base.join("a")).unwrap();
        assert!(registry.can_open_additional());
        let b = registry.open(&base.join("b")).unwrap();
        assert_ne!(a.id(), b.id());
        assert_eq!(registry.handles().len(), 2);

        // The limit, when set, is the one switch back to a single project.
        let single = ProjectRegistry::with_limit(dirs, 1);
        single.open(&base.join("a")).unwrap();
        assert!(!single.can_open_additional());
        assert!(matches!(
            single.open(&base.join("b")),
            Err(RegisterError::LimitReached)
        ));
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn a_closed_handle_reaches_nothing() {
        let (base, registry) = registry("registry-close", 1);
        let root = base.join("a");
        create_project(&root, "A");
        let handle = registry.open(&root).unwrap();
        *handle.pending_open_session().lock() = Some("map-1".to_string());

        let closed = registry.close(handle.id()).unwrap();

        assert!(registry.get(handle.id()).is_none());
        assert!(registry.sole().is_err());
        assert_eq!(closed.native().err().as_deref(), Some(PROJECT_CLOSED));
        assert_eq!(closed.open_project().unwrap_err(), PROJECT_CLOSED);
        assert!(closed.map_context().is_err());
        assert!(closed.pending_open_session().lock().is_none());
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn a_handles_memory_and_wiki_live_in_its_own_project() {
        let (base, registry) = registry("registry-memory", 2);
        create_project(&base.join("a"), "A");
        create_project(&base.join("b"), "B");
        let a = registry.open(&base.join("a")).unwrap();
        let b = registry.open(&base.join("b")).unwrap();

        let a_store = a.memory().unwrap().store_dir().unwrap();
        let b_store = b.memory().unwrap().store_dir().unwrap();

        assert!(a_store.starts_with(a.root()));
        assert!(b_store.starts_with(b.root()));
        assert_ne!(a_store, b_store);
        std::fs::remove_dir_all(base).ok();
    }

    /// Two projects open side by side: each handle's prompt sections read
    /// only its own project, and changing the configured (last opened)
    /// project path afterwards moves none of them.
    #[test]
    fn prompt_sections_of_a_handle_read_only_its_own_project() {
        use crate::engine::{
            MemoryProvider, ProjectMapProvider, ProjectStateProvider, WikiProvider,
        };
        let (base, registry) = registry("registry-prompts", 2);
        create_project(&base.join("a"), "Alpha");
        create_project(&base.join("b"), "Beta");
        let a = registry.open(&base.join("a")).unwrap();
        let b = registry.open(&base.join("b")).unwrap();
        for (handle, marker, source) in [
            (&a, "ALPHA-MEMORY", "src/alpha_only.eps"),
            (&b, "BETA-MEMORY", "src/beta_only.eps"),
        ] {
            assert!(handle.memory().unwrap().write("resources", marker).ok);
            handle.native().unwrap().create_source(source, "").unwrap();
            let mut wiki = crate::wiki::WikiStore::load(
                handle
                    .memory()
                    .unwrap()
                    .store_dir()
                    .map(|dir| dir.join("wiki")),
            );
            wiki.upsert(crate::wiki::LedgerEntry {
                table: "dat".to_string(),
                dat: "units".to_string(),
                obj_id: 0,
                property: "HP".to_string(),
                value: serde_json::json!(marker),
                item_name: Some(marker.to_string()),
                applied_at: 1,
                edited_by_user: false,
            });
            wiki.save().unwrap();
        }
        // The launcher's record of the last opened project points elsewhere.
        let mut config = registry.dirs().load_config().unwrap();
        config.project_path = b.root().to_string_lossy().into_owned();
        registry.dirs().save_config(&config).unwrap();

        let providers = HandleProviders::new(&a);
        let memory = MemoryProvider::render_section(&providers);
        let state = ProjectStateProvider::render_section(&providers);
        let map = ProjectMapProvider::render_section(&providers).unwrap();
        let wiki = WikiProvider::render_section(&providers, "HP").unwrap_or_default();

        assert!(memory.contains("ALPHA-MEMORY"), "{memory}");
        assert!(!memory.contains("BETA-MEMORY"), "{memory}");
        assert!(state.contains("project=Alpha"), "{state}");
        assert!(!state.contains("Beta"), "{state}");
        assert!(map.contains("alpha_only.eps"), "{map}");
        assert!(!map.contains("beta_only.eps"), "{map}");
        assert!(!wiki.contains("BETA-MEMORY"), "{wiki}");
        std::fs::remove_dir_all(base).ok();
    }

    /// A closed project's providers render nothing of it.
    #[test]
    fn a_closed_projects_prompt_sections_read_nothing() {
        use crate::engine::{MemoryProvider, ProjectMapProvider, ProjectStateProvider};
        let (base, registry) = registry("registry-closed-prompts", 1);
        create_project(&base.join("a"), "Alpha");
        let a = registry.open(&base.join("a")).unwrap();
        assert!(a.memory().unwrap().write("resources", "ALPHA-MEMORY").ok);
        let providers = HandleProviders::new(&a);

        registry.close(a.id());

        assert!(!MemoryProvider::render_section(&providers).contains("ALPHA-MEMORY"));
        assert!(!ProjectStateProvider::render_section(&providers).contains("project=Alpha"));
        assert!(ProjectMapProvider::render_section(&providers).is_none());
        std::fs::remove_dir_all(base).ok();
    }

    /// Work running on one project never keeps another from closing: each
    /// project's close waits only for its own admitted work.
    #[test]
    fn closing_a_project_waits_only_for_its_own_work() {
        let (base, registry) = registry("registry-close-gate", 2);
        create_project(&base.join("a"), "A");
        create_project(&base.join("b"), "B");
        let a = registry.open(&base.join("a")).unwrap();
        let b = registry.open(&base.join("b")).unwrap();

        let b_work = b.admit().unwrap();
        let a_closing = a.close_gate().try_write().unwrap();
        // A is closing: new work on A is refused, B's keeps running.
        assert_eq!(a.admit().unwrap_err(), PROJECT_CLOSING);
        assert!(b.admit().is_ok());
        drop(a_closing);

        // Work on A keeps A open.
        let a_work = a.admit().unwrap();
        assert!(a.close_gate().try_write().is_err());
        drop(a_work);
        assert!(a.close_gate().try_write().is_ok());
        drop(b_work);

        registry.close(a.id());
        assert!(a.admit().is_err());
        assert!(b.admit().is_ok());
    }

    #[test]
    fn window_suffixes_are_unique_and_fixed_while_open() {
        // Two ids sharing their first 12 hex digits: the later one gets its
        // whole id.
        let first = ProjectId::from_stored(format!("abcdef012345{}", "0".repeat(52)));
        let second = ProjectId::from_stored(format!("abcdef012345{}", "1".repeat(52)));
        let taken = [unique_suffix(&first, std::iter::empty())];
        assert_eq!(taken[0], "abcdef012345");
        assert_eq!(unique_suffix(&second, taken.iter()), second.as_str());

        // An open project's windows keep their labels when others open or
        // close around it.
        let (base, registry) = registry("registry-suffix", 3);
        create_project(&base.join("a"), "A");
        create_project(&base.join("b"), "B");
        let a = registry.open(&base.join("a")).unwrap();
        let label = registry.map_window_label(a.id());
        assert_eq!(
            label,
            format!("{MAP_WINDOW_PREFIX}{}", &a.id().as_str()[..12])
        );
        let b = registry.open(&base.join("b")).unwrap();
        assert_eq!(registry.map_window_label(a.id()), label);
        registry.close(b.id());
        assert_eq!(registry.map_window_label(a.id()), label);
    }

    #[test]
    fn windows_resolve_to_the_project_they_are_bound_to() {
        let (base, registry) = registry("registry-windows", 2);
        create_project(&base.join("a"), "A");
        create_project(&base.join("b"), "B");
        let a = registry.open(&base.join("a")).unwrap();
        let b = registry.open(&base.join("b")).unwrap();
        registry.bind_window(MAIN_WINDOW_LABEL, a.id());
        registry.bind_window(&registry.map_window_label(a.id()), a.id());
        registry.bind_window("main-b", b.id());
        registry.bind_window(&registry.map_window_label(b.id()), b.id());

        assert_eq!(registry.for_window(MAIN_WINDOW_LABEL).unwrap().id(), a.id());
        assert_eq!(registry.for_window("main-b").unwrap().id(), b.id());
        assert_eq!(
            registry.for_window("map-import-unbound").unwrap_err(),
            NO_PROJECT_OPEN
        );
        // A project's events go to its windows only.
        let a_labels = registry.window_labels(a.id());
        assert_eq!(a_labels.len(), 2);
        assert!(a_labels.contains(&MAIN_WINDOW_LABEL.to_string()));
        assert!(!a_labels.iter().any(|label| label == "main-b"));
        assert!(!a_labels.contains(&registry.map_window_label(b.id())));
        assert_ne!(
            registry.map_window_label(a.id()),
            registry.map_window_label(b.id())
        );
        assert!(registry
            .map_window_label(a.id())
            .starts_with(MAP_WINDOW_PREFIX));

        // Closing a project unbinds its windows; a window closing unbinds itself.
        registry.close(a.id());
        assert!(registry.window_labels(a.id()).is_empty());
        assert!(registry.for_window(MAIN_WINDOW_LABEL).is_err());
        assert_eq!(registry.unbind_window("main-b"), Some(b.id().clone()));
        assert!(registry.for_window("main-b").is_err());
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn a_legacy_name_matches_only_a_unique_open_project() {
        let (base, registry) = registry("registry-name", 3);
        create_project(&base.join("a"), "Same");
        create_project(&base.join("b"), "Other");
        let a = registry.open(&base.join("a")).unwrap();
        registry.open(&base.join("b")).unwrap();
        assert_eq!(registry.unique_by_live_name("Same").unwrap().id(), a.id());

        create_project(&base.join("c"), "Same");
        registry.open(&base.join("c")).unwrap();
        assert!(registry.unique_by_live_name("Same").is_none());
        assert!(registry.unique_by_live_name("Missing").is_none());
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn each_handle_sees_only_its_own_build_marker() {
        let (base, registry) = registry("registry-building", 2);
        create_project(&base.join("a"), "A");
        create_project(&base.join("b"), "B");
        let a = registry.open(&base.join("a")).unwrap();
        let b = registry.open(&base.join("b")).unwrap();
        std::fs::create_dir_all(a.root().join("build")).unwrap();
        std::fs::write(a.root().join("build/.building"), b"").unwrap();

        assert!(a.is_building());
        assert!(!b.is_building());
        let b_map = b.root().join("maps/source.scx");
        assert_eq!(registry.owner_of(&b_map).unwrap().id(), b.id());
        assert!(!registry.owner_of(&b_map).unwrap().is_building());
        std::fs::remove_dir_all(base).ok();
    }
}
