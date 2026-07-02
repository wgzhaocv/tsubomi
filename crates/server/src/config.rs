use anyhow::Context;
use base64::Engine;
use ipnet::Ipv4Net;
use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Config {
    pub bind_addr: SocketAddr,
    /// ビルド済み SPA(index.html + assets)の置き場。`/api` 以外のルートは
    /// すべてここへフォールバックして配信する。
    pub web_dir: String,
    pub database_url: String,
    /// 管制面プール(pg-platform)の最大接続数。`TSUBOMI_DB_MAX_CONN`、既定 20。
    /// 全認証リクエストが「認証クエリ + ハンドラのクエリ」でここを取り合う。
    /// **LAN 実測の結論:このプール幅はスループットの律速ではない** — 10→50 に広げても
    /// /api/databases は ~2350 rps で頭打ちのまま(CPU + 毎リクエスト 3 往復の syscall/IPC が律速)。
    /// なので既定 20 は「提速」ではなく、遅い query(web SQL の大スキャン等)が接続を握っても
    /// 速い認証リクエストを待たせないための余裕。Postgres 側 `max_connections` 未満に保つこと。
    pub db_max_conn: u32,
    /// 管制面プールの最小保持接続数。`TSUBOMI_DB_MIN_CONN`、既定 5。常時保温して
    /// 冷えたプールでの接続確立コストを消す。
    pub db_min_conn: u32,
    /// pg-tenant への admin(DDL)プールの最大接続数。`TSUBOMI_TENANT_ADMIN_MAX_CONN`、既定 10。
    /// DDL は低頻度だが web SQL タブもここを通る。**注意:子網 CIDR の `TSUBOMI_TENANT_POOL`
    /// とは無関係**(あちらは私網 subnet プール、こちらは DB コネクションプール)。
    pub tenant_admin_max_conn: u32,
    pub google_client_id: String,
    pub google_client_secret: String,
    pub google_redirect_uri: String,
    /// このサーバの正規外部 URL(末尾スラッシュなし)。tbm ログインの
    /// redirect_uri 許可リストの組み立てに使う — ブラウザから到達する
    /// オリジンと一致させること(dev では vite の :5173)。
    pub server_url: String,
    /// WebSocket 升级で許可する管制面オリジン(CSWSH 対策)。テナント app は `<sub>.<domain>` =
    /// 管制面と same-site なので `SameSite=Lax` cookie だけでは WS 乗っ取りを防げない。升级時に
    /// `Origin` をこの allowlist と照合して弾く。既定は `server_url`(= ブラウザが到達するオリジン)、
    /// `TSUBOMI_CONTROL_ORIGIN`(カンマ区切り)で追加できる(管制面が複数オリジンを持つ場合)。
    pub control_origins: Vec<String>,
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
    /// **内部**(service へ注入する app role)接続文字列の sslmode。dev=disable、prod=require。
    /// 内部は `db_internal_host`(=`tsubomi-pgbouncer`)へ docker DNS で繋ぐので、pgbouncer 証明書の
    /// SAN(`db.<域名>`)とホスト名が一致しない → verify-full は使えず **require 据え置き**。
    pub db_internal_sslmode: String,
    /// **外部**(human が手にする公開)接続文字列の sslmode。既定は内部に追従(未設定時)、
    /// 公網 VPS 中継 + 公開 LE 証明書の部署では `verify-full` にする(`TSUBOMI_DB_SSLMODE_EXTERNAL`)。
    /// 外部は `db_public_host`(=`db.<域名>`)経由なので証明書 SAN と一致し verify-full が成立する。
    /// verify-ca/verify-full のときは `build_url` が `sslrootcert=system` を付与する(下記参照)。
    pub db_public_sslmode: String,
    /// 外部(human)接続文字列を提供するか(`TSUBOMI_DB_PUBLIC_ENABLED`、既定 false)。
    /// **off**(CF Tunnel など公網 TCP 入口を持たない部署):web は接続文字列カードを出さず、
    /// `/url`・`/rotate` も後端で拒否する(届かない LAN IP を見せて誤誘導するのを断つ)。
    /// **on**(公網 IP の VPS):提供する。web SQL タブと human role 自体は本フラグと無関係で
    /// 常に動く(web SQL は tenant_admin_url 経由・公開ホストを使わないため)。
    pub db_public_enabled: bool,
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
    /// 公開 cache(人が手にする外部 `rediss://`)のホスト / ポート / 提供可否。`db_public_*` の cache 版。
    /// **off**(既定):build_url は内部 `redis://`、web は内部串カードのまま。**on**(公網 VPS + sni-gate +
    /// valkey TLS):build_url が `rediss://cache_public_host:port` を出す。公網ポートは会社防火墙の都合で
    /// 443 一択(`incident-frp-pg-public-2026-06-22`)。値は url/rotate 表示時に解決(人が手に持つ外部串)。
    pub cache_public_host: String,
    pub cache_public_port: u16,
    pub cache_public_enabled: bool,

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
    /// **CF を経由しない**registry 直連 push ホスト(`TSUBOMI_REGISTRY_DIRECT`、任意)。
    /// CF proxy は request body ≈100MB 上限があり、イメージの大きな層の push が 413 で割れる
    /// (単層 >100MB は経路として不成立)。設定すると:①traefik に直連入口 router
    /// (entrypoint `registrydirect`、LE DNS-01 終端)を追記 ②CI へ配る push 先
    /// (`RegistryCreds.host` = GitHub Variable `TSUBOMI_REGISTRY`)がこの host になる。
    /// CF 経由の `registry_push` 入口も**共存**(pull / 小さい層はそのまま)。
    /// 実装級は doc/paas-registry-direct-design.md。
    pub registry_direct: Option<String>,
    /// build 対象の arch(GitHub Variable `TSUBOMI_PLATFORMS`)。§6.6 のデータ駆動:
    /// 将来 x86_64 host を足したら `linux/arm64,linux/amd64` に変えるだけ。
    pub platforms: String,
    /// per-service 私網の名前接頭辞(`TSUBOMI_SVC_NETWORK_PREFIX`、既定 `tsubomi-svc-`)。
    /// 各 service は `<prefix><service_id>` の専用 bridge に隔離され、二度と他テナントと
    /// 同じ網を共有しない(東西向=横移動の遮断。背骨「隔離は仕組みで守る」)。infra(traefik/
    /// pgbouncer/valkey)はこの私網へ on-demand で attach される。**旧 `tsubomi-edge` 共有網は
    /// テナントにとって無用化** — compose では infra が居つくだけで Rust からは参照しない。
    pub svc_network_prefix: String,
    /// テナント私網に明示割当する subnet の親プール(`TSUBOMI_TENANT_POOL`、既定 `10.231.0.0/16`)。
    /// 各 service 桥は ここから `/24` を取り、租户トラフィックを**源 CIDR で一意識別**できるようにする
    /// (egress 防火墙の `-s <pool>` マッチの前提。`doc/paas-egress-design.md` §3.1)。**10/8 を選ぶ理由**:
    /// このホストで LAN(192.168)/ docker 自動(172.17・192.168.16)/ tailnet(100.x)と重ならない。
    /// docker 任せの自動割当は範囲が読めず LAN に近づくので、明示割当でプールを固定する。
    /// 起動時に CIDR として parse + `/24` 以上を検証する(domain / master_key と同じく fail-fast)。
    pub tenant_pool: Ipv4Net,
    /// per-service 私網へ attach する infra コンテナ**名**(`connect_network` の対象)。
    /// 既定は compose の `container_name`。注入文字列の DNS 名(`db_internal_host` 等)とは
    /// 別フィールド — 別名設定で乖離させないため(片方は connect 対象、片方は DNS 解決名)。
    pub traefik_container: String,
    pub pgbouncer_container: String,
    pub valkey_container: String,
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
    /// **危険操作の確認コードを log に出すことを明示的に許す**(`TSUBOMI_DEV_INSECURE_LOG_ACTION_CODES`、
    /// 既定 false)。dev で Resend 未契約のとき owner がコードを使えるようにする退路。**本番では絶対に
    /// 立てない** — log アクセス権だけで owner の危険操作(他人資源の stop/delete)を完遂できてしまう。
    /// off のまま mail も未設定なら、危険操作は「配信できない」として fail-fast する(admin/actions.rs)。
    pub dev_insecure_log_action_codes: bool,
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
    /// **直連入口(`registry_direct`)だけ設定された部署でも true**:CI へ配る push 先が直連 host に
    /// なるのに router が書かれず docker login が黙って割れる、を防ぐ(codex 監査)。
    pub fn registry_ingress(&self) -> bool {
        self.registry_push != self.registry_pull || self.registry_direct.is_some()
    }

    /// traefik の `Host(...)` ルール用の push ホスト名(`registry_push` から `:port` を落とす。
    /// Host マッチはホスト名のみ。push 自体は docker login 用に port 付きでも可)。
    pub fn registry_host(&self) -> &str {
        self.registry_push
            .split(':')
            .next()
            .unwrap_or(&self.registry_push)
    }

    /// 直連入口の `Host(...)` 用ホスト名(`registry_direct` から `:port` を落とす)。未設定なら None。
    pub fn registry_direct_host(&self) -> Option<&str> {
        self.registry_direct
            .as_deref()
            .map(|d| d.split(':').next().unwrap_or(d))
    }

    /// CI(GitHub Secret/Variable)へ配る push 先。直連入口があればそれを優先
    /// (大きな層が CF 100MB 上限で 413 にならない経路)。無ければ従来の CF 経由 push ホスト。
    pub fn registry_ci_host(&self) -> &str {
        self.registry_direct.as_deref().unwrap_or(&self.registry_push)
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

    /// WebSocket 升级の `Origin` が管制面オリジンか(CSWSH 対策)。ブラウザは WS 升级で必ず
    /// `Origin` を送るため、**欠落も拒否**する(対話 WS は web 専用 = ブラウザ経路のみ想定)。
    pub fn origin_allowed(&self, origin: Option<&str>) -> bool {
        match origin {
            Some(o) => {
                let o = o.trim_end_matches('/');
                self.control_origins.iter().any(|a| a == o)
            }
            None => false,
        }
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
        // DB 接続プール:実測ではプール幅はスループットの律速ではない(CPU/IPC 律速)。env で上書き可、
        // 未設定なら本番でも安全な既定が効く(子網 CIDR の TSUBOMI_TENANT_POOL とは無関係)。
        // max=0 はプール枯渇=全 query が acquire_timeout で失敗するので最低 1 に丸める。
        // min は max を超えても無意味なので max で頭打ち(誤設定で起動を壊さない)。
        let db_max_conn = std::env::var("TSUBOMI_DB_MAX_CONN")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(20)
            .max(1);
        let db_min_conn = std::env::var("TSUBOMI_DB_MIN_CONN")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(5)
            .min(db_max_conn);
        let tenant_admin_max_conn = std::env::var("TSUBOMI_TENANT_ADMIN_MAX_CONN")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10)
            .max(1);
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
        // 管制面オリジンの allowlist(WS の CSWSH 対策)。常に server_url を含め、
        // TSUBOMI_CONTROL_ORIGIN の各値を追加する(設定し忘れで自分を締め出さないよう union)。
        let mut control_origins = vec![server_url.clone()];
        if let Ok(extra) = std::env::var("TSUBOMI_CONTROL_ORIGIN") {
            for o in extra
                .split(',')
                .map(|o| o.trim().trim_end_matches('/'))
                .filter(|o| !o.is_empty())
            {
                if !control_origins.iter().any(|x| x == o) {
                    control_origins.push(o.to_string());
                }
            }
        }
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
        let db_internal_sslmode =
            std::env::var("TSUBOMI_DB_SSLMODE").unwrap_or_else(|_| "disable".to_string());
        // 外部は未設定なら内部に追従(従来は両者共有だったので後方互換)。公開 DB を verify-full で
        // 出す部署だけ `TSUBOMI_DB_SSLMODE_EXTERNAL=verify-full` を足す。dev は内部=disable を継ぐ。
        let db_public_sslmode = std::env::var("TSUBOMI_DB_SSLMODE_EXTERNAL")
            .unwrap_or_else(|_| db_internal_sslmode.clone());
        // 外部接続文字列の提供可否。既定 false(公網 TCP 入口を持たない CF 部署で誤って
        // 届かない接続文字列を見せないため)。公網 IP の VPS / ローカル dev で明示的に true にする。
        let db_public_enabled = std::env::var("TSUBOMI_DB_PUBLIC_ENABLED")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);
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
        let valkey_admin_url = std::env::var("TSUBOMI_VALKEY_ADMIN_URL").unwrap_or_else(|_| {
            "redis://tsubomi-admin:tsubomi_valkey_dev@127.0.0.1:6433".to_string()
        });
        // 注入する内部入口(コンテナ → edge 上の valkey を docker DNS で。外部入口は無い §11-B)。
        let cache_internal_host = std::env::var("TSUBOMI_CACHE_INTERNAL_HOST")
            .unwrap_or_else(|_| "tsubomi-valkey".to_string());
        let cache_internal_port: u16 = std::env::var("TSUBOMI_CACHE_INTERNAL_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(6379);
        // 公開 cache(外部 rediss://)。db_public_* の cache 版。公網は会社防火墙の都合で 443 一択
        // (incident-frp-pg-public-2026-06-22)。既定 off(公網入口を持たない部署で誤って届かない串を見せない)。
        let cache_public_host = std::env::var("TSUBOMI_CACHE_PUBLIC_HOST")
            .unwrap_or_else(|_| "127.0.0.1".to_string());
        let cache_public_port: u16 = std::env::var("TSUBOMI_CACHE_PUBLIC_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(443);
        let cache_public_enabled = std::env::var("TSUBOMI_CACHE_PUBLIC_ENABLED")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

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
        // CF を通らない直連 push 入口(任意)。書式は registry_push と同じ host[:port]。
        let registry_direct = std::env::var("TSUBOMI_REGISTRY_DIRECT")
            .ok()
            .filter(|s| !s.is_empty());
        if let Some(d) = &registry_direct
            && !d
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':'))
        {
            anyhow::bail!(
                "TSUBOMI_REGISTRY_DIRECT must be host[:port]([a-zA-Z0-9.:-] のみ、scheme/path 不可): {d}"
            );
        }
        let platforms =
            std::env::var("TSUBOMI_PLATFORMS").unwrap_or_else(|_| "linux/arm64".to_string());
        // M6 網隔離:per-service 私網の接頭辞 + 私網へ attach する infra コンテナ名。
        let svc_network_prefix = std::env::var("TSUBOMI_SVC_NETWORK_PREFIX")
            .unwrap_or_else(|_| "tsubomi-svc-".to_string());
        // 起動時に parse + 検証(use 時の parse / 黙ったフォールバックを避け、E2 egress が
        // 「全租户網は pool 内」を不変条件にできるようにする)。/24 を 1 個以上切り出せる広さが要る。
        let tenant_pool: Ipv4Net = {
            let raw =
                std::env::var("TSUBOMI_TENANT_POOL").unwrap_or_else(|_| "10.231.0.0/16".to_string());
            let net: Ipv4Net = raw.parse().map_err(|e| {
                anyhow::anyhow!("TSUBOMI_TENANT_POOL を CIDR として解析できません({raw}): {e}")
            })?;
            if net.prefix_len() > 24 {
                anyhow::bail!(
                    "TSUBOMI_TENANT_POOL は /24 以上の広さが必要です(>= 1 個の /24 を切り出すため): {net}"
                );
            }
            net
        };
        let traefik_container = std::env::var("TSUBOMI_TRAEFIK_CONTAINER")
            .unwrap_or_else(|_| "tsubomi-traefik".to_string());
        let pgbouncer_container = std::env::var("TSUBOMI_PGBOUNCER_CONTAINER")
            .unwrap_or_else(|_| "tsubomi-pgbouncer".to_string());
        let valkey_container = std::env::var("TSUBOMI_VALKEY_CONTAINER")
            .unwrap_or_else(|_| "tsubomi-valkey".to_string());
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
        // 危険操作コードの log 出力許可(dev 退路。本番では立てない — §admin/actions.rs)。
        let dev_insecure_log_action_codes = std::env::var("TSUBOMI_DEV_INSECURE_LOG_ACTION_CODES")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        Ok(Self {
            bind_addr,
            web_dir,
            database_url,
            db_max_conn,
            db_min_conn,
            tenant_admin_max_conn,
            google_client_id,
            google_client_secret,
            google_redirect_uri,
            server_url,
            control_origins,
            allowed_hds,
            owner_emails,
            cookie_secure,
            release_dir,
            tenant_admin_url,
            db_public_host,
            db_public_port,
            db_internal_sslmode,
            db_public_sslmode,
            db_public_enabled,
            db_internal_host,
            db_internal_port,
            valkey_admin_url,
            cache_internal_host,
            cache_internal_port,
            cache_public_host,
            cache_public_port,
            cache_public_enabled,
            master_key,
            backup_dir,
            trash_dir,
            volumes_dir,
            max_upload_bytes,
            domain,
            registry_pull,
            registry_push,
            registry_direct,
            platforms,
            svc_network_prefix,
            tenant_pool,
            traefik_container,
            pgbouncer_container,
            valkey_container,
            tls,
            traefik_dynamic_dir,
            resend_api_key,
            mail_from,
            disk_warn_pct,
            disk_critical_pct,
            dev_insecure_log_action_codes,
        })
    }
}
