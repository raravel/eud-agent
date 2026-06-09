#Requires -Version 5.1
<#
.SYNOPSIS
    Copy the slim EUD Editor 3 Lua bridge into the editor install.

.DESCRIPTION
    Re-runs are idempotent: the bridge file is overwritten in place.

.PARAMETER EditorPath
    Path to the EUD Editor 3 install folder.
#>
[CmdletBinding()]
param(
    [string]$EditorPath = 'C:\Users\ifthe\proj\eud\EUD.Editor.3.0.19.6.0'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$OutputEncoding = [System.Text.UTF8Encoding]::new($false)

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path

$EditorExeName = 'EUD Editor 3.exe'
$TriggerEditorRel = 'Data\Lua\TriggerEditor'
$LuaName = 'ZZZ_10_agent_bridge.lua'

function Fail([string]$msg) {
    [Console]::Error.WriteLine("ERROR: $msg")
    exit 1
}

if (-not (Test-Path -LiteralPath $EditorPath -PathType Container)) {
    Fail "editor path does not exist or is not a directory: '$EditorPath'"
}

$editorExe = Join-Path $EditorPath $EditorExeName
$triggerEditorDir = Join-Path $EditorPath $TriggerEditorRel
$exeOk = Test-Path -LiteralPath $editorExe -PathType Leaf
$triggerOk = Test-Path -LiteralPath $triggerEditorDir -PathType Container
if (-not ($exeOk -and $triggerOk)) {
    Fail ("'$EditorPath' is not a valid EUD Editor 3 folder: expected both " +
        "'$EditorExeName' and '$TriggerEditorRel' to exist " +
        "(exe=$exeOk, triggerEditor=$triggerOk). Pass -EditorPath pointing at " +
        "the editor install folder.")
}

$bridgeSrc = Join-Path $RepoRoot 'bridge\ZZZ_10_agent_bridge.lua'
if (-not (Test-Path -LiteralPath $bridgeSrc -PathType Leaf)) {
    Fail "bridge source not found: '$bridgeSrc'"
}

$bridgeDst = Join-Path $triggerEditorDir $LuaName
Copy-Item -LiteralPath $bridgeSrc -Destination $bridgeDst -Force

Write-Output "copied bridge -> $bridgeDst"
