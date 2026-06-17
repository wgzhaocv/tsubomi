use anyhow::Result;
use serde::Serialize;
use tsubomi_shared::Me;

use crate::api::fetch_me;
use crate::commands::{OutputFormat, print_json, resolve_server_from, resolve_token_from};
use crate::config;
use crate::platform;

/// `tbm whoami` の出力。`Me`(サーバ由来の認証ユーザ)に、CLI 側で分かる 2 つのアーキを足す。
/// AI は捕捉時に JSON で受け取るので、デプロイ前のアーキ判断がこの 1 コマンドで完結する。
#[derive(Serialize)]
struct WhoamiOut {
    #[serde(flatten)]
    me: Me,
    /// デプロイ対象プラットフォーム(tsubomi)のアーキ。リリース時に焼き込んだ値。
    platform_arch: &'static str,
    /// この tbm が動いているマシン(= `tbm deploy --local` のビルド機)のアーキ。
    machine_arch: &'static str,
}

pub async fn run(
    server_override: Option<String>,
    token_override: Option<String>,
    out: OutputFormat,
) -> Result<()> {
    let cfg = config::load()?;
    let server_url = resolve_server_from(server_override.as_deref(), cfg.as_ref());
    let token = resolve_token_from(token_override, cfg)?;
    let me = fetch_me(&server_url, &token).await?;
    let platform_arch = platform::host_arch();
    let machine_arch = platform::machine_arch();
    if out.is_json() {
        print_json(&WhoamiOut {
            me,
            platform_arch,
            machine_arch,
        })?;
    } else {
        println!(
            "{} ({}) · プラットフォーム {} / 本機 {}",
            me.email, me.role, platform_arch, machine_arch
        );
    }
    Ok(())
}
