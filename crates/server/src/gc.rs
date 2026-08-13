//! reconcile ループ(tech-design §3)の種。M1 時点の職務:
//!
//! - 認証まわりの期限切れ掃除(sessions / oauth_states / authcodes)
//! - ゴミ箱の期限到来(purge_after)→ 物理削除(trash::purge_resource を共有)
//! - 日次バックアップ(各テナント DB + 管制面の pg_dump + volumes の rsync、7 日保持)
//!
//! M3 でコンテナの存在収束・孤児掃除がここに合流する。

use crate::databases::audit;
use crate::mail;
use crate::state::AppState;
use crate::tenant;
use crate::trash;
use serde_json::{Value, json};
use std::path::Path;
use std::time::Duration;
use uuid::Uuid;

/// ハウスキーピング(認証掃除 + ゴミ箱 purge)の間隔。
const HOUSEKEEPING_INTERVAL: Duration = Duration::from_secs(3600);
/// 日次バックアップの間隔。
const BACKUP_INTERVAL: Duration = Duration::from_secs(24 * 3600);
/// バックアップ保持日数。
const BACKUP_RETAIN_DAYS: i64 = 7;
/// registry GC の実行時刻(UTC)。**固定時刻**であって間隔ではない — 理由は
/// [`spawn_registry_gc`] のドキュメント参照(起動 tick は本番事故で廃止)。
/// 19:05 UTC = 04:05 JST(push が最少の深夜帯。finance 系の quiet hours とも整合)。
const REGISTRY_GC_UTC: (u32, u32) = (19, 5);

pub fn spawn(state: AppState) {
    spawn_housekeeping(state.clone());
    spawn_backup(state.clone());
    spawn_registry_gc(state);
}

/// 1 時間毎:期限切れの認証行を掃除し、ゴミ箱の期限到来を物理削除する。
/// 最初の掃除は起動直後(interval の 0 tick)。
fn spawn_housekeeping(state: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(HOUSEKEEPING_INTERVAL);
        loop {
            tick.tick().await;
            sweep_auth(&state).await;
            sweep_trash(&state).await;
            check_disk(&state).await;
        }
    });
}

/// platform_config にディスク警告の状態を持つキー(level + notified_at で去重)。
const DISK_STATE_KEY: &str = "disk_alert_state";
/// 同じ level に留まっている間も、この間隔を超えたら 1 回だけ再喚起する。
const DISK_REALERT_AFTER: chrono::Duration = chrono::Duration::hours(24);

