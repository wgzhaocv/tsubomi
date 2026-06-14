use anyhow::{Context, Result};
use futures_util::StreamExt;
use tokio::io::AsyncWriteExt;
use tsubomi_shared::{
    ConnectionUrlResp, CreateDatabaseReq, CreateInjectionReq, CreateServiceReq, CreateServiceResp,
    CreateVolumeReq, DatabaseDto, DeployConfig, DeployDto, Health, InjectionDto, ListDirResp,
    LogsResp, Me, MoveReq, RenameVolumeReq, RollbackReq, ServiceDto, SetEnvReq, TrashItemDto,
    VolumeDto,
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

// ============ M3 service ============

pub async fn service_list(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
) -> Result<Vec<ServiceDto>> {
    let resp = send_ok(
        c.get(format!("{server_url}/api/services"))
            .bearer_auth(token),
    )
    .await?;
    resp.json()
        .await
        .context("failed to parse services response")
}

/// service を作成し、GitHub 連携に必要な全値(deploy_key / registry creds / workflow など)を
/// 受け取る。deploy_key / registry.pass はこの 1 回しか平文で返らない。
pub async fn service_create(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    name: &str,
) -> Result<CreateServiceResp> {
    let resp = send_ok(
        c.post(format!("{server_url}/api/services"))
            .bearer_auth(token)
            .json(&CreateServiceReq {
                name: name.to_owned(),
            }),
    )
    .await?;
    resp.json()
        .await
        .context("failed to parse create service response")
}

pub async fn service_get(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    id: &str,
) -> Result<ServiceDto> {
    let resp = send_ok(
        c.get(format!("{server_url}/api/services/{id}"))
            .bearer_auth(token),
    )
    .await?;
    resp.json().await.context("failed to parse service response")
}

pub async fn service_deploys(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    id: &str,
) -> Result<Vec<DeployDto>> {
    let resp = send_ok(
        c.get(format!("{server_url}/api/services/{id}/deploys"))
            .bearer_auth(token),
    )
    .await?;
    resp.json().await.context("failed to parse deploys response")
}

/// `tbm deploy --local` 用の build+hook 情報(deploy_key / registry creds を含む。自分の service のみ)。
pub async fn deploy_config(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    id: &str,
) -> Result<DeployConfig> {
    let resp = send_ok(
        c.get(format!("{server_url}/api/services/{id}/deploy-config"))
            .bearer_auth(token),
    )
    .await?;
    resp.json()
        .await
        .context("failed to parse deploy-config response")
}

// ============ M3 service lifecycle ============

pub async fn service_start(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    id: &str,
) -> Result<()> {
    send_ok(
        c.post(format!("{server_url}/api/services/{id}/start"))
            .bearer_auth(token),
    )
    .await?;
    Ok(())
}

pub async fn service_stop(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    id: &str,
) -> Result<()> {
    send_ok(
        c.post(format!("{server_url}/api/services/{id}/stop"))
            .bearer_auth(token),
    )
    .await?;
    Ok(())
}

pub async fn service_delete(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    id: &str,
) -> Result<()> {
    send_ok(
        c.delete(format!("{server_url}/api/services/{id}"))
            .bearer_auth(token),
    )
    .await?;
    Ok(())
}

pub async fn service_logs(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    id: &str,
    tail: Option<usize>,
) -> Result<String> {
    // この最小ビルドの reqwest は .query() が無効なので URL に直接組む。
    let url = match tail {
        Some(n) => format!("{server_url}/api/services/{id}/logs?tail={n}"),
        None => format!("{server_url}/api/services/{id}/logs"),
    };
    let resp = send_ok(c.get(url).bearer_auth(token)).await?;
    let r: LogsResp = resp.json().await.context("failed to parse logs response")?;
    Ok(r.logs)
}

pub async fn service_rollback(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    id: &str,
    deploy_id: &str,
) -> Result<()> {
    send_ok(
        c.post(format!("{server_url}/api/services/{id}/rollback"))
            .bearer_auth(token)
            .json(&RollbackReq {
                deploy_id: deploy_id.parse().context("invalid deploy id")?,
            }),
    )
    .await?;
    Ok(())
}

// ============ M3 注入 / 静的 env ============

pub async fn inject_list(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    service_id: &str,
) -> Result<Vec<InjectionDto>> {
    let resp = send_ok(
        c.get(format!("{server_url}/api/services/{service_id}/injections"))
            .bearer_auth(token),
    )
    .await?;
    resp.json()
        .await
        .context("failed to parse injections response")
}

pub async fn inject_create(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    service_id: &str,
    resource_id: &str,
    env_var: Option<&str>,
    mount_path: Option<&str>,
) -> Result<InjectionDto> {
    let req = CreateInjectionReq {
        resource_id: resource_id.parse().context("invalid resource id")?,
        env_var: env_var.map(str::to_owned),
        mount_path: mount_path.map(str::to_owned),
    };
    let resp = send_ok(
        c.post(format!("{server_url}/api/services/{service_id}/injections"))
            .bearer_auth(token)
            .json(&req),
    )
    .await?;
    resp.json()
        .await
        .context("failed to parse injection response")
}

pub async fn inject_delete(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    injection_id: &str,
) -> Result<()> {
    send_ok(
        c.delete(format!("{server_url}/api/injections/{injection_id}"))
            .bearer_auth(token),
    )
    .await?;
    Ok(())
}

pub async fn env_keys(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    service_id: &str,
) -> Result<Vec<String>> {
    let resp = send_ok(
        c.get(format!("{server_url}/api/services/{service_id}/env"))
            .bearer_auth(token),
    )
    .await?;
    resp.json().await.context("failed to parse env response")
}

pub async fn env_set(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    service_id: &str,
    key: &str,
    value: &str,
) -> Result<()> {
    send_ok(
        c.post(format!("{server_url}/api/services/{service_id}/env"))
            .bearer_auth(token)
            .json(&SetEnvReq {
                key: key.to_owned(),
                value: value.to_owned(),
            }),
    )
    .await?;
    Ok(())
}

pub async fn env_unset(
    c: &reqwest::Client,
    server_url: &str,
    token: &str,
    service_id: &str,
    key: &str,
) -> Result<()> {
    // key は任意文字を含みうるのでパスセグメントとしてエンコードする。
    let mut url =
        url::Url::parse(&format!("{server_url}/api/services/{service_id}/env")).context("invalid server url")?;
    url.path_segments_mut()
        .map_err(|()| anyhow::anyhow!("invalid url base"))?
        .push(key);
    send_ok(c.delete(url).bearer_auth(token)).await?;
    Ok(())
}

/// deploy hook を叩く(no-auth、HMAC)。**署名済みの生バイトをそのまま**送る
/// (再シリアライズすると署名が割れる)。受理は 202、それ以外は ApiError。
pub async fn post_deploy_hook(
    c: &reqwest::Client,
    hook_url: &str,
    signature_hex: &str,
    body: Vec<u8>,
) -> Result<()> {
    let resp = c
        .post(hook_url)
        .header("content-type", "application/json")
        .header("x-tsubomi-signature", signature_hex)
        .body(body)
        .send()
        .await
        .context("hook の送信に失敗しました")?;
    if resp.status().is_success() {
        return Ok(());
    }
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    Err(ApiError {
        code: code_for(status),
        message: if text.is_empty() {
            format!("hook が失敗しました(HTTP {status})")
        } else {
            text
        },
    }
    .into())
}
