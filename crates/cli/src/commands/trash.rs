use anyhow::Result;
use clap::Subcommand;
use serde_json::json;

use crate::api;
use crate::commands::{OutputFormat, print_json, resolve_server_from, resolve_token_from};
use crate::config;
use tsubomi_shared::TrashItemDto;

/// `tbm trash <サブコマンド>`。4 種リソース共通のゴミ箱(M1/M2)。
/// 復元 / 完全削除は表示名で指定する(同名が複数種別にある場合のみ曖昧エラー)。
#[derive(Subcommand)]
pub enum TrashCmd {
    /// ゴミ箱の中身を一覧
    List,
    /// 復元(削除から 3 日以内)
    Restore {
        /// 復元するリソースの表示名(`tbm trash list` で確認)
        name: String,
    },
    /// 完全に削除(元に戻せません)
    Purge {
        /// 完全削除するリソースの表示名(`tbm trash list` で確認)
        name: String,
    },
}

pub async fn run(
    action: TrashCmd,
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
        TrashCmd::List => {
            let items = api::trash_list(&c, &server_url, &token).await?;
            if json {
                print_json(&items)?;
            } else if items.is_empty() {
                println!("(ゴミ箱は空です)");
            } else {
                for it in items {
                    let purge = it
                        .purge_after
                        .map(|p| p.format("%Y-%m-%d").to_string())
                        .unwrap_or_else(|| "—".into());
                    println!(
                        "{:<12} {:<24} 削除 {} / 自動削除 {}",
                        kind_ja(&it.kind),
                        it.display_name,
                        it.deleted_at.format("%Y-%m-%d"),
                        purge,
                    );
                }
            }
        }
        TrashCmd::Restore { name } => {
            let id = resolve_id(&c, &server_url, &token, &name).await?;
            api::trash_restore(&c, &server_url, &token, &id).await?;
            if json {
                print_json(&json!({ "status": "restored", "name": name }))?;
            } else {
                println!("復元しました:{name}");
            }
        }
        TrashCmd::Purge { name } => {
            let id = resolve_id(&c, &server_url, &token, &name).await?;
            api::trash_purge(&c, &server_url, &token, &id).await?;
            if json {
                print_json(&json!({ "status": "purged", "name": name }))?;
            } else {
                println!("完全に削除しました:{name}(元に戻せません)");
            }
        }
    }
    Ok(())
}

/// kind → 日本語表示。
fn kind_ja(kind: &str) -> &'static str {
    match kind {
        "service" => "サービス",
        "database" => "データベース",
        "cache" => "キャッシュ",
        "volume" => "ボリューム",
        _ => "その他",
    }
}

/// 表示名 → id をゴミ箱一覧から解決する。同名が複数種別にあるときだけ曖昧エラー。
async fn resolve_id(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    name: &str,
) -> Result<String> {
    let items = api::trash_list(c, server_url, token).await?;
    let matches: Vec<&TrashItemDto> = items.iter().filter(|t| t.display_name == name).collect();
    match matches.as_slice() {
        [one] => Ok(one.id.to_string()),
        [] => Err(api::ApiError {
            code: "not_found",
            message: format!("ゴミ箱に '{name}' が見つかりません(`tbm trash list` で確認)"),
        }
        .into()),
        many => {
            let kinds: Vec<&str> = many.iter().map(|t| kind_ja(&t.kind)).collect();
            Err(api::ApiError {
                code: "conflict",
                message: format!("'{name}' が複数あります(種別: {})", kinds.join(", ")),
            }
            .into())
        }
    }
}
