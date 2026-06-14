use anyhow::Result;
use clap::Subcommand;
use serde_json::json;

use crate::api;
use crate::commands::{OutputFormat, print_json, resolve_server_from, resolve_token_from};
use crate::config;
use tsubomi_shared::VolumeDto;

/// `tbm volume <サブコマンド>`。各コマンド = API 呼び出し 1 本(web と同じハンドラ)。
/// 資源 CRUD に加え、假根の中のファイル操作(ls/put/get/rm/mkdir/mv)も持つ。
#[derive(Subcommand)]
pub enum VolumeCmd {
    /// ボリュームを作成
    Create {
        /// 表示名(例:myapp-storage)
        name: String,
    },
    /// 一覧
    List,
    /// 表示名を変更(host_path・ファイルは不変)
    Rename {
        /// 対象ボリュームの表示名(`tbm volume list` で確認)
        name: String,
        /// 新しい表示名
        new_name: String,
    },
    /// 削除(ゴミ箱へ。3 日間は復元可能)
    Delete {
        /// 削除するボリュームの表示名(`tbm volume list` で確認)
        name: String,
    },
    /// ディレクトリの中身を一覧
    Ls {
        /// 対象ボリュームの表示名(`tbm volume list` で確認)
        volume: String,
        /// 假根からの相対パス(省略時はルート)
        #[arg(default_value = "")]
        path: String,
    },
    /// ローカルファイルをアップロード
    Put {
        /// 対象ボリュームの表示名(`tbm volume list` で確認)
        volume: String,
        /// アップロードするローカルファイル
        local: String,
        /// 假根内の保存先パス(省略時はローカルファイル名でルート直下)
        remote: Option<String>,
    },
    /// ファイルをダウンロード
    Get {
        /// 対象ボリュームの表示名(`tbm volume list` で確認)
        volume: String,
        /// 假根内のファイルパス
        remote: String,
        /// 保存先ローカルパス(省略時は remote のファイル名)
        local: Option<String>,
    },
    /// ファイル / ディレクトリを削除(ディレクトリは再帰)
    Rm {
        /// 対象ボリュームの表示名(`tbm volume list` で確認)
        volume: String,
        /// 假根からの相対パス
        path: String,
    },
    /// ディレクトリを作成(mkdir -p)
    Mkdir {
        /// 対象ボリュームの表示名(`tbm volume list` で確認)
        volume: String,
        /// 假根からの相対パス
        path: String,
    },
    /// 同一ボリューム内で移動 / リネーム
    Mv {
        /// 対象ボリュームの表示名(`tbm volume list` で確認)
        volume: String,
        /// 移動元(假根からの相対パス)
        from: String,
        /// 移動先(假根からの相対パス)
        to: String,
    },
}

