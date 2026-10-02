use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::bootstrap::process_tree::ProcessCancellation;

struct OwnedFile {
    path: PathBuf,
    sha256: String,
}

pub(super) struct PublicationReceipt {
    files: Vec<OwnedFile>,
    directories: Vec<PathBuf>,
    committed: bool,
}

impl PublicationReceipt {
    pub fn commit(&mut self) {
        self.committed = true;
    }

    pub fn rollback(&mut self) -> Option<String> {
        let mut errors = Vec::new();
        for owned in self.files.iter().rev() {
            match sha256_file(&owned.path) {
                Ok(hash) if hash == owned.sha256 => {
                    if let Err(error) = fs::remove_file(&owned.path) {
                        errors.push(format!("{}: {error}", owned.path.display()));
                    }
                }
                Ok(_) => errors.push(format!(
                    "owned file changed and was preserved: {}",
                    owned.path.display()
                )),
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    errors.push(format!("{}: {error}", owned.path.display()))
                }
                Err(_) => {}
            }
        }
        for directory in self.directories.iter().rev() {
            if let Err(error) = fs::remove_dir(directory) {
                if error.kind() != std::io::ErrorKind::NotFound
                    && error.kind() != std::io::ErrorKind::DirectoryNotEmpty
                {
                    errors.push(format!("{}: {error}", directory.display()));
                }
            }
        }
        self.files.clear();
        self.directories.clear();
        (!errors.is_empty()).then(|| errors.join("; "))
    }
}

impl Drop for PublicationReceipt {
    fn drop(&mut self) {
        if !self.committed {
            if let Some(error) = self.rollback() {
                eprintln!("eud-agent: E2 publication rollback failed: {error}");
            }
        }
    }
}

pub(super) fn publish(
    stage: &Path,
    destination: &Path,
    cancellation: &ProcessCancellation,
) -> Result<PublicationReceipt, String> {
    ensure_empty(destination)?;
    let mut relative_files = Vec::new();
    collect(stage, stage, &mut relative_files)?;
    relative_files.sort();
    let mut receipt = PublicationReceipt {
        files: Vec::new(),
        directories: Vec::new(),
        committed: false,
    };
    let result = (|| {
        for relative in relative_files {
            check(cancellation)?;
            let source = stage.join(&relative);
            let target = destination.join(&relative);
            create_parents(
                destination,
                target
                    .parent()
                    .ok_or_else(|| "published file has no parent".to_string())?,
                &mut receipt.directories,
            )?;
            let mut input = fs::File::open(&source).map_err(|error| error.to_string())?;
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)
                .map_err(|error| {
                    format!("publication collision '{}': {error}", target.display())
                })?;
            let mut hasher = Sha256::new();
            let mut buffer = [0_u8; 64 * 1024];
            let copied = (|| {
                loop {
                    check(cancellation)?;
                    let count = input.read(&mut buffer).map_err(|error| error.to_string())?;
                    if count == 0 {
                        break;
                    }
                    output
                        .write_all(&buffer[..count])
                        .map_err(|error| error.to_string())?;
                    hasher.update(&buffer[..count]);
                }
                output.sync_all().map_err(|error| error.to_string())?;
                Ok(format!("{:x}", hasher.finalize()))
            })();
            drop(output);
            match copied {
                Ok(sha256) => receipt.files.push(OwnedFile {
                    path: target,
                    sha256,
                }),
                Err(error) => {
                    match sha256_file(&target) {
                        Ok(sha256) => receipt.files.push(OwnedFile {
                            path: target,
                            sha256,
                        }),
                        Err(hash_error) => {
                            return match fs::remove_file(&target) {
                                Ok(()) => Err(error),
                                Err(cleanup) => Err(format!(
                                    "{error}; cannot fingerprint partial publication: {hash_error}; cleanup failed: {cleanup}"
                                )),
                            };
                        }
                    }
                    return Err(error);
                }
            }
        }
        Ok(())
    })();
    match result {
        Ok(()) => Ok(receipt),
        Err(primary) => Err(receipt.rollback().map_or(primary.clone(), |cleanup| {
            format!("{primary}; cleanup failed: {cleanup}")
        })),
    }
}

fn check(cancellation: &ProcessCancellation) -> Result<(), String> {
    cancellation
        .is_cancelled()
        .then_some("E2 import was cancelled".to_string())
        .map_or(Ok(()), Err)
}

fn ensure_empty(destination: &Path) -> Result<(), String> {
    if fs::read_dir(destination)
        .map_err(|error| error.to_string())?
        .next()
        .is_some()
    {
        return Err("destination changed after preflight".to_string());
    }
    Ok(())
}

fn collect(root: &Path, current: &Path, output: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(current).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let file_type = entry.file_type().map_err(|error| error.to_string())?;
        if file_type.is_dir() {
            collect(root, &entry.path(), output)?;
        } else if file_type.is_file() {
            output.push(
                entry
                    .path()
                    .strip_prefix(root)
                    .map_err(|error| error.to_string())?
                    .to_path_buf(),
            );
        } else {
            return Err(format!(
                "staged entry is not a regular file: {}",
                entry.path().display()
            ));
        }
    }
    Ok(())
}

fn create_parents(root: &Path, parent: &Path, receipt: &mut Vec<PathBuf>) -> Result<(), String> {
    if parent == root {
        return Ok(());
    }
    let relative = parent
        .strip_prefix(root)
        .map_err(|_| "publication parent escapes destination".to_string())?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        match fs::create_dir(&current) {
            Ok(()) => receipt.push(current.clone()),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && current.is_dir() => {
            }
            Err(error) => {
                return Err(format!(
                    "cannot create publication directory '{}': {error}",
                    current.display()
                ))
            }
        }
    }
    Ok(())
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    let bytes = fs::read(path)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
