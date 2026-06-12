use crate::config::Config;
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use std::ops::Deref;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

pub struct AppStateInner {
    pub config: Config,
    pub db: PgPool,
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

        // redirect: none — このクライアントは Google の token / userinfo
        // エンドポイントしか叩かない。どちらかがリダイレクトを返したら
        // それ自体が異常なので、追従しない。
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()?;

        Ok(Self(Arc::new(AppStateInner { config, db, http })))
    }
}

impl Deref for AppState {
    type Target = AppStateInner;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
