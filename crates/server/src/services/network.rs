//! M6 網隔離:service ごとに専用 bridge 私網 `<prefix><id>` を与え、テナント app を
//! 互いに隔離する(東西向=横移動の遮断。背骨「隔離は仕組みで守る」)。
//!
//! infra(traefik / pgbouncer / valkey)はこの私網へ on-demand で attach され、
//! ルーティング(traefik が **コンテナ名**で後端を引く route.rs)と注入(`tsubomi-pgbouncer` /
//! `tsubomi-valkey` の DNS 解決 — inject.rs)を per-service 網内でも成立させる。**注入文字列・
//! route の yaml は無改修**:同名コンテナ DNS は私網に attach すれば引けるため。pgbouncer/valkey
//! は私網からも到達可だが、隔離は資格(pg role / valkey ACL)が担保 = データ安全は本変更で不変。
//!
//! ライフサイクルは **service 紐づき**(deploy ではない):start-first swap の新旧コンテナは
//! 同じ私網に同居する。create は冪等(`run()` がコンテナ起動の直前に ensure)、撤去は
//! 削除 / 購読 + reconcile の孤児 GC。infra 単独再起動や手動削除からは reconcile が自己回復する。

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use anyhow::anyhow;
use bollard::models::{
    Ipam, IpamConfig, NetworkConnectRequest, NetworkCreateRequest, NetworkDisconnectRequest,
};
use bollard::query_parameters::ListNetworksOptionsBuilder;
use ipnet::Ipv4Net;
use std::collections::{HashMap, HashSet};
use std::net::Ipv4Addr;
use std::sync::LazyLock;
use tokio::sync::Mutex;
use uuid::Uuid;

use super::docker::{LABEL_MANAGED, LABEL_SERVICE_ID};

/// service の私網名 `<prefix><service_id>`(prefix は config、既定 `tsubomi-svc-`)。
pub(crate) fn svc_network_name(state: &AppState, service_id: Uuid) -> String {
    format!("{}{}", state.config.svc_network_prefix, service_id)
}

/// per-service 私網へ attach / detach する infra コンテナ名(単一の出所)。
/// traefik=route の後端解決 / pgbouncer=DB 注入の DNS / valkey=cache 注入の DNS。
fn infra_containers(state: &AppState) -> [&str; 3] {
    let cfg = &state.config;
    [
        &cfg.traefik_container,
        &cfg.pgbouncer_container,
        &cfg.valkey_container,
    ]
}

/// bollard の Error が指定 HTTP ステータスか(冪等化のための分岐に使う)。
fn is_status(e: &bollard::errors::Error, code: u16) -> bool {
    matches!(
        e,
        bollard::errors::Error::DockerResponseServerError { status_code, .. } if *status_code == code
    )
}

/// テナント私網の subnet サイズ。`tenant_pool`(/24 以上を起動時検証済み)から この大きさで切り出す。
const TENANT_SUBNET_PREFIX_LEN: u8 = 24;

/// 網の「採番 → 作成」を直列化するプロセス内ロック。これが無いと、別 service の同時 deploy が同じ
/// docker 網スナップショットを見て同一の空き /24 を選び、2 つ目の create が subnet 重複で虚假失敗
/// する(最悪 同一 CIDR を共有して E2 の「全租户網は pool 内・互いに別 subnet」不変条件を壊す)。
/// 作成は新規 service 時のみで稀なので、直列化のコストは無視できる(reconcile は元々逐次)。
/// tokio の Mutex::new は const ではないので LazyLock で包む。
static NET_ALLOC_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// テナント私網に与える (subnet, gateway) を `config.tenant_pool` から採番する。pool 内で、現存する
/// **全 docker 網**のどれとも重ならない最初の `/24` を返す(gateway はその `/24` の `.1`)。空きが
/// 無ければ **Err**(黙って docker 自動割当に倒さない — pool 外の subnet は egress が識別できず E2 の
/// 「全租户網は pool 内」不変条件を壊すため。プール拡張を促す)。
///
/// 既存網の subnet を読み直して再利用はしない:呼び出し側は新規作成時にだけ本関数を呼ぶ(reconcile の
/// 既存網パスでは呼ばない)。
async fn allocate_subnet(state: &AppState) -> AppResult<(String, String)> {
    let pool = state.config.tenant_pool; // 起動時に parse + /24 以上を検証済み(Ipv4Net は Copy)
    // 現存する全 docker 網の subnet を集める(tsubomi 以外の栈とも overlap させない)。
    let opts = ListNetworksOptionsBuilder::default().build();
    let networks = state
        .docker
        .list_networks(Some(opts))
        .await
        .map_err(|e| AppError::Other(anyhow!("網一覧の取得に失敗: {e}")))?;
    let used: Vec<Ipv4Net> = networks
        .iter()
        .filter_map(|n| n.ipam.as_ref())
        .filter_map(|i| i.config.as_ref())
        .flatten()
        .filter_map(|c| c.subnet.as_ref())
        .filter_map(|s| s.parse::<Ipv4Net>().ok())
        .collect();

    // pool は起動時検証済みなので subnets() は成功する(防御的に ? で伝播)。
    let candidates = pool
        .subnets(TENANT_SUBNET_PREFIX_LEN)
        .map_err(|e| AppError::Other(anyhow!("tenant_pool {pool} から /24 を切り出せません: {e}")))?;
    for cand in candidates {
        if used.iter().all(|u| !nets_overlap(*u, cand)) {
            let gateway = Ipv4Addr::from(u32::from(cand.network()) + 1);
            return Ok((cand.to_string(), gateway.to_string()));
        }
    }
    Err(AppError::Other(anyhow!(
        "テナントプール {pool} に空きの /24 がありません。TSUBOMI_TENANT_POOL を広げてください"
    )))
}

