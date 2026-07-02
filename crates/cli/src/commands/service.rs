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
use tsubomi_shared::{
    CreateServiceResp, InjectionDto, ServiceDto, VISIBILITY_COMPANY, VISIBILITY_PRIVATE,
    VISIBILITY_PUBLIC, WORKFLOW_PATH,
};

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
    /// コンテナ内のファイルを表示する(`exec -- cat <path>` の糖衣。線上の設定 / ログ確認用)
    Cat {
        /// 対象サービスの表示名(`tbm service list` で確認)
        name: String,
        /// コンテナ内の絶対パス(例:/app/config.json)
        path: String,
    },
    /// 公開 URL の存活を検証:根 HTML とそこから参照される js/css 子リソースが全部 2xx か。
    /// deploy=succeeded + 根 200 でも assets が 404 で白画面、という取りこぼしを検出する
    Verify {
        /// 対象サービスの表示名(`tbm service list` で確認)
        name: String,
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
    /// 公開範囲を切り替える(**即時反映・再デプロイ不要**。現在値は `tbm service status` で確認)
    Visibility {
        /// 対象サービスの表示名(`tbm service list` で確認)
        name: String,
        /// 新しい公開範囲
        #[arg(value_enum)]
        visibility: VisibilityArg,
    },
}

/// `tbm service visibility` の値(clap ValueEnum = 取値を help に列挙、綴りミスは exit 2)。
/// サーバ側の 400 検証が最終ガード。
#[derive(Clone, Copy, clap::ValueEnum)]
pub enum VisibilityArg {
    /// 公開 URL を無効にする(外部からアクセス不可。内部リンク / logs / exec は従来どおり)
    Private,
    /// 会社の IP 許可リストからのみアクセス可(既定)
    Company,
    /// 一般公開(IP 制限なし。アプリ側に認証が無ければ誰でもアクセス可能)
    Public,
}

