//! per-user の registry 資格情報(`ensure_account`)。
//!
//! ユーザ app のイメージ push 先 registry のアカウントを **ユーザ単位で 1 つ**持つ
//! (per-service ではない — digest ピン留めで per-repo ACL 不要。決定 #3 / §11-D)。
//! service create のたびに同じ creds を返すので、同じユーザの複数 service が同じ
//! GitHub Secret を共有できる(冪等)。
//!
//! 平台は password の **原文**を GitHub Secret 用に返す必要があるので、復元可能に
//! 暗号化して持つ(crypto.rs。ハッシュにできる session / cli_token とは別)。
//!
//! registry の htpasswd ファイルへの同期(bcrypt 行の追記 + registry への SIGHUP
//! リロード)は **prod-infra スライス**で足す:認証付き registry が立ってから実機
//! 検証する(dev の registry は認証なし)。本モジュールはアカウントの永続化と creds
//! 返却までを担う。

use anyhow::{Context, anyhow};

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use tsubomi_shared::{RegistryCreds, random_b64};
use uuid::Uuid;

/// registry password の乱数バイト数(base64url で ≈32 字)。
const PASSWORD_BYTES: usize = 24;

/// registry コンテナ名。dev / prod とも compose の `container_name` で `tsubomi-registry` に固定。
/// GC コマンド(下記 [`garbage_collect`])と config パスは stock の `registry:2` イメージ前提 ——
/// 三者まとめて「固定された配備形」として const に置く(片方だけ env 可変にしない)。
const REGISTRY_CONTAINER: &str = "tsubomi-registry";

/// ユーザの registry アカウントを取得、無ければ作る(冪等)。返すのは host を含む
/// 完全な creds(password は平文)。同時 create にも強い:`ON CONFLICT DO NOTHING`
/// で 2 重挿入を避け、最後に確定行を読み直してから復号する。
pub async fn ensure_account(state: &AppState, user_id: Uuid) -> AppResult<RegistryCreds> {
    if let Some(creds) = load(state, user_id).await? {
        return Ok(creds);
    }

    // username は user_id 由来で安定 & 衝突しない。password は乱数 → 暗号化して保存。
    let username = format!("u-{}", user_id.simple());
    let password = random_b64(PASSWORD_BYTES);
    let password_enc = state.crypto.encrypt(&password)?;
    sqlx::query(
        "INSERT INTO registry_accounts (user_id, username, password_enc)
              VALUES ($1, $2, $3) ON CONFLICT (user_id) DO NOTHING",
    )
    .bind(user_id)
    .bind(&username)
    .bind(&password_enc)
    .execute(&state.db)
    .await?;

    // 自分が挿入したか、同時実行が先んじたかに依らず確定値を読み直す
    // (DO NOTHING で自分の INSERT が無視された場合でも正しい creds を返す)。
    let creds = load(state, user_id)
        .await?
        .ok_or_else(|| AppError::Other(anyhow::anyhow!("registry アカウントの作成に失敗")))?;
    // 新規アカウント経路(既存は冒頭で早期 return 済み)→ traefik の registry basicAuth を
    // 更新する(本番のみ実質動作。file provider がホットリロード)。
    sync_traefik(state).await;
    Ok(creds)
}

// ===== 本番 registry の push 入口(traefik basicAuth)=====

