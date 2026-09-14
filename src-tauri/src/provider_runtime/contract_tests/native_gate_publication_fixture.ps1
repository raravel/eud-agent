param([string]$Mode)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[IO.File]::WriteAllText((Join-Path $PSScriptRoot "process-$PID.started"), $PID.ToString())

function Write-Wire($message) {
    [Console]::Out.WriteLine(($message | ConvertTo-Json -Depth 32 -Compress))
}

function Post-Mcp($endpoint, $headers, $message) {
    Invoke-WebRequest -UseBasicParsing -Method Post -Uri $endpoint -Headers $headers -ContentType 'application/json' -Body ($message | ConvertTo-Json -Depth 20 -Compress) -TimeoutSec 10
}

function Call-Gate($endpoint) {
    $headers = @{ Accept='application/json, text/event-stream' }
    $init = Post-Mcp $endpoint $headers @{jsonrpc='2.0'; id=1; method='initialize'; params=@{protocolVersion='2025-06-18'; capabilities=@{}; clientInfo=@{name='gate-policy-fixture'; version='1'}}}
    $headers['mcp-session-id'] = [string]$init.Headers['mcp-session-id']
    $null = Post-Mcp $endpoint $headers @{jsonrpc='2.0'; method='notifications/initialized'}
    $scenario = [IO.File]::ReadAllText((Join-Path $PSScriptRoot 'gate-scenario.txt')).Trim()
    if ($scenario -eq 'unknown') {
        $name = 'not_a_tool'
        $arguments = @{}
    } else {
        $name = 'read_file'
        $arguments = @{path='src/does-not-exist.eps'}
    }
    $response = Post-Mcp $endpoint $headers @{jsonrpc='2.0'; id='gate-call'; method='tools/call'; params=@{name=$name; arguments=$arguments}}
    $body = [string]$response.Content
    if ([string]$response.Headers['Content-Type'] -like 'text/event-stream*') {
        $payload = @($body -split "`r?`n" | Where-Object { $_ -match '^data:\s*\S' } | ForEach-Object { ($_ -replace '^data:\s*', '') | ConvertFrom-Json } | Where-Object { $_.id -eq 'gate-call' })[0]
    } else {
        $payload = $body | ConvertFrom-Json
    }
    if (!$payload.result.isError) { throw 'fixture expected an MCP tool error result' }
}

if ($Mode -ne 'codex') { throw 'gate policy fixture requires Codex mode' }
$endpoint = $null
while (($line = [Console]::In.ReadLine()) -ne $null) {
    $message = $line | ConvertFrom-Json
    switch ($message.method) {
        'initialize' { Write-Wire @{jsonrpc='2.0'; id=$message.id; result=@{protocolVersion=1}} }
        'windowsSandbox/readiness' { Write-Wire @{jsonrpc='2.0'; id=$message.id; result=@{status='ready'}} }
        'thread/start' {
            $endpoint = $message.params.config.mcp_servers.'eud-tools'.url
            Write-Wire @{jsonrpc='2.0'; id=$message.id; result=@{}}
            Write-Wire @{jsonrpc='2.0'; method='thread/started'; params=@{thread=@{id='gate-policy-thread'}}}
        }
        'turn/start' {
            Write-Wire @{jsonrpc='2.0'; id=$message.id; result=@{turn=@{id='gate-policy-turn'}}}
            Write-Wire @{jsonrpc='2.0'; method='turn/started'; params=@{threadId='gate-policy-thread'; turn=@{id='gate-policy-turn'; items=@(); status='inProgress'}}}
            Call-Gate $endpoint
            Write-Wire @{jsonrpc='2.0'; method='item/agentMessage/delta'; params=@{delta='fixture completed'}}
            Write-Wire @{jsonrpc='2.0'; method='turn/completed'; params=@{turn=@{id='gate-policy-turn'; status='completed'}}}
        }
        'turn/interrupt' { Write-Wire @{jsonrpc='2.0'; id=$message.id; result=@{}} }
    }
}
