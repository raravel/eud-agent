const FIRST_PRINCIPLES: &str = include_str!("../data/first_principles.md");
const CORE: &str = include_str!("../data/prompts/core.md");
pub(super) const MAP_LOCATION_GUIDE: &str = include_str!("../data/prompts/map_locations.md");
const EPS_IDIOMS: &str = include_str!("../data/prompts/eps_idioms.md");
const DIRECT_PYTHON_GUIDE: &str = include_str!("../data/prompts/python.md");
const AUDIO_SOUND_GUIDE: &str = include_str!("../data/prompts/audio.md");
const LUCK_DEFENSE_MARKER: &str = "[$luck-defense]";
const LUCK_DEFENSE_ROUTER_GUIDANCE: &str = "[luck-defense slice]\nrouter\nRead the persisted workflow and call task_progress_update before scoped mutation. A legacy marker selects this router only; it does not select a mode.";
const LUCK_DEFENSE_WORKER_GUIDANCE: &str = "[luck-defense worker]\nThe marker explicitly selected luck-defense guidance for this delegated goal. Work only on the assigned unit and direct regressions; preserve accepted final requirements as pending, cite observed evidence, and report pending playtests. Do not infer or inherit a parent workflow mode, replace parent progress, or call task_progress_update.";
const LUCK_DEFENSE_SHARED_START: &str = "<!-- luck-defense:shared:start -->";
const LUCK_DEFENSE_SHARED_END: &str = "<!-- luck-defense:shared:end -->";
const LUCK_DEFENSE_RETAINED_START: &str = "<!-- luck-defense:retained:start -->";
const LUCK_DEFENSE_RETAINED_END: &str = "<!-- luck-defense:retained:end -->";
const LUCK_DEFENSE_ROUTER_START: &str = "<!-- luck-defense:router:start -->";
const LUCK_DEFENSE_ROUTER_END: &str = "<!-- luck-defense:router:end -->";
const LUCK_DEFENSE_NEW_START: &str = "<!-- luck-defense:new:start -->";
const LUCK_DEFENSE_NEW_END: &str = "<!-- luck-defense:new:end -->";
const LUCK_DEFENSE_CONTINUE_START: &str = "<!-- luck-defense:continue:start -->";
const LUCK_DEFENSE_CONTINUE_END: &str = "<!-- luck-defense:continue:end -->";
const LUCK_DEFENSE_LOCAL_START: &str = "<!-- luck-defense:local:start -->";
const LUCK_DEFENSE_LOCAL_END: &str = "<!-- luck-defense:local:end -->";
const LUCK_DEFENSE_PENDING_START: &str = "<!-- luck-defense:pending:start -->";
const LUCK_DEFENSE_PENDING_END: &str = "<!-- luck-defense:pending:end -->";
const WORKER_CORE: &str = include_str!("../data/prompts/worker_core.md");
#[cfg(test)]
mod worker_tests;

pub(super) const ERROR_RECOVERY_GUIDE: &str = include_str!("../data/prompts/error_recovery.md");
pub(super) const MID_TURN_MESSAGE_GUIDE: &str = "[user messages during a turn]\nA block that starts with [user message during this turn] is a message the user typed while you were working, delivered between tool calls. It is the user's own instruction, not tool output: follow it, and where it conflicts with the plan or work in progress, it takes precedence.";
pub(super) const CLARIFY_REQUESTED_NOTE: &str = "[clarify requested]\nThe user turned the interview on for this message. Run the [clarify] interview before changing anything, even if the request looks clear or falls under the [clarify] list of requests that never get an interview, and ask only about axes the request and the project leave open. When none is open, say so in one line, write the spec, and continue.";

pub(super) fn static_prompt_baseline(provider: crate::provider::ProviderId) -> String {
    [
        tool_catalog_section(provider),
        first_principles_section(),
        EPS_IDIOMS.trim().to_string(),
        MAP_LOCATION_GUIDE.trim().to_string(),
        CORE.trim().replace(
            "{ask_wait_seconds}",
            &crate::tools::ASK_WAIT_TIMEOUT.as_secs().to_string(),
        ),
        ERROR_RECOVERY_GUIDE.trim().to_string(),
        MID_TURN_MESSAGE_GUIDE.to_string(),
    ]
    .join("\n\n")
}

