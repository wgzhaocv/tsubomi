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

REM PATH integration. Two requirements:
REM   1. setx truncates at 1024 chars (silently corrupts a long user PATH).
REM      Write the registry directly with "reg add".
REM   2. "reg add" does not fire WM_SETTINGCHANGE, so explorer keeps handing the
REM      stale env to new cmd windows. setx fires it as a side effect -- write and
REM      delete a throwaway var to broadcast the change without touching PATH.
REM This updates the registry only; new terminals pick it up. We do NOT rewrite the
REM current window's PATH (see the closing note for why that is unsafe).
set "USER_PATH="
for /f "tokens=2,*" %%A in ('reg query "HKCU\Environment" /v Path 2^>nul ^| findstr /i "REG_"') do set "USER_PATH=%%B"

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

REM Prerequisite tools (git / gh = GitHub CLI / claude = Claude Code). Skip whatever
REM already exists; install the rest with no admin. gh lands in the same bin dir as
REM tbm (single binary; PATH + uninstall cover it); git goes to %LOCALAPPDATA%\tbm\git
REM and its \cmd is added to the user PATH; claude installs to %USERPROFILE%\.local\bin
REM (we add that to PATH too). gh / claude also get a login hint when not signed in.
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
        echo gh installed. To connect to GitHub now, run: "%INSTALL_DIR%\gh.exe" auth login --web --git-protocol https --clipboard
    ) else (
        echo warning: failed to auto-install gh. Install manually: https://github.com/cli/cli/releases
    )
) else (
    gh auth status >nul 2>&1
    if errorlevel 1 echo gh not logged in. To connect to GitHub, run: gh auth login --web --git-protocol https --clipboard
)

REM claude (Claude Code = the AI CLI used to drive this PaaS). Official native
REM installer, no admin; lands in %USERPROFILE%\.local\bin\claude.exe. Either branch
REM sets two defaults in its settings.json (see :configure_claude).
where claude >nul 2>&1
if errorlevel 1 (
    echo claude ^(Claude Code^) not found. Installing ^(no admin^)...
    call :install_claude
    if defined CLAUDE_OK (
        call :configure_claude
        echo claude installed. To log in now, run: "%USERPROFILE%\.local\bin\claude.exe" auth login
    ) else (
        echo warning: failed to auto-install claude. Install manually: https://claude.ai/install.cmd
    )
) else (
    call :configure_claude
    claude auth status >nul 2>&1
    if errorlevel 1 echo claude not logged in. To log in, run: claude auth login
)

REM PATH for NEW terminals is already written to the registry above (reg add +
REM a WM_SETTINGCHANGE broadcast). We intentionally do NOT rewrite the current
REM window's PATH here: re-expanding a machine PATH that contains a quoted or
REM special-character entry can break the quoted "set" and make cmd execute a
REM path fragment (this showed up as a stray error on some machines). A new
REM terminal picks up the registry PATH cleanly.
echo.
echo Done. Open a NEW terminal window so PATH changes take effect, then run: tbm login
endlocal
exit /b 0

:bad_manifest
echo incomplete manifest from %TSUBOMI_SERVER_URL%
rmdir /s /q "%TMP_DIR%" >nul 2>&1
exit /b 1

REM ---- subroutines (reached only via "call"; the main flow exits above) ----

