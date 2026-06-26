use anyhow::{Result, bail};
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
    /// 表示名を変更(接続文字列・namespace は不変)
    Rename {
        /// 対象キャッシュの表示名(`tbm cache list` で確認)
        name: String,
        /// 新しい表示名
        new_name: String,
    },
    /// 状態(namespace / REDIS_KEY_PREFIX / キー数 / 最終 rotate)
    Status {
        /// 対象キャッシュの表示名(`tbm cache list` で確認)
        name: String,
    },
    /// 接続文字列を表示(= パスワードそのもの。git に commit しない / 共有しない)。公開時は外部
    /// `rediss://`(手元から直接繋がる)、否なら内部 `redis://`(注入された service コンテナ専用)
    Url {
        /// 対象キャッシュの表示名(`tbm cache list` で確認)
        name: String,
    },
    /// パスワードを再生成(古い接続文字列は即座に失効。注入の反映には再デプロイが必要)
    Rotate {
        /// rotate するキャッシュの表示名(`tbm cache list` で確認)
        name: String,
    },
    /// 削除(ゴミ箱へ。3 日間は復元可能)
    Delete {
        /// 削除するキャッシュの表示名(`tbm cache list` で確認)
        name: String,
    },
    /// redis-cli で接続(パスワードを露出せず接続。要 redis-cli。公開 cache が有効な部署のみ)
    Connect {
        /// 接続するキャッシュの表示名(`tbm cache list` で確認)
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
        CacheCmd::Rename { name, new_name } => {
            let id = resolve_id(&c, &server_url, &token, &name).await?;
            let d = api::cache_rename(&c, &server_url, &token, &id, &new_name).await?;
            if json {
                print_json(&d)?;
            } else {
                println!("名前を変更しました:{}", d.display_name);
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
                // 警告は stderr、文字列は stdout(パイプで拾えるように)。
                eprintln!("⚠ この文字列はパスワードそのものです。共有・commit しないこと。");
                cache_url_notes(&url);
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
        CacheCmd::Connect { name } => {
            let id = resolve_id(&c, &server_url, &token, &name).await?;
            let url = api::cache_url(&c, &server_url, &token, &id).await?;
            if json {
                // json モードでは対話的 redis-cli は起動せず、接続先だけ返す(AI 用。db connect と同型)。
                print_json(&json!({ "url": url }))?;
            } else {
                connect_redis_cli(&url)?;
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

/// url / connect の text モードで出す注意書き(stderr)。外部 `rediss://` と内部 `redis://` で文言を分け、
/// 外部時は keyPrefix(= `<namespace>:`)を付けないと NOPERM になる点を出す。
fn cache_url_notes(url: &str) {
    match redis_namespace(url) {
        Some(ns) if url.starts_with("rediss://") => {
            eprintln!("  あなたの PC から直接繋がります(TLS)。キー前缀 \"{ns}:\" を必ず付けてください");
            eprintln!("  (付けないと NOPERM。例:new Redis(URL, {{ keyPrefix: \"{ns}:\" }}))");
        }
        _ => eprintln!("  (内部入口のため、注入された service のコンテナからのみ接続できます)"),
    }
}

/// `rediss://c_xxx:pw@host:port` の username(= acl_user = namespace)を取り出す。
fn redis_namespace(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    let user = u.username();
    (!user.is_empty()).then(|| user.to_owned())
}

/// redis-cli を exec する。パスワードは `REDISCLI_AUTH` で渡し argv には載せない(`ps` 対策。db の
/// `connect_psql` と同型)。外部 `rediss://` のときだけ起動する(内部 `redis://` は手元から届かないので
/// 説明して終える)。redis-cli が無ければ接続文字列を表示してフォールバック。
fn connect_redis_cli(url: &str) -> Result<()> {
    if !url.starts_with("rediss://") {
        eprintln!(
            "この接続文字列は内部入口(注入された service コンテナ専用)で、手元からは繋がりません。"
        );
        eprintln!("`tbm cache connect` は公開 cache が有効な部署でのみ使えます。接続文字列:");
        println!("{url}");
        return Ok(());
    }
    let parsed = url::Url::parse(url)?;
    let host = parsed.host_str().unwrap_or_default().to_owned();
    let port = parsed.port().unwrap_or(443);
    let user = parsed.username().to_owned(); // = acl_user = namespace
    let password = parsed.password().unwrap_or_default().to_owned();
    if !user.is_empty() {
        eprintln!("💡 キー前缀 \"{user}:\" を付けて操作してください(例:GET {user}:foo)。前缀なしは NOPERM。");
    }

    // redis-cli / valkey-cli を **明示フラグ**で起動する(`-u rediss://…` だと (1) SNI を送らず
    // 辺縁の sni-gate に握手段で切られ (2) AUTH env も URL モードでは拾われない、の 2 つで繋がらない。
    // 実機検証済み)。--sni で gate を通し、--user で ACL ユーザ、パスワードは AUTH env で渡し argv に
    // 載せない(`ps` 対策)。redis-cli/valkey-cli の両系 env を立てる。NotFound なら次の候補へ。
    for bin in ["redis-cli", "valkey-cli"] {
        match std::process::Command::new(bin)
            .arg("--tls")
            .arg("--sni")
            .arg(&host)
            .arg("-h")
            .arg(&host)
            .arg("-p")
            .arg(port.to_string())
            .arg("--user")
            .arg(&user)
            .env("REDISCLI_AUTH", &password)
            .env("VALKEYCLI_AUTH", &password)
            .status()
        {
            Ok(s) if s.success() => return Ok(()),
            Ok(s) => bail!("{bin} が異常終了しました:{s}"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        }
    }
    eprintln!("redis-cli / valkey-cli が見つかりません。手動で接続してください:");
    println!("{url}");
    Ok(())
}
