//! deploy hook(no-auth、HMAC 検証)と `run_digest`(build 済みイメージを起こす単一操作)。
//!
//! build と run は別部分(m3-design §6.8 / 決定 #3):平台は **build しない**。CI か
//! `tbm deploy --local` が registry に push し、hook が digest を運んでくる。平台の仕事は
//! 「digest を受けて起こす」だけ。run_digest は hook / --local / start / rollback /
//! reconcile が共有する(注入は S6 — ここは PORT のみ)。
//!
//! swap は **start-first**(S5、決定 E を翻案):新コンテナを deploy 一意名で起こし、存活を
//! 確認し、route を新へ切り替えてから旧を消す。pull / create / start / 存活のどこで失敗しても
//! **旧コンテナと route は触らない**ので、失敗したデプロイは「旧版が生き続ける」で着地する
//! (m3-design §6.4。旧停止→新起動だと失敗時に旧版が消えるという §6.4/§6.5 の矛盾を解消)。
//! 同一 service の並行 deploy は `state.deploy_lock` で直列化する。

use crate::databases::{audit, map_unique};
use crate::error::{AppError, AppResult};
use crate::services::Visibility;
use crate::services::docker::{self, RunSpec};
use crate::services::inject;
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use futures_util::FutureExt;
use serde::Deserialize;
use serde_json::json;
use std::panic::AssertUnwindSafe;
use tsubomi_shared::hmac_sha256;
use uuid::Uuid;

const SIGNATURE_HEADER: &str = "x-tsubomi-signature";
/// ts の許容ずれ(リプレイ防御の片割れ。もう片方は nonce 一意)。
const MAX_SKEW_SECS: i64 = 300;

/// run_digest を起こす契機。reconcile はロック取得後に「まだ走るべき(desired=running かつ
/// phase=running)」かを再確認する — 候補取得とロック取得の間に stop が割り込むと停止済みの
/// service を蘇らせてしまうため。user 操作(hook / start / rollback)は明示的意図なので再確認しない。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DeployTrigger {
    User,
    Reconcile,
}

/// hook body。**生バイトで HMAC 検証してから** serde で読む(serde 経由で受けて
/// 再シリアライズすると 1 バイトの差で署名が割れるため、Bytes で生を取る)。
#[derive(Deserialize)]
struct HookBody {
    service_id: Uuid,
    git_sha: String,
    image_digest: String,
    ts: i64,
    nonce: String,
    /// commit の件名(`git log -1 --pretty=%s`)。message を送らない旧 workflow / 旧 CLI からの
    /// hook では欠落するので `#[serde(default)]`(None でも 202 で通す = 後方互換)。
    #[serde(default)]
    commit_message: Option<String>,
}

/// commit_message を保存用に健全化(空 → None、char 境界で 500 文字に切る = DB 膨張防止)。
/// git_sha / nonce は識別子なので不正は 400 で弾くが、これは表示専用の情報なので**切り詰めて
/// 通す**(長い commit message で deploy 自体を失敗させない)。HMAC 済みなので注入はしない。
fn sanitize_commit_message(m: Option<String>) -> Option<String> {
    let m = m?;
    let t = m.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.chars().take(500).collect())
    }
}

