//! 認証。amber(crates/server/src/auth)からの移植で、tsubomi の差分:
//! session / oauth-state / PKCE を Redis → Postgres に移し、Google の
//! hd ドメイン制限と owner ロール昇格を追加、apps の概念は無し。

pub mod cookie;
pub mod extractor;
pub mod google;
pub mod middleware;
pub mod oauth;
pub mod oauth_state;
pub mod session;
pub mod tokens;

use crate::config::Config;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use axum::Router;
use axum::http::HeaderMap;
use axum::routing::{delete, get, post};
use uuid::Uuid;

/// WebSocket 升级の `Origin` が管制面オリジンか検証する(CSWSH 対策)。不一致 / 欠落は
/// Forbidden。terminal / metrics の両 WS ハンドラが升级前に呼ぶ(SameSite=Lax は same-site の
/// テナント app からの WS 乗っ取りを防げないため、Origin で明示的に弾く)。
pub fn require_ws_origin(headers: &HeaderMap, config: &Config) -> AppResult<()> {
    let origin = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok());
    if config.origin_allowed(origin) {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

#[derive(Clone, Debug)]
pub struct AuthCtx {
    pub user_id: Uuid,
    /// `"user"` か `"owner"`。認証時に `users` から JOIN して入れるので、
    /// owner 専用ハンドラは追加クエリなしで判定できる。(design v2 §7 の
    /// 「owner 操作はバックエンドで毎回検証」の入力がこのフィールド —
    /// リクエスト毎に取り直すので常に新鮮。)
    pub role: String,
    pub source: AuthSource,
    /// **このセッションが現在、有効な共有パスワード viewer grant を持つか**
    /// (ユーザ属性ではない — session 単位・8h で失効)。閲覧ゲート
    /// `admin::require_viewer_web` の入力。viewer は web/session 専用なので
    /// Bearer 経路では常に false。(design v2 §7「見るは共有密码」)
    pub is_viewer: bool,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub enum AuthSource {
    Session,
    Token { token_id: Uuid },
}

impl AuthCtx {
    /// owner ロールか(role 文字列の比較を 1 箇所に集約 — owner 専用ハンドラの共通入力)。
    pub fn is_owner(&self) -> bool {
        self.role == "owner"
    }

    /// web セッション由来か(Bearer cli_token ではない)。owner ガバナンスは web 専用なので
    /// admin ハンドラがこれを要求する(CLI は AI 駆動のユーザ資源操作専用)。
    pub fn is_session(&self) -> bool {
        matches!(self.source, AuthSource::Session)
    }
}

pub fn public_routes() -> Router<AppState> {
    Router::new()
        .route("/auth/info", get(google::info))
        .route("/auth/google/start", get(google::start))
        .route("/auth/google/callback", get(google::callback))
        .route("/oauth/token", post(oauth::token))
}

pub fn protected_routes() -> Router<AppState> {
    Router::new()
        .route("/auth/me", get(google::me))
        .route("/auth/logout", post(google::logout))
        .route("/oauth/authorize", post(oauth::authorize))
        .route("/tokens", get(tokens::list).post(tokens::create))
        .route("/tokens/{id}", delete(tokens::revoke))
}
