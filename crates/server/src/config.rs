use anyhow::Context;
use base64::Engine;
use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Config {
    pub bind_addr: SocketAddr,
    /// ビルド済み SPA(index.html + assets)の置き場。`/api` 以外のルートは
    /// すべてここへフォールバックして配信する。
    pub web_dir: String,
    pub database_url: String,
    pub google_client_id: String,
    pub google_client_secret: String,
    pub google_redirect_uri: String,
    /// このサーバの正規外部 URL(末尾スラッシュなし)。tbm ログインの
    /// redirect_uri 許可リストの組み立てに使う — ブラウザから到達する
    /// オリジンと一致させること(dev では vite の :5173)。
    pub server_url: String,
    /// Google Workspace の hosted domain(env ではカンマ区切りで複数可)。
    /// アカウントの `hd` claim と email ドメインの両方がこのリストに
    /// 含まれないとログイン拒否。登録制限はここ(.env)にあり、検証は
    /// サーバ側のみで行う。
    pub allowed_hds: Vec<String>,
    /// owner の種(design v2 §7 により最大 2 名)。ログイン時に昇格を適用。
    /// first-login-wins ではなく明示的な種付け。
    pub owner_emails: Vec<String>,
    pub cookie_secure: bool,
    /// CLI リリースの manifest / バイナリ置き場(/api/cli/version を配信)。
    /// None = まだリリース未発行。エンドポイントは 404 を返し、CLI の
    /// バージョンチェックは沈黙する。
    pub release_dir: Option<PathBuf>,

    // ===== M1 database =====
    /// pg-tenant の admin 接続(DDL 実行用)。
    /// 例:postgres://tsubomi_admin:..@127.0.0.1:5435/postgres
    pub tenant_admin_url: String,
    /// human が手にする外部接続文字列のホスト。dev=127.0.0.1 / prod=db.<域名>。
    pub db_public_host: String,
    pub db_public_port: u16,
    /// 接続文字列の sslmode。dev=disable(pgbouncer に TLS なし)、prod は env で調整。
    pub db_sslmode: String,
    /// at-rest 暗号化の master key(32 bytes)。DB パスワードの暗号化に使う。
    pub master_key: MasterKey,
    /// 日次バックアップの置き場 / ゴミ箱(dump)の置き場(server ホスト上)。
    /// pg_dump / psql は TENANT_ADMIN_URL 経由で TCP 直結する(docker exec ではない)。
    pub backup_dir: PathBuf,
    pub trash_dir: PathBuf,

    // ===== M2 volume =====
    /// volume 実体の置き場(server ホスト上)。各 volume は
    /// `<volumes_dir>/<user_id>/<volume_id>/` の假根サンドボックス。
    pub volumes_dir: PathBuf,
    /// ファイルアップロードの 1 リクエスト上限(バイト)。無制限だと
    /// メモリ/ディスクを一撃で食えるので硬上限を被せる(磁盘 quota は M4)。
    pub max_upload_bytes: usize,
}

/// master key のラッパ。Config は Debug 派生なので、生鍵が `{:?}` で漏れないように
/// 手書き Debug で伏せる。
#[derive(Clone)]
pub struct MasterKey(pub [u8; 32]);

impl std::fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MasterKey([redacted])")
    }
}

