#Requires -Version 7
<#
.SYNOPSIS
    Run the v2 Tauri app in dev mode (Rust core + panel dev server).

.DESCRIPTION
    The v1 standalone Python server is gone (see EUD-121: `server/` removed). The
    v2 app is a single Tauri 2 desktop binary, so dev = `cargo tauri dev`, which
    builds the Rust core, links the isom static lib, and serves the React panel
    with hot-reload in the app's own WebView2 window.

    Requires the Tauri CLI (`cargo install tauri-cli` / `cargo-tauri`). AI provider
    binaries and credentials are selected, installed, and gated inside the app;
    development startup does not require Codex or any other provider globally.

.PARAMETER TauriArgs
    Extra arguments passed through to `cargo tauri dev` (e.g. -- --release).

.EXAMPLE
    pwsh -NoProfile -File scripts\dev_run.ps1
#>
[CmdletBinding()]
param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$TauriArgs
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# Emit all output as UTF-8 (no BOM) so a subprocess capturing stdout/stderr
# decodes cleanly; PowerShell's default console encoding is OEM/CP949 here.
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$OutputEncoding = [System.Text.UTF8Encoding]::new($false)

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path

function Fail([string]$msg) {
    [Console]::Error.WriteLine("ERROR: $msg")
    exit 1
}


# --- prerequisite: tauri CLI ----------------------------------------------
# `cargo tauri` resolves through cargo; a missing cargo-tauri surfaces as a
# cargo error. Probe cargo itself so the guidance is actionable.
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Fail "cargo not found on PATH. Install the Rust toolchain (https://rustup.rs)."
}

Write-Output "running: cargo tauri dev (cwd=$RepoRoot)"

# `cargo tauri dev` discovers src-tauri\tauri.conf.json from the repo root.
Push-Location -LiteralPath $RepoRoot
try {
    if ($TauriArgs) {
        & cargo tauri dev @TauriArgs
    } else {
        & cargo tauri dev
    }
    $code = $LASTEXITCODE
} finally {
    Pop-Location
}

exit $code
