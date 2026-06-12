pub mod web;

use axum::{Json, Router, routing::get};
use tower_http::trace::TraceLayer;
use tsubomi_shared::{Greeting, Health};

pub fn build_router(web_dir: &str) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/hello", get(hello))
        // Everything that isn't an /api route falls through to the built SPA,
        // with index.html as the fallback so client-side routes (e.g. /about)
        // resolve. Same hosting approach as amber (crates/server/src/routes/web.rs).
        .fallback_service(web::fallback(web_dir))
        .layer(TraceLayer::new_for_http())
}

async fn health() -> Json<Health> {
    Json(Health {
        status: "ok".into(),
        version: env!("CARGO_PKG_VERSION").into(),
    })
}

async fn hello() -> Json<Greeting> {
    Json(Greeting {
        message: "蕾 — hello from tsubomi-server".into(),
    })
}
