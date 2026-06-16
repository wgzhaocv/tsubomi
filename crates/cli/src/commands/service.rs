use anyhow::{Context, Result, bail};
use clap::Subcommand;
use serde_json::json;
use std::io::Write;
use std::process::{Command, Stdio};

use crate::api;
use crate::commands::{
    OutputFormat, print_json, resolve_server_from, resolve_service_id, resolve_token_from,
};
use crate::config;
use tsubomi_shared::{CreateServiceResp, InjectionDto, ServiceDto, WORKFLOW_PATH};

/// `tbm service <サブコマンド>`。各コマンド = API 呼び出し 1 本(web と同じハンドラ)。
/// create だけは API の後にユーザ自身の `gh` で GitHub 連携を組み立てる(平台は GitHub に
/// 一切触れない)。
#[derive(Subcommand)]
pub enum ServiceCmd {
    /// サービスを作成し、GitHub 連携(repo / secret / variable / workflow)を準備する
    Create {
        /// 表示名(例:myapp)。GitHub repo 名には subdomain を使う
        name: String,
    },
    /// サービス一覧
    List,
    /// サービスの状態(phase / desired / digest)とデプロイ履歴を表示
    Status {
        /// 対象サービスの表示名(`tbm service list` で確認)
        name: String,
    },
    /// サービスを開始(現 image_digest で再起動)
    Start {
        /// 対象サービスの表示名
        name: String,
    },
    /// サービスを停止(コンテナ停止 + ルート削除)
    Stop {
        /// 対象サービスの表示名
        name: String,
    },
    /// コンテナの直近ログを表示
    Logs {
        /// 対象サービスの表示名
        name: String,
        /// 取得する行数(既定 200)
        #[arg(long)]
        tail: Option<usize>,
    },
    /// コンテナ内で 1 コマンドを実行(非対話。`docker exec` 相当 = 線上診断 / スクリプト用。
    /// 対話シェルは web のターミナルを使う)。例:`tbm service exec myapp -- ps aux`
    Exec {
        /// 対象サービスの表示名(`tbm service list` で確認)
        name: String,
        /// コンテナ内で実行する argv(`--` の後ろにそのまま。例:`-- ps aux` /
        /// pipe/glob は `-- sh -c "ps | grep node"`)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        command: Vec<String>,
    },
    /// サービスを削除(ゴミ箱へ。3 日間は復元可能)
    Delete {
        /// 対象サービスの表示名
        name: String,
    },
    /// 指定したデプロイに戻す(再ビルドなし。deploy-id は `tbm service status` で確認)
    Rollback {
        /// 対象サービスの表示名
        name: String,
        /// 戻し先のデプロイ id
        deploy_id: String,
    },
}

