Set-StrictMode -Version Latest
$script:E2QaLastPickerObservation = $null

function Initialize-E2QaUia {
    Add-Type -AssemblyName UIAutomationClient
    Add-Type -AssemblyName UIAutomationTypes
    Add-Type -AssemblyName System.Windows.Forms
    Add-Type -AssemblyName System.Drawing
    if (-not ('E2Qa.NativeWindow' -as [type])) {
        Add-Type -ReferencedAssemblies @(([Windows.Automation.AutomationElement].Assembly.Location), ([Windows.Automation.AutomationProperty].Assembly.Location)) @'
using System;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Windows.Automation;
namespace E2Qa {
  public static class NativeWindow {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);
    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)] static extern int GetClassNameW(IntPtr hwnd, StringBuilder className, int maximumCount);
    [DllImport("user32.dll", SetLastError = true)] public static extern int GetDlgCtrlID(IntPtr hwnd);
    [DllImport("user32.dll", SetLastError = true)] static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint processId);
    [DllImport("user32.dll")] public static extern bool IsChild(IntPtr parent, IntPtr child);
    [DllImport("user32.dll", SetLastError = true)] static extern IntPtr SendMessageTimeoutW(IntPtr hwnd, uint message, UIntPtr wParam, IntPtr lParam, uint flags, uint timeout, out UIntPtr result);
    public static string GetWindowClass(IntPtr hwnd) { var value = new StringBuilder(256); if (GetClassNameW(hwnd, value, value.Capacity) == 0) throw new InvalidOperationException("GetClassNameW failed"); return value.ToString(); }
    public static uint GetWindowProcessId(IntPtr hwnd) { uint processId; if (GetWindowThreadProcessId(hwnd, out processId) == 0) throw new InvalidOperationException("GetWindowThreadProcessId failed"); return processId; }
    public static bool ClickButton(IntPtr hwnd) { UIntPtr result; return SendMessageTimeoutW(hwnd, 0x00F5, UIntPtr.Zero, IntPtr.Zero, 0x22, 5000, out result) != IntPtr.Zero; }
    public sealed class EnabledLatch : IDisposable {
      readonly AutomationElement element; readonly AutoResetEvent signal = new AutoResetEvent(false);
      readonly AutomationPropertyChangedEventHandler handler; volatile bool disposed; bool registered;
      public EnabledLatch(AutomationElement element) {
        this.element = element; handler = Changed;
        try { Automation.AddAutomationPropertyChangedEventHandler(element, TreeScope.Element, handler, AutomationElement.IsEnabledProperty); registered = true; if (element.Current.IsEnabled) signal.Set(); }
        catch { if (registered) Automation.RemoveAutomationPropertyChangedEventHandler(element, handler); signal.Dispose(); throw; }
      }
      void Changed(object sender, AutomationPropertyChangedEventArgs e) {
        if (e.Property == AutomationElement.IsEnabledProperty && e.NewValue is bool && (bool)e.NewValue) { try { if (!disposed) signal.Set(); } catch (ObjectDisposedException) {} }
      }
      public bool Wait(int milliseconds) { return signal.WaitOne(milliseconds); }
      public void Dispose() { if (disposed) return; disposed = true; if (registered) Automation.RemoveAutomationPropertyChangedEventHandler(element, handler); signal.Dispose(); }
    }
  }
}
'@
    }
}

function Get-E2QaControlType {
    param([Parameter(Mandatory)][string]$Name)
    $field = [Windows.Automation.ControlType].GetField($Name, [Reflection.BindingFlags]'Public,Static')
    if ($null -eq $field) { throw "Unknown UIA control type: $Name" }
    $field.GetValue($null)
}

function Find-E2QaElement {
    param(
        [Parameter(Mandatory)][Windows.Automation.AutomationElement]$Root,
        [string]$Name,
        [string]$AutomationId,
        [string]$ControlType
    )
    $conditions = [Collections.Generic.List[Windows.Automation.Condition]]::new()
    if ($Name) { $conditions.Add([Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, $Name)) }
    if ($AutomationId) { $conditions.Add([Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::AutomationIdProperty, $AutomationId)) }
    if ($ControlType) { $conditions.Add([Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty, (Get-E2QaControlType $ControlType))) }
    if (-not $conditions.Count) { throw 'At least one UIA selector is required' }
    $condition = if ($conditions.Count -eq 1) { $conditions[0] } else { [Windows.Automation.AndCondition]::new($conditions.ToArray()) }
    $Root.FindFirst([Windows.Automation.TreeScope]::Descendants, $condition)
}

function Wait-E2QaElement {
    param(
        [Parameter(Mandatory)][Windows.Automation.AutomationElement]$Root,
        [string]$Name,
        [string]$AutomationId,
        [string]$ControlType,
        [int]$TimeoutSeconds = 30
    )
    Wait-E2QaCondition -TimeoutSeconds $TimeoutSeconds -Failure "UIA element timed out: $Name/$AutomationId/$ControlType" -Condition {
        Find-E2QaElement -Root $Root -Name $Name -AutomationId $AutomationId -ControlType $ControlType
    }
}

