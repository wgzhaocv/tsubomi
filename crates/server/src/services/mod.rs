//! service リソースの API(tech-design §6 の service 面)。web と CLI は同一ハンドラの
//! 2 入口 — 認証 extractor(AuthCtx)だけが分岐点。
//!
//! M3 第 1 チャンク(S1–S3、曳光弾)は最小 create + deploy hook + コンテナ起動まで。
//! gh オーケストレーション / 注入 / start・stop・logs / rollback / web 画面 / reconcile は
//! 後チャンク(plan・paas-m3-design.md)。

pub mod deploy;
pub mod docker;
pub mod route;

use crate::auth::AuthCtx;
use crate::databases::{audit, map_unique};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::validate;
use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::PgPool;
use tsubomi_shared::{CreateServiceReq, CreateServiceResp, ServiceDto};
use uuid::Uuid;

const MAX_NAME_LEN: usize = 64;
/// subdomain 生成の予約語(平台 / インフラのホスト名と衝突させない)。
const RESERVED_SUBDOMAINS: &[&str] = &["paas", "registry", "traefik", "www", "api"];
/// deploy_key の乱数バイト数(base64url で ≈43 字)。HMAC の鍵そのもの。
const DEPLOY_KEY_BYTES: usize = 32;

pub fn routes() -> Router<AppState> {
    Router::new().route("/services", post(create))
}

/// `POST /api/services`:service の平台側メタを作る(resources + service_details +
/// deploy_key 生成 + subdomain 採番)。gh / registry 資格情報 / workflow は後チャンク。
/// **deploy_key は発行時の 1 回だけ**平文で返す(HMAC の鍵)。
pub async fn create(
    auth: AuthCtx,
    State(state): State<AppState>,
    Json(req): Json<CreateServiceReq>,
) -> AppResult<(StatusCode, Json<CreateServiceResp>)> {
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
        match insert_attempt(&state.db, auth.user_id, &display_name, &candidate, &deploy_key_enc)
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
        AppError::Conflict("subdomain を生成できませんでした。表示名を変えて再試行してください".into())
    })?;

    audit(
        &state.db,
        Some(auth.user_id),
        "service.create",
        dto.id,
        json!({ "display_name": display_name, "subdomain": dto.subdomain }),
    )
    .await;

    Ok((
        StatusCode::CREATED,
        Json(CreateServiceResp {
            service: dto,
            deploy_key,
        }),
    ))
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
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1::text), 42)")
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
            assert!(s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()));
        }
    }
}
