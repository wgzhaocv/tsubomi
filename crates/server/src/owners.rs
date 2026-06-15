//! owner 管理(env→DB、web で 2 人目の owner を増減)。design v2 §7:最多 2 名の対等 owner、
//! 互いに外せるが自分は外せない(最低 1 名)、外された人へメール通知、初期 owner は env 種。
//! web と CLI は同一ハンドラの 2 入口だが、owner ガバナンスは **web 専用**
//! (`require_owner_web` が Bearer cli_token を拒否)。
//!
//! **真相は `users.role`**(毎リクエスト JOIN で読む授権の唯一の源)。本モジュールが持つ
//! `platform_config['owner_roster']`(≤2 個の email)は role が表せない 2 つの穴だけを埋める:
//!   ① 未ログイン email の「ログインしたら昇格すべき」意図(`auth::google` の補昇が引く)。
//!   ② 「全 owner へ」の宛先(`gc` の磁盘告警)。
//! roster は role を**反推しない**(対账しない)— 授権は常に role を見て、roster は穴を埋めるだけ。
//!
//! 増減は **トランザクション + `SELECT … FOR UPDATE`** で roster 行をロックしてから RMW +
//! 同トランザクションで `users.role` を変える。これが無いと「A が B を、B が A を同時に外して
//! owner が 0 になる」事故(不可逆)や上限 2 の並行すり抜けが起きる。Resend 通知は commit 後
//! (メールは巻き戻せない)。

use crate::auth::AuthCtx;
use crate::databases::{audit, audit_with_target};
use crate::error::{AppError, AppResult};
use crate::mail;
use crate::state::AppState;
use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::routing::{get, post};
use serde_json::{Value, json};
use sqlx::PgPool;
use tsubomi_shared::{AdminOwnerDto, OwnerEmailReq};
use uuid::Uuid;

/// platform_config のキー。値 = lowercase の email 配列(≤2)。
const OWNER_ROSTER_KEY: &str = "owner_roster";
/// 対等 owner の上限(design v2 §7)。
const MAX_OWNERS: usize = 2;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/admin/owners", get(list).post(add))
        // 削除は専用パス(email を body で受ける。path に email を載せない)。
        .route("/admin/owners/remove", post(remove))
}

/// roster を読む(未設定 = 空)。`auth::google` の補昇 / `gc` の宛先 / 本モジュールが共用。
pub(crate) async fn roster(db: &PgPool) -> Vec<String> {
    let v: Option<Value> = sqlx::query_scalar("SELECT value FROM platform_config WHERE key = $1")
        .bind(OWNER_ROSTER_KEY)
        .fetch_optional(db)
        .await
        .ok()
        .flatten();
    v.and_then(|v| serde_json::from_value::<Vec<String>>(v).ok())
        .unwrap_or_default()
}

/// 冷启动种:roster が無い or 空配列のとき env 種(config.owner_emails)で初期化する。
/// 「env を直して再起動」を owner 全滅時の破窗逃生口に残すため、**空のときも種を入れ直す**
/// (key 不存在だけを条件にすると、並行 bug で空になった roster を env から救えない)。
pub(crate) async fn seed_if_empty(db: &PgPool, seed: &[String]) -> AppResult<()> {
    if !roster(db).await.is_empty() {
        return Ok(());
    }
    if seed.is_empty() {
        // roster も env 種も空 = owner が一人も居ない。web で owner 操作ができず、新規 owner も
        // 作れない(bootstrap 不能)。boot は止めず大声で警告する(初期セットアップ中の起動は許す)。
        tracing::warn!(
            "owner が未設定です(owner_roster 空 + TSUBOMI_OWNER_EMAILS 空)。\
             env に owner を設定して再起動してください — でないと管理操作ができません"
        );
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO platform_config (key, value, updated_at) VALUES ($1, $2, now())
         ON CONFLICT (key) DO UPDATE SET value = $2, updated_at = now()",
    )
    .bind(OWNER_ROSTER_KEY)
    .bind(json!(seed))
    .execute(db)
    .await?;
    tracing::info!(count = seed.len(), "owner_roster を env 種で初期化");
    Ok(())
}

/// `GET /api/admin/owners`:roster + users join(真名 / 登録済みか / 自分か)。
pub async fn list(
    auth: AuthCtx,
    State(state): State<AppState>,
) -> AppResult<Json<Vec<AdminOwnerDto>>> {
    crate::admin::require_owner_web(&auth)?;
    list_json(&state, auth.user_id).await
}

