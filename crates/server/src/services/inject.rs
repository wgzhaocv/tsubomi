//! 注入の解決(S6、コンテナ起動の瞬間 — 決定 #5)。
//!
//! `injections` 表は **バインディング**だけを持ち、値はここでコンテナ create の直前に解決する。
//! 最終 env = `service_env`(復号した静的値)∪ injections を 1 件ずつ解決(database → 内部 app
//! role の接続文字列 / volume → bind マウント + パス env / cache → ACL ユーザ URL + 前缀 /
//! service → 別 app の内部直連 URL `http://<subdomain>:<port>`)。PORT は呼び出し側(deploy.rs)が足す。
//!
//! service 注入は **値の解決だけ** — 実際に届くかは `network.rs` の網リンク(注入元 B の稼働
//! コンテナを A の私網へ別名 attach)が担保する(env 文字列があっても網が無ければ繋がらない =
//! 別関心事)。詳細は `doc/paas-service-link-design.md`。
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
                        cfg.db_internal_host, cfg.db_internal_port, cfg.db_internal_sslmode
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
            "cache" => {
                // 内部入口の ACL ユーザ接続文字列 + key 前缀。失効(None)はスキップ。
                if let Some((acl_user, namespace, pass_enc)) =
                    fetch_cache_creds(state, resource_id).await?
                {
                    let pass = state.crypto.decrypt(&pass_enc)?;
                    let cfg = &state.config;
                    let url = format!(
                        "redis://{acl_user}:{pass}@{}:{}",
                        cfg.cache_internal_host, cfg.cache_internal_port
                    );
                    // REDIS_KEY_PREFIX も注入する(§11-C):app はクライアントの keyPrefix にこれを設定。
                    // ACL が `~<ns>:*` で兜底するので、前缀無しアクセスは NOPERM = fail-safe。
                    // 名前は env_var の `_URL` を `_KEY_PREFIX` に置換(無ければ付加)。値は常に `<ns>:`。
                    let prefix_env = key_prefix_env(&env_var);
                    env.push((env_var, url));
                    env.push((prefix_env, format!("{namespace}:")));
                }
            }
            "service" => {
                // 別 app の内部直連。失効(注入元 service が削除済み)→ None でスキップ。
                // `_URL` は subdomain を docker 網別名として引く `http://<subdomain>:<port>`
                // (http 固定 — 内部網なので TLS 無し。§9)。加えて素材の `_HOST` / `_PORT` も
                // 注入する:非 HTTP ソフト(自帯 postgres 等)には http テンプレが廃紙で、
                // 利用側が `postgres://user:pass@$X_HOST:$X_PORT/db` を自分のスキームで組める
                // ようにする(stateful 設計 §0-H)。名前は `_URL` を剥いだ基底に付ける(cache の
                // `key_prefix_env` と同型)。派生名が別注入と衝突したら dedup_env_last の後勝ち
                // (受容)。実到達は network.rs の網リンクが担保する。
                if let Some((subdomain, port)) = fetch_service_endpoint(state, resource_id).await? {
                    let url = format!("http://{subdomain}:{port}");
                    let base = host_port_base(&env_var);
                    env.push((format!("{base}_HOST"), subdomain));
                    env.push((format!("{base}_PORT"), port.to_string()));
                    env.push((env_var, url));
                }
            }
            // 未知 kind は無視。
            _ => {}
        }
    }

    Ok((env, binds))
}

/// REDIS_URL の env 名から REDIS_KEY_PREFIX の env 名を導く:末尾 `_URL` を `_KEY_PREFIX` に
/// 置換、無ければ `_KEY_PREFIX` を付加(REDIS_URL→REDIS_KEY_PREFIX / CACHE_URL→CACHE_KEY_PREFIX。§5)。
fn key_prefix_env(env_var: &str) -> String {
    format!("{}_KEY_PREFIX", host_port_base(env_var))
}

/// service 注入の `_HOST` / `_PORT` の名前基底:env 名の末尾 `_URL` を剥ぐ(無ければそのまま)。
/// `MYPG_URL` → `MYPG_HOST` / `MYPG_PORT`(stateful 設計 §0-H。`key_prefix_env` と共有)。
fn host_port_base(env_var: &str) -> &str {
    env_var.strip_suffix("_URL").unwrap_or(env_var)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_port_base_strips_url_suffix() {
        // 既定名(subdomain 由来の `<X>_URL`)→ 基底に剥がれる。
        assert_eq!(host_port_base("MYPG_URL"), "MYPG");
        assert_eq!(host_port_base("API_BACKEND_URL"), "API_BACKEND");
        // `_URL` で終わらないカスタム名はそのまま基底になる(`FOO` → `FOO_HOST`)。
        assert_eq!(host_port_base("FOO"), "FOO");
        // key_prefix_env も同じ基底導出を共有する(cache の既存規約と一貫)。
        assert_eq!(key_prefix_env("REDIS_URL"), "REDIS_KEY_PREFIX");
        assert_eq!(key_prefix_env("CACHE"), "CACHE_KEY_PREFIX");
    }
}

/// 注入元 cache の (acl_user, namespace, password_enc) を引く。
/// 削除済み(失効)/ cache でない → None。所有権は注入作成時に検証済みなので resource_id で引く。
async fn fetch_cache_creds(
    state: &AppState,
    resource_id: Uuid,
) -> AppResult<Option<(String, String, Vec<u8>)>> {
    let row: Option<(String, String, Vec<u8>)> = sqlx::query_as(
        "SELECT d.acl_user, d.namespace, d.password_enc
           FROM resources r
           JOIN cache_details d ON d.resource_id = r.id
          WHERE r.id = $1 AND r.kind = 'cache' AND r.deleted_at IS NULL",
    )
    .bind(resource_id)
    .fetch_optional(&state.db)
    .await?;
    Ok(row)
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

/// 注入元 service の (subdomain, container_port) を引く。削除済み(失効)/ service でない → None。
/// 所有権は注入作成時に検証済みなので resource_id で引く。値 = 内部直連 URL `http://<subdomain>:<port>`。
async fn fetch_service_endpoint(
    state: &AppState,
    resource_id: Uuid,
) -> AppResult<Option<(String, i32)>> {
    let row: Option<(String, i32)> = sqlx::query_as(
        "SELECT d.subdomain, d.container_port
           FROM resources r
           JOIN service_details d ON d.resource_id = r.id
          WHERE r.id = $1 AND r.kind = 'service' AND r.deleted_at IS NULL",
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
