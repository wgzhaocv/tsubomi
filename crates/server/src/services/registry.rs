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

use crate::error::{AppError, AppResult};
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
    let creds = load(state, user_id)
        .await?
        .ok_or_else(|| AppError::Other(anyhow::anyhow!("registry アカウントの作成に失敗")))?;
    // 新規アカウント経路(既存は冒頭で早期 return 済み)→ traefik の registry basicAuth を
    // 更新する(本番のみ実質動作。file provider がホットリロード)。
    sync_traefik(state).await;
    Ok(creds)
}

// ===== 本番 registry の push 入口(traefik basicAuth)=====

/// 本番(tls)で registry の公網 push 入口を traefik に出す:`registry.<domain>` → registry:5000、
/// basicAuth(全 registry_accounts を bcrypt した inline users)、LE。registry コンテナ自体は
/// **無認証**(ループバック :5000 のまま — 平台の pull はそのまま通る)。認証は traefik 層だけに付ける。
/// IP 許可リスト middleware は付けない(決定 #4:registry は免除)。dev(tls=false)は何もしない。
/// 起動時 + `ensure_account` の新規時に呼ぶ(traefik file provider がホットリロード、SIGHUP 不要)。
pub async fn sync_traefik(state: &AppState) {
    // prod(push 先が公網の別ホスト)でのみ入口を書く。dev / 単機無認証では何もしない。
    // TLS の有無(traefik 終端 / 上流終端)とは独立 — tunnel(tls=false)でも入口は要る。
    if !state.config.registry_ingress() {
        // tls=true なのに push==pull は本番の設定漏れ(REGISTRY_PUSH 未設定)。push 入口が
        // 書かれず CI の docker login が 404 で黙って割れるので警告する。
        if state.config.tls {
            tracing::warn!(
                "TSUBOMI_TLS=true だが REGISTRY_PUSH==PULL — registry push 入口を書きません。\
                 TSUBOMI_REGISTRY_PUSH=registry.<域名> を設定してください"
            );
        }
        return;
    }
    // best-effort:失敗しても account 行は DB にあり、起動時 / 次の create で再同期して収束する
    // (現実の registry.yml は file provider が即ホットリロード。並行 create は稀で自己修復する)。
    if let Err(e) = sync_traefik_inner(state).await {
        tracing::error!(error = ?e, "registry の traefik 入口同期に失敗 — 次回 create / 再起動で収束");
    }
}

async fn sync_traefik_inner(state: &AppState) -> AppResult<()> {
    // 全アカウントを復号 → bcrypt(basicAuth は一方向。GitHub Secret 用の平文ラインとは別物)。
    // ※ bcrypt(cost 12)は 1 件 ≈数百 ms。アカウント数 N に対し毎回 N 回(create / 起動ごと)。
    //   社内少数ユーザでは許容。ユーザが増えたら hash を DB にキャッシュして差分だけ算出する。
    let rows: Vec<(String, Vec<u8>)> =
        sqlx::query_as("SELECT username, password_enc FROM registry_accounts")
            .fetch_all(&state.db)
            .await?;
    let mut users: Vec<String> = Vec::with_capacity(rows.len());
    for (user, pass_enc) in rows {
        let pass = state.crypto.decrypt(&pass_enc)?;
        let hash = bcrypt::hash(&pass, bcrypt::DEFAULT_COST)
            .map_err(|e| AppError::Other(anyhow::anyhow!("bcrypt に失敗: {e}")))?;
        users.push(format!("{user}:{hash}"));
    }

    let target = state.config.traefik_dynamic_dir.join("registry.yml");
    let doc = render(state.config.registry_host(), &users, state.config.tls);
    crate::services::route::write_atomic(&target, &doc)?;
    tracing::info!(accounts = users.len(), "registry の traefik 入口を同期した");
    Ok(())
}

/// traefik 動的設定(registry router + basicAuth middleware + service)を組み立てる。
/// `push_host` = 公網の push ホスト(`registry_push`、例 registry.<域名>)。`tls`=true なら traefik 終端
/// (websecure + LE)、false なら上流終端(web、HTTP。CF Tunnel / 逆代理の後ろ)。
/// bcrypt ハッシュは `$`/`.`/`/` のみ(引用符・バックスラッシュ無し)なので二重引用符で安全に包める。
/// file provider なので compose の `$$` 二重化は不要。
/// **users 空(アカウント未作成)→ router を書かない**:push 入口は 404 = push 不可(fail-closed)。
/// 空の basicAuth `users` が traefik で allow-all に倒れて push 入口が開く事故を避ける。
fn render(push_host: &str, users: &[String], tls: bool) -> String {
    use crate::services::route::{entrypoint, push_tls_block};
    let mut s = String::new();
    s.push_str("# 平台が自動生成(services/registry.rs)。手で編集しない。\n");
    if users.is_empty() {
        s.push_str("# (registry アカウント未作成 — push 入口は未公開 = fail-closed)\n");
        return s;
    }
    s.push_str("http:\n");
    s.push_str("  routers:\n");
    s.push_str("    tsubomi-registry:\n");
    s.push_str(&format!("      rule: \"Host(`{push_host}`)\"\n"));
    s.push_str(&format!("      entryPoints: [\"{}\"]\n", entrypoint(tls)));
    s.push_str("      service: \"tsubomi-registry\"\n");
    s.push_str("      middlewares: [\"tsubomi-registry-auth@file\"]\n");
    push_tls_block(&mut s, tls);
    s.push_str("  middlewares:\n");
    s.push_str("    tsubomi-registry-auth:\n");
    s.push_str("      basicAuth:\n");
    s.push_str("        users:\n");
    for u in users {
        s.push_str(&format!("          - \"{u}\"\n"));
    }
    s.push_str("  services:\n");
    s.push_str("    tsubomi-registry:\n");
    s.push_str("      loadBalancer:\n");
    s.push_str("        servers:\n");
    s.push_str("          - url: \"http://tsubomi-registry:5000\"\n");
    s
}

#[cfg(test)]
mod tests {
    use super::render;

    const USER: &str = "u-abc:$2b$12$hashhashhash";

    #[test]
    fn render_tls_uses_websecure_and_le() {
        // 直 VPS(tls=true):websecure + certResolver。Host は push ホストをそのまま。
        let doc = render("registry.example.com", &[USER.to_string()], true);
        assert!(doc.contains("Host(`registry.example.com`)"));
        assert!(doc.contains("entryPoints: [\"websecure\"]"));
        assert!(doc.contains("certResolver: le"));
        assert!(doc.contains("tsubomi-registry-auth@file"));
        assert!(doc.contains(USER));
        assert!(doc.contains("http://tsubomi-registry:5000"));
    }

    #[test]
    fn render_no_tls_uses_web_and_no_certresolver() {
        // 上流終端(tls=false。CF Tunnel/逆代理):web エントリ・tls ブロック無し。basicAuth は付ける。
        let doc = render("registry.example.com", &[USER.to_string()], false);
        assert!(doc.contains("entryPoints: [\"web\"]"));
        assert!(!doc.contains("certResolver"));
        assert!(!doc.contains("tls:"));
        assert!(doc.contains("tsubomi-registry-auth@file"));
        assert!(doc.contains(USER));
    }

    #[test]
    fn render_empty_is_fail_closed() {
        // アカウント 0 → router も basicAuth も書かない(push 入口は 404 = push 不可)。tls 不問。
        let doc = render("registry.example.com", &[], false);
        assert!(!doc.contains("routers"));
        assert!(!doc.contains("basicAuth"));
        assert!(!doc.contains("Host("));
    }
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
