//! Google ログイン(Authorization Code Grant)。
//!
//! oauth2 crate は使わず手書き:認可 URL の組み立てと code→token 交換は
//! GET リダイレクト 1 本と POST フォーム 1 本だけで、crate を入れると
//! reqwest のバージョンが 0.12 に縛られるため(oauth2 5.0 時点)。

use crate::auth::cookie::{self, OAUTH_STATE_COOKIE, SESSION_COOKIE};
use crate::auth::{AuthCtx, oauth_state, session};
use crate::error::{AppError, AppResult};
use crate::owners;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::CookieJar;
use serde::Deserialize;
use sqlx::PgPool;
use tsubomi_shared::{AuthInfo, Me, random_b64};
use uuid::Uuid;

const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

/// email のドメインが許可リスト(`allowed_hds`)に入っているか。login の email ドメイン判定と
/// owner 追加時の判定で共用する(login は hd claim も併せて見るが、こちらはドメインだけ)。
/// 呼び出し側が lowercase 済みであることを前提にする。
pub(crate) fn email_domain_allowed(email: &str, allowed: &[String]) -> bool {
    let domain = email.rsplit_once('@').map(|(_, d)| d).unwrap_or("");
    allowed.iter().any(|a| a == domain)
}
const USERINFO_URL: &str = "https://www.googleapis.com/oauth2/v3/userinfo";

#[derive(Deserialize)]
pub struct CallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
}

#[derive(Deserialize, Debug)]
struct UserInfo {
    sub: String,
    email: Option<String>,
    name: Option<String>,
    picture: Option<String>,
    /// Google Workspace の hosted domain。個人アカウントには `hd` が無く、
    /// 下のドメイン制限で弾かれる。
    hd: Option<String>,
}

/// 未ログインでも読める公開情報。ログイン画面が許可ドメインを表示するため。
pub async fn info(State(state): State<AppState>) -> Json<AuthInfo> {
    Json(AuthInfo {
        allowed_domains: state.config.allowed_hds.clone(),
        db_public_enabled: state.config.db_public_enabled,
    })
}

pub async fn start(State(state): State<AppState>) -> AppResult<Response> {
    let csrf = random_b64(32);

    let mut auth_url = url::Url::parse(AUTH_URL).expect("static URL");
    auth_url
        .query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &state.config.google_client_id)
        .append_pair("redirect_uri", &state.config.google_redirect_uri)
        .append_pair("scope", "openid email profile")
        .append_pair("state", &csrf);

    oauth_state::store(&state.db, &csrf).await?;

    let cookie = cookie::build_oauth_state(csrf, state.config.cookie_secure);
    let jar = CookieJar::new().add(cookie);

    Ok((jar, Redirect::to(auth_url.as_str())).into_response())
}

pub async fn callback(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(q): Query<CallbackQuery>,
) -> AppResult<Response> {
    if let Some(err) = q.error {
        return Err(AppError::BadRequest(format!("Google OAuth エラー: {err}")));
    }
    let code = q
        .code
        .ok_or_else(|| AppError::BadRequest("code がありません".into()))?;
    let state_param = q
        .state
        .ok_or_else(|| AppError::BadRequest("state がありません".into()))?;

    // ブラウザ束縛:query の state は /start で置いた state cookie と一致する
    // こと。DB 側の単回消費が第二層(消費後のリプレイを捕まえる)。
    let cookie_state = jar
        .get(OAUTH_STATE_COOKIE)
        .map(|c| c.value().to_string())
        .ok_or_else(|| AppError::BadRequest("state cookie がありません".into()))?;
    if cookie_state != state_param {
        return Err(AppError::BadRequest("state cookie が一致しません".into()));
    }

    if !oauth_state::consume(&state.db, &state_param).await? {
        return Err(AppError::BadRequest(
            "state が不正または期限切れです".into(),
        ));
    }

    // code → access_token 交換(RFC 6749 §4.1.3)
    let token: TokenResponse = {
        let resp = state
            .http
            .post(TOKEN_URL)
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code.as_str()),
                ("client_id", state.config.google_client_id.as_str()),
                ("client_secret", state.config.google_client_secret.as_str()),
                ("redirect_uri", state.config.google_redirect_uri.as_str()),
            ])
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            // 4xx は誤設定や不正 code(=BadRequest)、それ以外はこちらの障害扱い
            if status.is_client_error() {
                return Err(AppError::BadRequest(format!("OAuth: {status} {body}")));
            }
            return Err(AppError::Other(anyhow::anyhow!(
                "oauth token exchange: {status} {body}"
            )));
        }
        resp.json().await?
    };

    let info: UserInfo = state
        .http
        .get(USERINFO_URL)
        .bearer_auth(&token.access_token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    // ---- 登録ドメイン制限(design v2 §7):サーバ側で `hd` claim と email
    // ドメインの両方が TSUBOMI_ALLOWED_HD(カンマ区切りリスト)に入っている
    // ことを検証。個人 Gmail は hd が無いので最初の条件で落ちる。
    // hd と email ドメインは独立に判定(同一は強制しない):複数ドメイン構成の
    // Workspace でも全ドメインを列挙すれば通る。フロントは一切信用しない。
    let allowed = &state.config.allowed_hds;
    let email = info
        .email
        .as_deref()
        .ok_or_else(|| AppError::BadRequest("email がありません".into()))?
        .to_lowercase();
    let hd = info.hd.as_deref().map(str::to_lowercase);
    let hd_ok = hd
        .as_deref()
        .is_some_and(|h| allowed.iter().any(|a| a == h));
    let email_ok = email_domain_allowed(&email, allowed);
    if !hd_ok || !email_ok {
        tracing::warn!(sub = %info.sub, hd = ?info.hd, %email, "login rejected: outside allowed domains");
        // ブラウザ遷移の途中なので、素の 403 本文ではなく専用の /forbidden
        // 画面へリダイレクトして「権限なし」を見せる。中途で立った
        // oauth_state cookie も掃除しておく(session は元々張っていない)。
        let jar = CookieJar::new().add(cookie::build_oauth_state_clear(state.config.cookie_secure));
        return Ok((jar, Redirect::to("/forbidden")).into_response());
    }

    let user_id = upsert_google_user(&state.db, &info, &email).await?;

    // owner の補昇:**roster(DB、env から冷启动种)** に email があれば昇格する。
    // env をここで読まないのが要点 — web で外した owner が env に残っていても再ログインで
    // 「神秘復昇」しない(roster が真相)。自動降格はしない — 除名は web の明示操作(owners.rs)。
    if owners::roster(&state.db).await.contains(&email) {
        sqlx::query(
            "UPDATE users SET role = 'owner', updated_at = now() WHERE id = $1 AND role <> 'owner'",
        )
        .bind(user_id)
        .execute(&state.db)
        .await?;
    }

    tracing::info!(user_id = %user_id, sub = %info.sub, "google login");

    let session_token = session::create(&state.db, user_id).await?;
    let secure = state.config.cookie_secure;
    let jar = CookieJar::new()
        .add(cookie::build_session(session_token, secure))
        .add(cookie::build_oauth_state_clear(secure));

    Ok((jar, Redirect::to("/")).into_response())
}

