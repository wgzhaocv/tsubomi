//! bollard(docker.sock)の薄いラッパ。M3 のコンテナ操作を 1 箇所に集約する:
//! digest 指定の pull / 起動(tsubomi 管理ラベル付き、edge 網のみ。ルーティングは
//! file provider = services/route.rs)/ 旧コンテナの停止削除(swap・削除で再利用)。
//! 後の reconcile(S8)/ lifecycle(S7)もここを通す。
//!
//! ネットワークは tsubomi-edge **のみ**(隔離の一線):コンテナは edge 上の traefik /
//! pgbouncer にしか会えず、infra 内部網(pg-platform / pg-tenant / registry 内部面)には
//! 物理的に届かない。

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use anyhow::anyhow;
use bollard::models::{
    ContainerCreateBody, ContainerSummary, HostConfig, RestartPolicy, RestartPolicyNameEnum,
};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, CreateImageOptionsBuilder, ListContainersOptionsBuilder,
    RemoveContainerOptionsBuilder,
};
use futures_util::StreamExt;
use std::collections::HashMap;
use uuid::Uuid;

/// 平台が付ける管理ラベル(reconcile / 孤児検出 / swap がこれで引く)。
const LABEL_SERVICE_ID: &str = "tsubomi.service_id";
const LABEL_GIT_SHA: &str = "tsubomi.git_sha";
const LABEL_MANAGED: &str = "tsubomi.managed";

/// 起動に必要な service の確定値(run_digest が DB から読んで渡す)。
pub struct RunSpec {
    pub service_id: Uuid,
    /// このコンテナの名前。start-first swap のため **deploy ごとに一意**
    /// (`tsubomi-<id>-<deploy 短码>`):新旧が一瞬共存するので同名衝突を避ける。
    pub container_name: String,
    pub subdomain: String,
    pub git_sha: String,
    pub container_port: i32,
    pub memory_mb: i32,
    pub cpu_shares: i32,
    pub env: Vec<(String, String)>,
}

/// digest 指定で registry から pull する(決定 #3:tag ではなく内容アドレス)。
/// 戻り値は `create_container` に渡す digest ピン留め参照 `<repo>@<digest>`。
pub async fn pull(state: &AppState, service_id: Uuid, image_digest: &str) -> AppResult<String> {
    let repo = format!("{}/{}", state.config.registry_pull, service_id);
    let opts = CreateImageOptionsBuilder::default()
        .from_image(&repo)
        .tag(image_digest)
        .build();
    let mut stream = state.docker.create_image(Some(opts), None, None);
    while let Some(item) = stream.next().await {
        item.map_err(|e| {
            AppError::Other(anyhow!("イメージ pull に失敗({repo}@{image_digest}): {e}"))
        })?;
    }
    Ok(format!("{repo}@{image_digest}"))
}

/// 新コンテナを create + start(edge 網のみ)。コンテナ名は安定 `tsubomi-<id>`
/// (file provider の後端 URL を固定で書けるため。swap は旧停止→新起動なので同名衝突しない)。
/// 起動した container 名を返す。
pub async fn run(state: &AppState, spec: &RunSpec, image_ref: &str) -> AppResult<String> {
    let cfg = &state.config;
    let name = spec.container_name.clone();

    let env: Vec<String> = spec.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
    let labels = mgmt_labels(spec);

    let host_config = HostConfig {
        // tsubomi-edge のみ:infra 内部網には繋がない(隔離の一線)。
        network_mode: Some(cfg.edge_network.clone()),
        // --memory 硬上限(OOM は単一コンテナだけ殺す)/ --cpu-shares ソフト制限。
        memory: Some((spec.memory_mb as i64) * 1024 * 1024),
        cpu_shares: Some(spec.cpu_shares as i64),
        // 第一の保険(reconcile が第二)。
        restart_policy: Some(RestartPolicy {
            name: Some(RestartPolicyNameEnum::UNLESS_STOPPED),
            maximum_retry_count: None,
        }),
        ..Default::default()
    };

    let body = ContainerCreateBody {
        image: Some(image_ref.to_string()),
        env: Some(env),
        labels: Some(labels),
        host_config: Some(host_config),
        ..Default::default()
    };

    let create_opts = CreateContainerOptionsBuilder::default().name(&name).build();
    state
        .docker
        .create_container(Some(create_opts), body)
        .await
        .map_err(|e| AppError::Other(anyhow!("コンテナ作成に失敗: {e}")))?;
    state
        .docker
        .start_container(&name, None)
        .await
        .map_err(|e| AppError::Other(anyhow!("コンテナ起動に失敗: {e}")))?;
    Ok(name)
}

