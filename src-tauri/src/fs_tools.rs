//! `fs_*` — the project root as a plain filesystem.
//!
//! Codex and Claude Code reach the root through their own CLI file tools
//! (Phase 3). OpenCode Go, Ollama and Antigravity have no native tools at all,
//! so without these they would still be authoring through `file_write` and
//! `source_search`, which see only the manifest's editable sources. These
//! close that gap: the same root the other two providers already have, under
//! the same §N2 write boundary, and nothing else. No journal, no evidence, no
//! write registration — reading and writing a file is not a semantic mutation
//! the app has an opinion about. What makes it reversible is the turn commit.

use std::fs;
use std::path::{Path, PathBuf};

use regex::RegexBuilder;
use serde_json::{json, Value};

use crate::workspace::{apply_exact_text_edits, ExactTextEdit};

/// One `fs_read` without an explicit range returns at most this many lines.
const READ_DEFAULT_LINES: usize = 400;
/// A file larger than this is never returned whole; the caller pages it.
const READ_MAX_BYTES: u64 = 4 * 1024 * 1024;
/// What one `fs_write` may put on disk.
const WRITE_MAX_BYTES: usize = 8 * 1024 * 1024;
/// How far the walker descends before it reports the tree as truncated.
const WALK_MAX_ENTRIES: usize = 50_000;
const GLOB_DEFAULT_LIMIT: usize = 200;
const GLOB_MAX_LIMIT: usize = 1_000;
const GLOB_MAX_PATTERN_CHARS: usize = 512;
/// `**` is the expensive segment; a handful is a path, a dozen is a fuzzer.
const GLOB_MAX_WILDCARD_SEGMENTS: usize = 4;
const GREP_DEFAULT_LIMIT: usize = 20;
const GREP_MAX_LIMIT: usize = 100;
const GREP_MAX_CONTEXT_LINES: usize = 20;
const GREP_MAX_PATTERN_CHARS: usize = 512;
/// A file bigger than this is a build artifact or a map, not source.
const GREP_MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// One returned region. A minified line is not worth a whole observation.
const GREP_MAX_REGION_CHARS: usize = 4_000;
/// How much of a file is sampled to decide it is binary.
const BINARY_PROBE_BYTES: usize = 8_192;

/// §N2: the agent owns the project root, with exceptions it may read but never
/// write. Returns the refusal to show the model, which names the reason — a
/// model that is only told "no" tries the same thing a different way.
fn write_refusal(relative: &str) -> Option<String> {
    let first = relative.split('/').next().unwrap_or(relative);
    let reason = if first.eq_ignore_ascii_case("maps") || first.eq_ignore_ascii_case("references") {
        "맵 파일은 MapSafe의 백업·CHK 재추출 검증·공유락을 거쳐야 합니다. 맵 변경은 Map 세션이나 map_* 도구로 하세요."
    } else if first.eq_ignore_ascii_case(".git") {
        "git 히스토리는 이 프로젝트의 되돌리기 수단 자체입니다."
    } else if first.eq_ignore_ascii_case(".claude")
        || relative.eq_ignore_ascii_case(".mcp.json")
        || relative.eq_ignore_ascii_case(".codex")
    {
        "에이전트 권한 설정은 세션이 스스로 바꾸지 않습니다."
    } else {
        return None;
    };
    Some(format!("'{relative}'에는 쓸 수 없습니다: {reason}"))
}

