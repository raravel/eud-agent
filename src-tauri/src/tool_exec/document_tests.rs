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
        "skills/helper/notes.md",
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
    // Agent-instruction documents are not free: the main EPS session asks
    // the user first (never on this synchronous path), every other role is
    // refused.
    for path in [".claude/skills/helper/SKILL.md", "docs/CLAUDE.md"] {
        assert!(
            runtime
                .execute("fs_write", &json!({"path": path, "content": "x"}))
                .is_err(),
            "{path}"
        );
    }
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_instruction_write_through_the_tool_gate_waits_for_the_users_answer() {
    use crate::provider_runtime::{RunId, RunIdentity, WorkspaceAccess};
    use crate::provider_tool_loop::RunGate;

    // Given: the main EPS session's run gate, with an ask card channel.
    let (services, root) = project();
    let runtime = services.session("native-instructions");
    let (_cancel, cancellation) = tokio::sync::watch::channel(0_u64);
    runtime.set_cancellation(cancellation);
    runtime
        .begin_request("native-instructions-request", "project")
        .unwrap();
    let (events, mut emitted) = tokio::sync::mpsc::unbounded_channel();
    runtime.set_ask_emitter(move |event| {
        events
            .send(event)
            .map_err(|_| "ask event receiver closed".to_string())
    });
    let gate = RunGate::new(
        RunIdentity {
            session_id: runtime.session_id().into(),
            run_id: RunId::new(81_011),
            request_id: "native-instructions-request".into(),
            session_kind: runtime.kind(),
            cancellation_generation: 0,
        },
        runtime.clone(),
        WorkspaceAccess::Read,
        None,
    );
    let answer = |label: &str, event: &crate::ipc::AskEvent| {
        runtime
            .answer_ask(
                &event.request_id,
                std::collections::BTreeMap::from([(
                    "instruction_write".to_string(),
                    crate::ipc::AskAnswer {
                        answers: vec![label.to_string()],
                    },
                )]),
            )
            .unwrap();
    };

    // When: the model writes docs/CLAUDE.md and the user allows it.
    let allowed = {
        let gate = gate.clone();
        tokio::spawn(async move {
            gate.dispatch_native(
                Some("allowed".into()),
                "fs_write".into(),
                json!({"path": "docs/CLAUDE.md", "content": "# x"}),
            )
            .await
        })
    };
    let card = emitted.recv().await.unwrap();
    assert_eq!(card.kind, crate::ipc::AskKind::InstructionWrite);
    assert!(!root.join("docs/CLAUDE.md").exists());
    answer("허용", &card);
    allowed.await.unwrap().unwrap();
    // Then: it is written.
    assert_eq!(
        std::fs::read_to_string(root.join("docs/CLAUDE.md")).unwrap(),
        "# x"
    );

    // And a refused one is not.
    let refused = {
        let gate = gate.clone();
        tokio::spawn(async move {
            gate.dispatch_native(
                Some("refused".into()),
                "fs_write".into(),
                json!({"path": "AGENTS.md", "content": "# rules"}),
            )
            .await
        })
    };
    let card = emitted.recv().await.unwrap();
    answer("거부", &card);
    let result = serde_json::to_value(refused.await.unwrap().unwrap()).unwrap();
    assert!(
        result.to_string().contains("허용하지 않았습니다"),
        "{result}"
    );
    assert!(!root.join("AGENTS.md").exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_queued_instruction_writes_each_run_once_allowed() {
    use crate::provider_runtime::{RunId, RunIdentity, WorkspaceAccess};
    use crate::provider_tool_loop::RunGate;

    // Given: the main EPS session's run gate and two instruction writes
    // the model sent in one batch.
    let (services, root) = project();
    let runtime = services.session("queued-instructions");
    let (_cancel, cancellation) = tokio::sync::watch::channel(0_u64);
    runtime.set_cancellation(cancellation);
    runtime
        .begin_request("queued-instructions-request", "project")
        .unwrap();
    let (events, mut emitted) = tokio::sync::mpsc::unbounded_channel();
    runtime.set_ask_emitter(move |event| {
        events
            .send(event)
            .map_err(|_| "ask event receiver closed".to_string())
    });
    let gate = RunGate::new(
        RunIdentity {
            session_id: runtime.session_id().into(),
            run_id: RunId::new(81_012),
            request_id: "queued-instructions-request".into(),
            session_kind: runtime.kind(),
            cancellation_generation: 0,
        },
        runtime.clone(),
        WorkspaceAccess::Read,
        None,
    );
    let write = |id: &'static str, path: &'static str| {
        let gate = gate.clone();
        tokio::spawn(async move {
            gate.dispatch_native(
                Some(id.into()),
                "fs_write".into(),
                json!({"path": path, "content": id}),
            )
            .await
        })
    };
    let allow = |event: &crate::ipc::AskEvent| {
        runtime
            .answer_ask(
                &event.request_id,
                std::collections::BTreeMap::from([(
                    "instruction_write".to_string(),
                    crate::ipc::AskAnswer {
                        answers: vec!["허용".to_string()],
                    },
                )]),
            )
            .unwrap();
    };
    let first = write("first", "AGENTS.md");
    let card = emitted.recv().await.unwrap();
    let second = write("second", "docs/CLAUDE.md");
    let (first_path, second_path) = if card.questions[0].question.contains("AGENTS.md") {
        ("AGENTS.md", "docs/CLAUDE.md")
    } else {
        ("docs/CLAUDE.md", "AGENTS.md")
    };

    // When: the user allows the shown card.
    allow(&card);
    // Then: the next card comes up only once the allowed write ran.
    let next = emitted.recv().await.unwrap();
    assert!(
        root.join(first_path).is_file(),
        "the allowed write ran before the next card"
    );
    assert!(next.questions[0].question.contains(second_path));
    allow(&next);
    for call in [first, second] {
        let result = serde_json::to_value(call.await.unwrap().unwrap()).unwrap();
        assert!(!result.to_string().contains("허용해야"), "{result}");
    }
    assert_eq!(
        std::fs::read_to_string(root.join("AGENTS.md")).unwrap(),
        "first"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("docs/CLAUDE.md")).unwrap(),
        "second"
    );
    std::fs::remove_dir_all(root).unwrap();
}
