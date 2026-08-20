//! Per-project memory store ported from `server/eud_agent/memory.py`.
//!
//! The Rust v2 store is rooted at `%appdata%\eud-agent\memory\<project>\`; callers pass
//! the memory root (`DataDirs::memory_dir()`) plus the raw project name.

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// The four codex/panel-editable markdown files, in render order.
pub const MEMORY_FILES: [&str; 4] = ["resources", "structure", "conventions", "lessons"];

/// Per-file write cap, in UTF-8 bytes (over-budget writes are rejected).
pub const CONTENT_CAP_BYTES: usize = 8192;

/// Rendered `[project memory]` section cap, in characters.
pub const SECTION_CAP_CHARS: usize = 40000;

/// Suffix appended to the `## structure` heading when the LIST hash drifted.
pub const STALE_SUFFIX: &str = "(may be outdated — project files changed since last memory update)";

/// Rendered body when the store is disabled or unreadable.
pub const NO_MEMORY: &str = "(no project memory)";

/// Marker appended after section-cap truncation.
pub const TRUNCATED_MARKER: &str = "memory section truncated";

const META_FILE: &str = "meta.json";
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);
const INSTRUCTION_BLOCK: &str = concat!(
    "Record only durable, project-specific facts via the memory_write tool: ",
    "resource allocations (switches, death counters, locations, EUD addresses), ",
    "file roles, naming/trigger conventions, and user corrections. Never record ",
    "transient or code-derivable detail. Each file is a full replacement; rewrite ",
    "it faithfully from what you see below."
);

/// Outcome of a [`ProjectMemory::write`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteResult {
    pub ok: bool,
    pub reason: String,
}

impl WriteResult {
    fn ok() -> Self {
        Self {
            ok: true,
            reason: String::new(),
        }
    }

    fn rejected(reason: impl Into<String>) -> Self {
        Self {
            ok: false,
            reason: reason.into(),
        }
    }
}

/// Sanitize a bridge project name into a Windows-safe directory name.
///
/// Characters invalid in Windows file names (`<>:"/\|?*` and control chars) are replaced
/// with `_`, and trailing dots/spaces are stripped. An empty or whitespace-only name, or a
/// name that collapses to empty after stripping, returns `""` and disables the store.
pub fn sanitize_project_name(name: &str) -> String {
    if name.trim().is_empty() {
        return String::new();
    }

    let mut cleaned = String::with_capacity(name.len());
    for ch in name.chars() {
        if is_invalid_windows_filename_char(ch) {
            cleaned.push('_');
        } else {
            cleaned.push(ch);
        }
    }

    cleaned.trim_end_matches(['.', ' ']).to_string()
}

/// Return the sha256 hex digest of a bridge LIST reply.
pub fn list_hash(list_reply: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(list_reply.as_bytes());
    hex_lower(&hasher.finalize())
}

#[derive(Debug, Clone)]
pub struct ProjectMemory {
    memory_root: PathBuf,
    project_name: String,
    sanitized: String,
}

impl ProjectMemory {
    /// Construct from the v2 memory root (`DataDirs::memory_dir()`) and raw project name.
    pub fn new(memory_root: impl Into<PathBuf>, project_name: impl Into<String>) -> Self {
        let project_name = project_name.into();
        let sanitized = sanitize_project_name(&project_name);
        Self {
            memory_root: memory_root.into(),
            project_name,
            sanitized,
        }
    }

    /// True when a non-empty project name yields a usable store.
    pub fn enabled(&self) -> bool {
        !self.sanitized.is_empty()
    }

    /// The store directory, or `None` when the store is disabled.
    pub fn store_dir(&self) -> Option<PathBuf> {
        self.enabled()
            .then(|| self.memory_root.join(&self.sanitized))
    }

    /// Raw project name supplied at construction time.
    pub fn project_name(&self) -> &str {
        &self.project_name
    }

