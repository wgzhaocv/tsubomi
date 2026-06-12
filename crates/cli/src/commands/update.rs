//! 手動セルフアップデート:manifest を取得し、sha256 を検証して
//! バイナリを入れ替える。自動では決して走らない — バージョンチェックは
//! ここを指す一言を出すだけ。

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::commands::resolve_server_from;
use crate::config;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const TARGET: &str = "aarch64-apple-darwin";

// 香橙派(およびその他の arm64 Linux 機)。
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
const TARGET: &str = "aarch64-unknown-linux-gnu";

// 将来の x86_64 ホスト。
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const TARGET: &str = "x86_64-unknown-linux-gnu";

// Windows(PowerShell / cmd どちらの利用者も同じバイナリ)。
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const TARGET: &str = "x86_64-pc-windows-gnu";

#[cfg(windows)]
const BIN_NAME: &str = "tbm.exe";
#[cfg(not(windows))]
const BIN_NAME: &str = "tbm";

#[cfg(not(any(
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "linux", target_arch = "aarch64"),
    all(target_os = "linux", target_arch = "x86_64"),
    all(target_os = "windows", target_arch = "x86_64")
)))]
compile_error!(
    "tbm update supports aarch64-apple-darwin, aarch64/x86_64-unknown-linux-gnu and x86_64-pc-windows-gnu"
);

#[derive(Deserialize)]
struct TargetVersionInfo {
    version: String,
    url: String,
    sha256: String,
}

pub async fn run(server_override: Option<String>) -> Result<()> {
    let cfg = config::load()?;
    let server_url = resolve_server_from(server_override.as_deref(), cfg.as_ref());
    let info: TargetVersionInfo = reqwest::Client::new()
        .get(format!("{server_url}/api/cli/version/{TARGET}"))
        .send()
        .await
        .context("failed to reach update server")?
        .error_for_status()
        .context("update server returned an error (no release published yet?)")?
        .json()
        .await
        .context("failed to parse manifest response")?;

    let current = env!("CARGO_PKG_VERSION");
    if info.version == current {
        println!("tbm is up to date ({current})");
        return Ok(());
    }
    println!("updating: {current} → {}", info.version);

    // manifest の url は相対パス(/api/cli/dl/…)— デプロイ先のドメインに
    // 依存しないため。絶対 URL ならそのまま使う。
    let url = if info.url.starts_with('/') {
        format!("{server_url}{}", info.url)
    } else {
        info.url.clone()
    };

    let tmp = tempfile::tempdir().context("failed to create temp dir")?;
    let archive = tmp.path().join(archive_name(&url));
    download_and_verify(&url, &archive, &info.sha256).await?;

    let extract_dir = tmp.path().join("extracted");
    fs::create_dir(&extract_dir).context("failed to create extract dir")?;
    let new_binary = extract(&archive, &extract_dir)?;

    // self_replace は実行中バイナリを原子的に入れ替える。新しいファイルは
    // 元のパスと(Unix では)パーミッションを引き継ぐ。
    self_replace::self_replace(&new_binary).context("failed to swap binary")?;
    println!("tbm updated to {}", info.version);
    Ok(())
}

fn archive_name(url: &str) -> &str {
    url.rsplit('/').next().unwrap_or("archive")
}

/// ダウンロードをファイルと sha256 ハッシャーの両方に一回で流し込む。
/// 検証のためにアーカイブをディスクから読み直すことはしない。
async fn download_and_verify(url: &str, dest: &Path, expected_hex: &str) -> Result<()> {
    let mut resp = reqwest::get(url)
        .await
        .with_context(|| format!("failed to download {url}"))?
        .error_for_status()
        .with_context(|| format!("download {url} failed"))?;

    let mut file =
        fs::File::create(dest).with_context(|| format!("failed to create {}", dest.display()))?;
    let mut hasher = Sha256::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .context("failed to read download chunk")?
    {
        hasher.update(&chunk);
        file.write_all(&chunk)
            .with_context(|| format!("failed to write {}", dest.display()))?;
    }
    let actual = hex::encode(hasher.finalize());
    if !actual.eq_ignore_ascii_case(expected_hex) {
        bail!("sha256 mismatch (expected {expected_hex}, got {actual})");
    }
    Ok(())
}

// unix は tar.gz、windows は zip(Windows 10 1803+ 同梱の bsdtar が zip も
// 扱える)。`-xf` は両者で圧縮形式を自動判別する。
fn extract(archive: &Path, into: &Path) -> Result<PathBuf> {
    let status = Command::new("tar")
        .args(["-xf"])
        .arg(archive)
        .arg("-C")
        .arg(into)
        .status()
        .context("failed to spawn tar")?;
    if !status.success() {
        bail!("tar extract failed (status {status})");
    }
    Ok(into.join(BIN_NAME))
}
