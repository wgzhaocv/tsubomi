use anyhow::{Context, Result, bail};
use clap::Args;
use serde_json::json;
use std::io::Write;
use std::process::{Command, Stdio};

use crate::api;
use crate::commands::{
    OutputFormat, print_json, resolve_server_from, resolve_service_id, resolve_token_from,
};
use crate::config;
use tsubomi_shared::{DeployConfig, hmac_sha256, random_b64};

/// `tbm deploy`(GitHub 非依存の退路)。ローカルで build + push して自分で hook を叩く。
/// 平台は build しない(決定 #3)— build はここ(ユーザ機の docker)。CI が無い / 緊急時に使う。
#[derive(Args)]
pub struct DeployArgs {
    /// ローカルで build + push して hook を叩く(GitHub 非依存の退路。`--watch` と排他)
    #[arg(long, conflicts_with = "watch")]
    pub local: bool,
    /// GitHub 経路を一括実行:push → Actions を追跡 → デプロイ完走を待って検証まで(一条龙)。
    /// 手元が service の git repo で、`tbm service create --github` 済みが前提
    #[arg(long)]
    pub watch: bool,
    /// 対象サービスの表示名(省略時、サービスが 1 つだけならそれを使う)
    #[arg(long)]
    pub service: Option<String>,
    /// build コンテキスト(Dockerfile のあるディレクトリ)
    #[arg(long, default_value = ".")]
    pub context: String,
    /// `--watch` 全体のタイムアウト秒数(CI ビルド + デプロイ + 検証の合計予算。各待機に
    /// 残り時間を配る)。text モードの CI 追跡は `gh run watch` に従う
    #[arg(long, default_value_t = 900)]
    pub timeout: u64,
    /// デプロイ前の事前チェック(preflight)を飛ばす。既定は実行 —
    /// .env 混入 / Dockerfile の COPY 元不在 / listen ポート不一致を**警告**する(阻止はしない)
    #[arg(long)]
    pub no_preflight: bool,
    /// (--watch 用)追跡する commit の sha(`HEAD` も可。省略時 HEAD)。`verify --for-sha` と同型
    // conflicts_with も要る:requires だけだと clap は「必須が conflict で消えた」ケースを
    // 免除し、`--local --for-sha X` が黙って --for-sha を捨てて通ってしまう(review で実証)。
    #[arg(long, value_name = "SHA", requires = "watch", conflicts_with = "local")]
    pub for_sha: Option<String>,
}

pub async fn run(
    args: DeployArgs,
    server: Option<String>,
    token: Option<String>,
    out: OutputFormat,
) -> Result<()> {
    if args.watch {
        return run_watch(args, server, token, out).await;
    }
    if !args.local {
        bail!(
            "`tbm deploy` は `--local`(ローカル build)か `--watch`(GitHub 経路を追跡)のいずれかを指定してください。素の git push でも GitHub Actions が自動デプロイします"
        );
    }
    let cfg = config::load()?;
    let server_url = resolve_server_from(server.as_deref(), cfg.as_ref());
    let token = resolve_token_from(token, cfg)?;
    let json = out.is_json();
    let c = reqwest::Client::new();

    // 1. 対象 service を決める → build+hook に要る全値を取得(deploy_key / registry / hook_url / platforms)。
    let (id, svc_name) = resolve_service(&c, &server_url, &token, args.service.as_deref()).await?;
    let dc = api::deploy_config(&c, &server_url, &token, &id).await?;

    // 1.5 preflight(既定 on):よくある落とし穴を **警告**する(阻止しない)。listen ポート照合の
    // ために container_port を取る(service_get は追加 1 往復だが preflight は build 前の一度きり)。
    if !args.no_preflight {
        let port = api::service_get(&c, &server_url, &token, &id)
            .await
            .ok()
            .map(|s| s.container_port);
        run_preflight(&args.context, port);
    }

    // 2. build + push(平台は build しない。ここはユーザ機の docker buildx)。
    let git_sha = tag();
    let image_tag = format!("{}/{}:{}", dc.registry.host, dc.service_id, git_sha);
    docker_login_if_needed(&dc)?;
    let digest = buildx_push(&args.context, &dc.platforms, &image_tag)?;

    // 3. hook を署名して叩く(workflow テンプレと同じ body / 署名。生バイトに HMAC)。
    let body = serde_json::to_vec(&json!({
        "service_id": dc.service_id,
        "git_sha": git_sha,
        "image_digest": digest,
        "commit_message": commit_subject(),
        "ts": crate::commands::now_unix(),
        "nonce": random_b64(16),
    }))
    .context("hook body の組み立てに失敗")?;
    let sig = hex::encode(hmac_sha256(dc.deploy_key.as_bytes(), &body));
    api::post_deploy_hook(&c, &dc.hook_url, &sig, body).await?;

    if json {
        print_json(&json!({
            "service_id": dc.service_id,
            "image_digest": digest,
            "git_sha": git_sha,
            "status": "accepted",
        }))?;
    } else {
        eprintln!(
            "デプロイを送信しました。`tbm service verify {svc_name} --wait` で完了まで待って検証できます。"
        );
        println!("{digest}");
    }
    Ok(())
}

