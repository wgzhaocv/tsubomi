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

/// 公開 DB(Postgres)を Traefik の TCP 入口経由にする時の入口名。**`compose.prod.db-public.yml`
/// の `--entrypoints.postgres.address` と一致させること**(compose ↔ コードの契約。route.rs の
/// ENTRYPOINT_HTTP/TLS と同じ約束)。
const POSTGRES_ENTRYPOINT: &str = "postgres";

/// 公開 DB の TCP ipAllowList middleware 名(HTTP の TRAEFIK_MIDDLEWARE とは別系統 = `tcp:` 配下)。
const TRAEFIK_TCP_MIDDLEWARE: &str = "tsubomi-pg-ipallow";

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
        auth.client_ip.as_deref(),
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
        auth.client_ip.as_deref(),
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}

// ===== traefik への収束 =====

/// 現在の許可リストを読んで traefik の動的設定を原子的に書き直す:HTTP の `ipallow.yml`(常時)+
/// 公開 DB 有効時の TCP `db-tcp.yml`(無効なら削除)。起動時(main)と各変更後に呼ぶ。best-effort:
/// 失敗してもリクエストは止めない(DB が真実源。起動時同期や次回変更で収束する)。
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
    let db_tcp = dir.join("db-tcp.yml");
    // 公開 DB を開くべきか。公開 DB は **fail-closed**:許可リストが空なら入口を書かない
    // (空=全開で DB を公網に晒す事故を防ぐ。HTTP service は公網 app が本意なので空=fail-open のままだが、
    // DB は別ポリシー)。off / 空 = 閉じる。
    let open_db = state.config.db_public_enabled && !cidrs.is_empty();

    // 【順序が安全境界】公開 DB を**閉じる**べき状態(off / 許可リスト空)なら、HTTP 書き込みの成否に
    // **依存せず先に** db-tcp.yml を消す。HTTP を先に書いてその失敗で早期 return すると、古い fail-open
    // の db-tcp.yml(0.0.0.0/0)が消し残り DB を晒し続ける穴があった(codex 指摘)。閉じるを先頭に置く。
    if !open_db {
        if state.config.db_public_enabled {
            tracing::warn!(
                "公開 DB が有効だが IP 許可リストが空 — fail-closed で TCP 入口を書きません(DB を公網に晒さない)。会社 CIDR を 1 件以上登録してください"
            );
        }
        match tokio::fs::remove_file(&db_tcp).await {
            Ok(()) => {
                tracing::info!("db-tcp.yml を削除(公開 DB 無効、または許可リスト空で fail-closed)")
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }

    // HTTP の IP 許可リスト(常時)。一時ファイルへ書いて atomic rename(traefik が中途半端を読まない)。
    let target = dir.join("ipallow.yml");
    let tmp = dir.join(".ipallow.yml.tmp");
    tokio::fs::write(&tmp, render_yaml(&cidrs)).await?;
    tokio::fs::rename(&tmp, &target).await?;
    tracing::info!(count = cidrs.len(), "IP 許可リストを traefik へ同期した");

    // 公開 DB を**開く**場合だけ最後に TCP 入口を書く(Postgres も Traefik の TCP 入口経由にし同じ
    // 許可リストを流用。HTTP は pgbouncer 直結を通らないので別途 tcp: で被せる)。HTTP 書き込みが失敗して
    // 早期 return しても、その時は DB 入口を書かない = fail-closed 側に倒れる(安全)。
    if open_db {
        let backend = format!(
            "{}:{}",
            state.config.db_internal_host, state.config.db_internal_port
        );
        let tmp = dir.join(".db-tcp.yml.tmp");
        tokio::fs::write(&tmp, render_db_tcp_yaml(&cidrs, &backend)).await?;
        tokio::fs::rename(&tmp, &db_tcp).await?;
        tracing::info!(
            count = cidrs.len(),
            "公開 DB(TCP)の IP 許可リストを traefik へ同期した"
        );
    }
    Ok(())
}

/// 許可リストを traefik の sourceRange へ。空 = fail-open(`0.0.0.0/0` + `::/0` で全 v4/v6 許可)。
/// HTTP(`render_yaml`)と TCP(`render_db_tcp_yaml`)の両描画が共有する **fail-open ポリシーの唯一の真実**
/// (片方だけ変えてズレるのを防ぐ)。CIDR は normalize_cidr を通った値だけなので安全。
fn fail_open_ranges(cidrs: &[String]) -> Vec<&str> {
    if cidrs.is_empty() {
        vec!["0.0.0.0/0", "::/0"]
    } else {
        cidrs.iter().map(String::as_str).collect()
    }
}

/// ipAllowList が実 client IP を `X-Forwarded-For` から選ぶ際に**飛ばす**内部/プロキシ網
/// (cloudflared・docker・loopback 等)。これらを除いた残りが実 client。CF Tunnel 配下では
/// CF の Transform Rule が XFF を `ip.src` で**上書き**(偽装不可)するので、ここに残るのは
/// 実 client(公網)1 個になる。internal app は XFF を持たないが、ipStrategy を付けるのは
/// 許可リスト非空時のみ(下記)なので影響しない。
const TRUSTED_PROXY_NETS: &[&str] = &[
    "127.0.0.0/8",
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "::1/128",
    "fc00::/7",
];

/// traefik 動的設定(YAML)を組み立てる。空リスト = fail-open(全 IP 許可)。
/// CIDR は normalize_cidr を通った値だけなので安全(それでも引用符で包む)。
fn render_yaml(cidrs: &[String]) -> String {
    // middleware は常に定義する(ルータが参照する name が未定義だと traefik がそのルートを弾くため)。
    let ranges = fail_open_ranges(cidrs);

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
    // 許可リストが**非空のときだけ** ipStrategy を付ける。CF Tunnel 配下では cloudflared が直接の
    // peer(loopback/docker)なので、既定(remote addr 判定)だと全員その内部 IP に見えて白名単が効かない。
    // XFF(CF が ip.src で上書き)から内部ホップを除いて実 client を選ぶ。**空(fail-open)では付けない**
    // — XFF が内部 IP だけ/不在のとき excludedIPs が空を返し 403 になる罠(traefik#10561)を避け、
    // fail-open(全許可・無条件マッチ)の不変条件を守るため。
    if !cidrs.is_empty() {
        s.push_str("        ipStrategy:\n");
        s.push_str("          excludedIPs:\n");
        for net in TRUSTED_PROXY_NETS {
            s.push_str(&format!("            - \"{net}\"\n"));
        }
    }
    s
}

/// 公開 DB(Postgres)用の traefik **TCP** 動的設定(YAML)。HTTP と同じ会社 IP 許可リストを
/// TCP の ipAllowList として流用する。**呼び出し側(sync)は許可リストが空なら本関数を呼ばない**
/// = 公開 DB は fail-closed(DB を空リストで 0.0.0.0/0 に晒さない。HTTP service の空=fail-open とは別)。
/// pgbouncer が client TLS を終端するので Traefik は **素の TCP passthrough**(`HostSNI(*)`・TLS 無し)=
/// client の `sslmode=require` は pgbouncer と端到端で TLS を張る。`backend` = 内部 pgbouncer の
/// `host:port`(db_internal_host:db_internal_port。値は平台生成なのでそのまま埋めて安全)。
fn render_db_tcp_yaml(cidrs: &[String], backend: &str) -> String {
    let ranges = fail_open_ranges(cidrs);

    let mut s = String::new();
    s.push_str("# 平台(tsubomi-server)が自動生成。手で編集しない —\n");
    s.push_str(
        "# 公開 DB(TSUBOMI_DB_PUBLIC_ENABLED)有効時のみ書かれ、IP 許可リスト変更で上書きされる。\n",
    );
    s.push_str(
        "# Postgres を Traefik の TCP 入口(postgres)経由にし、HTTP と同じ許可リストを流用する。\n",
    );
    s.push_str("# 公開 DB は fail-closed:空許可リストでは sync がこのファイルを書かない(1 件以上の CIDR だけ許可)。\n");
    s.push_str("tcp:\n");
    s.push_str("  routers:\n");
    s.push_str("    tsubomi-postgres:\n");
    s.push_str(&format!("      entryPoints: [\"{POSTGRES_ENTRYPOINT}\"]\n"));
    // HostSNI(`*`) = 非 TLS の素 TCP を全マッチ(pgbouncer が TLS 終端 = Traefik は passthrough)。
    s.push_str("      rule: \"HostSNI(`*`)\"\n");
    s.push_str("      service: \"tsubomi-postgres\"\n");
    s.push_str(&format!(
        "      middlewares: [\"{TRAEFIK_TCP_MIDDLEWARE}@file\"]\n"
    ));
    s.push_str("  middlewares:\n");
    s.push_str(&format!("    {TRAEFIK_TCP_MIDDLEWARE}:\n"));
    s.push_str("      ipAllowList:\n");
    s.push_str("        sourceRange:\n");
    for c in ranges {
        s.push_str(&format!("          - \"{c}\"\n"));
    }
    s.push_str("  services:\n");
    s.push_str("    tsubomi-postgres:\n");
    s.push_str("      loadBalancer:\n");
    s.push_str("        servers:\n");
    s.push_str(&format!("          - address: \"{backend}\"\n"));
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
        // fail-open では ipStrategy を付けない(XFF 不在で空 IP→403 の罠を避ける)。
        assert!(!yaml.contains("ipStrategy"));
    }

    #[test]
    fn nonempty_list_only_lists_given_ranges() {
        let yaml = render_yaml(&["203.0.113.0/24".to_string()]);
        assert!(yaml.contains("203.0.113.0/24"));
        assert!(!yaml.contains("0.0.0.0/0"));
        // 非空時は ipStrategy.excludedIPs で XFF から実 client を選ぶ(内部ホップ除外)。
        assert!(yaml.contains("ipStrategy"));
        assert!(yaml.contains("excludedIPs"));
        assert!(yaml.contains("127.0.0.0/8"));
    }

    #[test]
    fn db_tcp_render_is_passthrough_and_fail_open() {
        let yaml = render_db_tcp_yaml(&[], "tsubomi-pgbouncer:6432");
        // TCP 入口 + 素 passthrough + backend + middleware を含む。
        assert!(yaml.contains("tcp:"));
        assert!(yaml.contains("HostSNI(`*`)"));
        assert!(yaml.contains(POSTGRES_ENTRYPOINT));
        assert!(yaml.contains(TRAEFIK_TCP_MIDDLEWARE));
        assert!(yaml.contains("tsubomi-pgbouncer:6432"));
        // 空リスト = fail-open。
        assert!(yaml.contains("0.0.0.0/0"));
        assert!(yaml.contains("::/0"));
    }

    #[test]
    fn db_tcp_render_restricts_to_given_cidrs() {
        let yaml = render_db_tcp_yaml(&["10.0.0.0/8".to_string()], "tsubomi-pgbouncer:6432");
        assert!(yaml.contains("10.0.0.0/8"));
        assert!(!yaml.contains("0.0.0.0/0"));
    }
}