/// Resolve a project-relative path under `root`. Everything `rules.md` rejects
/// is rejected before any I/O, and the result is confirmed to still be inside
/// the root so a symlink cannot walk out of it.
fn resolve(root: &Path, requested: &str) -> Result<(String, PathBuf), String> {
    let requested = requested.trim().replace('\\', "/");
    let relative = crate::native_project::normalize_relative_path(&requested)?;
    let path = root.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
    match fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || crate::memory::is_reparse_point(&metadata) {
                return Err(format!(
                    "'{relative}'은(는) 심볼릭 링크 또는 재분석 지점입니다."
                ));
            }
            let canonical = fs::canonicalize(&path).map_err(|error| error.to_string())?;
            if !canonical.starts_with(root) {
                return Err(format!("'{relative}'이(가) 프로젝트 루트를 벗어납니다."));
            }
            Ok((relative, canonical))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut existing = path.parent();
            while let Some(candidate) = existing {
                if candidate.exists() {
                    break;
                }
                existing = candidate.parent();
            }
            let existing =
                existing.ok_or_else(|| format!("'{relative}'에 존재하는 상위 폴더가 없습니다."))?;
            let canonical = fs::canonicalize(existing).map_err(|error| error.to_string())?;
            if !canonical.starts_with(root) {
                return Err(format!("'{relative}'이(가) 프로젝트 루트를 벗어납니다."));
            }
            Ok((relative, path))
        }
        Err(error) => Err(error.to_string()),
    }
}

/// Read any file in the project root, optionally by inclusive 1-based line range.
pub(crate) fn read(root: &Path, args: &Value) -> Result<Value, String> {
    let (relative, path) = resolve(root, str_arg(args, "path")?)?;
    let metadata = fs::metadata(&path)
        .map_err(|error| format!("'{relative}'을(를) 읽지 못했습니다: {error}"))?;
    if metadata.is_dir() {
        return Err(format!(
            "'{relative}'은(는) 폴더입니다. 목록은 fs_glob을 쓰세요."
        ));
    }
    if metadata.len() > READ_MAX_BYTES {
        return Err(format!(
            "'{relative}'은(는) {}바이트로 fs_read의 한도({READ_MAX_BYTES})를 넘습니다.",
            metadata.len()
        ));
    }
    let bytes =
        fs::read(&path).map_err(|error| format!("'{relative}'을(를) 읽지 못했습니다: {error}"))?;
    if is_binary(&bytes) {
        return Err(format!(
            "'{relative}'은(는) 텍스트 파일이 아닙니다. 맵은 map_info로 읽으세요."
        ));
    }
    let content = String::from_utf8(bytes)
        .map_err(|_| format!("'{relative}'은(는) UTF-8 텍스트가 아닙니다."))?;
    ranged_result(&relative, &content, args)
}

/// Create or overwrite a file in the project root.
pub(crate) fn write(root: &Path, args: &Value) -> Result<Value, String> {
    let (relative, path) = resolve(root, str_arg(args, "path")?)?;
    if let Some(refusal) = write_refusal(&relative) {
        return Err(refusal);
    }
    let content = str_arg(args, "content")?;
    if content.len() > WRITE_MAX_BYTES {
        return Err(format!(
            "fs_write content는 {WRITE_MAX_BYTES}바이트를 넘을 수 없습니다."
        ));
    }
    if path.is_dir() {
        return Err(format!("'{relative}'은(는) 폴더입니다."));
    }
    let created = !path.exists();
    crate::memory::write_atomic_bytes(&path, content.as_bytes())
        .map_err(|error| format!("'{relative}'을(를) 쓰지 못했습니다: {error}"))?;
    Ok(json!({
        "ok": true,
        "path": relative,
        "created": created,
        "bytes": content.len(),
    }))
}

/// Apply ordered exact-text edits to an existing file in the project root.
pub(crate) fn edit(root: &Path, args: &Value) -> Result<Value, String> {
    let (relative, path) = resolve(root, str_arg(args, "path")?)?;
    if let Some(refusal) = write_refusal(&relative) {
        return Err(refusal);
    }
    let edits: Vec<ExactTextEdit> = serde_json::from_value(
        args.get("edits")
            .cloned()
            .ok_or_else(|| "missing argument 'edits'".to_string())?,
    )
    .map_err(|error| format!("invalid fs_edit edits: {error}"))?;
    let bytes =
        fs::read(&path).map_err(|error| format!("'{relative}'을(를) 읽지 못했습니다: {error}"))?;
    if is_binary(&bytes) {
        return Err(format!("'{relative}'은(는) 텍스트 파일이 아닙니다."));
    }
    let content = String::from_utf8(bytes)
        .map_err(|_| format!("'{relative}'은(는) UTF-8 텍스트가 아닙니다."))?;
    let edited = apply_exact_text_edits("fs_edit", &relative, &content, &edits)
        .map_err(|error| error.to_string())?;
    crate::memory::write_atomic_bytes(&path, edited.as_bytes())
        .map_err(|error| format!("'{relative}'을(를) 쓰지 못했습니다: {error}"))?;
    Ok(json!({
        "ok": true,
        "path": relative,
        "editsApplied": edits.len(),
        "bytes": edited.len(),
    }))
}

