//! CLI 起動毎のバックグラウンドバージョンチェック。タイムアウト 1 秒、
//! クールダウン 1 時間(成功・失敗を問わず刻印)なので、落ちている
//! サーバが毎回タイムアウトを焼くことはない。失敗は沈黙。
//!
//! プロジェクト決定により**通知のみ** — 対話式の自動更新プロンプトは無い
//! (amber の auto_update.rs は意図的に移植していない)。更新は常に
//! ユーザが明示的に `tbm update` を打つ。

use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use serde::Deserialize;

use crate::config::{self, Config};

const VERSION_PATH: &str = "/api/cli/version";
const TIMEOUT: Duration = Duration::from_secs(1);
const COOLDOWN_HOURS: i64 = 1;

#[derive(Deserialize)]
struct VersionInfo {
    version: String,
}

/// 新しいバージョンがあり、かつクールダウンが明けていれば `Some(latest)`。
/// それ以外は `None`。main が読み込んだ config をそのまま受け取り
/// (再パースしない)、HTTP フェッチの前に刻印を保存する。
pub async fn maybe_check(server_url: &str, config: Option<Config>) -> Option<String> {
    let mut cfg = config?;

    if let Some(last) = cfg.last_version_check
        && Utc::now() - last < ChronoDuration::hours(COOLDOWN_HOURS)
    {
        return None;
    }

    cfg.last_version_check = Some(Utc::now());
    // 刻印を永続化できない(読み取り専用 / ディスク満杯)ならチェック自体を
    // やめる — でないとクールダウンが事実上無効になり、毎コマンドが
    // ネットワークコストを払う。
    config::save(&cfg).ok()?;

    let client = reqwest::Client::builder().timeout(TIMEOUT).build().ok()?;
    let info: VersionInfo = client
        .get(format!("{server_url}{VERSION_PATH}"))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()?;

    (info.version != env!("CARGO_PKG_VERSION")).then_some(info.version)
}
