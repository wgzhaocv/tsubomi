//! M6 網隔離:service ごとに専用 bridge 私網 `<prefix><id>` を与え、テナント app を
//! 互いに隔離する(東西向=横移動の遮断。背骨「隔離は仕組みで守る」)。
//!
//! infra(traefik / pgbouncer / valkey)はこの私網へ on-demand で attach され、
//! ルーティング(traefik が **コンテナ名**で後端を引く route.rs)と注入(`tsubomi-pgbouncer` /
//! `tsubomi-valkey` の DNS 解決 — inject.rs)を per-service 網内でも成立させる。**注入文字列・
//! route の yaml は無改修**:同名コンテナ DNS は私網に attach すれば引けるため。pgbouncer/valkey
//! は私網からも到達可だが、隔離は資格(pg role / valkey ACL)が担保 = データ安全は本変更で不変。
//!
//! **service↔service 内部リンク**(`doc/paas-service-link-design.md`):A が B を注入すると、B(callee)
//! の稼働コンテナを A(caller)の私網へ **docker 網別名 = B の subdomain** で客人 attach する。A は
//! `http://<subdomain>:<port>` を docker DNS で引いて B へ直連できる(公網を通らない)。同一 owner 限定
//! (注入作成時に担保)= 跨租户の東西向は開かない。caller 側は `ensure_service_network`(deploy 前 +
//! reconcile)、callee 側は `attach_as_callee`(B の deploy 直後)で収束、eject は `detach_callee` で即掃除。
//!
//! ライフサイクルは **service 紐づき**(deploy ではない):start-first swap の新旧コンテナは
//! 同じ私網に同居する。create は冪等(`run()` がコンテナ起動の直前に ensure)、撤去は
//! 削除 / 購読 + reconcile の孤児 GC。infra 単独再起動や手動削除からは reconcile が自己回復する。

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use anyhow::anyhow;
use bollard::models::{
    EndpointSettings, Ipam, IpamConfig, NetworkConnectRequest, NetworkCreateRequest,
    NetworkDisconnectRequest,
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
    let used = extract_subnets(&networks);

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

/// docker 網一覧から IPAM の v4 subnet を抜き出す(allocate_subnet / tenant_subnets で共用)。
fn extract_subnets(networks: &[bollard::models::Network]) -> Vec<Ipv4Net> {
    networks
        .iter()
        .filter_map(|n| n.ipam.as_ref())
        .filter_map(|i| i.config.as_ref())
        .flatten()
        .filter_map(|c| c.subnet.as_ref())
        .filter_map(|s| s.parse::<Ipv4Net>().ok())
        .collect()
}

/// 生存する tsubomi-svc 網(`tsubomi.managed=true`)の subnet 一覧。egress の「同桥東西向は放行」
/// (同 subnet 宛 RETURN)を組むのに使う。pool 外の旧網も混ざり得るが、RETURN 例外なので無害。
pub(crate) async fn tenant_subnets(state: &AppState) -> AppResult<Vec<Ipv4Net>> {
    let mut filters: HashMap<String, Vec<String>> = HashMap::new();
    filters.insert("label".into(), vec![format!("{LABEL_MANAGED}=true")]);
    let opts = ListNetworksOptionsBuilder::default().filters(&filters).build();
    let networks = state
        .docker
        .list_networks(Some(opts))
        .await
        .map_err(|e| AppError::Other(anyhow!("網一覧の取得に失敗: {e}")))?;
    Ok(extract_subnets(&networks))
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
        connect(state, &name, container, &[]).await?;
    }

    // この service が注入する別 service(callee)を私網へ客人 attach(別名=callee.subdomain)。
    // **失敗は伝播させない** — リンク 1 本の不調で caller 全体の deploy を止めない(reconcile が後で拾う)。
    // infra と違い「届かなくても caller 自身は起動できる」ので best-effort が正しい。
    attach_callees(state, &name, service_id).await;
    Ok(())
}

