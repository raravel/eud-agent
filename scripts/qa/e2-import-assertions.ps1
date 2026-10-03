Set-StrictMode -Version Latest

function Get-E2QaHash {
    param([Parameter(Mandatory)][string]$Path)
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "Cannot hash missing file: $Path" }
    (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
}

function Assert-E2QaEqual {
    param($Actual, $Expected, [Parameter(Mandatory)][string]$Message)
    if ($Actual -ne $Expected) { throw "$Message (expected '$Expected', found '$Actual')" }
}

function Assert-E2QaProtectedHashes {
    param([Parameter(Mandatory)]$Expected)
    foreach ($property in $Expected.PSObject.Properties) {
        Assert-E2QaEqual (Get-E2QaHash $property.Name) $property.Value "Protected file changed: $($property.Name)"
    }
}

function Get-E2QaConfigSnapshot {
    param([Parameter(Mandatory)][string]$DataRoot)
    $files = @(
        Get-ChildItem -LiteralPath $DataRoot -File -Recurse -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -match 'config|recent|settings' } | Sort-Object FullName
    )
    [ordered]@{
        files = @($files | ForEach-Object { [ordered]@{ path = $_.FullName; sha256 = Get-E2QaHash $_.FullName; text = Get-Content -Raw -LiteralPath $_.FullName } })
    }
}

function Assert-E2QaNoActivation {
    param([Parameter(Mandatory)]$Before, [Parameter(Mandatory)]$After, [Parameter(Mandatory)][string]$Scenario)
    Assert-E2QaEqual ($After | ConvertTo-Json -Depth 8 -Compress) ($Before | ConvertTo-Json -Depth 8 -Compress) "$Scenario changed config or recents"
}

