use anyhow::Result;

use crate::api::fetch_health;
use crate::commands::resolve_server_from;
use crate::config;

pub async fn run(server_override: Option<String>) -> Result<()> {
    let cfg = config::load()?;
    let server_url = resolve_server_from(server_override.as_deref(), cfg.as_ref());
    let health = fetch_health(&server_url).await?;
    println!("status: {}  version: {}", health.status, health.version);
    Ok(())
}
