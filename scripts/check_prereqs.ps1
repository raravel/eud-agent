#Requires -Version 5.1
<#
.SYNOPSIS
    Shared prerequisite checks for the deployment scripts (dot-source only).
    Runs on Windows PowerShell 5.1 (builtin) and PowerShell 7+; keep the
    source ASCII-only (5.1 reads BOM-less files as ANSI/CP949).

.DESCRIPTION
    The v2 Tauri/Rust app has no Python venv: the old `uv` + `venv-python`
    checks are gone with the `server/` stack. The one remaining runtime
    prerequisite is the codex CLI the Rust core spawns:
      - codex : the LLM CLI. Resolve order is CODEX_CMD env override first,
                then PATH. NEVER spawn bare "codex" (rules.md) -- we resolve to
                a real file path and fail with install guidance otherwise.

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

function Get-PrereqFailures {
    <#
    .SYNOPSIS
        Run the requested prerequisite checks; return one message per failure.
        An empty result means every requested check passed.
    #>
    param(
        [Parameter(Mandatory)]
        [ValidateSet('codex')]
        [string[]]$Require
    )

    $failures = @()

    if ($Require -contains 'codex') {
        $codex = Resolve-CodexCmd
        if (-not $codex) {
            # codex normally installs via npm; tailor the guidance to whether
            # npm (and therefore Node.js) is already available.
            if (Get-Command npm -ErrorAction SilentlyContinue) {
                $failures += ("codex CLI not found (checked CODEX_CMD env, then " +
                    "PATH). Install it with: npm install -g @openai/codex")
            } else {
                $failures += ("codex CLI not found (checked CODEX_CMD env, then " +
                    "PATH), and npm is not available to install it. Either " +
                    "install Node.js (https://nodejs.org) and run " +
                    "'npm install -g @openai/codex', or download a standalone " +
                    "codex binary and set CODEX_CMD to its full path.")
            }
        } elseif (-not (Test-Path -LiteralPath $codex -PathType Leaf)) {
            $failures += "codex: resolved path does not exist: '$codex'"
        }
    }

    return $failures
}
