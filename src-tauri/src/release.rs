//! Release (배포) versions and names.
//!
//! A release is `vMAJOR.MINOR`. Each one is recorded as the git tag
//! `release/<version>` on the turn commit whose build succeeded, so the
//! changelog of the next release is every project commit after the newest such
//! tag. A release also names the build output after itself
//! (`build/[EUD]{title}{version}.scx`) and writes `{title} {version}` into the
//! OUTPUT map's scenario title; the source map is never touched.

use std::path::Path;

/// Prefix of every release tag.
pub const TAG_PREFIX: &str = "release/";
/// Where the release changelog lives, newest section first.
pub const CHANGELOG_PATH: &str = "CHANGELOG.md";
/// Most commits `release_history` reads after the previous release.
pub const HISTORY_COMMIT_LIMIT: usize = 200;

/// A release version, ordered by major then minor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReleaseVersion {
    pub major: u32,
    pub minor: u32,
}

impl ReleaseVersion {
    /// The version of a project's first release.
    pub const FIRST: Self = Self { major: 1, minor: 0 };

    /// Parse `v1.2` (the leading `v` may be omitted or upper case).
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let digits = text.strip_prefix(['v', 'V']).unwrap_or(text);
        let invalid = || format!("버전 '{text}'은 vMAJOR.MINOR 형식이어야 합니다 (예: v1.2).");
        let (major, minor) = digits.split_once('.').ok_or_else(invalid)?;
        let number = |part: &str| -> Result<u32, String> {
            if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(invalid());
            }
            part.parse().map_err(|_| invalid())
        };
        Ok(Self {
            major: number(major)?,
            minor: number(minor)?,
        })
    }

    /// The default next release: one minor version up.
    pub fn next_minor(self) -> Self {
        Self {
            major: self.major,
            minor: self.minor.saturating_add(1),
        }
    }

    /// The git tag that records this release.
    pub fn tag(self) -> String {
        format!("{TAG_PREFIX}{self}")
    }
}

impl std::fmt::Display for ReleaseVersion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "v{}.{}", self.major, self.minor)
    }
}

/// The version a release tag names, or `None` for a tag that is not one.
pub fn version_of_tag(tag: &str) -> Option<ReleaseVersion> {
    let version = tag.strip_prefix(TAG_PREFIX)?;
    version
        .starts_with('v')
        .then(|| ReleaseVersion::parse(version).ok())
        .flatten()
}

/// The newest release tag of the project and its version, by version order.
pub fn latest_release(root: &Path) -> Result<Option<(String, ReleaseVersion)>, String> {
    Ok(crate::git::tags_matching(root, &format!("{TAG_PREFIX}*"))?
        .into_iter()
        .filter_map(|tag| version_of_tag(&tag).map(|version| (tag, version)))
        .max_by_key(|(_, version)| *version))
}

/// The version a release gets: the requested one, which must be newer than the
/// previous release, or one minor version up from it (`v1.0` for the first).
pub fn resolve_version(
    requested: Option<&str>,
    previous: Option<ReleaseVersion>,
) -> Result<ReleaseVersion, String> {
    let Some(requested) = requested else {
        return Ok(previous.map_or(ReleaseVersion::FIRST, ReleaseVersion::next_minor));
    };
    let version = ReleaseVersion::parse(requested)?;
    match previous {
        Some(previous) if version <= previous => Err(format!(
            "새 버전 {version}은 이전 배포 {previous}보다 커야 합니다. 더 큰 버전을 지정하세요."
        )),
        _ => Ok(version),
    }
}

/// A scenario title without a trailing release version, so a source map whose
/// title already reads `Name v1.2` does not become `Name v1.2 v1.3`.
pub fn title_without_version(title: &str) -> &str {
    let trimmed = title.trim_end();
    match trimmed.rsplit_once(char::is_whitespace) {
        Some((head, last))
            if (last.starts_with('v') || last.starts_with('V'))
                && ReleaseVersion::parse(last).is_ok()
                && !head.trim().is_empty() =>
        {
            head.trim_end()
        }
        _ => trimmed,
    }
}

