use anyhow::{Context, Result, bail};
use clap::Args;
use serde_json::json;
use std::io::Write;
use std::process::{Command, Stdio};

use crate::api;
use crate::commands::{OutputFormat, print_json, resolve_server_from, resolve_token_from};
use crate::config;
use tsubomi_shared::{DeployConfig, hmac_sha256, random_b64};

/// `tbm deploy`(GitHub 非依存の退路)。ローカルで build + push して自分で hook を叩く。
/// 平台は build しない(決定 #3)— build はここ(ユーザ機の docker)。CI が無い / 緊急時に使う。
#[derive(Args)]
pub struct DeployArgs {
    /// ローカルで build + push して hook を叩く(現状この経路のみ。`--local` 未指定はエラー — GitHub 経路は git push)
    #[arg(long)]
    pub local: bool,
    /// 対象サービスの表示名(省略時、サービスが 1 つだけならそれを使う)
    #[arg(long)]
    pub service: Option<String>,
    /// build コンテキスト(Dockerfile のあるディレクトリ)
    #[arg(long, default_value = ".")]
    pub context: String,
}

pub async fn run(
    args: DeployArgs,
    server: Option<String>,
    token: Option<String>,
    out: OutputFormat,
) -> Result<()> {
    if !args.local {
        bail!(
            "現状 `tbm deploy` は --local のみ対応です。GitHub 連携なら git push で自動デプロイされます(`tbm service create` の手順参照)"
        );
    }
    let cfg = config::load()?;
    let server_url = resolve_server_from(server.as_deref(), cfg.as_ref());
    let token = resolve_token_from(token, cfg)?;
    let json = out.is_json();
    let c = reqwest::Client::new();

    // 1. 対象 service を決める → build+hook に要る全値を取得(deploy_key / registry / hook_url / platforms)。
    let id = resolve_service(&c, &server_url, &token, args.service.as_deref()).await?;
    let dc = api::deploy_config(&c, &server_url, &token, &id).await?;

    // 2. build + push(平台は build しない。ここはユーザ機の docker buildx)。
    let git_sha = tag();
    let image_tag = format!("{}/{}:{}", dc.registry.host, dc.service_id, git_sha);
    docker_login_if_needed(&dc)?;
    let digest = buildx_push(&args.context, &dc.platforms, &image_tag)?;

    // 3. hook を署名して叩く(workflow テンプレと同じ body / 署名。生バイトに HMAC)。
    let body = serde_json::to_vec(&json!({
        "service_id": dc.service_id,
        "git_sha": git_sha,
        "image_digest": digest,
        "ts": now_unix(),
        "nonce": random_b64(16),
    }))
    .context("hook body の組み立てに失敗")?;
    let sig = hex::encode(hmac_sha256(dc.deploy_key.as_bytes(), &body));
    api::post_deploy_hook(&c, &dc.hook_url, &sig, body).await?;

    if json {
        print_json(&json!({
            "service_id": dc.service_id,
            "image_digest": digest,
            "git_sha": git_sha,
            "status": "accepted",
        }))?;
    } else {
        eprintln!(
            "デプロイを送信しました。`tbm service status` で deploying→running を確認してください。"
        );
        println!("{digest}");
    }
    Ok(())
}

/// --service 名で解決、省略時はサービスが 1 つだけならそれ(複数 / 0 はエラー)。
async fn resolve_service(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    name: Option<&str>,
) -> Result<String> {
    let svcs = api::service_list(c, server_url, token).await?;
    match name {
        Some(n) => match svcs.iter().find(|s| s.display_name == n) {
            Some(s) => Ok(s.id.to_string()),
            None => Err(api::ApiError {
                code: "not_found",
                message: format!("サービス '{n}' が見つかりません(`tbm service list` で確認)"),
            }
            .into()),
        },
        None => match svcs.as_slice() {
            [only] => Ok(only.id.to_string()),
            [] => {
                bail!("サービスがありません。先に `tbm service create <名前>` を実行してください")
            }
            _ => bail!("サービスが複数あります。`--service <名前>` で対象を指定してください"),
        },
    }
}

/// 認証付き registry(本番)なら docker login する。loopback の dev registry(認証なし)は不要。
fn docker_login_if_needed(dc: &DeployConfig) -> Result<()> {
    let host = &dc.registry.host;
    if host.starts_with("127.0.0.1") || host.starts_with("localhost") {
        return Ok(());
    }
    eprintln!("$ docker login {host} -u {}", dc.registry.user);
    let mut child = Command::new("docker")
        .args(["login", host, "-u", &dc.registry.user, "--password-stdin"])
        .stdin(Stdio::piped())
        .spawn()
        .context("docker login の起動に失敗しました(docker はありますか?)")?;
    child
        .stdin
        .take()
        .context("docker の stdin を開けません")?
        .write_all(dc.registry.pass.as_bytes())
        .context("docker login へのパスワード書き込みに失敗")?;
    if !child.wait()?.success() {
        bail!("docker login が失敗しました");
    }
    Ok(())
}

/// `docker buildx build --push` して image digest を返す(metadata-file から取得)。
fn buildx_push(context: &str, platforms: &str, image_tag: &str) -> Result<String> {
    // 一意なメタデータファイル(PID + 乱数。PID 再利用での古いファイル誤読を避ける)。
    let meta = std::env::temp_dir().join(format!(
        "tbm-meta-{}-{}.json",
        std::process::id(),
        random_b64(8)
    ));
    eprintln!("$ docker buildx build --platform {platforms} --push -t {image_tag} {context}");
    let status = Command::new("docker")
        .args([
            "buildx",
            "build",
            "--platform",
            platforms,
            "--push",
            "-t",
            image_tag,
            "--metadata-file",
        ])
        .arg(&meta)
        .arg(context)
        .status()
        .context("docker buildx の実行に失敗しました(docker / buildx はありますか?)")?;
    if !status.success() {
        let _ = std::fs::remove_file(&meta);
        bail!("docker buildx build が失敗しました");
    }
    let text = std::fs::read_to_string(&meta).context("buildx メタデータを読めません")?;
    let _ = std::fs::remove_file(&meta);
    let v: serde_json::Value =
        serde_json::from_str(&text).context("buildx メタデータが不正な JSON です")?;
    let digest = v
        .get("containerimage.digest")
        .and_then(|d| d.as_str())
        .context("buildx メタデータに containerimage.digest がありません")?;
    // 署名・送信の前に digest 形式を検証(壊れたメタデータで誤った image を deploy しない)。
    if !is_sha256_digest(digest) {
        bail!("buildx が返した digest の形式が不正です: {digest}");
    }
    Ok(digest.to_string())
}

/// `sha256:` + 64 桁 16 進かどうか(server 側の hook 検証と同じ契約)。
fn is_sha256_digest(s: &str) -> bool {
    s.strip_prefix("sha256:")
        .is_some_and(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// git の短い SHA(repo 外なら "local")。deploy 履歴の表示用。
fn tag() -> String {
    Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "local".to_string())
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
