use std::fs;
use std::path::Path;

use crate::bootstrap::process_tree::ProcessCancellation;
use crate::e2_import::{
    E2ImportRequest, E2ImportResult, E2IssueCode, E2PreflightRequest, E2PreflightResult,
};
use crate::native_build::{run_native_build, EuddraftLaunch};
use crate::native_project::NativeProject;

use super::package::{Package, TileMode};
use super::relocated_build::compat_root;

pub(super) fn assert_same_metadata_drift_fails(package: &Package) {
    import_without_build(package);
    fs::rename(&package.destination, &package.moved).unwrap();
    fs::rename(&package.root, &package.unavailable).unwrap();
    let project = NativeProject::open(&package.moved).unwrap();
    let resource = project
        .manifest()
        .e2_import
        .as_ref()
        .unwrap()
        .resources
        .iter()
        .find(|resource| resource.id == "FileManagerSET:iscript")
        .unwrap();
    let path = project.root().join(&resource.path);
    let before = fs::metadata(&path).unwrap();
    let stamp = file_times(&path);
    let mut bytes = fs::read(&path).unwrap();
    bytes[0] ^= 1;
    fs::write(&path, &bytes).unwrap();
    restore_modified_time(&path, stamp);
    let after = fs::metadata(&path).unwrap();
    assert_eq!(after.len(), before.len());
    assert_eq!(after.modified().unwrap(), before.modified().unwrap());

    let prior_output = project.root().join("build/prior-output.scx");
    fs::create_dir_all(prior_output.parent().unwrap()).unwrap();
    fs::write(&prior_output, b"previous-output-is-not-green").unwrap();
    let prior_bytes = fs::read(&prior_output).unwrap();
    let missing_launcher = package.moved.join("launcher-must-not-run.exe");
    let error = run_native_build(
        &project,
        &compat_root(),
        &EuddraftLaunch::Executable(missing_launcher.clone()),
    )
    .expect_err("same metadata with changed bytes must be rejected");
    assert!(error.to_ascii_lowercase().contains("sha256"), "{error}");
    assert!(
        !error.contains("launcher-must-not-run"),
        "resource check precedes spawn"
    );
    assert_eq!(fs::read(&prior_output).unwrap(), prior_bytes);
    assert!(!missing_launcher.exists());
}

pub(super) fn assert_missing_and_case_collision(owner: &super::package::OwnedRoot) {
    let missing = Package::create(owner, "missing", TileMode::Memory);
    fs::remove_file(missing.root.join("Resource/scene.bin")).unwrap();
    assert_resource_refusal(&missing, &[E2IssueCode::InvalidResource]);

    let collision = Package::create(owner, "collision", TileMode::Memory);
    fs::remove_file(collision.root.join("Resource/scene.bin")).unwrap();
    let source = fs::read_to_string(&collision.source).unwrap();
    assert!(source.contains("DataFile : Resource/scene.bin"));
    fs::write(
        &collision.source,
        source.replacen(
            "DataFile : Resource/scene.bin",
            "DataFile : Missing/scene.bin",
            1,
        ),
    )
    .unwrap();
    fs::write(collision.root.join("Resource/Scene.bin"), [2, 0, 0, 0]).unwrap();
    fs::write(collision.root.join("SCENE.BIN"), [2, 0, 0, 0]).unwrap();
    assert_resource_refusal(&collision, &[E2IssueCode::InvalidResource]);
}

fn assert_resource_refusal(package: &Package, accepted: &[E2IssueCode]) {
    fs::create_dir(&package.destination).unwrap();
    let result = super::super::transaction::preflight(&E2PreflightRequest {
        source_path: package.source.to_string_lossy().into_owned(),
        destination_path: package.destination.to_string_lossy().into_owned(),
        relinks: Vec::new(),
    });
    match result {
        E2PreflightResult::NeedsRelink { needs, .. } => {
            assert!(needs
                .iter()
                .any(|need| need.id == "resource:BinEditSET:DataFile"));
        }
        E2PreflightResult::Fatal { issues, .. } => {
            assert!(issues.iter().any(|issue| accepted.contains(&issue.code)));
        }
        other => panic!("missing/ambiguous resource was accepted: {other:?}"),
    }
    assert_eq!(fs::read_dir(&package.destination).unwrap().count(), 0);
}

fn import_without_build(package: &Package) {
    fs::create_dir(&package.destination).unwrap();
    let fingerprint = match super::super::transaction::preflight(&E2PreflightRequest {
        source_path: package.source.to_string_lossy().into_owned(),
        destination_path: package.destination.to_string_lossy().into_owned(),
        relinks: Vec::new(),
    }) {
        E2PreflightResult::Ready { fingerprint, .. } => fingerprint,
        other => panic!("drift fixture preflight failed: {other:?}"),
    };
    let result = super::super::transaction::import(
        "task-16-drift",
        &E2ImportRequest {
            source_path: package.source.to_string_lossy().into_owned(),
            destination_path: package.destination.to_string_lossy().into_owned(),
            fingerprint,
            relinks: Vec::new(),
        },
        &package.dirs,
        &compat_root(),
        &ProcessCancellation::default(),
        |_| {},
        |_| Ok(()),
    );
    assert!(
        matches!(result, E2ImportResult::Completed { .. }),
        "{result:?}"
    );
}

#[cfg(windows)]
#[derive(Clone, Copy)]
struct FileTimes([windows_sys::Win32::Foundation::FILETIME; 3]);

#[cfg(windows)]
fn file_times(path: &Path) -> FileTimes {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::Storage::FileSystem::GetFileTime;

    let file = fs::File::open(path).unwrap();
    let mut created = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut accessed = created;
    let mut modified = created;
    unsafe {
        assert_ne!(
            GetFileTime(
                file.as_raw_handle() as _,
                &mut created,
                &mut accessed,
                &mut modified
            ),
            0
        );
    }
    FileTimes([created, accessed, modified])
}

#[cfg(windows)]
fn restore_modified_time(path: &Path, times: FileTimes) {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::SetFileTime;

    let file = fs::OpenOptions::new().write(true).open(path).unwrap();
    unsafe {
        assert_ne!(
            SetFileTime(
                file.as_raw_handle() as _,
                &times.0[0],
                &times.0[1],
                &times.0[2]
            ),
            0
        );
    }
}

#[cfg(not(windows))]
type FileTimes = ();

#[cfg(not(windows))]
fn file_times(_path: &Path) -> FileTimes {}

#[cfg(not(windows))]
fn restore_modified_time(_path: &Path, _times: FileTimes) {}
