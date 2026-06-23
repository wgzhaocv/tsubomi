//! 共有パスワード viewer(M4 S5、web 専用)。design v2 §7「見るは共有密码」。
//! ログイン済み社内ユーザが共有パスワードを入れると、その session に 8h の閲覧 grant が
//! 立ち、管制面(overview / ranking)を只读で見られる(`require_viewer_web`)。
//!
//! - `login`:任意の session(Bearer は拒否 — viewer は web 専用)。bcrypt::verify。
//! - `set_password` / `status`:owner のみ(`require_owner_web`)。
//!   設定 / リセットすると旧 grant を全失効させる(§7「重置即旧の全失効」)。
//!
//! bcrypt は cost 12 ≈数百 ms の同期 CPU なので、tokio worker を塞がないよう
//! 必ず `spawn_blocking` に逃がす(registry.rs の bcrypt と同じコスト感)。

use crate::admin::require_owner_web;
use crate::auth::cookie::SESSION_COOKIE;
use crate::auth::{AuthCtx, session};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use anyhow::anyhow;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum_extra::extract::CookieJar;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::PgPool;
use tsubomi_shared::{ViewerLoginReq, ViewerPasswordReq, ViewerStatusResp};
use uuid::Uuid;

/// platform_config のキー。値 = `{ "hash": "<bcrypt>", "updated_by": "<uuid>" }`
/// (更新時刻は platform_config.updated_at 列を使う = 二重持ちしない)。
const VIEWER_PASSWORD_KEY: &str = "viewer_password";
/// 閲覧 grant の有効期間(時間)。
const VIEWER_GRANT_HOURS: i32 = 8;
/// 共有パスワードの最小長。bcrypt(cost 12 ≈数百 ms / 試行)と併せてオンライン総当たりを
/// 不経済にするための下限(本格的なレート制限は後相 — doc/paas-m4-design.md S5 の積み残し)。
const MIN_VIEWER_PASSWORD_LEN: usize = 8;

/// bcrypt(cost 12 ≈数百 ms の同期 CPU)を spawn_blocking に逃がし、JoinError と
/// BcryptError を AppError に畳む。verify(login)/ hash(set_password)で共有。
async fn bcrypt_job<T: Send + 'static>(
    job: impl FnOnce() -> Result<T, bcrypt::BcryptError> + Send + 'static,
) -> AppResult<T> {
    tokio::task::spawn_blocking(job)
        .await
        .map_err(|e| AppError::Other(anyhow!("bcrypt タスク失敗: {e}")))?
        .map_err(|e| AppError::Other(anyhow!("bcrypt 失敗: {e}")))
}

