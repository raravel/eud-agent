@echo off
setlocal
title EUD Agent - uninstall
rem Double-clickable uninstaller: runs uninstall_dropin.ps1 (removes the
rem bridge lua + Data\agent; WebView2 DLLs are left in place by default).
rem The window stays open until Enter is pressed, so failure messages
rem are never lost. ASCII only (CP949 console).

set "RC=0"
set "SCRIPT_DIR=%~dp0"

rem --- prefer PowerShell 7 (pwsh), fall back to builtin Windows PowerShell --
set "PSH=pwsh"
where pwsh >nul 2>nul
if errorlevel 1 set "PSH=powershell"
where %PSH% >nul 2>nul
if errorlevel 1 (
    echo ERROR: no PowerShell found on PATH ^(tried pwsh and powershell^).
    set "RC=1"
    goto :done
)

rem --- editor path: empty input = the uninstall_dropin.ps1 default ----------
set "EDITOR_PATH="
set /p EDITOR_PATH="EUD Editor 3 folder path (Enter = default): "
if defined EDITOR_PATH set "EDITOR_PATH=%EDITOR_PATH:"=%"

echo.
echo === uninstall_dropin: remove bridge lua + Data\agent ===
if defined EDITOR_PATH (
    %PSH% -NoProfile -ExecutionPolicy Bypass -File "%SCRIPT_DIR%uninstall_dropin.ps1" -EditorPath "%EDITOR_PATH%"
) else (
    %PSH% -NoProfile -ExecutionPolicy Bypass -File "%SCRIPT_DIR%uninstall_dropin.ps1"
)
if errorlevel 1 (
    echo.
    echo ERROR: uninstall_dropin failed.
    set "RC=1"
    goto :done
)

echo.
echo uninstall complete.

:done
echo.
set /p _DUMMY="Press Enter to close..."
exit /b %RC%
