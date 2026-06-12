//! tbm CLI ログイン用の OAuth Authorization Code Grant + PKCE。amber からの
//! 移植で、保留中コードは Redis ではなく Postgres に置く。
//!
//! フロー(RFC 6749 §4.1 + RFC 7636):
//! 1. CLI が `code_verifier`(乱数)と `code_challenge =
//!    base64url(sha256(verifier))` を生成し、ブラウザで `/oauth/authorize?...`
//!    を開く。
//! 2. ブラウザが `POST /api/oauth/authorize`(session 認証)を叩く。サーバは
//!    challenge / state / user_id を保留行として保存し、`code` と `state` を
//!    含む `redirect_to` URL を返す。
//! 3. ブラウザがそこへ遷移する。遷移先は 2 通り(RFC 8252):
//!    - **loopback(デフォルト)**:`http://127.0.0.1:<port>/callback` —
//!      CLI が立てた一回限りのローカルリスナーが code を直接受け取る。
//!      コピペ不要。
//!    - **manual(`tbm login --manual`)**:`<server>/oauth/code/callback` —
//!      ページがコードを表示し、ユーザが CLI に貼り戻す(SSH 先など
//!      ブラウザと CLI が別マシンの場合用)。
//! 4. CLI が `code` / `code_verifier` / `state` を `/api/oauth/token`(公開)に
//!    POST。サーバは `sha256(verifier) == challenge` を検証し、cli_tokens 行を
//!    発行して平文を `access_token` として返す。
//!
//! loopback でも安全性は落ちない:verifier は CLI プロセスから出ないので、
//! 同一マシンの他プロセスが code を横取りしても token には交換できない。

use crate::auth::tokens;
use crate::auth::{AuthCtx, AuthSource};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tsubomi_shared::{
    AUTHCODE_PREFIX, OAUTH_CALLBACK_PATH, OAUTH_CLIENT_ID, pkce_challenge, random_b64,
};
use uuid::Uuid;

/// 保留中 authcode の寿命。oauth_state::TTL_SECS(Google ログインの CSRF)と
/// たまたま同じ 10 分だが、別プロトコルの独立した値 — 片方だけ変えてよい。
const TTL_SECS: i64 = 600;
const STATE_MIN_LEN: usize = 10;
const STATE_MAX_LEN: usize = 256;
const CHALLENGE_MIN_LEN: usize = 43; // base64url(32 bytes) = 43 文字
const CHALLENGE_MAX_LEN: usize = 128;

pub fn generate_authcode() -> String {
    format!("{}{}", AUTHCODE_PREFIX, random_b64(32))
}

struct OauthPending {
    user_id: Uuid,
    code_challenge: String,
    state: String,
    hint: Option<String>,
}

async fn store_pending(db: &PgPool, code: &str, p: &OauthPending) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO authcodes (code, user_id, code_challenge, state, hint, expires_at)
         VALUES ($1, $2, $3, $4, $5, now() + make_interval(secs => $6))",
    )
    .bind(code)
    .bind(p.user_id)
    .bind(&p.code_challenge)
    .bind(&p.state)
    .bind(&p.hint)
    .bind(TTL_SECS as f64)
    .execute(db)
    .await?;
    Ok(())
}

/// 原子的な単回消費:`DELETE .. RETURNING`(Postgres 版 GETDEL)。
/// 同じコードの 2 回目の交換は、どちらがレースに勝っても `None`。
async fn consume_pending(db: &PgPool, code: &str) -> AppResult<Option<OauthPending>> {
    let row: Option<(Uuid, String, String, Option<String>)> = sqlx::query_as(
        "DELETE FROM authcodes WHERE code = $1 AND expires_at > now()
         RETURNING user_id, code_challenge, state, hint",
    )
    .bind(code)
    .fetch_optional(db)
    .await?;
    Ok(
        row.map(|(user_id, code_challenge, state, hint)| OauthPending {
            user_id,
            code_challenge,
            state,
            hint,
        }),
    )
}

#[derive(Deserialize)]
pub struct AuthorizeReq {
    pub response_type: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub code_challenge_method: String,
    pub state: String,
    #[serde(default)]
    pub hint: Option<String>,
}

#[derive(Serialize)]
pub struct AuthorizeResp {
    /// ブラウザの遷移先:検証済み redirect_uri + code + エコーした state。
    pub redirect_to: String,
}

pub async fn authorize(
    auth: AuthCtx,
    State(state): State<AppState>,
    Json(req): Json<AuthorizeReq>,
) -> AppResult<Json<AuthorizeResp>> {
    // OAuth 認可は本物のブラウザセッションからしか始められない — Bearer
    // トークンで新しい authcode を承認できると、漏れたトークンが恒久的な
    // 後継を発行でき、失効が無意味になる。
    if !matches!(auth.source, AuthSource::Session) {
        return Err(AppError::Forbidden);
    }
    if req.response_type != "code" {
        return Err(AppError::BadRequest("unsupported_response_type".into()));
    }
    if req.client_id != OAUTH_CLIENT_ID {
        return Err(AppError::BadRequest("invalid_request: client_id".into()));
    }
    if !redirect_uri_allowed(&state.config.server_url, &req.redirect_uri) {
        return Err(AppError::BadRequest("invalid_request: redirect_uri".into()));
    }
    if req.code_challenge_method != "S256" {
        return Err(AppError::BadRequest(
            "invalid_request: code_challenge_method".into(),
        ));
    }
    if !is_url_safe_base64(&req.code_challenge)
        || req.code_challenge.len() < CHALLENGE_MIN_LEN
        || req.code_challenge.len() > CHALLENGE_MAX_LEN
    {
        return Err(AppError::BadRequest(
            "invalid_request: code_challenge".into(),
        ));
    }
    if req.state.len() < STATE_MIN_LEN || req.state.len() > STATE_MAX_LEN {
        return Err(AppError::BadRequest("invalid_request: state".into()));
    }
    let hint = match req.hint.as_deref() {
        Some(h) if !h.is_empty() => Some(tokens::validate_name(h)?),
        _ => None,
    };

    let code = generate_authcode();
    store_pending(
        &state.db,
        &code,
        &OauthPending {
            user_id: auth.user_id,
            code_challenge: req.code_challenge,
            state: req.state.clone(),
            hint,
        },
    )
    .await?;

    let mut url = url::Url::parse(&req.redirect_uri)
        .map_err(|_| AppError::BadRequest("invalid_request: redirect_uri".into()))?;
    url.query_pairs_mut()
        .append_pair("code", &code)
        .append_pair("state", &req.state);
    Ok(Json(AuthorizeResp {
        redirect_to: url.to_string(),
    }))
}

