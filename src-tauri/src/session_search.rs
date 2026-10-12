//! Content search over the session sidebar's conversations.
//!
//! A session record carries base64 previews and tool details (tens of MB for
//! a long session), while its searchable text is only the `panelLog.log[]`
//! entries of kind `you` and `agent`. That text is extracted once per record
//! version into a regenerable sidecar under Local app data
//! (`session-search/{id}.json`, keyed by the record's size + mtime) and kept
//! in memory, so a search reads no record that did not change.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::config::DataDirs;

const SIDECAR_DIR: &str = "session-search";
const INDEX_FILE_STEM: &str = "index";
const SEARCHED_KINDS: [&str; 2] = ["you", "agent"];
const PREVIEW_CHARS: usize = 120;

/// The version of one session record a cached extraction belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct RecordStamp {
    len: u64,
    /// Modification time in nanoseconds since the Unix epoch.
    modified: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct Sidecar {
    stamp: RecordStamp,
    entries: Vec<String>,
}

/// Only the panel log of a stored record; every other field is skipped
/// unparsed.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecordPanelLog {
    #[serde(default)]
    panel_log: Option<PanelLog>,
}

#[derive(Deserialize)]
struct PanelLog {
    #[serde(default)]
    log: Vec<PanelLogEntry>,
}

#[derive(Deserialize)]
struct PanelLogEntry {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    text: Option<serde_json::Value>,
}

/// One session whose conversation matched: its first matching entry as a
/// one-line preview, with `[start, end)` match ranges in UTF-16 code units.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchMatch {
    pub session_id: String,
    pub preview: String,
    pub ranges: Vec<[u32; 2]>,
}

/// `session_search` command output.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchResponse {
    pub query: String,
    pub matches: Vec<SessionSearchMatch>,
}

#[derive(Debug)]
pub struct SessionSearchIndex {
    sessions_dir: PathBuf,
    sidecar_dir: PathBuf,
    memory: Mutex<HashMap<String, (RecordStamp, Arc<Vec<String>>)>>,
}

impl SessionSearchIndex {
    pub fn new(dirs: &DataDirs) -> Self {
        Self {
            sessions_dir: dirs.sessions_dir(),
            sidecar_dir: dirs.app_local_data().join(SIDECAR_DIR),
            memory: Mutex::new(HashMap::new()),
        }
    }

    /// Extract every stored session once and delete the sidecars of deleted
    /// sessions, so the first search reads no record.
    pub fn warm_up(&self) {
        for id in json_stems(&self.sessions_dir) {
            if id != INDEX_FILE_STEM {
                self.entries(&id);
            }
        }
        for id in json_stems(&self.sidecar_dir) {
            if !self.record_path(&id).is_file() {
                self.memory.lock().remove(&id);
                if let Err(error) = fs::remove_file(self.sidecar_path(&id)) {
                    eprintln!("eud-agent: stale session search cache removal failed: {error}");
                }
            }
        }
    }

    /// Literal, case-insensitive search of the given sessions, in their
    /// order: the first matching entry of each matching session.
    pub fn search(&self, query: &str, session_ids: &[String]) -> Vec<SessionSearchMatch> {
        let query = query.trim();
        if query.is_empty() {
            return Vec::new();
        }
        let Ok(pattern) = regex::RegexBuilder::new(&regex::escape(query))
            .case_insensitive(true)
            .build()
        else {
            return Vec::new();
        };
        session_ids
            .iter()
            .filter_map(|id| {
                let entries = self.entries(id)?;
                entries.iter().find_map(|entry| {
                    // One preview line: each line break becomes one space, so
                    // byte offsets stay those of the matched text.
                    let line = entry.replace(['\r', '\n', '\t'], " ");
                    let ranges = pattern
                        .find_iter(&line)
                        .map(|found| (found.start(), found.end()))
                        .collect::<Vec<_>>();
                    if ranges.is_empty() {
                        return None;
                    }
                    let (preview, ranges) =
                        crate::workspace::match_preview(&line, &ranges, PREVIEW_CHARS);
                    Some(SessionSearchMatch {
                        session_id: id.clone(),
                        preview,
                        ranges,
                    })
                })
            })
            .collect()
    }

    /// The searchable entries of one stored session, from memory, then its
    /// sidecar, then the record itself; `None` when it has no record.
    fn entries(&self, id: &str) -> Option<Arc<Vec<String>>> {
        if !is_safe_id(id) {
            return None;
        }
        let Some(stamp) = record_stamp(&self.record_path(id)) else {
            self.memory.lock().remove(id);
            return None;
        };
        if let Some((cached, entries)) = self.memory.lock().get(id) {
            if *cached == stamp {
                return Some(entries.clone());
            }
        }
        let entries = match self.read_sidecar(id) {
            Some(sidecar) if sidecar.stamp == stamp => sidecar.entries,
            _ => {
                let entries = self.extract(id);
                self.write_sidecar(id, stamp, &entries);
                entries
            }
        };
        let entries = Arc::new(entries);
        self.memory
            .lock()
            .insert(id.to_string(), (stamp, entries.clone()));
        Some(entries)
    }

