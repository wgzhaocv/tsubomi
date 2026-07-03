use anyhow::Result;
use serde::Serialize;

use crate::api::fetch_me;
use crate::commands::{OutputFormat, print_json, resolve_server_from, resolve_token_from};
use crate::config;
use crate::platform;

/// `tbm whoami` の出力。`Me`(サーバ由来の認証ユーザ)から**表示する項目だけ**を選んだ
/// CLI ビュー(avatar_url は長大な URL で AI 捕捉出力を埋める純粋な雑音 — 構造的に持たない。
/// flatten でなく明示列挙なのは、落とす判断をこのファイル 1 箇所に閉じるため)+
/// CLI 側で分かる 2 つのアーキ。AI は捕捉時に JSON で受け取るので、デプロイ前の
/// アーキ判断がこの 1 コマンドで完結する。
#[derive(Serialize)]
struct WhoamiOut {
    user_id: String,
    email: String,
    name: Option<String>,
    /// `"user"` か `"owner"`。
    role: String,
    is_viewer: bool,
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
            user_id: me.user_id,
            email: me.email,
            name: me.name,
            role: me.role,
            is_viewer: me.is_viewer,
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
