use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::super::{
    parser::{MAX_AGGREGATE_RESOURCE_BYTES, MAX_RESOURCE_BYTES, MAX_SOURCE_CANDIDATES},
    E2RelinkKind, E2RelinkNeed, E2SourceKind, ProjectResourceKind,
};
use super::E2ResourceResolveError;

mod confinement;
mod inventory;
pub(super) use confinement::{
    allowed_extensions, check_cancelled, ensure_no_links, invalid, io_issue, path_identity,
    read_bounded, read_directory, require_extension, stored_path, validate_existing_file,
    validate_path_text,
};
pub(super) use inventory::{case_fold, fingerprint_package_inventory, hash_part};

pub(super) struct SelectedFile {
    pub(super) display_path: String,
    pub(super) extension: String,
    pub(super) bytes: Vec<u8>,
    pub(super) sha256: String,
}

pub(super) enum ReferenceResolution {
    Selected(SelectedFile),
    Need(E2RelinkNeed),
}

#[expect(
    clippy::too_many_arguments,
    reason = "resolution carries explicit security budgets"
)]
pub(super) fn resolve_reference(
    package_root: &Path,
    source_kind: E2SourceKind,
    need_id: &str,
    stored: &str,
    kind: ProjectResourceKind,
    relink: Option<&str>,
    excluded_identity: Option<&str>,
    base_hasher: &mut Sha256,
    full_hasher: &mut Sha256,
    aggregate_bytes: &mut u64,
    is_cancelled: &impl Fn() -> bool,
) -> Result<ReferenceResolution, E2ResourceResolveError> {
    validate_path_text(stored)?;
    let stored_candidate = stored_path(package_root, stored)?;
    if stored_candidate.is_file() {
        let candidate = load_candidate(
            &stored_candidate,
            excluded_identity,
            aggregate_bytes,
            is_cancelled,
        )?;
        if let Some(candidate) = candidate {
            require_extension(kind, &candidate, &stored_candidate)?;
            hash_candidate(base_hasher, &candidate);
            hash_candidate(full_hasher, &candidate);
            return Ok(ReferenceResolution::Selected(candidate));
        }
    }

    let normalized = stored.replace('\\', "/");
    let basename = Path::new(&normalized)
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| invalid("resource path has no UTF-8 file name", None))?;
    let mut candidates = Vec::new();
    let mut identities = BTreeSet::new();
    for root in candidate_roots(package_root, source_kind, kind) {
        for path in exact_named_files(&root, basename, is_cancelled)? {
            if candidates.len() >= MAX_SOURCE_CANDIDATES {
                return Err(invalid("source candidate count exceeds 128", Some(&root)));
            }
            if let Some(candidate) =
                load_candidate(&path, excluded_identity, aggregate_bytes, is_cancelled)?
            {
                require_extension(kind, &candidate, &path)?;
                let identity = path_identity(Path::new(&candidate.display_path))?;
                if identities.insert(identity) {
                    hash_candidate(base_hasher, &candidate);
                    hash_candidate(full_hasher, &candidate);
                    candidates.push(candidate);
                }
            }
        }
    }

    if let Some(relink) = relink {
        validate_path_text(relink)?;
        let selected_path = PathBuf::from(relink);
        if !selected_path.is_absolute() {
            return Err(invalid(
                "relink path must be absolute",
                Some(&selected_path),
            ));
        }
        let selected = load_candidate(
            &selected_path,
            excluded_identity,
            aggregate_bytes,
            is_cancelled,
        )?
        .ok_or_else(|| {
            invalid(
                "relink points at the excluded output map",
                Some(&selected_path),
            )
        })?;
        require_extension(kind, &selected, &selected_path)?;
        hash_candidate(full_hasher, &selected);
        return Ok(ReferenceResolution::Selected(selected));
    }

    if candidates.len() == 1 {
        let selected = candidates.pop().ok_or_else(|| {
            invalid(
                "candidate selection lost its only entry",
                Some(package_root),
            )
        })?;
        return Ok(ReferenceResolution::Selected(selected));
    }
    let candidate_paths = candidates
        .iter()
        .map(|candidate| candidate.display_path.clone())
        .collect::<Vec<_>>();
    let kind_value = if kind == ProjectResourceKind::Map {
        E2RelinkKind::InputMap
    } else {
        E2RelinkKind::Resource
    };
    Ok(ReferenceResolution::Need(E2RelinkNeed {
        id: need_id.to_string(),
        kind: kind_value,
        label: basename.to_string(),
        original_path: Some(stored.to_string()),
        candidates: candidate_paths,
        extensions: allowed_extensions(kind)
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
    }))
}

