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

/// GC が service ごとに温存する成功版の数(現役 digest は別枠で常に温存)。rollback の実効窓 =
/// この数(それより古い版の manifest は回収済み = rollback は pull 404。ディスクとの取引)。
const KEEP_SUCCEEDED_DEPLOYS: usize = 5;

/// buildx は multi-arch で OCI image index を push し得る。schema2 / OCI の両系統を Accept して
/// 頂点 manifest(index か単一 manifest)を扱う(delete_repo / GC 保護の共有)。
const MANIFEST_ACCEPT: &str = "application/vnd.docker.distribution.manifest.v2+json, \
     application/vnd.docker.distribution.manifest.list.v2+json, \
     application/vnd.oci.image.manifest.v1+json, \
     application/vnd.oci.image.index.v1+json";

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
    let base = repo_base(state, service_id);

    // 1) tag 一覧。404 = repo が無い(既に綺麗)= 冪等に成功扱い。
    let Some(tags) = list_tags(state, &base).await? else {
        return Ok(());
    };

    // 2) tag ごとに digest を引いて manifest を DELETE。None は静かにスキップ —
    //    同一 digest を指す tag が複数あると 2 回目以降の解決が 404 になる(1 回目の DELETE が
    //    manifest ごと全 tag を消す)= 期待どおりの冪等(ヘッダ欠落の異常系は manifest_digest が warn)。
    for tag in tags {
        let Some(digest) = manifest_digest(state, &base, &tag).await? else {
            continue;
        };
        delete_manifest(state, &base, &digest).await?;
    }
    Ok(())
}

/// service の repo の registry API ベース URL(loopback の pull 入口、無認証)。
fn repo_base(state: &AppState, service_id: Uuid) -> String {
    format!("http://{}/v2/{}", state.config.registry_pull, service_id)
}

/// repo の tag 一覧。repo 不在(404)は None(呼び出し側が「既に綺麗」に倒す)。
async fn list_tags(state: &AppState, base: &str) -> AppResult<Option<Vec<String>>> {
    let resp = state
        .http
        .get(format!("{base}/tags/list"))
        .send()
        .await
        .context("registry tags/list 取得に失敗")?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
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
    Ok(Some(
        resp.json::<TagsList>()
            .await
            .context("registry tags/list の解析に失敗")?
            .tags
            .unwrap_or_default(),
    ))
}

/// 参照(tag or digest)の頂点 manifest digest を引く。不在(404)は None(冪等パスの正常系 =
/// 呼び出し側は静かにスキップしてよい)。200 なのにヘッダが無い異常系はここで warn して None。
async fn manifest_digest(state: &AppState, base: &str, reference: &str) -> AppResult<Option<String>> {
    let resp = state
        .http
        .get(format!("{base}/manifests/{reference}"))
        .header(reqwest::header::ACCEPT, MANIFEST_ACCEPT)
        .send()
        .await
        .context("registry manifest 取得に失敗")?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let digest = resp
        .headers()
        .get("Docker-Content-Digest")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    if digest.is_none() {
        tracing::warn!(base, reference, "registry: Docker-Content-Digest ヘッダ無し");
    }
    Ok(digest)
}

/// manifest を digest 指定で削除(その manifest を指す**全 tag も一緒に消える** — distribution v2
/// に tag 単体の削除は無い)。202 = 受理、404 = 既に無い(冪等)。
async fn delete_manifest(state: &AppState, base: &str, digest: &str) -> AppResult<()> {
    let del = state
        .http
        .delete(format!("{base}/manifests/{digest}"))
        .send()
        .await
        .context("registry manifest 削除に失敗")?;
    if !del.status().is_success() && del.status() != reqwest::StatusCode::NOT_FOUND {
        return Err(AppError::Other(anyhow!(
            "registry manifest DELETE が {} を返しました",
            del.status()
        )));
    }
    Ok(())
}

