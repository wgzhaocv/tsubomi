//! service リソースの API(tech-design §6 の service 面)。web と CLI は同一ハンドラの
//! 2 入口 — 認証 extractor(AuthCtx)だけが分岐点。
//!
//! M3 第 1 チャンク(S1–S3、曳光弾)は最小 create + deploy hook + コンテナ起動まで。
//! gh オーケストレーション / 注入 / start・stop・logs / rollback / web 画面 / reconcile は
//! 後チャンク(plan・paas-m3-design.md)。

pub mod deploy;
pub mod docker;
pub mod inject;
pub mod network;
pub mod reconcile;
pub mod registry;
pub mod route;
pub mod workflow;

use crate::auth::AuthCtx;
use crate::config::Config;
use crate::databases::{audit, map_unique};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::validate;
use axum::Json;
use axum::Router;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::PgPool;
use tsubomi_shared::{
    CreateInjectionReq, CreateServiceReq, CreateServiceResp, DeployConfig, DeployDto, ExecReq,
    ExecResult, InjectionDto, LogsResp, RollbackReq, ServiceDto, SetEnvReq,
};
use uuid::Uuid;

const MAX_NAME_LEN: usize = 64;
/// subdomain 生成の予約語(平台 / インフラのホスト名と衝突させない)。
const RESERVED_SUBDOMAINS: &[&str] = &["paas", "registry", "traefik", "www", "api"];
/// deploy_key の乱数バイト数(base64url で ≈43 字)。HMAC の鍵そのもの。
const DEPLOY_KEY_BYTES: usize = 32;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/services", get(list).post(create))
        .route("/services/{id}", get(get_one).delete(delete_service))
        .route("/services/{id}/start", post(start))
        .route("/services/{id}/stop", post(stop))
        .route("/services/{id}/logs", get(logs))
        .route("/services/{id}/exec", post(exec))
        .route("/services/{id}/terminal", get(terminal))
        .route("/services/{id}/rollback", post(rollback))
        .route("/services/{id}/deploys", get(deploys))
        .route("/services/{id}/deploy-config", get(deploy_config))
        .route(
            "/services/{id}/injections",
            get(list_injections).post(create_injection),
        )
        .route("/injections/{id}", delete(delete_injection))
        .route("/services/{id}/env", get(list_env).post(set_env))
        .route("/services/{id}/env/{key}", delete(unset_env))
}

/// list / get_one が共有する行(resources + service_details の join)。
type ServiceRow = (
    Uuid,                  // id
    String,                // display_name
    i32,                   // anon_seq
    DateTime<Utc>,         // created_at
    String,                // subdomain
    String,                // phase
    String,                // desired_state
    i32,                   // container_port
    Option<String>,        // image_digest
    Option<DateTime<Utc>>, // last_deploy_at
);

fn service_row_to_dto(r: ServiceRow, config: &Config) -> ServiceDto {
    // url は subdomain を移動させる前に算出(同一リテラル内で借用 + 移動はできない)。
    let url = config.service_url(&r.4);
    ServiceDto {
        id: r.0,
        display_name: r.1,
        anon_seq: r.2,
        created_at: r.3,
        subdomain: r.4,
        phase: r.5,
        desired_state: r.6,
        container_port: r.7,
        image_digest: r.8,
        last_deploy_at: r.9,
        url,
    }
}

/// `GET /api/services`:自分の service 一覧(ゴミ箱内は除く)。秘密は含まない。
pub async fn list(
    auth: AuthCtx,
    State(state): State<AppState>,
) -> AppResult<Json<Vec<ServiceDto>>> {
    let rows: Vec<ServiceRow> = sqlx::query_as(
        "SELECT r.id, r.display_name, r.anon_seq, r.created_at,
                s.subdomain, s.phase, s.desired_state, s.container_port, s.image_digest, s.last_deploy_at
           FROM resources r JOIN service_details s ON s.resource_id = r.id
          WHERE r.user_id = $1 AND r.kind = 'service' AND r.deleted_at IS NULL
          ORDER BY r.anon_seq",
    )
    .bind(auth.user_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| service_row_to_dto(r, &state.config))
            .collect(),
    ))
}

