//! tsubomi サーバと tbm CLI が共有する型・定数・暗号プリミティブ。
//!
//! リクエスト/レスポンスの形やプロトコル定数をここで一度だけ定義することで、
//! サーバと CLI の契約が同期し続ける(片方だけ変えてズレる事故を構造的に防ぐ)。

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

// ============ OAuth / PKCE プロトコル定数(サーバと CLI の合意点)============

/// tbm CLI の OAuth client_id。サーバは検証側、CLI は申告側として同じ値を使う。
pub const OAUTH_CLIENT_ID: &str = "tbm-cli";
/// CLI ログインがブラウザで開く SPA ルート。
pub const OAUTH_AUTHORIZE_PATH: &str = "/oauth/authorize";
/// 認可後にコードを表示する SPA ルート(redirect_uri の固定部分)。
pub const OAUTH_CALLBACK_PATH: &str = "/oauth/code/callback";
/// code → token 交換のエンドポイント。
pub const OAUTH_TOKEN_PATH: &str = "/api/oauth/token";
/// PKCE 認可コードのプレフィックス。
pub const AUTHCODE_PREFIX: &str = "tbmc_";
/// CLI トークン平文のプレフィックス(GitHub 流のリーク検出マーカー)。
pub const CLI_TOKEN_PREFIX: &str = "tbm_";

/// ユーザの repo に置く GitHub Actions workflow のパス(サーバ / CLI の単一真源)。
/// サーバは setup_commands のコメントで参照、CLI は実ファイルの書き出し先に使う。
pub const WORKFLOW_PATH: &str = ".github/workflows/tsubomi-deploy.yml";

// ============ service 公開範囲(visibility)の wire 値 ============
// DB の CHECK・API リクエスト・DTO で共通の文字列(サーバの enum と CLI の表示 / 比較が共有)。

/// route ファイル無し = 公網不可視(subdomain は温存)。
pub const VISIBILITY_PRIVATE: &str = "private";
/// 既定:会社 IP 許可リストからのみ(従来挙動)。
pub const VISIBILITY_COMPANY: &str = "company";
/// ipallow middleware 無し = 全網公開。
pub const VISIBILITY_PUBLIC: &str = "public";

/// インストーラ(install.sh)がシェル rc に書く PATH ブロックの目印。
/// `tbm uninstall` がこれを手がかりにブロックを丸ごと取り除く。
/// ★ シェルスクリプトは Rust の const を import できないため、
///   crates/server/scripts/install.sh に同じ文字列がインライン展開されている。
///   ここを変えるときは install.sh も必ず揃えること(揃わないと uninstall が
///   rc の掃除に静かに失敗する)。
pub const PATH_MARKER_BEGIN: &str = ">>> tbm cli >>>";
pub const PATH_MARKER_END: &str = "<<< tbm cli <<<";

// ============ 暗号プリミティブ ============

/// 乱数 `n_bytes` バイトを base64-url-safe-no-pad で文字列化する。
/// セッショントークン・CLI トークン・authcode・CSRF state・PKCE verifier の
/// 生成は全部これを通る(実装が 1 箇所なら強度変更も 1 箇所で済む)。
pub fn random_b64(n_bytes: usize) -> String {
    let mut bytes = vec![0u8; n_bytes];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// sha256 の hex 表現。トークン類の保存用ハッシュ(DB には平文を残さない)。
pub fn sha256_hex(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    hex::encode(hasher.finalize())
}

/// PKCE S256:`base64url(sha256(verifier))`(RFC 7636)。
/// サーバ(検証側)と CLI(生成側)が同じ実装を共有する。
pub fn pkce_challenge(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(hasher.finalize())
}

/// RFC 2104 の HMAC-SHA256(block=64、鍵が block より長ければ一度ハッシュ)。
/// deploy hook の署名で server(検証側、生 body に対して計算)と CLI の
/// `tbm deploy --local`(生成側)が**同じ実装**を使う(片方だけ変えると署名が割れる)。
/// hmac crate を足さず、既存の sha2 を直接使う(版衝突回避)。
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(msg);
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner);
    outer.finalize().into()
}

// ============ API レスポンス型 ============

/// `GET /api/health` のレスポンスボディ。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Health {
    pub status: String,
    pub version: String,
}

