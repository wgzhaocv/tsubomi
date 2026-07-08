//! M6 egress(出站隔離):テナント容器の宛先を iptables で縛る。**宿主機 + 全私網を遮断、公網は
//! 全 TCP 放行**。同桥東西向(app↔infra)は同 subnet 宛 RETURN で例外放行。脅威モデル・規則の根拠は
//! `doc/paas-egress-design.md`(§1-3)。
//!
//! **prod Linux + root のみ**動く(server は root の host プロセス)。dev macOS / 非 root は no-op。
//! 期望状態(`config.tenant_pool` + 生存テナント subnet)を毎回 iptables へ収束させる(ipblock と同型・
//! 冪等)。起動時 + reconcile tick + コンテナ起動の直前(`docker::run`)に呼ぶ。
//!
//! 構成:**外殻チェイン + 内実 A/B 二重バッファ**。
//!   - 外殻 `TSUBOMI-EGRESS`(FORWARD = 容器 → 他網):DOCKER-USER から jump。中身は
//!     「アクティブな内実チェインへの jump 1 本」だけ。
//!   - 外殻 `TSUBOMI-INGRESS-HOST`(INPUT = 容器 → 宿主機の任意 IP):INPUT 先頭へ jump。同上。
//!   - 内実 `…-A` / `…-B`:実際の規則本体。毎回**非アクティブ側**へ全規則を組み立ててから、
//!     外殻の jump を 1 コマンド(`-R` = 原子的な単一規則置換)で新しい側へ切り替える。
//!
//! 旧実装は外殻チェインを直接 `-F`(flush)→ 逐条 refill しており、refill 途中で iptables が
//! 1 本でも失敗すると **DROP 規則が欠けたまま次 tick(30s)まで fail-open** だった(AI 審査 R9)。
//! A/B swap では組み立て失敗時に切替をしない = 旧規則がそのまま効き続ける(fail-closed)。
//!
//! 入口 jump は「無ければ挿す」で冪等。旧レイアウト(外殻に規則がフラットに並ぶ)からの移行は
//! `swap_refill` が吸収する:jump を先頭に挿してから残骸を**末尾から逆順**に払う(inner の
//! RETURN は外殻の残骸も評価するため、逆順で「RETURN 類が DROP 類より前」の並びを保ちながら
//! 消す = 移行中も丢包しない)。

use crate::services::network;
use crate::state::AppState;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::process::Command;
use tokio::sync::Mutex;

/// FORWARD(容器 → 他網)を縛る外殻チェイン(DOCKER-USER から jump される)。
const FWD_CHAIN: &str = "TSUBOMI-EGRESS";
/// INPUT(容器 → 宿主機)を縛る外殻チェイン。
const HOST_CHAIN: &str = "TSUBOMI-INGRESS-HOST";
/// FWD の内実チェイン(A/B 二重バッファ。iptables のチェイン名上限 28 字に収まる)。
const FWD_INNER: [&str; 2] = ["TSUBOMI-EGRESS-A", "TSUBOMI-EGRESS-B"];
/// INPUT の内実チェイン(同上)。
const HOST_INNER: [&str; 2] = ["TSUBOMI-INGRESS-HOST-A", "TSUBOMI-INGRESS-HOST-B"];

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

/// swap を直列化するロック。`reconcile` は reconcile tick と deploy(`docker::run`)から
/// 並行に呼ばれ得る。並行すると A/B の書き込み先が衝突し、切替直後の側を別スレッドが flush し得る。
/// **try_lock で飛ばさず必ず待つ** — deploy 経路は新 subnet を反映して収束させる義務があるため。
/// tokio の Mutex::new は const ではないので LazyLock で包む。
static EGRESS_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// egress 規則を現実へ収束させる。best-effort:失敗はログのみ(次 tick / 次起動で収束)。
/// 組み立て途中の失敗では切替しない = 直前の規則が効き続ける(fail-closed)。
pub async fn reconcile(state: &AppState) {
    if !is_active() {
        if !ANNOUNCED_NOOP.swap(true, Ordering::Relaxed) {
            tracing::info!("egress: Linux+root 以外のため no-op(網フィルタは prod でのみ有効)");
        }
        return;
    }
    let _guard = EGRESS_LOCK.lock().await;
    if let Err(e) = reconcile_inner(state).await {
        tracing::error!(error = ?e, "egress: iptables 収束に失敗 — 旧規則のまま次 tick / 次起動で再試行");
    }
}

/// prod Linux + root でのみ動く。dev(macOS)は cfg で false、非 root は euid で false。
fn is_active() -> bool {
    // SAFETY: geteuid() は副作用が無く常に成功する POSIX 呼び出し。
    cfg!(target_os = "linux") && unsafe { libc::geteuid() } == 0
}