/// `tbm deploy --watch`:GitHub 経路を一括で回す。手元 repo で HEAD を push(未 push 時)→
/// その commit の Actions run を探して追跡 → CI 成功後、その sha のデプロイ完走を待って検証まで。
/// 平台は GitHub に触れない設計なので、ここでも `gh` を叩くのは**ユーザ自身**(create --github と同じ)。
async fn run_watch(
    args: DeployArgs,
    server: Option<String>,
    token: Option<String>,
    out: OutputFormat,
) -> Result<()> {
    let cfg = config::load()?;
    let server_url = resolve_server_from(server.as_deref(), cfg.as_ref());
    let token = resolve_token_from(token, cfg)?;
    let json = out.is_json();
    let c = reqwest::Client::new();

    // gh 前提(存在 + 認証)。無ければ --local へ誘導(create --github と同じ判断)。
    if !crate::commands::service::gh_ok() {
        bail!(
            "gh が使えません(未インストール / 未ログイン)。`gh auth login` するか、`tbm deploy --local` でローカルビルドに切り替えてください"
        );
    }

    // 対象 service(--watch は subdomain=repo 名の解決に表示名も使う)。
    let (id, svc_name) = resolve_service(&c, &server_url, &token, args.service.as_deref()).await?;

    // preflight(既定 on):CI が同じ repo をビルドするので push 前に落とし穴を警告する。
    // --watch は cwd(=repo)を対象にする(--context は --local 用)。
    if !args.no_preflight {
        let port = api::service_get(&c, &server_url, &token, &id)
            .await
            .ok()
            .map(|s| s.container_port);
        run_preflight(".", port);
    }

    // 全フェーズで 1 つの deadline を共有する(--timeout は「全体」の予算。各待機には残り時間を配る
    // ので合計が予算を超えない)。text モードの `gh run watch` だけは gh 自身が時間管理する例外。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(args.timeout);
    let remaining = || deadline.saturating_duration_since(std::time::Instant::now()).as_secs();

    // 1) 追跡対象の sha(既定 HEAD、--for-sha で明示)と上流を確認 → 未 push なら push。
    let head = crate::commands::git_head_sha()?;
    let sha = match args.for_sha.as_deref() {
        None | Some("HEAD") => head.clone(),
        // 短縮 sha は full に解決(gh run list --commit は full sha 前提)。branch/tag は
        // verify --for-sha と同じく受けない(sha か HEAD に絞る)。`--verify ^{commit}` で
        // **オブジェクトの実在まで**確認する(素の rev-parse は 40 桁 hex をノーチェックで
        // echo し返すため、typo sha が全 timeout を空費してから「run が現れない」と誤診する)。
        Some(s) => {
            if !crate::commands::looks_like_sha(s) {
                bail!(
                    "--for-sha はコミット sha か `HEAD` を指定してください(branch/tag 名は不可): {s}"
                );
            }
            git_out(&["rev-parse", "--verify", "--quiet", &format!("{s}^{{commit}}")])
                .with_context(|| {
                    format!("sha '{s}' が手元 repo に見つかりません(fetch 済みか・typo を確認)")
                })?
        }
    };
    let upstream = match git_upstream() {
        Some(u) => u,
        None => {
            // 初回(追跡ブランチ未設定)。remote は**実在する名前**で選ぶ:`service create
            // --github` が作る remote は `origin` ではなく `tsubomi`(origin 固定の案内は
            // そのまま打つと失敗する — 実利用フィードバック)。選べたら -u 付き push まで
            // 自動でやる(次回からは通常の push で済む)。
            let branch = git_out(&["rev-parse", "--abbrev-ref", "HEAD"]).context(
                "現在のブランチを特定できません(git リポジトリの中で実行してください)",
            )?;
            if branch == "HEAD" {
                bail!(
                    "detached HEAD では --watch を使えません。ブランチに checkout してから再実行してください"
                );
            }
            let remote = pick_remote().context(
                "追跡ブランチ(upstream)が無く、push 先の remote も特定できません。`git remote -v` で確認し、`git push -u <remote> <branch>` で一度設定してください",
            )?;
            eprintln!("追跡ブランチ(upstream)未設定 → `git push -u {remote} {branch}` を実行します…");
            run_git(&["push", "-u", &remote, &branch])?;
            format!("{remote}/{branch}")
        }
    };
    if sha == head {
        if git_has_unpushed(&upstream) {
            if !json {
                eprintln!("未 push のコミットがあります。push します…");
            }
            run_git(&["push"])?;
        }
    } else {
        // 過去 commit を追うモード:ここで push すると HEAD の未 push WIP まで巻き込み、
        // 新しい CI/デプロイが対象 sha を追い越す(検証対象が変わる)。push はせず、対象が
        // upstream に**未着なら**手動 push を案内して止める。
        if !git_is_ancestor(&sha, &upstream) {
            bail!(
                "--for-sha {} は upstream({upstream})に含まれていません。先にその commit を push してから再実行してください",
                crate::commands::service::short_sha(&sha)
            );
        }
    }

    // 2) この commit の Actions run を探す(push 直後は現れるまで数秒かかる)。gh の対象 repo は
    // -R で明示する(tsubomi + origin の複数 remote だと既定解決が非対話ではエラーになる)。
    let repo = repo_slug();
    if !json {
        eprintln!(
            "commit {} の GitHub Actions run を待っています…",
            crate::commands::service::short_sha(&sha)
        );
    }
    let run = wait_for_run(&sha, repo.as_deref(), remaining()).await?;
    // run URL は追跡先として常に出す(捕捉側が持てるよう stderr へ)。
    eprintln!("Actions run: {}", run.url);

    // 3) run の完了を待つ。text は `gh run watch`(ログを継承、時間管理は gh)、json は静かに輪詢。
    if json {
        wait_for_run_conclusion(&run.id, repo.as_deref(), remaining()).await?;
    } else {
        // gh run watch は run 完了で成功、CI 失敗時は --exit-status で非零。
        let mut a = vec!["run", "watch", &run.id, "--exit-status"];
        if let Some(r) = repo.as_deref() {
            a.extend(["-R", r]);
        }
        crate::commands::service::run_gh(&a)
            .context("CI が失敗しました(上のログを確認)。修正して再度 push してください")?;
    }

    // 4) CI 成功 = hook 到達済み。この sha のデプロイ完走を待って検証(端到端)。
    if !json {
        eprintln!("CI 成功。デプロイ完走を待って検証します…");
    }
    crate::commands::service::run_verify(
        &c,
        &server_url,
        &token,
        &svc_name,
        true,
        Some(&sha),
        remaining(),
        json,
    )
    .await
}

