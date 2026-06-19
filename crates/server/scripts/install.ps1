# tbm CLI インストーラ — PowerShell(Windows)。
# 使い方: irm https://<ドメイン>/install.ps1 | iex
#
# %LOCALAPPDATA%\tbm\bin\tbm.exe に入れて(管理者権限不要)、ユーザ PATH に
# 追加する。.NET API 経由なので setx の 1024 文字切り詰めが無く、
# WM_SETTINGCHANGE も発火する。あわせて前提ツール(git / gh)を確認し、無ければ
# 管理者権限なしで導入する(git=MinGit、gh=GitHub 公式 release)。
# `tbm uninstall` が PATH エントリと %LOCALAPPDATA%\tbm 配下を丸ごと取り除く。
# __SERVER_URL__ は配信時にサーバが実ドメインへ置換する。
$ErrorActionPreference = "Stop"
# native コマンドの非零終了は「エラー」ではなく想定内(gh / claude の auth status や
# 子インストーラの戻り値で判定する)。PS 7.4+ は既定でこれを Stop のとき例外化するので
# 明示的に切る(Windows PowerShell 5.1 では未知変数の代入 = 無害)。
$PSNativeCommandUseErrorActionPreference = $false

$Server = if ($env:TSUBOMI_SERVER_URL) { $env:TSUBOMI_SERVER_URL } else { "__SERVER_URL__" }
$InstallRoot = Join-Path $env:LOCALAPPDATA "tbm"
$InstallDir = Join-Path $InstallRoot "bin"

$arch = $env:PROCESSOR_ARCHITECTURE
if ($env:PROCESSOR_ARCHITEW6432) { $arch = $env:PROCESSOR_ARCHITEW6432 }
if ($arch -ne "AMD64") {
    Write-Error "tbm は Windows $arch には未対応です(x86_64 のみ)。"
}
$Target = "x86_64-pc-windows-gnu"

# ディレクトリをユーザ PATH に冪等に足す(.NET API 経由 / 現セッションにも反映)。
# tbm の bin と MinGit の cmd の両方で使う。uninstall は %LOCALAPPDATA%\tbm 配下の
# エントリをまとめて取り除く(crates/cli/src/commands/uninstall.rs)。
function Add-UserPath($dir) {
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $entries = ($userPath -split ";") | Where-Object { $_ -ne "" }
    if ($entries -notcontains $dir) {
        [Environment]::SetEnvironmentVariable("Path", (($entries + $dir) -join ";"), "User")
        Write-Host "$dir をユーザ PATH に追加しました"
    }
    if (($env:Path -split ";") -notcontains $dir) {
        $env:Path = "$env:Path;$dir"
    }
}

# git(MinGit)を %LOCALAPPDATA%\tbm\git に入れ、その cmd を PATH に足す。
# MinGit は純粋な zip(自己展開 exe ではない)= 管理者権限不要。バージョンは
# GitHub API の tag_name から取る。tag は `v2.54.0.windows.1` の形 — DL URL は
# tag 全体を使い、資産名だけ `.windows` を剥がして `MinGit-2.54.0-64-bit.zip` に
# する。両者を一緒くたにすると 404 になるので剥離処理は消さないこと。
function Install-MinGit {
    try {
        $tag = (Invoke-RestMethod "https://api.github.com/repos/git-for-windows/git/releases/latest").tag_name
        $ver = ($tag -replace '^v', '' -replace '\.windows.*$', '')
        if (-not $ver) { return $false }
        $asset = "MinGit-$ver-64-bit.zip"
        $url = "https://github.com/git-for-windows/git/releases/download/$tag/$asset"
        $gitRoot = Join-Path $InstallRoot "git"
        $tmp = Join-Path $env:TEMP "tbm-git-$(Get-Random)"
        New-Item -ItemType Directory -Path $tmp | Out-Null
        try {
            $zip = Join-Path $tmp "mingit.zip"
            Invoke-WebRequest -Uri $url -OutFile $zip
            if (Test-Path $gitRoot) { Remove-Item -Recurse -Force $gitRoot }
            New-Item -ItemType Directory -Path $gitRoot -Force | Out-Null
            Expand-Archive -LiteralPath $zip -DestinationPath $gitRoot -Force
        } finally {
            Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
        }
        $gitCmd = Join-Path $gitRoot "cmd"
        if (Test-Path (Join-Path $gitCmd "git.exe")) {
            Add-UserPath $gitCmd
            return $true
        }
        return $false
    } catch {
        return $false
    }
}

