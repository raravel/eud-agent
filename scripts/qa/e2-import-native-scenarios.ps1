Set-StrictMode -Version Latest

function Get-E2QaText {
    param([Parameter(Mandatory)][string]$AsciiJsonEscape)
    ('"' + $AsciiJsonEscape + '"') | ConvertFrom-Json
}

function Open-E2QaImportDialog {
    param([Parameter(Mandatory)][Windows.Automation.AutomationElement]$Window)
    Invoke-E2QaNamedButton $Window (Get-E2QaText 'E2 \ud504\ub85c\uc81d\ud2b8 \uac00\uc838\uc624\uae30')
    Wait-E2QaElement -Root $Window -Name (Get-E2QaText 'E2S/E2P \uc120\ud0dd') -ControlType 'Button' | Out-Null
}

function Close-E2QaImportDialog {
    param([Parameter(Mandatory)][Windows.Automation.AutomationElement]$Window)
    $dialogName = Get-E2QaText 'E2 \ud504\ub85c\uc81d\ud2b8 \uac00\uc838\uc624\uae30'
    $dialogs = @($Window.FindAll([Windows.Automation.TreeScope]::Descendants, [Windows.Automation.Condition]::TrueCondition) | Where-Object { $_.Current.Name -eq $dialogName -and $_.Current.ControlType.ProgrammaticName -eq 'ControlType.Window' })
    if ($dialogs.Count -ne 1) { throw "E2 import dialog identity is not unique: count=$($dialogs.Count)" }
    $closeName = Get-E2QaText '\ub2eb\uae30'
    $closeButtons = @($dialogs[0].FindAll([Windows.Automation.TreeScope]::Descendants, [Windows.Automation.Condition]::TrueCondition) | Where-Object { $_.Current.Name -eq $closeName -and $_.Current.ControlType.ProgrammaticName -eq 'ControlType.Button' })
    if ($closeButtons.Count -ne 1 -or -not $closeButtons[0].Current.IsEnabled) { throw "E2 import close button is not uniquely enabled: count=$($closeButtons.Count)" }
    Invoke-E2QaElement $closeButtons[0]
    Wait-E2QaCondition -TimeoutSeconds 10 -Failure 'E2 import dialog did not close' -Condition {
        $null -eq (Find-E2QaElement -Root $Window -Name $dialogName -ControlType 'Window')
    } | Out-Null
}

function Set-E2QaImportInputs {
    param(
        [Parameter(Mandatory)][Windows.Automation.AutomationElement]$Window,
        [Parameter(Mandatory)][int]$ProcessId,
        [Parameter(Mandatory)][string]$Source,
        [Parameter(Mandatory)][string]$Destination,
        [Parameter(Mandatory)][string]$FolderObservationDirectory,
        [switch]$ObserveDestinationFolder
    )
    Invoke-E2QaNamedButton $Window (Get-E2QaText 'E2S/E2P \uc120\ud0dd')
    Select-E2QaNativePath $ProcessId $Source
    if ($ObserveDestinationFolder) { Observe-E2QaNativeFolderDialog $Window $ProcessId $FolderObservationDirectory }
    else { Select-E2QaNativeFolderPath $Window $ProcessId $Destination $FolderObservationDirectory | Out-Null }
}

function New-E2QaScenarioDestination {
    param([Parameter(Mandatory)]$Lifecycle, [Parameter(Mandatory)][string]$Destination)
    $parent = Split-Path -Parent $Destination
    if (-not (Test-Path -LiteralPath $parent)) { New-Item -ItemType Directory -Path $parent -ErrorAction Stop | Out-Null; Register-E2QaOwnedTree $Lifecycle $parent }
    elseif (-not $Lifecycle.roots.Contains($parent)) { throw "Scenario destination parent was not created by this lifecycle: $parent" }
    if (Test-Path -LiteralPath $Destination) { throw "Scenario destination must be freshly absent: $Destination" }
    New-Item -ItemType Directory -Path $Destination -ErrorAction Stop | Out-Null; Register-E2QaOwnedTree $Lifecycle $Destination
}

