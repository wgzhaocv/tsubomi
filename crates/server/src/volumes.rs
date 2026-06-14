//! volume リソースの API ハンドラ(tech-design §6 の volume 面)。
//! web と CLI は同一ハンドラの 2 入口 — 認証 extractor(AuthCtx)だけが分岐点。
//!
//! 背骨:平台が「期望状態」を resources / volume_details に持ち、現実(host_path の
//! ディレクトリ)をそこへ収束させる。volume は顶层リソースで、各 volume は
//! `<volumes_dir>/<user_id>/<volume_id>/` の独立した假根サンドボックス。
//! 注入(service への mount)は M3 — ここではファイル置き場の実体だけを扱う。
//!
//! ファイル API のパスは全て `safe_path`(唯一のハード安全境界)を通す。

mod safe_path;

use crate::auth::AuthCtx;
use crate::databases::{audit, map_unique};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::validate;
use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::json;
use sqlx::PgPool;
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;
use tsubomi_shared::{
    CreateVolumeReq, FileEntryDto, ListDirResp, MoveReq, RenameVolumeReq, VolumeDto, VolumeUsageDto,
};
use uuid::Uuid;

const MAX_NAME_LEN: usize = 64;

/// `?path=` クエリ。未指定は假根("")。
#[derive(Debug, Deserialize)]
pub struct PathQuery {
    #[serde(default)]
    pub path: String,
    /// download エンドポイント専用:true でプレビュー(inline 配信)に切り替える。
    /// 他のエンドポイントは無視する。
    #[serde(default)]
    pub inline: bool,
}

/// `list` / `get_one` の行(id, display_name, anon_seq, created_at)。
type VolRow = (Uuid, String, i32, DateTime<Utc>);

fn vol_row_to_dto((id, display_name, anon_seq, created_at): VolRow) -> VolumeDto {
    VolumeDto {
        id,
        display_name,
        anon_seq,
        created_at,
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/volumes", get(list).post(create))
        .route("/volumes/{id}", get(get_one).patch(rename).delete(delete))
        // ファイル API(§7 トラバーサル防御を通す)。upload は生 body をストリームするため
        // Bytes ではなく Body extractor を使い(DefaultBodyLimit を回避)、上限はハンドラで掛ける。
        .route(
            "/volumes/{id}/files",
            get(list_files).put(upload).delete(delete_entry),
        )
        .route("/volumes/{id}/files/download", get(download))
        .route("/volumes/{id}/dirs", post(mkdir))
        .route("/volumes/{id}/move", post(move_entry))
        .route("/volumes/{id}/usage", get(usage))
}

// ===== 共通の取得 =====

/// 所有者チェック付きで host_path を引く。見つからない / 他ユーザ / 削除済みは 404。
async fn fetch_volume_path(db: &PgPool, user_id: Uuid, id: Uuid) -> AppResult<PathBuf> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT d.host_path
           FROM resources r
           JOIN volume_details d ON d.resource_id = r.id
          WHERE r.id = $1 AND r.user_id = $2 AND r.kind = 'volume' AND r.deleted_at IS NULL",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    row.map(|(p,)| PathBuf::from(p)).ok_or(AppError::NotFound)
}

// ===== リソース CRUD =====