function Wait-E2QaAppWindow {
    param([Parameter(Mandatory)][int]$ProcessId, [int]$TimeoutSeconds = 30)
    $desktop = [Windows.Automation.AutomationElement]::RootElement
    $window = Wait-E2QaCondition -TimeoutSeconds $TimeoutSeconds -Failure "Unique main window for PID $ProcessId did not appear" -Condition {
        $matches = @($desktop.FindAll([Windows.Automation.TreeScope]::Children, [Windows.Automation.Condition]::TrueCondition) | ForEach-Object {
            try {
                $current = $_.Current
                if ($current.ProcessId -eq $ProcessId -and $current.NativeWindowHandle -ne 0 -and $current.ControlType -eq [Windows.Automation.ControlType]::Window -and @($_.GetSupportedPatterns() | Where-Object { $_ -eq [Windows.Automation.WindowPattern]::Pattern }).Count) { $_ }
            } catch { }
        })
        if ($matches.Count -gt 1) { throw ("Ambiguous main window for PID " + $ProcessId + ": " + $matches.Count + " exact WindowPattern roots") }
        if ($matches.Count -eq 1) { $matches[0] }
    }
    $script:E2QaLastMainWindowIdentity = [ordered]@{ processId = $window.Current.ProcessId; nativeWindowHandle = [string]$window.Current.NativeWindowHandle; runtimeId = (@($window.GetRuntimeId()) -join ','); controlType = $window.Current.ControlType.ProgrammaticName }
    $window.SetFocus()
    $window
}

function Invoke-E2QaElement {
    param([Parameter(Mandatory)][Windows.Automation.AutomationElement]$Element)
    $pattern = $Element.GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern)
    ([Windows.Automation.InvokePattern]$pattern).Invoke()
}

function Set-E2QaElementValue {
    param([Parameter(Mandatory)][Windows.Automation.AutomationElement]$Element, [Parameter(Mandatory)][string]$Value)
    $pattern = $Element.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern)
    ([Windows.Automation.ValuePattern]$pattern).SetValue($Value)
}

