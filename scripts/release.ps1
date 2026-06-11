#Requires -Version 5.1
<#
.SYNOPSIS
    Build, sign, and publish an eud-agent release to GitHub Releases with a Tauri
    updater manifest (latest.json).

.DESCRIPTION
    Local manual release pipeline (Decision 04; GitHub Actions CI is a later phase).
    Steps: preflight (signing key + gh + clean tree) -> bump version in tauri.conf.json
    and src-tauri/Cargo.toml -> `tauri build` (signs the NSIS updater bundle) -> collect
    the nsis artifacts -> synthesize latest.json from the .sig (a local build does NOT
    emit it; only tauri-action does) -> `gh release create` and upload the installer,
    the updater bundle, and latest.json.

    The updater endpoint in tauri.conf.json points at
    releases/latest/download/latest.json, so publishing this asset on the newest release
    is what makes installed apps see the update.

.PARAMETER Version
    Semver version, e.g. 0.1.1. Becomes the git tag v<Version> and the bundle version.

.PARAMETER Notes
    Optional release notes (shown in the in-app update banner and the GitHub Release body).

.NOTES
    Requires env vars before running:
      TAURI_SIGNING_PRIVATE_KEY           - path to (or contents of) the minisign private key
      TAURI_SIGNING_PRIVATE_KEY_PASSWORD  - its password (set to "" if the key has none)
    The private key is NEVER committed; keep it under %USERPROFILE%\.tauri\.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Version,
    [string]$Notes = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$OutputEncoding = [System.Text.UTF8Encoding]::new($false)

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$Repo = 'raravel/eud-agent'
$Utf8NoBom = [System.Text.UTF8Encoding]::new($false)

function Fail([string]$msg) {
    [Console]::Error.WriteLine("ERROR: $msg")
    exit 1
}

# Write text as UTF-8 without a BOM (rules.md: app-written files are BOM-free; a BOM in
# latest.json or the manifests is a needless risk).
function Write-NoBom([string]$path, [string]$text) {
    [System.IO.File]::WriteAllText($path, $text, $Utf8NoBom)
}

# --- Preflight ------------------------------------------------------------------------

if ($Version -notmatch '^\d+\.\d+\.\d+$') {
    Fail "Version must be semver (e.g. 0.1.1); got '$Version'."
}

if (-not $env:TAURI_SIGNING_PRIVATE_KEY) {
    Fail ("TAURI_SIGNING_PRIVATE_KEY is not set. Generate a key once with " +
        "`cargo tauri signer generate -w `$env:USERPROFILE\.tauri\eud-agent_updater.key` " +
        "and export the key path + TAURI_SIGNING_PRIVATE_KEY_PASSWORD before releasing.")
}
if ($null -eq $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD) {
    Fail "TAURI_SIGNING_PRIVATE_KEY_PASSWORD is not set (use '' for a passwordless key)."
}

if (-not (Get-Command gh -ErrorAction SilentlyContinue)) {
    Fail "GitHub CLI 'gh' not found on PATH. Install it and run 'gh auth login'."
}
& gh auth status 2>$null | Out-Null
if ($LASTEXITCODE -ne 0) { Fail "gh is not authenticated. Run 'gh auth login'." }

Push-Location $RepoRoot
try {
    $dirty = (& git status --porcelain)
    if ($dirty) {
        Fail "Working tree is not clean. Commit or stash changes before releasing."
    }

    # --- Bump version (BOM-free, surgical regex; ConvertTo-Json would reflow the file) ---
    $confPath = Join-Path $RepoRoot 'src-tauri\tauri.conf.json'
    $conf = Get-Content -LiteralPath $confPath -Raw
    $conf = [regex]::Replace($conf, '("version"\s*:\s*")[^"]*(")', "`${1}$Version`${2}", 1)
    Write-NoBom $confPath $conf

    $cargoPath = Join-Path $RepoRoot 'src-tauri\Cargo.toml'
    $cargo = Get-Content -LiteralPath $cargoPath -Raw
    $cargo = [regex]::Replace($cargo, '(?m)^(version\s*=\s*")[^"]*(")', "`${1}$Version`${2}", 1)
    Write-NoBom $cargoPath $cargo

    Write-Output "bumped version -> $Version"

    # --- Build (beforeBuildCommand builds the panel; tauri signs the updater bundle) -----
    # This repo uses cargo-tauri (see scripts/dev_run.ps1 `cargo tauri dev`), NOT the npm
    # `@tauri-apps/cli` — `npx tauri` is not installed here.
    Write-Output "building (cargo tauri build)…"
    & cargo tauri build
    if ($LASTEXITCODE -ne 0) { Fail "cargo tauri build failed." }

    # --- Collect NSIS artifacts ---------------------------------------------------------
    # Cargo workspace: the bundle lands under the WORKSPACE-ROOT target, not src-tauri\target.
    $nsisDir = Join-Path $RepoRoot 'target\release\bundle\nsis'
    if (-not (Test-Path -LiteralPath $nsisDir)) { Fail "NSIS bundle dir not found: $nsisDir" }

    function Get-Single([string]$dir, [string]$pattern, [string]$label) {
        $items = @(Get-ChildItem -LiteralPath $dir -Filter $pattern -File)
        if ($items.Count -ne 1) {
            Fail "expected exactly one $label ($pattern) in $dir, found $($items.Count)."
        }
        return $items[0]
    }

    # Tauri 2 NSIS updater: the signed artifact IS the `-setup.exe` (there is no `.nsis.zip`);
    # the updater downloads + runs it. The new-install installer and the updater target are
    # the same file, so latest.json's url points at this exe and we upload it once.
    # Match THIS version's artifacts only — the bundle dir may retain prior versions'
    # `-setup.exe` files (e.g. a 0.1.0 build before a 0.1.1 build), which a bare
    # `*-setup.exe` glob would ambiguously match.
    $installer = Get-Single $nsisDir "*_${Version}_*-setup.exe" 'installer / updater artifact'
    $sigFile = Get-Single $nsisDir "*_${Version}_*-setup.exe.sig" 'updater signature'

    $signature = (Get-Content -LiteralPath $sigFile.FullName -Raw).Trim()

    # --- Synthesize latest.json (local builds don't emit it) ----------------------------
    $stageDir = Join-Path $RepoRoot 'release'
    if (-not (Test-Path -LiteralPath $stageDir)) {
        New-Item -ItemType Directory -Path $stageDir | Out-Null
    }
    $installerUrl = "https://github.com/$Repo/releases/download/v$Version/$($installer.Name)"
    $pubDate = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")

    $manifest = [ordered]@{
        version   = $Version
        notes     = $Notes
        pub_date  = $pubDate
        platforms = [ordered]@{
            'windows-x86_64' = [ordered]@{
                signature = $signature
                url       = $installerUrl
            }
        }
    }
    $latestPath = Join-Path $stageDir 'latest.json'
    Write-NoBom $latestPath ($manifest | ConvertTo-Json -Depth 6)
    Write-Output "wrote $latestPath"

    # --- Publish ------------------------------------------------------------------------
    Write-Output "creating GitHub release v$Version…"
    & gh release create "v$Version" `
        --repo $Repo `
        --title "v$Version" `
        --notes $Notes `
        $installer.FullName `
        $latestPath
    if ($LASTEXITCODE -ne 0) {
        Fail "gh release create failed (does the tag v$Version already exist?)."
    }

    Write-Output "released v${Version}: setup.exe (installer + updater target) + latest.json uploaded."
    Write-Output "NOTE: commit the version bump (tauri.conf.json, Cargo.toml) and tag if desired."
}
finally {
    Pop-Location
}
