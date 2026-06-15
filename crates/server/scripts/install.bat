@echo off
REM tbm CLI installer for cmd.exe (no PowerShell, no admin required).
REM Usage:
REM   curl -fsSL https://<domain>/install.bat -o %TEMP%\tbm-install.bat ^&^& %TEMP%\tbm-install.bat
REM
REM Installs to %LOCALAPPDATA%\tbm\bin\tbm.exe and adds that dir to the user PATH
REM via "reg add" (setx truncates at 1024 chars, so it is avoided). "tbm uninstall"
REM removes the PATH entry and the directory (no leftovers).
REM __SERVER_URL__ is replaced with the real domain by the server at serve time.
REM
REM IMPORTANT: keep this file ASCII-only. cmd.exe parses batch files using the
REM console's OEM codepage (cp932 / Shift-JIS on Japanese Windows). UTF-8 non-ASCII
REM bytes get mis-paired there, which swallows spaces and line breaks and makes cmd
REM execute fragments of comments as commands. The design rationale lives in
REM crates/server/src/cli_release.rs (Rust source is read as UTF-8, never by cmd).
setlocal enabledelayedexpansion

if not defined TSUBOMI_SERVER_URL set "TSUBOMI_SERVER_URL=__SERVER_URL__"
set "INSTALL_DIR=%LOCALAPPDATA%\tbm\bin"

set "ARCH=%PROCESSOR_ARCHITECTURE%"
if defined PROCESSOR_ARCHITEW6432 set "ARCH=%PROCESSOR_ARCHITEW6432%"
if /i not "%ARCH%"=="AMD64" (
    echo tbm does not support Windows %ARCH% ^(x86_64 only^)
    exit /b 1
)
set "TARGET=x86_64-pc-windows-gnu"

set "TMP_DIR=%TEMP%\tbm-install-%RANDOM%%RANDOM%"
mkdir "%TMP_DIR%" >nul 2>&1
if errorlevel 1 (
    echo failed to create temp dir %TMP_DIR%
    exit /b 1
)

set "MANIFEST=%TMP_DIR%\manifest.json"
curl -fsSL "%TSUBOMI_SERVER_URL%/api/cli/version/%TARGET%" -o "%MANIFEST%"
if errorlevel 1 (
    echo failed to fetch %TSUBOMI_SERVER_URL%/api/cli/version/%TARGET%
    rmdir /s /q "%TMP_DIR%" >nul 2>&1
    exit /b 1
)

REM manifest is compact JSON: {"version":"...","target":"...","url":"...","sha256":"..."}
REM Pure cmd parse: load into one var, strip up to each key, take up to the next quote.
set "JSON="
for /f "usebackq delims=" %%L in ("%MANIFEST%") do set "JSON=!JSON!%%L"

set "_rest=!JSON:*"url":"=!"
for /f tokens^=1^ delims^=^" %%V in ("!_rest!") do set "URL=%%V"

set "_rest=!JSON:*"sha256":"=!"
for /f tokens^=1^ delims^=^" %%V in ("!_rest!") do set "EXPECTED_SHA=%%V"

if not defined URL goto :bad_manifest
if not defined EXPECTED_SHA goto :bad_manifest

REM manifest url is a relative path (domain-independent). If it starts with /, prefix the server.
if "!URL:~0,1!"=="/" set "URL=%TSUBOMI_SERVER_URL%!URL!"

set "ARCHIVE=%TMP_DIR%\tbm.zip"
echo downloading !URL!
curl -fsSL "!URL!" -o "%ARCHIVE%"
if errorlevel 1 (
    echo download failed
    rmdir /s /q "%TMP_DIR%" >nul 2>&1
    exit /b 1
)

REM Integrity check with certutil. Same intent as install.sh: stop a tampered or
REM truncated archive before it lands on PATH.
set "ACTUAL_SHA="
for /f "skip=1 delims=" %%H in ('certutil -hashfile "%ARCHIVE%" SHA256') do (
    if not defined ACTUAL_SHA set "ACTUAL_SHA=%%H"
)
set "ACTUAL_SHA=%ACTUAL_SHA: =%"

