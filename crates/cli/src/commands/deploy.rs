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
        "ts": now_unix(),
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
    let (_id, svc_name) = resolve_service(&c, &server_url, &token, args.service.as_deref()).await?;

    // 全フェーズで 1 つの deadline を共有する(--timeout は「全体」の予算。各待機には残り時間を配る
    // ので合計が予算を超えない)。text モードの `gh run watch` だけは gh 自身が時間管理する例外。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(args.timeout);
    let remaining = || deadline.saturating_duration_since(std::time::Instant::now()).as_secs();

    // 1) HEAD(full sha)と上流を確認 → 未 push なら push。
    let sha = crate::commands::git_head_sha()?;
    let upstream = git_upstream()
        .context("追跡ブランチ(upstream)がありません。`git push -u origin <branch>` で一度設定してください")?;
    if git_has_unpushed(&upstream) {
        if !json {
            eprintln!("未 push のコミットがあります。push します…");
        }
        git_push()?;
    }

    // 2) この commit の Actions run を探す(push 直後は現れるまで数秒かかる)。
    if !json {
        eprintln!(
            "commit {} の GitHub Actions run を待っています…",
            crate::commands::service::short_sha(&sha)
        );
    }
    let run = wait_for_run(&sha, remaining()).await?;
    // run URL は追跡先として常に出す(捕捉側が持てるよう stderr へ)。
    eprintln!("Actions run: {}", run.url);

    // 3) run の完了を待つ。text は `gh run watch`(ログを継承、時間管理は gh)、json は静かに輪詢。
    if json {
        wait_for_run_conclusion(&run.id, remaining()).await?;
    } else {
        // gh run watch は run 完了で成功、CI 失敗時は --exit-status で非零。
        crate::commands::service::run_gh(&["run", "watch", &run.id, "--exit-status"])
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
async fn wait_for_run(sha: &str, timeout_secs: u64) -> Result<GhRun> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    loop {
        // 最新の該当 run 1 件(databaseId と url)。まだ無ければ空配列。
        let out = crate::commands::service::gh_capture(&[
            "run",
            "list",
            "--commit",
            sha,
            "--limit",
            "1",
            "--json",
            "databaseId,url",
        ])?;
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
async fn wait_for_run_conclusion(run_id: &str, timeout_secs: u64) -> Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    loop {
        let out = crate::commands::service::gh_capture(&[
            "run",
            "view",
            run_id,
            "--json",
            "status,conclusion",
        ])?;
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

/// `git push`(出力は継承)。失敗はエラー。
fn git_push() -> Result<()> {
    let status = Command::new("git")
        .arg("push")
        .status()
        .context("git push の実行に失敗しました")?;
    if !status.success() {
        bail!("git push が失敗しました。手元の git 状態を確認してください");
    }
    Ok(())
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
                _ => bail!("サービスが複数あります。`--service <名前>` で対象を指定してください"),
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

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
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
}