pub async fn run(
    action: VolumeCmd,
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
        VolumeCmd::Create { name } => {
            let v = api::volume_create(&c, &server_url, &token, &name).await?;
            if json {
                print_json(&v)?;
            } else {
                println!("作成しました:{} (volume{})", v.display_name, v.anon_seq);
                println!("ファイル一覧:  tbm volume ls {}", v.display_name);
            }
        }
        VolumeCmd::List => {
            let vs = api::volume_list(&c, &server_url, &token).await?;
            if json {
                print_json(&vs)?;
            } else if vs.is_empty() {
                println!("(ボリュームはありません。`tbm volume create <名前>` で作成)");
            } else {
                for v in vs {
                    println!("volume{:<3} {}", v.anon_seq, v.display_name);
                }
            }
        }
        VolumeCmd::Rename { name, new_name } => {
            let id = resolve_id(&c, &server_url, &token, &name).await?;
            let v = api::volume_rename(&c, &server_url, &token, &id, &new_name).await?;
            if json {
                print_json(&v)?;
            } else {
                println!("名前を変更しました:{}", v.display_name);
            }
        }
        VolumeCmd::Delete { name } => {
            let id = resolve_id(&c, &server_url, &token, &name).await?;
            api::volume_delete(&c, &server_url, &token, &id).await?;
            if json {
                print_json(&json!({ "status": "deleted", "recoverable_days": 3 }))?;
            } else {
                println!("削除しました(ゴミ箱へ。3 日間は復元可能)。");
            }
        }
        VolumeCmd::Ls { volume, path } => {
            let id = resolve_id(&c, &server_url, &token, &volume).await?;
            let listing = api::volume_ls(&c, &server_url, &token, &id, &path).await?;
            if json {
                print_json(&listing)?;
            } else if listing.entries.is_empty() {
                println!("(空のディレクトリ)");
            } else {
                for e in &listing.entries {
                    let kind = if e.is_dir { "d" } else { "-" };
                    let size = if e.is_dir {
                        String::new()
                    } else {
                        e.size.to_string()
                    };
                    let name = if e.is_dir {
                        format!("{}/", e.name)
                    } else {
                        e.name.clone()
                    };
                    println!("{kind} {size:>10}  {name}");
                }
            }
        }
        VolumeCmd::Put {
            volume,
            local,
            remote,
        } => {
            let id = resolve_id(&c, &server_url, &token, &volume).await?;
            let remote = remote.unwrap_or_else(|| basename(&local));
            // ストリーム上传(api 側でファイルを開いて逐次送信)。返り値が送信バイト数。
            let bytes = api::volume_upload(&c, &server_url, &token, &id, &remote, &local).await?;
            if json {
                print_json(&json!({ "status": "uploaded", "path": remote, "bytes": bytes }))?;
            } else {
                println!("アップロードしました:{remote} ({bytes} バイト)");
            }
        }
        VolumeCmd::Get {
            volume,
            remote,
            local,
        } => {
            let id = resolve_id(&c, &server_url, &token, &volume).await?;
            let local = local.unwrap_or_else(|| basename(&remote));
            // ストリーム下载(api 側で dest へ逐次書き込み)。返り値が書込バイト数。
            let bytes = api::volume_download(&c, &server_url, &token, &id, &remote, &local).await?;
            if json {
                print_json(&json!({ "status": "downloaded", "path": local, "bytes": bytes }))?;
            } else {
                println!("ダウンロードしました:{local} ({bytes} バイト)");
            }
        }
        VolumeCmd::Rm { volume, path } => {
            let id = resolve_id(&c, &server_url, &token, &volume).await?;
            api::volume_rm(&c, &server_url, &token, &id, &path).await?;
            if json {
                print_json(&json!({ "status": "removed", "path": path }))?;
            } else {
                println!("削除しました:{path}");
            }
        }
        VolumeCmd::Mkdir { volume, path } => {
            let id = resolve_id(&c, &server_url, &token, &volume).await?;
            api::volume_mkdir(&c, &server_url, &token, &id, &path).await?;
            if json {
                print_json(&json!({ "status": "created", "path": path }))?;
            } else {
                println!("作成しました:{path}/");
            }
        }
        VolumeCmd::Mv { volume, from, to } => {
            let id = resolve_id(&c, &server_url, &token, &volume).await?;
            api::volume_move(&c, &server_url, &token, &id, &from, &to).await?;
            if json {
                print_json(&json!({ "status": "moved", "from": from, "to": to }))?;
            } else {
                println!("移動しました:{from} → {to}");
            }
        }
    }
    Ok(())
}

/// 表示名 → id を一覧から解決する(専用エンドポイントを増やさない。db.rs と同じ流儀)。
async fn resolve_id(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    name: &str,
) -> Result<String> {
    let vs = api::volume_list(c, server_url, token).await?;
    match vs.iter().find(|v: &&VolumeDto| v.display_name == name) {
        Some(v) => Ok(v.id.to_string()),
        None => Err(api::ApiError {
            code: "not_found",
            message: format!("ボリューム '{name}' が見つかりません(`tbm volume list` で確認)"),
        }
        .into()),
    }
}

/// パス文字列の最後の成分(ファイル名)を取り出す。put/get の保存先デフォルトに使う。
fn basename(p: &str) -> String {
    std::path::Path::new(p)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.to_owned())
}