fn exact_named_files(
    root: &Path,
    basename: &str,
    is_cancelled: &impl Fn() -> bool,
) -> Result<Vec<PathBuf>, E2ResourceResolveError> {
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(io_issue("cannot inspect candidate directory", root, error)),
    };
    if crate::memory::is_untrusted_link(&metadata) || !metadata.is_dir() {
        return Err(invalid(
            "candidate root is not a safe directory",
            Some(root),
        ));
    }
    ensure_no_links(root)?;
    let mut matches = Vec::new();
    for path in read_directory(root)? {
        check_cancelled(is_cancelled)?;
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            return Err(invalid("candidate name must be UTF-8", Some(&path)));
        };
        if name.eq_ignore_ascii_case(basename) {
            matches.push(path);
        }
    }
    matches.sort_by_key(|path| case_fold(&path.to_string_lossy()));
    Ok(matches)
}

fn candidate_roots(
    package_root: &Path,
    source_kind: E2SourceKind,
    kind: ProjectResourceKind,
) -> Vec<PathBuf> {
    if source_kind == E2SourceKind::E2s {
        return vec![package_root.to_path_buf()];
    }
    let mut roots = match kind {
        ProjectResourceKind::Map => vec![package_root.join("Map")],
        ProjectResourceKind::Audio => {
            vec![package_root.join("Sound"), package_root.join("sound")]
        }
        ProjectResourceKind::Bin
        | ProjectResourceKind::Tile
        | ProjectResourceKind::Plugin
        | ProjectResourceKind::Dat
        | ProjectResourceKind::File => vec![package_root.join("Resource")],
    };
    roots.push(package_root.to_path_buf());
    roots
}

fn load_candidate(
    path: &Path,
    excluded_identity: Option<&str>,
    aggregate_bytes: &mut u64,
    is_cancelled: &impl Fn() -> bool,
) -> Result<Option<SelectedFile>, E2ResourceResolveError> {
    let canonical = validate_existing_file(path, None)?;
    let identity = path_identity(&canonical)?;
    if excluded_identity.is_some_and(|excluded| identity == excluded) {
        return Ok(None);
    }
    let metadata = fs::metadata(&canonical)
        .map_err(|error| io_issue("cannot inspect resource", &canonical, error))?;
    if metadata.len() > MAX_RESOURCE_BYTES {
        return Err(invalid("resource exceeds 64 MiB", Some(&canonical)));
    }
    *aggregate_bytes = aggregate_bytes
        .checked_add(metadata.len())
        .ok_or_else(|| invalid("aggregate resource size overflow", Some(&canonical)))?;
    if *aggregate_bytes > MAX_AGGREGATE_RESOURCE_BYTES {
        return Err(invalid("resources exceed 512 MiB", Some(&canonical)));
    }
    let bytes = read_bounded(&canonical, MAX_RESOURCE_BYTES, is_cancelled)?;
    let extension = canonical
        .extension()
        .and_then(|value| value.to_str())
        .ok_or_else(|| invalid("resource must have a UTF-8 extension", Some(&canonical)))?
        .to_ascii_lowercase();
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    Ok(Some(SelectedFile {
        display_path: canonical.to_string_lossy().into_owned(),
        extension,
        bytes,
        sha256,
    }))
}

fn hash_candidate(hasher: &mut Sha256, candidate: &SelectedFile) {
    hash_part(hasher, candidate.display_path.as_bytes());
    hash_part(hasher, candidate.sha256.as_bytes());
    hash_part(hasher, candidate.bytes.len().to_string().as_bytes());
}
