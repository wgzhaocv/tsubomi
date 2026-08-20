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
pub mod source;
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
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use chrono::{DateTime, Utc};
use serde_json::json;
use futures_util::FutureExt;
use sqlx::PgPool;
use std::panic::AssertUnwindSafe;
use tsubomi_shared::{
    CreateInjectionReq, CreateServiceReq, CreateServiceResp, DeployConfig, DeployDto, ExecReq,
    ExecResult, InjectionDto, LogsResp, ResolvedEnvDto, RollbackReq, ServiceDto,
    SetServiceVisibilityReq, SetEnvReq, SetEnvResp,
};
use uuid::Uuid;

const MAX_NAME_LEN: usize = 64;
/// subdomain 生成の予約語(プラットフォーム / インフラのホスト名と衝突させない)。
/// `db` / `cache` は公開 DB / 公開 cache の入口名(`db.<domain>` = pgbouncer 証書の公開名、
/// `cache.<domain>`)— 取られると traefik の Host router がそれらの个別 DNS と衝突し得る。
const RESERVED_SUBDOMAINS: &[&str] = &["paas", "registry", "traefik", "www", "api", "db", "cache"];
/// subdomain の最大長(slugify の切り詰めと同じ値。DNS ラベル上限 63 より短い節度)。
const MAX_SUBDOMAIN_LEN: usize = 50;

/// subdomain が予約済みか。固定語に加えて **`tsubomi-` 前綴**も予約 — subdomain は M6 リンクで
/// per-service 私網の docker 網別名になるため、私網に同居する infra / app コンテナ名
/// (`tsubomi-pgbouncer` / `tsubomi-valkey` / `tsubomi-<uuid>`)と docker DNS で衝突させない。
fn reserved_subdomain(s: &str) -> bool {
    RESERVED_SUBDOMAINS.contains(&s) || s.starts_with("tsubomi-")
}

/// 起動時の残余暗穴チェック:`tsubomi-` 前綴の予約は**新規のみ**を塞ぐ — 旧規則で通った
/// 既存 subdomain が残っていれば、その M6 別名は今後も私網で infra / app コンテナ名と
/// DNS 衝突し得るので warn で可視化する(自動改名はしない — 判断は人間。
/// `log_orphan_tenant_dbs` と同型の起動時一回)。
pub async fn warn_reserved_subdomains(state: &AppState) {
    let rows: Vec<(String,)> = match sqlx::query_as(
        "SELECT s.subdomain FROM service_details s
           JOIN resources r ON r.id = s.resource_id
          WHERE r.deleted_at IS NULL AND s.subdomain LIKE 'tsubomi-%'",
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = ?e, "予約 subdomain の残余チェックに失敗");
            return;
        }
    };
    for (sub,) in rows {
        tracing::warn!(
            subdomain = %sub,
            "既存 service の subdomain が予約前綴 `tsubomi-` に該当します(私網の infra/app コンテナ名と DNS 衝突し得る)。`tbm service subdomain` での改名を推奨"
        );
    }
}

/// subdomain 409 の文言(create の明示指定 / 変更端点で共用)。subdomain の UNIQUE は
/// display_name と違い**ゴミ箱内も占有**する(表級 UNIQUE のまま — 公開 URL の同一性は
/// 復元で戻るべきもの)ので、一覧に見えない占有者への次の一手を含める。
fn subdomain_taken_msg(sub: &str) -> String {
    format!(
        "subdomain '{sub}' は既に使われています(ゴミ箱内の service も占有します — `tbm trash list` で確認)。別の名前を指定してください"
    )
}

/// ユーザ明示指定の subdomain の検証(create / 変更端点で共用)。規則は slugify の出力形と
/// 一致させる:小文字英数と `-`・英字始まり・`-` 終わり禁止・50 字以内。予約語も弾く。
/// この集合は `route::ensure_yaml_embeddable` の許可リストの部分集合(YAML 埋め込み安全)。
fn validate_subdomain(s: &str) -> AppResult<()> {
    let rule = format!(
        "subdomain は小文字英数と '-'(英字始まり・'-' 終わり不可)で {MAX_SUBDOMAIN_LEN} 文字以内にしてください"
    );
    let ok = !s.is_empty()
        && s.chars().count() <= MAX_SUBDOMAIN_LEN
        && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && s.starts_with(|c: char| c.is_ascii_lowercase())
        && !s.ends_with('-');
    if !ok {
        return Err(AppError::BadRequest(format!("{rule}: {s:?}")));
    }
    if reserved_subdomain(s) {
        return Err(AppError::BadRequest(format!(
            "subdomain '{s}' は予約されています(プラットフォーム / インフラ名と衝突)。別の名前にしてください"
        )));
    }
    Ok(())
}
/// deploy_key の乱数バイト数(base64url で ≈43 字)。HMAC の鍵そのもの。
const DEPLOY_KEY_BYTES: usize = 32;
/// プラットフォームの HTTP 契約港(PORT env の既定 = workflow / traefik の想定)。visibility 推導の基準。
/// INSERT が常に列を明示するので実効真源はこの定数 — DDL の DEFAULT 8080 と一致させること。
const DEFAULT_CONTAINER_PORT: i32 = 8080;
const CONTAINER_PORT_RANGE: std::ops::RangeInclusive<i32> = 1..=65535;
/// メモリ上限の既定 / 範囲(MiB)。既定 **1024** = migration 20260620 が OOM 対策で
/// 512→1024 へ引き上げた DDL DEFAULT と一致させる(512 に戻すと是正の逆行)。
/// 下限は最小級の app、上限は 16GB 共有ホストの節度。
///
/// CPU の上界(下)はホストの事実に置き換えたのに、こちらが固定値のままなのは意図的:
/// 4096 は「共有ホストで 1 app が取り過ぎない」という**方針値**で、物理量ではない。
/// docker は物理メモリ超えの `--memory` を(コア数超えの NanoCPUs と違って)拒否しないので、
/// 「入口は通るのにデプロイで失敗する」という CPU 側の病がそもそも起きない。
const DEFAULT_MEMORY_MB: i32 = 1024;
const MEMORY_MB_RANGE: std::ops::RangeInclusive<i32> = 128..=4096;
/// CPU 上限の下限(millicores)。100 = 0.1 CPU(それ未満は実用にならない)。
/// **上界は固定値ではなくホストのコア数**(`AppState::host_cores`)— docker daemon は
/// コア数を超える NanoCPUs でコンテナ作成そのものを拒否する。以前はここを 16000 固定にし
/// 「超過は docker がエラーにするだけ」としていたが、上限の変更は**次のデプロイから反映**
/// されるので、その拒否は設定操作から遠く離れた時点で「デプロイ失敗」として現れる
/// (原因に辿り着けない)。入口で弾くのが正しい。コア数が取れない環境では下の
/// フォールバックを使う(緩めるだけ = 従来の挙動)。
const CPU_LIMIT_MILLIS_MIN: i32 = 100;
const CPU_LIMIT_MILLIS_MAX_FALLBACK: i32 = 16000;

/// このホストで許される CPU 上限の上界(millicores)。コア数不明なら従来の固定値。
fn cpu_limit_millis_max(state: &AppState) -> i32 {
    state
        .host_cores
        .map_or(CPU_LIMIT_MILLIS_MAX_FALLBACK, |n| n.saturating_mul(1000))
}

/// 公開範囲(`service_details.visibility`)。DB の CHECK と対を成す単一真源 —
/// API 入力検証(不正値は 400)と route 分岐(ipallow 有無)をここに集約する。
/// 意味論は公開範囲設計 §0:private = route ファイルを書かない(インターネット不可視・subdomain 温存)、
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
/// 以後 port と visibility は独立)。8080 = プラットフォームの HTTP 契約港 → 従来どおり company。それ以外 =
/// 非 HTTP ソフト(持ち込み DB 等)の想定 → private(traefik は HTTP しか話せないので route が
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
        .route(
            "/services/{id}",
            get(get_one).patch(rename).delete(delete_service),
        )
        .route("/services/{id}/start", post(start))
        .route("/services/{id}/stop", post(stop))
        .route("/services/{id}/logs", get(logs))
        .route("/services/{id}/logs/stream", get(logs_stream))
        .route("/services/{id}/metrics", get(metrics))
        .route("/services/{id}/stats", get(crate::stats::stats))
        .route("/services/{id}/probe", get(probe))
        .route("/services/{id}/exec", post(exec))
        .route("/services/{id}/terminal", get(terminal))
        .route("/services/{id}/rollback", post(rollback))
        .route("/services/{id}/visibility", post(set_visibility))
        .route("/services/{id}/subdomain", post(set_subdomain))
        .route("/services/{id}/limits", post(set_limits))
        .route("/services/{id}/stateful", post(set_stateful))
        .route("/services/{id}/deploys", get(deploys))
        .route("/services/{id}/deploy-config", get(deploy_config))
        .route("/services/{id}/deploy-source", post(source::deploy_source))
        .route("/services/{id}/callers", get(list_callers))
        .route(
            "/services/{id}/redeploy-callers",
            post(redeploy_callers),
        )
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
    Option<i32>,           // cpu_limit_millis
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
        cpu_limit_millis: r.13,
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
                s.visibility, s.stateful, s.memory_mb, s.cpu_limit_millis
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
    fetch_service_dto(&state, auth.user_id, id).await.map(Json)
}