pub async fn run(
    action: ServiceCmd,
    server: Option<String>,
    token: Option<String>,
    out: OutputFormat,
) -> Result<()> {
    let cfg = config::load()?;
    let server_url = resolve_server_from(server.as_deref(), cfg.as_ref());
    let token = resolve_token_from(token, cfg)?;
    let json = out.is_json();
    let c = reqwest::Client::new();

    match action {
        ServiceCmd::List => {
            let svcs = api::service_list(&c, &server_url, &token).await?;
            if json {
                print_json(&svcs)?;
            } else if svcs.is_empty() {
                println!("(サービスはありません。`tbm service create <名前>` で作成)");
            } else {
                for s in &svcs {
                    println!(
                        "service{:<3} {:<24} {}",
                        s.anon_seq, s.display_name, s.phase
                    );
                }
            }
        }
        ServiceCmd::Status { name } => {
            let id = resolve_service_id(&c, &server_url, &token, &name).await?;
            // 4 つの読み取りは独立なので並行取得する(逐次 4 往復 → 1 往復ぶん)。
            let (svc, deploys, injections, env) = tokio::join!(
                api::service_get(&c, &server_url, &token, &id),
                api::service_deploys(&c, &server_url, &token, &id),
                api::inject_list(&c, &server_url, &token, &id),
                api::env_keys(&c, &server_url, &token, &id),
            );
            let (svc, deploys, injections, env) = (svc?, deploys?, injections?, env?);
            if json {
                print_json(
                    &json!({ "service": svc, "deploys": deploys, "injections": injections, "env_keys": env }),
                )?;
            } else {
                print_status(&svc, &deploys, &injections, &env);
            }
        }
        ServiceCmd::Start { name } => {
            let id = resolve_service_id(&c, &server_url, &token, &name).await?;
            api::service_start(&c, &server_url, &token, &id).await?;
            if json {
                print_json(&json!({ "status": "running" }))?;
            } else {
                println!("起動しました(running)。");
            }
        }
        ServiceCmd::Stop { name } => {
            let id = resolve_service_id(&c, &server_url, &token, &name).await?;
            api::service_stop(&c, &server_url, &token, &id).await?;
            if json {
                print_json(&json!({ "status": "stopped" }))?;
            } else {
                println!("停止しました。");
            }
        }
        ServiceCmd::Logs { name, tail } => {
            let id = resolve_service_id(&c, &server_url, &token, &name).await?;
            let logs = api::service_logs(&c, &server_url, &token, &id, tail).await?;
            if json {
                print_json(&json!({ "logs": logs }))?;
            } else if logs.is_empty() {
                println!("(ログがありません。コンテナが走っていない可能性があります)");
            } else {
                print!("{logs}");
            }
        }
        ServiceCmd::Exec { name, command } => {
            let id = resolve_service_id(&c, &server_url, &token, &name).await?;
            let result = api::service_exec(&c, &server_url, &token, &id, &command).await?;
            if json {
                // 共有 DTO をそのまま:{ stdout, stderr, exit_code, truncated, timed_out }。
                // exit_code は **データ**(tbm 自身は 0 で終わる = リクエスト成功 ≠ 業務エラー。
                // AI はこの値で分岐する)。
                print_json(&result)?;
            } else {
                // text(端末)は ssh / docker exec 風:stdout は stdout・stderr は stderr へ
                // 素通しし、コンテナ内コマンドの終了コードを tbm の終了コードへ伝播する
                // (シェルの `&&` 連結のため)。
                print!("{}", result.stdout);
                eprint!("{}", result.stderr);
                if result.truncated {
                    eprintln!("(出力が上限を超えたため切り詰めました)");
                }
                if result.timed_out {
                    eprintln!(
                        "(タイムアウトで打ち切りました。長時間 / 対話は web のターミナルを使ってください)"
                    );
                }
                // process::exit は main の version nudge をスキップするが、exec はスクリプト用途
                // なのでクリーンな終了コードを優先する。終了コードは 0..=255 のみ素直に伝播し、
                // 想定外の値 / 確定不能(timeout 等で None)は「成功と確認できない」= 1 に倒す。
                std::io::stdout().flush().ok();
                std::io::stderr().flush().ok();
                let code = match result.exit_code {
                    Some(c) if (0..=255).contains(&c) => c as i32,
                    _ => 1,
                };
                std::process::exit(code);
            }
        }
        ServiceCmd::Delete { name } => {
            let id = resolve_service_id(&c, &server_url, &token, &name).await?;
            api::service_delete(&c, &server_url, &token, &id).await?;
            if json {
                print_json(&json!({ "status": "deleted", "recoverable_days": 3 }))?;
            } else {
                println!("削除しました(ゴミ箱へ。3 日間は復元可能)。");
            }
        }
        ServiceCmd::Rollback { name, deploy_id } => {
            let id = resolve_service_id(&c, &server_url, &token, &name).await?;
            api::service_rollback(&c, &server_url, &token, &id, &deploy_id).await?;
            if json {
                print_json(&json!({ "status": "running", "rolled_back_to": deploy_id }))?;
            } else {
                println!("ロールバックしました(running)。");
            }
        }
        ServiceCmd::Create { name } => {
            let resp = api::service_create(&c, &server_url, &token, &name).await?;
            if json {
                // AI 向け:gh は実行せず DTO をそのまま返す(AI が setup_commands を実行)。
                // resp は service(flatten)+ deploy_key + registry + hook_url + platforms
                // + workflow_yaml + setup_commands を含む。秘密はこの応答にしか出ない。
                print_json(&resp)?;
            } else {
                orchestrate(&resp)?;
            }
        }
    }
    Ok(())
}