/// GitHub Actions の run(id と URL)。
struct GhRun {
    id: String,
    url: String,
}

/// commit sha の run が現れるまで輪詢する(push 直後は登録に数秒かかる)。timeout 超過はエラー。
async fn wait_for_run(sha: &str, repo: Option<&str>, timeout_secs: u64) -> Result<GhRun> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    loop {
        // 最新の該当 run 1 件(databaseId と url)。まだ無ければ空配列。
        let mut a = vec![
            "run",
            "list",
            "--commit",
            sha,
            "--limit",
            "1",
            "--json",
            "databaseId,url",
        ];
        if let Some(r) = repo {
            a.extend(["-R", r]);
        }
        let out = crate::commands::service::gh_capture(&a)?;
        if let Some(run) = parse_first_run(&out) {
            return Ok(run);
        }
        if std::time::Instant::now() >= deadline {
            bail!(
                "commit {} の Actions run が現れませんでした。workflow が設定済みか(`tbm service create --github`)、push が済んでいるか確認してください",
                crate::commands::service::short_sha(sha)
            );
        }
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
}

/// json モードで run の結論を輪詢する(text は `gh run watch` に任せる)。CI 失敗は非零で bail。
async fn wait_for_run_conclusion(
    run_id: &str,
    repo: Option<&str>,
    timeout_secs: u64,
) -> Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    loop {
        let mut a = vec!["run", "view", run_id, "--json", "status,conclusion"];
        if let Some(r) = repo {
            a.extend(["-R", r]);
        }
        let out = crate::commands::service::gh_capture(&a)?;
        let v: serde_json::Value = serde_json::from_str(&out).unwrap_or_default();
        if v.get("status").and_then(|s| s.as_str()) == Some("completed") {
            return match v.get("conclusion").and_then(|c| c.as_str()) {
                Some("success") => Ok(()),
                other => bail!(
                    "CI が失敗しました(conclusion={})。`gh run view {run_id} --log-failed` で確認してください",
                    other.unwrap_or("unknown")
                ),
            };
        }
        if std::time::Instant::now() >= deadline {
            bail!("CI が {timeout_secs} 秒以内に完了しませんでした(`gh run view {run_id}` で確認)");
        }
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
}