/// `GET /api/auth/info` のレスポンスボディ。未ログインでも読める公開情報で、
/// ログイン画面が「どの会社ドメインで入れるか」を表示するために使う。
/// 秘密ではない(誰でもログイン可能なドメインは知れるべき情報)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthInfo {
    /// ログインを許可された Google Workspace ドメイン(`TSUBOMI_ALLOWED_HD`)。
    pub allowed_domains: Vec<String>,
    /// 外部(human)接続文字列機能が有効か(`TSUBOMI_DB_PUBLIC_ENABLED`)。off の部署
    /// (CF Tunnel 等、公網 TCP 入口なし)では web が DB の接続文字列カードを隠す。
    /// 秘密ではない(機能の有無を示すだけ)。古いクライアント互換のため `serde(default)`。
    #[serde(default)]
    pub db_public_enabled: bool,
    /// キャッシュの外部(`rediss://`)接続文字列機能が有効か(`TSUBOMI_CACHE_PUBLIC_ENABLED`)。
    /// on の部署では web が cache 詳細で「手元から繋がる外部串」カードを出す(off は内部串の控えのまま)。
    /// 秘密ではない。古いクライアント互換のため `serde(default)`。
    #[serde(default)]
    pub cache_public_enabled: bool,
}

/// `GET /api/auth/me` のレスポンスボディ。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Me {
    pub user_id: String,
    pub email: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
    /// `"user"` か `"owner"`。
    pub role: String,
    /// このセッションが現在、有効な共有パスワード viewer grant を持つか
    /// (web 専用・8h で失効)。前端の閲覧ルート守衛が `role=="owner" || is_viewer`
    /// で判定する(表示制御は UX、実防御は後端ゲート)。
    #[serde(default)]
    pub is_viewer: bool,
}

// ============ M1 database(server ⇄ CLI / web の単一契約)============

/// `GET /api/databases` の各要素 / `POST /api/databases` のレスポンス。
/// 秘密(パスワード / 接続文字列)は含まない — それは `/url` 専用。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseDto {
    pub id: Uuid,
    /// ユーザの自由名(改名は接続文字列に触れない)。
    pub display_name: String,
    /// 匿名番号(user+kind 内連番):database1/2…
    pub anon_seq: i32,
    pub created_at: DateTime<Utc>,
    /// human role の最後の rotate 時刻。これより前にコピーした文字列は失効済み。
    #[serde(default)]
    pub rotated_at: Option<DateTime<Utc>>,
    /// この DB の 1 ロールあたりの最大接続数(human role の CONNECTION LIMIT)。
    /// この欄が無い旧サーバ応答との後方互換のため既定値を持つ。旧サーバ時代の DB は
    /// 旧上限 20 なので、フォールバックは 20(新規 DB の既定 100 とは別概念)。
    #[serde(default = "default_conn_limit")]
    pub conn_limit: i32,
}

fn default_conn_limit() -> i32 {
    20
}

/// `GET /api/databases/:id/capacity` のレスポンス。接続枠の上限 + 実時の使用量。
/// 「合法接続で枠を食い潰さないか」をユーザ/AI が判断するための可視化(web 詳細 / `tbm db info`)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseCapacityDto {
    /// 1 ロールあたりの接続上限(CONNECTION LIMIT)。
    pub conn_limit: i32,
    /// human role(公開接続文字列)が今握っている接続数。
    pub human_connections: i32,
    /// app role(service 注入)が今握っている接続数。
    pub app_connections: i32,
    /// pgbouncer のプール方式(説明用)。例: "transaction"。
    pub pool_mode: String,
}

/// `POST /api/databases` のリクエストボディ。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateDatabaseReq {
    pub name: String,
}

/// `PATCH /api/databases/:id`:表示名のリネーム(接続文字列・dbname は不変)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameDatabaseReq {
    pub name: String,
}

/// `GET /api/databases/:id/url` / `POST /api/databases/:id/rotate` のレスポンス。
/// 外部(human role)接続文字列。**パスワードそのもの** — 表示箇所で警告すること。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionUrlResp {
    pub url: String,
}

// ============ M5 cache(server ⇄ CLI / web の単一契約)============

/// `GET /api/caches` の各要素 / `POST /api/caches` のレスポンス。
/// 秘密(接続文字列 / パスワード)は含まない — それは `/url` 専用(database と同じ規律)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheDto {
    pub id: Uuid,
    /// ユーザの自由名(改名は接続文字列・namespace に触れない)。
    pub display_name: String,
    /// 匿名番号(user+kind 内連番):cache1/2…
    pub anon_seq: i32,
    pub created_at: DateTime<Utc>,
    /// 最後の rotate 時刻。これより前にコピーした接続文字列は失効済み。
    #[serde(default)]
    pub rotated_at: Option<DateTime<Utc>>,
}

/// `POST /api/caches` のリクエストボディ。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateCacheReq {
    pub name: String,
}