/// status の text 表示(phase / desired / digest / 注入 / env keys / 直近のデプロイ履歴)。
fn print_status(
    svc: &ServiceDto,
    deploys: &[tsubomi_shared::DeployDto],
    injections: &[InjectionDto],
    env_keys: &[String],
) {
    println!(
        "{} (service{})  phase={} desired={}",
        svc.display_name, svc.anon_seq, svc.phase, svc.desired_state
    );
    println!("  subdomain:   {}", svc.subdomain);
    if !svc.url.is_empty() {
        println!("  url:         {}", svc.url);
    }
    if let Some(d) = &svc.image_digest {
        println!("  digest:      {}", short_digest(d));
    }
    if let Some(t) = &svc.last_deploy_at {
        println!("  last deploy: {t}");
    }
    if !injections.is_empty() {
        println!("  注入(反映には再デプロイ):");
        for i in injections {
            let stale = if i.valid { "" } else { "  [失効]" };
            println!(
                "    {} ← {} ({}){}  id={}",
                i.env_var, i.resource_name, i.resource_kind, stale, i.id
            );
        }
    }
    if !env_keys.is_empty() {
        println!("  env: {}", env_keys.join(", "));
    }
    if deploys.is_empty() {
        println!("  (まだデプロイがありません。`tbm deploy --local` か git push で開始)");
        return;
    }
    println!("  デプロイ履歴(新しい順。rollback は id を使う):");
    for d in deploys.iter().take(10) {
        let err = d
            .error
            .as_deref()
            .map(|e| format!("  — {e}"))
            .unwrap_or_default();
        println!(
            "    {}  {:<9} {}  id={}{}",
            d.created_at,
            d.status,
            short_sha(&d.git_sha),
            d.id,
            err
        );
    }
}

/// `sha256:<64hex>` → `sha256:<先頭 12>`(表示用の短縮)。
fn short_digest(d: &str) -> String {
    match d.split_once(':') {
        Some((algo, hex)) => format!("{algo}:{}", &hex[..hex.len().min(12)]),
        None => d.chars().take(19).collect(),
    }
}

fn short_sha(s: &str) -> String {
    s.chars().take(12).collect()
}

/// text モード:ローカル workflow を置き、gh が使えれば repo/secret/variable を組み立てる。
/// gh が無い / 未ログインなら手順を表示してフォールバックする(値は stdout、警告は stderr)。
fn orchestrate(resp: &CreateServiceResp) -> Result<()> {
    let svc = &resp.service;
    eprintln!(
        "サービスを作成しました:{} (service{}, subdomain={})",
        svc.display_name, svc.anon_seq, svc.subdomain
    );

    // 1. ローカル workflow ファイル(gh 不要)。無ければ書く。
    write_workflow_file(&resp.workflow_yaml)?;

    // 2. gh が使えなければ手順を出して終わり。
    if !gh_ok() {
        eprintln!("⚠ gh が見つからない / 未ログインです。リポジトリ直下で以下を実行してください");
        eprintln!("  (deploy_key / registry pass は秘密です。共有・commit しないこと):");
        // 手順は平台が組み立てた setup_commands をそのまま出す(CLI で再構築しない)。
        for line in &resp.setup_commands {
            println!("{line}");
        }
        return Ok(());
    }

    // 3. repo(冪等)。owner はログインユーザ、repo 名は subdomain(GitHub/DNS 安全な ascii)。
    let owner = gh_capture(&["api", "user", "-q", ".login"])?;
    let repo = format!("{owner}/{}", svc.subdomain);
    if gh_silent(&["repo", "view", &repo]) {
        eprintln!("repo {repo} は既にあります(再利用)");
    } else {
        run_gh(&[
            "repo",
            "create",
            &repo,
            "--private",
            "--source=.",
            "--remote=tsubomi",
        ])?;
    }

    // 4. secrets(値は argv に載せず stdin で渡す = `ps` で見えない)+ variables。
    gh_secret(&repo, "TSUBOMI_DEPLOY_KEY", &resp.deploy_key)?;
    gh_secret(&repo, "TSUBOMI_REGISTRY_USER", &resp.registry.user)?;
    gh_secret(&repo, "TSUBOMI_REGISTRY_PASS", &resp.registry.pass)?;
    gh_variable(&repo, "TSUBOMI_SERVICE_ID", &svc.id.to_string())?;
    gh_variable(&repo, "TSUBOMI_REGISTRY", &resp.registry.host)?;
    gh_variable(&repo, "TSUBOMI_HOOK_URL", &resp.hook_url)?;
    gh_variable(&repo, "TSUBOMI_PLATFORMS", &resp.platforms)?;

    eprintln!(
        "完了。`git add -A && git commit -m deploy && git push -u tsubomi main` で自動デプロイが走ります。"
    );
    Ok(())
}

