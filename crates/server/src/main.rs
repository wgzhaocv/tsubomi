// グローバルアロケータは jemalloc。今後のメモリ集約的なバックエンド処理を
// 見据えた選択:マルチスレッドのアロケーション churn では glibc malloc より
// 断片化がはるかに少なく、解放済みメモリを OS に返しやすい。
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

mod auth;
mod cli_release;
mod config;
mod crypto;
mod databases;
mod error;
mod gc;
mod routes;
mod state;
mod tenant;
mod trash;
mod validate;

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
    let app = routes::build_router(state);
    axum::serve(listener, app).await?;
    Ok(())
}