/// GC の前段:**窓の外の旧版 manifest(index + その子)を平台が明示削除する**(stateful 設計
/// §10-E の修正)。keep 集合 = 現役 `image_digest` ∪ 直近 [`KEEP_SUCCEEDED_DEPLOYS`] 成功版。
///
/// 背景のバグ:従来の `garbage_collect --delete-untagged` は「tag に参照されない manifest = ゴミ」
/// と見なすが、(a)同じ tag への再 push(同一 commit の deploy 再実行 — build 非再現で digest は
/// 毎回変わる)でも旧 digest は失参照になり、後続 deploy が失敗すると**現に serving 中の digest が
/// 回収**され start / rollback の pull が 404 で全滅、さらに(b)**tag 付き index の子 manifest まで
/// 食う**(distribution の既知欠陥 — 本番の既存 index で子欠損を実証。multi-arch の別アーキ・
/// attestation が静かに欠けていた)。
///
/// 対策の形(**`--delete-untagged` は廃止** — 何が消えるかは平台だけが決める):
/// 1. **期限切れ**:deploys 由来の **terminal な**(succeeded/failed のみ = in-flight を触らない)
///    distinct digest のうち keep 外を、**index → その子 manifests** の順に DELETE。子は
///    keep / in-flight の index が参照している分を除外する(buildx キャッシュで別 index が同一の
///    子を共有し得るため — 盲目削除は保護対象を壊す)。tag-only の digest(push 済み・hook 未達)は
///    触らない(deploy 中のレース回避。孤児として残るのは失敗 push のみ = 小さな漏れとして受容、
///    repo ごと消える purge の delete_repo が最終掃除)。
/// 2. その後の [`garbage_collect`](blob 掃除のみ)が、参照を失った層の実体を回収する。
///
/// 全体 best-effort(per-service / per-manifest で warn、他を止めない)。呼び出しは gc.rs の
/// 日次 tick で `garbage_collect` の**直前**。
pub async fn protect_and_expire_manifests(state: &AppState) -> AppResult<()> {
    let services: Vec<(Uuid, Option<String>)> = sqlx::query_as(
        "SELECT r.id, s.image_digest
           FROM resources r JOIN service_details s ON s.resource_id = r.id
          WHERE r.kind = 'service' AND r.deleted_at IS NULL",
    )
    .fetch_all(&state.db)
    .await?;

    for (sid, current) in services {
        if let Err(e) = protect_and_expire_one(state, sid, current.as_deref()).await {
            tracing::warn!(error = ?e, %sid, "registry GC 前処理:manifest の保護 / 期限切れに失敗(次回 tick で再試行)");
        }
    }
    Ok(())
}