/// List project files matching a glob.
pub(crate) fn glob(root: &Path, args: &Value) -> Result<Value, String> {
    let pattern = str_arg(args, "pattern")?.trim().replace('\\', "/");
    let pattern = validate_pattern(&pattern, GLOB_MAX_PATTERN_CHARS)?;
    let limit = usize_arg_default(args, "limit", GLOB_DEFAULT_LIMIT)?.clamp(1, GLOB_MAX_LIMIT);
    let offset = usize_arg_default(args, "offset", 0)?;
    let include_generated = bool_arg_default(args, "includeGenerated", false)?;

    let walk = walk(root, include_generated);
    let mut files = Vec::new();
    let mut total = 0usize;
    for file in &walk.files {
        if !glob_matches(&pattern, &file.path) {
            continue;
        }
        if total >= offset && files.len() < limit {
            files.push(json!({ "path": file.path, "bytes": file.bytes }));
        }
        total += 1;
    }

    let next_offset = offset.saturating_add(files.len());
    let has_more = next_offset < total;
    Ok(json!({
        "pattern": pattern,
        "offset": offset,
        "limit": limit,
        "total": total,
        "count": files.len(),
        "hasMore": has_more,
        "nextOffset": has_more.then_some(next_offset),
        "treeTruncated": walk.truncated,
        "files": files,
    }))
}

