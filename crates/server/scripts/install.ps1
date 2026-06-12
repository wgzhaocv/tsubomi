# tbm CLI インストーラ — PowerShell(Windows)。
# 使い方: irm https://<ドメイン>/install.ps1 | iex
#
# %LOCALAPPDATA%\tbm\bin\tbm.exe に入れて(管理者権限不要)、ユーザ PATH に
# 追加する。.NET API 経由なので setx の 1024 文字切り詰めが無く、
# WM_SETTINGCHANGE も発火する。`tbm uninstall` が PATH エントリと
# ディレクトリを丸ごと取り除く(残留物ゼロ)。
# __SERVER_URL__ は配信時にサーバが実ドメインへ置換する。
$ErrorActionPreference = "Stop"

$Server = if ($env:TSUBOMI_SERVER_URL) { $env:TSUBOMI_SERVER_URL } else { "__SERVER_URL__" }
$InstallRoot = Join-Path $env:LOCALAPPDATA "tbm"
$InstallDir = Join-Path $InstallRoot "bin"

$arch = $env:PROCESSOR_ARCHITECTURE
if ($env:PROCESSOR_ARCHITEW6432) { $arch = $env:PROCESSOR_ARCHITEW6432 }
if ($arch -ne "AMD64") {
    Write-Error "tbm は Windows $arch には未対応です(x86_64 のみ)。"
}
$Target = "x86_64-pc-windows-gnu"

$info = Invoke-RestMethod "$Server/api/cli/version/$Target"
if (-not $info.version -or -not $info.url -or -not $info.sha256) {
    Write-Error "incomplete manifest from $Server"
}
# manifest の url は相対パス(ドメイン非依存)。
$url = if ($info.url.StartsWith("/")) { "$Server$($info.url)" } else { $info.url }

$tmp = Join-Path $env:TEMP "tbm-install-$(Get-Random)"
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    $archive = Join-Path $tmp "tbm.zip"
    Write-Host "downloading tbm $($info.version) ($Target)"
    Invoke-WebRequest -Uri $url -OutFile $archive

    # manifest の sha256 と照合。PATH に置く前に止める。
    $actual = (Get-FileHash -Algorithm SHA256 -Path $archive).Hash.ToLower()
    if ($actual -ne $info.sha256.ToLower()) {
        Write-Error "checksum mismatch: expected $($info.sha256), got $actual"
    }

    Expand-Archive -LiteralPath $archive -DestinationPath $tmp -Force
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    Move-Item -Force (Join-Path $tmp "tbm.exe") (Join-Path $InstallDir "tbm.exe")
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

Write-Host ""
Write-Host "tbm $($info.version) installed to $InstallDir\tbm.exe"

# ユーザ PATH(冪等:既にあれば触らない)
$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
$entries = ($userPath -split ";") | Where-Object { $_ -ne "" }
if ($entries -notcontains $InstallDir) {
    [Environment]::SetEnvironmentVariable("Path", (($entries + $InstallDir) -join ";"), "User")
    Write-Host "added $InstallDir to user PATH"
}
# このセッションにも反映
if (($env:Path -split ";") -notcontains $InstallDir) {
    $env:Path = "$env:Path;$InstallDir"
}

# 初期設定:server_url を書いておく(インストーラは自分のドメインを知っている)。
# 既存の設定(トークン入りかもしれない)は壊さない。
# パスは Rust 側 ProjectDirs(directories crate)の Windows 解決結果
# %APPDATA%\<org>\<app>\config をミラーしている(crates/cli/src/config.rs)。
$cfgDir = Join-Path $env:APPDATA "flegrowth\tsubomi\config"
$cfgFile = Join-Path $cfgDir "config.toml"
if (-not (Test-Path $cfgFile)) {
    New-Item -ItemType Directory -Path $cfgDir -Force | Out-Null
    Set-Content -Path $cfgFile -Value "server_url = `"$Server`""
    Write-Host "configured server: $Server"
}

Write-Host ""
Write-Host "next: tbm login"
