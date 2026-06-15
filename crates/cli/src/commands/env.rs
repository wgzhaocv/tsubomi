use anyhow::Result;
use clap::Subcommand;
use serde_json::json;

use crate::api;
use crate::commands::{
    OutputFormat, print_json, resolve_server_from, resolve_service_id, resolve_token_from,
};
use crate::config;

/// `tbm env <サブコマンド> <svc>`。service の静的 env(値は暗号化保存、反映には再デプロイ)。
#[derive(Subcommand)]
pub enum EnvCmd {
    /// env を設定(KEY=VALUE。複数可)。反映には再デプロイ
    Set {
        /// サービスの表示名
        svc: String,
        /// KEY=VALUE(複数可)
        #[arg(required = true)]
        pairs: Vec<String>,
    },
    /// env を削除(KEY。複数可)
    Unset {
        /// サービスの表示名
        svc: String,
        /// 削除する KEY(複数可)
        #[arg(required = true)]
        keys: Vec<String>,
    },
    /// env の key 一覧(値は表示しない)
    List {
        /// サービスの表示名
        svc: String,
    },
}

pub async fn run(
    action: EnvCmd,
    server: Option<String>,
    token: Option<String>,
    out: OutputFormat,
) -> Result<()> {
    let cfg = config::load()?;
    let server_url = resolve_server_from(server.as_deref(), cfg.as_ref());
    let token = resolve_token_from(token, cfg)?;
    let json = out.is_json();
    let c = reqwest::Client::new();

    match action {
        EnvCmd::Set { svc, pairs } => {
            let id = resolve_service_id(&c, &server_url, &token, &svc).await?;
            let mut keys = Vec::new();
            for pair in &pairs {
                let (k, v) = pair
                    .split_once('=')
                    .ok_or_else(|| anyhow::anyhow!("'{pair}' は KEY=VALUE 形式ではありません"))?;
                api::env_set(&c, &server_url, &token, &id, k, v).await?;
                keys.push(k.to_string());
            }
            if json {
                print_json(&json!({ "set": keys }))?;
            } else {
                eprintln!(
                    "env を設定しました(反映には再デプロイが必要です):{}",
                    keys.join(", ")
                );
            }
        }
        EnvCmd::Unset { svc, keys } => {
            let id = resolve_service_id(&c, &server_url, &token, &svc).await?;
            for key in &keys {
                api::env_unset(&c, &server_url, &token, &id, key).await?;
            }
            if json {
                print_json(&json!({ "unset": keys }))?;
            } else {
                eprintln!(
                    "env を削除しました(反映には再デプロイが必要です):{}",
                    keys.join(", ")
                );
            }
        }
        EnvCmd::List { svc } => {
            let id = resolve_service_id(&c, &server_url, &token, &svc).await?;
            let keys = api::env_keys(&c, &server_url, &token, &id).await?;
            if json {
                // 値は秘密。key のみ返す。
                print_json(&keys)?;
            } else if keys.is_empty() {
                println!("(env はありません。`tbm env set {svc} KEY=VALUE` で設定)");
            } else {
                for k in &keys {
                    println!("{k}");
                }
            }
        }
    }
    Ok(())
}
