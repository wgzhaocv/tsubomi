//! reconcile(tech-design §0 決定 #5 / §3、m3-design §8):管制面 Postgres の期望状態へ
//! 現実(コンテナ / route)を収束させる **第二の保険**(第一は restart=unless-stopped)。
//!
//! 起動時に 1 回フル + 30 秒毎にライト(gc と同型の spawn)。1 パスの職務(意図的に短い):
//!   1. 存在収束:DB が「走っている」と信じる(phase=running)service にコンテナが無ければ、
//!      直近成功 deploy の digest で起こし直す(= 正規の deploy 経路。route も書き直される)。
//!   2. 孤児掃除:DB に生きた行が無い管理コンテナ / route ファイルを消す。
//!
//! **やらないこと(決定 #5)**:env / 注入のドリフトは追わない(値は起動の瞬間にだけ解決され、
//! 変更 / rotate / リソース削除は自動再起動を引き起こさない — 再 deploy して初めて効く)。
//! phase の帳尻合わせもしない(phase 遷移の権限は run_digest / stop だけが持つ)。

use crate::databases::audit;
use crate::error::AppResult;
use crate::services::deploy::DeployTrigger;
use crate::services::{docker, latest_succeeded_deploy, redeploy, route};
use crate::state::AppState;
use serde_json::json;
use std::collections::HashSet;
use std::time::Duration;
use uuid::Uuid;

/// reconcile パスの間隔(m3-design §8 既定)。
const RECONCILE_INTERVAL: Duration = Duration::from_secs(30);

/// reconcile ループを起こす(gc::spawn と同型)。最初のフルパスは起動直後(interval の 0 tick)。
/// パスは逐次(`tick.tick().await; pass().await;`)なので重ならない — 遅いパス(イメージ pull)は
/// その回だけ他 service を待たせるが、単機規模では許容。
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(RECONCILE_INTERVAL);
        loop {
            tick.tick().await;
            reconcile_pass(&state).await;
        }
    });
}

/// 1 パス:存在収束 → 孤児掃除。どちらも内部でエラーを log に握り潰し、片方の失敗で
/// もう片方を止めない(背景処理なのでパス自体は決して落とさない)。
async fn reconcile_pass(state: &AppState) {
    converge_existence(state).await;
    cleanup_orphans(state).await;
}

/// 存在収束:`phase=running`(= DB が走っていると信じる)かつ未削除・digest 持ちの service で
/// コンテナが消えていれば、直近成功 deploy の digest で起こし直す。
///
/// 対象を `phase=running` に絞るのが **churn の安全弁** — failed / deploying / created / stopped は
/// 触らない(壊れたイメージを毎パス再起動し続ける暴走を作らない)。復活に失敗すれば run_digest が
/// phase=failed にし、次パスからは対象外になる(= 自己沈静化)。
async fn converge_existence(state: &AppState) {
    let candidates: Vec<(Uuid,)> = match sqlx::query_as(
        "SELECT s.resource_id
           FROM service_details s
           JOIN resources r ON r.id = s.resource_id
          WHERE r.kind = 'service' AND r.deleted_at IS NULL
            AND s.desired_state = 'running' AND s.phase = 'running'
            AND s.image_digest IS NOT NULL",
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(error = ?e, "reconcile: 候補一覧に失敗");
            return;
        }
    };

    let mut restored = 0usize;
    for (id,) in candidates {
        match docker::is_present(state, id).await {
            Ok(true) => continue, // 走っている(restarting 含む)→ 何もしない
            Ok(false) => {}
            Err(e) => {
                tracing::warn!(error = ?e, %id, "reconcile: 存在確認に失敗");
                continue;
            }
        }
        // コンテナが消えているのに DB は running → 純粋なドリフト。直近成功 digest で復活させる。
        let latest = match latest_succeeded_deploy(state, id).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = ?e, %id, "reconcile: 直近 deploy 取得に失敗");
                continue;
            }
        };
        // 成功 deploy が無い(理屈上は phase=running と矛盾)→ 触らない。
        let Some((digest, git_sha)) = latest else {
            continue;
        };
        tracing::info!(%id, "reconcile: コンテナ消失を検知 — 復活させる");
        audit(
            &state.db,
            None,
            "service.reconcile",
            id,
            json!({ "reason": "container_missing" }),
        )
        .await;
        // ロックは持たずに redeploy を呼ぶ(run_digest が内部で deploy_lock を取る — 二重取得回避)。
        // Reconcile 契機:run_digest がロック取得後に「まだ走るべきか」を再確認し、その間に stop が
        // 割り込んでいたら蘇らせない(stop レース防止)。
        if let Err(e) = redeploy(state, id, &digest, &git_sha, DeployTrigger::Reconcile).await {
            tracing::warn!(error = ?e, %id, "reconcile: 復活に失敗(phase=failed。次パスでは対象外)");
        } else {
            restored += 1;
        }
    }
    if restored > 0 {
        tracing::info!(restored, "reconcile: 存在収束 完了");
    }
}

