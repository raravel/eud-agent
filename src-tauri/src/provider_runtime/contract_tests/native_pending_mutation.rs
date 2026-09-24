use std::{sync::Arc, time::Duration};

use parking_lot::Mutex;
use serde_json::json;

use crate::{
    codex_adapter::CodexAdapter,
    codex_client::CodexLaunchConfig,
    journal::{DecisionIds, JournalError, JournalStore},
    provider::{ProviderConversationState, ProviderId},
    provider_runtime::{
        AdapterEventKind, ProviderRuntime, ProviderRuntimeError, RunOutcome, RuntimeEventSink,
        RuntimeExecutor, WorkspaceAccess,
    },
    provider_tool_loop::RunGate,
};

use super::fixtures::{EventCollector, RuntimeFixture};

const WRITE_TRANSITION_SCRIPT: &str = r#"
param([string]$Mode = 'write')
$ErrorActionPreference = 'Stop'
function Wire($value) { [Console]::Out.WriteLine(($value | ConvertTo-Json -Depth 20 -Compress)) }
function Post-Mcp($endpoint, $headers, $id, $name, $arguments) {
    $message = @{jsonrpc='2.0'; id=$id; method='tools/call'; params=@{name=$name; arguments=$arguments}}
    $response = Invoke-WebRequest -UseBasicParsing -Method Post -Uri $endpoint -Headers $headers -ContentType 'application/json' -Body ($message | ConvertTo-Json -Depth 20 -Compress) -TimeoutSec 10
    $body = [string]$response.Content
    if ([string]$response.Headers['Content-Type'] -like 'text/event-stream*') {
        return @($body -split "`r?`n" | Where-Object { $_ -match '^data:\s*\S' } | ForEach-Object { ($_ -replace '^data:\s*', '') | ConvertFrom-Json } | Where-Object { $_.id -eq $id })[0]
    }
    return $body | ConvertFrom-Json
}
$endpoint = $null
while (($line = [Console]::In.ReadLine()) -ne $null) {
    $message = $line | ConvertFrom-Json
    switch ($message.method) {
        'initialize' { Wire @{jsonrpc='2.0'; id=$message.id; result=@{protocolVersion=1}} }
        'windowsSandbox/readiness' { Wire @{jsonrpc='2.0'; id=$message.id; result=@{status='ready'}} }
        'thread/start' {
            $endpoint = $message.params.config.mcp_servers.'eud-tools'.url
            Wire @{jsonrpc='2.0'; id=$message.id; result=@{}}
            Wire @{jsonrpc='2.0'; method='thread/started'; params=@{thread=@{id='write-transition-thread'}}}
        }
        'turn/start' {
            Wire @{jsonrpc='2.0'; id=$message.id; result=@{turn=@{id='write-transition-turn'}}}
            Wire @{jsonrpc='2.0'; method='turn/started'; params=@{threadId='write-transition-thread'; turn=@{id='write-transition-turn'; items=@(); status='inProgress'}}}
            $headers = @{Accept='application/json, text/event-stream'}
            $init = Invoke-WebRequest -UseBasicParsing -Method Post -Uri $endpoint -Headers $headers -ContentType 'application/json' -Body '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"write-transition-fixture","version":"1"}}}' -TimeoutSec 10
            $headers['mcp-session-id'] = [string]$init.Headers['mcp-session-id']
            $null = Invoke-WebRequest -UseBasicParsing -Method Post -Uri $endpoint -Headers $headers -ContentType 'application/json' -Body '{"jsonrpc":"2.0","method":"notifications/initialized"}' -TimeoutSec 10
            $create = Post-Mcp $endpoint $headers 'create-directly' 'file_create' @{path='src/native-direct-write.eps'; ftype='CUIEps'; code="const native_direct_write = 1;`n"}
            if ($Mode -eq 'write') {
                if ($create.error -or $create.result.isError) { throw 'direct mutation failed' }
                $answer = 'mutation completed'
            } else {
                $answer = 'read mutation observed'
            }
            Wire @{jsonrpc='2.0'; method='item/agentMessage/delta'; params=@{delta=$answer}}
            Wire @{jsonrpc='2.0'; method='turn/completed'; params=@{turn=@{id='write-transition-turn'; status='completed'}}}
        }
        'turn/interrupt' { Wire @{jsonrpc='2.0'; id=$message.id; result=@{}} }
    }
}
"#;

