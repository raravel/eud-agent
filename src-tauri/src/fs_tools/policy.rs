use serde_json::Value;

fn is_document_path(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("txt")
        })
}

/// Map and investigation roles can write documents without gaining source or
/// binary authority. Path confinement is still enforced by the filesystem tool.
/// With the project `root`, the path is checked as it is on disk (where the
/// write lands), not only as the caller spelled it.
pub(crate) fn require_document_write(
    tool: &str,
    args: &Value,
    root: Option<&std::path::Path>,
) -> Result<(), String> {
    if matches!(
        tool,
        crate::tools::FS_WRITE_TOOL | crate::tools::FS_EDIT_TOOL
    ) {
        let path = super::str_arg(args, "path")?;
        let normalized = match root {
            Some(root) => super::resolve(root, path)?.0,
            None => {
                crate::native_project::normalize_relative_path(&path.trim().replace('\\', "/"))?
            }
        };
        if !is_document_path(&normalized) {
            return Err(format!(
                "{tool}: 이 역할은 .md, .txt 문서만 쓸 수 있습니다: '{path}'"
            ));
        }
    }
    Ok(())
}

/// Top-level folders agent CLIs and CI load instructions (or run workflows)
/// from; everything inside one counts.
const INSTRUCTION_FOLDERS: &[&str] = &[
    ".claude",
    ".codex",
    ".gemini",
    ".cursor",
    ".github",
    ".agents",
    ".clinerules",
];
/// The editor files (root-relative) that run or steer tools on their own:
/// VS Code's settings (terminal profiles, formatters, `*.path` executables),
/// `tasks.json` (`runOn: folderOpen`), `launch.json`, recommended
/// extensions and MCP servers; JetBrains' File Watchers (run on save),
/// `startup.xml` (the shared Startup Tasks list, run when the project opens)
/// and `workspace.xml`, which holds the non-shared run configurations and the
/// local Startup Tasks that run them. All of them need the
/// user's card for `fs_write`/`fs_edit` and are blocked for the native CLIs.
/// `workspace.xml` is also rewritten by an open JetBrains IDE all the time,
/// so the turn-end guard only watches it ([`is_watch_only_instruction_path`])
/// instead of putting it back. The rest of `.vscode/` and `.idea/` (shelved
/// changes, caches, code styles) is ordinary editor state and not guarded.
const EDITOR_INSTRUCTION_FILES: &[&str] = &[
    ".vscode/settings.json",
    ".vscode/tasks.json",
    ".vscode/launch.json",
    ".vscode/extensions.json",
    ".vscode/mcp.json",
    ".idea/workspace.xml",
    ".idea/watcherTasks.xml",
    ".idea/startup.xml",
];
/// Editor folders (root-relative) whose whole contents run: JetBrains'
/// shared run configurations, which Startup Tasks launch on open.
const EDITOR_INSTRUCTION_FOLDERS: &[&str] = &[".idea/runConfigurations"];

/// The editor folders that hold [`EDITOR_INSTRUCTION_FILES`] and
/// [`EDITOR_INSTRUCTION_FOLDERS`]: not guarded themselves, but where a
/// provider profile looks for them.
pub(crate) const EDITOR_FOLDERS: &[&str] = &[".vscode", ".idea"];
/// File names agent CLIs read as instructions or tool settings, at any depth.
const INSTRUCTION_FILES: &[&str] = &[
    "claude.md",
    "claude.local.md",
    "agents.md",
    "agents.override.md",
    "gemini.md",
    "skill.md",
    ".mcp.json",
    ".cursorrules",
    ".windsurfrules",
    ".clinerules",
];

/// Whether `relative` (a normalized, `/`-separated project path) is a file
/// an agent CLI reads as its own instructions: anything under a top-level
/// [`INSTRUCTION_FOLDERS`] folder, one of the [`EDITOR_INSTRUCTION_FILES`]
/// or anything under an [`EDITOR_INSTRUCTION_FOLDERS`] folder, or a file
/// named like [`INSTRUCTION_FILES`] anywhere, in any case. A session writing
/// one would change what the next agent (or editor) run does, so the main
/// EPS session asks the user first and every other role is refused.
pub(crate) fn is_agent_instruction_path(relative: &str) -> bool {
    let mut parts = relative.split('/');
    let first = parts.next().unwrap_or(relative);
    let nested = relative.contains('/');
    if nested
        && INSTRUCTION_FOLDERS
            .iter()
            .any(|folder| first.eq_ignore_ascii_case(folder))
    {
        return true;
    }
    if EDITOR_INSTRUCTION_FILES
        .iter()
        .any(|file| relative.eq_ignore_ascii_case(file))
    {
        return true;
    }
    if EDITOR_INSTRUCTION_FOLDERS.iter().any(|folder| {
        relative.len() > folder.len() + 1
            && relative
                .get(..folder.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(folder))
            && relative.as_bytes().get(folder.len()) == Some(&b'/')
    }) {
        return true;
    }
    let name = relative.rsplit('/').next().unwrap_or(relative);
    INSTRUCTION_FILES
        .iter()
        .any(|file| name.eq_ignore_ascii_case(file))
}

/// The [`EDITOR_INSTRUCTION_FILES`] the IDE itself keeps rewriting.
const WATCH_ONLY_INSTRUCTION_FILES: &[&str] = &[".idea/workspace.xml"];

