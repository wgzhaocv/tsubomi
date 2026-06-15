//! 注入の解決(S6、コンテナ起動の瞬間 — 決定 #5)。
//!
//! `injections` 表は **バインディング**だけを持ち、値はここでコンテナ create の直前に解決する。
//! 最終 env = `service_env`(復号した静的値)∪ injections を 1 件ずつ解決(database → 内部 app
//! role の接続文字列 / volume → bind マウント + パス env)。PORT は呼び出し側(deploy.rs)が足す。
//!
//! 失効(注入元がソフト削除済み)→ その 1 件は**空に解決**(env に出さない / bind を張らない)。
//! service は普通に起動する(§7.1)。復元すれば次の deploy で自動的に生き返る。
//!
//! database は **app role**(human ではない)を内部入口 `tsubomi-pgbouncer` 経由で注入する
//! (§7.2):外部 key の rotate が走る service を切らない。コンテナは edge 網のみで社外に出ない。

use crate::error::AppResult;
use crate::state::AppState;
use uuid::Uuid;

/// service の最終 env(PORT を除く)と volume bind を解決する。
/// 返り値 `(env, binds)`:env = `(KEY, VALUE)` の列、binds = `"<host_path>:<mount_path>"` の列。
pub async fn resolve(
    state: &AppState,
    service_id: Uuid,
) -> AppResult<(Vec<(String, String)>, Vec<String>)> {
    let mut env: Vec<(String, String)> = Vec::new();
    let mut binds: Vec<String> = Vec::new();

    // 1. 静的 env(復号)。
    let static_env: Vec<(String, Vec<u8>)> =
        sqlx::query_as("SELECT key, value_enc FROM service_env WHERE service_id = $1")
            .bind(service_id)
            .fetch_all(&state.db)
            .await?;
    for (key, value_enc) in static_env {
        env.push((key, state.crypto.decrypt(&value_enc)?));
    }

    // 2. 注入(バインディング)を 1 件ずつ解決。失効(資源が削除済み)は空に解決してスキップ。
    let injections: Vec<(Uuid, String, String, Option<String>)> = sqlx::query_as(
        "SELECT r.id, r.kind, i.env_var, i.mount_path
           FROM injections i JOIN resources r ON r.id = i.resource_id
          WHERE i.service_id = $1
          ORDER BY i.env_var",
    )
    .bind(service_id)
    .fetch_all(&state.db)
    .await?;

    for (resource_id, kind, env_var, mount_path) in injections {
        match kind.as_str() {
            "database" => {
                // app role(内部)の接続文字列。失効(None)はスキップ。
                if let Some((dbname, role, pass_enc)) = fetch_app_role(state, resource_id).await? {
                    let pass = state.crypto.decrypt(&pass_enc)?;
                    let cfg = &state.config;
                    let url = format!(
                        "postgres://{role}:{pass}@{}:{}/{dbname}?sslmode={}",
                        cfg.db_internal_host, cfg.db_internal_port, cfg.db_sslmode
                    );
                    env.push((env_var, url));
                }
            }
            "volume" => {
                // host_path を mount_path に bind し、env に mount_path を入れる。失効はスキップ。
                // mount_path は注入作成時に必ず入る(create_injection が既定を確定)。万一 None なら
                // データ不整合 — ここで別の既定を捏造せずスキップする(既定の単一真源は create 側)。
                if let (Some(host_path), Some(mount)) = (
                    fetch_volume_host_path(state, resource_id).await?,
                    mount_path,
                ) {
                    // bind 元(host 側)が無ければ作る(volume 作成時に在るはずだが念のため)。
                    let _ = std::fs::create_dir_all(&host_path);
                    binds.push(format!("{host_path}:{mount}"));
                    env.push((env_var, mount));
                }
            }
            // cache(REDIS_URL)は M5。未知 kind は無視。
            _ => {}
        }
    }

    Ok((env, binds))
}

/// 注入元 database の app role を引く(pg_dbname, pg_role, password_enc)。
/// 削除済み(失効)/ database でない → None。所有権は注入作成時に検証済みなので resource_id で引く。
async fn fetch_app_role(
    state: &AppState,
    resource_id: Uuid,
) -> AppResult<Option<(String, String, Vec<u8>)>> {
    let row: Option<(String, String, Vec<u8>)> = sqlx::query_as(
        "SELECT d.pg_dbname, ro.pg_role, ro.password_enc
           FROM resources r
           JOIN database_details d ON d.resource_id = r.id
           JOIN database_roles ro ON ro.resource_id = r.id AND ro.role_kind = 'app'
          WHERE r.id = $1 AND r.kind = 'database' AND r.deleted_at IS NULL",
    )
    .bind(resource_id)
    .fetch_optional(&state.db)
    .await?;
    Ok(row)
}

/// 注入元 volume の host_path を引く。削除済み(失効)/ volume でない → None。
async fn fetch_volume_host_path(state: &AppState, resource_id: Uuid) -> AppResult<Option<String>> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT d.host_path
           FROM resources r
           JOIN volume_details d ON d.resource_id = r.id
          WHERE r.id = $1 AND r.kind = 'volume' AND r.deleted_at IS NULL",
    )
    .bind(resource_id)
    .fetch_optional(&state.db)
    .await?;
    Ok(row.map(|(p,)| p))
}
