//! 認証系エンドポイントの基礎レート制限(AI 審査 R2)。
//!
//! 対象は **総当たり / 濫用の面になる少数の入口だけ**:ログイン開始・OAuth callback・
//! token 交換(未認証で叩ける)と、viewer 共有パスワード検証(bcrypt = 1 発数百 ms の
//! CPU も守る)。一般 API 全体には掛けない — CLI は AI 駆動で正当なバーストが日常であり、
//! そこを絞ると実害(誤 429 → AI の無駄リトライ)の方が大きい。
//!
//! 実装は**固定窓カウンタ**(単機・インメモリ。ipblock / deploy_lock と同じ「単機だから
//! プロセス内で足りる」型)。鍵は client IP:`require_auth` と同じ **CF-Connecting-IP**
//! (server は loopback listen 前提で偽装不能 — auth/middleware.rs の信頼前提を共有)。
//! ヘッダ無し(dev / LAN 直アクセス)は単一バケツに収束するが、上限は正当利用に十分緩い。
//! 固定窓は境界で最大 2 倍通る粗さがあるが、目的(オンライン総当たり・無限ループの頭打ち)には足りる。

use axum::extract::Request;
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// ログイン開始 / OAuth callback / token 交換(未認証入口)の上限:30 回 / 分 / IP。
/// 人間のログインにもリトライする AI にも十分緩く、オンライン総当たりには致命的に狭い。
static LOGIN: LazyLock<RateLimiter> = LazyLock::new(|| RateLimiter::new(30, Duration::from_secs(60)));

/// viewer 共有パスワード検証の上限:10 回 / 分 / IP(bcrypt 1 発数百 ms なので CPU 保護も兼ねる)。
static SENSITIVE: LazyLock<RateLimiter> =
    LazyLock::new(|| RateLimiter::new(10, Duration::from_secs(60)));

/// 未認証の認証入口(login / callback / token)用 middleware。
pub async fn limit_login(req: Request, next: Next) -> Response {
    limit_with(&LOGIN, req, next).await
}

/// パスワード類の検証入口(viewer login 等)用 middleware(より狭い)。
pub async fn limit_sensitive(req: Request, next: Next) -> Response {
    limit_with(&SENSITIVE, req, next).await
}

async fn limit_with(limiter: &RateLimiter, req: Request, next: Next) -> Response {
    let key = client_key(req.headers());
    if !limiter.check(&key) {
        // AI フレンドリ:何が起きたか + 次の一手(待つ)を本文に。Retry-After も添える。
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [("retry-after", "60")],
            "リクエストが多すぎます(レート制限)。60 秒ほど待ってから再試行してください",
        )
            .into_response();
    }
    next.run(req).await
}

/// レート制限の鍵 = client IP。第一候補は `require_auth` と同じ CF-Connecting-IP(信頼前提も
/// 同じ)。**直 VPS(traefik 終端)部署**は CF ヘッダが無いので X-Forwarded-For の**末尾**
/// (= 直近の信頼プロキシ traefik が付けた実 client)に退避する(codex 審査 — これが無いと
/// 非 CF 部署で全ユーザが単一バケツを共有し、一人のリトライ過多が全員を 429 にする)。
/// どちらも無い(dev / LAN 直)は "direct" の単一バケツ(上限は正当利用に十分緩い)。
fn client_key(headers: &HeaderMap) -> String {
    if let Some(ip) = headers.get("cf-connecting-ip").and_then(|v| v.to_str().ok()) {
        return ip.to_string();
    }
    if let Some(xff) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok())
        && let Some(last) = xff.rsplit(',').next().map(str::trim).filter(|s| !s.is_empty())
    {
        // 末尾のみ信頼する:先頭側は client が任意に注入できる(信頼できるのは直近 hop の追記だけ)。
        return last.to_string();
    }
    "direct".to_string()
}

/// バケツ(= 追跡する client IP)数の上限。満杯時の新規鍵は掃除後も空きが無ければ拒否
/// (fail-closed)— 洪水でメモリ・CPU を無限に食わせない。
const MAX_BUCKETS: usize = 10_000;

/// 固定窓カウンタ(鍵ごとに「窓の開始時刻 + 回数」)。lock は認証入口のみ通る低頻度なので争わない。
struct RateLimiter {
    max: u32,
    window: Duration,
    buckets: Mutex<HashMap<String, (Instant, u32)>>,
}

impl RateLimiter {
    fn new(max: u32, window: Duration) -> Self {
        Self {
            max,
            window,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    fn check(&self, key: &str) -> bool {
        self.check_at(key, Instant::now())
    }

    /// 現在時刻を注入できる本体(テスト用)。窓が過ぎていればリセットして数え直す。
    fn check_at(&self, key: &str, now: Instant) -> bool {
        let mut buckets = self.buckets.lock().expect("ratelimit lock poisoned");
        // 満杯なら:既知の鍵はそのまま数え、**新規の鍵は掃除して空きが出た時だけ**受け入れる
        // (掃除しても満杯 = 洪水中 → 新規は fail-closed で拒否)。IPv6 /64 轮换のような
        // 「全部新鮮な鍵」の洪水でも、メモリは 10k 桶で頭打ち・毎リクエスト O(n) retain にも
        // ならない(掃除が走るのは新規鍵かつ満杯の時だけ — 審査指摘)。
        if buckets.len() >= MAX_BUCKETS && !buckets.contains_key(key) {
            buckets.retain(|_, (start, _)| now.duration_since(*start) < self.window);
            if buckets.len() >= MAX_BUCKETS {
                return false;
            }
        }
        let entry = buckets.entry(key.to_string()).or_insert((now, 0));
        if now.duration_since(entry.0) >= self.window {
            *entry = (now, 0);
        }
        entry.1 += 1;
        entry.1 <= self.max
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_up_to_max_then_blocks_then_resets() {
        let rl = RateLimiter::new(3, Duration::from_secs(60));
        let t0 = Instant::now();
        assert!(rl.check_at("ip1", t0));
        assert!(rl.check_at("ip1", t0));
        assert!(rl.check_at("ip1", t0));
        assert!(!rl.check_at("ip1", t0)); // 4 回目は 429
        // 別 IP は独立。
        assert!(rl.check_at("ip2", t0));
        // 窓が過ぎればリセット。
        assert!(rl.check_at("ip1", t0 + Duration::from_secs(61)));
    }

    #[test]
    fn flood_of_fresh_keys_is_bounded_and_fail_closed() {
        let rl = RateLimiter::new(1, Duration::from_secs(1));
        let t0 = Instant::now();
        for i in 0..MAX_BUCKETS + 100 {
            rl.check_at(&format!("ip{i}"), t0);
        }
        // 窓内の洪水:バケツは上限で頭打ち、あふれた新規鍵は拒否(fail-closed)。
        assert_eq!(rl.buckets.lock().unwrap().len(), MAX_BUCKETS);
        assert!(!rl.check_at("attacker-fresh", t0));
        // 既知の鍵は満杯でも数えられる(正当ユーザの巻き添え最小化)。
        // max=1 なので 2 回目は超過 = false だが「拒否ではなく計数」の経路を通る。
        assert!(!rl.check_at("ip0", t0));
        // 窓が過ぎれば掃除で空きが出て新規鍵が通る。
        assert!(rl.check_at("fresh", t0 + Duration::from_secs(2)));
        assert!(rl.buckets.lock().unwrap().len() < MAX_BUCKETS);
    }
}
