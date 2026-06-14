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
    ContainerCreateBody, ContainerStatsResponse, ContainerSummary, ContainerSummaryStateEnum,
    HostConfig, RestartPolicy, RestartPolicyNameEnum,
};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, CreateImageOptionsBuilder, ListContainersOptionsBuilder,
    LogsOptionsBuilder, RemoveContainerOptionsBuilder, StatsOptionsBuilder,
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
    /// volume 注入のバインドマウント(`"<host_path>:<mount_path>"`)。空なら無し。
    pub binds: Vec<String>,
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
        // volume 注入のバインドマウント(`<host_path>:<mount_path>`。S6)。無ければ付けない。
        binds: (!spec.binds.is_empty()).then(|| spec.binds.clone()),
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

/// reconcile 用の緩い存在判定:running / restarting のコンテナが 1 つでもあれば true。
/// 厳格な `is_live`(restart_count==0 を要求)とは**別物** — reconcile であれを使うと
/// クラッシュループ中(restarting)のコンテナを「不在」と誤判し毎パス作り直してしまう。
/// restarting は docker の restart policy が面倒を見ているので「存在」とみなし手出ししない。
pub(crate) async fn is_present(state: &AppState, service_id: Uuid) -> AppResult<bool> {
    use ContainerSummaryStateEnum::{RESTARTING, RUNNING};
    Ok(list_by_service(state, service_id)
        .await?
        .iter()
        .any(|c| matches!(c.state, Some(RUNNING | RESTARTING))))
}

/// 全ての管理コンテナ(`tsubomi.managed=true`)を `(コンテナ id, service_id ラベルの parse 結果)`
/// で返す(停止中も含む)。reconcile の孤児検出が使う:service_id が DB に生きた行を持たなければ
/// 孤児。ラベルが欠落 / 不正なら `None`(個別削除の対象)。
pub(crate) async fn list_managed(state: &AppState) -> AppResult<Vec<(String, Option<Uuid>)>> {
    let mut filters: HashMap<String, Vec<String>> = HashMap::new();
    filters.insert("label".into(), vec![format!("{LABEL_MANAGED}=true")]);
    let opts = ListContainersOptionsBuilder::default()
        .all(true)
        .filters(&filters)
        .build();
    let containers = state
        .docker
        .list_containers(Some(opts))
        .await
        .map_err(|e| AppError::Other(anyhow!("管理コンテナ一覧に失敗: {e}")))?;
    Ok(containers
        .into_iter()
        .filter_map(|c| {
            let id = c.id?;
            let sid = c
                .labels
                .as_ref()
                .and_then(|l| l.get(LABEL_SERVICE_ID))
                .and_then(|s| Uuid::parse_str(s).ok());
            Some((id, sid))
        })
        .collect())
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

/// 名前 or id でコンテナを停止 + 強制削除(冪等)。**削除の失敗は伝播する**(stop / delete /
/// purge が孤児を取り残さないため。stop は既に止まっていてもよいので無視)。swap の片付けだけは
/// best-effort にしたいので呼び出し側(remove_one / run_digest の remove_others)で握り潰す。
async fn force_remove(state: &AppState, name_or_id: &str) -> AppResult<()> {
    let _ = state.docker.stop_container(name_or_id, None).await;
    let rm = RemoveContainerOptionsBuilder::default().force(true).build();
    state
        .docker
        .remove_container(name_or_id, Some(rm))
        .await
        .map_err(|e| AppError::Other(anyhow!("コンテナ削除に失敗({name_or_id}): {e}")))
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
            force_remove(state, &id).await?;
        }
    }
    Ok(())
}

/// 指定 service の現行コンテナを **全て** 停止 + 削除(service 削除 / stop / 失敗時の全掃除)。冪等。
pub async fn stop_remove(state: &AppState, service_id: Uuid) -> AppResult<()> {
    remove_service_containers(state, service_id, None).await
}

/// 指定 service の **keep_name 以外** を停止 + 削除(start-first swap の収尾:新コンテナを
/// 起こして route を切り替えた後に、旧コンテナだけを消す)。冪等。
pub async fn remove_others(state: &AppState, service_id: Uuid, keep_name: &str) -> AppResult<()> {
    remove_service_containers(state, service_id, Some(keep_name)).await
}

