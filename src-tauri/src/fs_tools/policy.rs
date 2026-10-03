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
pub(crate) fn require_document_write(tool: &str, args: &Value) -> Result<(), String> {
    if matches!(
        tool,
        crate::tools::FS_WRITE_TOOL | crate::tools::FS_EDIT_TOOL
    ) {
        let path = super::str_arg(args, "path")?;
        let normalized =
            crate::native_project::normalize_relative_path(&path.trim().replace('\\', "/"))?;
        if !is_document_path(&normalized) {
            return Err(format!(
                "{tool}: 이 역할은 .md, .txt 문서만 쓸 수 있습니다: '{path}'"
            ));
        }
    }
    Ok(())
}

pub(super) fn write_refusal(relative: &str) -> Option<String> {
    let first = relative.split('/').next().unwrap_or(relative);
    let reason = if first.eq_ignore_ascii_case(".git") {
        "git 히스토리는 이 프로젝트의 되돌리기 수단 자체입니다."
    } else if is_document_path(relative) {
        return None;
    } else if first.eq_ignore_ascii_case("maps") || first.eq_ignore_ascii_case("references") {
        "맵 파일은 MapSafe의 백업·CHK 재추출 검증·공유락을 거쳐야 합니다. 맵 변경은 Map 세션이나 map_* 도구로 하세요."
    } else if first.eq_ignore_ascii_case(".claude")
        || relative.eq_ignore_ascii_case(".mcp.json")
        || first.eq_ignore_ascii_case(".codex")
    {
        "에이전트 권한 설정은 세션이 스스로 바꾸지 않습니다."
    } else {
        return None;
    };
    Some(format!("'{relative}'에는 쓸 수 없습니다: {reason}"))
}