# gh(GitHub CLI)を tbm と同じ bin に入れる(単一バイナリ = PATH も uninstall も
# tbm と同じ仕組みでカバーされる)。GitHub 公式 release の zip = 管理者権限不要。
function Install-Gh {
    try {
        $tag = (Invoke-RestMethod "https://api.github.com/repos/cli/cli/releases/latest").tag_name
        $ver = $tag -replace '^v', ''
        if (-not $ver) { return $false }
        $asset = "gh_${ver}_windows_amd64.zip"
        $url = "https://github.com/cli/cli/releases/download/$tag/$asset"
        $tmp = Join-Path $env:TEMP "tbm-gh-$(Get-Random)"
        New-Item -ItemType Directory -Path $tmp | Out-Null
        try {
            $zip = Join-Path $tmp "gh.zip"
            Invoke-WebRequest -Uri $url -OutFile $zip
            Expand-Archive -LiteralPath $zip -DestinationPath $tmp -Force
            $bin = Get-ChildItem -Path $tmp -Recurse -Filter "gh.exe" | Select-Object -First 1
            if (-not $bin) { return $false }
            Move-Item -Force $bin.FullName (Join-Path $InstallDir "gh.exe")
        } finally {
            Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
        }
        return $true
    } catch {
        return $false
    }
}

# claude(Claude Code)を公式インストーラで導入する。子プロセスの powershell で走らせて
# 隔離する(リモートスクリプトの exit / $ErrorActionPreference がこのスクリプトを巻き
# 込まないように)。インストール先は %USERPROFILE%\.local\bin\claude.exe。
function Install-ClaudeCode {
    try {
        & powershell -NoProfile -ExecutionPolicy Bypass -Command "irm https://claude.ai/install.ps1 | iex"
        if ($LASTEXITCODE -ne 0) { return $false }
        return (Test-Path (Join-Path $env:USERPROFILE ".local\bin\claude.exe"))
    } catch {
        return $false
    }
}

# Claude Code のユーザ設定(%USERPROFILE%\.claude\settings.json)に 2 つの既定を入れる:
#   permissions.defaultMode = "auto"(プロンプトをほぼ出さない auto モード。
#     ※ auto は Opus 4.6+ / Sonnet 4.6 かつ「ユーザ級」設定でのみ有効。条件を
#     満たさないと claude が静かに既定モードへ戻る — それは仕様)
#   tui = "fullscreen"(ちらつかない全画面描画)
# 既存の設定は壊さない(この 2 キーだけ上書き)。PS 5.1 でも動くよう PSCustomObject +
# Add-Member -Force でマージし、BOM 無し UTF-8 で書き戻す。
function Set-ClaudeSettings {
    $dir = Join-Path $env:USERPROFILE ".claude"
    $file = Join-Path $dir "settings.json"
    New-Item -ItemType Directory -Path $dir -Force | Out-Null
    $cfg = $null
    if (Test-Path $file) {
        try { $cfg = Get-Content -Raw -Path $file | ConvertFrom-Json } catch { $cfg = $null }
    }
    if (($null -eq $cfg) -or ($cfg -isnot [pscustomobject])) { $cfg = [PSCustomObject]@{} }
    if (($null -eq $cfg.permissions) -or ($cfg.permissions -isnot [pscustomobject])) {
        $cfg | Add-Member -NotePropertyName permissions -NotePropertyValue ([PSCustomObject]@{}) -Force
    }
    # 既存が bypassPermissions(より強い)なら降格しない。それ以外は auto に。
    if ($cfg.permissions.defaultMode -ne "bypassPermissions") {
        $cfg.permissions | Add-Member -NotePropertyName defaultMode -NotePropertyValue "auto" -Force
    }
    # tui は常に fullscreen。
    $cfg | Add-Member -NotePropertyName tui -NotePropertyValue "fullscreen" -Force
    [System.IO.File]::WriteAllText($file, ($cfg | ConvertTo-Json -Depth 20))
    Write-Host "Claude Code の設定を更新しました(auto モード + fullscreen)"
}

$info = Invoke-RestMethod "$Server/api/cli/version/$Target"
if (-not $info.version -or -not $info.url -or -not $info.sha256) {
    Write-Error "$Server から不完全な manifest を受け取りました"
}
# manifest の url は相対パス(ドメイン非依存)。
$url = if ($info.url.StartsWith("/")) { "$Server$($info.url)" } else { $info.url }