function Resolve-E2QaInputMap {
    param([Parameter(Mandatory)][Windows.Automation.AutomationElement]$Window, [Parameter(Mandatory)][int]$ProcessId, [Parameter(Mandatory)][string]$Map)
    Invoke-E2QaNamedButton $Window (Get-E2QaText '\uc6d0\ubcf8 \ub9f5 \ub2e4\uc2dc \uc5f0\uacb0')
    Select-E2QaNativePath $ProcessId $Map
}

function Save-E2QaNativeState {
    param([Parameter(Mandatory)]$Lifecycle, [Parameter(Mandatory)][Windows.Automation.AutomationElement]$Window, [Parameter(Mandatory)][string]$Scenario, [Parameter(Mandatory)][string]$State)
    $dir = Join-Path $Lifecycle.evidence "native\$Scenario"
    New-Item -ItemType Directory -Path $dir -Force | Out-Null
    $tree = Join-Path $dir "$State-uia.json"
    $shot = Join-Path $dir "$State.png"
    Save-E2QaUiaTree $Window $tree
    $png = Save-E2QaWindowScreenshot $Window $shot
    [ordered]@{ tree = $tree; screenshot = $png }
}

function Invoke-E2QaFailureScenario {
    param(
        [Parameter(Mandatory)]$Lifecycle,
        [Parameter(Mandatory)][Windows.Automation.AutomationElement]$Window,
        [Parameter(Mandatory)][int]$ProcessId,
        [Parameter(Mandatory)][string]$Id,
        [Parameter(Mandatory)][string]$Source,
        [Parameter(Mandatory)][string]$Destination,
        [Parameter(Mandatory)][string[]]$ExpectedText,
        [Parameter(Mandatory)][string]$FolderObservationDirectory,
        [switch]$ObserveDestinationFolder,
        [string]$RelinkMap
    )
    if (-not (Test-Path -LiteralPath $Destination)) { New-E2QaScenarioDestination $Lifecycle $Destination }
    $before = Get-E2QaConfigSnapshot $Lifecycle.root
    Open-E2QaImportDialog $Window
    Set-E2QaImportInputs $Window $ProcessId $Source $Destination $FolderObservationDirectory -ObserveDestinationFolder:$ObserveDestinationFolder
    if ($RelinkMap) { Resolve-E2QaInputMap $Window $ProcessId $RelinkMap }
    Wait-E2QaCondition -TimeoutSeconds 30 -Failure "$Id did not reach its typed terminal UI" -Condition {
        $tree = Get-E2QaUiaTree $Window | ConvertTo-Json -Depth 5
        @($ExpectedText | Where-Object { $tree.IndexOf($_, [StringComparison]::Ordinal) -lt 0 }).Count -eq 0
    } | Out-Null
    if ($Id -eq 'ambiguous-map') {
        $relink = Wait-E2QaElement -Root $Window -Name (Get-E2QaText '\uc6d0\ubcf8 \ub9f5 \ub2e4\uc2dc \uc5f0\uacb0') -ControlType 'Button'
        $import = Wait-E2QaElement -Root $Window -Name (Get-E2QaText '\uc120\ud0dd\ud55c \uc704\uce58\ub85c \uac00\uc838\uc624\uae30') -ControlType 'Button'
        if (-not $relink.Current.IsEnabled -or $import.Current.IsEnabled) { throw 'Ambiguous-map recovery controls did not retain enabled relink and disabled import' }
    }
    $state = Save-E2QaNativeState $Lifecycle $Window $Id 'terminal'
    Assert-E2QaUiManifest $state.tree $ExpectedText
    $after = Get-E2QaConfigSnapshot $Lifecycle.root
    Assert-E2QaNoActivation $before $after $Id
    Write-E2QaScenarioReceipt (Join-Path $Lifecycle.evidence "native\$Id\receipt.json") $Id @{ state = $state; noActivation = $true }
    Close-E2QaImportDialog $Window
    Register-E2QaOwnedTree $Lifecycle $Destination
}