/// `PATCH /api/caches/:id`:表示名のリネーム(接続文字列・namespace は不変)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameCacheReq {
    pub name: String,
}

/// `GET /api/caches/:id`:単体詳細。一覧(CacheDto)に **namespace と key 数**を足す
/// (key_prefix = `<namespace>:`。key_count は valkey を SCAN した概算 = §4.2。取得不能は null)。
/// 接続文字列は秘密なので含まない(`/url` 専用)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheDetailDto {
    pub id: Uuid,
    pub display_name: String,
    pub anon_seq: i32,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub rotated_at: Option<DateTime<Utc>>,
    /// key 前缀のもと。`REDIS_KEY_PREFIX` = `<namespace>:`。
    pub namespace: String,
    /// namespace 配下の key 数(SCAN 概算)。取得不能(valkey 不通)は null。
    #[serde(default)]
    pub key_count: Option<i64>,
}

/// `POST /api/databases/:id/query`(web SQL / `tbm db query`)のリクエスト。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryReq {
    pub sql: String,
    /// 位置パラメータ($1..$n)の束(`tbm db query --param`)。空 = 従来どおり
    /// 単純クエリプロトコル(複数文可)。非空 = 拡張プロトコル(単一文・bind)。
    /// `Option<String>` の None は SQL NULL を束縛する。旧クライアント互換のため
    /// `serde(default)`(web の `{sql}` ボディはそのまま通る)。
    #[serde(default)]
    pub params: Vec<Option<String>>,
}

/// web SQL の 1 文ぶんの結果。SELECT 系は columns/rows、それ以外(INSERT/UPDATE/
/// CREATE 等)は columns 空 + rows_affected。値はすべて text 表現(NULL は None)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResultSet {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    /// 返した行数(上限で切り詰めた場合は truncated=true)。
    pub row_count: usize,
    pub truncated: bool,
    /// SELECT 以外の影響行数(SELECT は 0)。
    pub rows_affected: u64,
}

/// web SQL の結果。複数文を投げると文ごとに 1 集合ずつ返る(結果が混ざらない)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResp {
    pub results: Vec<QueryResultSet>,
}

/// `GET /api/resources`:4 種をフラットに(dashboard 用)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceDto {
    pub id: Uuid,
    pub kind: String,
    pub display_name: String,
    pub anon_seq: i32,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub deleted_at: Option<DateTime<Utc>>,
}

/// `GET /api/trash`:ソフト削除済みリソース。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrashItemDto {
    pub id: Uuid,
    pub kind: String,
    pub display_name: String,
    pub deleted_at: DateTime<Utc>,
    #[serde(default)]
    pub purge_after: Option<DateTime<Utc>>,
}

// ============ M2 volume(server ⇄ CLI / web の単一契約)============

/// `GET /api/volumes` の各要素 / `POST /api/volumes` のレスポンス。
/// volume は顶层リソース。host_path(物理パス)は公開しない — 假根の中だけを見せる。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeDto {
    pub id: Uuid,
    /// ユーザの自由名(改名は host_path に触れない)。
    pub display_name: String,
    /// 匿名番号(user+kind 内連番):volume1/2…
    pub anon_seq: i32,
    pub created_at: DateTime<Utc>,
}

/// `POST /api/volumes` のリクエストボディ。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateVolumeReq {
    pub name: String,
}

/// `PATCH /api/volumes/:id`:表示名のリネーム(host_path は不変)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameVolumeReq {
    pub name: String,
}

/// ディレクトリ内の 1 エントリ(`GET /api/volumes/:id/files`)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntryDto {
    pub name: String,
    pub is_dir: bool,
    /// ファイルのバイト数(ディレクトリは 0)。
    pub size: u64,
    /// 最終更新時刻(取得不能なら None)。
    #[serde(default)]
    pub modified: Option<DateTime<Utc>>,
}

/// `GET /api/volumes/:id/files?path=` のレスポンス(ディレクトリ列挙)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListDirResp {
    /// 假根からの正規化済み相対パス(root は "")。
    pub path: String,
    pub entries: Vec<FileEntryDto>,
}

/// `POST /api/volumes/:id/move`:同一 volume 内の rename / move。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveReq {
    pub from: String,
    pub to: String,
}

/// `GET /api/volumes/:id/usage`:卷の使用量(概要ページ用)。假根を再帰的に走査して
/// 集計する(symlink は辿らない)。一覧では出さない — 全卷を走査すると高コストなので。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeUsageDto {
    pub size_bytes: u64,
    pub file_count: u64,
    pub dir_count: u64,
    /// 走査が時間予算を超えて打ち切られた = 値は下限(UI は「≥」表示)。
    pub truncated: bool,
}