/// Whether the turn-end guard only watches the instruction path `relative`:
/// it is never put back or deleted, but a change a turn made without the
/// card stays out of the commits and is reported. It still needs the card
/// for `fs_write`/`fs_edit`, and the native CLIs stay blocked from it.
pub(crate) fn is_watch_only_instruction_path(relative: &str) -> bool {
    WATCH_ONLY_INSTRUCTION_FILES
        .iter()
        .any(|file| relative.eq_ignore_ascii_case(file))
}

/// [`is_agent_instruction_path`] as root-relative globs for a provider's own
/// path rules: `<folder>/**` per folder, `**/<name>` per file name in the
/// spelling its CLI uses (`CLAUDE.md`) and in lower case. Glob rules are
/// case-sensitive where the predicate is not, so other mixed spellings stay
/// to the provider's sandbox and the turn-start checks.
pub(crate) fn agent_instruction_globs() -> Vec<String> {
    let mut globs = INSTRUCTION_FOLDERS
        .iter()
        .chain(EDITOR_INSTRUCTION_FOLDERS)
        .map(|folder| format!("{folder}/**"))
        .chain(EDITOR_INSTRUCTION_FILES.iter().map(|file| file.to_string()))
        .collect::<Vec<_>>();
    for name in INSTRUCTION_FILES {
        let canonical = match name.find('.') {
            Some(stem) if stem > 0 => {
                format!("{}{}", name[..stem].to_ascii_uppercase(), &name[stem..])
            }
            _ => name.to_string(),
        };
        for spelling in [canonical, name.to_string()] {
            let glob = format!("**/{spelling}");
            if !globs.contains(&glob) {
                globs.push(glob);
            }
        }
    }
    globs
}

/// The `/`-separated components of `path` without its Windows verbatim
/// prefix (`\\?\`, `\\?\UNC\`), compared case-insensitively on Windows.
fn path_parts(path: &str) -> (Vec<String>, bool) {
    let mut normalized = path.trim().replace('\\', "/");
    if let Some(rest) = normalized.strip_prefix("//?/UNC/") {
        normalized = format!("//{rest}");
    } else if let Some(rest) = normalized.strip_prefix("//?/") {
        normalized = rest.to_string();
    }
    let bytes = normalized.as_bytes();
    let absolute = normalized.starts_with('/')
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':');
    let parts = normalized
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .map(|part| {
            if cfg!(windows) {
                part.to_ascii_lowercase()
            } else {
                part.to_string()
            }
        })
        .collect();
    (parts, absolute)
}

/// Whether a path a provider's own file-change tool names could be an
/// agent-instruction file. With the provider's project `root` known, a
/// path is judged relative to it (the root's own folders, such as a root
/// under `.vscode/`, never count): a relative path is root-relative, and an
/// absolute path outside the root counts, since this session writes only
/// inside it. Without a root, some tail of it is one by
/// [`is_agent_instruction_path`], wherever the root sits along it. Fails
/// closed: an empty path or one with `..` counts, since where it lands
/// cannot be told from its spelling.
pub(crate) fn touches_agent_instructions(path: &str, root: Option<&std::path::Path>) -> bool {
    let (parts, absolute) = path_parts(path);
    if parts.is_empty() || parts.iter().any(|part| part == "..") {
        return true;
    }
    if let Some(root) = root {
        if !absolute {
            return is_agent_instruction_path(&parts.join("/"));
        }
        let spellings = [std::fs::canonicalize(root).ok(), Some(root.to_path_buf())];
        let inside = spellings.iter().flatten().find_map(|root| {
            let (root_parts, _) = path_parts(&root.to_string_lossy());
            (!root_parts.is_empty() && parts.starts_with(&root_parts))
                .then(|| parts[root_parts.len()..].join("/"))
        });
        return match inside {
            Some(relative) if relative.is_empty() => true,
            Some(relative) => is_agent_instruction_path(&relative),
            None => true,
        };
    }
    (0..parts.len()).any(|start| is_agent_instruction_path(&parts[start..].join("/")))
}

pub(super) fn write_refusal(relative: &str) -> Option<String> {
    let first = relative.split('/').next().unwrap_or(relative);
    let reason = if first.eq_ignore_ascii_case(".git") {
        "git 히스토리는 이 프로젝트의 되돌리기 수단 자체입니다."
    } else if first.eq_ignore_ascii_case(".claude") {
        // Before the document rule: a `.claude/` document is no less a
        // `.claude/` entry, and its folder alone stops every Claude turn.
        "Claude Code CLI가 이 폴더를 자기 설정·명령·에이전트로 읽기 때문에, 프로젝트에 .claude/ 폴더가 있으면 Claude Code 세션의 모든 턴이 시작 전에 거부됩니다. 이 폴더는 만들지도 고치지도 말고, 필요하면 사용자에게 직접 하도록(이미 있으면 지우도록) 요청하세요."
    } else if is_document_path(relative) {
        return None;
    } else if first.eq_ignore_ascii_case("maps") || first.eq_ignore_ascii_case("references") {
        "맵 파일은 MapSafe의 백업·CHK 재추출 검증·공유락을 거쳐야 합니다. 맵 변경은 Map 세션이나 map_* 도구로 하세요."
    } else if relative.eq_ignore_ascii_case(".mcp.json") || first.eq_ignore_ascii_case(".codex") {
        "에이전트 권한 설정은 세션이 스스로 바꾸지 않습니다."
    } else {
        return None;
    };
    Some(format!("'{relative}'에는 쓸 수 없습니다: {reason}"))
}
