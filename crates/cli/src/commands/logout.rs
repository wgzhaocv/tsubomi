use anyhow::Result;

use crate::commands::{OutputFormat, print_json};
use crate::config;

pub async fn run(_server_override: Option<String>, out: OutputFormat) -> Result<()> {
    config::delete()?;
    if out.is_json() {
        print_json(&serde_json::json!({
            "status": "logged_out",
            "note": "server-side token is still valid; revoke it from the web tokens page"
        }))?;
    } else {
        println!("ok");
        eprintln!(
            "note: サーバ側のトークンはまだ有効です。失効させるには web のトークン画面から削除してください。"
        );
    }
    Ok(())
}
