use anyhow::Context;
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
        })
    }
}
