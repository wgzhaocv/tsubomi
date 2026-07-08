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
        .context("not logged in (run: tbm login)")
}

/// 現在の unix 秒(deploy hook の ts / logs --follow の再接続カーソルが共有)。
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `--for-sha` の値が commit sha の形か(4 桁以上の hex。branch/tag は受けない —
/// 前綴一致し得ず timeout まで空回りするため早期に弾く)。verify / deploy --watch が共有。
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
    let svcs = api::service_list(c, server_url, token).await?;
    svcs.iter()
        .find(|s| s.display_name == name)
        .map(|s| s.id.to_string())
        .ok_or_else(|| {
            api::ApiError {
                code: "not_found",
                message: format!("サービス '{name}' が見つかりません(`tbm service list` で確認)"),
            }
            .into()
        })
}
