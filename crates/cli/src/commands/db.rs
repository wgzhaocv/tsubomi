use anyhow::{Context, Result, bail};
use clap::Subcommand;
use serde_json::json;

use crate::api;
use crate::commands::{OutputFormat, print_json, resolve_server_from, resolve_token_from};
use crate::config;
use tsubomi_shared::{DatabaseDto, QueryResp};

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
    /// 接続枠の上限と現在の使用量を表示(接続が満杯に近いかの確認)
    Info {
        /// 対象データベースの表示名(`tbm db list` で確認)
        name: String,
    },
    /// 外部接続文字列を表示(= パスワードそのもの。git に commit しない / 共有しない)
    Url {
        /// 対象データベースの表示名(`tbm db list` で確認)
        name: String,
    },
    /// パスワードを再生成(古い接続文字列は即座に失効)
    Rotate {
        /// rotate するデータベースの表示名(`tbm db list` で確認)
        name: String,
    },
    /// 削除(ゴミ箱へ。3 日間は復元可能)
    Delete {
        /// 削除するデータベースの表示名(`tbm db list` で確認)
        name: String,
    },
    /// psql で接続(パスワードを露出せず接続。要 psql)
    Connect {
        /// 接続するデータベースの表示名(`tbm db list` で確認)
        name: String,
    },
    /// SQL を実行(psql 不要。web の SQL エディタと同じ経路。複数文可)
    Query {
        /// 対象データベースの表示名(`tbm db list` で確認)
        name: String,
        /// 実行する SQL(`-` を渡すと標準入力から読む)
        sql: String,
    },
}

pub async fn run(
    action: DbCmd,
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
        DbCmd::Create { name } => {
            let db = api::db_create(&c, &server_url, &token, &name).await?;
            if json {
                print_json(&db)?;
            } else {
                println!("作成しました:{} (database{})", db.display_name, db.anon_seq);
                println!("接続文字列:  tbm db url {}", db.display_name);
            }
        }
        DbCmd::List => {
            let dbs = api::db_list(&c, &server_url, &token).await?;
            if json {
                print_json(&dbs)?;
            } else if dbs.is_empty() {
                println!("(データベースはありません。`tbm db create <名前>` で作成)");
            } else {
                for db in dbs {
                    println!("database{:<3} {}", db.anon_seq, db.display_name);
                }
            }
        }
        DbCmd::Info { name } => {
            let id = resolve_id(&c, &server_url, &token, &name).await?;
            let cap = api::db_capacity(&c, &server_url, &token, &id).await?;
            if json {
                // 共有 DTO(DatabaseCapacityDto)をそのまま:
                // { conn_limit, human_connections, app_connections, pool_mode }。jq で拾える。
                print_json(&cap)?;
            } else {
                println!("接続上限:   {}(1 ロールあたり)", cap.conn_limit);
                println!(
                    "現在の接続: human {} / app {}(pool={})",
                    cap.human_connections, cap.app_connections, cap.pool_mode
                );
                println!(
                    "💡 コネクションプール推奨:少数の長命接続を使い回す(リクエスト毎の新規接続は上限を圧迫)"
                );
            }
        }
        DbCmd::Url { name } => {
            let id = resolve_id(&c, &server_url, &token, &name).await?;
            let url = api::db_url(&c, &server_url, &token, &id).await?;
            if json {
                print_json(&json!({ "url": url }))?;
            } else {
                // 警告は stderr、文字列は stdout(パイプで拾えるように)。
                eprintln!("⚠ この文字列はパスワードそのものです。共有・commit しないこと。");
                eprintln!(
                    "💡 コネクションプール推奨:少数の長命接続を使い回す(上限・使用量は `tbm db info {name}`)。"
                );
                println!("{url}");
            }
        }
        DbCmd::Rotate { name } => {
            let id = resolve_id(&c, &server_url, &token, &name).await?;
            let url = api::db_rotate(&c, &server_url, &token, &id).await?;
            if json {
                print_json(&json!({ "url": url, "rotated": true }))?;
            } else {
                eprintln!("rotate しました。古い接続文字列は失効しました。新しい接続文字列:");
                println!("{url}");
            }
        }
        DbCmd::Delete { name } => {
            let id = resolve_id(&c, &server_url, &token, &name).await?;
            api::db_delete(&c, &server_url, &token, &id).await?;
            if json {
                print_json(&json!({ "status": "deleted", "recoverable_days": 3 }))?;
            } else {
                println!("削除しました(ゴミ箱へ。3 日間は復元可能)。");
            }
        }
        DbCmd::Connect { name } => {
            let id = resolve_id(&c, &server_url, &token, &name).await?;
            let url = api::db_url(&c, &server_url, &token, &id).await?;
            if json {
                // json モードでは対話的 psql は起動せず、接続先だけ返す(AI 用)。
                print_json(&json!({ "url": url }))?;
            } else {
                connect_psql(&url)?;
            }
        }
        DbCmd::Query { name, sql } => {
            let sql = read_sql_arg(&sql)?;
            let id = resolve_id(&c, &server_url, &token, &name).await?;
            let resp = api::db_query(&c, &server_url, &token, &id, &sql).await?;
            if json {
                // 共有 DTO(QueryResp)をそのまま出す:{ "results": [ { columns, rows,
                // row_count, truncated, rows_affected }, ... ] }。jq で拾える。
                print_json(&resp)?;
            } else {
                print_results_text(&resp);
            }
        }
    }
    Ok(())
}

