//! M6 egress(出站隔離):テナント容器の宛先を iptables で縛る。**宿主機 + 全私網を遮断、公網は
//! 全 TCP 放行**。同桥東西向(app↔infra)は同 subnet 宛 RETURN で例外放行。脅威モデル・規則の根拠は
//! `doc/paas-egress-design.md`(§1-3)。
//!
//! **prod Linux + root のみ**動く(server は root の host プロセス)。dev macOS / 非 root は no-op。
//! 期望状態(`config.tenant_pool` + 生存テナント subnet)を毎回 iptables へ収束させる(ipblock と同型・
//! 冪等)。起動時 + reconcile tick + コンテナ起動の直前(`docker::run`)に呼ぶ。
//!
//! 構成:2 つの自前チェインに中身を閉じ込め、入口だけ親チェインに固定する。
//!   - `TSUBOMI-EGRESS`(FORWARD = 容器 → 他網):DOCKER-USER から jump。
//!   - `TSUBOMI-INGRESS-HOST`(INPUT = 容器 → 宿主機の任意 IP):INPUT 先頭へ jump。
//!
//! 入口 jump は「無ければ挿す」で冪等。チェイン中身は毎回 flush → refill。

use crate::services::network;
use crate::state::AppState;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::process::Command;
use tokio::sync::Mutex;

/// FORWARD(容器 → 他網)を縛る自前チェイン。
const FWD_CHAIN: &str = "TSUBOMI-EGRESS";
/// INPUT(容器 → 宿主機)を縛る自前チェイン。
const HOST_CHAIN: &str = "TSUBOMI-INGRESS-HOST";

/// 遮断する私網(宿主 LAN / docker / tailscale CGNAT / link-local・クラウドメタデータ)。
/// 公網(これ以外)はチェイン末尾の RETURN を抜けて放行 = 全 TCP 出站可。
const PRIVATE_NETS: [&str; 5] = [
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "100.64.0.0/10",  // tailscale CGNAT
    "169.254.0.0/16", // link-local / クラウドメタデータ
];

/// 非 Linux / 非 root で no-op を一度だけ告知するフラグ(毎 tick のログ氾濫を防ぐ)。
static ANNOUNCED_NOOP: AtomicBool = AtomicBool::new(false);

/// flush → refill を直列化するロック。`reconcile` は reconcile tick と deploy(`docker::run`)から
/// 並行に呼ばれ得る。並行すると(a)チェインが空の一瞬に遮断が外れる、(b)入口 jump が二重挿入される。
/// **try_lock で飛ばさず必ず待つ** — deploy 経路は新 subnet を反映して収束させる義務があるため。
/// tokio の Mutex::new は const ではないので LazyLock で包む。
static EGRESS_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// egress 規則を現実へ収束させる。best-effort:失敗はログのみ(次 tick / 次起動で収束)。
pub async fn reconcile(state: &AppState) {
    if !is_active() {
        if !ANNOUNCED_NOOP.swap(true, Ordering::Relaxed) {
            tracing::info!("egress: Linux+root 以外のため no-op(網フィルタは prod でのみ有効)");
        }
        return;
    }
    let _guard = EGRESS_LOCK.lock().await;
    if let Err(e) = reconcile_inner(state).await {
        tracing::error!(error = ?e, "egress: iptables 収束に失敗 — 次 tick / 次起動で再試行");
    }
}

/// prod Linux + root でのみ動く。dev(macOS)は cfg で false、非 root は euid で false。
fn is_active() -> bool {
    // SAFETY: geteuid() は副作用が無く常に成功する POSIX 呼び出し。
    cfg!(target_os = "linux") && unsafe { libc::geteuid() } == 0
}

async fn reconcile_inner(state: &AppState) -> anyhow::Result<()> {
    let pool = state.config.tenant_pool.to_string();
    let subnets = network::tenant_subnets(state).await?;

    // 入口を冪等に確保(DOCKER-USER は Docker が再起動でも保全、INPUT は安定)。
    ensure_jump("DOCKER-USER", FWD_CHAIN).await?;
    ensure_jump("INPUT", HOST_CHAIN).await?;

    // FORWARD(容器 → 他網):established 放行 → 同桥(同 subnet)RETURN → 私網 DROP → 末尾 RETURN
    // (公網は素通りして DOCKER-FORWARD で放行)。flush してから順に積む。
    iptables(&["-F", FWD_CHAIN]).await?;
    iptables(&[
        "-A", FWD_CHAIN, "-m", "conntrack", "--ctstate", "ESTABLISHED,RELATED", "-j", "RETURN",
    ])
    .await?;
    for s in &subnets {
        let s = s.to_string();
        iptables(&["-A", FWD_CHAIN, "-s", s.as_str(), "-d", s.as_str(), "-j", "RETURN"]).await?;
    }
    for net in PRIVATE_NETS {
        iptables(&["-A", FWD_CHAIN, "-s", pool.as_str(), "-d", net, "-j", "DROP"]).await?;
    }
    iptables(&["-A", FWD_CHAIN, "-j", "RETURN"]).await?;

    // INPUT(容器 → 宿主機の任意 IP):established 放行 → pool 源は全 DROP(sshd / 裸PG / redis / panel…)。
    iptables(&["-F", HOST_CHAIN]).await?;
    iptables(&[
        "-A", HOST_CHAIN, "-m", "conntrack", "--ctstate", "ESTABLISHED,RELATED", "-j", "RETURN",
    ])
    .await?;
    iptables(&["-A", HOST_CHAIN, "-s", pool.as_str(), "-j", "DROP"]).await?;

    tracing::debug!(tenant_subnets = subnets.len(), %pool, "egress: iptables 収束完了");
    Ok(())
}

/// 自前チェインを作り(無ければ)、親チェインの先頭に jump を一度だけ挿す(冪等)。
async fn ensure_jump(parent: &str, chain: &str) -> anyhow::Result<()> {
    // -N は既存だと非零終了(冪等なので握り潰す)。
    let _ = run_iptables(&["-N", chain]).await;
    if !rule_exists(&["-C", parent, "-j", chain]).await {
        iptables(&["-I", parent, "1", "-j", chain]).await?;
    }
    Ok(())
}

/// iptables を実行し、非零終了をエラーにする。
async fn iptables(args: &[&str]) -> anyhow::Result<()> {
    let out = run_iptables(args).await?;
    if !out.status.success() {
        anyhow::bail!(
            "iptables {} が失敗: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// iptables を実行し Output を返す(終了コードは呼び出し側が判定)。
async fn run_iptables(args: &[&str]) -> anyhow::Result<std::process::Output> {
    Command::new("iptables")
        .args(args)
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("iptables 実行に失敗({}): {e}", args.join(" ")))
}

/// ルールが既に在るか(`-C`)。在れば exit 0。
async fn rule_exists(args: &[&str]) -> bool {
    run_iptables(args)
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}
