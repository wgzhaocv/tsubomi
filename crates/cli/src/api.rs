use anyhow::{Context, Result, bail};
use tsubomi_shared::{ConnectionUrlResp, CreateDatabaseReq, DatabaseDto, Health, Me};

pub const ME_PATH: &str = "/api/auth/me";

pub async fn fetch_me(server_url: &str, token: &str) -> Result<Me> {
    let resp = reqwest::Client::new()
        .get(format!("{server_url}{ME_PATH}"))
        .bearer_auth(token)
        .send()
        .await
        .context("failed to call /api/auth/me")?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        bail!("token invalid (run: tbm login)");
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        bail!("/api/auth/me failed: HTTP {status} {body}");
    }
    resp.json()
        .await
        .context("failed to parse /api/auth/me response")
}

pub async fn fetch_health(server_url: &str) -> Result<Health> {
    let resp = reqwest::Client::new()
        .get(format!("{server_url}/api/health"))
        .send()
        .await
        .context("failed to call /api/health")?
        .error_for_status()
        .context("/api/health returned an error")?;
    resp.json().await.context("failed to parse health response")
}

// ============ M1 database ============
// db_* は呼び出し側(commands/db.rs)が作る 1 つの reqwest::Client を共有する
// (コマンド毎の TLS 初期化を 1 回に減らす)。send_ok が送信 + ステータス検査の
// 定型を 1 箇所に集約する。

/// 送信 → 非成功なら CLI 向けエラー(サーバの日本語本文を保つ)に変換。
async fn send_ok(rb: reqwest::RequestBuilder) -> Result<reqwest::Response> {
    let resp = rb
        .send()
        .await
        .context("request to tsubomi server failed")?;
    if resp.status().is_success() {
        return Ok(resp);
    }
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        bail!("認証に失敗しました(tbm login を実行してください)");
    }
    let body = resp.text().await.unwrap_or_default();
    bail!("HTTP {status}: {body}")
}

pub async fn db_list(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
) -> Result<Vec<DatabaseDto>> {
    let resp = send_ok(
        c.get(format!("{server_url}/api/databases"))
            .bearer_auth(token),
    )
    .await?;
    resp.json()
        .await
        .context("failed to parse databases response")
}

pub async fn db_create(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    name: &str,
) -> Result<DatabaseDto> {
    let resp = send_ok(
        c.post(format!("{server_url}/api/databases"))
            .bearer_auth(token)
            .json(&CreateDatabaseReq {
                name: name.to_owned(),
            }),
    )
    .await?;
    resp.json().await.context("failed to parse create response")
}

pub async fn db_url(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    id: &str,
) -> Result<String> {
    let resp = send_ok(
        c.get(format!("{server_url}/api/databases/{id}/url"))
            .bearer_auth(token),
    )
    .await?;
    let r: ConnectionUrlResp = resp.json().await.context("failed to parse url response")?;
    Ok(r.url)
}

pub async fn db_rotate(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    id: &str,
) -> Result<String> {
    let resp = send_ok(
        c.post(format!("{server_url}/api/databases/{id}/rotate"))
            .bearer_auth(token),
    )
    .await?;
    let r: ConnectionUrlResp = resp
        .json()
        .await
        .context("failed to parse rotate response")?;
    Ok(r.url)
}

pub async fn db_delete(c: &reqwest::Client, server_url: &str, token: &str, id: &str) -> Result<()> {
    send_ok(
        c.delete(format!("{server_url}/api/databases/{id}"))
            .bearer_auth(token),
    )
    .await?;
    Ok(())
}
