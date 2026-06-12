mod config;
mod routes;

use config::Config;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "tsubomi_server=debug,tower_http=debug".into()),
        )
        .init();

    let config = Config::from_env()?;

    let app = routes::build_router(&config.web_dir);
    let listener = tokio::net::TcpListener::bind(config.bind_addr).await?;
    tracing::info!(addr = %config.bind_addr, web_dir = %config.web_dir, "tsubomi-server listening");
    axum::serve(listener, app).await?;
    Ok(())
}
