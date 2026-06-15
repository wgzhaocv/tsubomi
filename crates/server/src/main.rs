// グローバルアロケータは jemalloc。今後のメモリ集約的なバックエンド処理を
// 見据えた選択:マルチスレッドのアロケーション churn では glibc malloc より
// 断片化がはるかに少なく、解放済みメモリを OS に返しやすい。
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

mod admin;
mod auth;
mod caches;
mod cli_release;
mod config;
mod crypto;
mod databases;
mod error;
mod gc;
mod ipblock;
mod mail;
mod respond;
mod routes;
mod services;
mod state;
mod tenant;
mod trash;
mod valkey;
mod validate;
mod volumes;

use config::Config;
use state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // cwd の .env があれば読む。無くても構わない(本番は systemd の
    // EnvironmentFile を使う)。
    let _ = dotenvy::dotenv();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "tsubomi_server=debug,tower_http=debug".into()),
        )
        .init();

    let config = Config::from_env()?;
    let listener = tokio::net::TcpListener::bind(config.bind_addr).await?;
    tracing::info!(
        addr = %config.bind_addr,
        web_dir = %config.web_dir,
        allowed_hds = %config.allowed_hds.join(","),
        "tsubomi-server starting"
    );

    let state = AppState::new(config).await?;
    gc::spawn(state.clone());
    // 現実(コンテナ/route)を期望状態へ収束させる第二の保険(restart=unless-stopped が第一)。
    // 起動時フル + 30s ライト(S8)。serve はブロックしない(初回フルは spawn 内の 0 tick)。
    services::reconcile::spawn(state.clone());
    // 起動時に IP 許可リストを traefik へ収束させる(middleware を必ず定義済みにする。
    // best-effort:書けなくてもサーバは起動する)。
    ipblock::sync_traefik(&state).await;
    // M5 cache:起動時に valkey の per-cache ACL を期望状態へ収束させる(揮発なので。§7.3)。
    // best-effort:valkey が落ちていても起動する(周期収束が次の tick で復活させる)。
    valkey::reconcile_acls(&state).await;
    // 本番 TLS:registry の push 入口(basicAuth)と apex router を traefik へ書く
    // (どちらも tls=false の dev では no-op。best-effort)。
    services::registry::sync_traefik(&state).await;
    if let Err(e) = services::route::write_apex(&state) {
        tracing::error!(error = ?e, "apex route の書き出しに失敗(本番のみ)");
    }
    let app = routes::build_router(state);
    axum::serve(listener, app).await?;
    Ok(())
}
