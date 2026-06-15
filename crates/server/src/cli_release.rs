//! tbm CLI のリリース manifest・インストールスクリプト・アーカイブ本体を
//! 配信する(バージョンチェックの通知と `tbm update`、各 OS のインストーラが
//! 使う)。manifest は `<TSUBOMI_RELEASE_DIR>/latest/manifest.json`。
//! env 未設定 / ファイル無しは 404 — CLI 側は「リリース未発行」とみなして
//! 沈黙する。

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderName, StatusCode, header};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct VersionInfo {
    pub version: String,
    pub targets: Vec<TargetInfo>,
}

#[derive(Serialize, Deserialize)]
pub struct TargetInfo {
    pub target: String,
    pub url: String,
    pub sha256: String,
}

/// ターゲット単位のフラットな形。install スクリプトがネストした配列を
/// パースしなくて済むようにする。
#[derive(Serialize)]
pub struct TargetVersionInfo {
    pub version: String,
    pub target: String,
    pub url: String,
    pub sha256: String,
}

async fn read_manifest(state: &AppState) -> AppResult<VersionInfo> {
    let dir = state
        .config
        .release_dir
        .as_ref()
        .ok_or(AppError::NotFound)?;
    let path = dir.join("latest/manifest.json");
    let bytes = match tokio::fs::read(&path).await {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(AppError::NotFound),
        Err(e) => {
            tracing::warn!(error = ?e, path = %path.display(), "failed to read CLI manifest");
            return Err(AppError::Io(e));
        }
    };
    serde_json::from_slice(&bytes).map_err(|e| {
        tracing::error!(error = ?e, path = %path.display(), "failed to parse CLI manifest");
        AppError::Json(e)
    })
}

pub async fn version(State(state): State<AppState>) -> AppResult<Json<VersionInfo>> {
    Ok(Json(read_manifest(&state).await?))
}

pub async fn version_target(
    State(state): State<AppState>,
    Path(target): Path<String>,
) -> AppResult<Json<TargetVersionInfo>> {
    let manifest = read_manifest(&state).await?;
    let info = manifest
        .targets
        .into_iter()
        .find(|t| t.target == target)
        .ok_or(AppError::NotFound)?;
    Ok(Json(TargetVersionInfo {
        version: manifest.version,
        target: info.target,
        url: info.url,
        sha256: info.sha256,
    }))
}

// ---- インストールスクリプト ----
// スクリプトはバイナリに埋め込み、配信時に __SERVER_URL__ を実際の
// TSUBOMI_SERVER_URL に置換する。スクリプト自体はどのデプロイ先でも同一で、
// ドメインはサーバが注入する(多ドメイン展開のため、何も書き換えずに済む)。

fn serve_script(
    state: &AppState,
    body: &str,
    content_type: &'static str,
) -> axum::response::Response {
    // インストールスクリプトは可変(ドメインを注入する / 内容を直す)。Cloudflare の
    // エッジにキャッシュされると修正版を出してもしばらく旧版が配られる(実害あり:
    // LF 版が居座って Windows が壊れた)。no-store で edge・ブラウザ双方のキャッシュを
    // 止める。CDN-Cache-Control は Cloudflare が個別に尊重するので併記。
    // ※ 既にキャッシュ済みのエントリは TTL 切れまで残る — 切替直後は手動 purge が要る。
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "no-store, must-revalidate"),
            (HeaderName::from_static("cdn-cache-control"), "no-store"),
        ],
        body.replace("__SERVER_URL__", &state.config.server_url),
    )
        .into_response()
}

pub async fn install_sh(State(state): State<AppState>) -> axum::response::Response {
    serve_script(
        &state,
        include_str!("../scripts/install.sh"),
        "text/x-shellscript; charset=utf-8",
    )
}

pub async fn install_ps1(State(state): State<AppState>) -> axum::response::Response {
    serve_script(
        &state,
        include_str!("../scripts/install.ps1"),
        "text/plain; charset=utf-8",
    )
}

pub async fn install_bat(State(state): State<AppState>) -> axum::response::Response {
    // install.bat は 2 つの Windows 制約を満たす必要がある:
    //  (1) 改行は CRLF 必須。LF のみだと goto / for / 遅延展開が壊れトークンが
    //      途中で千切れる(EXPECTED_SHA → CTED_SHA 等)。リポジトリ実体は LF・
    //      サーバは Linux なので include_str! も LF — ここで CRLF へ正規化する。
    //  (2) 中身は ASCII のみ(install.bat 側で担保)。cmd は OEM コードページ
    //      (日本語 Windows は cp932)でバッチを解釈するため、UTF-8 の日本語を
    //      混ぜると Shift-JIS として誤対合され、空白や行境界を食って REM の断片を
    //      コマンド実行してしまう。よって install.bat だけは日本語コメント禁止。
    let crlf = include_str!("../scripts/install.bat")
        .replace("\r\n", "\n")
        .replace('\n', "\r\n");
    serve_script(&state, &crlf, "text/plain; charset=utf-8")
}