/// ディスク使用率を `df` で見て、warn/critical を跨いだら(or 同 level でも 24h 経過で)owner に
/// メールする。1h tick で呼ばれるので、毎回送ると受信箱が溢れる → platform_config の
/// 前回状態(level + notified_at)で去重する(§4.2)。best-effort:df 失敗 / 送信失敗は log のみ。
async fn check_disk(state: &AppState) {
    let cfg = &state.config;
    let Some(pct) = disk_used_pct(&cfg.volumes_dir).await else {
        return; // df 失敗(best-effort:警告は安全側に倒し止めない)
    };
    let level = if pct >= cfg.disk_critical_pct {
        "critical"
    } else if pct >= cfg.disk_warn_pct {
        "warn"
    } else {
        "ok"
    };

    let prev: Option<Value> =
        sqlx::query_scalar("SELECT value FROM platform_config WHERE key = $1")
            .bind(DISK_STATE_KEY)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
    let prev_level = prev
        .as_ref()
        .and_then(|v| v.get("level"))
        .and_then(Value::as_str)
        .unwrap_or("ok");
    let prev_notified = prev
        .as_ref()
        .and_then(|v| v.get("notified_at"))
        .and_then(Value::as_str)
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&chrono::Utc));

    let rank = |l: &str| match l {
        "critical" => 2,
        "warn" => 1,
        _ => 0,
    };
    let now = chrono::Utc::now();
    let escalated = rank(level) > rank(prev_level);
    // 再喚起は **同 level に留まっている間だけ** 24h 間隔で(de-escalation では送らない — §4.2)。
    // 初回観測(prev_notified なし)で同 level なら即送る。
    let stale = level == prev_level && prev_notified.is_none_or(|t| now - t > DISK_REALERT_AFTER);
    let should_notify = level != "ok" && (escalated || stale);

    // 通知できた時だけ notified_at を進める。送信失敗(Resend の一時障害など)では据え置き、
    // 次 tick で再試行する(さもないと 1 通も届かないまま 24h 沈黙してしまう)。
    let notified = if should_notify {
        let subject = format!("[tsubomi] ディスク使用率 {pct}%({level})");
        let body = format!(
            "tsubomi のディスク使用率が {pct}% に達しました(level={level}、warn={}% / critical={}%)。\n\
             監視パス:{}\n\n古いバックアップ / ゴミ箱の整理、不要な volume の削除、容量増設を検討してください。",
            cfg.disk_warn_pct,
            cfg.disk_critical_pct,
            cfg.volumes_dir.display()
        );
        // 宛先は owner_roster(DB、運用中に web で増減する)。env は冷启动种のみ。
        let owners = crate::owners::roster(&state.db).await;
        // HTML(React Email)+ text(上の素文面 fallback)。accent は level 別(warn=黄 / critical=赤)。
        // accent はテンプレの裸 CSS 値(style="…background-color:{{accent}}…")に入る。mail::render の
        // HTML エスケープは CSS インジェクションを守らないので、ここは**定数の 2 色のみ**に保つ(外部入力を入れない)。
        let accent = if level == "critical" { "#e05a5a" } else { "#f5c31c" };
        let pct_s = pct.to_string();
        let warn_s = cfg.disk_warn_pct.to_string();
        let crit_s = cfg.disk_critical_pct.to_string();
        let path_s = cfg.volumes_dir.display().to_string();
        let html = mail::render(
            mail::TPL_DISK_ALERT,
            &[
                ("accent", accent),
                ("pct", &pct_s),
                ("level", level),
                ("warn", &warn_s),
                ("critical", &crit_s),
                ("path", &path_s),
            ],
        );
        match mail::send(state, &owners, &subject, &html, &body).await {
            Ok(()) => {
                // target_resource は無い(platform 全体のイベント)ので nil uuid。詳細は detail に。
                audit(
                    &state.db,
                    None,
                    "disk.alert",
                    Uuid::nil(),
                    json!({ "used_pct": pct, "level": level }),
                    None,
                )
                .await;
                tracing::warn!(pct, level, "ディスク水位警告 — owner に通知");
                true
            }
            Err(e) => {
                tracing::warn!(error = ?e, "ディスク警告メールの送信に失敗 — 次 tick で再試行");
                false
            }
        }
    } else {
        false
    };

    // 状態を更新:level は常に最新へ。notified_at は通知に**成功した時だけ** now に進める
    // (同 level の再喚起判定 + 送信失敗時の再試行に使う)。
    let notified_at = if notified { Some(now) } else { prev_notified };
    let new_state = json!({
        "level": level,
        "used_pct": pct,
        "notified_at": notified_at.map(|t| t.to_rfc3339()),
    });
    if let Err(e) = sqlx::query(
        "INSERT INTO platform_config (key, value, updated_at) VALUES ($1, $2, now())
         ON CONFLICT (key) DO UPDATE SET value = $2, updated_at = now()",
    )
    .bind(DISK_STATE_KEY)
    .bind(&new_state)
    .execute(&state.db)
    .await
    {
        tracing::warn!(error = ?e, "ディスク警告状態の保存に失敗");
    }
}

/// 指定パスを含む filesystem の使用率(%)。`df` 解析は metrics と共有(`metrics::disk_metrics`)。
/// 解析失敗は None(best-effort)。
async fn disk_used_pct(path: &Path) -> Option<u8> {
    crate::metrics::disk_metrics(path).await.map(|d| d.pct)
}

