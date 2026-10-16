//! Delivering a project's events to that project's windows only.
//!
//! `Emitter::emit` broadcasts to every window, including one called on a
//! window object. Events that belong to a project go through [`emit_project`]
//! instead: one `emit_to` per window bound to the project, with the payload
//! carrying `projectId` so a window can also drop anything not its own.
//! Process-wide events (bootstrap, RAG warmup, provider progress, session
//! activity) stay broadcasts.

use serde::Serialize;
use tauri::{Emitter, Manager};

use crate::project_id::ProjectId;
use crate::project_registry::ProjectRegistry;
use crate::remote::hub::RemoteHub;

/// A payload tagged with the project it belongs to.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectScoped<T> {
    pub project_id: String,
    #[serde(flatten)]
    pub payload: T,
}

impl<T> ProjectScoped<T> {
    pub fn new(project: &ProjectId, payload: T) -> Self {
        Self {
            project_id: project.as_str().to_string(),
            payload,
        }
    }
}

/// A payload that carries only the project (an event that means "reload").
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSignal {
    pub project_id: String,
}

impl ProjectSignal {
    pub fn new(project: &ProjectId) -> Self {
        Self {
            project_id: project.as_str().to_string(),
        }
    }
}

/// Emit `event` to the windows bound to `project`, and to no other window.
/// Remote connections bound to the project have no window: they receive it
/// once through the remote hub instead.
pub fn emit_project<S: Serialize + Clone>(
    app: &tauri::AppHandle,
    project: &ProjectId,
    event: &str,
    payload: S,
) -> tauri::Result<()> {
    let registry = app.state::<ProjectRegistry>();
    let mut result = Ok(());
    for label in window_targets(registry.window_labels(project)) {
        if let Err(error) = app.emit_to(label.as_str(), event, payload.clone()) {
            result = Err(error);
        }
    }
    if let Some(hub) = app.try_state::<RemoteHub>() {
        hub.publish_project(project, event, &payload);
    }
    result
}

/// The labels among `labels` that are desktop windows.
fn window_targets(labels: Vec<String>) -> impl Iterator<Item = String> {
    labels
        .into_iter()
        .filter(|label| !crate::remote::caller::is_remote_label(label))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_labels_are_never_window_targets() {
        let labels = vec![
            "main".to_string(),
            "map-agent-abc".to_string(),
            "remote-dev1-0011".to_string(),
        ];
        assert_eq!(
            window_targets(labels).collect::<Vec<_>>(),
            ["main", "map-agent-abc"]
        );
    }
}