// ============ M3 service(server ⇄ CLI / web の単一契約)============

/// `GET /api/services` の各要素 / service 詳細。秘密(deploy_key)は含まない —
/// それは作成時のレスポンスでしか出さない。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceDto {
    pub id: Uuid,
    /// ユーザの自由名(改名は subdomain / 接続に触れない)。
    pub display_name: String,
    /// 匿名番号(user+kind 内連番):service1/2…
    pub anon_seq: i32,
    pub created_at: DateTime<Utc>,
    /// `<subdomain>.<domain>` の左辺。
    pub subdomain: String,
    /// 観測された実際の段階:created / deploying / running / stopped / failed。
    pub phase: String,
    /// 期望状態:running / stopped。
    pub desired_state: String,
    /// app が容器内で listen する port(traefik の転送先)。
    pub container_port: i32,
    /// 現在走るべきイメージ(まだ deploy していなければ None)。
    #[serde(default)]
    pub image_digest: Option<String>,
    #[serde(default)]
    pub last_deploy_at: Option<DateTime<Utc>>,
    /// 公開 URL(`<scheme>://<subdomain>.<domain>`)。scheme はサーバ設定由来
    /// (dev=localhost→http / prod=https)。サーバが算出して載せる(web/CLI 共通)。
    /// 古いサーバ相手でも壊れないよう default(空文字)を許す。
    #[serde(default)]
    pub url: String,
    /// 公開範囲:private(route 無し = 公網不可視)/ company(既定 = 会社 IP のみ)/
    /// public(全網)。旧サーバ相手は default(空文字)= company 扱い。
    #[serde(default)]
    pub visibility: String,
    /// true = 有状態(deploy は stop-first:数秒瞬断と引き換えにデータ目録を単独占有。
    /// 自帯 DB 等)。false(既定)= start-first swap(無瞬断)。旧サーバ相手は default(false)。
    #[serde(default)]
    pub stateful: bool,
    /// メモリ硬上限 MiB。旧サーバ相手は default(0 = 不明)— CLI の作成回显検証が
    /// 「指定値が反映されていない」を正しく検出できる値にしておく。
    #[serde(default)]
    pub memory_mb: i32,
    /// CPU 硬上限(millicores、1000 = 1 CPU)。None = 硬上限なし(cpu_shares のソフト
    /// 権重のみ)。旧サーバ相手は default(None)。
    #[serde(default)]
    pub cpu_limit_millis: Option<i32>,
}

/// `POST /api/services` のリクエストボディ。name 以外は任意 — 省略時の既定
/// (port 8080 / visibility は port から推導 / stateful false / memory 1024)は
/// **サーバが単一真源**として決める(CLI / web は None を素通しする)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateServiceReq {
    pub name: String,
    /// app が容器内で listen する port(省略 = 8080)。
    #[serde(default)]
    pub container_port: Option<i32>,
    /// 公開範囲(省略 = port から推導:8080 → company / それ以外 → private)。
    #[serde(default)]
    pub visibility: Option<String>,
    /// true = 有状態コンテナとして作成(deploy が stop-first になる)。省略 = false。
    #[serde(default)]
    pub stateful: Option<bool>,
    /// メモリ硬上限 MiB(省略 = 1024)。
    #[serde(default)]
    pub memory_mb: Option<i32>,
    /// CPU 硬上限(millicores、1000 = 1 CPU。省略 = 硬上限なし)。
    #[serde(default)]
    pub cpu_limit_millis: Option<i32>,
}

/// service の registry 資格情報(GitHub Actions が docker login + push に使う)。
/// **per-user**(per-service ではない — digest ピン留めで per-repo ACL 不要。決定 #3)。
/// `pass` は平文で、**作成レスポンスにだけ**載る。ただし per-user なので、同じユーザが
/// 2 個目以降の service を作るたびに**同じ pass が再度**返る(2 個目の repo の gh secret
/// 設定に要るため。deploy_key の「per-service 1 回限り」とはここが違う)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryCreds {
    /// push 先 host(dev=127.0.0.1:5000・認証なし / prod=registry.<domain>)。
    pub host: String,
    pub user: String,
    pub pass: String,
}