if /i not "%ACTUAL_SHA%"=="%EXPECTED_SHA%" (
    echo checksum mismatch for !URL!
    echo   expected: %EXPECTED_SHA%
    echo   actual:   %ACTUAL_SHA%
    rmdir /s /q "%TMP_DIR%" >nul 2>&1
    exit /b 1
)

REM tar ships with Windows 10 1803+ and handles zip.
tar -xf "%ARCHIVE%" -C "%TMP_DIR%"
if errorlevel 1 (
    echo failed to extract %ARCHIVE%
    rmdir /s /q "%TMP_DIR%" >nul 2>&1
    exit /b 1
)

if not exist "%INSTALL_DIR%" mkdir "%INSTALL_DIR%"
move /y "%TMP_DIR%\tbm.exe" "%INSTALL_DIR%\tbm.exe" >nul
if errorlevel 1 (
    echo failed to install to %INSTALL_DIR%
    rmdir /s /q "%TMP_DIR%" >nul 2>&1
    exit /b 1
)

rmdir /s /q "%TMP_DIR%" >nul 2>&1

echo.
echo tbm installed to %INSTALL_DIR%\tbm.exe

REM PATH integration. Three requirements:
REM   1. setx truncates at 1024 chars (silently corrupts a long user PATH).
REM      Write the registry directly with "reg add".
REM   2. "reg add" does not fire WM_SETTINGCHANGE, so explorer keeps handing the
REM      stale env to new cmd windows. setx fires it as a side effect -- write and
REM      delete a throwaway var to broadcast the change without touching PATH.
REM   3. The calling cmd (the window that ran "&& install.bat") holds its pre-run
REM      env. The "endlocal ^& set" trick injects into the caller's PATH too so tbm
REM      works in the same window right away.
set "USER_PATH="
for /f "tokens=2,*" %%A in ('reg query "HKCU\Environment" /v Path 2^>nul ^| findstr /i "REG_"') do set "USER_PATH=%%B"

set "SHELL_HAS_DIR="
echo ;%PATH%; | findstr /i /c:";%INSTALL_DIR%;" >nul && set "SHELL_HAS_DIR=1"
set "REG_HAS_DIR="
if defined USER_PATH (
    echo ;!USER_PATH!; | findstr /i /c:";%INSTALL_DIR%;" >nul && set "REG_HAS_DIR=1"
)

if not defined REG_HAS_DIR (
    if defined USER_PATH (
        set "NEW_PATH=!USER_PATH!;%INSTALL_DIR%"
    ) else (
        set "NEW_PATH=%INSTALL_DIR%"
    )
    reg add "HKCU\Environment" /v Path /t REG_EXPAND_SZ /d "!NEW_PATH!" /f >nul
    if errorlevel 1 (
        echo.
        echo warning: failed to update user PATH. Add manually:
        echo   %INSTALL_DIR%
        endlocal
        exit /b 0
    )
    setx _TBM_REFRESH 1 >nul 2>&1
    reg delete "HKCU\Environment" /v _TBM_REFRESH /f >nul 2>&1
    echo added %INSTALL_DIR% to user PATH.
)

REM Initial config: write server_url (the installer knows its own domain).
REM Do not clobber an existing config (it may hold a token).
REM This path mirrors the Rust ProjectDirs Windows resolution
REM %APPDATA%\org\app\config (crates/cli/src/config.rs).
set "CFG_DIR=%APPDATA%\flegrowth\tsubomi\config"
if not exist "%CFG_DIR%\config.toml" (
    if not exist "%CFG_DIR%" mkdir "%CFG_DIR%"
    >"%CFG_DIR%\config.toml" echo server_url = "%TSUBOMI_SERVER_URL%"
    echo configured server: %TSUBOMI_SERVER_URL%
)

echo.
echo next: tbm login

REM Inject into the caller shell's PATH. The right-hand side is expanded before
REM endlocal; after endlocal returns to the parent scope, "set" writes to the parent.
REM Skip if already on PATH (no duplication on re-run).
if defined SHELL_HAS_DIR (
    endlocal
    exit /b 0
)
endlocal & set "PATH=%PATH%;%LOCALAPPDATA%\tbm\bin"
exit /b 0

:bad_manifest
echo incomplete manifest from %TSUBOMI_SERVER_URL%
rmdir /s /q "%TMP_DIR%" >nul 2>&1
exit /b 1