/// 自分の service 1 件の DTO を引く(get_one / set_subdomain の冪等応答で共用)。
async fn fetch_service_dto(state: &AppState, user_id: Uuid, id: Uuid) -> AppResult<ServiceDto> {
    let row: Option<ServiceRow> = sqlx::query_as(
        "SELECT r.id, r.display_name, r.anon_seq, r.created_at,
                s.subdomain, s.phase, s.desired_state, s.container_port, s.image_digest, s.last_deploy_at,
                s.visibility, s.stateful, s.memory_mb, s.cpu_limit_millis
           FROM resources r JOIN service_details s ON s.resource_id = r.id
          WHERE r.id = $1 AND r.user_id = $2 AND r.kind = 'service' AND r.deleted_at IS NULL",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?;
    row.map(|r| service_row_to_dto(r, &state.config))
        .ok_or(AppError::NotFound)
}

/// `PATCH /api/services/:id`:表示名のリネーム。**subdomain はここでは動かない** — 公開 URL・
/// GitHub repo 名・registry repo・route ファイルはすべて subdomain / id に紐づくので
/// 何も動かない(db rename の「接続文字列は変えない」と同型)。display_name は
/// 表示と名前→id 解決にだけ効く。同名衝突は稼働中の部分ユニークが 409 に落とす。
/// subdomain 自体を変えたいときは別端点 `POST /:id/subdomain`(set_subdomain)。
pub async fn rename(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<tsubomi_shared::RenameServiceReq>,
) -> AppResult<Json<ServiceDto>> {
    let display_name = validate::name(&req.name, MAX_NAME_LEN)?;

    // db/cache rename と同型:UPDATE…FROM…RETURNING の 1 文で全量 DTO 行まで取る
    // (subdomain/url が不変なことが応答で見える)。
    let row: Option<ServiceRow> = sqlx::query_as(
        "UPDATE resources r SET display_name = $1
           FROM service_details s
          WHERE r.id = $2 AND r.user_id = $3 AND r.kind = 'service' AND r.deleted_at IS NULL
            AND s.resource_id = r.id
      RETURNING r.id, r.display_name, r.anon_seq, r.created_at,
                s.subdomain, s.phase, s.desired_state, s.container_port, s.image_digest, s.last_deploy_at,
                s.visibility, s.stateful, s.memory_mb, s.cpu_limit_millis",
    )
    .bind(&display_name)
    .bind(id)
    .bind(auth.user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| {
        map_unique(
            e,
            format!("サービス名 '{display_name}' は既に使われています"),
        )
    })?;
    let row = row.ok_or(AppError::NotFound)?;

    audit(
        &state.db,
        Some(auth.user_id),
        "service.rename",
        id,
        json!({ "display_name": display_name }),
        auth.client_ip.as_deref(),
    )
    .await;
    Ok(Json(service_row_to_dto(row, &state.config)))
}

/// memory_mb の範囲検証(create / limits 共有。定数・文言の単一真源)。
fn check_memory_mb(m: i32) -> AppResult<()> {
    if !MEMORY_MB_RANGE.contains(&m) {
        return Err(AppError::BadRequest(format!(
            "memory_mb は {}〜{} にしてください",
            MEMORY_MB_RANGE.start(),
            MEMORY_MB_RANGE.end()
        )));
    }
    Ok(())
}

/// cpu_limit_millis の範囲検証(create / limits 共有)。上界はホストのコア数由来
/// (`cpu_limit_millis_max`)なので、機体を移せば自動で追従する。
fn check_cpu_limit_millis(state: &AppState, cpu: i32) -> AppResult<()> {
    let max = cpu_limit_millis_max(state);
    if !(CPU_LIMIT_MILLIS_MIN..=max).contains(&cpu) {
        // 上界そのものが「このホストのコア数 × 1000」なので、コア数が分かるときは数字を繰り返さない。
        // 分からないときだけ、その上界が暫定値であることを言う。
        let note = if state.host_cores.is_none() {
            "(このホストのコア数は不明です)"
        } else {
            ""
        };
        // **両方の単位で言う**。この列の単位は millicores だが、入口の多くは `tbm service limits
        // --cpus` / web の入力欄で **コア数**を扱う。片方だけ書くと読み手が 1000 で割る作業を
        // 引き受けることになり、「次の一手を含める」に反する(f64 の {} は 8.0 を "8" と出す)。
        return Err(AppError::BadRequest(format!(
            "cpu_limit_millis は {CPU_LIMIT_MILLIS_MIN}〜{max}(millicores、1000 = 1 CPU)にしてください{note}。             コア数で指定する入口(`tbm service limits --cpus` / web)では {}〜{} です",
            CPU_LIMIT_MILLIS_MIN as f64 / 1000.0,
            max as f64 / 1000.0
        )));
    }
    Ok(())
}

/// 自分の service か確認する(他人 / 不在 / 削除済みは 404)。所有権ゲート。
pub(crate) async fn ensure_owned(state: &AppState, user_id: Uuid, id: Uuid) -> AppResult<()> {
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
    String,
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
        trigger: r.8,
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
        "SELECT id, git_sha, image_digest, status, error, created_at, finished_at, commit_message,
                trigger
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
        // **`finished_at` 順**(created_at ではない):deploy 行は deploy_lock を取る**前**に作られるので、
        // A→B の順に行ができても B→A の順に成功し得る。実際に serving しているのは「最後に成功した」
        // 方なので、行の作成順ではなく完了順で選ぶ(codex review 2026-07-26)。
        "SELECT image_digest, git_sha, commit_message FROM deploys
          WHERE service_id = $1 AND status = 'succeeded'
          ORDER BY finished_at DESC NULLS LAST, created_at DESC LIMIT 1",
    )
    .bind(service_id)
    .fetch_optional(&state.db)
    .await?)
}

/// 直近に成功した deploy の **id**。route が指すべきコンテナ名は `deploy::container_name(service_id, この id)`
/// で一意に決まる(start-first の命名規約)。reconcile の route ドリフト収束 / 中断デプロイ復旧が、
/// 「走っている任意のコンテナ」ではなく**このコンテナ**を正とするための真源(新旧併存時に route を旧へ巻き戻さない)。
pub(crate) async fn latest_succeeded_deploy_id(
    state: &AppState,
    service_id: Uuid,
) -> AppResult<Option<Uuid>> {
    Ok(latest_succeeded_deploy_ref(state, service_id)
        .await?
        .map(|(id, _)| id))
}

/// 直近に成功した deploy の **(id, 行の作成時刻)**。時刻は「そのコンテナの env が凍結された瞬間より
/// 前」を保証する下限として使う — 注入値は `inject::resolve`(deploy 中の starting 段階)で解決される
/// ので、行の作成(received 段階)は必ずそれより前。**`finished_at` を使ってはいけない**:
/// commit_success は readiness 探測(既定 60s)の後なので、「デプロイ中に注入した」ケースで
/// `created_at < finished_at` となり **未反映が反映済みに反転する**(この機能が最も要る場面で
/// 裏返る = 見逃し。simplify/codex review 2026-07-26)。過剰警告(pull 中の注入)側に倒す。
pub(crate) async fn latest_succeeded_deploy_ref(
    state: &AppState,
    service_id: Uuid,
) -> AppResult<Option<(Uuid, DateTime<Utc>)>> {
    Ok(sqlx::query_as(
        // 並びは `latest_succeeded_deploy` と同じ理由で `finished_at` 順。返す時刻は **行の作成時刻**
        // (env 凍結より必ず前 = 見逃さない側)。
        "SELECT id, created_at FROM deploys
          WHERE service_id = $1 AND status = 'succeeded'
          ORDER BY finished_at DESC NULLS LAST, created_at DESC LIMIT 1",
    )
    .bind(service_id)
    .fetch_optional(&state.db)
    .await?)
}

/// serving すべきコンテナ名 = **直近成功 deploy のコンテナ**(`container_name`)を DB から導く
/// (稼働確認はしない)。成功 deploy 無し = 未デプロイは None。
pub(crate) async fn expected_container_name(state: &AppState, id: Uuid) -> Option<String> {
    let deploy_id = match latest_succeeded_deploy_id(state, id).await {
        Ok(Some(d)) => d,
        Ok(None) => return None,
        Err(e) => {
            tracing::warn!(error = ?e, %id, "serving コンテナの解決:直近成功 deploy の取得に失敗");
            return None;
        }
    };
    Some(deploy::container_name(id, deploy_id))
}

