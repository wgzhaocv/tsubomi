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
use bollard::models::{ContainerCreateBody, HostConfig, RestartPolicy, RestartPolicyNameEnum};
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
    let name = format!("tsubomi-{}", spec.service_id);

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

/// 指定 service の現行コンテナを全て停止 + 削除(swap の旧側 / service 削除で使う)。
/// 既に停止 / 消滅していても続行する(冪等)。
pub async fn stop_remove(state: &AppState, service_id: Uuid) -> AppResult<()> {
    let mut filters: HashMap<String, Vec<String>> = HashMap::new();
    filters.insert(
        "label".into(),
        vec![format!("{LABEL_SERVICE_ID}={service_id}")],
    );
    let opts = ListContainersOptionsBuilder::default()
        .all(true)
        .filters(&filters)
        .build();
    let list = state
        .docker
        .list_containers(Some(opts))
        .await
        .map_err(|e| AppError::Other(anyhow!("コンテナ一覧に失敗: {e}")))?;
    for c in list {
        let Some(id) = c.id else { continue };
        let _ = state.docker.stop_container(&id, None).await;
        let rm = RemoveContainerOptionsBuilder::default().force(true).build();
        if let Err(e) = state.docker.remove_container(&id, Some(rm)).await {
            tracing::warn!(error = ?e, container = %id, "旧コンテナの削除に失敗(続行)");
        }
    }
    Ok(())
}
