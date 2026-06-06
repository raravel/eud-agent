#Requires -Version 7
<#
.SYNOPSIS
    Build the distribution zip: runtime-minimal eud-agent + bundled RAG DB.

.DESCRIPTION
    Stages a runtime-minimal copy of the repo into a TEMP folder and zips it
    (architecture.md "Repository layout"; distribution was planned as a
    packaged release). The zip mirrors the repo layout so scripts\install.bat
    works from the extracted folder unchanged:

      eud-agent\
        bridge\ZZZ_10_agent_bridge.lua
        server\pyproject.toml, uv.lock, eud_agent\**   (no tests/.venv/spikes)
        panel\dist\**                                  (built output only)
        vendor\webview2\*.dll
        rag\chromadb_bge\**                            (bundled RAG DB copy;
                                                        install_dropin detects
                                                        it -> agent.cfg rag_db)
        scripts\  install.bat / uninstall.bat / *.ps1 (deploy set)
        README.md                                      (from README.release.md)

    The panel is rebuilt (npm run build) unless -SkipPanelBuild; staging
    happens OUTSIDE the repo (rules.md: never import chromadb into the repo —
    the bundle is a copy in TEMP/zip only).

.PARAMETER OutDir
    Where the zip is written. Default: <repo>\release (gitignored).

.PARAMETER RagDb
    Source RAG DB folder to bundle. Default: the ECA chromadb_bge store.

.PARAMETER SkipPanelBuild
    Use the existing panel\dist as-is instead of rebuilding.

.EXAMPLE
    pwsh -NoProfile -File scripts\package_release.ps1
    pwsh -NoProfile -File scripts\package_release.ps1 -SkipPanelBuild
#>
[CmdletBinding()]
param(
    [string]$OutDir = '',
    [string]$RagDb = 'C:\Users\ifthe\proj\eud\ECA\chromadb_bge',
    [switch]$SkipPanelBuild
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# Emit all output as UTF-8 (no BOM) so a subprocess capturing stdout/stderr
# decodes cleanly; PowerShell's default console encoding is OEM/CP949 here.
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$OutputEncoding = [System.Text.UTF8Encoding]::new($false)

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
if (-not $OutDir) { $OutDir = Join-Path $RepoRoot 'release' }

function Fail([string]$msg) {
    [Console]::Error.WriteLine("ERROR: $msg")
    exit 1
}

# --- validate inputs BEFORE staging ----------------------------------------
if (-not (Test-Path -LiteralPath (Join-Path $RagDb 'chroma.sqlite3') -PathType Leaf)) {
    Fail ("RAG DB '$RagDb' does not look like a chromadb store " +
        "(chroma.sqlite3 missing); pass -RagDb pointing at chromadb_bge.")
}

$panelDir = Join-Path $RepoRoot 'panel'
if (-not $SkipPanelBuild) {
    $npm = Get-Command npm -ErrorAction SilentlyContinue
    if (-not $npm) {
        Fail "npm not found on PATH; install Node.js or pass -SkipPanelBuild to use the existing panel\dist."
    }
    Write-Output 'building panel (npm run build) ...'
    Push-Location -LiteralPath $panelDir
    try {
        & npm run build
        if ($LASTEXITCODE -ne 0) { Fail "panel build failed (exit $LASTEXITCODE)" }
    } finally {
        Pop-Location
    }
}

$distIndex = Join-Path $panelDir 'dist\index.html'
if (-not (Test-Path -LiteralPath $distIndex -PathType Leaf)) {
    Fail "panel built output missing: '$distIndex' (run npm run build in panel\)"
}

# --- stage into TEMP (never inside the repo) -------------------------------
$stageRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("eud-agent-pkg-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
$stage = Join-Path $stageRoot 'eud-agent'
New-Item -ItemType Directory -Path $stage -Force | Out-Null

try {
    # bridge
    New-Item -ItemType Directory -Path (Join-Path $stage 'bridge') -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $RepoRoot 'bridge\ZZZ_10_agent_bridge.lua') `
        -Destination (Join-Path $stage 'bridge') -Force

    # server: pyproject + lockfile + package source (no tests/spikes/.venv)
    New-Item -ItemType Directory -Path (Join-Path $stage 'server') -Force | Out-Null
    foreach ($f in 'pyproject.toml', 'uv.lock') {
        Copy-Item -LiteralPath (Join-Path $RepoRoot "server\$f") `
            -Destination (Join-Path $stage 'server') -Force
    }
    Copy-Item -LiteralPath (Join-Path $RepoRoot 'server\eud_agent') `
        -Destination (Join-Path $stage 'server\eud_agent') -Recurse -Force
    Get-ChildItem -LiteralPath (Join-Path $stage 'server\eud_agent') `
        -Recurse -Directory -Filter '__pycache__' |
        Remove-Item -Recurse -Force

    # panel: built output only
    New-Item -ItemType Directory -Path (Join-Path $stage 'panel') -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $panelDir 'dist') `
        -Destination (Join-Path $stage 'panel\dist') -Recurse -Force

    # vendored WebView2 DLLs
    New-Item -ItemType Directory -Path (Join-Path $stage 'vendor') -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $RepoRoot 'vendor\webview2') `
        -Destination (Join-Path $stage 'vendor\webview2') -Recurse -Force

    # bundled RAG DB -> rag\chromadb_bge (install_dropin auto-detects this)
    New-Item -ItemType Directory -Path (Join-Path $stage 'rag') -Force | Out-Null
    Copy-Item -LiteralPath $RagDb `
        -Destination (Join-Path $stage 'rag\chromadb_bge') -Recurse -Force

    # deploy scripts (the dev-only dev_run.ps1 stays out)
    New-Item -ItemType Directory -Path (Join-Path $stage 'scripts') -Force | Out-Null
    foreach ($f in 'install.bat', 'uninstall.bat', 'setup_env.ps1',
        'install_dropin.ps1', 'uninstall_dropin.ps1', 'check_prereqs.ps1') {
        Copy-Item -LiteralPath (Join-Path $RepoRoot "scripts\$f") `
            -Destination (Join-Path $stage 'scripts') -Force
    }

    # user-facing README at the zip root
    Copy-Item -LiteralPath (Join-Path $RepoRoot 'scripts\README.release.md') `
        -Destination (Join-Path $stage 'README.md') -Force

    # --- zip ----------------------------------------------------------------
    New-Item -ItemType Directory -Path $OutDir -Force | Out-Null
    $zipName = "eud-agent-{0}.zip" -f (Get-Date -Format 'yyyyMMdd')
    $zipPath = Join-Path $OutDir $zipName
    if (Test-Path -LiteralPath $zipPath) {
        try {
            Remove-Item -LiteralPath $zipPath -Force -ErrorAction Stop
        } catch {
            Fail ("cannot replace existing zip '$zipPath' - it is locked by " +
                "another process (close any Explorer/archiver window viewing " +
                "it and re-run).")
        }
    }

    Write-Output "compressing -> $zipPath ..."
    Compress-Archive -Path $stage -DestinationPath $zipPath -CompressionLevel Optimal

    $sizeMb = (Get-Item -LiteralPath $zipPath).Length / 1MB
    Write-Output ("package_release: done -> {0} ({1:N1} MB)" -f $zipPath, $sizeMb)
} finally {
    Remove-Item -LiteralPath $stageRoot -Recurse -Force -ErrorAction SilentlyContinue
}

exit 0
