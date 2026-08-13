use anyhow::Result;
use clap::Subcommand;
use serde_json::json;

use crate::api;
use crate::commands::{OutputFormat, print_json, resolve_server_from, resolve_token_from};
use crate::config;
use tsubomi_shared::TrashItemDto;

/// `tbm trash <サブコマンド>`。4 種リソース共通のゴミ箱(M1/M2)。
/// 復元 / 完全削除は表示名で指定する。ゴミ箱は名前を占有しない(削除 → 同名で
/// 作り直し可)ため同名が複数堆積し得る — そのときは id(`tbm trash list` の
/// 先頭列。前方一致で可)で特定する。
#[derive(Subcommand)]
pub enum TrashCmd {
    /// ゴミ箱の中身を一覧
    List,
    /// 復元(削除から 3 日以内)
    Restore {
        /// 復元するリソースの表示名、または id(同名が複数あるとき。前方一致可。`tbm trash list` で確認)
        name: String,
    },
    /// 完全に削除(元に戻せません)
    Purge {
        /// 完全削除するリソースの表示名、または id(同名が複数あるとき。前方一致可。`tbm trash list` で確認)
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
                        "{} {:<12} {:<24} 削除 {} / 自動削除 {}",
                        short_id(&it),
                        kind_ja(&it.kind),
                        it.display_name,
                        it.deleted_at.format("%Y-%m-%d"),
                        purge,
                    );
                }
            }
        }
        TrashCmd::Restore { name } => {
            // 回显は解決した実物の名前で(id 指定でも本当の対象名を報告する)。
            let it = resolve_item(&c, &server_url, &token, &name).await?;
            api::trash_restore(&c, &server_url, &token, &it.id.to_string()).await?;
            if json {
                print_json(
                    &json!({ "status": "restored", "name": it.display_name, "id": it.id }),
                )?;
            } else {
                println!("復元しました:{}", it.display_name);
            }
        }
        TrashCmd::Purge { name } => {
            let it = resolve_item(&c, &server_url, &token, &name).await?;
            api::trash_purge(&c, &server_url, &token, &it.id.to_string()).await?;
            if json {
                print_json(&json!({ "status": "purged", "name": it.display_name, "id": it.id }))?;
            } else {
                println!("完全に削除しました:{}(元に戻せません)", it.display_name);
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

/// 一覧・曖昧エラーで見せる id の短縮形(uuid 先頭 8 桁)。前方一致で解決できる長さ。
fn short_id(it: &TrashItemDto) -> String {
    it.id.to_string().chars().take(8).collect()
}

/// 入力が id 前方一致の対象になり得る形か。**8 桁以上**の hex(+ハイフン)に限る:
/// 空文字列(未設定のシェル変数が典型)や 1〜2 桁が唯一の項目に「たまたま」一致して
/// **不可逆な purge** に流れる事故を防ぐ。8 桁は一覧が表示する短縮 id と同じ長さ。
fn looks_like_id_prefix(s: &str) -> bool {
    s.len() >= 8 && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// 表示名 or id → ゴミ箱一覧から対象を解決する(回显用に行ごと返す)。
/// ゴミ箱は名前を占有しないため同名(同種別含む)が複数あり得る:
/// - 完全な UUID は **id として**解決する(名前は見ない — 一覧で見た id は常に正しく効く)。
/// - それ以外は名前の完全一致と id 前方一致(8 桁以上)を両方引き、**片方だけが一意**の
///   ときに採用。両方に該当があれば曖昧エラー(名前が hex 8 桁で別項目の id 頭と被る
///   ケースを黙って名前優先にしない — 不可逆操作の誤対象を防ぐ)。
async fn resolve_item(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    name: &str,
) -> Result<TrashItemDto> {
    let items = api::trash_list(c, server_url, token).await?;
    if let Ok(u) = name.parse::<uuid::Uuid>() {
        return items.into_iter().find(|t| t.id == u).ok_or_else(|| {
            anyhow::Error::from(api::ApiError {
                code: "not_found",
                message: format!("ゴミ箱に id '{name}' が見つかりません(`tbm trash list` で確認)"),
            })
        });
    }
    let needle = name.to_lowercase();
    let matches: Vec<&TrashItemDto> = items
        .iter()
        .filter(|t| {
            t.display_name == name
                || (looks_like_id_prefix(name) && t.id.to_string().starts_with(&needle))
        })
        .collect();
    match matches.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(api::ApiError {
            code: "not_found",
            message: format!("ゴミ箱に '{name}' が見つかりません(`tbm trash list` で確認)"),
        }
        .into()),
        many => {
            // 短縮 id 同士が衝突していても次の一手が打てるよう、候補は完全な id で見せる。
            let candidates: Vec<String> = many
                .iter()
                .map(|t| {
                    format!(
                        "{}({} {}、削除 {})",
                        t.id,
                        kind_ja(&t.kind),
                        t.display_name,
                        t.deleted_at.format("%Y-%m-%d"),
                    )
                })
                .collect();
            Err(api::ApiError {
                code: "conflict",
                message: format!(
                    "'{name}' に該当する項目が複数あります。完全な id で指定してください: {}",
                    candidates.join(" / ")
                ),
            }
            .into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::looks_like_id_prefix;

    #[test]
    fn id_prefix_truth_table() {
        // 8 桁以上の hex(+ハイフン)だけが id 前方一致の対象
        assert!(looks_like_id_prefix("28487b01"));
        assert!(looks_like_id_prefix("28487b01-9f"));
        assert!(looks_like_id_prefix("ABCDEF01")); // 大文字 hex も可(照合側で小文字化)
        // 事故防止:空(未設定シェル変数)・短すぎ・hex 以外は対象外
        assert!(!looks_like_id_prefix(""));
        assert!(!looks_like_id_prefix("2"));
        assert!(!looks_like_id_prefix("28487b0")); // 7 桁
        assert!(!looks_like_id_prefix("my-volume")); // 普通の名前
        assert!(!looks_like_id_prefix("28487b0g")); // hex 外の文字
    }
}
