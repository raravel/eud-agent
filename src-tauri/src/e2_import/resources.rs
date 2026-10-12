use std::collections::{BTreeMap, BTreeSet};

use sha2::{Digest, Sha256};

mod path;
mod plan;
use path::{
    case_fold, check_cancelled, fingerprint_package_inventory, hash_part, invalid, path_identity,
    read_bounded, resolve_reference, stored_path, validate_existing_file, ReferenceResolution,
};
use plan::{push_copy, relink_choices};
pub use plan::{
    E2InactiveResourceProvenance, E2PlannedCopy, E2ResourceCopyPlan, E2ResourceResolution,
    E2ResourceResolveError, E2ResourceResolveRequest,
};

use super::{
    parser::{MAX_RESOURCES, MAX_TEXT_BYTES},
    E2Activation, E2Document, E2Feature, E2Issue, E2IssueCode, E2IssueSeverity, E2RelinkChoice,
    E2RelinkNeed, ProjectResource, ProjectResourceConsumer, ProjectResourceKind,
    ProjectResourceProvenance,
};

/// Resolve and hash an E2 project's map and active resources without writing a destination.
pub fn resolve_copy_plan(
    request: E2ResourceResolveRequest<'_>,
    is_cancelled: impl Fn() -> bool,
) -> Result<E2ResourceResolution, E2ResourceResolveError> {
    check_cancelled(&is_cancelled)?;
    let source_path = validate_existing_file(request.source_path, None)?;
    let source_bytes = read_bounded(
        &source_path,
        u64::try_from(MAX_TEXT_BYTES).map_err(|_| invalid("invalid text byte limit", None))?,
        &is_cancelled,
    )?;
    let package_root = source_path
        .parent()
        .ok_or_else(|| invalid("E2 source has no parent directory", Some(&source_path)))?;

    let choices = relink_choices(request.relinks)?;
    let mut used_choices = BTreeSet::new();
    let mut base_hasher = Sha256::new();
    base_hasher.update(b"eud-agent/e2-resource-resolution/v1\0");
    hash_part(&mut base_hasher, source_path.to_string_lossy().as_bytes());
    hash_part(&mut base_hasher, &source_bytes);
    hash_part(
        &mut base_hasher,
        request.document.settings.input_map.as_bytes(),
    );
    hash_part(
        &mut base_hasher,
        request.document.settings.output_map.as_bytes(),
    );
    for reference in &request.document.resources {
        hash_part(&mut base_hasher, reference.id.as_bytes());
        hash_part(&mut base_hasher, reference.source_path.as_bytes());
    }
    fingerprint_package_inventory(
        package_root,
        request.document.source_kind,
        &mut base_hasher,
        &is_cancelled,
    )?;

    let output_identity = path_identity(&stored_path(
        package_root,
        &request.document.settings.output_map,
    )?)?;
    let mut aggregate_bytes = 0_u64;
    let mut copies = Vec::new();
    let mut inactive = Vec::new();
    let mut needs = Vec::new();
    let mut destination_keys = BTreeSet::new();
    let mut full_hasher = base_hasher.clone();

    let map_need_id = "input-map";
    match resolve_reference(
        package_root,
        request.document.source_kind,
        map_need_id,
        &request.document.settings.input_map,
        ProjectResourceKind::Map,
        choices.get(map_need_id).copied(),
        Some(&output_identity),
        &mut base_hasher,
        &mut full_hasher,
        &mut aggregate_bytes,
        &is_cancelled,
    )? {
        ReferenceResolution::Selected(selected) => {
            if choices.contains_key(map_need_id) {
                used_choices.insert(map_need_id.to_string());
            }
            push_copy(
                &mut copies,
                &mut destination_keys,
                "ProjectSET:InputMap",
                E2Feature::ProjectMap,
                ProjectResourceKind::Map,
                &request.document.settings.input_map,
                selected,
            )?;
        }
        ReferenceResolution::Need(need) => needs.push(need),
    }

    let activations = request
        .document
        .features
        .iter()
        .map(|state| (state.feature, state.activation))
        .collect::<BTreeMap<_, _>>();
    let mut ids = BTreeSet::new();
    for reference in &request.document.resources {
        check_cancelled(&is_cancelled)?;
        if reference.kind == ProjectResourceKind::Map {
            continue;
        }
        if !ids.insert(case_fold(&reference.id)) {
            return Err(invalid("resource ids collide when case is ignored", None));
        }
        let active = activations
            .get(&reference.feature)
            .copied()
            .unwrap_or(E2Activation::Inactive)
            == E2Activation::Active;
        if !active {
            inactive.push(E2InactiveResourceProvenance {
                id: reference.id.clone(),
                feature: reference.feature,
                kind: reference.kind,
                original_path: reference.source_path.clone(),
            });
            continue;
        }
        if reference.feature == E2Feature::Grp {
            return Err(E2ResourceResolveError::Fatal(E2Issue {
                code: E2IssueCode::ActiveGrpUnsupported,
                severity: E2IssueSeverity::Fatal,
                message: "active GRP resources are not supported".to_string(),
                path: Some(reference.source_path.clone()),
                feature: Some(E2Feature::Grp),
                need_id: None,
            }));
        }
        let need_id = format!("resource:{}", reference.id);
        match resolve_reference(
            package_root,
            request.document.source_kind,
            &need_id,
            &reference.source_path,
            reference.kind,
            choices.get(need_id.as_str()).copied(),
            None,
            &mut base_hasher,
            &mut full_hasher,
            &mut aggregate_bytes,
            &is_cancelled,
        )? {
            ReferenceResolution::Selected(selected) => {
                if choices.contains_key(need_id.as_str()) {
                    used_choices.insert(need_id);
                }
                push_copy(
                    &mut copies,
                    &mut destination_keys,
                    &reference.id,
                    reference.feature,
                    reference.kind,
                    &reference.source_path,
                    selected,
                )?;
            }
            ReferenceResolution::Need(need) => needs.push(need),
        }
    }
    if copies.len() > MAX_RESOURCES {
        return Err(invalid("resource count exceeds 4,096", None));
    }
    if used_choices.len() != choices.len() {
        return Err(invalid("relink choices contain an unknown need id", None));
    }

    let base_fingerprint = format!("{:x}", base_hasher.finalize());
    let full_fingerprint = format!("{:x}", full_hasher.finalize());
    if let Some(expected) = request.expected_fingerprint {
        if expected != base_fingerprint && expected != full_fingerprint {
            return Err(E2ResourceResolveError::Fatal(E2Issue {
                code: E2IssueCode::SourceChanged,
                severity: E2IssueSeverity::Fatal,
                message: "E2 source, package inventory, or selected relink changed".to_string(),
                path: Some(source_path.to_string_lossy().into_owned()),
                feature: None,
                need_id: None,
            }));
        }
    }
    if needs.is_empty() {
        copies.sort_by(|left, right| left.resource.id.cmp(&right.resource.id));
        inactive.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(E2ResourceResolution::Ready(E2ResourceCopyPlan {
            fingerprint: full_fingerprint,
            copies,
            inactive,
        }))
    } else {
        needs.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(E2ResourceResolution::NeedsRelink {
            fingerprint: full_fingerprint,
            needs,
        })
    }
}

#[cfg(test)]
#[path = "resources/tests.rs"]
mod resource_tests;

#[cfg(test)]
#[test]
fn resolution_and_inventory_matrix() {
    resource_tests::resolution_and_inventory_matrix_scenario();
}

#[cfg(test)]
#[test]
fn relink_and_confinement_failures() {
    resource_tests::relink_and_confinement_failures_scenario();
}