/// 2 つの v4 ネットが重なるか(u32 レンジの交差判定)。
fn nets_overlap(a: Ipv4Net, b: Ipv4Net) -> bool {
    let (a_lo, a_hi) = (u32::from(a.network()), u32::from(a.broadcast()));
    let (b_lo, b_hi) = (u32::from(b.network()), u32::from(b.broadcast()));
    a_lo <= b_hi && b_lo <= a_hi
}

/// service の私網を冪等に用意する:無ければ pool から /24 を採番して作成 → infra(traefik/pgbouncer/
/// valkey)を attach。**順序が肝心** — app コンテナ起動の直前に呼び、DNS 解決 + traefik 経路を成立させて
/// から start する。既存網は inspect で検出して作成を飛ばし(subnet 据え置き = 冪等。旧 pool 外網の移行は
/// 手動)、競合作成の 409・既接続 infra の 403 は冪等に握り潰す(2 回目以降の deploy は全部この経路)。
pub(crate) async fn ensure_service_network(state: &AppState, service_id: Uuid) -> AppResult<()> {
    let name = svc_network_name(state, service_id);

    // 採番〜作成は直列化する(NET_ALLOC_LOCK)。別 service の同時 deploy が同じ空き /24 を掴む TOCTOU を
    // 防ぐ。ロック下で存在を再確認 → 無ければ pool から /24 を採番して作る。reconcile が毎 tick 全 service に
    // 対し呼ぶので、存在時は重い list_networks(採番)を避け、軽い inspect で済ませる(ロックは無競合 = 安価)。
    {
        let _guard = NET_ALLOC_LOCK.lock().await;
        if !network_exists(state, &name).await {
            // 管理ラベル(GC が `tsubomi.managed=true` で列挙し service_id を読む)。
            let mut labels: HashMap<String, String> = HashMap::new();
            labels.insert(LABEL_MANAGED.to_string(), "true".to_string());
            labels.insert(LABEL_SERVICE_ID.to_string(), service_id.to_string());

            // 租户私網に pool 内の /24 を明示割当し、源 CIDR で識別可能にする(egress の前提・§3.1)。
            let (subnet, gateway) = allocate_subnet(state).await?;
            let req = NetworkCreateRequest {
                name: name.clone(),
                driver: Some("bridge".to_string()),
                labels: Some(labels),
                ipam: Some(Ipam {
                    config: Some(vec![IpamConfig {
                        subnet: Some(subnet),
                        gateway: Some(gateway),
                        ..Default::default()
                    }]),
                    ..Default::default()
                }),
                ..Default::default()
            };
            match state.docker.create_network(req).await {
                Ok(_) => {}
                Err(e) if is_status(&e, 409) => {} // ロック前に作られた等の競合(冪等)
                Err(e) => return Err(AppError::Other(anyhow!("網 {name} の作成に失敗: {e}"))),
            }
        }
    }

    // infra を attach。失敗は伝播させる(infra 不達のまま app を起こすと注入/route が壊れた
    // service になる — 黙って成功させない。reconcile から呼ばれた時は呼び出し側が per-item で log)。
    for container in infra_containers(state) {
        connect(state, &name, container).await?;
    }
    Ok(())
}

/// 私網が既に在るか(inspect で軽く確認)。エラーは「無い」扱い — 新規作成パスへ倒し、実在していれば
/// create が 409 で冪等に握り潰す。
async fn network_exists(state: &AppState, name: &str) -> bool {
    state
        .docker
        .inspect_network(name, None::<bollard::query_parameters::InspectNetworkOptions>)
        .await
        .is_ok()
}

