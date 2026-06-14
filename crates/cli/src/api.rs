use anyhow::{Context, Result};
use futures_util::StreamExt;
use tokio::io::AsyncWriteExt;
use tsubomi_shared::{
    ConnectionUrlResp, CreateDatabaseReq, CreateVolumeReq, DatabaseDto, Health, ListDirResp, Me,
    MoveReq, RenameVolumeReq, TrashItemDto, VolumeDto,
};

pub const ME_PATH: &str = "/api/auth/me";

/// API 由来のエラー。`code` は安定した機械可読コード(json 出力のエラー信封で使う)。
/// anyhow に載せて伝播し、main で downcast して code を取り出す。
#[derive(Debug)]
pub struct ApiError {
    pub code: &'static str,
    pub message: String,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl std::error::Error for ApiError {}

/// HTTP ステータス → 安定コード(AI が機械分岐できるよう text とは別に持つ)。
fn code_for(status: reqwest::StatusCode) -> &'static str {
    match status.as_u16() {
        401 => "unauthorized",
        403 => "forbidden",
        404 => "not_found",
        409 => "conflict",
        400 | 422 => "validation",
        _ => "server_error",
    }
}

pub async fn fetch_me(server_url: &str, token: &str) -> Result<Me> {
    let resp = reqwest::Client::new()
        .get(format!("{server_url}{ME_PATH}"))
        .bearer_auth(token)
        .send()
        .await
        .context("failed to call /api/auth/me")?;
    let status = resp.status();
    if !status.is_success() {
        let message = if status == reqwest::StatusCode::UNAUTHORIZED {
            "token invalid (run: tbm login)".to_owned()
        } else {
            let body = resp.text().await.unwrap_or_default();
            format!("/api/auth/me failed: HTTP {status} {body}")
        };
        return Err(ApiError {
            code: code_for(status),
            message,
        }
        .into());
    }
    resp.json()
        .await
        .context("failed to parse /api/auth/me response")
}

pub async fn fetch_health(server_url: &str) -> Result<Health> {
    let resp = reqwest::Client::new()
        .get(format!("{server_url}/api/health"))
        .send()
        .await
        .context("failed to call /api/health")?
        .error_for_status()
        .context("/api/health returned an error")?;
    resp.json().await.context("failed to parse health response")
}

// ============ M1 database ============
// db_* は呼び出し側(commands/db.rs)が作る 1 つの reqwest::Client を共有する
// (コマンド毎の TLS 初期化を 1 回に減らす)。send_ok が送信 + ステータス検査の
// 定型を 1 箇所に集約する。

/// 送信 → 非成功なら CLI 向けエラー(サーバの日本語本文を保つ)に変換。
async fn send_ok(rb: reqwest::RequestBuilder) -> Result<reqwest::Response> {
    let resp = rb
        .send()
        .await
        .context("request to tsubomi server failed")?;
    if resp.status().is_success() {
        return Ok(resp);
    }
    let status = resp.status();
    let message = if status == reqwest::StatusCode::UNAUTHORIZED {
        "認証に失敗しました(tbm login を実行してください)".to_owned()
    } else {
        let body = resp.text().await.unwrap_or_default();
        if body.is_empty() {
            format!("HTTP {status}")
        } else {
            body
        }
    };
    Err(ApiError {
        code: code_for(status),
        message,
    }
    .into())
}

pub async fn db_list(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
) -> Result<Vec<DatabaseDto>> {
    let resp = send_ok(
        c.get(format!("{server_url}/api/databases"))
            .bearer_auth(token),
    )
    .await?;
    resp.json()
        .await
        .context("failed to parse databases response")
}

pub async fn db_create(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    name: &str,
) -> Result<DatabaseDto> {
    let resp = send_ok(
        c.post(format!("{server_url}/api/databases"))
            .bearer_auth(token)
            .json(&CreateDatabaseReq {
                name: name.to_owned(),
            }),
    )
    .await?;
    resp.json().await.context("failed to parse create response")
}

pub async fn db_url(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    id: &str,
) -> Result<String> {
    let resp = send_ok(
        c.get(format!("{server_url}/api/databases/{id}/url"))
            .bearer_auth(token),
    )
    .await?;
    let r: ConnectionUrlResp = resp.json().await.context("failed to parse url response")?;
    Ok(r.url)
}

pub async fn db_rotate(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    id: &str,
) -> Result<String> {
    let resp = send_ok(
        c.post(format!("{server_url}/api/databases/{id}/rotate"))
            .bearer_auth(token),
    )
    .await?;
    let r: ConnectionUrlResp = resp
        .json()
        .await
        .context("failed to parse rotate response")?;
    Ok(r.url)
}

pub async fn db_delete(c: &reqwest::Client, server_url: &str, token: &str, id: &str) -> Result<()> {
    send_ok(
        c.delete(format!("{server_url}/api/databases/{id}"))
            .bearer_auth(token),
    )
    .await?;
    Ok(())
}

// ============ M2 volume ============
// `?path=` は reqwest の .query() で URL エンコードする(特殊文字・日本語名対応)。

pub async fn volume_list(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
) -> Result<Vec<VolumeDto>> {
    let resp = send_ok(
        c.get(format!("{server_url}/api/volumes"))
            .bearer_auth(token),
    )
    .await?;
    resp.json()
        .await
        .context("failed to parse volumes response")
}

