use crate::config::Config;
use crate::crypto::Cipher;
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use std::ops::Deref;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

pub struct AppStateInner {
    pub config: Config,
    /// 管制面 Postgres(pg-platform):期望状態のメタデータ。
    pub db: PgPool,
    /// ユーザ DB インスタンス(pg-tenant)への admin 接続:CREATE DATABASE / ROLE
    /// などの DDL 用。web SQL は別途この URL から human role の臨時接続を作る。
    pub tenant_admin: PgPool,
    /// at-rest 暗号化(DB パスワードの暗号化 / 復号)。
    pub crypto: Cipher,
    pub http: reqwest::Client,
}

#[derive(Clone)]
pub struct AppState(Arc<AppStateInner>);

impl AppState {
    pub async fn new(config: Config) -> anyhow::Result<Self> {
        let pg_opts =
            PgConnectOptions::from_str(&config.database_url)?.application_name("tsubomi-server");

        let db = PgPoolOptions::new()
            .max_connections(10)
            .acquire_timeout(Duration::from_secs(5))
            .connect_with(pg_opts)
            .await?;

        // マイグレーションをバイナリに埋め込む:新しい Postgres(ローカルでも
        // 香橙派でも)は起動時に自動でスキーマに収束する。
        sqlx::migrate!("../../migrations").run(&db).await?;

        // pg-tenant への admin 接続(DDL 用)。プールは小さめ — 同時に大量の
        // DDL を流すことはない。
        let tenant_opts = PgConnectOptions::from_str(&config.tenant_admin_url)?
            .application_name("tsubomi-tenant-admin");
        let tenant_admin = PgPoolOptions::new()
            .max_connections(5)
            .acquire_timeout(Duration::from_secs(5))
            .connect_with(tenant_opts)
            .await?;

        let crypto = Cipher::new(&config.master_key.0);

        // redirect: none — このクライアントは Google の token / userinfo
        // エンドポイントしか叩かない。どちらかがリダイレクトを返したら
        // それ自体が異常なので、追従しない。
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()?;

        Ok(Self(Arc::new(AppStateInner {
            config,
            db,
            tenant_admin,
            crypto,
            http,
        })))
    }
}

impl Deref for AppState {
    type Target = AppStateInner;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
