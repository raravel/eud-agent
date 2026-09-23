$ErrorActionPreference = 'Stop'
$Rest = @($args)

$schemaIndex = [Array]::IndexOf($Rest, '--json-schema')
if ($schemaIndex -ge 0) {
    $prompt = [Console]::In.ReadToEnd()
    $structured = if ($prompt -eq 'structured-semantic-over-limit') {
        @{value=('x' * 70000)}
    } else {
        @{ok=$true}
    }
    $result = @{
        type = 'result'
        subtype = 'success'
        is_error = $false
        structured_output = $structured
    }
    if ($prompt -eq 'structured-raw-overhead') {
        $result.padding = 'x' * 70000
    }
    $wire = $result | ConvertTo-Json -Compress -Depth 16
    $scenario = if ($prompt -eq 'structured-raw-overhead') {
        1
    } elseif ($prompt -eq 'structured-semantic-over-limit') {
        2
    } else {
        0
    }
    $structuredWire = $structured | ConvertTo-Json -Compress -Depth 16
    $witness = @{
        scenario = $scenario
        rawBytes = [Text.Encoding]::UTF8.GetByteCount($wire)
        paddingChars = if ($result.ContainsKey('padding')) { ([string]$result.padding).Length } else { 0 }
        structuredBytes = [Text.Encoding]::UTF8.GetByteCount($structuredWire)
    }
    [IO.File]::WriteAllText(
        (Join-Path $PSScriptRoot 'structured-size-witness.json'),
        ($witness | ConvertTo-Json -Compress)
    )
    [Console]::Out.WriteLine($wire)
    exit 0
}

$inputMessage = [Console]::In.ReadLine() | ConvertFrom-Json
$prompt = [string]$inputMessage.message.content[0].text
$wireLines = @()
$wireLines += (@{
    type = 'system'
    subtype = 'init'
    session_id = 'claude-size-session'
    tools = @('mcp__eud-tools__read_file')
    mcp_servers = @(@{name='eud-tools'; status='connected'})
} | ConvertTo-Json -Compress -Depth 16)
if ($prompt -eq 'foreground-raw-overhead') {
    $wireLines += (@{type='wire_padding'; padding=('x' * 70000)} | ConvertTo-Json -Compress -Depth 16)
}
$answer = if ($prompt -eq 'foreground-semantic-over-limit') {
    'x' * 70000
} else {
    'tiny foreground result'
}
$wireLines += (@{
    type = 'stream_event'
    event = @{
        type = 'content_block_delta'
        index = 0
        delta = @{type='text_delta'; text=$answer}
    }
} | ConvertTo-Json -Compress -Depth 16)
$wireLines += (@{
    type = 'result'
    subtype = 'success'
    session_id = 'claude-size-session'
    is_error = $false
    result = $answer
} | ConvertTo-Json -Compress -Depth 16)
$rawBytes = 0
foreach ($wireLine in $wireLines) {
    $rawBytes += [Text.Encoding]::UTF8.GetByteCount($wireLine + "`n")
}
$scenario = if ($prompt -eq 'foreground-raw-overhead') { 1 } else { 2 }
$witness = @{
    scenario = $scenario
    rawBytes = $rawBytes
    semanticBytes = [Text.Encoding]::UTF8.GetByteCount($answer)
}
[IO.File]::WriteAllText(
    (Join-Path $PSScriptRoot 'foreground-size-witness.json'),
    ($witness | ConvertTo-Json -Compress)
)
foreach ($wireLine in $wireLines) {
    [Console]::Out.WriteLine($wireLine)
}
