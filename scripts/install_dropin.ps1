#Requires -Version 5.1
<#
.SYNOPSIS
    Drop-in install of the EUD Editor 3 agent: bridge lua, WebView2 DLLs, agent.cfg.
    Runs on Windows PowerShell 5.1 (builtin) and PowerShell 7+; keep the
    source ASCII-only (5.1 reads BOM-less files as ANSI/CP949).

.DESCRIPTION
    Integration with EUD Editor 3 is file copies only (rules.md "Editor
    integrity"). This script:
      1. validates -EditorPath points at a real editor (the editor exe AND the
         Data\Lua\TriggerEditor folder must exist) BEFORE copying anything;
      2. checks the shared prerequisites (uv + codex + venv python via
         scripts\check_prereqs.ps1) and fails BEFORE copying when one is
         missing (a drop-in pointing at a missing venv/codex only surfaces as
         a runtime failure inside the editor);
      3. copies bridge\ZZZ_10_agent_bridge.lua -> <editor>\Data\Lua\TriggerEditor\;
      4. copies vendor\webview2\*.dll -> next to the editor exe;
      5. creates <editor>\Data\agent\ and writes agent.cfg as
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
    [string]$EditorPath = 'C:\Users\ifthe\proj\eud\EUD.Editor.3.0.19.6.0',
    # RAG DB folder to record in agent.cfg as "rag_db". When omitted, a
    # release-bundled DB at <repo>\rag\chromadb_bge is auto-detected; when
    # neither exists the key is left out and the server uses its default
    # (the dev-machine ECA path).
    [string]$RagDb = ''
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

# --- shared prerequisite checks (uv + codex + venv python) ----------------
# Fail BEFORE copying anything: a drop-in pointing at a missing venv/codex
# would only surface as a runtime failure inside the editor.
. (Join-Path $PSScriptRoot 'check_prereqs.ps1')
$prereqFailures = @(Get-PrereqFailures -Require 'uv', 'codex', 'venv-python' -RepoRoot $RepoRoot)
if ($prereqFailures.Count -gt 0) {
    Fail ($prereqFailures -join "`n")
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

# rag_db: explicit -RagDb wins; else auto-detect the release-bundled DB at
# <repo>\rag\chromadb_bge (package_release.ps1 puts it there). Without either,
# the key is omitted and the server falls back to its built-in default.
$ragDbResolved = $RagDb
if (-not $ragDbResolved) {
    $bundledRag = Join-Path $RepoRoot 'rag\chromadb_bge'
    if (Test-Path -LiteralPath (Join-Path $bundledRag 'chroma.sqlite3') -PathType Leaf) {
        $ragDbResolved = $bundledRag
    }
}
if ($ragDbResolved) {
    if (-not (Test-Path -LiteralPath (Join-Path $ragDbResolved 'chroma.sqlite3') -PathType Leaf)) {
        Fail ("rag_db '$ragDbResolved' does not look like a chromadb store " +
            "(chroma.sqlite3 missing)")
    }
    $cfg['rag_db'] = (Resolve-Path -LiteralPath $ragDbResolved).Path
    Write-Output "agent.cfg rag_db -> $($cfg['rag_db'])"
}

$json = $cfg | ConvertTo-Json -Depth 4

$cfgPath = Join-Path $agentDir 'agent.cfg'
$utf8NoBom = [System.Text.UTF8Encoding]::new($false)
[System.IO.File]::WriteAllText($cfgPath, $json, $utf8NoBom)
Write-Output "wrote agent.cfg -> $cfgPath (UTF-8 no BOM)"

Write-Output 'install_dropin: done'
exit 0
