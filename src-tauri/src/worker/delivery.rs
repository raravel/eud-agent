use super::{WorkerRecord, PROMPT_WORKER_LIMIT};
use crate::context_state::ContextDeliveryMode;

pub(crate) fn report_note(
    workers: &[WorkerRecord],
    mode: ContextDeliveryMode,
) -> Option<(String, Vec<String>)> {
    let mut selected = workers
        .iter()
        .filter(|worker| match mode {
            ContextDeliveryMode::Full => true,
            ContextDeliveryMode::Delta => worker.status.is_active() || !worker.reported,
        })
        .collect::<Vec<_>>();
    selected.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then(b.id.cmp(&a.id)));
    selected.truncate(PROMPT_WORKER_LIMIT);
    if selected.is_empty() {
        return None;
    }
    let shown = selected
        .iter()
        .filter(|worker| !worker.status.is_active() && !worker.reported)
        .map(|worker| worker.id.clone())
        .collect();
    let lines = selected
        .iter()
        .map(|worker| worker.prompt_line())
        .collect::<Vec<_>>()
        .join("\n");
    let guidance = if workers.iter().any(|worker| worker.status.is_active()) {
        "Queued or running workers continue this conversation on their own when they settle: do not poll worker_status or wait for them, and do not edit the files they own meanwhile. Review settled workers' changes with worker_status before building on them, and revert a wrong result with worker_revert."
    } else {
        "Review settled workers' changes with worker_status before building on them, and revert a wrong result with worker_revert. Failed, cancelled, or interrupted workers may have committed part of their work."
    };
    Some((format!("[workers]\n{lines}\n{guidance}"), shown))
}

#[cfg(test)]
pub fn prompt_note(workers: &[WorkerRecord]) -> Option<String> {
    report_note(workers, ContextDeliveryMode::Delta).map(|(note, _)| note)
}

#[cfg(test)]
pub fn prompt_note_for_turn(
    sessions: &crate::session::SessionStore,
    parent_id: &str,
) -> Option<(String, Vec<String>)> {
    match sessions.load(parent_id) {
        Ok(record) => report_note(&record.workers, ContextDeliveryMode::Delta),
        Err(error) => {
            eprintln!("eud-agent: worker reports could not be loaded: {error}");
            None
        }
    }
}