/// `POST /api/hook/deploy`(session 不要、IP 除外。決定 #4)。
/// HMAC = 権限そのもの。署名不一致は 401、ts 範囲外は 400、nonce 重複は 409、受理は 202。
pub async fn deploy(
    State(state): State<AppState>,
    headers: HeaderMap,
    raw: Bytes,
) -> AppResult<StatusCode> {
    // 1. service_id を取り出す(鍵を引くため。まだ信用しない)。
    let body: HookBody = serde_json::from_slice(&raw)
        .map_err(|_| AppError::BadRequest("hook body が不正な JSON です".into()))?;

    // 2. deploy_key を引いて HMAC を定数時間比較。鍵が無い(= service 不在 **または削除済み**)も
    //    401 に収束させ、署名の前に存在/状態を漏らさない。**deleted_at IS NULL を認証前に課す**ので、
    //    ソフト削除された service への漏洩鍵 / 旧 GitHub Action からの hook はここで弾かれ、nonce や
    //    deploys 行を書かない(run_digest 段まで進めて DB を汚さない)。
    let key_enc: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT s.deploy_key_enc FROM service_details s
           JOIN resources r ON r.id = s.resource_id
          WHERE s.resource_id = $1 AND r.kind = 'service' AND r.deleted_at IS NULL",
    )
    .bind(body.service_id)
    .fetch_optional(&state.db)
    .await?;
    let key_enc = key_enc.ok_or(AppError::Unauthorized)?;
    let deploy_key = state.crypto.decrypt(&key_enc)?;

    let sig = headers
        .get(SIGNATURE_HEADER)
        .and_then(|v| v.to_str().ok())
        .ok_or(AppError::Unauthorized)?;
    let provided = hex::decode(sig).map_err(|_| AppError::Unauthorized)?;
    let expected = hmac_sha256(deploy_key.as_bytes(), &raw);
    if !ct_eq(&expected, &provided) {
        return Err(AppError::Unauthorized);
    }

    // 認証済み。image_digest が本物の digest か検証する(決定 #3 の内容アドレス invariant)。
    if !is_sha256_digest(&body.image_digest) {
        return Err(AppError::BadRequest(
            "image_digest は sha256:<64桁16進> 形式の digest である必要があります(tag は不可 — 決定 #3)"
                .into(),
        ));
    }
    // git_sha は HMAC 済みなので注入はしないが、label / audit / deploys 行に入るので念のため
    // 長さ + 文字種を縛る(`local` や sha・tag を許容。security review S5)。
    if body.git_sha.is_empty()
        || body.git_sha.len() > 64
        || !body
            .git_sha
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'/'))
    {
        return Err(AppError::BadRequest(
            "git_sha は 1〜64 文字の英数字 . _ - / のみにしてください".into(),
        ));
    }
    // nonce は一意キーとして deploy_nonces に保存される。任意長 / 任意文字を許すと巨大 nonce で
    // DB を膨らませられるので長さ + 文字種を縛る(クライアントは hex16=32桁 か b64url16=22桁。
    // どちらも [A-Za-z0-9_-] に収まる)。HMAC 済みなので注入はしないが、保存物として健全化する。
    if body.nonce.len() < 16
        || body.nonce.len() > 128
        || !body
            .nonce
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err(AppError::BadRequest(
            "nonce は 16〜128 文字の英数字 - _ のみにしてください".into(),
        ));
    }

    // 3. リプレイ防御(時刻窓)。
    let now = chrono::Utc::now().timestamp();
    if (now - body.ts).abs() > MAX_SKEW_SECS {
        return Err(AppError::BadRequest(format!(
            "ts が許容窓(±{MAX_SKEW_SECS}s)の外です。送信側とサーバの時刻ずれを確認してください"
        )));
    }

    // 4. nonce 消費 + deploys(received) 記録を **1 トランザクション**で(nonce が消費された
    //    ⟺ deploy が記録された、を原子に保つ。片方だけ commit されてリトライ不能になるのを防ぐ)。
    let mut tx = state.db.begin().await?;
    sqlx::query("INSERT INTO deploy_nonces (service_id, nonce) VALUES ($1, $2)")
        .bind(body.service_id)
        .bind(&body.nonce)
        .execute(&mut *tx)
        .await
        .map_err(|e| map_unique(e, "この nonce は既に使われています(リプレイ)"))?;
    let deploy_id: Uuid = sqlx::query_scalar(
        "INSERT INTO deploys (service_id, git_sha, image_digest, status, commit_message)
              VALUES ($1, $2, $3, 'received', $4) RETURNING id",
    )
    .bind(body.service_id)
    .bind(&body.git_sha)
    .bind(&body.image_digest)
    .bind(sanitize_commit_message(body.commit_message))
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;

    // 非同期パイプラインへ。GH Action / --local を待たせず 202。
    let state2 = state.clone();
    let service_id = body.service_id;
    let image_digest = body.image_digest.clone();
    let git_sha = body.git_sha.clone();
    tokio::spawn(async move {
        // パイプラインを panic 包囲する(spawn 内の panic はタスクを黙って殺し、deploy が
        // deploying のまま残るため)。panic 時は **まだ deploying のものだけ** failed にする
        // (条件付き UPDATE。commit 済みの running は触らない)。
        let outcome = AssertUnwindSafe(run_digest(
            &state2,
            deploy_id,
            service_id,
            &image_digest,
            &git_sha,
            DeployTrigger::User,
        ))
        .catch_unwind()
        .await;
        match outcome {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                tracing::error!(error = ?e, %deploy_id, %service_id, "deploy パイプライン失敗")
            }
            Err(_) => {
                tracing::error!(%deploy_id, %service_id, "deploy タスクが panic");
                let _ = sqlx::query(
                    "UPDATE service_details SET phase='failed', phase_detail='内部エラー(panic)'
                       WHERE resource_id=$1 AND phase='deploying'",
                )
                .bind(service_id)
                .execute(&state2.db)
                .await;
                let _ = sqlx::query(
                    "UPDATE deploys SET status='failed', error='内部エラー(panic)', finished_at=now()
                       WHERE id=$1 AND status NOT IN ('succeeded','failed')",
                )
                .bind(deploy_id)
                .execute(&state2.db)
                .await;
            }
        }
    });

    Ok(StatusCode::ACCEPTED)
}