/// Search project file contents with a regular expression.
pub(crate) fn grep(root: &Path, args: &Value) -> Result<Value, String> {
    let pattern = str_arg(args, "pattern")?;
    if pattern.trim().is_empty() {
        return Err("fs_grep pattern must not be empty".to_string());
    }
    if pattern.chars().count() > GREP_MAX_PATTERN_CHARS {
        return Err(format!(
            "fs_grep pattern exceeds {GREP_MAX_PATTERN_CHARS} characters"
        ));
    }
    let case_sensitive = bool_arg_default(args, "caseSensitive", false)?;
    let regex = RegexBuilder::new(pattern)
        .case_insensitive(!case_sensitive)
        .size_limit(1 << 20)
        .build()
        .map_err(|error| format!("fs_grep pattern is not a valid regular expression: {error}"))?;

    let file_glob = match args.get("glob").and_then(Value::as_str) {
        Some(raw) if !raw.trim().is_empty() => Some(validate_pattern(
            &raw.trim().replace('\\', "/"),
            GLOB_MAX_PATTERN_CHARS,
        )?),
        _ => None,
    };
    let context_lines = usize_arg_default(args, "contextLines", 2)?.min(GREP_MAX_CONTEXT_LINES);
    let limit = usize_arg_default(args, "limit", GREP_DEFAULT_LIMIT)?.clamp(1, GREP_MAX_LIMIT);
    let offset = usize_arg_default(args, "offset", 0)?;
    let include_generated = bool_arg_default(args, "includeGenerated", false)?;

    let walk = walk(root, include_generated);
    let mut matches = Vec::new();
    let mut total = 0usize;
    let mut skipped = 0usize;
    for file in &walk.files {
        if let Some(glob) = file_glob.as_deref() {
            if !glob_matches(glob, &file.path) {
                continue;
            }
        }
        if file.bytes > GREP_MAX_FILE_BYTES {
            skipped += 1;
            continue;
        }
        let bytes = match fs::read(root.join(file.path.replace('/', std::path::MAIN_SEPARATOR_STR)))
        {
            Ok(bytes) => bytes,
            // A file that vanished or is locked mid-walk is not this search's
            // problem; report it as skipped rather than failing the call.
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        if is_binary(&bytes) {
            skipped += 1;
            continue;
        }
        let Ok(content) = String::from_utf8(bytes) else {
            skipped += 1;
            continue;
        };
        let lines: Vec<&str> = content.lines().collect();
        for (start, end) in match_regions(&lines, &regex, context_lines) {
            if total >= offset && matches.len() < limit {
                matches.push(json!({
                    "path": file.path,
                    "startLine": start + 1,
                    "endLine": end,
                    "text": clamp_chars(&lines[start..end].join("\n"), GREP_MAX_REGION_CHARS),
                }));
            }
            total += 1;
        }
    }

    let next_offset = offset.saturating_add(matches.len());
    let has_more = next_offset < total;
    Ok(json!({
        "pattern": pattern,
        "glob": file_glob,
        "caseSensitive": case_sensitive,
        "offset": offset,
        "limit": limit,
        "total": total,
        "count": matches.len(),
        "hasMore": has_more,
        "nextOffset": has_more.then_some(next_offset),
        "skippedFiles": skipped,
        "treeTruncated": walk.truncated,
        "matches": matches,
    }))
}

fn ranged_result(path: &str, content: &str, args: &Value) -> Result<Value, String> {
    let total_lines = content.lines().count();
    if total_lines == 0 {
        return Ok(json!({
            "path": path,
            "content": "",
            "startLine": Value::Null,
            "endLine": 0,
            "totalLines": 0,
            "hasMore": false,
        }));
    }
    let start = usize_arg_default(args, "startLine", 1)?;
    if start == 0 {
        return Err("fs_read startLine is 1-based and must be at least 1".to_string());
    }
    if start > total_lines {
        return Err(format!(
            "fs_read startLine {start} exceeds {total_lines} total lines"
        ));
    }
    let default_end = start
        .saturating_add(READ_DEFAULT_LINES - 1)
        .min(total_lines);
    let end = usize_arg_default(args, "endLine", default_end)?.min(total_lines);
    if end < start {
        return Err(format!("fs_read endLine {end} precedes startLine {start}"));
    }
    let selected = content
        .lines()
        .skip(start - 1)
        .take(end - start + 1)
        .collect::<Vec<_>>()
        .join("\n");
    Ok(json!({
        "path": path,
        "content": selected,
        "startLine": start,
        "endLine": end,
        "totalLines": total_lines,
        "hasMore": end < total_lines,
    }))
}

struct WalkedFile {
    path: String,
    bytes: u64,
}

struct Walked {
    files: Vec<WalkedFile>,
    truncated: bool,
}

/// Collect every regular file under the root as a `/`-separated relative path,
/// sorted, skipping what the app's own `.gitignore` excludes. Symlinks and
/// reparse points are never followed: `rules.md` forbids a path that escapes
/// the root, and the cheapest way to guarantee that is to never leave it.
fn walk(root: &Path, include_generated: bool) -> Walked {
    let mut files = Vec::new();
    let mut truncated = false;
    let mut stack = vec![String::new()];
    while let Some(relative) = stack.pop() {
        let directory = if relative.is_empty() {
            root.to_path_buf()
        } else {
            root.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR))
        };
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.file_type().is_symlink() || crate::memory::is_reparse_point(&metadata) {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let child = if relative.is_empty() {
                name.clone()
            } else {
                format!("{relative}/{name}")
            };
            if metadata.is_dir() {
                if skipped_dir(&child, &name, include_generated) {
                    continue;
                }
                stack.push(child);
            } else if metadata.is_file() {
                if files.len() >= WALK_MAX_ENTRIES {
                    truncated = true;
                    continue;
                }
                files.push(WalkedFile {
                    path: child,
                    bytes: metadata.len(),
                });
            }
        }
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Walked { files, truncated }
}

