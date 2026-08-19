pub mod cache;
pub mod db;
pub mod deploy;
pub mod env;
pub mod health;
pub mod inject;
pub mod login;
pub mod logout;
pub mod service;
pub mod skill;
pub mod trash;
pub mod uninstall;
pub mod update;
pub mod volume;
pub mod whoami;

use anyhow::{Context, Result};

use crate::api;
use crate::config::Config;

/// 出力形式。text=人間向けの整形、json=機械(AI/スクリプト)向けの構造化出力。
/// auto(既定)= stdout が端末なら text、そうでなければ(パイプ/捕捉)json。
/// tsubomi は主に AI が CLI を駆動するので、捕捉時に既定で構造化されるのが要点
/// (AI 側が `-o` を覚えなくてよい)。全コマンド共通のグローバル `-o/--output`。
#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputFormat {
    Auto,
    Text,
    Json,
}

impl OutputFormat {
    /// auto を実フォーマットへ解決する。stdout が端末(対話的に人が見る)なら text、
    /// パイプ/リダイレクト(AI・スクリプトが拾う)なら json。
    pub fn resolve(self) -> OutputFormat {
        match self {
            OutputFormat::Auto => {
                use std::io::IsTerminal;
                if std::io::stdout().is_terminal() {
                    OutputFormat::Text
                } else {
                    OutputFormat::Json
                }
            }
            resolved => resolved,
        }
    }

    pub fn is_json(self) -> bool {
        matches!(self.resolve(), OutputFormat::Json)
    }
}

/// JSON モードで Serialize 値を 1 つ stdout へ(pretty)。各コマンドが分岐で使う。
pub fn print_json<T: serde::Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

/// dev のデフォルトは vite のオリジン(/api を :9090 にプロキシする)。
/// ログインフローが SPA ルート(/oauth/authorize)を必要とするため。
/// 本番ではサーバが両方を一つのオリジンで配信するので問題にならない。
pub const DEFAULT_SERVER: &str = "http://localhost:5173";

/// 優先順位:--server / TSUBOMI_SERVER > 保存済み設定 > デフォルト。
pub fn resolve_server_from(over: Option<&str>, cfg: Option<&Config>) -> String {
    over.map(str::to_owned)
        .or_else(|| {
            cfg.filter(|c| !c.server_url.is_empty())
                .map(|c| c.server_url.clone())
        })
        .unwrap_or_else(|| DEFAULT_SERVER.to_owned())
        .trim_end_matches('/')
        .to_owned()
}

/// 優先順位:--token / TSUBOMI_TOKEN > 保存済み設定。
pub fn resolve_token_from(over: Option<String>, cfg: Option<Config>) -> Result<String> {
    over.or_else(|| cfg.and_then(|c| c.token))
        .context("ログインしていません(`tbm login` を実行してください)")
}

