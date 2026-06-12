use std::net::SocketAddr;

#[derive(Clone, Debug)]
pub struct Config {
    pub bind_addr: SocketAddr,
    /// Directory holding the built SPA (index.html + assets). Served as the
    /// fallback for everything that isn't an `/api` route.
    pub web_dir: String,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let bind_addr = std::env::var("TSUBOMI_BIND_ADDR")
            .unwrap_or_else(|_| "0.0.0.0:8080".to_string())
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid TSUBOMI_BIND_ADDR: {e}"))?;
        let web_dir = std::env::var("TSUBOMI_WEB_DIR").unwrap_or_else(|_| "web/dist".to_string());
        Ok(Self { bind_addr, web_dir })
    }
}
