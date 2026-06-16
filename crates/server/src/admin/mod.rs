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
use axum::routing::{get, post};

mod actions;
mod audit_view;
mod overview;
mod viewer;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/admin/overview", get(overview::overview))
        .route("/admin/ranking", get(overview::ranking))
        // ホスト(サーバ本体)の CPU/メモリ/ディスク使用量を WS で配信(共有サンプラ)。
        // 升级は cookie 付き GET なので require_auth + AuthCtx が効く(handler 内で viewer 検証)。
        .route("/admin/metrics", get(crate::metrics::metrics_ws))
        // 最後の砦(S3):owner が他人の資源を停止 / 削除(二段確認 + 検証コード)。
        .route("/admin/resources/{id}/stop", post(actions::stop))
        .route("/admin/resources/{id}/delete", post(actions::delete))
        // 監査ログ閲覧(S4)。
        .route("/admin/audit", get(audit_view::list))
        // 共有パスワード viewer(S5):login = 任意の session、password = owner のみ。
        .route("/admin/viewer/login", post(viewer::login))
        .route(
            "/admin/viewer/password",
            get(viewer::status).post(viewer::set_password),
        )
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

/// 閲覧ゲート(S5):web セッション由来 **かつ**(owner **または** 有効な viewer grant)。
/// 設計 v2 §7「見るは共有密码」— 共有パスワードを入れた社内ユーザは只读で管制面を
/// 見られる。**只读の可視化(overview / ranking)専用**。危険操作(stop/delete)・
/// viewer パスワード設定・監査ログ・IP 許可リストは owner のみ(require_owner_web)。
pub(crate) fn require_viewer_web(auth: &AuthCtx) -> AppResult<()> {
    if auth.is_session() && (auth.is_owner() || auth.is_viewer) {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}