function Select-E2QaNativePath {
    param([Parameter(Mandatory)][int]$ProcessId, [Parameter(Mandatory)][string]$Path)
    if (-not [IO.Path]::IsPathRooted($Path) -or $Path -match '^[A-Za-z]:[^\\/]') { throw "Native picker path must be absolute: $Path" }
    $desktop = [Windows.Automation.AutomationElement]::RootElement
    $script:E2QaLastPickerObservation = $null
    $dialog = $null
    $stage = 'discover'
    try {
        $dialog = Wait-E2QaCondition -TimeoutSeconds 20 -Failure 'Native picker window did not appear' -Condition {
            $windows = $desktop.FindAll([Windows.Automation.TreeScope]::Children, [Windows.Automation.Condition]::TrueCondition)
            @($windows) | Where-Object { $_.Current.ProcessId -eq $ProcessId -and (Find-E2QaElement -Root $_ -AutomationId '1148' -ControlType 'Edit') } | Select-Object -First 1
        }
        $stage = 'edit'
        $edit = Wait-E2QaElement -Root $dialog -AutomationId '1148' -ControlType 'Edit'
        $stage = 'value'
        Set-E2QaElementValue $edit $Path
        $stage = 'invoke'
        $before = $null; $beforeCaptureError = $null
        try {
            $readback = $null; $readbackError = $null
            try { $readback = ([Windows.Automation.ValuePattern]$edit.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern)).Current.Value } catch { $readbackError = $_.Exception.Message }
            if ($null -ne $readbackError -or $readback -ne $Path) { throw 'Native picker Edit1148 value readback does not match the requested path' }
            $identityProbe = [ordered]@{ completeCount = $null; truncated = $null; tuples = @(); queryError = $null; tupleErrorCount = 0; tupleErrors = @() }
            try {
                $idOne = @($dialog.FindAll([Windows.Automation.TreeScope]::Descendants, [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::AutomationIdProperty, '1')))
                $identityProbe.completeCount = $idOne.Count; $identityProbe.truncated = ($idOne.Count -gt 8)
                $tuple = [ordered]@{ count = 0; errors = [Collections.Generic.List[string]]::new() }
                $identityProbe.tuples = @($idOne | Select-Object -First 8 | ForEach-Object { $element = $_; try { [ordered]@{ name = $element.Current.Name; automationId = $element.Current.AutomationId; controlType = $element.Current.ControlType.ProgrammaticName; runtimeId = (@($element.GetRuntimeId()) -join ','); enabled = $element.Current.IsEnabled; patterns = @($element.GetSupportedPatterns() | ForEach-Object { $_.ProgrammaticName }) } } catch { $tuple.count++; if ($tuple.errors.Count -lt 8) { $tuple.errors.Add($_.Exception.Message) }; [ordered]@{ unavailable = $true } } })
                $identityProbe.tupleErrorCount = $tuple.count; $identityProbe.tupleErrors = @($tuple.errors)
            } catch { $identityProbe.queryError = $_.Exception.Message }
            $dialog.SetFocus()
            $edit.SetFocus()
            $editRuntime = (@($edit.GetRuntimeId()) -join ',')
            $focused = Wait-E2QaCondition -TimeoutSeconds 5 -Failure 'Native picker Edit1148 focus did not retain the exact PID and runtime ID' -Condition {
                try {
                    $candidate = [Windows.Automation.AutomationElement]::FocusedElement
                    if ($null -ne $candidate -and $candidate.Current.ProcessId -eq $ProcessId -and (@($candidate.GetRuntimeId()) -join ',') -eq $editRuntime) { $candidate }
                } catch { $false }
            }
            $focusedRuntime = (@($focused.GetRuntimeId()) -join ',')
            if ($edit.Current.ProcessId -ne $ProcessId -or $focused.Current.ProcessId -ne $ProcessId -or $focusedRuntime -ne $editRuntime) { throw 'Native picker Edit1148 focus did not retain the exact PID and runtime ID' }
            $openName = Get-E2QaText '\uc5f4\uae30(O)'
            $openCandidates = @($dialog.FindAll([Windows.Automation.TreeScope]::Descendants, [Windows.Automation.Condition]::TrueCondition) | Where-Object { $_.Current.AutomationId -eq '1' -and $_.Current.Name -eq $openName -and $_.Current.ControlType.ProgrammaticName -in @('ControlType.Button','ControlType.Pane') })
            if ($openCandidates.Count -ne 1) { throw "Native picker default button identity is not unique: count=$($openCandidates.Count)" }
            $openPane = $openCandidates[0]; $dialogHwnd = [IntPtr]$dialog.Current.NativeWindowHandle; $buttonHwnd = [IntPtr]$openPane.Current.NativeWindowHandle
            if (-not $openPane.Current.IsEnabled -or $dialogHwnd -eq [IntPtr]::Zero -or $buttonHwnd -eq [IntPtr]::Zero) { throw 'Native picker default button is disabled or lacks a native window handle' }
            $buttonClass = [E2Qa.NativeWindow]::GetWindowClass($buttonHwnd); $buttonControlId = [E2Qa.NativeWindow]::GetDlgCtrlID($buttonHwnd); $buttonProcessId = [E2Qa.NativeWindow]::GetWindowProcessId($buttonHwnd)
            if ($openPane.Current.ProcessId -ne $ProcessId -or $buttonProcessId -ne $ProcessId -or $buttonClass -cne 'Button' -or $buttonControlId -ne 1 -or -not [E2Qa.NativeWindow]::IsChild($dialogHwnd, $buttonHwnd)) { throw 'Native picker default button did not satisfy the exact PID, class, control ID, and dialog-child boundary' }
            $defaultButton = [ordered]@{ processId = $buttonProcessId; nativeWindowHandle = [string]$buttonHwnd; runtimeId = (@($openPane.GetRuntimeId()) -join ','); name = $openPane.Current.Name; automationId = $openPane.Current.AutomationId; controlType = $openPane.Current.ControlType.ProgrammaticName; enabled = $openPane.Current.IsEnabled; win32Class = $buttonClass; controlId = $buttonControlId; dialogNativeWindowHandle = [string]$dialogHwnd; isDialogChild = $true }
            $before = [ordered]@{ processId = $dialog.Current.ProcessId; nativeWindowHandle = [string]$dialog.Current.NativeWindowHandle; runtimeId = (@($dialog.GetRuntimeId()) -join ','); requestPath = $Path; edit = [ordered]@{ processId = $edit.Current.ProcessId; automationId = $edit.Current.AutomationId; controlType = $edit.Current.ControlType.ProgrammaticName; runtimeId = $editRuntime; valueReadback = $readback; valueReadbackError = $readbackError }; focusedElement = [ordered]@{ processId = $focused.Current.ProcessId; runtimeId = $focusedRuntime }; automationId1 = $identityProbe; defaultButton = $defaultButton }
        } catch { $beforeCaptureError = $_.Exception.Message }
        if ($null -ne $beforeCaptureError) { throw "Native picker exact Edit1148 default action cannot be proven: $beforeCaptureError" }
        $stage = 'default-action'
        if (-not [E2Qa.NativeWindow]::ClickButton($buttonHwnd)) { throw 'Native picker default button BM_CLICK failed or timed out' }
        $observePicker = {
            try {
                $filter = [ordered]@{ count = 0; errors = [Collections.Generic.List[string]]::new() }
                $owned = @($desktop.FindAll([Windows.Automation.TreeScope]::Children, [Windows.Automation.Condition]::TrueCondition) | ForEach-Object { $element = $_; try { if ($element.Current.ProcessId -eq $ProcessId) { $element } } catch { $filter.count++; if ($filter.errors.Count -lt 8) { $filter.errors.Add($_.Exception.Message) } } })
                $tuple = [ordered]@{ count = 0; errors = [Collections.Generic.List[string]]::new() }
                $tuples = @($owned | Select-Object -First 24 | ForEach-Object { $element = $_; try { [ordered]@{ nativeWindowHandle = [string]$element.Current.NativeWindowHandle; runtimeId = (@($element.GetRuntimeId()) -join ','); name = $element.Current.Name; controlType = $element.Current.ControlType.ProgrammaticName; enabled = $element.Current.IsEnabled; offscreen = $element.Current.IsOffscreen } } catch { $tuple.count++; if ($tuple.errors.Count -lt 8) { $tuple.errors.Add($_.Exception.Message) }; [ordered]@{ unavailable = $true } } })
                $found = @($tuples | Where-Object { $_.Contains('nativeWindowHandle') -and $_.Contains('runtimeId') -and $_.nativeWindowHandle -eq $before.nativeWindowHandle -and $_.runtimeId -eq $before.runtimeId }).Count -gt 0
                $identity = if ($found) { 'present' } elseif ($filter.count -eq 0 -and $owned.Count -le 24 -and $tuple.count -eq 0) { 'absent' } else { 'unknown' }
                [ordered]@{ observation = [ordered]@{ ownedTopLevelCount = $owned.Count; ownedTopLevelMaximum = 24; ownedTopLevelTruncated = ($owned.Count -gt 24); ownedTopLevel = $tuples; ownedTopLevelFilterErrorCount = $filter.count; ownedTopLevelFilterErrors = @($filter.errors); ownedTopLevelTupleErrorCount = $tuple.count; ownedTopLevelTupleErrors = @($tuple.errors); pickerIdentity = $identity }; captureError = $null }
            } catch { [ordered]@{ observation = $null; captureError = $_.Exception.Message } }
        }
        $completionState = [ordered]@{ attempts = 0; last = (& $observePicker) }
        $script:E2QaLastPickerObservation = [ordered]@{ processId = $ProcessId; before = $before; beforeCaptureError = $beforeCaptureError; immediateAfterInvoke = $completionState.last.observation; afterCaptureError = $completionState.last.captureError; completion = $null }
        $stage = 'completion'
        try {
            $completed = Wait-E2QaCondition -TimeoutSeconds 20 -Failure 'Native picker exact identity did not become absent' -Condition {
                $completionState.attempts++
                $completionState.last = & $observePicker
                $script:E2QaLastPickerObservation.completion = [ordered]@{ attempts = $completionState.attempts; final = $completionState.last.observation; captureError = $completionState.last.captureError }
                if ($null -eq $completionState.last.captureError -and $completionState.last.observation.pickerIdentity -eq 'absent') { $completionState.last.observation }
            }
        } catch {
            $identity = if ($null -ne $completionState.last.observation) { $completionState.last.observation.pickerIdentity } else { 'unknown' }
            throw "Native picker completion was not proven: identity=$identity captureError=$($completionState.last.captureError)"
        }
    } catch {
        $primary = $_
        $diagnostic = try {
            $windows = @($desktop.FindAll([Windows.Automation.TreeScope]::Children, [Windows.Automation.Condition]::TrueCondition))
            $appWindows = @($windows | Where-Object { $_.Current.ProcessId -eq $ProcessId })
            $pickerTopLevel = @($windows | Where-Object { Find-E2QaElement -Root $_ -AutomationId '1148' -ControlType 'Edit' }).Count; $ownedPickerTopLevel = @($appWindows | Where-Object { Find-E2QaElement -Root $_ -AutomationId '1148' -ControlType 'Edit' }).Count
            $descendantWindows = @($appWindows | ForEach-Object { $_.FindAll([Windows.Automation.TreeScope]::Descendants, [Windows.Automation.Condition]::TrueCondition) | Where-Object { $_.Current.ControlType -eq [Windows.Automation.ControlType]::Window } }).Count
            $selectedRoot = if ($null -ne $dialog -and $dialog.Current.ProcessId -eq $ProcessId) { $dialog } else { $null }
            $identity = if ($null -ne $selectedRoot) { [ordered]@{ processId = $selectedRoot.Current.ProcessId; nativeWindowHandle = [string]$selectedRoot.Current.NativeWindowHandle; name = $selectedRoot.Current.Name; automationId = $selectedRoot.Current.AutomationId; controlType = $selectedRoot.Current.ControlType.ProgrammaticName } | ConvertTo-Json -Compress } else { 'null' }
            $tuples = @(if ($null -ne $selectedRoot) {
                @($selectedRoot.FindAll([Windows.Automation.TreeScope]::Subtree, [Windows.Automation.Condition]::TrueCondition) | Where-Object {
                    $current = $_.Current
                    $current.ProcessId -eq $ProcessId -and ($current.ControlType -eq [Windows.Automation.ControlType]::Button -or ($current.AutomationId -eq '1148' -and ($current.ControlType -eq [Windows.Automation.ControlType]::Edit -or $current.ControlType -eq [Windows.Automation.ControlType]::ComboBox)))
                } | Select-Object -First 24 | ForEach-Object {
                    $current = $_.Current; $patterns = @()
                    try { $patterns = @($_.GetSupportedPatterns() | ForEach-Object { $_.ProgrammaticName }) } catch { $patterns = @('unavailable') }
                    [ordered]@{ name = $current.Name; automationId = $current.AutomationId; controlType = $current.ControlType.ProgrammaticName; enabled = $current.IsEnabled; patterns = $patterns }
                })
            } else { @() })
            $selectedTree = if ($null -ne $selectedRoot) { @(Get-E2QaUiaTree -Root $selectedRoot -Maximum 64) | ConvertTo-Json -Compress } else { '[]' }
            $appFallbackTree = if ($null -eq $selectedRoot -and $appWindows.Count) { @(Get-E2QaUiaTree -Root $appWindows[0] -Maximum 64) | ConvertTo-Json -Compress } else { '[]' }
            "stage=$stage appTop=$($appWindows.Count) pickerTop=$pickerTopLevel ownedPickerTop=$ownedPickerTopLevel appDescendantWindow=$descendantWindows selectedRoot=$identity selectedTupleCount=$($tuples.Count) selectedTuples=$($tuples | ConvertTo-Json -Depth 4 -Compress) selectedTree=$selectedTree appFallback=$($null -eq $selectedRoot) appFallbackTree=$appFallbackTree"
        } catch { "stage=$stage selectedRoot=unavailable selectedTuples=[] selectedTree=[] appFallback=unavailable appFallbackTree=[] diagnostic=unavailable" }
        throw "$($primary.Exception.Message); $diagnostic"
    }
}