async fn sweep_auth(state: &AppState) {
    for (what, sql) in [
        ("sessions", "DELETE FROM sessions WHERE expires_at <= now()"),
        (
            "oauth_states",
            "DELETE FROM oauth_states WHERE expires_at <= now()",
        ),
        (
            "authcodes",
            "DELETE FROM authcodes WHERE expires_at <= now()",
        ),
        // deploy hook のリプレイ防御 nonce。窓(MAX_SKEW=±300s)を十分越えた古い行は
        // もう照合されないので掃除する(m3-design §8。reconcile の職務だが DB ハウスキーピング
        // なのでここに同居 — reconcile は容器/route 収束に純化する)。
        (
            "deploy_nonces",
            "DELETE FROM deploy_nonces WHERE seen_at < now() - interval '1 hour'",
        ),
        // 危険操作の 6 桁コード。消費 / 再発行では**同一対象の行しか**消えないので
        // (admin/actions.rs)、別対象の未使用コードが期限切れ後も残り続ける。
        // `expires_at` の index は migration が既に張っている。
        (
            "admin_action_codes",
            "DELETE FROM admin_action_codes WHERE expires_at <= now()",
        ),
        // 監査ログの保持期限。体積は小さい(利用者の操作回数ぶんだけ増える。reconcile 等の
        // 自動処理は書かない)が、無期限に持つ理由も無いので上限を切る。**90 日**は
        // 「事後追跡には短すぎない / 無期限より運用が読める」の折衷(業界の下限が概ね 90 日)。
        // 監査は owner ガバナンスの唯一の一次情報なので、これ以上短くしないこと。
        (
            "audit_log",
            "DELETE FROM audit_log WHERE created_at < now() - interval '90 days'",
        ),
    ] {
        match sqlx::query(sql).execute(&state.db).await {
            Ok(r) if r.rows_affected() > 0 => {
                tracing::debug!(what, rows = r.rows_affected(), "gc swept");
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(what, error = ?e, "gc sweep failed"),
        }
    }
    sweep_old_deploys(state).await;
}

/// 古い deploys 行を掃除する。**`registry` の keep 窓に要る行は必ず残す**。
///
/// 素朴に「90 日より古い行を消す」とすると **rollback が壊れる**:
/// `registry::protect_and_expire_one` の keep 窓(現役 ∪ 直近
/// [`KEEP_SUCCEEDED_DEPLOYS`] distinct 成功版)は**この表から算出**されるので、行が消えると
/// 対応 manifest が「窓外」と判定されて registry から消え、宿主イメージも消え、戻る先が無くなる。
/// 逆に版数だけで切るのも駄目(低頻度デプロイの service が保護されない)。よって
/// **「90 日超」かつ「keep 窓の外」かつ「terminal」**の 3 条件が揃った行だけを消す。
///
/// 時間と版数のどちらか一方では正しくならない、というのがここの要点。
async fn sweep_old_deploys(state: &AppState) {
    let sql = "
        DELETE FROM deploys d
         WHERE d.created_at < now() - make_interval(days => $1)
           AND d.status IN ('succeeded','failed')
           AND NOT EXISTS (
             SELECT 1 FROM (
               SELECT service_id, image_digest,
                      ROW_NUMBER() OVER (
                        PARTITION BY service_id ORDER BY MAX(created_at) DESC
                      ) AS rn
                 FROM deploys
                WHERE status = 'succeeded'
                GROUP BY service_id, image_digest
             ) k
              WHERE k.service_id = d.service_id
                AND k.image_digest = d.image_digest
                AND k.rn <= $2
           )";
    match sqlx::query(sql)
        .bind(DEPLOY_RETAIN_DAYS as i32)
        .bind(crate::services::registry::KEEP_SUCCEEDED_DEPLOYS as i64)
        .execute(&state.db)
        .await
    {
        Ok(r) if r.rows_affected() > 0 => {
            tracing::info!(rows = r.rows_affected(), "gc: 古い deploys 行を掃除した");
        }
        Ok(_) => {}
        Err(e) => tracing::warn!(error = ?e, "gc: deploys の掃除に失敗"),
    }
}

/// deploys 行の保持日数。**rollback 窓(直近 5 版)はこれとは独立に守られる** —
/// `sweep_old_deploys` の 3 条件を参照。
const DEPLOY_RETAIN_DAYS: i64 = 90;

/// purge_after <= now() のゴミ箱を物理削除(reconcile の自動 purge)。
async fn sweep_trash(state: &AppState) {
    let expired: Vec<(Uuid, String, Option<Value>)> = match sqlx::query_as(
        "SELECT id, kind, trash_meta FROM resources
          WHERE purge_after IS NOT NULL AND purge_after <= now()",
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(error = ?e, "gc: list expired trash failed");
            return;
        }
    };

    for (id, kind, _meta) in expired {
        // purge_resource がロック内で「まだゴミ箱に居るか」を見直す — このスキャンの後に
        // restore されていたら None(触らない)。kind/meta もロック内で読み直される。
        match trash::purge_resource(state, id).await {
            Ok(Some(kind)) => {
                tracing::info!(%id, kind, "gc: purged expired trash");
                audit(
                    &state.db,
                    None,
                    "trash.purge.auto",
                    id,
                    json!({ "kind": kind }),
                    None,
                )
                .await;
            }
            Ok(None) => {
                tracing::info!(%id, kind, "gc: skip(スキャン後に復元された)");
            }
            Err(e) => tracing::warn!(error = ?e, %id, "gc: purge failed"),
        }
    }
}

