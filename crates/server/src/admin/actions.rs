//! 最後の砦(M4 S3):owner が他人の資源を stop / delete する二段確認。
//! 1 段目(code 無し)= 6 桁コードを生成して owner **自身**にメール(本人確認)。
//! 2 段目(code 有り)= admin_action_codes を単回消費で検証 → 実行(既存のソフト削除を再利用)
//! → `target_user` 付きで audit。owner + session を毎回検証(require_owner_web)。
//!
//! 削除は普通のソフト削除(対象ユーザのゴミ箱へ・3 日猶予・復元可)= owner は「処置」するが
//! 「抹消」はしない(設計 v2 §11 / m4 §10-E)。

use crate::admin::require_owner_web;
use crate::auth::AuthCtx;
use crate::databases::{self, audit_with_target};
use crate::error::{AppError, AppResult};
use crate::mail;
use crate::services;
use crate::state::AppState;
use crate::volumes;
use anyhow::anyhow;
use axum::Json;
use axum::extract::{Path, State};
use rand::RngExt;
use serde_json::json;
use tsubomi_shared::{AdminActionReq, AdminActionResp, sha256_hex};
use uuid::Uuid;

/// 検証コードの有効期限(分)。
const CODE_TTL_MINUTES: i64 = 10;

/// `POST /api/admin/resources/:id/stop`(owner・web)。
pub async fn stop(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<AdminActionReq>,
) -> AppResult<Json<AdminActionResp>> {
    handle(auth, state, id, "stop", req).await
}

/// `POST /api/admin/resources/:id/delete`(owner・web)。ソフト削除(対象ユーザのゴミ箱へ)。
pub async fn delete(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<AdminActionReq>,
) -> AppResult<Json<AdminActionResp>> {
    handle(auth, state, id, "delete", req).await
}

async fn handle(
    auth: AuthCtx,
    state: AppState,
    id: Uuid,
    action: &str,
    req: AdminActionReq,
) -> AppResult<Json<AdminActionResp>> {
    require_owner_web(&auth)?;
    let (kind, target_user) = resolve_resource(&state, id).await?;
    validate_action(action, &kind)?;

    match req.code {
        // 1 段目:コードを発行して owner にメール。
        None => {
            issue_code(&state, auth.user_id, id, action, &kind).await?;
            Ok(Json(AdminActionResp {
                code_required: true,
            }))
        }
        // 2 段目:単回消費で検証 → 実行 → target_user 付き audit。
        Some(code) => {
            consume_code(&state, auth.user_id, id, action, &code).await?;
            execute(&state, id, action, &kind).await?;
            audit_with_target(
                &state.db,
                auth.user_id,
                &format!("owner.{action}_{kind}"),
                id,
                target_user,
                json!({ "action": action, "kind": kind }),
            )
            .await;
            tracing::warn!(%id, kind, action, %target_user, "owner 代理操作を実行(最後の砦)");
            Ok(Json(AdminActionResp {
                code_required: false,
            }))
        }
    }
}

/// 対象資源の (kind, 所有者 user_id)。削除済み / 不在は 404。
async fn resolve_resource(state: &AppState, id: Uuid) -> AppResult<(String, Uuid)> {
    sqlx::query_as("SELECT kind, user_id FROM resources WHERE id = $1 AND deleted_at IS NULL")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound)
}

/// action が kind に妥当か。stop = service のみ(DB/volume/cache に「停止」は無い — §10-G)、
/// delete = service / database / volume / cache。
fn validate_action(action: &str, kind: &str) -> AppResult<()> {
    let ok = match action {
        "stop" => kind == "service",
        "delete" => matches!(kind, "service" | "database" | "volume" | "cache"),
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(AppError::BadRequest(format!(
            "{action} は {kind} には適用できません"
        )))
    }
}

