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

// ============ API レスポンス型 ============

/// `GET /api/health` のレスポンスボディ。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Health {
    pub status: String,
    pub version: String,
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

/// `POST /api/databases/:id/query`(web SQL)のリクエスト。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryReq {
    pub sql: String,
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
