#Requires -Version 7
<#
.SYNOPSIS
    Drop-in install of the EUD Editor 3 agent: bridge lua, WebView2 DLLs, agent.cfg.

.DESCRIPTION
    Integration with EUD Editor 3 is file copies only (rules.md "Editor
    integrity"). This script:
      1. validates -EditorPath points at a real editor (the editor exe AND the
         Data\Lua\TriggerEditor folder must exist) BEFORE copying anything;
      2. copies bridge\ZZZ_10_agent_bridge.lua -> <editor>\Data\Lua\TriggerEditor\;
      3. copies vendor\webview2\*.dll -> next to the editor exe;
      4. creates <editor>\Data\agent\ and writes agent.cfg as
         { "python_exe": <abs server\.venv python>, "repo_root": <abs repo root>,
           "port": 8765 } in UTF-8 WITHOUT BOM (architecture.md "Boot and
         lifecycle"; the drop-in lua parses the first line, so a BOM corrupts it).

    Re-runs are idempotent (overwrites in place, no duplication).

.PARAMETER EditorPath
    Path to the EUD Editor 3 install folder.

.EXAMPLE
    pwsh -NoProfile -File scripts\install_dropin.ps1
    pwsh -NoProfile -File scripts\install_dropin.ps1 -EditorPath D:\Editors\EUD3
#>
[CmdletBinding()]
param(
    [string]$EditorPath = 'C:\Users\ifthe\proj\eud\EUD.Editor.3.0.19.6.0'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# Emit all output as UTF-8 (no BOM) so a subprocess capturing stdout/stderr
# decodes cleanly; PowerShell's default console encoding is OEM/CP949 here.
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$OutputEncoding = [System.Text.UTF8Encoding]::new($false)

# Repo root is computed from this script's location (scripts/ lives at the repo
# root), NOT from the caller's cwd.
$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path

$EditorExeName = 'EUD Editor 3.exe'
$TriggerEditorRel = 'Data\Lua\TriggerEditor'
$LuaName = 'ZZZ_10_agent_bridge.lua'

function Fail([string]$msg) {
    # Plain stderr write (not Write-Error) to avoid the rich formatter's
    # source-line truncation, which injects a CP949 ellipsis byte.
    [Console]::Error.WriteLine("ERROR: $msg")
    exit 1
}

# --- validate the editor path BEFORE touching anything --------------------
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

# --- locate the artifacts we copy from this repo --------------------------
$luaSrc = Join-Path $RepoRoot ('bridge\' + $LuaName)
if (-not (Test-Path -LiteralPath $luaSrc -PathType Leaf)) {
    Fail "bridge lua not found in repo: '$luaSrc'"
}

$webview2Dir = Join-Path $RepoRoot 'vendor\webview2'
$dllSources = @(Get-ChildItem -LiteralPath $webview2Dir -Filter '*.dll' -File -ErrorAction SilentlyContinue)
if ($dllSources.Count -eq 0) {
    Fail "no WebView2 DLLs found in '$webview2Dir'"
}

$venvPython = Join-Path $RepoRoot 'server\.venv\Scripts\python.exe'

# --- copy the bridge lua --------------------------------------------------
$luaDest = Join-Path $triggerEditorDir $LuaName
Copy-Item -LiteralPath $luaSrc -Destination $luaDest -Force
Write-Output "copied bridge lua -> $luaDest"

# --- copy the WebView2 DLLs next to the editor exe ------------------------
foreach ($dll in $dllSources) {
    $dst = Join-Path $EditorPath $dll.Name
    Copy-Item -LiteralPath $dll.FullName -Destination $dst -Force
    Write-Output "copied $($dll.Name) -> $dst"
}

# --- create Data\agent\ and write agent.cfg (UTF-8 WITHOUT BOM) -----------
$agentDir = Join-Path $EditorPath 'Data\agent'
if (-not (Test-Path -LiteralPath $agentDir -PathType Container)) {
    New-Item -ItemType Directory -Path $agentDir -Force | Out-Null
}

$cfg = [ordered]@{
    python_exe = $venvPython
    repo_root  = $RepoRoot
    port       = 8765
}
$json = $cfg | ConvertTo-Json -Depth 4

$cfgPath = Join-Path $agentDir 'agent.cfg'
$utf8NoBom = [System.Text.UTF8Encoding]::new($false)
[System.IO.File]::WriteAllText($cfgPath, $json, $utf8NoBom)
Write-Output "wrote agent.cfg -> $cfgPath (UTF-8 no BOM)"

if (-not (Test-Path -LiteralPath $venvPython -PathType Leaf)) {
    Write-Warning ("agent.cfg points python_exe at '$venvPython' which does not " +
        "exist yet; run scripts\setup_env.ps1 to create the venv before starting " +
        "the editor.")
}

Write-Output 'install_dropin: done'
exit 0