$tmp = Join-Path $env:TEMP "tbm-install-$(Get-Random)"
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    $archive = Join-Path $tmp "tbm.zip"
    Write-Host "tbm $($info.version) をダウンロードしています($Target)"
    Invoke-WebRequest -Uri $url -OutFile $archive

    # manifest の sha256 と照合。PATH に置く前に止める。
    $actual = (Get-FileHash -Algorithm SHA256 -Path $archive).Hash.ToLower()
    if ($actual -ne $info.sha256.ToLower()) {
        Write-Error "チェックサムが一致しません: 期待値 $($info.sha256) / 実際 $actual"
    }

    Expand-Archive -LiteralPath $archive -DestinationPath $tmp -Force
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    Move-Item -Force (Join-Path $tmp "tbm.exe") (Join-Path $InstallDir "tbm.exe")
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

Write-Host ""
Write-Host "tbm $($info.version) を $InstallDir\tbm.exe に入れました"

# ユーザ PATH(冪等)+ このセッションにも反映
Add-UserPath $InstallDir

# 初期設定:server_url を書いておく(インストーラは自分のドメインを知っている)。
# 既存の設定(トークン入りかもしれない)は壊さない。
# パスは Rust 側 ProjectDirs(directories crate)の Windows 解決結果
# %APPDATA%\<org>\<app>\config をミラーしている(crates/cli/src/config.rs)。
$cfgDir = Join-Path $env:APPDATA "flegrowth\tsubomi\config"
$cfgFile = Join-Path $cfgDir "config.toml"
if (-not (Test-Path $cfgFile)) {
    New-Item -ItemType Directory -Path $cfgDir -Force | Out-Null
    Set-Content -Path $cfgFile -Value "server_url = `"$Server`""
    Write-Host "接続先サーバを設定しました: $Server"
}

# 前提ツール(git / gh = GitHub CLI)。tbm の GitHub デプロイ経路で必須。
# 既にあれば触らない。無ければ管理者権限なしで導入する。
Write-Host ""
if (Get-Command git -ErrorAction SilentlyContinue) {
    # 既にある → 触らない
} else {
    Write-Host "git が見つかりません。インストールしています…"
    if (Install-MinGit) {
        Write-Host "git(MinGit)をインストールしました"
    } else {
        Write-Warning "git の自動インストールに失敗しました。手動で導入してください: https://gitforwindows.org/"
    }
}
if (Get-Command gh -ErrorAction SilentlyContinue) {
    # 既にある → 触らない。未ログインなら一手だけ案内。
    gh auth status 2>$null | Out-Null
    if ($LASTEXITCODE -ne 0) {
        Write-Host "gh は未ログインです。GitHub と連携するには: gh auth login --web --git-protocol https --clipboard"
    }
} else {
    Write-Host "gh(GitHub CLI)が見つかりません。インストールしています…"
    if (Install-Gh) {
        Write-Host "gh をインストールしました。GitHub と連携するには: gh auth login --web --git-protocol https --clipboard"
    } else {
        Write-Warning "gh の自動インストールに失敗しました。手動で導入してください: https://github.com/cli/cli/releases"
    }
}

# claude(Claude Code = この PaaS を AI で操作する CLI)。無ければ公式インストーラで導入
# (管理者権限不要・%USERPROFILE%\.local\bin に入る)。Windows では claude が PATH を
# 自動追加しないことがあるので、その bin をユーザ PATH に足す(現セッションにも反映)。
$claudeBin = Join-Path $env:USERPROFILE ".local\bin"
$claudeOk = $false
if (Get-Command claude -ErrorAction SilentlyContinue) {
    $claudeOk = $true  # 既にある → インストールはスキップ
} else {
    Write-Host "claude(Claude Code)が見つかりません。インストールしています…"
    if (Install-ClaudeCode) {
        $claudeOk = $true
        Add-UserPath $claudeBin
    } else {
        Write-Warning "claude の自動インストールに失敗しました。手動で: irm https://claude.ai/install.ps1 | iex"
    }
}
if ($claudeOk) {
    Set-ClaudeSettings
    # ログイン確認。今のセッションの PATH にまだ無いかもしれないので絶対パスも試す。
    $claudeExe = (Get-Command claude -ErrorAction SilentlyContinue).Source
    if (-not $claudeExe) { $claudeExe = Join-Path $claudeBin "claude.exe" }
    if (Test-Path $claudeExe) {
        & $claudeExe auth status 2>$null | Out-Null
        if ($LASTEXITCODE -ne 0) {
            Write-Host "Claude Code は未ログインです。ログインするには: claude auth login"
        }
    }
}

Write-Host ""
Write-Host "次のステップ: tbm login"