function Get-E2QaFolderDialogTopLevels {
    param([Parameter(Mandatory)][Windows.Automation.AutomationElement]$Desktop, [Parameter(Mandatory)][int]$ProcessId)
    $roots = [Collections.Generic.List[object]]::new()
    foreach ($element in @($Desktop.FindAll([Windows.Automation.TreeScope]::Children, [Windows.Automation.Condition]::TrueCondition))) {
        try {
            $current = $element.Current; $runtimeId = @($element.GetRuntimeId()) -join ','
            if ($current.ProcessId -eq $ProcessId -and $current.NativeWindowHandle -ne 0 -and -not [string]::IsNullOrWhiteSpace($runtimeId)) {
                $roots.Add([pscustomobject]@{ element = $element; nativeWindowHandle = [string]$current.NativeWindowHandle; runtimeId = $runtimeId; name = $current.Name; controlType = $current.ControlType.ProgrammaticName })
            }
        } catch { }
    }
    @($roots)
}

function Observe-E2QaNativeFolderDialog {
    param([Parameter(Mandatory)][Windows.Automation.AutomationElement]$Window, [Parameter(Mandatory)][int]$ProcessId, [Parameter(Mandatory)][string]$EvidenceDirectory)
    if (-not [IO.Path]::IsPathRooted($EvidenceDirectory)) { throw "Folder observation directory must be absolute: $EvidenceDirectory" }
    if (Test-Path -LiteralPath $EvidenceDirectory) { throw "Folder observation directory must be absent: $EvidenceDirectory" }
    New-Item -ItemType Directory -Path $EvidenceDirectory -ErrorAction Stop | Out-Null
    $desktop = [Windows.Automation.AutomationElement]::RootElement; $before = @(Get-E2QaFolderDialogTopLevels $desktop $ProcessId)
    $beforeKeys = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    foreach ($item in $before) { [void]$beforeKeys.Add("$($item.nativeWindowHandle)|$($item.runtimeId)") }
    Invoke-E2QaNamedButton $Window (Get-E2QaText '\uc0c8 \uc791\uc5c5 \ud3f4\ub354 \uc120\ud0dd')
    $appeared = Wait-E2QaCondition -TimeoutSeconds 20 -Failure 'Folder picker did not expose a newly admitted PID-owned top-level' -Condition {
        $newRoots = @(Get-E2QaFolderDialogTopLevels $desktop $ProcessId | Where-Object { -not $beforeKeys.Contains("$($_.nativeWindowHandle)|$($_.runtimeId)") })
        if ($newRoots.Count) { [pscustomobject]@{ roots = $newRoots } }
    }
    $captures = [Collections.Generic.List[object]]::new(); $index = 0
    foreach ($candidate in @($appeared.roots)) {
        $index++; $all = @($candidate.element.FindAll([Windows.Automation.TreeScope]::Subtree, [Windows.Automation.Condition]::TrueCondition)); $maximum = [Math]::Min(600, [Math]::Max(1, $all.Count))
        $tree = @(Get-E2QaUiaTree -Root $candidate.element -Maximum $maximum -CaptureDetails)
        $capture = [ordered]@{ processId = $ProcessId; nativeWindowHandle = $candidate.nativeWindowHandle; runtimeId = $candidate.runtimeId; name = $candidate.name; controlType = $candidate.controlType; completeCount = $all.Count; capturedCount = $tree.Count; truncated = ($all.Count -gt $maximum -or $tree.Count -ne $all.Count); elements = $tree; screenshot = (Join-Path $EvidenceDirectory "folder-root-$index.png"); screenshotError = $null }
        $captures.Add($capture); $capturePath = Join-Path $EvidenceDirectory "folder-root-$index.json"; Write-E2QaJson $capturePath $capture 12
        try { Save-E2QaWindowScreenshot $candidate.element $capture.screenshot } catch { $capture.screenshotError = $_.Exception.Message; Write-E2QaJson $capturePath $capture 12 }
    }
    Write-E2QaJson (Join-Path $EvidenceDirectory 'folder-dialog-observation.json') ([ordered]@{ processId = $ProcessId; preClickTopLevels = @($before | ForEach-Object { [ordered]@{ nativeWindowHandle = $_.nativeWindowHandle; runtimeId = $_.runtimeId; name = $_.name; controlType = $_.controlType } }); newlyAdmittedRootCount = $captures.Count; roots = @($captures); action = 'observation-only-no-folder-control-invoked' }) 12
    throw "Folder dialog observation retained for $($captures.Count) newly admitted root(s); bind a unique observed folder control before destination selection"
}

