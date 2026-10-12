Set-StrictMode -Version Latest
$script:E2QaUtilityManifest = $PSHOME + '\Modules\Microsoft.PowerShell.Utility\Microsoft.PowerShell.Utility.psd1'
Import-Module -Name $script:E2QaUtilityManifest -Force -ErrorAction Stop
$script:E2QaHashCommand = Get-Command Get-FileHash -ErrorAction Stop
if ($script:E2QaHashCommand.Module.Version -ne [Version]'3.1.0.0' -or $script:E2QaHashCommand.Module.Path -ne $script:E2QaUtilityManifest) {
    throw 'Windows PowerShell Utility 3.1.0.0 was not loaded from PSHOME'
}
$script:E2QaHashRuntime = [ordered]@{
    executable = [Diagnostics.Process]::GetCurrentProcess().MainModule.FileName
    psVersion = $PSVersionTable.PSVersion.ToString()
    psHome = $PSHOME
    psModulePath = $env:PSModulePath
    autoLoadingPreference = if (Test-Path variable:PSModuleAutoLoadingPreference) { [string]$PSModuleAutoLoadingPreference } else { '<default-unset>' }
    utilityManifest = $script:E2QaHashCommand.Module.Path
    utilityVersion = $script:E2QaHashCommand.Module.Version.ToString()
}
$script:E2QaUtf8 = [Text.UTF8Encoding]::new($false)

function Write-E2QaJson {
    param([Parameter(Mandatory)][string]$Path, [Parameter(Mandatory)]$Value, [int]$Depth = 12)
    [IO.File]::WriteAllText($Path, ($Value | ConvertTo-Json -Depth $Depth), $script:E2QaUtf8)
}

function Write-E2QaJsonNew {
    param([Parameter(Mandatory)][string]$Path, [Parameter(Mandatory)]$Value, [int]$Depth = 12)
    $bytes = $script:E2QaUtf8.GetBytes(($Value | ConvertTo-Json -Depth $Depth))
    $stream = [IO.File]::Open($Path,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None)
    try { $stream.Write($bytes,0,$bytes.Length) } finally { $stream.Dispose() }
}

function Get-E2QaPathIdentity {
    param([Parameter(Mandatory)][string]$Path)
    $item = Get-Item -LiteralPath $Path -Force
    [ordered]@{ directory=[bool]$item.PSIsContainer; creationTicks=$item.CreationTimeUtc.Ticks }
}

function Assert-E2QaAbsolutePath {
    param([Parameter(Mandatory)][string]$Path, [Parameter(Mandatory)][string]$Label)
    if (-not [IO.Path]::IsPathRooted($Path) -or $Path -match '^[A-Za-z]:[^\\/]') { throw "$Label must be absolute: $Path" }
    [IO.Path]::GetFullPath($Path)
}

$script:E2QaIdentifier = 'dev.tree-some.eud-agent.task18.qa'; $script:E2QaTauriConfig = '{"identifier":"dev.tree-some.eud-agent.task18.qa"}'; $script:E2QaTauriConfigSha256 = 'f96c17c4ae4aa393007cbd83684aee78edc76ef9e21d0564426640752836e461'

