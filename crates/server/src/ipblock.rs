//! 会社 IP 許可リストの API ハンドラ + traefik 動的設定への収束(ガバナンス)。
//! web と CLI は同一ハンドラの 2 入口 — owner だけが触れる(role を毎回検証)。
//!
//! 背骨:管制面 Postgres(ip_allow_entries)が「期望状態」を持ち、現実(traefik の
//! ipAllowList middleware)をそこへ収束させる。owner が CIDR を足す / 消すたびに、
//! 平台が traefik の動的設定ファイルを書き直し、file provider がホットリロードする。
//!
//! 意味は「許可リスト」:
//!   * 空        = 制限なし(全 IP 許可、fail-open)。
//!   * 1 件以上  = 列挙した CIDR だけが service に到達でき、他は遮断。
//!
//! 個々の service ルータがこの middleware を参照する label を持つ(docker.rs)。
//! registry / deploy hook は label を付けないことで除外する(決定 #4)。

use crate::auth::AuthCtx;
use crate::databases::{audit, map_unique};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use chrono::{DateTime, Utc};
use serde_json::json;
use std::net::IpAddr;
use std::str::FromStr;
use tsubomi_shared::{CreateIpAllowReq, IpAllowEntryDto};
use uuid::Uuid;

/// traefik 動的設定で定義する middleware 名。docker.rs のラベルが
/// `<NAME>@file` で参照する(file provider 由来を示す `@file` サフィックス)。
pub const TRAEFIK_MIDDLEWARE: &str = "tsubomi-ipallow";

/// メモの最大長(表示名と同じ感覚の自由文字列。暴走入力だけ弾く)。
const MAX_NOTE_LEN: usize = 200;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/ip-allowlist", get(list).post(create))
        .route("/ip-allowlist/{id}", axum::routing::delete(delete))
}

/// owner 専用ゲート。design v2 §7「owner 操作はバックエンドで毎回検証」。
/// IP 許可リストは owner ガバナンス = web 専用なので、admin と同じく **owner 身分 かつ
/// session 由来**を要求する(Bearer cli_token は拒否)。`require_owner_web` を再利用。
fn require_owner(auth: &AuthCtx) -> AppResult<()> {
    crate::admin::require_owner_web(auth)
}

/// 入力 CIDR を正規化:単一 IP は /32(v4)・/128(v6)に、レンジはそのまま検証して
/// 正規表現ではなく ipnet/IpAddr のパーサで受理。受理した文字列を traefik にそのまま
/// 流すので、ここを通った値だけが設定ファイルに載る(不正値の混入を断つ)。
fn normalize_cidr(raw: &str) -> AppResult<String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(AppError::BadRequest(
            "CIDR が空です(例:203.0.113.0/24 または 198.51.100.7)".into(),
        ));
    }
    if let Ok(net) = ipnet::IpNet::from_str(s) {
        return Ok(net.to_string());
    }
    if let Ok(ip) = IpAddr::from_str(s) {
        let prefix = if ip.is_ipv4() { 32 } else { 128 };
        return Ok(format!("{ip}/{prefix}"));
    }
    Err(AppError::BadRequest(format!(
        "CIDR の形式が不正です: '{s}'(例:203.0.113.0/24 または 198.51.100.7)"
    )))
}

// ===== ハンドラ =====

/// `GET /api/ip-allowlist`:現在の許可レンジ一覧(新しい順)。owner のみ。
pub async fn list(
    auth: AuthCtx,
    State(state): State<AppState>,
) -> AppResult<Json<Vec<IpAllowEntryDto>>> {
    require_owner(&auth)?;
    let rows: Vec<(Uuid, String, String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT id, cidr, note, created_at FROM ip_allow_entries ORDER BY created_at DESC",
    )
    .fetch_all(&state.db)
    .await?;
    let dtos = rows
        .into_iter()
        .map(|(id, cidr, note, created_at)| IpAllowEntryDto {
            id,
            cidr,
            note,
            created_at,
        })
        .collect();
    Ok(Json(dtos))
}

/// `POST /api/ip-allowlist`:CIDR を 1 件追加 → traefik へ即時反映。owner のみ。
pub async fn create(
    auth: AuthCtx,
    State(state): State<AppState>,
    Json(req): Json<CreateIpAllowReq>,
) -> AppResult<(StatusCode, Json<IpAllowEntryDto>)> {
    require_owner(&auth)?;
    let cidr = normalize_cidr(&req.cidr)?;
    let note = req.note.trim();
    if note.chars().count() > MAX_NOTE_LEN {
        return Err(AppError::BadRequest(format!(
            "メモは{MAX_NOTE_LEN}文字以内です"
        )));
    }

    let (id, created_at): (Uuid, DateTime<Utc>) = sqlx::query_as(
        "INSERT INTO ip_allow_entries (cidr, note, created_by) VALUES ($1, $2, $3)
         RETURNING id, created_at",
    )
    .bind(&cidr)
    .bind(note)
    .bind(auth.user_id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| {
        map_unique(
            e,
            format!("CIDR '{cidr}' は既に許可リストにあります。一覧で確認してください"),
        )
    })?;

    // DB(期望状態)を真実源として、traefik へ収束させる。書き込み失敗は best-effort
    // でログのみ(行は保存済み — 次回の変更 / サーバ再起動の起動時同期で収束する)。
    sync_traefik(&state).await;

    audit(
        &state.db,
        Some(auth.user_id),
        "ip_allowlist.add",
        id,
        json!({ "cidr": cidr, "note": note }),
    )
    .await;

    Ok((
        StatusCode::CREATED,
        Json(IpAllowEntryDto {
            id,
            cidr,
            note: note.to_owned(),
            created_at,
        }),
    ))
}

