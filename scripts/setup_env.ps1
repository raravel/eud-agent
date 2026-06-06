#Requires -Version 5.1
<#
.SYNOPSIS
    Create/sync the server venv (server\.venv) via uv and sanity-check it.
    Runs on Windows PowerShell 5.1 (builtin) and PowerShell 7+; keep the
    source ASCII-only (5.1 reads BOM-less files as ANSI/CP949).

.DESCRIPTION
    Checks the shared prerequisites (uv + codex via scripts\check_prereqs.ps1,
    fail fast before any work), then runs `uv sync` inside server\ (uv-managed
    venv at server\.venv per tech-stack.md), then:
      - sanity-imports the core deps through the venv python;
      - checks for the bge-m3 weights in the HF cache and WARNS (does not fail)
        about the ~4.3 GB first-query download when absent (tech-stack.md
        "Build Artifacts").

    The -Cpu switch is documented for CUDA-less machines: per server\pyproject.toml
    the torch source must be switched to the cpu index there before syncing. This
    script does NOT edit pyproject.toml; with -Cpu it prints the exact manual edit
    and the consequence (torch 2.12.0+cpu, reduced seq/batch), then continues.

.PARAMETER Cpu
    Document the CPU-only fallback (no CUDA). See server\pyproject.toml comments.

.EXAMPLE
    pwsh -NoProfile -File scripts\setup_env.ps1
    pwsh -NoProfile -File scripts\setup_env.ps1 -Cpu
#>
[CmdletBinding()]
param(
    [switch]$Cpu
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# Emit all output as UTF-8 (no BOM) so a subprocess capturing stdout/stderr
# decodes cleanly; PowerShell's default console encoding is OEM/CP949 here.
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$OutputEncoding = [System.Text.UTF8Encoding]::new($false)

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$ServerDir = Join-Path $RepoRoot 'server'

function Fail([string]$msg) {
    [Console]::Error.WriteLine("ERROR: $msg")
    exit 1
}

# --- shared prerequisite checks (uv + codex, before any work) -------------
. (Join-Path $PSScriptRoot 'check_prereqs.ps1')
$prereqFailures = @(Get-PrereqFailures -Require 'uv', 'codex')
if ($prereqFailures.Count -gt 0) {
    Fail ($prereqFailures -join "`n")
}

if (-not (Test-Path -LiteralPath (Join-Path $ServerDir 'pyproject.toml') -PathType Leaf)) {
    Fail "server\pyproject.toml not found under '$ServerDir'"
}

# --- CPU fallback guidance (no edit performed here) -----------------------
if ($Cpu) {
    Write-Warning ("CPU fallback requested. This script does not edit " +
        "pyproject.toml. For a CUDA-less machine, edit server\pyproject.toml: " +
        "under [tool.uv.sources] set torch to `index = `"pytorch-cpu`"` and " +
        "uncomment the pytorch-cpu index block, then re-run setup_env. torch " +
        "then resolves to 2.12.0+cpu (run the server with reduced seq/batch).")
}

# --- uv sync inside server\ (push/pop location) ---------------------------
Write-Output "uv sync in $ServerDir ..."
Push-Location -LiteralPath $ServerDir
try {
    & uv sync
    if ($LASTEXITCODE -ne 0) {
        Fail "uv sync failed (exit $LASTEXITCODE)"
    }
} finally {
    Pop-Location
}

# --- locate the synced venv python ----------------------------------------
$venvPython = Join-Path $ServerDir '.venv\Scripts\python.exe'
if (-not (Test-Path -LiteralPath $venvPython -PathType Leaf)) {
    Fail "uv sync completed but venv python is missing: '$venvPython'"
}
Write-Output "venv python: $venvPython"

# --- sanity import via the venv python ------------------------------------
Write-Output 'sanity import (fastapi, uvicorn) ...'
& $venvPython -c 'import fastapi, uvicorn; print("ok", fastapi.__version__, uvicorn.__version__)'
if ($LASTEXITCODE -ne 0) {
    Fail "venv sanity import failed (exit $LASTEXITCODE)"
}

# --- bge-m3 HF cache presence check (warn only) ---------------------------
$hfModelDir = Join-Path $env:USERPROFILE '.cache\huggingface\hub\models--BAAI--bge-m3'
if (Test-Path -LiteralPath $hfModelDir -PathType Container) {
    Write-Output "bge-m3 weights present in HF cache: $hfModelDir"
} else {
    Write-Warning ("bge-m3 weights NOT found in the HF cache " +
        "($hfModelDir). The first RAG query will download ~4.3 GB. This is a " +
        "warning, not a failure - setup can proceed.")
}

Write-Output 'setup_env: done'
exit 0