/// infra コンテナを私網へ接続(既接続=403 は冪等に握り潰す)。
async fn connect(state: &AppState, network: &str, container: &str) -> AppResult<()> {
    let req = NetworkConnectRequest {
        container: container.to_string(),
        endpoint_config: None,
    };
    match state.docker.connect_network(network, req).await {
        Ok(()) => Ok(()),
        Err(e) if is_status(&e, 403) => Ok(()), // 既に接続済み(冪等)
        Err(e) => Err(AppError::Other(anyhow!(
            "網 {network} へ {container} の接続に失敗: {e}"
        ))),
    }
}

/// service の私網を撤去する:infra を disconnect(force)→ 網削除。**順序厳守** — endpoint が
/// 残ると remove は "active endpoints" で失敗する。app コンテナは呼び出し側が先に stop_remove
/// 済みである前提(soft_delete / purge / 孤児掃除はいずれもそうしている)。網が無い(404)は成功扱い。
pub(crate) async fn remove_service_network(state: &AppState, service_id: Uuid) -> AppResult<()> {
    let name = svc_network_name(state, service_id);
    for container in infra_containers(state) {
        disconnect(state, &name, container).await;
    }
    match state.docker.remove_network(&name).await {
        Ok(()) => Ok(()),
        Err(e) if is_status(&e, 404) => Ok(()), // 既に無い(冪等)
        Err(e) => Err(AppError::Other(anyhow!("網 {name} の削除に失敗: {e}"))),
    }
}

/// infra コンテナを私網から切断(best-effort:未接続 / 網無し / コンテナ無しは無視 = remove 前掃除)。
async fn disconnect(state: &AppState, network: &str, container: &str) {
    let req = NetworkDisconnectRequest {
        container: container.to_string(),
        force: Some(true),
    };
    if let Err(e) = state.docker.disconnect_network(network, req).await {
        tracing::debug!(error = ?e, network, container, "網 disconnect(best-effort)");
    }
}

/// 網の期望状態への収束(valkey::reconcile_acls と同型:毎 tick fresh SELECT・best-effort・
/// per-item・panic しない)。(1)生存 service には私網 + infra attach を保証、(2)生存 service を
/// 持たない孤児私網(`tsubomi.managed=true` ラベル)を撤去する。infra 単独再起動や手動削除からの
/// 自己回復をここで担保する(起動時収束だけでは塞げない穴)。
pub(crate) async fn reconcile_networks(state: &AppState) {
    // (1) 生存 service に私網を保証。
    let live: Vec<(Uuid,)> = match sqlx::query_as(
        "SELECT id FROM resources WHERE kind = 'service' AND deleted_at IS NULL",
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(error = ?e, "network reconcile: service 一覧の取得に失敗");
            return;
        }
    };
    let mut live_ids: HashSet<Uuid> = HashSet::new();
    for (id,) in &live {
        live_ids.insert(*id);
        if let Err(e) = ensure_service_network(state, *id).await {
            tracing::warn!(error = ?e, %id, "network reconcile: 私網の収束に失敗");
        }
    }

    // (2) 孤児私網 GC:tsubomi 管理網のうち生存 service を持たないものを撤去。
    let mut filters: HashMap<String, Vec<String>> = HashMap::new();
    filters.insert("label".into(), vec![format!("{LABEL_MANAGED}=true")]);
    let opts = ListNetworksOptionsBuilder::default().filters(&filters).build();
    let networks = match state.docker.list_networks(Some(opts)).await {
        Ok(n) => n,
        Err(e) => {
            tracing::warn!(error = ?e, "network reconcile: 網一覧の取得に失敗");
            return;
        }
    };
    let mut removed = 0usize;
    for net in networks {
        let Some(sid) = net
            .labels
            .as_ref()
            .and_then(|l| l.get(LABEL_SERVICE_ID))
            .and_then(|s| s.parse::<Uuid>().ok())
        else {
            continue;
        };
        if live_ids.contains(&sid) {
            continue;
        }
        // スナップショット(上の SELECT)取得後に作られ deploy 中の service を孤児と誤判して
        // 私網を奪わないよう、撤去の直前に最新の生存を fresh 再確認する(背骨「現実は fresh に
        // 読む」。RACE 回避 — これが無いと新規 service の私網を同パスで消し infra を剥がし得る)。
        match super::reconcile::service_alive(state, sid).await {
            Ok(true) => continue, // スナップショット後に作成 = 生存 → 触らない
            Ok(false) => {}
            Err(e) => {
                tracing::warn!(error = ?e, %sid, "network reconcile: 生存再確認に失敗");
                continue;
            }
        }
        match remove_service_network(state, sid).await {
            Ok(()) => removed += 1,
            Err(e) => tracing::warn!(error = ?e, %sid, "network reconcile: 孤児私網の撤去に失敗"),
        }
    }
    tracing::debug!(live = live.len(), orphan_removed = removed, "network reconcile: 網収束");
}
