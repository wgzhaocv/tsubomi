//! database リソースの API ハンドラ(tech-design §6 の database 面)。
//! web と CLI は同一ハンドラの 2 入口 — 認証 extractor(AuthCtx)だけが分岐点。
//!
//! 背骨:平台が「期望状態」を resources / database_details / database_roles に持ち、
//! 現実(pg-tenant の DB / role)をそこへ収束させる。create は tenant DDL を先に
//! 流し、成功してから platform 行を入れる(失敗時は tenant 側を掃除)。

use crate::auth::AuthCtx;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::tenant::{self, DbNames};
use crate::validate;
use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use serde_json::json;
use sqlx::{Column, Connection, Executor, PgPool, Row, SqlSafeStr};
use tsubomi_shared::{
    ConnectionUrlResp, CreateDatabaseReq, DatabaseDto, QueryReq, QueryResp, QueryResultSet,
    RenameDatabaseReq, ResourceDto,
};
use uuid::Uuid;

const MAX_NAME_LEN: usize = 64;
/// web SQL が返す最大行数(超過は truncated=true で切り詰め)。
const MAX_QUERY_ROWS: usize = 1000;

/// `list` / `get_one` の行(id, display_name, anon_seq, created_at, rotated_at)。
type DbRow = (Uuid, String, i32, DateTime<Utc>, Option<DateTime<Utc>>);
/// `list_resources` の行(+ kind, deleted_at)。
type ResourceRow = (
    Uuid,
    String,
    String,
    i32,
    DateTime<Utc>,
    Option<DateTime<Utc>>,
);

