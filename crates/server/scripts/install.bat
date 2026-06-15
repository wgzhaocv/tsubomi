@echo off
REM tbm CLI installer for cmd.exe (no PowerShell, no admin required).
REM Usage:
REM   curl -fsSL https://<domain>/install.bat -o %TEMP%\tbm-install.bat ^&^& %TEMP%\tbm-install.bat
REM
REM Installs to %LOCALAPPDATA%\tbm\bin\tbm.exe and adds that dir to the user PATH
REM via "reg add" (setx truncates at 1024 chars, so it is avoided). It also checks
REM the prerequisite tools (git / gh) and installs whatever is missing without admin
REM (git = MinGit zip, gh = official GitHub release). "tbm uninstall" removes the
REM PATH entries and everything under %LOCALAPPDATA%\tbm (no leftovers).
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

REM Prerequisite tools (git / gh = GitHub CLI). Required by tbm's GitHub deploy
REM path. Skip whatever already exists; install the rest with no admin. gh lands
REM in the same bin dir as tbm (single binary; PATH + uninstall cover it). git
REM goes to %LOCALAPPDATA%\tbm\git and its \cmd is added to PATH. GIT_CMD is set
REM when MinGit was just installed, so the caller-PATH injection can add it too.
echo.
where git >nul 2>&1
if errorlevel 1 (
    echo git not found. Installing MinGit ^(no admin^)...
    call :install_mingit
    if defined GIT_OK (
        echo git ^(MinGit^) installed.
    ) else (
        echo warning: failed to auto-install git. Install manually: https://gitforwindows.org/
    )
)
where gh >nul 2>&1
if errorlevel 1 (
    echo gh ^(GitHub CLI^) not found. Installing ^(no admin^)...
    call :install_gh
    if defined GH_OK (
        echo gh installed. To connect to GitHub, run: gh auth login
    ) else (
        echo warning: failed to auto-install gh. Install manually: https://github.com/cli/cli/releases
    )
)

echo.
echo next: tbm login

REM Inject into the caller shell's PATH so the tools work in the same window.
REM bin holds tbm + gh; git\cmd holds MinGit (only if just installed). The
REM right-hand side is expanded before endlocal; after endlocal it writes the
REM parent scope. Skip dirs already present (no duplication on re-run).
set "EXTRA="
if not defined SHELL_HAS_DIR set "EXTRA=!EXTRA!;%INSTALL_DIR%"
if defined GIT_CMD (
    echo ;%PATH%; | findstr /i /c:";!GIT_CMD!;" >nul || set "EXTRA=!EXTRA!;!GIT_CMD!"
)
if not defined EXTRA (
    endlocal
    exit /b 0
)
endlocal & set "PATH=%PATH%%EXTRA%"
exit /b 0

:bad_manifest
echo incomplete manifest from %TSUBOMI_SERVER_URL%
rmdir /s /q "%TMP_DIR%" >nul 2>&1
exit /b 1

REM ---- subroutines (reached only via "call"; the main flow exits above) ----

:install_mingit
REM git (MinGit) -> %LOCALAPPDATA%\tbm\git, add its \cmd to PATH. MinGit is a plain
REM zip (not a self-extracting exe) so no admin is needed. Version comes from the
REM releases/latest redirect (no GitHub API rate limit). The tag looks like
REM "v2.54.0.windows.1": the download URL uses the FULL tag, but the asset name
REM drops ".windows" -> "MinGit-2.54.0-64-bit.zip". Do not collapse the two (404).
set "GIT_OK="
for /f "delims=" %%U in ('curl -fsSLI -o nul -w "%%{url_effective}" "https://github.com/git-for-windows/git/releases/latest" 2^>nul') do set "GIT_TAGURL=%%U"
set "GIT_TAG=!GIT_TAGURL:*/tag/=!"
if "!GIT_TAG!"=="!GIT_TAGURL!" exit /b 0
set "GIT_TAGNOV=!GIT_TAG:~1!"
for /f "tokens=1,2,3 delims=." %%a in ("!GIT_TAGNOV!") do set "GIT_VER=%%a.%%b.%%c"
if not defined GIT_VER exit /b 0
set "GIT_ROOT=%LOCALAPPDATA%\tbm\git"
set "GIT_TMP=%TEMP%\tbm-git-%RANDOM%%RANDOM%"
mkdir "!GIT_TMP!" >nul 2>&1
curl -fsSL "https://github.com/git-for-windows/git/releases/download/!GIT_TAG!/MinGit-!GIT_VER!-64-bit.zip" -o "!GIT_TMP!\mingit.zip"
if errorlevel 1 (
    rmdir /s /q "!GIT_TMP!" >nul 2>&1
    exit /b 0
)
if exist "!GIT_ROOT!" rmdir /s /q "!GIT_ROOT!" >nul 2>&1
mkdir "!GIT_ROOT!" >nul 2>&1
tar -xf "!GIT_TMP!\mingit.zip" -C "!GIT_ROOT!"
set "_TAR_ERR=!errorlevel!"
rmdir /s /q "!GIT_TMP!" >nul 2>&1
if not "!_TAR_ERR!"=="0" exit /b 0
if not exist "!GIT_ROOT!\cmd\git.exe" exit /b 0
call :add_user_path "!GIT_ROOT!\cmd"
set "GIT_CMD=!GIT_ROOT!\cmd"
set "GIT_OK=1"
exit /b 0