/// `GET /api/services/:id`:単一 service の詳細(所有者チェック。無 / 他人 / 削除済みは 404)。
pub async fn get_one(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<Json<ServiceDto>> {
    let row: Option<ServiceRow> = sqlx::query_as(
        "SELECT r.id, r.display_name, r.anon_seq, r.created_at,
                s.subdomain, s.phase, s.desired_state, s.container_port, s.image_digest, s.last_deploy_at
           FROM resources r JOIN service_details s ON s.resource_id = r.id
          WHERE r.id = $1 AND r.user_id = $2 AND r.kind = 'service' AND r.deleted_at IS NULL",
    )
    .bind(id)
    .bind(auth.user_id)
    .fetch_optional(&state.db)
    .await?;
    row.map(|r| service_row_to_dto(r, &state.config))
        .map(Json)
        .ok_or(AppError::NotFound)
}

/// 自分の service か確認する(他人 / 不在 / 削除済みは 404)。所有権ゲート。
async fn ensure_owned(state: &AppState, user_id: Uuid, id: Uuid) -> AppResult<()> {
    let ok: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM resources
          WHERE id=$1 AND user_id=$2 AND kind='service' AND deleted_at IS NULL)",
    )
    .bind(id)
    .bind(user_id)
    .fetch_one(&state.db)
    .await?;
    if ok { Ok(()) } else { Err(AppError::NotFound) }
}

/// deploys 行(id, git_sha, image_digest, status, error, created_at, finished_at)。
type DeployRow = (
    Uuid,
    String,
    String,
    String,
    Option<String>,
    DateTime<Utc>,
    Option<DateTime<Utc>>,
);

fn deploy_row_to_dto(r: DeployRow) -> DeployDto {
    DeployDto {
        id: r.0,
        git_sha: r.1,
        image_digest: r.2,
        status: r.3,
        error: r.4,
        created_at: r.5,
        finished_at: r.6,
    }
}