/// 本番(tls)で registry の公網 push 入口を traefik に出す:`registry.<domain>` → registry:5000、
/// basicAuth(全 registry_accounts を bcrypt した inline users)、LE。registry コンテナ自体は
/// **無認証**(ループバック :5000 のまま — 平台の pull はそのまま通る)。認証は traefik 層だけに付ける。
/// IP 許可リスト middleware は付けない(決定 #4:registry は免除)。dev(tls=false)は何もしない。
/// 起動時 + `ensure_account` の新規時に呼ぶ(traefik file provider がホットリロード、SIGHUP 不要)。
pub async fn sync_traefik(state: &AppState) {
    // prod(push 先が公網の別ホスト)でのみ入口を書く。dev / 単機無認証では何もしない。
    // TLS の有無(traefik 終端 / 上流終端)とは独立 — tunnel(tls=false)でも入口は要る。
    if !state.config.registry_ingress() {
        // tls=true なのに push==pull は本番の設定漏れ(REGISTRY_PUSH 未設定)。push 入口が
        // 書かれず CI の docker login が 404 で黙って割れるので警告する。
        if state.config.tls {
            tracing::warn!(
                "TSUBOMI_TLS=true だが REGISTRY_PUSH==PULL — registry push 入口を書きません。\
                 TSUBOMI_REGISTRY_PUSH=registry.<域名> を設定してください"
            );
        }
        return;
    }
    // best-effort:失敗しても account 行は DB にあり、起動時 / 次の create で再同期して収束する
    // (現実の registry.yml は file provider が即ホットリロード。並行 create は稀で自己修復する)。
    if let Err(e) = sync_traefik_inner(state).await {
        tracing::error!(error = ?e, "registry の traefik 入口同期に失敗 — 次回 create / 再起動で収束");
    }
}

async fn sync_traefik_inner(state: &AppState) -> AppResult<()> {
    // 全アカウントを復号 → bcrypt(basicAuth は一方向。GitHub Secret 用の平文ラインとは別物)。
    // ※ bcrypt(cost 12)は 1 件 ≈数百 ms。アカウント数 N に対し毎回 N 回(create / 起動ごと)。
    //   社内少数ユーザでは許容。ユーザが増えたら hash を DB にキャッシュして差分だけ算出する。
    let rows: Vec<(String, Vec<u8>)> =
        sqlx::query_as("SELECT username, password_enc FROM registry_accounts")
            .fetch_all(&state.db)
            .await?;
    let mut users: Vec<String> = Vec::with_capacity(rows.len());
    for (user, pass_enc) in rows {
        let pass = state.crypto.decrypt(&pass_enc)?;
        let hash = bcrypt::hash(&pass, bcrypt::DEFAULT_COST)
            .map_err(|e| AppError::Other(anyhow::anyhow!("bcrypt に失敗: {e}")))?;
        users.push(format!("{user}:{hash}"));
    }

    let target = state.config.traefik_dynamic_dir.join("registry.yml");
    let doc = render(
        state.config.registry_host(),
        state.config.registry_direct_host(),
        &users,
        state.config.tls,
    );
    crate::services::route::write_atomic(&target, &doc)?;
    tracing::info!(accounts = users.len(), "registry の traefik 入口を同期した");
    Ok(())
}