    /// The `you`/`agent` texts of a record. An unreadable or malformed record
    /// has nothing to search; it never fails the search of the others.
    fn extract(&self, id: &str) -> Vec<String> {
        let Ok(bytes) = fs::read(self.record_path(id)) else {
            return Vec::new();
        };
        let Ok(record) = serde_json::from_slice::<RecordPanelLog>(&bytes) else {
            return Vec::new();
        };
        record
            .panel_log
            .map(|panel_log| panel_log.log)
            .unwrap_or_default()
            .into_iter()
            .filter(|entry| {
                entry
                    .kind
                    .as_deref()
                    .is_some_and(|kind| SEARCHED_KINDS.contains(&kind))
            })
            .filter_map(|entry| match entry.text {
                Some(serde_json::Value::String(text)) => Some(text),
                _ => None,
            })
            .collect()
    }

    fn read_sidecar(&self, id: &str) -> Option<Sidecar> {
        serde_json::from_slice(&fs::read(self.sidecar_path(id)).ok()?).ok()
    }

    fn write_sidecar(&self, id: &str, stamp: RecordStamp, entries: &[String]) {
        let sidecar = serde_json::json!({ "stamp": stamp, "entries": entries });
        let written = serde_json::to_vec(&sidecar)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| crate::memory::write_atomic_bytes(&self.sidecar_path(id), &bytes));
        if let Err(error) = written {
            eprintln!("eud-agent: session search cache write failed: {error}");
        }
    }

    fn record_path(&self, id: &str) -> PathBuf {
        self.sessions_dir.join(format!("{id}.json"))
    }

    fn sidecar_path(&self, id: &str) -> PathBuf {
        self.sidecar_dir.join(format!("{id}.json"))
    }
}

/// Session ids are the record file stems the session store writes.
fn is_safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn record_stamp(path: &Path) -> Option<RecordStamp> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    let modified = metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos() as u64;
    Some(RecordStamp {
        len: metadata.len(),
        modified,
    })
}

/// The safe ids of the `{id}.json` files directly in `dir`.
fn json_stems(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let id = name.strip_suffix(".json")?;
            is_safe_id(id).then(|| id.to_string())
        })
        .collect()
}

/// Managed state of the `session_search` command.
#[derive(Clone)]
pub struct SessionSearchManaged {
    index: Arc<SessionSearchIndex>,
    sessions: crate::session::SessionStore,
}

impl SessionSearchManaged {
    pub fn new(index: Arc<SessionSearchIndex>, sessions: crate::session::SessionStore) -> Self {
        Self { index, sessions }
    }
}