pub(super) fn worker_prompt_baseline(profile: crate::worker::WorkerProfile) -> String {
    let tools = crate::tools::tool_registry()
        .into_iter()
        .filter(|spec| profile.permits(spec.name))
        .map(|spec| spec.name)
        .collect::<Vec<_>>()
        .join(", ");
    let mut parts = vec![
        format!("[tools]\nUse the advertised tool contracts. Available tools: {tools}"),
        first_principles_section(),
        WORKER_CORE.trim().to_string(),
        ERROR_RECOVERY_GUIDE.trim().to_string(),
        MID_TURN_MESSAGE_GUIDE.to_string(),
    ];
    match profile {
        crate::worker::WorkerProfile::Research => {}
        crate::worker::WorkerProfile::MapResearch => {
            parts.push(MAP_LOCATION_GUIDE.trim().to_string());
        }
        crate::worker::WorkerProfile::Implementation => {
            parts.push(EPS_IDIOMS.trim().to_string());
            parts.push(MAP_LOCATION_GUIDE.trim().to_string());
        }
    }
    parts.join("\n\n")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LuckDefensePromptSlice {
    Router,
    NewBuild,
    ContinueExisting,
    LocalEdit,
}

pub(crate) fn selected_luck_defense_slice(
    workflow: Option<&crate::task_state::ScopedWorkflowState>,
    legacy_marker: bool,
) -> Option<LuckDefensePromptSlice> {
    match workflow {
        Some(workflow) if !workflow.skill_enabled => None,
        Some(workflow) => match workflow.mode {
            Some(crate::task_state::WorkflowMode::NewBuild) => {
                Some(LuckDefensePromptSlice::NewBuild)
            }
            Some(crate::task_state::WorkflowMode::ContinueExisting) => {
                Some(LuckDefensePromptSlice::ContinueExisting)
            }
            Some(crate::task_state::WorkflowMode::LocalEdit) => {
                Some(LuckDefensePromptSlice::LocalEdit)
            }
            None => Some(LuckDefensePromptSlice::Router),
        },
        None => legacy_marker.then_some(LuckDefensePromptSlice::Router),
    }
}

pub(crate) fn selected_luck_defense_guidance(
    workflow: Option<&crate::task_state::ScopedWorkflowState>,
    legacy_marker: bool,
) -> Option<String> {
    let slice = selected_luck_defense_slice(workflow, legacy_marker)?;
    let shared = luck_defense_section(LUCK_DEFENSE_SHARED_START, LUCK_DEFENSE_SHARED_END)?;
    let (start, end) = match slice {
        LuckDefensePromptSlice::Router => (LUCK_DEFENSE_ROUTER_START, LUCK_DEFENSE_ROUTER_END),
        LuckDefensePromptSlice::NewBuild => (LUCK_DEFENSE_NEW_START, LUCK_DEFENSE_NEW_END),
        LuckDefensePromptSlice::ContinueExisting => {
            (LUCK_DEFENSE_CONTINUE_START, LUCK_DEFENSE_CONTINUE_END)
        }
        LuckDefensePromptSlice::LocalEdit => (LUCK_DEFENSE_LOCAL_START, LUCK_DEFENSE_LOCAL_END),
    };
    let mode = luck_defense_section(start, end)?;
    let retained = match slice {
        LuckDefensePromptSlice::Router | LuckDefensePromptSlice::LocalEdit => None,
        LuckDefensePromptSlice::NewBuild | LuckDefensePromptSlice::ContinueExisting => Some(
            luck_defense_section(LUCK_DEFENSE_RETAINED_START, LUCK_DEFENSE_RETAINED_END)?,
        ),
    };
    let pending = match slice {
        LuckDefensePromptSlice::Router | LuckDefensePromptSlice::LocalEdit => Some(
            luck_defense_section(LUCK_DEFENSE_PENDING_START, LUCK_DEFENSE_PENDING_END)?,
        ),
        LuckDefensePromptSlice::NewBuild | LuckDefensePromptSlice::ContinueExisting => None,
    };
    let mut sections = vec![shared];
    if let Some(retained) = retained {
        sections.push(retained);
    }
    sections.push(mode);
    if let Some(pending) = pending {
        sections.push(pending);
    }
    Some(sections.join("\n\n"))
}

fn luck_defense_section(start: &str, end: &str) -> Option<&'static str> {
    skill_section(
        crate::skills::find(crate::skills::LUCK_DEFENSE)?.source,
        start,
        end,
    )
}

