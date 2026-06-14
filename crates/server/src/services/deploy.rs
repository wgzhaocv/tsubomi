//! deploy hook(no-auth、HMAC 検証)と `run_digest`(build 済みイメージを起こす単一操作)。
//!
//! build と run は別部分(m3-design §6.8 / 決定 #3):平台は **build しない**。CI か
//! `tbm deploy --local` が registry に push し、hook が digest を運んでくる。平台の仕事は
//! 「digest を受けて起こす」だけ。run_digest は hook / --local / start / rollback /
//! reconcile が共有する(S3 では hook 経路だけを通す。注入は S6 — ここは PORT のみ)。

use crate::databases::{audit, map_unique};
use crate::error::{AppError, AppResult};
use crate::services::docker::{self, RunSpec};
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

const SIGNATURE_HEADER: &str = "x-tsubomi-signature";
/// ts の許容ずれ(リプレイ防御の片割れ。もう片方は nonce 一意)。
const MAX_SKEW_SECS: i64 = 300;

/// hook body。**生バイトで HMAC 検証してから** serde で読む(serde 経由で受けて
/// 再シリアライズすると 1 バイトの差で署名が割れるため、Bytes で生を取る)。
#[derive(Deserialize)]
struct HookBody {
    service_id: Uuid,
    git_sha: String,
    image_digest: String,
    ts: i64,
    nonce: String,
}

/// `POST /api/hook/deploy`(session 不要、IP 除外。決定 #4)。
/// HMAC = 権限そのもの。署名不一致は 401、ts 範囲外は 400、nonce 重複は 409、受理は 202。
pub async fn deploy(
    State(state): State<AppState>,
    headers: HeaderMap,
    raw: Bytes,
) -> AppResult<StatusCode> {
    // 1. service_id を取り出す(鍵を引くため。まだ信用しない)。
    let body: HookBody = serde_json::from_slice(&raw)
        .map_err(|_| AppError::BadRequest("hook body が不正な JSON です".into()))?;

    // 2. deploy_key を引いて HMAC を定数時間比較。鍵が無い(= service 不在)も 401 に
    //    収束させ、署名の前に service の存在を漏らさない。
    let key_enc: Option<Vec<u8>> =
        sqlx::query_scalar("SELECT deploy_key_enc FROM service_details WHERE resource_id = $1")
            .bind(body.service_id)
            .fetch_optional(&state.db)
            .await?;
    let key_enc = key_enc.ok_or(AppError::Unauthorized)?;
    let deploy_key = state.crypto.decrypt(&key_enc)?;

    let sig = headers
        .get(SIGNATURE_HEADER)
        .and_then(|v| v.to_str().ok())
        .ok_or(AppError::Unauthorized)?;
    let provided = hex::decode(sig).map_err(|_| AppError::Unauthorized)?;
    let expected = hmac_sha256(deploy_key.as_bytes(), &raw);
    if !ct_eq(&expected, &provided) {
        return Err(AppError::Unauthorized);
    }

    // 認証済み。image_digest が本物の digest か検証する:平台は digest でしか pull しない
    // (決定 #3 の内容アドレス invariant)。tag や不正参照を受けると invariant が崩れるので弾く。
    if !is_sha256_digest(&body.image_digest) {
        return Err(AppError::BadRequest(
            "image_digest は sha256:<64桁16進> 形式の digest である必要があります(tag は不可 — 決定 #3)"
                .into(),
        ));
    }

    // 3. リプレイ防御:時刻窓 + nonce 一意。
    let now = chrono::Utc::now().timestamp();
    if (now - body.ts).abs() > MAX_SKEW_SECS {
        return Err(AppError::BadRequest(format!(
            "ts が許容窓(±{MAX_SKEW_SECS}s)の外です。送信側とサーバの時刻ずれを確認してください"
        )));
    }
    sqlx::query("INSERT INTO deploy_nonces (service_id, nonce) VALUES ($1, $2)")
        .bind(body.service_id)
        .bind(&body.nonce)
        .execute(&state.db)
        .await
        .map_err(|e| map_unique(e, "この nonce は既に使われています(リプレイ)"))?;

    // 4. deploys を received で記録 → 非同期パイプラインへ。GH Action を待たせず 202。
    let deploy_id: Uuid = sqlx::query_scalar(
        "INSERT INTO deploys (service_id, git_sha, image_digest, status)
              VALUES ($1, $2, $3, 'received') RETURNING id",
    )
    .bind(body.service_id)
    .bind(&body.git_sha)
    .bind(&body.image_digest)
    .fetch_one(&state.db)
    .await?;

    let state2 = state.clone();
    let service_id = body.service_id;
    let image_digest = body.image_digest.clone();
    let git_sha = body.git_sha.clone();
    tokio::spawn(async move {
        if let Err(e) =
            run_digest(&state2, deploy_id, service_id, &image_digest, &git_sha).await
        {
            tracing::error!(error = ?e, %deploy_id, %service_id, "deploy パイプライン失敗");
        }
    });

    Ok(StatusCode::ACCEPTED)
}

