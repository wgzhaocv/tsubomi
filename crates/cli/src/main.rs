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
    /// 出力形式(env: TBM_OUTPUT)。auto=端末は text・パイプ/捕捉は json(AI 向け)
    #[arg(
        long,
        short = 'o',
        global = true,
        default_value = "auto",
        env = "TBM_OUTPUT"
    )]
    output: commands::OutputFormat,
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
    /// データベース(作成 / 一覧 / 接続文字列 / rotate / 削除 / psql 接続)
    Db {
        #[command(subcommand)]
        action: commands::db::DbCmd,
    },
    /// キャッシュ(valkey。作成 / 一覧 / 削除)
    Cache {
        #[command(subcommand)]
        action: commands::cache::CacheCmd,
    },
    /// サービス(作成 + GitHub 連携 / 一覧 / 状態)
    Service {
        #[command(subcommand)]
        action: commands::service::ServiceCmd,
    },
    /// デプロイ(`--local`:ローカルで build+push して hook を叩く。GitHub 非依存の退路)
    Deploy(commands::deploy::DeployArgs),
    /// リソース(database / volume)を service に注入する
    Inject(commands::inject::InjectArgs),
    /// 注入を外す(injection-id は `tbm service status` で確認)
    Eject {
        /// 外す注入の id
        id: String,
    },
    /// 静的 env(set / unset / list)。反映には再デプロイが必要
    Env {
        #[command(subcommand)]
        action: commands::env::EnvCmd,
    },
    /// ボリューム(作成 / 一覧 / 削除 + ファイル操作 ls/put/get/rm/mkdir/mv)
    Volume {
        #[command(subcommand)]
        action: commands::volume::VolumeCmd,
    },
    /// ゴミ箱(一覧 / 復元 / 完全削除)— 4 種リソース共通
    Trash {
        #[command(subcommand)]
        action: commands::trash::TrashCmd,
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
    // auto を一度だけ解決(端末→text / パイプ→json)。以後はこの out を配る。
    let out = cli.output.resolve();
    let json = out.is_json();

    // バージョンチェック:沈黙・クールダウン付き・**通知のみ** — 更新は常に
    // 明示的な `tbm update`(プロジェクト決定:自動更新はしない)。
    // `update`(自分で取得する)と `uninstall`(去る人)ではスキップ。
    // json モードでは出力を汚さない(かつ追加のネットワークも避ける)ため一切やらない。
    let nudge = if json {
        None
    } else {
        match cli.command {
            Cmd::Update | Cmd::Uninstall => None,
            _ => {
                let cfg = config::load().ok().flatten();
                let server = commands::resolve_server_from(cli.server.as_deref(), cfg.as_ref());
                version_check::maybe_check(&server, cfg).await
            }
        }
    };

    let result = match cli.command {
        Cmd::Login { manual, web } => commands::login::run(cli.server, manual, web).await,
        Cmd::Db { action } => commands::db::run(action, cli.server, cli.token, out).await,
        Cmd::Cache { action } => commands::cache::run(action, cli.server, cli.token, out).await,
        Cmd::Service { action } => commands::service::run(action, cli.server, cli.token, out).await,
        Cmd::Deploy(args) => commands::deploy::run(args, cli.server, cli.token, out).await,
        Cmd::Inject(args) => commands::inject::run_inject(args, cli.server, cli.token, out).await,
        Cmd::Eject { id } => commands::inject::run_eject(id, cli.server, cli.token, out).await,
        Cmd::Env { action } => commands::env::run(action, cli.server, cli.token, out).await,
        Cmd::Volume { action } => commands::volume::run(action, cli.server, cli.token, out).await,
        Cmd::Trash { action } => commands::trash::run(action, cli.server, cli.token, out).await,
        Cmd::Whoami => commands::whoami::run(cli.server, cli.token, out).await,
        Cmd::Logout => commands::logout::run(cli.server, out).await,
        Cmd::Health => commands::health::run(cli.server, out).await,
        Cmd::Update => commands::update::run(cli.server).await,
        Cmd::Uninstall => commands::uninstall::run(cli.server).await,
    };

    if let Some(latest) = nudge {
        let current = env!("CARGO_PKG_VERSION");
        eprintln!(
            "note: tbm {latest} is available (you have {current}). Run 'tbm update' to upgrade."
        );
    }

    // json モードのエラーは構造化して stdout に出す({error, code}、非零終了)。
    // code は API 由来なら ApiError から、それ以外は "error"。text は従来どおり
    // anyhow が stderr に「Error: …」を出す。
    if let Err(e) = &result
        && json
    {
        let code = e
            .downcast_ref::<api::ApiError>()
            .map(|a| a.code)
            .unwrap_or("error");
        let env = serde_json::json!({ "error": format!("{e:#}"), "code": code });
        println!("{env}");
        std::process::exit(1);
    }
    result
}