function Invoke-E2QaCancelScenario {
    param([Parameter(Mandatory)]$Lifecycle, [Parameter(Mandatory)][Windows.Automation.AutomationElement]$Window, [Parameter(Mandatory)][int]$ProcessId, [Parameter(Mandatory)]$Fixtures)
    $destination = Join-Path $Lifecycle.root 'projects\cancelled'
    New-E2QaScenarioDestination $Lifecycle $destination
    $before = Get-E2QaConfigSnapshot $Lifecycle.root
    Open-E2QaImportDialog $Window
    Set-E2QaImportInputs $Window $ProcessId $Fixtures.cancelLarge.source $destination (Join-Path $Lifecycle.evidence 'native\busy-cancel\folder-observation')
    Invoke-E2QaNamedButton $Window (Get-E2QaText '\uc120\ud0dd\ud55c \uc704\uce58\ub85c \uac00\uc838\uc624\uae30')
    $cancel = Wait-E2QaElement -Root $Window -Name (Get-E2QaText '\uac00\uc838\uc624\uae30 \ucde8\uc18c') -ControlType 'Button' -TimeoutSeconds 20
    if (-not $cancel.Current.IsEnabled) { throw 'Cancellation was never enabled during a real import' }
    $busy = Save-E2QaNativeState $Lifecycle $Window 'busy-cancel' 'progress'
    Invoke-E2QaElement $cancel
    Wait-E2QaElement -Root $Window -Name (Get-E2QaText '\uac00\uc838\uc624\uae30\ub97c \ucde8\uc18c\ud588\uc2b5\ub2c8\ub2e4. \uc6d0\ubcf8 \ud30c\uc77c\uacfc \ub300\uc0c1 \ud3f4\ub354 \uc0c1\ud0dc\ub97c \ud655\uc778\ud55c \ub4a4 \ub2e4\uc2dc \uc2dc\ub3c4\ud560 \uc218 \uc788\uc2b5\ub2c8\ub2e4.') -TimeoutSeconds 45 | Out-Null
    $terminal = Save-E2QaNativeState $Lifecycle $Window 'busy-cancel' 'cancelled'
    Assert-E2QaNoActivation $before (Get-E2QaConfigSnapshot $Lifecycle.root) 'busy-cancel'
    if (@(Get-ChildItem -LiteralPath $destination -Force).Count) { throw 'Cancelled destination is not empty' }
    Write-E2QaScenarioReceipt (Join-Path $Lifecycle.evidence 'native\busy-cancel\receipt.json') 'busy-cancel' @{ busy = $busy; terminal = $terminal; noActivation = $true }
    Register-E2QaOwnedTree $Lifecycle $destination
    Close-E2QaImportDialog $Window
}