fn write_transition_runtime(
    fixture: &RuntimeFixture,
    mode: &str,
) -> (crate::provider_runtime::BindingSnapshot, ProviderRuntime) {
    let script = fixture.root.join("write-transition.ps1");
    std::fs::write(&script, WRITE_TRANSITION_SCRIPT).unwrap();
    let adapter = CodexAdapter::new(
        fixture.root.clone(),
        CodexLaunchConfig {
            executable: which::which("powershell.exe").unwrap(),
            executable_args: [
                "-NoProfile".to_string(),
                "-NonInteractive".to_string(),
                "-ExecutionPolicy".to_string(),
                "Bypass".to_string(),
                "-File".to_string(),
                script.to_string_lossy().into_owned(),
                mode.to_string(),
            ]
            .into_iter()
            .map(Into::into)
            .collect(),
            profile_dir: fixture.root.join("profile"),
            large_context_models: Default::default(),
        },
    );
    let mut binding = fixture.binding("");
    binding.provider = ProviderId::Codex;
    binding.base_url = None;
    binding.conversation = ProviderConversationState::empty(ProviderId::Codex);
    let runtime = ProviderRuntime::new(
        Box::new(adapter),
        binding.clone(),
        fixture.dirs.clone(),
        fixture.root.clone(),
        fixture.tools.clone(),
        fixture.cancellation.subscribe(),
        fixture.events.clone(),
    )
    .unwrap();
    (binding, runtime)
}

struct TerminalObserver {
    events: Arc<EventCollector>,
    terminal: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    completed_call: Mutex<Option<String>>,
}

impl RuntimeEventSink for TerminalObserver {
    fn emit(&self, event: &AdapterEventKind) -> Result<(), ProviderRuntimeError> {
        self.events.emit(event)?;
        if let AdapterEventKind::Block(crate::provider_runtime::NormalizedBlock::ToolResult {
            result,
            ..
        }) = event
        {
            *self.completed_call.lock() = Some(result.id.clone());
        }
        if matches!(event, AdapterEventKind::ResponseFinished { .. }) {
            if let Some(terminal) = self.terminal.lock().take() {
                let _ = terminal.send(());
            }
        }
        Ok(())
    }
}

#[tokio::test]
async fn native_success_before_mcp_mutation_returns_preserves_unconfirmed_result() {
    // Given: a production MCP mutation pauses after changing the file and journal.
    let fixture = RuntimeFixture::new("native-premature-mutation");
    fixture
        .tools
        .execute("search_docs", &json!({"query": "파일 생성"}))
        .unwrap();
    fixture
        .tools
        .register_write_request("create source fixture")
        .unwrap();
    assert!(fixture.tools.owns_write_registration());
    let (mutated, release) = fixture.tools.pause_mutation_completion();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let script = fixture.root.join("pending-mutation.ps1");
    std::fs::write(&script, super::native_ask::SCRIPT).unwrap();
    let adapter = CodexAdapter::new(
        fixture.root.clone(),
        CodexLaunchConfig {
            executable: which::which("powershell.exe").unwrap(),
            executable_args: [
                "-NoProfile".to_string(),
                "-NonInteractive".to_string(),
                "-ExecutionPolicy".to_string(),
                "Bypass".to_string(),
                "-File".to_string(),
                script.to_string_lossy().into_owned(),
                listener.local_addr().unwrap().port().to_string(),
                "file_create".to_string(),
            ]
            .into_iter()
            .map(Into::into)
            .collect(),
            profile_dir: fixture.root.join("profile"),
            large_context_models: Default::default(),
        },
    );
    let mut binding = fixture.binding("");
    binding.provider = ProviderId::Codex;
    binding.base_url = None;
    binding.conversation = ProviderConversationState::empty(ProviderId::Codex);
    let (terminal, finished) = tokio::sync::oneshot::channel();
    let events = Arc::new(TerminalObserver {
        events: fixture.events.clone(),
        terminal: Mutex::new(Some(terminal)),
        completed_call: Mutex::new(None),
    });
    let mut runtime = ProviderRuntime::new(
        Box::new(adapter),
        binding.clone(),
        fixture.dirs.clone(),
        fixture.root.clone(),
        fixture.tools.clone(),
        fixture.cancellation.subscribe(),
        events.clone(),
    )
    .unwrap();
    let run_id = 71_002;
    let mut request = fixture.foreground(binding, run_id);
    request.turn.workspace_access = WorkspaceAccess::Write;
    request.policy.active_deadline = Some(Duration::from_secs(30));
    let mut running = Box::pin(runtime.run_foreground(request));
    tokio::time::timeout(Duration::from_secs(15), async {
        tokio::select! {
            changed = mutated => changed.expect("mutation barrier closed"),
            outcome = &mut running => panic!("run ended before actual mutation: {outcome:?}"),
        }
    })
    .await
    .unwrap();
    let path = fixture.root.join("project/src/early-completed.eps");
    assert_eq!(std::fs::read(&path).unwrap(), b"const completed = 1;\n");
    let (mut signal, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
        .await
        .unwrap()
        .unwrap();

    // When: the native provider reports success before the admitted call returns.
    tokio::io::AsyncWriteExt::write_all(&mut signal, b"c")
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        tokio::select! {
            event = finished => event.expect("terminal observer closed"),
            outcome = &mut running => panic!("run ended before native terminal: {outcome:?}"),
        }
    })
    .await
    .unwrap();
    release.send(()).unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(10), &mut running)
        .await
        .unwrap();
    drop(running);

    // Then: the answer fails while the exact completed mutation remains recoverable.
    assert!(
        matches!(
            &outcome,
            RunOutcome::Failed(ProviderRuntimeError::Protocol(error))
                if error.contains("tool call was still in flight")
        ),
        "premature native terminal was accepted: {outcome:?}"
    );
    assert!(!runtime.conversation_state().is_started());
    assert_eq!(std::fs::read(&path).unwrap(), b"const completed = 1;\n");
    let entries = JournalStore::new(fixture.dirs.app_data())
        .selected_entries(&fixture.request_id, &DecisionIds::All)
        .unwrap();
    assert_eq!(entries.len(), 1);
    let receipt_path = RunGate::new(
        fixture.identity(run_id),
        fixture.tools.clone(),
        WorkspaceAccess::Write,
        None,
    )
    .receipt_path()
    .unwrap();
    let receipt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&receipt_path).unwrap()).unwrap();
    assert_eq!(receipt["lifecycle"], "unknown");
    assert_eq!(receipt["candidateNativeId"], serde_json::Value::Null);
    assert_eq!(receipt["completions"].as_array().unwrap().len(), 1);
    assert_eq!(
        receipt["completions"][0]["callId"],
        events
            .completed_call
            .lock()
            .clone()
            .expect("published MCP completion id")
    );
    assert_eq!(
        receipt["completions"][0]["result"],
        json!({"ok": true, "path": "src/early-completed.eps"})
    );
    runtime.acknowledge_persisted().await.unwrap();
    assert!(
        receipt_path.exists(),
        "failed native answer must not acknowledge its receipt"
    );
    fixture.tools.release_write_registration().unwrap();
}

