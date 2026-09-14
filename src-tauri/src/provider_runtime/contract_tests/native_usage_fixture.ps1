param([string]$Mode)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

if ($Mode -ne 'codex') { throw 'native usage fixture only supports Codex' }

function Write-Wire($message) {
    [Console]::Out.WriteLine(($message | ConvertTo-Json -Depth 32 -Compress))
}

function Usage($turnId, $inputTokens, $totalTokens) {
    Write-Wire @{
        jsonrpc = '2.0'
        method = 'thread/tokenUsage/updated'
        params = @{
            threadId = 'authoritative-native-thread'
            turnId = $turnId
            tokenUsage = @{
                last = @{
                    inputTokens = $inputTokens
                    cachedInputTokens = 0
                    cacheWriteInputTokens = 0
                    outputTokens = 1
                    reasoningOutputTokens = 0
                    totalTokens = $totalTokens
                }
                total = @{
                    inputTokens = $inputTokens
                    cachedInputTokens = 0
                    cacheWriteInputTokens = 0
                    outputTokens = 1
                    reasoningOutputTokens = 0
                    totalTokens = $totalTokens
                }
                modelContextWindow = 1000
            }
        }
    }
}

while (($line = [Console]::In.ReadLine()) -ne $null) {
    $message = $line | ConvertFrom-Json
    switch ($message.method) {
        'initialize' {
            Write-Wire @{jsonrpc='2.0'; id=$message.id; result=@{protocolVersion=1}}
        }
        'windowsSandbox/readiness' {
            Write-Wire @{jsonrpc='2.0'; id=$message.id; result=@{status='ready'}}
        }
        'initialized' {}
        'thread/resume' {
            Write-Wire @{jsonrpc='2.0'; id=$message.id; result=@{}}
            Usage 'prior-native-turn' 100 101
        }
        'turn/start' {
            $startedTurn = @{id='current-native-turn'; items=@(); status='inProgress'}
            Write-Wire @{jsonrpc='2.0'; id=$message.id; result=@{turn=$startedTurn}}
            Write-Wire @{jsonrpc='2.0'; method='turn/started'; params=@{threadId='authoritative-native-thread'; turn=$startedTurn}}
            Usage 'prior-native-turn' 200 201
            Usage 'current-native-turn' 300 301
            Write-Wire @{jsonrpc='2.0'; method='item/agentMessage/delta'; params=@{threadId='authoritative-native-thread'; turnId='current-native-turn'; itemId='answer-item'; delta='usage-correlated answer'}}
            Write-Wire @{jsonrpc='2.0'; method='turn/completed'; params=@{threadId='authoritative-native-thread'; turn=@{id='current-native-turn'; items=@(); status='completed'}}}
        }
        default {
            if ($null -ne $message.id) {
                Write-Wire @{jsonrpc='2.0'; id=$message.id; error=@{code=-32601; message="unexpected fixture method: $($message.method)"}}
                exit 1
            }
        }
    }
}
