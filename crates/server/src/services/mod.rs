//! service リソースの API(tech-design §6 の service 面)。web と CLI は同一ハンドラの
//! 2 入口 — 認証 extractor(AuthCtx)だけが分岐点。
//!
//! M3 第 1 チャンク(S1–S3、曳光弾)は最小 create + deploy hook + コンテナ起動まで。
//! gh オーケストレーション / 注入 / start・stop・logs / rollback / web 画面 / reconcile は
//! 後チャンク(plan・doc/paas-m3-design.md)。

pub mod deploy;
pub mod docker;
pub mod egress;
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
    ExecResult, InjectionDto, LogsResp, ResolvedEnvDto, RollbackReq, ServiceDto,
    SetServiceVisibilityReq, SetEnvReq, SetEnvResp,
};
use uuid::Uuid;

const MAX_NAME_LEN: usize = 64;
/// subdomain 生成の予約語(平台 / インフラのホスト名と衝突させない)。
const RESERVED_SUBDOMAINS: &[&str] = &["paas", "registry", "traefik", "www", "api"];
/// deploy_key の乱数バイト数(base64url で ≈43 字)。HMAC の鍵そのもの。
const DEPLOY_KEY_BYTES: usize = 32;
/// 平台の HTTP 契約港(PORT env の既定 = workflow / traefik の想定)。visibility 推導の基準。
/// INSERT が常に列を明示するので実効真源はこの定数 — DDL の DEFAULT 8080 と一致させること。
const DEFAULT_CONTAINER_PORT: i32 = 8080;
const CONTAINER_PORT_RANGE: std::ops::RangeInclusive<i32> = 1..=65535;
/// メモリ硬上限の既定 / 範囲(MiB)。既定 **1024** = migration 20260620 が OOM 対策で
/// 512→1024 へ引き上げた DDL DEFAULT と一致させる(512 に戻すと是正の逆行)。
/// 下限は最小級の app、上限は 16GB 共有ホストの節度。
const DEFAULT_MEMORY_MB: i32 = 1024;
const MEMORY_MB_RANGE: std::ops::RangeInclusive<i32> = 128..=4096;

/// 公開範囲(`service_details.visibility`)。DB の CHECK と対を成す単一真源 —
/// API 入力検証(不正値は 400)と route 分岐(ipallow 有無)をここに集約する。
/// 意味論は公開範囲設計 §0:private = route ファイルを書かない(公網不可視・subdomain 温存)、
/// company = 既定(route + 会社 IP 許可リスト)、public = route はあるが ipallow を挂けない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Visibility {
    Private,
    Company,
    Public,
}

impl Visibility {
    /// DB / API の文字列表現から。未知は None(API 側で 400 にする。DB は CHECK が保証)。
    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s {
            tsubomi_shared::VISIBILITY_PRIVATE => Some(Self::Private),
            tsubomi_shared::VISIBILITY_COMPANY => Some(Self::Company),
            tsubomi_shared::VISIBILITY_PUBLIC => Some(Self::Public),
            _ => None,
        }
    }

    /// DB 由来の値を読む(CHECK が保証するが防御的に:未知値は既定の company へ倒す)。
    /// 「触らない」に倒したい読み手(reconcile の fresh 再確認)は `parse` を使う — 方針の違いは意図。
    pub(crate) fn from_db(s: &str) -> Self {
        Self::parse(s).unwrap_or(Self::Company)
    }

    /// `parse` の逆(DB / DTO へ書く文字列)。
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Private => tsubomi_shared::VISIBILITY_PRIVATE,
            Self::Company => tsubomi_shared::VISIBILITY_COMPANY,
            Self::Public => tsubomi_shared::VISIBILITY_PUBLIC,
        }
    }

    /// route に会社 IP 許可リスト middleware を挂けるか(public だけ外す)。
    pub(crate) fn ipallow(self) -> bool {
        !matches!(self, Self::Public)
    }
}

