Set-StrictMode -Version Latest

function Set-E2QaLine {
    param([Parameter(Mandatory)][string]$Text, [Parameter(Mandatory)][string]$Name, [Parameter(Mandatory)][string]$Value)
    $pattern = "(?m)^$([regex]::Escape($Name))\s*:\s*.*$"
    if (-not [regex]::IsMatch($Text, $pattern)) { throw "Authentic E2 source lacks field: $Name" }
    [regex]::Replace($Text, $pattern, "$Name : $Value", 1)
}

function Add-E2QaGrpRows {
    param([Parameter(Mandatory)][string]$Text, [Parameter(Mandatory)][bool]$Use)
    $rows = "GRPCount : 1`r`nGRPUse0 : $Use`r`n"
    if ($Text -match '(?m)^GRPCount\s*:') {
        $text = Set-E2QaLine $Text 'GRPCount' '1'
        return Set-E2QaLine $text 'GRPUse0' ([string]$Use)
    }
    $Text.Replace("E_GRPEditorSET", "$rows" + 'E_GRPEditorSET')
}

function New-E2QaFixtureSet {
    param(
        [Parameter(Mandatory)]$Lifecycle,
        [Parameter(Mandatory)][string]$E2Sample,
        [Parameter(Mandatory)][string]$Map,
        [Parameter(Mandatory)][string]$Euddraft
    )
    $fixtures = Join-Path $Lifecycle.root 'fixtures'
    New-Item -ItemType Directory -Path $fixtures | Out-Null
    $originals = [ordered]@{}
    foreach ($path in @($E2Sample, $Map, $Euddraft)) { $originals[$path] = Get-E2QaHash $path }
    $authentic = Join-Path $fixtures 'authentic'
    New-Item -ItemType Directory -Path $authentic | Out-Null
    $source = Join-Path $authentic 'defense_mob.e2s'
    $inputMap = Join-Path $authentic 'basemap.scx'
    Copy-Item -LiteralPath $E2Sample -Destination $source
    Copy-Item -LiteralPath $Map -Destination $inputMap
    $baseText = [IO.File]::ReadAllText($source, [Text.Encoding]::UTF8)
    $baseText = Set-E2QaLine $baseText 'InputMap' 'basemap.scx'
    [IO.File]::WriteAllText($source, $baseText, [Text.UTF8Encoding]::new($false))

    $variants = [ordered]@{}
    foreach ($name in @('missing-map','ambiguous-map','active-scdb','effective-grp','empty-grp')) {
        $dir = Join-Path $fixtures $name
        New-Item -ItemType Directory -Path $dir | Out-Null
        $variantSource = Join-Path $dir $(if ($name -eq 'ambiguous-map') { 'case.e2p' } else { 'case.e2s' })
        $text = $baseText
        switch ($name) {
            'missing-map' { $text = Set-E2QaLine $text 'InputMap' 'Missing\scene.scx' }
            'ambiguous-map' {
                $text = Set-E2QaLine $text 'InputMap' 'Missing\scene.scx'
                New-Item -ItemType Directory -Path (Join-Path $dir 'Map') | Out-Null
                Copy-Item -LiteralPath $Map -Destination (Join-Path $dir 'Map\scene.scx')
                Copy-Item -LiteralPath $Map -Destination (Join-Path $dir 'scene.scx')
            }
            'active-scdb' { $text = Set-E2QaLine $text 'SCDBUse' 'True'; $text = Set-E2QaLine $text 'SCDBDeath' '1,2' }
            'effective-grp' { $text = Set-E2QaLine $text 'UsedSetting5' 'True'; $text = Add-E2QaGrpRows $text $true }
            'empty-grp' { $text = Set-E2QaLine $text 'UsedSetting5' 'True'; $text = Add-E2QaGrpRows $text $false }
        }
        [IO.File]::WriteAllText($variantSource, $text, [Text.UTF8Encoding]::new($false))
        $variants[$name] = [ordered]@{ root = $dir; source = $variantSource }
    }

    $large = Join-Path $fixtures 'cancel-large'
    New-Item -ItemType Directory -Path (Join-Path $large 'Map'), (Join-Path $large 'Sound') -Force | Out-Null
    Copy-Item -LiteralPath $Map -Destination (Join-Path $large 'Map\basemap.scx')
    $tone = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\crates\isom\tests\fixtures\tone.ogg'))
    if (-not (Test-Path -LiteralPath $tone)) { throw "Authentic cancellation audio fixture is missing: $tone" }
    $soundRows = [Collections.Generic.List[string]]::new()
    0..95 | ForEach-Object {
        $name = "cancel-{0:D3}.ogg" -f $_
        Copy-Item -LiteralPath $tone -Destination (Join-Path $large "Sound\$name")
        $soundRows.Add("C:\lost\$name")
    }
    $cancelText = Set-E2QaLine $baseText 'InputMap' 'basemap.scx'
    $cancelText = Set-E2QaLine $cancelText 'UsedSetting7' 'True'
    $soundSection = "S_SoundPlayerSET`r`n96`r`n$($soundRows -join "`r`n")`r`nE_SoundPlayerSET`r`n"
    $soundPattern = '(?ms)^S_SoundPlayerSET\r?\n.*?^E_SoundPlayerSET\r?\n?'
    if ([regex]::IsMatch($cancelText, $soundPattern)) { $cancelText = [regex]::Replace($cancelText, $soundPattern, $soundSection, 1) }
    else { $cancelText = $cancelText.Replace('S_FileManagerSET', $soundSection + 'S_FileManagerSET') }
    $cancelSource = Join-Path $large 'cancel-large.e2p'
    [IO.File]::WriteAllText($cancelSource, $cancelText, [Text.UTF8Encoding]::new($false))
    $nonempty = Join-Path $fixtures 'nonempty-destination'
    New-Item -ItemType Directory -Path $nonempty | Out-Null
    $sentinel = Join-Path $nonempty 'concurrent-user.txt'
    [IO.File]::WriteAllText($sentinel, 'task18-user-sentinel', [Text.UTF8Encoding]::new($false))

    $receipt = [ordered]@{
        originals = $originals
        authentic = [ordered]@{ root = $authentic; source = $source; map = $inputMap }
        variants = $variants
        cancelLarge = [ordered]@{ root = $large; source = $cancelSource; audioCount = 96; toneSha256 = Get-E2QaHash $tone }
        nonempty = [ordered]@{ root = $nonempty; sentinel = $sentinel; sha256 = Get-E2QaHash $sentinel }
    }
    Write-E2QaJson (Join-Path $Lifecycle.evidence 'fixtures.json') $receipt 10
    Register-E2QaOwnedTree $Lifecycle $fixtures
    $receipt
}