/// 孤児掃除:
///  (a) `tsubomi.managed` だが DB に生きた service 行が無いコンテナ → 全停止 + 削除 + route 削除。
///  (b) service_id ラベルが欠落 / 不正な管理コンテナ → 個別削除。
///  (c) 生きた service 行に対応しない `svc-<id>.yml` route ファイル → 削除。
async fn cleanup_orphans(state: &AppState) {
    let managed = match docker::list_managed(state).await {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(error = ?e, "reconcile: 管理コンテナ一覧に失敗");
            return;
        }
    };

    // 処理済みの service_id(コンテナ掃除で route も消す + route ファイル走査の重複回避)。
    let mut handled: HashSet<Uuid> = HashSet::new();
    for (container_id, sid) in managed {
        let Some(sid) = sid else {
            // service_id を欠く管理コンテナ(手動 docker run / ラベル破損)→ 個別削除(best-effort)。
            tracing::warn!(container = %container_id, "reconcile: service_id ラベル無しの管理コンテナを削除");
            docker::remove_one(state, &container_id).await;
            continue;
        };
        if !handled.insert(sid) {
            continue; // 同 service の別コンテナは初回に全消し済み
        }
        match service_alive(state, sid).await {
            Ok(true) => {} // 生きた service → 触らない
            Ok(false) => remove_orphan_service(state, sid).await,
            Err(e) => tracing::warn!(error = ?e, %sid, "reconcile: 生存確認に失敗"),
        }
    }

    // route ファイルだけ残ったケース(コンテナは既に無い)。コンテナ掃除で処理済みの id は飛ばす。
    for sid in route::list_service_ids(state) {
        if handled.contains(&sid) {
            continue;
        }
        match service_alive(state, sid).await {
            Ok(false) => {
                tracing::info!(%sid, "reconcile: 孤児 route ファイルを削除");
                if let Err(e) = route::remove(state, sid) {
                    tracing::warn!(error = ?e, %sid, "reconcile: 孤児 route 削除に失敗");
                }
            }
            Ok(true) => {}
            Err(e) => tracing::warn!(error = ?e, %sid, "reconcile: 生存確認に失敗"),
        }
    }
}

/// 孤児 service(DB に生きた行が無い)のコンテナ全停止 + 削除 + route 削除。in-flight な
/// run_digest(削除直前に届いた hook 等)と競合しないよう当該 service の deploy_lock を取る。
async fn remove_orphan_service(state: &AppState, sid: Uuid) {
    let lock = state.deploy_lock(sid);
    let _guard = lock.lock().await;
    tracing::info!(%sid, "reconcile: 孤児コンテナを掃除(DB に生きた行が無い)");
    if let Err(e) = docker::stop_remove(state, sid).await {
        tracing::warn!(error = ?e, %sid, "reconcile: 孤児コンテナ削除に失敗");
    }
    if let Err(e) = route::remove(state, sid) {
        tracing::warn!(error = ?e, %sid, "reconcile: 孤児 route 削除に失敗");
    }
}

/// service の生きた行(未ソフト削除)が存在するか。
async fn service_alive(state: &AppState, id: Uuid) -> AppResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM resources WHERE id=$1 AND kind='service' AND deleted_at IS NULL)",
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?)
}