    /// Return a markdown file's content, or `""` when absent/disabled/unreadable.
    ///
    /// A read never creates the store dir and never errors.
    pub fn read(&self, name: &str) -> String {
        let Some(path) = self.file_path(name) else {
            return String::new();
        };
        if !path.is_file() {
            return String::new();
        }
        fs::read_to_string(path).unwrap_or_default()
    }

    /// Atomically write a markdown file (full replacement); return the outcome.
    ///
    /// Rejected writes do not touch disk when disabled, `name` is unknown, or `content`
    /// exceeds the UTF-8 byte cap. Successful writes are UTF-8 bytes without BOM.
    pub fn write(&self, name: &str, content: &str) -> WriteResult {
        let Some(store) = self.store_dir() else {
            return WriteResult::rejected("no project is open; memory is disabled");
        };
        if !MEMORY_FILES.contains(&name) {
            return WriteResult::rejected(format!(
                "unknown memory file '{name}'; expected one of {}",
                MEMORY_FILES.join(", ")
            ));
        }

        let encoded = content.as_bytes();
        if encoded.len() > CONTENT_CAP_BYTES {
            return WriteResult::rejected(format!(
                "content is {} bytes, over the {CONTENT_CAP_BYTES}-byte budget; condense it.",
                encoded.len()
            ));
        }

        match write_atomic_bytes(&store.join(format!("{name}.md")), encoded) {
            Ok(()) => WriteResult::ok(),
            Err(err) => WriteResult::rejected(err.to_string()),
        }
    }

    /// Return `meta.json` as an object, or `{}` when absent/disabled/malformed.
    pub fn read_meta(&self) -> Map<String, Value> {
        let Some(path) = self.meta_path() else {
            return Map::new();
        };
        if !path.is_file() {
            return Map::new();
        }

        let Ok(bytes) = fs::read(path) else {
            return Map::new();
        };
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(Value::Object(meta)) => meta,
            _ => Map::new(),
        }
    }

    /// Atomically write `meta.json` (UTF-8 no BOM); no-op when disabled.
    pub fn write_meta(&self, meta: &Map<String, Value>) -> anyhow::Result<()> {
        let Some(path) = self.meta_path() else {
            return Ok(());
        };
        let bytes = serde_json::to_vec_pretty(meta)?;
        write_atomic_bytes(&path, &bytes)?;
        Ok(())
    }

    /// Record the current LIST reply's hash and an epoch-second timestamp in `meta.json`.
    pub fn update_list_hash(&self, list_reply: &str) -> anyhow::Result<()> {
        let mut meta = self.read_meta();
        meta.insert("version".to_string(), Value::from(1));
        meta.insert("list_hash".to_string(), Value::from(list_hash(list_reply)));
        meta.insert("list_hash_ts".to_string(), Value::from(epoch_seconds()));
        self.write_meta(&meta)
    }

    /// True when the stored `list_hash` differs from the current LIST reply.
    ///
    /// A store with no recorded hash is treated as stale.
    pub fn is_stale(&self, list_reply: &str) -> bool {
        let meta = self.read_meta();
        match meta.get("list_hash").and_then(Value::as_str) {
            Some(stored) => stored != list_hash(list_reply),
            None => true,
        }
    }

    /// Build the `[project memory]` prompt section.
    pub fn render_section(&self, list_reply: Option<&str>) -> String {
        if !self.enabled() {
            return no_memory_section();
        }

        match self.render_enabled(list_reply) {
            Ok(section) => section,
            Err(_) => no_memory_section(),
        }
    }

    fn render_enabled(&self, list_reply: Option<&str>) -> anyhow::Result<String> {
        let mut files = Vec::with_capacity(MEMORY_FILES.len());
        for name in MEMORY_FILES {
            files.push((name, self.read_for_render(name)?));
        }

        let stale = list_reply.is_some_and(|reply| self.is_stale(reply));
        let mut body_parts = vec![INSTRUCTION_BLOCK.to_string()];
        body_parts.extend(render_file_blocks(&files, stale, None));

        let section = section_from_parts(&body_parts);
        if section.chars().count() <= SECTION_CAP_CHARS {
            return Ok(section);
        }

        Ok(render_with_truncated_lessons(&files, stale))
    }

    fn read_for_render(&self, name: &str) -> anyhow::Result<String> {
        let Some(path) = self.file_path(name) else {
            return Ok(String::new());
        };
        if !path.is_file() {
            return Ok(String::new());
        }
        Ok(fs::read_to_string(path)?)
    }

    fn file_path(&self, name: &str) -> Option<PathBuf> {
        self.store_dir()
            .map(|store| store.join(format!("{name}.md")))
    }

    fn meta_path(&self) -> Option<PathBuf> {
        self.store_dir().map(|store| store.join(META_FILE))
    }
}

