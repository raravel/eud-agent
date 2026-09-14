param([string]$Mode)
$null = New-Item -ItemType Directory -Force -Path $env:CODEX_HOME
Set-Content -LiteralPath (Join-Path $env:CODEX_HOME "process-$PID.started") -Value $PID
$thread = 'fixture-thread'
$turn = 'fixture-turn'
$resumed = $false
while (($line = [Console]::In.ReadLine()) -ne $null) {
    $message = $line | ConvertFrom-Json
    $method = $message.method
    if ($method -eq 'initialize') {
        [Console]::Out.WriteLine((@{jsonrpc='2.0'; id=$message.id; result=@{protocolVersion=1}} | ConvertTo-Json -Compress -Depth 8))
    } elseif ($method -eq 'windowsSandbox/readiness') {
        [Console]::Out.WriteLine((@{jsonrpc='2.0'; id=$message.id; result=@{status='ready'}} | ConvertTo-Json -Compress -Depth 8))
    } elseif ($method -eq 'thread/start') {
        [Console]::Out.WriteLine((@{jsonrpc='2.0'; id=$message.id; result=@{}} | ConvertTo-Json -Compress -Depth 8))
        [Console]::Out.WriteLine((@{jsonrpc='2.0'; method='thread/started'; params=@{thread=@{id=$thread}}} | ConvertTo-Json -Compress -Depth 8))
    } elseif ($method -eq 'thread/resume') {
        $thread = $message.params.threadId
        $resumed = $true
        [Console]::Out.WriteLine((@{jsonrpc='2.0'; id=$message.id; result=@{}} | ConvertTo-Json -Compress -Depth 8))
    } elseif ($method -eq 'turn/start') {
        if (($message.params.input[0].text -eq 'cancel') -or ($message.params.input[0].text -eq 'drop')) {
            $descendant = Start-Process powershell.exe -WindowStyle Hidden -ArgumentList @('-NoProfile', '-NonInteractive', '-Command', 'Start-Sleep -Seconds 60') -PassThru
            [IO.File]::WriteAllText((Join-Path $env:CODEX_HOME "descendant-$PID.started"), $descendant.Id.ToString())
        }
        [Console]::Out.WriteLine((@{jsonrpc='2.0'; id=$message.id; result=@{turn=@{id=$turn}}} | ConvertTo-Json -Compress -Depth 8))
        [Console]::Out.WriteLine((@{jsonrpc='2.0'; method='turn/started'; params=@{threadId=$thread; turn=@{id=$turn; items=@(); status='inProgress'}}} | ConvertTo-Json -Compress -Depth 8))
        if (($message.params.input[0].text -ne 'cancel') -and ($message.params.input[0].text -ne 'drop')) {
            [Console]::Out.WriteLine((@{jsonrpc='2.0'; method='item/reasoning/textDelta'; params=@{delta='fixture reasoning'}} | ConvertTo-Json -Compress -Depth 8))
            [Console]::Out.WriteLine((@{jsonrpc='2.0'; method='item/started'; params=@{item=@{id='fixture-call'; type='webSearch'; query='fixture'}}} | ConvertTo-Json -Compress -Depth 8))
            [Console]::Out.WriteLine((@{jsonrpc='2.0'; method='item/completed'; params=@{item=@{id='fixture-call'; type='webSearch'; query='fixture'}}} | ConvertTo-Json -Compress -Depth 8))
            $answer = if ($resumed) { 'resumed answer' } else { 'fixture answer' }
            [Console]::Out.WriteLine((@{jsonrpc='2.0'; method='item/agentMessage/delta'; params=@{delta=$answer}} | ConvertTo-Json -Compress -Depth 8))
            [Console]::Out.WriteLine((@{jsonrpc='2.0'; method='turn/completed'; params=@{turn=@{id=$turn; status='completed'}}} | ConvertTo-Json -Compress -Depth 8))
        }
    } elseif ($method -eq 'thread/compact/start') {
        [Console]::Out.WriteLine((@{jsonrpc='2.0'; id=$message.id; result=@{}} | ConvertTo-Json -Compress -Depth 8))
        [Console]::Out.WriteLine((@{jsonrpc='2.0'; method='item/started'; params=@{item=@{id='compact-1'; type='contextCompaction'}}} | ConvertTo-Json -Compress -Depth 8))
        [Console]::Out.WriteLine((@{jsonrpc='2.0'; method='item/completed'; params=@{item=@{id='compact-1'; type='contextCompaction'}}} | ConvertTo-Json -Compress -Depth 8))
    } elseif ($method -eq 'turn/interrupt') {
        Set-Content -LiteralPath (Join-Path $env:CODEX_HOME "interrupt-$PID.seen") -Value $turn
        [Console]::Out.WriteLine((@{jsonrpc='2.0'; id=$message.id; result=@{}} | ConvertTo-Json -Compress -Depth 8))
        [Console]::Out.WriteLine((@{jsonrpc='2.0'; method='turn/completed'; params=@{turn=@{id=$turn; status='interrupted'}}} | ConvertTo-Json -Compress -Depth 8))
    }
}
