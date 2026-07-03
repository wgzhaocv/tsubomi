pub mod web;

use crate::auth;
use crate::cli_release;
use crate::state::AppState;
use axum::routing::{get, post};
use axum::{Json, Router, middleware};
use tower_http::trace::TraceLayer;
use tsubomi_shared::Health;

pub fn build_router(state: AppState) -> Router {
    let mut public = Router::new()
        .route("/health", get(health))
        .route("/cli/version", get(cli_release::version))
        .route("/cli/version/{target}", get(cli_release::version_target))
        // deploy hook は HMAC = 権限。session/Bearer は通さない(IP 除外、決定 #4)。
        .route("/hook/deploy", post(crate::services::deploy::deploy))
        .merge(auth::public_routes());

    // CLI リリースのアーカイブ本体。manifest の url(相対パス
    // /api/cli/dl/…)がここを指す。release_dir 未設定なら配信しない。
    if let Some(dir) = &state.config.release_dir {
        public = public.nest_service(
            "/cli/dl",
            tower_http::services::ServeDir::new(dir.join("dl")),
        );
    }

    // protected:認証(session / Bearer)の後ろ。auth + database + trash の各面を
    // 同じ require_auth layer の下に束ねる(web も CLI も同じ extractor を通る)。
    let protected = auth::protected_routes()
        .merge(crate::databases::routes())
        .merge(crate::caches::routes())
        .merge(crate::volumes::routes())
        .merge(crate::services::routes())
        .merge(crate::ipblock::routes())
        .merge(crate::admin::routes())
        .merge(crate::owners::routes())
        .merge(crate::trash::routes())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::middleware::require_auth,
        ));

    Router::new()
        // /api 配下の未マッチはここで 404 に確定させる。これが無いと外側の SPA fallback に
        // 落ちて **200 + index.html** が返り、新しい CLI が古いサーバの「未対応エンドポイント」を
        // 機械判別できない(JSON を期待して HTML を掴む)。認証層の外で良い — 存在しない
        // パスに秘密は無く、未ログインでも 404 が正しい答え。
        .nest("/api", public.merge(protected).fallback(api_not_found))
        // インストールスクリプト(curl | sh で叩く短い URL)。配信時に
        // サーバがドメインを注入する(cli_release::serve_script)。
        .route("/install.sh", get(cli_release::install_sh))
        .route("/install.ps1", get(cli_release::install_ps1))
        .route("/install.bat", get(cli_release::install_bat))
        // /api 以外はすべてビルド済み SPA へフォールバック。index.html を
        // fallback にすることでクライアントサイドのルート(/oauth/authorize
        // など)も解決する。amber と同じ配信方式。
        .fallback_service(web::fallback(&state.config.web_dir))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn health() -> Json<Health> {
    Json(Health {
        status: "ok".into(),
        version: env!("CARGO_PKG_VERSION").into(),
    })
}

/// `/api` 配下の未マッチ路径。本文は AppError と同じ**素の text**(サーバは JSON エラー体を
/// 出さない契約 — CLI は本文を message にそのまま載せ、code は HTTP 404 から導く。
/// JSON にすると CLI 出力に JSON-in-string の二重包みが出る)。
async fn api_not_found() -> (axum::http::StatusCode, &'static str) {
    (
        axum::http::StatusCode::NOT_FOUND,
        "この API パスはこのサーバにありません(サーバが古い可能性。プラットフォーム管理者にサーバ更新を確認してください)",
    )
}
