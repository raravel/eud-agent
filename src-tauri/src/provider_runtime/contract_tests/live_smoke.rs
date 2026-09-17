//! Throwaway real-Codex verification of the project-root cwd contract (V3/V4/V5).
//!
//! Runs three live foreground turns against the configured project with the
//! user's installed Codex distribution and auth:
//! - read turn: cwd is the project root, `src/` is readable;
//! - write turn: `src/` write refused, `.eud-agent/workspace/.tmp/` write allowed;
//! - read turn again: no path duality (the CLI sees `src/...`, not a mirror).
//!
//! Deleted after the smoke run; not part of the permanent suite.

use std::{path::PathBuf, sync::Arc};

use crate::{
    config::DataDirs,
    map_candidate::CandidateStore,
    map_import::MapImportStore,
    native_runtime::NativeProjectManager,
    provider::{ModelCapabilities, ProviderConversationState, ProviderId},
    provider_runtime::{
        contract_tests::fixtures::{policy, EventCollector},
        AgentTurnInput, BindingSnapshot, ForegroundRequest, JobBase, ProviderRuntime, RunId,
        RunIdentity, RuntimeEventSink, RuntimeExecutor, WorkspaceAccess,
    },
    tool_exec::ToolServices,
    write_coordinator::ProjectWriteCoordinator,
};

fn binding(model: String) -> BindingSnapshot {
    BindingSnapshot {
        provider: ProviderId::Codex,
        model,
        reasoning: None,
        base_url: None,
        capabilities: Some(ModelCapabilities {
            tool_calls: true,
            strict_structured_output: true,
            native_compaction: true,
            ..ModelCapabilities::default()
        }),
        conversation: ProviderConversationState::Codex { thread_id: None },
    }
}

fn turn(
    session_id: &str,
    request_id: &str,
    run: u64,
    binding: BindingSnapshot,
    text: &str,
    access: WorkspaceAccess,
) -> ForegroundRequest {
    ForegroundRequest {
        identity: RunIdentity {
            session_id: session_id.to_string(),
            run_id: RunId::new(run),
            request_id: request_id.to_string(),
            session_kind: crate::session::SessionKind::Eps,
            cancellation_generation: 0,
        },
        binding,
        turn: AgentTurnInput::text(text.to_string()).with_access(access),
        checkpoint: JobBase {
            revision: 7,
            instruction_epoch: 3,
            branch: Some("probe".to_string()),
        },
        policy: policy(Some(std::time::Duration::from_secs(240))),
    }
}

async fn run_turn(
    runtime: &mut ProviderRuntime,
    request: ForegroundRequest,
    label: &str,
) -> String {
    match runtime.run_foreground(request).await {
        crate::provider_runtime::RunOutcome::Completed { text, .. } => {
            eprintln!("=== {label} completed ===\n{text}");
            text
        }
        other => panic!("{label} did not complete: {other:?}"),
    }
}

