[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$AppPath,
    [Parameter(Mandatory)][string]$BuildReceipt,
    [Parameter(Mandatory)][string]$E2Sample,
    [Parameter(Mandatory)][string]$Map,
    [Parameter(Mandatory)][string]$Euddraft,
    [Parameter(Mandatory)][string]$EvidenceDir,
    [string]$BrowserPath = 'C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe',
    [string]$DistRoot = (Join-Path $PSScriptRoot '..\..\panel\dist'),
    [ValidateSet('All','Browser','Native','Cleanup')][string]$Mode = 'All',
    [switch]$RemoveOwnedRoot
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
foreach ($module in @('lifecycle','assertions','fixtures','uia','native-scenarios')) {
    . (Join-Path $PSScriptRoot "e2-import-$module.ps1")
}

$root = [IO.Path]::GetFullPath((Join-Path $EvidenceDir 'owned-runtime'))
$evidence = [IO.Path]::GetFullPath($EvidenceDir)
if ($Mode -eq 'Cleanup') {
    throw 'Cleanup requires the in-memory ownership receipt from the same invocation; standalone recursive cleanup is refused'
}
foreach ($candidate in @($AppPath,$BuildReceipt,$E2Sample,$Map,$Euddraft,$EvidenceDir,$BrowserPath,$DistRoot)) {
    Assert-E2QaAbsolutePath $candidate 'Task18 input' | Out-Null
}
foreach ($candidate in @($AppPath,$BuildReceipt,$E2Sample,$Map,$Euddraft,$BrowserPath,$DistRoot)) {
    if (-not (Test-Path -LiteralPath $candidate)) { throw "Task18 input is missing: $candidate" }
}
if (-not (Test-Path -LiteralPath $evidence -PathType Container)) { throw "Registered evidence directory must exist before runtime: $evidence" }

$lifecycle = $null
$app = $null
$failure = $null
$cleanupFailure = $null
try {
    $build = Assert-E2QaBuildReceipt $BuildReceipt $AppPath $DistRoot
    [ordered]@{
        ownedRoot = $root
        evidence = $evidence
        ports = @(6173,6174) + @(6200..6379)
        processRoles = @('panel-preview','readiness-stub','visible-app','browser-fixture','owned-edge')
        cleanup = 'PID/start-time binding; hash-preserving files; empty directories only'
    } | ConvertTo-Json -Depth 6 | ForEach-Object { [IO.File]::WriteAllText((Join-Path $evidence 'ownership-plan.json'), $_, [Text.UTF8Encoding]::new($false)) }
    $lifecycle = New-E2QaLifecycle $root $evidence $build $AppPath
    $fixtures = New-E2QaFixtureSet $lifecycle $E2Sample $Map $Euddraft
    $isolated = Initialize-E2QaIsolatedState $lifecycle $Euddraft
    $inputs = [ordered]@{
        capturedUtc = [DateTime]::UtcNow.ToString('o')
        gitCommit = $build.receipt.gitCommit
        app = @{ path = $AppPath; sha256 = $build.appSha256; effectiveIdentifier = $build.receipt.build.effectiveIdentifier; installedApp = $build.receipt.installedApp }
        dist = @{ path = $DistRoot; sha256 = $build.distSha256; files = $build.distFiles }
        originals = $fixtures.originals
        isolated = $isolated
    }
    Write-E2QaJson (Join-Path $evidence 'runtime-inputs.json') $inputs 12

    $browserResult = $null
    if ($Mode -in @('All','Browser')) {
        $browserEvidence = Join-Path $evidence 'browser'
        $browserScript = Join-Path $PSScriptRoot 'e2-import-browser.mjs'
        & node $browserScript --browser $BrowserPath --dist $DistRoot --evidence $browserEvidence
        if ($LASTEXITCODE -ne 0) { throw "Controlled browser matrix failed with exit $LASTEXITCODE" }
        $browserResult = Get-Content -Raw -LiteralPath (Join-Path $browserEvidence 'index.json') | ConvertFrom-Json
    }

    $nativeResult = $null
    if ($Mode -in @('All','Native')) {
        Start-E2QaPreview $lifecycle ([IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\panel')))
        Start-E2QaReadinessStub $lifecycle
        $app = Start-E2QaApp $lifecycle $AppPath
        $nativeResult = Invoke-E2QaNativeMatrix $lifecycle $app $fixtures $AppPath $Euddraft
    }

    Assert-E2QaProtectedHashes ([pscustomobject]$fixtures.originals)
    $index = [ordered]@{
        status = 'passed'
        source = $inputs
        browser = $browserResult
        native = $nativeResult
        limitations = @('Browser evidence covers controlled transport/layout only; native rows require the Tauri run above.')
    }
    Write-E2QaJson (Join-Path $evidence 'index.json') $index 20
} catch {
    $failure = $_
    $nativeFailureDiagnostic = $null; $appEligibilityError = $null; $appEligible = $false
    try { if ($null -ne $app) { $appEligible = -not $app.HasExited } } catch { $appEligibilityError = $_.Exception.Message }
    if ($appEligible) {
        try {
            $desktop = [Windows.Automation.AutomationElement]::RootElement
            $condition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ProcessIdProperty, $app.Id)
            $ownedWindow = $desktop.FindFirst([Windows.Automation.TreeScope]::Children, $condition)
            if ($null -eq $ownedWindow -or $ownedWindow.Current.ProcessId -ne $app.Id) { throw 'No current PID-matched top-level app window for failure capture' }
            $directory = Join-Path $evidence 'native\failure'; New-Item -ItemType Directory -Path $directory -ErrorAction Stop | Out-Null
            $nativeFailureDiagnostic = [ordered]@{ processId = $app.Id; tree = $null; treeMaximum = 64; treeError = $null; screenshot = $null; screenshotError = $null; focused = $ownedWindow.Current.HasKeyboardFocus; offscreen = $ownedWindow.Current.IsOffscreen; captureScope = 'owned top-level window rectangle'; foreground = 'unobserved by current helper'; occlusion = 'unverified by CopyFromScreen' }
            try { $nativeFailureDiagnostic.tree = Join-Path $directory 'failure-uia.json'; Write-E2QaJson $nativeFailureDiagnostic.tree (Get-E2QaUiaTree -Root $ownedWindow -Maximum 64) 5 } catch { $nativeFailureDiagnostic.treeError = $_.Exception.Message }
            try { $nativeFailureDiagnostic.screenshot = Join-Path $directory 'failure.png'; Save-E2QaWindowScreenshot $ownedWindow $nativeFailureDiagnostic.screenshot } catch { $nativeFailureDiagnostic.screenshotError = $_.Exception.Message }
        } catch { $nativeFailureDiagnostic = [ordered]@{ captureError = $_.Exception.Message } }
    }
    $retained = @(); $retentionError = $null
    if ($null -ne $lifecycle) { try { $retained = Save-E2QaAvailableArtifacts $lifecycle 'failure' } catch { $retentionError = $_.Exception.Message } }
    [IO.File]::WriteAllText((Join-Path $evidence 'failure.json'), ([ordered]@{ status = 'failed'; message = $failure.Exception.Message; stack = $failure.ScriptStackTrace; nativeFailureDiagnostic = $nativeFailureDiagnostic; appEligibilityError = $appEligibilityError; retained = $retained; retentionError = $retentionError } | ConvertTo-Json -Depth 10), [Text.UTF8Encoding]::new($false))
} finally {
    if ($null -ne $lifecycle) { try { Stop-E2QaLifecycle $lifecycle -RemoveOwnedRoot:$RemoveOwnedRoot | Out-Null } catch { $cleanupFailure = $_ } }
    if ($null -ne $cleanupFailure -and $null -ne $failure) {
        $path = Join-Path $evidence 'cleanup-failure.json'; $bytes = $script:E2QaUtf8.GetBytes(([ordered]@{ message = $cleanupFailure.Exception.Message; stack = $cleanupFailure.ScriptStackTrace } | ConvertTo-Json -Depth 4))
        $stream = [IO.File]::Open($path, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None); try { $stream.Write($bytes, 0, $bytes.Length) } finally { $stream.Dispose() }
    }
}
if ($null -ne $failure) { throw $failure }
if ($null -ne $cleanupFailure) { throw $cleanupFailure }
