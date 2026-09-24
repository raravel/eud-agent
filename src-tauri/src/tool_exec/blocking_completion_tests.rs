use super::*;
use crate::{
    native_project::{ProjectManifest, ProjectSettings, PROJECT_SCHEMA_VERSION},
    provider_runtime::{RunId, RunIdentity},
    provider_tool_loop::RunGate,
};

#[tokio::test]
async fn cancelled_mutation_future_keeps_real_file_journal_and_old_run_receipt() {
    // Given: a real project tool pauses after its mutation and journal write, before returning.
    let runtime = SessionToolRuntime::for_tests();
    let root = runtime
        .data_dirs()
        .app_data()
        .join("blocking-mutation-project");
    runtime
        .services
        .native()
        .create_project(&root, manifest())
        .unwrap();
    std::fs::create_dir_all(root.join("maps")).unwrap();
    std::fs::write(root.join("maps/source.scx"), b"fixture map").unwrap();
    runtime
        .services
        .native()
        .write_source("src/main.eps", "function onPluginStart() {}\n")
        .unwrap();
    let (cancel, cancellation) = tokio::sync::watch::channel(1_u64);
    runtime.set_cancellation(cancellation);
    runtime.begin_request("mutation-old", "project").unwrap();
    runtime
        .execute("search_docs", &json!({"query": "파일 생성"}))
        .unwrap();
    runtime
        .register_write_request("create source fixture")
        .unwrap();
    let identity = RunIdentity {
        session_id: runtime.session_id().to_string(),
        run_id: RunId::new(1),
        request_id: "mutation-old".to_string(),
        session_kind: runtime.kind(),
        cancellation_generation: 1,
    };
    let gate = RunGate::new(
        identity,
        runtime.clone(),
        crate::provider_runtime::WorkspaceAccess::Write,
        None,
    );
    let (reached, completed_mutation) = tokio::sync::oneshot::channel();
    let (release, released) = std::sync::mpsc::channel();
    *runtime.completion_barrier.lock() = Some(ToolCompletionBarrier { reached, released });
    let running_gate = gate.clone();
    let running = tokio::spawn(async move {
        running_gate.dispatch_native(
            Some("real-create".to_string()),
            "file_create".to_string(),
            json!({"path": "src/completed.eps", "ftype": "CUIEps", "code": "const completed = 1;\n"}),
        ).await
    });
    tokio::time::timeout(std::time::Duration::from_secs(15), completed_mutation)
        .await
        .unwrap()
        .unwrap();
    let before_cancel = std::fs::read(root.join("src/completed.eps")).unwrap();

    // When: cancellation drops the awaiting future while the admitted worker still owns its lock.
    cancel.send_replace(2);
    gate.cancel();
    running.abort();
    assert!(running.await.unwrap_err().is_cancelled());
    let rotation = runtime
        .begin_request("mutation-new", "project")
        .unwrap_err();
    assert!(rotation.contains("previously admitted tool"));
    release.send(()).unwrap();
    gate.drain(std::time::Duration::from_secs(15))
        .await
        .unwrap();

    // Then: the actual mutation, journal and exact old-run result remain without replay.
    assert_eq!(before_cancel, b"const completed = 1;\n");
    assert_eq!(
        std::fs::read(root.join("src/completed.eps")).unwrap(),
        before_cancel
    );
    let journal = JournalStore::new(runtime.data_dirs().app_data());
    let entries = journal
        .selected_entries("mutation-old", &crate::journal::DecisionIds::All)
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].tool, WriteTool::FileCreate);
    assert_eq!(
        entries[0].target,
        JournalTarget::Path {
            path: "src/completed.eps".to_string()
        }
    );
    let receipt: Value =
        serde_json::from_slice(&std::fs::read(gate.receipt_path().unwrap()).unwrap()).unwrap();
    assert_eq!(receipt["requestId"], "mutation-old");
    assert_eq!(receipt["completions"].as_array().unwrap().len(), 1);
    assert_eq!(receipt["completions"][0]["callId"], "real-create");
    assert_eq!(
        receipt["completions"][0]["result"],
        json!({"ok": true, "path": "src/completed.eps"})
    );
    assert_eq!(receipt["completions"][0]["isError"], false);
    runtime.release_write_registration().unwrap();
    runtime.begin_request("mutation-new", "project").unwrap();
    assert!(gate
        .dispatch_native(
            Some("real-create".to_string()),
            "file_create".to_string(),
            json!({"path": "src/completed.eps", "ftype": "CUIEps", "code": "duplicate"})
        )
        .await
        .is_err());
    assert_eq!(
        std::fs::read(root.join("src/completed.eps")).unwrap(),
        before_cancel
    );
    assert_eq!(
        journal
            .selected_entries("mutation-old", &crate::journal::DecisionIds::All)
            .unwrap()
            .len(),
        1
    );
}

fn manifest() -> ProjectManifest {
    ProjectManifest {
        schema_version: PROJECT_SCHEMA_VERSION,
        name: "Blocking mutation fixture".to_string(),
        source_map: "maps/source.scx".to_string(),
        output_map: "build/output.scx".to_string(),
        main_file: "src/main.eps".to_string(),
        settings: ProjectSettings::default(),
        plugins: Vec::new(),
        python_entrypoints: Vec::new(),
        python_dependencies: Vec::new(),
        python_lock: None,
        editor_compatibility: None,
    }
}