function Select-E2QaNativeFolderPath {
    param([Parameter(Mandatory)][Windows.Automation.AutomationElement]$Window, [Parameter(Mandatory)][int]$ProcessId, [Parameter(Mandatory)][string]$Path, [Parameter(Mandatory)][string]$EvidenceDirectory)
    if (-not [IO.Path]::IsPathRooted($Path)) { throw "Folder picker path must be absolute: $Path" }
    if (Test-Path -LiteralPath $EvidenceDirectory) { throw "Folder action evidence directory must be absent: $EvidenceDirectory" }
    New-Item -ItemType Directory -Path $EvidenceDirectory -ErrorAction Stop | Out-Null
    $desktop = [Windows.Automation.AutomationElement]::RootElement; $before = @(Get-E2QaFolderDialogTopLevels $desktop $ProcessId)
    $keys = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal); foreach ($item in $before) { [void]$keys.Add("$($item.nativeWindowHandle)|$($item.runtimeId)") }
    Invoke-E2QaNamedButton $Window (Get-E2QaText '\uc0c8 \uc791\uc5c5 \ud3f4\ub354 \uc120\ud0dd')
    $roots = Wait-E2QaCondition -TimeoutSeconds 20 -Failure 'Folder picker did not expose one newly admitted PID-owned root' -Condition {
        $new = @(Get-E2QaFolderDialogTopLevels $desktop $ProcessId | Where-Object { -not $keys.Contains("$($_.nativeWindowHandle)|$($_.runtimeId)") })
        if ($new.Count -eq 1) { $new }
    }
    $root = @($roots)[0].element
    $edit = @($root.FindAll([Windows.Automation.TreeScope]::Descendants, [Windows.Automation.AndCondition]::new([Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::AutomationIdProperty, '1152'), [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty, (Get-E2QaControlType 'Edit')), [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, (Get-E2QaText '\ud3f4\ub354:')))))
    $confirm = @($root.FindAll([Windows.Automation.TreeScope]::Descendants, [Windows.Automation.AndCondition]::new([Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::AutomationIdProperty, '1'), [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty, (Get-E2QaControlType 'Button')), [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, (Get-E2QaText '\ud3f4\ub354 \uc120\ud0dd')))))
    if ($edit.Count -ne 1 -or $confirm.Count -ne 1) { throw "Observed folder controls are not unique: edit=$($edit.Count) confirm=$($confirm.Count)" }
    $edit = $edit[0]; $confirm = $confirm[0]
    if ($edit.Current.ProcessId -ne $ProcessId -or $confirm.Current.ProcessId -ne $ProcessId -or -not $edit.Current.IsEnabled -or -not $confirm.Current.IsEnabled) { throw 'Observed folder controls are not enabled exact PID-owned controls' }
    $value = [Windows.Automation.ValuePattern]$edit.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern); $invoke = [Windows.Automation.InvokePattern]$confirm.GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern)
    $value.SetValue($Path); $readback = $value.Current.Value; $edit.SetFocus(); $focused = [Windows.Automation.AutomationElement]::FocusedElement
    if ($readback -ne $Path -or $focused.Current.ProcessId -ne $ProcessId -or (@($focused.GetRuntimeId()) -join ',') -ne (@($edit.GetRuntimeId()) -join ',')) { throw 'Observed folder Edit1152 value or focus identity did not match' }
    $beforeAction = [ordered]@{ root = [ordered]@{ processId = $ProcessId; nativeWindowHandle = [string]$root.Current.NativeWindowHandle; runtimeId = (@($root.GetRuntimeId()) -join ',') }; requestPath = $Path; editRuntimeId = (@($edit.GetRuntimeId()) -join ','); confirmRuntimeId = (@($confirm.GetRuntimeId()) -join ','); readback = $readback }
    Write-E2QaJson (Join-Path $EvidenceDirectory 'before-action.json') $beforeAction 8; try { Save-E2QaWindowScreenshot $root (Join-Path $EvidenceDirectory 'before-action.png') } catch { Write-E2QaJson (Join-Path $EvidenceDirectory 'before-action-screenshot-error.json') @{ error = $_.Exception.Message } 4 }
    $invoke.Invoke()
    Wait-E2QaCondition -TimeoutSeconds 20 -Failure 'Observed folder dialog did not close after its exact confirmation action' -Condition { -not @(Get-E2QaFolderDialogTopLevels $desktop $ProcessId | Where-Object { $_.nativeWindowHandle -eq $beforeAction.root.nativeWindowHandle -and $_.runtimeId -eq $beforeAction.root.runtimeId }).Count } | Out-Null
    Write-E2QaJson (Join-Path $EvidenceDirectory 'after-action.json') @{ requestPath = $Path; dialogClosed = $true } 4
}

