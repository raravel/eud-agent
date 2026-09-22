param([string]$Mode)
$Rest = $args
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
trap {
    [Console]::Error.WriteLine("native fixture failed at line $($_.InvocationInfo.ScriptLineNumber): $($_.FullyQualifiedErrorId)")
    exit 1
}
[IO.File]::WriteAllText((Join-Path $PSScriptRoot "process-$PID.started"), $PID.ToString())
[IO.File]::WriteAllText((Join-Path $PSScriptRoot "$Mode-args.json"), ($Rest | ConvertTo-Json -Compress))
[IO.File]::WriteAllText((Join-Path $PSScriptRoot 'provider-cwd.txt'), [Environment]::CurrentDirectory)

function Write-Wire($message) {
    [Console]::Out.WriteLine(($message | ConvertTo-Json -Depth 32 -Compress))
}

function Post-Mcp($endpoint, $headers, $message) {
    Invoke-WebRequest -UseBasicParsing -Method Post -Uri $endpoint -Headers $headers -ContentType 'application/json' -Body ($message | ConvertTo-Json -Depth 20 -Compress) -TimeoutSec 10
}

function Hold-StructuredProcess {
    $child = Start-Process powershell.exe -WindowStyle Hidden -ArgumentList @('-NoProfile', '-NonInteractive', '-Command', 'Start-Sleep -Seconds 300') -PassThru
    [IO.File]::WriteAllText((Join-Path $PSScriptRoot "process-$($child.Id).started"), $child.Id.ToString())
    [IO.File]::WriteAllText((Join-Path $PSScriptRoot 'descendant.ready'), $child.Id.ToString())
    Start-Sleep -Seconds 300
    throw 'Structured fixture was not terminated'
}

function Start-McpSession($endpoint) {
    $headers = @{ Accept='application/json, text/event-stream' }
    $init = Post-Mcp $endpoint $headers @{jsonrpc='2.0'; id=1; method='initialize'; params=@{protocolVersion='2025-06-18'; capabilities=@{}; clientInfo=@{name='native-process-fixture'; version='1'}}}
    if (!$init.Headers) { throw 'MCP initialize returned no HTTP headers' }
    $headers['mcp-session-id'] = [string]$init.Headers['mcp-session-id']
    if (!$headers['mcp-session-id']) { throw 'MCP session was not established' }
    $null = Post-Mcp $endpoint $headers @{jsonrpc='2.0'; method='notifications/initialized'}
    return $headers
}

# One native tool call over MCP with its provider wire events. Returns the
# first content text; a tool error is returned as text only when $allowError.
function Invoke-McpTool($endpoint, $headers, $index, $name, $arguments, $allowError) {
    $callId = "native-call-$index"
    if ($Mode -eq 'codex') {
        Write-Wire @{jsonrpc='2.0'; method='item/started'; params=@{item=@{id=$callId; type='mcpToolCall'; server='eud-tools'; tool=$name; arguments=$arguments}}}
    } else {
        Write-Wire @{type='stream_event'; event=@{type='content_block_start'; index=$index; content_block=@{type='tool_use'; id=$callId; name="mcp__eud-tools__$name"}}}
        Write-Wire @{type='stream_event'; event=@{type='content_block_delta'; index=$index; delta=@{type='input_json_delta'; partial_json=($arguments | ConvertTo-Json -Compress)}}}
        Write-Wire @{type='stream_event'; event=@{type='content_block_stop'; index=$index}}
    }
    $response = Post-Mcp $endpoint $headers @{jsonrpc='2.0'; id="native-mcp-request-$index"; method='tools/call'; params=@{name=$name; arguments=$arguments}}
    $body = [string]$response.Content
    if ([string]$response.Headers['Content-Type'] -like 'text/event-stream*') {
        $replies = @($body -split "`r?`n" |
            Where-Object { $_ -match '^data:\s*\S' } |
            ForEach-Object { ($_ -replace '^data:\s*', '') | ConvertFrom-Json } |
            Where-Object { $_.id -eq "native-mcp-request-$index" })
        if ($replies.Count -ne 1) { throw 'MCP stream must contain exactly one matching response' }
        $payload = $replies[0]
    } else {
        $payload = $body | ConvertFrom-Json
    }
    if ($payload.error) { throw 'MCP tool request failed' }
    if ($payload.result.isError -and -not $allowError) { throw 'MCP tool request failed' }
    if (!$payload.result.content) { throw 'MCP tool response returned no content blocks' }
    $text = [string]$payload.result.content[0].text
    if ($Mode -eq 'codex') {
        $status = if ($payload.result.isError) { 'failed' } else { 'completed' }
        Write-Wire @{jsonrpc='2.0'; method='item/completed'; params=@{item=@{id=$callId; type='mcpToolCall'; server='eud-tools'; tool=$name; result=$payload.result; status=$status}}}
    } else {
        Write-Wire @{type='user'; message=@{role='user'; content=@(@{type='tool_result'; tool_use_id=$callId; content=$text; is_error=[bool]$payload.result.isError})}}
    }
    return $text
}

