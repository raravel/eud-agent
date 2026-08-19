[CmdletBinding()]
param(
    [switch]$Verify
)

$ErrorActionPreference = "Stop"
$Commit = "7f175df06ae57e9da65b8add25d084b5f5df0e1f"
$ArchiveSha256 = "46a6d89d5c7ba6ac40d8b5114d409f231409d56b37a610168d0e32a7b4637f46"
$ArchiveUrl = "https://github.com/zuhanit/epscript-lsp/archive/$Commit.zip"
$RepoRoot = Split-Path -Parent $PSScriptRoot
$ToolRoot = Join-Path $RepoRoot "tools\epscript-lsp-agent"
$VendorRoot = Join-Path $RepoRoot "vendor\epscript-lsp-agent"
$TempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("eud-epscript-lsp-" + [guid]::NewGuid().ToString("N"))
$ArchivePath = Join-Path $TempRoot "upstream.zip"
$ExtractRoot = Join-Path $TempRoot "source"
$GeneratedBundle = Join-Path $TempRoot "adapter.cjs"
$Utf8NoBom = New-Object System.Text.UTF8Encoding($false)

function Get-Sha256Hex([string]$Path) {
    $Stream = [System.IO.File]::OpenRead($Path)
    $Sha256 = [System.Security.Cryptography.SHA256]::Create()
    try {
        return ([System.BitConverter]::ToString($Sha256.ComputeHash($Stream))).Replace("-", "").ToLowerInvariant()
    }
    finally {
        $Sha256.Dispose()
        $Stream.Dispose()
    }
}

try {
    New-Item -ItemType Directory -Path $TempRoot, $ExtractRoot -Force | Out-Null
    Invoke-WebRequest -UseBasicParsing -Uri $ArchiveUrl -OutFile $ArchivePath
    $ActualArchiveSha256 = Get-Sha256Hex $ArchivePath
    if ($ActualArchiveSha256 -ne $ArchiveSha256) {
        throw "upstream archive checksum mismatch: expected $ArchiveSha256, got $ActualArchiveSha256"
    }

    Expand-Archive -Path $ArchivePath -DestinationPath $ExtractRoot
    $UpstreamRoot = Join-Path $ExtractRoot "epscript-lsp-$Commit\packages\server\src"
    if (-not (Test-Path (Join-Path $UpstreamRoot "analyzer.ts") -PathType Leaf)) {
        throw "pinned upstream analyzer source is missing"
    }

    Push-Location $ToolRoot
    try {
        & npm.cmd ci
        if ($LASTEXITCODE -ne 0) {
            throw "npm ci failed with exit code $LASTEXITCODE"
        }
        $PreviousSource = $env:EPSCRIPT_LSP_SOURCE
        $PreviousOutput = $env:EPSCRIPT_LSP_OUTPUT
        $env:EPSCRIPT_LSP_SOURCE = $UpstreamRoot
        $env:EPSCRIPT_LSP_OUTPUT = $GeneratedBundle
        try {
            & npm.cmd run build
            if ($LASTEXITCODE -ne 0) {
                throw "adapter build failed with exit code $LASTEXITCODE"
            }
        }
        finally {
            $env:EPSCRIPT_LSP_SOURCE = $PreviousSource
            $env:EPSCRIPT_LSP_OUTPUT = $PreviousOutput
        }
    }
    finally {
        Pop-Location
    }

    $GeneratedSha256 = Get-Sha256Hex $GeneratedBundle
    $CommittedBundle = Join-Path $VendorRoot "adapter.cjs"
    $CommittedChecksum = Join-Path $VendorRoot "adapter.sha256"
    $UpstreamLicense = Join-Path $ExtractRoot "epscript-lsp-$Commit\LICENSE.md"
    $Provenance = Join-Path $ToolRoot "upstream.json"

    if ($Verify) {
        if (-not (Test-Path $CommittedBundle -PathType Leaf)) {
            throw "committed adapter bundle is missing"
        }
        $CommittedSha256 = Get-Sha256Hex $CommittedBundle
        if ($CommittedSha256 -ne $GeneratedSha256) {
            throw "adapter bytes differ: committed $CommittedSha256, regenerated $GeneratedSha256"
        }
        $ChecksumText = [System.IO.File]::ReadAllText($CommittedChecksum).Trim()
        if ($ChecksumText -ne "$CommittedSha256  adapter.cjs") {
            throw "adapter.sha256 does not match the committed bundle"
        }
        Write-Host "epscript-lsp agent adapter verified: $CommittedSha256"
    }
    else {
        New-Item -ItemType Directory -Path $VendorRoot -Force | Out-Null
        [System.IO.File]::WriteAllBytes($CommittedBundle, [System.IO.File]::ReadAllBytes($GeneratedBundle))
        [System.IO.File]::WriteAllText($CommittedChecksum, "$GeneratedSha256  adapter.cjs`n", $Utf8NoBom)
        [System.IO.File]::WriteAllBytes((Join-Path $VendorRoot "LICENSE.md"), [System.IO.File]::ReadAllBytes($UpstreamLicense))
        [System.IO.File]::WriteAllBytes((Join-Path $VendorRoot "provenance.json"), [System.IO.File]::ReadAllBytes($Provenance))
        Write-Host "epscript-lsp agent adapter regenerated: $GeneratedSha256"
    }
}
finally {
    Remove-Item -LiteralPath $TempRoot -Recurse -Force -ErrorAction SilentlyContinue
}