/// Remove obsolete `episodes.jsonl` files left by versions that stored request history
/// alongside project memory. Best-effort and limited to immediate project directories.
pub(crate) fn cleanup_legacy_episode_files(memory_root: &Path) -> usize {
    let Ok(projects) = fs::read_dir(memory_root) else {
        return 0;
    };
    projects
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter(|entry| fs::remove_file(entry.path().join("episodes.jsonl")).is_ok())
        .count()
}

fn is_invalid_windows_filename_char(ch: char) -> bool {
    matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || (ch as u32) <= 0x1f
}

/// Atomically write `bytes` to `path` (temp + rename) as raw bytes (no BOM).
///
/// Shared with [`crate::wiki`] so the dat-edit ledger lands beside the project's
/// memory dir with the SAME write semantics (UTF-8 without BOM, crash-safe rename).
pub(crate) fn write_atomic_bytes(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let tmp = tmp_path(path);
    if let Err(err) = fs::write(&tmp, bytes) {
        let _ = fs::remove_file(&tmp);
        return Err(err.into());
    }
    if let Err(err) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(err.into());
    }
    Ok(())
}

fn tmp_path(path: &Path) -> PathBuf {
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let pid = std::process::id();
    let file_name = path.file_name().unwrap_or_default().to_string_lossy();
    path.with_file_name(format!("{file_name}.{pid}.{nanos}.{seq}.tmp"))
}

fn render_file_blocks(
    files: &[(&str, String)],
    stale: bool,
    lessons_override: Option<&str>,
) -> Vec<String> {
    let mut blocks = Vec::new();
    for (name, body) in files {
        let body = if *name == "lessons" {
            lessons_override.unwrap_or(body.trim())
        } else {
            body.trim()
        };
        if body.is_empty() {
            continue;
        }

        let mut heading = format!("## {name}");
        if *name == "structure" && stale {
            heading = format!("{heading} {STALE_SUFFIX}");
        }
        blocks.push(format!("{heading}\n{body}"));
    }
    blocks
}

fn render_with_truncated_lessons(files: &[(&str, String)], stale: bool) -> String {
    let mut fixed_parts = vec![INSTRUCTION_BLOCK.to_string()];
    for (name, body) in files {
        if *name == "lessons" {
            continue;
        }
        let body = body.trim();
        if body.is_empty() {
            continue;
        }

        let mut heading = format!("## {name}");
        if *name == "structure" && stale {
            heading = format!("{heading} {STALE_SUFFIX}");
        }
        fixed_parts.push(format!("{heading}\n{body}"));
    }

    let lessons_body = files
        .iter()
        .find_map(|(name, body)| {
            if *name == "lessons" {
                Some(body.trim())
            } else {
                None
            }
        })
        .unwrap_or("");

    let frame_parts = [
        fixed_parts.clone(),
        vec!["## lessons\n".to_string(), TRUNCATED_MARKER.to_string()],
    ]
    .concat();
    let frame_len = section_from_parts(&frame_parts).chars().count();
    let budget = SECTION_CAP_CHARS.saturating_sub(frame_len);
    let head = take_chars(lessons_body, budget);

    let mut parts = fixed_parts;
    if !head.is_empty() {
        parts.push(format!("## lessons\n{head}"));
    }
    parts.push(TRUNCATED_MARKER.to_string());

    clamp_chars(&section_from_parts(&parts), SECTION_CAP_CHARS)
}

