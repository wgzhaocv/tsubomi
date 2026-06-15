//! cache(valkey)リソースの API ハンドラ(paas-m5-design.md §4)。database(`databases.rs`)を
//! 範に、平台が「期望状態」を resources / cache_details に持ち、現実(valkey の per-cache ACL)を
//! そこへ収束させる。create は valkey に ACL を先に作り、成功してから platform 行を入れる
//! (失敗時は ACL を掃除)。web と CLI は同一ハンドラの 2 入口 — 認証 extractor(AuthCtx)だけが分岐点。

use crate::auth::AuthCtx;
use crate::databases::{audit, map_unique};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::{tenant, valkey, validate};
use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::PgPool;
use tsubomi_shared::{CacheDetailDto, CacheDto, ConnectionUrlResp, CreateCacheReq, RenameCacheReq};
use uuid::Uuid;

const MAX_NAME_LEN: usize = 64;

/// `list` の行(id, display_name, anon_seq, created_at, rotated_at)。
type CacheRow = (Uuid, String, i32, DateTime<Utc>, Option<DateTime<Utc>>);

fn row_to_dto((id, display_name, anon_seq, created_at, rotated_at): CacheRow) -> CacheDto {
    CacheDto {
        id,
        display_name,
        anon_seq,
        created_at,
        rotated_at,
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/caches", get(list).post(create))
        .route("/caches/{id}", get(get_one).patch(rename).delete(delete))
        .route("/caches/{id}/url", get(url))
        .route("/caches/{id}/rotate", post(rotate))
}

/// 内部入口の接続文字列(REDIS_URL)を組み立てる。host は docker DNS の tsubomi-valkey(§11-A)
/// = 注入経由でコンテナからのみ届く(利用者の手元からは繋がらない・§11-B)。
fn build_url(state: &AppState, acl_user: &str, password: &str) -> String {
    let cfg = &state.config;
    format!(
        "redis://{acl_user}:{password}@{}:{}",
        cfg.cache_internal_host, cfg.cache_internal_port
    )
}

/// 所有者チェック付きで (acl_user, namespace, password_enc) を引く。url / rotate が共有。
/// 見つからない / 他ユーザ / 削除済みは 404。
async fn fetch_creds(
    db: &PgPool,
    user_id: Uuid,
    id: Uuid,
) -> AppResult<(String, String, Vec<u8>)> {
    let row: Option<(String, String, Vec<u8>)> = sqlx::query_as(
        "SELECT d.acl_user, d.namespace, d.password_enc
           FROM resources r
           JOIN cache_details d ON d.resource_id = r.id
          WHERE r.id = $1 AND r.user_id = $2 AND r.kind = 'cache' AND r.deleted_at IS NULL",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    row.ok_or(AppError::NotFound)
}

/// `POST /api/caches`:cache 作成。
pub async fn create(
    auth: AuthCtx,
    State(state): State<AppState>,
    Json(req): Json<CreateCacheReq>,
) -> AppResult<(StatusCode, Json<CacheDto>)> {
    let display_name = validate::name(&req.name, MAX_NAME_LEN)?;

    // 同名チェックを ACL 作成の前に(無駄な SETUSER/DELUSER を避ける)。UNIQUE はゴミ箱内
    // (deleted_at)も含むので全行を見る。競合(同時 create)は insert_rows の UNIQUE が最終ガード。
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM resources WHERE user_id = $1 AND kind = 'cache' AND display_name = $2)",
    )
    .bind(auth.user_id)
    .bind(&display_name)
    .fetch_one(&state.db)
    .await?;
    if exists {
        return Err(AppError::Conflict(format!(
            "キャッシュ名 '{display_name}' は既に使われています(ゴミ箱内を含む)。別の名前にしてください"
        )));
    }

    // acl_user = namespace = c_<shortid>(§2)。password は復元可能な暗号化で保存(rotate/restore 用)。
    let name = valkey::gen_name();
    let password = tenant::gen_password();

    // 1. valkey に ACL を先に作る(失敗で中止 — platform 行は入れない)。
    valkey::set_user(&state.valkey, &name, &name, &password).await?;

    // 2. platform にメタを記録(パスワードは暗号化)。失敗したら valkey の ACL を掃除。
    let enc = state.crypto.encrypt(&password)?;
    let dto = match insert_rows(&state.db, auth.user_id, &display_name, &name, enc).await {
        Ok(dto) => dto,
        Err(e) => {
            let _ = valkey::del_user(&state.valkey, &name).await;
            return Err(e);
        }
    };

    audit(
        &state.db,
        Some(auth.user_id),
        "cache.create",
        dto.id,
        json!({ "display_name": display_name, "namespace": name }),
    )
    .await;
    Ok((StatusCode::CREATED, Json(dto)))
}

