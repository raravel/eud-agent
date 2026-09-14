use super::{
    corrupt_receipt_audit_directory, fs, io, persist_receipt, read_receipt, receipt_directory,
    receipt_path, remove_receipt, validate_native_id, NativeRunReceiptState, ProviderId,
    RunIdentity, UnresolvedNativeRun,
};

pub fn unresolved_native_runs(
    journal_dir: &std::path::Path,
    session_id: &str,
) -> Result<Vec<UnresolvedNativeRun>, String> {
    let directory = receipt_directory(journal_dir, session_id);
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("native run receipts cannot be listed: {error}")),
    };
    let mut unresolved = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|error| format!("native run receipt entry cannot be read: {error}"))?
            .path();
        let receipt = read_receipt(&path)?;
        if receipt.session_id != session_id {
            return Err("native run receipt session identity mismatch".to_string());
        }
        let Some(state) = receipt.lifecycle else {
            continue;
        };
        if state == NativeRunReceiptState::Cleared {
            continue;
        }
        let provider = receipt
            .provider
            .ok_or_else(|| "native run receipt has no provider".to_string())?;
        unresolved.push(UnresolvedNativeRun {
            provider,
            prior_native_id: receipt.prior_native_id,
            candidate_native_id: receipt.candidate_native_id,
            state,
            identity: RunIdentity {
                session_id: receipt.session_id,
                run_id: crate::provider_runtime::RunId::new(receipt.run_id),
                request_id: receipt.request_id,
                session_kind: receipt.session_kind,
                cancellation_generation: receipt.cancellation_generation,
            },
        });
    }
    unresolved.sort_by_key(|run| {
        (
            run.identity.cancellation_generation,
            run.identity.run_id.get(),
        )
    });
    Ok(unresolved)
}

pub fn clear_native_run_recovery(
    journal_dir: &std::path::Path,
    session_id: &str,
    provider: ProviderId,
    prior_native_id: Option<&str>,
) -> Result<(), String> {
    validate_native_id(prior_native_id)?;
    let directory = receipt_directory(journal_dir, session_id);
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("native run receipts cannot be listed: {error}")),
    };
    for entry in entries {
        let path = entry
            .map_err(|error| format!("native run receipt entry cannot be read: {error}"))?
            .path();
        if !is_owned_receipt_path(&path)? {
            continue;
        }
        let mut receipt = match read_receipt(&path) {
            Ok(receipt) => receipt,
            Err(_) => {
                archive_corrupt_receipt(journal_dir, session_id, &path)?;
                continue;
            }
        };
        if receipt.session_id != session_id {
            return Err("native run receipt session identity mismatch".to_string());
        }
        if receipt.provider == Some(provider)
            && receipt.prior_native_id.as_deref() == prior_native_id
            && receipt.lifecycle != Some(NativeRunReceiptState::Cleared)
        {
            receipt.lifecycle = Some(NativeRunReceiptState::Cleared);
            persist_receipt(&path, &receipt)?;
        }
    }
    Ok(())
}

fn is_owned_receipt_path(path: &std::path::Path) -> Result<bool, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("native run receipt metadata cannot be read: {error}"))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Ok(false);
    }
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return Ok(false);
    };
    let Some(stem) = name.strip_suffix(".json") else {
        return Ok(false);
    };
    let mut segments = stem.split('-');
    let valid = segments
        .next()
        .is_some_and(|run| run.parse::<u64>().is_ok())
        && segments
            .next()
            .is_some_and(|generation| generation.parse::<u64>().is_ok())
        && segments.next().is_some_and(|request| {
            request.len() == 16
                && request
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        && segments.next().is_none();
    Ok(valid)
}

fn archive_corrupt_receipt(
    journal_dir: &std::path::Path,
    session_id: &str,
    path: &std::path::Path,
) -> Result<(), String> {
    let directory = corrupt_receipt_audit_directory(journal_dir, session_id);
    fs::create_dir_all(&directory).map_err(|error| {
        format!("native run receipt audit directory cannot be created: {error}")
    })?;
    let name = path
        .file_name()
        .ok_or_else(|| "native run receipt has no file name".to_string())?
        .to_string_lossy();
    let audit_path = directory.join(format!("{name}.corrupt-{}", uuid::Uuid::new_v4()));
    fs::rename(path, audit_path)
        .map_err(|error| format!("corrupt native run receipt cannot be archived: {error}"))
}

pub fn acknowledge_native_run(
    journal_dir: &std::path::Path,
    identity: &RunIdentity,
) -> Result<(), String> {
    remove_receipt(&receipt_path(journal_dir, identity))
}
