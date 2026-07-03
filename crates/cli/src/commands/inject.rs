use anyhow::{Result, bail};
use clap::Args;
use serde_json::json;

use crate::api;
use crate::commands::{
    OutputFormat, print_json, resolve_server_from, resolve_service_id, resolve_token_from,
};
use crate::config;

/// `tbm inject <resource> --into <service> [--as ENV] [--mount /path]`。
/// database / volume / cache / 別 service を service に注入(バインディングを保存。値は起動の瞬間に解決)。
/// service 注入は内部直連 URL `http://<subdomain>:<port>` に加え `_HOST` / `_PORT`(素材)を渡す
/// (公網を通らない。同一 owner 限定。非 HTTP ソフトは HOST/PORT で自分のスキームの接続文字列を組む)。
#[derive(Args)]
pub struct InjectArgs {
    /// 注入するリソースの表示名(database / volume / cache / service)
    pub resource: String,
    /// 注入先サービスの表示名
    #[arg(long)]
    pub into: String,
    /// env 変数名(既定:database=DATABASE_URL / volume=STORAGE_PATH / cache=REDIS_URL。
    /// cache は加えて REDIS_KEY_PREFIX(--as 指定時は <ENV>_KEY_PREFIX)、service は加えて
    /// <名前>_HOST / <名前>_PORT(--as 指定時は <ENV> の _URL を剥いだ基底 + _HOST/_PORT)も注入される)
    #[arg(long = "as")]
    pub env_as: Option<String>,
    /// volume のコンテナ内マウント先(既定 /data/<名前>)
    #[arg(long)]
    pub mount: Option<String>,
}

pub async fn run_inject(
    args: InjectArgs,
    server: Option<String>,
    token: Option<String>,
    out: OutputFormat,
) -> Result<()> {
    let cfg = config::load()?;
    let server_url = resolve_server_from(server.as_deref(), cfg.as_ref());
    let token = resolve_token_from(token, cfg)?;
    let json = out.is_json();
    let c = reqwest::Client::new();

    let svc_id = resolve_service_id(&c, &server_url, &token, &args.into).await?;
    let res_id = resolve_resource(&c, &server_url, &token, &args.resource).await?;
    let inj = api::inject_create(
        &c,
        &server_url,
        &token,
        &svc_id,
        &res_id,
        args.env_as.as_deref(),
        args.mount.as_deref(),
    )
    .await?;

    if json {
        print_json(&inj)?;
    } else {
        eprintln!("注入しました。反映には再デプロイ(または `tbm service start`)が必要です。");
        println!(
            "{} ← {} ({})",
            inj.env_var, inj.resource_name, inj.resource_kind
        );
    }
    Ok(())
}

/// `tbm eject <injection-id>`。注入を外す(injection の id は `tbm service status` で確認)。
pub async fn run_eject(
    id: String,
    server: Option<String>,
    token: Option<String>,
    out: OutputFormat,
) -> Result<()> {
    let cfg = config::load()?;
    let server_url = resolve_server_from(server.as_deref(), cfg.as_ref());
    let token = resolve_token_from(token, cfg)?;
    let json = out.is_json();
    let c = reqwest::Client::new();

    api::inject_delete(&c, &server_url, &token, &id).await?;
    if json {
        print_json(&json!({ "status": "ejected" }))?;
    } else {
        println!("注入を外しました。反映には再デプロイが必要です。");
    }
    Ok(())
}

/// リソース表示名 → id(database + volume + cache + service を横断検索)。複数種別ヒットは曖昧エラー。
async fn resolve_resource(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    name: &str,
) -> Result<String> {
    // database / volume / cache / service 一覧は独立なので並行取得する。
    let (dbs, vols, caches, svcs) = tokio::join!(
        api::db_list(c, server_url, token),
        api::volume_list(c, server_url, token),
        api::cache_list(c, server_url, token),
        api::service_list(c, server_url, token),
    );
    let (dbs, vols, caches, svcs) = (dbs?, vols?, caches?, svcs?);
    let mut hits: Vec<String> = Vec::new();
    for d in &dbs {
        if d.display_name == name {
            hits.push(d.id.to_string());
        }
    }
    for v in &vols {
        if v.display_name == name {
            hits.push(v.id.to_string());
        }
    }
    for ca in &caches {
        if ca.display_name == name {
            hits.push(ca.id.to_string());
        }
    }
    for s in &svcs {
        if s.display_name == name {
            hits.push(s.id.to_string());
        }
    }
    match hits.len() {
        1 => Ok(hits.remove(0)),
        0 => Err(api::ApiError {
            code: "not_found",
            message: format!(
                "リソース '{name}' が見つかりません(database / volume / cache / service)"
            ),
        }
        .into()),
        _ => bail!(
            "'{name}' が複数の種別(database / volume / cache / service)に存在します。一方を改名してから注入してください"
        ),
    }
}