/// `POST /api/volumes`:volume 作成。
pub async fn create(
    auth: AuthCtx,
    State(state): State<AppState>,
    Json(req): Json<CreateVolumeReq>,
) -> AppResult<(StatusCode, Json<VolumeDto>)> {
    let display_name = validate::name(&req.name, MAX_NAME_LEN)?;

    // 同名チェック(ゴミ箱内も含む。UNIQUE が最終ガード)。
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM resources WHERE user_id = $1 AND kind = 'volume' AND display_name = $2)",
    )
    .bind(auth.user_id)
    .bind(&display_name)
    .fetch_one(&state.db)
    .await?;
    if exists {
        return Err(AppError::Conflict(format!(
            "ボリューム名 '{display_name}' は既に使われています(ゴミ箱内を含む)。別の名前にしてください"
        )));
    }

    let (dto, host_path) = insert_rows(
        &state.db,
        auth.user_id,
        &display_name,
        &state.config.volumes_dir,
    )
    .await?;

    // 実体ディレクトリを作る。失敗したら行を巻き戻す(「行が在る ⇒ ディレクトリが在る」を保つ)。
    if let Err(e) = std::fs::create_dir_all(&host_path) {
        let _ = sqlx::query("DELETE FROM resources WHERE id = $1")
            .bind(dto.id)
            .execute(&state.db)
            .await;
        return Err(e.into());
    }

    audit(
        &state.db,
        Some(auth.user_id),
        "volume.create",
        dto.id,
        json!({ "display_name": display_name, "host_path": host_path.to_string_lossy() }),
    )
    .await;
    Ok((StatusCode::CREATED, Json(dto)))
}

async fn insert_rows(
    db: &PgPool,
    user_id: Uuid,
    display_name: &str,
    volumes_dir: &std::path::Path,
) -> AppResult<(VolumeDto, PathBuf)> {
    let mut tx = db.begin().await?;

    // ユーザ単位で anon_seq の採番を直列化(volume 用の分類子 44。db の 42 と別)。
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1::text), 44)")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;

    let anon_seq: i32 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(anon_seq),0)+1 FROM resources WHERE user_id=$1 AND kind='volume'",
    )
    .bind(user_id)
    .fetch_one(&mut *tx)
    .await?;

    let (id, created_at): (Uuid, DateTime<Utc>) = sqlx::query_as(
        "INSERT INTO resources (user_id, kind, display_name, anon_seq)
              VALUES ($1, 'volume', $2, $3)
         RETURNING id, created_at",
    )
    .bind(user_id)
    .bind(display_name)
    .bind(anon_seq)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| {
        map_unique(
            e,
            format!("ボリューム名 '{display_name}' は既に使われています"),
        )
    })?;

    // host_path は volumes_dir/<user_id>/<volume_id>。id 確定後に組む。
    let host_path = volumes_dir.join(user_id.to_string()).join(id.to_string());
    sqlx::query("INSERT INTO volume_details (resource_id, host_path) VALUES ($1, $2)")
        .bind(id)
        .bind(host_path.to_string_lossy().as_ref())
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok((
        VolumeDto {
            id,
            display_name: display_name.to_owned(),
            anon_seq,
            created_at,
        },
        host_path,
    ))
}

/// `GET /api/volumes`:自分の volume 一覧。
pub async fn list(auth: AuthCtx, State(state): State<AppState>) -> AppResult<Json<Vec<VolumeDto>>> {
    let rows: Vec<VolRow> = sqlx::query_as(
        "SELECT r.id, r.display_name, r.anon_seq, r.created_at
           FROM resources r
           JOIN volume_details d ON d.resource_id = r.id
          WHERE r.user_id = $1 AND r.kind = 'volume' AND r.deleted_at IS NULL
          ORDER BY r.anon_seq",
    )
    .bind(auth.user_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(rows.into_iter().map(vol_row_to_dto).collect()))
}

