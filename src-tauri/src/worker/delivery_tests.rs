use super::*;
use crate::context_state::ContextDeliveryMode;

#[test]
fn fresh_context_restores_recent_reported_results() {
    let mut worker = WorkerRecord::new("child".into(), "req".into(), "task".into(), "goal".into());
    worker.status = WorkerStatus::Settled;
    worker.summary = Some("result-restore-789".into());
    worker.reported = true;

    let (note, shown) = report_note(&[worker], ContextDeliveryMode::Full).unwrap();

    assert_eq!(note.matches("result-restore-789").count(), 1);
    assert!(shown.is_empty());
}

#[test]
fn reported_workers_do_not_repeat_in_the_prompt_note() {
    let mut worker = WorkerRecord::new("child".into(), "req".into(), "task".into(), "goal".into());
    worker.status = WorkerStatus::Settled;
    worker.reported = true;

    let note = prompt_note(&[worker]);

    assert!(note.is_none());
}

#[test]
fn only_workers_actually_in_the_bounded_note_are_acknowledged() {
    let dirs = crate::config::DataDirs::from_bases(
        &std::env::temp_dir().join(format!("worker-delivery-{}", uuid::Uuid::new_v4())),
        &std::env::temp_dir(),
    );
    let store = crate::session::SessionStore::new(&dirs);
    let mut parent = super::tests::parent_record(Vec::new());
    for index in 0..PROMPT_WORKER_LIMIT + 2 {
        let mut worker = WorkerRecord::new(
            format!("child-{index}"),
            "req".into(),
            "task".into(),
            "goal".into(),
        );
        worker.id = format!("worker-{index}");
        worker.status = WorkerStatus::Settled;
        worker.updated_at = u64::try_from(index).unwrap();
        parent.workers.push(worker);
    }
    store.save(&parent).unwrap();

    let (note, shown) = prompt_note_for_turn(&store, &parent.meta.id).unwrap();

    assert_eq!(shown.len(), PROMPT_WORKER_LIMIT);
    assert!(shown.iter().all(|id| note.contains(id)));
    std::fs::remove_dir_all(dirs.app_data().parent().unwrap()).unwrap();
}
