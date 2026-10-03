use std::fs;
use std::path::PathBuf;

use sha2::{Digest, Sha256};

use super::{generate_eds, resolve_e2_build_plan, GeneratedEdsSections};
use crate::e2_import::{
    E2Feature, E2ImportManifest, E2ImportProvenance, E2SourceKind, E2Version, ProjectResource,
    ProjectResourceConsumer, ProjectResourceKind, ProjectResourceProvenance,
};
use crate::native_project::{
    NativeProject, ProjectManifest, ProjectSettings, PROJECT_SCHEMA_VERSION,
};

#[test]
fn mixed_case_e2_sources_emit_once_in_producer_order() {
    let root = unique_root();
    let _cleanup = Cleanup(root.clone());
    fs::create_dir_all(root.join("maps")).unwrap();
    fs::create_dir_all(root.join("resources/e2/original")).unwrap();
    fs::create_dir_all(root.join("src/e2/plugins")).unwrap();
    fs::write(root.join("maps/source.scx"), b"map").unwrap();

    let plugin_a = b"from eudplib import *\n";
    let plugin_b = b"function plugin_b() {}\n";
    let plugin_c = b"from eudplib import *\n";
    let segment = b"OggS-segment";
    let resources = vec![
        resource("A", "A.py", "src/e2/plugins/pluginset-a.py", plugin_a),
        resource("B", "B.eps", "src/e2/plugins/pluginset-b.eps", plugin_b),
        resource("c", "c.py", "src/e2/plugins/pluginset-c.py", plugin_c),
        segment_resource(segment),
    ];
    for (name, bytes) in [
        ("A.py", plugin_a.as_slice()),
        ("B.eps", plugin_b.as_slice()),
        ("c.py", plugin_c.as_slice()),
        ("segment.ogg", segment.as_slice()),
    ] {
        fs::write(root.join("resources/e2/original").join(name), bytes).unwrap();
    }
    for (path, bytes) in [
        ("src/direct.py", b"from eudplib import *\n".as_slice()),
        ("src/e2/tct.py", b"from eudplib import *\n".as_slice()),
        ("src/e2/bin_compat.eps", b"function bin() {}\n".as_slice()),
        ("src/e2/tile_compat.eps", b"function tile() {}\n".as_slice()),
        ("src/e2/plugins/pluginset-a.py", plugin_a.as_slice()),
        ("src/e2/plugins/pluginset-b.eps", plugin_b.as_slice()),
        ("src/e2/plugins/pluginset-c.py", plugin_c.as_slice()),
        ("src/e2/bgm.eps", b"function bgm() {}\n".as_slice()),
    ] {
        let target = root.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, bytes).unwrap();
    }

    let project = NativeProject::create(
        &root,
        ProjectManifest {
            schema_version: PROJECT_SCHEMA_VERSION,
            name: "E2 source order".to_string(),
            source_map: "maps/source.scx".to_string(),
            output_map: "build/output.scx".to_string(),
            main_file: "src/main.eps".to_string(),
            settings: ProjectSettings::default(),
            plugins: Vec::new(),
            python_entrypoints: vec![
                "src/direct.py".to_string(),
                "src/e2/plugins/pluginset-a.py".to_string(),
                "src/e2/plugins/pluginset-c.py".to_string(),
            ],
            python_dependencies: Vec::new(),
            python_lock: None,
            editor_compatibility: None,
            e2_import: Some(E2ImportManifest {
                version: E2Version::V0_17_9_7,
                source_kind: E2SourceKind::E2p,
                fingerprint: "fixture-fingerprint".to_string(),
                resources,
                provenance: E2ImportProvenance {
                    source_name: "fixture.e2p".to_string(),
                    disabled_sections: Vec::new(),
                },
            }),
        },
    )
    .unwrap();
    let plan = resolve_e2_build_plan(&project, None).unwrap();
    let eds = generate_eds(
        project.root(),
        project.manifest(),
        &project.root().join("build/euddraft/eud-agent.eds"),
        GeneratedEdsSections {
            python_path: true,
            data_editor: false,
            wireframe_editor: false,
            extra_editor: false,
            custom_tbl: false,
            e2_plan: &plan,
        },
    )
    .unwrap();

    let headers = [
        "[PythonPath.py]",
        "[../../src/direct.py]",
        "[../../src/e2/bin_compat.eps]",
        "[../../src/e2/tile_compat.eps]",
        "[../../src/e2/plugins/pluginset-a.py]",
        "[../../src/e2/plugins/pluginset-b.eps]",
        "[../../src/e2/plugins/pluginset-c.py]",
        "[../../src/e2/bgm.eps]",
        "[../../src/main.eps]",
    ];
    let positions = headers.map(|header| {
        assert_eq!(eds.matches(header).count(), 1, "{header}\n{eds}");
        eds.find(header).unwrap()
    });
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]), "{eds}");
    assert!(!eds.contains("[../../src/e2/tct.py]"));
}

fn resource(id: &str, name: &str, destination: &str, bytes: &[u8]) -> ProjectResource {
    ProjectResource {
        id: format!("PluginSET:{id}"),
        path: format!("resources/e2/original/{name}"),
        kind: ProjectResourceKind::Plugin,
        size: bytes.len() as u64,
        sha256: sha(bytes),
        consumer: ProjectResourceConsumer::Plugin,
        build_destination: destination.to_string(),
        provenance: ProjectResourceProvenance {
            feature: E2Feature::Plugin,
            original_path: format!("Resource/{name}"),
        },
    }
}

fn segment_resource(bytes: &[u8]) -> ProjectResource {
    ProjectResource {
        id: "SoundPlayerSET:3:segment:0".to_string(),
        path: "resources/e2/original/segment.ogg".to_string(),
        kind: ProjectResourceKind::Audio,
        size: bytes.len() as u64,
        sha256: sha(bytes),
        consumer: ProjectResourceConsumer::Mpq,
        build_destination: "M3_0.ogg".to_string(),
        provenance: ProjectResourceProvenance {
            feature: E2Feature::SoundBgmReferences,
            original_path: "Sound/track.ogg".to_string(),
        },
    }
}

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn unique_root() -> PathBuf {
    std::env::temp_dir().join(format!(
        "eud-agent-e2-source-order-{}",
        uuid::Uuid::new_v4()
    ))
}

struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).ok();
    }
}
