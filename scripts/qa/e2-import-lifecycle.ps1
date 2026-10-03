Set-StrictMode -Version Latest
$script:E2QaUtf8 = [Text.UTF8Encoding]::new($false)

function Write-E2QaJson {
    param([Parameter(Mandatory)][string]$Path, [Parameter(Mandatory)]$Value, [int]$Depth = 12)
    [IO.File]::WriteAllText($Path, ($Value | ConvertTo-Json -Depth $Depth), $script:E2QaUtf8)
}

function Assert-E2QaAbsolutePath {
    param([Parameter(Mandatory)][string]$Path, [Parameter(Mandatory)][string]$Label)
    if (-not [IO.Path]::IsPathRooted($Path) -or $Path -match '^[A-Za-z]:[^\\/]') { throw "$Label must be absolute: $Path" }
    [IO.Path]::GetFullPath($Path)
}

function New-E2QaLifecycle {
    param(
        [Parameter(Mandatory)][string]$Root,
        [Parameter(Mandatory)][string]$EvidenceDir
    )
    $rootPath = Assert-E2QaAbsolutePath $Root 'Owned root'
    $evidencePath = Assert-E2QaAbsolutePath $EvidenceDir 'Evidence directory'
    $existing = @(Get-Process -Name 'eud-agent' -ErrorAction SilentlyContinue)
    if ($existing.Count) { throw "Refusing single-instance collision with existing eud-agent PID(s): $($existing.Id -join ',')" }
    if (Test-Path -LiteralPath $rootPath) { throw "Owned root already exists: $rootPath" }
    New-Item -ItemType Directory -Path $rootPath | Out-Null
    New-Item -ItemType Directory -Path $evidencePath -Force | Out-Null
    $marker = Join-Path $rootPath '.task18-owned.json'
    $state = [ordered]@{
        root = $rootPath
        evidence = $evidencePath
        marker = $marker
        createdUtc = [DateTime]::UtcNow.ToString('o')
        pids = [Collections.Generic.List[object]]::new()
        roots = [Collections.Generic.List[string]]::new()
        files = @{}
        ports = [Collections.Generic.List[int]]::new()
    }
    $state.roots.Add($rootPath)
    Write-E2QaJson $marker ([ordered]@{ root = $rootPath; createdUtc = $state.createdUtc })
    Register-E2QaOwnedFile $state $marker
    $state
}

