//! per-user の registry 資格情報(`ensure_account`)。
//!
//! ユーザ app のイメージ push 先 registry のアカウントを **ユーザ単位で 1 つ**持つ
//! (per-service ではない — digest ピン留めで per-repo ACL 不要。決定 #3 / §11-D)。
//! service create のたびに同じ creds を返すので、同じユーザの複数 service が同じ
//! GitHub Secret を共有できる(冪等)。
//!
//! 平台は password の **原文**を GitHub Secret 用に返す必要があるので、復元可能に
//! 暗号化して持つ(crypto.rs。ハッシュにできる session / cli_token とは別)。
//!
//! registry の htpasswd ファイルへの同期(bcrypt 行の追記 + registry への SIGHUP
//! リロード)は **prod-infra スライス**で足す:認証付き registry が立ってから実機
//! 検証する(dev の registry は認証なし)。本モジュールはアカウントの永続化と creds
//! 返却までを担う。

use crate::error::AppResult;
use crate::state::AppState;
use tsubomi_shared::{RegistryCreds, random_b64};
use uuid::Uuid;

/// registry password の乱数バイト数(base64url で ≈32 字)。
const PASSWORD_BYTES: usize = 24;

/// ユーザの registry アカウントを取得、無ければ作る(冪等)。返すのは host を含む
/// 完全な creds(password は平文)。同時 create にも強い:`ON CONFLICT DO NOTHING`
/// で 2 重挿入を避け、最後に確定行を読み直してから復号する。
pub async fn ensure_account(state: &AppState, user_id: Uuid) -> AppResult<RegistryCreds> {
    if let Some(creds) = load(state, user_id).await? {
        return Ok(creds);
    }

    // username は user_id 由来で安定 & 衝突しない。password は乱数 → 暗号化して保存。
    let username = format!("u-{}", user_id.simple());
    let password = random_b64(PASSWORD_BYTES);
    let password_enc = state.crypto.encrypt(&password)?;
    sqlx::query(
        "INSERT INTO registry_accounts (user_id, username, password_enc)
              VALUES ($1, $2, $3) ON CONFLICT (user_id) DO NOTHING",
    )
    .bind(user_id)
    .bind(&username)
    .bind(&password_enc)
    .execute(&state.db)
    .await?;

    // 自分が挿入したか、同時実行が先んじたかに依らず確定値を読み直す
    // (DO NOTHING で自分の INSERT が無視された場合でも正しい creds を返す)。
    load(state, user_id)
        .await?
        .ok_or_else(|| crate::error::AppError::Other(anyhow::anyhow!("registry アカウントの作成に失敗")))
}

/// 既存アカウントを読んで復号する(無ければ None)。
async fn load(state: &AppState, user_id: Uuid) -> AppResult<Option<RegistryCreds>> {
    let row: Option<(String, Vec<u8>)> =
        sqlx::query_as("SELECT username, password_enc FROM registry_accounts WHERE user_id = $1")
            .bind(user_id)
            .fetch_optional(&state.db)
            .await?;
    match row {
        Some((user, password_enc)) => {
            let pass = state.crypto.decrypt(&password_enc)?;
            Ok(Some(RegistryCreds {
                host: state.config.registry_push.clone(),
                user,
                pass,
            }))
        }
        None => Ok(None),
    }
}
