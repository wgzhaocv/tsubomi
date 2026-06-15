use anyhow::Result;
use clap::Subcommand;
use serde_json::json;

use crate::api;
use crate::commands::{OutputFormat, print_json, resolve_server_from, resolve_token_from};
use crate::config;
use tsubomi_shared::CacheDto;

/// `tbm cache <サブコマンド>`。各コマンド = API 呼び出し 1 本(web と同じハンドラ)。
#[derive(Subcommand)]
pub enum CacheCmd {
    /// キャッシュを作成
    Create {
        /// 表示名(例:myapp-cache)
        name: String,
    },
    /// 一覧
    List,
    /// 状態(namespace / REDIS_KEY_PREFIX / キー数 / 最終 rotate)
    Status {
        /// 対象キャッシュの表示名(`tbm cache list` で確認)
        name: String,
    },
    /// 内部接続文字列(REDIS_URL)を表示(= パスワードそのもの。git に commit しない / 共有しない)
    Url {
        /// 対象キャッシュの表示名(`tbm cache list` で確認)
        name: String,
    },
    /// パスワードを再生成(古い接続文字列は即座に失効。反映には再デプロイが必要)
    Rotate {
        /// rotate するキャッシュの表示名(`tbm cache list` で確認)
        name: String,
    },
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
        CacheCmd::Status { name } => {
            let id = resolve_id(&c, &server_url, &token, &name).await?;
            let d = api::cache_get(&c, &server_url, &token, &id).await?;
            if json {
                print_json(&d)?;
            } else {
                println!("cache{:<3} {}", d.anon_seq, d.display_name);
                println!("REDIS_KEY_PREFIX: {}:", d.namespace);
                match d.key_count {
                    Some(n) => println!("キー数:           {n}(概算)"),
                    None => println!("キー数:           (取得不能)"),
                }
                if let Some(at) = d.rotated_at {
                    println!("最終 rotate:      {at}");
                }
            }
        }
        CacheCmd::Url { name } => {
            let id = resolve_id(&c, &server_url, &token, &name).await?;
            let url = api::cache_url(&c, &server_url, &token, &id).await?;
            if json {
                print_json(&json!({ "url": url }))?;
            } else {
                // 警告は stderr、文字列は stdout(パイプで拾えるように)。内部入口なので
                // 注入された service コンテナからのみ繋がる(手元からは届かない)。
                eprintln!("⚠ この文字列はパスワードそのものです。共有・commit しないこと。");
                eprintln!("  (内部入口のため、注入された service のコンテナからのみ接続できます)");
                println!("{url}");
            }
        }
        CacheCmd::Rotate { name } => {
            let id = resolve_id(&c, &server_url, &token, &name).await?;
            let url = api::cache_rotate(&c, &server_url, &token, &id).await?;
            if json {
                print_json(&json!({ "url": url, "rotated": true }))?;
            } else {
                eprintln!(
                    "rotate しました。古い接続文字列は失効しました(反映には再デプロイが必要)。新しい接続文字列:"
                );
                println!("{url}");
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