fn section_from_parts(parts: &[String]) -> String {
    format!("[project memory]\n{}", parts.join("\n\n"))
}

fn no_memory_section() -> String {
    format!("[project memory]\n{NO_MEMORY}")
}

fn take_chars(s: &str, limit: usize) -> String {
    s.chars().take(limit).collect()
}

fn clamp_chars(s: &str, limit: usize) -> String {
    s.chars().take(limit).collect()
}

fn epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DataDirs;
    use serde_json::json;

    fn unique_temp_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("eud-agent-memory-test-{tag}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn memory_root(tag: &str) -> (PathBuf, PathBuf) {
        let base = unique_temp_dir(tag);
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        (base, dirs.memory_dir())
    }

    #[test]
    fn write_then_read_round_trips_under_memory_root() {
        let (base, root) = memory_root("round-trip");
        let memory = ProjectMemory::new(root.clone(), "My<Project>");

        let result = memory.write("resources", "Switch 12 = boss phase");

        assert!(result.ok, "{result:?}");
        assert_eq!(memory.read("resources"), "Switch 12 = boss phase");
        assert_eq!(
            fs::read_to_string(root.join("My_Project_").join("resources.md")).unwrap(),
            "Switch 12 = boss phase"
        );
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn atomic_write_uses_unique_temp_paths_and_leaves_no_tmp_files() {
        let base = unique_temp_dir("atomic-temp");
        let target = base.join("resources.md");

        let first_tmp = tmp_path(&target);
        let second_tmp = tmp_path(&target);
        assert_ne!(first_tmp, second_tmp);
        assert_eq!(first_tmp.parent(), target.parent());
        assert_eq!(second_tmp.parent(), target.parent());

        let first_name = first_tmp.file_name().unwrap().to_string_lossy();
        let second_name = second_tmp.file_name().unwrap().to_string_lossy();
        assert!(first_name.starts_with("resources.md."));
        assert!(second_name.starts_with("resources.md."));
        assert!(first_name.ends_with(".tmp"));
        assert!(second_name.ends_with(".tmp"));

        write_atomic_bytes(&target, b"first").unwrap();
        write_atomic_bytes(&target, b"second").unwrap();

        assert_eq!(fs::read_to_string(&target).unwrap(), "second");
        let tmp_files: Vec<PathBuf> = fs::read_dir(&base)
            .unwrap()
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(".tmp"))
            })
            .collect();
        assert!(tmp_files.is_empty(), "leftover temp files: {tmp_files:?}");

        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn write_rejects_over_cap_and_unknown_name_without_touching_prior_content() {
        let (base, root) = memory_root("rejects");
        let memory = ProjectMemory::new(root, "Project");
        assert!(memory.write("lessons", "prior").ok);

        let over = "x".repeat(CONTENT_CAP_BYTES + 1);
        let result = memory.write("lessons", &over);
        assert!(!result.ok);
        assert_eq!(
            result.reason,
            format!(
                "content is {} bytes, over the {CONTENT_CAP_BYTES}-byte budget; condense it.",
                CONTENT_CAP_BYTES + 1
            )
        );
        assert_eq!(memory.read("lessons"), "prior");

        let result = memory.write("unknown", "new");
        assert!(!result.ok);
        assert_eq!(
            result.reason,
            "unknown memory file 'unknown'; expected one of resources, structure, conventions, lessons"
        );
        assert_eq!(memory.read("lessons"), "prior");
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn sanitize_invalid_chars_trailing_dots_and_disabled_empty_names() {
        assert_eq!(sanitize_project_name(r#"a<>:"/\|?*b"#), "a_________b");
        assert_eq!(sanitize_project_name("inner. dot .  "), "inner. dot");
        assert_eq!(sanitize_project_name("   "), "");
        assert_eq!(sanitize_project_name("..."), "");

        let (base, root) = memory_root("disabled");
        let memory = ProjectMemory::new(root.clone(), "   ");
        assert!(!memory.enabled());
        assert_eq!(memory.store_dir(), None);
        assert_eq!(memory.read("resources"), "");

        let result = memory.write("resources", "content");
        assert!(!result.ok);
        assert_eq!(result.reason, "no project is open; memory is disabled");
        assert_eq!(
            memory.render_section(None),
            "[project memory]\n(no project memory)"
        );
        assert!(
            !root.exists(),
            "disabled store must not create the memory root"
        );
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn cleanup_legacy_episode_files_removes_only_episode_logs() {
        let (base, root) = memory_root("legacy-episodes");
        let project = root.join("Project");
        fs::create_dir_all(&project).unwrap();
        fs::write(
            project.join("episodes.jsonl"),
            "{\"decision\":\"answer\"}\n",
        )
        .unwrap();
        fs::write(project.join("resources.md"), "Switch 1 = reserved").unwrap();

        assert_eq!(cleanup_legacy_episode_files(&root), 1);
        assert!(!project.join("episodes.jsonl").exists());
        assert_eq!(
            fs::read_to_string(project.join("resources.md")).unwrap(),
            "Switch 1 = reserved"
        );
        assert_eq!(cleanup_legacy_episode_files(&root), 0);
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn meta_write_read_and_staleness() {
        let (base, root) = memory_root("meta");
        let memory = ProjectMemory::new(root, "Project");

        assert!(memory.is_stale("LIST a"));

        let mut meta = Map::new();
        meta.insert("custom".to_string(), json!("kept"));
        memory.write_meta(&meta).unwrap();
        assert_eq!(memory.read_meta().get("custom"), Some(&json!("kept")));

        memory.update_list_hash("LIST a").unwrap();
        assert!(!memory.is_stale("LIST a"));
        assert!(memory.is_stale("LIST b"));
        assert_eq!(memory.read_meta().get("version"), Some(&json!(1)));
        assert_eq!(
            memory.read_meta().get("list_hash"),
            Some(&json!(list_hash("LIST a")))
        );
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn render_section_disabled_and_enabled_order_staleness() {
        let (base, root) = memory_root("render");
        let disabled = ProjectMemory::new(root.clone(), "");
        assert_eq!(
            disabled.render_section(None),
            "[project memory]\n(no project memory)"
        );

        let memory = ProjectMemory::new(root, "Project");
        assert!(memory.write("resources", "res").ok);
        assert!(memory.write("structure", "struct").ok);
        assert!(memory.write("conventions", "conv").ok);
        assert!(memory.write("lessons", "").ok);
        memory.update_list_hash("LIST old").unwrap();

        let section = memory.render_section(Some("LIST new"));
        let resources = section.find("## resources\nres").unwrap();
        let structure = section
            .find(&format!("## structure {STALE_SUFFIX}\nstruct"))
            .unwrap();
        let conventions = section.find("## conventions\nconv").unwrap();

        assert!(resources < structure);
        assert!(structure < conventions);
        assert!(!section.contains("## lessons"));
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn render_truncation_tail_truncates_lessons() {
        let (base, root) = memory_root("truncate");
        let memory = ProjectMemory::new(root, "Project");
        assert!(memory.write("resources", "res").ok);

        let store = memory.store_dir().unwrap();
        fs::create_dir_all(&store).unwrap();
        fs::write(
            store.join("lessons.md"),
            "L".repeat(SECTION_CAP_CHARS + 1000),
        )
        .unwrap();

        let section = memory.render_section(None);
        assert!(section.chars().count() <= SECTION_CAP_CHARS);
        assert!(section.contains(TRUNCATED_MARKER));
        assert!(section.contains("## lessons\n"));
        assert!(section.contains(&"L".repeat(100)));
        assert!(!section.contains(&"L".repeat(SECTION_CAP_CHARS)));
        fs::remove_dir_all(base).ok();
    }
}