/// `POST /api/admin/owners` {email}:roster に追加(≤2)。users 行があれば即 role=owner、
/// 無ければログイン時に補昇。FOR UPDATE で並行追加の取りこぼし / 上限すり抜けを防ぐ。
pub async fn add(
    auth: AuthCtx,
    State(state): State<AppState>,
    Json(req): Json<OwnerEmailReq>,
) -> AppResult<Json<Vec<AdminOwnerDto>>> {
    crate::admin::require_owner_web(&auth)?;
    let email = req.email.trim().to_lowercase();
    if email.is_empty() || email.len() > 254 {
        // 254 = RFC 5321 のアドレス上限。空 / 暴走入力を弾く(ドメイン判定の前に)。
        return Err(AppError::BadRequest("メールアドレスを入力してください".into()));
    }
    // owner は会社ドメインのみ(login の email ドメイン判定を共用)。外部ドメインを入れても
    // その人はそもそもログインできない = roster に永久に効かない脏項が残るだけなので弾く。
    if !crate::auth::google::email_domain_allowed(&email, &state.config.allowed_hds) {
        return Err(AppError::BadRequest(
            "会社ドメインのメールアドレスのみ管理者にできます".into(),
        ));
    }

    let mut tx = state.db.begin().await?;
    let mut list = roster_locked(&mut tx).await?;
    ensure_still_owner(&mut tx, auth.user_id).await?;
    if list.iter().any(|e| e == &email) {
        return Err(AppError::Conflict("既に管理者です".into()));
    }
    if list.len() >= MAX_OWNERS {
        return Err(AppError::BadRequest(format!(
            "管理者は最大 {MAX_OWNERS} 名です。先に誰かを外してください"
        )));
    }
    list.push(email.clone());
    write_roster(&mut tx, &list).await?;
    // users 行があれば即昇格。無ければ次回ログインで `auth::google` が roster を見て補昇する。
    sqlx::query(
        "UPDATE users SET role = 'owner', updated_at = now()
          WHERE lower(email) = $1 AND role <> 'owner'",
    )
    .bind(&email)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    audit_owner(&state, auth.user_id, "owner.add", &email).await;
    tracing::info!(actor = %auth.user_id, %email, "owner を追加");
    list_json(&state, auth.user_id).await
}

/// `POST /api/admin/owners/remove` {email}:roster から外し、users.role を user へ戻す。
/// 自分は外せない + 最低 1 名は残す(両条件とも FOR UPDATE 事務内で判定)。外した人へ Resend 通知。
pub async fn remove(
    auth: AuthCtx,
    State(state): State<AppState>,
    Json(req): Json<OwnerEmailReq>,
) -> AppResult<Json<Vec<AdminOwnerDto>>> {
    crate::admin::require_owner_web(&auth)?;
    let email = req.email.trim().to_lowercase();
    let me = actor_email(&state.db, auth.user_id).await;
    if me.as_deref() == Some(email.as_str()) {
        return Err(AppError::BadRequest(
            "自分自身は管理者から外せません(別の管理者に依頼してください)".into(),
        ));
    }

    let mut tx = state.db.begin().await?;
    let mut list = roster_locked(&mut tx).await?;
    ensure_still_owner(&mut tx, auth.user_id).await?;
    if !list.iter().any(|e| e == &email) {
        return Err(AppError::NotFound);
    }
    if list.len() <= 1 {
        return Err(AppError::BadRequest(
            "最後の管理者は外せません(最低 1 名必要です)".into(),
        ));
    }
    list.retain(|e| e != &email);
    write_roster(&mut tx, &list).await?;
    // 即座に降格(ログインの「只昇不降」とは別 — 除名は明示操作なのでここで role を戻す)。
    sqlx::query("UPDATE users SET role = 'user', updated_at = now() WHERE lower(email) = $1")
        .bind(&email)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    audit_owner(&state, auth.user_id, "owner.remove", &email).await;
    // 通知は commit 後(メールは巻き戻せない)。best-effort。
    let subject = "[tsubomi] 管理者権限が解除されました";
    let text = "あなたの tsubomi の管理者権限が解除されました。\n\
                心当たりがなければ、別の管理者にご確認ください。";
    let html = mail::render(mail::TPL_OWNER_REMOVE, &[]);
    if let Err(e) = mail::send(&state, std::slice::from_ref(&email), subject, &html, text).await {
        tracing::warn!(error = ?e, %email, "owner 解除通知メールの送信に失敗");
    }
    tracing::info!(actor = %auth.user_id, %email, "owner を解除");
    list_json(&state, auth.user_id).await
}