/// master key を env から読む:`TSUBOMI_MASTER_KEY`(base64 インライン)優先、
/// 無ければ `TSUBOMI_MASTER_KEY_FILE`(base64 を書いたファイル、prod は root のみ)。
/// どちらも 32 bytes にデコードされること。
fn load_master_key() -> anyhow::Result<[u8; 32]> {
    let b64 = if let Ok(inline) = std::env::var("TSUBOMI_MASTER_KEY") {
        inline
    } else if let Ok(path) = std::env::var("TSUBOMI_MASTER_KEY_FILE") {
        std::fs::read_to_string(&path).with_context(|| format!("reading {path}"))?
    } else {
        anyhow::bail!(
            "TSUBOMI_MASTER_KEY or TSUBOMI_MASTER_KEY_FILE must be set (at-rest 暗号化の master key, base64 of 32 bytes)"
        );
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .context("master key must be valid base64")?;
    bytes.as_slice().try_into().map_err(|_| {
        anyhow::anyhow!(
            "master key must decode to exactly 32 bytes (got {})",
            bytes.len()
        )
    })
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        // デフォルト 9090:8080 は同居する amber が使っている(香橙派でも
        // ローカル dev でも衝突しないように)。
        let bind_addr = std::env::var("TSUBOMI_BIND_ADDR")
            .unwrap_or_else(|_| "0.0.0.0:9090".to_string())
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid TSUBOMI_BIND_ADDR: {e}"))?;
        let web_dir = std::env::var("TSUBOMI_WEB_DIR").unwrap_or_else(|_| "web/dist".to_string());
        let database_url = std::env::var("DATABASE_URL").context("DATABASE_URL must be set")?;
        let google_client_id =
            std::env::var("GOOGLE_CLIENT_ID").context("GOOGLE_CLIENT_ID must be set")?;
        let google_client_secret =
            std::env::var("GOOGLE_CLIENT_SECRET").context("GOOGLE_CLIENT_SECRET must be set")?;
        let google_redirect_uri =
            std::env::var("GOOGLE_REDIRECT_URI").context("GOOGLE_REDIRECT_URI must be set")?;
        let server_url = std::env::var("TSUBOMI_SERVER_URL")
            .context("TSUBOMI_SERVER_URL must be set")?
            .trim_end_matches('/')
            .to_string();
        let allowed_hds: Vec<String> = std::env::var("TSUBOMI_ALLOWED_HD")
            .context("TSUBOMI_ALLOWED_HD must be set (company Google Workspace domain(s), comma-separated)")?
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        if allowed_hds.is_empty() {
            anyhow::bail!("TSUBOMI_ALLOWED_HD must contain at least one domain");
        }

        let owner_emails: Vec<String> = std::env::var("TSUBOMI_OWNER_EMAILS")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        if owner_emails.len() > 2 {
            anyhow::bail!("TSUBOMI_OWNER_EMAILS allows at most 2 owners (design v2 §7)");
        }

        let cookie_secure = std::env::var("TSUBOMI_COOKIE_SECURE")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        let release_dir = std::env::var("TSUBOMI_RELEASE_DIR").ok().map(PathBuf::from);

        let tenant_admin_url = std::env::var("TENANT_ADMIN_URL")
            .context("TENANT_ADMIN_URL must be set (pg-tenant admin 接続)")?;
        let db_public_host =
            std::env::var("TSUBOMI_DB_PUBLIC_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
        let db_public_port: u16 = std::env::var("TSUBOMI_DB_PUBLIC_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(6432);
        let db_sslmode =
            std::env::var("TSUBOMI_DB_SSLMODE").unwrap_or_else(|_| "disable".to_string());
        let master_key = MasterKey(load_master_key()?);
        let backup_dir = std::env::var("TSUBOMI_BACKUP_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/srv/tsubomi/backups"));
        let trash_dir = std::env::var("TSUBOMI_TRASH_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/srv/tsubomi/trash"));
        let volumes_dir = std::env::var("TSUBOMI_VOLUMES_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/srv/tsubomi/volumes"));
        // 既定 100 MiB。env で上書き可(将来の磁盘 quota とは別レイヤの即時防御)。
        let max_upload_bytes: usize = std::env::var("TSUBOMI_MAX_UPLOAD_BYTES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(100 * 1024 * 1024);

        Ok(Self {
            bind_addr,
            web_dir,
            database_url,
            google_client_id,
            google_client_secret,
            google_redirect_uri,
            server_url,
            allowed_hds,
            owner_emails,
            cookie_secure,
            release_dir,
            tenant_admin_url,
            db_public_host,
            db_public_port,
            db_sslmode,
            master_key,
            backup_dir,
            trash_dir,
            volumes_dir,
            max_upload_bytes,
        })
    }
}