async fn insert_rows(
    db: &PgPool,
    user_id: Uuid,
    display_name: &str,
    name: &str,
    enc: Vec<u8>,
) -> AppResult<CacheDto> {
    let mut tx = db.begin().await?;

    // ユーザ単位で anon_seq の採番を直列化(同時 create の競合を防ぐ。database と同じロック鍵)。
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1::text), 42)")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;

    let anon_seq: i32 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(anon_seq),0)+1 FROM resources WHERE user_id=$1 AND kind='cache'",
    )
    .bind(user_id)
    .fetch_one(&mut *tx)
    .await?;

    let (id, created_at): (Uuid, DateTime<Utc>) = sqlx::query_as(
        "INSERT INTO resources (user_id, kind, display_name, anon_seq)
              VALUES ($1, 'cache', $2, $3)
         RETURNING id, created_at",
    )
    .bind(user_id)
    .bind(display_name)
    .bind(anon_seq)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| map_unique(e, format!("キャッシュ名 '{display_name}' は既に使われています")))?;

    // acl_user = namespace = name(同値。$2 を両方に)。
    sqlx::query(
        "INSERT INTO cache_details (resource_id, acl_user, namespace, password_enc)
              VALUES ($1, $2, $2, $3)",
    )
    .bind(id)
    .bind(name)
    .bind(enc)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(CacheDto {
        id,
        display_name: display_name.to_owned(),
        anon_seq,
        created_at,
        rotated_at: None,
    })
}

/// `GET /api/caches`:自分の cache 一覧。
pub async fn list(auth: AuthCtx, State(state): State<AppState>) -> AppResult<Json<Vec<CacheDto>>> {
    let rows: Vec<CacheRow> = sqlx::query_as(
        "SELECT r.id, r.display_name, r.anon_seq, r.created_at, d.rotated_at
           FROM resources r
           JOIN cache_details d ON d.resource_id = r.id
          WHERE r.user_id = $1 AND r.kind = 'cache' AND r.deleted_at IS NULL
          ORDER BY r.anon_seq",
    )
    .bind(auth.user_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(rows.into_iter().map(row_to_dto).collect()))
}

