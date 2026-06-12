use tower_http::services::{ServeDir, ServeFile};

pub fn fallback(web_dir: &str) -> ServeDir<ServeFile> {
    let index = format!("{web_dir}/index.html");
    ServeDir::new(web_dir).fallback(ServeFile::new(index))
}