// ===== 内部ヘルパ =====

/// 操作者の email(lowercase)。is_current 判定と自分削除ガードに使う。
async fn actor_email(db: &PgPool, actor: Uuid) -> Option<String> {
    sqlx::query_scalar::<_, String>("SELECT lower(email) FROM users WHERE id = $1")
        .bind(actor)
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
}

/// トランザクション内で roster 行を FOR UPDATE ロックして読む(並行 RMW のロストアップデート防止)。
/// **行が無いと FOR UPDATE は何もロックしない**ので、まず空配列で確実に行を作ってからロックする
/// (種未投入 / 並行初回 add で上限 2 がすり抜けるのを防ぐ)。
async fn roster_locked(tx: &mut sqlx::PgConnection) -> AppResult<Vec<String>> {
    sqlx::query(
        "INSERT INTO platform_config (key, value, updated_at) VALUES ($1, '[]'::jsonb, now())
         ON CONFLICT (key) DO NOTHING",
    )
    .bind(OWNER_ROSTER_KEY)
    .execute(&mut *tx)
    .await?;
    let v: Option<Value> =
        sqlx::query_scalar("SELECT value FROM platform_config WHERE key = $1 FOR UPDATE")
            .bind(OWNER_ROSTER_KEY)
            .fetch_optional(&mut *tx)
            .await?;
    Ok(v.and_then(|v| serde_json::from_value::<Vec<String>>(v).ok())
        .unwrap_or_default())
}

/// 事務内で操作者がまだ owner かを再確認する(Codex HIGH)。`require_owner_web` は
/// リクエスト開始時の AuthCtx を見るだけなので、その後・本操作の前に並行降格された
/// owner がそのまま実行してしまう時序穴がある。roster ロックを保持中はすべての owner
/// 変更がこのロック待ちで直列化されるので、ロック取得後の素の role 再読で十分。降格済み = Forbidden。
async fn ensure_still_owner(tx: &mut sqlx::PgConnection, actor: Uuid) -> AppResult<()> {
    let role: Option<String> = sqlx::query_scalar("SELECT role::text FROM users WHERE id = $1")
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await?;
    if role.as_deref() == Some("owner") {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

/// roster を書き戻す(upsert)。
async fn write_roster(tx: &mut sqlx::PgConnection, list: &[String]) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO platform_config (key, value, updated_at) VALUES ($1, $2, now())
         ON CONFLICT (key) DO UPDATE SET value = $2, updated_at = now()",
    )
    .bind(OWNER_ROSTER_KEY)
    .bind(json!(list))
    .execute(&mut *tx)
    .await?;
    Ok(())
}

/// owner.add / owner.remove の監査。対象 email に users 行があれば target_user を埋める。
async fn audit_owner(state: &AppState, actor: Uuid, action: &str, email: &str) {
    let target = sqlx::query_scalar::<_, Uuid>("SELECT id FROM users WHERE lower(email) = $1")
        .bind(email)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();
    let detail = json!({ "email": email });
    match target {
        Some(uid) => audit_with_target(&state.db, actor, action, Uuid::nil(), uid, detail).await,
        None => audit(&state.db, Some(actor), action, Uuid::nil(), detail).await,
    }
}

/// 変更後の一覧を返す(add / remove のレスポンス = 最新 roster)。
async fn list_json(state: &AppState, actor: Uuid) -> AppResult<Json<Vec<AdminOwnerDto>>> {
    let me = actor_email(&state.db, actor).await;
    let mut out = Vec::new();
    for email in roster(&state.db).await {
        let row: Option<(Option<String>, String)> =
            sqlx::query_as("SELECT name, role::text FROM users WHERE lower(email) = $1")
                .bind(&email)
                .fetch_optional(&state.db)
                .await?;
        let (name, registered) = match row {
            Some((name, role)) => (name, role == "owner"),
            None => (None, false),
        };
        out.push(AdminOwnerDto {
            is_current: me.as_deref() == Some(email.as_str()),
            email,
            name,
            registered,
        });
    }
    Ok(Json(out))
}
