//! Following a project folder that moved.
//!
//! A `ProjectId` is a hash of the canonical root, so moving the folder gives
//! the project a new id while its `.eud-agent` workspace ID stays. An EPS
//! session bound to the old id follows the move only when the old folder is
//! verifiably gone: a folder that was COPIED still holds the original project,
//! and a session never follows a copy. Anything that cannot be confirmed
//! (a detached drive, an unreadable folder, a probe that does not answer) is
//! reported as ambiguous and refused.

use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::Arc;
use std::time::Duration;

use crate::journal::{Journal, JournalTarget, Snapshot};
use crate::project_id::ProjectId;

/// How long one old-root candidate may take to answer before it counts as
/// ambiguous (a network path that does not respond).
pub(crate) const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
/// The most old-root candidates one rebind examines.
pub(crate) const MAX_CANDIDATES: usize = 32;

/// What a folder that once held the old project holds now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Probe {
    /// The folder is gone and its parent is still there: moved away.
    Missing,
    /// The folder is there but holds no project manifest.
    NoProject,
    /// The folder holds a project with this id.
    Project(ProjectId),
    /// The folder could not be examined.
    Unreadable(String),
}

/// Where the old project's folder stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OldRoot {
    /// Verifiably gone; `root` is `None` when no candidate named it (recents
    /// no longer list it), which cannot be told apart from a move.
    Gone { root: Option<PathBuf> },
    /// The old project is still there: the opened folder is a copy.
    StillThere(PathBuf),
    /// It could not be confirmed either way.
    Uncertain { root: PathBuf, reason: String },
}

pub(crate) type Prober = Arc<dyn Fn(&Path) -> Probe + Send + Sync>;

/// Examine `root` on the real filesystem.
pub(crate) fn probe_root(root: &Path) -> Probe {
    match std::fs::symlink_metadata(root) {
        Ok(metadata) if metadata.is_dir() => {
            match crate::native_project::NativeProject::open(root) {
                Ok(project) => Probe::Project(ProjectId::of(&project)),
                Err(error) => match holds_project_manifest(root) {
                    Ok(false) => Probe::NoProject,
                    Ok(true) => Probe::Unreadable(error),
                    Err(error) => Probe::Unreadable(error.to_string()),
                },
            }
        }
        Ok(_) => Probe::NoProject,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match root.parent().map(std::fs::symlink_metadata) {
                Some(Ok(parent)) if parent.is_dir() => Probe::Missing,
                Some(Err(parent)) => Probe::Unreadable(format!(
                    "상위 폴더도 찾을 수 없습니다(분리된 드라이브일 수 있습니다): {parent}"
                )),
                _ => Probe::Unreadable("상위 폴더를 확인할 수 없습니다.".to_string()),
            }
        }
        Err(error) => Probe::Unreadable(error.to_string()),
    }
}