/// `GET /api/volumes/:id`:単体。
pub async fn get_one(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<Json<VolumeDto>> {
    let row: Option<VolRow> = sqlx::query_as(
        "SELECT r.id, r.display_name, r.anon_seq, r.created_at
           FROM resources r
           JOIN volume_details d ON d.resource_id = r.id
          WHERE r.id = $1 AND r.user_id = $2 AND r.kind = 'volume' AND r.deleted_at IS NULL",
    )
    .bind(id)
    .bind(auth.user_id)
    .fetch_optional(&state.db)
    .await?;

    Ok(Json(vol_row_to_dto(row.ok_or(AppError::NotFound)?)))
}

/// `GET /api/volumes/:id/usage`:卷の使用量(概要ページ用)。假根を再帰走査して集計。
/// 走査は重くなり得るので spawn_blocking でランタイムスレッドを塞がない。
pub async fn usage(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<Json<VolumeUsageDto>> {
    let root = fetch_volume_path(&state.db, auth.user_id, id).await?;
    let (size_bytes, file_count, dir_count, truncated) =
        tokio::task::spawn_blocking(move || dir_usage(&root))
            .await
            .map_err(|e| AppError::Other(anyhow::anyhow!("使用量の集計に失敗しました: {e}")))??;
    Ok(Json(VolumeUsageDto {
        size_bytes,
        file_count,
        dir_count,
        truncated,
    }))
}

/// 走査の時間予算。これを超えたら打ち切り、truncated=true で下限値を返す
/// (巨大な卷や遅いストレージで 1 リクエストが長引くのを防ぐ。精密な集計は M4)。
const USAGE_TIME_BUDGET: std::time::Duration = std::time::Duration::from_millis(1500);

/// 假根を再帰走査して (合計バイト, ファイル数, ディレクトリ数, 打ち切りか) を返す。
/// symlink は辿らない(read_dir の file_type はエントリ自身の型 — symlink は file/dir
/// どちらにも該当せず素通り)。スタックで回し、深いツリーでも再帰オーバーフローしない。
/// 時間予算を超えたら途中で打ち切る(値は下限、truncated=true)。
fn dir_usage(root: &std::path::Path) -> std::io::Result<(u64, u64, u64, bool)> {
    let start = std::time::Instant::now();
    let (mut size, mut files, mut dirs, mut seen) = (0u64, 0u64, 0u64, 0u64);
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let ft = entry.file_type()?;
            if ft.is_dir() {
                dirs += 1;
                stack.push(entry.path());
            } else if ft.is_file() {
                files += 1;
                size += entry.metadata().map(|m| m.len()).unwrap_or(0);
            }
            // 毎回 Instant を読むのは過剰なので一定間隔だけ時間予算を確認する。
            seen += 1;
            if seen.is_multiple_of(2048) && start.elapsed() > USAGE_TIME_BUDGET {
                return Ok((size, files, dirs, true));
            }
        }
    }
    Ok((size, files, dirs, false))
}

/// `PATCH /api/volumes/:id`:表示名のリネーム(host_path は不変)。
pub async fn rename(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<RenameVolumeReq>,
) -> AppResult<Json<VolumeDto>> {
    let display_name = validate::name(&req.name, MAX_NAME_LEN)?;

    let row: Option<VolRow> = sqlx::query_as(
        "UPDATE resources SET display_name = $1
          WHERE id = $2 AND user_id = $3 AND kind = 'volume' AND deleted_at IS NULL
      RETURNING id, display_name, anon_seq, created_at",
    )
    .bind(&display_name)
    .bind(id)
    .bind(auth.user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| {
        map_unique(
            e,
            format!("ボリューム名 '{display_name}' は既に使われています"),
        )
    })?;

    let row = row.ok_or(AppError::NotFound)?;
    audit(
        &state.db,
        Some(auth.user_id),
        "volume.rename",
        id,
        json!({ "display_name": display_name }),
    )
    .await;
    Ok(Json(vol_row_to_dto(row)))
}