async fn reconcile_inner(state: &AppState) -> anyhow::Result<()> {
    let pool = state.config.tenant_pool.to_string();
    let subnets: Vec<String> = network::tenant_subnets(state)
        .await?
        .iter()
        .map(|s| s.to_string())
        .collect();

    // 入口を冪等に確保(DOCKER-USER は Docker が再起動でも保全、INPUT は安定)。
    ensure_jump("DOCKER-USER", FWD_CHAIN).await?;
    ensure_jump("INPUT", HOST_CHAIN).await?;

    swap_refill(FWD_CHAIN, FWD_INNER, &fwd_rules(&pool, &subnets)).await?;
    swap_refill(HOST_CHAIN, HOST_INNER, &host_rules(&pool)).await?;

    tracing::debug!(tenant_subnets = subnets.len(), %pool, "egress: iptables 収束完了");
    Ok(())
}

/// FORWARD(容器 → 他網)の規則本体:established 放行 → 同桥(同 subnet)RETURN → 私網 DROP →
/// 末尾 RETURN(公網は素通りして DOCKER-FORWARD で放行)。
fn fwd_rules(pool: &str, subnets: &[String]) -> Vec<Vec<String>> {
    let mut rules: Vec<Vec<String>> = Vec::with_capacity(subnets.len() + PRIVATE_NETS.len() + 2);
    rules.push(strs(&[
        "-m", "conntrack", "--ctstate", "ESTABLISHED,RELATED", "-j", "RETURN",
    ]));
    for s in subnets {
        rules.push(strs(&["-s", s, "-d", s, "-j", "RETURN"]));
    }
    for net in PRIVATE_NETS {
        rules.push(strs(&["-s", pool, "-d", net, "-j", "DROP"]));
    }
    rules.push(strs(&["-j", "RETURN"]));
    rules
}

/// INPUT(容器 → 宿主機の任意 IP)の規則本体:established 放行 → pool 源は全 DROP
/// (sshd / 裸PG / redis / panel…)。
fn host_rules(pool: &str) -> Vec<Vec<String>> {
    vec![
        strs(&["-m", "conntrack", "--ctstate", "ESTABLISHED,RELATED", "-j", "RETURN"]),
        strs(&["-s", pool, "-j", "DROP"]),
    ]
}

fn strs(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

/// 非アクティブな内実チェインへ全規則を組み立て、外殻の jump を原子的に切り替える。
/// 手順:①内実 A/B を確保 ②非アクティブ側を flush → 逐条 append(**この間 現行規則は無傷**)
/// ③外殻の先頭規則を `-R`(単一規則の原子置換)で新しい側へ ④外殻 2 行目以降の残骸を払う
/// (旧レイアウトのフラット規則 / 二重 jump の移行掃除)⑤旧側 flush(次回の書き込み先を空に)。
/// ②までのどこで失敗しても切替前 = fail-closed。
async fn swap_refill(outer: &str, inner: [&str; 2], rules: &[Vec<String>]) -> anyhow::Result<()> {
    // -N は既存だと非零終了(冪等なので握り潰す)。
    let _ = run_iptables(&["-N", inner[0]]).await;
    let _ = run_iptables(&["-N", inner[1]]).await;

    let listing = list_rules(outer).await?;
    let target = swap_target(active_inner(&listing, inner), inner);

    iptables(&["-F", target]).await?;
    for rule in rules {
        let mut args: Vec<&str> = vec!["-A", target];
        args.extend(rule.iter().map(String::as_str));
        iptables(&args).await?;
    }

    // 切替:外殻の先頭が内実 jump なら -R(原子置換)、無ければ -I(初回 / 旧レイアウト移行)。
    // 切替後の残骸(旧レイアウトのフラット規則 / 想定外の二重 jump)の位置を snapshot から確定する:
    // -R は規則数不変(残骸 = 2..=N)、-I は 1 本増える(旧規則が 2..=N+1 へ押し下がる)。
    let last = if first_is_inner_jump(&listing, inner) {
        iptables(&["-R", outer, "1", "-j", target]).await?;
        listing.len()
    } else {
        iptables(&["-I", outer, "1", "-j", target]).await?;
        listing.len() + 1
    };

    // 残骸掃除は**末尾から逆順**に消す。inner 末尾の RETURN は外殻へ戻って残骸も評価するため、
    // 先頭(位置 2)から消すと「RETURN 類が先に消え DROP 類だけ残る」中間態ができ、同桥東西向 /
    // established が残骸 DROP に丢包される(審査指摘)。逆順なら残骸は常に「元の布局の前綴」
    // (RETURN 類が DROP 類より前)を保ち、inner を RETURN で抜けた包が残骸で落ちることはない。
    // 削除失敗は無視(best-effort — 残っても次 tick の同経路で再収束)。
    for pos in (2..=last).rev() {
        let _ = run_iptables(&["-D", outer, &pos.to_string()]).await;
    }

    // 旧側を空にしておく(失敗しても実害なし — 次回 swap 前の -F で再試行される)。
    let other = swap_target(Some(target), inner);
    let _ = run_iptables(&["-F", other]).await;
    Ok(())
}

/// 外殻チェインの規則一覧(`iptables -S <chain>` の `-A` 行のみ、順序保持)。
async fn list_rules(chain: &str) -> anyhow::Result<Vec<String>> {
    let out = run_iptables(&["-S", chain]).await?;
    if !out.status.success() {
        anyhow::bail!(
            "iptables -S {chain} が失敗: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.starts_with("-A "))
        .map(str::to_string)
        .collect())
}

/// 外殻の規則一覧から、現在 jump が指している内実チェインを探す(無ければ None = 初回 / 旧レイアウト)。
fn active_inner<'a>(listing: &[String], inner: [&'a str; 2]) -> Option<&'a str> {
    listing
        .iter()
        .find_map(|l| inner.iter().find(|name| l.ends_with(&format!("-j {name}"))).copied())
}