/// A title as it may appear in a file name: StarCraft color/control codes and
/// characters Windows refuses in a file name are dropped, and no trailing dot
/// or space is left.
pub fn file_title(title: &str) -> String {
    let kept: String = title
        .chars()
        .filter(|character| {
            !character.is_control()
                && !matches!(
                    character,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                )
        })
        .collect();
    kept.trim().trim_end_matches(['.', ' ']).to_string()
}

/// The release build output: `build/[EUD]{title}{version}.{extension}`.
pub fn output_map(file_title: &str, version: ReleaseVersion, extension: &str) -> String {
    format!("build/[EUD]{file_title}{version}.{extension}")
}

/// The in-game scenario title of a release build: `{title} {version}`.
pub fn output_title(title: &str, version: ReleaseVersion) -> String {
    format!("{} {version}", title.trim_end())
}

/// Record a release on the turn commit, or say in Korean why it was not.
///
/// The tag goes only on a commit this boundary made, and only when the
/// request built the current revision successfully: a release whose build
/// failed, or whose work was not recorded, is not a release.
pub fn tag_release(
    root: &Path,
    version: &str,
    commit: Option<&crate::git::CommitRecord>,
    built: bool,
) -> Result<String, String> {
    let tag = format!("{TAG_PREFIX}{version}");
    if !built {
        return Err(format!(
            "현재 상태의 빌드가 성공하지 않아 배포 태그 {tag}를 만들지 않았습니다. 빌드 오류를 고친 뒤 다시 배포하세요."
        ));
    }
    let Some(commit) = commit else {
        return Err(format!(
            "이 턴에 기록된 커밋이 없어 배포 태그 {tag}를 만들지 않았습니다."
        ));
    };
    crate::git::tag(root, &tag, &commit.sha)
        .map_err(|error| format!("배포 태그 {tag}를 만들지 못했습니다: {error}"))?;
    Ok(tag)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(text: &str) -> ReleaseVersion {
        ReleaseVersion::parse(text).unwrap()
    }

    #[test]
    fn versions_parse_compare_and_bump() {
        assert_eq!(version("v1.2"), ReleaseVersion { major: 1, minor: 2 });
        assert_eq!(version("1.10").to_string(), "v1.10");
        assert_eq!(version("V2.0").tag(), "release/v2.0");
        for bad in ["", "v1", "v1.", "v.1", "v1.2.3", "va.b", "v-1.2", "v1.+2"] {
            assert!(ReleaseVersion::parse(bad).is_err(), "{bad}");
        }
        assert!(version("v1.10") > version("v1.9"));
        assert!(version("v2.0") > version("v1.99"));
        assert_eq!(version("v1.9").next_minor(), version("v1.10"));

        assert_eq!(resolve_version(None, None).unwrap(), ReleaseVersion::FIRST);
        assert_eq!(
            resolve_version(None, Some(version("v1.2"))).unwrap(),
            version("v1.3")
        );
        assert_eq!(
            resolve_version(Some("v2.0"), Some(version("v1.2"))).unwrap(),
            version("v2.0")
        );
        assert!(resolve_version(Some("v1.2"), Some(version("v1.2"))).is_err());
        assert!(resolve_version(Some("v1.1"), Some(version("v1.2"))).is_err());
        assert!(resolve_version(Some("1.2.3"), None).is_err());

        assert_eq!(version_of_tag("release/v1.3"), Some(version("v1.3")));
        assert_eq!(version_of_tag("release/1.3"), None);
        assert_eq!(version_of_tag("v1.3"), None);
    }

    #[test]
    fn release_names_follow_the_title_and_version() {
        let v13 = version("v1.3");
        assert_eq!(
            output_map(&file_title("운빨디펜스"), v13, "scx"),
            "build/[EUD]운빨디펜스v1.3.scx"
        );
        assert_eq!(output_title("운빨디펜스", v13), "운빨디펜스 v1.3");
        assert_eq!(file_title("\u{3}운빨\u{4}:디펜스?. "), "운빨디펜스");
        assert_eq!(title_without_version("운빨디펜스 v1.2"), "운빨디펜스");
        assert_eq!(title_without_version("Arena 2.0"), "Arena 2.0");
        assert_eq!(title_without_version("v1.2"), "v1.2");
    }

    fn temp_repo(tag: &str) -> Option<std::path::PathBuf> {
        if !crate::git::available() {
            eprintln!("skipping {tag}: git is not installed on this machine");
            return None;
        }
        let root =
            std::env::temp_dir().join(format!("eud-agent-release-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.eps"), "const a = 1;").unwrap();
        crate::git::prepare(&root);
        Some(root)
    }

    fn commit(root: &Path, file: &str, text: &str, subject: &str) -> crate::git::CommitRecord {
        let path = root.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
        crate::git::commit_turn(root, subject).unwrap().unwrap()
    }

    #[test]
    fn history_since_the_last_release_tag_covers_only_later_commits() {
        let Some(root) = temp_repo("history") else {
            return;
        };
        let (all, total) = crate::git::commits_since(&root, None, 50).unwrap();
        assert_eq!(total, 1, "the initial commit");
        assert_eq!(all.len(), 1);
        assert_eq!(latest_release(&root).unwrap(), None);

        let first = commit(&root, "src/main.eps", "const a = 2;", "체력 상향");
        tag_release(&root, "v1.0", Some(&first), true).unwrap();
        commit(&root, "src/유닛.eps", "const b = 1;", "새 유닛 추가");
        let last = commit(&root, "CHANGELOG.md", "# changes", "변경 기록");
        tag_release(&root, "v1.2", Some(&last), true).unwrap();
        commit(&root, "src/main.eps", "const a = 3;", "보상 조정");

        let (tag, latest) = latest_release(&root).unwrap().unwrap();
        assert_eq!((tag.as_str(), latest), ("release/v1.2", version("v1.2")));
        let (since, total) = crate::git::commits_since(&root, Some(&tag), 50).unwrap();
        assert_eq!(total, 1);
        assert_eq!(since[0].subject, "보상 조정");
        assert_eq!(since[0].paths, ["src/main.eps"]);

        let (since_first, total) =
            crate::git::commits_since(&root, Some("release/v1.0"), 1).unwrap();
        assert_eq!(total, 3);
        assert_eq!(since_first.len(), 1, "bounded by the limit");
        let (since_first, _) = crate::git::commits_since(&root, Some("release/v1.0"), 50).unwrap();
        let subjects: Vec<_> = since_first.iter().map(|c| c.subject.as_str()).collect();
        assert_eq!(subjects, ["보상 조정", "변경 기록", "새 유닛 추가"]);
        assert_eq!(
            since_first[2].paths,
            ["src/유닛.eps"],
            "non-ASCII paths unquoted"
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_release_is_tagged_only_after_a_successful_build_of_a_recorded_commit() {
        let Some(root) = temp_repo("tag") else {
            return;
        };
        let record = commit(&root, "CHANGELOG.md", "## v1.0", "배포 v1.0");

        let failed = tag_release(&root, "v1.0", Some(&record), false).unwrap_err();
        assert!(failed.contains("빌드"), "{failed}");
        assert!(crate::git::tags_matching(&root, "release/*")
            .unwrap()
            .is_empty());

        let uncommitted = tag_release(&root, "v1.0", None, true).unwrap_err();
        assert!(uncommitted.contains("커밋"), "{uncommitted}");
        assert!(crate::git::tags_matching(&root, "release/*")
            .unwrap()
            .is_empty());

        assert_eq!(
            tag_release(&root, "v1.0", Some(&record), true).unwrap(),
            "release/v1.0"
        );
        // The tag sits on the release commit, which is HEAD: nothing follows it.
        assert_eq!(
            crate::git::commits_since(&root, Some("release/v1.0"), 10)
                .unwrap()
                .1,
            0
        );
        // An existing release tag is never moved.
        assert!(tag_release(&root, "v1.0", Some(&record), true).is_err());
        std::fs::remove_dir_all(root).ok();
    }
}
