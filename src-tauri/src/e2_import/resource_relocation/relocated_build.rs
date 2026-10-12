use std::cell::Cell;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::bootstrap::process_tree::ProcessCancellation;
use crate::e2_import::{
    E2Feature, E2ImportRequest, E2ImportResult, E2PreflightRequest, E2PreflightResult,
};
use crate::native_build::{run_native_build, EuddraftLaunch, NativeBuildResult};
use crate::native_project::NativeProject;

use super::package::{Package, EUDDRAFT};

pub(super) struct RelocatedBuild {
    pub project: NativeProject,
    pub result: NativeBuildResult,
    pub output: PathBuf,
    pub log: String,
    pub features: Vec<E2Feature>,
    pub original_resource_count: usize,
}

pub(super) fn import_move_and_build(package: &Package, import_id: &str) -> RelocatedBuild {
    fs::create_dir(&package.destination).unwrap();
    let preflight = super::super::transaction::preflight(&E2PreflightRequest {
        source_path: package.source.to_string_lossy().into_owned(),
        destination_path: package.destination.to_string_lossy().into_owned(),
        relinks: Vec::new(),
    });
    let (fingerprint, features) = match preflight {
        E2PreflightResult::Ready {
            fingerprint,
            features,
            warnings,
            ..
        } => {
            assert!(warnings.is_empty(), "preflight warnings: {warnings:?}");
            (fingerprint, features)
        }
        other => panic!("authentic E2P preflight was not ready: {other:?}"),
    };
    let activation_count = Cell::new(0_u32);
    let manager = crate::native_runtime::NativeProjectSetup::new(package.dirs.clone());
    let request = E2ImportRequest {
        source_path: package.source.to_string_lossy().into_owned(),
        destination_path: package.destination.to_string_lossy().into_owned(),
        fingerprint: fingerprint.clone(),
        relinks: Vec::new(),
    };
    let result = super::super::transaction::import(
        import_id,
        &request,
        &package.dirs,
        &compat_root(),
        &ProcessCancellation::default(),
        |_| {},
        |project| {
            activation_count.set(activation_count.get() + 1);
            manager.activate_project(project)
        },
    );
    assert!(
        matches!(
            result,
            E2ImportResult::Completed {
                project_opened: true,
                ..
            }
        ),
        "import failed: {result:?}"
    );
    assert_eq!(activation_count.get(), 1, "activation occurs exactly once");

    let published = NativeProject::open(&package.destination).unwrap();
    let before_snapshot = published.resource_snapshot().unwrap();
    let before_revision = published.revision().unwrap();
    drop(published);
    fs::rename(&package.destination, &package.moved).unwrap();
    fs::rename(&package.root, &package.unavailable).unwrap();
    assert!(
        !package.source.exists(),
        "source package must be unavailable"
    );
    package.assert_originals(&package.unavailable);

    let project = NativeProject::open(&package.moved).expect("absolute moved EAP reopens");
    assert_eq!(project.resource_snapshot().unwrap(), before_snapshot);
    assert_eq!(project.revision().unwrap(), before_revision);
    let launcher = EuddraftLaunch::resolve(Path::new(EUDDRAFT)).unwrap();
    let result = run_native_build(&project, &compat_root(), &launcher).unwrap();
    retain_build_evidence(import_id, &result);
    assert!(result.ok, "real build errors: {:?}", result.errors);
    assert!(result.errors.is_empty());
    assert!(
        result.warnings.is_empty(),
        "real build warnings: {:?}",
        result.warnings
    );
    let output = PathBuf::from(&result.artifacts.output_map);
    assert!(output.is_file() && fs::metadata(&output).unwrap().len() > 0);
    let log = fs::read_to_string(&result.log_path).unwrap();
    for marker in ["[[ freeze activated ]]", "Injecting plugins", "[Stage 4/3]"] {
        assert!(log.contains(marker), "missing actual build marker {marker}");
    }
    assert!(!log.to_ascii_lowercase().contains("error"));
    assert_eq!(project.revision().unwrap(), before_revision);
    RelocatedBuild {
        original_resource_count: before_snapshot.resources.len(),
        project,
        result,
        output,
        log,
        features,
    }
}

fn retain_build_evidence(import_id: &str, result: &NativeBuildResult) {
    let root = PathBuf::from(std::env::var("EUD_AGENT_TASK16_EVIDENCE").unwrap());
    assert!(root.is_absolute() && root.parent().is_some());
    match fs::create_dir(&root) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists && root.is_dir() => {}
        Err(error) => panic!("cannot create Task 16 evidence root: {error}"),
    }
    let variant = import_id.strip_prefix("task-16-").unwrap();
    let destination = root.join(variant);
    fs::create_dir(&destination).unwrap();
    let mut log = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination.join("native-build.log"))
        .unwrap();
    io::copy(&mut fs::File::open(&result.log_path).unwrap(), &mut log).unwrap();
    let mut eds = fs::File::create_new(destination.join("generated.eds")).unwrap();
    let mut generated = fs::File::open(&result.artifacts.eds_path).unwrap();
    io::copy(&mut generated, &mut eds).unwrap();
    let output = PathBuf::from(&result.artifacts.output_map);
    if output.is_file() {
        let bytes = fs::read(&output).unwrap();
        let mut retained = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination.join("fresh-output.scx"))
            .unwrap();
        retained.write_all(&bytes).unwrap();
        let mut receipt = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination.join("fresh-output.sha256"))
            .unwrap();
        writeln!(receipt, "{:x}", Sha256::digest(&bytes)).unwrap();
    }
}

pub(super) fn compat_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/eud-editor-compat")
}