/// caller が注入する callee service の (id, subdomain)。未削除の service 注入だけ。
async fn service_callees(state: &AppState, caller_id: Uuid) -> AppResult<Vec<(Uuid, String)>> {
    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT r.id, d.subdomain
           FROM injections i
           JOIN resources r ON r.id = i.resource_id
           JOIN service_details d ON d.resource_id = r.id
          WHERE i.service_id = $1 AND r.kind = 'service' AND r.deleted_at IS NULL",
    )
    .bind(caller_id)
    .fetch_all(&state.db)
    .await?;
    Ok(rows)
}

/// caller の私網へ、その callee 群の稼働コンテナを別名 attach する(best-effort・per-item で log)。
/// callee が未稼働(停止/未デプロイ/削除)なら skip。`ensure_service_network` と reconcile から呼ぶ。
async fn attach_callees(state: &AppState, network: &str, caller_id: Uuid) {
    let callees = match service_callees(state, caller_id).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = ?e, %caller_id, "callee 一覧の取得に失敗(網リンク)");
            return;
        }
    };
    for (callee_id, subdomain) in callees {
        // 対象は callee の **serving 容器**(= 直近成功 deploy の容器が実走中の時だけ Some)。
        // DB + docker から解決し **route ファイルに依存しない** — private callee(route 無し)への
        // リンクを成立させるのが要点(公開範囲設計 §5)。in-flight な swap 中も commit 済みの版
        // だけを指すので別名を取り違えない。未稼働(停止 / 未デプロイ / 削除)なら skip。
        if let Some(container) = super::serving_container(state, callee_id).await
            && let Err(e) = connect(state, network, &container, std::slice::from_ref(&subdomain)).await
        {
            tracing::warn!(error = ?e, %callee_id, alias = %subdomain, "callee の attach に失敗");
        }
    }
}

/// B(callee)の新コンテナを、**B を注入している caller 群**の私網へ別名=B.subdomain で attach する。
/// B の deploy(start-first swap)直後に `docker::run` から呼ぶ(旧コンテナ撤去で消えた endpoint を
/// 即補い、次 reconcile までの A→B 断を塞ぐ)。caller 未デプロイ(網無し)なら skip — その caller の
/// deploy 時に `attach_callees` が付ける。best-effort(reconcile が漏れを拾う)。
pub(crate) async fn attach_as_callee(state: &AppState, callee_id: Uuid, subdomain: &str, container: &str) {
    // caller(i.service_id)が **生存している** service だけを対象にする(soft-delete 済みだが網撤去に
    // 失敗して網が残っている caller の孤児網へ、B redeploy が客人を入れ直す事故を防ぐ — codex 監査)。
    let callers: Vec<(Uuid,)> = match sqlx::query_as(
        "SELECT i.service_id
           FROM injections i
           JOIN resources caller ON caller.id = i.service_id
           JOIN resources src    ON src.id = i.resource_id
          WHERE i.resource_id = $1
            AND src.kind = 'service'
            AND caller.deleted_at IS NULL",
    )
    .bind(callee_id)
    .fetch_all(&state.db)
    .await
    {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = ?e, %callee_id, "caller 一覧の取得に失敗(網リンク)");
            return;
        }
    };
    for (caller_id,) in callers {
        let net = svc_network_name(state, caller_id);
        if !network_exists(state, &net).await {
            continue; // caller 未デプロイ = その deploy 時に attach される
        }
        if let Err(e) = connect(state, &net, container, &[subdomain.to_string()]).await {
            tracing::warn!(error = ?e, %caller_id, alias = %subdomain, "caller 網への attach に失敗");
        }
    }
}

