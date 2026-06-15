use crate::auth::cookie::SESSION_COOKIE;
use crate::auth::{AuthCtx, AuthSource, session, tokens};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use axum::extract::{Request, State};
use axum::http::header::AUTHORIZATION;
use axum::middleware::Next;
use axum::response::Response;
use axum_extra::extract::CookieJar;

/// 認証は Bearer トークン(`Authorization` ヘッダがあればそちら優先)か
/// `tsubomi_session` cookie(フォールバック)。1 リクエスト内で両者は
/// 排他:
///
/// - `Authorization` ヘッダあり → 有効な `Bearer tbm_…` であること。
///   それ以外の値は cookie にフォールバック**せず** 401。認証ソースを
///   曖昧にしないため。
/// - `Authorization` ヘッダなし → 通常の cookie 経路。
pub async fn require_auth(
    State(state): State<AppState>,
    jar: CookieJar,
    mut req: Request,
    next: Next,
) -> AppResult<Response> {
    if let Some(value) = req.headers().get(AUTHORIZATION) {
        let header = value.to_str().map_err(|_| AppError::Unauthorized)?;
        let plaintext = tokens::parse_bearer(header).ok_or(AppError::Unauthorized)?;
        let auth = tokens::validate_token(&state.db, plaintext).await?;
        // last_used_at は観測用メタデータ。ホットパス(全保護リクエストが通る)
        // で DB 往復を 1 回待つ価値はないので、バックグラウンドで投げ捨てる。
        {
            let db = state.db.clone();
            let token_id = auth.token_id;
            tokio::spawn(async move { tokens::touch_last_used(&db, token_id).await });
        }
        req.extensions_mut().insert(AuthCtx {
            user_id: auth.user_id,
            role: auth.role,
            source: AuthSource::Token {
                token_id: auth.token_id,
            },
            // viewer は web/session 専用 — Bearer 経路では grant を持たない。
            is_viewer: false,
        });
        return Ok(next.run(req).await);
    }

    let session_token = jar
        .get(SESSION_COOKIE)
        .map(|c| c.value())
        .ok_or(AppError::Unauthorized)?;

    let (user_id, role, is_viewer) = session::get(&state.db, session_token)
        .await?
        .ok_or(AppError::Unauthorized)?;

    req.extensions_mut().insert(AuthCtx {
        user_id,
        role,
        source: AuthSource::Session,
        is_viewer,
    });

    Ok(next.run(req).await)
}
