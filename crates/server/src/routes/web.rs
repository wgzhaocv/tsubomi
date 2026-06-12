use tower_http::services::{ServeDir, ServeFile};

/// `/api` 以外を SPA に流すフォールバック。index.html を fallback にする
/// ことでクライアントサイドルーティングが成立する。
pub fn fallback(web_dir: &str) -> ServeDir<ServeFile> {
    let index = format!("{web_dir}/index.html");
    ServeDir::new(web_dir).fallback(ServeFile::new(index))
}
