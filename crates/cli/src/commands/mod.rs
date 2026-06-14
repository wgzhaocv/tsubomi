pub mod db;
pub mod deploy;
pub mod health;
pub mod login;
pub mod logout;
pub mod service;
pub mod trash;
pub mod uninstall;
pub mod update;
pub mod volume;
pub mod whoami;

use anyhow::{Context, Result};

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
