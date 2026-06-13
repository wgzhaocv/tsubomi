//! pg-tenant インスタンスとのやり取りを 1 箇所に集約する:
//! DDL(CREATE DATABASE / ROLE)、パスワード rotate、pg_dump / restore、
//! web SQL 用の human role 臨時接続。管制面 DB(pg-platform)はここには出てこない。
//!
//! 識別子は全て平台が生成し `is_safe_ident` で二重チェックする(DDL は識別子を
//! パラメータ化できないので文字列埋め込みになる)。パスワードは base64url
//! (英数字 + `-_`)で、SQL 字面量にも URL にもそのまま入る(`'` を含まない)。

use crate::error::{AppError, AppResult};
use anyhow::anyhow;
use sqlx::postgres::{PgConnectOptions, PgRow};
use sqlx::{Column, Connection, Executor, PgConnection, PgPool, Postgres, Row, TypeInfo};
use std::path::Path;
use std::process::Stdio;
use std::str::FromStr;
use tokio::process::Command;

/// 1 つの database に紐づく pg 名の組。app/human は登録資格情報、owner は両者が
/// 属する NOLOGIN グループ(DB + 全オブジェクトを所有 → どちらの role でも全権)。
pub struct DbNames {
    pub dbname: String,
    pub owner: String,
    pub app: String,
    pub human: String,
}

impl DbNames {
    /// 新規作成用にランダムな wire 名から導出する。
    pub fn generate() -> Self {
        Self::from_dbname(gen_dbname())
    }

    /// 既存の pg_dbname から導出(復元 / 物理削除で使う。owner は常に派生名)。
    pub fn from_dbname(dbname: String) -> Self {
        Self {
            owner: format!("{dbname}_owner"),
            app: format!("{dbname}_app"),
            human: format!("{dbname}_human"),
            dbname,
        }
    }

    fn all_idents(&self) -> [&str; 4] {
        [&self.dbname, &self.owner, &self.app, &self.human]
    }
}

/// pg 識別子として安全か(英小文字始まり、英小文字 / 数字 / `_` のみ、63 字以内)。
fn is_safe_ident(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && s.as_bytes()[0].is_ascii_lowercase()
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn check_idents(names: &DbNames) -> AppResult<()> {
    for id in names.all_idents() {
        if !is_safe_ident(id) {
            return Err(AppError::Other(anyhow!("生成された識別子が不正: {id}")));
        }
    }
    Ok(())
}

/// パスワードが SQL 字面量に安全か(base64url 由来なので `'` は本来含まれない。念のため)。
fn check_password(pw: &str) -> AppResult<()> {
    if pw.is_empty()
        || !pw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(AppError::Other(anyhow!("生成パスワードが不正")));
    }
    Ok(())
}

/// `db_<shortid>`:pg-tenant 内でグローバル一意な wire 名。
pub fn gen_dbname() -> String {
    format!("db_{}", short_id())
}

/// 英小文字始まりの英数字 12 文字。識別子規則を満たす。
fn short_id() -> String {
    use rand::Rng;
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut buf = [0u8; 12];
    rand::rng().fill_bytes(&mut buf);
    let mut s = String::with_capacity(12);
    s.push((b'a' + (buf[0] % 26)) as char); // 先頭は英字
    for &b in &buf[1..] {
        s.push(ALPHABET[(b as usize) % ALPHABET.len()] as char);
    }
    s
}

/// 32 文字の base64url パスワード(SQL 字面量にも URL にもそのまま入る)。
pub fn gen_password() -> String {
    tsubomi_shared::random_b64(24)
}

// ===== 接続情報のパース(pg_dump / psql 用)=====

struct ConnParts {
    host: String,
    port: String,
    user: String,
    password: String,
}

fn conn_parts(admin_url: &str) -> AppResult<ConnParts> {
    let u = url::Url::parse(admin_url)
        .map_err(|e| AppError::Other(anyhow!("TENANT_ADMIN_URL のパースに失敗: {e}")))?;
    // url の username()/password() は percent-encoded のまま返るので復号する。
    let decode = |s: &str| {
        percent_encoding::percent_decode_str(s)
            .decode_utf8()
            .map(|c| c.into_owned())
            .unwrap_or_else(|_| s.to_string())
    };
    Ok(ConnParts {
        host: u.host_str().unwrap_or("localhost").to_string(),
        port: u.port().unwrap_or(5432).to_string(),
        user: if u.username().is_empty() {
            "postgres".to_string()
        } else {
            decode(u.username())
        },
        password: u.password().map(decode).unwrap_or_default(),
    })
}

