//! owner ガバナンスの管制面(M4)。背骨(paas-m4-design.md):可視性(見える)+
//! 兜底(動かす)の 2 枚。**owner 機能は web 専用** — 各ハンドラは owner 身分 **かつ**
//! session 由来を毎回検証する(Bearer cli_token では触れない。CLI は AI 駆動のユーザ
//! 資源操作専用というプロジェクト規約)。前端の表示制御はただの UX。
//!
//! S1 可視化(overview / ranking):跨ユーザの匿名化された一覧。真名は出すが、資源は
//! `display_name` ではなく匿名番号(service1 等)、内容(DB/ファイル/env)は出さない。

use crate::auth::AuthCtx;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use axum::Router;
use axum::routing::get;

mod overview;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/admin/overview", get(overview::overview))
        .route("/admin/ranking", get(overview::ranking))
}

/// admin ゲート:owner 身分 **かつ** session 由来(Bearer cli_token は拒否)。
/// 設計 v2 §7「owner 操作はバックエンドで毎回検証」+ owner 機能は web 専用。
/// AuthCtx.role / source は認証時に解決済みなので追加クエリ不要。
pub(crate) fn require_owner_web(auth: &AuthCtx) -> AppResult<()> {
    if auth.is_owner() && auth.is_session() {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}
