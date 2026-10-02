use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::{
    E2Feature, E2ImportManifest, E2ImportPhase, E2ImportProvenance, E2ImportResult, E2Issue,
    E2IssueCode, E2IssueSeverity, E2PreflightResult, E2SourceKind, E2Version, ProjectResource,
    ProjectResourceConsumer, ProjectResourceKind, ProjectResourceProvenance,
};
use crate::native_project::{
    validate_manifest, NativeProject, ProjectManifest, ProjectSettings, PROJECT_SCHEMA_VERSION,
};

struct TestRoot(PathBuf);

impl TestRoot {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "eud-agent-e2-contract-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock must be after the Unix epoch")
                .as_nanos()
        ));
        fs::create_dir_all(root.join("maps")).expect("map directory must be created");
        fs::write(root.join("maps/source.scx"), b"map").expect("source map must be written");
        Self(root)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.0) {
            if error.kind() != std::io::ErrorKind::NotFound {
                panic!("test root cleanup failed: {error}");
            }
        }
    }
}

fn manifest(e2_import: Option<E2ImportManifest>) -> ProjectManifest {
    ProjectManifest {
        schema_version: PROJECT_SCHEMA_VERSION,
        name: "E2 contract".to_string(),
        source_map: "maps/source.scx".to_string(),
        output_map: "build/output.scx".to_string(),
        main_file: "src/main.eps".to_string(),
        settings: ProjectSettings::default(),
        plugins: Vec::new(),
        python_entrypoints: Vec::new(),
        python_dependencies: Vec::new(),
        python_lock: None,
        editor_compatibility: None,
        e2_import,
    }
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn resource(path: &str, bytes: &[u8]) -> ProjectResource {
    ProjectResource {
        id: "audio-1".to_string(),
        path: path.to_string(),
        kind: ProjectResourceKind::Audio,
        size: u64::try_from(bytes.len()).expect("fixture length must fit u64"),
        sha256: sha256(bytes),
        consumer: ProjectResourceConsumer::Mpq,
        build_destination: "M1_0.ogg".to_string(),
        provenance: ProjectResourceProvenance {
            feature: E2Feature::SoundBgmReferences,
            original_path: "Data/temp/M1_0.ogg".to_string(),
        },
    }
}

fn e2_manifest(resources: Vec<ProjectResource>) -> E2ImportManifest {
    E2ImportManifest {
        version: E2Version::V0_17_9_7,
        source_kind: E2SourceKind::E2p,
        fingerprint: "fixture-fingerprint".to_string(),
        resources,
        provenance: E2ImportProvenance {
            source_name: "fixture.e2p".to_string(),
            disabled_sections: Vec::new(),
        },
    }
}

#[test]
fn legacy_v2_stays_field_absent() {
    // Given
    let root = TestRoot::new("legacy");

    // When
    let project = NativeProject::create(root.path(), manifest(None))
        .expect("legacy schema-v2 project must be created");
    let before = project.revision().expect("revision must be available");
    let serialized = project
        .manifest_bytes()
        .expect("manifest bytes must be readable");
    let reopened = NativeProject::open(root.path()).expect("legacy project must reopen");

    // Then
    assert_eq!(reopened.manifest().e2_import, None);
    assert_eq!(
        reopened.revision().expect("revision must be stable"),
        before
    );
    let value: serde_json::Value =
        serde_json::from_slice(&serialized).expect("manifest must be valid JSON");
    assert!(value.get("e2Import").is_none());
}

#[test]
fn resource_bytes_drive_revision_and_snapshot() {
    // Given
    let root = TestRoot::new("revision");
    let path = "resources/e2/audio/M1_0.ogg";
    let original = b"OggS-original";
    fs::create_dir_all(root.path().join("resources/e2/audio"))
        .expect("resource directory must be created");
    fs::write(root.path().join(path), original).expect("resource must be written");
    let project = NativeProject::create(
        root.path(),
        manifest(Some(e2_manifest(vec![resource(path, original)]))),
    )
    .expect("resource-bearing project must be created");
    let before = project.revision().expect("revision must be available");

    // When
    let snapshot = project
        .resource_snapshot()
        .expect("resource snapshot must be available");
    fs::write(root.path().join(path), b"OggS-modified").expect("resource must be changed");
    let after = project
        .revision()
        .expect("revision must include current resource bytes");
    let reopen_error = NativeProject::open(root.path()).expect_err("stale hash must reject reopen");

    // Then
    assert_eq!(snapshot.resources.len(), 1);
    assert_eq!(snapshot.resources[0].path, path);
    assert_eq!(snapshot.resources[0].sha256, sha256(original));
    assert_ne!(after, before);
    assert!(reopen_error.contains("sha256"));
}

#[test]
fn resource_confinement_matrix() {
    // Given
    let bytes = b"resource";
    let invalid_paths = ["C:/escape.bin", "../escape.bin", "resources/e2/bad\0.bin"];

    // When / Then: lexical confinement
    for path in invalid_paths {
        let error = validate_manifest(&manifest(Some(e2_manifest(vec![resource(path, bytes)]))))
            .expect_err("unsafe resource path must fail manifest validation");
        assert!(
            error.contains("resource"),
            "unexpected error for {path}: {error}"
        );
    }

    // Given: case collision
    let first = resource("resources/e2/audio/A.ogg", bytes);
    let mut second = resource("resources/e2/audio/a.ogg", bytes);
    second.id = "audio-2".to_string();

    // When / Then: case collision
    let collision = validate_manifest(&manifest(Some(e2_manifest(vec![first, second]))))
        .expect_err("case-colliding resources must fail");
    assert!(collision.contains("case"));

    // Given: missing resource
    let missing_root = TestRoot::new("missing");

    // When / Then: missing resource
    let missing = NativeProject::create(
        missing_root.path(),
        manifest(Some(e2_manifest(vec![resource(
            "resources/e2/missing.bin",
            bytes,
        )]))),
    )
    .expect_err("missing resource must fail");
    assert!(missing.contains("does not exist"));

    // Given: size and hash mismatches
    let stale_root = TestRoot::new("stale");
    let stale_path = "resources/e2/files/data.bin";
    fs::create_dir_all(stale_root.path().join("resources/e2/files"))
        .expect("resource directory must be created");
    fs::write(stale_root.path().join(stale_path), bytes).expect("resource must be written");
    let mut wrong_size = resource(stale_path, bytes);
    wrong_size.size = wrong_size.size.saturating_add(1);

    // When / Then: stale size
    let size_error = NativeProject::create(
        stale_root.path(),
        manifest(Some(e2_manifest(vec![wrong_size]))),
    )
    .expect_err("stale resource size must fail");
    assert!(size_error.contains("size"));

    // When / Then: stale hash
    let mut wrong_hash = resource(stale_path, bytes);
    wrong_hash.sha256 = "0".repeat(64);
    let hash_error = NativeProject::create(
        stale_root.path(),
        manifest(Some(e2_manifest(vec![wrong_hash]))),
    )
    .expect_err("stale resource hash must fail");
    assert!(hash_error.contains("sha256"));

    // Given: a resource symlink/reparse point that canonically escapes the root
    let link_root = TestRoot::new("link");
    fs::create_dir_all(link_root.path().join("resources/e2/files"))
        .expect("resource directory must be created");
    let outside = TestRoot::new("outside");
    let outside_file = outside.path().join("data.bin");
    fs::write(&outside_file, bytes).expect("outside fixture must be written");
    #[cfg(windows)]
    {
        let junction = link_root
            .path()
            .join("resources")
            .join("e2")
            .join("files")
            .join("link-dir");
        let command = format!(
            "mklink /J {} {}",
            junction.display(),
            outside.path().display()
        );
        let status = std::process::Command::new("cmd.exe")
            .args(["/d", "/s", "/c", &command])
            .status()
            .expect("junction command must start");
        assert!(status.success(), "resource junction must be created");
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            &outside_file,
            link_root.path().join("resources/e2/files/link.bin"),
        )
        .expect("resource symlink must be created");
    }

    // When / Then: symlink/reparse and canonical escape
    let link_error = NativeProject::create(
        link_root.path(),
        manifest(Some(e2_manifest(vec![resource(
            if cfg!(windows) {
                "resources/e2/files/link-dir/data.bin"
            } else {
                "resources/e2/files/link.bin"
            },
            bytes,
        )]))),
    )
    .expect_err("linked resource must fail");
    assert!(link_error.contains("symlink") || link_error.contains("reparse"));
    #[cfg(windows)]
    fs::remove_dir(
        link_root
            .path()
            .join("resources")
            .join("e2")
            .join("files")
            .join("link-dir"),
    )
    .expect("resource junction must be removed");
    #[cfg(unix)]
    fs::remove_file(link_root.path().join("resources/e2/files/link.bin"))
        .expect("resource symlink must be removed");
}