/// 書き込み先(非アクティブ側)を決める。アクティブが A なら B、それ以外(B / 不明)なら A。
fn swap_target<'a>(active: Option<&str>, inner: [&'a str; 2]) -> &'a str {
    if active == Some(inner[0]) {
        inner[1]
    } else {
        inner[0]
    }
}

/// 外殻の**先頭**規則が内実チェインへの jump か(-R で置換してよいか)。
fn first_is_inner_jump(listing: &[String], inner: [&str; 2]) -> bool {
    listing
        .first()
        .is_some_and(|l| inner.iter().any(|name| l.ends_with(&format!("-j {name}"))))
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
/// `-w 5` = xtables lock を最大 5s 待つ:dockerd がコンテナ start/stop で並行に iptables を
/// 触るため、lock 競合の exit 4 で組み立てが無駄に失敗して次 tick まで持ち越すのを防ぐ
/// (deploy 経路は新 subnet の同桥 RETURN を即時に入れる義務がある)。
async fn run_iptables(args: &[&str]) -> anyhow::Result<std::process::Output> {
    Command::new("iptables")
        .args(["-w", "5"])
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fwd_rules_shape_is_stable() {
        // established → 同桥 RETURN(subnet 数)→ 私網 DROP(5)→ 末尾 RETURN の順序。
        let subnets = vec!["172.20.1.0/24".to_string(), "172.20.2.0/24".to_string()];
        let rules = fwd_rules("172.20.0.0/16", &subnets);
        assert_eq!(rules.len(), 1 + 2 + 5 + 1);
        assert_eq!(rules[0][0], "-m"); // established が先頭
        assert_eq!(rules[1], strs(&["-s", "172.20.1.0/24", "-d", "172.20.1.0/24", "-j", "RETURN"]));
        assert!(rules[3..8].iter().all(|r| r.last().unwrap() == "DROP"));
        assert_eq!(*rules.last().unwrap(), strs(&["-j", "RETURN"]));
    }

    #[test]
    fn host_rules_drop_pool_after_established() {
        let rules = host_rules("172.20.0.0/16");
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[1], strs(&["-s", "172.20.0.0/16", "-j", "DROP"]));
    }

    #[test]
    fn swap_alternates_and_defaults_to_a() {
        // アクティブ A → 書き込み先 B、アクティブ B → A、不明(初回/旧レイアウト)→ A。
        assert_eq!(swap_target(Some("TSUBOMI-EGRESS-A"), FWD_INNER), "TSUBOMI-EGRESS-B");
        assert_eq!(swap_target(Some("TSUBOMI-EGRESS-B"), FWD_INNER), "TSUBOMI-EGRESS-A");
        assert_eq!(swap_target(None, FWD_INNER), "TSUBOMI-EGRESS-A");
    }

    #[test]
    fn detects_active_inner_and_first_jump() {
        // 定常状態:先頭が内実 jump。
        let steady = vec!["-A TSUBOMI-EGRESS -j TSUBOMI-EGRESS-B".to_string()];
        assert_eq!(active_inner(&steady, FWD_INNER), Some("TSUBOMI-EGRESS-B"));
        assert!(first_is_inner_jump(&steady, FWD_INNER));

        // 旧レイアウト(フラット規則):jump 無し → -I 移行経路。
        let legacy = vec![
            "-A TSUBOMI-EGRESS -m conntrack --ctstate ESTABLISHED,RELATED -j RETURN".to_string(),
            "-A TSUBOMI-EGRESS -s 172.20.0.0/16 -d 10.0.0.0/8 -j DROP".to_string(),
        ];
        assert_eq!(active_inner(&legacy, FWD_INNER), None);
        assert!(!first_is_inner_jump(&legacy, FWD_INNER));

        // 空チェイン(初回)。
        assert_eq!(active_inner(&[], FWD_INNER), None);
        assert!(!first_is_inner_jump(&[], FWD_INNER));
    }
}