fn holds_project_manifest(root: &Path) -> std::io::Result<bool> {
    for entry in std::fs::read_dir(root)? {
        let name = entry?.file_name().to_string_lossy().to_lowercase();
        if name.ends_with(".eap")
            || name == crate::native_project::LEGACY_PROJECT_MANIFEST_FILE
            || name.ends_with(".eudproj")
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Decide where the project `old` lived among `candidates` (roots named by
/// the session's own journals first, then the recents list), asking `probe`
/// about each root whose spelling hashes to `old`, each within `timeout`.
pub(crate) fn locate_old_root(
    old: &ProjectId,
    candidates: &[PathBuf],
    probe: Prober,
    timeout: Duration,
) -> OldRoot {
    let mut seen = std::collections::BTreeSet::new();
    let mut gone = None;
    let mut uncertain = None;
    for candidate in candidates {
        if seen.len() >= MAX_CANDIDATES {
            break;
        }
        if !seen.insert(path_key(candidate)) || !spells_project_id(candidate, old) {
            continue;
        }
        let verdict = probe_with_timeout(candidate, probe.clone(), timeout);
        match verdict {
            Probe::Missing | Probe::NoProject => {
                gone.get_or_insert_with(|| candidate.clone());
            }
            Probe::Project(id) if &id == old => return OldRoot::StillThere(candidate.clone()),
            Probe::Project(_) => {
                uncertain.get_or_insert_with(|| OldRoot::Uncertain {
                    root: candidate.clone(),
                    reason: "옛 폴더에 다른 프로젝트가 있습니다.".to_string(),
                });
            }
            Probe::Unreadable(reason) => {
                uncertain.get_or_insert_with(|| OldRoot::Uncertain {
                    root: candidate.clone(),
                    reason,
                });
            }
        }
    }
    uncertain.unwrap_or(OldRoot::Gone { root: gone })
}

fn probe_with_timeout(root: &Path, probe: Prober, timeout: Duration) -> Probe {
    let root = root.to_path_buf();
    with_timeout(timeout, move || probe(&root)).unwrap_or_else(|| {
        Probe::Unreadable("옛 폴더가 제한 시간 안에 응답하지 않았습니다.".to_string())
    })
}

/// Run a read-only filesystem look on its own thread and give up after
/// `timeout`. A look that hangs on a dead network share is left behind; it
/// only reads, and its answer is ignored once the timeout passed.
pub(crate) fn with_timeout<T: Send + 'static>(
    timeout: Duration,
    look: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(look());
    });
    receiver.recv_timeout(timeout).ok()
}

/// Whether `root`, however the user or a journal spelled it, is the folder
/// whose canonical form hashed to `id`.
fn spells_project_id(root: &Path, id: &ProjectId) -> bool {
    let text = root.to_string_lossy();
    let trimmed = text.trim_end_matches(['\\', '/']);
    let mut spellings = vec![trimmed.to_string()];
    if cfg!(windows) {
        let plain = strip_verbatim(trimmed).replace('/', "\\");
        spellings.push(match plain.strip_prefix(r"\\") {
            Some(unc) => format!(r"\\?\UNC\{unc}"),
            None => format!(r"\\?\{plain}"),
        });
        spellings.push(plain);
    }
    spellings.into_iter().any(|spelling| {
        crate::map_context::project_id_for_path(PathBuf::from(spelling)) == id.as_str()
    })
}