#[derive(Deserialize)]
pub struct TokenReq {
    pub grant_type: String,
    pub code: String,
    pub code_verifier: String,
    pub state: String,
    pub client_id: String,
    pub redirect_uri: String,
}

#[derive(Serialize)]
pub struct TokenResp {
    pub access_token: String,
    pub token_type: &'static str,
    pub scope: Option<String>,
}

pub async fn token(
    State(state): State<AppState>,
    Json(req): Json<TokenReq>,
) -> AppResult<Json<TokenResp>> {
    if req.grant_type != "authorization_code" {
        return Err(AppError::BadRequest("unsupported_grant_type".into()));
    }
    let pending = consume_pending(&state.db, &req.code)
        .await?
        .ok_or(AppError::Unauthorized)?;

    if req.state != pending.state {
        return Err(AppError::Unauthorized);
    }
    if req.client_id != OAUTH_CLIENT_ID {
        return Err(AppError::BadRequest("invalid_request: client_id".into()));
    }
    if !redirect_uri_allowed(&state.config.server_url, &req.redirect_uri) {
        return Err(AppError::BadRequest("invalid_request: redirect_uri".into()));
    }
    if pkce_challenge(&req.code_verifier) != pending.code_challenge {
        return Err(AppError::Unauthorized);
    }

    let token_name = pending.hint.as_deref().unwrap_or("cli");
    let (_id, plaintext, _created_at) =
        tokens::insert_personal_token(&state.db, pending.user_id, token_name, None).await?;

    Ok(Json(TokenResp {
        access_token: plaintext,
        token_type: "Bearer",
        scope: None,
    }))
}

fn is_url_safe_base64(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// 許可する redirect_uri は 2 形だけ:
/// 1. manual フロー:`<server_url>/oauth/code/callback`(完全一致)
/// 2. loopback フロー:`http://127.0.0.1:<port>/callback` 等(RFC 8252 §7.3 —
///    ループバックはポート可変を許容しなければならない)
fn redirect_uri_allowed(server_url: &str, uri: &str) -> bool {
    if uri == format!("{server_url}{OAUTH_CALLBACK_PATH}") {
        return true;
    }
    let Ok(u) = url::Url::parse(uri) else {
        return false;
    };
    u.scheme() == "http"
        && matches!(
            u.host_str(),
            Some("127.0.0.1" | "localhost" | "[::1]" | "::1")
        )
        && u.path() == "/callback"
        && u.query().is_none()
        && u.fragment().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    // PKCE の RFC 7636 テストベクタは tsubomi-shared::tests に一本化
    // (実装が共有になったので、ここで重複検証する意味が消えた)。
    #[test]
    fn generate_authcode_shape() {
        let a = generate_authcode();
        let b = generate_authcode();
        assert!(a.starts_with(AUTHCODE_PREFIX));
        assert_eq!(a.len(), AUTHCODE_PREFIX.len() + 43);
        assert_ne!(a, b);
    }

    #[test]
    fn is_url_safe_base64_examples() {
        assert!(is_url_safe_base64("abc-_xyz123"));
        assert!(!is_url_safe_base64("has space"));
        assert!(!is_url_safe_base64("has+plus"));
        assert!(!is_url_safe_base64(""));
    }

    #[test]
    fn redirect_uri_allowed_cases() {
        let s = "https://tsubomi.example.com";
        // manual フロー:完全一致のみ
        assert!(redirect_uri_allowed(
            s,
            "https://tsubomi.example.com/oauth/code/callback"
        ));
        assert!(!redirect_uri_allowed(
            s,
            "https://evil.example.com/oauth/code/callback"
        ));
        // loopback フロー:ポート可変、パスは /callback 固定
        assert!(redirect_uri_allowed(s, "http://127.0.0.1:49152/callback"));
        assert!(redirect_uri_allowed(s, "http://localhost:8000/callback"));
        // 拒否:https 以外のホスト、別パス、query 付き
        assert!(!redirect_uri_allowed(
            s,
            "http://192.168.0.5:49152/callback"
        ));
        assert!(!redirect_uri_allowed(s, "http://127.0.0.1:49152/other"));
        assert!(!redirect_uri_allowed(
            s,
            "http://127.0.0.1:49152/callback?x=1"
        ));
        // loopback は http のみ(https の 127.0.0.1 は証明書が成立しない)
        assert!(!redirect_uri_allowed(s, "https://127.0.0.1:49152/callback"));
    }
}