/// 1 service ぶんの保護 + 期限切れ(本体は [`protect_and_expire_manifests`] のドキュメント参照)。
///
/// **クエリの順序が命**(codex review 2026-07-03 #1):期限切れ候補(expendable)を**先に**、
/// keep 集合を**後に**読む。並行 deploy の commit_success が両クエリの間に割り込んでも、
/// 「候補 = 古い現実の部分集合、保護 = 新しい現実の超集合」なので、新しく成功した現役 digest は
/// 候補に居らず(古い快照)、逆に候補にいる旧 digest が current 化した場合は keep(新しい快照)が
/// 拾って除外する。逆順だと「keep に無いが候補にある新現役」を消し得る。
async fn protect_and_expire_one(
    state: &AppState,
    service_id: Uuid,
    current: Option<&str>,
) -> AppResult<()> {
    // 1) 期限切れ候補(古い快照):terminal な deploys の distinct digest。
    //    非 terminal 行(received/pulling/starting)を 1 つでも持つ digest は in-flight = 触らない。
    //    **48h の年齢下限**(HAVING):直近に push された digest は消さない — (a)失敗 deploy の
    //    イメージは即時の再試行 / 診断にまだ要る、(b)温かい buildx キャッシュが同一 digest を
    //    再 push し得る = 「DELETE 直後の再 push が blob 掃除と競合して假 201 で毒される」
    //    (2026-07-08 本番実証)の餌をまかない。48h 分の失敗イメージのディスクは受容。
    let expendable: Vec<(String,)> = sqlx::query_as(
        "SELECT d.image_digest FROM deploys d
          WHERE d.service_id = $1 AND d.status IN ('succeeded','failed')
            AND NOT EXISTS (
              SELECT 1 FROM deploys x
               WHERE x.service_id = d.service_id AND x.image_digest = d.image_digest
                 AND x.status NOT IN ('succeeded','failed'))
          GROUP BY d.image_digest
          HAVING MAX(d.created_at) < now() - interval '48 hours'",
    )
    .bind(service_id)
    .fetch_all(&state.db)
    .await?;

    // 2) keep 集合(新しい快照):現役 ∪ distinct 直近 N 成功版。distinct は SQL 側で取る
    //    (同一 digest の連続 deploy が多いと LIMIT 先取りで distinct が痩せる — codex #3)。
    let succeeded: Vec<(String,)> = sqlx::query_as(
        "SELECT image_digest FROM deploys
          WHERE service_id = $1 AND status = 'succeeded'
          GROUP BY image_digest
          ORDER BY MAX(created_at) DESC
          LIMIT $2",
    )
    .bind(service_id)
    .bind(KEEP_SUCCEEDED_DEPLOYS as i64)
    .fetch_all(&state.db)
    .await?;
    let succeeded: Vec<String> = succeeded.into_iter().map(|(d,)| d).collect();
    let keep = keep_window(&succeeded, current, KEEP_SUCCEEDED_DEPLOYS);

    // 3) in-flight(非 terminal 行を持つ)digest — 期限切れ候補からは NOT EXISTS で除外済みだが、
    //    **子の共有防護**にも要る:buildx キャッシュは「変わらなかったアーキの子 manifest」を
    //    複数 index 間で同一 digest のまま共有し得る。守るべき index(keep + in-flight)の子は
    //    消してはならない。
    let inflight: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT image_digest FROM deploys
          WHERE service_id = $1 AND status NOT IN ('succeeded','failed')",
    )
    .bind(service_id)
    .fetch_all(&state.db)
    .await?;

    let base = repo_base(state, service_id);
    // 守るべき index の子 manifest 集合(keep ∪ in-flight。高々 N+α 回の GET/日)。
    let mut protected_children: std::collections::HashSet<String> = std::collections::HashSet::new();
    for digest in keep.iter().chain(inflight.iter().map(|(d,)| d)) {
        match manifest_children(state, &base, digest).await {
            Ok(children) => protected_children.extend(children),
            Err(e) => {
                tracing::warn!(error = ?e, %service_id, digest, "registry GC 前処理:保護 index の子列挙に失敗");
            }
        }
    }

    // 4) 期限切れ:候補のうち keep 外を「index → その子」の順に削除(既に消えた分は 404 = 冪等。
    //    回収済み digest への再 DELETE は日次 + loopback なので受容 — efficiency review)。
    //    子は保護集合(keep / in-flight の index が参照)に入っている分をスキップ。
    for (digest,) in expendable {
        if keep.contains(&digest) {
            continue;
        }
        // 子を先に列挙(index 削除後は本体が読めない)、削除は index が先(参照を断ってから子)。
        let children = manifest_children(state, &base, &digest)
            .await
            .unwrap_or_default();
        if let Err(e) = delete_manifest(state, &base, &digest).await {
            tracing::warn!(error = ?e, %service_id, digest, "registry GC 前処理:旧版 manifest の削除に失敗");
            continue; // 本体が消せなければ子も温存(片肺の index を作らない)
        }
        for child in children {
            if protected_children.contains(&child) {
                continue;
            }
            if let Err(e) = delete_manifest(state, &base, &child).await {
                tracing::warn!(error = ?e, %service_id, digest = child, "registry GC 前処理:子 manifest の削除に失敗");
            }
        }
    }
    Ok(())
}

