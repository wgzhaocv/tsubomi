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
    /// 外部(human)も内部(注入する app)も同じ pgbouncer なので sslmode を共有する。
    pub db_sslmode: String,
    /// service へ注入する **内部**接続文字列のホスト / ポート(コンテナが docker DNS で引く
    /// pgbouncer)。外部入口 db_public_host とは別 — コンテナは社外に出ず内部路径で繋ぐ(§7.2)。
    pub db_internal_host: String,
    pub db_internal_port: u16,
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

    // ===== M5 cache(valkey)=====
    /// valkey の admin 接続(per-cache ACL の発行 / 収束用)。`tsubomi-admin` ユーザで繋ぐ
    /// (default は off。§11-J)。例:redis://tsubomi-admin:..@127.0.0.1:6433。dev は loopback。
    pub valkey_admin_url: String,
    /// service へ注入する **内部**入口(コンテナが docker DNS で引く valkey)。外部入口は無い
    /// (§11-B:cache は内部注入のみ)。pgbouncer と同型でコンテナは社外に出ない。REDIS_URL に載る。
    pub cache_internal_host: String,
    pub cache_internal_port: u16,

    // ===== M3 service =====
    /// service の subdomain のルートドメイン。ルーティングは `<subdomain>.<domain>`。
    /// dev=localhost(ブラウザが `*.localhost` を 127.0.0.1 に解決)、prod=会社ドメイン。
    pub domain: String,
    /// 平台が digest pull する registry の host:port。dev=127.0.0.1:5000
    /// (localhost は docker が insecure registry として許すので証明書不要)。
    pub registry_pull: String,
    /// GitHub Actions が docker login + push する registry の host。dev=registry_pull
    /// と同じ(127.0.0.1:5000・認証なし)、prod=registry.<domain>。service create が
    /// 返す DTO の `registry.host` に載る(digest 内容アドレスなので push/pull の host が
    /// 違っても問題ない — 決定 #3)。
    pub registry_push: String,
    /// build 対象の arch(GitHub Variable `TSUBOMI_PLATFORMS`)。§6.6 のデータ駆動:
    /// 将来 x86_64 host を足したら `linux/arm64,linux/amd64` に変えるだけ。
    pub platforms: String,
    /// ユーザコンテナを attach する docker ネットワーク名(traefik も参加)。
    pub edge_network: String,
    /// 「**誰が** TLS を終端するか」(`TSUBOMI_TLS`)。true = traefik 自身が websecure + LE
    /// (certResolver `le`)で終端(直 VPS)、route の router を websecure にし apex router も書く。
    /// false(既定)= 上流(CF Tunnel / 逆代理)が終端 → router は web(HTTP)、apex は traefik を
    /// 経由しない。**registry の push 入口の有無は tls ではなく `registry_ingress()`(push≠pull)が
    /// 決める** — tunnel(tls=false)でも入口は書く。ACME メールは compose が env から直接読む。
    pub tls: bool,

    // ===== ガバナンス:IP 許可リスト =====
    /// 平台が traefik の動的設定(ipAllowList middleware)を書き出すディレクトリ。
    /// traefik(compose)が file provider でこの同じホストパスを読む。会社 IP
    /// 許可リストの変更はここへ書き直され、traefik がホットリロードする。
    pub traefik_dynamic_dir: PathBuf,

    // ===== M4 ガバナンス:メール基盤(Resend)+ ディスク水位警告 =====
    /// Resend の API キー。未設定 = メールを送らず log のみ(dev / 未契約時の退路)。
    pub resend_api_key: Option<String>,
    /// メール送信元(例:"tsubomi <noreply@tsubomi-app.com>")。resend_api_key が在る時は必須。
    pub mail_from: Option<String>,
    /// ディスク使用率の警告閾値(%)。warn 以上で owner にメール、critical でさらに強調。
    /// gc の周期(1h)で `df` を見て判定し、platform_config の状態で去重する(§4.2)。
    pub disk_warn_pct: u8,
    pub disk_critical_pct: u8,
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
    /// prod で registry の push 入口(traefik basicAuth)を書くか。push 先が pull(ローカル
    /// 127.0.0.1:5000)と別ホスト = 公網の認証付き registry がある = prod、という自然な信号。
    /// dev は両者一致なので false(registry は無認証ループバック直結で入口を書かない)。
    /// TLS の有無(traefik 終端 / 上流終端)とは独立 — tunnel(tls=false)でも入口は要る。
    pub fn registry_ingress(&self) -> bool {
        self.registry_push != self.registry_pull
    }

    /// traefik の `Host(...)` ルール用の push ホスト名(`registry_push` から `:port` を落とす。
    /// Host マッチはホスト名のみ。push 自体は docker login 用に port 付きでも可)。
    pub fn registry_host(&self) -> &str {
        self.registry_push
            .split(':')
            .next()
            .unwrap_or(&self.registry_push)
    }

    /// service の公開 URL(`<scheme>://<subdomain>.<domain>`)を組み立てる。
    /// scheme は dev(domain=localhost)が http、それ以外(prod = CF tunnel / 直 VPS の
    /// いずれも公開面は https)が https。ServiceDto.url に載せて web/CLI が表示する。
    pub fn service_url(&self, subdomain: &str) -> String {
        let scheme = if self.domain == "localhost" {
            "http"
        } else {
            "https"
        };
        format!("{scheme}://{subdomain}.{}", self.domain)
    }

    pub fn from_env() -> anyhow::Result<Self> {
        // 既定は **loopback** :9090(同居する amber は 8080)。本番は前段(CF Tunnel / 逆代理)が
        // localhost へ転送する想定なので公網露出しないのが安全側。直 VPS で traefik コンテナが
        // host-gateway 経由 apex を叩く場合だけ明示的に 0.0.0.0:9090 にし、:9090 は FW で塞ぐ(§13.B)。
        let bind_addr = std::env::var("TSUBOMI_BIND_ADDR")
            .unwrap_or_else(|_| "127.0.0.1:9090".to_string())
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

        // 明示設定が優先。未設定なら **fail-safe**:dev(TSUBOMI_DOMAIN 未設定 / localhost)以外は
        // Secure を付ける(env を書き忘れても本番で非 Secure cookie を出さない。security review S3)。
        let cookie_secure = std::env::var("TSUBOMI_COOKIE_SECURE")
            .map(|v| v == "true" || v == "1")
            .unwrap_or_else(|_| {
                std::env::var("TSUBOMI_DOMAIN")
                    .map(|d| d != "localhost")
                    .unwrap_or(false)
            });

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
        // 注入する内部入口(コンテナ → edge 上の pgbouncer を docker DNS で)。§7.2。
        let db_internal_host = std::env::var("TSUBOMI_DB_INTERNAL_HOST")
            .unwrap_or_else(|_| "tsubomi-pgbouncer".to_string());
        let db_internal_port: u16 = std::env::var("TSUBOMI_DB_INTERNAL_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(6432);
        let master_key = MasterKey(load_master_key()?);
        let backup_dir = std::env::var("TSUBOMI_BACKUP_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/srv/tsubomi/backups"));
        let trash_dir = std::env::var("TSUBOMI_TRASH_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/srv/tsubomi/trash"));
        // ===== M5 cache(valkey)=====
        // dev 既定:loopback の tsubomi-admin(compose の TSUBOMI_VALKEY_ADMIN_PASS と揃える)。
        // prod は env で実値に上書き。default ユーザは off なので必ず tsubomi-admin で繋ぐ。
        // dev 既定:loopback の tsubomi-admin。ホスト側 port は 6433(compose と揃える。6379 は
        // ローカル redis に取られがちなので衝突回避)。prod は env で実値に上書き。
        let valkey_admin_url = std::env::var("TSUBOMI_VALKEY_ADMIN_URL")
            .unwrap_or_else(|_| "redis://tsubomi-admin:tsubomi_valkey_dev@127.0.0.1:6433".to_string());
        // 注入する内部入口(コンテナ → edge 上の valkey を docker DNS で。外部入口は無い §11-B)。
        let cache_internal_host = std::env::var("TSUBOMI_CACHE_INTERNAL_HOST")
            .unwrap_or_else(|_| "tsubomi-valkey".to_string());
        let cache_internal_port: u16 = std::env::var("TSUBOMI_CACHE_INTERNAL_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(6379);

        let volumes_dir = std::env::var("TSUBOMI_VOLUMES_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/srv/tsubomi/volumes"));
        // 既定 100 MiB。env で上書き可(将来の磁盘 quota とは別レイヤの即時防御)。
        let max_upload_bytes: usize = std::env::var("TSUBOMI_MAX_UPLOAD_BYTES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(100 * 1024 * 1024);

        // ===== M3 service =====
        let domain = std::env::var("TSUBOMI_DOMAIN").unwrap_or_else(|_| "localhost".to_string());
        // ホスト名以外の文字を弾く:domain は traefik の Host(`<sub>.<domain>`) ルールへ
        // そのまま埋め込まれるので、引用符 / 空白 / バックスラッシュ等が混じると設定が壊れる。
        if domain.is_empty()
            || !domain
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        {
            anyhow::bail!("TSUBOMI_DOMAIN must be a hostname ([a-zA-Z0-9.-] のみ): {domain}");
        }
        let registry_pull =
            std::env::var("TSUBOMI_REGISTRY_PULL").unwrap_or_else(|_| "127.0.0.1:5000".to_string());
        // push 入口。未設定なら pull と同じ(dev の無認証 registry)。
        let registry_push =
            std::env::var("TSUBOMI_REGISTRY_PUSH").unwrap_or_else(|_| registry_pull.clone());
        // push host は registry.yml の traefik Host(...) ルールへ埋め込む(registry_host())。
        // host[:port] のみ許可(scheme `//` / path `/` / 引用符 / 空白を弾く。注入・設定崩れ防止)。
        if registry_push.is_empty()
            || !registry_push
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':'))
        {
            anyhow::bail!(
                "TSUBOMI_REGISTRY_PUSH must be host[:port]([a-zA-Z0-9.:-] のみ、scheme/path 不可): {registry_push}"
            );
        }
        let platforms =
            std::env::var("TSUBOMI_PLATFORMS").unwrap_or_else(|_| "linux/arm64".to_string());
        let edge_network =
            std::env::var("TSUBOMI_EDGE_NETWORK").unwrap_or_else(|_| "tsubomi-edge".to_string());
        // 本番のみ true。route.rs / registry の traefik 出力を websecure+LE 形へ切り替える。
        let tls = std::env::var("TSUBOMI_TLS")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        // ===== ガバナンス:IP 許可リスト =====
        // volumes_dir / backup_dir と同じ /srv/tsubomi 配下の規約。本番は compose の
        // bind mount と一致させること(同じホストパスを traefik が file provider で読む)。
        let traefik_dynamic_dir = std::env::var("TSUBOMI_TRAEFIK_DYNAMIC_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/srv/tsubomi/traefik-dynamic"));

        // ===== M4 ガバナンス:メール基盤 + ディスク水位警告 =====
        let resend_api_key = std::env::var("RESEND_API_KEY")
            .ok()
            .filter(|s| !s.is_empty());
        let mail_from = std::env::var("TSUBOMI_MAIL_FROM")
            .ok()
            .filter(|s| !s.is_empty());
        if resend_api_key.is_some() && mail_from.is_none() {
            anyhow::bail!("RESEND_API_KEY が設定されているなら TSUBOMI_MAIL_FROM も必要です");
        }
        let disk_warn_pct: u8 = std::env::var("TSUBOMI_DISK_WARN_PCT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(80);
        let disk_critical_pct: u8 = std::env::var("TSUBOMI_DISK_CRITICAL_PCT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(90);

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
            db_internal_host,
            db_internal_port,
            valkey_admin_url,
            cache_internal_host,
            cache_internal_port,
            master_key,
            backup_dir,
            trash_dir,
            volumes_dir,
            max_upload_bytes,
            domain,
            registry_pull,
            registry_push,
            platforms,
            edge_network,
            tls,
            traefik_dynamic_dir,
            resend_api_key,
            mail_from,
            disk_warn_pct,
            disk_critical_pct,
        })
    }
}
