//! M6 ネットワーク隔離:service ごとに専用 bridge プライベートネットワーク `<prefix><id>` を与え、テナント app を
//! 互いに隔離する(東西向=横移動の遮断。背骨「隔離は仕組みで守る」)。
//!
//! infra(traefik / pgbouncer / valkey)はこのプライベートネットワークへ on-demand で attach され、
//! ルーティング(traefik が **コンテナ名**でバックエンドを引く route.rs)と注入(`tsubomi-pgbouncer` /
//! `tsubomi-valkey` の DNS 解決 — inject.rs)を per-service 網内でも成立させる。**注入文字列・
//! route の yaml は無改修**:同名コンテナ DNS はプライベートネットワークに attach すれば引けるため。pgbouncer/valkey
//! はプライベートネットワークからも到達可だが、隔離は資格(pg role / valkey ACL)が担保 = データ安全は本変更で不変。
//!
//! **service↔service 内部リンク**(`doc/paas-service-link-design.md`):A が B を注入すると、B(callee)
//! の稼働コンテナを A(caller)のプライベートネットワークへ **docker ネットワーク別名 = B の subdomain** で客人 attach する。A は
//! `http://<subdomain>:<port>` を docker DNS で引いて B へ直接接続できる(インターネットを通らない)。同一 owner 限定
//! (注入作成時に担保)= テナント横断の東西向は開かない。caller 側は `ensure_service_network`(deploy 前 +
//! reconcile)、callee 側は `attach_as_callee`(B の deploy 直後)で収束、eject は `detach_callee` で即掃除。
//!
//! ライフサイクルは **service 紐づき**(deploy ではない):start-first swap の新旧コンテナは
//! 同じプライベートネットワークに同居する。create は冪等(`run()` がコンテナ起動の直前に ensure)、撤去は
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

/// service のプライベートネットワーク名 `<prefix><service_id>`(prefix は config、既定 `tsubomi-svc-`)。
pub(crate) fn svc_network_name(state: &AppState, service_id: Uuid) -> String {
    format!("{}{}", state.config.svc_network_prefix, service_id)
}

/// per-service プライベートネットワークへ attach / detach する infra コンテナ名(単一の出所)。
/// traefik=route のバックエンド解決 / pgbouncer=DB 注入の DNS / valkey=cache 注入の DNS。
fn infra_containers(state: &AppState) -> [&str; 3] {
    let cfg = &state.config;
    [
        &cfg.traefik_container,
        &cfg.pgbouncer_container,
        &cfg.valkey_container,
    ]
}

/// pgbouncer をプライベートネットワークへ attach するときに付ける docker ネットワーク別名。**注入する接続文字列の host**
/// (`db_internal_host`)がコンテナ名と違う部署では、その名前で引けないと繋がらない — 名前を
/// pgbouncer の client TLS 証書の公開名に揃える設計(m3 設計 §11 決定 A')の実体はここ。
/// コンテナ名と同じ(dev / 旧部署)なら別名は不要 = 空。
///
/// **なぜ compose ではここなのか**:テナントコンテナは per-service プライベートネットワークにしか居らず(M6 ネットワーク隔離、
/// `docker.rs` の `network_mode`)、`tsubomi-edge` はプラットフォームコードから参照されない残骸。compose 側で
/// edge に別名を生やしてもテナントからは見えない。
fn pgbouncer_aliases(state: &AppState) -> Vec<String> {
    let cfg = &state.config;
    if cfg.db_internal_host == cfg.pgbouncer_container {
        return Vec::new();
    }
    vec![cfg.db_internal_host.clone()]
}

/// bollard の Error が指定 HTTP ステータスか(冪等化のための分岐に使う)。
fn is_status(e: &bollard::errors::Error, code: u16) -> bool {
    matches!(
        e,
        bollard::errors::Error::DockerResponseServerError { status_code, .. } if *status_code == code
    )
}

/// テナントプライベートネットワークの subnet サイズ。`tenant_pool`(/24 以上を起動時検証済み)から この大きさで切り出す。
const TENANT_SUBNET_PREFIX_LEN: u8 = 24;