function Get-E2QaUiaTree {
    param([Parameter(Mandatory)][Windows.Automation.AutomationElement]$Root, [int]$Maximum = 600, [switch]$CaptureDetails)
    $all = $Root.FindAll([Windows.Automation.TreeScope]::Subtree, [Windows.Automation.Condition]::TrueCondition)
    $rows = [Collections.Generic.List[object]]::new()
    foreach ($element in @($all)) {
        if ($rows.Count -ge $Maximum) { break }
        try {
            $row = [ordered]@{
                name = $element.Current.Name
                automationId = $element.Current.AutomationId
                controlType = $element.Current.ControlType.ProgrammaticName
                enabled = $element.Current.IsEnabled
                focused = $element.Current.HasKeyboardFocus
                bounds = $element.Current.BoundingRectangle.ToString()
            }
            if ($CaptureDetails) {
                $patterns = @($element.GetSupportedPatterns() | ForEach-Object { $_.ProgrammaticName }); $row.processId = $element.Current.ProcessId; $row.runtimeId = (@($element.GetRuntimeId()) -join ','); $row.offscreen = $element.Current.IsOffscreen; $row.patterns = $patterns
                if (@($element.GetSupportedPatterns() | Where-Object { $_ -eq [Windows.Automation.ValuePattern]::Pattern }).Count) { try { $row.value = ([Windows.Automation.ValuePattern]$element.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern)).Current.Value } catch { $row.valueError = $_.Exception.Message } }
            }
            $rows.Add($row)
        } catch { $rows.Add([ordered]@{ unavailable = $true; error = $_.Exception.Message }) }
    }
    $rows
}

