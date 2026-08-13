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

/// 所有者チェック付きでゴミ箱の (kind, display_name, trash_meta) を引く。restore / purge が共有。
/// 見つからない / 他ユーザ / 未削除は 404 に収束。
async fn fetch_trashed(
    db: &PgPool,
    id: Uuid,
    user_id: Uuid,
) -> AppResult<(String, String, Option<Value>)> {
    let row: Option<(String, String, Option<Value>)> = sqlx::query_as(
        "SELECT kind, display_name, trash_meta FROM resources
          WHERE id = $1 AND user_id = $2 AND deleted_at IS NOT NULL",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    row.ok_or(AppError::NotFound)
}

/// restore が活体同名と衝突したときの 409 文案。事前チェックと UPDATE の
/// map_unique(TOCTOU の最終ガード)が同じ一文を使う — 二重管理しない。
/// 具体的なコマンド名は書かない(kind→コマンドの対応は CLI の領分。
/// 実在しないコマンドを案内する事故をサーバ側に作らない)。
fn restore_conflict_msg(kind: &str, display_name: &str) -> String {
    format!(
        "同名の稼働中の{}「{display_name}」があるため復元できません。先にそちらの名前を変更するか削除してから復元してください",
        kind_ja(kind)
    )
}

/// restore 前の活体同名チェック。ゴミ箱は名前を占有しない(20260813000001)ので、
/// 削除後に同名で作り直された活体と復元が衝突し得る。**物理復元(DB 再作成 /
/// volume 移動 / ACL 再作成)より前に**弾く — 後段の 23505 では実体側の作業が
/// 済んだ後になり、副作用だけ残して 500 になる。
async fn ensure_restore_name_free(
    db: &PgPool,
    user_id: Uuid,
    kind: &str,
    display_name: &str,
) -> AppResult<()> {
    if crate::databases::live_name_exists(db, user_id, kind, display_name).await? {
        return Err(AppError::Conflict(restore_conflict_msg(kind, display_name)));
    }
    Ok(())
}

/// kind の日本語名(エラー文案用)。
fn kind_ja(kind: &str) -> &'static str {
    match kind {
        "service" => "サービス",
        "database" => "データベース",
        "cache" => "キャッシュ",
        "volume" => "ボリューム",
        _ => "リソース",
    }
}

/// trash_meta から dump パスを取り出す(無ければ trash_dir/<id>.sql に既定)。
fn dump_path(meta: &Option<Value>, trash_dir: &std::path::Path, id: Uuid) -> PathBuf {
    meta.as_ref()
        .and_then(|m| m.get("dump_path"))
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .unwrap_or_else(|| trash_dir.join(format!("{id}.sql")))
}

