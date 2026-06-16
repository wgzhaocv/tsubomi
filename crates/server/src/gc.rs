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
/// registry の未参照 blob 回収(GC)の間隔。backup と同じ日次だが別タスクにする
/// (関心事が無関係 + 遅い backup に GC が引きずられないように)。
const REGISTRY_GC_INTERVAL: Duration = Duration::from_secs(24 * 3600);

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
    ] {
        match sqlx::query(sql).execute(&state.db).await {
            Ok(r) if r.rows_affected() > 0 => {
                tracing::debug!(what, rows = r.rows_affected(), "gc swept");
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(what, error = ?e, "gc sweep failed"),
        }
    }
}

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

    for (id, kind, meta) in expired {
        match trash::purge_resource(state, id, &kind, &meta).await {
            Ok(()) => {
                tracing::info!(%id, kind, "gc: purged expired trash");
                audit(
                    &state.db,
                    None,
                    "trash.purge.auto",
                    id,
                    json!({ "kind": kind }),
                )
                .await;
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
/// backup とは独立したタスク。並行 push との競合を避けるため 1h ではなく日次。最初の回収は
/// 起動直後(interval の 0 tick)。best-effort:失敗は log のみ。
fn spawn_registry_gc(state: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(REGISTRY_GC_INTERVAL);
        loop {
            tick.tick().await;
            if let Err(e) = crate::services::registry::garbage_collect(&state).await {
                tracing::warn!(error = ?e, "gc: registry garbage-collect failed");
            }
        }
    });
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
    if state.config.volumes_dir.exists()
        && let Err(e) = rsync_dir(&state.config.volumes_dir, &dir.join("volumes")).await
    {
        tracing::warn!(error = ?e, "gc: volumes backup failed");
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
async fn rsync_dir(src: &std::path::Path, dest: &std::path::Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dest)?;
    // 末尾スラッシュ = 「src の中身を dest 直下へ」。--delete は付けない
    // (同日の再実行で消えても、削除済みファイルを残す方がバックアップとして保守的)。
    let src_arg = format!("{}/", src.display());
    let status = tokio::process::Command::new("rsync")
        .arg("-a")
        .arg(&src_arg)
        .arg(dest)
        .status()
        .await?;
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
