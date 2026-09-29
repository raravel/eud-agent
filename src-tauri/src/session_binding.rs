//! Binding stored sessions to the project they belong to.
//!
//! A session carries the [`ProjectId`] of its project. Sessions written before
//! ids existed carry only the manifest name and are backfilled once, under the
//! session store lock, when the open project is the one project that name can
//! mean. A session bound to an id follows its project folder when the folder
//! moved (same name, same workspace ID, old folder verifiably gone) and never
//! when it was copied. Whatever cannot be bound stays byte-for-byte as it was;
//! the reason is logged and is the error the session returns when it is opened.

use std::path::Path;
use std::sync::Arc;

use crate::config::DataDirs;
use crate::native_project::NativeProject;
use crate::project_id::ProjectId;
use crate::project_rebind::{OldRoot, PathRewrite, Prober};
use crate::session::{SessionKind, SessionProject, SessionRecord, SessionStore};

pub(crate) const NOT_THIS_PROJECT: &str = "이 세션은 현재 에디터 프로젝트에 속하지 않습니다.";

/// The explicitly opened project sessions are bound against.
pub(crate) struct OpenProject {
    pub project: NativeProject,
    pub session: SessionProject,
}

impl OpenProject {
    /// The project at `root`, with its workspace ID created if it had none.
    /// Only an explicit open calls this.
    pub(crate) fn open(dirs: &DataDirs, project: NativeProject) -> Self {
        let session = SessionProject::of_opened(dirs, &project);
        Self { project, session }
    }
}

#[derive(Clone)]
pub(crate) struct SessionBinder {
    sessions: SessionStore,
    dirs: DataDirs,
    probe: Prober,
}

impl SessionBinder {
    pub(crate) fn new(sessions: SessionStore, dirs: DataDirs) -> Self {
        Self {
            sessions,
            dirs,
            probe: Arc::new(crate::project_rebind::probe_root),
        }
    }

    /// Replace how a moved session's old folder is examined.
    #[cfg(test)]
    pub(crate) fn with_probe(mut self, probe: Prober) -> Self {
        self.probe = probe;
        self
    }

    fn report(session_id: &str, report: &str) {
        eprintln!("eud-agent: session {session_id}: {report}");
    }

    /// Bind every stored session the open project owns and can take over:
    /// Map sessions without an id, legacy EPS sessions that name it, bound
    /// sessions without a workspace ID, and sessions whose folder moved here.
    pub(crate) fn bind_open_project_sessions(&self, open: &OpenProject) -> Result<(), String> {
        let metas = self.sessions.list().map_err(|error| error.to_string())?;
        let open_id = open.session.id.as_str();
        let mut ambiguity = None;
        for meta in metas {
            let candidate = match (meta.kind, meta.project_id.as_deref()) {
                (SessionKind::Map, bound) => bound.is_none(),
                (SessionKind::Eps, None) => meta.project == open.session.name,
                (SessionKind::Eps, Some(id)) if id == open_id => meta.workspace_id.is_none(),
                // Bound to another id: a folder move is the only way here.
                (SessionKind::Eps, Some(_)) => {
                    meta.workspace_id.is_some()
                        && meta.workspace_id == open.session.workspace_id
                        && meta.project == open.session.name
                }
            };
            if !candidate {
                continue;
            }
            let record = match self.sessions.load(&meta.id) {
                Ok(record) => record,
                Err(error) => {
                    Self::report(&meta.id, &error.to_string());
                    continue;
                }
            };
            if let Err(error) = self.bind_with(record, Some(open), &mut ambiguity) {
                if error != NOT_THIS_PROJECT {
                    Self::report(&meta.id, &error);
                }
            }
        }
        Ok(())
    }

    /// Give `record` the project id it belongs to before any engine uses it.
    ///
    /// A Map session already keys its project by id and copies it. An EPS
    /// session of the open project is backfilled when its name names only
    /// the open project among the recent projects; one bound to another id
    /// follows a folder move only when the old folder is verifiably gone.
    /// With no project open, the record is left as it is.
    pub(crate) fn bind(
        &self,
        record: SessionRecord,
        open: Option<&OpenProject>,
    ) -> Result<SessionRecord, String> {
        self.bind_with(record, open, &mut None)
    }

