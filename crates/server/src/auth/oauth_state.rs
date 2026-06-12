//! Google ログインの CSRF state(単回使用)。`DELETE .. RETURNING` は
//! amber の Redis `GETDEL` の Postgres 等価:callback が並行しても勝者は
//! ちょうど 1 つ。

use crate::error::AppResult;
use sqlx::PgPool;

pub const TTL_SECS: i64 = 600;

pub async fn store(db: &PgPool, csrf: &str) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO oauth_states (state, expires_at)
         VALUES ($1, now() + make_interval(secs => $2))",
    )
    .bind(csrf)
    .bind(TTL_SECS as f64)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn consume(db: &PgPool, state: &str) -> AppResult<bool> {
    let row: Option<(String,)> = sqlx::query_as(
        "DELETE FROM oauth_states WHERE state = $1 AND expires_at > now() RETURNING state",
    )
    .bind(state)
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}
