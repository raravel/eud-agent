use std::path::Path;

use crate::bootstrap::process_tree::ProcessCancellation;
use crate::config::DataDirs;
use crate::native_project::NativeProject;

use super::{
    E2ImportPhase, E2ImportRequest, E2ImportResult, E2Issue, E2IssueCode, E2IssueSeverity,
    E2PreflightRequest, E2PreflightResult,
};

mod native_projection;
mod player_scope;
mod preflight;
mod publication;
#[cfg(test)]
#[path = "transaction/publication_race_scenario.rs"]
mod publication_race_scenario;
mod staging;

pub fn preflight(request: &E2PreflightRequest) -> E2PreflightResult {
    preflight::inspect(request)
}

pub(crate) fn import(
    import_id: &str,
    request: &E2ImportRequest,
    dirs: &DataDirs,
    compat_root: &Path,
    cancellation: &ProcessCancellation,
    mut emit: impl FnMut(&E2ImportResult),
    activate: impl FnOnce(&NativeProject) -> Result<(), String>,
) -> E2ImportResult {
    let progress = |phase, completed, message: &str| E2ImportResult::Progress {
        import_id: import_id.to_string(),
        fingerprint: request.fingerprint.clone(),
        phase,
        completed,
        total: 8,
        message: message.to_string(),
    };
    let mut report = |phase, completed, message| {
        emit(&progress(phase, completed, message));
        !cancellation.is_cancelled()
    };
    if !report(E2ImportPhase::Preflight, 0, "Rechecking E2 package") {
        return cancelled(import_id, &request.fingerprint);
    }
    let prepared = match preflight::prepare_import(request, cancellation) {
        Ok(value) => value,
        Err(result) => return bind_failure(result, import_id, &request.fingerprint),
    };
    if !report(E2ImportPhase::Resolving, 1, "Resolved immutable inputs") {
        return cancelled(import_id, &request.fingerprint);
    }
    let projection = match native_projection::compile(&prepared, dirs, compat_root, cancellation) {
        Ok(value) => value,
        Err(_) if cancellation.is_cancelled() => return cancelled(import_id, &request.fingerprint),
        Err(issue) => return fatal(import_id, &request.fingerprint, issue),
    };
    if !report(
        E2ImportPhase::Compiling,
        2,
        "Compiled editable native sources",
    ) {
        return cancelled(import_id, &request.fingerprint);
    }
    if !report(
        E2ImportPhase::CollectingResources,
        3,
        "Collected owned resources",
    ) {
        return cancelled(import_id, &request.fingerprint);
    }
    let staged = match staging::materialize(
        Path::new(&request.destination_path),
        import_id,
        projection,
        compat_root,
        cancellation,
    ) {
        Ok(value) => value,
        Err(_) if cancellation.is_cancelled() => return cancelled(import_id, &request.fingerprint),
        Err(error) => return fatal_message(import_id, &request.fingerprint, error),
    };
    if !report(
        E2ImportPhase::Materializing,
        4,
        "Materialized confined stage",
    ) {
        return cancelled(import_id, &request.fingerprint);
    }
    if !report(E2ImportPhase::Validating, 5, "Validated staged reopen") {
        return cancelled(import_id, &request.fingerprint);
    }
    let mut receipt = match publication::publish(
        &staged.root,
        Path::new(&request.destination_path),
        cancellation,
    ) {
        Ok(value) => value,
        Err(_) if cancellation.is_cancelled() => return cancelled(import_id, &request.fingerprint),
        Err(error) => return fatal_message(import_id, &request.fingerprint, error),
    };
    if !report(
        E2ImportPhase::Publishing,
        6,
        "Published without replacement",
    ) {
        return rollback_cancelled(receipt, import_id, &request.fingerprint);
    }
    let published = match staging::validate_published(Path::new(&request.destination_path)) {
        Ok(value) => value,
        Err(error) => return rollback_failure(receipt, import_id, &request.fingerprint, error),
    };
    if !report(E2ImportPhase::Opening, 7, "Reopened published project") {
        return rollback_cancelled(receipt, import_id, &request.fingerprint);
    }
    if let Err(error) = activate(&published) {
        return rollback_failure(receipt, import_id, &request.fingerprint, error);
    }
    receipt.commit();
    let completed = E2ImportResult::Completed {
        import_id: import_id.to_string(),
        fingerprint: request.fingerprint.clone(),
        project_opened: true,
        project_path: published.root().to_string_lossy().into_owned(),
    };
    emit(&progress(
        E2ImportPhase::Completed,
        8,
        "E2 import completed",
    ));
    completed
}

fn cancelled(import_id: &str, fingerprint: &str) -> E2ImportResult {
    E2ImportResult::Cancelled {
        import_id: import_id.to_string(),
        fingerprint: fingerprint.to_string(),
    }
}
fn fatal(import_id: &str, fingerprint: &str, issue: E2Issue) -> E2ImportResult {
    E2ImportResult::Fatal {
        import_id: Some(import_id.to_string()),
        fingerprint: Some(fingerprint.to_string()),
        issues: vec![issue],
    }
}
fn fatal_message(import_id: &str, fingerprint: &str, message: String) -> E2ImportResult {
    fatal(
        import_id,
        fingerprint,
        issue(E2IssueCode::MalformedInput, message),
    )
}
fn bind_failure(result: E2ImportResult, import_id: &str, fingerprint: &str) -> E2ImportResult {
    match result {
        E2ImportResult::Cancelled { .. } => cancelled(import_id, fingerprint),
        E2ImportResult::Fatal { issues, .. } => E2ImportResult::Fatal {
            import_id: Some(import_id.to_string()),
            fingerprint: Some(fingerprint.to_string()),
            issues,
        },
        other => other,
    }
}
fn rollback_failure(
    mut receipt: publication::PublicationReceipt,
    import_id: &str,
    fingerprint: &str,
    error: String,
) -> E2ImportResult {
    let message = receipt.rollback().map_or(error.clone(), |cleanup| {
        format!("{error}; cleanup failed: {cleanup}")
    });
    fatal_message(import_id, fingerprint, message)
}
fn rollback_cancelled(
    mut receipt: publication::PublicationReceipt,
    import_id: &str,
    fingerprint: &str,
) -> E2ImportResult {
    match receipt.rollback() {
        None => cancelled(import_id, fingerprint),
        Some(cleanup) => fatal_message(
            import_id,
            fingerprint,
            format!("import cancelled; cleanup failed: {cleanup}"),
        ),
    }
}
fn issue(code: E2IssueCode, message: impl Into<String>) -> E2Issue {
    E2Issue {
        code,
        severity: E2IssueSeverity::Fatal,
        message: message.into(),
        path: None,
        feature: None,
        need_id: None,
    }
}

#[cfg(test)]
#[path = "transaction/failure_scenario.rs"]
mod failure_scenario;
#[cfg(all(test, windows))]
mod force_owned_map_scenario;
#[cfg(all(test, windows))]
mod player_scope_test_support;
#[cfg(all(test, windows))]
mod playerloop_invalid_scope_scenario;
#[cfg(test)]
#[path = "transaction/scenario_fixture.rs"]
mod scenario_fixture;
#[cfg(test)]
#[path = "transaction/success_scenario.rs"]
mod success_scenario;
#[cfg(test)]
#[test]
fn successful_state_sequence() {
    success_scenario::run();
}
#[cfg(test)]
#[test]
fn failure_recovery_matrix() {
    failure_scenario::run();
}
