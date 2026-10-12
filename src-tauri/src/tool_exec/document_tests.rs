use super::{SessionToolRuntime, ToolServices};
use crate::native_project::{NativeProject, ProjectManifest, PROJECT_SCHEMA_VERSION};
use crate::worker::WorkerProfile;
use serde_json::json;

fn project() -> (ToolServices, std::path::PathBuf) {
    let services = ToolServices::for_tests();
    let root = services.dirs.app_data().join("documents-project");
    std::fs::create_dir_all(root.join("maps")).unwrap();
    std::fs::write(root.join("maps/source.scx"), b"fixture map").unwrap();
    let project = NativeProject::create(
        &root,
        ProjectManifest {
            schema_version: PROJECT_SCHEMA_VERSION,
            name: "Documents".into(),
            source_map: "maps/source.scx".into(),
            output_map: "build/output.scx".into(),
            main_file: "src/main.eps".into(),
            settings: Default::default(),
            plugins: Vec::new(),
            python_entrypoints: Vec::new(),
            python_dependencies: Vec::new(),
            python_lock: None,
            editor_compatibility: None,
            e2_import: None,
        },
    )
    .unwrap();
    services.bind_for_tests(&project);
    (services, root)
}

fn document_round_trip(runtime: &SessionToolRuntime) {
    let names = runtime
        .tool_descriptors()
        .into_iter()
        .map(|value| value["name"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    for tool in crate::tools::FS_TOOLS {
        assert!(names.iter().any(|name| name == tool), "{tool} missing");
    }
    for path in [
        "README.md",
        "notes/design.TXT",
        ".eud-agent/workspace/specs/game.md",
        "maps/notes.md",
        "references/notes.txt",
        ".claude/skills/helper/SKILL.md",
    ] {
        runtime
            .execute("fs_write", &json!({"path": path, "content": "draft\n"}))
            .unwrap();
        runtime
            .execute(
                "fs_edit",
                &json!({"path": path,
            "edits": [{"old_text": "draft", "new_text": "accepted"}]}),
            )
            .unwrap();
        let read = runtime.execute("fs_read", &json!({"path": path})).unwrap();
        assert_eq!(read["content"], "accepted");
        runtime
            .execute("fs_write", &json!({"path": path, "content": "updated\n"}))
            .unwrap();
        assert_eq!(
            runtime.execute("fs_read", &json!({"path": path})).unwrap()["content"],
            "updated"
        );
    }
    let files = runtime
        .execute("fs_glob", &json!({"pattern": "**/*.md"}))
        .unwrap();
    assert!(files.to_string().contains("README.md"));
    let matches = runtime
        .execute("fs_grep", &json!({"pattern": "updated", "glob": "*.txt"}))
        .unwrap();
    assert!(matches.to_string().contains("notes/design.TXT"));
    assert!(runtime.write_ticket().is_none());
}

#[test]
fn document_access_is_free_for_eps_and_investigation_workers() {
    for profile in [
        WorkerProfile::Implementation,
        WorkerProfile::Research,
        WorkerProfile::MapResearch,
    ] {
        let (services, root) = project();
        let runtime = services.session("documents");
        if profile != WorkerProfile::Implementation {
            runtime.set_worker_parent(Some("parent".into()));
        }
        runtime.set_worker_profile(profile);
        runtime
            .begin_request("documents-request", "project")
            .unwrap();
        document_round_trip(&runtime);
        if profile != WorkerProfile::Implementation {
            for path in ["src/main.eps", "dat/standard.json", "maps/source.scx"] {
                let error = runtime
                    .execute("fs_write", &json!({"path": path, "content": "x"}))
                    .unwrap_err();
                assert!(error.contains(".md") && error.contains(".txt"), "{error}");
            }
            assert!(runtime.register_write_request("mutation").is_err());
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn document_access_is_free_for_map_without_opening_binary_writes() {
    let (services, root) = project();
    let runtime = services.map_session("documents-map");
    runtime
        .begin_request("documents-request", "project")
        .unwrap();
    document_round_trip(&runtime);
    for path in [
        "maps/source.scx",
        "src/main.eps",
        "project.eap",
        ".claude/settings.json",
        "../escape.md",
        ".git/notes.md",
    ] {
        assert!(
            runtime
                .execute("fs_write", &json!({"path": path, "content": "x"}))
                .is_err(),
            "{path}"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn native_document_access_uses_the_real_read_mode_gate() {
    use crate::provider_runtime::{RunId, RunIdentity, WorkspaceAccess};
    use crate::provider_tool_loop::RunGate;

    let (services, root) = project();
    let runtime = services.session("native-documents");
    let (_cancel, cancellation) = tokio::sync::watch::channel(0_u64);
    runtime.set_cancellation(cancellation);
    runtime
        .begin_request("native-documents-request", "project")
        .unwrap();
    let gate = RunGate::new(
        RunIdentity {
            session_id: runtime.session_id().into(),
            run_id: RunId::new(81_010),
            request_id: "native-documents-request".into(),
            session_kind: runtime.kind(),
            cancellation_generation: 0,
        },
        runtime.clone(),
        WorkspaceAccess::Read,
        None,
    );
    for (id, name, args) in [
        (
            "write",
            "fs_write",
            json!({"path": "native.md", "content": "draft"}),
        ),
        (
            "edit",
            "fs_edit",
            json!({"path": "native.md",
            "edits": [{"old_text": "draft", "new_text": "accepted"}]}),
        ),
        ("read", "fs_read", json!({"path": "native.md"})),
    ] {
        let result = gate
            .dispatch_native(Some(id.into()), name.into(), args)
            .await
            .unwrap();
        let value = serde_json::to_value(result).unwrap();
        if name == "fs_read" {
            assert!(value.to_string().contains("accepted"), "{value}");
        }
    }
    assert_eq!(
        std::fs::read_to_string(root.join("native.md")).unwrap(),
        "accepted"
    );
    assert_eq!(gate.completed().len(), 3);
    assert!(runtime.write_ticket().is_none());
    std::fs::remove_dir_all(root).unwrap();
}