function Initialize-E2QaIsolatedState {
    param([Parameter(Mandatory)]$Lifecycle, [Parameter(Mandatory)][string]$Euddraft)
    $roaming = Join-Path $Lifecycle.root 'roaming\eud-agent'
    $local = Join-Path $Lifecycle.root 'local\eud-agent'
    New-Item -ItemType Directory -Path $roaming, $local -Force | Out-Null
    $rag = Join-Path $local 'rag\rag-index.bin'
    New-Item -ItemType Directory -Path (Split-Path $rag) -Force | Out-Null
    [IO.File]::WriteAllBytes($rag, [byte[]](0x45,0x52,0x41,0x47,0x02,0,0,0,0,0,0,0))
    $ragHash = Get-E2QaHash $rag
    $settings = [ordered]@{
        schema_version = 3
        project_path = ''
        euddraft_path = $Euddraft
        default_provider = 'ollama'
        providers = [ordered]@{
            codex = @{ executableOverride = $null; defaultModel = $null; defaultReasoning = $null; largeContextModels = @() }
            claudeCode = @{ executableOverride = $null; defaultModel = $null; defaultReasoning = $null }
            antigravity = @{ defaultModel = $null; defaultReasoning = $null }
            opencodeGo = @{ defaultModel = $null; defaultReasoning = $null }
            ollama = @{ baseUrl = 'http://127.0.0.1:6174'; defaultModel = 'task18-owned-model'; defaultReasoning = $null }
        }
        rag_index = @{ name = 'task18-owned-rag-index'; sha256 = $ragHash; version = '4' }
        project_recents = @()
        project_recents_initialized = $true
    }
    Write-E2QaJson (Join-Path $roaming 'config.json') $settings 8
    Register-E2QaOwnedTree $Lifecycle $roaming
    Register-E2QaOwnedTree $Lifecycle $local
    [ordered]@{ roaming = $roaming; local = $local; rag = $rag; ragSha256 = $ragHash }
}