/// `GET /api/caches/:id`:単体詳細(namespace + key 数つき)。key 数は valkey の SCAN 概算
/// (取得不能 = valkey 不通は null。best-effort)。
pub async fn get_one(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<Json<CacheDetailDto>> {
    type DetailRow = (Uuid, String, i32, DateTime<Utc>, Option<DateTime<Utc>>, String);
    let row: Option<DetailRow> = sqlx::query_as(
        "SELECT r.id, r.display_name, r.anon_seq, r.created_at, d.rotated_at, d.namespace
           FROM resources r
           JOIN cache_details d ON d.resource_id = r.id
          WHERE r.id = $1 AND r.user_id = $2 AND r.kind = 'cache' AND r.deleted_at IS NULL",
    )
    .bind(id)
    .bind(auth.user_id)
    .fetch_optional(&state.db)
    .await?;

    let (id, display_name, anon_seq, created_at, rotated_at, namespace) =
        row.ok_or(AppError::NotFound)?;
    let key_count = valkey::count_keys(&state.valkey, &namespace).await;
    Ok(Json(CacheDetailDto {
        id,
        display_name,
        anon_seq,
        created_at,
        rotated_at,
        namespace,
        key_count,
    }))
}

/// `PATCH /api/caches/:id`:表示名のリネーム。display_name だけ更新し、acl_user / namespace /
/// 接続文字列は一切変えない(リネームは UI 上のラベル変更。databases と同じ)。
pub async fn rename(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<RenameCacheReq>,
) -> AppResult<Json<CacheDto>> {
    let display_name = validate::name(&req.name, MAX_NAME_LEN)?;
    let row: Option<CacheRow> = sqlx::query_as(
        "UPDATE resources r SET display_name = $1
           FROM cache_details d
          WHERE r.id = $2 AND r.user_id = $3 AND r.kind = 'cache' AND r.deleted_at IS NULL
            AND d.resource_id = r.id
      RETURNING r.id, r.display_name, r.anon_seq, r.created_at, d.rotated_at",
    )
    .bind(&display_name)
    .bind(id)
    .bind(auth.user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| map_unique(e, format!("キャッシュ名 '{display_name}' は既に使われています")))?;

    let row = row.ok_or(AppError::NotFound)?;
    audit(
        &state.db,
        Some(auth.user_id),
        "cache.rename",
        id,
        json!({ "display_name": display_name }),
    )
    .await;
    Ok(Json(row_to_dto(row)))
}

/// `GET /api/caches/:id/url`:内部接続文字列(REDIS_URL)。**パスワードそのもの**。
/// 内部入口(tsubomi-valkey)なので注入された service コンテナからのみ使える(§11-B)。
pub async fn url(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<Json<ConnectionUrlResp>> {
    let (acl_user, _namespace, enc) = fetch_creds(&state.db, auth.user_id, id).await?;
    let pw = state.crypto.decrypt(&enc)?;
    Ok(Json(ConnectionUrlResp {
        url: build_url(&state, &acl_user, &pw),
    }))
}

/// `POST /api/caches/:id/rotate`:パスワードを差し替える(旧接続文字列は即失効)。
/// `valkey::set_user`(reset → 新パスで再構築)で旧パスを消し新パスを設定 = 既存の key 規則
/// /コマンド白名単は維持(§7.1)。**再デプロイで新文字列が効く**(値は起動の瞬間に解決)。
///
/// 順序は **DB(真実源)を先に更新 → valkey に適用**(背骨:cache_details が期望状態、valkey は
/// そこへ収束する)。これにより set_user が落ちても reconcile が DB の新パスへ**前向きに**収束する
/// (旧パスは復活しない)。逆順(valkey 先)だと DB 更新失敗時に reconcile が旧パスへ revert し、
/// rotate 済みの旧資格が蘇る。
pub async fn rotate(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<Json<ConnectionUrlResp>> {
    let (acl_user, namespace, _) = fetch_creds(&state.db, auth.user_id, id).await?;
    let new_pw = tenant::gen_password();
    let enc = state.crypto.encrypt(&new_pw)?;

    // 1. DB(真実源)を先に更新。
    sqlx::query("UPDATE cache_details SET password_enc = $1, rotated_at = now() WHERE resource_id = $2")
        .bind(enc)
        .bind(id)
        .execute(&state.db)
        .await?;
    // 2. valkey に新パスを適用(失敗しても周期収束が DB から前向きに貼り直す)。
    valkey::set_user(&state.valkey, &acl_user, &namespace, &new_pw).await?;

    audit(&state.db, Some(auth.user_id), "cache.rotate", id, json!({})).await;
    Ok(Json(ConnectionUrlResp {
        url: build_url(&state, &acl_user, &new_pw),
    }))
}

/// `DELETE /api/caches/:id`:ソフト削除(ACL DELUSER → ゴミ箱。key は温存)。
pub async fn delete(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<StatusCode> {
    // 所有権チェック(他人の cache は 404 に収束)。実体の削除は soft_delete に委譲。
    let owned: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM resources
          WHERE id = $1 AND user_id = $2 AND kind = 'cache' AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(auth.user_id)
    .fetch_optional(&state.db)
    .await?;
    owned.ok_or(AppError::NotFound)?;

    let namespace = soft_delete(&state, id).await?;
    audit(
        &state.db,
        Some(auth.user_id),
        "cache.delete",
        id,
        json!({ "namespace": namespace }),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// cache のソフト削除(`ACL DELUSER` → ゴミ箱)。**所有権も audit もしない素の操作** —
/// ユーザ口(`delete`)と owner 代理(S3 の admin の最後の砦)が共有する。namespace を返す
/// (audit detail 用)。valkey が落ちていると DELUSER に失敗 → 削除も失敗する(database が
/// tenant を要するのと同型 — 資格を確実に無効化してから削除済みにする)。
pub(crate) async fn soft_delete(state: &AppState, id: Uuid) -> AppResult<String> {
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT d.acl_user, d.namespace
           FROM resources r
           JOIN cache_details d ON d.resource_id = r.id
          WHERE r.id = $1 AND r.kind = 'cache' AND r.deleted_at IS NULL",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    let (acl_user, namespace) = row.ok_or(AppError::NotFound)?;

    // ACL を削除(= 即座にその資格でログイン不可)。key は内存に温存(復元で生き返る)。
    valkey::del_user(&state.valkey, &acl_user).await?;

    let meta = json!({ "acl_user": acl_user, "namespace": namespace });
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
    Ok(namespace)
}