fn strip_verbatim(path: &str) -> String {
    if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else {
        path.strip_prefix(r"\\?\").unwrap_or(path).to_string()
    }
}

/// One comparable key per path component: prefixes spelled with or without
/// `\\?\` agree, and letter case is ignored.
fn component_keys(path: &Path) -> Vec<String> {
    path.components()
        .map(|component| match component {
            Component::Prefix(prefix) => match prefix.kind() {
                Prefix::Verbatim(text) => text.to_string_lossy().to_lowercase(),
                Prefix::VerbatimUNC(server, share) | Prefix::UNC(server, share) => {
                    format!("//{}/{}", server.to_string_lossy(), share.to_string_lossy())
                        .to_lowercase()
                }
                Prefix::VerbatimDisk(disk) | Prefix::Disk(disk) => {
                    format!("{}:", (disk as char).to_ascii_lowercase())
                }
                Prefix::DeviceNS(name) => format!("//./{}", name.to_string_lossy()).to_lowercase(),
            },
            Component::RootDir => "/".to_string(),
            Component::CurDir => ".".to_string(),
            Component::ParentDir => "..".to_string(),
            Component::Normal(part) => part.to_string_lossy().to_lowercase(),
        })
        .collect()
}

/// Whether `path` is `root` or lies under it, however either is spelled.
pub(crate) fn is_within(path: &Path, root: &Path) -> bool {
    let path = component_keys(path);
    let root = component_keys(root);
    path.len() >= root.len() && path[..root.len()] == root[..]
}

fn path_key(path: &Path) -> String {
    component_keys(path).join("|")
}

fn is_verbatim(path: &Path) -> bool {
    matches!(
        path.components().next(),
        Some(Component::Prefix(prefix)) if prefix.kind().is_verbatim()
    )
}

/// The root a recorded absolute source-map path lives under, given the
/// project-relative source map (`maps/source.scx`).
pub(crate) fn root_of_source_map(recorded: &Path, source_map: &str) -> Option<PathBuf> {
    let relative = source_map
        .split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>();
    let keys = component_keys(recorded);
    if relative.is_empty() || keys.len() <= relative.len() {
        return None;
    }
    let split = keys.len() - relative.len();
    if keys[split..] != relative[..] {
        return None;
    }
    Some(recorded.components().take(split).collect())
}

/// Rewrites absolute paths under the old root onto the new one.
#[derive(Debug, Clone)]
pub(crate) struct PathRewrite {
    old_root: Vec<String>,
    new_root: PathBuf,
}

impl PathRewrite {
    /// `new_root` is the canonical root of the opened project.
    pub(crate) fn new(old_root: &Path, new_root: &Path) -> Self {
        Self {
            old_root: component_keys(old_root),
            new_root: new_root.to_path_buf(),
        }
    }

    /// `path` moved onto the new root, or `None` when it is not under the old
    /// root (a relative path, a backup in AppData, another folder). The new
    /// path keeps the original's `\\?\` spelling or its absence.
    pub(crate) fn rewrite(&self, path: &Path) -> Option<PathBuf> {
        if !path.is_absolute() {
            return None;
        }
        let keys = component_keys(path);
        if keys.len() < self.old_root.len() || keys[..self.old_root.len()] != self.old_root[..] {
            return None;
        }
        let base = if is_verbatim(path) {
            self.new_root.clone()
        } else {
            PathBuf::from(strip_verbatim(&self.new_root.to_string_lossy()))
        };
        let mut rewritten = base;
        for component in path.components().skip(self.old_root.len()) {
            rewritten.push(component.as_os_str());
        }
        Some(rewritten)
    }

    fn rewrite_string(&self, value: &mut String) -> bool {
        match self.rewrite(Path::new(value.as_str())) {
            Some(path) => {
                *value = path.to_string_lossy().into_owned();
                true
            }
            None => false,
        }
    }
}

/// Move every absolute project path a journal recorded (targets and map
/// backup snapshots, never the AppData backup file) onto the new root.
/// Returns how many paths changed.
pub(crate) fn rewrite_journal(journal: &mut Journal, rewrite: &PathRewrite) -> usize {
    let mut changed = 0;
    for entry in &mut journal.entries {
        match &mut entry.target {
            JournalTarget::Path { path }
            | JournalTarget::ProjectManifest { path }
            | JournalTarget::Map { path, .. } => {
                changed += usize::from(rewrite.rewrite_string(path));
            }
            JournalTarget::Rename { from, to } => {
                changed += usize::from(rewrite.rewrite_string(from));
                changed += usize::from(rewrite.rewrite_string(to));
            }
            JournalTarget::MapSound { source_map, .. } => {
                if let Some(path) = rewrite.rewrite(source_map) {
                    *source_map = path;
                    changed += 1;
                }
            }
            JournalTarget::Dat { .. }
            | JournalTarget::WorkspacePath { .. }
            | JournalTarget::Setting { .. }
            | JournalTarget::Plugin { .. } => {}
        }
        for snapshot in [&mut entry.before, &mut entry.after] {
            if let Snapshot::MapBackup { map_path, .. } = snapshot {
                changed += usize::from(rewrite.rewrite_string(map_path));
            }
        }
    }
    changed
}

/// The old-root candidates of a session: roots its journals name through the
/// source map first, then the recents list, in order.
pub(crate) fn old_root_candidates(
    journals: &[Journal],
    source_map: &str,
    recents: &[crate::config::RecentProjectRecord],
) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    for journal in journals {
        for entry in &journal.entries {
            let mut recorded: Vec<PathBuf> = Vec::new();
            match &entry.target {
                JournalTarget::Map { path, .. } => recorded.push(PathBuf::from(path)),
                JournalTarget::MapSound { source_map, .. } => recorded.push(source_map.clone()),
                _ => {}
            }
            for snapshot in [&entry.before, &entry.after] {
                if let Snapshot::MapBackup { map_path, .. } = snapshot {
                    recorded.push(PathBuf::from(map_path));
                }
            }
            candidates.extend(
                recorded
                    .iter()
                    .filter(|path| path.is_absolute())
                    .filter_map(|path| root_of_source_map(path, source_map)),
            );
        }
    }
    candidates.extend(recents.iter().map(|recent| PathBuf::from(&recent.path)));
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{JournalEntry, WriteTool};
    use crate::project_id::test_support::{create_project, temp_root};

    fn entry(target: JournalTarget, before: Snapshot) -> JournalEntry {
        JournalEntry {
            id: "e".to_string(),
            seq: 1,
            tool: WriteTool::FileWrite,
            target,
            before,
            after: Snapshot::Created,
            ts: 1,
        }
    }

    #[test]
    fn a_rewrite_matches_any_spelling_of_the_old_root() {
        if !cfg!(windows) {
            return;
        }
        let rewrite = PathRewrite::new(Path::new(r"E:\Old\Proj"), Path::new(r"\\?\F:\New\Proj"));
        for (recorded, expected) in [
            (r"E:\Old\Proj\maps\a.scx", r"F:\New\Proj\maps\a.scx"),
            (r"\\?\E:\Old\Proj\maps\a.scx", r"\\?\F:\New\Proj\maps\a.scx"),
            (r"e:/old/proj/maps/a.scx", r"F:\New\Proj\maps\a.scx"),
            (r"E:\OLD\PROJ\maps\a.scx", r"F:\New\Proj\maps\a.scx"),
        ] {
            assert_eq!(
                rewrite.rewrite(Path::new(recorded)),
                Some(PathBuf::from(expected)),
                "{recorded}"
            );
        }
        for untouched in [
            r"E:\Old\Project\maps\a.scx",
            r"C:\Users\me\AppData\Roaming\eud-agent\map_backups\a.scx",
            "src/main.eps",
        ] {
            assert_eq!(rewrite.rewrite(Path::new(untouched)), None, "{untouched}");
        }
    }

    #[test]
    fn a_journal_rewrite_moves_targets_and_backup_snapshots_but_not_backups() {
        if !cfg!(windows) {
            return;
        }
        let rewrite = PathRewrite::new(Path::new(r"E:\Old\Proj"), Path::new(r"F:\New\Proj"));
        let mut journal = Journal {
            request_id: "r".to_string(),
            entries: vec![
                entry(
                    JournalTarget::Map {
                        path: r"E:\Old\Proj\maps\a.scx".to_string(),
                        summary: "location".to_string(),
                    },
                    Snapshot::Created,
                ),
                entry(
                    JournalTarget::Path {
                        path: "src/main.eps".to_string(),
                    },
                    Snapshot::MapBackup {
                        map_path: r"\\?\e:\old\proj\maps\a.scx".to_string(),
                        backup_path: r"C:\AppData\eud-agent\map_backups\x.scx".to_string(),
                    },
                ),
            ],
        };
        assert_eq!(rewrite_journal(&mut journal, &rewrite), 2);
        assert_eq!(
            journal.entries[0].target,
            JournalTarget::Map {
                path: r"F:\New\Proj\maps\a.scx".to_string(),
                summary: "location".to_string(),
            }
        );
        assert_eq!(
            journal.entries[1].target,
            JournalTarget::Path {
                path: "src/main.eps".to_string()
            }
        );
        assert_eq!(
            journal.entries[1].before,
            Snapshot::MapBackup {
                map_path: r"F:\New\Proj\maps\a.scx".to_string(),
                backup_path: r"C:\AppData\eud-agent\map_backups\x.scx".to_string(),
            }
        );
    }

    #[test]
    fn a_moved_folder_is_gone_and_a_copied_one_is_still_there() {
        let temp = temp_root("rebind-locate");
        let original = create_project(&temp.join("orig"), "Sample");
        let old = ProjectId::of(&original);
        let root = original.root().to_path_buf();

        let still = locate_old_root(
            &old,
            std::slice::from_ref(&root),
            Arc::new(probe_root),
            PROBE_TIMEOUT,
        );
        assert_eq!(still, OldRoot::StillThere(root.clone()));

        std::fs::rename(&root, temp.join("moved")).unwrap();
        let gone = locate_old_root(
            &old,
            std::slice::from_ref(&root),
            Arc::new(probe_root),
            PROBE_TIMEOUT,
        );
        assert_eq!(
            gone,
            OldRoot::Gone {
                root: Some(root.clone())
            }
        );

        // No candidate names the old root at all: indistinguishable from a move.
        let unknown = locate_old_root(
            &old,
            &[temp.join("unrelated")],
            Arc::new(probe_root),
            PROBE_TIMEOUT,
        );
        assert_eq!(unknown, OldRoot::Gone { root: None });
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn a_detached_or_unreadable_old_root_is_uncertain() {
        let temp = temp_root("rebind-detached");
        let original = create_project(&temp.join("drive").join("proj"), "Sample");
        let old = ProjectId::of(&original);
        let root = original.root().to_path_buf();
        // The whole parent vanished, the way a detached drive does.
        std::fs::remove_dir_all(temp.join("drive")).unwrap();
        let detached = locate_old_root(
            &old,
            std::slice::from_ref(&root),
            Arc::new(probe_root),
            PROBE_TIMEOUT,
        );
        assert!(
            matches!(detached, OldRoot::Uncertain { .. }),
            "{detached:?}"
        );

        let unreadable = locate_old_root(
            &old,
            std::slice::from_ref(&root),
            Arc::new(|_: &Path| Probe::Unreadable("access denied".to_string())),
            PROBE_TIMEOUT,
        );
        assert_eq!(
            unreadable,
            OldRoot::Uncertain {
                root: root.clone(),
                reason: "access denied".to_string()
            }
        );
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn an_unanswered_probe_is_uncertain_within_the_timeout() {
        let temp = temp_root("rebind-timeout");
        let original = create_project(&temp.join("proj"), "Sample");
        let old = ProjectId::of(&original);
        let started = std::time::Instant::now();
        let verdict = locate_old_root(
            &old,
            &[original.root().to_path_buf()],
            Arc::new(|_: &Path| {
                std::thread::sleep(Duration::from_secs(5));
                Probe::Missing
            }),
            Duration::from_millis(200),
        );
        assert!(matches!(verdict, OldRoot::Uncertain { .. }), "{verdict:?}");
        assert!(started.elapsed() < Duration::from_secs(2));
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn journals_name_the_old_root_through_the_source_map() {
        if !cfg!(windows) {
            return;
        }
        let journal = Journal {
            request_id: "r".to_string(),
            entries: vec![entry(
                JournalTarget::MapSound {
                    source_map: PathBuf::from(r"\\?\E:\Old\Proj\Maps\Source.scx"),
                    mpq_path: "staredit/wav/a.ogg".to_string(),
                    normalized_sha256: "0".repeat(64),
                },
                Snapshot::Created,
            )],
        };
        let candidates = old_root_candidates(&[journal], "maps/source.scx", &[]);
        assert_eq!(candidates, vec![PathBuf::from(r"\\?\E:\Old\Proj")]);
    }
}