// ===== admin 接続(特定 DB へ)=====

/// admin 資格情報で特定の DB に臨時接続する(schema GRANT 用など)。
async fn connect_admin_db(admin_url: &str, dbname: &str) -> AppResult<PgConnection> {
    let opts = PgConnectOptions::from_str(admin_url)?.database(dbname);
    Ok(PgConnection::connect_with(&opts).await?)
}

/// **その DB 自身の human role** で臨時接続する(web SQL 専用。admin は使わない)。
/// host / port / sslmode は admin_url から流用し、user/pass/db を差し替える。
pub async fn connect_as_human(
    admin_url: &str,
    role: &str,
    password: &str,
    dbname: &str,
) -> AppResult<PgConnection> {
    let opts = PgConnectOptions::from_str(admin_url)?
        .username(role)
        .password(password)
        .database(dbname);
    Ok(PgConnection::connect_with(&opts).await?)
}

// ===== DDL =====

/// 動的 DDL を実行する小ヘルパ。SQL を所有 String として受け取り、await を跨いで
/// 生存させる(`format!(..).as_str()` の一時値は await を跨げない E0716 を回避)。
/// 識別子は呼び出し側で検証済み、パスワードは英数字 — それが「SQL safe」の根拠。
async fn exec<'e, E>(executor: E, sql: String) -> AppResult<()>
where
    E: Executor<'e, Database = Postgres>,
{
    executor.execute(sqlx::AssertSqlSafe(sql)).await?;
    Ok(())
}

/// owner グループ + DB + app/human role + 隔離 GRANT を作る。
/// `CREATE DATABASE` はトランザクション不可なので各文を個別に実行する。
pub async fn create_database(
    pool: &PgPool,
    admin_url: &str,
    names: &DbNames,
    app_pw: &str,
    human_pw: &str,
) -> AppResult<()> {
    check_idents(names)?;
    check_password(app_pw)?;
    check_password(human_pw)?;

    let DbNames {
        dbname,
        owner,
        app,
        human,
    } = names;

    exec(pool, format!("CREATE ROLE {owner} NOLOGIN")).await?;
    exec(pool, format!("CREATE DATABASE {dbname} OWNER {owner}")).await?;
    // 跨库隔离:他のテナント role が PUBLIC 経由で連がれないように。
    exec(
        pool,
        format!("REVOKE CONNECT ON DATABASE {dbname} FROM PUBLIC"),
    )
    .await?;
    exec(
        pool,
        format!(
            "CREATE ROLE {app} LOGIN PASSWORD '{app_pw}' NOSUPERUSER NOCREATEDB INHERIT IN ROLE {owner} CONNECTION LIMIT 20"
        ),
    )
    .await?;
    exec(
        pool,
        format!(
            "CREATE ROLE {human} LOGIN PASSWORD '{human_pw}' NOSUPERUSER NOCREATEDB INHERIT IN ROLE {owner} CONNECTION LIMIT 20"
        ),
    )
    .await?;
    exec(
        pool,
        format!("GRANT CONNECT ON DATABASE {dbname} TO {owner}"),
    )
    .await?;
    // app/human が作るオブジェクトを owner 所有にし、所有権の割れを防ぐ。
    exec(pool, format!("ALTER ROLE {app} SET ROLE {owner}")).await?;
    exec(pool, format!("ALTER ROLE {human} SET ROLE {owner}")).await?;

    grant_schema(admin_url, names).await
}

/// 新 DB に接続して public スキーマの権限を owner に集約する。
async fn grant_schema(admin_url: &str, names: &DbNames) -> AppResult<()> {
    let mut conn = connect_admin_db(admin_url, &names.dbname).await?;
    (&mut conn)
        .execute("REVOKE ALL ON SCHEMA public FROM PUBLIC")
        .await?;
    exec(
        &mut conn,
        format!("GRANT ALL ON SCHEMA public TO {}", names.owner),
    )
    .await?;
    conn.close().await?;
    Ok(())
}