/// `DELETE /api/volumes/:id`:ソフト削除。実体を trash へ mv(同一 FS なのでほぼゼロコスト、§8)。
pub async fn delete(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<StatusCode> {
    let host_path = fetch_volume_path(&state.db, auth.user_id, id).await?;
    let trash_path = state.config.trash_dir.join(id.to_string());

    // trash 親を用意し、実体を mv。実体が既に無ければ(不整合)mv はスキップして行だけ畳む。
    std::fs::create_dir_all(&state.config.trash_dir)?;
    if host_path.exists() {
        std::fs::rename(&host_path, &trash_path)?;
    }

    let meta = json!({
        "host_path": host_path.to_string_lossy(),
        "trash_path": trash_path.to_string_lossy(),
    });
    sqlx::query(
        "UPDATE resources
            SET deleted_at = now(),
                purge_after = now() + interval '3 days',
                trash_meta = $2
          WHERE id = $1",
    )
    .bind(id)
    .bind(meta)
    .execute(&state.db)
    .await?;

    audit(
        &state.db,
        Some(auth.user_id),
        "volume.delete",
        id,
        json!({ "host_path": host_path.to_string_lossy() }),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

// ===== ファイル API(全て safe_path を通す)=====

/// `GET /api/volumes/:id/files?path=`:ディレクトリ列挙。
pub async fn list_files(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<PathQuery>,
) -> AppResult<Json<ListDirResp>> {
    let root = fetch_volume_path(&state.db, auth.user_id, id).await?;
    let dir = safe_path::resolve_existing(&root, &q.path)?;

    let meta = std::fs::metadata(&dir)?;
    if !meta.is_dir() {
        return Err(AppError::BadRequest(
            "指定パスはディレクトリではありません".into(),
        ));
    }

    let mut entries: Vec<FileEntryDto> = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        let md = entry.metadata().ok();
        let is_dir = ft.is_dir();
        entries.push(FileEntryDto {
            name: entry.file_name().to_string_lossy().into_owned(),
            is_dir,
            size: if is_dir {
                0
            } else {
                md.as_ref().map(|m| m.len()).unwrap_or(0)
            },
            modified: md
                .as_ref()
                .and_then(|m| m.modified().ok())
                .map(DateTime::<Utc>::from),
        });
    }
    // ディレクトリ先・名前順。
    entries.sort_by(|a, b| (!a.is_dir, &a.name).cmp(&(!b.is_dir, &b.name)));

    // 応答の path は假根からの正規化済み相対(resolve_existing が組んだ dir から導出 —
    // normalize_rel を二度走らせない)。
    let rel = dir
        .strip_prefix(&root)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(Json(ListDirResp { path: rel, entries }))
}

/// `GET /api/volumes/:id/files/download?path=`:ファイルをバイト列でストリーム返却。
pub async fn download(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<PathQuery>,
) -> AppResult<impl IntoResponse> {
    let root = fetch_volume_path(&state.db, auth.user_id, id).await?;
    let file_path = safe_path::resolve_existing(&root, &q.path)?;
    let meta = std::fs::metadata(&file_path)?;
    if meta.is_dir() {
        return Err(AppError::BadRequest(
            "ディレクトリはダウンロードできません".into(),
        ));
    }
    // Content-Length を付けてブラウザがダウンロード進捗(%)を出せるようにする
    // (無いと chunked になり総量不明=不確定プログレス)。単一書き手 + atomic rename
    // なので読み取り中にサイズが変わる経路は無い(safe_path の TOCTOU 受容と同じ)。
    let len = meta.len();

    let filename = file_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "download".into());

    let file = tokio::fs::File::open(&file_path).await?;
    let stream = tokio_util::io::ReaderStream::new(file);
    let body = Body::from_stream(stream);

    // inline(プレビュー)は推測 MIME で、attachment(ダウンロード)は octet-stream で
    // 必ず保存させる。ユーザのアップロード物を inline 同源描画すると html/svg のスクリプトで
    // XSS になりうるので、nosniff + CSP sandbox(スクリプト無効 + 唯一オリジン)で無害化する。
    let (content_type, disposition_type) = if q.inline {
        (guess_mime(&filename).to_string(), "inline")
    } else {
        ("application/octet-stream".to_string(), "attachment")
    };
    // filename は ASCII フォールバック + RFC 5987 の filename*(日本語名対応)。
    let disposition = format!(
        "{disposition_type}; filename=\"{}\"; filename*=UTF-8''{}",
        filename.replace('"', ""),
        urlencode(&filename)
    );
    Ok((
        [
            (header::CONTENT_TYPE, content_type),
            (header::CONTENT_LENGTH, len.to_string()),
            (header::CONTENT_DISPOSITION, disposition),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
            (header::CONTENT_SECURITY_POLICY, "sandbox".to_string()),
        ],
        body,
    ))
}

/// 拡張子から代表的な MIME を推測(プレビューの inline 配信用)。未知は octet-stream。
/// html/svg も真の型で返すが、応答に付く CSP sandbox がスクリプト実行を無効化する。
fn guess_mime(filename: &str) -> &'static str {
    match filename
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "txt" | "log" | "md" | "csv" | "yml" | "yaml" | "toml" | "ini" => {
            "text/plain; charset=utf-8"
        }
        "json" => "application/json",
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        _ => "application/octet-stream",
    }
}