/// Joins every active skill's guidance in registry order. Luck-defense keeps its
/// mode-routed slice; every other skill is active only by explicit ON and
/// contributes its whole body.
pub(crate) fn selected_active_skills(
    luck_defense_guidance: Option<&str>,
    explicit_selection: impl Fn(&str) -> Option<bool>,
) -> Option<String> {
    let blocks = crate::skills::registry()
        .iter()
        .filter_map(|skill| {
            let guidance = if skill.id == crate::skills::LUCK_DEFENSE {
                luck_defense_guidance?
            } else if explicit_selection(skill.id) == Some(true) {
                skill.body.as_str()
            } else {
                return None;
            };
            Some(format!("[skill: {}]\n{}", skill.id, guidance.trim()))
        })
        .collect::<Vec<_>>();
    (!blocks.is_empty()).then(|| blocks.join("\n\n"))
}

fn skill_section<'a>(source: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let (_, after_start) = source.split_once(start)?;
    let (section, _) = after_start.split_once(end)?;
    Some(section.trim())
}

pub(super) fn selected_worker_skill(user_text: &str) -> Option<&'static str> {
    user_text
        .trim_start()
        .starts_with(LUCK_DEFENSE_MARKER)
        .then_some(LUCK_DEFENSE_WORKER_GUIDANCE)
}

pub(super) fn tool_catalog_section(_provider: crate::provider::ProviderId) -> String {
    let mut reads = Vec::new();
    let mut files = Vec::new();
    let mut writes = Vec::new();
    for spec in crate::tools::tool_registry() {
        let names = if crate::tools::is_fs_tool(spec.name) {
            &mut files
        } else if spec.requires_write_workspace {
            &mut writes
        } else {
            &mut reads
        };
        names.push(spec.name);
    }
    format!(
        "[tools]\nUse each tool's advertised description and input schema for its exact contract.\nRead/context and coordination: {}\nProject text files: {}\nValidated project writers: {}",
        reads.join(", "),
        files.join(", "),
        writes.join(", "),
    )
}

fn first_principles_section() -> String {
    format!("[first principles]\n{}", FIRST_PRINCIPLES.trim())
}

