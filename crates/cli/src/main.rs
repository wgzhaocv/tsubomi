use anyhow::Result;
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};

mod api;
mod commands;
mod config;
mod oauth;
mod platform;
mod skill;
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
    /// データベース(create / list / rename / url(接続文字列)/ info(接続枠)/
    /// rotate / delete / connect(psql)/ query(SQL 実行))
    Db {
        #[command(subcommand)]
        action: commands::db::DbCmd,
    },
    /// キャッシュ(valkey。create / list / rename / status / url(接続文字列)/
    /// rotate / delete / connect(redis-cli))
    Cache {
        #[command(subcommand)]
        action: commands::cache::CacheCmd,
    },
    /// サービス(create(+GitHub 連携)/ list / status / start / stop / logs / metrics /
    /// deploys / exec / cat / verify / open / rollback / visibility / delete)
    Service {
        #[command(subcommand)]
        action: commands::service::ServiceCmd,
    },
    /// デプロイ(`--watch`:git push → Actions 追跡 → 検証まで一括 / `--local`:ローカルで
    /// build+push する GitHub 非依存の退路)
    Deploy(commands::deploy::DeployArgs),
    /// リソース(database / volume / cache / 別 service)を service に注入する
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
    /// ボリューム(create / list / rename / delete + ファイル操作 ls / put / get /
    /// rm / mkdir / mv)
    Volume {
        #[command(subcommand)]
        action: commands::volume::VolumeCmd,
    },
    /// ゴミ箱(list / restore / purge)— 4 種リソース共通
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
    /// AI エージェント向けデプロイ skill を管理(install / where / print)。
    /// 普段は毎回の self-heal が自動で最新へ揃えるので、明示実行は強制再書き出し用。
    Skill {
        #[command(subcommand)]
        action: commands::skill::SkillCmd,
    },
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

    // --help に「プラットフォーム / 本機のアーキ」を載せるため、after_help を実行時に組んでから
    // パースする(clap derive の after_help は文字列リテラルしか取れない)。プラットフォームアーキは
    // リリース時に焼き込んだ値(`platform::host_arch`)— どのマシンにデプロイしてもよく、arm を仮定しない。
    let cmd = Cli::command().after_help(format!(
        "プラットフォーム(tsubomi)のアーキテクチャ: {}\n現在のマシンのアーキテクチャ:           {}",
        platform::host_arch(),
        platform::machine_arch(),
    ));
    let mut matches = cmd.get_matches();
    let cli = Cli::from_arg_matches_mut(&mut matches).unwrap_or_else(|e| e.exit());
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

    // skill の self-heal:二進制内嵌の最新 skill をローカルの agent ターゲットへ投影する
    // (旧 / 欠けのときだけ書く。ネットワーク不要 = 「二進制だけ手動 update、skill はその投影」)。
    // json でも行う — AI が主な利用者で、書き出しは静かに(nudge は stderr のみ)。
    // `uninstall`(去る人)/ `skill`(自分で書く)では行わない。書いたら後で nudge を出す。
    let skill_wrote = if matches!(cli.command, Cmd::Uninstall | Cmd::Skill { .. }) {
        false
    } else {
        skill::ensure_fresh()
    };
    let is_deploy = matches!(cli.command, Cmd::Deploy(_));

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
        Cmd::Skill { action } => commands::skill::run(action).await,
        Cmd::Update => commands::update::run(cli.server).await,
        Cmd::Uninstall => commands::uninstall::run(cli.server).await,
    };

    if let Some(latest) = nudge {
        let current = env!("CARGO_PKG_VERSION");
        eprintln!(
            "note: tbm {latest} is available (you have {current}). Run 'tbm update' to upgrade."
        );
    }

    // skill の案内(stderr のみ。json でも出す — AI が読む)。書き出した直後は「今セッションでは
    // 未ロード = 直接読め」、それ以外でデプロイ系コマンドなら「手順は skill にある」と指す。
    if skill_wrote {
        if let Some(p) = skill::claude_skill_path() {
            eprintln!(
                "note: tsubomi のデプロイ skill を {} に書き出しました。今セッションでは未ロードです — デプロイ前にこのファイルを直接読んでください。",
                p.display()
            );
        }
    } else if is_deploy
        && let Some(p) = skill::claude_skill_path()
    {
        eprintln!(
            "note: デプロイ手順は skill にあります({})。未読ならまず読んでから進めてください。",
            p.display()
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 親コマンドの一行説明(about)が実際のサブコマンド名を全部列挙し続けることを保証する。
    /// トップの `tbm --help` は AI の第一発見面 — 実装とズレた要約が最も害が大きい
    /// (query / verify の存在を知らずに一段掘る羽目になる)。サブコマンドを足したら
    /// about にも足す、という規約をここで機械化する。
    #[test]
    fn parent_about_lists_all_subcommands() {
        let cmd = Cli::command();
        for parent in cmd
            .get_subcommands()
            .filter(|c| c.get_subcommands().next().is_some())
        {
            let about = parent
                .get_about()
                .map(|s| s.to_string())
                .unwrap_or_default();
            for sub in parent.get_subcommands() {
                let name = sub.get_name();
                if name == "help" {
                    continue;
                }
                assert!(
                    about.contains(name),
                    "`tbm {}` の説明にサブコマンド '{}' が載っていません(main.rs の doc comment に追記すること)",
                    parent.get_name(),
                    name
                );
            }
        }
    }
}