/// Outputs and history, never authoring state. A grep that returns the
/// generated shadow of a source file sends the model to edit the wrong one,
/// and `.git` is both enormous and never the answer.
fn skipped_dir(relative: &str, name: &str, include_generated: bool) -> bool {
    if name.eq_ignore_ascii_case(".git") {
        return true;
    }
    if include_generated {
        return false;
    }
    name.eq_ignore_ascii_case("build")
        || name.eq_ignore_ascii_case("compat")
        || crate::native_project::is_generated_artifact_dir(name)
        || relative.eq_ignore_ascii_case(".eud-agent/state")
}

fn validate_pattern(pattern: &str, max_chars: usize) -> Result<String, String> {
    if pattern.is_empty() {
        return Err("glob pattern must not be empty".to_string());
    }
    if pattern.chars().count() > max_chars {
        return Err(format!("glob pattern exceeds {max_chars} characters"));
    }
    if pattern.contains('\0') || pattern.starts_with('/') {
        return Err("glob pattern must be a project-relative '/'-separated pattern".to_string());
    }
    if pattern.split('/').filter(|part| *part == "**").count() > GLOB_MAX_WILDCARD_SEGMENTS {
        return Err(format!(
            "glob pattern uses more than {GLOB_MAX_WILDCARD_SEGMENTS} '**' segments"
        ));
    }
    Ok(pattern.to_string())
}

/// Match a `/`-separated relative path against a glob. `*` and `?` stay inside
/// one path component, `**` spans components, and a pattern with no `/` at all
/// matches the file name at any depth — `fs_glob("*.eps")` means what a model
/// means by it. Matching is case-insensitive, like the filesystem underneath.
fn glob_matches(pattern: &str, path: &str) -> bool {
    if !pattern.contains('/') {
        let name = path.rsplit('/').next().unwrap_or(path);
        return segment_matches(pattern, name);
    }
    let pattern: Vec<&str> = pattern.split('/').collect();
    let path: Vec<&str> = path.split('/').collect();
    match_segments(&pattern, &path)
}

fn match_segments(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => {
            if rest.is_empty() {
                return true;
            }
            (0..=path.len()).any(|skip| match_segments(rest, &path[skip..]))
        }
        Some((head, rest)) => match path.split_first() {
            Some((first, tail)) if segment_matches(head, first) => match_segments(rest, tail),
            _ => false,
        },
    }
}

/// `*` and `?` against one path component, case-insensitively. Linear with one
/// backtrack point, which is all a single `*` per position needs.
fn segment_matches(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().flat_map(char::to_lowercase).collect();
    let text: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    let (mut p, mut t) = (0usize, 0usize);
    let (mut star, mut resume) = (None, 0usize);
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            resume = t;
            p += 1;
        } else if let Some(index) = star {
            p = index + 1;
            resume += 1;
            t = resume;
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|glyph| *glyph == '*')
}

/// Matching lines expanded by `context_lines` on each side, with overlapping
/// windows merged, so one dense region is one excerpt instead of many.
fn match_regions(
    lines: &[&str],
    regex: &regex::Regex,
    context_lines: usize,
) -> Vec<(usize, usize)> {
    let mut regions: Vec<(usize, usize)> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if !regex.is_match(line) {
            continue;
        }
        let start = index.saturating_sub(context_lines);
        let end = index.saturating_add(context_lines + 1).min(lines.len());
        if let Some(last) = regions.last_mut() {
            if start <= last.1 {
                last.1 = last.1.max(end);
                continue;
            }
        }
        regions.push((start, end));
    }
    regions
}

fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(BINARY_PROBE_BYTES).any(|byte| *byte == 0)
}

fn clamp_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    text.chars().take(max_chars).collect::<String>() + "\n…"
}

fn str_arg<'a>(args: &'a Value, name: &str) -> Result<&'a str, String> {
    args.get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing or non-string argument '{name}'"))
}

