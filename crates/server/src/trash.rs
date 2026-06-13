//! ゴミ箱(tech-design §8)。ソフト削除されたリソースの一覧 / 復元 / 永久削除。
//! 通用の壳 + kind 毎の派発(M1 は database のみ実装)。
//!
//! 物理削除のコア(`purge_resource`)は gc(reconcile)からも呼ばれる:
//! ユーザが「永久に削除」したときと、purge_after 到来で自動削除するときで同じ経路。

use crate::auth::AuthCtx;
use crate::databases::audit;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::tenant::{self, DbNames};
use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::path::PathBuf;
use tsubomi_shared::TrashItemDto;
use uuid::Uuid;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/trash", get(list))
        .route("/trash/{id}/restore", post(restore))
        .route("/trash/{id}", delete(purge))
}

/// 所有者チェック付きでゴミ箱の (kind, trash_meta) を引く。restore / purge が共有。
/// 見つからない / 他ユーザ / 未削除は 404 に収束。
async fn fetch_trashed(db: &PgPool, id: Uuid, user_id: Uuid) -> AppResult<(String, Option<Value>)> {
    let row: Option<(String, Option<Value>)> = sqlx::query_as(
        "SELECT kind, trash_meta FROM resources
          WHERE id = $1 AND user_id = $2 AND deleted_at IS NOT NULL",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    row.ok_or(AppError::NotFound)
}

/// trash_meta から dump パスを取り出す(無ければ trash_dir/<id>.sql に既定)。
fn dump_path(meta: &Option<Value>, trash_dir: &std::path::Path, id: Uuid) -> PathBuf {
    meta.as_ref()
        .and_then(|m| m.get("dump_path"))
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .unwrap_or_else(|| trash_dir.join(format!("{id}.sql")))
}

/// trash 一覧の行(id, kind, display_name, deleted_at, purge_after)。
type TrashRow = (Uuid, String, String, DateTime<Utc>, Option<DateTime<Utc>>);

/// `GET /api/trash`:ソフト削除済みリソース一覧。
pub async fn list(
    auth: AuthCtx,
    State(state): State<AppState>,
) -> AppResult<Json<Vec<TrashItemDto>>> {
    let rows: Vec<TrashRow> = sqlx::query_as(
        "SELECT id, kind, display_name, deleted_at, purge_after
           FROM resources
          WHERE user_id = $1 AND deleted_at IS NOT NULL
          ORDER BY deleted_at DESC",
    )
    .bind(auth.user_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(
        rows.into_iter()
            .map(
                |(id, kind, display_name, deleted_at, purge_after)| TrashItemDto {
                    id,
                    kind,
                    display_name,
                    deleted_at,
                    purge_after,
                },
            )
            .collect(),
    ))
}

/// `POST /api/trash/:id/restore`:復元。kind で派発(M1 は database のみ)。
pub async fn restore(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<StatusCode> {
    let (kind, trash_meta) = fetch_trashed(&state.db, id, auth.user_id).await?;

    match kind.as_str() {
        "database" => restore_database(&state, id, &trash_meta).await?,
        other => {
            return Err(AppError::BadRequest(format!("復元未対応の種別: {other}")));
        }
    }

    // 物理復元が成功してから resource を active に戻す。**これを dump 削除より先に**:
    // ここで失敗しても dump が残り、gc に消されず再 restore できる(データを失わない)。
    sqlx::query(
        "UPDATE resources SET deleted_at = NULL, purge_after = NULL, trash_meta = NULL WHERE id = $1",
    )
    .bind(id)
    .execute(&state.db)
    .await?;

    // active 化が確定したので dump を片付ける(残っても無害なのでベストエフォート)。
    let _ = std::fs::remove_file(dump_path(&trash_meta, &state.config.trash_dir, id));

    audit(&state.db, Some(auth.user_id), "db.restore", id, json!({})).await;
    Ok(StatusCode::NO_CONTENT)
}

/// database の復元:role は残っているので DATABASE を再作成して dump を流し込む。
/// dump 削除は呼び出し側(deleted_at クリア後)が行う。
async fn restore_database(state: &AppState, id: Uuid, trash_meta: &Option<Value>) -> AppResult<()> {
    let (dbname,): (String,) =
        sqlx::query_as("SELECT pg_dbname FROM database_details WHERE resource_id = $1")
            .bind(id)
            .fetch_one(&state.db)
            .await?;
    let names = DbNames::from_dbname(dbname);

    // 作りかけの空 DB を残さないよう、dump を先に検証してから DATABASE を作る。
    let dump = dump_path(trash_meta, &state.config.trash_dir, id);
    if !dump.exists() {
        return Err(AppError::BadRequest(
            "バックアップ(dump)が見つからないため復元できません".into(),
        ));
    }

    tenant::recreate_for_restore(&state.tenant_admin, &state.config.tenant_admin_url, &names)
        .await?;

    if let Err(e) = tenant::restore_database(
        &state.config.tenant_admin_url,
        &names.dbname,
        &names.owner,
        &dump,
    )
    .await
    {
        // reload 失敗 → 作りかけの DATABASE を落とす(role は残す)。
        let _ = tenant::drop_database(&state.tenant_admin, &names.dbname).await;
        return Err(e);
    }
    Ok(())
}

/// `DELETE /api/trash/:id`:永久削除(ユーザ操作)。
pub async fn purge(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<StatusCode> {
    let (kind, trash_meta) = fetch_trashed(&state.db, id, auth.user_id).await?;

    purge_resource(&state, id, &kind, &trash_meta).await?;
    audit(
        &state.db,
        Some(auth.user_id),
        "trash.purge",
        id,
        json!({ "kind": kind }),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// 物理削除のコア。ユーザの永久削除と reconcile の自動 purge が共有する。
/// kind 毎に実体(tenant DB / role / dump)を片付けてから行を物理削除する
/// (resources の行を消すと detail / roles はカスケードで消える)。
pub(crate) async fn purge_resource(
    state: &AppState,
    id: Uuid,
    kind: &str,
    trash_meta: &Option<Value>,
) -> AppResult<()> {
    if kind == "database" {
        if let Ok((dbname,)) = sqlx::query_as::<_, (String,)>(
            "SELECT pg_dbname FROM database_details WHERE resource_id = $1",
        )
        .bind(id)
        .fetch_one(&state.db)
        .await
        {
            let names = DbNames::from_dbname(dbname);
            // 実体の掃除が失敗したら **行を消さない**(消すと管理対象外の活きた DB /
            // role を取り残す)。エラーを伝播し、行は次回まで残す。
            // DROP は IF EXISTS なので既に消えていても成功する。
            tenant::drop_database_and_roles(&state.tenant_admin, &names).await?;
        }
        // dump ファイルの削除はベストエフォート(残っても無害)。
        let dump = dump_path(trash_meta, &state.config.trash_dir, id);
        let _ = std::fs::remove_file(&dump);
    }

    sqlx::query("DELETE FROM resources WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(())
}