/// serving すべきコンテナ名が今 `running_names` に居る(= 実際に走っている)時だけ Some。走っていない
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

/// `expected_running_container` の糖衣:docker から稼働中一覧を引いてから判定する。
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
    // trigger を行に焼く:これが無いと reconcile の復活と caller 再リンクが、部署履歴で
    // ユーザ自身の再デプロイと区別できない(同じ commit 件名の行が並ぶ)。
    let deploy_id: Uuid = sqlx::query_scalar(
        "INSERT INTO deploys (service_id, git_sha, image_digest, status, commit_message, trigger)
              VALUES ($1, $2, $3, 'received', $4, $5) RETURNING id",
    )
    .bind(service_id)
    .bind(git_sha)
    .bind(image_digest)
    .bind(commit_message)
    .bind(trigger.as_db())
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
    // 削除を実際に行った時だけプライベートネットワークを撤去する(コンテナは stop_containers で除去済み = 順序 OK)。
    // 競合で rows_affected==0 の側は先行 deleter が撤去済みなので触らない。restore は次 deploy の
    // ensure_service_network で再生成されるので restore 側は無改修。失敗は reconcile の孤児 GC が回収。
    if let Err(e) = network::remove_service_network(state, id).await {
        tracing::warn!(error = ?e, %id, "soft_delete: プライベートネットワークの撤去に失敗(reconcile が後で回収)");
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
/// public(ipallow 無し = 全網公開)も**本人裁量 + audit による事後追跡**で owner 限定にしない(§0-C)。
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
    // 文言直通の 503(UnavailableMsg)で「次の一手」を返す(AI が自己修正できる — CLI 契約。
    // 通常の 5xx は into_response が「内部エラー」に編校し文言が届かない)。生エラーは log のみ
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
            // serving 中(直近成功 deploy のコンテナが稼働)の時だけ route を書く。停止 / 未デプロイは
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

/// `POST /api/services/:id/subdomain`:subdomain(= 公開 URL)の変更。set_visibility と同じ
/// 「DB 先行 → 現実収束(route + M6 網別名)」の型。旧 URL は catch-all → 302 /noservice に
/// 自然落ち(凍結しない — 受容済み)。GitHub repo 名は旧 subdomain のまま(rename と同型)。
/// この service を注入している caller の `_URL`/`_HOST` はコンテナ起動時に解決済みの旧値 =
/// caller の再デプロイまで断線し得る。未反映は list_injections の needs_redeploy
/// (subdomain_changed_at 参加)が出す — だから同値変更では時刻を動かさない(偽の未反映を
/// 出さない)。ただし**収束段は同値でも再実行する**:前回の route/別名の反映失敗を、
/// 同じコマンドの再実行で回収できるようにするため(「再実行も可能」を嘘にしない)。
pub async fn set_subdomain(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<tsubomi_shared::SetServiceSubdomainReq>,
) -> AppResult<Json<ServiceDto>> {
    ensure_owned(&state, auth.user_id, id).await?; // 404 ゲート(lock 外・安価)
    validate_subdomain(&req.subdomain)?;

    // deploy / start / stop / visibility と同一 lock で直列化(route ファイル・網別名と
    // コンテナ状態の競合防止。deploy の spec 読取も lock 内なので旧値で走り抜ける心配はない)。
    let lock = state.deploy_lock(id);
    let _guard = lock.lock().await;

    // 現値(audit の from + 同値判定)。lock 待ち中に削除が完走したケースはここが 404 を返す。
    let current = fetch_service_dto(&state, auth.user_id, id).await?;
    let dto = if current.subdomain == req.subdomain {
        current // 同値:UPDATE・audit・subdomain_changed_at は動かさない(冪等)
    } else {
        // DB 先行(背骨:DB=期望状態)。UNIQUE 違反(他 service が使用中)は 409。
        let row: Option<ServiceRow> = sqlx::query_as(
            "UPDATE service_details s SET subdomain = $2, subdomain_changed_at = now()
               FROM resources r
              WHERE s.resource_id = $1 AND r.id = s.resource_id AND r.deleted_at IS NULL
          RETURNING r.id, r.display_name, r.anon_seq, r.created_at,
                    s.subdomain, s.phase, s.desired_state, s.container_port, s.image_digest, s.last_deploy_at,
                    s.visibility, s.stateful, s.memory_mb, s.cpu_limit_millis",
        )
        .bind(id)
        .bind(&req.subdomain)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| map_unique(e, subdomain_taken_msg(&req.subdomain)))?;
        let row = row.ok_or(AppError::NotFound)?;

        // 恒久的な状態変化(DB)の直後に監査 — 後段の収束が失敗しても監査は DB と一致する。
        audit(
            &state.db,
            Some(auth.user_id),
            "service.subdomain",
            id,
            json!({ "from": current.subdomain, "to": req.subdomain }),
            auth.client_ip.as_deref(),
        )
        .await;
        service_row_to_dto(row, &state.config)
    };

    // 現実収束(lock 内。同値の再実行でもここは走る)。失敗しても DB は更新済み = reconcile が
    // ≤30s で収束させる(route は host drift 判定・網別名は attach_callees の別名検査)。
    let converge_err = |e: AppError| {
        tracing::error!(error = ?e, %id, "subdomain 変更の反映に失敗");
        AppError::UnavailableMsg(
            "subdomain は保存しましたが反映に失敗しました。reconcile が 30 秒以内に収束させます(再実行も可能)".into(),
        )
    };
    if let Some(container) = serving_container(&state, id).await {
        // route:private は「ファイル無し」が期望状態なので何もしない。company/public は
        // 新 host で同一ファイル(svc-<id>.yml)を原子上書き — 旧 host の router は消える。
        if let Some(vis) = Visibility::parse(&dto.visibility)
            && vis != Visibility::Private
        {
            route::write(
                &state,
                id,
                &req.subdomain,
                &container,
                dto.container_port,
                vis.ipallow(),
            )
            .map_err(converge_err)?;
        }
        // M6 網別名の換血:この service を注入している全 caller 私網で、旧別名の endpoint を
        // 剥がして新別名で attach し直す(既に正しい網は触らない)。caller コンテナ内の解決済み
        // env は旧値のまま = caller 再デプロイまでの断線は仕様。needs_redeploy が可視化する。
        network::realias_as_callee(&state, id, &req.subdomain, &container).await;
    }

    Ok(Json(dto))
}
/// `POST /api/services/:id/limits`:memory / cpus 上限の変更。値は run_digest_inner が
/// デプロイのたびに DB から読み直すので、**次のデプロイから反映**(実行中コンテナには
/// 遡及しない — docker の memory / nano_cpus はコンテナ作成時パラメータ)。visibility と
/// 違い現実収束段が無い = DB 更新だけなので deploy_lock も不要(UPDATE は原子的で、
/// 進行中デプロイは自分が読んだ時点の値で完走する。それも仕様どおり「次から」)。
pub async fn set_limits(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<tsubomi_shared::SetServiceLimitsReq>,
) -> AppResult<Json<tsubomi_shared::ServiceLimitsDto>> {
    ensure_owned(&state, auth.user_id, id).await?;

    if req.memory_mb.is_none() && req.cpu_limit_millis.is_none() && !req.clear_cpu_limit {
        return Err(AppError::BadRequest(
            "変更する項目を指定してください(memory_mb / cpu_limit_millis / clear_cpu_limit)".into(),
        ));
    }
    if req.cpu_limit_millis.is_some() && req.clear_cpu_limit {
        return Err(AppError::BadRequest(
            "cpu_limit_millis と clear_cpu_limit は同時に指定できません".into(),
        ));
    }
    // 範囲検証は create と共有(check_* が定数・文言ごと単一真源)。
    if let Some(m) = req.memory_mb {
        check_memory_mb(m)?;
    }
    if let Some(cpu) = req.cpu_limit_millis {
        check_cpu_limit_millis(&state, cpu)?;
    }

    let row: Option<(i32, Option<i32>)> = sqlx::query_as(
        "UPDATE service_details s SET
                memory_mb        = COALESCE($2, s.memory_mb),
                cpu_limit_millis = CASE WHEN $4 THEN NULL
                                        ELSE COALESCE($3, s.cpu_limit_millis) END
           FROM resources r
          WHERE s.resource_id = $1 AND r.id = s.resource_id AND r.deleted_at IS NULL
      RETURNING s.memory_mb, s.cpu_limit_millis",
    )
    .bind(id)
    .bind(req.memory_mb)
    .bind(req.cpu_limit_millis)
    .bind(req.clear_cpu_limit)
    .fetch_optional(&state.db)
    .await?;
    let (memory_mb, cpu_limit_millis) = row.ok_or(AppError::NotFound)?;

    audit(
        &state.db,
        Some(auth.user_id),
        "service.limits",
        id,
        json!({ "memory_mb": memory_mb, "cpu_limit_millis": cpu_limit_millis }),
        auth.client_ip.as_deref(),
    )
    .await;
    // 変更後の確定値を返す(部分変更でも全量 — CLI/web が「今の姿」をそのまま出せる)。
    Ok(Json(tsubomi_shared::ServiceLimitsDto {
        memory_mb,
        cpu_limit_millis,
    }))
}

