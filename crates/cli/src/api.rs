use anyhow::{Context, Result};
use tsubomi_shared::{ConnectionUrlResp, CreateDatabaseReq, DatabaseDto, Health, Me};

pub const ME_PATH: &str = "/api/auth/me";

/// API 由来のエラー。`code` は安定した機械可読コード(json 出力のエラー信封で使う)。
/// anyhow に載せて伝播し、main で downcast して code を取り出す。
#[derive(Debug)]
pub struct ApiError {
    pub code: &'static str,
    pub message: String,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl std::error::Error for ApiError {}

/// HTTP ステータス → 安定コード。
fn code_for(status: reqwest::StatusCode) -> &'static str {
    match status.as_u16() {
        401 => "unauthorized",
        403 => "forbidden",
        404 => "not_found",
        409 => "conflict",
        400 => "bad_request",
        _ => "server_error",
    }
}

pub async fn fetch_me(server_url: &str, token: &str) -> Result<Me> {
    let resp = reqwest::Client::new()
        .get(format!("{server_url}{ME_PATH}"))
        .bearer_auth(token)
        .send()
        .await
        .context("failed to call /api/auth/me")?;
    let status = resp.status();
    if !status.is_success() {
        let message = if status == reqwest::StatusCode::UNAUTHORIZED {
            "token invalid (run: tbm login)".to_owned()
        } else {
            let body = resp.text().await.unwrap_or_default();
            format!("/api/auth/me failed: HTTP {status} {body}")
        };
        return Err(ApiError {
            code: code_for(status),
            message,
        }
        .into());
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
    let message = if status == reqwest::StatusCode::UNAUTHORIZED {
        "認証に失敗しました(tbm login を実行してください)".to_owned()
    } else {
        let body = resp.text().await.unwrap_or_default();
        if body.is_empty() {
            format!("HTTP {status}")
        } else {
            body
        }
    };
    Err(ApiError {
        code: code_for(status),
        message,
    }
    .into())
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