/// build 済みイメージ(digest)を起こす単一操作。同一 service の並行 deploy を直列化し、
/// 失敗は deploys / service_details に記録する(start-first なので失敗時も旧版は無傷)。
pub async fn run_digest(
    state: &AppState,
    deploy_id: Uuid,
    service_id: Uuid,
    image_digest: &str,
    git_sha: &str,
    trigger: DeployTrigger,
) -> AppResult<()> {
    // 同一 service の deploy を直列化(コンテナ / route / 状態の競合を防ぐ。単機インメモリ)。
    let lock = state.deploy_lock(service_id);
    let _guard = lock.lock().await;

    // ロック取得待ちの間に状態が変わった可能性(delete / stop / 別 deploy と競合)。行が無い =
    // 削除済み → 起動しない(削除済み service に孤児コンテナ / route を作らない)。
    let cur: Option<(String, String)> = sqlx::query_as(
        "SELECT s.desired_state, s.phase FROM service_details s
           JOIN resources r ON r.id = s.resource_id
          WHERE s.resource_id = $1 AND r.kind = 'service' AND r.deleted_at IS NULL",
    )
    .bind(service_id)
    .fetch_optional(&state.db)
    .await?;
    let Some((desired, phase)) = cur else {
        tracing::warn!(%service_id, %deploy_id, "deploy 対象が削除済み — スキップ(孤児防止)");
        abort_deploy(state, deploy_id, "service は削除済みです").await;
        return Ok(());
    };
    // reconcile の復活は「まだ走るべき」時だけ:候補取得とロック取得の間に stop が割り込んで
    // desired/phase が running でなくなっていたら停止済み service を蘇らせない(決定 #5)。
    // commit_success が desired=running に戻してしまうので、ここで弾くのが唯一の防壁。
    if trigger == DeployTrigger::Reconcile && (desired != "running" || phase != "running") {
        tracing::info!(%service_id, %deploy_id, desired, phase, "reconcile: 復活直前に状態が変化 — スキップ");
        abort_deploy(
            state,
            deploy_id,
            "reconcile: 復活前に状態が変化したためスキップ",
        )
        .await;
        return Ok(());
    }

    let _ = sqlx::query(
        "UPDATE service_details SET phase='deploying', phase_detail=NULL WHERE resource_id=$1",
    )
    .bind(service_id)
    .execute(&state.db)
    .await;

    let outcome =
        run_digest_inner(state, deploy_id, service_id, image_digest, git_sha, trigger).await;
    if let Err(e) = &outcome
        && let Err(e2) = mark_failed(state, deploy_id, service_id, &e.to_string()).await
    {
        tracing::error!(error = ?e2, %deploy_id, "deploy 失敗の記録に失敗");
    }
    outcome
}

/// deploy ごとに一意なコンテナ名(`tsubomi-<service 短码>-<deploy 短码 8 桁>`)。start-first で
/// 新旧が一瞬共存するため deploy 単位で一意にする。route の backend もこの名前を指すので、reconcile
/// の中断デプロイ復旧(直近成功 deploy のコンテナ = route が指す版を残す)もこの命名規約に依存する。
pub(crate) fn container_name(service_id: Uuid, deploy_id: Uuid) -> String {
    format!(
        "tsubomi-{}-{}",
        service_id.simple(),
        &deploy_id.simple().to_string()[..8]
    )
}