/// 網の「採番 → 作成」を直列化するプロセス内ロック。これが無いと、別 service の同時 deploy が同じ
/// docker 網スナップショットを見て同一の空き /24 を選び、2 つ目の create が subnet 重複で虚假失敗
/// する(最悪 同一 CIDR を共有して E2 の「全租户網は pool 内・互いに別 subnet」不変条件を壊す)。
/// 作成は新規 service 時のみで稀なので、直列化のコストは無視できる(reconcile は元々逐次)。
/// tokio の Mutex::new は const ではないので LazyLock で包む。
static NET_ALLOC_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// テナントプライベートネットワークに与える (subnet, gateway) を `config.tenant_pool` から採番する。pool 内で、現存する
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

/// 生存する tsubomi-svc 網(`tsubomi.managed=true`)の subnet 一覧。egress の「同桥東西向は許可」
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

/// service のプライベートネットワークを冪等に用意する:無ければ pool から /24 を採番して作成 → infra(traefik/pgbouncer/
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

            // 租户プライベートネットワークに pool 内の /24 を明示割当し、源 CIDR で識別可能にする(egress の前提・§3.1)。
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
        let aliases = if container == state.config.pgbouncer_container {
            pgbouncer_aliases(state)
        } else {
            Vec::new()
        };
        connect(state, &name, container, &aliases).await?;
    }

    // この service が注入する別 service(callee)をプライベートネットワークへ客人 attach(別名=callee.subdomain)。
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

/// caller のプライベートネットワークへ、その callee 群の稼働コンテナを別名 attach する(best-effort・per-item で log)。
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
        // 対象は callee の **serving コンテナ**(= 直近成功 deploy のコンテナが稼働中の時だけ Some)。
        // DB + docker から解決し **route ファイルに依存しない** — private callee(route 無し)への
        // リンクを成立させるのが要点(公開範囲設計 §5)。in-flight な swap 中も commit 済みの版
        // だけを指すので別名を取り違えない。未稼働(停止 / 未デプロイ / 削除)なら skip。
        let Some(container) = super::serving_container(state, callee_id).await else {
            continue;
        };
        match connect(state, network, &container, std::slice::from_ref(&subdomain)).await {
            // 既接続 = 別名は今回の指定で更新されていない。callee の subdomain 変更後に旧別名が
            // 残っているかもしれないので検査し、陳腐なら付け替える(変更端点 realias_as_callee の
            // 取りこぼしをここで ≤30s に収束させる)。検査は**三値**:inspect 失敗(None)は
            // 「判定不能 = 触らない」— 不明を陳腐扱いすると、健全な稼働リンクを毎 tick
            // force-disconnect(既存 TCP 切断)する周期瞬断になり得る。
            Ok(true) => {
                if endpoint_alias_state(state, &container, network, &subdomain).await
                    != Some(false)
                {
                    continue; // Some(true) = 正しい / None = 判定不能(触らない)
                }
                // 陳腐確定。ただしループ冒頭で読んだ `subdomain` は、set_subdomain(callee の
                // lock 下)の realias と交錯していると**旧値**かもしれない — 動かす直前に
                // fresh 再読し、付いたばかりの正しい新別名を剥がす巻き戻りを防ぐ(lock 後
                // fresh 再確認の家風。lock は取らないので ms 級の窓は残る = 次 tick が回収)。
                let Some(alias) = fresh_subdomain(state, callee_id).await else {
                    continue; // 削除済み / 取得失敗 → 触らない
                };
                if endpoint_has_alias(state, &container, network, &alias).await {
                    continue; // fresh 値では正しかった(読みが古かっただけ)
                }
                match reattach_with_alias(state, network, &container, &alias).await {
                    Ok(true) => {
                        tracing::info!(%callee_id, alias = %alias, "callee の網別名を付け替えました")
                    }
                    Ok(false) => tracing::warn!(
                        %callee_id, alias = %alias,
                        "callee の別名付け替えが確認できません(次の reconcile で再試行)"
                    ),
                    Err(e) => {
                        tracing::warn!(error = ?e, %callee_id, alias = %alias, "callee の別名付け替えに失敗")
                    }
                }
            }
            Ok(false) => {} // fresh connect = 今回の別名が確定している
            Err(e) => {
                tracing::warn!(error = ?e, %callee_id, alias = %subdomain, "callee の attach に失敗");
            }
        }
    }
}