/// `gh run list --json databaseId,url` の先頭を GhRun にする(空配列 / 解析失敗は None)。
fn parse_first_run(json: &str) -> Option<GhRun> {
    let arr = serde_json::from_str::<serde_json::Value>(json).ok()?;
    let first = arr.as_array()?.first()?;
    let id = first.get("databaseId")?.as_i64()?;
    let url = first.get("url")?.as_str()?.to_string();
    Some(GhRun {
        id: id.to_string(),
        url,
    })
}

// ===== preflight(デプロイ前の事前チェック。**警告のみ**・阻止しない) =====

/// よくある落とし穴を build/push の前に警告する(高信号のものだけ)。判定できない項目は黙る
/// (誤検知で AI/人を惑わせない)。すべて stderr。`--no-preflight` で無効化。
/// - .env 混入:git 追跡下に .env* があり、Dockerfile が `COPY .` で .dockerignore が除外しない
/// - Dockerfile の COPY/ADD 元(`--from=` 以外)が context に無い(タイプミス / 追加し忘れ)
/// - `EXPOSE` が service の container_port と食い違う(502 の典型)
fn run_preflight(context: &str, container_port: Option<i32>) {
    let ctx = std::path::Path::new(context);
    let mut warns: Vec<String> = Vec::new();

    let dockerfile = std::fs::read_to_string(ctx.join("Dockerfile")).ok();
    let dockerignore = std::fs::read_to_string(ctx.join(".dockerignore")).unwrap_or_default();

    // (1).env 混入。git 追跡下の .env*(.env.example は除く)を拾う。
    let tracked_env: Vec<String> = git_out(&["ls-files", "*.env", ".env*"])
        .unwrap_or_default()
        .lines()
        .filter(|l| {
            let base = l.rsplit('/').next().unwrap_or(l);
            base.starts_with(".env") && base != ".env.example" && base != ".env.sample"
        })
        .map(str::to_string)
        .collect();
    if !tracked_env.is_empty() {
        warns.push(format!(
            ".env らしきファイルが git 追跡下にあります({})。秘密が commit / イメージに載る恐れ — .gitignore と .dockerignore で除外してください",
            tracked_env.join(", ")
        ));
    }
    // Dockerfile が `COPY .`(context 丸ごと)で .dockerignore が .env を除外しないと、追跡外の
    // ローカル .env もイメージに焼き込まれる。
    if let Some(df) = &dockerfile
        && copies_whole_context(df)
        && ctx.join(".env").exists()
        && !dockerignore_excludes_env(&dockerignore)
    {
        warns.push(
            "ローカルの .env が `COPY .` でイメージに焼き込まれます(.dockerignore に `.env` を追加してください)".into(),
        );
    }

    // (2)Dockerfile の COPY/ADD 元が context に無い(ocr_input を Dockerfile に足し忘れた類)。
    if let Some(df) = &dockerfile {
        for src in copy_sources(df) {
            // glob / 変数展開 / リモート URL(`ADD https://…`)は判定しない(誤検知回避)。
            // 素直な相対パスだけ存在確認する。
            if src.contains('*') || src.contains('$') || src.contains("://") || src == "." {
                continue;
            }
            if !ctx.join(&src).exists() {
                warns.push(format!(
                    "Dockerfile の COPY/ADD 元 '{src}' が context({context})に見つかりません(パス / 追加し忘れを確認)"
                ));
            }
        }
    }

    // (3)EXPOSE と container_port の食い違い。**弱い信号**:tsubomi は各コンテナに
    // `PORT=<container_port>` を注入し、アプリは `$PORT` を listen する契約なので、`$PORT` を
    // 読むアプリは EXPOSE が何であれ正常(EXPOSE は基底イメージ由来 / 単なる文書のことが多い)。
    // 固定ポート listen のアプリだけ問題になるので、断定せず気付きを促す文言にする。EXPOSE 無しは黙る。
    if let (Some(df), Some(port)) = (&dockerfile, container_port)
        && let Some(exposed) = first_expose(df)
        && exposed != port
    {
        warns.push(format!(
            "Dockerfile の EXPOSE {exposed} が service の container_port {port} と違います。`$PORT`(={port})を listen していれば問題ありませんが、{exposed} 固定で listen していると 502 になります"
        ));
    }

    for w in &warns {
        eprintln!("⚠ preflight: {w}");
    }
}