:install_mingit
REM git (MinGit) -> %LOCALAPPDATA%\tbm\git, add its \cmd to PATH. MinGit is a plain
REM zip (not a self-extracting exe) so no admin is needed. The latest version tag
REM comes from the GitHub API. Call PowerShell DIRECTLY (NOT inside a for /f backtick
REM -- the parens/quotes in the PowerShell command make cmd mis-parse the for set, so
REM that previously failed with "powershell not found"). PowerShell writes the tag to
REM a temp file via WriteAllText = UTF-8 no-BOM (a ">" redirect on Win PowerShell 5.1
REM is UTF-16+BOM and corrupts set /p); set /p then reads it. The tag looks like
REM "v2.54.0.windows.1": the download URL uses the FULL tag, but the asset name drops
REM ".windows" -> "MinGit-2.54.0-64-bit.zip". Do not collapse the two (404).
set "GIT_OK="
set "GIT_ROOT=%LOCALAPPDATA%\tbm\git"
set "GIT_TMP=%TEMP%\tbm-git-%RANDOM%%RANDOM%"
mkdir "!GIT_TMP!" >nul 2>&1
powershell -NoProfile -ExecutionPolicy Bypass -Command "[System.IO.File]::WriteAllText('!GIT_TMP!\tag.txt', (Invoke-RestMethod 'https://api.github.com/repos/git-for-windows/git/releases/latest').tag_name)" >nul 2>&1
set "GIT_TAG="
if exist "!GIT_TMP!\tag.txt" set /p GIT_TAG=<"!GIT_TMP!\tag.txt"
if not defined GIT_TAG (
    rmdir /s /q "!GIT_TMP!" >nul 2>&1
    exit /b 0
)
set "GIT_TAGNOV=!GIT_TAG:~1!"
for /f "tokens=1,2,3 delims=." %%a in ("!GIT_TAGNOV!") do set "GIT_VER=%%a.%%b.%%c"
if not defined GIT_VER (
    rmdir /s /q "!GIT_TMP!" >nul 2>&1
    exit /b 0
)
curl -fsSL "https://github.com/git-for-windows/git/releases/download/!GIT_TAG!/MinGit-!GIT_VER!-64-bit.zip" -o "!GIT_TMP!\mingit.zip" >nul 2>&1
if errorlevel 1 (
    rmdir /s /q "!GIT_TMP!" >nul 2>&1
    exit /b 0
)
if exist "!GIT_ROOT!" rmdir /s /q "!GIT_ROOT!" >nul 2>&1
mkdir "!GIT_ROOT!" >nul 2>&1
tar -xf "!GIT_TMP!\mingit.zip" -C "!GIT_ROOT!" >nul 2>&1
set "_TAR_ERR=!errorlevel!"
rmdir /s /q "!GIT_TMP!" >nul 2>&1
if not "!_TAR_ERR!"=="0" exit /b 0
if not exist "!GIT_ROOT!\cmd\git.exe" exit /b 0
call :add_user_path "!GIT_ROOT!\cmd"
set "GIT_OK=1"
exit /b 0

:install_gh
REM gh -> same bin dir as tbm. Official GitHub release zip = no admin. The latest
REM version tag comes from the GitHub API. Call PowerShell DIRECTLY (NOT inside a
REM for /f backtick -- the parens/quotes in the PowerShell command make cmd mis-parse
REM the for set, so that previously failed with "powershell not found"). PowerShell
REM writes the tag to a temp file via WriteAllText = UTF-8 no-BOM (a ">" redirect on
REM Win PowerShell 5.1 is UTF-16+BOM and corrupts set /p); set /p then reads it.
REM curl + tar then fetch and unpack the asset -- the same tools that installed tbm.
set "GH_OK="
set "GH_TMP=%TEMP%\tbm-gh-%RANDOM%%RANDOM%"
mkdir "!GH_TMP!" >nul 2>&1
powershell -NoProfile -ExecutionPolicy Bypass -Command "[System.IO.File]::WriteAllText('!GH_TMP!\tag.txt', (Invoke-RestMethod 'https://api.github.com/repos/cli/cli/releases/latest').tag_name)" >nul 2>&1
set "GH_TAG="
if exist "!GH_TMP!\tag.txt" set /p GH_TAG=<"!GH_TMP!\tag.txt"
if not defined GH_TAG (
    rmdir /s /q "!GH_TMP!" >nul 2>&1
    exit /b 0
)
set "GH_VER=!GH_TAG:~1!"
if not defined GH_VER (
    rmdir /s /q "!GH_TMP!" >nul 2>&1
    exit /b 0
)
curl -fsSL "https://github.com/cli/cli/releases/download/!GH_TAG!/gh_!GH_VER!_windows_amd64.zip" -o "!GH_TMP!\gh.zip" >nul 2>&1
if errorlevel 1 (
    rmdir /s /q "!GH_TMP!" >nul 2>&1
    exit /b 0
)
tar -xf "!GH_TMP!\gh.zip" -C "!GH_TMP!" >nul 2>&1
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
move /y "!GH_FOUND!" "%INSTALL_DIR%\gh.exe" >nul 2>&1
set "_MV_ERR=!errorlevel!"
rmdir /s /q "!GH_TMP!" >nul 2>&1
if not "!_MV_ERR!"=="0" exit /b 0
set "GH_OK=1"
exit /b 0