/// visibility 省略時の既定を port から推導する(stateful 設計 §0-B。推導は create のこの一度きり —
/// 以後 port と visibility は独立)。8080 = 平台の HTTP 契約港 → 従来どおり company。それ以外 =
/// 非 HTTP ソフト(自帯 DB 等)の想定 → private(traefik は HTTP しか話せないので route が
/// 在っても乱码/502 の噪音にしかならない。公開したい非 8080 の HTTP 工具は明示指定で開ける)。
fn default_visibility(container_port: i32) -> Visibility {
    if container_port == DEFAULT_CONTAINER_PORT {
        Visibility::Company
    } else {
        Visibility::Private
    }
}

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
        .route("/services/{id}/visibility", post(set_visibility))
        .route("/services/{id}/deploys", get(deploys))
        .route("/services/{id}/deploy-config", get(deploy_config))
        .route(
            "/services/{id}/injections",
            get(list_injections).post(create_injection),
        )
        .route("/injections/{id}", delete(delete_injection))
        .route("/services/{id}/env", get(list_env).post(set_env))
        .route("/services/{id}/env/resolved", get(list_env_resolved))
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
    String,                // visibility
    bool,                  // stateful
    i32,                   // memory_mb
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
        visibility: r.10,
        stateful: r.11,
        memory_mb: r.12,
    }
}

/// `GET /api/services`:自分の service 一覧(ゴミ箱内は除く)。秘密は含まない。
pub async fn list(
    auth: AuthCtx,
    State(state): State<AppState>,
) -> AppResult<Json<Vec<ServiceDto>>> {
    let rows: Vec<ServiceRow> = sqlx::query_as(
        "SELECT r.id, r.display_name, r.anon_seq, r.created_at,
                s.subdomain, s.phase, s.desired_state, s.container_port, s.image_digest, s.last_deploy_at,
                s.visibility, s.stateful, s.memory_mb
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
                s.subdomain, s.phase, s.desired_state, s.container_port, s.image_digest, s.last_deploy_at,
                s.visibility, s.stateful, s.memory_mb
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

/// deploys 行(id, git_sha, image_digest, status, error, created_at, finished_at, commit_message)。
type DeployRow = (
    Uuid,
    String,
    String,
    String,
    Option<String>,
    DateTime<Utc>,
    Option<DateTime<Utc>>,
    Option<String>,
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
        commit_message: r.7,
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
        "SELECT id, git_sha, image_digest, status, error, created_at, finished_at, commit_message
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

/// 直近に成功した deploy の `(image_digest, git_sha, commit_message)`(同じ行なので整合)。
/// 1 件も無ければ未デプロイ。start(現行を再起動)と reconcile(消えたコンテナを復活)が共有する。
pub(crate) async fn latest_succeeded_deploy(
    state: &AppState,
    service_id: Uuid,
) -> AppResult<Option<(String, String, Option<String>)>> {
    Ok(sqlx::query_as(
        "SELECT image_digest, git_sha, commit_message FROM deploys
          WHERE service_id = $1 AND status = 'succeeded'
          ORDER BY created_at DESC LIMIT 1",
    )
    .bind(service_id)
    .fetch_optional(&state.db)
    .await?)
}

/// 直近に成功した deploy の **id**。route が指すべき容器名は `deploy::container_name(service_id, この id)`
/// で一意に決まる(start-first の命名規約)。reconcile の route ドリフト収束 / 中断デプロイ復旧が、
/// 「走っている任意の容器」ではなく**この容器**を正とするための真源(新旧併存時に route を旧へ巻き戻さない)。
pub(crate) async fn latest_succeeded_deploy_id(
    state: &AppState,
    service_id: Uuid,
) -> AppResult<Option<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM deploys
          WHERE service_id = $1 AND status = 'succeeded'
          ORDER BY created_at DESC LIMIT 1",
    )
    .bind(service_id)
    .fetch_optional(&state.db)
    .await?)
}

/// serving すべき容器名 = **直近成功 deploy の容器**(`container_name`)を DB から導く
/// (実走確認はしない)。成功 deploy 無し = 未デプロイは None。
async fn expected_container_name(state: &AppState, id: Uuid) -> Option<String> {
    let deploy_id = match latest_succeeded_deploy_id(state, id).await {
        Ok(Some(d)) => d,
        Ok(None) => return None,
        Err(e) => {
            tracing::warn!(error = ?e, %id, "serving 容器の解決:直近成功 deploy の取得に失敗");
            return None;
        }
    };
    Some(deploy::container_name(id, deploy_id))
}

/// serving すべき容器名が今 `running_names` に居る(= 実際に走っている)時だけ Some。走っていない
/// (mid-deploy / クラッシュ)や成功 deploy 無しは None。新旧併存時に「正しい新版」を一意に選ぶ
/// 唯一の判断点(reconcile の route drift 収束と網リンクの callee 解決が共有 — route ファイルでは
/// なく DB を真源にするので private でも解ける)。
pub(crate) async fn expected_running_container(
    state: &AppState,
    id: Uuid,
    running_names: &[String],
) -> Option<String> {
    let expected = expected_container_name(state, id).await?;
    running_names.contains(&expected).then_some(expected)
}