function Run-McpSequence($endpoint) {
    $headers = Start-McpSession $endpoint
    $listing = Invoke-McpTool $endpoint $headers 1 'list_files' @{} $false
    if ($listing -notmatch 'src/main\.eps') { throw 'list_files result did not select the expected source' }
    $sourcePath = $Matches[0]
    $source = Invoke-McpTool $endpoint $headers 2 'read_file' @{path=$sourcePath} $false
    if ($source -notmatch 'onPluginStart') { throw 'read_file result did not contain the source' }
    return $source
}

# Delegated-run prompts: the CLI reads, then either submits its result,
# attempts a write, or ends in prose without submitting.
function Run-DelegatedSequence($endpoint, $prompt) {
    $headers = Start-McpSession $endpoint
    switch ($prompt) {
        'delegated-submit' {
            $listing = Invoke-McpTool $endpoint $headers 1 'list_files' @{} $false
            if ($listing -notmatch 'src/main\.eps') { throw 'list_files result did not select the expected source' }
            $source = Invoke-McpTool $endpoint $headers 2 'read_file' @{path=$Matches[0]} $false
            if ($source -notmatch 'onPluginStart') { throw 'read_file result did not contain the source' }
            $accepted = Invoke-McpTool $endpoint $headers 3 'submit_result' @{summary='one entry module'; files=@('src/main.eps')} $false
            if ($accepted -notmatch 'accepted') { throw 'submit_result was not accepted' }
            return 'submitted'
        }
        'delegated-submit-hang' {
            $accepted = Invoke-McpTool $endpoint $headers 1 'submit_result' @{summary='submitted early'; files=@()} $false
            if ($accepted -notmatch 'accepted') { throw 'submit_result was not accepted' }
            # The CLI keeps its turn open past the run deadline after submitting.
            Start-Sleep -Seconds 300
            throw 'Hanging delegated fixture was not terminated'
        }
        'delegated-write' {
            $refusal = Invoke-McpTool $endpoint $headers 1 'file_create' @{path='src/x.eps'; ftype='CUIEps'; code="// x`n"} $true
            if ($refusal -notmatch 'unknown tool') { throw "write was not refused as unknown: $refusal" }
            return 'wrote'
        }
        'delegated-prose' {
            $null = Invoke-McpTool $endpoint $headers 1 'list_files' @{} $false
            return 'prose only'
        }
        default { throw "unknown delegated prompt $prompt" }
    }
}

if ($Mode -eq 'claude') {
    if ($Rest -contains '--json-schema') {
        $promptIndex = [Array]::IndexOf($Rest, '-p')
        $prompt = $Rest[$promptIndex + 1]
        $result = @{type='result'; subtype='success'; is_error=$false; structured_output=@{ok=$true}}
        switch ($prompt) {
            'structured-hang' { Hold-StructuredProcess }
            'structured-truncated' { [Console]::Out.Write('{"type":"result","structured_output":'); exit 0 }
            'structured-duplicate' { Write-Wire $result }
            'structured-forbidden-tool' { Write-Wire @{type='stream_event'; event=@{type='content_block_start'; index=0; content_block=@{type='tool_use'; id='forbidden'; name='Read'}}} }
            'structured-error' { $result.subtype='error_during_execution'; $result.is_error=$true }
            'structured-wrong-schema' { $result.structured_output.ok='wrong-type' }
        }
        Write-Wire $result
        exit 0
    }
    $inputMessage = [Console]::In.ReadLine() | ConvertFrom-Json
    $nativeSession = 'native-claude-thread'
    $resumeIndex = [Array]::IndexOf($Rest, '--resume')
    if ($resumeIndex -ge 0) { $nativeSession = $Rest[$resumeIndex + 1] }
    $configIndex = [Array]::IndexOf($Rest, '--mcp-config')
    if ($configIndex -lt 0 -and $resumeIndex -ge 0 -and $inputMessage.message.content[0].text -eq '/compact') {
        [IO.File]::WriteAllText((Join-Path $PSScriptRoot 'native-compact.json'), (@{nativeId=$nativeSession;foregroundTurns=0} | ConvertTo-Json -Compress))
        Write-Wire @{type='system'; subtype='init'; session_id=$nativeSession; tools=@(); mcp_servers=@()}
        Write-Wire @{type='result'; subtype='success'; session_id=$nativeSession; is_error=$false; result='compacted'}
        exit 0
    }
    if ($configIndex -lt 0) { throw 'Claude was not granted its run MCP endpoint' }
    $endpoint = ($Rest[$configIndex + 1] | ConvertFrom-Json).mcpServers.'eud-tools'.url
    Write-Wire @{type='system'; subtype='init'; session_id=$nativeSession; tools=@('mcp__eud-tools__list_files', 'mcp__eud-tools__read_file'); mcp_servers=@(@{name='eud-tools'; status='connected'})}
    $claudePrompt = [string]$inputMessage.message.content[0].text
    if ($claudePrompt -like 'delegated-*') { $answer = Run-DelegatedSequence $endpoint $claudePrompt }
    else { $answer = Run-McpSequence $endpoint }
    Write-Wire @{type='stream_event'; event=@{type='content_block_delta'; index=3; delta=@{type='text_delta'; text=$answer}}}
    Write-Wire @{type='result'; subtype='success'; session_id=$nativeSession; is_error=$false; result=$answer}
    exit 0
}