/// workflow ファイルを書く(既存は上書きしない — ユーザの編集を尊重)。
fn write_workflow_file(yaml: &str) -> Result<()> {
    let path = std::path::Path::new(WORKFLOW_PATH);
    if path.exists() {
        eprintln!("{WORKFLOW_PATH} は既にあります(上書きしません)");
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, yaml).with_context(|| format!("{WORKFLOW_PATH} を書けません"))?;
    eprintln!("{WORKFLOW_PATH} を作成しました");
    Ok(())
}

// ===== gh ヘルパ =====

/// gh が使える(存在 + 認証済み)か。`gh auth status` が成功なら true。
fn gh_ok() -> bool {
    gh_silent(&["auth", "status"])
}

/// gh を出力を捨てて実行し、成功したかだけ返す(存在チェック / repo view 用)。
fn gh_silent(args: &[&str]) -> bool {
    Command::new("gh")
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// gh を実行(コマンドは stderr にエコー、出力は継承)。失敗は anyhow エラー。
fn run_gh(args: &[&str]) -> Result<()> {
    eprintln!("$ gh {}", args.join(" "));
    let status = Command::new("gh")
        .args(args)
        .status()
        .context("gh の実行に失敗しました(gh はインストール済みですか?)")?;
    if !status.success() {
        bail!("gh コマンドが失敗しました: gh {}", args.join(" "));
    }
    Ok(())
}

/// gh の標準出力を取得する(失敗はエラー)。
fn gh_capture(args: &[&str]) -> Result<String> {
    let out = Command::new("gh")
        .args(args)
        .output()
        .context("gh の実行に失敗しました")?;
    if !out.status.success() {
        bail!("gh コマンドが失敗しました: gh {}", args.join(" "));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// secret を設定する。値は argv ではなく stdin で渡す(`ps` で見えない)。
fn gh_secret(repo: &str, name: &str, value: &str) -> Result<()> {
    eprintln!("$ gh secret set {name} -R {repo}");
    let mut child = Command::new("gh")
        .args(["secret", "set", name, "-R", repo])
        .stdin(Stdio::piped())
        .spawn()
        .context("gh secret set の起動に失敗しました")?;
    child
        .stdin
        .take()
        .context("gh の stdin を開けません")?
        .write_all(value.as_bytes())
        .context("gh への secret 書き込みに失敗しました")?;
    let status = child.wait().context("gh secret set の待機に失敗しました")?;
    if !status.success() {
        bail!("gh secret set {name} が失敗しました");
    }
    Ok(())
}

/// variable を設定する(非機密なので --body でよい)。
fn gh_variable(repo: &str, name: &str, value: &str) -> Result<()> {
    run_gh(&["variable", "set", name, "-R", repo, "--body", value])
}