/// `POST /api/services` のレスポンス。秘密(deploy_key / registry.pass)は平文で、この
/// レスポンスにだけ載る(他の API では出さない。表示箇所で警告すること)。
/// - **deploy_key は per-service の 1 回限り**(service ごとに新規。以後取得不可)。
/// - **registry.pass は per-user**(同ユーザの各 service 作成で同じ値が再度返る — RegistryCreds)。
///
/// CLI / web はこの DTO で GitHub 連携(repo / secret / variable / workflow)を組み立てる —
/// 平台は GitHub に一切触れない(ユーザ自身の gh が実行する)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateServiceResp {
    #[serde(flatten)]
    pub service: ServiceDto,
    /// HMAC の鍵(GitHub Secret `TSUBOMI_DEPLOY_KEY`)。
    pub deploy_key: String,
    /// registry 資格情報(GitHub Secret `TSUBOMI_REGISTRY_USER` / `TSUBOMI_REGISTRY_PASS`)。
    pub registry: RegistryCreds,
    /// deploy hook の URL(GitHub Variable `TSUBOMI_HOOK_URL`)。
    pub hook_url: String,
    /// build 対象 arch(GitHub Variable `TSUBOMI_PLATFORMS`、例 `linux/arm64`。§6.6)。
    pub platforms: String,
    /// GHA ランナー(GitHub Variable `TSUBOMI_RUNNER`。平台が platforms から導出 —
    /// arm64 単独なら `ubuntu-24.04-arm` 原生でビルドが桁違いに速い)。旧サーバは
    /// 送ってこないので default(空)— CLI は空なら設定をスキップする。
    #[serde(default)]
    pub runner: String,
    /// `.github/workflows/tsubomi-deploy.yml` のテンプレ(平台が単一真源として配る)。
    pub workflow_yaml: String,
    /// GitHub 連携の手順コマンド列(リポジトリ直下で実行)。平台が **単一真源**として
    /// 組み立てる(workflow_yaml と同じく GitHub 連携契約の一部)。CLI(json の steps /
    /// gh 不在時のフォールバック表示)と web がそのまま表示に使う — 文字列を二重定義しない。
    pub setup_commands: Vec<String>,
}

/// `GET /api/services/:id/deploys` の各要素。デプロイ履歴の 1 行(秘密は含まない)。
/// `tbm service status` / 将来の web 詳細が phase 遷移と履歴を見せるのに使う。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployDto {
    pub id: Uuid,
    pub git_sha: String,
    /// commit の件名(`git log -1 --pretty=%s`)。旧 deploy / 旧 workflow は None。
    #[serde(default)]
    pub commit_message: Option<String>,
    pub image_digest: String,
    /// received / pulling / starting / succeeded / failed(deploys.status)。
    pub status: String,
    /// 失敗理由(人間可読、失敗時のみ)。
    #[serde(default)]
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub finished_at: Option<DateTime<Utc>>,
}

/// `GET /api/services/:id/deploy-config` のレスポンス。`tbm deploy --local` が
/// 自分の service を **GitHub 抜き**で build+push+hook するのに要る全値。
/// **deploy_key / registry.pass を再度平文で返す**(owner 自身の service にだけ。設計 §4b
/// が許す退路 — CI が無い / 緊急時の経路)。表示・ログに残さないこと。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployConfig {
    pub service_id: Uuid,
    pub registry: RegistryCreds,
    pub deploy_key: String,
    pub hook_url: String,
    pub platforms: String,
}

/// デプロイ源の種別(`POST /services/:id/deploy-source` の kind)。DB の
/// `service_details.source_kind` CHECK と対(migration 20260709000001)。
pub const SOURCE_KIND_IMAGE: &str = "image";
pub const SOURCE_KIND_DOCKERFILE: &str = "dockerfile";
/// コンテキスト無し Dockerfile 全文の上限(サーバが強制。CLI も同値で送信前に弾く =
/// 契約なので shared に置く。サーバが上げても古い CLI が過剰拒否しないよう単一真源)。
pub const MAX_DOCKERFILE_BYTES: usize = 8 * 1024;

/// `POST /api/services/:id/deploy-source` のリクエスト。**第 3 のデプロイ経路**:サーバ側で
/// 既成イメージを pull / コンテキスト無し Dockerfile を build して内部 registry へ push し、
/// 既存パイプライン(run_digest)で起こす — GitHub にもローカル docker にも依存しない。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploySourceReq {
    /// "image"(既成イメージ参照)か "dockerfile"(コンテキスト無し Dockerfile 全文)。
    pub kind: String,
    /// kind=image: イメージ参照(例 pgvector/pgvector:pg17)。
    /// kind=dockerfile: Dockerfile 全文(≤8KiB。COPY/ADD 不可 = コンテキスト無しの契約)。
    pub spec: String,
}