function Save-E2QaUiaTree {
    param([Parameter(Mandatory)][Windows.Automation.AutomationElement]$Root, [Parameter(Mandatory)][string]$Path)
    Write-E2QaJson $Path (Get-E2QaUiaTree $Root) 5
}

function Save-E2QaWindowScreenshot {
    param([Parameter(Mandatory)][Windows.Automation.AutomationElement]$Window, [Parameter(Mandatory)][string]$Path)
    $rect = New-Object E2Qa.NativeWindow+RECT
    if (-not [E2Qa.NativeWindow]::GetWindowRect([IntPtr]$Window.Current.NativeWindowHandle, [ref]$rect)) { throw 'GetWindowRect failed' }
    $width = $rect.Right - $rect.Left; $height = $rect.Bottom - $rect.Top
    if ($width -lt 1 -or $height -lt 1) { throw 'Window has empty screenshot bounds' }
    $bitmap = [Drawing.Bitmap]::new($width, $height)
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    try {
        $graphics.CopyFromScreen($rect.Left, $rect.Top, 0, 0, $bitmap.Size)
        $bitmap.Save($Path, [Drawing.Imaging.ImageFormat]::Png)
    } finally { $graphics.Dispose(); $bitmap.Dispose() }
    Assert-E2QaPng $Path
}

