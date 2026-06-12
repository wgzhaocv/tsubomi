use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tsubomi_shared::{
    AUTHCODE_PREFIX, OAUTH_AUTHORIZE_PATH, OAUTH_CALLBACK_PATH, OAUTH_CLIENT_ID, OAUTH_TOKEN_PATH,
    random_b64,
};
use url::Url;

pub fn generate_verifier() -> String {
    random_b64(32)
}

pub fn generate_state() -> String {
    random_b64(16)
}

/// manual フロー用:サーバのコード表示ページ。
pub fn manual_redirect_uri(server_url: &str) -> String {
    format!("{server_url}{OAUTH_CALLBACK_PATH}")
}

/// loopback フロー用(RFC 8252):CLI が立てたローカルリスナー。
pub fn loopback_redirect_uri(port: u16) -> String {
    format!("http://127.0.0.1:{port}/callback")
}

pub fn build_authorize_url(
    server_url: &str,
    redirect_uri: &str,
    challenge: &str,
    state: &str,
    hint: &str,
) -> Result<String> {
    let mut url = Url::parse(&format!("{server_url}{OAUTH_AUTHORIZE_PATH}"))
        .with_context(|| format!("invalid server_url: {server_url}"))?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", OAUTH_CLIENT_ID)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", state)
        .append_pair("hint", hint);
    Ok(url.to_string())
}

#[derive(Serialize)]
struct TokenReq<'a> {
    grant_type: &'a str,
    code: &'a str,
    code_verifier: &'a str,
    state: &'a str,
    client_id: &'a str,
    redirect_uri: &'a str,
}

#[derive(Deserialize)]
struct TokenResp {
    access_token: String,
}

pub async fn exchange_code(
    server_url: &str,
    redirect_uri: &str,
    code: &str,
    verifier: &str,
    state: &str,
) -> Result<String> {
    if !code.starts_with(AUTHCODE_PREFIX) {
        bail!("invalid code (must start with '{AUTHCODE_PREFIX}')");
    }
    let resp = reqwest::Client::new()
        .post(format!("{server_url}{OAUTH_TOKEN_PATH}"))
        .json(&TokenReq {
            grant_type: "authorization_code",
            code,
            code_verifier: verifier,
            state,
            client_id: OAUTH_CLIENT_ID,
            redirect_uri,
        })
        .send()
        .await
        .context("failed to send token request")?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        bail!("token exchange failed: HTTP {status} {body}");
    }
    let parsed: TokenResp = resp
        .json()
        .await
        .context("failed to parse token response")?;
    Ok(parsed.access_token)
}

// PKCE 自体のテスト(RFC 7636 ベクタ)は tsubomi-shared::tests に一本化。
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_url_has_all_oauth_params() {
        let url = build_authorize_url(
            "https://x.example.com",
            "http://127.0.0.1:49152/callback",
            "challenge",
            "statexxxxx",
            "host",
        )
        .unwrap();
        assert!(url.contains("response_type=code"));
        assert!(url.contains("client_id=tbm-cli"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A49152%2Fcallback"));
    }
}
