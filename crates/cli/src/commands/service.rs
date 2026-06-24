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
        /// GitHub 連携(repo / secret / variable + workflow ファイル)も `gh` で自動的に組み立てる。
        /// JSON 出力時でも実行するので、setup_commands の shell を手で叩く必要がない
        /// (Windows / mac / Linux いずれの shell でも動き、secret は stdin 渡しで argv に出さない)。
        /// `gh` が無い / 未ログインなら setup_commands を返すだけ(手動 fallback)。
        #[arg(long)]
        github: bool,
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
        ServiceCmd::Create { name, github } => {
            // GitHub 連携(`gh repo create --source=.` と後の `git push`)は **カレントを git
            // リポジトリとして** GitHub に繋ぐので、repo でなければ service 作成(= サーバ側の
            // 副作用)の **前** に `git init` して半端な状態(service だけ出来て連携が失敗)を防ぐ。
            // init が要るのは configure_github が実際に走るときだけ — それは **gh が使えて**、
            // かつ「json なら `--github` / text なら常に」連携経路に入るとき。gh が無い経路
            // (fallback で setup_commands を返すだけ)は repo を作らないので init しない
            // (不要な `.git` を掘らない)。gh_ok() は orchestrate でも再評価するが安価。
            if gh_ok() && (github || !json) {
                ensure_git_repo()?;
            }
            let resp = api::service_create(&c, &server_url, &token, &name).await?;
            if json {
                if github {
                    // AI 経路でも GitHub 連携を Rust 側で組み立てる(setup_commands の bash 文字列を
                    // AI が実行しなくてよい = OS 非依存。secret は stdin 渡しで argv に出さない)。
                    // 結果は機械可読な JSON(秘密は出さない。gh 不在なら setup_commands を返す)。
                    print_json(&orchestrate_json(&resp)?)?;
                } else {
                    // 既定:gh は実行せず DTO をそのまま返す(AI が setup_commands を実行)。
                    // resp は service(flatten)+ deploy_key + registry + hook_url + platforms
                    // + workflow_yaml + setup_commands を含む。秘密はこの応答にしか出ない。
                    print_json(&resp)?;
                }
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

    // 3. repo(冪等)→ secrets(stdin)→ variables + `tsubomi` remote。json 経路と同じ手順を共有。
    configure_github(resp)?;

    eprintln!(
        "完了。`git add -A && git commit -m deploy && git push -u tsubomi main` で自動デプロイが走ります。"
    );
    Ok(())
}

/// gh で repo(冪等)→ secrets(値は argv に載せず stdin で渡す = `ps` で見えない)→ variables を
/// 設定し、ローカルの `tsubomi` remote も確実にする。設定した repo (`owner/sub`) を返す。
/// text / json 両経路の単一実装(秘密名は workflow テンプレが参照する固定の契約 = 平台が単一真源)。
fn configure_github(resp: &CreateServiceResp) -> Result<String> {
    let svc = &resp.service;
    // owner はログインユーザ、repo 名は subdomain(GitHub/DNS 安全な ascii)。
    let owner = gh_capture(&["api", "user", "-q", ".login"])?;
    let repo = format!("{owner}/{}", svc.subdomain);
    if gh_silent(&["repo", "view", &repo]) {
        eprintln!("repo {repo} は既にあります(再利用)");
        // 既存 repo なら create をスキップするので、`git push -u tsubomi main` が通るよう
        // ローカル remote を補う(create 時は --remote=tsubomi で張られる)。
        ensure_tsubomi_remote(&repo);
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
    gh_secret(&repo, "TSUBOMI_DEPLOY_KEY", &resp.deploy_key)?;
    gh_secret(&repo, "TSUBOMI_REGISTRY_USER", &resp.registry.user)?;
    gh_secret(&repo, "TSUBOMI_REGISTRY_PASS", &resp.registry.pass)?;
    gh_variable(&repo, "TSUBOMI_SERVICE_ID", &svc.id.to_string())?;
    gh_variable(&repo, "TSUBOMI_REGISTRY", &resp.registry.host)?;
    gh_variable(&repo, "TSUBOMI_HOOK_URL", &resp.hook_url)?;
    gh_variable(&repo, "TSUBOMI_PLATFORMS", &resp.platforms)?;
    Ok(repo)
}

/// カレントが git リポジトリでなければ `git init -b main` する(`--github` 連携の前提)。
/// `gh repo create --source=.` と後の `git push` に repo が要り、カレントは元々 repo にする対象
/// なので自動初期化する。**service 作成(サーバ側副作用)の前** に呼ぶことで半端な状態を防ぐ。
/// 出力は stderr / null に倒し、JSON モードの stdout を汚さない。
fn ensure_git_repo() -> Result<()> {
    // `rev-parse --is-inside-work-tree` は work tree 内なら exit 0、repo 外なら非零(128)。
    let inside = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if inside {
        return Ok(());
    }
    // 初期ブランチを **main** に固定する。素の `git init` は init.defaultBranch 未設定だと
    // master を作るが、デプロイ経路は一貫して main を前提にする(成功手順の
    // `git push -u tsubomi main`、生成 workflow の `branches: [main]`)。ここで master のまま
    // だと push が refspec 不一致で失敗 / workflow が起動せず、半端さを別の形で再発させる。
    // `-b` は git 2.28+(古い git は下の失敗 bail で気付ける)。
    eprintln!("$ git init -b main(カレントは git リポジトリではないので初期化します)");
    let ok = Command::new("git")
        .args(["init", "-b", "main"])
        .stdout(Stdio::null()) // 初期化メッセージで JSON の stdout を汚さない。
        .status()
        .context("git init の起動に失敗しました(git はインストール済みですか?)")?
        .success();
    if !ok {
        bail!("git init に失敗しました(古い git なら `git init -b main` 相当を手動で)。カレントディレクトリを確認してください");
    }
    Ok(())
}

/// ローカルに `tsubomi` remote が無ければ HTTPS で張る(既存なら触らない)。
/// gh の HTTPS 資格ヘルパで push が通る。失敗は致命でないので無視(push 時に気付ける)。
fn ensure_tsubomi_remote(repo: &str) {
    let exists = Command::new("git")
        .args(["remote", "get-url", "tsubomi"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if exists {
        return;
    }
    let url = format!("https://github.com/{repo}.git");
    eprintln!("$ git remote add tsubomi {url}");
    let _ = Command::new("git")
        .args(["remote", "add", "tsubomi", &url])
        .status();
}

/// JSON 出力 + `--github` 時の GitHub 連携。configure_github() を呼び、進捗は stderr・
/// **結果は機械可読な JSON** で返す。
///
/// 秘密の扱い:成功時(`configured: true`)は stdout に秘密を出さない。**gh が無い / 途中で
/// 失敗した場合だけ** fallback として `setup_commands`(deploy_key / registry pass を含む)を返す
/// — これは非 `--github`(setup_commands を必ず返す)と同じ露出ティアで、受容済み。
///
/// なぜ失敗を Err にせず fallback の JSON にするか:service は既にサーバ側で作成済みなので、
/// ここでハード失敗すると AI は秘密(create 応答にしか出ない)を失い、再 `create` が 409 conflict
/// になって詰む。手順(秘密込み)を返せば、手動完遂 / 別 OS での再開ができる。
fn orchestrate_json(resp: &CreateServiceResp) -> Result<serde_json::Value> {
    let svc = &resp.service;
    // workflow ファイルは gh の有無に関係なく置く(git push で CI が回る)。
    write_workflow_file(&resp.workflow_yaml)?;

    // gh 不在 / 途中失敗の共通 fallback(setup_commands で手動完遂・再開できるようにする)。
    let fallback = |reason: String| {
        json!({
            "service": svc,
            "github": {
                "configured": false,
                "reason": reason,
                "workflow_path": WORKFLOW_PATH,
                "setup_commands": resp.setup_commands,
            }
        })
    };

    if !gh_ok() {
        return Ok(fallback(
            "gh が見つからない / 未ログイン(`gh auth login` 後に再実行、または setup_commands を実行)"
                .to_string(),
        ));
    }

    // 設定済みなら AI が使うのは「設定できたか / どの repo か / 次の一手」だけ(秘密名一覧は
    // 載せない = テンプレ契約の重複と drift を避ける)。途中失敗は fallback に倒す(上記の理由)。
    match configure_github(resp) {
        Ok(repo) => Ok(json!({
            "service": svc,
            "github": {
                "configured": true,
                "repo": repo,
                "workflow_path": WORKFLOW_PATH,
                "next": "git add -A && git commit -m deploy && git push -u tsubomi main で自動デプロイ",
            }
        })),
        Err(e) => Ok(fallback(format!(
            "gh での設定が途中で失敗しました(service は作成済み)。setup_commands を実行して完遂してください: {e}"
        ))),
    }
}

/// workflow ファイルを書く(既存は基本上書きしない — ユーザの編集を尊重)。
/// 例外:旧版の壊れた配方(存在しない npm パッケージ `@railway/nixpacks` を呼び CI が必ず失敗する)が
/// 残っている場合だけは修正版で上書きする。これは平台の生成物でユーザ編集ではなく、放置すると
/// `--github` が成功しても CI が同じ原因で失敗し続ける(= 今回の修正が届かない)。
fn write_workflow_file(yaml: &str) -> Result<()> {
    let path = std::path::Path::new(WORKFLOW_PATH);
    if path.exists() {
        let existing = std::fs::read_to_string(path).unwrap_or_default();
        if existing.contains("@railway/nixpacks") {
            std::fs::write(path, yaml)
                .with_context(|| format!("{WORKFLOW_PATH} を更新できません"))?;
            eprintln!("{WORKFLOW_PATH} の旧版(壊れた nixpacks 配方)を修正版に更新しました");
        } else {
            eprintln!("{WORKFLOW_PATH} は既にあります(上書きしません)");
        }
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