/// Search the conversations of this window's project sessions. `sessionIds`
/// narrows the search to the sidebar's rows; ids of other projects' sessions
/// are never searched.
#[tauri::command]
pub async fn session_search(
    window: tauri::WebviewWindow,
    registry: tauri::State<'_, crate::project_registry::ProjectRegistry>,
    managed: tauri::State<'_, SessionSearchManaged>,
    query: String,
    session_ids: Option<Vec<String>>,
) -> Result<SessionSearchResponse, String> {
    let handle = registry.for_window(window.label())?;
    let registry = registry.inner().clone();
    let managed = managed.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let sessions = managed
            .sessions
            .list_kind(crate::session::SessionKind::Eps)
            .map_err(|error| error.to_string())?;
        let owned = crate::engine::sessions_of_project(sessions, &handle, &registry)
            .into_iter()
            .map(|meta| meta.id)
            .collect::<Vec<_>>();
        let ids = match session_ids {
            Some(requested) => requested
                .into_iter()
                .filter(|id| owned.contains(id))
                .collect(),
            None => owned,
        };
        let matches = managed.index.search(&query, &ids);
        Ok(SessionSearchResponse { query, matches })
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn index(tag: &str) -> (PathBuf, DataDirs, SessionSearchIndex) {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!("eud-agent-session-search-{tag}-{nanos}"));
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        fs::create_dir_all(dirs.sessions_dir()).unwrap();
        let index = SessionSearchIndex::new(&dirs);
        (base, dirs, index)
    }

    fn write_record(dirs: &DataDirs, id: &str, log: serde_json::Value) {
        let record = json!({
            "id": id,
            "preview": "x".repeat(4096),
            "panelLog": { "schemaVersion": 2, "logSeq": 1, "log": log },
        });
        fs::write(
            dirs.sessions_dir().join(format!("{id}.json")),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
    }

    fn ids(values: &[&str]) -> Vec<String> {
        values.iter().map(|id| id.to_string()).collect()
    }

    #[test]
    fn searches_only_user_and_agent_entries() {
        let (base, dirs, index) = index("kinds");
        write_record(
            &dirs,
            "s1",
            json!([
                { "id": 1, "kind": "info", "text": "needle info" },
                { "id": 2, "kind": "progress", "text": "needle progress" },
                { "id": 3, "kind": "warn", "text": "needle warn" },
                { "id": 4, "kind": "error", "text": "needle error" },
                { "id": 5, "kind": "ok", "text": "needle ok" },
            ]),
        );
        write_record(
            &dirs,
            "s2",
            json!([
                { "id": 1, "kind": "info", "text": "needle info" },
                { "id": 2, "kind": "you", "text": "first request" },
                { "id": 3, "kind": "agent", "text": "Agent found the NEEDLE" },
                { "id": 4, "kind": "you", "text": "needle again" },
            ]),
        );

        assert_eq!(
            index.search("needle", &ids(&["s1", "s2", "missing"])),
            vec![SessionSearchMatch {
                session_id: "s2".to_string(),
                preview: "Agent found the NEEDLE".to_string(),
                ranges: vec![[16, 22]],
            }]
        );
        assert!(index.search("  ", &ids(&["s2"])).is_empty());
        // Literal, never a pattern.
        assert!(index.search("found.*needle", &ids(&["s2"])).is_empty());
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn a_changed_record_invalidates_the_cached_entries() {
        let (base, dirs, index) = index("stamp");
        write_record(&dirs, "s1", json!([{ "kind": "you", "text": "old words" }]));
        assert_eq!(index.search("old", &ids(&["s1"])).len(), 1);

        write_record(
            &dirs,
            "s1",
            json!([{ "kind": "you", "text": "brand new words, longer than before" }]),
        );
        assert!(index.search("old", &ids(&["s1"])).is_empty());
        assert_eq!(index.search("brand new", &ids(&["s1"])).len(), 1);
        let sidecar: Sidecar =
            serde_json::from_slice(&fs::read(index.sidecar_path("s1")).unwrap()).unwrap();
        assert_eq!(sidecar.entries, vec!["brand new words, longer than before"]);
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn a_fresh_index_reuses_a_sidecar_whose_stamp_matches() {
        let (base, dirs, first) = index("sidecar");
        write_record(
            &dirs,
            "s1",
            json!([{ "kind": "agent", "text": "from the record" }]),
        );
        first.warm_up();
        let path = first.sidecar_path("s1");
        assert!(path.starts_with(dirs.app_local_data()));
        let bytes = fs::read(&path).unwrap();
        assert!(!bytes.starts_with(&[0xef, 0xbb, 0xbf]));

        // Same stamp, different text: only the sidecar can answer this.
        let mut sidecar: Sidecar = serde_json::from_slice(&bytes).unwrap();
        sidecar.entries = vec!["from the sidecar".to_string()];
        fs::write(&path, serde_json::to_vec(&sidecar).unwrap()).unwrap();

        let second = SessionSearchIndex::new(&dirs);
        assert_eq!(second.search("sidecar", &ids(&["s1"])).len(), 1);
        assert!(second.search("record", &ids(&["s1"])).is_empty());
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn previews_count_utf16_units_and_collapse_line_breaks() {
        let (base, dirs, index) = index("preview");
        write_record(
            &dirs,
            "s1",
            json!([{ "kind": "you", "text": "첫 줄\n두 번째 \u{1f600} 검색어 줄" }]),
        );
        assert_eq!(
            index.search("검색어", &ids(&["s1"])),
            vec![SessionSearchMatch {
                session_id: "s1".to_string(),
                preview: "첫 줄 두 번째 \u{1f600} 검색어 줄".to_string(),
                ranges: vec![[12, 15]],
            }]
        );

        let long = format!("{}검색어{}", "가".repeat(200), "나".repeat(200));
        write_record(&dirs, "s2", json!([{ "kind": "agent", "text": long }]));
        let found = index.search("검색어", &ids(&["s2"]));
        let preview = &found[0].preview;
        assert_eq!(preview.chars().count(), PREVIEW_CHARS + 2);
        assert!(preview.starts_with('\u{2026}') && preview.ends_with('\u{2026}'));
        let [start, end] = found[0].ranges[0];
        let utf16 = preview.encode_utf16().collect::<Vec<_>>();
        assert_eq!(
            String::from_utf16(&utf16[start as usize..end as usize]).unwrap(),
            "검색어"
        );
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn warm_up_sweeps_the_sidecars_of_deleted_sessions() {
        let (base, dirs, index) = index("sweep");
        write_record(&dirs, "s1", json!([{ "kind": "you", "text": "kept" }]));
        write_record(&dirs, "s2", json!([{ "kind": "you", "text": "deleted" }]));
        fs::write(dirs.sessions_dir().join("index.json"), b"{}").unwrap();
        index.warm_up();
        assert!(index.sidecar_path("s1").is_file());
        assert!(index.sidecar_path("s2").is_file());
        assert!(!index.sidecar_path("index").exists());

        fs::remove_file(dirs.sessions_dir().join("s2.json")).unwrap();
        index.warm_up();
        assert!(index.sidecar_path("s1").is_file());
        assert!(!index.sidecar_path("s2").exists());
        assert!(index.search("deleted", &ids(&["s2"])).is_empty());
        assert_eq!(index.search("kept", &ids(&["s1"])).len(), 1);
        fs::remove_dir_all(base).ok();
    }
}
