use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::config::DataDirs;
use crate::e2_import::{
    E2Feature, E2PreflightRequest, E2PreflightResult, E2RelinkChoice, E2RelinkKind,
};
use crate::native_build::NativeBuildResult;
use crate::native_project::NativeProject;

const SAMPLE_DIR: &str = "E:/proj/eud/eud-agent/defense_mob";
pub(super) const SAMPLE_E2S: &str = "defense_mob.e2s";
pub(super) const INPUT_MAP: &str = "basemap.scx";
pub(super) const COMPARISON_OUTPUT: &str = "defense_mob.scx";

pub(super) struct SamplePaths {
    pub(super) source: PathBuf,
    pub(super) input_map: PathBuf,
    pub(super) comparison_output: PathBuf,
}

pub(super) struct DisposableRoot(pub(super) PathBuf);

impl Drop for DisposableRoot {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).ok();
    }
}

pub(super) fn ready_with_owned_input(
    source: &Path,
    destination: &Path,
    input: &Path,
) -> (String, Vec<E2RelinkChoice>, Vec<E2Feature>) {
    let first = super::super::preflight(&E2PreflightRequest {
        source_path: source.to_string_lossy().into_owned(),
        destination_path: destination.to_string_lossy().into_owned(),
        relinks: Vec::new(),
    });
    let E2PreflightResult::NeedsRelink { needs, .. } = first else {
        panic!("legacy absolute input must require relink: {first:?}");
    };
    let input_need = needs
        .into_iter()
        .find(|need| need.kind == E2RelinkKind::InputMap)
        .expect("input-map relink");
    let relinks = vec![E2RelinkChoice {
        need_id: input_need.id,
        path: input.to_string_lossy().into_owned(),
    }];
    match super::super::preflight(&E2PreflightRequest {
        source_path: source.to_string_lossy().into_owned(),
        destination_path: destination.to_string_lossy().into_owned(),
        relinks: relinks.clone(),
    }) {
        E2PreflightResult::Ready {
            fingerprint,
            features,
            ..
        } => (fingerprint, relinks, features),
        other => panic!("owned input-map relink must be fingerprint-ready: {other:?}"),
    }
}

pub(super) fn protected_paths() -> SamplePaths {
    let root = PathBuf::from(SAMPLE_DIR);
    SamplePaths {
        source: root.join(SAMPLE_E2S),
        input_map: root.join(INPUT_MAP),
        comparison_output: root.join(COMPARISON_OUTPUT),
    }
}

pub(super) fn protected_hashes(paths: &SamplePaths) -> [String; 3] {
    [
        sha256(&paths.source),
        sha256(&paths.input_map),
        sha256(&paths.comparison_output),
    ]
}

pub(super) fn data_dirs(root: &Path) -> DataDirs {
    let dirs = DataDirs::from_bases(&root.join("roaming"), &root.join("local"));
    dirs.ensure_dirs().unwrap();
    dirs
}

pub(super) fn disposable_root(tag: &str) -> DisposableRoot {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "eud-agent-task15-{tag}-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&root).unwrap();
    DisposableRoot(root)
}

pub(super) fn compat_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/eud-editor-compat")
}

pub(super) fn required_env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} must be set"))
}

fn sha256(path: &Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(path).unwrap()))
}

pub(super) fn fresh_evidence_root() -> PathBuf {
    let root = PathBuf::from(required_env("EUD_AGENT_BUILD_EVIDENCE_DIR"));
    assert!(root.is_absolute() && root.parent().is_some());
    assert!(!root.exists(), "Task 15 evidence root must be absent");
    fs::create_dir(&root).expect("create fresh Task 15 evidence root");
    root
}

pub(super) fn retain_build_evidence(
    root: &Path,
    project: &NativeProject,
    build: &Result<NativeBuildResult, String>,
    generated_main: &str,
    original_hashes: &[String; 3],
    protected_after_build: &[String; 3],
) {
    fs::write(root.join("generated-main.eps"), generated_main).unwrap();
    copy_available(
        root,
        &project.root().join("src/main.eps"),
        "project-main.eps",
    );
    copy_available(root, project.manifest_path(), "project.eap");
    copy_available(
        root,
        &crate::native_build::build_log_path(project.root()),
        "native-build.log",
    );
    copy_available(
        root,
        &project.root().join("build/euddraft/eud-agent.eds"),
        "generated.eds",
    );
    if let Ok(output) = project.output_map_path() {
        copy_available(root, &output, "fresh-output.scx");
        if output.is_file() {
            fs::write(
                root.join("fresh-output.sha256"),
                format!("{}\n", sha256(&output)),
            )
            .unwrap();
        }
    }
    fs::write(
        root.join("protected-hashes.txt"),
        format!(
            "before={}\nafter={}\n",
            original_hashes.join(","),
            protected_after_build.join(",")
        ),
    )
    .unwrap();
    match build {
        Ok(result) => {
            copy_available(root, Path::new(&result.log_path), "native-build.log");
            copy_available(root, Path::new(&result.artifacts.eds_path), "generated.eds");
            let output = PathBuf::from(&result.artifacts.output_map);
            copy_available(root, &output, "fresh-output.scx");
            if output.is_file() {
                fs::write(
                    root.join("fresh-output.sha256"),
                    format!("{}\n", sha256(&output)),
                )
                .unwrap();
            }
            fs::write(
                root.join("native-build-result.txt"),
                format!("ok={}\nerrors={:?}\n", result.ok, result.errors),
            )
            .unwrap();
        }
        Err(error) => {
            fs::write(root.join("native-build-error.txt"), format!("{error}\n")).unwrap();
        }
    }
}

fn copy_available(root: &Path, source: &Path, name: &str) {
    if source.is_file() && !root.join(name).exists() {
        fs::copy(source, root.join(name)).unwrap();
    }
}