impl VisibilityArg {
    fn as_str(self) -> &'static str {
        match self {
            Self::Private => VISIBILITY_PRIVATE,
            Self::Company => VISIBILITY_COMPANY,
            Self::Public => VISIBILITY_PUBLIC,
        }
    }
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
            emit_exec_result(&result, json)?;
        }
        ServiceCmd::Cat { name, path } => {
            // `exec -- cat <path>` の糖衣(サーバ側は同じ /exec エンドポイント)。出力・終了
            // コードの流儀も exec と完全に同じ:text はファイル内容を stdout へ素通し。
            let id = resolve_service_id(&c, &server_url, &token, &name).await?;
            let cmd = ["cat".to_string(), path];
            let result = api::service_exec(&c, &server_url, &token, &id, &cmd).await?;
            emit_exec_result(&result, json)?;
        }
        ServiceCmd::Verify { name } => {
            let id = resolve_service_id(&c, &server_url, &token, &name).await?;
            let svc = api::service_get(&c, &server_url, &token, &id).await?;
            // private は公開 URL 自体が無効(route 無し)。探測すると接続失敗になり「サーバ障害」と
            // 誤読させる(AI が無駄リトライする既知の実害パターン)ので、明確な文言で短絡する。
            // 旧サーバ(visibility 空)は company 扱いで従来どおり探測する。
            if svc.visibility == VISIBILITY_PRIVATE {
                bail!(
                    "このサービスは非公開(visibility=private)です。公開 URL は無効のため検証をスキップしました。公開するには `tbm service visibility {name} company`(または public)を実行してください"
                );
            }
            if svc.url.is_empty() {
                bail!("このサービスには公開 URL がありません(`tbm service status {name}` で確認)");
            }
            let report = verify_url(&c, &svc.url).await?;
            if json {
                // 報告は JSON で出しつつ、終了コードも検証結果を映す(grep 型の「チェック
                // コマンド」なのでシェル / CI が exit code だけで分岐できる — codex 監査)。
                print_json(&report)?;
                if !report.ok {
                    std::io::stdout().flush().ok();
                    std::process::exit(1);
                }
            } else {
                let mark = |s: u16| if (200..300).contains(&s) { "✓" } else { "✗" };
                println!("{} {} (根 HTML)", mark(report.root_status), svc.url);
                for r in &report.resources {
                    println!("  {} {} {}", mark(r.status), r.status, r.url);
                }
                if report.ok {
                    println!(
                        "OK:根 + 子リソース {} 件すべて 2xx。",
                        report.resources.len()
                    );
                } else {
                    // 白画面の典型原因と次の一手(AI / 人間の自己修正用)。
                    println!(
                        "NG:2xx でないリソースがあります。assets 404 は build 出力パス / base 設定 / 直近デプロイの失敗が典型です(`tbm service status {name}` でデプロイ履歴を確認)。"
                    );
                    std::io::stdout().flush().ok();
                    std::process::exit(1);
                }
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
        ServiceCmd::Visibility { name, visibility } => {
            let id = resolve_service_id(&c, &server_url, &token, &name).await?;
            let v = visibility.as_str();
            api::service_set_visibility(&c, &server_url, &token, &id, v).await?;
            if json {
                print_json(&json!({ "visibility": v }))?;
            } else {
                match visibility {
                    VisibilityArg::Private => println!(
                        "非公開にしました(即時反映)。公開 URL は無効になりますが、内部リンク・logs・exec は従来どおり使えます。"
                    ),
                    VisibilityArg::Company => println!(
                        "社内限定にしました(即時反映)。会社の IP 許可リストからのみアクセスできます。"
                    ),
                    VisibilityArg::Public => println!(
                        "一般公開にしました(即時反映)。IP 制限はありません — アプリ側の認証にご注意ください。"
                    ),
                }
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
            let created_git = gh_ok() && (github || !json) && ensure_git_repo()?;
            // service 作成(サーバ側の最初の副作用)。失敗時、直前に掘った `.git` はまだ何も
            // 載っていない(空ディレクトリで init しただけ)ので削除して原子性を保つ —
            // 「remote add 失敗後に半端な状態が残る」という実利用フィードバックへの対処。
            // service 作成 **後** の gh 失敗は巻き戻さない(repo は再開に必要。orchestrate_json の
            // fallback = setup_commands で完遂できる)。
            let resp = api::service_create(&c, &server_url, &token, &name)
                .await
                .inspect_err(|_| {
                    if created_git {
                        let _ = std::fs::remove_dir_all(".git");
                        eprintln!("(初期化した .git は削除しました — 再実行はクリーンな状態から)");
                    }
                })?;
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

/// exec / cat 共通の結果出力。
/// json:共有 DTO をそのまま:{ stdout, stderr, exit_code, truncated, timed_out }。
/// exit_code は **データ**(tbm 自身は 0 で終わる = リクエスト成功 ≠ 業務エラー。AI はこの値で分岐)。
/// text:ssh / docker exec 風に stdout / stderr を素通しし、コンテナ内コマンドの終了コードを
/// tbm の終了コードへ伝播する(シェルの `&&` 連結のため)。process::exit は main の version nudge を
/// スキップするが、スクリプト用途なのでクリーンな終了コードを優先。0..=255 のみ素直に伝播し、
/// 想定外 / 確定不能(timeout 等で None)は「成功と確認できない」= 1 に倒す。
fn emit_exec_result(result: &tsubomi_shared::ExecResult, json: bool) -> Result<()> {
    if json {
        print_json(result)?;
        return Ok(());
    }
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
    std::io::stdout().flush().ok();
    std::io::stderr().flush().ok();
    let code = match result.exit_code {
        Some(c) if (0..=255).contains(&c) => c as i32,
        _ => 1,
    };
    std::process::exit(code);
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
        // private でも URL 文字列は温存して表示する(再公開すれば同じ URL で復活する)。
        let suffix = if svc.visibility == VISIBILITY_PRIVATE {
            "(非公開のため無効)"
        } else {
            ""
        };
        println!("  url:         {}{suffix}", svc.url);
    }
    // 旧サーバ(フィールド無し = 空文字)は行ごと出さない。
    if !svc.visibility.is_empty() {
        println!("  visibility:  {}", svc.visibility);
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
        let msg = d
            .commit_message
            .as_deref()
            .map(|m| format!("  {m}"))
            .unwrap_or_default();
        println!(
            "    {}  {:<9} {}{}  id={}{}",
            d.created_at,
            d.status,
            short_sha(&d.git_sha),
            msg,
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
    // ランナーは平台が platforms から導出した値をそのまま使う(CLI で再導出しない =
    // 単一真源)。旧サーバは runner を返さない(空)ので、その場合は設定しない
    // (workflow テンプレの || 'ubuntu-latest' フォールバックが効く)。
    if !resp.runner.is_empty() {
        gh_variable(&repo, "TSUBOMI_RUNNER", &resp.runner)?;
    }
    Ok(repo)
}

/// カレントが git リポジトリでなければ `git init -b main` する(`--github` 連携の前提)。
/// `gh repo create --source=.` と後の `git push` に repo が要り、カレントは元々 repo にする対象
/// なので自動初期化する。**service 作成(サーバ側副作用)の前** に呼ぶことで半端な状態を防ぐ。
/// 出力は stderr / null に倒し、JSON モードの stdout を汚さない。
///
/// 汚染防止(実利用のフィードバック起因):**repo でもなく空でもないディレクトリでは拒否**する。
/// 誤って別プロジェクトの根や home で実行すると、そのディレクトリ全体が新 repo として GitHub に
/// push される事故になる。デプロイ対象なら `git init -b main` を明示実行してから再実行してもらう
/// (= ユーザの明示同意)。戻り値は「この呼び出しで `.git` を新規作成したか」— 呼び側が
/// service 作成失敗時のロールバック(掘った `.git` の削除)に使う。
fn ensure_git_repo() -> Result<bool> {
    // `rev-parse --is-inside-work-tree` は work tree 内なら exit 0、repo 外なら非零(128)。
    let inside = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if inside {
        return Ok(false);
    }
    // 空判定:macOS の `.DS_Store` だけは無視(実質空)。読めないエントリは「非空」に倒す
    // (安全側 = 空と証明できない限り init しない)。
    let non_empty = std::fs::read_dir(".")
        .context("カレントディレクトリを読めません")?
        .any(|e| match e {
            Ok(e) => e.file_name() != ".DS_Store",
            Err(_) => true,
        });
    if non_empty {
        bail!(
            "カレントディレクトリは git リポジトリではなく、空でもありません。誤ったディレクトリを GitHub へ push する事故を防ぐため中止しました。このディレクトリをデプロイ対象にするなら `git init -b main` を実行してから再実行、そうでなければ空のディレクトリで実行してください"
        );
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
    Ok(true)
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

// ===== verify(公開 URL の存活検証) =====

/// `tbm service verify` の結果(JSON はこの DTO をそのまま serde)。
#[derive(serde::Serialize)]
struct VerifyReport {
    /// 根 + 全子リソースが 2xx か(AI はこれで分岐)。
    ok: bool,
    url: String,
    root_status: u16,
    /// 根 HTML が参照する js / css 子リソースの検証結果。
    resources: Vec<VerifyResource>,
}

#[derive(serde::Serialize)]
struct VerifyResource {
    url: String,
    /// HTTP ステータス(接続自体の失敗は 0)。
    status: u16,
}

/// 根 HTML を取り、参照する子リソース(script src / link href)を並行検証する。
/// deploy 成功 + 根 200 でも assets 404 で白画面、を検出するのが目的(実利用フィードバック起因)。
/// ネットワーク到達不能などリクエスト自体の失敗だけ Err、業務上の NG(4xx/5xx)は報告に載せる。
async fn verify_url(c: &reqwest::Client, root: &str) -> Result<VerifyReport> {
    let resp = c
        .get(root)
        .send()
        .await
        .with_context(|| format!("{root} に接続できません(DNS / ネットワークを確認)"))?;
    let root_status = resp.status().as_u16();
    // リダイレクト後の最終 URL を基準に相対パスを解決する(`/assets/x.js` の解決先を实体に揃える)。
    let base = url::Url::parse(resp.url().as_str())?;
    let body = resp.text().await.unwrap_or_default();

    // HTML でなければ根の 2xx だけで判定(API サービスなど)。雑な判定で十分:
    // 抽出器はタグが無ければ空を返すので、誤検出しても「子リソース 0 件」に落ちるだけ。
    let refs = extract_subresources(&body);
    // 上限 50:実 SPA の参照は数件〜十数件で、これは病的な HTML(数千タグ)で無制限に
    // 並行接続を張らないための安全弁。50 件を超える分は検証しない(実ページでは起きない)。
    let checks = refs.iter().take(50).filter_map(|r| {
        // data: / mailto: 等は join で弾かれるか非 http になるので除外。
        let u = base.join(r).ok()?;
        matches!(u.scheme(), "http" | "https").then_some(u)
    });
    let results = futures_util::future::join_all(checks.map(|u| {
        let c = c.clone();
        async move {
            let status = match c.get(u.as_str()).send().await {
                Ok(r) => r.status().as_u16(),
                Err(_) => 0, // 接続不能 = 0(NG 扱い)。
            };
            VerifyResource {
                url: u.to_string(),
                status,
            }
        }
    }))
    .await;

    let ok = (200..300).contains(&root_status)
        && results.iter().all(|r| (200..300).contains(&r.status));
    Ok(VerifyReport {
        ok,
        url: root.to_string(),
        root_status,
        resources: results,
    })
}

/// HTML から `<script src=…>` / `<link href=…>` の参照先を抜く。正規表現 crate を足すほどでは
/// ないので素朴な走査:タグ開始を大文字小文字無視で探し、タグ内(`>` まで)の属性値を読む。
/// SPA の白画面検出が目的なので js / css が取れれば十分(srcset や動的 import までは追わない)。
/// `<link>` は rel でフィルタ:stylesheet / preload / modulepreload だけが描画に効く。
/// canonical / preconnect / icon 等を検証すると健全な app を誤 NG にする(codex 監査)。
fn extract_subresources(html: &str) -> Vec<String> {
    // **ASCII** 小文字化:`to_lowercase()` は非 ASCII でバイト長が変わり得て、lower 側の
    // オフセットで原文をスライスすると境界 panic になる。ASCII 変換は長さ不変(codex 監査)。
    let lower = html.to_ascii_lowercase();
    let mut out = Vec::new();
    for (tag, attr) in [("<script", "src"), ("<link", "href")] {
        let mut pos = 0;
        while let Some(i) = lower[pos..].find(tag) {
            let tag_start = pos + i;
            // タグ終端(無ければ以降を諦める — 壊れた HTML で無限ループしない)。
            let Some(end_rel) = lower[tag_start..].find('>') else {
                break;
            };
            let tag_end = tag_start + end_rel;
            let (orig_tag, lower_tag) = (&html[tag_start..tag_end], &lower[tag_start..tag_end]);
            let rendering_link = attr == "href"
                && matches!(
                    attr_value(orig_tag, lower_tag, "rel"),
                    Some("stylesheet" | "preload" | "modulepreload")
                );
            if (attr == "src" || rendering_link)
                && let Some(v) = attr_value(orig_tag, lower_tag, attr)
                && !v.is_empty()
            {
                out.push(v.to_string());
            }
            pos = tag_end + 1;
        }
    }
    out
}

/// タグ文字列から `attr="値"` / `attr='値'` の値を返す(属性名は小文字化済み lower 側で探し、
/// 値は原文 orig 側から切り出す = 大文字小文字とパーセントエンコードを保存)。
fn attr_value<'a>(orig: &'a str, lower: &str, attr: &str) -> Option<&'a str> {
    let needle = format!("{attr}=");
    let mut search = 0;
    loop {
        let i = lower[search..].find(&needle)? + search;
        // 属性名の途中一致(`data-src=` の `src=` 等)を除外:直前が英数/ハイフンなら別属性。
        if i > 0
            && lower
                .as_bytes()
                .get(i - 1)
                .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'-')
        {
            search = i + needle.len();
            continue;
        }
        let rest = &orig[i + needle.len()..];
        let quote = rest.chars().next()?;
        if quote != '"' && quote != '\'' {
            // 引用符なし属性は追わない(vite / 各種 bundler の出力は必ず引用符付き)。
            search = i + needle.len();
            continue;
        }
        let inner = &rest[1..];
        return inner.find(quote).map(|end| &inner[..end]);
    }
}

#[cfg(test)]
mod tests {
    use super::{attr_value, extract_subresources};

    #[test]
    fn extracts_script_and_link() {
        let html = r#"<!doctype html><html><head>
            <link rel="stylesheet" href="/assets/index-abc.css">
            <script type="module" src="/assets/index-def.js"></script>
            <link rel="modulepreload" href="/assets/vendor.js">
        </head><body></body></html>"#;
        let refs = extract_subresources(html);
        assert_eq!(
            refs,
            vec![
                "/assets/index-def.js",
                "/assets/index-abc.css",
                "/assets/vendor.js"
            ]
        );
    }

    #[test]
    fn skips_non_rendering_links() {
        // canonical / preconnect / icon / manifest は描画に効かない = 検証対象外
        // (外部 origin や無い favicon で健全な app を誤 NG にしない)。
        let html = r#"<link rel="canonical" href="https://example.com/">
            <link rel="preconnect" href="https://fonts.gstatic.com">
            <link rel="icon" href="/favicon.ico">
            <link rel="manifest" href="/manifest.json">
            <link href="/no-rel.css">"#;
        assert!(extract_subresources(html).is_empty());
    }

    #[test]
    fn non_ascii_before_tag_keeps_offsets() {
        // 非 ASCII(全角)がタグ前にあってもオフセットがずれない(ASCII 小文字化は長さ不変)。
        let html = "<p>日本語テキストİ</p><script src=\"/app.js\"></script>";
        assert_eq!(extract_subresources(html), vec!["/app.js"]);
    }

    #[test]
    fn ignores_data_src_and_unquoted() {
        // data-src は src ではない / 引用符なしは追わない。
        let html = r#"<script data-src="/x.js"></script><link href=/y.css>"#;
        assert!(extract_subresources(html).is_empty());
    }

    #[test]
    fn case_insensitive_tags() {
        let html = r#"<SCRIPT SRC="/A.js"></SCRIPT>"#;
        assert_eq!(extract_subresources(html), vec!["/A.js"]);
    }

    #[test]
    fn attr_value_basics() {
        assert_eq!(attr_value(r#"<script src="/a.js""#, r#"<script src="/a.js""#, "src"), Some("/a.js"));
        assert_eq!(attr_value("<script>", "<script>", "src"), None);
    }

    #[test]
    fn broken_html_terminates() {
        // タグ終端が無い壊れた HTML でも無限ループしない。
        assert!(extract_subresources("<script src=\"/a.js\"").is_empty());
    }
}
