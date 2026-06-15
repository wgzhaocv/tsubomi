//! 完全アンインストール — 残留物ゼロが要件:
//!   1. 設定ディレクトリ(トークン込み)
//!   2. インストーラが rc ファイルに書いた PATH ブロック(unix)/
//!      ユーザ PATH レジストリのエントリ(windows)
//!   3. バイナリ自体と、インストーラ専用ディレクトリ(~/.tbm /
//!      %LOCALAPPDATA%\tbm)
//!
//! 順序に意味がある:設定と PATH が先 — 途中で失敗しても `uninstall` を
//! もう一度走らせれば済む。self-delete は最後 — そこで権限エラーが出ても、
//! 再試行できる動くバイナリが手元に残る。

use anyhow::Result;

use crate::config;
// マーカーの定義元は tsubomi-shared(install.sh との同期契約コメント付き)。
// windows は rc ではなくレジストリ PATH なので unix のみ使用。
#[cfg(unix)]
use tsubomi_shared::{PATH_MARKER_BEGIN as MARKER_BEGIN, PATH_MARKER_END as MARKER_END};

pub async fn run(_server_override: Option<String>) -> Result<()> {
    // 1. 設定(トークン込み)。親方向に空ディレクトリを 3 段まで掃除する
    //    (windows は org\app\config の入れ子なので、config だけ消すと
    //    殻が残る)。remove_dir は空でないと失敗する = 安全。
    let cfg_path = config::config_path()?;
    config::delete()?;
    let mut dir = cfg_path.parent();
    for _ in 0..3 {
        let Some(d) = dir else { break };
        if std::fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }

    // 2. PATH の残留物
    #[cfg(unix)]
    strip_rc_markers();
    #[cfg(windows)]
    remove_from_user_path();

    // 2.5 AI エージェント向け skill(Claude / Codex の全 agent ターゲット)。
    //     Claude=ディレクトリごと、Codex=管理ブロックのみ除去(共有ファイルを壊さない)。
    crate::skill::remove();

    // 3. バイナリ + インストールディレクトリ
    self_destruct()?;

    println!("アンインストールしました");
    Ok(())
}

/// インストーラ専用ディレクトリ(その中に exe がいる場合)を特定する。
/// 手動で別の場所に置いた場合は None — バイナリだけ消す。
fn install_root() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    #[cfg(unix)]
    let root = directories::BaseDirs::new()?.home_dir().join(".tbm");
    #[cfg(windows)]
    let root = std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("tbm");
    exe.starts_with(&root).then_some(root)
}

#[cfg(unix)]
fn self_destruct() -> Result<()> {
    // unix は実行中バイナリの inode を即座に unlink できるので、
    // その後にディレクトリごと消せる。
    self_replace::self_delete()?;
    if let Some(root) = install_root() {
        let _ = std::fs::remove_dir_all(root);
    }
    Ok(())
}

#[cfg(windows)]
fn self_destruct() -> Result<()> {
    // windows は実行中の exe を含むディレクトリを消せない。
    // self_delete_outside_path が exe を一時領域へ退避してから削除を
    // 仕込むので、インストールディレクトリを丸ごと消せる。
    if let Some(root) = install_root() {
        self_replace::self_delete_outside_path(&root)?;
        let _ = std::fs::remove_dir_all(root);
    } else {
        self_replace::self_delete()?;
    }
    Ok(())
}

/// install.sh が書いた `# >>> tbm cli >>> … # <<< tbm cli <<<` ブロックを
/// 各シェルの rc ファイルから取り除く。ブロックが無いファイルは触らない。
#[cfg(unix)]
fn strip_rc_markers() {
    let Some(dirs) = directories::BaseDirs::new() else {
        return;
    };
    let home = dirs.home_dir();
    for rel in [
        ".zshrc",
        ".bashrc",
        ".bash_profile",
        ".profile",
        ".config/fish/config.fish",
    ] {
        let path = home.join(rel);
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        if !content.contains(MARKER_BEGIN) {
            continue;
        }
        let mut out = String::with_capacity(content.len());
        let mut skipping = false;
        for line in content.lines() {
            if line.contains(MARKER_BEGIN) {
                skipping = true;
                continue;
            }
            if line.contains(MARKER_END) {
                skipping = false;
                continue;
            }
            if !skipping {
                out.push_str(line);
                out.push('\n');
            }
        }
        if std::fs::write(&path, out).is_ok() {
            eprintln!("{} から PATH ブロックを削除しました", path.display());
        }
    }
}

/// インストーラが HKCU\Environment の Path に足したエントリを取り除く。
/// レジストリ直書き(setx は 1024 文字で切り詰める)— install.bat と対称。
/// インストーラは %LOCALAPPDATA%\tbm 配下に複数の dir を足し得る
/// (tbm/gh = `\tbm\bin`、MinGit = `\tbm\git\cmd`)。よって個別名で消すのではなく、
/// `\tbm\` 配下のエントリをまとめて取り除く。
#[cfg(windows)]
fn remove_from_user_path() {
    use std::process::Command;
    let Some(local) = std::env::var_os("LOCALAPPDATA") else {
        return;
    };
    // 末尾にセパレータを付けた接頭辞で前方一致させる(`\tbm\bin` も
    // `\tbm\git\cmd` も拾い、無関係な `\tbmfoo` は拾わない)。
    let mut prefix = std::path::PathBuf::from(local).join("tbm");
    prefix.push("");
    let prefix = prefix.to_string_lossy().to_lowercase();

    let out = Command::new("reg")
        .args(["query", "HKCU\\Environment", "/v", "Path"])
        .output();
    let Ok(out) = out else { return };
    let text = String::from_utf8_lossy(&out.stdout);
    // 出力形式: "    Path    REG_EXPAND_SZ    C:\...;C:\..."
    let Some(current) = text
        .lines()
        .find_map(|l| {
            l.split_once("REG_EXPAND_SZ")
                .or_else(|| l.split_once("REG_SZ"))
        })
        .map(|(_, v)| v.trim().to_string())
    else {
        return;
    };

    let new_path: Vec<&str> = current
        .split(';')
        .filter(|p| !p.trim().to_lowercase().starts_with(&prefix))
        .collect();
    let new_path = new_path.join(";");
    if new_path == current {
        return;
    }
    let r = Command::new("reg")
        .args([
            "add",
            "HKCU\\Environment",
            "/v",
            "Path",
            "/t",
            "REG_EXPAND_SZ",
            "/d",
            &new_path,
            "/f",
        ])
        .status();
    if matches!(r, Ok(s) if s.success()) {
        eprintln!("ユーザ PATH から tbm のエントリを削除しました");
    }
}
