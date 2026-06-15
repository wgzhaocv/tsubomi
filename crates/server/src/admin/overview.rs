//! 管制面の可視化(M4 S1):overview(種別ごと集計)+ ranking(匿名行を使用量降順)。
//! 指標採集はオンデマンド + best-effort(§3.4):service=bollard stats(稼働中内存 + CPU%)、
//! database=`pg_database_size`、volume=`volumes::dir_usage`(時間予算で打ち切り)。取得不能は null / 0。

use crate::admin::require_viewer_web;
use crate::auth::AuthCtx;
use crate::error::AppResult;
use crate::services::docker;
use crate::state::AppState;
use crate::volumes;
use axum::Json;
use axum::extract::{Query, State};
use futures_util::stream::{self, StreamExt};
use serde::Deserialize;
use tsubomi_shared::{AdminOverviewKind, AdminOverviewResp, AdminResourceRow};
use uuid::Uuid;

/// overview に並べる種別の固定順。cache の「使用量」は **key 数**(§4.2。正確なメモリは
/// valkey に無い)— web は種別で単位表示を分ける(bytes / 個)。
const KINDS: [&str; 4] = ["service", "database", "volume", "cache"];

/// (resource_id, owner_name, kind, anon_seq, pg_dbname?, host_path?, namespace?)。
type RawRow = (
    Uuid,
    String,
    String,
    i32,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// 削除されていない全資源を跨ユーザで引き、各々の指標を**並行**に解決する。
/// pg_dbname / host_path / namespace は指標採集にだけ使い、DTO には載せない(匿名化の趣旨)。
async fn gather_rows(state: &AppState) -> AppResult<Vec<AdminResourceRow>> {
    let raw: Vec<RawRow> = sqlx::query_as(
        "SELECT r.id, COALESCE(u.name, u.email) AS owner_name, r.kind, r.anon_seq,
                d.pg_dbname, v.host_path, c.namespace
           FROM resources r
           JOIN users u ON u.id = r.user_id
           LEFT JOIN database_details d ON d.resource_id = r.id
           LEFT JOIN volume_details   v ON v.resource_id = r.id
           LEFT JOIN cache_details    c ON c.resource_id = r.id
          WHERE r.deleted_at IS NULL
            AND r.kind IN ('service','database','volume','cache')
          ORDER BY owner_name, r.kind, r.anon_seq",
    )
    .fetch_all(&state.db)
    .await?;

    // service stats は 1 件 ~1 秒・volume du も I/O 待ち・cache は SCAN → 並行に集める。
    // ただし**同時実行を上限つき**に(buffer_unordered)— 単一ホストで N 個の docker stats
    // ストリーム + du 走査 + valkey 接続を一斉に張ると箱が飽和するため(perf review P1)。
    // 順序はこの後 overview=集計 / ranking=usage 降順ソートで作るので不問。
    Ok(stream::iter(raw)
        .map(|r| resolve_row(state, r))
        .buffer_unordered(METRIC_CONCURRENCY)
        .collect()
        .await)
}

/// 指標採集の同時実行上限(単一 ARM64 ホストを飽和させない)。
const METRIC_CONCURRENCY: usize = 6;

async fn resolve_row(state: &AppState, raw: RawRow) -> AdminResourceRow {
    let (resource_id, owner_name, kind, anon_seq, pg_dbname, host_path, namespace) = raw;
    let anon_label = format!("{kind}{anon_seq}");
    let (usage_bytes, cpu_pct, running) = match kind.as_str() {
        "service" => match docker::stats(state, resource_id).await {
            Some(s) => (Some(s.mem_bytes), s.cpu_pct, Some(true)),
            None => (None, None, Some(false)),
        },
        "database" => (db_size(state, pg_dbname.as_deref()).await, None, None),
        "volume" => (dir_size_bytes(host_path.as_deref()).await, None, None),
        // cache の「使用量」は key 数(§4.2)。usage_bytes に載せ、web が単位を出し分ける。
        "cache" => match namespace.as_deref() {
            Some(ns) => (
                crate::valkey::count_keys(&state.valkey, ns).await,
                None,
                None,
            ),
            None => (None, None, None),
        },
        _ => (None, None, None),
    };
    AdminResourceRow {
        resource_id,
        owner_name,
        kind,
        anon_label,
        usage_bytes,
        cpu_pct,
        running,
    }
}

/// pg-tenant の DB サイズ(bytes)。失敗は None(best-effort)。
async fn db_size(state: &AppState, dbname: Option<&str>) -> Option<i64> {
    let dbname = dbname?;
    sqlx::query_scalar::<_, i64>("SELECT pg_database_size($1)")
        .bind(dbname)
        .fetch_one(&state.tenant_admin)
        .await
        .ok()
}

/// volume の占用(bytes)。M2 の `volumes::dir_usage`(symlink を辿らない再帰走査 +
/// 時間予算で打ち切り)を再利用する。走査は blocking なので spawn_blocking で。失敗は None。
async fn dir_size_bytes(path: Option<&str>) -> Option<i64> {
    let root = std::path::PathBuf::from(path?);
    let (size, ..) = tokio::task::spawn_blocking(move || volumes::dir_usage(&root))
        .await
        .ok()?
        .ok()?;
    i64::try_from(size).ok()
}

#[derive(Deserialize)]
pub struct RankingQuery {
    /// 絞り込み(service / database / volume)。未指定 = 全種別。
    pub kind: Option<String>,
    /// 上位 N に切り詰め。未指定 = 全件。
    pub limit: Option<usize>,
}

/// `GET /api/admin/ranking?kind=&limit=`:匿名行を使用量降順で。owner または viewer(web)。
pub async fn ranking(
    auth: AuthCtx,
    State(state): State<AppState>,
    Query(q): Query<RankingQuery>,
) -> AppResult<Json<Vec<AdminResourceRow>>> {
    require_viewer_web(&auth)?;
    let mut rows = gather_rows(&state).await?;
    if let Some(kind) = q.kind.as_deref() {
        rows.retain(|r| r.kind == kind);
    }
    // 使用量降順(取得不能の null は -1 = 末尾へ)。
    rows.sort_by_key(|r| std::cmp::Reverse(r.usage_bytes.unwrap_or(-1)));
    if let Some(limit) = q.limit {
        rows.truncate(limit);
    }
    Ok(Json(rows))
}

/// `GET /api/admin/overview`:種別ごと総数 + 総使用量 + 資源保有ユーザ数。owner または viewer(web)。
pub async fn overview(
    auth: AuthCtx,
    State(state): State<AppState>,
) -> AppResult<Json<AdminOverviewResp>> {
    require_viewer_web(&auth)?;
    let rows = gather_rows(&state).await?;

    let kinds = KINDS
        .iter()
        .map(|&k| {
            let mut count = 0i64;
            let mut total_usage_bytes = 0i64;
            for r in rows.iter().filter(|r| r.kind == k) {
                count += 1;
                total_usage_bytes += r.usage_bytes.unwrap_or(0);
            }
            AdminOverviewKind {
                kind: k.to_string(),
                count,
                total_usage_bytes,
            }
        })
        .collect();

    // 資源を 1 つ以上持つユーザ数(owner_name は同名衝突しうるので user_id で distinct)。
    let user_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT user_id) FROM resources WHERE deleted_at IS NULL",
    )
    .fetch_one(&state.db)
    .await?;

    Ok(Json(AdminOverviewResp { user_count, kinds }))
}