/// `POST /api/services/:id/deploy-source` のレスポンス(202)。取得〜起動は非同期 —
/// git_sha を `tbm service verify --wait --for-sha` に渡して完走を待てる。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploySourceResp {
    pub service_id: Uuid,
    pub deploy_id: Uuid,
    /// 合成 sha(sha256(kind‖spec) 先頭 12 hex)。deploys.git_sha / registry tag と同値。
    /// **純 hex**(CLI の sha 判定と互換)。
    pub git_sha: String,
}

/// `GET /api/services/:id/env/resolved` の各要素。**今この瞬間**に注入バインディングを解決した
/// env(コンテナの実際の値は起動の瞬間に解決 — rotate 後は再デプロイで初めて変わる。決定 #5)。
/// URL 形の値はパスワード部を `***` に伏せる(暴露ティアは exec/web SQL と同じ owner 限定だが、
/// コピペ・ログ残留の事故を防ぐ)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedEnvDto {
    pub key: String,
    /// 解決済みの値(URL のパスワード部のみ `***`)。
    pub value: String,
    /// 由来:static / database / volume / cache / service(キー導出の `_KEY_PREFIX` は cache)。
    pub source: String,
}

/// `GET /api/services/:id/injections` の各要素 / `POST` のレスポンス。注入の **バインディング**
/// (値はコンテナ起動の瞬間に解決 — 決定 #5)。秘密(解決後の接続文字列)は含まない。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InjectionDto {
    pub id: Uuid,
    /// 注入元リソース(database / volume)。
    pub resource_id: Uuid,
    pub resource_kind: String,
    pub resource_name: String,
    /// 注入先 env 変数名(database 既定 DATABASE_URL / volume 既定 STORAGE_PATH)。
    pub env_var: String,
    /// volume のみ:コンテナ内マウント先。
    #[serde(default)]
    pub mount_path: Option<String>,
    /// 注入元が生きているか(ソフト削除済み = false = 失効。service は起動するが env は出ない)。
    pub valid: bool,
}

/// `POST /api/services/:id/injections` のリクエスト。env_var / mount_path 省略時は kind 既定。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateInjectionReq {
    pub resource_id: Uuid,
    #[serde(default)]
    pub env_var: Option<String>,
    #[serde(default)]
    pub mount_path: Option<String>,
}

/// `POST /api/services/:id/env` のリクエスト(静的 env を 1 件 upsert。値は暗号化保存)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetEnvReq {
    pub key: String,
    pub value: String,
}

/// `POST /api/services/:id/env` のレスポンス。`warning` は注意喚起がある場合だけ入る
/// (例:値が公開 DB ホストを指す = 静的 env でなく注入を使うべき footgun)。値自体は秘密なので含めない。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SetEnvResp {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// `GET /api/services/:id/logs` のレスポンス。コンテナの直近ログ(text)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogsResp {
    pub logs: String,
}

/// `GET /api/services/:id/metrics` のレスポンス。稼働中コンテナの 1 サンプル指標 +
/// inspect 由来のライフサイクル情報。停止 / 未デプロイでも 200(running=false + 各 None)で
/// 返す — 「なぜ落ちているか」を見るのが主用途なので、不在を答えとして扱う。全フィールドは
/// 取得不能時 None(旧クライアント / 停止コンテナで欠落しても壊れないよう `serde(default)`)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceMetricsDto {
    /// コンテナが今 running か(inspect の State.Running)。
    pub running: bool,
    /// CPU 使用率(%)。算出には 2 サンプル要るので停止中 / 取得不能は None。
    #[serde(default)]
    pub cpu_pct: Option<f64>,
    /// メモリ使用量(bytes)。
    #[serde(default)]
    pub mem_bytes: Option<i64>,
    /// メモリ硬上限(bytes、`--memory`)。無制限なら宿主機 RAM だが tsubomi は必ず設定する。
    #[serde(default)]
    pub mem_limit_bytes: Option<i64>,
    /// 起動以来の再起動回数(inspect の RestartCount)。OOM / クラッシュループの手がかり。
    #[serde(default)]
    pub restart_count: Option<i64>,
    /// 現コンテナの起動時刻(RFC3339)。
    #[serde(default)]
    pub started_at: Option<String>,
    /// 起動からの経過秒(サーバが started_at から算出)。
    #[serde(default)]
    pub uptime_secs: Option<i64>,
    /// 直近の終了が OOM だったか(inspect の State.OOMKilled)。
    #[serde(default)]
    pub oom_killed: Option<bool>,
}

/// `POST /api/services/:id/rollback` のリクエスト。`deploys` 履歴の戻し先。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollbackReq {
    pub deploy_id: Uuid,
}

