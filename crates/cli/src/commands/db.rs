use anyhow::{Result, bail};
use clap::Subcommand;

use crate::api;
use crate::commands::{resolve_server_from, resolve_token_from};
use crate::config;
use tsubomi_shared::DatabaseDto;

/// `tbm db <サブコマンド>`。各コマンド = API 呼び出し 1 本(web と同じハンドラ)。
#[derive(Subcommand)]
pub enum DbCmd {
    /// データベースを作成
    Create {
        /// 表示名(例:myapp-db)
        name: String,
    },
    /// 一覧
    List,
    /// 外部接続文字列を表示(= パスワードそのもの。git に commit しない / 共有しない)
    Url { name: String },
    /// パスワードを再生成(古い接続文字列は即座に失効)
    Rotate { name: String },
    /// 削除(ゴミ箱へ。3 日間は復元可能)
    Delete { name: String },
    /// psql で接続(パスワードを露出せず接続。要 psql)
    Connect { name: String },
}

pub async fn run(action: DbCmd, server: Option<String>, token: Option<String>) -> Result<()> {
    let cfg = config::load()?;
    let server_url = resolve_server_from(server.as_deref(), cfg.as_ref());
    let token = resolve_token_from(token, cfg)?;
    // コマンド内の全リクエストで 1 つの client を使い回す(TLS 初期化を 1 回に)。
    let c = reqwest::Client::new();

    match action {
        DbCmd::Create { name } => {
            let db = api::db_create(&c, &server_url, &token, &name).await?;
            println!("作成しました:{} (database{})", db.display_name, db.anon_seq);
            println!("接続文字列:  tbm db url {}", db.display_name);
        }
        DbCmd::List => {
            let dbs = api::db_list(&c, &server_url, &token).await?;
            if dbs.is_empty() {
                println!("(データベースはありません。`tbm db create <名前>` で作成)");
            } else {
                for db in dbs {
                    println!("database{:<3} {}", db.anon_seq, db.display_name);
                }
            }
        }
        DbCmd::Url { name } => {
            let id = resolve_id(&c, &server_url, &token, &name).await?;
            let url = api::db_url(&c, &server_url, &token, &id).await?;
            // 警告は stderr、文字列は stdout(パイプで拾えるように)。
            eprintln!("⚠ この文字列はパスワードそのものです。共有・commit しないこと。");
            println!("{url}");
        }
        DbCmd::Rotate { name } => {
            let id = resolve_id(&c, &server_url, &token, &name).await?;
            let url = api::db_rotate(&c, &server_url, &token, &id).await?;
            eprintln!("rotate しました。古い接続文字列は失効しました。新しい接続文字列:");
            println!("{url}");
        }
        DbCmd::Delete { name } => {
            let id = resolve_id(&c, &server_url, &token, &name).await?;
            api::db_delete(&c, &server_url, &token, &id).await?;
            println!("削除しました(ゴミ箱へ。3 日間は復元可能)。");
        }
        DbCmd::Connect { name } => {
            let id = resolve_id(&c, &server_url, &token, &name).await?;
            let url = api::db_url(&c, &server_url, &token, &id).await?;
            connect_psql(&url)?;
        }
    }
    Ok(())
}

/// 表示名 → id を一覧から解決する(専用エンドポイントを増やさない)。
async fn resolve_id(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    name: &str,
) -> Result<String> {
    let dbs = api::db_list(c, server_url, token).await?;
    match dbs.iter().find(|d: &&DatabaseDto| d.display_name == name) {
        Some(db) => Ok(db.id.to_string()),
        None => bail!("データベース '{name}' が見つかりません(`tbm db list` で確認)"),
    }
}

/// psql を exec する。パスワードは PGPASSWORD で渡し、argv(= `ps` で見える)には
/// 載せない。psql が無ければ接続文字列を表示してフォールバックする。
fn connect_psql(url: &str) -> Result<()> {
    let mut parsed = url::Url::parse(url)?;
    let password = parsed.password().unwrap_or_default().to_owned();
    // argv からパスワードを外す(host/user/db/sslmode だけを残す)。
    let _ = parsed.set_password(None);

    let status = std::process::Command::new("psql")
        .arg(parsed.as_str())
        .env("PGPASSWORD", password)
        .status();

    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => bail!("psql が異常終了しました:{s}"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("psql が見つかりません。手動で接続してください:");
            println!("{url}");
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}