/// 日次:各テナント DB + 管制面を pg_dump し、古いバックアップを掃除する。
/// 最初のバックアップは起動直後に走る(interval の 0 tick)。
fn spawn_backup(state: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(BACKUP_INTERVAL);
        loop {
            tick.tick().await;
            if let Err(e) = run_backup(&state).await {
                tracing::warn!(error = ?e, "gc: backup run failed");
            }
        }
    });
}

/// 日次:registry の未参照 blob を回収する(削除済み service の旧イメージ / 上書きで孤立した版)。
/// backup とは独立したタスク。**毎日 19:05 UTC 固定・起動直後 tick は廃止**:blob 掃除は
/// Pi で 10 分超走り、その間に「掃除対象と同一 digest」を再 push すると dedup が掃除前の
/// blob を見て書き込みを省略 → 直後に実体が掃除され **PUT 201 なのに GET 404** の假成功で
/// 毒される(2026-07-08 本番実証 — push 成功ログと pull manifest unknown が同時に成立)。
/// 起動 tick だと ship のたびに任意時刻で GC が走り、アクティブなデプロイと衝突するため、
/// push が最少の深夜帯に固定する。best-effort:失敗は log のみ。
fn spawn_registry_gc(state: AppState) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(until_next_utc(REGISTRY_GC_UTC.0, REGISTRY_GC_UTC.1)).await;
            // 旧版 manifest(index + 子)の期限切れを**先に**、blob 回収を後に —
            // manifest を消す判断は平台の keep 窓だけが行う(--delete-untagged は廃止。§10-E)。
            if let Err(e) = crate::services::registry::protect_and_expire_manifests(&state).await {
                tracing::warn!(error = ?e, "gc: registry manifest 期限切れ failed");
            }
            prune_host_images(&state).await;
            match crate::services::registry::garbage_collect(&state).await {
                Err(e) => tracing::warn!(error = ?e, "gc: registry garbage-collect failed"),
                // 掃除成功後は registry を再起動して descriptor cache の毒を抜く(理由は
                // registry::restart_registry のドキュメント — 假 201 の恒久毒を残さない)。
                Ok(()) => {
                    if let Err(e) = crate::services::registry::restart_registry(&state).await {
                        tracing::warn!(error = ?e, "gc: registry 再起動に失敗(掃除済み blob への push が假 201 になり得る — 手動 `docker restart tsubomi-registry` を推奨)");
                    }
                }
            }
        }
    });
}