/// `POST /api/services/:id/visibility` のリクエスト。公開範囲の切替(即時反映・再デプロイ不要)。
/// 値は `private` / `company` / `public` のいずれか(それ以外はサーバが 400)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetServiceVisibilityReq {
    pub visibility: String,
}

/// `POST /api/services/:id/exec` のリクエスト。稼働中コンテナ内で **1 コマンド**を
/// 非対話に実行する(`docker exec`(`-it` なし)相当 = AI / スクリプト / 線上診断用。
/// 対話シェルは web ターミナルが担う)。argv はそのまま渡す(shell 解釈なし):
/// pipe/glob が要るときは呼び出し側が `["sh","-c","ps | grep node"]` と組む。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecReq {
    /// 実行する argv(空は 400)。
    pub cmd: Vec<String>,
}

/// `POST /api/services/:id/exec` のレスポンス。捕獲した stdout/stderr と終了コード。
/// `exit_code` は **データ**(コマンドが非零で終わってもリクエスト自体は成功 = CLI は 0 終了、
/// AI は JSON のこの値を読む)。inspect で確定できない場合のみ None。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i64>,
    /// 出力が上限を超えて打ち切られたか。
    pub truncated: bool,
    /// サーバ側タイムアウトで打ち切ったか(長時間コマンドは web ターミナルへ誘導)。
    pub timed_out: bool,
}

// ============ ガバナンス:IP 許可リスト(server ⇄ CLI / web の単一契約)============

/// `GET /api/ip-allowlist` の各要素。会社 IP 許可リストの 1 エントリ。
/// 許可リストが空 = 制限なし(全 IP 許可、fail-open)。1 件以上ある時だけ、
/// 列挙された CIDR だけが service に到達でき、他は traefik の ipAllowList で遮断される。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpAllowEntryDto {
    pub id: Uuid,
    /// 正規化済み CIDR(例:203.0.113.0/24 / 198.51.100.7/32)。
    pub cidr: String,
    /// 何の IP かの人間用メモ(空可)。
    pub note: String,
    pub created_at: DateTime<Utc>,
}

/// `POST /api/ip-allowlist` のリクエストボディ。`cidr` は単一 IP(/32・/128 に正規化)
/// でも CIDR レンジでも受ける。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateIpAllowReq {
    pub cidr: String,
    #[serde(default)]
    pub note: String,
}

// ============ ガバナンス:管制面の可視化(M4 S1、owner 専用・web)============

/// `GET /api/admin/ranking` の各行 / overview の素材。匿名化済み(設計 v2 §7):
/// **ユーザの真名は出すが、資源は `display_name` ではなく匿名番号**(`service1` 等)。
/// 資源の内容(DB の中身 / ファイル / env 明文)は一切載せない。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminResourceRow {
    /// 資源 ID(不透明な UUID。名前/内容ではないので匿名化を破らない)。owner 専用で、
    /// S3「最後の砦」の代理操作(stop/delete)が対象を指すのに使う。
    pub resource_id: Uuid,
    /// 資源の所有者の真名(users.name、無ければ email)。
    pub owner_name: String,
    pub kind: String,
    /// 匿名ラベル `<kind><anon_seq>`(例:service1 / database2 / volume1)。display_name は出さない。
    pub anon_label: String,
    /// ソート対象の使用量(bytes)。database=存储 / volume=占用 / service=内存(稼働中)。
    /// 取得不能(停止中 service / 計測タイムアウト / 未対応の cache 等)は null。
    pub usage_bytes: Option<i64>,
    /// service のみ:CPU 使用率(%)。取得不能 / 停止中は null。
    pub cpu_pct: Option<f64>,
    /// service のみ:コンテナが稼働中か。他種別は null。
    pub running: Option<bool>,
}

/// `GET /api/admin/overview` の種別ごと集計。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminOverviewKind {
    pub kind: String,
    pub count: i64,
    /// 使用量の合計(bytes)。種別内で意味は単一(service=稼働中内存 / database=存储 /
    /// volume=占用)。取得できなかった分は 0 として加算。
    pub total_usage_bytes: i64,
}

/// `GET /api/admin/overview` のレスポンス。匿名化された全体サマリ。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminOverviewResp {
    /// 資源を 1 つ以上持つ(削除されていない)ユーザ数。
    pub user_count: i64,
    /// kind は service / database / volume の固定順(cache は M5)。
    pub kinds: Vec<AdminOverviewKind>,
}

/// `POST /api/admin/resources/:id/{stop|delete}`(最後の砦・S3)のリクエスト。
/// code 無し = 1 段目(コードを生成して owner にメール)、code 有り = 2 段目(検証して実行)。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AdminActionReq {
    #[serde(default)]
    pub code: Option<String>,
}