#[tokio::test]
async fn writable_native_run_executes_mutation_without_an_intent_tool() {
    // Given: a production native adapter is already executing with write access.
    let fixture = RuntimeFixture::new("native-direct-write");
    fixture
        .tools
        .execute("search_docs", &json!({"query": "file creation"}))
        .unwrap();
    fixture
        .tools
        .register_write_request("prepare writable fixture")
        .unwrap();
    let (binding, mut runtime) = write_transition_runtime(&fixture, "write");
    let run_id = 71_003;
    let mut request = fixture.foreground(binding, run_id);
    request.turn.workspace_access = WorkspaceAccess::Write;
    request.policy.active_deadline = Some(Duration::from_secs(30));

    // When: the native CLI calls the mutation directly.
    let outcome = runtime.run_foreground(request).await;

    // Then: the mutation completes without a model-facing write-intent call.
    assert!(
        matches!(outcome, RunOutcome::Completed { ref text, .. } if text == "mutation completed"),
        "direct native mutation failed: {outcome:?}"
    );
    assert_eq!(
        std::fs::read(fixture.root.join("project/src/native-direct-write.eps")).unwrap(),
        b"const native_direct_write = 1;\n"
    );
    let receipt_path = RunGate::new(
        fixture.identity(run_id),
        fixture.tools.clone(),
        WorkspaceAccess::Write,
        None,
    )
    .receipt_path()
    .unwrap();
    let receipt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&receipt_path).unwrap()).unwrap();
    let completions = receipt["completions"].as_array().unwrap();
    assert_eq!(completions.len(), 1);
    assert_eq!(completions[0]["name"], "file_create");
    assert_eq!(
        completions[0]["result"],
        json!({"ok": true, "path": "src/native-direct-write.eps"})
    );
}

#[tokio::test]
async fn read_native_run_requests_write_transition_on_first_mutation() {
    // Given: a read-only native run with no prior write registration.
    let fixture = RuntimeFixture::new("native-automatic-write-transition");
    let (binding, mut runtime) = write_transition_runtime(&fixture, "read-mutate");
    let run_id = 71_005;
    let mut request = fixture.foreground(binding, run_id);
    request.policy.active_deadline = Some(Duration::from_secs(30));

    // When: the native agent directly attempts a mutation.
    let outcome = runtime.run_foreground(request).await;

    // Then: admission is registered, the mutation is parked, and no project bytes change.
    assert_eq!(outcome, RunOutcome::WriteTransition);
    assert!(fixture.tools.owns_write_registration());
    assert!(!fixture
        .root
        .join("project/src/native-direct-write.eps")
        .exists());
    match JournalStore::new(fixture.dirs.app_data())
        .selected_entries(&fixture.request_id, &DecisionIds::All)
    {
        Ok(entries) => assert!(entries.is_empty()),
        Err(JournalError::MissingJournal { request_id }) => {
            assert_eq!(request_id, fixture.request_id)
        }
        Err(error) => panic!("unexpected journal error after parked mutation: {error}"),
    }
    let receipt_path = RunGate::new(
        fixture.identity(run_id),
        fixture.tools.clone(),
        WorkspaceAccess::Read,
        None,
    )
    .receipt_path()
    .unwrap();
    let receipt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&receipt_path).unwrap()).unwrap();
    let completions = receipt["completions"].as_array().unwrap();
    assert_eq!(completions.len(), 1);
    assert_eq!(completions[0]["name"], "file_create");
    assert_eq!(completions[0]["isError"], true);
    assert!(completions[0]["result"]
        .as_str()
        .is_some_and(|error| error.starts_with("WriteWorkspaceTransition:")));
}