    /// [`Self::bind`], reusing `ambiguity` (the open project's legacy-name
    /// verdict) across the sessions of one pass so the recents are examined
    /// once.
    fn bind_with(
        &self,
        record: SessionRecord,
        open: Option<&OpenProject>,
        ambiguity: &mut Option<Option<String>>,
    ) -> Result<SessionRecord, String> {
        let to_error = |error: anyhow::Error| error.to_string();
        if record.meta.kind == SessionKind::Map {
            if record.meta.project_id.is_some() {
                return Ok(record);
            }
            let workspace = open
                .filter(|open| open.session.id.as_str() == record.meta.project)
                .and_then(|open| open.session.workspace_id.clone());
            return self
                .sessions
                .update_project_id(
                    &record.meta.id,
                    &ProjectId::from_stored(record.meta.project.clone()),
                    workspace.as_deref(),
                )
                .map_err(to_error);
        }
        let Some(open) = open else {
            return Ok(record);
        };
        let workspace = open.session.workspace_id.as_deref();
        match record.meta.project_id.clone() {
            Some(id) if id == open.session.id.as_str() => {
                if record.meta.workspace_id.is_some() {
                    return Ok(record);
                }
                self.sessions
                    .update_project_id(&record.meta.id, &open.session.id, workspace)
                    .map_err(to_error)
            }
            Some(old) => self.rebind_moved(&record, open, &ProjectId::from_stored(old)),
            None => {
                if record.meta.project != open.session.name {
                    return Err(NOT_THIS_PROJECT.to_string());
                }
                if ambiguity.is_none() {
                    *ambiguity = Some(self.legacy_name_ambiguity(open)?);
                }
                if let Some(report) = ambiguity.clone().flatten() {
                    Self::report(&record.meta.id, &report);
                    return Err(report);
                }
                self.sessions
                    .update_project_id(&record.meta.id, &open.session.id, workspace)
                    .map_err(to_error)
            }
        }
    }

    /// A report when another recent project carries the open project's
    /// name, so a session that names it cannot tell which one it belongs to.
    /// Recents that cannot be opened are skipped: their names are unknown.
    fn legacy_name_ambiguity(&self, open: &OpenProject) -> Result<Option<String>, String> {
        let config = self.dirs.load_config().map_err(|error| error.to_string())?;
        let open_key =
            crate::config::RecentProjectRecord::key(&open.project.root().to_string_lossy());
        let mut unreadable = 0_usize;
        for recent in &config.project_recents {
            if crate::config::RecentProjectRecord::key(&recent.path) == open_key {
                continue;
            }
            let path = recent.path.clone();
            let opened = crate::project_rebind::with_timeout(
                crate::project_rebind::PROBE_TIMEOUT,
                move || {
                    NativeProject::open(Path::new(&path))
                        .map(|project| (project.manifest().name.clone(), ProjectId::of(&project)))
                },
            );
            match opened {
                Some(Ok((name, id))) => {
                    if name == open.session.name && id != open.session.id {
                        return Ok(Some(format!(
                            "같은 이름({})을 가진 다른 프로젝트가 있어 이 세션이 어느 프로젝트의 것인지 확인할 수 없습니다: {}. 이 세션을 만든 프로젝트 폴더를 열어 주세요.",
                            open.session.name, recent.path
                        )));
                    }
                }
                Some(Err(_)) => unreadable += 1,
                // Unanswered is not "unknown name": it could carry this name.
                None => {
                    return Ok(Some(format!(
                        "최근 프로젝트 {}가 응답하지 않아 이 세션이 어느 프로젝트의 것인지 확인할 수 없습니다. 그 폴더를 연결하거나 이 세션을 만든 프로젝트 폴더를 열어 주세요.",
                        recent.path
                    )));
                }
            }
        }
        if unreadable > 0 {
            eprintln!(
                "eud-agent: {unreadable} recent project(s) could not be opened while binding legacy sessions of '{}'",
                open.session.name
            );
        }
        Ok(None)
    }

