//! reconcile ループ(tech-design §3)の種。M1 時点の職務:
//!
//! - 認証まわりの期限切れ掃除(sessions / oauth_states / authcodes)
//! - ゴミ箱の期限到来(purge_after)→ 物理削除(trash::purge_resource を共有)
//! - 日次バックアップ(各テナント DB + 管制面の pg_dump、7 日保持)
//!
//! M3 でコンテナの存在収束・孤児掃除がここに合流する。

use crate::databases::audit;
use crate::state::AppState;
use crate::tenant;
use crate::trash;
use serde_json::{Value, json};
use std::time::Duration;
use uuid::Uuid;

/// ハウスキーピング(認証掃除 + ゴミ箱 purge)の間隔。
const HOUSEKEEPING_INTERVAL: Duration = Duration::from_secs(3600);
/// 日次バックアップの間隔。
const BACKUP_INTERVAL: Duration = Duration::from_secs(24 * 3600);
/// バックアップ保持日数。
const BACKUP_RETAIN_DAYS: i64 = 7;

pub fn spawn(state: AppState) {
    spawn_housekeeping(state.clone());
    spawn_backup(state);
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
        }
    });
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

    prune_old_backups(state);
    tracing::info!(
        date,
        tenant_dbs = dbs.len(),
        tenant_ok = ok,
        "gc: backup done"
    );
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