pub(super) async fn run() {
    let dirs = DataDirs::from_bases(
        PathBuf::from(std::env::var("APPDATA").expect("APPDATA")).as_path(),
        PathBuf::from(std::env::var("LOCALAPPDATA").expect("LOCALAPPDATA")).as_path(),
    );
    dirs.ensure_dirs().expect("ensure dirs");
    let config = dirs.load_config().expect("load config");
    let project_path = config.project_path.clone();
    let model = config
        .providers
        .codex
        .default_model
        .clone()
        .expect("configured codex model");
    let project = NativeProjectManager::new(dirs.clone())
        .open()
        .expect("open configured project");
    let root = project.root().to_path_buf();
    // Idempotency: remove probe files left by earlier runs before observing.
    std::fs::remove_file(root.join("src/probe-write.txt")).ok();
    std::fs::remove_file(root.join("src/probe-read.txt")).ok();
    eprintln!("probe project root: {}", root.display());

    let session_id = format!("probe-{}", uuid::Uuid::new_v4().simple());
    let candidates = CandidateStore::new(dirs.clone(), MapImportStore::new(dirs.clone()));
    let services = ToolServices::new(dirs.clone(), candidates, ProjectWriteCoordinator::silent());
    let tools = services.session(session_id.clone());
    tools
        .begin_request("probe-read-1", &project_path)
        .expect("begin probe request");
    let (cancellation, receiver) = tokio::sync::watch::channel(0);
    tools.set_cancellation(receiver);
    let events: Arc<dyn RuntimeEventSink> = Arc::new(EventCollector::default());

    let adapter = crate::provider_runtime::production_adapter(
        &binding(model.clone()).to_binding(),
        &dirs,
        root.clone(),
    )
    .expect("construct production Codex adapter");
    let mut runtime = ProviderRuntime::new(
        adapter,
        binding(model.clone()),
        dirs.clone(),
        root.clone(),
        tools.clone(),
        cancellation.subscribe(),
        events,
    )
    .expect("construct runtime");
    let read_prompt = format!(
        "Run these shell commands one by one and report each result verbatim:\n\
         1. `cd`\n\
         2. `dir project.eap`\n\
         3. `dir src`\n\
         4. `powershell -NoProfile -Command \"Get-Content -LiteralPath '{}' -TotalCount 4\"`\n\
         Do not use any tool other than shell. Answer with the four outputs.",
        project.manifest().main_file.replace('\'', "''")
    );
    let first = run_turn(
        &mut runtime,
        turn(
            &session_id,
            "probe-read-1",
            1,
            binding(model.clone()),
            &read_prompt,
            WorkspaceAccess::Read,
        ),
        "V3 read turn",
    )
    .await;
    assert!(
        first.contains("project.eap"),
        "read turn must see the manifest in cwd: {first}"
    );
    assert!(
        first.contains("survivor_hero.eps"),
        "read turn must list the real src/ tree: {first}"
    );

    // Discriminator: a READ-profile turn must not be able to write anywhere
    // under the root. If this lands on disk, sandbox enforcement is off in
    // this environment entirely (not a write-profile permission bug).
    tools
        .begin_request("probe-read-3", &project_path)
        .expect("begin probe read-enforcement request");
    let fourth = run_turn(
        &mut runtime,
        turn(
            &session_id,
            "probe-read-3",
            4,
            binding(model.clone()),
            "Run `echo probe2 > src\\probe-read.txt` and report the raw output including errors.",
            WorkspaceAccess::Read,
        ),
        "V4 read-enforcement turn",
    )
    .await;
    let read_write_landed = root.join("src/probe-read.txt").exists();
    std::fs::remove_file(root.join("src/probe-read.txt")).ok();
    eprintln!("V4 read-profile write landed on disk: {read_write_landed} ({fourth})");
    assert!(
        !read_write_landed,
        "read profile must refuse every write under the root"
    );

    let write_prompt = format!(
        "Run these shell commands one by one and report each result verbatim, including any error text:\n\
         1. `echo probe > src\\probe-write.txt`\n\
         2. `mkdir .eud-agent\\workspace\\.tmp\\{session_id} 2>nul & echo probe > .eud-agent\\workspace\\.tmp\\{session_id}\\probe.txt`\n\
         3. `type .eud-agent\\workspace\\.tmp\\{session_id}\\probe.txt`\n\
         Do not use any tool other than shell. Answer with the three outputs."
    );
    tools
        .begin_request("probe-write-1", &project_path)
        .expect("begin probe write request");
    tools
        .register_write_request("probe write turn")
        .expect("register the write lease for the probe write turn");
    let second = run_turn(
        &mut runtime,
        turn(
            &session_id,
            "probe-write-1",
            2,
            binding(model.clone()),
            &write_prompt,
            WorkspaceAccess::Write,
        ),
        "V4/V5 write turn",
    )
    .await;
    // The sandbox verdict is proven on disk: the write-profile turn ran shell
    // redirections into `src/` and into the session `.tmp/`; only the latter
    // may exist afterwards. (cmd writes refusals to stderr, which the model's
    // verbatim blocks do not always echo.)
    assert!(
        !root.join("src/probe-write.txt").exists(),
        "write profile must refuse src/ writes: the probe file landed on disk"
    );
    let tmp_probe = root.join(format!(".eud-agent/workspace/.tmp/{session_id}/probe.txt"));
    assert!(
        tmp_probe.is_file(),
        "write profile must allow .tmp writes: {second}"
    );
    std::fs::remove_file(&tmp_probe).ok();

    let duality_prompt = "Run `dir src` and `dir source` and report both results verbatim. \
        Then end your answer with exactly one line: `ANSWER: YES` if a folder named `source` \
        exists in the current directory, otherwise `ANSWER: NO`.";
    tools
        .begin_request("probe-read-2", &project_path)
        .expect("begin probe duality request");
    let third = run_turn(
        &mut runtime,
        turn(
            &session_id,
            "probe-read-2",
            3,
            binding(model.clone()),
            duality_prompt,
            WorkspaceAccess::Read,
        ),
        "V5 duality turn",
    )
    .await;
    assert!(
        third.contains("ANSWER: NO"),
        "read turn must observe no source/ mirror: {third}"
    );

    eprintln!("=== probe complete: V3/V4/V5 observed ===");
}