/// callee の現在の subdomain を fresh に読む(未削除のみ)。attach_callees の付け替え直前の
/// 再確認用 — None = 削除済み / 取得失敗 → 呼び出し側は触らない。
async fn fresh_subdomain(state: &AppState, callee_id: Uuid) -> Option<String> {
    sqlx::query_scalar(
        "SELECT d.subdomain FROM service_details d
           JOIN resources r ON r.id = d.resource_id
          WHERE d.resource_id = $1 AND r.deleted_at IS NULL",
    )
    .bind(callee_id)
    .fetch_optional(&state.db)
    .await
    .unwrap_or_default()
}

/// この callee を注入している**生存 caller** の id 一覧(attach_as_callee / realias_as_callee 共用)。
/// DISTINCT — 同一 caller が同じ callee を複数の env 名で注入していても網操作は 1 回でよい。
/// soft-delete 済み caller を除くのは、網撤去に失敗して残った孤児網へ客人を入れ直さないため(codex 監査)。
async fn service_callers(state: &AppState, callee_id: Uuid) -> Result<Vec<Uuid>, sqlx::Error> {
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT DISTINCT i.service_id
           FROM injections i
           JOIN resources caller ON caller.id = i.service_id
           JOIN resources src    ON src.id = i.resource_id
          WHERE i.resource_id = $1
            AND src.kind = 'service'
            AND caller.deleted_at IS NULL",
    )
    .bind(callee_id)
    .fetch_all(&state.db)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// disconnect → 別名付き connect → 閉環確認、の 3 手をまとめる(付け替えの唯一のレシピ)。
/// docker の網別名は**初回 connect でしか確定しない**(既接続 403 は冪等吞み = 別名未更新)ため、
/// 付け替えは必ずこの形になる(`migrate_pgbouncer_aliases` で実証済み)。戻り値 Ok(true) =
/// 別名が付いたことを inspect で確認済み。Ok(false) = connect は通ったが別名が確認できない
/// (disconnect が効かず既接続のままだった等 — 403 吞みの偽成功をここで検出)。
async fn reattach_with_alias(
    state: &AppState,
    network: &str,
    container: &str,
    alias: &str,
) -> AppResult<bool> {
    disconnect(state, network, container).await;
    connect(state, network, container, &[alias.to_string()]).await?;
    Ok(endpoint_has_alias(state, container, network, alias).await)
}

/// B(callee)の新コンテナを、**B を注入している caller 群**のプライベートネットワークへ別名=B.subdomain で attach する。
/// B の deploy(start-first swap)直後に `docker::run` から呼ぶ(旧コンテナ撤去で消えた endpoint を
/// 即補い、次 reconcile までの A→B 断を塞ぐ)。caller 未デプロイ(網無し)なら skip — その caller の
/// deploy 時に `attach_callees` が付ける。best-effort(reconcile が漏れを拾う)。
pub(crate) async fn attach_as_callee(state: &AppState, callee_id: Uuid, subdomain: &str, container: &str) {
    let callers = match service_callers(state, callee_id).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = ?e, %callee_id, "caller 一覧の取得に失敗(網リンク)");
            return;
        }
    };
    for caller_id in callers {
        let net = svc_network_name(state, caller_id);
        if !network_exists(state, &net).await {
            continue; // caller 未デプロイ = その deploy 時に attach される
        }
        if let Err(e) = connect(state, &net, container, &[subdomain.to_string()]).await {
            tracing::warn!(error = ?e, %caller_id, alias = %subdomain, "caller 網への attach に失敗");
        }
    }
}