/// index(manifest list)の子 manifest digest を列挙する。単一 manifest(`manifests` 配列なし)や
/// 不在(404)は空。JSON は distribution / OCI の頂点形だけを素朴に読む(`manifests[].digest`)。
async fn manifest_children(state: &AppState, base: &str, digest: &str) -> AppResult<Vec<String>> {
    let resp = state
        .http
        .get(format!("{base}/manifests/{digest}"))
        .header(reqwest::header::ACCEPT, MANIFEST_ACCEPT)
        .send()
        .await
        .context("registry manifest 取得に失敗(子列挙)")?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(Vec::new());
    }
    if !resp.status().is_success() {
        return Err(AppError::Other(anyhow!(
            "registry manifest GET が {} を返しました",
            resp.status()
        )));
    }
    let body: serde_json::Value = resp.json().await.context("registry manifest の解析に失敗")?;
    Ok(extract_children(&body))
}

/// 頂点 manifest JSON から子 digest を取り出す純関数(index/list 以外は空)。
fn extract_children(manifest: &serde_json::Value) -> Vec<String> {
    manifest
        .get("manifests")
        .and_then(|m| m.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("digest").and_then(|d| d.as_str()).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// keep 集合(純関数):現役 digest ∪ 成功 digest 列(新しい順・重複あり)の distinct 先頭 n 個。
fn keep_window(
    succeeded_desc: &[String],
    current: Option<&str>,
    n: usize,
) -> std::collections::HashSet<String> {
    let mut keep: std::collections::HashSet<String> = std::collections::HashSet::new();
    if let Some(c) = current {
        keep.insert(c.to_string());
    }
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for d in succeeded_desc {
        if seen.len() >= n {
            break;
        }
        if seen.insert(d.as_str()) {
            keep.insert(d.clone());
        }
    }
    keep
}

/// 日次:registry の未参照 blob を回収する(`registry garbage-collect`)。manifest が消えた後
/// (期限切れ / service purge)の layer 実体を解放する。storage は volume 内でサーバから直接
/// 見えないため、registry コンテナ内で docker exec して実行する。
///
/// **`--delete-untagged` は使わない**:あれは「tag 失参照 = ゴミ」と見なすが、(a)同 tag 再 push で
/// 失参照になった**現役** digest も食い、(b)**tag 付き index の子 manifest まで食う**(distribution
/// の既知欠陥 — 本番の既存 index で子欠損を実証)。manifest を消す判断は
/// [`protect_and_expire_manifests`](平台の keep 窓)だけが行い、ここは blob 掃除に徹する。
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
    use super::keep_window;
    use super::render;

    const USER: &str = "u-abc:$2b$12$hashhashhash";

    #[test]
    fn keep_window_keeps_current_and_recent_distinct() {
        let s = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // 新しい順・重複あり(同じ digest の再 deploy)。distinct で 2 個まで + 現役。
        let succ = s(&["d3", "d3", "d2", "d1"]);
        let keep = keep_window(&succ, Some("d9"), 2);
        assert!(keep.contains("d9")); // 現役は成功列に無くても常に温存
        assert!(keep.contains("d3") && keep.contains("d2"));
        assert!(!keep.contains("d1")); // 窓の外 = 期限切れ候補
        // 現役が窓内と重複しても集合なので二重にならない。成功ゼロでも現役だけは守る。
        assert_eq!(keep_window(&[], Some("d1"), 5).len(), 1);
        assert!(keep_window(&[], None, 5).is_empty());
    }

    #[test]
    fn extract_children_reads_index_manifests() {
        // OCI index / docker manifest list:manifests[].digest を列挙。
        let index = serde_json::json!({
            "schemaVersion": 2,
            "manifests": [
                { "digest": "sha256:aaa", "platform": { "architecture": "arm64" } },
                { "digest": "sha256:bbb", "platform": { "architecture": "amd64" } },
            ]
        });
        assert_eq!(super::extract_children(&index), vec!["sha256:aaa", "sha256:bbb"]);
        // 単一 manifest(layers はあるが manifests は無い)→ 空 = 子なし。
        let single = serde_json::json!({ "schemaVersion": 2, "layers": [{ "digest": "sha256:x" }] });
        assert!(super::extract_children(&single).is_empty());
    }

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
