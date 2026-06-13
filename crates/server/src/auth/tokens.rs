use crate::auth::AuthCtx;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tsubomi_shared::{CLI_TOKEN_PREFIX as TOKEN_PREFIX, random_b64, sha256_hex};
use uuid::Uuid;

const MAX_NAME_LEN: usize = 100;

/// 平文の形式:`tbm_` + base64-url-safe-no-pad(乱数 32 bytes) ≈ 47 文字。
/// プレフィックスは GitHub 流のスキャンマーカー(`ghp_` の類例)で、
/// リーク検出器がログやソース中の tsubomi トークンを拾えるようにする。
pub fn generate_plaintext() -> String {
    format!("{}{}", TOKEN_PREFIX, random_b64(32))
}

/// `Bearer tbm_xxx` をパースする。RFC 7235 §2.1 によりスキーム名は
/// 大文字小文字を区別しないが、トークン平文(`tbm_…`)は区別する。
pub fn parse_bearer(header: &str) -> Option<&str> {
    let (scheme, rest) = header.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("Bearer") {
        return None;
    }
    let rest = rest.trim_start();
    if rest.starts_with(TOKEN_PREFIX) && rest.len() > TOKEN_PREFIX.len() {
        Some(rest)
    } else {
        None
    }
}

pub struct TokenAuth {
    pub token_id: Uuid,
    pub user_id: Uuid,
    pub role: String,
}

pub async fn validate_token(db: &PgPool, plaintext: &str) -> AppResult<TokenAuth> {
    let hash = sha256_hex(plaintext);
    let row: Option<(Uuid, Uuid, String)> = sqlx::query_as(
        "SELECT t.id, t.user_id, u.role::text
           FROM cli_tokens t
           JOIN users u ON u.id = t.user_id
          WHERE t.token_hash = $1
            AND t.revoked_at IS NULL
            AND (t.expires_at IS NULL OR t.expires_at > now())",
    )
    .bind(&hash)
    .fetch_optional(db)
    .await?;

    let (token_id, user_id, role) = row.ok_or(AppError::Unauthorized)?;
    Ok(TokenAuth {
        token_id,
        user_id,
        role,
    })
}

/// `last_used_at` のベストエフォート更新。エラーはログに残すだけで
/// 伝播させない:リクエストの認証自体は既に成功している。
pub async fn touch_last_used(db: &PgPool, token_id: Uuid) {
    let r = sqlx::query("UPDATE cli_tokens SET last_used_at = now() WHERE id = $1")
        .bind(token_id)
        .execute(db)
        .await;
    if let Err(e) = r {
        tracing::warn!(error = ?e, %token_id, "touch_last_used failed");
    }
}

#[derive(Deserialize)]
pub struct CreateTokenReq {
    pub name: String,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Serialize)]
pub struct CreateTokenResp {
    pub id: Uuid,
    pub name: String,
    /// 平文。返すのは**この一度きり**。DB には `token_hash` しか残らない。
    pub token: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct TokenDto {
    pub id: Uuid,
    pub name: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

pub(super) fn validate_name(raw: &str) -> AppResult<String> {
    crate::validate::name(raw, MAX_NAME_LEN)
}

/// 個人 CLI トークンを発行する。Web の `POST /api/tokens` と OAuth の
/// `token` エンドポイントの両方がここを通るので、行の形とハッシュ形式が
/// 常に一致する。
pub async fn insert_personal_token(
    db: &PgPool,
    user_id: Uuid,
    name: &str,
    expires_at: Option<DateTime<Utc>>,
) -> AppResult<(Uuid, String, DateTime<Utc>)> {
    let plaintext = generate_plaintext();
    let hash = sha256_hex(&plaintext);
    let row: (Uuid, DateTime<Utc>) = sqlx::query_as(
        "INSERT INTO cli_tokens (user_id, name, token_hash, expires_at)
              VALUES ($1, $2, $3, $4)
         RETURNING id, created_at",
    )
    .bind(user_id)
    .bind(name)
    .bind(&hash)
    .bind(expires_at)
    .fetch_one(db)
    .await?;
    Ok((row.0, plaintext, row.1))
}

pub async fn create(
    auth: AuthCtx,
    State(state): State<AppState>,
    Json(req): Json<CreateTokenReq>,
) -> AppResult<Json<CreateTokenResp>> {
    let name = validate_name(&req.name)?;

    // `<= now()` にする理由:認証側の条件が `expires_at > now()` なので、
    // 現在時刻ちょうどで作るとトークンが即死する。
    if let Some(exp) = req.expires_at
        && exp <= Utc::now()
    {
        return Err(AppError::BadRequest("有効期限が過去または現在です".into()));
    }

    let (id, plaintext, created_at) =
        insert_personal_token(&state.db, auth.user_id, &name, req.expires_at).await?;
    Ok(Json(CreateTokenResp {
        id,
        name,
        token: plaintext,
        expires_at: req.expires_at,
        created_at,
    }))
}

pub async fn list(auth: AuthCtx, State(state): State<AppState>) -> AppResult<Json<Vec<TokenDto>>> {
    let rows: Vec<TokenDto> = sqlx::query_as(
        "SELECT id, name, expires_at, last_used_at, revoked_at, created_at
           FROM cli_tokens
          WHERE user_id = $1
          ORDER BY created_at DESC",
    )
    .bind(auth.user_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(rows))
}

pub async fn revoke(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<StatusCode> {
    // 存在しない / 他ユーザの / 失効済み、の 3 つはすべて 0 行 → 404 に
    // 収束させる。冪等な revoke と一貫したエラーコード。
    let row: Option<(Uuid,)> = sqlx::query_as(
        "UPDATE cli_tokens
            SET revoked_at = now(), updated_at = now()
          WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL
         RETURNING id",
    )
    .bind(id)
    .bind(auth.user_id)
    .fetch_optional(&state.db)
    .await?;

    row.ok_or(AppError::NotFound)?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bearer_accepts_tbm_prefix() {
        assert_eq!(parse_bearer("Bearer tbm_xyz"), Some("tbm_xyz"));
        assert_eq!(parse_bearer("bearer tbm_xyz"), Some("tbm_xyz"));
        assert_eq!(parse_bearer("BEARER tbm_xyz"), Some("tbm_xyz"));
    }

    #[test]
    fn parse_bearer_rejects_other_schemes_and_missing_prefix() {
        assert!(parse_bearer("Basic dXNlcjpwYXNz").is_none());
        assert!(parse_bearer("").is_none());
        assert!(parse_bearer("Bearer ").is_none());
        assert!(parse_bearer("Bearer amb_xyz").is_none());
        assert!(parse_bearer("Bearer tbm_").is_none());
    }

    #[test]
    fn generate_plaintext_shape() {
        let a = generate_plaintext();
        let b = generate_plaintext();
        assert!(a.starts_with(TOKEN_PREFIX));
        // 32 bytes → ceil(32 * 4/3) = 43 文字、パディング無し。
        assert_eq!(a.len(), TOKEN_PREFIX.len() + 43);
        assert_ne!(a, b);
    }

    #[test]
    fn validate_name_trims_and_caps() {
        assert_eq!(validate_name("  mac  ").unwrap(), "mac");
        assert!(validate_name("").is_err());
        assert!(validate_name("a\nb").is_err());
        assert!(validate_name(&"a".repeat(MAX_NAME_LEN + 1)).is_err());
    }
}