/// 平台の管理ラベルだけ(reconcile / 孤児検出 / swap が `tsubomi.service_id` で引く)。
/// ルーティングは docker provider ではなく **file provider**(services/route.rs)が担うので
/// traefik.* ラベルは付けない(Docker Engine 29 で docker provider が壊れる回避。route.rs / compose 参照)。
fn mgmt_labels(spec: &RunSpec) -> HashMap<String, String> {
    let mut m: HashMap<String, String> = HashMap::new();
    m.insert(LABEL_SERVICE_ID.into(), spec.service_id.to_string());
    m.insert(LABEL_GIT_SHA.into(), spec.git_sha.clone());
    m.insert(LABEL_MANAGED.into(), "true".into());
    m
}

/// 指定 service の管理コンテナ一覧(`tsubomi.service_id` ラベルで引く。停止中も含む)。
async fn list_by_service(state: &AppState, service_id: Uuid) -> AppResult<Vec<ContainerSummary>> {
    let mut filters: HashMap<String, Vec<String>> = HashMap::new();
    filters.insert(
        "label".into(),
        vec![format!("{LABEL_SERVICE_ID}={service_id}")],
    );
    let opts = ListContainersOptionsBuilder::default()
        .all(true)
        .filters(&filters)
        .build();
    state
        .docker
        .list_containers(Some(opts))
        .await
        .map_err(|e| AppError::Other(anyhow!("コンテナ一覧に失敗: {e}")))
}

/// 名前 or id でコンテナを停止 + 強制削除(冪等。失敗はログだけ — 呼び出しを止めない)。
async fn force_remove(state: &AppState, name_or_id: &str) {
    let _ = state.docker.stop_container(name_or_id, None).await;
    let rm = RemoveContainerOptionsBuilder::default().force(true).build();
    if let Err(e) = state.docker.remove_container(name_or_id, Some(rm)).await {
        tracing::warn!(error = ?e, container = %name_or_id, "コンテナ削除に失敗(続行)");
    }
}

/// 指定 service の管理コンテナを停止 + 削除する(冪等)。`keep=Some(name)` ならその名前だけ
/// 残す(start-first swap の収尾)、`keep=None` なら全部消す(service 削除 = S7 / 失敗時の全掃除)。
async fn remove_service_containers(
    state: &AppState,
    service_id: Uuid,
    keep: Option<&str>,
) -> AppResult<()> {
    for c in list_by_service(state, service_id).await? {
        // docker の名前は "/name" 形式で入るので先頭スラッシュを外して比較する。
        let is_keep = keep.is_some_and(|k| {
            c.names
                .as_deref()
                .unwrap_or_default()
                .iter()
                .any(|n| n.trim_start_matches('/') == k)
        });
        if is_keep {
            continue;
        }
        if let Some(id) = c.id {
            force_remove(state, &id).await;
        }
    }
    Ok(())
}

/// 指定 service の現行コンテナを **全て** 停止 + 削除(service 削除 = S7 / 失敗時の全掃除)。冪等。
#[allow(dead_code)] // S7(service delete / stop)で使用予定
pub async fn stop_remove(state: &AppState, service_id: Uuid) -> AppResult<()> {
    remove_service_containers(state, service_id, None).await
}

/// 指定 service の **keep_name 以外** を停止 + 削除(start-first swap の収尾:新コンテナを
/// 起こして route を切り替えた後に、旧コンテナだけを消す)。冪等。
pub async fn remove_others(state: &AppState, service_id: Uuid, keep_name: &str) -> AppResult<()> {
    remove_service_containers(state, service_id, Some(keep_name)).await
}

/// 名前指定で 1 つだけ停止 + 削除(start-first 失敗時の新コンテナ片付け。冪等)。
pub async fn remove_one(state: &AppState, name: &str) {
    force_remove(state, name).await;
}

/// 新コンテナが「起動直後に落ちていない」ことを確認する(就緒ではなく **存活** 判定。
/// 決定 E:HTTP ready 探针は持たない)。起動してすぐ exit する壊れたイメージを swap の
/// **前**に弾き、§6.4「失敗時は旧版を生かす」を守る。少し間を空けて複数回 inspect する。
///
/// ★ RestartPolicy=unless-stopped を付けているので、クラッシュするコンテナは docker に
///   自動再起動され、inspect の瞬間だけ Running=true に見えうる(= 存活と誤判)。これを
///   防ぐため Running だけでなく **restart_count==0 かつ restarting==false** も要求する:
///   起動直後に一度でも再起動していればクラッシュループ = 不健全と判定する。
pub async fn is_live(state: &AppState, name: &str) -> bool {
    for attempt in 0..3 {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        match state.docker.inspect_container(name, None).await {
            Ok(info) => {
                let restart_count = info.restart_count.unwrap_or(0);
                let st = info.state.as_ref();
                let running = st.and_then(|s| s.running).unwrap_or(false);
                let restarting = st.and_then(|s| s.restarting).unwrap_or(false);
                // exit 済み / 再起動中 / 一度でも再起動した → クラッシュ(ループ)と見なす。
                if !running || restarting || restart_count > 0 {
                    return false;
                }
                if attempt == 2 {
                    return true; // 窓の間ずっと running・無再起動を確認できた
                }
            }
            Err(_) => return false,
        }
    }
    true
}