async fn run_digest_inner(
    state: &AppState,
    deploy_id: Uuid,
    service_id: Uuid,
    image_digest: &str,
    git_sha: &str,
    trigger: DeployTrigger,
) -> AppResult<()> {
    // 起動に必要な確定値を引く。
    // (subdomain, container_port, memory_mb, cpu_shares, visibility, stateful, cpu_limit_millis)
    type LaunchRow = (String, i32, i32, i32, String, bool, Option<i32>);
    let row: Option<LaunchRow> = sqlx::query_as(
        "SELECT subdomain, container_port, memory_mb, cpu_shares, visibility, stateful, cpu_limit_millis
           FROM service_details WHERE resource_id = $1",
    )
    .bind(service_id)
    .fetch_optional(&state.db)
    .await?;
    let (subdomain, container_port, memory_mb, cpu_shares, visibility, stateful, cpu_limit_millis) =
        row.ok_or(AppError::NotFound)?;
    let visibility = Visibility::from_db(&visibility);

    set_status(state, deploy_id, "pulling").await;
    let image_ref = docker::pull(state, service_id, image_digest).await?;

    set_status(state, deploy_id, "starting").await;
    // 注入を起動の瞬間に解決(静的 env + database/volume、+ volume の bind。決定 #5)。
    // PORT は最後に足す。重複キーは **後勝ち**で畳む(injection が static を、PORT が両方を
    // 上書き)。Docker の重複 env の扱い(実装依存)に頼らず、ここで決定的にする。
    let (mut env, binds) = inject::resolve(state, service_id).await?;
    env.push(("PORT".to_string(), container_port.to_string()));
    let env = dedup_env_last(env);

    // 新コンテナは deploy 一意名で起こす。
    let new_name = container_name(service_id, deploy_id);
    let spec = RunSpec {
        service_id,
        container_name: new_name.clone(),
        subdomain,
        git_sha: git_sha.to_string(),
        container_port,
        memory_mb,
        cpu_shares,
        cpu_limit_millis,
        env,
        binds,
    };

    // stateful は **stop-first**(設計 §3):swap は新旧コンテナが同一データ目録を同時に開く
    // (postgres の postmaster.pid 防双開は跨 PID namespace で信頼できない → 双開 = 破壊)ため
    // 禁忌。先に旧を止める — ただし **remove はしない**(§0-E:新の起動が失敗したら再 start で
    // 自動復旧する退路。stopped 容器の網 endpoint / binds は docker が温存する)。瞬断は stateful
    // の契約(§0-F)。pull / 注入解決は上で済ませてある = 停止窓を最小にする順序。
    // stateless は空 Vec = 以後の復旧呼び出しが全て no-op(現行の start-first と完全に同じ動き)。
    let stopped_old: Vec<String> = if stateful {
        docker::stop_running(state, service_id, docker::STATEFUL_STOP_GRACE_SECS).await?
    } else {
        Vec::new()
    };

    // 新コンテナを起こし存活を確認(create+start → is_live)→ 成功を route 切替の **前に**
    // DB へ記録する(DB 書き込みは最も多い失敗点で、route がまだ旧を指す §6.4 の安全な失敗点)。
    // どちらで失敗しても巻き戻しは同一なので一箇所に畳む:新を片付け、stateful は温存した旧を
    // 再 start して旧版へ自動復旧(§0-E。stateless と違い旧は stopped なので**能動的に**戻す —
    // 設計 §6 地雷 2。stateless は stopped_old が空 = no-op で、旧が走ったまま = 従来どおり)。
    // readiness 探測を課す条件(どれも欠けたら存活確認のみ):
    //  - **ユーザ契機のみ**(審査指摘):reconcile の復活対象は一度 succeeded した版 = readiness は
    //    初回デプロイで検証済み。復活は容器一斉消失後の再建等で Pi が飽和し「健全だが遅い」が
    //    起きやすく、ここで failed にすると phase=failed で converge_running の候補から永久に
    //    外れる(自沉静化は壊れたイメージ向けの安全弁で、健全な app の静默停止に使わない)。
    //  - **company/public、または「M6 リンクの callee になっている private」**(codex 審査):
    //    素の private は listen しない純 worker を許容する契約なので門を掛けないが、誰かに注入
    //    されている private は内部リンク先 = listen する契約なので、監听錯 port のまま succeeded →
    //    attach_as_callee が呼び出し元を不達の新容器へ切り替える穴をここで塞ぐ。
    let probe = trigger == DeployTrigger::User
        && (visibility != Visibility::Private || is_linked_callee(state, service_id).await);
    let staged = async {
        start_container(state, &spec, &image_ref, probe).await?;
        commit_success(state, deploy_id, service_id, image_digest).await
    }
    .await;
    if let Err(e) = staged {
        // readiness TimedOut では新コンテナは**走行中**のまま消される。stateful は起動期の
        // WAL 回復 / 迁移中に SIGKILL しないよう 30s 猶予で止める(§0-G。審査指摘 —
        // stop-first と同じ丁寧さをこの回滚路径にも)。stateless は即殺でよい(データ無共有)。
        let grace = stateful.then_some(docker::STATEFUL_STOP_GRACE_SECS);
        docker::remove_one(state, &new_name, grace).await;
        docker::restart_stopped(state, &stopped_old).await;
        return Err(e);
    }

    // ★ ここから先は「成功確定」点を越えている(DB 上 new が正、新コンテナは起動済み)。route
    //   切替・旧削除の失敗は **致命にしない**:failed と誤記録すると「実際は成功した deploy」を
    //   巻き戻すことになる。不整合は reconcile(S8)/ 再 deploy が収束させる。
    //   まず route を visibility どおりに合わせ(cutover 可否が決まる)、可なら内部リンク切替 + 旧掃除。
    let cutover = if visibility == Visibility::Private {
        // private の期望状態 = route ファイル無し(公開範囲設計 §6)。旧 visibility の残骸を掃く(冪等)。
        // remove 失敗でも cutover を進めるのは意図した **fail-closed**:陳腐ファイルは消えた backend を
        // 指し公網は最悪 502(= 内容不可達)で、旧掃除を止めて旧版が公網に出続けるより安全側。
        // reconcile の private 分岐が ≤30s でファイルを回収し /noservice へ収束する。
        if let Err(e) = crate::services::route::remove(state, service_id) {
            tracing::error!(error = ?e, %service_id, "private の route 撤去に失敗(fail-closed で続行。reconcile が回収)");
        }
        true
    } else {
        match crate::services::route::write(
            state,
            service_id,
            &spec.subdomain,
            &new_name,
            spec.container_port,
            visibility.ipallow(),
        ) {
            Ok(()) => true,
            Err(e) => {
                // route 切替失敗。**stateless**:旧を消すと route→消えた旧 で 502 になるため旧を
                // 残す(旧版が当面トラフィックを受ける。reconcile / 再 deploy が route を直す)。
                // **stateful**:旧は stop-first で既に停止済み =「温存」に serving の意味が無く、
                // 公開 route はどのみち stale(→ reconcile が ≤30s で新へ修復)。ここで止めると
                // 内部リンクまで新版へ向かないまま残るので、内部カットオーバーは進める(codex
                // review 2026-07-03 #2)。
                tracing::error!(error = ?e, %service_id, stateful, "route 切替に失敗(stateless=旧版を温存 / stateful=内部切替は続行。reconcile / 再 deploy で収束)");
                stateful
            }
        }
    };
    if cutover {
        // route が新を指した(private は公開 route 無し = 切替点は commit_success)。**内部リンクも
        // 同一の瞬間に切替える**:この service を callee として注入している caller 群の私網へ、新コンテナを
        // 別名で attach する。commit_success より後 = 旧版にしか繋がっていなかった内部呼び出しも、ここで
        // 初めて新版へ向く(公開と内部のカットオーバーが揃う)。先に新を付けてから旧を消す(旧 endpoint は
        // 旧コンテナ削除で自然消滅 = 別名は新へ収束。新を付ける前に旧を消すと一瞬 A→B が切れるため順序が肝心)。
        crate::services::network::attach_as_callee(state, service_id, &spec.subdomain, &new_name)
            .await;
        // 旧を消してよい(失敗しても新は稼働中。reconcile が掃除)。
        if let Err(e) = docker::remove_others(state, service_id, &new_name).await {
            tracing::warn!(error = ?e, %service_id, "旧コンテナの掃除に失敗(新は稼働中。reconcile が後で掃除)");
        }
    }

    audit(
        &state.db,
        None,
        "service.deploy",
        service_id,
        json!({ "git_sha": git_sha, "image_digest": image_digest }),
        None,
    )
    .await;
    Ok(())
}

