//! Compiled-in skill registry.
//!
//! Each skill lives in `skills/<id>/SKILL.md` and is embedded at build time.
//! Adding a skill means adding its folder and one line to [`SKILL_SOURCES`].

use std::sync::OnceLock;

use serde::Serialize;

pub const LUCK_DEFENSE: &str = "luck-defense";

/// Registry order is the deterministic order of active skill guidance.
const SKILL_SOURCES: &[(&str, &str)] = &[
    (
        LUCK_DEFENSE,
        include_str!("../../skills/luck-defense/SKILL.md"),
    ),
    (
        "map-capacity",
        include_str!("../../skills/map-capacity/SKILL.md"),
    ),
    ("release", include_str!("../../skills/release/SKILL.md")),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub id: &'static str,
    pub name: String,
    pub description: String,
    /// Korean display title taken from the body's first `# ` heading.
    pub title: String,
    /// Raw `SKILL.md` text, including any section markers.
    pub source: &'static str,
    /// Text after the frontmatter, trimmed.
    pub body: String,
}

/// Panel-facing skill summary returned by `skill_list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkillSummary {
    pub id: String,
    pub title: String,
    pub description: String,
}

pub fn registry() -> &'static [Skill] {
    static REGISTRY: OnceLock<Vec<Skill>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        SKILL_SOURCES
            .iter()
            .map(|(id, source)| {
                parse_skill(id, source).unwrap_or_else(|error| panic!("skill {id}: {error}"))
            })
            .collect()
    })
}

pub fn find(id: &str) -> Option<&'static Skill> {
    registry().iter().find(|skill| skill.id == id)
}

fn parse_skill(id: &'static str, source: &'static str) -> Result<Skill, String> {
    let text = source.trim_start_matches('\u{feff}');
    let mut lines = text.lines().map(|line| line.trim_end_matches('\r'));
    if lines.next() != Some("---") {
        return Err("missing frontmatter".into());
    }
    let mut name = None;
    let mut description = None;
    let mut closed = false;
    for line in lines.by_ref() {
        if line == "---" {
            closed = true;
            break;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = unquote(value.trim());
        match key.trim() {
            "name" => name = Some(value),
            "description" => description = Some(value),
            _ => {}
        }
    }
    if !closed {
        return Err("unterminated frontmatter".into());
    }
    let body = lines.collect::<Vec<_>>().join("\n").trim().to_string();
    let name = name.ok_or("frontmatter has no name")?;
    if name != id {
        return Err(format!("frontmatter name {name} does not match folder"));
    }
    let title = body
        .lines()
        .find_map(|line| line.strip_prefix("# "))
        .map(|title| title.trim().to_string())
        .ok_or("body has no title heading")?;
    Ok(Skill {
        id,
        name,
        description: description.ok_or("frontmatter has no description")?,
        title,
        source,
        body,
    })
}

fn unquote(value: &str) -> String {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value)
        .to_string()
}

#[tauri::command]
pub fn skill_list() -> Vec<SkillSummary> {
    registry()
        .iter()
        .map(|skill| SkillSummary {
            id: skill.id.to_string(),
            title: skill.title.clone(),
            description: skill.description.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_parses_every_compiled_skill() {
        let ids: Vec<_> = registry().iter().map(|skill| skill.id).collect();
        assert_eq!(ids, [LUCK_DEFENSE, "map-capacity", "release"]);
        let release = find("release").unwrap();
        assert_eq!(release.title, "배포");
        assert!(release.body.contains("release_prepare"));
        let capacity = find("map-capacity").unwrap();
        assert_eq!(capacity.name, "map-capacity");
        assert_eq!(capacity.title, "맵 용량 최적화");
        assert!(capacity
            .description
            .starts_with("Reduce a freeze-protected map"));
        assert!(capacity.body.starts_with("# 맵 용량 최적화"));
        assert!(!capacity.body.contains("name: map-capacity"));
        assert_eq!(find(LUCK_DEFENSE).unwrap().title, "운빨 디펜스 제작");
        assert!(find("unknown").is_none());
    }

    #[test]
    fn parser_accepts_crlf_and_rejects_malformed_frontmatter() {
        let skill = parse_skill(
            "demo",
            "---\r\nname: demo\r\ndescription: \"Short text\"\r\n---\r\n\r\n# 데모\r\nBody",
        )
        .unwrap();
        assert_eq!(skill.description, "Short text");
        assert_eq!(skill.title, "데모");
        assert_eq!(skill.body, "# 데모\nBody");
        assert!(parse_skill("demo", "# 데모").is_err());
        assert!(parse_skill("demo", "---\nname: demo\ndescription: x\n# 데모").is_err());
        assert!(parse_skill("demo", "---\nname: other\ndescription: x\n---\n# 데모").is_err());
        assert!(parse_skill("demo", "---\nname: demo\n---\n# 데모").is_err());
    }

    #[test]
    fn skill_list_exposes_registry_summaries() {
        let list = skill_list();
        assert_eq!(list.len(), registry().len());
        assert_eq!(
            serde_json::to_value(&list[1]).unwrap(),
            serde_json::json!({
                "id": "map-capacity",
                "title": "맵 용량 최적화",
                "description": find("map-capacity").unwrap().description,
            })
        );
    }
}