function Invoke-E2QaSuccessScenario {
    param([Parameter(Mandatory)]$Lifecycle, [Parameter(Mandatory)][Windows.Automation.AutomationElement]$Window, [Parameter(Mandatory)][int]$ProcessId, [Parameter(Mandatory)]$Fixtures)
    $destination = Join-Path $Lifecycle.root 'projects\authentic-import'
    New-E2QaScenarioDestination $Lifecycle $destination
    $before = Get-E2QaConfigSnapshot $Lifecycle.root
    Open-E2QaImportDialog $Window
    Set-E2QaImportInputs $Window $ProcessId $Fixtures.variants.'missing-map'.source $destination (Join-Path $Lifecycle.evidence 'native\success\folder-observation')
    Resolve-E2QaInputMap $Window $ProcessId $Fixtures.authentic.map
    Invoke-E2QaNamedButton $Window (Get-E2QaText '\uc120\ud0dd\ud55c \uc704\uce58\ub85c \uac00\uc838\uc624\uae30')
    $progress = Wait-E2QaElement -Root $Window -Name (Get-E2QaText '\uac00\uc838\uc624\uae30 \ucde8\uc18c') -ControlType 'Button' -TimeoutSeconds 20
    if (-not $progress.Current.IsEnabled) { throw 'Authentic import did not expose cancellable progress' }
    $busy = Save-E2QaNativeState $Lifecycle $Window 'success' 'progress'
    Wait-E2QaCondition -TimeoutSeconds 180 -Failure 'Authentic import did not complete' -Condition { Test-Path -LiteralPath (Join-Path $destination 'project.eap') } | Out-Null
    Wait-E2QaCondition -TimeoutSeconds 30 -Failure 'Completed import did not close its terminal dialog' -Condition { $null -eq (Find-E2QaElement -Root $Window -Name (Get-E2QaText 'E2 \ud504\ub85c\uc81d\ud2b8 \uac00\uc838\uc624\uae30') -ControlType 'Window') } | Out-Null
    $completed = Save-E2QaNativeState $Lifecycle $Window 'success' 'completed'
    Assert-E2QaSingleActivation $before (Get-E2QaConfigSnapshot $Lifecycle.root) $destination
    Assert-E2QaPublishedProject $destination @('project.eap','src')
    Register-E2QaOwnedTree $Lifecycle $destination
    $retained = Save-E2QaAvailableArtifacts $Lifecycle 'import-completed'
    Write-E2QaScenarioReceipt (Join-Path $Lifecycle.evidence 'native\success\receipt.json') 'success' @{ project = $destination; progress = $busy; completed = $completed; activatedOnce = $true }
    $destination
}

function Invoke-E2QaEmptyGrpScenario {
    param([Parameter(Mandatory)]$Lifecycle, [Parameter(Mandatory)][Windows.Automation.AutomationElement]$Window, [Parameter(Mandatory)][int]$ProcessId, [Parameter(Mandatory)]$Fixtures)
    $destination = Join-Path $Lifecycle.root 'projects\empty-grp'
    New-E2QaScenarioDestination $Lifecycle $destination
    $before = Get-E2QaConfigSnapshot $Lifecycle.root
    Invoke-E2QaNamedButton $Window (Get-E2QaText '\uc124\uc815 \uc5f4\uae30')
    Invoke-E2QaNamedButton $Window (Get-E2QaText '\ud504\ub85c\uc81d\ud2b8')
    Invoke-E2QaNamedButton $Window (Get-E2QaText 'E2 \ud504\ub85c\uc81d\ud2b8 \uac00\uc838\uc624\uae30')
    Wait-E2QaElement -Root $Window -Name (Get-E2QaText 'E2S/E2P \uc120\ud0dd') -ControlType 'Button' | Out-Null
    Set-E2QaImportInputs $Window $ProcessId $Fixtures.variants.'empty-grp'.source $destination (Join-Path $Lifecycle.evidence 'native\empty-grp\folder-observation')
    Resolve-E2QaInputMap $Window $ProcessId $Fixtures.authentic.map
    Invoke-E2QaNamedButton $Window (Get-E2QaText '\uc120\ud0dd\ud55c \uc704\uce58\ub85c \uac00\uc838\uc624\uae30')
    Wait-E2QaCondition -TimeoutSeconds 180 -Failure 'True-empty GRP import did not complete' -Condition { Test-Path -LiteralPath (Join-Path $destination 'project.eap') } | Out-Null
    Wait-E2QaCondition -TimeoutSeconds 30 -Failure 'True-empty GRP import did not close its terminal dialog' -Condition { $null -eq (Find-E2QaElement -Root $Window -Name (Get-E2QaText 'E2 \ud504\ub85c\uc81d\ud2b8 \uac00\uc838\uc624\uae30') -ControlType 'Window') } | Out-Null
    $manifest = Get-Content -Raw -LiteralPath (Join-Path $destination 'project.eap') | ConvertFrom-Json
    $activeGrp = @($manifest.e2Import.resources | Where-Object { $_.provenance.feature -eq 'grp' })
    Assert-E2QaEqual $activeGrp.Count 0 'True-empty GRP created an immutable active-GRP resource'
    Assert-E2QaSingleActivation $before (Get-E2QaConfigSnapshot $Lifecycle.root) $destination
    $state = Save-E2QaNativeState $Lifecycle $Window 'empty-grp' 'completed'
    Register-E2QaOwnedTree $Lifecycle $destination
    Write-E2QaScenarioReceipt (Join-Path $Lifecycle.evidence 'native\empty-grp\receipt.json') 'empty-grp' @{ project = $destination; state = $state; immutableGrpResources = 0 }
}

