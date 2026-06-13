use anyhow::Result;
use clap::{Parser, Subcommand};

mod api;
mod commands;
mod config;
mod oauth;
mod version_check;

/// tbm — tsubomi プラットフォーム CLI。
#[derive(Parser)]
#[command(name = "tbm", version, about = "tsubomi platform CLI")]
struct Cli {
    /// サーバ URL(env: TSUBOMI_SERVER)。保存済み設定より優先
    #[arg(long, env = "TSUBOMI_SERVER", global = true)]
    server: Option<String>,
    /// Bearer トークン(env: TSUBOMI_TOKEN)。保存済み設定より優先 — CI / スクリプト用
    #[arg(long, env = "TSUBOMI_TOKEN", global = true)]
    token: Option<String>,
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// ブラウザで OAuth + PKCE によりこの CLI を認可する。
    /// 無指定なら自動判定(SSH 先・ヘッドレスは手動に倒す)。
    Login {
        /// コピペ方式を強制(ブラウザと CLI が別マシンの場合。SSH 先など)
        #[arg(long, conflicts_with = "web")]
        manual: bool,
        /// ブラウザ(loopback)方式を強制(自動判定を上書き)
        #[arg(long)]
        web: bool,
    },
    /// 現在の認証ユーザを表示
    Whoami,
    /// ローカル設定を削除(サーバ側トークンは失効しない)
    Logout,
    /// サーバのヘルスチェック
    Health,
    /// このプラットフォーム向けの最新 tbm バイナリを取得して入れ替える
    Update,
    /// ローカル設定とバイナリ自体を削除(完全アンインストール)
    Uninstall,
}

#[tokio::main]
async fn main() -> Result<()> {
    // TLS の crypto provider は ring を明示インストールする。reqwest の
    // デフォルト(aws-lc)は windows-gnu / linux へのクロスコンパイルが
    // 通らないため rustls-no-provider + ring 構成にしている。
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("failed to install rustls ring provider");

    let cli = Cli::parse();

    // バージョンチェック:沈黙・クールダウン付き・**通知のみ** — 更新は常に
    // 明示的な `tbm update`(プロジェクト決定:自動更新はしない)。
    // `update`(自分で取得する)と `uninstall`(去る人)ではスキップ。
    let nudge = match cli.command {
        Cmd::Update | Cmd::Uninstall => None,
        _ => {
            let cfg = config::load().ok().flatten();
            let server = commands::resolve_server_from(cli.server.as_deref(), cfg.as_ref());
            version_check::maybe_check(&server, cfg).await
        }
    };

    let result = match cli.command {
        Cmd::Login { manual, web } => commands::login::run(cli.server, manual, web).await,
        Cmd::Whoami => commands::whoami::run(cli.server, cli.token).await,
        Cmd::Logout => commands::logout::run(cli.server).await,
        Cmd::Health => commands::health::run(cli.server).await,
        Cmd::Update => commands::update::run(cli.server).await,
        Cmd::Uninstall => commands::uninstall::run(cli.server).await,
    };

    if let Some(latest) = nudge {
        let current = env!("CARGO_PKG_VERSION");
        eprintln!(
            "note: tbm {latest} is available (you have {current}). Run 'tbm update' to upgrade."
        );
    }
    result
}
