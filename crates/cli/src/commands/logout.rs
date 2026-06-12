use anyhow::Result;

use crate::config;

pub async fn run(_server_override: Option<String>) -> Result<()> {
    config::delete()?;
    println!("ok");
    eprintln!("note: server-side token is still valid; revoke it from the web tokens page");
    Ok(())
}