/// traefik 動的設定(registry router + basicAuth middleware + service)を組み立てる。
/// `push_host` = 公網の push ホスト(`registry_push`、例 registry.<域名>)。`tls`=true なら traefik 終端
/// (websecure + LE)、false なら上流終端(web、HTTP。CF Tunnel / 逆代理の後ろ)。
/// `direct_host` = CF を経由しない直連入口(任意。VPS sni-gate + frp 経由で届く。entrypoint
/// `registrydirect` + LE **DNS-01**(`ledns`)で traefik が TLS 終端 — CF の 100MB 上限を回避する
/// push 専用経路。compose.prod.registry-direct.yml とセット。doc/paas-registry-direct-design.md)。
/// bcrypt ハッシュは `$`/`.`/`/` のみ(引用符・バックスラッシュ無し)なので二重引用符で安全に包める。
/// file provider なので compose の `$$` 二重化は不要。
/// **users 空(アカウント未作成)→ router を書かない**:push 入口は 404 = push 不可(fail-closed)。
/// 空の basicAuth `users` が traefik で allow-all に倒れて push 入口が開く事故を避ける。
fn render(push_host: &str, direct_host: Option<&str>, users: &[String], tls: bool) -> String {
    use crate::services::route::{entrypoint, push_tls_block};
    let mut s = String::new();
    s.push_str("# 平台が自動生成(services/registry.rs)。手で編集しない。\n");
    if users.is_empty() {
        s.push_str("# (registry アカウント未作成 — push 入口は未公開 = fail-closed)\n");
        return s;
    }
    s.push_str("http:\n");
    s.push_str("  routers:\n");
    s.push_str("    tsubomi-registry:\n");
    s.push_str(&format!("      rule: \"Host(`{push_host}`)\"\n"));
    s.push_str(&format!("      entryPoints: [\"{}\"]\n", entrypoint(tls)));
    s.push_str("      service: \"tsubomi-registry\"\n");
    s.push_str("      middlewares: [\"tsubomi-registry-auth@file\"]\n");
    push_tls_block(&mut s, tls);
    if let Some(direct) = direct_host {
        // 直連入口(CF 不経由)。traefik がここで TLS 終端(証明書は DNS-01 = 公網 :80 不要)。
        // basicAuth は CF 入口と同じ middleware を共有(資格情報は 1 系統)。
        s.push_str("    tsubomi-registry-direct:\n");
        s.push_str(&format!("      rule: \"Host(`{direct}`)\"\n"));
        s.push_str("      entryPoints: [\"registrydirect\"]\n");
        s.push_str("      service: \"tsubomi-registry\"\n");
        s.push_str("      middlewares: [\"tsubomi-registry-auth@file\"]\n");
        s.push_str("      tls:\n");
        s.push_str("        certResolver: ledns\n");
    }
    s.push_str("  middlewares:\n");
    s.push_str("    tsubomi-registry-auth:\n");
    s.push_str("      basicAuth:\n");
    s.push_str("        users:\n");
    for u in users {
        s.push_str(&format!("          - \"{u}\"\n"));
    }
    s.push_str("  services:\n");
    s.push_str("    tsubomi-registry:\n");
    s.push_str("      loadBalancer:\n");
    s.push_str("        servers:\n");
    s.push_str("          - url: \"http://tsubomi-registry:5000\"\n");
    s
}

// ===== 永久削除時の repo 掃除 + 日次の未参照 blob 回収 =====

/// service の永久削除(purge)時に呼ぶ:registry から `<service_id>` repo の全 manifest を
/// 削除する。各 deploy は一意 tag(git_sha。git 無しは `local`)で push されるので、tag を
/// 列挙してそれぞれ digest を引き、manifest を DELETE する。manifest が消えると layer blob は
/// 無参照になり、日次の [`garbage_collect`] が実体を回収する(`REGISTRY_STORAGE_DELETE_ENABLED=true`)。
///
/// purge は rollback 対象ごと消える操作なので、活きた service の旧版を誤って消す心配はない
/// (削除済み service の repo まるごとが対象)。repo が既に無い(404)なら冪等に `Ok`。
/// loopback の pull registry(`registry_pull`、無認証)へ `state.http` で直結する。
pub async fn delete_repo(state: &AppState, service_id: Uuid) -> AppResult<()> {
    let base = format!("http://{}/v2/{}", state.config.registry_pull, service_id);

    // buildx は multi-arch で OCI image index を push し得る。schema2 / OCI の両系統を Accept
    // して、tagged な頂点 manifest(index か単一 manifest)の digest を引く。
    const MANIFEST_ACCEPT: &str = "application/vnd.docker.distribution.manifest.v2+json, \
         application/vnd.docker.distribution.manifest.list.v2+json, \
         application/vnd.oci.image.manifest.v1+json, \
         application/vnd.oci.image.index.v1+json";

    // 1) tag 一覧。404 = repo が無い(既に綺麗)= 冪等に成功扱い。
    let resp = state
        .http
        .get(format!("{base}/tags/list"))
        .send()
        .await
        .context("registry tags/list 取得に失敗")?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(());
    }
    if !resp.status().is_success() {
        return Err(AppError::Other(anyhow!(
            "registry tags/list が {} を返しました",
            resp.status()
        )));
    }
    #[derive(serde::Deserialize)]
    struct TagsList {
        tags: Option<Vec<String>>,
    }
    let tags = resp
        .json::<TagsList>()
        .await
        .context("registry tags/list の解析に失敗")?
        .tags
        .unwrap_or_default();

    // 2) tag ごとに digest を引いて manifest を DELETE(同一 digest を指す tag が複数でも、
    //    2 回目は 404 = 冪等に許容)。
    for tag in tags {
        let head = state
            .http
            .get(format!("{base}/manifests/{tag}"))
            .header(reqwest::header::ACCEPT, MANIFEST_ACCEPT)
            .send()
            .await
            .context("registry manifest 取得に失敗")?;
        if head.status() == reqwest::StatusCode::NOT_FOUND {
            continue;
        }
        let Some(digest) = head
            .headers()
            .get("Docker-Content-Digest")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
        else {
            tracing::warn!(%service_id, tag, "registry: Docker-Content-Digest 無し — manifest を削除できない");
            continue;
        };
        let del = state
            .http
            .delete(format!("{base}/manifests/{digest}"))
            .send()
            .await
            .context("registry manifest 削除に失敗")?;
        // 202 Accepted = 受理。404 = 既に消えている(冪等)。それ以外は失敗。
        if !del.status().is_success() && del.status() != reqwest::StatusCode::NOT_FOUND {
            return Err(AppError::Other(anyhow!(
                "registry manifest DELETE が {} を返しました",
                del.status()
            )));
        }
    }
    Ok(())
}

