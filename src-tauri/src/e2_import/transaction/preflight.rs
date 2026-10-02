use std::fs;
use std::path::Path;

use super::super::parser::parse_e2_path;
use super::super::resources::{
    resolve_copy_plan, E2ResourceCopyPlan, E2ResourceResolution, E2ResourceResolveError,
    E2ResourceResolveRequest,
};
use super::super::{
    E2Document, E2ImportRequest, E2ImportResult, E2IssueSeverity, E2PreflightRequest,
    E2PreflightResult,
};
use crate::bootstrap::process_tree::ProcessCancellation;

pub(super) struct PreparedImport {
    pub document: E2Document,
    pub resources: E2ResourceCopyPlan,
    pub source_name: String,
}

pub(super) fn inspect(request: &E2PreflightRequest) -> E2PreflightResult {
    if let Err(message) = request.validate() {
        return fatal(
            None,
            super::issue(super::E2IssueCode::MalformedInput, message),
        );
    }
    let cancellation = ProcessCancellation::default();
    match prepare(
        Path::new(&request.source_path),
        Path::new(&request.destination_path),
        &request.relinks,
        None,
        &cancellation,
    ) {
        Ok(value) => E2PreflightResult::Ready {
            fingerprint: value.resources.fingerprint,
            source_kind: value.document.source_kind,
            features: value
                .document
                .features
                .iter()
                .filter(|state| state.activation == super::super::E2Activation::Active)
                .map(|state| state.feature)
                .collect(),
            warnings: Vec::new(),
        },
        Err(PrepareError::NeedsRelink { fingerprint, needs }) => {
            E2PreflightResult::NeedsRelink { fingerprint, needs }
        }
        Err(PrepareError::Fatal { fingerprint, issue }) => fatal(fingerprint, issue),
        Err(PrepareError::Cancelled) => fatal(
            None,
            super::issue(super::E2IssueCode::SourceChanged, "preflight was cancelled"),
        ),
    }
}

pub(super) fn prepare_import(
    request: &E2ImportRequest,
    cancellation: &ProcessCancellation,
) -> Result<PreparedImport, E2ImportResult> {
    if let Err(message) = request.validate() {
        return Err(E2ImportResult::Fatal {
            import_id: None,
            fingerprint: Some(request.fingerprint.clone()),
            issues: vec![super::issue(super::E2IssueCode::MalformedInput, message)],
        });
    }
    prepare(
        Path::new(&request.source_path),
        Path::new(&request.destination_path),
        &request.relinks,
        Some(&request.fingerprint),
        cancellation,
    )
    .map_err(|error| match error {
        PrepareError::Cancelled => E2ImportResult::Cancelled {
            import_id: String::new(),
            fingerprint: request.fingerprint.clone(),
        },
        PrepareError::Fatal { issue, .. } => E2ImportResult::Fatal {
            import_id: None,
            fingerprint: Some(request.fingerprint.clone()),
            issues: vec![issue],
        },
        PrepareError::NeedsRelink { .. } => E2ImportResult::Fatal {
            import_id: None,
            fingerprint: Some(request.fingerprint.clone()),
            issues: vec![super::issue(
                super::E2IssueCode::SourceChanged,
                "E2 relink resolution changed after preflight",
            )],
        },
    })
}

enum PrepareError {
    Cancelled,
    NeedsRelink {
        fingerprint: String,
        needs: Vec<super::super::E2RelinkNeed>,
    },
    Fatal {
        fingerprint: Option<String>,
        issue: super::E2Issue,
    },
}

fn prepare(
    source: &Path,
    destination: &Path,
    relinks: &[super::super::E2RelinkChoice],
    expected: Option<&str>,
    cancellation: &ProcessCancellation,
) -> Result<PreparedImport, PrepareError> {
    if cancellation.is_cancelled() {
        return Err(PrepareError::Cancelled);
    }
    let parsed = parse_e2_path(source).map_err(|issues| PrepareError::Fatal {
        fingerprint: None,
        issue: issues.into_iter().next().unwrap_or_else(|| {
            super::issue(
                super::E2IssueCode::MalformedInput,
                "E2 parser returned no issue",
            )
        }),
    })?;
    if let Some(issue) = parsed
        .issues
        .iter()
        .find(|issue| issue.severity == E2IssueSeverity::Fatal)
    {
        return Err(PrepareError::Fatal {
            fingerprint: None,
            issue: issue.clone(),
        });
    }
    let resolution = resolve_copy_plan(
        E2ResourceResolveRequest {
            source_path: source,
            document: &parsed.document,
            relinks,
            expected_fingerprint: expected,
        },
        || cancellation.is_cancelled(),
    )
    .map_err(|error| match error {
        E2ResourceResolveError::Cancelled => PrepareError::Cancelled,
        E2ResourceResolveError::Fatal(issue) => PrepareError::Fatal {
            fingerprint: expected.map(str::to_string),
            issue,
        },
    })?;
    match resolution {
        E2ResourceResolution::NeedsRelink { fingerprint, needs } => {
            Err(PrepareError::NeedsRelink { fingerprint, needs })
        }
        E2ResourceResolution::Ready(resources) => {
            ensure_empty_destination(destination).map_err(|issue| PrepareError::Fatal {
                fingerprint: Some(resources.fingerprint.clone()),
                issue,
            })?;
            let source_name = source
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("import.e2s")
                .to_string();
            Ok(PreparedImport {
                document: parsed.document,
                resources,
                source_name,
            })
        }
    }
}

fn ensure_empty_destination(destination: &Path) -> Result<(), super::E2Issue> {
    let metadata = fs::symlink_metadata(destination).map_err(|error| {
        super::issue(
            super::E2IssueCode::MalformedInput,
            format!("cannot inspect destination: {error}"),
        )
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(super::issue(
            super::E2IssueCode::MalformedInput,
            "destination must be a regular directory",
        ));
    }
    if fs::read_dir(destination)
        .map_err(|error| {
            super::issue(
                super::E2IssueCode::MalformedInput,
                format!("cannot read destination: {error}"),
            )
        })?
        .next()
        .is_some()
    {
        return Err(super::issue(
            super::E2IssueCode::MalformedInput,
            "destination must remain empty",
        ));
    }
    Ok(())
}

fn fatal(fingerprint: Option<String>, issue: super::E2Issue) -> E2PreflightResult {
    E2PreflightResult::Fatal {
        fingerprint,
        issues: vec![issue],
    }
}