function Assert-E2QaHashBinding { param([Parameter(Mandatory)][string]$Expected,[Parameter(Mandatory)][string]$Actual,[Parameter(Mandatory)][string]$Failure);if($Expected-ne$Actual){throw $Failure} }
function Assert-E2QaIdentifierBinding {
    param([Parameter(Mandatory)]$Receipt)
    if($Receipt.build.effectiveIdentifier-ne$script:E2QaIdentifier-or$Receipt.build.tauriConfigOverride-ne$script:E2QaTauriConfig-or$Receipt.build.tauriConfigSha256-ne$script:E2QaTauriConfigSha256){throw 'Build receipt lacks the exact Task18 Tauri identifier override'}
    if($Receipt.build.cargoArtifactFresh-eq$false){return}
    $repo=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'));$priorPath=[IO.Path]::GetFullPath((Join-Path $repo '.omo\evidence\task-18-e2-project-import-v1-0-2\source-bound-native-identifier-1\build-receipt.json'));$priorSha='e63e05dd337fc2195d514c1098826d1f7fbc4a94ab316585119424a3ea05c672'
    $authorityProperty=$Receipt.build.PSObject.Properties['priorAuthority'];$authority=if($null-ne$authorityProperty){$authorityProperty.Value}else{$null}
    if($null-eq$authority-or[IO.Path]::GetFullPath([string]$authority.receiptPath)-ne$priorPath-or$authority.receiptSha256-ne$priorSha-or-not(Test-Path -LiteralPath $priorPath -PathType Leaf)){throw 'Cached Task18 build lacks the accepted physical NB1 receipt'}
    Assert-E2QaHashBinding $priorSha (Get-FileHash -Algorithm SHA256 -LiteralPath $priorPath).Hash.ToLowerInvariant() 'Accepted NB1 receipt bytes changed'
    $prior=Get-Content -Raw -LiteralPath $priorPath|ConvertFrom-Json
    if($prior.build.effectiveIdentifier-ne$script:E2QaIdentifier-or$prior.build.tauriConfigOverride-ne$script:E2QaTauriConfig-or$prior.build.tauriConfigSha256-ne$script:E2QaTauriConfigSha256-or$prior.build.cargoArtifactFresh-ne$false){throw 'Accepted NB1 receipt lacks non-fresh identifier authority'}
    if([IO.Path]::GetFullPath([string]$prior.appPath)-ne[IO.Path]::GetFullPath([string]$Receipt.appPath)-or$prior.appSha256-ne$Receipt.appSha256-or$authority.appSha256-ne$prior.appSha256){throw 'Cached Task18 app identity differs from accepted NB1'}
    $expected=@('scripts/qa/e2-import-browser.mjs','scripts/qa/e2-import-browser-fixture.mjs','scripts/qa/e2-import-desktop.ps1','scripts/qa/e2-import-lifecycle.ps1','scripts/qa/e2-import-uia.ps1');$sourcePrior=$null;$sourceCurrent=$null
    foreach($property in @('sourceFiles','qa8')){
        $priorMap=@{};$currentMap=@{}
        foreach($row in @($prior.$property)){$path=[string]$row.path;if([IO.Path]::IsPathRooted($path)){$path=[IO.Path]::GetFullPath($path).Substring($repo.Length+1)};$path=$path.Replace('\','/');if($priorMap.ContainsKey($path)){throw "Accepted NB1 has duplicate $property path: $path"};$priorMap[$path]=[string]$row.sha256}
        foreach($row in @($Receipt.$property)){$path=[string]$row.path;if([IO.Path]::IsPathRooted($path)){$path=[IO.Path]::GetFullPath($path).Substring($repo.Length+1)};$path=$path.Replace('\','/');if($currentMap.ContainsKey($path)){throw "Current receipt has duplicate $property path: $path"};$currentMap[$path]=[string]$row.sha256}
        if($priorMap.Count-ne$currentMap.Count-or@($priorMap.Keys|Where-Object{-not$currentMap.ContainsKey($_)}).Count){throw "Cached Task18 $property path set changed"}
        $changed=@($priorMap.Keys|Where-Object{$priorMap[$_]-ne$currentMap[$_]}|Sort-Object)
        if(($changed|ConvertTo-Json -Compress)-ne($expected|ConvertTo-Json -Compress)){throw "Cached Task18 $property delta is not the exact reviewed QA quintet"}
        if($property-eq'sourceFiles'){$sourcePrior=$priorMap;$sourceCurrent=$currentMap}
    }
    $delta=@{};foreach($item in @($authority.sourceDelta)){$path=[string]$item.path;if($delta.ContainsKey($path)){throw "Cached Task18 authority has duplicate source delta: $path"};$delta[$path]=$item}
    if($delta.Count-ne5){throw 'Cached Task18 authority must record five source deltas'}
    foreach($path in $expected){$item=$delta[$path];if($null-eq$item-or$item.priorSha256-ne$sourcePrior[$path]-or$item.currentSha256-ne$sourceCurrent[$path]){throw "Cached Task18 authority source hashes are inconsistent: $path"}}
}
function Get-E2QaAppProcesses { @(Get-CimInstance Win32_Process -Filter "Name='eud-agent.exe'"|Sort-Object ProcessId|ForEach-Object{$created=[DateTimeOffset]$_.CreationDate;$executable=$null;if($_.ExecutablePath){$executable=[IO.Path]::GetFullPath($_.ExecutablePath)};[ordered]@{pid=[int]$_.ProcessId;executablePath=$executable;creationEpochMs=$created.ToUnixTimeMilliseconds()}}) }
function Assert-E2QaAppProcessPolicy {
    param([Parameter(Mandatory)][array]$Processes, [Parameter(Mandatory)]$InstalledApp, [Parameter(Mandatory)][string]$AppPath, $OwnedApp = $null)
    $app = [IO.Path]::GetFullPath($AppPath); $installedCount = 0; $ownedCount = 0
    foreach ($process in $Processes) {
        $isInstalled=$process.pid-eq$InstalledApp.pid-and$process.executablePath-eq$InstalledApp.executablePath-and$process.creationEpochMs-eq$InstalledApp.creationEpochMs
        $isOwned=$null-ne$OwnedApp-and$process.pid-eq$OwnedApp.pid-and$process.executablePath-eq$OwnedApp.executablePath-and$process.creationEpochMs-eq$OwnedApp.creationEpochMs
        if ($isInstalled) { $installedCount++; continue }
        if ($isOwned) { $ownedCount++; continue }
        if ($process.executablePath -eq $app) { throw 'A canonical Task18 QA app process already exists or its PID was reused' }
        throw "Unexpected eud-agent process identity: PID $($process.pid)"
    }
    if($installedCount-ne1){throw 'Receipt-bound installed app identity is absent or changed'};if($null-eq$OwnedApp-and$ownedCount-ne0){throw 'Unexpected owned Task18 process metadata'};if($null-ne$OwnedApp-and$ownedCount-ne1){throw 'Receipt-bound Task18 QA process identity is absent or changed'}
}

function New-E2QaLifecycle {
    param([Parameter(Mandatory)][string]$Root,[Parameter(Mandatory)][string]$EvidenceDir,[Parameter(Mandatory)]$Build,[Parameter(Mandatory)][string]$AppPath)
    $rootPath = Assert-E2QaAbsolutePath $Root 'Owned root'
    $evidencePath = Assert-E2QaAbsolutePath $EvidenceDir 'Evidence directory'
    $appPath = Assert-E2QaAbsolutePath $AppPath 'Task18 QA app'
    Assert-E2QaAppProcessPolicy @(Get-E2QaAppProcesses) $Build.receipt.installedApp $appPath
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
        fileIdentities = @{}
        directories = @{}
        generatedNamespaces = [Collections.Generic.List[object]]::new()
        ports = [Collections.Generic.List[int]]::new()
        installedApp=$Build.receipt.installedApp;appPath=$appPath;appSha256=$Build.appSha256;effectiveIdentifier=$Build.receipt.build.effectiveIdentifier;ownedApp=$null;powershellHashRuntime=$script:E2QaHashRuntime
    }
    $state.roots.Add($rootPath)
    $state.directories[$rootPath] = Get-E2QaPathIdentity $rootPath
    Write-E2QaJson $marker ([ordered]@{ root = $rootPath; createdUtc = $state.createdUtc })
    Register-E2QaOwnedFile $state $marker
    foreach ($name in @('local','roaming','webview2')) { Register-E2QaGeneratedNamespace $state $name }
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
    param([Parameter(Mandatory)]$Lifecycle, [Parameter(Mandatory)][string]$Path, [switch]$Refresh)
    $full = Assert-E2QaOwnedPath $Lifecycle $Path
    if (-not (Test-Path -LiteralPath $full -PathType Leaf)) { throw "Owned file is missing: $full" }
    if ($Refresh -or -not $Lifecycle.files.ContainsKey($full)) {
        $Lifecycle.files[$full] = (Get-FileHash -Algorithm SHA256 -LiteralPath $full).Hash
        $Lifecycle.fileIdentities[$full] = Get-E2QaPathIdentity $full
    }
}

function Register-E2QaGeneratedNamespace {
    param([Parameter(Mandatory)]$Lifecycle,[Parameter(Mandatory)][ValidateSet('local','roaming','webview2')][string]$Name)
    $path = Assert-E2QaOwnedPath $Lifecycle (Join-Path $Lifecycle.root $Name)
    if (Test-Path -LiteralPath $path) { throw "Generated namespace already exists: $path" }
    [IO.Directory]::CreateDirectory($path) | Out-Null
    $identity = Get-E2QaPathIdentity $path
    $token = [Guid]::NewGuid().ToString('D')
    $marker = Join-Path $path '.task18-generated.json'
    Write-E2QaJsonNew $marker ([ordered]@{ root=$Lifecycle.root; name=$Name; token=$token })
    if (-not $Lifecycle.roots.Contains($path)) { $Lifecycle.roots.Add($path) }
    $Lifecycle.directories[$path] = $identity
    Register-E2QaOwnedFile $Lifecycle $marker
    $Lifecycle.generatedNamespaces.Add([ordered]@{ name=$Name; path=$path; marker=$marker; token=$token; identity=$identity })
}

function Register-E2QaOwnedTree {
    param([Parameter(Mandatory)]$Lifecycle, [Parameter(Mandatory)][string]$Path, [switch]$RefreshGeneratedFiles)
    $root = Assert-E2QaOwnedPath $Lifecycle $Path
    $queue = [Collections.Generic.Queue[string]]::new(); $queue.Enqueue($root)
    while ($queue.Count) {
        $dir = $queue.Dequeue()
        if (-not $Lifecycle.roots.Contains($dir)) { $Lifecycle.roots.Add($dir); $Lifecycle.directories[$dir] = Get-E2QaPathIdentity $dir }
        foreach ($item in Get-ChildItem -LiteralPath $dir -Force) {
            if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Refusing reparse traversal: $($item.FullName)" }
            if ($item.PSIsContainer) { $queue.Enqueue($item.FullName) }
            else {
                $immutableMarker = @($Lifecycle.generatedNamespaces | ForEach-Object marker) -contains $item.FullName
                Register-E2QaOwnedFile $Lifecycle $item.FullName -Refresh:($RefreshGeneratedFiles -and -not $immutableMarker)
            }
        }
    }
}

function Assert-E2QaBuildReceipt {
    param([Parameter(Mandatory)][string]$ReceiptPath,[Parameter(Mandatory)][string]$AppPath,[Parameter(Mandatory)][string]$DistRoot)
    $receiptFile = Assert-E2QaAbsolutePath $ReceiptPath 'Build receipt'
    $appFile = Assert-E2QaAbsolutePath $AppPath 'App executable'
    $dist = Assert-E2QaAbsolutePath $DistRoot 'Panel dist'
    foreach ($path in @($receiptFile, $appFile, $dist)) {
        if (-not (Test-Path -LiteralPath $path)) { throw "Required build input is missing: $path" }
    }
    $receipt = Get-Content -Raw -LiteralPath $receiptFile | ConvertFrom-Json
    Assert-E2QaIdentifierBinding $receipt;if($null-eq$receipt.installedApp-or$null-eq$receipt.installedApp.pid-or[string]::IsNullOrWhiteSpace([string]$receipt.installedApp.executablePath)-or$null-eq$receipt.installedApp.creationEpochMs){throw 'Build receipt lacks the protected installed app identity'}
    $receipt.installedApp.executablePath=[IO.Path]::GetFullPath([string]$receipt.installedApp.executablePath)
    if ($receipt.installedApp.executablePath -ne 'C:\Users\ifthe\AppData\Local\eud-agent\eud-agent.exe') { throw 'Build receipt installed app path is not the protected installation' }
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
        if (-not $source.StartsWith($repo + '\', [StringComparison]::OrdinalIgnoreCase)) { throw "Build receipt source pin escaped the repository: $($row.path)" }
        Assert-E2QaHashBinding ([string]$row.sha256) (Get-FileHash -Algorithm SHA256 -LiteralPath $source).Hash.ToLowerInvariant() "Build receipt source pin is stale: $($row.path)"
        [void]$pinned.Add($relative.Replace('\','/'))
    }
    $dirtySources = @(& git -C $repo diff --name-only HEAD -- src-tauri panel scripts/qa) + @(& git -C $repo ls-files --others --exclude-standard -- src-tauri panel scripts/qa)
    foreach ($relative in @($dirtySources | Sort-Object -Unique)) { if (-not $pinned.Contains($relative.Replace('\','/'))) { throw "Build receipt omits dirty source pin: $relative" } }
    $appHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $appFile).Hash.ToLowerInvariant()
    Assert-E2QaHashBinding ([string]$receipt.appSha256) $appHash 'App executable does not match its source-bound build receipt'
    Assert-E2QaAppProcessPolicy @(Get-E2QaAppProcesses) $receipt.installedApp $appFile
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
    Assert-E2QaHashBinding $Lifecycle.appSha256 (Get-FileHash -Algorithm SHA256 -LiteralPath $AppPath).Hash.ToLowerInvariant() 'Task18 QA app changed after receipt validation'
    $process = Start-Process -FilePath $AppPath -PassThru
    Add-E2QaOwnedProcess $Lifecycle $process 'visible-app' | Out-Null
    $ownedApp = [ordered]@{ pid = $process.Id; executablePath = [IO.Path]::GetFullPath($AppPath); creationEpochMs = ([DateTimeOffset]$process.StartTime).ToUnixTimeMilliseconds() }; $Lifecycle.ownedApp = $ownedApp
    Wait-E2QaCondition -TimeoutSeconds 15 -Failure 'Task18 QA app did not establish an isolated live process identity' -Condition {try{Assert-E2QaAppProcessPolicy @(Get-E2QaAppProcesses) $Lifecycle.installedApp $Lifecycle.appPath $ownedApp;$true}catch{$false}}|Out-Null
    $process
}

function Stop-E2QaOwnedProcess {
    param([Parameter(Mandatory)]$Lifecycle, [Parameter(Mandatory)][Diagnostics.Process]$Process)
    $owned = @($Lifecycle.pids | Where-Object id -eq $Process.Id | Select-Object -Last 1)
    if ($owned.Count -ne 1) { throw "PID $($Process.Id) is not registered to this QA run" }
    $live = Get-Process -Id $Process.Id -ErrorAction SilentlyContinue
    if ($null -eq $live) { return }
    if ($live.StartTime.ToUniversalTime().ToString('o') -ne $owned[0].startedUtc) { throw "PID $($Process.Id) was reused" }
    Stop-Process -InputObject $live
    if (-not $live.WaitForExit(20000)) { throw "Owned PID $($Process.Id) did not exit within 20 seconds" }
}

function Stop-E2QaLifecycle {
    param([Parameter(Mandatory)]$Lifecycle, [switch]$RemoveOwnedRoot)
    $stopped = @()
    foreach ($owned in @($Lifecycle.pids) | Sort-Object id -Descending) {
        $process = Get-Process -Id $owned.id -ErrorAction SilentlyContinue
        if ($null -eq $process) { continue }
        if ($process.StartTime.ToUniversalTime().ToString('o') -ne $owned.startedUtc) { throw "PID $($owned.id) was reused; refusing termination" }
        Stop-Process -InputObject $process
        if ($process.WaitForExit(20000)) { $stopped += $owned }
    }
    $remaining = @($Lifecycle.pids | Where-Object { Get-Process -Id $_.id -ErrorAction SilentlyContinue })
    $rootToken = $Lifecycle.root
    $ownedChildren = @(Get-CimInstance Win32_Process | Where-Object { $null -ne $_.CommandLine -and $_.CommandLine.IndexOf($rootToken, [StringComparison]::OrdinalIgnoreCase) -ge 0 } | Select-Object ProcessId,Name,CommandLine)
    $listeners = @($Lifecycle.ports | Where-Object { Get-NetTCPConnection -LocalPort $_ -State Listen -ErrorAction SilentlyContinue })
    $appPolicyFailure=$null;try{Assert-E2QaAppProcessPolicy @(Get-E2QaAppProcesses) $Lifecycle.installedApp $Lifecycle.appPath}catch{$appPolicyFailure=$_.Exception.Message}
    $preserved = [Collections.Generic.List[string]]::new(); $preservedReasons = [Collections.Generic.List[object]]::new()
    $preserve = { param($Path,$Reason) if (-not $preserved.Contains($Path)) { $preserved.Add($Path) }; $preservedReasons.Add([ordered]@{path=$Path;reason=$Reason}) }
    $cleanupFailure = $null
    if ($RemoveOwnedRoot -and -not $remaining.Count -and -not $ownedChildren.Count -and -not $listeners.Count -and -not $appPolicyFailure) {
        try {
            $rootMarker = Get-Content -Raw -LiteralPath $Lifecycle.marker | ConvertFrom-Json
            if ($rootMarker.root -ne $Lifecycle.root -or $rootMarker.createdUtc -ne $Lifecycle.createdUtc) { throw 'Owned-root marker changed' }
            Assert-E2QaOwnedPath $Lifecycle $Lifecycle.root | Out-Null
            foreach ($namespace in @($Lifecycle.generatedNamespaces)) {
                Assert-E2QaOwnedPath $Lifecycle $namespace.path | Out-Null
                $current = Get-E2QaPathIdentity $namespace.path
                if ($current.directory -ne $namespace.identity.directory -or $current.creationTicks -ne $namespace.identity.creationTicks) { throw "Generated namespace identity changed: $($namespace.path)" }
                $metadata = Get-Content -Raw -LiteralPath $namespace.marker | ConvertFrom-Json
                if ($metadata.root -ne $Lifecycle.root -or $metadata.name -ne $namespace.name -or $metadata.token -ne $namespace.token) { throw "Generated namespace marker changed: $($namespace.path)" }
                Register-E2QaOwnedTree $Lifecycle $namespace.path -RefreshGeneratedFiles
            }
            foreach ($path in @($Lifecycle.files.Keys) | Where-Object { $_ -ne $Lifecycle.marker }) {
                if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { continue }
                Assert-E2QaOwnedPath $Lifecycle $path | Out-Null
                $identity = Get-E2QaPathIdentity $path; $expected = $Lifecycle.fileIdentities[$path]
                if ($identity.directory -ne $expected.directory -or $identity.creationTicks -ne $expected.creationTicks) { & $preserve $path 'identity-changed'; continue }
                if ((Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash -eq $Lifecycle.files[$path]) { Remove-Item -LiteralPath $path }
                else { & $preserve $path 'content-changed' }
            }
            foreach ($directory in @($Lifecycle.roots) | Where-Object { $_ -ne $Lifecycle.root } | Sort-Object Length -Descending) {
                Assert-E2QaOwnedPath $Lifecycle $directory | Out-Null
                if (-not (Test-Path -LiteralPath $directory -PathType Container)) { continue }
                $identity = Get-E2QaPathIdentity $directory; $expected = $Lifecycle.directories[$directory]
                if ($identity.creationTicks -ne $expected.creationTicks) { & $preserve $directory 'identity-changed'; continue }
                if (-not (Get-ChildItem -LiteralPath $directory -Force | Select-Object -First 1)) { Remove-Item -LiteralPath $directory }
                else { & $preserve $directory 'not-empty' }
            }
            $residual = @(Get-ChildItem -LiteralPath $Lifecycle.root -Force | Where-Object { $_.FullName -ne $Lifecycle.marker })
            foreach ($item in $residual) { & $preserve $item.FullName 'unregistered-or-preserved-entry' }
            if (-not $residual.Count) {
                $identity = Get-E2QaPathIdentity $Lifecycle.marker; $expected = $Lifecycle.fileIdentities[$Lifecycle.marker]
                if ($identity.creationTicks -eq $expected.creationTicks -and (Get-FileHash -Algorithm SHA256 -LiteralPath $Lifecycle.marker).Hash -eq $Lifecycle.files[$Lifecycle.marker]) {
                    Remove-Item -LiteralPath $Lifecycle.marker
                    if (-not (Get-ChildItem -LiteralPath $Lifecycle.root -Force | Select-Object -First 1)) { Remove-Item -LiteralPath $Lifecycle.root }
                } else { & $preserve $Lifecycle.marker 'root-marker-changed' }
            } else { & $preserve $Lifecycle.root 'root-has-residual-entries' }
        } catch {
            $cleanupFailure = $_.Exception.Message
            & $preserve $Lifecycle.root ('cleanup-refused: ' + $cleanupFailure)
        }
    }
    $receipt = [ordered]@{ stopped = $stopped; remaining = $remaining; ownedChildren = $ownedChildren; listeningPorts = $listeners; cleanupRequested = [bool]$RemoveOwnedRoot; rootExists = Test-Path -LiteralPath $Lifecycle.root; preserved = @($preserved); preservedReasons = @($preservedReasons); cleanupFailure=$cleanupFailure; installedApp = $Lifecycle.installedApp; ownedApp = $Lifecycle.ownedApp; appPolicyFailure = $appPolicyFailure; powershellHashRuntime = $Lifecycle.powershellHashRuntime }
    Write-E2QaJson (Join-Path $Lifecycle.evidence 'cleanup.json') $receipt 6
    if ($remaining.Count) { throw 'Owned processes remain after cleanup' }
    if ($ownedChildren.Count) { throw 'Owned WebView/helper processes remain after cleanup' }
    if ($appPolicyFailure) { throw $appPolicyFailure }
    if ($listeners.Count) { throw "Owned ports remain after cleanup: $($listeners -join ',')" }
    if ($RemoveOwnedRoot -and ($receipt.rootExists -or $preserved.Count -or $cleanupFailure)) { throw 'Owned root was preserved; see cleanup receipt' }
    $receipt
}