/// DbRow → DatabaseDto(list と get_one が共有)。DatabaseDto も DbRow も外部型なので
/// From は孤児規則で書けず、自由関数にする。
fn db_row_to_dto((id, display_name, anon_seq, created_at, rotated_at): DbRow) -> DatabaseDto {
    DatabaseDto {
        id,
        display_name,
        anon_seq,
        created_at,
        rotated_at,
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/resources", get(list_resources))
        .route("/databases", get(list).post(create))
        .route("/databases/{id}", get(get_one).patch(rename).delete(delete))
        .route("/databases/{id}/url", get(url))
        .route("/databases/{id}/rotate", post(rotate))
        .route("/databases/{id}/query", post(query))
}

// ===== 監査 =====

/// audit_log への記録。ベストエフォート(失敗してもリクエストは成功扱い、ログだけ残す)。
/// actor=None はシステム操作(reconcile の自動 purge など)。trash / gc から再利用する。
pub(crate) async fn audit(
    db: &PgPool,
    actor: Option<Uuid>,
    action: &str,
    target: Uuid,
    detail: serde_json::Value,
) {
    let r = sqlx::query(
        "INSERT INTO audit_log (actor_id, action, target_resource, detail) VALUES ($1,$2,$3,$4)",
    )
    .bind(actor)
    .bind(action)
    .bind(target)
    .bind(detail)
    .execute(db)
    .await;
    if let Err(e) = r {
        tracing::warn!(error = ?e, action, "audit insert failed");
    }
}

/// owner の代理操作用:`target_user`(誰の資源を触ったか)も埋める audit。
/// 通常の `audit` は target_user を埋めない(本人操作なので actor = 所有者)。owner が他人の
/// 資源を stop/delete する「最後の砦」(M4 S3)はここを使い、誰の何を動かしたかを残す。
pub(crate) async fn audit_with_target(
    db: &PgPool,
    actor: Uuid,
    action: &str,
    target_resource: Uuid,
    target_user: Uuid,
    detail: serde_json::Value,
) {
    let r = sqlx::query(
        "INSERT INTO audit_log (actor_id, action, target_resource, target_user, detail)
         VALUES ($1,$2,$3,$4,$5)",
    )
    .bind(actor)
    .bind(action)
    .bind(target_resource)
    .bind(target_user)
    .bind(detail)
    .execute(db)
    .await;
    if let Err(e) = r {
        tracing::warn!(error = ?e, action, "audit insert failed");
    }
}

// ===== 共通の取得 =====

/// 所有者チェック付きで human role の (pg_dbname, pg_role, password_enc) を引く。
/// 見つからない / 他ユーザ / 削除済みは 404 に収束。
async fn fetch_human(db: &PgPool, user_id: Uuid, id: Uuid) -> AppResult<(String, String, Vec<u8>)> {
    let row: Option<(String, String, Vec<u8>)> = sqlx::query_as(
        "SELECT d.pg_dbname, ro.pg_role, ro.password_enc
           FROM resources r
           JOIN database_details d ON d.resource_id = r.id
           JOIN database_roles ro ON ro.resource_id = r.id AND ro.role_kind = 'human'
          WHERE r.id = $1 AND r.user_id = $2 AND r.kind = 'database' AND r.deleted_at IS NULL",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    row.ok_or(AppError::NotFound)
}

fn build_url(state: &AppState, role: &str, password: &str, dbname: &str) -> String {
    let cfg = &state.config;
    format!(
        "postgres://{role}:{password}@{}:{}/{dbname}?sslmode={}",
        cfg.db_public_host, cfg.db_public_port, cfg.db_sslmode
    )
}

// ===== ハンドラ =====

/// sqlx の UNIQUE 制約違反(23505)を Conflict(409)へ変換し、それ以外は Sqlx(500)
/// のまま返す。重複(同名)を「内部エラー」に潰さず、原因の分かる 4xx にするため。
/// volumes など他のリソースモジュールからも再利用する。
pub(crate) fn map_unique(e: sqlx::Error, conflict_msg: impl Into<String>) -> AppError {
    match &e {
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            AppError::Conflict(conflict_msg.into())
        }
        _ => AppError::Sqlx(e),
    }
}

/// `POST /api/databases`:DB 作成。
pub async fn create(
    auth: AuthCtx,
    State(state): State<AppState>,
    Json(req): Json<CreateDatabaseReq>,
) -> AppResult<(StatusCode, Json<DatabaseDto>)> {
    let display_name = validate::name(&req.name, MAX_NAME_LEN)?;

    // 同名チェックを tenant DDL の前に行い、無駄な CREATE/DROP DATABASE を避ける。
    // UNIQUE (user_id, kind, display_name) はゴミ箱内(deleted_at)も含むので
    // ここも全行を見る。競合(同時 create)は insert_rows の UNIQUE が最終ガード。
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM resources WHERE user_id = $1 AND kind = 'database' AND display_name = $2)",
    )
    .bind(auth.user_id)
    .bind(&display_name)
    .fetch_one(&state.db)
    .await?;
    if exists {
        return Err(AppError::Conflict(format!(
            "データベース名 '{display_name}' は既に使われています(ゴミ箱内を含む)。別の名前にしてください"
        )));
    }

    let names = DbNames::generate();
    let app_pw = tenant::gen_password();
    let human_pw = tenant::gen_password();

    // 1. tenant 側 DDL を先に(CREATE DATABASE はトランザクション不可)。
    if let Err(e) = tenant::create_database(
        &state.tenant_admin,
        &state.config.tenant_admin_url,
        &names,
        &app_pw,
        &human_pw,
    )
    .await
    {
        // 途中で失敗 → 残骸を掃除(「platform 行が在る ⇒ tenant DB が在る」を保つ)。
        let _ = tenant::drop_database_and_roles(&state.tenant_admin, &names).await;
        return Err(e);
    }

    // 2. platform 側にメタを記録(パスワードは暗号化)。
    let app_enc = state.crypto.encrypt(&app_pw)?;
    let human_enc = state.crypto.encrypt(&human_pw)?;
    let dto = match insert_rows(
        &state.db,
        auth.user_id,
        &display_name,
        &names,
        app_enc,
        human_enc,
    )
    .await
    {
        Ok(dto) => dto,
        Err(e) => {
            // platform 挿入に失敗 → tenant 側をロールバック。
            let _ = tenant::drop_database_and_roles(&state.tenant_admin, &names).await;
            return Err(e);
        }
    };

    audit(
        &state.db,
        Some(auth.user_id),
        "db.create",
        dto.id,
        json!({ "display_name": display_name, "pg_dbname": names.dbname }),
    )
    .await;
    Ok((StatusCode::CREATED, Json(dto)))
}