function Invoke-E2QaNamedButton {
    param([Parameter(Mandatory)][Windows.Automation.AutomationElement]$Root, [Parameter(Mandatory)][string]$Name, [int]$TimeoutSeconds = 30)
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $button = Wait-E2QaElement -Root $Root -Name $Name -ControlType 'Button' -TimeoutSeconds $TimeoutSeconds
    $remaining = [Math]::Max(0, [int](($deadline - [DateTime]::UtcNow).TotalMilliseconds))
    $latch = [E2Qa.NativeWindow+EnabledLatch]::new($button)
    try {
        if (-not $latch.Wait($remaining)) {
            $primary = "Button did not become enabled: $Name"
            $diagnostic = [ordered]@{ rootProcessId = $null; originalButton = $null; originalButtonError = $null; exactNameButtonCount = $null; exactNameButtonMaximum = 8; exactNameButtons = @(); exactNameButtonErrorCount = 0; exactNameButtonErrors = @(); exactNameButtonQueryError = $null; ownedTopLevelCount = $null; ownedTopLevelMaximum = 24; ownedTopLevelTruncated = $null; ownedTopLevel = @(); ownedTopLevelFilterErrorCount = 0; ownedTopLevelFilterErrors = @(); ownedTopLevelTupleErrorCount = 0; ownedTopLevelTupleErrors = @(); ownedTopLevelError = $null; appTreeMaximum = 64; appTree = @(); appTreeError = $null; lastPicker = $null; pickerAtFailure = 'unavailable'; pickerAtFailureError = $null }
            try { $diagnostic.rootProcessId = $Root.Current.ProcessId } catch { $diagnostic.ownedTopLevelError = "root-process: $($_.Exception.Message)" }
            try { $diagnostic.originalButton = [ordered]@{ runtimeId = (@($button.GetRuntimeId()) -join ','); name = $button.Current.Name; automationId = $button.Current.AutomationId; controlType = $button.Current.ControlType.ProgrammaticName; enabled = $button.Current.IsEnabled; patterns = @($button.GetSupportedPatterns() | ForEach-Object { $_.ProgrammaticName }) } } catch { $diagnostic.originalButtonError = $_.Exception.Message }
            try {
                $condition = [Windows.Automation.AndCondition]::new([Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, $Name), [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty, (Get-E2QaControlType 'Button')))
                $matches = @($Root.FindAll([Windows.Automation.TreeScope]::Descendants, $condition)); $diagnostic.exactNameButtonCount = $matches.Count
                $state = [ordered]@{ count = 0; errors = [Collections.Generic.List[string]]::new() }
                $diagnostic.exactNameButtons = @($matches | Select-Object -First 8 | ForEach-Object { $element = $_; try { [ordered]@{ runtimeId = (@($element.GetRuntimeId()) -join ','); name = $element.Current.Name; automationId = $element.Current.AutomationId; controlType = $element.Current.ControlType.ProgrammaticName; enabled = $element.Current.IsEnabled; offscreen = $element.Current.IsOffscreen; patterns = @($element.GetSupportedPatterns() | ForEach-Object { $_.ProgrammaticName }) } } catch { $state.count++; if ($state.errors.Count -lt 8) { $state.errors.Add($_.Exception.Message) }; [ordered]@{ unavailable = $true } } })
                $diagnostic.exactNameButtonErrorCount = $state.count; $diagnostic.exactNameButtonErrors = @($state.errors)
            } catch { $diagnostic.exactNameButtonQueryError = $_.Exception.Message }
            $topLevelTuples = @()
            try {
                $filter = [ordered]@{ count = 0; errors = [Collections.Generic.List[string]]::new() }
                $owned = @([Windows.Automation.AutomationElement]::RootElement.FindAll([Windows.Automation.TreeScope]::Children, [Windows.Automation.Condition]::TrueCondition) | ForEach-Object { $element = $_; try { if ($element.Current.ProcessId -eq $diagnostic.rootProcessId) { $element } } catch { $filter.count++; if ($filter.errors.Count -lt 8) { $filter.errors.Add($_.Exception.Message) } } })
                $tuple = [ordered]@{ count = 0; errors = [Collections.Generic.List[string]]::new() }
                $topLevelTuples = @($owned | Select-Object -First 24 | ForEach-Object { $element = $_; try { [ordered]@{ nativeWindowHandle = [string]$element.Current.NativeWindowHandle; runtimeId = (@($element.GetRuntimeId()) -join ','); name = $element.Current.Name; controlType = $element.Current.ControlType.ProgrammaticName; enabled = $element.Current.IsEnabled; offscreen = $element.Current.IsOffscreen } } catch { $tuple.count++; if ($tuple.errors.Count -lt 8) { $tuple.errors.Add($_.Exception.Message) }; [ordered]@{ unavailable = $true } } })
                $diagnostic.ownedTopLevelCount = $owned.Count; $diagnostic.ownedTopLevelTruncated = ($owned.Count -gt 24); $diagnostic.ownedTopLevel = $topLevelTuples; $diagnostic.ownedTopLevelFilterErrorCount = $filter.count; $diagnostic.ownedTopLevelFilterErrors = @($filter.errors); $diagnostic.ownedTopLevelTupleErrorCount = $tuple.count; $diagnostic.ownedTopLevelTupleErrors = @($tuple.errors)
            } catch { $diagnostic.ownedTopLevelError = $_.Exception.Message }
            try { $diagnostic.appTree = @(Get-E2QaUiaTree -Root $Root -Maximum 64) } catch { $diagnostic.appTreeError = $_.Exception.Message }
            if ($null -ne $script:E2QaLastPickerObservation -and $script:E2QaLastPickerObservation.processId -eq $diagnostic.rootProcessId) {
                $diagnostic.lastPicker = $script:E2QaLastPickerObservation
                try {
                    $before = $diagnostic.lastPicker.before
                    if ($null -ne $before -and $before.Contains('nativeWindowHandle') -and $before.Contains('runtimeId')) {
                        $found = @($topLevelTuples | Where-Object { $_.Contains('nativeWindowHandle') -and $_.Contains('runtimeId') -and $_.nativeWindowHandle -eq $before.nativeWindowHandle -and $_.runtimeId -eq $before.runtimeId }).Count -gt 0
                        $diagnostic.pickerAtFailure = if ($found) { 'present' } elseif ($null -ne $diagnostic.rootProcessId -and $null -ne $diagnostic.ownedTopLevelCount -and $null -eq $diagnostic.exactNameButtonQueryError -and $null -eq $diagnostic.ownedTopLevelError -and $diagnostic.ownedTopLevelFilterErrorCount -eq 0 -and $diagnostic.ownedTopLevelTupleErrorCount -eq 0 -and -not $diagnostic.ownedTopLevelTruncated) { 'absent' } else { 'unknown' }
                    }
                } catch { $diagnostic.pickerAtFailureError = $_.Exception.Message }
            }
            $serialized = 'diagnostic=unavailable'
            try { $serialized = "diagnostic=$($diagnostic | ConvertTo-Json -Depth 10 -Compress)" } catch { $serialized = "diagnostic=serialization-unavailable:$($_.Exception.Message)" }
            throw "$primary; $serialized"
        }
        if (-not $button.Current.IsEnabled) { throw "Button is disabled: $Name" }
        $button.SetFocus()
        Invoke-E2QaElement $button
    } finally { $latch.Dispose() }
}