/// 同レスポンス。`code_required=true` = 1 段目(メールのコードを入れて再送)、false = 実行済み。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminActionResp {
    pub code_required: bool,
}

/// `GET /api/admin/audit` の各行(監査ログ閲覧・S4、owner 専用・web)。actor / target_user は
/// 真名(name、無ければ email)で join 済み。target_resource は匿名化のため UUID のまま
/// (資源名は出さない)。detail は非機密の jsonb(cidr / kind / 用量など)をそのまま。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntryDto {
    /// audit_log.id(BIGINT)。キーセット分頁のカーソルにも使う(id DESC)。
    pub id: i64,
    pub created_at: DateTime<Utc>,
    /// 'owner.delete_service' / 'db.rotate' / 'disk.alert' など。
    pub action: String,
    /// 操作者の真名。システム操作(reconcile の自動 purge 等)は null。
    pub actor_name: Option<String>,
    /// 代理操作の対象ユーザの真名(owner.* のみ)。それ以外は null。
    pub target_user_name: Option<String>,
    /// 対象資源(UUID のまま。資源名は出さない)。
    pub target_resource: Option<Uuid>,
    /// 付帯情報(非機密の jsonb)。
    pub detail: Option<serde_json::Value>,
    /// 操作元の client IP(`CF-Connecting-IP`)。background / dev / 旧行は null。
    pub client_ip: Option<String>,
}

// ============ ガバナンス:共有パスワード viewer(M4 S5、web 専用)============
// design v2 §7「見るは共有密码」— ログイン済み社内ユーザが共有パスワードを入れると
// 管制面を**只读**で見られる(overview / ranking)。owner は設定 / リセットでき、
// リセットすると旧 grant は全失効する。

/// `POST /api/admin/viewer/login` のリクエスト。共有パスワード平文(HTTPS 前提)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewerLoginReq {
    pub password: String,
}

/// `POST /api/admin/viewer/password`(owner)のリクエスト。新しい共有パスワード。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewerPasswordReq {
    pub password: String,
}

/// `GET /api/admin/viewer/password`(owner)のレスポンス。設定ページ表示用。
/// パスワード本体・hash は返さない(設定済みか否かとメタだけ)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewerStatusResp {
    pub set: bool,
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
    /// 最後に設定した owner の真名(無ければ null)。
    #[serde(default)]
    pub updated_by_name: Option<String>,
}

// ============ ガバナンス:owner 管理(M4 後相、web 専用)============
// design v2 §7:最多 2 名の対等 owner。env は冷启动种のみ、運用中は web で増減する。
// 真相は users.role(毎リクエスト実時読)。owner_roster(platform_config)は未ログイン
// email の「昇格すべき」意図 + 宛先を保持するだけ。

/// `GET /api/admin/owners` の各行。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminOwnerDto {
    pub email: String,
    /// users 行があれば真名(無ければ null)。
    #[serde(default)]
    pub name: Option<String>,
    /// 操作中の本人か(前端で自分の削除ボタンを無効化する)。
    pub is_current: bool,
    /// 既にログイン済み(users 行があり role=owner)= 有効。false = 次回ログインで昇格。
    pub registered: bool,
}

/// `POST /api/admin/owners`(追加)/ `POST /api/admin/owners/remove`(削除)のリクエスト。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnerEmailReq {
    pub email: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 7636 Appendix B のテストベクタ。実装が 1 箇所になったので
    /// このテストも 1 箇所だけで足りる。
    #[test]
    fn pkce_challenge_rfc7636() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let expected = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        assert_eq!(pkce_challenge(verifier), expected);
    }

    #[test]
    fn random_b64_shape() {
        // 32 bytes → ceil(32 * 4/3) = 43 文字、パディング無し。毎回違う値。
        let a = random_b64(32);
        let b = random_b64(32);
        assert_eq!(a.len(), 43);
        assert_ne!(a, b);
        assert_eq!(random_b64(16).len(), 22);
    }

    /// RFC 4231 Test Case 2(key="Jefe")の HMAC-SHA256 既知ベクタ。
    /// server / CLI 共有の実装が正しいことを固定する。
    #[test]
    fn hmac_sha256_rfc4231_case2() {
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            hex::encode(mac),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn sha256_hex_is_lowercase_hex() {
        let h = sha256_hex("tbm_test");
        assert_eq!(h.len(), 64);
        assert!(
            h.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        assert_eq!(sha256_hex("tbm_test"), h); // 安定
    }
}
