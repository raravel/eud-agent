use super::*;
use crate::worker::WorkerProfile;

#[test]
fn worker_profiles_keep_their_registered_tool_exposure() {
    for profile in [
        WorkerProfile::Research,
        WorkerProfile::MapResearch,
        WorkerProfile::Implementation,
    ] {
        let offered = crate::tools::tool_registry()
            .into_iter()
            .filter(|tool| profile.permits(tool.name))
            .collect::<Vec<_>>();
        assert!(!offered.is_empty(), "profile={}", profile.label());
        for tool in offered {
            assert!(
                !crate::tools::hidden_from_workers(tool.name),
                "profile={} tool={}",
                profile.label(),
                tool.name
            );
            if !profile.can_write() {
                assert!(
                    !tool.requires_write_workspace && !tool.requires_project_transaction,
                    "profile={} tool={}",
                    profile.label(),
                    tool.name
                );
            }
        }
    }
}

#[test]
fn workers_require_a_marker_while_main_legacy_activation_remains_available() {
    assert!(selected_worker_skill("운빨 디펜스 구현 근거 조사").is_none());
    assert!(selected_worker_skill("[$luck-defense]\ninspect existing design").is_some());
    assert!(selected_luck_defense_skill("운빨 디펜스 만들어줘").is_some());
    assert!(selected_luck_defense_skill("[$luck-defense]\ninspect existing design").is_some());
}