/// 名前指定で 1 つだけ停止 + 削除(start-first 失敗時の新コンテナ片付け)。best-effort
/// (失敗しても reconcile が孤児として後で掃除する。ここで失敗を伝播させない)。
pub async fn remove_one(state: &AppState, name: &str) {
    let _ = force_remove(state, name).await;
}

/// 指定 service の(現行)コンテナの直近ログを text で返す(stdout+stderr、tail 行)。
/// コンテナが無い(stopped / 未デプロイ)→ 空文字。stream を行ごとに集約する(follow はしない)。
pub async fn logs(state: &AppState, service_id: Uuid, tail: Option<usize>) -> AppResult<String> {
    // service のコンテナ(start-first 後は通常 1 つ)。無ければ空。
    let Some(name) = list_by_service(state, service_id)
        .await?
        .into_iter()
        .find_map(|c| c.id)
    else {
        return Ok(String::new());
    };

    // tail に上限(既定 200、最大 2000)。巨大 tail で docker ログ全量をメモリに載せない。
    let tail_s = tail.unwrap_or(200).min(2000).to_string();
    let opts = LogsOptionsBuilder::default()
        .stdout(true)
        .stderr(true)
        .tail(&tail_s)
        .build();
    let mut stream = state.docker.logs(&name, Some(opts));
    let mut out = String::new();
    while let Some(item) = stream.next().await {
        match item {
            Ok(line) => out.push_str(&line.to_string()),
            Err(e) => return Err(AppError::Other(anyhow!("ログ取得に失敗: {e}"))),
        }
    }
    Ok(out)
}

/// owner ガバナンスの監視指標(M4 S1)。`(cpu_pct, mem_bytes)` を 1 サンプルで返す。
pub struct ServiceStat {
    /// CPU 使用率(%)。算出不能(system delta 0 / フィールド欠落)は None。
    pub cpu_pct: Option<f64>,
    /// 内存使用量(bytes)。
    pub mem_bytes: i64,
}

/// 指定 service の **稼働中** コンテナを 1 サンプル stats する(owner 可視化、§3.3)。
/// コンテナ不在 / 停止中 / 取得失敗は None(UI は「-」表示 = best-effort)。
///
/// `stream=false` で one_shot を**付けない**:daemon が約 1 秒間隔の 2 サンプルから
/// `precpu_stats` を埋めてくれるので CPU% を算出できる(one_shot=true だと precpu が
/// 無く CPU が出せない)。代わりに 1 コンテナにつき ~1 秒かかるので、呼び出し側
/// (overview/ranking)は service を並行に集める。
pub async fn stats(state: &AppState, service_id: Uuid) -> Option<ServiceStat> {
    let name = list_by_service(state, service_id)
        .await
        .ok()?
        .into_iter()
        .find(|c| matches!(c.state, Some(ContainerSummaryStateEnum::RUNNING)))
        .and_then(|c| c.id)?;

    let opts = StatsOptionsBuilder::default().stream(false).build();
    let sample = state.docker.stats(&name, Some(opts)).next().await?.ok()?;

    let mem_bytes = sample
        .memory_stats
        .as_ref()
        .and_then(|m| m.usage)
        .unwrap_or(0) as i64;
    Some(ServiceStat {
        cpu_pct: compute_cpu_pct(&sample),
        mem_bytes,
    })
}

/// Docker 公式の CPU% 算出:`(cpu_delta / system_delta) * online_cpus * 100`。
/// precpu(前サンプル)が無い / system delta が 0 なら None。
fn compute_cpu_pct(s: &ContainerStatsResponse) -> Option<f64> {
    let cpu = s.cpu_stats.as_ref()?;
    let pre = s.precpu_stats.as_ref()?;
    let cpu_delta = cpu
        .cpu_usage
        .as_ref()?
        .total_usage?
        .checked_sub(pre.cpu_usage.as_ref()?.total_usage?)? as f64;
    let sys_delta = cpu.system_cpu_usage?.checked_sub(pre.system_cpu_usage?)? as f64;
    if sys_delta <= 0.0 {
        return None;
    }
    let ncpu = cpu.online_cpus.unwrap_or(1).max(1) as f64;
    Some((cpu_delta / sys_delta) * ncpu * 100.0)
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