:install_gh
REM gh -> same bin dir as tbm. Official GitHub release zip = no admin. Version from
REM the releases/latest redirect (no API rate limit).
set "GH_OK="
for /f "delims=" %%U in ('curl -fsSLI -o nul -w "%%{url_effective}" "https://github.com/cli/cli/releases/latest" 2^>nul') do set "GH_TAGURL=%%U"
set "GH_TAG=!GH_TAGURL:*/tag/=!"
if "!GH_TAG!"=="!GH_TAGURL!" exit /b 0
set "GH_VER=!GH_TAG:~1!"
if not defined GH_VER exit /b 0
set "GH_TMP=%TEMP%\tbm-gh-%RANDOM%%RANDOM%"
mkdir "!GH_TMP!" >nul 2>&1
curl -fsSL "https://github.com/cli/cli/releases/download/!GH_TAG!/gh_!GH_VER!_windows_amd64.zip" -o "!GH_TMP!\gh.zip"
if errorlevel 1 (
    rmdir /s /q "!GH_TMP!" >nul 2>&1
    exit /b 0
)
tar -xf "!GH_TMP!\gh.zip" -C "!GH_TMP!"
if errorlevel 1 (
    rmdir /s /q "!GH_TMP!" >nul 2>&1
    exit /b 0
)
set "GH_FOUND="
for /r "!GH_TMP!" %%F in (gh.exe) do if not defined GH_FOUND set "GH_FOUND=%%F"
if not defined GH_FOUND (
    rmdir /s /q "!GH_TMP!" >nul 2>&1
    exit /b 0
)
if not exist "%INSTALL_DIR%" mkdir "%INSTALL_DIR%"
move /y "!GH_FOUND!" "%INSTALL_DIR%\gh.exe" >nul
set "_MV_ERR=!errorlevel!"
rmdir /s /q "!GH_TMP!" >nul 2>&1
if not "!_MV_ERR!"=="0" exit /b 0
set "GH_OK=1"
exit /b 0

:add_user_path
REM %~1 = directory to add to HKCU\Environment Path (idempotent). Mirrors the bin
REM PATH logic above: write the registry directly (setx truncates at 1024) and fire
REM WM_SETTINGCHANGE via a throwaway var so new windows pick up the change.
REM Known limitation (same as the bin block): an existing PATH entry containing a
REM literal "!" is mangled under delayed expansion. Such dirs are vanishingly rare,
REM so this matches the existing behavior rather than diverging only here.
set "_AUP_DIR=%~1"
set "_AUP_CUR="
for /f "tokens=2,*" %%A in ('reg query "HKCU\Environment" /v Path 2^>nul ^| findstr /i "REG_"') do set "_AUP_CUR=%%B"
set "_AUP_HAS="
if defined _AUP_CUR (
    echo ;!_AUP_CUR!; | findstr /i /c:";!_AUP_DIR!;" >nul && set "_AUP_HAS=1"
)
if not defined _AUP_HAS (
    if defined _AUP_CUR (
        set "_AUP_NEW=!_AUP_CUR!;!_AUP_DIR!"
    ) else (
        set "_AUP_NEW=!_AUP_DIR!"
    )
    reg add "HKCU\Environment" /v Path /t REG_EXPAND_SZ /d "!_AUP_NEW!" /f >nul
    setx _TBM_REFRESH 1 >nul 2>&1
    reg delete "HKCU\Environment" /v _TBM_REFRESH /f >nul 2>&1
    echo added !_AUP_DIR! to user PATH.
)
exit /b 0