async fn insert_rows(
    db: &PgPool,
    user_id: Uuid,
    display_name: &str,
    names: &DbNames,
    app_enc: Vec<u8>,
    human_enc: Vec<u8>,
) -> AppResult<DatabaseDto> {
    let mut tx = db.begin().await?;

    // ユーザ単位で anon_seq の採番を直列化(同時 create の競合を防ぐ)。
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1::text), 42)")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;

    let anon_seq: i32 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(anon_seq),0)+1 FROM resources WHERE user_id=$1 AND kind='database'",
    )
    .bind(user_id)
    .fetch_one(&mut *tx)
    .await?;

    let (id, created_at): (Uuid, DateTime<Utc>) = sqlx::query_as(
        "INSERT INTO resources (user_id, kind, display_name, anon_seq)
              VALUES ($1, 'database', $2, $3)
         RETURNING id, created_at",
    )
    .bind(user_id)
    .bind(display_name)
    .bind(anon_seq)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| {
        map_unique(
            e,
            format!("データベース名 '{display_name}' は既に使われています"),
        )
    })?;

    sqlx::query("INSERT INTO database_details (resource_id, pg_dbname) VALUES ($1, $2)")
        .bind(id)
        .bind(&names.dbname)
        .execute(&mut *tx)
        .await?;

    for (kind, role, enc) in [
        ("app", &names.app, app_enc),
        ("human", &names.human, human_enc),
    ] {
        sqlx::query(
            "INSERT INTO database_roles (resource_id, role_kind, pg_role, password_enc)
                  VALUES ($1, $2, $3, $4)",
        )
        .bind(id)
        .bind(kind)
        .bind(role)
        .bind(enc)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(DatabaseDto {
        id,
        display_name: display_name.to_owned(),
        anon_seq,
        created_at,
        rotated_at: None,
    })
}

/// `GET /api/databases`:自分の DB 一覧。
pub async fn list(
    auth: AuthCtx,
    State(state): State<AppState>,
) -> AppResult<Json<Vec<DatabaseDto>>> {
    let rows: Vec<DbRow> = sqlx::query_as(
        "SELECT r.id, r.display_name, r.anon_seq, r.created_at, d.rotated_at
           FROM resources r
           JOIN database_details d ON d.resource_id = r.id
          WHERE r.user_id = $1 AND r.kind = 'database' AND r.deleted_at IS NULL
          ORDER BY r.anon_seq",
    )
    .bind(auth.user_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(rows.into_iter().map(db_row_to_dto).collect()))
}

/// `PATCH /api/databases/:id`:表示名のリネーム。display_name(resources)だけ更新し、
/// pg_dbname / role / 接続文字列は一切変えない(リネームは UI 上のラベル変更)。
pub async fn rename(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<RenameDatabaseReq>,
) -> AppResult<Json<DatabaseDto>> {
    let display_name = validate::name(&req.name, MAX_NAME_LEN)?;

    // 所有者の自分の DB のみ更新。RETURNING で更新後の行をそのまま DTO 化する。
    let row: Option<DbRow> = sqlx::query_as(
        "UPDATE resources r SET display_name = $1
           FROM database_details d
          WHERE r.id = $2 AND r.user_id = $3 AND r.kind = 'database' AND r.deleted_at IS NULL
            AND d.resource_id = r.id
      RETURNING r.id, r.display_name, r.anon_seq, r.created_at, d.rotated_at",
    )
    .bind(&display_name)
    .bind(id)
    .bind(auth.user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| {
        map_unique(
            e,
            format!("データベース名 '{display_name}' は既に使われています"),
        )
    })?;

    let row = row.ok_or(AppError::NotFound)?;
    audit(
        &state.db,
        Some(auth.user_id),
        "db.rename",
        id,
        json!({ "display_name": display_name }),
    )
    .await;
    Ok(Json(db_row_to_dto(row)))
}