/// 日次:registry の未参照 blob を回収する(`registry garbage-collect`)。manifest が消えた後
/// (service purge / `local` tag の上書き)の layer 実体を解放する。storage は volume 内で
/// サーバから直接見えないため、registry コンテナ内で docker exec して実行する。
///
/// `--delete-untagged` で tag の無い manifest も併せて削除する(multi-arch index を消した後に
/// 残る子 manifest や、上書きで孤立した版を回収)。
///
/// **並行 push と競合し得る**(GC 実行中に upload 中の blob が消され得る)。read-only に切らない
/// 簡易運用なので、衝突確率を下げるため 1h tick ではなく**日次**に置く(まれな失敗は push 側の
/// リトライで回復)。best-effort:失敗は呼び出し側(gc.rs)で log する。
pub async fn garbage_collect(state: &AppState) -> AppResult<()> {
    use bollard::exec::{CreateExecOptions, StartExecResults};
    use futures_util::StreamExt;

    let created = state
        .docker
        .create_exec(
            REGISTRY_CONTAINER,
            CreateExecOptions {
                cmd: Some(vec![
                    "registry",
                    "garbage-collect",
                    "--delete-untagged=true",
                    "/etc/docker/registry/config.yml",
                ]),
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                ..Default::default()
            },
        )
        .await
        .context("registry GC の exec 作成に失敗")?;

    // 出力はドレインしないと exec が滞留する。ただし garbage-collect は走査した blob を全部
    // 吐くので無制限に溜めない:エラー文 / debug log に使う末尾だけ残せばよいので上限で打ち切る
    // (打ち切り後もストリームは読み続ける)。
    const MAX_LOG: usize = 8 * 1024;
    let mut log = String::new();
    if let StartExecResults::Attached { mut output, .. } = state
        .docker
        .start_exec(&created.id, None)
        .await
        .context("registry GC の exec 起動に失敗")?
    {
        while let Some(Ok(chunk)) = output.next().await {
            if log.len() < MAX_LOG {
                log.push_str(&String::from_utf8_lossy(&chunk.into_bytes()));
            }
        }
    }

    // exit code を確認。inspect 失敗 / exit_code 未確定(None)は「成功と確認できなかった」=
    // エラー扱いにする(呼び出し側が warn ログ。best-effort なので次の日次 tick で再走)。
    // ここを成功に倒すと、GC が実は走っていなくても "完了" と記録され失敗が永久に埋もれる。
    let inspected = state
        .docker
        .inspect_exec(&created.id)
        .await
        .context("registry GC の exec 状態取得に失敗")?;
    match inspected.exit_code {
        Some(0) => {
            tracing::debug!(output = %log.trim(), "registry: garbage-collect 完了");
            Ok(())
        }
        Some(code) => Err(AppError::Other(anyhow!(
            "registry garbage-collect が exit {code} で失敗: {}",
            log.trim()
        ))),
        None => Err(AppError::Other(anyhow!(
            "registry garbage-collect の exit code を確認できませんでした: {}",
            log.trim()
        ))),
    }
}