/// 日次:**宿主 docker のイメージ**を掃除する(registry の manifest / blob とは別実体)。
///
/// 掃除口がどこにも無く、deploy 回数に比例して宿主のディスクが育っていた(1 版で数百 MB。
/// `docker image prune -f` は dangling しか消さないので `<repo>:<tag>` の残骸には当たらない)。
///
/// **問い合わせの順序が命**(`protect_and_expire_manifests` と同じ規律):
/// 1. **先に**イメージを列挙する(= 古い快照)
/// 2. **後で** keep 窓 / `deploying` を DB から読む(= 新しい快照。単一トランザクション)
///
/// 逆順にすると、その間に成功して現役化した digest が「keep に無いが列挙にある」状態になり、
/// **現役イメージを消してしまう**。この向きなら、列挙より後に作られた service / 現役化した
/// digest は必ず新しい快照の keep に載る(あるいはそもそも列挙に居ない)ので安全側に倒れる
/// — 取り逃した古い残骸は翌日回収される。
///
/// 「最近の deploy のイメージを消さない」年齢窓は `host_image_plan` が `deploys.created_at` で
/// 判断する(registry 側の 48h 下限と同じ時間源。docker の `ImageSummary.created` は
/// イメージ自身のビルド時刻なので使えない)。best-effort:失敗は log のみで blob 回収は続行する。
async fn prune_host_images(state: &AppState) {
    // ① 列挙(古い快照)。ここで撮った参照だけが候補。**空でも先に進む** — ④の孤児 repo 掃除は
    //    宿主イメージの有無とは無関係(宿主が綺麗でも registry に孤児が残り得る)。
    let refs = crate::services::docker::list_service_image_refs(state).await;
    // ② 計画を読む(新しい快照・単一トランザクション)。読めないなら**何もしない**
    //    (現役を守れないまま消すより残す方が安全 = fail-closed)。
    let plan =
        match crate::services::registry::host_image_plan(state, HOST_IMAGE_RECENT_SECS).await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = ?e, "gc: 宿主イメージの掃除計画を読めずスキップ(現役を守れないため何もしない)");
                return;
            }
        };
    // ③ 参照単位で掃除。
    if !refs.is_empty() {
        let removed =
            crate::services::docker::prune_host_images(state, &refs, &plan.keeps, &plan.skip, None)
                .await;
        if removed > 0 {
            tracing::info!(removed, "gc: 宿主イメージ参照を掃除した");
        }
    }
    // ④ DB に service 行が無い registry repo(deploy-source の取得が purge を追い越した残骸など)。
    //    `plan.keeps` のキーが「存在する service」の単一快照なので、それを既知集合として使う。
    let known: Vec<uuid::Uuid> = plan.keeps.keys().copied().collect();
    match crate::services::registry::delete_orphan_repos(state, &known).await {
        Ok(n) if n > 0 => tracing::info!(removed = n, "gc: 孤児 registry repo を掃除した"),
        Ok(_) => {}
        Err(e) => tracing::warn!(error = ?e, "gc: 孤児 registry repo の掃除に失敗"),
    }
}

/// 「直近この秒数の deploy の digest は宿主から消さない」窓(48h)。registry の manifest
/// 期限切れと同値・**同じ時間源**(`deploys.created_at`)— 失敗した deploy のイメージは
/// 再試行 / 診断にまだ要る(§10-E、2026-07-08 事故由来)。
const HOST_IMAGE_RECENT_SECS: i64 = 48 * 3600;

/// 次に UTC の hh:mm を迎えるまでの時間(既に過ぎていれば翌日の同時刻)。registry GC の
/// 固定時刻スケジュール用。負値になり得ない構成だが、時計後退等の異常時は 60s で安全側に倒す。
fn until_next_utc(hour: u32, minute: u32) -> Duration {
    let now = chrono::Utc::now();
    let today = now
        .date_naive()
        .and_hms_opt(hour, minute, 0)
        .expect("固定時刻は常に有効")
        .and_utc();
    let next = if today > now {
        today
    } else {
        today + chrono::Duration::days(1)
    };
    (next - now).to_std().unwrap_or(Duration::from_secs(60))
}

