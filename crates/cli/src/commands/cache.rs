use anyhow::Result;
use clap::Subcommand;
use serde_json::json;

use crate::api;
use crate::commands::{OutputFormat, print_json, resolve_server_from, resolve_token_from};
use crate::config;
use tsubomi_shared::CacheDto;

/// `tbm cache <サブコマンド>`。各コマンド = API 呼び出し 1 本(web と同じハンドラ)。
/// url / rotate / status は S3 で足す。
#[derive(Subcommand)]
pub enum CacheCmd {
    /// キャッシュを作成
    Create {
        /// 表示名(例:myapp-cache)
        name: String,
    },
    /// 一覧
    List,
    /// 削除(ゴミ箱へ。3 日間は復元可能)
    Delete {
        /// 削除するキャッシュの表示名(`tbm cache list` で確認)
        name: String,
    },
}

pub async fn run(
    action: CacheCmd,
    server: Option<String>,
    token: Option<String>,
    out: OutputFormat,
) -> Result<()> {
    let cfg = config::load()?;
    let server_url = resolve_server_from(server.as_deref(), cfg.as_ref());
    let token = resolve_token_from(token, cfg)?;
    let json = out.is_json();
    // コマンド内の全リクエストで 1 つの client を使い回す(TLS 初期化を 1 回に)。
    let c = reqwest::Client::new();

    match action {
        CacheCmd::Create { name } => {
            let cache = api::cache_create(&c, &server_url, &token, &name).await?;
            if json {
                print_json(&cache)?;
            } else {
                println!(
                    "作成しました:{} (cache{})",
                    cache.display_name, cache.anon_seq
                );
                println!("サービスに注入:  tbm inject {} --into <service>", cache.display_name);
            }
        }
        CacheCmd::List => {
            let caches = api::cache_list(&c, &server_url, &token).await?;
            if json {
                print_json(&caches)?;
            } else if caches.is_empty() {
                println!("(キャッシュはありません。`tbm cache create <名前>` で作成)");
            } else {
                for cache in caches {
                    println!("cache{:<3} {}", cache.anon_seq, cache.display_name);
                }
            }
        }
        CacheCmd::Delete { name } => {
            let id = resolve_id(&c, &server_url, &token, &name).await?;
            api::cache_delete(&c, &server_url, &token, &id).await?;
            if json {
                print_json(&json!({ "status": "deleted", "recoverable_days": 3 }))?;
            } else {
                println!("削除しました(ゴミ箱へ。3 日間は復元可能)。");
            }
        }
    }
    Ok(())
}

/// 表示名 → id を一覧から解決する(専用エンドポイントを増やさない。db.rs と同型)。
async fn resolve_id(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    name: &str,
) -> Result<String> {
    let caches = api::cache_list(c, server_url, token).await?;
    match caches.iter().find(|x: &&CacheDto| x.display_name == name) {
        Some(cache) => Ok(cache.id.to_string()),
        None => Err(api::ApiError {
            code: "not_found",
            message: format!("キャッシュ '{name}' が見つかりません(`tbm cache list` で確認)"),
        }
        .into()),
    }
}
