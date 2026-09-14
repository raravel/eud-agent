use super::*;

#[test]
fn generation_publish_load_rewind_and_delete_are_addressed() {
    let (base, store) = store("roundtrip");
    let one = store
        .publish(
            ProviderId::OpencodeGo,
            "session-a",
            0,
            vec![TranscriptEntry::User {
                text: "first".to_string(),
                images: Vec::new(),
            }],
        )
        .unwrap();
    let two = store
        .publish(
            ProviderId::OpencodeGo,
            "session-a",
            one.revision,
            vec![
                TranscriptEntry::User {
                    text: "first".to_string(),
                    images: Vec::new(),
                },
                TranscriptEntry::AssistantText {
                    text: "answer".to_string(),
                },
            ],
        )
        .unwrap();
    store
        .publish(
            ProviderId::Antigravity,
            "session-b",
            0,
            vec![TranscriptEntry::User {
                text: "other".to_string(),
                images: Vec::new(),
            }],
        )
        .unwrap();
    assert_eq!(
        store
            .load_current(ProviderId::OpencodeGo, "session-a")
            .unwrap(),
        two
    );
    store
        .rewind(ProviderId::OpencodeGo, "session-a", 1)
        .unwrap();
    assert_eq!(
        store
            .load_current(ProviderId::OpencodeGo, "session-a")
            .unwrap()
            .checkpoint
            .blocks,
        one.checkpoint.blocks
    );
    store.delete_session("session-a").unwrap();
    assert_eq!(
        store
            .current_revision(ProviderId::OpencodeGo, "session-a")
            .unwrap(),
        0
    );
    assert_eq!(
        store
            .current_revision(ProviderId::Antigravity, "session-b")
            .unwrap(),
        1
    );
    fs::remove_dir_all(base).ok();
}

#[test]
fn mismatched_provider_and_revision_fail_closed() {
    let (base, store) = store("authority");
    store
        .publish(
            ProviderId::OpencodeGo,
            "session-a",
            0,
            vec![TranscriptEntry::AssistantText {
                text: "answer".to_string(),
            }],
        )
        .unwrap();
    assert!(store
        .load_current(ProviderId::Antigravity, "session-a")
        .is_err());
    assert!(store
        .publish(ProviderId::OpencodeGo, "session-a", 0, Vec::new())
        .is_err());
    fs::remove_dir_all(base).ok();
}

#[test]
fn corrupt_or_missing_committed_generation_never_resets_the_pointer() {
    let (base, store) = store("corrupt-head");
    store
        .publish(
            ProviderId::Ollama,
            "session-a",
            0,
            vec![TranscriptEntry::AssistantText {
                text: "done".to_string(),
            }],
        )
        .unwrap();
    let session_dir = store.session_dir("session-a").unwrap();
    let pointer_before = fs::read(session_dir.join("current.json")).unwrap();
    fs::write(session_dir.join("generations/1.json"), b"corrupt").unwrap();

    assert!(store.load_current(ProviderId::Ollama, "session-a").is_err());
    assert_eq!(
        fs::read(session_dir.join("current.json")).unwrap(),
        pointer_before
    );
    fs::remove_file(session_dir.join("generations/1.json")).unwrap();
    assert!(store.load_current(ProviderId::Ollama, "session-a").is_err());
    assert_eq!(
        fs::read(session_dir.join("current.json")).unwrap(),
        pointer_before
    );
    fs::remove_dir_all(base).ok();
}

#[test]
fn orphan_generation_is_never_adopted_or_overwritten_after_pointer_interruption() {
    let (base, store) = store("orphan-generation");
    let committed = store
        .publish(
            ProviderId::Ollama,
            "session-a",
            0,
            vec![TranscriptEntry::AssistantText {
                text: "committed".to_string(),
            }],
        )
        .unwrap();
    let session_dir = store.session_dir("session-a").unwrap();
    let mut orphan = committed.clone();
    orphan.revision = 2;
    orphan.checkpoint.blocks = vec![TranscriptBlock::AssistantText {
        response_id: "orphan-response".to_string(),
        text: "not pointed to".to_string(),
    }];
    orphan.checkpoint.boundary = CheckpointBoundary::ResponseCompleted {
        response_id: "orphan-response".to_string(),
    };
    let orphan_bytes = serde_json::to_vec_pretty(&orphan).unwrap();
    fs::write(session_dir.join("generations/2.json"), &orphan_bytes).unwrap();

    assert_eq!(
        store
            .load_current(ProviderId::Ollama, "session-a")
            .unwrap()
            .revision,
        1
    );
    let retried = store
        .publish(
            ProviderId::Ollama,
            "session-a",
            1,
            vec![TranscriptEntry::AssistantText {
                text: "retry".to_string(),
            }],
        )
        .unwrap();
    assert_eq!(retried.revision, 3);
    assert_eq!(
        fs::read(session_dir.join("generations/2.json")).unwrap(),
        orphan_bytes
    );
    fs::remove_dir_all(base).ok();
}

#[test]
fn reset_clears_only_head_and_next_generation_does_not_overwrite_history() {
    let (base, store) = store("clear-head");
    store
        .publish(
            ProviderId::Ollama,
            "session-a",
            0,
            vec![TranscriptEntry::AssistantText {
                text: "before reset".to_string(),
            }],
        )
        .unwrap();
    let session_dir = store.session_dir("session-a").unwrap();
    let generation_one = fs::read(session_dir.join("generations/1.json")).unwrap();

    store.rewind(ProviderId::Ollama, "session-a", 0).unwrap();

    assert_eq!(
        store
            .current_revision(ProviderId::Ollama, "session-a")
            .unwrap(),
        0
    );
    assert_eq!(
        fs::read(session_dir.join("generations/1.json")).unwrap(),
        generation_one
    );
    let after = store
        .publish(
            ProviderId::Ollama,
            "session-a",
            0,
            vec![TranscriptEntry::AssistantText {
                text: "after reset".to_string(),
            }],
        )
        .unwrap();
    assert_eq!(after.revision, 2);
    assert_eq!(
        fs::read(session_dir.join("generations/1.json")).unwrap(),
        generation_one
    );
    fs::remove_dir_all(base).ok();
}
