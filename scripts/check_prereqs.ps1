#Requires -Version 5.1
<#
.SYNOPSIS
    Shared prerequisite checks for the deployment scripts (dot-source only).
    Runs on Windows PowerShell 5.1 (builtin) and PowerShell 7+; keep the
    source ASCII-only (5.1 reads BOM-less files as ANSI/CP949).

.DESCRIPTION
    Dot-sourced by setup_env.ps1 and install_dropin.ps1 so both ends of the
    install path agree on what a working environment needs:
      - uv          : creates/syncs server\.venv (tech-stack.md convention)
      - codex       : the LLM CLI the server spawns. Resolve order mirrors
                      server/eud_agent/config.py _resolve_codex (CODEX_CMD env
                      override first, then PATH). NEVER spawn bare "codex"
                      (rules.md) -- we resolve to a real file path and fail
                      with install guidance otherwise.
      - venv python : server\.venv\Scripts\python.exe, the product of
                      setup_env.ps1 (agent.cfg points the bridge at it).

    Defines functions only -- no work happens at dot-source time.
#>

function Resolve-CodexCmd {
    # Mirrors config.py _resolve_codex: CODEX_CMD env override first, then the
    # PATH shim (the npm .cmd, or any codex binary the user put on PATH).
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
        [ValidateSet('uv', 'codex', 'venv-python')]
        [string[]]$Require,

        # Repo root; required only when 'venv-python' is in -Require.
        [string]$RepoRoot
    )

    $failures = @()

    if ($Require -contains 'uv') {
        if (-not (Get-Command uv -ErrorAction SilentlyContinue)) {
            $failures += ("uv not found on PATH. Install uv " +
                "(https://docs.astral.sh/uv/) - this project uses uv for the " +
                "venv + installs (the ECA venv has no pip; same convention here).")
        }
    }

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

    if ($Require -contains 'venv-python') {
        $venvPython = Join-Path $RepoRoot 'server\.venv\Scripts\python.exe'
        if (-not (Test-Path -LiteralPath $venvPython -PathType Leaf)) {
            $failures += ("venv python missing: '$venvPython'. Run " +
                "scripts\setup_env.ps1 first to create the server venv.")
        }
    }

    return $failures
}
