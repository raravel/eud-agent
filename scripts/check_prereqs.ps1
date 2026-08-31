#Requires -Version 5.1
<#
.SYNOPSIS
    Shared prerequisite checks for the deployment scripts (dot-source only).
    Runs on Windows PowerShell 5.1 (builtin) and PowerShell 7+; keep the
    source ASCII-only (5.1 reads BOM-less files as ANSI/CP949).

.DESCRIPTION
    The Tauri/Rust app installs and gates its selected provider at runtime, so
    normal build/dev scripts request no provider prerequisite. This file keeps
    opt-in CLI probes for provider-specific live smoke only:
      - codex: CODEX_CMD, then PATH
      - claude-code: CLAUDE_CODE_CMD, then PATH

    Direct Antigravity/OpenCode Go smoke is credential-driven inside the app and
    deliberately has no executable prerequisite.

    Defines functions only -- no work happens at dot-source time.
#>

function Resolve-CodexCmd {
    # CODEX_CMD env override first, then the PATH shim (the npm .cmd, or any
    # codex binary the user put on PATH).
    if ($env:CODEX_CMD) { return $env:CODEX_CMD }
    $cmd = Get-Command codex -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    return $null
}
function Resolve-ClaudeCodeCmd {
    if ($env:CLAUDE_CODE_CMD) { return $env:CLAUDE_CODE_CMD }
    $cmd = Get-Command claude -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    return $null
}


function Get-PrereqFailures {
    <#
    .SYNOPSIS
        Run the requested prerequisite checks; return one message per failure.
        An empty result means every requested check passed.
    #>
    param(
        [Parameter(Mandatory)]
        [ValidateSet('codex', 'claude-code')]
        [string[]]$Require
    )

    $failures = @()

    if ($Require -contains 'codex') {
        $codex = Resolve-CodexCmd
        if (-not $codex) {
            $failures += ("codex CLI not found for live smoke (checked CODEX_CMD, " +
                "then PATH). Install it from the app's AI provider settings.")
        } elseif (-not (Test-Path -LiteralPath $codex -PathType Leaf)) {
            $failures += "codex: resolved path does not exist: '$codex'"
        }
    }
    if ($Require -contains 'claude-code') {
        $claude = Resolve-ClaudeCodeCmd
        if (-not $claude) {
            $failures += ("Claude Code CLI not found for live smoke (checked " +
                "CLAUDE_CODE_CMD, then PATH). Install it from the app's AI provider settings.")
        } elseif (-not (Test-Path -LiteralPath $claude -PathType Leaf)) {
            $failures += "claude-code: resolved path does not exist: '$claude'"
        }
    }

    return $failures
}