/// eject(リンク削除)時に caller の私網から callee コンテナを即切断(best-effort)。これが無いと
/// callee は次の自分の redeploy まで caller 網に客人として残る(同 owner なので無害だが掃く)。
pub(crate) async fn detach_callee(state: &AppState, caller_id: Uuid, callee_id: Uuid) {
    let net = svc_network_name(state, caller_id);
    if let Ok(Some(container)) = super::docker::running_container_name(state, callee_id).await {
        disconnect(state, &net, &container).await;
    }
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

/// コンテナを私網へ接続(既接続=403 は冪等に握り潰す)。`aliases` 非空なら docker 網別名を付ける
/// (callee を caller の subdomain で引けるようにする。infra は別名なし `&[]` で呼ぶ)。
/// 別名は **初回 connect 時にのみ確定** — 既接続(403)は別名更新できないが、callee は両 attach 経路とも
/// 最初から別名付きで繋ぐので問題にならない(infra は元々別名不要)。
async fn connect(state: &AppState, network: &str, container: &str, aliases: &[String]) -> AppResult<()> {
    let endpoint_config = (!aliases.is_empty()).then(|| EndpointSettings {
        aliases: Some(aliases.to_vec()),
        ..Default::default()
    });
    let req = NetworkConnectRequest {
        container: container.to_string(),
        endpoint_config,
    };
    match state.docker.connect_network(network, req).await {
        Ok(()) => Ok(()),
        Err(e) if is_status(&e, 403) => Ok(()), // 既に接続済み(冪等)
        Err(e) => Err(AppError::Other(anyhow!(
            "網 {network} へ {container} の接続に失敗: {e}"
        ))),
    }
}

/// service の私網を撤去する:**網上の全 endpoint を disconnect(force)→ 網削除**。**順序厳守** —
/// endpoint が残ると remove は "active endpoints" で失敗する。infra に加え、客人として attach された
/// callee コンテナ(service↔service リンク)も剥がす必要があるので、固定 infra 名ではなく inspect で
/// 現接続コンテナを列挙して全部外す。app コンテナは呼び出し側が先に stop_remove 済みである前提
/// (soft_delete / purge / 孤児掃除はいずれもそうしている)。網が無い(404)は成功扱い。
pub(crate) async fn remove_service_network(state: &AppState, service_id: Uuid) -> AppResult<()> {
    let name = svc_network_name(state, service_id);
    // inspect で現在の接続コンテナ(キー=コンテナ id)を列挙し force-disconnect(best-effort・冪等)。
    // inspect が落ちても(網消失など)remove の 404 経路で吸収する。
    if let Ok(net) = state
        .docker
        .inspect_network(&name, None::<bollard::query_parameters::InspectNetworkOptions>)
        .await
        && let Some(containers) = net.containers
    {
        for cid in containers.keys() {
            disconnect(state, &name, cid).await;
        }
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
/// per-item・panic しない)。(1)生存 service には私網 + infra + 現リンクの callee attach を保証、
/// (2)生存 service を持たない孤児私網(`tsubomi.managed=true` ラベル)を撤去、(3)生存 caller の私網に
/// 居残る「現リンクに無い別 service の app 容器」(eject 即時 detach の取りこぼし等)を剥がす。
/// infra 単独再起動や手動削除からの自己回復をここで担保する(起動時収束だけでは塞げない穴)。
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

    // (3) 陳腐な客人 GC:生存 caller の私網に居残る「現リンクに無い別 service の app 容器」を剥がす。
    //     eject の即時 detach(`detach_callee`)が失敗した等で残った客人を、ここで収束させる(背骨どおり
    //     「DB の期望状態へ現実を寄せる」)。infra は `tsubomi.managed=true` を持たず list_managed に
    //     出ないので対象外 = 安全。caller 自身の容器と現リンク先(desired)は温存。
    let cid_to_svc: HashMap<String, Uuid> = super::docker::list_managed(state)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(cid, sid)| sid.map(|s| (cid, s)))
        .collect();
    for caller_id in &live_ids {
        let desired: HashSet<Uuid> = match service_callees(state, *caller_id).await {
            Ok(c) => c.into_iter().map(|(id, _)| id).collect(),
            Err(e) => {
                tracing::warn!(error = ?e, %caller_id, "network reconcile: callee 集合の取得に失敗");
                continue;
            }
        };
        let net = svc_network_name(state, *caller_id);
        let Ok(info) = state
            .docker
            .inspect_network(&net, None::<bollard::query_parameters::InspectNetworkOptions>)
            .await
        else {
            continue;
        };
        let Some(containers) = info.containers else {
            continue;
        };
        for cid in containers.keys() {
            // app 容器(managed)で、caller 自身でも現リンク先でもない = 陳腐な客人 → 剥がす。
            if let Some(svc) = cid_to_svc.get(cid)
                && *svc != *caller_id
                && !desired.contains(svc)
            {
                disconnect(state, &net, cid).await;
            }
        }
    }
    tracing::debug!(live = live.len(), orphan_removed = removed, "network reconcile: 網収束");
}