/// `POST /api/services/:id/stateful`:stateful を **false→true の単方向**で有効化する
/// (stateful 設計 §0-C / §10-D)。true→false は入口ごと作らない — stateless の swap
/// デプロイは新旧コンテナが同一データディレクトリを同時に開く方向で、既に貯めたデータを
/// 壊し得る。false→true は既存 workaround(DB を stateless で走らせてしまった)の救済で、
/// 次のデプロイから stop-first になるだけ(実行中のコンテナには遡及しない)。
/// 既に true なら冪等成功(触らない・audit も書かない)。
pub async fn set_stateful(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<StatusCode> {
    ensure_owned(&state, auth.user_id, id).await?;

    // deploy_lock で進行中デプロイと直列化する:swap デプロイの最中に flag が立つと
    // 「stateless 前提で新旧併走中なのにもう stateful のつもり」という取り違えを生む。
    // 待ってから立てれば「次のデプロイから stop-first」が正確に成立する。
    let lock = state.deploy_lock(id);
    let _guard = lock.lock().await;

    let changed = sqlx::query(
        "UPDATE service_details s SET stateful = true
           FROM resources r
          WHERE s.resource_id = $1 AND r.id = s.resource_id AND r.deleted_at IS NULL
            AND s.stateful = false",
    )
    .bind(id)
    .execute(&state.db)
    .await?
    .rows_affected();

    // 0 行 = 「既に true(冪等成功)」と「lock 待ち中に削除された」の両方があり得る。
    // 後者に 204 を返すと、restore 後も stateful=false のまま「有効化済み」と誤認させる
    // (codex 審査 2026-08-13)。生存確認で切り分ける。
    if changed == 0 {
        let alive: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM resources WHERE id = $1 AND deleted_at IS NULL)",
        )
        .bind(id)
        .fetch_one(&state.db)
        .await?;
        if !alive {
            return Err(AppError::NotFound);
        }
    }

    if changed > 0 {
        audit(
            &state.db,
            Some(auth.user_id),
            "service.stateful",
            id,
            json!({ "stateful": true }),
            auth.client_ip.as_deref(),
        )
        .await;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/services/:id/metrics`:稼働中コンテナの 1 発メトリクス(CPU / メモリ(上限比)/
/// 再起動回数 / uptime / OOM)。停止 / 未デプロイでも 200(running=false)。所有者のみ。
/// Bearer / session 両対応(logs / status と同層 = 自リソースの読み取り)。
pub async fn metrics(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<Json<tsubomi_shared::ServiceMetricsDto>> {
    ensure_owned(&state, auth.user_id, id).await?;
    Ok(Json(docker::service_metrics(&state, id).await))
}

/// `GET /api/services/:id/probe`:内部ネットワークへの単発 TCP 疎通確認(private service の verify 素材。
/// visibility 設計で verify を private 短絡にした際の残余 —「今この瞬間 listen しているか」を
/// 公開 URL 無しで確かめる入口が無かった)。metrics と同じ「不在も答え」型:停止 / 未デプロイ
/// でも 200(running=false)。判定の付加情報として is_callee(M6 リンクの被注入 = listen
/// していないのは異常)と container_port を併載し、解釈は CLI/呼び出し側に委ねる。
pub async fn probe(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<Json<tsubomi_shared::ServiceProbeDto>> {
    ensure_owned(&state, auth.user_id, id).await?;
    let container_port: i32 =
        sqlx::query_scalar("SELECT container_port FROM service_details WHERE resource_id = $1")
            .bind(id)
            .fetch_optional(&state.db)
            .await?
            .ok_or(AppError::NotFound)?;
    let is_callee = deploy::is_linked_callee(&state, id).await;

    let (running, listening) = match serving_container(&state, id).await {
        Some(name) => docker::probe_once(&state, &name, container_port).await,
        None => (false, None),
    };
    Ok(Json(tsubomi_shared::ServiceProbeDto {
        running,
        listening,
        is_callee,
        container_port,
    }))
}

/// `?tail=N&since=TS`(since = unix 秒。スナップショット / 流式で共用)。
#[derive(serde::Deserialize)]
pub struct LogsQuery {
    tail: Option<usize>,
    since: Option<i64>,
}

/// `GET /api/services/:id/logs?tail=N&since=TS`:走っているコンテナの直近ログ(stdout+stderr)。
pub async fn logs(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<LogsQuery>,
) -> AppResult<Json<LogsResp>> {
    ensure_owned(&state, auth.user_id, id).await?;
    let logs = docker::logs(&state, id, q.tail, q.since).await?;
    Ok(Json(LogsResp { logs }))
}

/// `GET /api/services/:id/logs/stream?tail=N&since=TS`:ログを follow で流す(chunked、
/// text/plain)。Bearer / session 両対応 — CSWSH は cookie 自動付与の問題なので、terminal と
/// 違い `is_session` は要求しない(exec と同じ CLI 主用途の判断。§terminal 設計)。
/// 半開き接続の打ち切りは docker.rs 側(LOG_STREAM_MAX)が担保済み。CF Tunnel は
/// さらに手前の無音 ~100s で切ることがある — 再接続(since 引き継ぎ)は CLI 側の責務。
pub async fn logs_stream(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<LogsQuery>,
) -> AppResult<impl axum::response::IntoResponse> {
    ensure_owned(&state, auth.user_id, id).await?;
    let stream = docker::logs_stream(&state, id, q.tail, q.since).await?;
    let body = axum::body::Body::from_stream(stream);
    Ok((
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            // 動的な私有データ:中間キャッシュ禁止。nosniff は text/plain の誤 sniff 防止。
            (header::CACHE_CONTROL, "no-store, private"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        body,
    ))
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
/// 線上診断用。対話シェルは web ターミナル)。所有者の自リソースのみ(`ensure_owned`)= 既存の
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
/// 所有者の自リソースのみ(`ensure_owned`)= web SQL と同一ティアの暴露。アップグレード前にコンテナ稼働中を
/// 確認し、双方向ポンプは `docker::handle_terminal`(地雷はそちらのコメント)。
pub async fn terminal(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: axum::http::HeaderMap,
    ws: WebSocketUpgrade,
) -> AppResult<impl IntoResponse> {
    // CSWSH 対策:アップグレードの Origin を管制面オリジンに固定する(SameSite=Lax は same-site の
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
    // deploy-source の取得前に失敗した行は digest がプレースホルダ('pending')のまま =
    // 内部 registry に実体が無い。rollback 先にすると pull が誤解を招くエラー(manifest
    // unknown → 「再 push してください」)になるので、ここで明確に弾く。
    if !deploy::is_sha256_digest(&digest) {
        return Err(AppError::BadRequest(
            "このデプロイはイメージ取得前に失敗しています(digest 未確定)。`tbm service deploys` で別のデプロイを指定してください".into(),
        ));
    }
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

/// 注入一覧の行(id, resource_id, kind, display_name, env_var, mount_path, valid, created_at)。
type InjectionRow = (
    Uuid,
    Uuid,
    String,
    String,
    String,
    Option<String>,
    bool,
    DateTime<Utc>,
);

/// 今 serving しているコンテナの env が凍結された時刻の**下限**。走っていなければ None。
/// これより後に作られた注入は**まだ効いていない**(値は起動の瞬間に解決される — 決定 #5)。
/// SQL 1 本 + docker 1 回(`latest_succeeded_deploy_ref` が id と時刻を同時に返すので、
/// 「同じ行を 2 度引く」も「2 本の間に deploy が commit して食い違う」も起きない)。
async fn serving_since(state: &AppState, id: Uuid) -> Option<DateTime<Utc>> {
    // SQL を先に引く — 未デプロイなら docker を撃たない(serving_container と同じ順序)。
    let (deploy_id, since) = match latest_succeeded_deploy_ref(state, id).await {
        Ok(v) => v?,
        Err(e) => {
            // 判定不能を黙って「反映済み」に倒すと警告が消えるので、痕跡は残す
            // (`expected_container_name` と同じ扱い)。
            tracing::warn!(error = ?e, %id, "注入の未反映判定:直近成功 deploy の取得に失敗");
            return None;
        }
    };
    // RESTARTING も「その env を握って生きている」に含める(`live_names` の doc 参照)。
    let live = docker::live_names(state, id).await.ok()?;
    live.contains(&deploy::container_name(id, deploy_id))
        .then_some(since)
}

fn injection_row_to_dto(r: InjectionRow, serving_since: Option<DateTime<Utc>>) -> InjectionDto {
    InjectionDto {
        id: r.0,
        resource_id: r.1,
        resource_kind: r.2,
        resource_name: r.3,
        env_var: r.4,
        mount_path: r.5,
        valid: r.6,
        warning: None, // 一覧では出さない(作成時のみの注意喚起)
        // 走っているコンテナより後に作られた注入 = 未反映。停止中(None)なら反映すべき相手が
        // 居ないので false。既存行の created_at は epoch(20260726000002)なので誤報しない。
        needs_redeploy: serving_since.is_some_and(|since| r.7 > since),
    }
}

/// 連帯再デプロイ(caller 再リンク)の対象判定。**`GET /callers` のプレビューと
/// `POST /redeploy-callers` の実行が同じ関数を引く**単一真源 — 別々に書くとプレビューが嘘になる。
/// 入力は純データ(DB / docker を触らない)なので真理値表で機械封じできる。
/// Err の文言はそのまま `skip_reason` として web / CLI に出るので**次の一手を含める**。
///
/// 順序は「より根本的な理由を先に」— 未デプロイの停止中 service には「まだデプロイされていない」
/// を出す(「停止中」より情報量が多い)。
///
/// **これは唯一の防壁ではない**:停止済みを起こさない規則は `deploy::run_digest` のロック後
/// 再確認門(非 user 契機)にも在り、そちらが最終防壁(プレビューと実行の間に stop が
/// 割り込むケースを拾えるのはロックの中だけ)。ここは「名単の見え方」を決める側。
fn caller_relink_verdict(r: &inject::CallerRow, callee_serving: bool) -> Result<(), &'static str> {
    // callee 自身が serving していないと `set_subdomain` の realias 段がそもそも走らない
    // (`serving_container` ガード)。網に新別名が無い状態で caller を回すのは純 churn。
    if !callee_serving {
        return Err("このサービス自身が稼働していないため対象外(起動してから再実行してください)");
    }
    if !r.deployed {
        return Err("まだデプロイされていないため対象外(先に一度デプロイしてください)");
    }
    // 停止中を起こさないのは**仕様**:`commit_success` が desired_state を running に戻すので、
    // ここで弾かないと「ユーザが止めた意図」を改名の副作用で消してしまう。
    if r.desired_state != "running" {
        return Err("停止中のため対象外(自動では起こしません。起動すると新しい値が入ります)");
    }
    // 進行中の deploy にキューで積むと、こちらが解決した digest が陳腐化し得るし swap も重なる。
    // なお改名**前**に starting へ入っていた caller は旧値で env が凍結され得る = 未反映バッジが
    // 立つので、完了後の再実行で回収できる。
    if r.deploy_in_flight {
        return Err("デプロイ進行中のため対象外(完了後にもう一度実行してください)");
    }
    // stateful は stop-first(実停機を伴う)。データを持つ service を止める時機はユーザが選ぶ。
    if r.stateful {
        return Err(
            "データを持つ(stateful)ため自動対象外(停止を伴うので手動で再デプロイしてください)",
        );
    }
    Ok(())
}

fn caller_row_to_dto(
    r: inject::CallerRow,
    callee_serving: bool,
) -> tsubomi_shared::ServiceCallerDto {
    let verdict = caller_relink_verdict(&r, callee_serving);
    tsubomi_shared::ServiceCallerDto {
        id: r.id,
        display_name: r.display_name,
        env_vars: r.env_vars,
        desired_state: r.desired_state,
        last_deploy_status: r.last_deploy_status,
        last_deploy_error: r.last_deploy_error,
        stateful: r.stateful,
        will_redeploy: verdict.is_ok(),
        skip_reason: verdict.err().map(str::to_owned),
    }
}

/// 名単 + 判定を作る(GET のプレビューと POST の実行計画が共有)。callee 自身が serving して
/// いるかは 1 度だけ解決する(caller が居なければ docker を叩かない)。
async fn caller_plan(
    state: &AppState,
    callee_id: Uuid,
) -> AppResult<Vec<tsubomi_shared::ServiceCallerDto>> {
    let rows = inject::service_caller_rows(state, callee_id).await?;
    let callee_serving = !rows.is_empty() && serving_container(state, callee_id).await.is_some();
    Ok(rows
        .into_iter()
        .map(|r| caller_row_to_dto(r, callee_serving))
        .collect())
}

/// `GET /api/services/:id/callers`:**この service を注入している別の service** の一覧。
/// 改名(`set_subdomain`)の影響範囲を出す入口 — 改名した瞬間、caller のコンテナ内に凍結された
/// `_URL`/`_HOST` は旧 subdomain のままなので内部リンクが切れる。逆引きの述語は
/// `inject::service_caller_rows` = 網操作(realias)と同一なので、「名単に出た集合」と
/// 「実際に触られる集合」がドリフトしない。連帯再デプロイの **dry-run** も兼ねる
/// (`will_redeploy` / `skip_reason` は `POST /redeploy-callers` と同じ純関数の出力)。
pub async fn list_callers(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Vec<tsubomi_shared::ServiceCallerDto>>> {
    ensure_owned(&state, auth.user_id, id).await?;
    Ok(Json(caller_plan(&state, id).await?))
}

/// `POST /api/services/:id/redeploy-callers`:この service を注入している呼び出し側を
/// **今の版のまま**再デプロイし、注入値(`_URL`/`_HOST`)を新しい subdomain へ追従させる。
///
/// 背骨は変えない — 値はコンテナ起動の瞬間に解決される。変えるのは「その再デプロイを誰が
/// 押すか」だけなので、**opt-in の一発**(静默の自動連鎖にはしない)。
///
/// **202 即返し**:N 件の deploy は分単位になり得る(CF Tunnel の ~100s 切断対策 = deploy-source
/// と同型)。応答は要求時点のスナップショットで約束ではない — 実行の直前に判定を取り直す。
///
/// 改名と**独立に再実行できる**(web の 2 リクエストが半完成したときの再試行 / 後から思い出して
/// 実行するケース)。
pub async fn redeploy_callers(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<(StatusCode, Json<tsubomi_shared::RedeployCallersResp>)> {
    ensure_owned(&state, auth.user_id, id).await?;

    // 入場制限は**実行枠そのもの**で行う(ハンドラで試す)。取れないまま 202 を返すと
    // 「開始しました」が嘘になる(枠待ちで何も始まっていない)。guard は spawn へ move し
    // Drop で解放(panic 経路も拾う)。`deploy_lock` は流用しない — fan-out は分単位で、
    // 同じ錠を取る stop / delete / visibility / 改名がその間固まるため。
    let Ok(slot) = state.relink_slot.clone().try_lock_owned() else {
        return Err(AppError::Conflict(
            "連帯再デプロイが進行中です(この platform では同時に 1 バッチ)。完了を待ってから再実行してください".into(),
        ));
    };

    let planned = caller_plan(&state, id).await?;
    let targets: Vec<Uuid> = planned
        .iter()
        .filter(|c| c.will_redeploy)
        .map(|c| c.id)
        .collect();

    // 対象ゼロなら **spawn しない**:何もしない task が枠を占め、その間この callee への
    // 再実行が 409 になり、空の完走 audit まで残る(審査指摘)。
    if targets.is_empty() {
        return Ok((
            StatusCode::ACCEPTED,
            Json(tsubomi_shared::RedeployCallersResp { planned }),
        ));
    }

    // 永続的な意図を先に監査(後段の spawn が失敗しても記録は残る)。
    audit(
        &state.db,
        Some(auth.user_id),
        "service.redeploy_callers",
        id,
        json!({ "targets": targets }),
        auth.client_ip.as_deref(),
    )
    .await;

    let state2 = state.clone();
    let targets2 = targets.clone();
    tokio::spawn(async move {
        let _slot = slot; // task の生存期間だけ枠を保持する(Drop で解放)
        // `CatchPanicLayer` はハンドラだけを守り、spawn した task は守らない(source.rs と同型)。
        let outcome =
            AssertUnwindSafe(relink_callers(&state2, id, &targets2, auth.user_id)).catch_unwind();
        if outcome.await.is_err() {
            tracing::error!(callee_id = %id, "連帯再デプロイの task が panic");
        }
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(tsubomi_shared::RedeployCallersResp { planned }),
    ))
}

/// caller 群を**直列**に再デプロイする(spawn の中身)。実行枠(`relink_slot`)はハンドラが
/// 取得して move してきている = プロセス全体で同時 1 バッチ。バッチ内も逐次(reconcile と
/// 同じ家風)— 単一ホストの共有機なので並行度をクリック回数に比例させない。
///
/// 1 件の失敗は他を止めない(`continue` + warn)。判定は**実行の直前に取り直す**(プレビューと
/// 実行の間に stop / 削除 / 新デプロイが割り込み得る)。最終防壁は `run_digest` のロック後
/// 再確認門なので、ここで漏れても停止済み service は起きない。
async fn relink_callers(state: &AppState, callee_id: Uuid, targets: &[Uuid], actor: Uuid) {
    let mut results = Vec::with_capacity(targets.len());
    for &caller_id in targets {
        // 実行直前の再判定(同じ純関数)。callee 側の serving も含めて取り直す。
        match caller_plan(state, callee_id).await {
            Ok(plan) => match plan.iter().find(|c| c.id == caller_id) {
                Some(c) if c.will_redeploy => {}
                Some(c) => {
                    let why = c.skip_reason.clone().unwrap_or_default();
                    tracing::info!(%caller_id, why = %why, "連帯再デプロイ: 実行直前に対象外へ変化 — スキップ");
                    results.push(json!({ "id": caller_id, "result": "skipped", "reason": why }));
                    continue;
                }
                None => {
                    tracing::info!(%caller_id, "連帯再デプロイ: 呼び出し側でなくなった — スキップ");
                    results.push(json!({ "id": caller_id, "result": "skipped", "reason": "eject" }));
                    continue;
                }
            },
            Err(e) => {
                tracing::warn!(error = ?e, %caller_id, "連帯再デプロイ: 再判定に失敗 — スキップ");
                results.push(json!({ "id": caller_id, "result": "skipped", "reason": "recheck_failed" }));
                continue;
            }
        }
        // digest は caller ごとに**この瞬間**解決する(バッチ先頭でスナップショットすると、
        // その間に自分で新版をデプロイした caller を旧版へ巻き戻す — 設計時審査 P0-4)。
        // ロック待ちの間に陳腐化する残余は run_digest の no-downgrade 門が精密に塞ぐ。
        let latest = match latest_succeeded_deploy(state, caller_id).await {
            Ok(Some(v)) => v,
            Ok(None) => {
                results.push(json!({ "id": caller_id, "result": "skipped", "reason": "no_deploy" }));
                continue;
            }
            Err(e) => {
                tracing::warn!(error = ?e, %caller_id, "連帯再デプロイ: 直近成功 deploy の取得に失敗");
                results.push(json!({ "id": caller_id, "result": "failed", "reason": "digest_lookup" }));
                continue;
            }
        };
        let (digest, git_sha, msg) = latest;
        match redeploy(
            state,
            caller_id,
            &digest,
            &git_sha,
            msg.as_deref(),
            deploy::DeployTrigger::CallerRelink,
        )
        .await
        {
            Ok(()) => {
                tracing::info!(%caller_id, %callee_id, "連帯再デプロイ: 完了");
                results.push(json!({ "id": caller_id, "result": "ok" }));
            }
            Err(e) => {
                // 失敗しても phase は落ちない(CallerRelink 契機)。start-first なので旧コンテナは
                // 無傷 = この caller は元の版で走り続ける。記録は deploys 行に残る。
                tracing::warn!(error = ?e, %caller_id, %callee_id, "連帯再デプロイ: 失敗(旧版で継続)");
                results.push(json!({
                    "id": caller_id, "result": "failed",
                    "reason": e.to_string().chars().take(200).collect::<String>()
                }));
            }
        }
    }
    // 完走の記録。owner の追跡用(一般ユーザ向けの結果表示は `GET /callers` の last_deploy_status)。
    audit(
        &state.db,
        Some(actor),
        "service.redeploy_callers.completed",
        callee_id,
        json!({ "results": results }),
        None,
    )
    .await;
}

/// `GET /api/services/:id/injections`:注入一覧(失効 = valid:false も含む)。
pub async fn list_injections(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Vec<InjectionDto>>> {
    ensure_owned(&state, auth.user_id, id).await?;
    let rows: Vec<InjectionRow> = sqlx::query_as(
        // 「注入値が今のコンテナと違う」時刻 = 注入の作成時刻・**cache の rotate 時刻**・
        // **注入元 service の subdomain 変更時刻**の一番遅いもの。cache rotate は注入される資格情報
        // そのもの(ACL パスワード)を、subdomain 変更は `_URL`/`_HOST` の中身を差し替えるので、
        // 実行中の app は旧値のまま = 再デプロイが要る。database の rotate は **human role だけ**を
        // 回し app role(注入される側)は不変なので、あちらは対象にしない(m3 設計 §7.2)。
        // GREATEST は NULL を無視する(cache/service 以外の注入は該当列が NULL)。
        //
        // 値は **両端を有限に丸める**:Postgres の infinity は `DateTime<Utc>` に読み込めず sqlx が
        // panic する(= `panic="abort"` なのでプロセスごと落ちる。2026-07-26 の事故)。
        // 下端だけ塞ぐと `+infinity` が素通りするので `LEAST(GREATEST(…), now())` で挟む。
        // 書き込み側は CHECK 制約(20260727000001 / 20260819000001)が拒むので、これは縦深防御。
        "SELECT i.id, i.resource_id, r.kind, r.display_name, i.env_var, i.mount_path,
                (r.deleted_at IS NULL) AS valid,
                LEAST(GREATEST(GREATEST(i.created_at, cd.rotated_at, sd.subdomain_changed_at),
                               'epoch'::timestamptz), now())
                  AS created_at
           FROM injections i
           JOIN resources r ON r.id = i.resource_id
           LEFT JOIN cache_details cd ON cd.resource_id = i.resource_id
           LEFT JOIN service_details sd ON sd.resource_id = i.resource_id
          WHERE i.service_id = $1
          ORDER BY i.env_var",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    // service 単位で 1 度だけ解決する(注入ごとに docker を叩かない)。
    let since = if rows.is_empty() {
        None
    } else {
        serving_since(&state, id).await
    };
    Ok(Json(
        rows.into_iter()
            .map(|r| injection_row_to_dto(r, since))
            .collect(),
    ))
}

/// `POST /api/services/:id/injections`:database / volume / cache / **別 service** を注入する
/// (バインディング)。反映には再デプロイ(値は起動の瞬間に解決 — 決定 #5)。service 注入は
/// 内部直接接続 URL を渡し、網リンクは deploy / reconcile が張る(`doc/paas-service-link-design.md`)。
pub async fn create_injection(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<CreateInjectionReq>,
) -> AppResult<(StatusCode, Json<InjectionDto>)> {
    ensure_owned(&state, auth.user_id, id).await?;

    // 注入元は本人の database / volume / cache / service(未削除)。kind・表示名・subdomain(service のみ)を取る。
    // 源クエリが user_id=$2 で縛るので、別ユーザのリソースは NotFound = **同一 owner 限定は自動で担保**。
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

    // 注入は env_var 本体に加えて**派生 env**(database の `_HOST`/`_PASSWORD` 等)も生む。既存注入の
    // 占有名(本体 + 派生)と 1 つでも被ると、deploy の後勝ちで「URL は A・パスワードは B」のような
    // 静かな取り違えになる(env_var 自体の重複は UNIQUE が弾くが、派生は素通りする)。ここで断る。
    // 検査と INSERT は**同一 tx + service 行の行ロック**で行う:別々にやると、空の service へ
    // `X` と `X_URL` を同時 POST した 2 本が互いを見ずに両方通り(env_var が違うので UNIQUE も
    // 効かない)、派生名が混線する(codex 深審の TOCTOU)。
    let mut tx = state.db.begin().await?;
    sqlx::query("SELECT 1 FROM resources WHERE id = $1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    let occupied = injection_env_names(&mut tx, id).await?;
    let mine = inject::occupied_env_keys(&kind, &env_var);
    if let Some((clash, owner)) = mine
        .iter()
        .find_map(|k| occupied.get(k).map(|owner| (k.clone(), owner.clone())))
    {
        return Err(AppError::BadRequest(format!(
            "env 変数 '{clash}' が既存の注入 '{owner}' と衝突します(注入は派生 env も生みます)。\
             --as で別の名前を指定してください"
        )));
    }

    // 静的 env と同名の**派生** env は注入しない(静的が勝つ = 既存 app を静かに壊さない。
    // inject.rs の push_derived)。ただし黙っていると「素材 env が来ない」ことに気付けないので、
    // どの名前が静的側に譲られたかを非破壊の警告で知らせる(set_env の warning と同型)。
    let derived = inject::derived_env_keys(&kind, &env_var);
    let shadowed = shadowed_static_env(&mut tx, id, &derived).await?;
    let warning = (!shadowed.is_empty()).then(|| {
        format!(
            "静的 env {} が既に在るため、同名の派生 env は注入しません(静的側が有効)。\
             注入値を使うなら `tbm env unset <サービス> {}` で消してください",
            shadowed.join(", "),
            shadowed.join(" ")
        )
    });

    let new_id: Uuid = sqlx::query_scalar(
        // created_at は **clock_timestamp()**(実時刻)で入れる。列の DEFAULT は `now()` =
        // **トランザクション開始時刻**なので、行ロック待ちで tx が長引くと「実際に INSERT した時刻より
        // 古い」created_at が入り、その間に走った deploy の env 解決を跨いだのに「反映済み」に
        // 見える窓ができる(codex review 2026-07-26)。
        "INSERT INTO injections (service_id, resource_id, env_var, mount_path, created_at)
              VALUES ($1, $2, $3, $4, clock_timestamp()) RETURNING id",
    )
    .bind(id)
    .bind(req.resource_id)
    .bind(&env_var)
    .bind(&mount_path)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| {
        map_unique(
            e,
            format!("env 変数 '{env_var}' はこの service で既に使われています"),
        )
    })?;
    tx.commit().await?;

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
            warning,
            // 「未反映」の定義は一覧と同じ関数に寄せる(時刻の基準を変えたとき片方だけ直る事故を防ぐ)。
            // 作った瞬間は、走っているコンテナがあれば必ず未反映(この注入より前に起動している)。
            needs_redeploy: serving_since(&state, id).await.is_some(),
        }),
    ))
}

/// この service の既存注入が**占有している env 名** → その持ち主の env_var。
/// 占有 = 注入の env_var 本体 + そこから生える派生名(`inject::occupied_env_keys`)。
async fn injection_env_names(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    service_id: Uuid,
) -> AppResult<std::collections::HashMap<String, String>> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT i.env_var, r.kind FROM injections i JOIN resources r ON r.id = i.resource_id
          WHERE i.service_id = $1",
    )
    .bind(service_id)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows
        .into_iter()
        .flat_map(|(env_var, kind)| {
            inject::occupied_env_keys(&kind, &env_var)
                .into_iter()
                .map(move |k| (k, env_var.clone()))
        })
        .collect())
}

/// `names` のうち、この service に静的 env として既に在るもの(= 注入で上書きされる分)。
async fn shadowed_static_env(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    service_id: Uuid,
    names: &[String],
) -> AppResult<Vec<String>> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT key FROM service_env WHERE service_id = $1 AND key = ANY($2) ORDER BY key",
    )
    .bind(service_id)
    .bind(names)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows.into_iter().map(|(k,)| k).collect())
}

/// `DELETE /api/injections/:id`:注入を外す(所有権は service 経由で確認)。service 注入なら
/// caller のプライベートネットワークから callee を即切断する(網リンクの掃除。再デプロイ不要)。
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
    // 派生キー(`_HOST` 等 = 注入 env_var 本体ではない)→ 由来 kind。注入側から**前向きに**名前を
    // 作るので、後缀から kind を逆推する必要がない(= service `FOO_URL` と cache `FOO` が併存しても
    // 構造的に取り違えない)。派生名の単一真源は inject.rs::derived_env_keys。
    let derived_kinds: std::collections::HashMap<String, String> = inj_kinds
        .iter()
        .flat_map(|(env_var, kind)| {
            inject::derived_env_keys(kind, env_var)
                .into_iter()
                .map(move |k| (k, kind.clone()))
        })
        .collect();

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
                // 注入 env_var の対応表に無い = 派生キー(database/service の `_HOST` 等、cache の
                // `_KEY_PREFIX`)。既知の限界(受容):明示注入の env_var が他の注入の派生名と同じだと、
                // 表示ラベルは明示注入側に倒れる(値は後勝ちで正しい)。create_injection がこの衝突を
                // 400 で弾くので、新規に作られることはない(既存データのみ)。
                derived_kinds
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(|| "injection".to_string())
            };
            let value = if source == "static" {
                "***".to_string()
            } else {
                mask_injected_value(&key, &value)
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

/// 注入由来の値の表示用マスク。`_PASSWORD` で終わるキーは**値そのものが秘密**なので全伏せ
/// (database の派生 env は裸のパスワード = URL 形ではないので `mask_url_password` では素通り
/// してしまう)。それ以外は URL のパスワード部だけ伏せる。
fn mask_injected_value(key: &str, value: &str) -> String {
    if key.ends_with("_PASSWORD") {
        return "***".to_string();
    }
    mask_url_password(value)
}

/// URL 形(`scheme://user:pass@host…`)の値のパスワード部だけを `***` に伏せる。
/// URL でない値はそのまま(STORAGE_PATH / 接頭辞 / 静的 env は原文 — 暴露ティアは exec と同じで、
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

/// `POST /api/services`:service のプラットフォーム側メタを作る(resources + service_details +
/// deploy_key 生成 + subdomain 採番)。gh / registry 資格情報 / workflow は後チャンク。
/// deploy_key(HMAC の鍵原文)は作成時にここで平文返却する。なお所有者は後から
/// `GET /services/:id/deploy-config`(`tbm deploy --local` の退路)で **再取得できる**(自分の
/// service のみ)— 平文をプラットフォームが持つので可能。**rotate API はまだ無い**:鍵漏洩時はサービスを
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
    check_memory_mb(memory_mb)?;
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
    // CPU 上限は任意(None = 従来どおりソフトな重み付けのみ)。指定時だけ範囲を検証。
    if let Some(cpu) = req.cpu_limit_millis {
        check_cpu_limit_millis(&state, cpu)?;
    }
    // subdomain の明示指定は任意(None = 従来どおり slug から自動採番)。指定時だけ規則を検証
    //(副作用 = registry アカウント作成より前に弾く)。
    if let Some(sub) = req.subdomain.as_deref() {
        validate_subdomain(sub)?;
    }

    // 同名チェック(UNIQUE が最終ガードだが、先に弾いて分かりやすく)。
    if crate::databases::live_name_exists(&state.db, auth.user_id, "service", &display_name).await? {
        return Err(AppError::Conflict(format!(
            "サービス名 '{display_name}' は既に使われています。別の名前にしてください"
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
        cpu_limit_millis: req.cpu_limit_millis,
    };

    // subdomain:明示指定は **1 回だけ**試す(衝突は 409 — 指定した名前を乱数サフィックスで
    // 別名に化けさせない)。省略時は display_name の slug を第一候補に、衝突 / 予約語なら
    // 乱数語を付けて再試行(UNIQUE が最終ガード)。slug が空になる名前(記号だけ等)は
    // "app" にフォールバック。
    let dto = if let Some(sub) = req.subdomain.as_deref() {
        match insert_attempt(&state.db, &state.config, auth.user_id, sub, &new).await {
            Ok(dto) => dto,
            Err(InsertErr::SubdomainTaken) => {
                return Err(AppError::Conflict(subdomain_taken_msg(sub)));
            }
            Err(InsertErr::App(e)) => return Err(e),
        }
    } else {
        let base = {
            let s = slugify(&display_name);
            if s.is_empty() { "app".to_string() } else { s }
        };
        // `tsubomi-` 前綴の base は乱数サフィックスを付けても前綴が残る = 全試行が予約 skip で
        // 必ず失敗する。前綴を剥がして救済する(剥がした残りが英字始まりでなければ "app")。
        let base = match base.strip_prefix("tsubomi-") {
            Some(rest) if rest.starts_with(|c: char| c.is_ascii_lowercase()) => rest.to_string(),
            Some(_) => "app".to_string(),
            None => base,
        };
        let mut created: Option<ServiceDto> = None;
        for attempt in 0..6 {
            let candidate = if attempt == 0 {
                base.clone()
            } else {
                suffixed_candidate(&base)
            };
            if reserved_subdomain(&candidate) {
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
        created.ok_or_else(|| {
            AppError::Conflict(
                "subdomain を生成できませんでした。表示名を変えて再試行してください".into(),
            )
        })?
    };

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
            "cpu_limit_millis": req.cpu_limit_millis,
        }),
        auth.client_ip.as_deref(),
    )
    .await;

    // GitHub 連携に必要な残りの値(プラットフォームは GitHub に触れない — CLI/web がこの値で組み立てる)。
    // setup_commands はプラットフォームが単一真源として作る(CLI/web は文字列を再構築しない)。registry は
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
    cpu_limit_millis: Option<i32>,
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
    // kind 横断 並行 create を無駄に直列化しない(perf review P6)。
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
                (resource_id, subdomain, deploy_key_enc, container_port, visibility, stateful, memory_mb, cpu_limit_millis)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(id)
    .bind(subdomain)
    .bind(new.deploy_key_enc)
    .bind(new.container_port)
    .bind(new.visibility.as_str())
    .bind(new.stateful)
    .bind(new.memory_mb)
    .bind(new.cpu_limit_millis)
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
        cpu_limit_millis: new.cpu_limit_millis,
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
        .take(MAX_SUBDOMAIN_LEN)
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

/// 衝突時の乱数語付き候補。**suffix 込みで 50 字上限を守る**(base を詰めてから付ける)—
/// でないと自動採番の出力(最長 55 字)が validate_subdomain(50 字)を通らず、
/// 「自動で付いた subdomain を変更端点で再指定できない」round-trip 不全になる。
/// base は slugify 出力(ASCII)なのでバイト切りで安全。切り詰めで末尾に残った '-' は落とす。
fn suffixed_candidate(base: &str) -> String {
    let stem_len = MAX_SUBDOMAIN_LEN - 5; // "-xxxx" の 5 字ぶん
    let stem = base[..base.len().min(stem_len)].trim_end_matches('-');
    format!("{stem}-{}", rand_suffix())
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

    /// 判定の入力だけを差し替えるための素の行(表示用の列は判定に影響しない)。
    fn caller_row(
        stateful: bool,
        desired: &str,
        deployed: bool,
        in_flight: bool,
    ) -> inject::CallerRow {
        inject::CallerRow {
            id: Uuid::nil(),
            display_name: "a".into(),
            env_vars: vec!["B_URL".into()],
            desired_state: desired.into(),
            last_deploy_status: None,
            last_deploy_error: None,
            stateful,
            deployed,
            deploy_in_flight: in_flight,
        }
    }

    /// `caller_relink_verdict` の真理値表。**プレビュー(GET)と実行(POST)が引く唯一の判定**
    /// なので、ここが崩れると「名単では対象と言ったのに動かない(or 動いてはいけないのに動く)」
    /// になる。理由の**優先順位**も固定する(文言が変わる = 利用者の次の一手が変わる)。
    #[test]
    fn caller_relink_verdict_table() {
        let ok = caller_row(false, "running", true, false);
        assert!(
            caller_relink_verdict(&ok, true).is_ok(),
            "健全な stateless caller は対象"
        );

        // callee 自身が停止 → 他の入力に関わらず対象外(realias が走らないので回しても純 churn)。
        // 判定の 1 行目で返るので全組合せを回す必要はない — 「健全な行でも弾く」の 1 本で足りる。
        assert!(
            caller_relink_verdict(&ok, false).is_err(),
            "callee 未稼働なら健全な caller でも対象外"
        );

        // 個別の対象外。
        assert!(caller_relink_verdict(&caller_row(false, "running", false, false), true).is_err());
        assert!(caller_relink_verdict(&caller_row(false, "stopped", true, false), true).is_err());
        assert!(caller_relink_verdict(&caller_row(false, "running", true, true), true).is_err());
        assert!(caller_relink_verdict(&caller_row(true, "running", true, false), true).is_err());

        // 優先順位:未デプロイ > 停止中 > 進行中 > stateful。
        assert!(
            caller_relink_verdict(&caller_row(true, "stopped", false, true), true)
                .unwrap_err()
                .contains("まだデプロイされていない"),
            "未デプロイが最優先の理由"
        );
        assert!(
            caller_relink_verdict(&caller_row(true, "stopped", true, true), true)
                .unwrap_err()
                .contains("停止中"),
            "停止中は進行中 / stateful より優先"
        );
        assert!(
            caller_relink_verdict(&caller_row(true, "running", true, true), true)
                .unwrap_err()
                .contains("デプロイ進行中"),
            "進行中は stateful より優先"
        );
    }

    /// **停止中の caller は決して対象にならない**(単独で釘付け)。`commit_success` が
    /// desired_state を running に戻すので、ここが破れると「ユーザが止めた service を
    /// 改名の副作用で叩き起こす」= 意図の消失になる。最終防壁は `run_digest` のロック後
    /// 再確認門(非 user 契機)だが、名単が嘘をつかないこともここで担保する。
    #[test]
    fn stopped_caller_is_never_redeployed() {
        for stateful in [false, true] {
            for in_flight in [false, true] {
                let r = caller_row(stateful, "stopped", true, in_flight);
                assert!(caller_relink_verdict(&r, true).is_err());
            }
        }
    }

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("My App"), "my-app");
        assert_eq!(slugify("  hello--world  "), "hello-world");
        assert_eq!(slugify("API_v2"), "api-v2");
        assert_eq!(slugify("123start"), "s123start");
        assert_eq!(slugify("!!!"), "");
        assert_eq!(slugify("日本語app"), "app");
    }

    /// validate_subdomain の真理値表(明示指定の入口)。
    #[test]
    fn validate_subdomain_rules() {
        for ok in ["myapp", "my-app", "a", "app2", "a-2-b", &"a".repeat(50)] {
            assert!(validate_subdomain(ok).is_ok(), "{ok:?} は通るべき");
        }
        for bad in [
            "",
            "My-App",     // 大文字
            "-app",       // 英字始まりでない
            "app-",       // '-' 終わり
            "2app",       // 数字始まり
            "my.app",     // '.' は不可(DNS ラベル 1 個ぶんだけ)
            "my app",     // 空白
            "日本語",     // 非 ASCII
            &"a".repeat(51),
        ] {
            assert!(validate_subdomain(bad).is_err(), "{bad:?} は弾くべき");
        }
        // 予約:固定語(公開 DB / cache 入口の db / cache 含む)+ `tsubomi-` 前綴
        // (私網の infra / app コンテナ名との docker DNS 衝突防止)。
        for reserved in ["www", "api", "registry", "db", "cache", "tsubomi-valkey", "tsubomi-x"] {
            assert!(validate_subdomain(reserved).is_err(), "{reserved:?} は予約");
        }
    }

    /// slugify の出力(+ 乱数サフィックス形)は validate_subdomain の形式規則を必ず通る
    /// (予約語は自動採番ループ側が skip するので形式のみ確認)。**上限いっぱいの長名**を
    /// 含める — suffix 込みの 50 字上限(suffixed_candidate)が破れると、自動採番の出力を
    /// 変更端点で再指定できない round-trip 不全になる(codex 監査で顕在化した穴)。
    #[test]
    fn slugify_output_passes_subdomain_rules() {
        let long = "a".repeat(60);
        let hyphen_tail = format!("{}-x", "b".repeat(44)); // 切り詰め位置に '-' が残る形
        for name in [
            "My App",
            "  hello--world  ",
            "API_v2",
            "123start",
            "日本語app x",
            long.as_str(),
            hyphen_tail.as_str(),
        ] {
            let s = slugify(name);
            if s.is_empty() {
                continue; // create は "app" にフォールバック
            }
            let suffixed = suffixed_candidate(&s);
            for candidate in [s, suffixed] {
                assert!(
                    validate_subdomain(&candidate).is_ok() || reserved_subdomain(&candidate),
                    "{candidate:?} が形式規則を通らない"
                );
            }
        }
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
    fn derived_env_keys_are_generated_per_kind() {
        // database は URL の他に接続の素材を 6 本。基底は `_URL` を剥いだ形。
        assert_eq!(
            inject::derived_env_keys("database", "DATABASE_URL"),
            [
                "DATABASE_HOST",
                "DATABASE_PORT",
                "DATABASE_USER",
                "DATABASE_PASSWORD",
                "DATABASE_NAME",
                "DATABASE_SSLMODE"
            ]
        );
        // `--as` の裸名(`_URL` 無し)はそのまま基底になる。
        assert_eq!(inject::derived_env_keys("service", "BARE"), ["BARE_HOST", "BARE_PORT"]);
        assert_eq!(inject::derived_env_keys("cache", "REDIS_URL"), ["REDIS_KEY_PREFIX"]);
        // volume は派生しない(mount_path を env_var に入れるだけ)。未知 kind も同様。
        assert!(inject::derived_env_keys("volume", "STORAGE_PATH").is_empty());
        assert!(inject::derived_env_keys("nope", "X_URL").is_empty());
        // 占有名 = 本体 + 派生(create_injection の衝突検査が引く集合)。
        let occupied = inject::occupied_env_keys("cache", "REDIS_URL");
        assert!(occupied.contains(&"REDIS_URL".to_string()));
        assert!(occupied.contains(&"REDIS_KEY_PREFIX".to_string()));
        // 基底が同じ 2 件(`X` と `X_URL`)は派生名が丸ごと衝突する = 検査が拾える形になっている。
        let a = inject::occupied_env_keys("database", "X");
        let b = inject::occupied_env_keys("database", "X_URL");
        assert!(a.iter().any(|k| b.contains(k)));
    }

    #[test]
    fn injected_password_is_masked_by_key() {
        // database 派生の裸パスワードは URL 形ではないので、キー後缀で全伏せする(安全)。
        assert_eq!(super::mask_injected_value("DATABASE_PASSWORD", "s3cret"), "***");
        assert_eq!(super::mask_injected_value("MYDB2_PASSWORD", "s3cret"), "***");
        // 秘密でない派生はそのまま見せる(ホスト / 形を知るのが env/resolved の目的)。
        assert_eq!(
            super::mask_injected_value("DATABASE_HOST", "tsubomi-pgbouncer"),
            "tsubomi-pgbouncer"
        );
        assert_eq!(super::mask_injected_value("DATABASE_USER", "app_ab12"), "app_ab12");
        // URL 本体は従来どおりパスワード部だけ伏せる。
        assert_eq!(
            super::mask_injected_value("DATABASE_URL", "postgres://app:secret@pgb:6432/db"),
            "postgres://app:***@pgb:6432/db"
        );
    }

    #[test]
    fn default_visibility_derives_from_port() {
        // 8080(プラットフォームの HTTP 契約港)= 従来どおり company。
        assert_eq!(default_visibility(8080), Visibility::Company);
        // それ以外(持ち込み DB 等の非 HTTP ソフト想定)= private。
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