/// 1 段目:6 桁コードを生成 → sha256 を admin_action_codes に保存 → owner 自身にメール。
/// 同一 (actor, resource, action) の古い未使用コードは先に消す(連続請求で散らからない)。
async fn issue_code(
    state: &AppState,
    actor: Uuid,
    id: Uuid,
    action: &str,
    kind: &str,
) -> AppResult<()> {
    let code = format!("{:06}", rand::rng().random_range(0..1_000_000u32));
    sqlx::query(
        "DELETE FROM admin_action_codes WHERE actor_id = $1 AND resource_id = $2 AND action = $3",
    )
    .bind(actor)
    .bind(id)
    .bind(action)
    .execute(&state.db)
    .await?;
    sqlx::query(
        "INSERT INTO admin_action_codes (code_hash, actor_id, resource_id, action, expires_at)
         VALUES ($1, $2, $3, $4, now() + make_interval(mins => $5))",
    )
    .bind(sha256_hex(&code))
    .bind(actor)
    .bind(id)
    .bind(action)
    .bind(CODE_TTL_MINUTES as i32)
    .execute(&state.db)
    .await?;

    // dev / Resend 未契約ではメールが飛ばない(mail は subject だけ log し本文は出さない)。
    // owner がコードを使えるよう、その時だけコードを log に出す(本番は email 経由で log に出さない)。
    if state.config.resend_api_key.is_none() {
        tracing::warn!(%code, action, %id, "[dev] owner 確認コード(RESEND 未設定のため log 表示)");
    }

    let to = owner_email(state, actor).await;
    let subject = format!("[tsubomi] 確認コード:{kind} を {action}");
    let body = format!(
        "owner 操作の確認コードです。\n\n  {code}\n\n\
         このコードを画面に入力すると、対象の {kind} を {action} します(有効期限 {CODE_TTL_MINUTES} 分)。\n\
         心当たりがなければ無視してください。"
    );
    let ttl = CODE_TTL_MINUTES.to_string();
    let html = mail::render(
        mail::TPL_ACTION_CODE,
        &[
            ("code", code.as_str()),
            ("kind", kind),
            ("action", action),
            ("ttl", &ttl),
        ],
    );
    mail::send(state, &to, &subject, &html, &body)
        .await
        .map_err(|e| AppError::Other(anyhow!("確認コードメールを送信できませんでした: {e}")))
}

/// 2 段目:コードを単回消費で検証(文脈一致 + 期限内を 1 文で)。無効 / 期限切れは 400。
///
/// **総当たり防止**:誤コードのときは、その (actor, resource, action) の未使用コードを**焼く**
/// (削除する)。6 桁 = 100 万通りだが、1 回の誤りで再請求(= 再メール)が必須になるので、
/// 総当たりには 100 万通の確認メールが要る = owner に即バレる。窓 10 分 + 焼却で総当たりを封じる。
async fn consume_code(
    state: &AppState,
    actor: Uuid,
    id: Uuid,
    action: &str,
    code: &str,
) -> AppResult<()> {
    let row: Option<(Uuid,)> = sqlx::query_as(
        "DELETE FROM admin_action_codes
          WHERE code_hash = $1 AND actor_id = $2 AND resource_id = $3 AND action = $4
            AND expires_at > now()
          RETURNING resource_id",
    )
    .bind(sha256_hex(code))
    .bind(actor)
    .bind(id)
    .bind(action)
    .fetch_optional(&state.db)
    .await?;
    if row.is_some() {
        return Ok(());
    }
    // 誤コード:この文脈の未使用コードを焼く(次は再請求 = 再メールが必要 → 総当たり不能)。
    sqlx::query(
        "DELETE FROM admin_action_codes WHERE actor_id = $1 AND resource_id = $2 AND action = $3",
    )
    .bind(actor)
    .bind(id)
    .bind(action)
    .execute(&state.db)
    .await?;
    Err(AppError::BadRequest(
        "確認コードが無効です。操作をやり直してコードを再送してください".into(),
    ))
}

/// コードの送り先 = **操作している owner 自身**のメール(本人確認)。owner の定義(cfg.owner_emails)
/// への broadcast ではなく、session の本人だけに送る = 別 owner に使えないコードを送らない。
/// AuthCtx は email を持たないので users から引く(rotate された人の行ではなく現 session の本人)。
/// 無ければ空(mail::send が宛先空で no-op)。
async fn owner_email(state: &AppState, actor: Uuid) -> Vec<String> {
    sqlx::query_scalar::<_, String>("SELECT email FROM users WHERE id = $1")
        .bind(actor)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .into_iter()
        .collect()
}

/// 実行 = 既存のソフト削除 / 停止を再利用(所有権チェックは admin ゲート + コードで代替済み)。
async fn execute(state: &AppState, id: Uuid, action: &str, kind: &str) -> AppResult<()> {
    match (action, kind) {
        ("stop", "service") => services::stop_service(state, id).await,
        ("delete", "service") => services::soft_delete(state, id).await,
        ("delete", "database") => databases::soft_delete(state, id).await.map(|_| ()),
        ("delete", "volume") => volumes::soft_delete(state, id).await.map(|_| ()),
        ("delete", "cache") => crate::caches::soft_delete(state, id).await.map(|_| ()),
        // validate_action を通っているのでここには来ない。
        _ => Err(AppError::BadRequest("未対応の操作です".into())),
    }
}