#[test]
fn wire_contract_rejects_invalid_terminal_states() {
    // Given
    let issue = E2Issue {
        code: E2IssueCode::ActiveDefinitionUnknown,
        severity: E2IssueSeverity::Fatal,
        message: "unknown enabled definition".to_string(),
        path: None,
        feature: Some(E2Feature::StructGui),
        need_id: None,
    };
    let progress = E2ImportResult::Progress {
        import_id: "import-1".to_string(),
        fingerprint: "fingerprint-1".to_string(),
        phase: E2ImportPhase::Compiling,
        completed: 2,
        total: 1,
        message: "compiling".to_string(),
    };
    let completed = E2ImportResult::Completed {
        import_id: "import-1".to_string(),
        fingerprint: "fingerprint-1".to_string(),
        project_opened: false,
        project_path: "C:/project".to_string(),
    };
    let fatal = E2PreflightResult::Fatal {
        fingerprint: None,
        issues: vec![issue],
    };

    // When
    let progress_error = progress
        .validate()
        .expect_err("completed greater than total must fail");
    let completed_error = completed
        .validate()
        .expect_err("false projectOpened must fail");
    let fatal_json = serde_json::to_value(fatal).expect("fatal result must serialize");

    // Then
    assert!(progress_error.contains("completed"));
    assert!(completed_error.contains("projectOpened"));
    assert_eq!(fatal_json["state"], "fatal");
    assert_eq!(fatal_json["issues"][0]["code"], "active_definition_unknown");
    assert!(fatal_json.get("fingerprint").is_none());
    assert!(fatal_json["issues"][0].get("path").is_none());
}