/// subdomain 変更時の網別名の換血:この service(callee)を注入している全 caller 私網で、
/// 稼働コンテナの別名を新 subdomain へ付け替える(`reattach_with_alias`)。既に正しい別名なら
/// 触らない — 同値変更の再実行(収束の再試行)で健全なリンクを無駄に瞬断しないための速路。
/// best-effort・per-item warn(取りこぼしは reconcile の別名検査(attach_callees)が ≤30s で
/// 収束させる。caller の deploy とは lock を共有しない = 交錯で旧別名が一時復活し得るが、
/// 同じく次 tick が回収する — 設計 doc §1)。
pub(crate) async fn realias_as_callee(
    state: &AppState,
    callee_id: Uuid,
    subdomain: &str,
    container: &str,
) {
    let callers = match service_callers(state, callee_id).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = ?e, %callee_id, "caller 一覧の取得に失敗(別名換血)");
            return;
        }
    };
    for caller_id in callers {
        let net = svc_network_name(state, caller_id);
        if !network_exists(state, &net).await {
            continue; // caller 未デプロイ = その deploy 時に新別名で attach される
        }
        if endpoint_has_alias(state, container, &net, subdomain).await {
            continue; // 既に正しい(同値再実行 / 直前の reconcile が付け替え済み)
        }
        match reattach_with_alias(state, &net, container, subdomain).await {
            Ok(true) => {
                tracing::info!(network = %net, alias = %subdomain, "網別名を付け替えました");
            }
            Ok(false) => tracing::warn!(
                network = %net, alias = %subdomain,
                "connect は成功したが別名が確認できません(reconcile が収束させます)"
            ),
            Err(e) => {
                tracing::warn!(error = ?e, %caller_id, alias = %subdomain, "caller 網への再 attach に失敗");
            }
        }
    }
}

/// eject(リンク削除)時に caller のプライベートネットワークから callee コンテナを即切断(best-effort)。これが無いと
/// callee は次の自分の redeploy まで caller 網に客人として残る(同 owner なので無害だが掃く)。
pub(crate) async fn detach_callee(state: &AppState, caller_id: Uuid, callee_id: Uuid) {
    let net = svc_network_name(state, caller_id);
    if let Ok(Some(container)) = super::docker::running_container_name(state, callee_id).await {
        disconnect(state, &net, &container).await;
    }
}

/// プライベートネットワークが既に在るか(inspect で軽く確認)。エラーは「無い」扱い — 新規作成パスへ倒し、実在していれば
/// create が 409 で冪等に握り潰す。
async fn network_exists(state: &AppState, name: &str) -> bool {
    state
        .docker
        .inspect_network(name, None::<bollard::query_parameters::InspectNetworkOptions>)
        .await
        .is_ok()
}

/// プラットフォームが作った per-service プライベートネットワークの**名前**の集合(`tsubomi.managed=true` ラベルで確定)。
/// 名前接頭辞での判定と違い、compose の網や共有網を絶対に掴まない。
async fn managed_network_names(state: &AppState) -> AppResult<std::collections::HashSet<String>> {
    let mut filters: HashMap<String, Vec<String>> = HashMap::new();
    filters.insert("label".into(), vec![format!("{LABEL_MANAGED}=true")]);
    let opts = ListNetworksOptionsBuilder::default().filters(&filters).build();
    let networks = state
        .docker
        .list_networks(Some(opts))
        .await
        .map_err(|e| AppError::Other(anyhow!("網一覧の取得に失敗: {e}")))?;
    Ok(networks.into_iter().filter_map(|n| n.name).collect())
}

