#Requires -Version 7
<#
.SYNOPSIS
    Run the agent server standalone for browser-based panel development.

.DESCRIPTION
    Runs `server\.venv\Scripts\python.exe -m eud_agent` with dev-friendly env
    vars, so the panel can be opened in a normal browser (no editor / WebView2).
    A local temp data dir is created and exported as EUD_DATA_DIR (the editor
    would normally provide Data\agent; in dev we point at a throwaway dir).

    The FastAPI app module does not exist yet (a later task): the entry currently
    prints "server app not implemented yet" and exits non-zero. This script
    surfaces that exit code HONESTLY — it does not mask it — and returns the same
    non-zero code. It becomes fully functional when the app task lands.

.PARAMETER DataDir
    Override the dev data directory. Default: a fresh temp dir under
    $env:TEMP\eud-agent-dev.

.PARAMETER Port
    Dev port for the server (exported as EUD_PORT). Default 8765.

.EXAMPLE
    pwsh -NoProfile -File scripts\dev_run.ps1
#>
[CmdletBinding()]
param(
    [string]$DataDir,
    [int]$Port = 8765
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# Emit all output as UTF-8 (no BOM) so a subprocess capturing stdout/stderr
# decodes cleanly; PowerShell's default console encoding is OEM/CP949 here.
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$OutputEncoding = [System.Text.UTF8Encoding]::new($false)

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$ServerDir = Join-Path $RepoRoot 'server'
$venvPython = Join-Path $ServerDir '.venv\Scripts\python.exe'

function Fail([string]$msg) {
    [Console]::Error.WriteLine("ERROR: $msg")
    exit 1
}

if (-not (Test-Path -LiteralPath $venvPython -PathType Leaf)) {
    Fail ("venv python not found: '$venvPython'. Run scripts\setup_env.ps1 first " +
        "to create server\.venv.")
}

# --- dev data dir ---------------------------------------------------------
if (-not $DataDir) {
    $DataDir = Join-Path $env:TEMP 'eud-agent-dev'
}
if (-not (Test-Path -LiteralPath $DataDir -PathType Container)) {
    New-Item -ItemType Directory -Path $DataDir -Force | Out-Null
}
Write-Output "dev data dir: $DataDir"

# --- dev env vars ---------------------------------------------------------
$env:EUD_DATA_DIR = $DataDir
$env:EUD_PORT = "$Port"
$env:EUD_DEV = '1'

Write-Output "running: $venvPython -m eud_agent (EUD_DATA_DIR=$DataDir, EUD_PORT=$Port)"

# Run the server entry from the server dir so `python -m eud_agent` resolves.
# Surface its exit code verbatim — do NOT mask the current "not implemented"
# non-zero state (it becomes functional when the app task lands).
Push-Location -LiteralPath $ServerDir
try {
    & $venvPython -m eud_agent
    $code = $LASTEXITCODE
} finally {
    Pop-Location
}

if ($code -ne 0) {
    Write-Warning "server exited non-zero ($code) — surfacing it honestly."
}
exit $code