/// Dockerfile に context 丸ごとの `COPY .`(= `COPY . <dst>` / `COPY --chown=… . <dst>`)があるか。
fn copies_whole_context(dockerfile: &str) -> bool {
    dockerfile.lines().any(|l| {
        let l = l.trim();
        if !l.to_uppercase().starts_with("COPY ") || l.contains("--from=") {
            return false;
        }
        // 先頭の COPY と --flag(--chown 等)を除いた最初の src トークンが "." か "./"。
        let first_src = l
            .split_whitespace()
            .skip(1)
            .find(|t| !t.starts_with("--"));
        matches!(first_src, Some(".") | Some("./"))
    })
}

/// .dockerignore が **`.env` 自体を** 除外するか。`.env.example` のような行に釣られない
/// (それは `.env.example` だけを無視し `.env` は依然 COPY される = 誤って警告を抑制しない)。
/// リテラル `.env` に実際にマッチする代表的パターンだけを真とする。
fn dockerignore_excludes_env(dockerignore: &str) -> bool {
    dockerignore
        .lines()
        .map(str::trim)
        .any(|l| matches!(l, ".env" | ".env*" | "*.env" | "**/.env" | ".env**"))
}

/// Dockerfile の COPY/ADD の **src トークン**(最後の dst を除く)を集める。`--from=` 付き
/// (ステージ間コピー)は context 外なので除外。`--flag` は飛ばす。
fn copy_sources(dockerfile: &str) -> Vec<String> {
    let mut out = Vec::new();
    for l in dockerfile.lines() {
        let l = l.trim();
        let up = l.to_uppercase();
        if !(up.starts_with("COPY ") || up.starts_with("ADD ")) || l.contains("--from=") {
            continue;
        }
        // 先頭の COPY/ADD と --flag を除いた引数列。最後は dst なので落とす。
        let args: Vec<&str> = l
            .split_whitespace()
            .skip(1)
            .filter(|t| !t.starts_with("--"))
            .collect();
        if args.len() >= 2 {
            for src in &args[..args.len() - 1] {
                out.push(src.trim_matches('"').to_string());
            }
        }
    }
    out
}

/// Dockerfile の最初の `EXPOSE <port>`(`EXPOSE 8080/tcp` の /tcp も落とす)。
fn first_expose(dockerfile: &str) -> Option<i32> {
    for l in dockerfile.lines() {
        let l = l.trim();
        if l.to_uppercase().starts_with("EXPOSE ") {
            let tok = l[7..].split_whitespace().next()?;
            let num = tok.split('/').next()?;
            if let Ok(p) = num.parse::<i32>() {
                return Some(p);
            }
        }
    }
    None
}