function Assert-E2QaOwnedPath {
    param([Parameter(Mandatory)]$Lifecycle, [Parameter(Mandatory)][string]$Path)
    $full = [IO.Path]::GetFullPath($Path)
    $prefix = $Lifecycle.root.TrimEnd('\') + '\'
    if ($full -ne $Lifecycle.root -and -not $full.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) { throw "Path escaped owned root: $full" }
    $cursor = $Lifecycle.root
    if ((Get-Item -LiteralPath $cursor -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Owned root is a reparse point: $cursor" }
    foreach ($part in $full.Substring($Lifecycle.root.Length).TrimStart('\').Split('\', [StringSplitOptions]::RemoveEmptyEntries)) {
        $cursor = Join-Path $cursor $part
        if ((Test-Path -LiteralPath $cursor) -and ((Get-Item -LiteralPath $cursor -Force).Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw "Refusing reparse traversal: $cursor" }
    }
    $full
}

function Register-E2QaOwnedFile {
    param([Parameter(Mandatory)]$Lifecycle, [Parameter(Mandatory)][string]$Path)
    $full = Assert-E2QaOwnedPath $Lifecycle $Path
    if (-not (Test-Path -LiteralPath $full -PathType Leaf)) { throw "Owned file is missing: $full" }
    $Lifecycle.files[$full] = (Get-FileHash -Algorithm SHA256 -LiteralPath $full).Hash
}

function Register-E2QaOwnedTree {
    param([Parameter(Mandatory)]$Lifecycle, [Parameter(Mandatory)][string]$Path)
    $root = Assert-E2QaOwnedPath $Lifecycle $Path
    $queue = [Collections.Generic.Queue[string]]::new(); $queue.Enqueue($root)
    while ($queue.Count) {
        $dir = $queue.Dequeue()
        if (-not $Lifecycle.roots.Contains($dir)) { $Lifecycle.roots.Add($dir) }
        foreach ($item in Get-ChildItem -LiteralPath $dir -Force) {
            if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Refusing reparse traversal: $($item.FullName)" }
            if ($item.PSIsContainer) { $queue.Enqueue($item.FullName) } else { Register-E2QaOwnedFile $Lifecycle $item.FullName }
        }
    }
}

function Assert-E2QaBuildReceipt {
    param(
        [Parameter(Mandatory)][string]$ReceiptPath,
        [Parameter(Mandatory)][string]$AppPath,
        [Parameter(Mandatory)][string]$DistRoot
    )
    $receiptFile = Assert-E2QaAbsolutePath $ReceiptPath 'Build receipt'
    $appFile = Assert-E2QaAbsolutePath $AppPath 'App executable'
    $dist = Assert-E2QaAbsolutePath $DistRoot 'Panel dist'
    foreach ($path in @($receiptFile, $appFile, $dist)) {
        if (-not (Test-Path -LiteralPath $path)) { throw "Required build input is missing: $path" }
    }
    $receipt = Get-Content -Raw -LiteralPath $receiptFile | ConvertFrom-Json
    $repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
    $liveCommit = (& git -C $repo rev-parse HEAD).Trim()
    if ($LASTEXITCODE -ne 0 -or $receipt.gitCommit -ne $liveCommit) { throw 'Build receipt commit does not match the live worktree' }
    if ([IO.Path]::GetFullPath([string]$receipt.appPath) -ne $appFile) { throw 'Build receipt app path does not match the canonical AppPath' }
    if ([IO.Path]::GetFullPath([string]$receipt.distRoot) -ne $dist) { throw 'Build receipt dist path does not match the canonical DistRoot' }
    $status = (& git -C $repo status --porcelain=v1 --untracked-files=all) -join "`n"
    $statusSha = [Security.Cryptography.SHA256]::Create()
    try { $statusHash = ([BitConverter]::ToString($statusSha.ComputeHash([Text.Encoding]::UTF8.GetBytes($status))) -replace '-', '').ToLowerInvariant() } finally { $statusSha.Dispose() }
    if ($receipt.gitStatusSha256 -ne $statusHash) { throw 'Build receipt dirty-worktree identity is stale' }
    if (@($receipt.sourceFiles).Count -eq 0) { throw 'Build receipt lacks current source-file pins' }
    $pinned = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($row in @($receipt.sourceFiles)) {
        $relative = [string]$row.path
        if ([IO.Path]::IsPathRooted($relative)) { throw "Build receipt source pin must be relative: $relative" }
        $source = [IO.Path]::GetFullPath((Join-Path $repo $relative))
        if (-not $source.StartsWith($repo + '\', [StringComparison]::OrdinalIgnoreCase) -or (Get-FileHash -Algorithm SHA256 -LiteralPath $source).Hash.ToLowerInvariant() -ne $row.sha256) { throw "Build receipt source pin is stale: $($row.path)" }
        [void]$pinned.Add($relative.Replace('\','/'))
    }
    $dirtySources = @(& git -C $repo diff --name-only HEAD -- src-tauri panel scripts/qa) + @(& git -C $repo ls-files --others --exclude-standard -- src-tauri panel scripts/qa)
    foreach ($relative in @($dirtySources | Sort-Object -Unique)) { if (-not $pinned.Contains($relative.Replace('\','/'))) { throw "Build receipt omits dirty source pin: $relative" } }
    $appHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $appFile).Hash.ToLowerInvariant()
    if ($receipt.appSha256 -ne $appHash) { throw 'App executable does not match its source-bound build receipt' }
    $distPrefix = $dist.TrimEnd('\') + '\'
    $distRows = Get-ChildItem -LiteralPath $dist -File -Recurse | Sort-Object FullName | ForEach-Object {
        [ordered]@{ path = $_.FullName.Substring($distPrefix.Length).Replace('\','/'); sha256 = (Get-FileHash $_.FullName).Hash.ToLowerInvariant() }
    }
    $distJson = $distRows | ConvertTo-Json -Compress
    $sha = [Security.Cryptography.SHA256]::Create()
    try { $distHash = ([BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($distJson))) -replace '-', '').ToLowerInvariant() } finally { $sha.Dispose() }
    if ($receipt.distSha256 -ne $distHash) { throw 'Panel dist does not match its source-bound build receipt' }
    [ordered]@{ receipt = $receipt; appSha256 = $appHash; distSha256 = $distHash; distFiles = $distRows; gitStatusSha256 = $statusHash }
}

function Add-E2QaOwnedProcess {
    param([Parameter(Mandatory)]$Lifecycle, [Parameter(Mandatory)][Diagnostics.Process]$Process, [Parameter(Mandatory)][string]$Role)
    $Lifecycle.pids.Add([ordered]@{ id = $Process.Id; role = $Role; startedUtc = $Process.StartTime.ToUniversalTime().ToString('o') })
    $Process
}

function Wait-E2QaCondition {
    param([Parameter(Mandatory)][scriptblock]$Condition, [int]$TimeoutSeconds = 30, [string]$Failure = 'condition timed out')
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    do {
        $value = & $Condition
        if ($value) { return $value }
        Start-Sleep -Milliseconds 125
    } while ([DateTime]::UtcNow -lt $deadline)
    throw $Failure
}

function Start-E2QaPreview {
    param([Parameter(Mandatory)]$Lifecycle, [Parameter(Mandatory)][string]$PanelRoot)
    $vite = Join-Path $PanelRoot 'node_modules\vite\bin\vite.js'
    if (-not (Test-Path -LiteralPath $vite)) { throw "Vite is missing: $vite" }
    if (Get-NetTCPConnection -LocalPort 6173 -State Listen -ErrorAction SilentlyContinue) { throw 'Preview port 6173 is already owned' }
    $node = (Get-Command node.exe -CommandType Application -ErrorAction Stop).Source
    $stdout = Join-Path $Lifecycle.evidence 'preview.stdout.log'
    $stderr = Join-Path $Lifecycle.evidence 'preview.stderr.log'
    $process = Start-Process -FilePath $node -ArgumentList @($vite,'preview','--host','127.0.0.1','--port','6173','--strictPort') -WorkingDirectory $PanelRoot -WindowStyle Hidden -PassThru -RedirectStandardOutput $stdout -RedirectStandardError $stderr
    Add-E2QaOwnedProcess $Lifecycle $process 'panel-preview' | Out-Null
    $Lifecycle.ports.Add(6173)
    Wait-E2QaCondition -TimeoutSeconds 30 -Failure 'Owned panel preview did not become ready' -Condition {
        try {
            $listener = Get-NetTCPConnection -LocalPort 6173 -State Listen -ErrorAction Stop
            $response = Invoke-WebRequest -UseBasicParsing -Uri 'http://127.0.0.1:6173/' -TimeoutSec 1
            $listener.OwningProcess -eq $process.Id -and $response.Content -eq [IO.File]::ReadAllText((Join-Path $PanelRoot 'dist\index.html'))
        } catch { $false }
    } | Out-Null
}

function Start-E2QaReadinessStub {
    param([Parameter(Mandatory)]$Lifecycle, [int]$Port = 6174)
    if (Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue) { throw "Readiness port $Port is already owned" }
    $script = @"
`$listener=[Net.HttpListener]::new();`$listener.Prefixes.Add('http://127.0.0.1:$Port/');`$listener.Start()
while(`$listener.IsListening){`$c=`$listener.GetContext();`$body=[Text.Encoding]::UTF8.GetBytes('{"data":[]}');`$c.Response.ContentType='application/json';`$c.Response.OutputStream.Write(`$body,0,`$body.Length);`$c.Response.Close()}
"@
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($script))
    $process = Start-Process -FilePath 'powershell.exe' -ArgumentList @('-NoProfile','-EncodedCommand',$encoded) -WindowStyle Hidden -PassThru
    Add-E2QaOwnedProcess $Lifecycle $process 'readiness-stub' | Out-Null
    $Lifecycle.ports.Add($Port)
    Wait-E2QaCondition -TimeoutSeconds 15 -Failure 'Readiness stub did not become ready' -Condition {
        try { (Invoke-RestMethod -Uri "http://127.0.0.1:$Port/v1/models" -TimeoutSec 1).data.Count -eq 0 } catch { $false }
    } | Out-Null
}

function Start-E2QaApp {
    param([Parameter(Mandatory)]$Lifecycle, [Parameter(Mandatory)][string]$AppPath)
    $env:EUD_AGENT_TEST_DATA_ROOT = $Lifecycle.root
    $env:WEBVIEW2_USER_DATA_FOLDER = Join-Path $Lifecycle.root 'webview2'
    $process = Start-Process -FilePath $AppPath -PassThru
    Add-E2QaOwnedProcess $Lifecycle $process 'visible-app'
}

function Stop-E2QaOwnedProcess {
    param([Parameter(Mandatory)]$Lifecycle, [Parameter(Mandatory)][Diagnostics.Process]$Process)
    $owned = @($Lifecycle.pids | Where-Object id -eq $Process.Id | Select-Object -Last 1)
    if ($owned.Count -ne 1) { throw "PID $($Process.Id) is not registered to this QA run" }
    $live = Get-Process -Id $Process.Id -ErrorAction SilentlyContinue
    if ($null -eq $live) { return }
    if ($live.StartTime.ToUniversalTime().ToString('o') -ne $owned[0].startedUtc) { throw "PID $($Process.Id) was reused" }
    Stop-Process -Id $Process.Id
    Wait-Process -Id $Process.Id -Timeout 20 -ErrorAction SilentlyContinue
    if (Get-Process -Id $Process.Id -ErrorAction SilentlyContinue) { throw "Owned PID $($Process.Id) did not exit" }
}

function Stop-E2QaLifecycle {
    param([Parameter(Mandatory)]$Lifecycle, [switch]$RemoveOwnedRoot)
    $stopped = @()
    foreach ($owned in @($Lifecycle.pids) | Sort-Object id -Descending) {
        $process = Get-Process -Id $owned.id -ErrorAction SilentlyContinue
        if ($null -eq $process) { continue }
        if ($process.StartTime.ToUniversalTime().ToString('o') -ne $owned.startedUtc) { throw "PID $($owned.id) was reused; refusing termination" }
        Stop-Process -Id $owned.id
        Wait-Process -Id $owned.id -Timeout 20 -ErrorAction SilentlyContinue
        $stopped += $owned
    }
    $remaining = @($Lifecycle.pids | Where-Object { Get-Process -Id $_.id -ErrorAction SilentlyContinue })
    $preserved = @()
    if ($RemoveOwnedRoot) {
        $marker = Get-Content -Raw -LiteralPath $Lifecycle.marker | ConvertFrom-Json
        if ($marker.root -ne $Lifecycle.root) { throw 'Owned-root marker changed; preserving root' }
        Assert-E2QaOwnedPath $Lifecycle $Lifecycle.root | Out-Null
        foreach ($path in @($Lifecycle.files.Keys)) {
            if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { continue }
            Assert-E2QaOwnedPath $Lifecycle $path | Out-Null
            if ((Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash -eq $Lifecycle.files[$path]) { Remove-Item -LiteralPath $path }
            else { $preserved += $path }
        }
        foreach ($directory in @($Lifecycle.roots) | Where-Object { $_ -ne $Lifecycle.root } | Sort-Object Length -Descending) {
            Assert-E2QaOwnedPath $Lifecycle $directory | Out-Null
            if ((Test-Path -LiteralPath $directory -PathType Container) -and -not (Get-ChildItem -LiteralPath $directory -Force | Select-Object -First 1)) { Remove-Item -LiteralPath $directory }
        }
        if (Test-Path -LiteralPath $Lifecycle.root -PathType Container) {
            if (-not (Get-ChildItem -LiteralPath $Lifecycle.root -Force | Select-Object -First 1)) { Remove-Item -LiteralPath $Lifecycle.root }
            else { $preserved += $Lifecycle.root }
        }
    }
    $rootToken = $Lifecycle.root
    $ownedChildren = @(Get-CimInstance Win32_Process | Where-Object { $null -ne $_.CommandLine -and $_.CommandLine.IndexOf($rootToken, [StringComparison]::OrdinalIgnoreCase) -ge 0 } | Select-Object ProcessId,Name,CommandLine)
    $listeners = @($Lifecycle.ports | Where-Object { Get-NetTCPConnection -LocalPort $_ -State Listen -ErrorAction SilentlyContinue })
    $receipt = [ordered]@{ stopped = $stopped; remaining = $remaining; ownedChildren = $ownedChildren; listeningPorts = $listeners; cleanupRequested = [bool]$RemoveOwnedRoot; rootExists = Test-Path -LiteralPath $Lifecycle.root; preserved = $preserved }
    Write-E2QaJson (Join-Path $Lifecycle.evidence 'cleanup.json') $receipt 6
    if ($remaining.Count) { throw 'Owned processes remain after cleanup' }
    if ($ownedChildren.Count) { throw 'Owned WebView/helper processes remain after cleanup' }
    if ($listeners.Count) { throw "Owned ports remain after cleanup: $($listeners -join ',')" }
    $receipt
}