/// volume の trash_meta から (host_path, trash_path) を取り出す。
/// host_path は復元先(無ければ None)、trash_path は実体の現在地
/// (無ければ trash_dir/<id> に既定)。
fn volume_paths(
    meta: &Option<Value>,
    trash_dir: &std::path::Path,
    id: Uuid,
) -> (Option<PathBuf>, PathBuf) {
    let get = |key: &str| {
        meta.as_ref()
            .and_then(|m| m.get(key))
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
    };
    let trash = get("trash_path").unwrap_or_else(|| trash_dir.join(id.to_string()));
    (get("host_path"), trash)
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
    // per-resource 直列化(deploy_lock は Uuid 汎用のロック表)。restore ↔ purge/gc の
    // 競合を閉じる:GC が期限切れ候補を読んだ後にユーザが restore しても、purge_resource は
    // 同じロックの中で「まだゴミ箱に居るか」を見直すので、復元済み実体を壊さない。
    // service の場合は deploy とも直列化される(望ましい副作用)。
    let lock = state.deploy_lock(id);
    let _guard = lock.lock().await;

    let (kind, display_name, trash_meta) = fetch_trashed(&state.db, id, auth.user_id).await?;
    // 活体同名との衝突は物理復元の前に弾く(副作用を残して 23505 で落ちない)。
    ensure_restore_name_free(&state.db, auth.user_id, &kind, &display_name).await?;

    let mut detail = json!({});
    // 物理復元と同時に「取り消し方」を覚える:active 化 UPDATE が同名活体と衝突(TOCTOU)
    // したとき、実体だけ復活した状態(旧資格情報で繋がる DB / 復活した ACL / host に戻った
    // volume)を残さないための補償材料。
    let mut undo = RestoreUndo::None;
    let action = match kind.as_str() {
        "database" => {
            let dbname = restore_database(&state, id, &trash_meta).await?;
            undo = RestoreUndo::Database(dbname);
            "db.restore"
        }
        "volume" => {
            let (host, trash) = restore_volume(&state, id, &trash_meta).await?;
            undo = RestoreUndo::Volume { host, trash };
            "volume.restore"
        }
        "cache" => {
            // ACL を再作成 + 生存 key 数を報告(TRASH-1。allkeys-lru で温存中の key が evict され
            // 空かもしれない = データ復元は best-effort・§11-D)。詳細表示の key_count でも見える。
            let (survived, acl_user) = restore_cache(&state, id).await?;
            tracing::info!(%id, surviving_keys = ?survived, "cache 復元(データは best-effort)");
            detail = json!({ "surviving_keys": survived });
            undo = RestoreUndo::Cache(acl_user);
            "cache.restore"
        }
        // service は永続実体が無い(コンテナは deploy で再生成)。下の deleted_at=NULL の
        // 共通処理が行を active 化し、`tbm service start` で再起動できる。
        "service" => "service.restore",
        other => {
            return Err(AppError::BadRequest(format!("復元未対応の種別: {other}")));
        }
    };

    // 物理復元が成功してから resource を active に戻す。**これを実体の片付けより先に**:
    // ここで失敗しても実体が残り、gc に消されず再 restore できる(データを失わない)。
    // 部分ユニーク(活体同名)との TOCTOU 衝突(事前チェックの後に同名 create が滑り込んだ
    // 縫間 — create はこのロックの外)は 409 にした上で、**物理復元を巻き戻す**:
    // 巻き戻さないと「復元失敗」なのに旧 DB が繋がる / volume の実体が trash に無く
    // 後の purge が host 側データを永久孤児化する。行はゴミ箱のまま = 再 restore 可能。
    let updated = sqlx::query(
        "UPDATE resources SET deleted_at = NULL, purge_after = NULL, trash_meta = NULL
          WHERE id = $1 AND deleted_at IS NOT NULL",
    )
    .bind(id)
    .execute(&state.db)
    .await;
    match updated {
        Ok(r) if r.rows_affected() == 0 => {
            // ロック内では fetch_trashed 後に行が消える経路は無いはずだが、万一消えていたら
            // 偽の 204 を返さない(実体は巻き戻して次の一手を明確に)。
            undo_restore(&state, undo).await;
            return Err(AppError::NotFound);
        }
        Ok(_) => {}
        Err(e) => {
            undo_restore(&state, undo).await;
            return Err(crate::databases::map_unique(
                e,
                restore_conflict_msg(&kind, &display_name),
            ));
        }
    }

    // database のみ:active 化確定後に dump を片付ける(残っても無害なのでベストエフォート)。
    // volume は実体を mv で戻し済みなので片付ける残骸は無い。
    if kind == "database" {
        let _ = std::fs::remove_file(dump_path(&trash_meta, &state.config.trash_dir, id));
    }

    audit(
        &state.db,
        Some(auth.user_id),
        action,
        id,
        detail,
        auth.client_ip.as_deref(),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// 物理復元の取り消し方(restore の active 化 UPDATE が失敗したときの補償材料)。
enum RestoreUndo {
    /// 再作成した DATABASE を落とす(role は温存 = ゴミ箱状態に戻す)。
    Database(String),
    /// host へ戻した実体を trash へ mv し直す。
    Volume {
        host: PathBuf,
        trash: PathBuf,
    },
    /// 再作成した ACL ユーザを消す。
    Cache(String),
    None,
}

/// 物理復元の巻き戻し(best-effort)。失敗は warn に留める — 行はゴミ箱のまま残って
/// いるので、再 restore / purge がもう一度片付けの機会になる(volume だけは host 側に
/// 残ると後の purge が拾えないため、失敗を大きめに警告する)。
async fn undo_restore(state: &AppState, undo: RestoreUndo) {
    match undo {
        RestoreUndo::Database(dbname) => {
            if let Err(e) = tenant::drop_database(&state.tenant_admin, &dbname).await {
                tracing::warn!(error = ?e, dbname, "restore 補償: DATABASE の巻き戻しに失敗");
            }
        }
        RestoreUndo::Volume { host, trash } => {
            if let Err(e) = std::fs::rename(&host, &trash) {
                tracing::error!(error = ?e, host = %host.display(), trash = %trash.display(),
                    "restore 補償: volume 実体を trash へ戻せなかった — 行はゴミ箱のままなので \
                     後の purge がこの host 側データを回収できない(手動対応が必要)");
            }
        }
        RestoreUndo::Cache(acl_user) => {
            if let Err(e) = crate::valkey::del_user(&state.valkey, &acl_user).await {
                tracing::warn!(error = ?e, acl_user, "restore 補償: ACL の巻き戻しに失敗");
            }
        }
        RestoreUndo::None => {}
    }
}

/// cache の復元:detail の password で **同じ acl_user / namespace** の ACL を再作成
/// (key は valkey に温存されていれば見える)。生存 key 数と acl_user(補償用)を返す。
async fn restore_cache(state: &AppState, id: Uuid) -> AppResult<(Option<i64>, String)> {
    let (acl_user, namespace, enc): (String, String, Vec<u8>) = sqlx::query_as(
        "SELECT acl_user, namespace, password_enc FROM cache_details WHERE resource_id = $1",
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    let pw = state.crypto.decrypt(&enc)?;
    crate::valkey::set_user(&state.valkey, &acl_user, &namespace, &pw).await?;
    let count = crate::valkey::count_keys(&state.valkey, &namespace).await;
    Ok((count, acl_user))
}

/// database の復元:role は残っているので DATABASE を再作成して dump を流し込む。
/// dump 削除は呼び出し側(deleted_at クリア後)が行う。dbname(補償用)を返す。
async fn restore_database(
    state: &AppState,
    id: Uuid,
    trash_meta: &Option<Value>,
) -> AppResult<String> {
    // 流し込みは admin ではなくその DB の app role で行う(tenant::restore_database の doc 参照)
    // ので、pg_dbname と一緒に app の資格情報も引いて復号する。
    let (dbname, app_pw_enc): (String, Vec<u8>) = sqlx::query_as(
        "SELECT d.pg_dbname, ro.password_enc
           FROM database_details d
           JOIN database_roles ro ON ro.resource_id = d.resource_id AND ro.role_kind = 'app'
          WHERE d.resource_id = $1",
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    let app_pw = state.crypto.decrypt(&app_pw_enc)?;
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
        &names.app,
        &app_pw,
        &dump,
    )
    .await
    {
        // reload 失敗 → 作りかけの DATABASE を落とす(role は残す)。
        let _ = tenant::drop_database(&state.tenant_admin, &names.dbname).await;
        return Err(e);
    }
    Ok(names.dbname.clone())
}

/// volume の復元:trash へ mv した実体を host_path へ mv で戻す。
/// active 化は呼び出し側(deleted_at クリア)が行う。(host, trash)(補償用)を返す。
async fn restore_volume(
    state: &AppState,
    id: Uuid,
    trash_meta: &Option<Value>,
) -> AppResult<(PathBuf, PathBuf)> {
    let (host, trash) = volume_paths(trash_meta, &state.config.trash_dir, id);
    let host =
        host.ok_or_else(|| AppError::BadRequest("復元に必要な host_path がありません".into()))?;

    // trash_meta 破損による枠外操作を防ぐ:host は volumes_dir 配下、trash は trash_dir 配下。
    if !host.starts_with(&state.config.volumes_dir) {
        return Err(AppError::BadRequest("復元先パスが不正です".into()));
    }
    if !trash.starts_with(&state.config.trash_dir) {
        return Err(AppError::BadRequest("ゴミ箱パスが不正です".into()));
    }

    match (trash.exists(), host.exists()) {
        // 通常:trash の実体を host へ戻す(親を用意してから)。
        (true, false) => {
            if let Some(parent) = host.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::rename(&trash, &host)?;
        }
        // 既に戻っている(active 化前に落ちた再試行)— 冪等に成功扱い。
        (false, true) => {}
        // 両方在る(異常)— 活きた host を壊さないため拒否。
        (true, true) => {
            return Err(AppError::Conflict(
                "復元先に既存のデータがあるため復元できません".into(),
            ));
        }
        // どちらも無い(異常)— データを失わないため作り直さず明示エラー。
        (false, false) => {
            return Err(AppError::BadRequest(
                "ゴミ箱の実体が見つからないため復元できません".into(),
            ));
        }
    }
    Ok((host, trash))
}

/// `DELETE /api/trash/:id`:永久削除(ユーザ操作)。
pub async fn purge(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<StatusCode> {
    // 所有権 + 存在チェック(他人 / 活体は 404)。実際の対象確定は purge_resource が
    // ロック内でやり直す(この読みと実行の間に restore が挟まれ得るため)。
    let (kind, _display_name, _trash_meta) = fetch_trashed(&state.db, id, auth.user_id).await?;

    // ロック内の再確認で「もうゴミ箱に居ない」= 並行 restore に負けた → 404
    // (復元済みの実体を壊すより「見つからない」が正しい)。
    if purge_resource(&state, id).await?.is_none() {
        return Err(AppError::NotFound);
    }
    audit(
        &state.db,
        Some(auth.user_id),
        "trash.purge",
        id,
        json!({ "kind": kind }),
        auth.client_ip.as_deref(),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// 物理削除のコア。ユーザの永久削除と reconcile の自動 purge が共有する。
/// kind 毎に実体(tenant DB / role / dump)を片付けてから行を物理削除する
/// (resources の行を消すと detail / roles はカスケードで消える)。
///
/// per-resource ロックの中で「まだゴミ箱に居るか」を**自分で**確認してから壊す:
/// 呼び出し側(gc の候補スキャン / ユーザ purge の所有権チェック)が読んだ状態は
/// ロック取得までに陳腐化し得る — 特に「GC が候補を読む → ユーザが restore →
/// GC が purge」の順だと、復元直後の実体を破壊してしまう(codex 監査 2026-08-13)。
/// 戻り値:Some(kind) = 消した / None = もうゴミ箱に居ない(触っていない)。
pub(crate) async fn purge_resource(state: &AppState, id: Uuid) -> AppResult<Option<String>> {
    let lock = state.deploy_lock(id);
    let _guard = lock.lock().await;

    let row: Option<(String, Option<Value>)> = sqlx::query_as(
        "SELECT kind, trash_meta FROM resources WHERE id = $1 AND deleted_at IS NOT NULL",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    let Some((kind, trash_meta)) = row else {
        return Ok(None);
    };
    let kind = kind.as_str();
    let trash_meta = &trash_meta;

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
    } else if kind == "volume" {
        // 実体(trash へ mv 済みのディレクトリ)を消す。失敗したら **行を消さない**
        // (取り残し防止 — db と同じ規律)。存在しなければスキップ。
        let (_host, trash) = volume_paths(trash_meta, &state.config.trash_dir, id);
        // 破壊操作の前に trash_dir 配下であることを必ず確認(trash_meta 破損時の暴走防止)。
        if !trash.starts_with(&state.config.trash_dir) {
            return Err(AppError::BadRequest("ゴミ箱パスが不正です".into()));
        }
        if trash.exists() {
            std::fs::remove_dir_all(&trash)?;
        }
    } else if kind == "service" {
        // service の永久削除:コンテナ + route を掃除する(掃除が失敗したら行を消さない =
        // db/volume と同じ規律。管理対象外の活きたコンテナを取り残さない)。
        crate::services::docker::stop_remove(state, id).await?;
        crate::services::route::remove(state, id)?;
        // 私網は通常 soft_delete で撤去済み。残っていても DELETE 後は生存行を持たない孤児 = reconcile の
        // 網 GC が回収するので、ここは best-effort(空 bridge の撤去失敗で永久削除を止めない)。
        if let Err(e) = crate::services::network::remove_service_network(state, id).await {
            tracing::warn!(error = ?e, %id, "purge: 私網の撤去に失敗(reconcile の網 GC が回収)");
        }
        // 永久削除なので rollback 対象も消える → registry の repo(全 manifest)も掃除する。
        // manifest を消すと layer blob は無参照になり、日次の garbage_collect が実体を回収する
        // (これをしないと削除済み service の旧イメージが registry に永久に堆積する)。
        // 失敗時は行を残す(上と同じ規律 → 次 tick で再試行。registry 一時障害で自己修復)。
        crate::services::registry::delete_repo(state, id).await?;
        // **宿主 docker のイメージは registry とは別実体**なので、ここで併せて消す
        // (registry の manifest を消しても pull 済みの宿主イメージは残り続ける = 1 版で数百 MB)。
        // keep 表を空で渡す = この service の全参照が対象。年齢下限も掛けない(0)— service ごと
        // 消える瞬間なので「失敗イメージを 48h 残す」理由がもう無い。コンテナは直前の
        // `stop_remove` で消えているので、掴まれていて消せない参照は通常無い。
        // best-effort:ディスクの解放が遅れるだけで正しさには影響しないので、失敗しても
        // 永久削除は進める(行を残すと「消したのに残っている」状態が続く方が害が大きい)。
        // 回収できなかった分は prune_host_images が warn を出す(この service は二度と
        // 訪れないので日次の再試行が効かない = 手動 `docker rmi` が必要)。
        let refs = crate::services::docker::list_service_image_refs(state).await;
        crate::services::docker::prune_host_images(
            state,
            &refs,
            &std::collections::HashMap::new(),
            &std::collections::HashSet::new(),
            Some(id),
        )
        .await;
        // **外部イメージ(`--image` の pgvector 等)は敢えて消さない。** 一見「用済み」だが:
        //  (1) 同居する他プロジェクトや他 service が同じ upstream ref を使っていることがあり、
        //      untag は相手の `docker run` を再 pull に落とす(`force=false` でも、内部 tag が
        //      付いている間は docker のコンテナ参照チェックが発火しないので**止められない** —
        //      Engine 29.4 で実測)。
        //  (2) `source_kind`/`source_spec` は provenance で、GitHub / `--local` 経路は**書き換えない**。
        //      昔一度 `--image nginx:alpine` を使った service を今 purge すると、陳腐な記録が
        //      宿主の `nginx:alpine` を消す = 破壊操作が古い情報で駆動される。
        //  (3) 同じ ref は上書きされるので**時間で累積しない**(一度きり数百 MB)。
        // 得(一度の数百 MB)より失(他人のイメージを消す)が大きいので回収しない。要るときは
        // 運用者が `docker rmi <ref>` で判断して消す。
    } else if kind == "cache" {
        // cache の永久削除:ACL ユーザを消し(冪等)、namespace の key を SCAN+UNLINK で
        // 確実に解放してから行を消す(掃除が失敗したら行を残す = 取り残し防止)。§7.2。
        let ns: Option<(String,)> =
            sqlx::query_as("SELECT namespace FROM cache_details WHERE resource_id = $1")
                .bind(id)
                .fetch_optional(&state.db)
                .await?;
        if let Some((namespace,)) = ns {
            // acl_user == namespace(§2)なので namespace を DELUSER の対象に使える。
            crate::valkey::del_user(&state.valkey, &namespace).await?;
            crate::valkey::purge_namespace(&state.valkey, &namespace).await?;
        }
    }

    // 述語はロック内再確認の縦深防御(ロックが正だが、消す側の最後の一手も条件付きに)。
    sqlx::query("DELETE FROM resources WHERE id = $1 AND deleted_at IS NOT NULL")
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(Some(kind.to_string()))
}