/// git を実行して trimmed stdout を返す(成功かつ非空のときだけ Some)。tag / commit_subject /
/// upstream / has-unpushed が共有する「git を叩いて出力を拾う」定型。
fn git_out(args: &[&str]) -> Option<String> {
    Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 現ブランチの upstream(`git rev-parse --abbrev-ref @{u}`)。未設定は None。
fn git_upstream() -> Option<String> {
    git_out(&["rev-parse", "--abbrev-ref", "@{u}"])
}

/// upstream に未 push のコミットがあるか(`git rev-list <upstream>..HEAD` が非空 = Some)。
fn git_has_unpushed(upstream: &str) -> bool {
    git_out(&["rev-list", &format!("{upstream}..HEAD")]).is_some()
}

/// `<sha>` が `<upstream>` に含まれる(= push 済み)か。--for-sha で過去 commit を追うとき、
/// 誤って HEAD を push しないための判定。
fn git_is_ancestor(sha: &str, upstream: &str) -> bool {
    Command::new("git")
        .args(["merge-base", "--is-ancestor", sha, upstream])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// git を出力継承で実行する(push / push -u が共有)。失敗はエラー。
fn run_git(args: &[&str]) -> Result<()> {
    let status = Command::new("git")
        .args(args)
        .status()
        .with_context(|| format!("git {} の実行に失敗しました", args.join(" ")))?;
    if !status.success() {
        bail!(
            "git {} が失敗しました。手元の git 状態を確認してください",
            args.join(" ")
        );
    }
    Ok(())
}

/// upstream 未設定時の push 先 remote。**git 自身の push 先設定を最優先**
/// (`branch.<名>.pushRemote` → `remote.pushDefault` — CLI が代理 push する以上、ユーザの
/// 明示設定を無視しない)。無ければ `tsubomi`(`service create --github` が作る名)→ `origin` →
/// 唯一の remote。複数あって決められない場合は None(勝手に選ばない — 誤 push 防止)。
fn pick_remote() -> Option<String> {
    if let Some(branch) = git_out(&["rev-parse", "--abbrev-ref", "HEAD"])
        && let Some(r) = git_out(&["config", "--get", &format!("branch.{branch}.pushRemote")])
    {
        return Some(r);
    }
    if let Some(r) = git_out(&["config", "--get", "remote.pushDefault"]) {
        return Some(r);
    }
    let out = git_out(&["remote"])?;
    let remotes: Vec<&str> = out.lines().collect();
    for pref in ["tsubomi", "origin"] {
        if remotes.contains(&pref) {
            return Some(pref.to_string());
        }
    }
    match remotes.as_slice() {
        [only] => Some((*only).to_string()),
        _ => None,
    }
}

/// 現 repo の GitHub `owner/repo`(gh の `-R` に渡す形)。remote の選好は pick_remote と同一
/// (方針を 1 箇所に:push 先と gh の対象 repo がズレない)。GitHub 以外 / repo 外は None。
fn repo_slug() -> Option<String> {
    pick_remote()
        .and_then(|r| git_out(&["remote", "get-url", &r]))
        .and_then(|u| gh_repo_from_url(&u))
}

/// git remote URL(https / ssh)→ gh の `-R` に渡す `owner/repo` 形。対象外の URL は None。
fn gh_repo_from_url(url: &str) -> Option<String> {
    let s = url.trim().trim_end_matches(".git");
    let tail = s
        .strip_prefix("git@github.com:")
        .or_else(|| s.split("github.com/").nth(1))?;
    let mut it = tail.split('/');
    let (owner, repo) = (it.next()?, it.next()?);
    (!owner.is_empty() && !repo.is_empty()).then(|| format!("{owner}/{repo}"))
}

/// 現在の git repo から対象 service の id を推断する。単一の真源は `service create --github` が
/// repo variable に焼いた `TSUBOMI_SERVICE_ID`(`gh variable get` で読む)。gh 不在 / repo 外 /
/// variable 無しは None — あくまで補助で、失敗したら従来の `--service` 指定エラーに落ちる。
fn infer_service_id_from_repo() -> Option<String> {
    if !crate::commands::service::gh_ok() {
        return None;
    }
    // -R を明示する(tsubomi + origin の複数 remote だと gh の既定 repo 解決が非対話では
    // エラーになる)。repo を特定できなければ gh の既定解決に任せる。
    let out = match repo_slug().as_deref() {
        Some(r) => {
            crate::commands::service::gh_capture(&["variable", "get", "TSUBOMI_SERVICE_ID", "-R", r])
        }
        None => crate::commands::service::gh_capture(&["variable", "get", "TSUBOMI_SERVICE_ID"]),
    }
    .ok()?;
    let id = out.trim().to_string();
    (!id.is_empty()).then_some(id)
}

/// --service 名で解決、省略時はサービスが 1 つだけならそれ(複数 / 0 はエラー)。
/// 表示名も返す(成功文案の `tbm service verify <名前> --wait` 指引に使う)。
/// 名前指定時は共有の `resolve_service_id` に委譲(not_found の文言・code を 1 箇所に保つ)。
async fn resolve_service(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    name: Option<&str>,
) -> Result<(String, String)> {
    match name {
        Some(n) => {
            let id = resolve_service_id(c, server_url, token, n).await?;
            Ok((id, n.to_string()))
        }
        None => {
            let svcs = api::service_list(c, server_url, token).await?;
            match svcs.as_slice() {
                [only] => Ok((only.id.to_string(), only.display_name.clone())),
                [] => bail!(
                    "サービスがありません。先に `tbm service create <名前>` を実行してください"
                ),
                _ => {
                    // 複数あっても、手元 repo の TSUBOMI_SERVICE_ID variable が一意に指すなら
                    // それを使う(deploy は対象 repo の中で打つのが普通 — 毎回 --service を
                    // 要求しない。実利用フィードバック)。
                    if let Some(inferred) = infer_service_id_from_repo()
                        && let Some(svc) = svcs.iter().find(|s| s.id.to_string() == inferred)
                    {
                        eprintln!(
                            "対象 service を repo の TSUBOMI_SERVICE_ID から自動推断:{}",
                            svc.display_name
                        );
                        return Ok((svc.id.to_string(), svc.display_name.clone()));
                    }
                    bail!(
                        "サービスが複数あります。`--service <名前>` で対象を指定してください(service の repo 内で gh ログイン済みなら、TSUBOMI_SERVICE_ID variable から自動推断します)"
                    )
                }
            }
        }
    }
}

/// 認証付き registry(本番)なら docker login する。loopback の dev registry(認証なし)は不要。
fn docker_login_if_needed(dc: &DeployConfig) -> Result<()> {
    let host = &dc.registry.host;
    if host.starts_with("127.0.0.1") || host.starts_with("localhost") {
        return Ok(());
    }
    eprintln!("$ docker login {host} -u {}", dc.registry.user);
    let mut child = Command::new("docker")
        .args(["login", host, "-u", &dc.registry.user, "--password-stdin"])
        .stdin(Stdio::piped())
        .spawn()
        .context("docker login の起動に失敗しました(docker はありますか?)")?;
    child
        .stdin
        .take()
        .context("docker の stdin を開けません")?
        .write_all(dc.registry.pass.as_bytes())
        .context("docker login へのパスワード書き込みに失敗")?;
    if !child.wait()?.success() {
        bail!("docker login が失敗しました");
    }
    Ok(())
}

/// `docker buildx build --push` して image digest を返す(metadata-file から取得)。
fn buildx_push(context: &str, platforms: &str, image_tag: &str) -> Result<String> {
    // 一意なメタデータファイル(PID + 乱数。PID 再利用での古いファイル誤読を避ける)。
    let meta = std::env::temp_dir().join(format!(
        "tbm-meta-{}-{}.json",
        std::process::id(),
        random_b64(8)
    ));
    eprintln!("$ docker buildx build --platform {platforms} --push -t {image_tag} {context}");
    // stderr は**流しながら**読む(進捗はそのまま見せつつ、失敗原因を拾って人話に翻訳する)。
    // 特に registry push の 413 は「Cloudflare 経由の単層上限」で、素の 413 だけでは原因に
    // たどり着けず時間を溶かす(実利用フィードバック #3)。
    let mut child = Command::new("docker")
        .args([
            "buildx",
            "build",
            "--platform",
            platforms,
            "--push",
            "-t",
            image_tag,
            "--metadata-file",
        ])
        .arg(&meta)
        .arg(context)
        .stderr(Stdio::piped())
        .spawn()
        .context("docker buildx の実行に失敗しました(docker / buildx はありますか?)")?;
    let mut saw_413 = false;
    if let Some(stderr) = child.stderr.take() {
        use std::io::BufRead;
        for line in std::io::BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            if line.contains("413") || line.contains("Payload Too Large") {
                saw_413 = true;
            }
            eprintln!("{line}");
        }
    }
    let status = child.wait().context("docker buildx の待機に失敗しました")?;
    if !status.success() {
        let _ = std::fs::remove_file(&meta);
        if saw_413 {
            bail!(
                "registry への push が 413(Payload Too Large)で拒否されました。Cloudflare 経由の \
                 registry は**イメージ 1 層あたり圧縮後 ≈100MB** が上限です(CF の request body 制限。\
                 registry 側では変えられません)。対処:Dockerfile の大きな RUN/COPY を分割して層を \
                 小さくする / slim・alpine 系の基底イメージにする / マルチステージビルドでビルド中間物を\
                 落とす。GitHub Actions 経由の push で同じ 413 が出る場合も原因は同じです"
            );
        }
        bail!("docker buildx build が失敗しました(上の docker 出力を確認してください)");
    }
    let text = std::fs::read_to_string(&meta).context("buildx メタデータを読めません")?;
    let _ = std::fs::remove_file(&meta);
    let v: serde_json::Value =
        serde_json::from_str(&text).context("buildx メタデータが不正な JSON です")?;
    let digest = v
        .get("containerimage.digest")
        .and_then(|d| d.as_str())
        .context("buildx メタデータに containerimage.digest がありません")?;
    // 署名・送信の前に digest 形式を検証(壊れたメタデータで誤った image を deploy しない)。
    if !is_sha256_digest(digest) {
        bail!("buildx が返した digest の形式が不正です: {digest}");
    }
    Ok(digest.to_string())
}

/// `sha256:` + 64 桁 16 進かどうか(server 側の hook 検証と同じ契約)。
fn is_sha256_digest(s: &str) -> bool {
    s.strip_prefix("sha256:")
        .is_some_and(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// git の短い SHA(repo 外なら "local")。deploy 履歴の表示用。
fn tag() -> String {
    git_out(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "local".to_string())
}

/// commit の件名(`git log -1 --pretty=%s`)。deploy 履歴の見出し。repo 外 / 失敗は None
/// (server 側 `#[serde(default)]` Option が null/欠落を吸収する)。
fn commit_subject() -> Option<String> {
    git_out(&["log", "-1", "--pretty=%s"])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_first_run_extracts_id_and_url() {
        let json = r#"[{"databaseId":123456789,"url":"https://github.com/o/r/actions/runs/123456789"}]"#;
        let run = parse_first_run(json).expect("should parse");
        assert_eq!(run.id, "123456789");
        assert_eq!(run.url, "https://github.com/o/r/actions/runs/123456789");
    }

    #[test]
    fn parse_first_run_empty_is_none() {
        assert!(parse_first_run("[]").is_none());
        assert!(parse_first_run("not json").is_none());
    }

    #[test]
    fn copies_whole_context_detects_copy_dot() {
        assert!(copies_whole_context("FROM x\nCOPY . /app\n"));
        assert!(copies_whole_context("copy ./ /app")); // 小文字 + ./
        assert!(copies_whole_context("COPY --chown=node:node . /app")); // --flag を跨いで検出
        assert!(!copies_whole_context("COPY app /app")); // 具体パスは対象外
        assert!(!copies_whole_context("COPY --from=build . /app")); // ステージ間は除外
    }

    #[test]
    fn copy_sources_skips_from_and_flags() {
        let df = "FROM x\nCOPY app pkg ./dst\nCOPY --from=b /a /b\nADD file.tar /out\n";
        let srcs = copy_sources(df);
        // COPY app pkg ./dst → src = app, pkg(最後の dst は除外)。--from= は丸ごと除外。
        assert_eq!(srcs, vec!["app", "pkg", "file.tar"]);
    }

    #[test]
    fn first_expose_parses_port_and_proto() {
        assert_eq!(first_expose("FROM x\nEXPOSE 8080\n"), Some(8080));
        assert_eq!(first_expose("EXPOSE 5432/tcp"), Some(5432));
        assert_eq!(first_expose("FROM x\n"), None);
    }

    #[test]
    fn gh_repo_from_url_parses_ssh_and_https() {
        assert_eq!(
            gh_repo_from_url("git@github.com:me/app.git").as_deref(),
            Some("me/app")
        );
        assert_eq!(
            gh_repo_from_url("https://github.com/me/app.git").as_deref(),
            Some("me/app")
        );
        assert_eq!(
            gh_repo_from_url("https://github.com/me/app").as_deref(),
            Some("me/app")
        );
        assert_eq!(gh_repo_from_url("https://gitlab.com/me/app"), None);
        assert_eq!(gh_repo_from_url("git@github.com:"), None);
    }

    #[test]
    fn dockerignore_env_detection() {
        assert!(dockerignore_excludes_env(".env\nnode_modules\n"));
        assert!(dockerignore_excludes_env("*.env"));
        assert!(dockerignore_excludes_env(".env*"));
        assert!(!dockerignore_excludes_env("node_modules\ndist\n"));
        // .env.example だけ無視しても .env は COPY される = 除外とみなさない(誤抑制しない)。
        assert!(!dockerignore_excludes_env(".env.example\n"));
    }
}
