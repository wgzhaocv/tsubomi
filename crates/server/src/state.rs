use crate::config::Config;
use crate::crypto::Cipher;
use anyhow::Context;
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use std::collections::HashMap;
use std::ops::Deref;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;

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
    /// docker.sock の async クライアント(M3)。コンテナの pull / 起動 / 停止 /
    /// 一覧(後の reconcile)が使う。プラットフォームはホスト直走りで docker.sock を保持。
    pub docker: bollard::Docker,
    /// service 単位のデプロイ直列化ロック(単機運用なのでインメモリで足りる)。
    /// 同一 service への同時 deploy(hook / `--local`)はここで順番待ちし、コンテナ /
    /// route / 状態への競合を防ぐ。S7/S8 の start/stop/reconcile も同 service を触る時は
    /// このロックを取るべき(現状は run_digest だけが使う)。外側 = map を守る std Mutex、
    /// 中身 = 各 service の tokio Mutex(.await をまたいで保持するため)。
    /// map は service 数ぶんしか増えない(小さな Arc が残るだけ。単機規模では掃除不要)。
    pub deploy_locks: Mutex<HashMap<Uuid, Arc<tokio::sync::Mutex<()>>>>,
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

        // docker.sock(unix)/ DOCKER_HOST を既定で拾う。起動時に ping して疎通を確認する:
        // docker 無しでは service フェーズは機能しないので、ここで早期に失敗させる。
        let docker =
            bollard::Docker::connect_with_local_defaults().context("docker.sock への接続に失敗")?;
        docker.ping().await.context(
            "docker daemon に ping できない(docker は起動しているか / DOCKER_HOST を確認)",
        )?;

        Ok(Self(Arc::new(AppStateInner {
            config,
            db,
            tenant_admin,
            crypto,
            http,
            docker,
            deploy_locks: Mutex::new(HashMap::new()),
        })))
    }

    /// service のデプロイ直列化ロックを取得(無ければ作る)。同一 service の
    /// 並行 deploy はこの tokio Mutex で順番待ちする(run_digest が先頭で .lock().await)。
    pub fn deploy_lock(&self, service_id: Uuid) -> Arc<tokio::sync::Mutex<()>> {
        // poison しても map 自体は壊れない(値は Arc だけ)。回復して使い続ける —
        // bookkeeping mutex の poison で全 service の deploy を巻き込まない。
        let mut map = self
            .0
            .deploy_locks
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        map.entry(service_id)
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }
}

impl Deref for AppState {
    type Target = AppStateInner;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
