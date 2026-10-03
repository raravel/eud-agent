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
$failure = $null
try {
    $build = Assert-E2QaBuildReceipt $BuildReceipt $AppPath $DistRoot
    [ordered]@{
        ownedRoot = $root
        evidence = $evidence
        ports = @(6173,6174) + @(6200..6379)
        processRoles = @('panel-preview','readiness-stub','visible-app','browser-fixture','owned-edge')
        cleanup = 'PID/start-time binding; hash-preserving files; empty directories only'
    } | ConvertTo-Json -Depth 6 | ForEach-Object { [IO.File]::WriteAllText((Join-Path $evidence 'ownership-plan.json'), $_, [Text.UTF8Encoding]::new($false)) }
    $lifecycle = New-E2QaLifecycle $root $evidence
    $fixtures = New-E2QaFixtureSet $lifecycle $E2Sample $Map $Euddraft
    $isolated = Initialize-E2QaIsolatedState $lifecycle $Euddraft
    $inputs = [ordered]@{
        capturedUtc = [DateTime]::UtcNow.ToString('o')
        gitCommit = $build.receipt.gitCommit
        app = @{ path = $AppPath; sha256 = $build.appSha256 }
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
    $retained = @(); $retentionError = $null
    if ($null -ne $lifecycle) { try { $retained = Save-E2QaAvailableArtifacts $lifecycle 'failure' } catch { $retentionError = $_.Exception.Message } }
    [IO.File]::WriteAllText((Join-Path $evidence 'failure.json'), ([ordered]@{ status = 'failed'; message = $failure.Exception.Message; stack = $failure.ScriptStackTrace; retained = $retained; retentionError = $retentionError } | ConvertTo-Json -Depth 8), [Text.UTF8Encoding]::new($false))
} finally {
    if ($null -ne $lifecycle) { Stop-E2QaLifecycle $lifecycle -RemoveOwnedRoot:$RemoveOwnedRoot | Out-Null }
}
if ($null -ne $failure) { throw $failure }