/// human(または app)role のパスワードを差し替える。新接続から即時有効。
pub async fn rotate_password(pool: &PgPool, role: &str, new_pw: &str) -> AppResult<()> {
    if !is_safe_ident(role) {
        return Err(AppError::Other(anyhow!("role 識別子が不正: {role}")));
    }
    check_password(new_pw)?;
    exec(pool, format!("ALTER ROLE {role} PASSWORD '{new_pw}'")).await
}

/// DATABASE を強制削除(接続中でも切る。pg13+ の WITH FORCE)。role は残す。
pub async fn drop_database(pool: &PgPool, dbname: &str) -> AppResult<()> {
    if !is_safe_ident(dbname) {
        return Err(AppError::Other(anyhow!("dbname 識別子が不正: {dbname}")));
    }
    exec(
        pool,
        format!("DROP DATABASE IF EXISTS {dbname} WITH (FORCE)"),
    )
    .await
}

/// 物理削除:DATABASE(残っていれば)+ role 3 つを落とす。
pub async fn drop_database_and_roles(pool: &PgPool, names: &DbNames) -> AppResult<()> {
    check_idents(names)?;
    drop_database(pool, &names.dbname).await?;
    for role in [&names.app, &names.human, &names.owner] {
        exec(pool, format!("DROP ROLE IF EXISTS {role}")).await?;
    }
    Ok(())
}

/// 復元用に DATABASE を再作成(role は残っている前提)+ schema GRANT。
pub async fn recreate_for_restore(
    pool: &PgPool,
    admin_url: &str,
    names: &DbNames,
) -> AppResult<()> {
    check_idents(names)?;
    let DbNames { dbname, owner, .. } = names;
    exec(pool, format!("CREATE DATABASE {dbname} OWNER {owner}")).await?;
    exec(
        pool,
        format!("REVOKE CONNECT ON DATABASE {dbname} FROM PUBLIC"),
    )
    .await?;
    exec(
        pool,
        format!("GRANT CONNECT ON DATABASE {dbname} TO {owner}"),
    )
    .await?;
    grant_schema(admin_url, names).await
}

// ===== pg_dump / restore(TCP 直結。docker exec ではない)=====