fn usize_arg_default(args: &Value, name: &str, default: usize) -> Result<usize, String> {
    let Some(value) = args.get(name) else {
        return Ok(default);
    };
    let value = value
        .as_u64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        .ok_or_else(|| format!("argument '{name}' must be a non-negative integer"))?;
    usize::try_from(value).map_err(|_| format!("argument '{name}' is too large"))
}

fn bool_arg_default(args: &Value, name: &str, default: bool) -> Result<bool, String> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(format!("argument '{name}' must be a boolean")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_root(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("eud-fs-tools-{name}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("src")).expect("temp project root");
        fs::canonicalize(&root).expect("canonical temp root")
    }

    fn write_file(root: &Path, relative: &str, content: &str) {
        let path = root.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
        fs::create_dir_all(path.parent().expect("parent")).expect("parent dirs");
        fs::write(path, content).expect("fixture file");
    }

    #[test]
    fn glob_star_stays_inside_one_component() {
        assert!(glob_matches("src/*.eps", "src/main.eps"));
        assert!(!glob_matches("src/*.eps", "src/unit/main.eps"));
        assert!(glob_matches("src/**/*.eps", "src/unit/main.eps"));
        assert!(glob_matches("src/**/*.eps", "src/main.eps"));
        assert!(glob_matches("**/*.py", "a/b/c/d.py"));
    }

    #[test]
    fn a_bare_glob_pattern_matches_the_file_name_at_any_depth() {
        assert!(glob_matches("*.eps", "src/unit/main.eps"));
        assert!(glob_matches("main.eps", "src/main.eps"));
        assert!(!glob_matches("*.eps", "src/main.py"));
    }

    #[test]
    fn glob_matching_ignores_case_like_the_filesystem() {
        assert!(glob_matches("SRC/*.EPS", "src/Main.eps"));
        assert!(segment_matches("ma?n*", "MAIN_unit.eps"));
    }

    #[test]
    fn a_pattern_with_too_many_wildcard_segments_is_refused() {
        let pattern = "**/**/**/**/**/x";
        assert!(validate_pattern(pattern, GLOB_MAX_PATTERN_CHARS).is_err());
        assert!(validate_pattern("src/**/*.eps", GLOB_MAX_PATTERN_CHARS).is_ok());
    }

    #[test]
    fn the_write_boundary_names_why_it_refused() {
        for path in ["maps/base.scx", "MAPS/base.scx", "references/other.scx"] {
            let refusal = write_refusal(path).expect("map paths are write-forbidden");
            assert!(refusal.contains("MapSafe"), "{refusal}");
        }
        assert!(write_refusal(".git/config")
            .expect("git is write-forbidden")
            .contains("되돌리기"));
        assert!(write_refusal(".claude/settings.json").is_some());
        assert!(write_refusal(".mcp.json").is_some());
        assert!(write_refusal("src/main.eps").is_none());
        assert!(write_refusal("dat/standard.json").is_none());
        assert!(write_refusal("project.eap").is_none());
        // `maps` is a prefix of `mapsize.eps`; only the folder is forbidden.
        assert!(write_refusal("mapsize.eps").is_none());
    }

    #[test]
    fn a_path_that_leaves_the_root_is_refused_before_any_io() {
        let root = temp_root("escape");
        for path in ["../outside.txt", "/etc/passwd", "C:/Windows/system.ini", ""] {
            assert!(resolve(&root, path).is_err(), "{path} must be refused");
        }
        let (relative, _) = resolve(&root, "src\\main.eps").expect("backslashes normalize");
        assert_eq!(relative, "src/main.eps");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn write_then_read_round_trips_and_refuses_the_map_folder() {
        let root = temp_root("write");
        let written = write(
            &root,
            &json!({"path": "src/unit/new.eps", "content": "const a = 1;\n"}),
        )
        .expect("write inside src");
        assert_eq!(written["created"], json!(true));

        let read_back = read(&root, &json!({"path": "src/unit/new.eps"})).expect("read back");
        assert_eq!(read_back["content"], json!("const a = 1;"));
        assert_eq!(read_back["totalLines"], json!(1));

        let refused = write(&root, &json!({"path": "maps/base.scx", "content": "x"}))
            .expect_err("maps are write-forbidden");
        assert!(refused.contains("MapSafe"), "{refused}");
        assert!(
            !root.join("maps").exists(),
            "a refusal must not create the folder"
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn edit_applies_exact_text_and_reports_the_tool_that_failed() {
        let root = temp_root("edit");
        write_file(&root, "src/main.eps", "const a = 1;\nconst b = 2;\n");
        edit(
            &root,
            &json!({
                "path": "src/main.eps",
                "edits": [{"old_text": "const b = 2;", "new_text": "const b = 3;"}],
            }),
        )
        .expect("edit applies");
        let content = fs::read_to_string(root.join("src").join("main.eps")).expect("edited file");
        assert!(content.contains("const b = 3;"));

        let error = edit(
            &root,
            &json!({
                "path": "src/main.eps",
                "edits": [{"old_text": "missing", "new_text": "x"}],
            }),
        )
        .expect_err("a missing old_text fails");
        assert!(error.contains("fs_edit"), "{error}");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn read_refuses_a_binary_file_instead_of_returning_mojibake() {
        let root = temp_root("binary");
        fs::create_dir_all(root.join("maps")).expect("maps dir");
        fs::write(
            root.join("maps").join("base.scx"),
            [0x4du8, 0x00, 0x1a, 0xff],
        )
        .expect("binary fixture");
        let error = read(&root, &json!({"path": "maps/base.scx"})).expect_err("binary is refused");
        assert!(error.contains("map_info"), "{error}");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn glob_skips_generated_trees_until_asked_for_them() {
        let root = temp_root("generated");
        write_file(&root, "src/main.eps", "x\n");
        write_file(&root, "src/__epspy__/main.py", "x\n");
        write_file(&root, "build/out.txt", "x\n");

        let listed = glob(&root, &json!({"pattern": "**/*"})).expect("glob");
        let paths: Vec<String> = listed["files"]
            .as_array()
            .expect("files")
            .iter()
            .map(|file| file["path"].as_str().expect("path").to_string())
            .collect();
        assert_eq!(paths, vec!["src/main.eps".to_string()]);

        let all = glob(&root, &json!({"pattern": "**/*", "includeGenerated": true}))
            .expect("glob with generated");
        assert_eq!(all["total"], json!(3));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn grep_finds_a_regex_with_context_and_skips_binaries() {
        let root = temp_root("grep");
        write_file(&root, "src/main.eps", "alpha\nconst hp = 100;\nbeta\n");
        write_file(&root, "src/other.py", "hp = 250\n");
        fs::create_dir_all(root.join("maps")).expect("maps dir");
        fs::write(root.join("maps").join("base.scx"), [0u8, 1, 2, 3]).expect("binary");

        let found = grep(
            &root,
            &json!({"pattern": "hp\\s*=\\s*\\d+", "contextLines": 1}),
        )
        .expect("grep");
        assert_eq!(found["total"], json!(2));
        assert_eq!(found["skippedFiles"], json!(1));
        let first = &found["matches"][0];
        assert_eq!(first["path"], json!("src/main.eps"));
        assert_eq!(first["startLine"], json!(1));
        assert!(first["text"].as_str().expect("text").contains("alpha"));

        let scoped =
            grep(&root, &json!({"pattern": "hp", "glob": "**/*.py"})).expect("scoped grep");
        assert_eq!(scoped["total"], json!(1));
        assert_eq!(scoped["matches"][0]["path"], json!("src/other.py"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn grep_rejects_an_invalid_regular_expression_with_the_reason() {
        let root = temp_root("badregex");
        let error = grep(&root, &json!({"pattern": "("})).expect_err("unbalanced group");
        assert!(error.contains("regular expression"), "{error}");
        fs::remove_dir_all(&root).ok();
    }
}