/// `DELETE /api/ip-allowlist/:id`:1 件削除 → traefik へ即時反映。owner のみ。
/// 最後の 1 件を消すと許可リストは空(= 全 IP 許可)に戻る。
pub async fn delete(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<StatusCode> {
    require_owner(&auth)?;
    let row: Option<(String,)> =
        sqlx::query_as("DELETE FROM ip_allow_entries WHERE id = $1 RETURNING cidr")
            .bind(id)
            .fetch_optional(&state.db)
            .await?;
    let Some((cidr,)) = row else {
        return Err(AppError::NotFound);
    };

    sync_traefik(&state).await;

    audit(
        &state.db,
        Some(auth.user_id),
        "ip_allowlist.remove",
        id,
        json!({ "cidr": cidr }),
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}

// ===== traefik への収束 =====

/// 現在の許可リストを読んで traefik の動的設定ファイル(ipallow.yml)を原子的に書き直す。
/// 起動時(main)と各変更後に呼ぶ。best-effort:失敗してもリクエストは止めない
/// (DB が真実源。起動時同期や次回変更で収束する)。
pub async fn sync_traefik(state: &AppState) {
    if let Err(e) = sync_traefik_inner(state).await {
        tracing::error!(
            error = ?e,
            "traefik 動的設定(IP 許可リスト)の同期に失敗 — DB は更新済み。再起動 / 次回変更で収束する"
        );
    }
}

async fn sync_traefik_inner(state: &AppState) -> AppResult<()> {
    let cidrs: Vec<String> =
        sqlx::query_scalar("SELECT cidr FROM ip_allow_entries ORDER BY created_at")
            .fetch_all(&state.db)
            .await?;

    let dir = &state.config.traefik_dynamic_dir;
    tokio::fs::create_dir_all(dir).await?;

    // 一時ファイルへ書いて atomic rename(traefik が中途半端な内容を読まない)。
    let target = dir.join("ipallow.yml");
    let tmp = dir.join(".ipallow.yml.tmp");
    tokio::fs::write(&tmp, render_yaml(&cidrs)).await?;
    tokio::fs::rename(&tmp, &target).await?;
    tracing::info!(count = cidrs.len(), "IP 許可リストを traefik へ同期した");
    Ok(())
}

/// traefik 動的設定(YAML)を組み立てる。空リスト = fail-open(全 IP 許可)。
/// CIDR は normalize_cidr を通った値だけなので安全(それでも引用符で包む)。
fn render_yaml(cidrs: &[String]) -> String {
    // 空 = 制限なし。0.0.0.0/0 + ::/0 で全 v4/v6 を許可(middleware は常に定義する —
    // ルータが参照する name が未定義だと traefik がそのルートを弾くため)。
    let ranges: Vec<&str> = if cidrs.is_empty() {
        vec!["0.0.0.0/0", "::/0"]
    } else {
        cidrs.iter().map(String::as_str).collect()
    };

    let mut s = String::new();
    s.push_str("# 平台(tsubomi-server)が自動生成。手で編集しない —\n");
    s.push_str("# owner の IP 許可リスト変更で毎回上書きされる。\n");
    s.push_str("# 空リスト = 全 IP 許可(fail-open)。1 件以上 = その CIDR だけ許可。\n");
    s.push_str("http:\n");
    s.push_str("  middlewares:\n");
    s.push_str(&format!("    {TRAEFIK_MIDDLEWARE}:\n"));
    s.push_str("      ipAllowList:\n");
    s.push_str("        sourceRange:\n");
    for c in ranges {
        s.push_str(&format!("          - \"{c}\"\n"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_accepts_cidr_and_single_ip() {
        assert_eq!(normalize_cidr("203.0.113.0/24").unwrap(), "203.0.113.0/24");
        assert_eq!(normalize_cidr(" 198.51.100.7 ").unwrap(), "198.51.100.7/32");
        assert_eq!(normalize_cidr("2001:db8::/48").unwrap(), "2001:db8::/48");
        assert_eq!(normalize_cidr("::1").unwrap(), "::1/128");
    }

    #[test]
    fn normalize_rejects_garbage() {
        assert!(normalize_cidr("").is_err());
        assert!(normalize_cidr("not-an-ip").is_err());
        assert!(normalize_cidr("203.0.113.0/99").is_err());
        assert!(normalize_cidr("999.0.0.1").is_err());
    }

    #[test]
    fn empty_list_is_fail_open() {
        let yaml = render_yaml(&[]);
        assert!(yaml.contains("0.0.0.0/0"));
        assert!(yaml.contains("::/0"));
        assert!(yaml.contains(TRAEFIK_MIDDLEWARE));
    }

    #[test]
    fn nonempty_list_only_lists_given_ranges() {
        let yaml = render_yaml(&["10.0.0.0/8".to_string()]);
        assert!(yaml.contains("10.0.0.0/8"));
        assert!(!yaml.contains("0.0.0.0/0"));
    }
}
