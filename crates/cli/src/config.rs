use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

/// ディスク上の状態。トークンも含む:chmod 600 が同一マシンの他ユーザから
/// 守り、フルディスク暗号化が盗難から守る。`~/.netrc` /
/// `~/.aws/credentials` と同じ脅威モデル(amber での keychain 実験は
/// この理由で取りやめた)。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Config {
    pub server_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    /// version_check が試行の度(成功でも失敗でも)に書くクールダウン刻印。
    /// 落ちているサーバ相手に毎回 1 秒のタイムアウトを払わないため。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_version_check: Option<DateTime<Utc>>,
}

fn project_dirs() -> Result<ProjectDirs> {
    ProjectDirs::from("jp", "flegrowth", "tsubomi")
        .context("failed to resolve config directory for jp.flegrowth.tsubomi")
}

pub fn config_path() -> Result<PathBuf> {
    Ok(project_dirs()?.config_dir().join("config.toml"))
}

pub fn load() -> Result<Option<Config>> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let s =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let cfg: Config =
        toml::from_str(&s).with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(Some(cfg))
}

pub fn save(c: &Config) -> Result<()> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let s = toml::to_string(c).context("failed to serialize config")?;

    // 最初から mode 600 で開く:Bearer トークンが create と chmod の間の
    // 一瞬でも world-readable にならないように(Unix)。
    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .with_context(|| format!("failed to open {} for write", path.display()))?;
        f.write_all(s.as_bytes())
            .with_context(|| format!("failed to write {}", path.display()))?;
    }
    #[cfg(not(unix))]
    fs::write(&path, s).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

pub fn delete() -> Result<()> {
    let path = config_path()?;
    if path.exists() {
        fs::remove_file(&path).with_context(|| format!("failed to delete {}", path.display()))?;
    }
    Ok(())
}