/// build 済みイメージ(digest)を起こす単一操作。失敗は deploys / service_details に記録する。
/// pull を stop_remove より先に行うので、**pull 失敗(最も多い失敗)では旧コンテナは無傷**。
/// pull 後の create/start 失敗は単純 swap の瞬断中に当たり旧は既に無い(chunk 1 は §6.5 の
/// 旧停止→新起動のまま。start-first 化 + 並行 deploy の直列化 + reconcile 復帰は S5/S8)。
pub async fn run_digest(
    state: &AppState,
    deploy_id: Uuid,
    service_id: Uuid,
    image_digest: &str,
    git_sha: &str,
) -> AppResult<()> {
    let _ = sqlx::query(
        "UPDATE service_details SET phase='deploying', phase_detail=NULL WHERE resource_id=$1",
    )
    .bind(service_id)
    .execute(&state.db)
    .await;

    let outcome = run_digest_inner(state, deploy_id, service_id, image_digest, git_sha).await;
    if let Err(e) = &outcome {
        let msg = e.to_string();
        let _ = sqlx::query(
            "UPDATE deploys SET status='failed', error=$2, finished_at=now() WHERE id=$1",
        )
        .bind(deploy_id)
        .bind(&msg)
        .execute(&state.db)
        .await;
        let _ = sqlx::query(
            "UPDATE service_details SET phase='failed', phase_detail=$2 WHERE resource_id=$1",
        )
        .bind(service_id)
        .bind(&msg)
        .execute(&state.db)
        .await;
    }
    outcome
}

async fn run_digest_inner(
    state: &AppState,
    deploy_id: Uuid,
    service_id: Uuid,
    image_digest: &str,
    git_sha: &str,
) -> AppResult<()> {
    // 起動に必要な確定値を引く。
    let row: Option<(String, i32, i32, i32)> = sqlx::query_as(
        "SELECT subdomain, container_port, memory_mb, cpu_shares
           FROM service_details WHERE resource_id = $1",
    )
    .bind(service_id)
    .fetch_optional(&state.db)
    .await?;
    let (subdomain, container_port, memory_mb, cpu_shares) = row.ok_or(AppError::NotFound)?;

    set_status(state, deploy_id, "pulling").await;
    let image_ref = docker::pull(state, service_id, image_digest).await?;

    set_status(state, deploy_id, "starting").await;
    // swap:旧停止 → 新起動(瞬断許容、health ゲートなし。m3-design §6.5)。
    docker::stop_remove(state, service_id).await?;
    // 注入は S6。S3 は PORT だけ(app が $PORT を読む流儀向け)。
    let spec = RunSpec {
        service_id,
        subdomain,
        git_sha: git_sha.to_string(),
        container_port,
        memory_mb,
        cpu_shares,
        env: vec![("PORT".to_string(), container_port.to_string())],
    };
    docker::run(state, &spec, &image_ref).await?;

    // traefik の file provider 用ルート(router + service)を書く。これでホスト名で到達可能になる。
    crate::services::route::write(state, service_id, &spec.subdomain, spec.container_port)?;

    // 成功:現行 digest / phase=running / desired_state=running を記録。
    sqlx::query(
        "UPDATE service_details
            SET image_digest=$2, phase='running', desired_state='running',
                phase_detail=NULL, last_deploy_at=now()
          WHERE resource_id=$1",
    )
    .bind(service_id)
    .bind(image_digest)
    .execute(&state.db)
    .await?;
    sqlx::query("UPDATE deploys SET status='succeeded', finished_at=now() WHERE id=$1")
        .bind(deploy_id)
        .execute(&state.db)
        .await?;
    audit(
        &state.db,
        None,
        "service.deploy",
        service_id,
        json!({ "git_sha": git_sha, "image_digest": image_digest }),
    )
    .await;
    Ok(())
}

async fn set_status(state: &AppState, deploy_id: Uuid, status: &str) {
    let _ = sqlx::query("UPDATE deploys SET status=$2 WHERE id=$1")
        .bind(deploy_id)
        .bind(status)
        .execute(&state.db)
        .await;
}

// ===== HMAC-SHA256(既存の sha2 0.11 を直接使う。hmac crate を足さず版衝突を避ける)=====

/// RFC 2104 の HMAC-SHA256。block=64。鍵が block より長ければ一度ハッシュする。
fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(msg);
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner);
    outer.finalize().into()
}

/// `sha256:` + 64 桁 16 進かどうか。tag や任意文字列を弾く(決定 #3 の digest ピン留め)。
fn is_sha256_digest(s: &str) -> bool {
    s.strip_prefix("sha256:")
        .is_some_and(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// 定数時間比較(長さ違いは即 false。HMAC 出力は固定長なので長さは秘密でない)。
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 4231 Test Case 2(key="Jefe", data="what do ya want for nothing?")の
    /// HMAC-SHA256 既知ベクタ。手書き HMAC が正しいことを固定する。
    #[test]
    fn hmac_sha256_rfc4231_case2() {
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            hex::encode(mac),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn ct_eq_basic() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
    }

    #[test]
    fn sha256_digest_validation() {
        assert!(is_sha256_digest(&format!("sha256:{}", "a".repeat(64))));
        assert!(is_sha256_digest(&format!("sha256:{}", "0123456789abcdef".repeat(4))));
        assert!(!is_sha256_digest("latest")); // tag
        assert!(!is_sha256_digest("myrepo:v1")); // tag
        assert!(!is_sha256_digest("sha256:abc")); // 短い
        assert!(!is_sha256_digest(&format!("sha256:{}", "g".repeat(64)))); // 非 16 進
        assert!(!is_sha256_digest(&"a".repeat(64))); // prefix 無し
    }
}
