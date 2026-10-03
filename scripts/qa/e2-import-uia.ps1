Set-StrictMode -Version Latest

function Initialize-E2QaUia {
    Add-Type -AssemblyName UIAutomationClient
    Add-Type -AssemblyName UIAutomationTypes
    Add-Type -AssemblyName System.Windows.Forms
    Add-Type -AssemblyName System.Drawing
    if (-not ('E2Qa.NativeWindow' -as [type])) {
        Add-Type @'
using System;
using System.Runtime.InteropServices;
namespace E2Qa {
  public static class NativeWindow {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);
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
    Wait-E2QaCondition -TimeoutSeconds $TimeoutSeconds -Failure "App window for PID $ProcessId did not appear" -Condition {
        $condition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ProcessIdProperty, $ProcessId)
        $desktop.FindFirst([Windows.Automation.TreeScope]::Children, $condition)
    }
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
    $dialog = Wait-E2QaCondition -TimeoutSeconds 20 -Failure 'Native picker window did not appear' -Condition {
        $windows = $desktop.FindAll([Windows.Automation.TreeScope]::Children, [Windows.Automation.Condition]::TrueCondition)
        @($windows) | Where-Object { $_.Current.ProcessId -eq $ProcessId -and (Find-E2QaElement -Root $_ -AutomationId '1148' -ControlType 'Edit') } | Select-Object -First 1
    }
    $edit = Wait-E2QaElement -Root $dialog -AutomationId '1148' -ControlType 'Edit'
    $confirm = Wait-E2QaElement -Root $dialog -AutomationId '1' -ControlType 'Button'
    Set-E2QaElementValue $edit $Path
    Invoke-E2QaElement $confirm
}

function Get-E2QaUiaTree {
    param([Parameter(Mandatory)][Windows.Automation.AutomationElement]$Root, [int]$Maximum = 600)
    $all = $Root.FindAll([Windows.Automation.TreeScope]::Subtree, [Windows.Automation.Condition]::TrueCondition)
    $rows = [Collections.Generic.List[object]]::new()
    foreach ($element in @($all)) {
        if ($rows.Count -ge $Maximum) { break }
        try {
            $rows.Add([ordered]@{
                name = $element.Current.Name
                automationId = $element.Current.AutomationId
                controlType = $element.Current.ControlType.ProgrammaticName
                enabled = $element.Current.IsEnabled
                focused = $element.Current.HasKeyboardFocus
                bounds = $element.Current.BoundingRectangle.ToString()
            })
        } catch { }
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
    $button = Wait-E2QaElement -Root $Root -Name $Name -ControlType 'Button' -TimeoutSeconds $TimeoutSeconds
    if (-not $button.Current.IsEnabled) { throw "Button is disabled: $Name" }
    $button.SetFocus()
    Invoke-E2QaElement $button
}
