//! 認証まわりの毎時ハウスキーピング:期限切れの sessions / oauth_states /
//! authcodes を削除する。最初の掃除は起動直後に走る(interval は 0 tick)。
//! これが reconcile ループ(tech-design §3)の種 — M3 でコンテナ収束が
//! ここに合流する。

use crate::state::AppState;
use std::time::Duration;

const INTERVAL: Duration = Duration::from_secs(3600);

pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(INTERVAL);
        loop {
            tick.tick().await;
            for (what, sql) in [
                ("sessions", "DELETE FROM sessions WHERE expires_at <= now()"),
                (
                    "oauth_states",
                    "DELETE FROM oauth_states WHERE expires_at <= now()",
                ),
                (
                    "authcodes",
                    "DELETE FROM authcodes WHERE expires_at <= now()",
                ),
            ] {
                match sqlx::query(sql).execute(&state.db).await {
                    Ok(r) if r.rows_affected() > 0 => {
                        tracing::debug!(what, rows = r.rows_affected(), "gc swept");
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!(what, error = ?e, "gc sweep failed"),
                }
            }
        }
    });
}