:install_claude
REM claude (Claude Code) via the official native installer (no admin). Run it in a
REM child cmd (cmd /c) so a bare "exit" in the upstream script cannot kill this one.
REM It writes %USERPROFILE%\.local\bin\claude.exe; we add that dir to the user PATH
REM because the installer does not always persist it on Windows.
set "CLAUDE_OK="
set "CL_TMP=%TEMP%\tbm-claude-%RANDOM%%RANDOM%"
mkdir "!CL_TMP!" >nul 2>&1
curl -fsSL "https://claude.ai/install.cmd" -o "!CL_TMP!\claude-install.cmd" >nul 2>&1
if errorlevel 1 (
    rmdir /s /q "!CL_TMP!" >nul 2>&1
    exit /b 0
)
cmd /c call "!CL_TMP!\claude-install.cmd"
rmdir /s /q "!CL_TMP!" >nul 2>&1
if exist "%USERPROFILE%\.local\bin\claude.exe" (
    call :add_user_path "%USERPROFILE%\.local\bin"
    set "CLAUDE_OK=1"
)
exit /b 0

:configure_claude
REM Merge two defaults into %USERPROFILE%\.claude\settings.json without clobbering
REM other keys, via PowerShell (always present on Win10+). If defaultMode is already
REM "bypassPermissions" (a stronger mode) keep it; otherwise set "auto". tui is always
REM "fullscreen". The PowerShell uses single-quoted string literals so no embedded
REM double-quotes are needed; the pipes are literal inside the cmd-quoted argument.
powershell -NoProfile -ExecutionPolicy Bypass -Command "$ErrorActionPreference='Stop'; $d=Join-Path $env:USERPROFILE '.claude'; $f=Join-Path $d 'settings.json'; New-Item -ItemType Directory -Path $d -Force | Out-Null; $c=$null; if (Test-Path $f) { try { $c = Get-Content -Raw -Path $f | ConvertFrom-Json } catch { $c=$null } }; if (($null -eq $c) -or ($c -isnot [pscustomobject])) { $c=[PSCustomObject]@{} }; if (($null -eq $c.permissions) -or ($c.permissions -isnot [pscustomobject])) { $c | Add-Member -NotePropertyName permissions -NotePropertyValue ([PSCustomObject]@{}) -Force }; if ($c.permissions.defaultMode -ne 'bypassPermissions') { $c.permissions | Add-Member -NotePropertyName defaultMode -NotePropertyValue 'auto' -Force }; $c | Add-Member -NotePropertyName tui -NotePropertyValue 'fullscreen' -Force; [System.IO.File]::WriteAllText($f, ($c | ConvertTo-Json -Depth 20))" >nul 2>&1
if errorlevel 1 (
    echo note: could not auto-update Claude settings. Add tui=fullscreen and permissions.defaultMode=auto to %USERPROFILE%\.claude\settings.json
) else (
    echo Claude Code settings updated ^(auto mode + fullscreen^).
)
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