function Assert-E2QaSingleActivation {
    param([Parameter(Mandatory)]$Before, [Parameter(Mandatory)]$After, [Parameter(Mandatory)][string]$ProjectPath)
    $pathKey = {
        param([string]$Value)
        if ([string]::IsNullOrWhiteSpace($Value)) { return '' }
        $full = [IO.Path]::GetFullPath($Value).TrimEnd('\')
        if ($full.StartsWith('\\?\UNC\', [StringComparison]::OrdinalIgnoreCase)) { return '\\' + $full.Substring(8) }
        if ($full.StartsWith('\\?\', [StringComparison]::OrdinalIgnoreCase)) { return $full.Substring(4) }
        $full
    }
    $projectKey = & $pathKey $ProjectPath
    $beforeConfigs = @($Before.files | ForEach-Object { try { $_.text | ConvertFrom-Json } catch { } })
    $afterConfigs = @($After.files | ForEach-Object { try { $_.text | ConvertFrom-Json } catch { } })
    $beforeActive = @($beforeConfigs | Where-Object { (& $pathKey ([string]$_.project_path)) -eq $projectKey }).Count
    $afterActive = @($afterConfigs | Where-Object { (& $pathKey ([string]$_.project_path)) -eq $projectKey }).Count
    $afterRecent = @($afterConfigs | ForEach-Object project_recents | Where-Object { (& $pathKey ([string]$_.path)) -eq $projectKey }).Count
    Assert-E2QaEqual $beforeActive 0 'Project was active before import'
    Assert-E2QaEqual $afterActive 1 'Successful import did not set exactly one active project'
    Assert-E2QaEqual $afterRecent 1 'Successful import did not create exactly one recent-project record'
}

function Assert-E2QaSentinel {
    param([Parameter(Mandatory)][string]$Path, [Parameter(Mandatory)][string]$Hash)
    Assert-E2QaEqual (Get-E2QaHash $Path) $Hash 'Concurrent/nonempty destination sentinel changed'
}

function Assert-E2QaPublishedProject {
    param([Parameter(Mandatory)][string]$ProjectRoot, [Parameter(Mandatory)][string[]]$RequiredPaths)
    foreach ($relative in $RequiredPaths) {
        $path = Join-Path $ProjectRoot $relative
        if (-not (Test-Path -LiteralPath $path)) { throw "Published EAP artifact missing: $relative" }
        $full = [IO.Path]::GetFullPath($path)
        if (-not $full.StartsWith(([IO.Path]::GetFullPath($ProjectRoot) + [IO.Path]::DirectorySeparatorChar), [StringComparison]::OrdinalIgnoreCase)) {
            throw "Published artifact escaped the EAP: $full"
        }
    }
}

function Assert-E2QaFreshBuild {
    param(
        [Parameter(Mandatory)][string]$OutputMap,
        [Parameter(Mandatory)][string]$BuildLog,
        [Parameter(Mandatory)][datetime]$StartedUtc
    )
    $map = Get-Item -LiteralPath $OutputMap -ErrorAction Stop
    if ($map.LastWriteTimeUtc -lt $StartedUtc -or $map.Length -eq 0) { throw 'Build output is stale or empty' }
    $logText = Get-Content -Raw -LiteralPath $BuildLog
    foreach ($marker in @('[[ freeze activated ]]','Injecting plugins','[Stage 4/3]')) {
        if ($logText.IndexOf($marker, [StringComparison]::Ordinal) -lt 0) { throw "Build log lacks strict native marker: $marker" }
    }
    if ($logText -match '(?im)^Error:\s*\S|^Errors?:\s*[1-9]|Traceback|fatal error|AttributeError|ModuleNotFoundError|UnclosedTriggerLoop') { throw 'Build log contains errors' }
    [ordered]@{ output = $map.FullName; sha256 = Get-E2QaHash $map.FullName; bytes = $map.Length; log = $BuildLog }
}

function Save-E2QaAvailableArtifacts {
    param([Parameter(Mandatory)]$Lifecycle, [Parameter(Mandatory)][string]$Label)
    $destination = Join-Path $Lifecycle.evidence "retained\$Label"
    New-Item -ItemType Directory -Path $destination -Force | Out-Null
    $queue = [Collections.Generic.Queue[string]]::new(); $queue.Enqueue($Lifecycle.root)
    $rows = [Collections.Generic.List[object]]::new()
    while ($queue.Count) {
        $directory = $queue.Dequeue()
        Assert-E2QaOwnedPath $Lifecycle $directory | Out-Null
        foreach ($item in Get-ChildItem -LiteralPath $directory -Force) {
            if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Refusing artifact traversal through reparse point: $($item.FullName)" }
            if ($item.PSIsContainer) { $queue.Enqueue($item.FullName); continue }
            if ($item.Extension -notin @('.eap','.eps','.py','.eds','.log','.scx','.json')) { continue }
            $relative = $item.FullName.Substring($Lifecycle.root.Length).TrimStart('\')
            $target = Join-Path $destination $relative
            New-Item -ItemType Directory -Path (Split-Path $target) -Force | Out-Null
            if (Test-Path -LiteralPath $target) { throw "Refusing to replace retained artifact: $target" }
            Copy-Item -LiteralPath $item.FullName -Destination $target
            $rows.Add([ordered]@{ source = $item.FullName; retained = $target; sha256 = Get-E2QaHash $target; bytes = $item.Length })
        }
    }
    Write-E2QaJson (Join-Path $destination 'receipt.json') ([ordered]@{ label = $Label; artifacts = $rows }) 8
    @($rows)
}

function Assert-E2QaPng {
    param([Parameter(Mandatory)][string]$Path, [int]$MinimumWidth = 320, [int]$MinimumHeight = 240)
    Add-Type -AssemblyName System.Drawing
    $bytes = [IO.File]::ReadAllBytes($Path)
    Assert-E2QaEqual (([BitConverter]::ToString($bytes[0..7])) -replace '-', '') '89504E470D0A1A0A' 'Screenshot is not a PNG'
    $image = [Drawing.Image]::FromFile($Path)
    try {
        if ($image.Width -lt $MinimumWidth -or $image.Height -lt $MinimumHeight) { throw "Screenshot is too small: $($image.Width)x$($image.Height)" }
        [ordered]@{ path = $Path; sha256 = Get-E2QaHash $Path; width = $image.Width; height = $image.Height }
    } finally { $image.Dispose() }
}

function Assert-E2QaUiManifest {
    param([Parameter(Mandatory)][string]$Path, [Parameter(Mandatory)][string[]]$RequiredText)
    $text = Get-Content -Raw -LiteralPath $Path
    foreach ($literal in $RequiredText) {
        if ($text.IndexOf($literal, [StringComparison]::Ordinal) -lt 0) { throw "UI evidence lacks '$literal': $Path" }
    }
}

function Write-E2QaScenarioReceipt {
    param([Parameter(Mandatory)][string]$Path, [Parameter(Mandatory)][string]$Id, [Parameter(Mandatory)][hashtable]$Observables)
    Write-E2QaJson $Path ([ordered]@{ id = $Id; capturedUtc = [DateTime]::UtcNow.ToString('o'); observables = $Observables }) 12
    if ((Get-Item -LiteralPath $Path).Length -eq 0) { throw "Empty scenario receipt: $Path" }
}