/// DB を無圧縮 SQL で dump_path に書き出す(`--no-owner --no-privileges`)。
pub async fn dump_database(admin_url: &str, dbname: &str, dump_path: &Path) -> AppResult<()> {
    let c = conn_parts(admin_url)?;
    if let Some(parent) = dump_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::File::create(dump_path)?;
    // stdout を file へ。`.output()` は stdout を内部 pipe に差し替えてしまい file が
    // 空になるので、spawn + wait_with_output を使う(stderr だけ pipe で捕まえる)。
    let out = Command::new("pg_dump")
        .args([
            "-h",
            &c.host,
            "-p",
            &c.port,
            "-U",
            &c.user,
            "-d",
            dbname,
            "--no-owner",
            "--no-privileges",
        ])
        .env("PGPASSWORD", &c.password)
        .stdout(Stdio::from(file))
        .stderr(Stdio::piped())
        .spawn()?
        .wait_with_output()
        .await?;
    if !out.status.success() {
        return Err(AppError::Other(anyhow!(
            "pg_dump 失敗: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(())
}

/// 接続 URI が指す DB をそのまま dump する(pg-platform の全量バックアップ用)。
/// パスワードは PGPASSWORD で渡し argv には載せない(`ps` / /proc に出さない)。
pub async fn dump_url(conn_url: &str, dump_path: &Path) -> AppResult<()> {
    let c = conn_parts(conn_url)?;
    // URI の path 部分が DB 名(無ければ postgres)。
    let dbname = url::Url::parse(conn_url)
        .ok()
        .map(|u| u.path().trim_start_matches('/').to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "postgres".to_string());
    if let Some(parent) = dump_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::File::create(dump_path)?;
    let out = Command::new("pg_dump")
        .args(["-h", &c.host, "-p", &c.port, "-U", &c.user, "-d", &dbname])
        .env("PGPASSWORD", &c.password)
        .stdout(Stdio::from(file))
        .stderr(Stdio::piped())
        .spawn()?
        .wait_with_output()
        .await?;
    if !out.status.success() {
        return Err(AppError::Other(anyhow!(
            "pg_dump (platform) 失敗: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(())
}

/// dump を再作成済みの DB に流し込む。`role=<owner>` で作成オブジェクトを owner 所有に。
pub async fn restore_database(
    admin_url: &str,
    dbname: &str,
    owner: &str,
    dump_path: &Path,
) -> AppResult<()> {
    if !is_safe_ident(owner) {
        return Err(AppError::Other(anyhow!("owner 識別子が不正: {owner}")));
    }
    let c = conn_parts(admin_url)?;
    let file = std::fs::File::open(dump_path)?;
    let out = Command::new("psql")
        .args([
            "-h",
            &c.host,
            "-p",
            &c.port,
            "-U",
            &c.user,
            "-d",
            dbname,
            "-v",
            "ON_ERROR_STOP=1",
            "-q",
        ])
        .env("PGPASSWORD", &c.password)
        // admin は superuser なので role を owner に切替 → 復元オブジェクトが owner 所有になる。
        .env("PGOPTIONS", format!("-c role={owner}"))
        .stdin(Stdio::from(file))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await?;
    if !out.status.success() {
        return Err(AppError::Other(anyhow!(
            "psql restore 失敗: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(())
}

// ===== web SQL の結果整形 =====

/// 行の i 番目の値を text 表現にする(NULL は None)。よくある型を網羅し、
/// 未対応の型は型名のプレースホルダにフォールバックする(最小版 web SQL)。
pub fn col_to_string(row: &PgRow, i: usize) -> Option<String> {
    let ty = row.columns()[i].type_info().name();
    macro_rules! try_decode {
        ($t:ty) => {
            if let Ok(v) = row.try_get::<Option<$t>, _>(i) {
                return v.map(|x| x.to_string());
            }
        };
    }
    match ty {
        "BOOL" => try_decode!(bool),
        "INT2" => try_decode!(i16),
        "INT4" => try_decode!(i32),
        "INT8" => try_decode!(i64),
        "FLOAT4" => try_decode!(f32),
        "FLOAT8" => try_decode!(f64),
        "TEXT" | "VARCHAR" | "BPCHAR" | "NAME" | "CHAR" | "\"char\"" => try_decode!(String),
        "UUID" => try_decode!(uuid::Uuid),
        "TIMESTAMPTZ" => try_decode!(chrono::DateTime<chrono::Utc>),
        "TIMESTAMP" => try_decode!(chrono::NaiveDateTime),
        "DATE" => try_decode!(chrono::NaiveDate),
        "JSON" | "JSONB" => {
            if let Ok(v) = row.try_get::<Option<serde_json::Value>, _>(i) {
                return v.map(|x| x.to_string());
            }
        }
        _ => {}
    }
    // フォールバック:text として読めれば返す、駄目なら型名。
    match row.try_get::<Option<String>, _>(i) {
        Ok(v) => v,
        Err(_) => Some(format!("({ty})")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gen_dbname_is_safe_ident() {
        for _ in 0..200 {
            let n = gen_dbname();
            assert!(n.starts_with("db_"));
            assert!(is_safe_ident(&n), "unsafe: {n}");
        }
    }

    #[test]
    fn gen_password_is_sql_url_safe() {
        for _ in 0..200 {
            let p = gen_password();
            assert!(check_password(&p).is_ok());
            assert!(!p.contains('\''));
        }
    }

    #[test]
    fn is_safe_ident_rejects_injection() {
        assert!(!is_safe_ident("db; DROP DATABASE x"));
        assert!(!is_safe_ident("DB_UPPER"));
        assert!(!is_safe_ident("1abc"));
        assert!(!is_safe_ident("a'b"));
        assert!(!is_safe_ident(""));
        assert!(is_safe_ident("db_abc123"));
    }

    #[test]
    fn derived_role_names() {
        let n = DbNames::from_dbname("db_test".into());
        assert_eq!(n.owner, "db_test_owner");
        assert_eq!(n.app, "db_test_app");
        assert_eq!(n.human, "db_test_human");
    }

    #[test]
    fn conn_parts_decodes_userinfo() {
        let c = conn_parts("postgres://us%40er:p%2Fss@127.0.0.1:5435/postgres").unwrap();
        assert_eq!(c.user, "us@er");
        assert_eq!(c.password, "p/ss");
        assert_eq!(c.host, "127.0.0.1");
        assert_eq!(c.port, "5435");
    }
}
