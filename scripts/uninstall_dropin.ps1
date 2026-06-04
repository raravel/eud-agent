#Requires -Version 7
<#
.SYNOPSIS
    Remove the EUD Editor 3 agent drop-in: bridge lua, agent.cfg, Data\agent runtime.

.DESCRIPTION
    Reverses scripts\install_dropin.ps1. Removes:
      - <editor>\Data\Lua\TriggerEditor\ZZZ_10_agent_bridge.lua
      - <editor>\Data\agent\  (agent.cfg plus all runtime state: inbox, outbox,
        jobs, server.ready, heartbeat.txt, status.txt, webview2 profile)
    The vendored WebView2 DLLs next to the editor exe are LEFT IN PLACE by
    default (they are harmless and may be shared); pass -RemoveDlls to delete them.

    Idempotent: missing targets are a no-op, not an error.

.PARAMETER EditorPath
    Path to the EUD Editor 3 install folder.

.PARAMETER RemoveDlls
    Also remove the vendored WebView2 DLLs copied next to the editor exe.

.EXAMPLE
    pwsh -NoProfile -File scripts\uninstall_dropin.ps1
    pwsh -NoProfile -File scripts\uninstall_dropin.ps1 -RemoveDlls
#>
[CmdletBinding()]
param(
    [string]$EditorPath = 'C:\Users\ifthe\proj\eud\EUD.Editor.3.0.19.6.0',
    [switch]$RemoveDlls
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# Emit all output as UTF-8 (no BOM) so a subprocess capturing stdout/stderr
# decodes cleanly; PowerShell's default console encoding is OEM/CP949 here.
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$OutputEncoding = [System.Text.UTF8Encoding]::new($false)

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path

$LuaName = 'ZZZ_10_agent_bridge.lua'
$TriggerEditorRel = 'Data\Lua\TriggerEditor'

function Fail([string]$msg) {
    [Console]::Error.WriteLine("ERROR: $msg")
    exit 1
}

if (-not (Test-Path -LiteralPath $EditorPath -PathType Container)) {
    Fail "editor path does not exist or is not a directory: '$EditorPath'"
}

# --- remove the bridge lua ------------------------------------------------
$luaDest = Join-Path $EditorPath (Join-Path $TriggerEditorRel $LuaName)
if (Test-Path -LiteralPath $luaDest -PathType Leaf) {
    Remove-Item -LiteralPath $luaDest -Force
    Write-Output "removed $luaDest"
} else {
    Write-Output "bridge lua already absent: $luaDest"
}

# --- remove Data\agent\ (agent.cfg + all runtime state) -------------------
$agentDir = Join-Path $EditorPath 'Data\agent'
if (Test-Path -LiteralPath $agentDir -PathType Container) {
    Remove-Item -LiteralPath $agentDir -Recurse -Force
    Write-Output "removed $agentDir (agent.cfg + runtime files)"
} else {
    Write-Output "Data\agent already absent: $agentDir"
}

# --- optionally remove the WebView2 DLLs ----------------------------------
if ($RemoveDlls) {
    $webview2Dir = Join-Path $RepoRoot 'vendor\webview2'
    $dllNames = @(
        Get-ChildItem -LiteralPath $webview2Dir -Filter '*.dll' -File -ErrorAction SilentlyContinue |
            Select-Object -ExpandProperty Name
    )
    foreach ($name in $dllNames) {
        $dst = Join-Path $EditorPath $name
        if (Test-Path -LiteralPath $dst -PathType Leaf) {
            Remove-Item -LiteralPath $dst -Force
            Write-Output "removed $dst"
        }
    }
} else {
    Write-Output 'left WebView2 DLLs in place (pass -RemoveDlls to delete them)'
}

Write-Output 'uninstall_dropin: done'
exit 0