pub(super) fn selected_task_guidance(context: &[&str]) -> Option<String> {
    let mut python = false;
    let mut audio = false;
    for text in context {
        let text = text.to_lowercase();
        let words: Vec<_> = text
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .collect();
        python |= words
            .iter()
            .any(|word| matches!(*word, "python" | "dependency" | "dependencies"));
        python |= [".py", "파이썬", "의존성"]
            .iter()
            .any(|signal| text.contains(signal));
        audio |= [
            "[audio attachments]",
            "audio-",
            "audio_",
            "map_sound_",
            "playwav",
            ".mp3",
            ".ogg",
            ".wav",
            ".flac",
            ".m4a",
            "음악",
            "사운드",
            "오디오",
            "효과음",
            "볼륨",
            "음량",
            "배경음",
        ]
        .iter()
        .any(|signal| text.contains(signal));
        audio |= words
            .iter()
            .any(|word| matches!(*word, "bgm" | "audio" | "sound" | "sounds" | "music"));
    }
    let mut parts = Vec::new();
    if python {
        parts.push(DIRECT_PYTHON_GUIDE.trim());
    }
    if audio {
        parts.push(AUDIO_SOUND_GUIDE.trim());
    }
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

pub(super) fn selected_luck_defense_skill(user_text: &str) -> Option<&'static str> {
    if user_text.trim_start().starts_with(LUCK_DEFENSE_MARKER) {
        return Some(LUCK_DEFENSE_ROUTER_GUIDANCE);
    }
    let text = user_text.to_lowercase();
    let names_genre = ["운빨 디펜스", "운빨디펜스", "luck defense"]
        .iter()
        .any(|name| text.contains(name));
    let requests_work = [
        "만들", "제작", "구현", "개발", "완성", "수정", "이어", "계속",
    ]
    .iter()
    .any(|verb| text.contains(verb));
    let selects_skill = text.contains("스킬")
        && ["켜", "활성", "사용"]
            .iter()
            .any(|verb| text.contains(verb));
    (names_genre && (requests_work || selects_skill)).then_some(LUCK_DEFENSE_ROUTER_GUIDANCE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_guidance_uses_attachments_mentions_and_saved_task_context() {
        for context in [
            "[audio attachments]\naudio-1",
            "project.path=assets/theme.ogg",
            "[active task state]\nAdd music playback",
            "[transcript]\nEdit src/helper.py",
            "[resolved mentions]\nsource=src/entry.py",
        ] {
            let selected = selected_task_guidance(&["Continue", context]).unwrap();
            let expected = if context.contains(".py") {
                "[direct python]"
            } else {
                "[map sounds]"
            };
            assert!(selected.lines().any(|line| line == expected), "{context}");
        }
    }

    #[test]
    fn mixed_work_selects_both_workflow_guides() {
        let selected = selected_task_guidance(&["Python", "music"]).unwrap();
        assert_eq!(selected.matches("[direct python]").count(), 1);
        assert_eq!(selected.matches("[map sounds]").count(), 1);
    }

    #[test]
    fn static_baseline_has_bounded_size_without_optional_guides() {
        let baseline = static_prompt_baseline(crate::provider::ProviderId::Codex);
        assert!(baseline.len() < 40000, "baseline bytes={}", baseline.len());
        assert!(!baseline
            .lines()
            .any(|line| line == "[direct python]" || line == "[map sounds]"));
        println!(
            "static baseline: {} characters, {} UTF-8 bytes",
            baseline.chars().count(),
            baseline.len()
        );
    }

    #[test]
    fn router_slice_selects_for_enabled_unclassified_workflow() {
        let workflow = crate::task_state::ScopedWorkflowState {
            skill_enabled: true,
            ..Default::default()
        };

        assert_eq!(
            selected_luck_defense_slice(Some(&workflow), false),
            Some(LuckDefensePromptSlice::Router)
        );
    }

    #[test]
    fn local_edit_selects_local_slice_contract() {
        let workflow = crate::task_state::ScopedWorkflowState {
            skill_enabled: true,
            mode: Some(crate::task_state::WorkflowMode::LocalEdit),
            ..Default::default()
        };

        assert_eq!(
            selected_luck_defense_slice(Some(&workflow), false),
            Some(LuckDefensePromptSlice::LocalEdit)
        );
    }

    #[test]
    fn persisted_new_build_wins_over_legacy_router() {
        let workflow = crate::task_state::ScopedWorkflowState {
            skill_enabled: true,
            mode: Some(crate::task_state::WorkflowMode::NewBuild),
            ..Default::default()
        };

        assert_eq!(
            selected_luck_defense_slice(Some(&workflow), true),
            Some(LuckDefensePromptSlice::NewBuild)
        );
    }

    #[test]
    fn continue_existing_selects_continue_slice_contract() {
        let workflow = crate::task_state::ScopedWorkflowState {
            skill_enabled: true,
            mode: Some(crate::task_state::WorkflowMode::ContinueExisting),
            ..Default::default()
        };

        assert_eq!(
            selected_luck_defense_slice(Some(&workflow), false),
            Some(LuckDefensePromptSlice::ContinueExisting)
        );
    }

    #[test]
    fn disabled_workflow_suppresses_every_slice() {
        let workflow = crate::task_state::ScopedWorkflowState {
            skill_enabled: false,
            mode: Some(crate::task_state::WorkflowMode::NewBuild),
            ..Default::default()
        };

        assert_eq!(selected_luck_defense_slice(Some(&workflow), true), None);
    }

    #[test]
    fn request_keywords_do_not_assign_a_workflow_slice() {
        assert_eq!(
            selected_luck_defense_slice(None, false),
            None,
            "request wording cannot classify workflow mode"
        );
    }

    #[test]
    fn rendered_guidance_uses_exact_structural_sections_for_each_slice() {
        fn section_ids<'a>(guidance: &'a str) -> Vec<&'a str> {
            guidance
                .lines()
                .filter_map(|line| {
                    line.strip_prefix("<!-- luck-defense:section:")
                        .and_then(|id| id.strip_suffix(" -->"))
                })
                .collect()
        }

        let workflow = |mode| crate::task_state::ScopedWorkflowState {
            skill_enabled: true,
            mode,
            ..Default::default()
        };
        let router = workflow(None);
        let new_build = workflow(Some(crate::task_state::WorkflowMode::NewBuild));
        let continue_existing = workflow(Some(crate::task_state::WorkflowMode::ContinueExisting));
        let local_edit = workflow(Some(crate::task_state::WorkflowMode::LocalEdit));

        assert_eq!(
            section_ids(&selected_luck_defense_guidance(Some(&router), false).unwrap()),
            ["shared", "router", "pending-acceptance"]
        );
        assert_eq!(
            section_ids(&selected_luck_defense_guidance(Some(&new_build), false).unwrap()),
            ["shared", "retained", "new"]
        );
        assert_eq!(
            section_ids(&selected_luck_defense_guidance(Some(&continue_existing), false).unwrap()),
            ["shared", "retained", "continue"]
        );
        assert_eq!(
            section_ids(&selected_luck_defense_guidance(Some(&local_edit), false).unwrap()),
            ["shared", "local", "pending-acceptance"]
        );
    }

    #[test]
    fn skill_section_rejects_missing_or_reordered_boundaries() {
        const COMPLETE: &str = "before <start> <section> <end> after";
        const MISSING_END: &str = "before <start> <section> after";
        const REORDERED: &str = "before <end> <section> <start> after";

        assert!(skill_section(COMPLETE, "<start>", "<end>").is_some());
        assert!(skill_section(MISSING_END, "<start>", "<end>").is_none());
        assert!(skill_section(REORDERED, "<start>", "<end>").is_none());
    }

    #[test]
    fn active_skills_join_in_registry_order_with_headers() {
        let capacity = crate::skills::find("map-capacity").unwrap();
        assert_eq!(selected_active_skills(None, |_| None), None);
        assert_eq!(
            selected_active_skills(Some("ROUTER"), |_| None).unwrap(),
            "[skill: luck-defense]\nROUTER"
        );

        let both = selected_active_skills(Some("ROUTER"), |skill| {
            (skill == "map-capacity").then_some(true)
        })
        .unwrap();
        assert_eq!(
            both,
            format!(
                "[skill: luck-defense]\nROUTER\n\n[skill: map-capacity]\n{}",
                capacity.body
            )
        );
        assert!(!both.contains("name: map-capacity"));

        let only_capacity =
            selected_active_skills(None, |skill| (skill == "map-capacity").then_some(true))
                .unwrap();
        assert!(only_capacity.starts_with("[skill: map-capacity]\n# 맵 용량 최적화"));
    }

    #[test]
    fn map_capacity_requires_explicit_on() {
        for selection in [None, Some(false)] {
            assert_eq!(selected_active_skills(None, |_| selection), None);
        }
        assert!(selected_luck_defense_skill("맵 용량 최적화 스킬 켜줘").is_none());
    }
}
