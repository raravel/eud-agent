@echo off
setlocal
title EUD Agent - install
rem Double-clickable installer: runs setup_env.ps1 (venv) then
rem install_dropin.ps1 (bridge lua + DLLs + agent.cfg) via PowerShell 7.
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

rem --- editor path: empty input = the install_dropin.ps1 default ------------
set "EDITOR_PATH="
set /p EDITOR_PATH="EUD Editor 3 folder path (Enter = default): "
if defined EDITOR_PATH set "EDITOR_PATH=%EDITOR_PATH:"=%"

echo.
echo === [1/2] setup_env: create/sync the server venv ===
%PSH% -NoProfile -ExecutionPolicy Bypass -File "%SCRIPT_DIR%setup_env.ps1"
if errorlevel 1 (
    echo.
    echo ERROR: setup_env failed - install aborted.
    set "RC=1"
    goto :done
)

echo.
echo === [2/2] install_dropin: copy bridge + DLLs, write agent.cfg ===
if defined EDITOR_PATH (
    %PSH% -NoProfile -ExecutionPolicy Bypass -File "%SCRIPT_DIR%install_dropin.ps1" -EditorPath "%EDITOR_PATH%"
) else (
    %PSH% -NoProfile -ExecutionPolicy Bypass -File "%SCRIPT_DIR%install_dropin.ps1"
)
if errorlevel 1 (
    echo.
    echo ERROR: install_dropin failed.
    set "RC=1"
    goto :done
)

echo.
echo install complete.

:done
echo.
set /p _DUMMY="Press Enter to close..."
exit /b %RC%