/// 既に在るプライベートネットワークの pgbouncer endpoint に**別名を後付けする**(起動時 1 回)。docker のネットワーク別名は
/// **初回 connect 時にしか確定しない**ので、`pgbouncer_aliases` を導入する前から在ったプライベートネットワークは
/// `connect` の 403(既接続)で冪等に握り潰されて別名が永遠に生えない。そこを塞ぐ移行処理。
///
/// 手順は endpoint 単位の disconnect → 別名付き reconnect。その service の DB 接続は一瞬切れるが
/// (プールが張り直す)、放置すると**注入ホスト名がプライベートネットワークで引けない** = 公開 DNS に落ちて通信が網外へ
/// 出る / 届かない、という遥かに悪い状態が続く。別名が要らない部署(コンテナ名と同じ = dev / 旧部署)は
/// 何もしない。best-effort:失敗しても起動は続け、次の deploy / この関数の次回起動で再試行される。
pub(crate) async fn migrate_pgbouncer_aliases(state: &AppState) {
    let aliases = pgbouncer_aliases(state);
    let Some(want) = aliases.first().cloned() else {
        return; // コンテナ名と同じ = 別名不要
    };
    let container = state.config.pgbouncer_container.clone();
    // **対象はプラットフォームが作った per-service プライベートネットワークだけ**をラベルで確定する(名前接頭辞で判定すると
    // `TSUBOMI_SVC_NETWORK_PREFIX` の設定次第で `tsubomi-edge` や compose の `..._default` まで
    // 掴み、pgbouncer を pg-tenant への網から切って**全 DB を落とす**。codex 深審 2026-07-26)。
    let managed = match managed_network_names(state).await {
        Ok(names) => names,
        Err(e) => {
            tracing::warn!(error = ?e, "網一覧を取れません(ネットワーク別名の移行を飛ばす)");
            return;
        }
    };
    let Ok(info) = state.docker.inspect_container(&container, None).await else {
        tracing::warn!(container, "pgbouncer を inspect できません(ネットワーク別名の移行を飛ばす)");
        return;
    };
    // pgbouncer が今居る網のうち、プラットフォーム管理のプライベートネットワークで **want を持っていない**ものだけを直す。
    let stale: Vec<String> = info
        .network_settings
        .and_then(|s| s.networks)
        .unwrap_or_default()
        .into_iter()
        .filter(|(net, ep)| {
            managed.contains(net) && !ep.aliases.as_deref().unwrap_or_default().contains(&want)
        })
        .map(|(net, _)| net)
        .collect();
    if stale.is_empty() {
        return;
    }
    tracing::info!(
        count = stale.len(),
        alias = %want,
        "既存の per-service プライベートネットワークに pgbouncer のネットワーク別名を後付けします(一瞬 DB が切れます)"
    );
    for net in stale {
        disconnect(state, &net, &container).await;
        // connect の 403(既接続)は冪等成功なので、**disconnect が失敗していると成功に見えてしまう**
        // (別名は付いていない)。周期 reconcile も同じ 403 を握るため、黙って次の再起動まで直らない。
        // よって「別名が実際に付いたか」を inspect で確かめるまでを 1 セットにする(codex 深審)。
        let connected = connect(state, &net, &container, &aliases).await;
        match connected {
            Ok(_) if endpoint_has_alias(state, &container, &net, &want).await => {
                tracing::info!(network = %net, alias = %want, "ネットワーク別名を付け直しました");
            }
            Ok(_) => tracing::error!(
                network = %net, alias = %want,
                "ネットワーク別名が付きませんでした(disconnect が効かず既接続のまま = この service の DB 注入は\
                 旧ホスト名でしか引けません)。手で `docker network disconnect` してから再起動してください"
            ),
            Err(e) => tracing::error!(
                network = %net, error = ?e,
                "ネットワーク別名の付け直しに失敗(この service の DB 注入は繋がりません。再デプロイで復旧)"
            ),
        }
    }
}

/// `container` の `network` 上の endpoint が `alias` を持っているか。**三値**:None = inspect
/// 失敗で判定不能。用途が 2 方向あるため分ける — 付け替えの**トリガー判定**(attach_callees)は
/// None を「触らない」に倒す(不明を陳腐扱いすると健全リンクを毎 tick 瞬断し得る)。
async fn endpoint_alias_state(
    state: &AppState,
    container: &str,
    network: &str,
    alias: &str,
) -> Option<bool> {
    let info = state.docker.inspect_container(container, None).await.ok()?;
    Some(
        info.network_settings
            .and_then(|s| s.networks)
            .and_then(|n| n.get(network).cloned())
            .and_then(|ep| ep.aliases)
            .is_some_and(|a| a.iter().any(|x| x == alias)),
    )
}