/// `GET /api/databases/:id`:単体。
pub async fn get_one(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<Json<DatabaseDto>> {
    let row: Option<DbRow> = sqlx::query_as(
        "SELECT r.id, r.display_name, r.anon_seq, r.created_at, d.rotated_at
           FROM resources r
           JOIN database_details d ON d.resource_id = r.id
          WHERE r.id = $1 AND r.user_id = $2 AND r.kind = 'database' AND r.deleted_at IS NULL",
    )
    .bind(id)
    .bind(auth.user_id)
    .fetch_optional(&state.db)
    .await?;

    let row = row.ok_or(AppError::NotFound)?;
    Ok(Json(db_row_to_dto(row)))
}

/// `GET /api/databases/:id/url`:外部(human)接続文字列。**パスワードそのもの**。
pub async fn url(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<axum::response::Response> {
    let (dbname, role, enc) = fetch_human(&state.db, auth.user_id, id).await?;
    let pw = state.crypto.decrypt(&enc)?;
    Ok(crate::respond::no_store(ConnectionUrlResp {
        url: build_url(&state, &role, &pw, &dbname),
    }))
}

/// `POST /api/databases/:id/rotate`:human のパスワードを差し替える(非破壊 — app は不変)。
pub async fn rotate(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<axum::response::Response> {
    let (dbname, role, _) = fetch_human(&state.db, auth.user_id, id).await?;
    let new_pw = tenant::gen_password();
    tenant::rotate_password(&state.tenant_admin, &role, &new_pw).await?;

    // 新パスワードの保存と rotated_at を 1 トランザクションで(片方だけ書けてズレない)。
    let enc = state.crypto.encrypt(&new_pw)?;
    let mut tx = state.db.begin().await?;
    sqlx::query(
        "UPDATE database_roles SET password_enc = $1 WHERE resource_id = $2 AND role_kind = 'human'",
    )
    .bind(enc)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE database_details SET rotated_at = now() WHERE resource_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    audit(&state.db, Some(auth.user_id), "db.rotate", id, json!({})).await;
    Ok(crate::respond::no_store(ConnectionUrlResp {
        url: build_url(&state, &role, &new_pw, &dbname),
    }))
}

/// `POST /api/databases/:id/query`:web SQL(L1 session 認証 + L2 所有者 + L3 その DB
/// 自身の human role で接続。admin は絶対に使わない — §7)。
pub async fn query(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<QueryReq>,
) -> AppResult<Json<QueryResp>> {
    let (dbname, role, enc) = fetch_human(&state.db, auth.user_id, id).await?;
    let pw = state.crypto.decrypt(&enc)?;

    let mut conn =
        tenant::connect_as_human(&state.config.tenant_admin_url, &role, &pw, &dbname).await?;

    // 暴走クエリ対策(二重)。statement_timeout はユーザが `SET statement_timeout=0`
    // で外せるので、サーバ側の tokio タイムアウトを硬い上限として被せる(超過で
    // 接続を落とす = クエリも中断)。
    conn.execute("SET statement_timeout = '10s'").await?;

    // raw_sql:単純クエリプロトコル(複数文 OK、値は text)。これは web SQL —
    // 任意 SQL の実行が目的なので AssertSqlSafe で包む(安全は L1/L2/L3 認証が担保)。
    // fetch_many で「行(Right)/ 文完了(Left)」を流し、文ごとに 1 集合へまとめる
    // (fetch_all だと全文の行が混ざり、別 SELECT の値が違う列に並んでしまう)。
    let collect = async {
        let stream = sqlx::raw_sql(sqlx::AssertSqlSafe(req.sql.as_str())).fetch_many(&mut conn);
        futures_util::pin_mut!(stream);

        let mut results: Vec<QueryResultSet> = Vec::new();
        let mut cols: Vec<String> = Vec::new();
        let mut rows: Vec<Vec<Option<String>>> = Vec::new();
        let mut total: usize = 0; // 上限で切り詰める前の実件数

        while let Some(item) = stream.next().await {
            match item.map_err(|e| AppError::BadRequest(format!("{e}")))? {
                // 文の完了 ⇒ 直前までの行を 1 集合として確定。
                sqlx::Either::Left(done) => {
                    results.push(QueryResultSet {
                        columns: std::mem::take(&mut cols),
                        rows: std::mem::take(&mut rows),
                        row_count: total.min(MAX_QUERY_ROWS),
                        truncated: total > MAX_QUERY_ROWS,
                        rows_affected: done.rows_affected(),
                    });
                    total = 0;
                }
                // 行。最初の行で列名を確定し、上限まで text 化して貯める。
                sqlx::Either::Right(row) => {
                    if cols.is_empty() {
                        cols = row.columns().iter().map(|c| c.name().to_string()).collect();
                    }
                    total += 1;
                    if rows.len() < MAX_QUERY_ROWS {
                        rows.push(
                            (0..row.len())
                                .map(|i| tenant::col_to_string(&row, i))
                                .collect(),
                        );
                    }
                }
            }
        }
        Ok::<_, AppError>(results)
    };

    let mut results = match tokio::time::timeout(std::time::Duration::from_secs(15), collect).await
    {
        Ok(r) => r?,
        Err(_) => {
            // タイムアウト:接続を落として中断する。
            drop(conn);
            return Err(AppError::BadRequest(
                "クエリがタイムアウトしました(15 秒)".into(),
            ));
        }
    };

    // 0 行 SELECT は列が落ちる:単純クエリプロトコルでは列を最初の行から確定するため、
    // 行が来ないと columns が空のまま返る(空テーブルの閲覧や WHERE 偽の SELECT)。
    // すると columns 空 = 非 SELECT(INSERT/CREATE 等)と区別がつかず、UI が
    // 「行を返さない」表示に倒れてヘッダ無しになる。結果集合が 1 つだけ・列も行も空・
    // rows_affected=0 のとき(= 空 SELECT か非 SELECT のどちらか)に限り describe で
    // 列だけ引き直す(Parse のみで実行はしない)。列が返れば行 0 の SELECT として補い、
    // 返らなければ非 SELECT なので空のまま(= OK 表示)。複数文のときは describe が
    // 通らない/曖昧なので触らない。
    if let [only] = results.as_mut_slice()
        && only.columns.is_empty()
        && only.rows.is_empty()
        && only.rows_affected == 0
    {
        let sql = sqlx::AssertSqlSafe(req.sql).into_sql_str();
        if let Ok(Ok(desc)) =
            tokio::time::timeout(std::time::Duration::from_secs(5), conn.describe(sql)).await
        {
            only.columns = desc.columns.iter().map(|c| c.name().to_string()).collect();
        }
    }

    let _ = conn.close().await;

    Ok(Json(QueryResp { results }))
}

/// `DELETE /api/databases/:id`:ソフト削除(dump → DROP DATABASE → deleted_at)。
/// role は残す(復元で同じパスワードで再作成するため)。
pub async fn delete(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<StatusCode> {
    // 所有権チェック(他人の DB は 404 に収束)。実体の削除は soft_delete に委譲。
    let owned: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM resources
          WHERE id = $1 AND user_id = $2 AND kind = 'database' AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(auth.user_id)
    .fetch_optional(&state.db)
    .await?;
    owned.ok_or(AppError::NotFound)?;

    let dbname = soft_delete(&state, id).await?;
    audit(
        &state.db,
        Some(auth.user_id),
        "db.delete",
        id,
        json!({ "pg_dbname": dbname }),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// database のソフト削除(dump → ゴミ箱 → DROP → deleted_at/trash_meta)。**所有権も audit も
/// しない素の操作** — ユーザ口(`delete`)と owner 代理(admin の最後の砦)が共有する(§5.2)。
/// id で引く(owner はどのユーザの DB も対象にできる)。pg_dbname を返す(audit detail 用)。
pub(crate) async fn soft_delete(state: &AppState, id: Uuid) -> AppResult<String> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT d.pg_dbname
           FROM resources r
           JOIN database_details d ON d.resource_id = r.id
          WHERE r.id = $1 AND r.kind = 'database' AND r.deleted_at IS NULL",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    let dbname = row.ok_or(AppError::NotFound)?.0;

    // dump(無圧縮)→ ゴミ箱へ。失敗したら削除を中止(復元不能な削除はしない)。
    let dump_path = state.config.trash_dir.join(format!("{id}.sql"));
    tenant::dump_database(&state.config.tenant_admin_url, &dbname, &dump_path).await?;

    // DATABASE を落とす(接続は WITH FORCE で切る)。role は残す。
    tenant::drop_database(&state.tenant_admin, &dbname).await?;

    let meta = json!({
        "pg_dbname": dbname,
        "dump_path": dump_path.to_string_lossy(),
    });
    sqlx::query(
        "UPDATE resources
            SET deleted_at = now(),
                purge_after = now() + interval '3 days',
                trash_meta = $2
          WHERE id = $1",
    )
    .bind(id)
    .bind(meta)
    .execute(&state.db)
    .await?;
    Ok(dbname)
}

/// `GET /api/resources`:4 種をフラットに(dashboard 用。M1 では database のみ存在)。
pub async fn list_resources(
    auth: AuthCtx,
    State(state): State<AppState>,
) -> AppResult<Json<Vec<ResourceDto>>> {
    let rows: Vec<ResourceRow> = sqlx::query_as(
        "SELECT id, kind, display_name, anon_seq, created_at, deleted_at
           FROM resources
          WHERE user_id = $1 AND deleted_at IS NULL
          ORDER BY kind, anon_seq",
    )
    .bind(auth.user_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(
        rows.into_iter()
            .map(
                |(id, kind, display_name, anon_seq, created_at, deleted_at)| ResourceDto {
                    id,
                    kind,
                    display_name,
                    anon_seq,
                    created_at,
                    deleted_at,
                },
            )
            .collect(),
    ))
}