    /// Follow a folder move: bind `record` (bound to `old`) to the open
    /// project when it has the same name and workspace ID and the folder
    /// that held `old` is verifiably gone. A copy (the old folder still holds
    /// the project) and anything unconfirmable are refused with a report.
    fn rebind_moved(
        &self,
        record: &SessionRecord,
        open: &OpenProject,
        old: &ProjectId,
    ) -> Result<SessionRecord, String> {
        let Some(workspace) = open.session.workspace_id.as_deref() else {
            return Err(NOT_THIS_PROJECT.to_string());
        };
        // One project is open at a time, so `old` is never an open project
        // here: it differs from the open project's id.
        if record.meta.project != open.session.name
            || record.meta.workspace_id.as_deref() != Some(workspace)
            || old == &open.session.id
        {
            return Err(NOT_THIS_PROJECT.to_string());
        }
        let mut journals = Vec::new();
        for request_id in &record.pending_request_ids {
            match crate::journal::JournalStore::load(self.dirs.app_data(), request_id) {
                Ok(journal) => journals.push(journal),
                Err(crate::journal::JournalError::Io(error))
                    if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    // A review that cannot be read cannot be checked for paths
                    // under the old folder.
                    let report = format!(
                        "이 세션의 대기 중인 변경 `{request_id}`을 읽을 수 없어 이 프로젝트에 연결하지 않았습니다: {error}. 원래 프로젝트 폴더를 열어 주세요."
                    );
                    Self::report(&record.meta.id, &report);
                    return Err(report);
                }
            }
        }
        let recents = self
            .dirs
            .load_config()
            .map(|config| config.project_recents)
            .unwrap_or_default();
        let candidates = crate::project_rebind::old_root_candidates(
            &journals,
            &open.project.manifest().source_map,
            &recents,
        );
        let verdict = crate::project_rebind::locate_old_root(
            old,
            &candidates,
            self.probe.clone(),
            crate::project_rebind::PROBE_TIMEOUT,
        );
        let old_root = match verdict {
            OldRoot::StillThere(root) => {
                let report = format!(
                    "같은 작업 공간을 가진 다른 폴더가 있습니다: {}. 원본 폴더를 열거나 복사본의 `.eud-agent/state/workspace.json`을 정리해 주세요.",
                    root.display()
                );
                Self::report(&record.meta.id, &report);
                return Err(report);
            }
            OldRoot::Uncertain { root, reason } => {
                let report = format!(
                    "이 세션의 원래 프로젝트 폴더({})를 확인할 수 없어 이 프로젝트에 연결하지 않았습니다: {reason}. 원래 폴더가 있는 드라이브를 연결하거나 폴더 상태를 확인한 뒤 다시 열어 주세요.",
                    root.display()
                );
                Self::report(&record.meta.id, &report);
                return Err(report);
            }
            OldRoot::Gone { root } => root,
        };
        if old_root.is_none() && journals.iter().any(journal_names_an_absolute_path) {
            // Its pending review names files by absolute path under a folder
            // no candidate identified; following would strand those paths.
            let report = "이 세션의 대기 중인 변경이 원래 폴더의 경로를 가리키는데, 원래 폴더의 위치를 확인할 수 없어 연결하지 않았습니다. 최근 프로젝트 목록에 원래 폴더를 다시 열어 주세요.".to_string();
            Self::report(&record.meta.id, &report);
            return Err(report);
        }
        let rewrite = old_root
            .as_deref()
            .map(|root| PathRewrite::new(root, open.project.root()));
        let rebound = self
            .sessions
            .rebind_project(
                &record.meta.id,
                old,
                &open.session.id,
                workspace,
                rewrite.as_ref(),
            )
            .map_err(|error| error.to_string())?;
        if old_root.is_none() {
            Self::report(
                &record.meta.id,
                "원래 프로젝트 폴더의 위치를 알 수 없어 폴더 이동으로 보고 이 프로젝트에 연결했습니다.",
            );
        }
        Ok(rebound)
    }
}