/// 新コンテナを create+start し、存活(restart_count==0 等)を確認する(route はまだ切らない)。
/// `probe`(company/public)なら存活の後に **container_port の TCP readiness** も門とする
/// (AI 審査 R1):監听錯 port / listen 前にクラッシュする app を succeeded にしない。
/// 失敗は呼び出し側が新コンテナを掃除する(旧は無傷)。
async fn start_container(
    state: &AppState,
    spec: &RunSpec,
    image_ref: &str,
    probe: bool,
) -> AppResult<()> {
    // 起動前の時刻を控える(-1s は時計の丸め保険):crash_summary が docker events(die/oom)を
    // この時刻以降で引く。inspect は restart でリセットされるため、events だけが「その退出」の
    // exit code を保持する。
    let since = chrono::Utc::now().timestamp() - 1;
    docker::run(state, spec, image_ref).await?;
    if !docker::is_live(state, &spec.container_name).await {
        return Err(crash_error(
            state,
            &spec.container_name,
            since,
            "新コンテナが起動直後に終了しました",
        )
        .await);
    }
    let timeout = std::time::Duration::from_secs(state.config.ready_timeout_secs);
    if !probe || timeout.is_zero() {
        return Ok(()); // private / reconcile 復活 / 明示無効化(=0)は存活のみ
    }
    match docker::wait_tcp_ready(state, &spec.container_name, spec.container_port, timeout).await {
        docker::Readiness::Ready => Ok(()),
        docker::Readiness::Died => Err(crash_error(
            state,
            &spec.container_name,
            since,
            "新コンテナが readiness 確認中に終了しました",
        )
        .await),
        docker::Readiness::TimedOut => {
            let port = spec.container_port;
            let tail = log_tail_detail(state, &spec.container_name).await;
            Err(AppError::Other(anyhow::anyhow!(
                "新コンテナは走っていますが、container_port(={port})の TCP 待受を {}s 以内に確認できませんでした。\
                 app が PORT 環境変数(={port})の値で 0.0.0.0 に listen しているか確認してください。\
                 listen しない worker なら `tbm service visibility <name> private` に切り替えると探測はスキップされます。{tail}",
                timeout.as_secs()
            )))
        }
    }
}