/// 既存アカウントを読んで復号する(無ければ None)。
async fn load(state: &AppState, user_id: Uuid) -> AppResult<Option<RegistryCreds>> {
    let row: Option<(String, Vec<u8>)> =
        sqlx::query_as("SELECT username, password_enc FROM registry_accounts WHERE user_id = $1")
            .bind(user_id)
            .fetch_optional(&state.db)
            .await?;
    match row {
        Some((user, password_enc)) => {
            let pass = state.crypto.decrypt(&password_enc)?;
            Ok(Some(RegistryCreds {
                // CI へ配る push 先:直連入口(CF 100MB 上限を回避)があればそれを優先。
                // 既存 service は gh variable `TSUBOMI_REGISTRY` を差し替えるだけで切替可能。
                host: state.config.registry_ci_host().to_string(),
                user,
                pass,
            }))
        }
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::render;

    const USER: &str = "u-abc:$2b$12$hashhashhash";

    #[test]
    fn render_tls_uses_websecure_and_le() {
        // 直 VPS(tls=true):websecure + certResolver。Host は push ホストをそのまま。
        let doc = render("registry.example.com", None, &[USER.to_string()], true);
        assert!(doc.contains("Host(`registry.example.com`)"));
        assert!(doc.contains("entryPoints: [\"websecure\"]"));
        assert!(doc.contains("certResolver: le"));
        assert!(doc.contains("tsubomi-registry-auth@file"));
        assert!(doc.contains(USER));
        assert!(doc.contains("http://tsubomi-registry:5000"));
    }

    #[test]
    fn render_no_tls_uses_web_and_no_certresolver() {
        // 上流終端(tls=false。CF Tunnel/逆代理):web エントリ・tls ブロック無し。basicAuth は付ける。
        let doc = render("registry.example.com", None, &[USER.to_string()], false);
        assert!(doc.contains("entryPoints: [\"web\"]"));
        assert!(!doc.contains("certResolver"));
        assert!(!doc.contains("tls:"));
        assert!(doc.contains("tsubomi-registry-auth@file"));
        assert!(doc.contains(USER));
    }

    #[test]
    fn render_direct_adds_second_router_with_dns01() {
        // 直連入口:registrydirect entrypoint + DNS-01(ledns)終端の第 2 router。CF 入口と共存し、
        // basicAuth middleware は共有。tunnel 部署(tls=false)でも直連側は常に TLS 終端。
        let doc = render(
            "registry.example.com",
            Some("registry-direct.example.com"),
            &[USER.to_string()],
            false,
        );
        assert!(doc.contains("Host(`registry.example.com`)"));
        assert!(doc.contains("Host(`registry-direct.example.com`)"));
        assert!(doc.contains("entryPoints: [\"registrydirect\"]"));
        assert!(doc.contains("certResolver: ledns"));
        // middleware は 1 定義を両 router が参照。
        assert_eq!(doc.matches("tsubomi-registry-auth@file").count(), 2);
        assert_eq!(doc.matches("basicAuth").count(), 1);
    }

    #[test]
    fn render_empty_is_fail_closed() {
        // アカウント 0 → router も basicAuth も書かない(push 入口は 404 = push 不可)。tls 不問。
        let doc = render("registry.example.com", None, &[], false);
        assert!(!doc.contains("routers"));
        assert!(!doc.contains("basicAuth"));
        assert!(!doc.contains("Host("));
    }
}