pub async fn volume_create(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    name: &str,
) -> Result<VolumeDto> {
    let resp = send_ok(
        c.post(format!("{server_url}/api/volumes"))
            .bearer_auth(token)
            .json(&CreateVolumeReq {
                name: name.to_owned(),
            }),
    )
    .await?;
    resp.json().await.context("failed to parse create response")
}

pub async fn volume_rename(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    id: &str,
    name: &str,
) -> Result<VolumeDto> {
    let resp = send_ok(
        c.patch(format!("{server_url}/api/volumes/{id}"))
            .bearer_auth(token)
            .json(&RenameVolumeReq {
                name: name.to_owned(),
            }),
    )
    .await?;
    resp.json().await.context("failed to parse rename response")
}

pub async fn volume_delete(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    id: &str,
) -> Result<()> {
    send_ok(
        c.delete(format!("{server_url}/api/volumes/{id}"))
            .bearer_auth(token),
    )
    .await?;
    Ok(())
}

/// `/api/volumes/<id><sub>?path=<encoded>` の URL を組む。reqwest の query()
/// (この最小ビルドでは無効)を使わず url crate でエンコードする
/// (特殊文字・日本語名を安全に query 値へ)。
fn vol_query_url(server_url: &str, id: &str, sub: &str, path: &str) -> Result<url::Url> {
    let mut u = url::Url::parse(&format!("{server_url}/api/volumes/{id}{sub}"))
        .context("invalid server url")?;
    u.query_pairs_mut().append_pair("path", path);
    Ok(u)
}

pub async fn volume_ls(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    id: &str,
    path: &str,
) -> Result<ListDirResp> {
    let resp = send_ok(
        c.get(vol_query_url(server_url, id, "/files", path)?)
            .bearer_auth(token),
    )
    .await?;
    resp.json().await.context("failed to parse ls response")
}

pub async fn volume_mkdir(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    id: &str,
    path: &str,
) -> Result<()> {
    send_ok(
        c.post(vol_query_url(server_url, id, "/dirs", path)?)
            .bearer_auth(token),
    )
    .await?;
    Ok(())
}

pub async fn volume_rm(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    id: &str,
    path: &str,
) -> Result<()> {
    send_ok(
        c.delete(vol_query_url(server_url, id, "/files", path)?)
            .bearer_auth(token),
    )
    .await?;
    Ok(())
}

pub async fn volume_move(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    id: &str,
    from: &str,
    to: &str,
) -> Result<()> {
    send_ok(
        c.post(format!("{server_url}/api/volumes/{id}/move"))
            .bearer_auth(token)
            .json(&MoveReq {
                from: from.to_owned(),
                to: to.to_owned(),
            }),
    )
    .await?;
    Ok(())
}

/// ローカルファイルをストリームで PUT(全体をメモリに載せない)。返り値は送信バイト数。
pub async fn volume_upload(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    id: &str,
    path: &str,
    local_file: &str,
) -> Result<u64> {
    let file = tokio::fs::File::open(local_file)
        .await
        .with_context(|| format!("ローカルファイルを開けません: {local_file}"))?;
    let len = file
        .metadata()
        .await
        .map(|m| m.len())
        .with_context(|| format!("ローカルファイルの情報を取得できません: {local_file}"))?;
    // ReaderStream でファイルを逐次読みし、reqwest のストリーム body にする(chunked)。
    let body = reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(file));
    send_ok(
        c.put(vol_query_url(server_url, id, "/files", path)?)
            .bearer_auth(token)
            .body(body),
    )
    .await?;
    Ok(len)
}

/// ダウンロードを dest へ逐次書き込み(全体をメモリに載せない)。返り値は書込バイト数。
pub async fn volume_download(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    id: &str,
    path: &str,
    dest_local: &str,
) -> Result<u64> {
    let resp = send_ok(
        c.get(vol_query_url(server_url, id, "/files/download", path)?)
            .bearer_auth(token),
    )
    .await?;
    let mut file = tokio::fs::File::create(dest_local)
        .await
        .with_context(|| format!("ローカルに作成できません: {dest_local}"))?;
    let mut stream = resp.bytes_stream();
    let mut total: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("ダウンロード読み取りエラー")?;
        file.write_all(&chunk).await?;
        total += chunk.len() as u64;
    }
    file.flush().await?;
    Ok(total)
}

// ============ ゴミ箱(M1/M2 共通)============

pub async fn trash_list(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
) -> Result<Vec<TrashItemDto>> {
    let resp = send_ok(c.get(format!("{server_url}/api/trash")).bearer_auth(token)).await?;
    resp.json().await.context("failed to parse trash response")
}

pub async fn trash_restore(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    id: &str,
) -> Result<()> {
    send_ok(
        c.post(format!("{server_url}/api/trash/{id}/restore"))
            .bearer_auth(token),
    )
    .await?;
    Ok(())
}

pub async fn trash_purge(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    id: &str,
) -> Result<()> {
    send_ok(
        c.delete(format!("{server_url}/api/trash/{id}"))
            .bearer_auth(token),
    )
    .await?;
    Ok(())
}