#[tokio::test]
#[ignore = "throwaway live Codex cwd smoke (V3/V4/V5); delete with this module"]
async fn live_codex_project_root_cwd_contract() {
    run().await;
}

/// V10: `trace_suite_run({})` discovers a temporary `src/tests/**` test in the
/// configured project and passes it, using the installed euddraft and StarCraft.
#[test]
#[ignore = "throwaway live trace-suite smoke (V10); delete with this module"]
fn live_trace_suite_discovers_and_passes_temporary_test() {
    use crate::{
        native_build::EuddraftLaunch,
        native_project::NativeSourceSnapshot,
        trace_test::{run_suite, select_persistent_tests, TraceSuiteInput, TraceTestStatus},
    };

    let dirs = DataDirs::from_bases(
        PathBuf::from(std::env::var("APPDATA").expect("APPDATA")).as_path(),
        PathBuf::from(std::env::var("LOCALAPPDATA").expect("LOCALAPPDATA")).as_path(),
    );
    dirs.ensure_dirs().expect("ensure dirs");
    let config = dirs.load_config().expect("load config");
    let project_path = PathBuf::from(config.project_path.trim_start_matches(r"\\?\"));
    let euddraft_path = std::env::var("EUD_SMOKE_EUDDRAFT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(config.euddraft_path.clone()));
    let euddraft =
        EuddraftLaunch::resolve(euddraft_path.as_path()).expect("resolve installed euddraft");
    let starcraft = PathBuf::from(r"C:\Program Files (x86)\StarCraft");
    let eds = project_path.join("build/euddraft/eud-agent.eds");
    assert!(eds.is_file(), "project EDS must exist: {}", eds.display());

    let test_dir = project_path.join("src/tests");
    std::fs::create_dir_all(&test_dir).expect("create src/tests");
    let test_file = test_dir.join("smoke-v10.tests.eps");
    std::fs::write(
        &test_file,
        "function eudAgentTestSetup() {}\nfunction eudAgentTestStep(tick) {\n    if (tick == 8) {\n        if (eudAgentAssertEq(100, 42, 42)) {\n            eudAgentPass(101);\n        }\n    }\n}",
    )
    .expect("write temporary test");
    let cleanup = || std::fs::remove_file(&test_file).ok();

    let snapshot = NativeSourceSnapshot {
        project: "smoke".to_string(),
        identity: project_path.to_string_lossy().into_owned(),
        main_file: "src/main.eps".to_string(),
        revision: "smoke".to_string(),
        files: vec![crate::native_project::NativeSourceFile {
            path: "src/tests/smoke-v10.tests.eps".to_string(),
            content: std::fs::read_to_string(&test_file).expect("read temporary test"),
            sha256: "smoke".to_string(),
        }],
    };
    let input = TraceSuiteInput {
        tests: None,
        timeout_ms: 60_000,
    };
    let selection = select_persistent_tests(&snapshot, &input).expect("discover persistent tests");
    assert_eq!(
        selection.discovered,
        vec!["src/tests/smoke-v10.tests.eps".to_string()],
        "trace_suite_run({{}}) must discover the temporary src/tests file"
    );
    let result = run_suite(
        &dirs,
        &eds,
        &euddraft,
        &starcraft,
        selection,
        input.timeout_ms,
        |phase| eprintln!("V10 phase={}", phase.as_str()),
    );
    cleanup();
    let result = result.expect("suite run must succeed");
    eprintln!(
        "{}",
        serde_json::to_string_pretty(&result).expect("serialize")
    );
    assert_eq!(result.status, TraceTestStatus::Passed, "{result:?}");
    assert_eq!(result.passed, 1);
    assert_eq!(result.tests[0].path, "src/tests/smoke-v10.tests.eps");
}

/// V8: the configured project's source snapshot is the canonical nine `.eps`
/// files only, `build_run` leaves `revision()` unchanged, and `export_e3s`
/// succeeds on the imported compat base.
#[test]
#[ignore = "throwaway live build/export smoke (V8); delete with this module"]
fn live_build_revision_stable_and_export_passes() {
    use crate::{e3s_nrbf, native_build::EuddraftLaunch, native_runtime::NativeProjectManager};

    let dirs = DataDirs::from_bases(
        PathBuf::from(std::env::var("APPDATA").expect("APPDATA")).as_path(),
        PathBuf::from(std::env::var("LOCALAPPDATA").expect("LOCALAPPDATA")).as_path(),
    );
    crate::native_build::sync_compat_assets(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/eud-editor-compat"),
        &dirs.native_assets_dir(),
    )
    .expect("sync compat assets");
    let config = dirs.load_config().expect("load config");
    let project_path = PathBuf::from(config.project_path.trim_start_matches(r"\\?\"));
    let manager = NativeProjectManager::new(dirs.clone());
    let project = manager.open().expect("open configured project");

    let snapshot = project.source_snapshot().expect("source snapshot");
    let eps: Vec<&str> = snapshot
        .files
        .iter()
        .filter(|file| file.path.ends_with(".eps"))
        .map(|file| file.path.as_str())
        .collect();
    eprintln!("V8 snapshot eps: {eps:?}");
    assert_eq!(eps.len(), 9, "canonical .eps count must be nine");
    assert!(
        snapshot
            .files
            .iter()
            .all(|file| !file.path.contains("__epspy__") && !file.path.contains("__pycache__")),
        "generated artifacts must not appear in the source snapshot"
    );

    let before = project.revision().expect("revision before build");
    let result = manager
        .build(&project_path.to_string_lossy())
        .expect("build_run");
    assert!(result.ok, "build must succeed: {:?}", result.errors);
    let after = manager
        .open()
        .expect("reopen")
        .revision()
        .expect("revision after");
    assert_eq!(before, after, "build must not change the source revision");

    let destination = std::env::temp_dir().join(format!(
        "eud-agent-v8-export-{}.e3s",
        uuid::Uuid::new_v4().simple()
    ));
    e3s_nrbf::export_e3s(&project, &destination, &dirs.native_assets_dir())
        .expect("export_e3s must pass on the imported compat base");
    assert!(destination.is_file(), "exported E3S must exist");
    std::fs::remove_file(&destination).ok();
    let _ = EuddraftLaunch::resolve(PathBuf::from(config.euddraft_path).as_path());
}

/// V12: importing the project's own E3S rewrites `TriggerEditor.`-prefixed
/// epScript imports to src-relative form, the imported project builds, and
/// export → re-import is byte-identical for `src/**.eps`.
#[test]
#[ignore = "throwaway live E3S import rewrite smoke (V12); delete with this module"]
fn live_e3s_import_rewrite_build_and_roundtrip() {
    use crate::{e3s_nrbf, native_project::NativeProject, native_runtime::NativeProjectManager};

    let dirs = DataDirs::from_bases(
        PathBuf::from(std::env::var("APPDATA").expect("APPDATA")).as_path(),
        PathBuf::from(std::env::var("LOCALAPPDATA").expect("LOCALAPPDATA")).as_path(),
    );
    crate::native_build::sync_compat_assets(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/eud-editor-compat"),
        &dirs.native_assets_dir(),
    )
    .expect("sync compat assets");
    let config = dirs.load_config().expect("load config");
    let project_path = PathBuf::from(config.project_path.trim_start_matches(r"\\?\"));
    let source_e3s = project_path.join("compat/editor-project.e3s");
    assert!(source_e3s.is_file(), "fixture E3S must exist");

    let import_root = std::env::temp_dir().join(format!(
        "eud-agent-v12-import-{}",
        uuid::Uuid::new_v4().simple()
    ));
    let imported = e3s_nrbf::import_e3s(&source_e3s, &import_root, &dirs.native_assets_dir())
        .expect("import E3S into a fresh root");
    let cleanup = || std::fs::remove_dir_all(&import_root).ok();

    let snapshot = imported.source_snapshot().expect("imported snapshot");
    let mut prefixed = 0usize;
    for file in &snapshot.files {
        if file.path.ends_with(".eps") {
            for line in file.content.lines() {
                let trimmed = line.trim_start();
                if trimmed.starts_with("import ") && trimmed.contains("TriggerEditor.") {
                    prefixed += 1;
                }
            }
        }
    }
    eprintln!("V12 imported eps files: {}", snapshot.files.len());
    assert_eq!(
        prefixed, 0,
        "imported sources must carry no TriggerEditor.-prefixed imports"
    );

    let imported_sources: std::collections::BTreeMap<String, String> = snapshot
        .files
        .into_iter()
        .filter(|file| file.path.ends_with(".eps"))
        .map(|file| (file.path, file.content))
        .collect();

    // Build the imported copy by pointing the app config at it for the duration.
    let mut swapped = config.clone();
    swapped.project_path = import_root.to_string_lossy().into_owned();
    dirs.save_config(&swapped).expect("swap configured project");
    let restore = || {
        dirs.save_config(&config)
            .expect("restore configured project");
    };
    let manager = NativeProjectManager::new(dirs.clone());
    let build = manager.build(&import_root.to_string_lossy());
    match build {
        Ok(result) => assert!(
            result.ok,
            "imported build must succeed: {:?}",
            result.errors
        ),
        Err(error) => {
            restore();
            cleanup();
            panic!("imported build failed: {error}");
        }
    }

    let exported = std::env::temp_dir().join(format!(
        "eud-agent-v12-export-{}.e3s",
        uuid::Uuid::new_v4().simple()
    ));
    let exported_project = NativeProject::open(&import_root).expect("open imported project");
    let export = e3s_nrbf::export_e3s(&exported_project, &exported, &dirs.native_assets_dir());
    if let Err(error) = export {
        restore();
        cleanup();
        panic!("export failed: {error}");
    }
    let reimport_root = std::env::temp_dir().join(format!(
        "eud-agent-v12-reimport-{}",
        uuid::Uuid::new_v4().simple()
    ));
    let reimported = e3s_nrbf::import_e3s(&exported, &reimport_root, &dirs.native_assets_dir());
    let reimported = match reimported {
        Ok(project) => project,
        Err(error) => {
            restore();
            cleanup();
            std::fs::remove_dir_all(&reimport_root).ok();
            panic!("re-import failed: {error}");
        }
    };
    let roundtrip: std::collections::BTreeMap<String, String> = reimported
        .source_snapshot()
        .expect("reimported snapshot")
        .files
        .into_iter()
        .filter(|file| file.path.ends_with(".eps"))
        .map(|file| (file.path, file.content))
        .collect();
    restore();
    assert_eq!(
        imported_sources, roundtrip,
        "export → re-import must be byte-identical for src/**.eps"
    );
    cleanup();
    std::fs::remove_dir_all(&reimport_root).ok();
    std::fs::remove_file(&exported).ok();
}
