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
use bollard::models::{NetworkConnectRequest, NetworkCreateRequest, NetworkDisconnectRequest};
use bollard::query_parameters::ListNetworksOptionsBuilder;
use std::collections::{HashMap, HashSet};
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

/// service の私網を冪等に用意する:無ければ作成 → infra(traefik/pgbouncer/valkey)を attach。
/// **順序が肝心** — app コンテナ起動の直前に呼び、DNS 解決 + traefik 経路を成立させてから start する。
/// 既存網は 409、既接続 infra は 403 で、どちらも冪等に握り潰す(2 回目以降の deploy は全部これ)。
pub(crate) async fn ensure_service_network(state: &AppState, service_id: Uuid) -> AppResult<()> {
    let name = svc_network_name(state, service_id);

    // 管理ラベルを付けて作成(GC が `tsubomi.managed=true` で列挙し service_id を読む)。
    // Docker Engine 29 は同名網を 409 で弾くので、create + 409 無視で冪等(list 事前確認は不要)。
    // 網名は我々が生成した service_id(UUID)を含むので、409=自分の既存網。無関係な網との
    // 名前衝突は事実上起き得ない(ので 409 時のラベル検証は省く — 毎 ensure の inspect を避ける)。
    let mut labels: HashMap<String, String> = HashMap::new();
    labels.insert(LABEL_MANAGED.to_string(), "true".to_string());
    labels.insert(LABEL_SERVICE_ID.to_string(), service_id.to_string());
    let req = NetworkCreateRequest {
        name: name.clone(),
        driver: Some("bridge".to_string()),
        labels: Some(labels),
        ..Default::default()
    };
    match state.docker.create_network(req).await {
        Ok(_) => {}
        Err(e) if is_status(&e, 409) => {} // 既存(冪等)
        Err(e) => return Err(AppError::Other(anyhow!("網 {name} の作成に失敗: {e}"))),
    }

    // infra を attach。失敗は伝播させる(infra 不達のまま app を起こすと注入/route が壊れた
    // service になる — 黙って成功させない。reconcile から呼ばれた時は呼び出し側が per-item で log)。
    for container in infra_containers(state) {
        connect(state, &name, container).await?;
    }
    Ok(())
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
