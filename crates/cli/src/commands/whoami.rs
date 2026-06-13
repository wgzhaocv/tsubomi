use anyhow::Result;

use crate::api::fetch_me;
use crate::commands::{OutputFormat, print_json, resolve_server_from, resolve_token_from};
use crate::config;

pub async fn run(
    server_override: Option<String>,
    token_override: Option<String>,
    out: OutputFormat,
) -> Result<()> {
    let cfg = config::load()?;
    let server_url = resolve_server_from(server_override.as_deref(), cfg.as_ref());
    let token = resolve_token_from(token_override, cfg)?;
    let me = fetch_me(&server_url, &token).await?;
    if out.is_json() {
        print_json(&me)?;
    } else {
        println!("{} ({})", me.email, me.role);
    }
    Ok(())
}
