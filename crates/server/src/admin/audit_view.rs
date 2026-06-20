//! 監査ログ閲覧(M4 S4、owner 専用・web)。書く一方だった audit_log を読む口を足す
//! (「監査 = ガバナンス可視性のもう半分」第 4 層 §7)。キーセット分頁(id DESC、OFFSET 不使用)+
//! action の前方一致フィルタ。actor / target_user は真名で join、target_resource は UUID のまま。

use crate::admin::require_owner_web;
use crate::auth::AuthCtx;
use crate::error::AppResult;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Query, State};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use tsubomi_shared::AuditEntryDto;
use uuid::Uuid;

const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 200;

#[derive(Deserialize)]
pub struct AuditQuery {
    /// この id より小さい行を返す(キーセット分頁。未指定 = 最新から)。
    pub cursor: Option<i64>,
    /// 1 頁の件数(既定 50、上限 200)。
    pub limit: Option<i64>,
    /// action の前方一致フィルタ(例 'owner.' で代理操作だけ)。未指定 = 全件。
    pub action: Option<String>,
}

type Row = (
    i64,
    DateTime<Utc>,
    String,
    Option<String>,
    Option<String>,
    Option<Uuid>,
    Option<serde_json::Value>,
    Option<String>,
);

/// `GET /api/admin/audit?cursor=&limit=&action=`:監査ログ(新しい順)。owner(web)のみ。
pub async fn list(
    auth: AuthCtx,
    State(state): State<AppState>,
    Query(q): Query<AuditQuery>,
) -> AppResult<Json<Vec<AuditEntryDto>>> {
    require_owner_web(&auth)?;
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let action_prefix = q.action.filter(|s| !s.is_empty());

    let rows: Vec<Row> = sqlx::query_as(
        "SELECT a.id, a.created_at, a.action,
                COALESCE(actor.name, actor.email),
                COALESCE(tu.name, tu.email),
                a.target_resource, a.detail, a.client_ip
           FROM audit_log a
           LEFT JOIN users actor ON actor.id = a.actor_id
           LEFT JOIN users tu    ON tu.id    = a.target_user
          WHERE ($1::bigint IS NULL OR a.id < $1)
            AND ($2::text   IS NULL OR a.action LIKE $2 || '%')
          ORDER BY a.id DESC
          LIMIT $3",
    )
    .bind(q.cursor)
    .bind(action_prefix)
    .bind(limit)
    .fetch_all(&state.db)
    .await?;

    let items = rows
        .into_iter()
        .map(
            |(
                id,
                created_at,
                action,
                actor_name,
                target_user_name,
                target_resource,
                detail,
                client_ip,
            )| {
                AuditEntryDto {
                    id,
                    created_at,
                    action,
                    actor_name,
                    target_user_name,
                    target_resource,
                    detail,
                    client_ip,
                }
            },
        )
        .collect();
    Ok(Json(items))
}