function Save-E2QaSetupHost {
    param([Parameter(Mandatory)]$Lifecycle, [Parameter(Mandatory)][string]$AppPath, [Parameter(Mandatory)][string]$Euddraft)
    $configPath = Join-Path $Lifecycle.root 'roaming\eud-agent\config.json'
    $config = Get-Content -Raw -LiteralPath $configPath | ConvertFrom-Json
    $previousProjectPath = [string]$config.project_path
    $previousEuddraftPath = [string]$config.euddraft_path
    $config.project_path = ''
    $config.euddraft_path = ''
    Write-E2QaJson $configPath $config 12
    Register-E2QaOwnedFile $Lifecycle $configPath
    $setupApp = Start-E2QaApp $Lifecycle $AppPath
    $setupWindow = Wait-E2QaAppWindow $setupApp.Id -TimeoutSeconds 60
    Invoke-E2QaNamedButton $setupWindow (Get-E2QaText '\uc2dc\uc791 \uc124\uc815')
    Wait-E2QaElement -Root $setupWindow -Name (Get-E2QaText 'eud-agent \uc2dc\uc791 \uc124\uc815') -ControlType 'Text' | Out-Null
    Wait-E2QaElement -Root $setupWindow -Name (Get-E2QaText 'Native EUD \ud504\ub85c\uc81d\ud2b8') -ControlType 'Text' | Out-Null
    $setupCapture = Save-E2QaNativeState $Lifecycle $setupWindow 'setup-host' 'setup'
    Open-E2QaImportDialog $setupWindow
    $dialogCapture = Save-E2QaNativeState $Lifecycle $setupWindow 'setup-host' 'dialog'
    Close-E2QaImportDialog $setupWindow
    Stop-E2QaOwnedProcess $Lifecycle $setupApp
    $config = Get-Content -Raw -LiteralPath $configPath | ConvertFrom-Json
    $config.project_path = $previousProjectPath
    $config.euddraft_path = $previousEuddraftPath
    Write-E2QaJson $configPath $config 12
    Register-E2QaOwnedFile $Lifecycle $configPath
    [ordered]@{ setup = $setupCapture; dialog = $dialogCapture }
}