/// `endpoint_alias_state` の bool 版(**閉環確認**用 — realias / migrate)。inspect できない =
/// 確認できない → false(成功を騙らない)。act の判定には使わないこと(上の三値を使う)。
async fn endpoint_has_alias(
    state: &AppState,
    container: &str,
    network: &str,
    alias: &str,
) -> bool {
    endpoint_alias_state(state, container, network, alias)
        .await
        .unwrap_or(false)
}

/// コンテナをプライベートネットワークへ接続(既接続=403 は冪等に握り潰す)。`aliases` 非空なら docker ネットワーク別名を付ける
/// (callee を caller の subdomain で引けるようにする。infra は別名なし `&[]` で呼ぶ)。
/// 別名は **初回 connect 時にのみ確定** — 既接続(403)は別名更新できない。pgbouncer は別名導入前から
/// 接続済みのプライベートネットワークが在り得るので起動時に `migrate_pgbouncer_aliases` が後付けし、
/// callee は subdomain 変更で別名が陳腐化し得るので `attach_callees` が既接続時に検査して付け替える。
/// 戻り値 = **既接続だったか**(true なら別名は今回の指定で更新されていない — 呼び出し側の検査材料)。
async fn connect(
    state: &AppState,
    network: &str,
    container: &str,
    aliases: &[String],
) -> AppResult<bool> {
    let endpoint_config = (!aliases.is_empty()).then(|| EndpointSettings {
        aliases: Some(aliases.to_vec()),
        ..Default::default()
    });
    let req = NetworkConnectRequest {
        container: container.to_string(),
        endpoint_config,
    };
    match state.docker.connect_network(network, req).await {
        Ok(()) => Ok(false),
        Err(e) if is_status(&e, 403) => Ok(true), // 既に接続済み(冪等)
        Err(e) => Err(AppError::Other(anyhow!(
            "網 {network} へ {container} の接続に失敗: {e}"
        ))),
    }
}

/// service のプライベートネットワークを撤去する:**網上の全 endpoint を disconnect(force)→ 網削除**。**順序厳守** —
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

/// infra コンテナをプライベートネットワークから切断(best-effort:未接続 / 網無し / コンテナ無しは無視 = remove 前掃除)。
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
/// per-item・panic しない)。(1)生存 service にはプライベートネットワーク + infra + 現リンクの callee attach を保証、
/// (2)生存 service を持たない孤児プライベートネットワーク(`tsubomi.managed=true` ラベル)を撤去、(3)生存 caller のプライベートネットワークに
/// 居残る「現リンクに無い別 service の app コンテナ」(eject 即時 detach の取りこぼし等)を剥がす。
/// infra 単独再起動や手動削除からの自己回復をここで担保する(起動時収束だけでは塞げない穴)。
pub(crate) async fn reconcile_networks(state: &AppState) {
    // (1) 生存 service にプライベートネットワークを保証。
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
            tracing::warn!(error = ?e, %id, "network reconcile: プライベートネットワークの収束に失敗");
        }
    }

    // (2) 孤児プライベートネットワーク GC:tsubomi 管理網のうち生存 service を持たないものを撤去。
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
        // プライベートネットワークを奪わないよう、撤去の直前に最新の生存を fresh 再確認する(背骨「現実は fresh に
        // 読む」。RACE 回避 — これが無いと新規 service のプライベートネットワークを同パスで消し infra を剥がし得る)。
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
            Err(e) => tracing::warn!(error = ?e, %sid, "network reconcile: 孤児プライベートネットワークの撤去に失敗"),
        }
    }

    // (3) 陳腐な客人 GC:生存 caller のプライベートネットワークに居残る「現リンクに無い別 service の app コンテナ」を剥がす。
    //     eject の即時 detach(`detach_callee`)が失敗した等で残った客人を、ここで収束させる(背骨どおり
    //     「DB の期望状態へ現実を寄せる」)。infra は `tsubomi.managed=true` を持たず list_managed に
    //     出ないので対象外 = 安全。caller 自身のコンテナと現リンク先(desired)は温存。
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
            // app コンテナ(managed)で、caller 自身でも現リンク先でもない = 陳腐な客人 → 剥がす。
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