/// 現在の unix 秒(deploy hook の ts / logs --follow の再接続カーソルが共有)。
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `--for-sha` の値が commit sha の形か(4 桁以上の hex。branch/tag は受けない —
/// 前方一致し得ず timeout まで空回りするため早期に弾く)。verify / deploy --watch が共有。
pub fn looks_like_sha(s: &str) -> bool {
    s.len() >= 4 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// 手元 repo の HEAD の full sha(40 桁)。`verify --for-sha HEAD` と `deploy --watch` が共有。
pub fn git_head_sha() -> Result<String> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .context("git の実行に失敗しました(git はインストール済みですか?)")?;
    if !out.status.success() {
        anyhow::bail!(
            "HEAD を解決できません(git リポジトリの中で実行してください。または sha を直接指定)"
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// サービスの表示名 → id を一覧から解決する(service / inject / env が共有。専用エンドポイント
/// を増やさない)。見つからなければ機械可読な not_found コードを付けて返す。
pub async fn resolve_service_id(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    name: &str,
) -> Result<String> {
    resolve_service_row(c, server_url, token, name)
        .await
        .map(|s| s.id.to_string())
}

/// 表示名 → service の行そのもの。id だけでなく **形**(memory_mb / cpu_limit_millis 等)も
/// 要る呼び出し(`service metrics` の未反映判定)向け。一覧を引くコストは id 解決と同じ
/// (resolve_service_id も内部でこれを呼ぶ)なので、追加のリクエストは発生しない。
/// 名前は `_row` 付き:`deploy.rs` に別戻り値の私有 `resolve_service` が既にある。
pub async fn resolve_service_row(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    name: &str,
) -> Result<tsubomi_shared::ServiceDto> {
    // 表示名の完全一致 **のみ**。UUID 直通は敢えて入れない — 表示名は UUID 形式を禁止して
    // いないため、「B の表示名 = A の id」のとき A へ誤配送し得る(codex 審査 2026-08-13)。
    // 長時間処理(deploy --watch)の rename 耐性は、解決済み id を関数引数で持ち回ることで
    // 実現する(run_verify / wait_deploy_only が id を直接受ける)。
    let svcs = api::service_list(c, server_url, token).await?;
    svcs.into_iter()
        .find(|s| s.display_name == name)
        .ok_or_else(|| {
            api::ApiError {
                code: "not_found",
                message: format!("サービス '{name}' が見つかりません(`tbm service list` で確認)"),
            }
            .into()
        })
}

// ===== MSYS(Git Bash)パス正規化 =====

/// MSYS 環境の情報。実行時は `msys_env()` で採取、テストでは直接組む
/// (login.rs の choose_manual と同じ「検出と判定の分離」)。
pub struct MsysEnv {
    /// MSYSTEM(MINGW64 等)が立っている = Git Bash / MSYS2 配下。
    pub in_msys: bool,
    /// Git Bash が輸出する EXEPATH(Git インストールルート。例 `C:\Program Files\Git`)。
    pub exepath: Option<String>,
}

/// 実行環境から MsysEnv を採取(Windows 以外は常に in_msys=false = 正規化は素通し)。
pub fn msys_env() -> MsysEnv {
    if !cfg!(windows) {
        return MsysEnv {
            in_msys: false,
            exepath: None,
        };
    }
    MsysEnv {
        in_msys: std::env::var_os("MSYSTEM").is_some(),
        exepath: std::env::var("EXEPATH").ok(),
    }
}

/// **遠端パス引数**(volume 假根内パス / inject --mount)の MSYS 化けを復元する。
/// Git Bash は POSIX 風の絶対パス引数(`/data/x`)をネイティブ exe に渡す瞬間に
/// `<Git ルート>/data/x`(例 `C:/Program Files/Git/data/x`)へ書き換える。遠端パスは
/// 協定上ドライブレターを持ち得ないため、この化けは**無歧義に**検出・復元できる
/// (EXEPATH 接頭辞の完全一致 = 確定的。ヒューリスティックではない)。ローカルパス引数は変換の
/// 恩恵を受ける(POSIX 風 `/c/Users/…` → 開ける Windows パス)ので触らないこと。
/// - MSYS 外:そのまま(Windows ネイティブ shell のドライブレターも素通し — サーバ側で弾かれる)。
/// - `//x…` → `/x…`(手動の双スラッシュエスケープ。MSYS は `//` 開頭を変換しない)。
/// - ドライブレター + EXEPATH 前方一致 → 接頭辞を剥がして `/rest` に復元。
/// - ドライブレター + 不一致(純 MSYS2 等で EXEPATH 無し含む)→ 次の一手つきエラー。
pub fn normalize_remote_path(arg: &str, env: &MsysEnv) -> Result<String> {
    if !env.in_msys {
        return Ok(arg.to_string());
    }
    if let Some(rest) = arg.strip_prefix("//") {
        return Ok(format!("/{rest}"));
    }
    if !has_drive_prefix(arg) {
        return Ok(arg.to_string());
    }
    if let Some(exepath) = &env.exepath {
        let arg_s = slashify(arg);
        let exe_s = slashify(exepath.trim_end_matches(['/', '\\']));
        // eq_ignore_ascii_case は長さ保存(NTFS の大文字小文字ゆれだけ吸収、非 ASCII は厳密比較)。
        // get(..) は文字境界も守る(境界を跨ぐなら一致し得ない = None で不一致扱い)。
        if let Some(prefix) = arg_s.get(..exe_s.len())
            && prefix.eq_ignore_ascii_case(&exe_s)
            && arg_s[exe_s.len()..].starts_with('/')
        {
            return Ok(arg_s[exe_s.len()..].to_string());
        }
    }
    anyhow::bail!(
        "パス '{arg}' は MSYS(Git Bash)のパス変換で書き換えられたようです。\
         `MSYS_NO_PATHCONV=1 tbm …` で変換を止めるか、先頭 `/` を外した相対パスで指定してください"
    )
}

/// `X:` 開頭(Windows ドライブレター)か。遠端パスには現れ得ない形。
fn has_drive_prefix(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

/// `\` → `/`(向きゆれの正規化。比較・復元は `/` 基準で行う)。
fn slashify(s: &str) -> String {
    s.replace('\\', "/")
}

#[cfg(test)]
mod msys_tests {
    use super::{MsysEnv, normalize_remote_path};

    fn msys(exepath: Option<&str>) -> MsysEnv {
        MsysEnv {
            in_msys: true,
            exepath: exepath.map(str::to_owned),
        }
    }

    #[test]
    fn normalize_remote_path_truth_table() {
        let plain = MsysEnv {
            in_msys: false,
            exepath: None,
        };
        let git = msys(Some(r"C:\Program Files\Git"));

        // MSYS 外:そのまま(ドライブレターも触らない)
        assert_eq!(normalize_remote_path("C:/x", &plain).unwrap(), "C:/x");
        assert_eq!(normalize_remote_path("/a/b", &plain).unwrap(), "/a/b");
        // 双スラッシュエスケープの還元
        assert_eq!(normalize_remote_path("//data/x", &git).unwrap(), "/data/x");
        // EXEPATH 接頭辞の剥離(スラッシュ向き・大文字小文字ゆれを吸収、rest の大文字小文字は保存)
        assert_eq!(
            normalize_remote_path("C:/Program Files/Git/data/X.txt", &git).unwrap(),
            "/data/X.txt"
        );
        assert_eq!(
            normalize_remote_path(r"c:\program files\git\Data", &git).unwrap(),
            "/Data"
        );
        // 接頭辞が似ているだけの別パスは剥がさない(Git2 ≠ Git)
        assert!(normalize_remote_path("C:/Program Files/Git2/data", &git).is_err());
        // EXEPATH 不明(純 MSYS2 等)でドライブレター → 次の一手つきエラー
        assert!(normalize_remote_path("C:/msys64/data/x", &msys(None)).is_err());
        // 相対・素の絶対・空はそのまま
        assert_eq!(normalize_remote_path("data/x", &git).unwrap(), "data/x");
        assert_eq!(normalize_remote_path("/data/x", &git).unwrap(), "/data/x");
        assert_eq!(normalize_remote_path("", &git).unwrap(), "");
    }
}
