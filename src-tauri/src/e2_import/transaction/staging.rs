use std::fs;
use std::path::{Path, PathBuf};

use super::native_projection::NativeProjection;
use crate::bootstrap::process_tree::ProcessCancellation;
use crate::native_project::NativeProject;

pub(super) struct StagedProject {
    pub root: PathBuf,
}

impl Drop for StagedProject {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.root) {
            if error.kind() != std::io::ErrorKind::NotFound {
                eprintln!("eud-agent: E2 stage cleanup failed: {error}");
            }
        }
    }
}

pub(super) fn materialize(
    destination: &Path,
    import_id: &str,
    projection: NativeProjection,
    compat_root: &Path,
    cancellation: &ProcessCancellation,
) -> Result<StagedProject, String> {
    let parent = destination
        .parent()
        .ok_or_else(|| "destination has no parent".to_string())?;
    let name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "destination name is not Unicode".to_string())?;
    let safe_id = import_id
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>();
    let root = parent.join(format!(".{name}.e2-stage-{safe_id}"));
    if fs::symlink_metadata(&root).is_ok() {
        return Err(format!("E2 stage already exists: {}", root.display()));
    }
    fs::create_dir(&root).map_err(|error| format!("cannot create E2 stage: {error}"))?;
    let result = (|| {
        for copy in &projection.copies {
            check(cancellation)?;
            write_new(&root, &copy.resource.path, &copy.bytes)?;
        }
        for entrypoint in &projection.manifest.python_entrypoints {
            check(cancellation)?;
            let bytes = projection.files.get(entrypoint).ok_or_else(|| {
                format!("Python entrypoint has no projected source bytes: {entrypoint}")
            })?;
            write_new(&root, entrypoint, bytes)?;
        }
        let mut project = NativeProject::create(&root, projection.manifest)?;
        project.replace_dat_state(projection.dat)?;
        for (path, bytes) in projection.files {
            check(cancellation)?;
            let text_source = path.ends_with(".eps") || path.ends_with(".py");
            if text_source {
                let text = String::from_utf8(bytes)
                    .map_err(|_| format!("generated source is not UTF-8: {path}"))?;
                project.write_source(&path, &text)?;
            } else {
                write_new(&root, &path, &bytes)?;
            }
        }
        validate(&root, compat_root, cancellation)?;
        Ok(StagedProject { root: root.clone() })
    })();
    match result {
        Ok(staged) => Ok(staged),
        Err(error) => match fs::remove_dir_all(&root) {
            Ok(()) => Err(error),
            Err(cleanup) => Err(format!("{error}; stage cleanup failed: {cleanup}")),
        },
    }
}

pub(super) fn validate_published(root: &Path) -> Result<NativeProject, String> {
    let project = NativeProject::open(root)?;
    project.resource_snapshot()?;
    project.revision()?;
    NativeProject::open(root)
}

fn validate(
    root: &Path,
    compat_root: &Path,
    cancellation: &ProcessCancellation,
) -> Result<(), String> {
    let project = NativeProject::open(root)?;
    project.resource_snapshot()?;
    project.revision()?;
    check(cancellation)?;
    crate::native_build::generate_native_build(&project, compat_root)?;
    check(cancellation)?;
    let build = root.join("build");
    fs::remove_dir_all(&build)
        .map_err(|error| format!("cannot remove staged native build: {error}"))?;
    NativeProject::open(root)?;
    Ok(())
}

fn write_new(root: &Path, relative: &str, bytes: &[u8]) -> Result<(), String> {
    let normalized = relative.replace('/', std::path::MAIN_SEPARATOR_STR);
    let relative_path = Path::new(&normalized);
    if relative_path.is_absolute()
        || relative_path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(format!("stage path escapes root: {relative}"));
    }
    let path = root.join(relative_path);
    let parent = path
        .parent()
        .ok_or_else(|| format!("stage path has no parent: {relative}"))?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| format!("cannot create staged '{relative}': {error}"))?;
    use std::io::Write;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|error| error.to_string())
}

fn check(cancellation: &ProcessCancellation) -> Result<(), String> {
    if cancellation.is_cancelled() {
        Err("E2 import was cancelled".to_string())
    } else {
        Ok(())
    }
}
