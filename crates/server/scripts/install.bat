@echo off
REM tbm CLI インストーラ — cmd.exe(PowerShell 不要、管理者権限不要)。
REM 使い方:
REM   curl -fsSL https://<ドメイン>/install.bat -o %TEMP%\tbm-install.bat ^&^& %TEMP%\tbm-install.bat
REM
REM %LOCALAPPDATA%\tbm\bin\tbm.exe に入れて、reg add でユーザ PATH に追加する
REM (setx は 1024 文字で切り詰めるので使わない)。`tbm uninstall` が PATH
REM エントリとディレクトリを丸ごと取り除く(残留物ゼロ)。
REM __SERVER_URL__ は配信時にサーバが実ドメインへ置換する。
setlocal enabledelayedexpansion

if not defined TSUBOMI_SERVER_URL set "TSUBOMI_SERVER_URL=__SERVER_URL__"
set "INSTALL_DIR=%LOCALAPPDATA%\tbm\bin"

set "ARCH=%PROCESSOR_ARCHITECTURE%"
if defined PROCESSOR_ARCHITEW6432 set "ARCH=%PROCESSOR_ARCHITEW6432%"
if /i not "%ARCH%"=="AMD64" (
    echo tbm は Windows %ARCH% には未対応です ^(x86_64 のみ^)
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

REM manifest はコンパクト JSON: {"version":"...","target":"...","url":"...","sha256":"..."}
REM 純 cmd パース:1 変数に読み込み、各キーまで文字列を剥がし、次の引用符まで取る。
set "JSON="
for /f "usebackq delims=" %%L in ("%MANIFEST%") do set "JSON=!JSON!%%L"

set "_rest=!JSON:*"url":"=!"
for /f tokens^=1^ delims^=^" %%V in ("!_rest!") do set "URL=%%V"

set "_rest=!JSON:*"sha256":"=!"
for /f tokens^=1^ delims^=^" %%V in ("!_rest!") do set "EXPECTED_SHA=%%V"

if not defined URL goto :bad_manifest
if not defined EXPECTED_SHA goto :bad_manifest

REM manifest の url は相対パス(ドメイン非依存)。先頭が / ならサーバを前置。
if "!URL:~0,1!"=="/" set "URL=%TSUBOMI_SERVER_URL%!URL!"

set "ARCHIVE=%TMP_DIR%\tbm.zip"
echo downloading !URL!
curl -fsSL "!URL!" -o "%ARCHIVE%"
if errorlevel 1 (
    echo download failed
    rmdir /s /q "%TMP_DIR%" >nul 2>&1
    exit /b 1
)

REM certutil で完全性チェック。install.sh と同じ意図:改竄・途中切れの
REM アーカイブを PATH に置く前に止める。
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

REM tar は Windows 10 1803+ に同梱で zip も扱える。
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

REM PATH 統合。満たすべき 3 点:
REM   1. setx は 1024 文字に切り詰める(長いユーザ PATH を黙って壊す)。
REM      reg add でレジストリに直接書く。
REM   2. reg add は WM_SETTINGCHANGE を発火しないので、explorer が古い env の
REM      まま新しい cmd に継がせる。setx は副作用としてそれを発火する —
REM      捨て変数を書いて消すことで PATH に触れず更新通知だけ出す。
REM   3. 呼び出し元の cmd(`&& install.bat` で実行した窓)は実行前の env を
REM      持っている。endlocal ^& set トリックで呼び出し元の PATH にも注入し、
REM      同じ窓ですぐ tbm が動くようにする。
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

REM 初期設定:server_url を書いておく(インストーラは自分のドメインを知っている)。
REM 既存の設定(トークン入りかもしれない)は壊さない。
REM パスは Rust 側 ProjectDirs の Windows 解決結果 %APPDATA%\org\app\config を
REM ミラーしている(crates/cli/src/config.rs)。
set "CFG_DIR=%APPDATA%\flegrowth\tsubomi\config"
if not exist "%CFG_DIR%\config.toml" (
    if not exist "%CFG_DIR%" mkdir "%CFG_DIR%"
    >"%CFG_DIR%\config.toml" echo server_url = "%TSUBOMI_SERVER_URL%"
    echo configured server: %TSUBOMI_SERVER_URL%
)

echo.
echo next: tbm login

REM 呼び出し元 shell の PATH へ注入。右辺は endlocal 前に展開され、
REM endlocal で親スコープに戻った後の set が親に書き込む。既に PATH に
REM あればスキップ(再実行で増殖しない)。
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