fn journal_names_an_absolute_path(journal: &crate::journal::Journal) -> bool {
    use crate::journal::{JournalTarget, Snapshot};
    journal.entries.iter().any(|entry| {
        let target = match &entry.target {
            JournalTarget::Map { path, .. } => Path::new(path).is_absolute(),
            JournalTarget::MapSound { source_map, .. } => source_map.is_absolute(),
            _ => false,
        };
        target
            || [&entry.before, &entry.after].into_iter().any(|snapshot| {
                matches!(snapshot, Snapshot::MapBackup { map_path, .. } if Path::new(map_path).is_absolute())
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{Journal, JournalEntry, JournalTarget, Snapshot, WriteTool};
    use crate::project_id::test_support::{create_project, temp_root};
    use crate::session::SessionMeta;
    use std::path::PathBuf;

    struct Fixture {
        base: PathBuf,
        dirs: DataDirs,
        sessions: SessionStore,
    }

    impl Fixture {
        fn new(label: &str) -> Self {
            let base = temp_root(label);
            let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
            dirs.ensure_dirs().unwrap();
            let sessions = SessionStore::new(&dirs);
            Self {
                base,
                dirs,
                sessions,
            }
        }

        fn binder(&self) -> SessionBinder {
            SessionBinder::new(self.sessions.clone(), self.dirs.clone())
        }

        /// Open `root` explicitly, as the launcher does, and list it in recents.
        fn open(&self, root: &Path) -> OpenProject {
            let project = NativeProject::open(root).unwrap();
            let mut config = self.dirs.load_config().unwrap();
            config.record_recent_project(
                project.manifest().name.clone(),
                root.to_string_lossy().into_owned(),
                crate::session::now_unix_millis(),
            );
            self.dirs.save_config(&config).unwrap();
            OpenProject::open(&self.dirs, project)
        }

        fn remember(&self, root: &Path, name: &str) {
            let mut config = self.dirs.load_config().unwrap();
            config.record_recent_project(
                name.to_string(),
                root.to_string_lossy().into_owned(),
                crate::session::now_unix_millis(),
            );
            self.dirs.save_config(&config).unwrap();
        }

        fn save(&self, meta: SessionMeta) -> SessionRecord {
            let binding = crate::provider::ProviderBinding::new(
                crate::provider::ProviderId::Codex,
                "gpt-test".to_string(),
                None,
            )
            .unwrap();
            let record = SessionRecord {
                meta,
                provider_binding: binding,
                pending_request_ids: Vec::new(),
                context_usage: None,
                panel_log: serde_json::Value::Null,
                context_state: Default::default(),
                task_state: Default::default(),
                autonomous_run: None,
                team_tasks: Vec::new(),
                workers: Vec::new(),
            };
            self.sessions.save(&record).unwrap();
            record
        }

        fn record_bytes(&self, id: &str) -> Vec<u8> {
            std::fs::read(self.dirs.sessions_dir().join(format!("{id}.json"))).unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    fn legacy_meta(kind: SessionKind, project: &str) -> SessionMeta {
        SessionMeta {
            id: crate::session::new_session_id(),
            name: "legacy".to_string(),
            project: project.to_string(),
            kind,
            created_at: 1,
            provider: crate::provider::ProviderId::Codex,
            model: "gpt-test".to_string(),
            last_conversation_at: 1,
            team_parent: None,
            worker_parent: None,
            project_id: None,
            workspace_id: None,
        }
    }

    fn index_meta(fixture: &Fixture, id: &str) -> SessionMeta {
        fixture
            .sessions
            .list()
            .unwrap()
            .into_iter()
            .find(|meta| meta.id == id)
            .unwrap()
    }

    #[test]
    fn a_legacy_session_that_names_only_the_open_project_is_backfilled() {
        let fixture = Fixture::new("bind-legacy");
        let root = fixture.base.join("a");
        create_project(&root, "Sample");
        let open = fixture.open(&root);
        let legacy = fixture.save(legacy_meta(SessionKind::Eps, "Sample"));

        let bound = fixture.binder().bind(legacy.clone(), Some(&open)).unwrap();

        assert_eq!(
            bound.meta.project_id.as_deref(),
            Some(open.session.id.as_str())
        );
        assert_eq!(bound.meta.workspace_id, open.session.workspace_id);
        assert_eq!(bound.meta.project, "Sample");
        let listed = index_meta(&fixture, &legacy.meta.id);
        assert_eq!(listed.project_id.as_deref(), Some(open.session.id.as_str()));
    }

    #[test]
    fn a_legacy_session_whose_name_another_recent_project_carries_is_left_untouched() {
        let fixture = Fixture::new("bind-ambiguous");
        let root = fixture.base.join("a");
        create_project(&root, "Sample");
        let other = fixture.base.join("b");
        create_project(&other, "Sample");
        fixture.remember(&other, "Sample");
        // A recent that no longer opens has no known name and is ignored.
        fixture.remember(&fixture.base.join("deleted"), "Sample");
        let open = fixture.open(&root);
        let legacy = fixture.save(legacy_meta(SessionKind::Eps, "Sample"));
        let before = fixture.record_bytes(&legacy.meta.id);
        let binder = fixture.binder();
        binder.bind_open_project_sessions(&open).unwrap();
        assert_eq!(fixture.record_bytes(&legacy.meta.id), before);

        let error = binder.bind(legacy.clone(), Some(&open)).unwrap_err();

        assert!(error.contains("같은 이름"), "{error}");
        assert_eq!(fixture.record_bytes(&legacy.meta.id), before);
    }

    #[test]
    fn an_unopenable_recent_does_not_make_a_legacy_name_ambiguous() {
        let fixture = Fixture::new("bind-unreadable-recent");
        let root = fixture.base.join("a");
        create_project(&root, "Sample");
        fixture.remember(&fixture.base.join("deleted"), "Sample");
        let open = fixture.open(&root);
        let legacy = fixture.save(legacy_meta(SessionKind::Eps, "Sample"));

        let bound = fixture.binder().bind(legacy, Some(&open)).unwrap();

        assert_eq!(
            bound.meta.project_id.as_deref(),
            Some(open.session.id.as_str())
        );
    }

    #[test]
    fn a_legacy_session_of_another_project_is_refused_unchanged() {
        let fixture = Fixture::new("bind-other-name");
        let root = fixture.base.join("a");
        create_project(&root, "Sample");
        let open = fixture.open(&root);
        let other = fixture.save(legacy_meta(SessionKind::Eps, "Other"));
        let before = fixture.record_bytes(&other.meta.id);

        let error = fixture
            .binder()
            .bind(other.clone(), Some(&open))
            .unwrap_err();

        assert_eq!(error, NOT_THIS_PROJECT);
        assert_eq!(fixture.record_bytes(&other.meta.id), before);
    }

    #[test]
    fn a_map_session_copies_its_project_key_without_any_lookup() {
        let fixture = Fixture::new("bind-map");
        let map = fixture.save(legacy_meta(SessionKind::Map, &"c".repeat(64)));

        // No project is open: a Map session still binds, by the key it has.
        let bound = fixture.binder().bind(map.clone(), None).unwrap();

        assert_eq!(
            bound.meta.project_id.as_deref(),
            Some(map.meta.project.as_str())
        );
        assert_eq!(bound.meta.workspace_id, None);
    }

    fn map_journal(request_id: &str, source_map: &Path) -> Journal {
        let entry = |id: &str, seq, target, before| JournalEntry {
            id: id.to_string(),
            seq,
            tool: WriteTool::FileWrite,
            target,
            before,
            after: Snapshot::Created,
            ts: 1,
        };
        Journal {
            request_id: request_id.to_string(),
            entries: vec![
                entry(
                    "location",
                    1,
                    JournalTarget::Map {
                        path: source_map.to_string_lossy().into_owned(),
                        summary: "location".to_string(),
                    },
                    Snapshot::MapBackup {
                        map_path: source_map.to_string_lossy().into_owned(),
                        backup_path: "C:/AppData/eud-agent/map_backups/x.scx".to_string(),
                    },
                ),
                entry(
                    "source",
                    2,
                    JournalTarget::Path {
                        path: "src/main.eps".to_string(),
                    },
                    Snapshot::Created,
                ),
            ],
        }
    }

    fn write_journal(fixture: &Fixture, journal: &Journal) {
        let path = fixture
            .dirs
            .app_data()
            .join("journal")
            .join(format!("{}.json", journal.request_id));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::to_vec_pretty(journal).unwrap()).unwrap();
    }

    fn read_journal(fixture: &Fixture, request_id: &str) -> Journal {
        crate::journal::JournalStore::load(fixture.dirs.app_data(), request_id).unwrap()
    }

    /// A session of the project at `root` with a pending review and a paused
    /// autonomous run, both recorded against the project's current id.
    fn bound_session_with_review(fixture: &Fixture, root: &Path) -> (SessionRecord, OpenProject) {
        let open = fixture.open(root);
        let binding = crate::provider::ProviderBinding::new(
            crate::provider::ProviderId::Codex,
            "gpt-test".to_string(),
            None,
        )
        .unwrap();
        let meta = SessionMeta::new_for(SessionKind::Eps, "work", &binding, &open.session, None);
        let mut record = fixture.save(meta);
        let source_map = open.project.root().join("maps").join("source.scx");
        write_journal(fixture, &map_journal("req-pending", &source_map));
        record.pending_request_ids = vec!["req-pending".to_string()];
        let mut run = crate::autonomous::AutonomousRunState::new(
            "goal".to_string(),
            "turn".to_string(),
            "req-auto".to_string(),
            open.session.id.as_str().to_string(),
            "rev".to_string(),
            Default::default(),
            1,
        );
        run.status = crate::autonomous::AutonomousRunStatus::Paused;
        record.autonomous_run = Some(run);
        fixture.sessions.save(&record).unwrap();
        (record, open)
    }

    #[test]
    fn a_moved_project_takes_its_sessions_reviews_and_runs_along() {
        let fixture = Fixture::new("bind-moved");
        let root = fixture.base.join("old").join("proj");
        create_project(&root, "Sample");
        let (record, before) = bound_session_with_review(&fixture, &root);
        let old_root = before.project.root().to_path_buf();

        let moved = fixture.base.join("new").join("proj");
        std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
        std::fs::rename(&root, &moved).unwrap();
        let after = fixture.open(&moved);
        assert_ne!(after.session.id, before.session.id);
        assert_eq!(after.session.workspace_id, before.session.workspace_id);

        let binder = fixture.binder();
        binder.bind_open_project_sessions(&after).unwrap();

        let rebound = fixture.sessions.load(&record.meta.id).unwrap();
        assert_eq!(
            rebound.meta.project_id.as_deref(),
            Some(after.session.id.as_str())
        );
        assert_eq!(
            rebound.autonomous_run.as_ref().unwrap().project_id,
            after.session.id.as_str()
        );
        assert_eq!(rebound.pending_request_ids, vec!["req-pending".to_string()]);
        let listed = index_meta(&fixture, &record.meta.id);
        assert_eq!(
            listed.project_id.as_deref(),
            Some(after.session.id.as_str())
        );

        // The review's absolute paths now name the new folder; relative ones
        // are untouched, and nothing points at (or was created in) the old one.
        let journal = read_journal(&fixture, "req-pending");
        let new_map = after.project.root().join("maps").join("source.scx");
        assert_eq!(
            journal.entries[0].target,
            JournalTarget::Map {
                path: new_map.to_string_lossy().into_owned(),
                summary: "location".to_string(),
            }
        );
        assert!(matches!(
            &journal.entries[0].before,
            Snapshot::MapBackup { map_path, backup_path }
                if Path::new(map_path) == new_map
                    && backup_path == "C:/AppData/eud-agent/map_backups/x.scx"
        ));
        assert_eq!(
            journal.entries[1].target,
            JournalTarget::Path {
                path: "src/main.eps".to_string()
            }
        );
        assert!(!old_root.exists());

        // The restored review belongs to the same session under the new id.
        let writes = crate::write_coordinator::ProjectWriteCoordinator::silent();
        crate::engine::restore_pending_review(
            &fixture.sessions,
            &fixture.dirs,
            &writes,
            &after.session.id,
        )
        .unwrap();
        assert!(writes.owns(
            &crate::write_coordinator::TxnKey::eps(&after.session.id),
            &record.meta.id,
            "req-pending"
        ));
    }

    #[test]
    fn a_copied_project_never_takes_the_original_sessions() {
        let fixture = Fixture::new("bind-copied");
        let root = fixture.base.join("orig");
        create_project(&root, "Sample");
        let (record, original) = bound_session_with_review(&fixture, &root);
        let copy = fixture.base.join("copy");
        copy_dir(&root, &copy);
        let before = fixture.record_bytes(&record.meta.id);
        let journal_before = read_journal(&fixture, "req-pending");

        let copied = fixture.open(&copy);
        assert_eq!(copied.session.workspace_id, original.session.workspace_id);
        let binder = fixture.binder();
        binder.bind_open_project_sessions(&copied).unwrap();
        let error = binder
            .bind(
                fixture.sessions.load(&record.meta.id).unwrap(),
                Some(&copied),
            )
            .unwrap_err();

        assert!(error.contains("같은 작업 공간을 가진 다른 폴더"), "{error}");
        assert_eq!(fixture.record_bytes(&record.meta.id), before);
        assert_eq!(read_journal(&fixture, "req-pending"), journal_before);

        // The original still owns it, and opening the copy again changes nothing.
        let reopened = fixture.open(&root);
        let bound = binder
            .bind(
                fixture.sessions.load(&record.meta.id).unwrap(),
                Some(&reopened),
            )
            .unwrap();
        assert_eq!(
            bound.meta.project_id.as_deref(),
            Some(original.session.id.as_str())
        );
        let copied = fixture.open(&copy);
        binder.bind_open_project_sessions(&copied).unwrap();
        assert_eq!(fixture.record_bytes(&record.meta.id), before);
    }

    #[test]
    fn a_same_named_project_with_another_workspace_never_takes_a_session() {
        let fixture = Fixture::new("bind-other-workspace");
        let root = fixture.base.join("a");
        create_project(&root, "Sample");
        let (record, _) = bound_session_with_review(&fixture, &root);
        std::fs::remove_dir_all(&root).unwrap();
        let fresh = fixture.base.join("b");
        create_project(&fresh, "Sample");
        let before = fixture.record_bytes(&record.meta.id);

        let open = fixture.open(&fresh);
        let binder = fixture.binder();
        binder.bind_open_project_sessions(&open).unwrap();
        let error = binder
            .bind(fixture.sessions.load(&record.meta.id).unwrap(), Some(&open))
            .unwrap_err();

        assert_eq!(error, NOT_THIS_PROJECT);
        assert_eq!(fixture.record_bytes(&record.meta.id), before);
    }

    #[test]
    fn a_moved_session_whose_old_folder_cannot_be_checked_is_refused() {
        let fixture = Fixture::new("bind-uncertain");
        let root = fixture.base.join("old");
        create_project(&root, "Sample");
        let (record, _) = bound_session_with_review(&fixture, &root);
        let moved = fixture.base.join("new");
        std::fs::rename(&root, &moved).unwrap();
        let before = fixture.record_bytes(&record.meta.id);

        let open = fixture.open(&moved);
        let binder = fixture.binder().with_probe(Arc::new(|_: &Path| {
            crate::project_rebind::Probe::Unreadable("network share offline".to_string())
        }));
        let error = binder
            .bind(fixture.sessions.load(&record.meta.id).unwrap(), Some(&open))
            .unwrap_err();

        assert!(error.contains("network share offline"), "{error}");
        assert_eq!(fixture.record_bytes(&record.meta.id), before);
    }

    #[test]
    fn a_project_without_workspace_state_gets_one_id_used_everywhere() {
        let fixture = Fixture::new("bind-no-workspace");
        let root = fixture.base.join("a");
        create_project(&root, "Sample");
        let state = root.join(".eud-agent/state/workspace.json");
        assert!(!state.exists());
        let legacy = fixture.save(legacy_meta(SessionKind::Eps, "Sample"));

        let open = fixture.open(&root);
        assert!(state.exists());
        let workspace_id = open.session.workspace_id.clone().unwrap();
        let bound = fixture.binder().bind(legacy, Some(&open)).unwrap();
        assert_eq!(
            bound.meta.workspace_id.as_deref(),
            Some(workspace_id.as_str())
        );

        // The first EPS turn prepares its workspace under the same ID.
        let snapshot = crate::source_snapshot::ProjectSnapshot {
            project: "Sample".to_string(),
            identity: open.project.root().to_string_lossy().into_owned(),
        };
        let prepared = crate::workspace::WorkspaceManager::new(fixture.dirs.clone())
            .prepare_snapshot(&snapshot)
            .unwrap();
        assert_eq!(prepared.id, workspace_id);
    }

    #[test]
    fn a_legacy_pending_review_is_restored_to_its_session_under_the_id() {
        let fixture = Fixture::new("bind-legacy-review");
        let root = fixture.base.join("a");
        create_project(&root, "Sample");
        let open = fixture.open(&root);
        let mut legacy = fixture.save(legacy_meta(SessionKind::Eps, "Sample"));
        write_journal(
            &fixture,
            &map_journal("req-legacy", &open.project.root().join("maps/source.scx")),
        );
        legacy.pending_request_ids = vec!["req-legacy".to_string()];
        fixture.sessions.save(&legacy).unwrap();

        fixture.binder().bind_open_project_sessions(&open).unwrap();
        let writes = crate::write_coordinator::ProjectWriteCoordinator::silent();
        let errors = crate::engine::restore_pending_review(
            &fixture.sessions,
            &fixture.dirs,
            &writes,
            &open.session.id,
        )
        .unwrap();

        assert!(errors.is_empty(), "{errors:?}");
        assert!(writes.owns(
            &crate::write_coordinator::TxnKey::eps(&open.session.id),
            &legacy.meta.id,
            "req-legacy"
        ));
        let bound = fixture.sessions.load(&legacy.meta.id).unwrap();
        assert_eq!(bound.pending_request_ids, vec!["req-legacy".to_string()]);
    }

    #[test]
    fn an_unusable_workspace_state_never_blocks_binding() {
        let fixture = Fixture::new("bind-bad-workspace");
        let root = fixture.base.join("a");
        create_project(&root, "Sample");
        let state = root.join(".eud-agent/state/workspace.json");
        std::fs::create_dir_all(state.parent().unwrap()).unwrap();
        // A user "cleaned" the file into an inconsistent state.
        std::fs::write(
            &state,
            br#"{"version":1,"id":"abc","project":"Sample","identityHash":"other"}"#,
        )
        .unwrap();
        let before = std::fs::read(&state).unwrap();
        let legacy = fixture.save(legacy_meta(SessionKind::Eps, "Sample"));

        let open = fixture.open(&root);
        let bound = fixture.binder().bind(legacy, Some(&open)).unwrap();

        assert_eq!(open.session.workspace_id, None);
        assert_eq!(
            bound.meta.project_id.as_deref(),
            Some(open.session.id.as_str())
        );
        assert_eq!(bound.meta.workspace_id, None);
        assert_eq!(std::fs::read(&state).unwrap(), before);
    }

    #[test]
    fn an_open_never_rewrites_an_unchanged_workspace_state() {
        let fixture = Fixture::new("bind-workspace-unchanged");
        let root = fixture.base.join("a");
        create_project(&root, "Sample");
        let first = fixture.open(&root);
        let state = root.join(".eud-agent/state/workspace.json");
        let modified = std::fs::metadata(&state).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));

        let second = fixture.open(&root);

        assert_eq!(second.session.workspace_id, first.session.workspace_id);
        assert_eq!(
            std::fs::metadata(&state).unwrap().modified().unwrap(),
            modified
        );
    }

    #[test]
    fn a_move_with_absolute_review_paths_and_no_known_old_folder_is_refused() {
        let fixture = Fixture::new("bind-unknown-old-root");
        let root = fixture.base.join("old");
        create_project(&root, "Sample");
        let (mut record, _) = bound_session_with_review(&fixture, &root);
        // The review names a map outside any folder a candidate could name.
        let elsewhere = fixture.base.join("elsewhere").join("other.scx");
        write_journal(&fixture, &map_journal("req-elsewhere", &elsewhere));
        record = fixture.sessions.load(&record.meta.id).unwrap();
        record.pending_request_ids = vec!["req-elsewhere".to_string()];
        fixture.sessions.save(&record).unwrap();
        let moved = fixture.base.join("new");
        std::fs::rename(&root, &moved).unwrap();
        let open = fixture.open(&moved);
        // The recents no longer list the old folder.
        let mut config = fixture.dirs.load_config().unwrap();
        config
            .project_recents
            .retain(|recent| Path::new(&recent.path) != root.as_path());
        fixture.dirs.save_config(&config).unwrap();
        let before = fixture.record_bytes(&record.meta.id);

        let error = fixture
            .binder()
            .bind(fixture.sessions.load(&record.meta.id).unwrap(), Some(&open))
            .unwrap_err();

        assert!(
            error.contains("원래 폴더의 위치를 확인할 수 없어"),
            "{error}"
        );
        assert_eq!(fixture.record_bytes(&record.meta.id), before);
    }

    #[test]
    fn a_move_whose_review_cannot_be_read_is_refused() {
        let fixture = Fixture::new("bind-unreadable-review");
        let root = fixture.base.join("old");
        create_project(&root, "Sample");
        let (record, _) = bound_session_with_review(&fixture, &root);
        let journal = fixture
            .dirs
            .app_data()
            .join("journal")
            .join("req-pending.json");
        std::fs::write(&journal, b"{ not json").unwrap();
        let moved = fixture.base.join("new");
        std::fs::rename(&root, &moved).unwrap();
        let open = fixture.open(&moved);
        let before = fixture.record_bytes(&record.meta.id);

        let error = fixture
            .binder()
            .bind(fixture.sessions.load(&record.meta.id).unwrap(), Some(&open))
            .unwrap_err();

        assert!(error.contains("req-pending"), "{error}");
        assert_eq!(fixture.record_bytes(&record.meta.id), before);
    }

    fn copy_dir(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_dir(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }
}