/// `PUT /api/volumes/:id/files?path=`:ファイル作成 / 上書き(生 body をストリーム書き込み)。
pub async fn upload(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<PathQuery>,
    body: Body,
) -> AppResult<StatusCode> {
    let root = fetch_volume_path(&state.db, auth.user_id, id).await?;
    let dest = safe_path::resolve_for_write(&root, &q.path)?;
    let cap = state.config.max_upload_bytes;

    // 同一ディレクトリのユニークな一時ファイルへ書き、完了後に atomic rename で
    // 置き換える。これで (a) 上限超過/中断時に既存ファイルを壊さない (b) dest が
    // symlink でも辿らない(rename は dest のエントリを置換するだけ)。
    let parent = dest.parent().unwrap_or(root.as_path());
    let tmp = parent.join(format!(".tbm-upload-{}.tmp", Uuid::new_v4()));

    let mut file = tokio::fs::File::create(&tmp).await?;
    let mut written: usize = 0;
    let mut stream = body.into_data_stream();
    loop {
        let Some(chunk) = stream.next().await else {
            break;
        };
        let res: AppResult<()> = async {
            let chunk = chunk
                .map_err(|e| AppError::BadRequest(format!("アップロード読み取りエラー: {e}")))?;
            written = written.saturating_add(chunk.len());
            if written > cap {
                return Err(AppError::BadRequest(format!(
                    "アップロード上限({cap} バイト)を超えました"
                )));
            }
            file.write_all(&chunk).await?;
            Ok(())
        }
        .await;
        if let Err(e) = res {
            drop(file);
            let _ = tokio::fs::remove_file(&tmp).await; // 既存 dest は無傷
            return Err(e);
        }
    }
    if let Err(e) = file.flush().await {
        drop(file);
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(e.into());
    }
    drop(file);

    if let Err(e) = tokio::fs::rename(&tmp, &dest).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(e.into());
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/volumes/:id/files?path=`:ファイル / ディレクトリ削除(ディレクトリは再帰)。
pub async fn delete_entry(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<PathQuery>,
) -> AppResult<StatusCode> {
    let root = fetch_volume_path(&state.db, auth.user_id, id).await?;
    let target = safe_path::resolve_existing(&root, &q.path)?;
    // 假根そのものは消させない(volume 削除は DELETE /volumes/:id)。
    if target == root {
        return Err(AppError::BadRequest(
            "ルートは削除できません(ボリュームごと削除してください)".into(),
        ));
    }
    if std::fs::metadata(&target)?.is_dir() {
        std::fs::remove_dir_all(&target)?;
    } else {
        std::fs::remove_file(&target)?;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/volumes/:id/dirs?path=`:mkdir -p。
pub async fn mkdir(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<PathQuery>,
) -> AppResult<StatusCode> {
    let root = fetch_volume_path(&state.db, auth.user_id, id).await?;
    safe_path::ensure_dir(&root, &q.path)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/volumes/:id/move`:同一 volume 内の rename / move。
pub async fn move_entry(
    auth: AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<MoveReq>,
) -> AppResult<StatusCode> {
    let root = fetch_volume_path(&state.db, auth.user_id, id).await?;
    let from = safe_path::resolve_existing(&root, &req.from)?;
    let to = safe_path::resolve_for_write(&root, &req.to)?;
    // 移動先が既にあれば拒否(rename は黙って上書きする — 事故・データ損失を防ぐ)。
    if std::fs::symlink_metadata(&to).is_ok() {
        return Err(AppError::Conflict("移動先が既に存在します".into()));
    }
    std::fs::rename(&from, &to)?;
    Ok(StatusCode::NO_CONTENT)
}

/// RFC 5987 用の最小 percent-encode(英数字と一部記号以外を %XX)。
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