/// この service が M6 内部リンクの callee(他 service に注入されている)か。private の readiness
/// 探測可否の判定用。best-effort:DB エラー時は false(探測を増やす向きに倒さない — デプロイ自体を
/// 止めないことを優先し、穴は次のユーザ契機デプロイで再判定される)。
async fn is_linked_callee(state: &AppState, service_id: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM injections WHERE resource_id = $1)")
        .bind(service_id)
        .fetch_one(&state.db)
        .await
        .unwrap_or(false)
}

/// 死んだ / 死につつある新コンテナから終了要因(events/inspect)とログ末尾を拾い、原因を 1 本の
/// エラーに畳む。これが無いと失敗 deploy で `tbm service logs`(現行=旧コンテナを引く)が
/// 空になり、クラッシュ原因が一切見えない盲点になる。ここで拾えば deploys.error → service
/// status に残る。
async fn crash_error(state: &AppState, name: &str, since: i64, headline: &str) -> AppError {
    let why = docker::crash_summary(state, name, since).await;
    let why = why
        .as_deref()
        .unwrap_or("終了要因を取得できませんでした");
    let detail = log_tail_detail(state, name).await;
    AppError::Other(anyhow::anyhow!("{headline}:{why}。{detail}"))
}

/// コンテナログ末尾を deploys.error 向けに整形する(best-effort)。
async fn log_tail_detail(state: &AppState, name: &str) -> String {
    let tail = docker::logs_by_name(state, name, 40).await;
    let tail = tail.trim();
    // logs_by_name は best-effort(取得失敗も空文字)なので「無出力」と断定しない。
    if tail.is_empty() {
        return "コンテナログ(stdout+stderr)無し — 何も出力していないか、ログ取得に失敗"
            .to_string();
    }
    // deploys.error 列に載るので末尾 1500 文字だけに切る(char 境界安全)。
    let n = tail.chars().count();
    let clipped: String = if n > 1500 {
        format!("…{}", tail.chars().skip(n - 1500).collect::<String>())
    } else {
        tail.to_string()
    };
    format!("コンテナログ末尾(stdout+stderr):\n{clipped}")
}