function Invoke-E2QaHostReopenAndBuild {
    param([Parameter(Mandatory)]$Lifecycle, [Parameter(Mandatory)][Windows.Automation.AutomationElement]$Window, [Parameter(Mandatory)][string]$ProjectRoot)
    Invoke-E2QaNamedButton $Window (Get-E2QaText '\uc124\uc815 \uc5f4\uae30')
    Invoke-E2QaNamedButton $Window (Get-E2QaText '\ud504\ub85c\uc81d\ud2b8')
    Open-E2QaImportDialog $Window
    $settings = Save-E2QaNativeState $Lifecycle $Window 'settings-host' 'dialog'
    Close-E2QaImportDialog $Window
    $Window.SetFocus()
    [Windows.Forms.SendKeys]::SendWait('{ESC}')
    Wait-E2QaElement -Root $Window -Name (Get-E2QaText '\ud504\ub85c\uc81d\ud2b8 \ube4c\ub4dc') -ControlType 'Button' -TimeoutSeconds 10 | Out-Null
    $editable = Get-ChildItem -LiteralPath (Join-Path $ProjectRoot 'src') -Filter '*.eps' -File -Recurse | Select-Object -First 1
    if ($null -eq $editable) { throw 'Imported project has no editable EPS source' }
    $revision = "`r`n// Task18 native reopen revision $([DateTime]::UtcNow.ToString('o'))`r`n"
    [IO.File]::AppendAllText($editable.FullName, $revision, [Text.Encoding]::UTF8)
    Register-E2QaOwnedFile $Lifecycle $editable.FullName -Refresh
    $manifestPath = Join-Path $ProjectRoot 'project.eap'
    $manifest = Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json
    $activeRoot = [string](Get-Content -Raw -LiteralPath (Join-Path $Lifecycle.root 'roaming\eud-agent\config.json') | ConvertFrom-Json).project_path
    if ([string]::IsNullOrWhiteSpace($activeRoot) -or -not [IO.Path]::IsPathRooted($activeRoot)) { throw 'Active project path is missing after E2 adoption' }
    $activeRoot = [IO.Path]::GetFullPath($activeRoot).TrimEnd('\')
    if ($activeRoot.StartsWith('\\?\UNC\', [StringComparison]::OrdinalIgnoreCase)) { $activeRoot = '\\' + $activeRoot.Substring(8) }
    elseif ($activeRoot.StartsWith('\\?\', [StringComparison]::OrdinalIgnoreCase)) { $activeRoot = $activeRoot.Substring(4) }
    $activeManifest = Join-Path $activeRoot 'project.eap'
    if (-not (Test-Path -LiteralPath $activeManifest -PathType Leaf) -or (Get-E2QaHash $activeManifest) -ne (Get-E2QaHash $manifestPath)) {
        throw 'Active project path does not identify the adopted project manifest'
    }
    $outputPath = Join-Path $activeRoot $manifest.outputMap
    $logPath = Join-Path $activeRoot 'build\euddraft\build.log'
    if (Test-Path -LiteralPath $outputPath) { throw "Expected output must be absent before UI build: $outputPath" }
    if (Test-Path -LiteralPath $logPath) { throw "Expected log must be absent before UI build: $logPath" }
    $started = [DateTime]::UtcNow
    Invoke-E2QaNamedButton $Window (Get-E2QaText '\ud504\ub85c\uc81d\ud2b8 \ube4c\ub4dc')
    $buildResultName = Get-E2QaText '\ube4c\ub4dc \uacb0\uacfc \uc131\uacf5'
    $buildResult = Wait-E2QaElement -Root $Window -Name $buildResultName -ControlType 'Window' -TimeoutSeconds 300
    $terminal = Save-E2QaNativeState $Lifecycle $Window 'success' 'build-result'
    Assert-E2QaUiManifest $terminal.tree @((Get-E2QaText '\uc131\uacf5'),$outputPath,$logPath,'0')
    Wait-E2QaCondition -TimeoutSeconds 30 -Failure 'Successful UI verdict lacked exact output/log artifacts' -Condition {
        (Test-Path -LiteralPath $outputPath -PathType Leaf) -and (Test-Path -LiteralPath $logPath -PathType Leaf)
    } | Out-Null
    Register-E2QaOwnedTree $Lifecycle $ProjectRoot
    $retained = Save-E2QaAvailableArtifacts $Lifecycle 'reopen-build'
    $build = Assert-E2QaFreshBuild $outputPath $logPath $started
    Write-E2QaScenarioReceipt (Join-Path $Lifecycle.evidence 'native\success\reopen-build.json') 'reopen-build' @{ settingsHost = $settings; terminal = $terminal; edited = $editable.FullName; build = $build; retained = $retained }
    $closeName = Get-E2QaText '\ub2eb\uae30'
    $closeButtons = @($buildResult.FindAll([Windows.Automation.TreeScope]::Descendants, [Windows.Automation.Condition]::TrueCondition) | Where-Object { $_.Current.Name -eq $closeName -and $_.Current.ControlType.ProgrammaticName -eq 'ControlType.Button' -and $_.Current.IsEnabled })
    if ($closeButtons.Count -ne 1) { throw "Build result close button is not uniquely enabled: count=$($closeButtons.Count)" }
    Invoke-E2QaElement $closeButtons[0]
    Wait-E2QaCondition -TimeoutSeconds 10 -Failure 'Build result dialog did not close' -Condition { $null -eq (Find-E2QaElement -Root $Window -Name $buildResultName -ControlType 'Window') } | Out-Null
}

function Invoke-E2QaNativeMatrix {
    param([Parameter(Mandatory)]$Lifecycle, [Parameter(Mandatory)][Diagnostics.Process]$App, [Parameter(Mandatory)]$Fixtures, [Parameter(Mandatory)][string]$AppPath, [Parameter(Mandatory)][string]$Euddraft, [switch]$ObserveDestinationFolder)
    Initialize-E2QaUia
    $window = Wait-E2QaAppWindow $App.Id -TimeoutSeconds 60
    $windowDir = Join-Path $Lifecycle.evidence 'native'; New-Item -ItemType Directory -Path $windowDir -Force | Out-Null
    Write-E2QaJson (Join-Path $windowDir 'main-window.json') $script:E2QaLastMainWindowIdentity 5
    Invoke-E2QaFailureScenario $Lifecycle $window $App.Id 'ambiguous-map' $Fixtures.variants.'ambiguous-map'.source (Join-Path $Lifecycle.root 'projects\ambiguous') @((Get-E2QaText '\uc6d0\ubcf8 \ub9f5 \ub2e4\uc2dc \uc5f0\uacb0')) (Join-Path $Lifecycle.evidence 'native\ambiguous-map\folder-observation') -ObserveDestinationFolder:$ObserveDestinationFolder
    Invoke-E2QaFailureScenario $Lifecycle $window $App.Id 'nonempty-destination' $Fixtures.authentic.source $Fixtures.nonempty.root @((Get-E2QaText '\ube44\uc5b4 \uc788\uc9c0 \uc54a\uc2b5\ub2c8\ub2e4')) (Join-Path $Lifecycle.evidence 'native\nonempty-destination\folder-observation')
    Assert-E2QaSentinel $Fixtures.nonempty.sentinel $Fixtures.nonempty.sha256
    Invoke-E2QaFailureScenario $Lifecycle $window $App.Id 'active-scdb' $Fixtures.variants.'active-scdb'.source (Join-Path $Lifecycle.root 'projects\scdb') @('SCDB') (Join-Path $Lifecycle.evidence 'native\active-scdb\folder-observation')
    Invoke-E2QaFailureScenario $Lifecycle $window $App.Id 'effective-grp' $Fixtures.variants.'effective-grp'.source (Join-Path $Lifecycle.root 'projects\grp') @('GRP') (Join-Path $Lifecycle.evidence 'native\effective-grp\folder-observation')
    Invoke-E2QaCancelScenario $Lifecycle $window $App.Id $Fixtures
    $project = Invoke-E2QaSuccessScenario $Lifecycle $window $App.Id $Fixtures
    Stop-E2QaOwnedProcess $Lifecycle $App
    $setup = Save-E2QaSetupHost $Lifecycle $AppPath $Euddraft
    $mainApp = Start-E2QaApp $Lifecycle $AppPath
    $mainWindow = Wait-E2QaAppWindow $mainApp.Id -TimeoutSeconds 60
    Invoke-E2QaHostReopenAndBuild $Lifecycle $mainWindow $project
    Invoke-E2QaEmptyGrpScenario $Lifecycle $mainWindow $mainApp.Id $Fixtures
    Stop-E2QaOwnedProcess $Lifecycle $mainApp
    Register-E2QaOwnedTree $Lifecycle (Join-Path $Lifecycle.root 'roaming')
    Register-E2QaOwnedTree $Lifecycle (Join-Path $Lifecycle.root 'local')
    Register-E2QaOwnedTree $Lifecycle (Join-Path $Lifecycle.root 'webview2')
    [ordered]@{ project = $project; setupHost = $setup; scenarios = @('ambiguous-map','nonempty-destination','active-scdb','effective-grp','busy-cancel','empty-grp','success','setup-host','settings-host','reopen-build') }
}