async fn run_backup(state: &AppState) -> anyhow::Result<()> {
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let dir = state.config.backup_dir.join(&date);
    std::fs::create_dir_all(&dir)?;

    // 生きているテナント DB を 1 つずつ dump(失敗は log のみ、他を止めない)。
    let dbs: Vec<(String,)> = sqlx::query_as(
        "SELECT d.pg_dbname FROM database_details d
           JOIN resources r ON r.id = d.resource_id
          WHERE r.deleted_at IS NULL",
    )
    .fetch_all(&state.db)
    .await?;

    let mut ok = 0usize;
    for (dbname,) in &dbs {
        let path = dir.join(format!("{dbname}.sql"));
        match tenant::dump_database(&state.config.tenant_admin_url, dbname, &path).await {
            Ok(()) => ok += 1,
            Err(e) => tracing::warn!(error = ?e, dbname, "gc: tenant db backup failed"),
        }
    }

    // 管制面(pg-platform)の全量。
    let platform_path = dir.join("platform.sql");
    if let Err(e) = tenant::dump_url(&state.config.database_url, &platform_path).await {
        tracing::warn!(error = ?e, "gc: platform backup failed");
    }

    // volumes の rsync スナップショット(§8)。失敗は log のみ(他を止めない)。
    // **前日のスナップショットを `--link-dest` の基準に渡す**:変わっていないファイルは
    // ハードリンクになるので、7 世代でもディスクは「実体 1 部 + 差分」に収まる
    // (無しだと live + 7 フルコピー = 常時 8 倍。Pi の共有 NVMe では効く)。
    if state.config.volumes_dir.exists() {
        let prev = (chrono::Utc::now() - chrono::Duration::days(1))
            .format("%Y-%m-%d")
            .to_string();
        let link_dest = state.config.backup_dir.join(&prev).join("volumes");
        if let Err(e) = rsync_dir(
            &state.config.volumes_dir,
            &dir.join("volumes"),
            link_dest.is_dir().then_some(link_dest.as_path()),
        )
        .await
        {
            tracing::warn!(error = ?e, "gc: volumes backup failed");
        }
    }

    prune_old_backups(state);
    tracing::info!(
        date,
        tenant_dbs = dbs.len(),
        tenant_ok = ok,
        "gc: backup done"
    );
    Ok(())
}

/// `rsync -a` でディレクトリ全体をバックアップ先へ複製する。pg_dump と同様に
/// 外部コマンドを TCP/ファイル経由で叩く(docker exec ではない)。
async fn rsync_dir(
    src: &std::path::Path,
    dest: &std::path::Path,
    link_dest: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(dest)?;
    // 末尾スラッシュ = 「src の中身を dest 直下へ」。--delete は付けない
    // (同日の再実行で消えても、削除済みファイルを残す方がバックアップとして保守的)。
    let src_arg = format!("{}/", src.display());
    let mut cmd = tokio::process::Command::new("rsync");
    cmd.arg("-a");
    // `--link-dest`:基準ディレクトリと同一内容のファイルはコピーせず**ハードリンク**にする。
    // 世代間で変わらないファイル(volume の大半)がディスクを二重に食わなくなる。基準が
    // 存在しない初回 / 欠番日は None で渡され、通常のフルコピーに倒れる(自己修復的)。
    // 注意:リンク先は**世代を跨いで共有される実体**なので、復元時に書き戻すなら必ず
    // コピーしてから触る(バックアップ側を直接編集すると他世代も変わる)。
    if let Some(base) = link_dest {
        cmd.arg(format!("--link-dest={}", base.display()));
    }
    let status = cmd.arg(&src_arg).arg(dest).status().await?;
    if !status.success() {
        anyhow::bail!("rsync が異常終了しました: {status}");
    }
    Ok(())
}

/// BACKUP_RETAIN_DAYS より古いバックアップ日次ディレクトリを削除する。
fn prune_old_backups(state: &AppState) {
    let cutoff = chrono::Utc::now() - chrono::Duration::days(BACKUP_RETAIN_DAYS);
    let entries = match std::fs::read_dir(&state.config.backup_dir) {
        Ok(e) => e,
        Err(_) => return, // まだ何も無い
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        // ディレクトリ名は YYYY-MM-DD。パースできて cutoff より古ければ削除。
        if let Ok(d) = chrono::NaiveDate::parse_from_str(name, "%Y-%m-%d")
            && d < cutoff.date_naive()
            && let Err(e) = std::fs::remove_dir_all(entry.path())
        {
            tracing::warn!(error = ?e, dir = name, "gc: prune backup failed");
        }
    }
}