/// SQL 引数を解決する。`-` のときは標準入力から全部読む(大きな SQL / here-doc 用)。
fn read_sql_arg(arg: &str) -> Result<String> {
    if arg == "-" {
        let mut buf = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)
            .context("標準入力の読み取りに失敗しました")?;
        Ok(buf)
    } else {
        Ok(arg.to_owned())
    }
}

/// text モードの結果表示。文ごとに 1 ブロック:SELECT 系は表 + 件数、それ以外
/// (INSERT/UPDATE/DDL)は影響行数を出す。複数文のときは見出しで区切る。
fn print_results_text(resp: &QueryResp) {
    // 空 SQL / コメントのみ等でサーバが結果集合を 1 つも返さないとき。json は
    // `{"results":[]}` で自明だが、text だと無出力で紛らわしいので一言出す。
    if resp.results.is_empty() {
        println!("(結果なし)");
        return;
    }
    let multi = resp.results.len() > 1;
    for (i, rs) in resp.results.iter().enumerate() {
        if multi {
            if i > 0 {
                println!();
            }
            println!("-- 文 {} --", i + 1);
        }
        if rs.columns.is_empty() {
            // 非 SELECT(列が無い)= 影響行数で結果を示す。
            println!("OK({} 行に影響)", rs.rows_affected);
        } else {
            print_table(&rs.columns, &rs.rows);
            if rs.truncated {
                println!("({} 行、上限で切り詰め)", rs.row_count);
            } else {
                println!("({} 行)", rs.row_count);
            }
        }
    }
}

/// 簡易な整列テーブル(NULL は `NULL` 表示)。幅は char 数で揃える
/// (CJK 全角は厳密には合わないが dev 確認用としては十分。AI 経路は JSON)。
fn print_table(cols: &[String], rows: &[Vec<Option<String>>]) {
    // NULL(None)は `NULL` と表示。クロージャだと借用寿命を表せないので fn にする。
    fn cell(c: Option<&String>) -> &str {
        c.map(String::as_str).unwrap_or("NULL")
    }
    let mut widths: Vec<usize> = cols.iter().map(|c| c.chars().count()).collect();
    for row in rows {
        for (i, c) in row.iter().enumerate() {
            if let Some(w) = widths.get_mut(i) {
                *w = (*w).max(cell(c.as_ref()).chars().count());
            }
        }
    }
    let pad = |s: &str, w: usize| {
        let n = s.chars().count();
        format!("{s}{}", " ".repeat(w.saturating_sub(n)))
    };
    let header: Vec<String> = cols.iter().enumerate().map(|(i, c)| pad(c, widths[i])).collect();
    println!("{}", header.join(" | "));
    let sep: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();
    println!("{}", sep.join("-+-"));
    for row in rows {
        let line: Vec<String> = (0..cols.len())
            .map(|i| pad(cell(row.get(i).and_then(|c| c.as_ref())), widths[i]))
            .collect();
        println!("{}", line.join(" | "));
    }
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
        // クライアント側解決の「見つからない」も安定コードを付ける(AI が
        // not_found を文字列照合せず機械分岐できるように)。
        None => Err(api::ApiError {
            code: "not_found",
            message: format!("データベース '{name}' が見つかりません(`tbm db list` で確認)"),
        }
        .into()),
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
