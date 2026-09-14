use std::time::Duration;

use crate::{
    codex_adapter::CodexAdapter,
    codex_client::CodexLaunchConfig,
    provider::{ProviderConversationState, ProviderId},
    provider_runtime::{ProviderRuntime, RunOutcome, RuntimeExecutor},
    provider_tool_loop::RunGate,
};

use super::fixtures::{EventSummary, RuntimeFixture};

#[tokio::test]
async fn c11_codex_process_exit_during_mcp_ask_releases_wait_once() {
    // Given: a real Codex transport calls the production MCP ASK endpoint before a controlled exit.
    let fixture = RuntimeFixture::new("native-ask-exit");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let script = fixture.root.join("native-ask.ps1");
    std::fs::write(&script, SCRIPT).unwrap();
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
    let (asks, mut observed) = tokio::sync::mpsc::channel(4);
    fixture
        .tools
        .set_ask_emitter(move |event| asks.try_send(event).map_err(|error| error.to_string()));
    let mut runtime = ProviderRuntime::new(
        Box::new(adapter),
        binding.clone(),
        fixture.dirs.clone(),
        fixture.root.clone(),
        fixture.tools.clone(),
        fixture.cancellation.subscribe(),
        fixture.events.clone(),
    )
    .unwrap();
    let mut request = fixture.foreground(binding, 71_001);
    request.policy.active_deadline = Some(Duration::from_secs(30));
    let mut running = Box::pin(runtime.run_foreground(request));
    let ask = tokio::time::timeout(Duration::from_secs(15), async {
        tokio::select! {
            ask = observed.recv() => ask.expect("ASK emitter closed"),
            outcome = &mut running => panic!("run ended before ASK: {outcome:?}"),
        }
    })
    .await
    .unwrap();
    assert!(fixture.tools.pending_ask().is_some());
    let (mut exit_signal, _) = tokio::time::timeout(Duration::from_secs(15), listener.accept())
        .await
        .unwrap()
        .unwrap();

    // When: the native process exits while the user has not answered.
    tokio::io::AsyncWriteExt::write_all(&mut exit_signal, b"x")
        .await
        .unwrap();
    let terminated = tokio::time::timeout(Duration::from_secs(10), &mut running).await;
    drop(running);

    // Then: the run fails, its one ASK is removed, and no completed ASK is fabricated.
    let outcome = terminated.expect("closed native transport stranded pending ASK");
    assert!(matches!(outcome, RunOutcome::Failed(_)), "{outcome:?}");
    assert!(fixture.tools.pending_ask().is_none());
    assert!(!*fixture.tools.subscribe_ask_waiting().borrow());
    assert!(fixture
        .tools
        .answer_ask(&ask.request_id, Default::default())
        .is_err());
    assert!(observed.try_recv().is_err());
    let receipt_path = RunGate::new(
        fixture.identity(71_001),
        fixture.tools.clone(),
        crate::provider_runtime::WorkspaceAccess::Read,
        None,
    )
    .receipt_path()
    .unwrap();
    let receipt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(receipt_path).unwrap()).unwrap();
    assert_eq!(receipt["completions"], serde_json::json!([]));
    assert!(!fixture
        .events
        .snapshot()
        .iter()
        .any(|event| matches!(event, EventSummary::Finished(_, true))));
}

pub(super) const SCRIPT: &str = r#"
param([int]$Port, [string]$Tool = 'ask')
$ErrorActionPreference = 'Stop'
function Wire($value) { [Console]::Out.WriteLine(($value | ConvertTo-Json -Depth 20 -Compress)) }
function Begin-Ask($endpoint) {
    $headers = @{Accept='application/json, text/event-stream'}
    $init = @{jsonrpc='2.0'; id=1; method='initialize'; params=@{protocolVersion='2025-06-18'; capabilities=@{}; clientInfo=@{name='native-ask-fixture'; version='1'}}}
    $reply = Invoke-WebRequest -UseBasicParsing -Method Post -Uri $endpoint -Headers $headers -ContentType 'application/json' -Body ($init | ConvertTo-Json -Depth 10 -Compress) -TimeoutSec 10
    $headers['mcp-session-id'] = [string]$reply.Headers['mcp-session-id']
    if (!$headers['mcp-session-id']) { throw 'MCP initialize returned no session id' }
    $null = Invoke-WebRequest -UseBasicParsing -Method Post -Uri $endpoint -Headers $headers -ContentType 'application/json' -Body '{"jsonrpc":"2.0","method":"notifications/initialized"}' -TimeoutSec 10
    $call = @{jsonrpc='2.0'; id='native-ask'; method='tools/call'; params=@{name='ask'; arguments=@{questions=@(@{id='mode'; question='Choose'; options=@(@{label='A'},@{label='B'}); multi=$false})}}}
    if ($Tool -eq 'file_create') {
        $call = @{jsonrpc='2.0'; id='pending-mutation'; method='tools/call'; params=@{name='file_create'; arguments=@{path='src/early-completed.eps'; ftype='CUIEps'; code="const completed = 1;`n"}}}
    }
    $body = [Text.Encoding]::UTF8.GetBytes(($call | ConvertTo-Json -Depth 20 -Compress))
    $uri = [Uri]$endpoint
    $client = [Net.Sockets.TcpClient]::new($uri.Host, $uri.Port)
    $head = "POST $($uri.PathAndQuery) HTTP/1.1`r`nHost: $($uri.Authority)`r`nAccept: application/json, text/event-stream`r`nContent-Type: application/json`r`nmcp-session-id: $($headers['mcp-session-id'])`r`nContent-Length: $($body.Length)`r`n`r`n"
    $headBytes = [Text.Encoding]::ASCII.GetBytes($head)
    $client.GetStream().Write($headBytes, 0, $headBytes.Length)
    $client.GetStream().Write($body, 0, $body.Length)
    $client.GetStream().Flush()
    return $client
}
$endpoint = $null
while (($line = [Console]::In.ReadLine()) -ne $null) {
    $message = $line | ConvertFrom-Json
    switch ($message.method) {
        'initialize' { Wire @{jsonrpc='2.0'; id=$message.id; result=@{}} }
        'windowsSandbox/readiness' { Wire @{jsonrpc='2.0'; id=$message.id; result=@{status='ready'}} }
        'thread/start' {
            $endpoint = $message.params.config.mcp_servers.'eud-tools'.url
            Wire @{jsonrpc='2.0'; id=$message.id; result=@{}}
            Wire @{jsonrpc='2.0'; method='thread/started'; params=@{thread=@{id='ask-thread'}}}
        }
        'turn/start' {
            Wire @{jsonrpc='2.0'; id=$message.id; result=@{turn=@{id='ask-turn'}}}
            Wire @{jsonrpc='2.0'; method='turn/started'; params=@{threadId='ask-thread'; turn=@{id='ask-turn'; items=@(); status='inProgress'}}}
            $pendingAsk = Begin-Ask $endpoint
            $signal = [Net.Sockets.TcpClient]::new('127.0.0.1', $Port)
            $control = $signal.GetStream().ReadByte()
            $signal.Dispose()
            if ($control -eq 99) {
                Wire @{jsonrpc='2.0'; method='item/agentMessage/delta'; params=@{delta='premature success'}}
                Wire @{jsonrpc='2.0'; method='turn/completed'; params=@{turn=@{id='ask-turn'; status='completed'}}}
            } else { exit 9 }
        }
        'turn/interrupt' { Wire @{jsonrpc='2.0'; id=$message.id; result=@{}} }
    }
}
"#;
