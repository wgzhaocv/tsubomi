use anyhow::{Context, Result, bail};
use tsubomi_shared::{Health, Me};

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