/// `GET /api/services/:id/deploys`:デプロイ履歴(新しい順、最大 50。所有者チェック)。
pub async fn deploys(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Vec<DeployDto>>> {
    ensure_owned(&state, auth.user_id, id).await?;
    let rows: Vec<DeployRow> = sqlx::query_as(
        "SELECT id, git_sha, image_digest, status, error, created_at, finished_at
           FROM deploys WHERE service_id = $1 ORDER BY created_at DESC LIMIT 50",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows.into_iter().map(deploy_row_to_dto).collect()))
}

/// `GET /api/services/:id/deploy-config`:`tbm deploy --local` 用の全値(所有者のみ)。
/// deploy_key / registry.pass を **再度平文で返す**(設計 §4b の退路。自分の service にだけ)。
pub async fn deploy_config(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<axum::response::Response> {
    // 所有権チェックと deploy_key 取得を一度に(他人 / 不在は 404)。
    let key_enc: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT s.deploy_key_enc FROM resources r JOIN service_details s ON s.resource_id = r.id
          WHERE r.id=$1 AND r.user_id=$2 AND r.kind='service' AND r.deleted_at IS NULL",
    )
    .bind(id)
    .bind(auth.user_id)
    .fetch_optional(&state.db)
    .await?;
    let key_enc = key_enc.ok_or(AppError::NotFound)?;
    let deploy_key = state.crypto.decrypt(&key_enc)?;
    let registry = registry::ensure_account(&state, auth.user_id).await?;
    let hook_url = format!("{}/api/hook/deploy", state.config.server_url);

    Ok(crate::respond::no_store(DeployConfig {
        service_id: id,
        registry,
        deploy_key,
        hook_url,
        platforms: state.config.platforms.clone(),
    }))
}

// ===== lifecycle(start / stop / logs / delete / rollback)=====

/// 直近に成功した deploy の `(image_digest, git_sha)`(同じ行なので整合)。1 件も無ければ未デプロイ。
/// start(現行を再起動)と reconcile(消えたコンテナを復活)が共有する。
pub(crate) async fn latest_succeeded_deploy(
    state: &AppState,
    service_id: Uuid,
) -> AppResult<Option<(String, String)>> {
    Ok(sqlx::query_as(
        "SELECT image_digest, git_sha FROM deploys
          WHERE service_id = $1 AND status = 'succeeded'
          ORDER BY created_at DESC LIMIT 1",
    )
    .bind(service_id)
    .fetch_optional(&state.db)
    .await?)
}

/// 指定 digest を新しい deploy として起こす(start / rollback / reconcile が共有)。deploys 行を
/// received で作り、run_digest を **await**(run_digest 内で deploy_lock + start-first swap + 状態記録)。
pub(crate) async fn redeploy(
    state: &AppState,
    service_id: Uuid,
    image_digest: &str,
    git_sha: &str,
    trigger: deploy::DeployTrigger,
) -> AppResult<()> {
    let deploy_id: Uuid = sqlx::query_scalar(
        "INSERT INTO deploys (service_id, git_sha, image_digest, status)
              VALUES ($1, $2, $3, 'received') RETURNING id",
    )
    .bind(service_id)
    .bind(git_sha)
    .bind(image_digest)
    .fetch_one(&state.db)
    .await?;
    deploy::run_digest(state, deploy_id, service_id, image_digest, git_sha, trigger).await
}

/// `POST /api/services/:id/start`:現 image_digest を再起動(desired_state=running)。
/// 未デプロイ(digest なし)は 400。run_digest を await し、起動できたら 204。
pub async fn start(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<StatusCode> {
    ensure_owned(&state, auth.user_id, id).await?;
    // 直近に成功した deploy の (digest, git_sha) を再起動する。1 件も無ければ未デプロイ。
    let (digest, git_sha) = latest_succeeded_deploy(&state, id).await?.ok_or_else(|| {
        AppError::BadRequest(
            "まだデプロイされていません(git push か `tbm deploy --local` でデプロイしてください)"
                .into(),
        )
    })?;
    redeploy(&state, id, &digest, &git_sha, deploy::DeployTrigger::User).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// コンテナを停止 + route を消し、phase/desired を stopped にする(stop / delete が共有)。
/// **deploy_lock は呼び出し側が取る**(delete は soft-delete まで lock を保持して start と競合しない)。
async fn stop_containers(state: &AppState, id: Uuid) -> AppResult<()> {
    docker::stop_remove(state, id).await?;
    route::remove(state, id)?;
    sqlx::query(
        "UPDATE service_details SET desired_state='stopped', phase='stopped' WHERE resource_id=$1",
    )
    .bind(id)
    .execute(&state.db)
    .await?;
    Ok(())
}

/// service の停止(deploy ロック取得 + コンテナ停止 + route 削除)。**所有権チェックも audit も
/// しない素の操作** — ユーザ口(`stop`)と owner 代理(admin の最後の砦)が共有する(§5.2)。
pub(crate) async fn stop_service(state: &AppState, id: Uuid) -> AppResult<()> {
    // 並行 deploy / start と直列化(コンテナ / route の競合防止)。
    let lock = state.deploy_lock(id);
    let _guard = lock.lock().await;
    stop_containers(state, id).await
}

/// service のソフト削除(停止 → deleted_at/purge_after)。**所有権も audit もしない素の操作**。
/// lock を soft-delete まで保持(stop と delete の間に start が割り込んで孤児コンテナを作るのを防ぐ)。
pub(crate) async fn soft_delete(state: &AppState, id: Uuid) -> AppResult<()> {
    let lock = state.deploy_lock(id);
    let _guard = lock.lock().await;
    stop_containers(state, id).await?;
    // service は永続データを持たない(コンテナは deploy で再生成)→ trash_meta は無し。
    // **`deleted_at IS NULL` を条件に**:候補取得から実行までの間に並行削除が割り込んでも、
    // 既削除を二度消して「成功」audit を出さない(rows_affected==0 → NotFound)。lock で直列化
    // されるので、競合した 2 つの削除のうち後者がここで弾かれる。
    let res = sqlx::query(
        "UPDATE resources SET deleted_at = now(), purge_after = now() + interval '3 days'
          WHERE id = $1 AND kind = 'service' AND deleted_at IS NULL",
    )
    .bind(id)
    .execute(&state.db)
    .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    // 削除を実際に行った時だけ私網を撤去する(コンテナは stop_containers で除去済み = 順序 OK)。
    // 競合で rows_affected==0 の側は先行 deleter が撤去済みなので触らない。restore は次 deploy の
    // ensure_service_network で再生成されるので restore 側は無改修。失敗は reconcile の孤児 GC が回収。
    if let Err(e) = network::remove_service_network(state, id).await {
        tracing::warn!(error = ?e, %id, "soft_delete: 私網の撤去に失敗(reconcile が後で回収)");
    }
    Ok(())
}

/// `POST /api/services/:id/stop`:コンテナ停止 + route 削除(desired_state=stopped）。
pub async fn stop(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<StatusCode> {
    ensure_owned(&state, auth.user_id, id).await?;
    stop_service(&state, id).await?;
    audit(&state.db, Some(auth.user_id), "service.stop", id, json!({})).await;
    Ok(StatusCode::NO_CONTENT)
}

/// `?tail=N`。
#[derive(serde::Deserialize)]
pub struct LogsQuery {
    tail: Option<usize>,
}

/// `GET /api/services/:id/logs?tail=N`:走っているコンテナの直近ログ(stdout+stderr)。
pub async fn logs(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<LogsQuery>,
) -> AppResult<Json<LogsResp>> {
    ensure_owned(&state, auth.user_id, id).await?;
    let logs = docker::logs(&state, id, q.tail).await?;
    Ok(Json(LogsResp { logs }))
}

/// exec の argv 制限(暴走入力だけ弾く。表示名と同じ感覚の素直な上限)。
const MAX_EXEC_ARGS: usize = 64;
const MAX_EXEC_ARG_LEN: usize = 8192;

/// exec / terminal 共通:稼働中コンテナ名を解決するか、無ければ 400(停止中 / 未デプロイ)。
/// 所有権は呼び出し側が先に `ensure_owned` で確認する(exec は間に argv 検証を挟むため分離)。
async fn running_container_or_400(state: &AppState, id: Uuid) -> AppResult<String> {
    docker::running_container_name(state, id)
        .await?
        .ok_or_else(|| {
            AppError::BadRequest(
                "コンテナが走っていません。先にデプロイして running にしてください".into(),
            )
        })
}

/// `POST /api/services/:id/exec`:稼働中コンテナ内で 1 コマンドを **非対話**に実行し、
/// stdout/stderr/exit_code を捕獲して返す(`docker exec`(`-it` なし)相当 = AI / スクリプト /
/// 線上診断用。対話シェルは web ターミナル)。所有者の自資源のみ(`ensure_owned`)= 既存の
/// web SQL と同一ティアの暴露(env 注入値が見える等は受容済み)。argv はそのまま渡す
/// (shell 解釈なし):pipe/glob は呼び出し側が `sh -c` を組む。
pub async fn exec(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<ExecReq>,
) -> AppResult<Json<ExecResult>> {
    ensure_owned(&state, auth.user_id, id).await?;
    if req.cmd.is_empty() {
        return Err(AppError::BadRequest(
            "実行するコマンドが空です(例:tbm service exec <name> -- ps aux)".into(),
        ));
    }
    if req.cmd.len() > MAX_EXEC_ARGS || req.cmd.iter().any(|a| a.len() > MAX_EXEC_ARG_LEN) {
        return Err(AppError::BadRequest("コマンドが長すぎます".into()));
    }
    let name = running_container_or_400(&state, id).await?;
    // 監査は exec の **起動イベントと argv** を記録する(対話 PTY の打鍵は記録不可なのと対照的に、
    // 一発 exec はコマンドが残せる)。出力は秘密を含み得るので記録しない。
    audit(
        &state.db,
        Some(auth.user_id),
        "service.exec",
        id,
        json!({ "cmd": req.cmd }),
    )
    .await;
    let result = docker::exec_capture(&state, &name, req.cmd).await?;
    Ok(Json(result))
}

/// `GET /api/services/:id/terminal`(WebSocket):所有者が自分の稼働中コンテナ内で対話シェルを
/// 開く(**web 専用** — 対話 PTY は CLI の AI フレンドリ JSON 契約に合わない。CLI は一発 exec)。
/// 所有者の自資源のみ(`ensure_owned`)= web SQL と同一ティアの暴露。升级前にコンテナ稼働中を
/// 確認し、双方向ポンプは `docker::handle_terminal`(地雷はそちらのコメント)。
pub async fn terminal(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: axum::http::HeaderMap,
    ws: WebSocketUpgrade,
) -> AppResult<impl IntoResponse> {
    // CSWSH 対策:升级の Origin を管制面オリジンに固定する(SameSite=Lax は same-site の
    // テナント app からの WS 乗っ取りを防げない)。
    crate::auth::require_ws_origin(&headers, &state.config)?;
    // 対話ターミナルは **web 専用**(owner ガバナンスと同じく session 由来を要求 =
    // Bearer cli_token は拒否)。対話 PTY は CLI の AI フレンドリ JSON 契約に合わないので
    // 入口を web セッションに限る(`require_owner_web` と同じ作法)。
    if !auth.is_session() {
        return Err(AppError::Forbidden);
    }
    ensure_owned(&state, auth.user_id, id).await?;
    let name = running_container_or_400(&state, id).await?;
    // 監査は **open イベント**のみ記録する(対話 PTY の打鍵内容は裸ストリームで記録不可。
    // 一発 exec[service.exec] が argv を残せるのと対照的)。
    audit(
        &state.db,
        Some(auth.user_id),
        "service.terminal.open",
        id,
        json!({}),
    )
    .await;
    Ok(ws.on_upgrade(move |socket| docker::handle_terminal(socket, state, name)))
}

/// `DELETE /api/services/:id`:ソフト削除(コンテナ/route を消し、ゴミ箱へ。3 日で purge)。
pub async fn delete_service(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<StatusCode> {
    ensure_owned(&state, auth.user_id, id).await?;
    soft_delete(&state, id).await?;
    audit(
        &state.db,
        Some(auth.user_id),
        "service.delete",
        id,
        json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/services/:id/rollback`:履歴の指定 deploy の digest を新 deploy として再起動
/// (再 build なし — §6.8)。指定 deploy が他 service / 不在なら 404。
pub async fn rollback(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<RollbackReq>,
) -> AppResult<StatusCode> {
    ensure_owned(&state, auth.user_id, id).await?;
    // 指定 deploy はこの service のものに限る(IDOR 防止)。
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT image_digest, git_sha FROM deploys WHERE id = $1 AND service_id = $2",
    )
    .bind(req.deploy_id)
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    let (digest, git_sha) = row.ok_or(AppError::NotFound)?;
    redeploy(&state, id, &digest, &git_sha, deploy::DeployTrigger::User).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ===== 注入(database / volume → service。バインディングだけ保存、値は起動時解決)=====

/// 注入一覧の行(id, resource_id, kind, display_name, env_var, mount_path, valid)。
type InjectionRow = (Uuid, Uuid, String, String, String, Option<String>, bool);

fn injection_row_to_dto(r: InjectionRow) -> InjectionDto {
    InjectionDto {
        id: r.0,
        resource_id: r.1,
        resource_kind: r.2,
        resource_name: r.3,
        env_var: r.4,
        mount_path: r.5,
        valid: r.6,
    }
}

/// `GET /api/services/:id/injections`:注入一覧(失効 = valid:false も含む)。
pub async fn list_injections(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Vec<InjectionDto>>> {
    ensure_owned(&state, auth.user_id, id).await?;
    let rows: Vec<InjectionRow> = sqlx::query_as(
        "SELECT i.id, i.resource_id, r.kind, r.display_name, i.env_var, i.mount_path,
                (r.deleted_at IS NULL) AS valid
           FROM injections i JOIN resources r ON r.id = i.resource_id
          WHERE i.service_id = $1
          ORDER BY i.env_var",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows.into_iter().map(injection_row_to_dto).collect()))
}

/// `POST /api/services/:id/injections`:database / volume を service に注入する(バインディング)。
/// 反映には再デプロイ(値は起動の瞬間に解決 — 決定 #5)。
pub async fn create_injection(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<CreateInjectionReq>,
) -> AppResult<(StatusCode, Json<InjectionDto>)> {
    ensure_owned(&state, auth.user_id, id).await?;

    // 注入元は本人の database / volume / cache(未削除)。kind と表示名を取る。
    let resource: Option<(String, String)> = sqlx::query_as(
        "SELECT kind, display_name FROM resources
          WHERE id=$1 AND user_id=$2 AND kind IN ('database','volume','cache') AND deleted_at IS NULL",
    )
    .bind(req.resource_id)
    .bind(auth.user_id)
    .fetch_optional(&state.db)
    .await?;
    let (kind, name) = resource.ok_or(AppError::NotFound)?;

    // env_var / mount_path の既定を kind で決める。
    let (env_var, mount_path) = match kind.as_str() {
        "database" => (req.env_var.unwrap_or_else(|| "DATABASE_URL".into()), None),
        // cache は REDIS_URL(既定)。REDIS_KEY_PREFIX は inject.rs が env_var から導出する(§5)。
        "cache" => (req.env_var.unwrap_or_else(|| "REDIS_URL".into()), None),
        _ => {
            // volume
            let ev = req.env_var.unwrap_or_else(|| "STORAGE_PATH".into());
            let mp = req.mount_path.unwrap_or_else(|| format!("/data/{name}"));
            validate_mount_path(&mp)?;
            (ev, Some(mp))
        }
    };
    validate_env_key(&env_var)?;

    let new_id: Uuid = sqlx::query_scalar(
        "INSERT INTO injections (service_id, resource_id, env_var, mount_path)
              VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(id)
    .bind(req.resource_id)
    .bind(&env_var)
    .bind(&mount_path)
    .fetch_one(&state.db)
    .await
    .map_err(|e| {
        map_unique(
            e,
            format!("env 変数 '{env_var}' はこの service で既に使われています"),
        )
    })?;

    audit(
        &state.db,
        Some(auth.user_id),
        "service.inject",
        id,
        json!({ "resource_id": req.resource_id, "env_var": env_var }),
    )
    .await;

    Ok((
        StatusCode::CREATED,
        Json(InjectionDto {
            id: new_id,
            resource_id: req.resource_id,
            resource_kind: kind,
            resource_name: name,
            env_var,
            mount_path,
            valid: true,
        }),
    ))
}

/// `DELETE /api/injections/:id`:注入を外す(所有権は service 経由で確認)。
pub async fn delete_injection(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<StatusCode> {
    let owned: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM injections i JOIN resources r ON r.id = i.service_id
                        WHERE i.id = $1 AND r.user_id = $2)",
    )
    .bind(id)
    .bind(auth.user_id)
    .fetch_one(&state.db)
    .await?;
    if !owned {
        return Err(AppError::NotFound);
    }
    sqlx::query("DELETE FROM injections WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ===== 静的 env(値は暗号化保存。GET は key のみ — 値は秘密)=====

/// `GET /api/services/:id/env`:env の key 一覧。
pub async fn list_env(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Vec<String>>> {
    ensure_owned(&state, auth.user_id, id).await?;
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT key FROM service_env WHERE service_id = $1 ORDER BY key")
            .bind(id)
            .fetch_all(&state.db)
            .await?;
    Ok(Json(rows.into_iter().map(|(k,)| k).collect()))
}

/// `POST /api/services/:id/env`:静的 env を 1 件 upsert(値は暗号化)。反映には再デプロイ。
pub async fn set_env(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<SetEnvReq>,
) -> AppResult<StatusCode> {
    ensure_owned(&state, auth.user_id, id).await?;
    validate_env_key(&req.key)?;
    let value_enc = state.crypto.encrypt(&req.value)?;
    sqlx::query(
        "INSERT INTO service_env (service_id, key, value_enc) VALUES ($1, $2, $3)
              ON CONFLICT (service_id, key) DO UPDATE SET value_enc = EXCLUDED.value_enc",
    )
    .bind(id)
    .bind(&req.key)
    .bind(&value_enc)
    .execute(&state.db)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/services/:id/env/:key`:静的 env を 1 件削除。
pub async fn unset_env(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path((id, key)): Path<(Uuid, String)>,
) -> AppResult<StatusCode> {
    ensure_owned(&state, auth.user_id, id).await?;
    sqlx::query("DELETE FROM service_env WHERE service_id = $1 AND key = $2")
        .bind(id)
        .bind(&key)
        .execute(&state.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// env 変数名の検査(空 / `=` / NUL を弾く)。
fn validate_env_key(key: &str) -> AppResult<()> {
    if key.is_empty() || key.contains('=') || key.contains('\0') {
        return Err(AppError::BadRequest(
            "env のキーが不正です(空 / '=' / NUL は不可)".into(),
        ));
    }
    Ok(())
}

/// マウント先パスの検査(絶対パス + NUL / `:` なし)。`:` を弾くのは、bind 文字列
/// `<host_path>:<mount_path>` に `:ro` / `:rshared` などの bind オプション・伝播モードを
/// 注入されるのを防ぐため(オプション注入 → ホスト mount namespace への伝播の足場になりうる)。
fn validate_mount_path(path: &str) -> AppResult<()> {
    if !path.starts_with('/') || path.contains('\0') || path.contains(':') {
        return Err(AppError::BadRequest(
            "mount パスは絶対パスで、':' / NUL を含めないでください".into(),
        ));
    }
    Ok(())
}

/// `POST /api/services`:service の平台側メタを作る(resources + service_details +
/// deploy_key 生成 + subdomain 採番)。gh / registry 資格情報 / workflow は後チャンク。
/// **deploy_key は発行時の 1 回だけ**平文で返す(HMAC の鍵)。
pub async fn create(
    auth: AuthCtx,
    State(state): State<AppState>,
    Json(req): Json<CreateServiceReq>,
) -> AppResult<axum::response::Response> {
    let display_name = validate::name(&req.name, MAX_NAME_LEN)?;

    // 同名チェック(ゴミ箱内含む)。UNIQUE が最終ガードだが、先に弾いて分かりやすく。
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM resources WHERE user_id=$1 AND kind='service' AND display_name=$2)",
    )
    .bind(auth.user_id)
    .bind(&display_name)
    .fetch_one(&state.db)
    .await?;
    if exists {
        return Err(AppError::Conflict(format!(
            "サービス名 '{display_name}' は既に使われています(ゴミ箱内を含む)。別の名前にしてください"
        )));
    }

    // registry アカウントは service 行を作る **前**に用意する(per-user で service に
    // 依存しない)。ここで失敗しても service の孤児行は残らない — 失敗後に同名で再作成
    // できる(insert を先にすると、ensure_account 失敗で service だけ残り deploy_key を
    // 二度と返せず、再作成も 409 で詰む)。
    let registry = registry::ensure_account(&state, auth.user_id).await?;

    let deploy_key = tsubomi_shared::random_b64(DEPLOY_KEY_BYTES);
    let deploy_key_enc = state.crypto.encrypt(&deploy_key)?;

    // subdomain は display_name の slug を第一候補に、衝突 / 予約語なら乱数語を付けて再試行
    // (UNIQUE が最終ガード)。slug が空になる名前(記号だけ等)は "app" にフォールバック。
    let base = {
        let s = slugify(&display_name);
        if s.is_empty() { "app".to_string() } else { s }
    };
    let mut created: Option<ServiceDto> = None;
    for attempt in 0..6 {
        let candidate = if attempt == 0 {
            base.clone()
        } else {
            format!("{base}-{}", rand_suffix())
        };
        if RESERVED_SUBDOMAINS.contains(&candidate.as_str()) {
            continue;
        }
        match insert_attempt(
            &state.db,
            &state.config,
            auth.user_id,
            &display_name,
            &candidate,
            &deploy_key_enc,
        )
        .await
        {
            Ok(dto) => {
                created = Some(dto);
                break;
            }
            Err(InsertErr::SubdomainTaken) => continue,
            Err(InsertErr::App(e)) => return Err(e),
        }
    }
    let dto = created.ok_or_else(|| {
        AppError::Conflict(
            "subdomain を生成できませんでした。表示名を変えて再試行してください".into(),
        )
    })?;

    audit(
        &state.db,
        Some(auth.user_id),
        "service.create",
        dto.id,
        json!({ "display_name": display_name, "subdomain": dto.subdomain }),
    )
    .await;

    // GitHub 連携に必要な残りの値(平台は GitHub に触れない — CLI/web がこの値で組み立てる)。
    // setup_commands は平台が単一真源として作る(CLI/web は文字列を再構築しない)。registry は
    // service 作成より前に用意済み(上)。
    let hook_url = format!("{}/api/hook/deploy", state.config.server_url);
    let platforms = state.config.platforms.clone();
    let setup_commands =
        workflow::setup_commands(&dto, &deploy_key, &registry, &hook_url, &platforms);

    Ok(crate::respond::no_store_created(CreateServiceResp {
        service: dto,
        deploy_key,
        registry,
        hook_url,
        platforms,
        workflow_yaml: workflow::TEMPLATE.to_string(),
        setup_commands,
    }))
}

/// insert_attempt の失敗は 2 種:subdomain の UNIQUE 違反(呼び出し側でリトライ)と
/// それ以外(そのまま返す)。
enum InsertErr {
    SubdomainTaken,
    App(AppError),
}

impl From<sqlx::Error> for InsertErr {
    fn from(e: sqlx::Error) -> Self {
        InsertErr::App(AppError::Sqlx(e))
    }
}

/// resources + service_details を 1 トランザクションで挿入する 1 回の試行。
/// anon_seq はユーザ単位で advisory lock を取って直列化する(同時 create の競合防止)。
async fn insert_attempt(
    db: &PgPool,
    config: &Config,
    user_id: Uuid,
    display_name: &str,
    subdomain: &str,
    deploy_key_enc: &[u8],
) -> Result<ServiceDto, InsertErr> {
    // subdomain の UNIQUE 違反だけリトライさせ、それ以外(表示名衝突など)は
    // 既存の map_unique に委ねる(unique → 409 Conflict、その他 → Sqlx)。
    let classify = |e: sqlx::Error| -> InsertErr {
        if let sqlx::Error::Database(d) = &e
            && d.is_unique_violation()
            && d.constraint().is_some_and(|c| c.contains("subdomain"))
        {
            return InsertErr::SubdomainTaken;
        }
        InsertErr::App(map_unique(
            e,
            format!("サービス名 '{display_name}' は既に使われています"),
        ))
    };

    let mut tx = db.begin().await?;
    // anon_seq 採番の直列化。ロック鍵は kind ごとに別(database=42/cache=43/volume=44/service=45)=
    // 跨 kind 並行 create を無駄に直列化しない(perf review P6)。
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1::text), 45)")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    let anon_seq: i32 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(anon_seq),0)+1 FROM resources WHERE user_id=$1 AND kind='service'",
    )
    .bind(user_id)
    .fetch_one(&mut *tx)
    .await?;

    let (id, created_at): (Uuid, DateTime<Utc>) = sqlx::query_as(
        "INSERT INTO resources (user_id, kind, display_name, anon_seq)
              VALUES ($1, 'service', $2, $3) RETURNING id, created_at",
    )
    .bind(user_id)
    .bind(display_name)
    .bind(anon_seq)
    .fetch_one(&mut *tx)
    .await
    .map_err(classify)?;

    sqlx::query(
        "INSERT INTO service_details (resource_id, subdomain, deploy_key_enc) VALUES ($1, $2, $3)",
    )
    .bind(id)
    .bind(subdomain)
    .bind(deploy_key_enc)
    .execute(&mut *tx)
    .await
    .map_err(classify)?;

    tx.commit().await?;

    Ok(ServiceDto {
        id,
        display_name: display_name.to_owned(),
        anon_seq,
        created_at,
        subdomain: subdomain.to_owned(),
        phase: "created".into(),
        desired_state: "stopped".into(),
        container_port: 8080,
        image_digest: None,
        last_deploy_at: None,
        url: config.service_url(subdomain),
    })
}

/// display_name → DNS ラベル安全な slug(英小文字 / 数字 / 単一ハイフン、英字始まり、
/// 50 字以内)。記号だけ等で空になることがある(呼び出し側がフォールバックする)。
fn slugify(name: &str) -> String {
    let mut s = String::with_capacity(name.len());
    let mut prev_hyphen = false;
    for c in name.chars() {
        let lc = c.to_ascii_lowercase();
        if lc.is_ascii_alphanumeric() {
            s.push(lc);
            prev_hyphen = false;
        } else if !s.is_empty() && !prev_hyphen {
            s.push('-');
            prev_hyphen = true;
        }
    }
    let s = s.trim_matches('-');
    // 英字始まりに寄せる(DNS ラベルとして安全側。数字始まり / 空は 's' を前置)。
    let s = match s.chars().next() {
        Some(c) if c.is_ascii_alphabetic() => s.to_string(),
        Some(_) => format!("s{s}"),
        None => return String::new(),
    };
    s.chars()
        .take(50)
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

/// 衝突回避用の 4 文字英数字サフィックス(DNS ラベル安全)。
fn rand_suffix() -> String {
    use rand::Rng;
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut buf = [0u8; 4];
    rand::rng().fill_bytes(&mut buf);
    buf.iter()
        .map(|&b| ALPHABET[(b as usize) % ALPHABET.len()] as char)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("My App"), "my-app");
        assert_eq!(slugify("  hello--world  "), "hello-world");
        assert_eq!(slugify("API_v2"), "api-v2");
        assert_eq!(slugify("123start"), "s123start");
        assert_eq!(slugify("!!!"), "");
        assert_eq!(slugify("日本語app"), "app");
    }

    #[test]
    fn rand_suffix_is_dns_safe() {
        for _ in 0..200 {
            let s = rand_suffix();
            assert_eq!(s.len(), 4);
            assert!(
                s.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            );
        }
    }
}
