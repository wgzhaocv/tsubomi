pub mod health;
pub mod login;
pub mod logout;
pub mod uninstall;
pub mod update;
pub mod whoami;

use anyhow::{Context, Result};

use crate::config::Config;

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
