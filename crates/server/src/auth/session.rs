//! Web セッションを Postgres に置く(amber は Redis だったが、tsubomi に
//! valkey が入るのは M5)。cookie には生トークン、DB には sha256 hex を
//! 保存する — 管制面 DB のダンプが漏れても有効なセッション cookie には
//! ならない。

use crate::error::AppResult;
use sqlx::PgPool;
use tsubomi_shared::{random_b64, sha256_hex};
use uuid::Uuid;

pub const SESSION_TTL_SECS: i64 = 30 * 24 * 3600; // 30日

/// セッション行を INSERT して生トークン(cookie 用)を返す。
pub async fn create(db: &PgPool, user_id: Uuid) -> AppResult<String> {
    let token = random_b64(32);
    sqlx::query(
        "INSERT INTO sessions (user_id, token_hash, expires_at)
         VALUES ($1, $2, now() + make_interval(secs => $3))",
    )
    .bind(user_id)
    .bind(sha256_hex(&token))
    .bind(SESSION_TTL_SECS as f64)
    .execute(db)
    .await?;
    Ok(token)
}

/// 生セッショントークンを (user_id, role) に解決する。期限切れ・未知 → None。
pub async fn get(db: &PgPool, token: &str) -> AppResult<Option<(Uuid, String)>> {
    let row: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT s.user_id, u.role::text
           FROM sessions s
           JOIN users u ON u.id = s.user_id
          WHERE s.token_hash = $1 AND s.expires_at > now()",
    )
    .bind(sha256_hex(token))
    .fetch_optional(db)
    .await?;
    Ok(row)
}

pub async fn delete(db: &PgPool, token: &str) -> AppResult<()> {
    sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
        .bind(sha256_hex(token))
        .execute(db)
        .await?;
    Ok(())
}
