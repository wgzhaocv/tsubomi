//! 秘密(接続文字列 / deploy_key / registry パスワード等)を返す応答に
//! `Cache-Control: no-store` を付けるヘルパ(security review S1)。中間プロキシ /
//! ブラウザキャッシュ / 戻るボタン / ディスクキャッシュに資格情報が残らないようにする。
//! 既に no-store を出している `cli_release`(配布物)と同じ作法を秘密 JSON にも揃える。

use axum::Json;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// 200 + `Cache-Control: no-store, private` + JSON。秘密を返す GET / POST 用。
pub fn no_store<T: Serialize>(body: T) -> Response {
    (
        [(header::CACHE_CONTROL, "no-store, private")],
        Json(body),
    )
        .into_response()
}

/// 201 Created + `Cache-Control: no-store, private` + JSON(秘密を含む create 応答用)。
pub fn no_store_created<T: Serialize>(body: T) -> Response {
    (
        StatusCode::CREATED,
        [(header::CACHE_CONTROL, "no-store, private")],
        Json(body),
    )
        .into_response()
}
