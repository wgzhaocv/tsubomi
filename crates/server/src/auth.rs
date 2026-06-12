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

use crate::state::AppState;
use axum::Router;
use axum::routing::{delete, get, post};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct AuthCtx {
    pub user_id: Uuid,
    /// `"user"` か `"owner"`。認証時に `users` から JOIN して入れるので、
    /// owner 専用ハンドラは追加クエリなしで判定できる。(design v2 §7 の
    /// 「owner 操作はバックエンドで毎回検証」の入力がこのフィールド —
    /// リクエスト毎に取り直すので常に新鮮。)
    pub role: String,
    pub source: AuthSource,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub enum AuthSource {
    Session,
    Token { token_id: Uuid },
}

pub fn public_routes() -> Router<AppState> {
    Router::new()
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