/// `POST /api/admin/viewer/login`:共有パスワードを入れて現 session に閲覧 grant を立てる。
pub async fn login(
    auth: AuthCtx,
    State(state): State<AppState>,
    jar: CookieJar,
    Json(req): Json<ViewerLoginReq>,
) -> AppResult<StatusCode> {
    // viewer は web/session 専用(owner ガバナンスと同じ規約)。Bearer cli_token は拒否。
    if !auth.is_session() {
        return Err(AppError::Forbidden);
    }
    // 未設定なら入れようがない。内部ツールなので timing oracle を隠すより親切な誘導を優先。
    let hash = stored_hash(&state.db).await?.ok_or_else(|| {
        AppError::BadRequest("共有パスワードが未設定です。管理者に設定を依頼してください".into())
    })?;

    // 前後空白は設定側でも落とすので、ここでも trim して一致条件を揃える(client の trim は UX)。
    let password = req.password.trim().to_string();
    let ok = bcrypt_job(move || bcrypt::verify(&password, &hash)).await?;
    if !ok {
        return Err(AppError::BadRequest("共有パスワードが違います".into()));
    }

    // 現 session(cookie の生トークン)に grant を立てる。token→token_hash の変換は
    // session モジュールに閉じる(logout と同じく cookie はここで取る)。
    let token = jar
        .get(SESSION_COOKIE)
        .map(|c| c.value().to_string())
        .ok_or(AppError::Unauthorized)?;
    session::grant_viewer(&state.db, &token, VIEWER_GRANT_HOURS).await?;

    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/admin/viewer/password`(owner)。共有パスワードを設定 / リセットし、旧 grant を全失効。
pub async fn set_password(
    auth: AuthCtx,
    State(state): State<AppState>,
    Json(req): Json<ViewerPasswordReq>,
) -> AppResult<Json<ViewerStatusResp>> {
    require_owner_web(&auth)?;
    // 前後空白を落として保存(login 側も trim するので一致条件が揃う = 単一の真実)。
    let pw = req.password.trim().to_string();
    if pw.chars().count() < MIN_VIEWER_PASSWORD_LEN {
        return Err(AppError::BadRequest(format!(
            "共有パスワードは {MIN_VIEWER_PASSWORD_LEN} 文字以上にしてください"
        )));
    }

    let hash = bcrypt_job(move || bcrypt::hash(&pw, bcrypt::DEFAULT_COST)).await?;
    let value = json!({ "hash": hash, "updated_by": auth.user_id.to_string() });

    // upsert + 旧 grant 失効を 1 トランザクションで(設定とリセットは不可分)。
    let mut tx = state.db.begin().await?;
    sqlx::query(
        "INSERT INTO platform_config (key, value, updated_at) VALUES ($1, $2, now())
         ON CONFLICT (key) DO UPDATE SET value = $2, updated_at = now()",
    )
    .bind(VIEWER_PASSWORD_KEY)
    .bind(&value)
    .execute(&mut *tx)
    .await?;
    // §7「重置即旧の全失効」。grant 持ちの行だけ touch(role 不問 — owner 自身の
    // プレビュー grant も一緒に切れるが無害。owner は role で見えるので影響なし)。
    sqlx::query("UPDATE sessions SET viewer_until = NULL WHERE viewer_until IS NOT NULL")
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    tracing::info!(owner = %auth.user_id, "共有 viewer パスワードを設定 / リセット(旧 grant 失効)");

    load_status(&state.db).await.map(Json)
}

/// `GET /api/admin/viewer/password`(owner)。設定済みか + メタ(本体・hash は返さない)。
pub async fn status(
    auth: AuthCtx,
    State(state): State<AppState>,
) -> AppResult<Json<ViewerStatusResp>> {
    require_owner_web(&auth)?;
    load_status(&state.db).await.map(Json)
}

/// platform_config から共有パスワードの hash だけ取り出す(未設定なら None)。
async fn stored_hash(db: &PgPool) -> AppResult<Option<String>> {
    let v: Option<Value> = sqlx::query_scalar("SELECT value FROM platform_config WHERE key = $1")
        .bind(VIEWER_PASSWORD_KEY)
        .fetch_optional(db)
        .await?;
    Ok(v.and_then(|v| v.get("hash").and_then(Value::as_str).map(str::to_string)))
}

/// 設定状態 + 最終更新メタ(updated_at は platform_config 列、設定者名は users から引く)。
/// `updated_by` の UUID 化は SQL の cast ではなく Rust 側で行う — 畸形値(手書き / 旧データ)で
/// JOIN の `::uuid` cast が走って status 全体が 500 になるのを避ける(正しい値のときだけ名前を引く)。
async fn load_status(db: &PgPool) -> AppResult<ViewerStatusResp> {
    let row: Option<(Option<String>, DateTime<Utc>)> = sqlx::query_as(
        "SELECT pc.value->>'updated_by', pc.updated_at FROM platform_config pc WHERE pc.key = $1",
    )
    .bind(VIEWER_PASSWORD_KEY)
    .fetch_optional(db)
    .await?;
    let Some((updated_by, updated_at)) = row else {
        return Ok(ViewerStatusResp {
            set: false,
            updated_at: None,
            updated_by_name: None,
        });
    };
    // updated_by が正しい UUID のときだけ users を引く(真名 → 無ければ email)。
    let updated_by_name = match updated_by.as_deref().and_then(|s| Uuid::parse_str(s).ok()) {
        Some(uid) => sqlx::query_as::<_, (Option<String>, Option<String>)>(
            "SELECT name, email FROM users WHERE id = $1",
        )
        .bind(uid)
        .fetch_optional(db)
        .await?
        .and_then(|(name, email)| name.or(email)),
        None => None,
    };
    Ok(ViewerStatusResp {
        set: true,
        updated_at: Some(updated_at),
        updated_by_name,
    })
}