$endpoint = $null
$nativeThread = 'native-codex-thread'
$resumed = $false
$foregroundTurns = 0
while (($line = [Console]::In.ReadLine()) -ne $null) {
    $message = $line | ConvertFrom-Json
    switch ($message.method) {
        'initialize' { Write-Wire @{jsonrpc='2.0'; id=$message.id; result=@{protocolVersion=1}} }
        'windowsSandbox/readiness' { Write-Wire @{jsonrpc='2.0'; id=$message.id; result=@{status='ready'}} }
        'thread/start' {
            $endpoint = $message.params.config.mcp_servers.'eud-tools'.url
            Write-Wire @{jsonrpc='2.0'; id=$message.id; result=@{}}
            Write-Wire @{jsonrpc='2.0'; method='thread/started'; params=@{thread=@{id=$nativeThread}}}
        }
        'thread/resume' {
            $nativeThread = $message.params.threadId
            $endpoint = $message.params.config.mcp_servers.'eud-tools'.url
            $resumed = $true
            Write-Wire @{jsonrpc='2.0'; id=$message.id; result=@{}}
        }
        'thread/compact/start' {
            if (!$resumed) { throw 'Saved thread must be resumed before native compaction' }
            [IO.File]::WriteAllText((Join-Path $PSScriptRoot 'native-compact.json'), (@{nativeId=$message.params.threadId;foregroundTurns=$foregroundTurns} | ConvertTo-Json -Compress))
            Write-Wire @{jsonrpc='2.0'; id=$message.id; result=@{}}
            Write-Wire @{jsonrpc='2.0'; method='item/started'; params=@{item=@{id='compact'; type='contextCompaction'}}}
            Write-Wire @{jsonrpc='2.0'; method='item/completed'; params=@{item=@{id='compact'; type='contextCompaction'}}}
        }
        'turn/start' {
            $foregroundTurns++
            [IO.File]::WriteAllText((Join-Path $PSScriptRoot 'codex-turn.json'), ($message.params | ConvertTo-Json -Depth 32 -Compress))
            Write-Wire @{jsonrpc='2.0'; id=$message.id; result=@{turn=@{id='native-turn'}}}
            Write-Wire @{jsonrpc='2.0'; method='turn/started'; params=@{threadId=$nativeThread; turn=@{id='native-turn'; items=@(); status='inProgress'}}}
            $prompt = [string]$message.params.input[0].text
            switch ($prompt) {
                'partial-exit' {
                    Write-Wire @{jsonrpc='2.0'; method='item/agentMessage/delta'; params=@{delta='partial-source-result'}}
                    exit 9
                }
                'structured-valid' { $answer = '{"ok":true}' }
                'structured-truncated' { $answer = '{"ok":' }
                'structured-duplicate' { $answer = '{"ok":true}{"ok":true}' }
                'structured-hang' { Hold-StructuredProcess }
                'structured-error' {
                    Write-Wire @{jsonrpc='2.0'; method='item/agentMessage/delta'; params=@{delta='{"ok":'}}
                    exit 9
                }
                'structured-wrong-schema' { $answer = '{"ok":"wrong-type"}' }
                'structured-forbidden-tool' {
                    Write-Wire @{jsonrpc='2.0'; method='item/started'; params=@{item=@{id='forbidden'; type='mcpToolCall'; tool='list_files'; arguments=@{}}}}
                    $answer = '{"ok":true}'
                }
                { $_ -like 'delegated-*' } { $answer = Run-DelegatedSequence $endpoint $prompt }
                default { $answer = Run-McpSequence $endpoint }
            }
            Write-Wire @{jsonrpc='2.0'; method='item/agentMessage/delta'; params=@{delta=$answer}}
            Write-Wire @{jsonrpc='2.0'; method='turn/completed'; params=@{turn=@{id='native-turn'; status='completed'}}}
        }
    }
}