/// `expected_running_container` の糖衣:docker から実走一覧を引いてから判定する。
/// `attach_callees`(網リンク)と visibility 切替が使う(reconcile は presence を既に
/// 手に持っているので本体を直接呼ぶ — docker 照会を二重にしない)。
/// SQL を先に引く — 未デプロイの callee で docker 照会を無駄撃ちしない。
pub(crate) async fn serving_container(state: &AppState, id: Uuid) -> Option<String> {
    let expected = expected_container_name(state, id).await?;
    let (_, running) = docker::presence(state, id).await.ok()?;
    running.contains(&expected).then_some(expected)
}

/// 指定 digest を新しい deploy として起こす(start / rollback / reconcile が共有)。deploys 行を
/// received で作り、run_digest を **await**(run_digest 内で deploy_lock + start-first swap + 状態記録)。
pub(crate) async fn redeploy(
    state: &AppState,
    service_id: Uuid,
    image_digest: &str,
    git_sha: &str,
    commit_message: Option<&str>,
    trigger: deploy::DeployTrigger,
) -> AppResult<()> {
    let deploy_id: Uuid = sqlx::query_scalar(
        "INSERT INTO deploys (service_id, git_sha, image_digest, status, commit_message)
              VALUES ($1, $2, $3, 'received', $4) RETURNING id",
    )
    .bind(service_id)
    .bind(git_sha)
    .bind(image_digest)
    .bind(commit_message)
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
    // 直近に成功した deploy の (digest, git_sha, message) を再起動する。1 件も無ければ未デプロイ。
    let (digest, git_sha, msg) = latest_succeeded_deploy(&state, id).await?.ok_or_else(|| {
        AppError::BadRequest(
            "まだデプロイされていません(git push か `tbm deploy --local` でデプロイしてください)"
                .into(),
        )
    })?;
    redeploy(
        &state,
        id,
        &digest,
        &git_sha,
        msg.as_deref(),
        deploy::DeployTrigger::User,
    )
    .await?;
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
    audit(
        &state.db,
        Some(auth.user_id),
        "service.stop",
        id,
        json!({}),
        auth.client_ip.as_deref(),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/services/:id/visibility`:公開範囲の切替(所有者のみ。公開範囲設計 §7)。
/// **即時反映** — route ファイルは DB の期望状態から再生成できるので、lock 内で DB を先に更新し
/// (背骨:DB=期望状態)、現実(ファイル)をその場で収束させる。env 注入と違い再デプロイ不要。
/// public(ipallow 無し = 全網公開)も**本人裁量 + audit 兜底**で owner 限定にしない(§0-C)。
pub async fn set_visibility(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<SetServiceVisibilityReq>,
) -> AppResult<StatusCode> {
    ensure_owned(&state, auth.user_id, id).await?; // 404 ゲート(lock 外・安価)
    let vis = Visibility::parse(&req.visibility).ok_or_else(|| {
        AppError::BadRequest(
            "visibility は private / company / public のいずれかにしてください".into(),
        )
    })?;

    // deploy / start / stop / delete と同一 lock で直列化(route とコンテナ状態の競合防止)。
    let lock = state.deploy_lock(id);
    let _guard = lock.lock().await;

    // DB 先(背骨:DB=期望状態)。lock 待ちの間に削除が完走したケースは rows=0 → 404。
    let row: Option<(String, i32)> = sqlx::query_as(
        "UPDATE service_details s SET visibility = $2
           FROM resources r
          WHERE s.resource_id = $1 AND r.id = s.resource_id AND r.deleted_at IS NULL
        RETURNING s.subdomain, s.container_port",
    )
    .bind(id)
    .bind(vis.as_str())
    .fetch_optional(&state.db)
    .await?;
    let (subdomain, container_port) = row.ok_or(AppError::NotFound)?;

    // 恒久的な状態変化(DB)の直後に監査 — 後段の収束が失敗しても監査は DB と一致する。
    audit(
        &state.db,
        Some(auth.user_id),
        "service.visibility",
        id,
        json!({ "visibility": vis.as_str() }),
        auth.client_ip.as_deref(),
    )
    .await;

    // 現実収束(lock 内)。失敗しても DB は更新済み = reconcile が ≤30s で収束させるので、
    // 文案直通の 503(UnavailableMsg)で「次の一手」を返す(AI が自己修正できる — CLI 契約。
    // 通常の 5xx は into_response が「内部エラー」に編校し文案が届かない)。生エラーは log のみ
    // (クライアントへ内部詳細は出さない)。
    let converge_err = |e: AppError| {
        tracing::error!(error = ?e, %id, "visibility 切替の route 反映に失敗");
        AppError::UnavailableMsg(
            "公開範囲は保存しましたが route の反映に失敗しました。reconcile が 30 秒以内に収束させます(再実行も可能)".into(),
        )
    };
    match vis {
        Visibility::Private => route::remove(&state, id).map_err(converge_err)?,
        Visibility::Company | Visibility::Public => {
            // serving 中(直近成功 deploy の容器が実走)の時だけ route を書く。停止 / 未デプロイは
            // 何も書かない —「停止 service に route ファイル無し」の不変条件を守り、次の
            // start / deploy が新しい visibility で書く(§7)。
            if let Some(container) = serving_container(&state, id).await {
                route::write(
                    &state,
                    id,
                    &subdomain,
                    &container,
                    container_port,
                    vis.ipallow(),
                )
                .map_err(converge_err)?;
            }
        }
    }
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
        auth.client_ip.as_deref(),
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
        auth.client_ip.as_deref(),
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
        auth.client_ip.as_deref(),
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
    // 指定 deploy はこの service のものに限る(IDOR 防止)。message も引き継ぐ(履歴の見出しが空かない)。
    let row: Option<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT image_digest, git_sha, commit_message FROM deploys WHERE id = $1 AND service_id = $2",
    )
    .bind(req.deploy_id)
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    let (digest, git_sha, msg) = row.ok_or(AppError::NotFound)?;
    redeploy(
        &state,
        id,
        &digest,
        &git_sha,
        msg.as_deref(),
        deploy::DeployTrigger::User,
    )
    .await?;
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

/// `POST /api/services/:id/injections`:database / volume / cache / **別 service** を注入する
/// (バインディング)。反映には再デプロイ(値は起動の瞬間に解決 — 決定 #5)。service 注入は
/// 内部直連 URL を渡し、網リンクは deploy / reconcile が張る(`doc/paas-service-link-design.md`)。
pub async fn create_injection(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<CreateInjectionReq>,
) -> AppResult<(StatusCode, Json<InjectionDto>)> {
    ensure_owned(&state, auth.user_id, id).await?;

    // 注入元は本人の database / volume / cache / service(未削除)。kind・表示名・subdomain(service のみ)を取る。
    // 源クエリが user_id=$2 で縛るので、別ユーザの資源は NotFound = **同一 owner 限定は自動で担保**。
    let resource: Option<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT r.kind, r.display_name, sd.subdomain
           FROM resources r
           LEFT JOIN service_details sd ON sd.resource_id = r.id
          WHERE r.id=$1 AND r.user_id=$2
            AND r.kind IN ('database','volume','cache','service') AND r.deleted_at IS NULL",
    )
    .bind(req.resource_id)
    .bind(auth.user_id)
    .fetch_optional(&state.db)
    .await?;
    let (kind, name, subdomain) = resource.ok_or(AppError::NotFound)?;

    // env_var / mount_path の既定を kind で決める。
    let (env_var, mount_path) = match kind.as_str() {
        "database" => (req.env_var.unwrap_or_else(|| "DATABASE_URL".into()), None),
        // cache は REDIS_URL(既定)。REDIS_KEY_PREFIX は inject.rs が env_var から導出する(§5)。
        "cache" => (req.env_var.unwrap_or_else(|| "REDIS_URL".into()), None),
        "service" => {
            // 自注入禁止(自分の URL を自分に注ぐのは無意味で、網リンクも自網へ自分を入れる無駄になる)。
            if req.resource_id == id {
                return Err(AppError::BadRequest("service 自身は注入できません".into()));
            }
            // 既定 env 名は subdomain から導く(例 api-backend → API_BACKEND_URL)。subdomain は
            // kind='service' なら service_details(1:1)に必ず在る = LEFT JOIN で Some。万一欠落
            // (データ不整合)ならハンドラを panic させず 500 に倒す(codex 監査:リクエスト経路で panic させない)。
            let subdomain = subdomain.ok_or_else(|| {
                AppError::Other(anyhow::anyhow!(
                    "service {} に service_details がありません(データ不整合)",
                    req.resource_id
                ))
            })?;
            let ev = req
                .env_var
                .unwrap_or_else(|| default_service_env_var(&subdomain));
            (ev, None)
        }
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
        auth.client_ip.as_deref(),
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

/// `DELETE /api/injections/:id`:注入を外す(所有権は service 経由で確認)。service 注入なら
/// caller の私網から callee を即切断する(網リンクの掃除。再デプロイ不要)。
pub async fn delete_injection(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<StatusCode> {
    // 所有権確認 + 掃除に要る情報を一発で取る(caller=i.service_id / 源=i.resource_id / 源 kind)。
    let row: Option<(Uuid, Uuid, String)> = sqlx::query_as(
        "SELECT i.service_id, i.resource_id, src.kind
           FROM injections i
           JOIN resources r   ON r.id = i.service_id
           JOIN resources src ON src.id = i.resource_id
          WHERE i.id = $1 AND r.user_id = $2",
    )
    .bind(id)
    .bind(auth.user_id)
    .fetch_optional(&state.db)
    .await?;
    let (caller_id, source_id, source_kind) = row.ok_or(AppError::NotFound)?;

    sqlx::query("DELETE FROM injections WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await?;

    // service↔service リンクなら caller 網から callee を即 detach(best-effort。失敗しても callee の
    // 次回 redeploy で自然消滅 = 同 owner なので無害)。db/volume/cache は網リンク無しなので何もしない。
    if source_kind == "service" {
        network::detach_callee(&state, caller_id, source_id).await;
    }
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

/// `GET /api/services/:id/env/resolved`:注入バインディングを**今この瞬間**に解決した env 一覧
/// (由来付き)。コンテナの実値は起動の瞬間に解決される(決定 #5)ので、これは「次のデプロイで
/// こうなる」プレビューでもある。「注入値が探针でしか確認できない」という実利用フィードバック #6
/// への回答。伏せ方(codex 監査):
/// - **静的 env の値は `***`**(`GET /env` の「key のみ = 値は秘密」契約と揃える。ユーザ自身が
///   設定した値なので見せる意味も薄い)
/// - 注入値は URL のパスワード部だけ `***`(知りたいのはホスト / 形 — フィードバックの本題)
///
/// 重複キーは deploy と同じ **後勝ち**で畳んでから返す(コンテナに入る実際の 1 本と一致させる。
/// deploy.rs::dedup_env_last と同じ規則 + ここでは表示順の安定のため出現順を保つ)。
pub async fn list_env_resolved(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<axum::response::Response> {
    ensure_owned(&state, auth.user_id, id).await?;
    // 由来の判定用:静的 env の key 集合と、注入の env_var → kind 対応。
    let static_keys: Vec<(String,)> =
        sqlx::query_as("SELECT key FROM service_env WHERE service_id = $1")
            .bind(id)
            .fetch_all(&state.db)
            .await?;
    let static_keys: std::collections::HashSet<String> =
        static_keys.into_iter().map(|(k,)| k).collect();
    let inj_kinds: Vec<(String, String)> = sqlx::query_as(
        "SELECT i.env_var, r.kind FROM injections i JOIN resources r ON r.id = i.resource_id
          WHERE i.service_id = $1",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    let inj_kinds: std::collections::HashMap<String, String> = inj_kinds.into_iter().collect();

    // resolve は静的 env → 注入の順で並ぶ(inject.rs)。由来は**出現位置**で判定する:
    // 同じキーの初出が静的 / 2 度目以降は注入(deploy の後勝ちで実際に効く方)。キーだけで
    // 引くと static と注入が衝突したとき両方 static 扱いになり、後勝ちの実態と食い違う。
    let (env, _binds) = inject::resolve(&state, id).await?;
    let mut seen = std::collections::HashSet::new();
    let labeled: Vec<ResolvedEnvDto> = env
        .into_iter()
        .map(|(key, value)| {
            let first = seen.insert(key.clone());
            let source = if first && static_keys.contains(&key) {
                "static".to_string()
            } else if let Some(kind) = inj_kinds.get(&key) {
                kind.clone()
            } else {
                // 注入 env_var の対応表に無い = cache の派生キー(`_URL` と対で注入される
                // `_KEY_PREFIX`。派生キーを生むのは今は inject.rs::key_prefix_env だけ —
                // 新しい派生元を足すときはここの判定も更新すること)。
                "cache".to_string()
            };
            let value = if source == "static" {
                "***".to_string()
            } else {
                mask_url_password(&value)
            };
            ResolvedEnvDto { key, value, source }
        })
        .collect();
    // 後勝ち dedup(出現順は保つ):後ろから見て初出だけ残す → 反転で元の順へ。
    let mut kept_keys = std::collections::HashSet::new();
    let mut list: Vec<ResolvedEnvDto> = labeled
        .into_iter()
        .rev()
        .filter(|e| kept_keys.insert(e.key.clone()))
        .collect();
    list.reverse();
    // 秘密(接続文字列の断片等)を含み得るので no-store(respond.rs の契約)。
    Ok(crate::respond::no_store(list))
}

/// URL 形(`scheme://user:pass@host…`)の値のパスワード部だけを `***` に伏せる。
/// URL でない値はそのまま(STORAGE_PATH / 前缀 / 静的 env は原文 — 暴露ティアは exec と同じで、
/// これは事故防止のエチケット)。
fn mask_url_password(value: &str) -> String {
    // scheme:// と @ の間に `user:pass` があるときだけ pass を置換。素朴なパースで十分
    // (自前生成の接続文字列が対象。誤検出しても「伏せすぎ」に倒れるだけ)。
    let Some(scheme_end) = value.find("://") else {
        return value.to_string();
    };
    let rest = &value[scheme_end + 3..];
    let Some(at) = rest.find('@') else {
        return value.to_string();
    };
    let userinfo = &rest[..at];
    let Some(colon) = userinfo.find(':') else {
        return value.to_string();
    };
    format!(
        "{}{}:***{}",
        &value[..scheme_end + 3],
        &userinfo[..colon],
        &rest[at..]
    )
}

/// `POST /api/services/:id/env`:静的 env を 1 件 upsert(値は暗号化)。反映には再デプロイ。
/// 値が公開 DB ホストを指す場合は非破壊の注意喚起(注入へ誘導)を `warning` に載せる(§7.2 footgun)。
pub async fn set_env(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<SetEnvReq>,
) -> AppResult<Json<SetEnvResp>> {
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
    let warning = public_db_env_warning(&state, id, &req.key, &req.value).await;
    Ok(Json(SetEnvResp { warning }))
}

/// 静的 env の値が公開 DB ホストを指していれば注意文を返す(非破壊の footgun 検知)。
/// コンテナは edge 網内なので DB は **注入(内部接続文字列)**で繋ぐべき:公開文字列を静的 env に
/// 置くと外部経路を一周(遅延)+ human role で `tbm db rotate` 後に黙って切れる。公開 DB 機能が
/// 無効な部署では公開入口が無い = footgun も無いので黙る。値は秘密なので含めず、KEY とホストだけ出す。
async fn public_db_env_warning(
    state: &AppState,
    service_id: Uuid,
    key: &str,
    value: &str,
) -> Option<String> {
    let host = state.config.db_public_host.as_str();
    if !value_points_at_public_db(state.config.db_public_enabled, host, value) {
        return None;
    }
    // 誘導コマンドに実 service 名を埋める(引けなければ汎用プレースホルダ)。
    let svc_name: String = sqlx::query_as("SELECT display_name FROM resources WHERE id = $1")
        .bind(service_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .map(|(n,): (String,)| n)
        .unwrap_or_else(|| "<service名>".to_string());
    Some(format!(
        "env '{key}' は公開 DB ホスト({host})を指しています。コンテナはアプリ内から内部接続文字列を\
         使うべきです — 静的 env ではなく注入を使ってください:`tbm inject <db名> --into \"{svc_name}\"`\
         (低遅延・rotate で切れない)。公開文字列を静的 env に置くと外部経路に出て、`tbm db rotate` で\
         黙って切れます。"
    ))
}

/// 値が公開 DB の接続文字列を指すか(純粋判定)。公開機能 off / ホスト空 / 不一致なら false。
/// Postgres URI 形(`postgres(ql)://…`)に限定して誤検知を抑える(dev の `127.0.0.1` ホストでも、
/// `http://127.0.0.1` 等の無関係な値を拾わない)。libpq keyword 形は稀なので非破壊機能として許容。
fn value_points_at_public_db(enabled: bool, host: &str, value: &str) -> bool {
    enabled
        && !host.is_empty()
        && (value.starts_with("postgres://") || value.starts_with("postgresql://"))
        && value.contains(host)
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
    // 制御文字(NUL 含む)を拒否:KEY は警告文・ログに出るので ANSI エスケープ等で出力を汚させない。
    if key.is_empty() || key.contains('=') || key.chars().any(|c| c.is_control()) {
        return Err(AppError::BadRequest(
            "env のキーが不正です(空 / '=' / 制御文字は不可)".into(),
        ));
    }
    Ok(())
}

/// service 注入の既定 env 名を subdomain から導く:英数は大文字化・それ以外は `_`・先頭が
/// 数字なら `_` を前置・末尾に `_URL`(例 `api-backend` → `API_BACKEND_URL`)。`validate_env_key`
/// (空 / `=` / 制御文字のみ拒否)を必ず通る形を返す(subdomain は DNS 安全 `[a-z0-9-]` 非空)。
fn default_service_env_var(subdomain: &str) -> String {
    let mut s: String = subdomain
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    if s.starts_with(|c: char| c.is_ascii_digit()) {
        s.insert(0, '_');
    }
    format!("{s}_URL")
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
/// deploy_key(HMAC の鍵原文)は作成時にここで平文返却する。なお所有者は後から
/// `GET /services/:id/deploy-config`(`tbm deploy --local` の退路)で **再取得できる**(自分の
/// service のみ)— 平文を平台が持つので可能。**rotate API はまだ無い**:鍵漏洩時はサービスを
/// 削除して作り直す(per-service deploy_key/registry pass の rotate は後相 §で検討)。
pub async fn create(
    auth: AuthCtx,
    State(state): State<AppState>,
    Json(req): Json<CreateServiceReq>,
) -> AppResult<axum::response::Response> {
    let display_name = validate::name(&req.name, MAX_NAME_LEN)?;

    // 任意パラメータの確定(検証 + 既定)。既定の単一真源はここ — CLI / web は None を素通しする。
    let container_port = req.container_port.unwrap_or(DEFAULT_CONTAINER_PORT);
    if !CONTAINER_PORT_RANGE.contains(&container_port) {
        return Err(AppError::BadRequest(format!(
            "container_port は {}〜{} にしてください",
            CONTAINER_PORT_RANGE.start(),
            CONTAINER_PORT_RANGE.end()
        )));
    }
    let memory_mb = req.memory_mb.unwrap_or(DEFAULT_MEMORY_MB);
    if !MEMORY_MB_RANGE.contains(&memory_mb) {
        return Err(AppError::BadRequest(format!(
            "memory_mb は {}〜{} にしてください",
            MEMORY_MB_RANGE.start(),
            MEMORY_MB_RANGE.end()
        )));
    }
    // visibility:明示指定 > port からの推導(§0-B。8080 → company / それ以外 → private)。
    let visibility = match req.visibility.as_deref() {
        Some(s) => Visibility::parse(s).ok_or_else(|| {
            AppError::BadRequest(
                "visibility は private / company / public のいずれかにしてください".into(),
            )
        })?,
        None => default_visibility(container_port),
    };
    let stateful = req.stateful.unwrap_or(false);

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

    let new = NewService {
        display_name: &display_name,
        deploy_key_enc: &deploy_key_enc,
        container_port,
        visibility,
        stateful,
        memory_mb,
    };

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
        match insert_attempt(&state.db, &state.config, auth.user_id, &candidate, &new).await {
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
        json!({
            "display_name": display_name,
            "subdomain": dto.subdomain,
            "container_port": container_port,
            "visibility": visibility.as_str(),
            "stateful": stateful,
            "memory_mb": memory_mb,
        }),
        auth.client_ip.as_deref(),
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
        runner: workflow::runner_for(&platforms).to_string(),
        platforms,
        workflow_yaml: workflow::TEMPLATE.to_string(),
        setup_commands,
    }))
}

/// create で確定済みの値(検証 + 既定解決済み)。insert_attempt へまとめて渡す
/// (subdomain だけはリトライごとに変わるので別引数)。
struct NewService<'a> {
    display_name: &'a str,
    deploy_key_enc: &'a [u8],
    container_port: i32,
    visibility: Visibility,
    stateful: bool,
    memory_mb: i32,
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
    subdomain: &str,
    new: &NewService<'_>,
) -> Result<ServiceDto, InsertErr> {
    let display_name = new.display_name;
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
        "INSERT INTO service_details
                (resource_id, subdomain, deploy_key_enc, container_port, visibility, stateful, memory_mb)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(id)
    .bind(subdomain)
    .bind(new.deploy_key_enc)
    .bind(new.container_port)
    .bind(new.visibility.as_str())
    .bind(new.stateful)
    .bind(new.memory_mb)
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
        container_port: new.container_port,
        image_digest: None,
        last_deploy_at: None,
        url: config.service_url(subdomain),
        visibility: new.visibility.as_str().into(),
        stateful: new.stateful,
        memory_mb: new.memory_mb,
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
    fn default_service_env_var_derives_from_subdomain() {
        // 典型:ハイフン → アンダースコア + 大文字 + _URL。
        assert_eq!(default_service_env_var("api-backend"), "API_BACKEND_URL");
        assert_eq!(default_service_env_var("web"), "WEB_URL");
        // 乱数語付き subdomain(<service>-<word>)も DNS 安全文字のみ = 全部通る。
        assert_eq!(default_service_env_var("shop-x7k2"), "SHOP_X7K2_URL");
        // 先頭が数字なら `_` 前置(env 名として安全)。subdomain 生成は基本数字始まりにしないが防御的に。
        assert_eq!(default_service_env_var("9to5"), "_9TO5_URL");
        // 返り値は必ず validate_env_key を通る(空 / '=' / 制御文字なし)。
        for s in ["api-backend", "web", "shop-x7k2", "9to5"] {
            assert!(validate_env_key(&default_service_env_var(s)).is_ok());
        }
    }

    #[test]
    fn default_visibility_derives_from_port() {
        // 8080(平台の HTTP 契約港)= 従来どおり company。
        assert_eq!(default_visibility(8080), Visibility::Company);
        // それ以外(自帯 DB 等の非 HTTP ソフト想定)= private。
        assert_eq!(default_visibility(5432), Visibility::Private);
        assert_eq!(default_visibility(6379), Visibility::Private);
        assert_eq!(default_visibility(3000), Visibility::Private);
        assert_eq!(default_visibility(1), Visibility::Private);
        assert_eq!(default_visibility(65535), Visibility::Private);
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

    #[test]
    fn public_db_value_detection() {
        let host = "db.tsubomi-app.com";
        let pub_url = "postgres://u:p@db.tsubomi-app.com:6432/app?sslmode=verify-full";
        let pub_url_alt = "postgresql://u:p@db.tsubomi-app.com:6432/app";
        // 公開機能 on + Postgres URI + 値がホストを含む → 検知(postgres:// と postgresql:// 両形)。
        assert!(value_points_at_public_db(true, host, pub_url));
        assert!(value_points_at_public_db(true, host, pub_url_alt));
        // 公開機能 off(CF Tunnel 等、公開入口なし)→ footgun なし、黙る。
        assert!(!value_points_at_public_db(false, host, pub_url));
        // 内部入口は別ホスト = 注入の正しい値 → 検知しない。
        let internal = "postgres://u:p@tsubomi-pgbouncer:6432/app?sslmode=require";
        assert!(!value_points_at_public_db(true, host, internal));
        // ホスト未設定(空)→ 何にもマッチさせない。
        assert!(!value_points_at_public_db(true, "", pub_url));
        // Postgres URI でない値はホストを含んでも拾わない(dev 127.0.0.1 の誤検知抑制)。
        assert!(!value_points_at_public_db(
            true,
            "127.0.0.1",
            "http://127.0.0.1:3000"
        ));
    }

    #[test]
    fn env_key_rejects_control_chars() {
        assert!(validate_env_key("DATABASE_URL").is_ok());
        assert!(validate_env_key("").is_err()); // 空
        assert!(validate_env_key("A=B").is_err()); // '='
        assert!(validate_env_key("A\0B").is_err()); // NUL
        assert!(validate_env_key("A\x1b[31mB").is_err()); // ANSI エスケープ
        assert!(validate_env_key("A\nB").is_err()); // 改行
    }

    /// URL 形の値だけパスワード部を伏せる(それ以外は原文)。
    #[test]
    fn mask_url_password_cases() {
        assert_eq!(
            super::mask_url_password("postgres://app:secret@pgb:6432/db?sslmode=require"),
            "postgres://app:***@pgb:6432/db?sslmode=require"
        );
        assert_eq!(
            super::mask_url_password("redis://c_ab:pw@tsubomi-valkey:6379"),
            "redis://c_ab:***@tsubomi-valkey:6379"
        );
        // パスワード無し / URL でない / userinfo 無しは原文のまま。
        assert_eq!(
            super::mask_url_password("http://api-backend:8080"),
            "http://api-backend:8080"
        );
        assert_eq!(super::mask_url_password("/data"), "/data");
        assert_eq!(super::mask_url_password("c_ab12:"), "c_ab12:");
    }
}