/// 成功を 1 tx で記録(image_digest=new / phase=running / desired=running / deploys=succeeded)。
async fn commit_success(
    state: &AppState,
    deploy_id: Uuid,
    service_id: Uuid,
    image_digest: &str,
) -> AppResult<()> {
    let mut tx = state.db.begin().await?;
    sqlx::query(
        "UPDATE service_details
            SET image_digest=$2, phase='running', desired_state='running',
                phase_detail=NULL, last_deploy_at=now()
          WHERE resource_id=$1",
    )
    .bind(service_id)
    .bind(image_digest)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE deploys SET status='succeeded', finished_at=now() WHERE id=$1")
        .bind(deploy_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// 失敗の記録(deploys=failed + service_details phase=failed を 1 tx で一致させる)。
pub(crate) async fn mark_failed(
    state: &AppState,
    deploy_id: Uuid,
    service_id: Uuid,
    msg: &str,
) -> AppResult<()> {
    let mut tx = state.db.begin().await?;
    sqlx::query("UPDATE deploys SET status='failed', error=$2, finished_at=now() WHERE id=$1")
        .bind(deploy_id)
        .bind(msg)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE service_details SET phase='failed', phase_detail=$2 WHERE resource_id=$1")
        .bind(service_id)
        .bind(msg)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// deploy を起こさずに deploys 行だけ failed で閉じる(削除済み / reconcile スキップの共通処理)。
/// service_details の phase は **触らない** — 既存の状態(stopped 等)を尊重する。
async fn abort_deploy(state: &AppState, deploy_id: Uuid, reason: &str) {
    let _ = sqlx::query(
        "UPDATE deploys SET status='failed', error=$2, finished_at=now()
          WHERE id=$1 AND status NOT IN ('succeeded','failed')",
    )
    .bind(deploy_id)
    .bind(reason)
    .execute(&state.db)
    .await;
}

pub(crate) async fn set_status(state: &AppState, deploy_id: Uuid, status: &str) {
    let _ = sqlx::query("UPDATE deploys SET status=$2 WHERE id=$1")
        .bind(deploy_id)
        .bind(status)
        .execute(&state.db)
        .await;
}

/// env の重複キーを「後勝ち」で畳む(後ろの要素が優先。env は集合なので順序は不問)。
/// service_env(静的)→ injection → PORT の順で積んであるので、injection が static を、
/// PORT が両方を上書きする。Docker の重複 env の扱い(実装依存)に頼らない。
fn dedup_env_last(env: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for (k, v) in env {
        map.insert(k, v);
    }
    map.into_iter().collect()
}

/// `sha256:` + 64 桁 16 進かどうか。tag や任意文字列を弾く(決定 #3 の digest ピン留め)。
pub(crate) fn is_sha256_digest(s: &str) -> bool {
    s.strip_prefix("sha256:")
        .is_some_and(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// 定数時間比較(長さ違いは即 false。HMAC 出力は固定長なので長さは秘密でない)。
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ct_eq_basic() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
    }

    #[test]
    fn dedup_env_keeps_last() {
        // 同じ KEY は後勝ち(injection が static を、PORT が両方を上書きする想定)。
        let env = vec![
            ("DATABASE_URL".to_string(), "static".to_string()),
            ("PORT".to_string(), "3000".to_string()),
            ("DATABASE_URL".to_string(), "injected".to_string()),
            ("PORT".to_string(), "8080".to_string()),
        ];
        let out: std::collections::HashMap<_, _> = dedup_env_last(env).into_iter().collect();
        assert_eq!(out.get("DATABASE_URL").unwrap(), "injected");
        assert_eq!(out.get("PORT").unwrap(), "8080");
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn sha256_digest_validation() {
        assert!(is_sha256_digest(&format!("sha256:{}", "a".repeat(64))));
        assert!(is_sha256_digest(&format!(
            "sha256:{}",
            "0123456789abcdef".repeat(4)
        )));
        assert!(!is_sha256_digest("latest")); // tag
        assert!(!is_sha256_digest("myrepo:v1")); // tag
        assert!(!is_sha256_digest("sha256:abc")); // 短い
        assert!(!is_sha256_digest(&format!("sha256:{}", "g".repeat(64)))); // 非 16 進
        assert!(!is_sha256_digest(&"a".repeat(64))); // prefix 無し
    }
}