async fn upsert_google_user(pool: &PgPool, info: &UserInfo, email: &str) -> AppResult<Uuid> {
    match upsert_once(pool, info, email).await {
        Err(AppError::Sqlx(sqlx::Error::Database(e))) if e.is_unique_violation() => {
            // レース負け:同じ `sub` の並行 callback が INSERT を先取りした。
            // やり直せば SELECT 側の分岐でその行が見つかる。
            tracing::info!(sub = %info.sub, "upsert race lost, retrying");
            upsert_once(pool, info, email).await
        }
        result => result,
    }
}

async fn upsert_once(pool: &PgPool, info: &UserInfo, email: &str) -> AppResult<Uuid> {
    let mut tx = pool.begin().await?;

    let existing: Option<(Uuid,)> = sqlx::query_as(
        "SELECT user_id FROM credentials WHERE type = 'google' AND external_id = $1",
    )
    .bind(&info.sub)
    .fetch_optional(&mut *tx)
    .await?;

    let user_id = if let Some((uid,)) = existing {
        sqlx::query(
            "UPDATE users
                SET name = COALESCE($2, name),
                    avatar_url = COALESCE($3, avatar_url),
                    last_login_at = now(),
                    updated_at = now()
              WHERE id = $1",
        )
        .bind(uid)
        .bind(&info.name)
        .bind(&info.picture)
        .execute(&mut *tx)
        .await?;
        uid
    } else {
        // Workspace はアカウントの削除→再作成ができる:email は同じで `sub`
        // が変わる。users.email UNIQUE で失敗させず、既存ユーザに新しい
        // credential を紐付ける。
        let by_email: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM users WHERE email = $1")
            .bind(email)
            .fetch_optional(&mut *tx)
            .await?;

        let uid = match by_email {
            Some((uid,)) => {
                sqlx::query(
                    "UPDATE users SET name = COALESCE($2, name), avatar_url = COALESCE($3, avatar_url),
                            last_login_at = now(), updated_at = now() WHERE id = $1",
                )
                .bind(uid)
                .bind(&info.name)
                .bind(&info.picture)
                .execute(&mut *tx)
                .await?;
                uid
            }
            None => {
                let (uid,): (Uuid,) = sqlx::query_as(
                    "INSERT INTO users (email, name, avatar_url, last_login_at)
                     VALUES ($1, $2, $3, now()) RETURNING id",
                )
                .bind(email)
                .bind(&info.name)
                .bind(&info.picture)
                .fetch_one(&mut *tx)
                .await?;
                uid
            }
        };

        sqlx::query(
            "INSERT INTO credentials (user_id, type, external_id) VALUES ($1, 'google', $2)",
        )
        .bind(uid)
        .bind(&info.sub)
        .execute(&mut *tx)
        .await?;

        uid
    };

    tx.commit().await?;
    Ok(user_id)
}

pub async fn me(auth: AuthCtx, State(state): State<AppState>) -> AppResult<Json<Me>> {
    let row: (String, Option<String>, Option<String>) =
        sqlx::query_as("SELECT email, name, avatar_url FROM users WHERE id = $1")
            .bind(auth.user_id)
            .fetch_one(&state.db)
            .await?;

    Ok(Json(Me {
        user_id: auth.user_id.to_string(),
        email: row.0,
        name: row.1,
        avatar_url: row.2,
        role: auth.role,
        // このセッションの実時 viewer grant をそのまま返す(前端の閲覧守衛用)。
        is_viewer: auth.is_viewer,
    }))
}

pub async fn logout(State(state): State<AppState>, jar: CookieJar) -> AppResult<Response> {
    if let Some(c) = jar.get(SESSION_COOKIE) {
        session::delete(&state.db, c.value()).await?;
    }
    let jar = jar.add(cookie::build_session_clear(state.config.cookie_secure));
    Ok((jar, StatusCode::NO_CONTENT).into_response())
}
