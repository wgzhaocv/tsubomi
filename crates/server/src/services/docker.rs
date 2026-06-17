//! bollard(docker.sock)の薄いラッパ。M3 のコンテナ操作を 1 箇所に集約する:
//! digest 指定の pull / 起動(tsubomi 管理ラベル付き、per-service 私網。ルーティングは
//! file provider = services/route.rs)/ 旧コンテナの停止削除(swap・削除で再利用)。
//! 後の reconcile(S8)/ lifecycle(S7)もここを通す。
//!
//! ネットワークは **per-service 私網 `tsubomi-svc-<id>` のみ**(M6 隔離の一線。services/network.rs):
//! コンテナは自分の私網に attach された traefik / pgbouncer / valkey にしか会えず、他テナント
//! app にも infra 内部網(pg-platform / pg-tenant / registry 内部面)にも物理的に届かない。

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use anyhow::anyhow;
use tsubomi_shared::ExecResult;
use bollard::models::{
    ContainerCreateBody, ContainerStatsResponse, ContainerSummary, ContainerSummaryStateEnum,
    HostConfig, HostConfigLogConfig, RestartPolicy, RestartPolicyNameEnum,
};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, CreateImageOptionsBuilder, ListContainersOptionsBuilder,
    LogsOptionsBuilder, RemoveContainerOptionsBuilder, StatsOptionsBuilder,
};
use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use uuid::Uuid;

/// 平台が付ける管理ラベル(reconcile / 孤児検出 / swap がこれで引く)。
/// network.rs も per-service 私網に同じラベル(managed / service_id)を付け GC で引く。
pub(crate) const LABEL_SERVICE_ID: &str = "tsubomi.service_id";
const LABEL_GIT_SHA: &str = "tsubomi.git_sha";
pub(crate) const LABEL_MANAGED: &str = "tsubomi.managed";

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
    // 全体に硬いタイムアウトを被せる:registry が固まると pull は無限に待ち得る。reconcile から
    // 呼ばれた場合に 1 件の hang が後背の収束ループ全体を凍らせる穴を塞ぐ(perf review P4)。
    let drain = async {
        let mut stream = state.docker.create_image(Some(opts), None, None);
        while let Some(item) = stream.next().await {
            item.map_err(|e| {
                AppError::Other(anyhow!("イメージ pull に失敗({repo}@{image_digest}): {e}"))
            })?;
        }
        Ok::<(), AppError>(())
    };
    match tokio::time::timeout(std::time::Duration::from_secs(180), drain).await {
        Ok(r) => r?,
        Err(_) => {
            return Err(AppError::Other(anyhow!(
                "イメージ pull がタイムアウトしました(180s。{repo}@{image_digest})"
            )));
        }
    }
    Ok(format!("{repo}@{image_digest}"))
}

/// 新コンテナを create + start(per-service 私網のみ。起動の直前に ensure_service_network で
/// 私網を用意し infra を attach 済みにする)。コンテナ名は **deploy ごとに一意**(`RunSpec` 参照)
/// で、start-first swap の新旧が同じ私網に同居しても衝突しない。起動した container 名を返す。
pub async fn run(state: &AppState, spec: &RunSpec, image_ref: &str) -> AppResult<String> {
    let name = spec.container_name.clone();

    let env: Vec<String> = spec.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
    let labels = mgmt_labels(spec);

    let host_config = HostConfig {
        // per-service 私網のみ(M6 網隔離):他テナント app には届かず、infra 内部網にも繋がない。
        // 私網は下の ensure_service_network が起動の直前に用意し、infra を attach 済みにする。
        network_mode: Some(super::network::svc_network_name(state, spec.service_id)),
        // volume 注入のバインドマウント(`<host_path>:<mount_path>`。S6)。無ければ付けない。
        binds: (!spec.binds.is_empty()).then(|| spec.binds.clone()),
        // --memory 硬上限(OOM は単一コンテナだけ殺す)/ --cpu-shares ソフト制限。
        memory: Some((spec.memory_mb as i64) * 1024 * 1024),
        cpu_shares: Some(spec.cpu_shares as i64),
        // 容器加固(背骨「隔離は仕組みで守る」。memory 硬上限の隣に並べる宿主機保護):
        //  - pids_limit:tasks(プロセス+スレッド)上限。fork 爆弾で宿主機の PID を食い潰させない。
        //    512 は単一 app には潤沢、かつ暴走を確実に頭打ちにする(memory 既定 512MB と整合)。
        pids_limit: Some(512),
        //  - log_config:json-file をローテート(10MB×3=最大 30MB/コンテナ)。無制限ログで
        //    宿主機ディスクを埋めさせない(平台のログ取得は引き続き docker logs = json-file)。
        log_config: Some(HostConfigLogConfig {
            typ: Some("json-file".to_string()),
            config: Some(HashMap::from([
                ("max-size".to_string(), "10m".to_string()),
                ("max-file".to_string(), "3".to_string()),
            ])),
        }),
        //  - no-new-privileges:setuid/setgid バイナリでの権限昇格を封じる。
        security_opt: Some(vec!["no-new-privileges=true".to_string()]),
        //  - cap_drop NET_RAW:生ソケットを奪い、私網内 infra への ARP / パケット偽装を断つ
        //    (per-service 私網 §M6 と二段構え。正規 app で NET_RAW を要るものはほぼ無い)。
        cap_drop: Some(vec!["NET_RAW".to_string()]),
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

    // per-service 私網を冪等に用意し infra を attach する(起動の直前。DNS 解決と traefik
    // 経路の成立のため、create より前である必要がある)。失敗時は起こさない(壊れた service を作らない)。
    super::network::ensure_service_network(state, spec.service_id).await?;
    // 【デプロイ経路で必須・周期 reconcile とは別役割】新規 deploy は新しい /24 の網を作って即この先で
    // コンテナを起動する。ここで egress を収束させないと、新 subnet の「同桥 RETURN」が入る前に app が
    // 起き、app→pgbouncer/valkey が `pool→私網 DROP` に巻かれて次の 30s reconcile まで DB/cache に繋がらない。
    // reconcile_pass 側の呼び出しは周期リフレッシュで、これとは別物(消すとデプロイ直後に穴が開く)。
    super::egress::reconcile(state).await;

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

/// reconcile 用スナップショット:**1 回の list_by_service** で `(存在するか, 走行容器名の集合)` を返す。
/// 存在収束(消えた容器の復活)と route ドリフト収束(route が正しい容器を指すか)が、これ 1 回で両方
/// 賄える(以前は是非判定と容器名取得で 2 回 docker を叩いていた)。
///  - **存在** = running / restarting が 1 つでも。厳格な `is_live`(restart_count==0 を要求)とは別物 —
///    クラッシュループ中(restarting)を「不在」と誤判すると毎パス作り直してしまう。restarting は
///    docker の restart policy が面倒を見るので「存在」とみなし手出ししない。
///  - **走行容器名** = RUNNING な全コンテナ名(先頭 `/` 除去 = route backend の docker DNS 名)。
///    start-first swap の旧片付け(`remove_others`、best-effort)が失敗すると新旧が併存し得るので、
///    呼び出し側は「どれが正か」を **deploy 履歴**(直近成功 deploy の容器名)で決める。ここは候補の列挙だけ
///    (任意の 1 つを「正」と決めない — それが route を旧版へ巻き戻す事故の元だった)。restarting のみは空。
pub(crate) async fn presence(
    state: &AppState,
    service_id: Uuid,
) -> AppResult<(bool, Vec<String>)> {
    use ContainerSummaryStateEnum::{RESTARTING, RUNNING};
    let mut present = false;
    let mut running_names = Vec::new();
    for c in list_by_service(state, service_id).await? {
        match c.state {
            Some(RUNNING) => {
                present = true;
                if let Some(name) = c.names.and_then(|ns| ns.into_iter().next()) {
                    running_names.push(name.trim_start_matches('/').to_string());
                }
            }
            Some(RESTARTING) => present = true,
            _ => {}
        }
    }
    Ok((present, running_names))
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

/// `logs` の出力バイト上限(行数 tail に加えた第二の安全弁。概算 1 MiB)。超えたら打ち切る。
const LOGS_OUTPUT_CAP: usize = 1024 * 1024;

/// 指定 service の(現行)コンテナの直近ログを text で返す(stdout+stderr、tail 行)。
/// コンテナが無い(stopped / 未デプロイ)→ 空文字。stream を行ごとに集約する(follow はしない)。
/// 注:`logs_by_name` とループが似るが **意図的に分離**する — こちらは API エンドポイント
/// (`GET /services/:id/logs`)で Docker エラーを Err として表に出す契約。`logs_by_name` は
/// 失敗 deploy 診断用の best-effort で取得失敗を握りつぶす = エラー契約が逆なので共有しない。
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
    let mut truncated = false;
    while let Some(item) = stream.next().await {
        match item {
            Ok(line) => {
                let s = line.to_string();
                // 行数 tail に加えた**バイト上限**(第二の安全弁):少数の超長行で server の
                // メモリ / 応答 JSON を膨らませない(exec_capture の EXEC_OUTPUT_CAP と同趣旨)。
                // 上限を跨ぐ行は丸ごと落として打ち切る(char 境界を気にせず安全)。
                if out.len() + s.len() > LOGS_OUTPUT_CAP {
                    truncated = true;
                    break;
                }
                out.push_str(&s);
            }
            Err(e) => return Err(AppError::Other(anyhow!("ログ取得に失敗: {e}"))),
        }
    }
    if truncated {
        out.push_str("\n…(ログが大きいため切り詰めました。tail で行数を絞ってください)\n");
    }
    Ok(out)
}

/// 指定した **名前**のコンテナの直近ログ(stdout+stderr、tail 行)。ベストエフォート
/// (取得失敗・コンテナ不在は空文字)。失敗 deploy で掃除される前の死にかけコンテナから、
/// クラッシュ原因をエラーに載せるために使う(`logs` は現行コンテナを service_id で引く別経路)。
pub async fn logs_by_name(state: &AppState, name: &str, tail: usize) -> String {
    let tail_s = tail.min(2000).to_string();
    let opts = LogsOptionsBuilder::default()
        .stdout(true)
        .stderr(true)
        .tail(&tail_s)
        .build();
    let mut stream = state.docker.logs(name, Some(opts));
    let mut out = String::new();
    while let Some(item) = stream.next().await {
        match item {
            Ok(line) => out.push_str(&line.to_string()),
            Err(_) => break, // ベストエフォート:取得失敗は静かに打ち切る
        }
    }
    out
}

/// service の **稼働中**コンテナ名を返す(start-first 後は通常 1 つ)。停止中 / 未デプロイ /
/// 不在は None。exec / terminal が共有する「中に入れる相手」の解決ロジック。`logs` は停止直前の
/// クラッシュ診断のため**任意状態**のコンテナを引く別契約なので共有しない(こちらは exec 可能 =
/// RUNNING 限定。stats[:319] と同じ絞り込み)。
pub async fn running_container_name(
    state: &AppState,
    service_id: Uuid,
) -> AppResult<Option<String>> {
    Ok(list_by_service(state, service_id)
        .await?
        .into_iter()
        .find(|c| matches!(c.state, Some(ContainerSummaryStateEnum::RUNNING)))
        .and_then(|c| c.id))
}

/// 出力捕獲の上限(stdout+stderr 合計の概算 bytes)。超えたら打ち切り `truncated=true`。
/// 巨大出力をメモリに丸ごと載せない(`tbm service exec app -- cat huge` 等)。
const EXEC_OUTPUT_CAP: usize = 1024 * 1024;
/// 1 コマンドの最大実行時間。超えたら捕獲済みを返して `timed_out=true`(長時間 / 対話は
/// web ターミナルへ誘導)。exec プロセス自体は容器内に残り、容器の終了 / 再デプロイで回収される。
const EXEC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// 稼働中コンテナ内で 1 コマンドを **非対話**に実行し、stdout/stderr/exit_code を捕獲して返す
/// (`docker exec`(`-it` なし)相当)。CLI `tbm service exec` / 線上診断の土台。対話 PTY が
/// 要るときは web ターミナル(`handle_terminal`)。registry GC[:249] とほぼ同型だが、こちらは
/// **tty なし** = daemon が多重化で stdout/stderr を分離するので別々に蓄積する。
pub async fn exec_capture(
    state: &AppState,
    container_name: &str,
    cmd: Vec<String>,
) -> AppResult<ExecResult> {
    use bollard::container::LogOutput;
    use bollard::exec::{CreateExecOptions, StartExecResults};

    let created = state
        .docker
        .create_exec(
            container_name,
            CreateExecOptions::<String> {
                cmd: Some(cmd),
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                // tty は立てない(既定 false):多重化を効かせて stdout/stderr を分離捕獲する。
                ..Default::default()
            },
        )
        .await
        .map_err(|e| AppError::Other(anyhow!("コマンドの起動準備に失敗: {e}")))?;

    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut truncated = false;
    let mut timed_out = false;

    if let StartExecResults::Attached { mut output, .. } = state
        .docker
        .start_exec(&created.id, None)
        .await
        .map_err(|e| AppError::Other(anyhow!("コマンドの起動に失敗: {e}")))?
    {
        // 出力はドレインしないと exec が滞留する。上限到達後もストリームは読み続ける
        // (registry GC と同じ作法)。全体に硬いタイムアウトを被せる。
        let drain = async {
            while let Some(item) = output.next().await {
                let Ok(chunk) = item else { break };
                // 残り予算を先に計算してから 1 つの buffer を借りる(両 len を借用衝突なく読む)。
                let remaining = EXEC_OUTPUT_CAP.saturating_sub(stdout.len() + stderr.len());
                let (buf, message) = match chunk {
                    LogOutput::StdErr { message } => (&mut stderr, message),
                    // tty なしで StdIn/Console は通常来ないが、来ても stdout 側へ寄せる。
                    LogOutput::StdOut { message }
                    | LogOutput::Console { message }
                    | LogOutput::StdIn { message } => (&mut stdout, message),
                };
                if message.len() > remaining {
                    // 1 フレームで予算を超える分は切り詰める(cap を厳密に保つ)。残りはドレイン継続。
                    buf.push_str(&String::from_utf8_lossy(&message[..remaining]));
                    truncated = true;
                } else {
                    buf.push_str(&String::from_utf8_lossy(&message));
                }
            }
        };
        if tokio::time::timeout(EXEC_TIMEOUT, drain).await.is_err() {
            timed_out = true;
        }
    }

    // exit_code を確認(timeout 時はまだ走っているので None になり得る = データとして返す)。
    let exit_code = state
        .docker
        .inspect_exec(&created.id)
        .await
        .ok()
        .and_then(|i| i.exit_code);

    Ok(ExecResult {
        stdout,
        stderr,
        exit_code,
        truncated,
        timed_out,
    })
}

/// 対話ターミナル 1 セッションの最大時間。逆プロキシ(CF Tunnel)越しの半開き接続で
/// `recv` も `output` も EOF せず `sh` が生き残るのを防ぐ backstop(axum は Ping に自動 Pong
/// するので liveness はプロキシ依存 = この timeout が最後の砦)。
const TERMINAL_MAX_SESSION: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// 1 WS セッション:所有者が自分の稼働中コンテナ内で対話シェル(`/bin/sh`)を開く(web 専用)。
/// 升级前に `ensure_owned` + コンテナ稼働中は確認済み。container_name は解決済み。
///
/// ワイヤープロトコル:**client→server** は `Binary`=生 stdin / `Text`=制御 `{"type":"resize",…}`、
/// **server→client** は exec 出力を `Binary`。詳細・地雷はコメント参照(Plan critique 反映)。
pub async fn handle_terminal(socket: WebSocket, state: AppState, container_name: String) {
    use bollard::exec::{CreateExecOptions, ResizeExecOptions, StartExecOptions, StartExecResults};
    use tokio::io::AsyncWriteExt;

    // 1) exec を作る。**tty を create / start の両方で立てる**:片方だけだと daemon の 8 バイト
    //    多重化ヘッダの有無が decoder とずれ、出力が壊れる(xterm にゴミ)。env に TERM。
    let created = match state
        .docker
        .create_exec(
            &container_name,
            CreateExecOptions {
                cmd: Some(vec!["/bin/sh"]),
                tty: Some(true),
                attach_stdin: Some(true),
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                env: Some(vec!["TERM=xterm-256color"]),
                ..Default::default()
            },
        )
        .await
    {
        Ok(c) => c,
        // 升级後はもう HTTP ステータスを返せない。開いた socket に人間可読のバイト列
        //(xterm がそのまま表示)+ Close で伝える。内部 docker 詳細は漏らさない。
        Err(_) => return terminal_fail(socket, "シェルを起動できませんでした").await,
    };

    // 2) start も tty:true。これで PTY が生まれる(resize は start 後にのみ有効)。
    let (mut output, mut input) = match state
        .docker
        .start_exec(
            &created.id,
            Some(StartExecOptions {
                detach: false,
                tty: true,
                output_capacity: None,
            }),
        )
        .await
    {
        Ok(StartExecResults::Attached { output, input }) => (output, input),
        // Detached / エラーともシェルは使えない。
        _ => return terminal_fail(socket, "シェルを起動できませんでした").await,
    };

    // 3) WS を送受信に分割する。**input と output は同一ハイジャック TCP の両半分** なので、
    //    1 つの select で直列化すると遅い write が出力を塞ぐ(HOL ブロック)。2 方向を独立に進める。
    let (mut ws_tx, mut ws_rx) = socket.split();

    // 方向A:コンテナ → クライアント。背圧は send().await に任せる(余分な mpsc を挟まない =
    //    暴走プロセスで無制限バッファになる)。tty 下なので into_bytes() は生 PTY バイト。
    let to_client = async {
        while let Some(item) = output.next().await {
            let Ok(chunk) = item else { break };
            if ws_tx
                .send(Message::Binary(chunk.into_bytes()))
                .await
                .is_err()
            {
                break; // client 切断
            }
        }
        let _ = ws_tx.send(Message::Close(None)).await; // output EOF = シェル終了
    };

    // 方向B:クライアント → コンテナ。Binary=stdin、Text=制御(resize)。この async を抜けると
    //    input が drop → stdin EOF → sh 終了 → daemon が exec を回収する。`delete_exec` は無いので
    //    **input の drop が唯一の後始末** = この future が確実に drop されないとゾンビ exec が残る。
    let to_container = async {
        while let Some(Ok(msg)) = ws_rx.next().await {
            match msg {
                Message::Binary(b) => {
                    let ok = input.write_all(&b).await.is_ok() && input.flush().await.is_ok();
                    if !ok {
                        break;
                    }
                }
                Message::Text(t) => {
                    if let Some((cols, rows)) = parse_resize(t.as_str()) {
                        // best-effort:resize 失敗で切断はしない。
                        let _ = state
                            .docker
                            .resize_exec(
                                &created.id,
                                ResizeExecOptions {
                                    width: cols,
                                    height: rows,
                                },
                            )
                            .await;
                    }
                }
                Message::Close(_) => break,
                _ => {} // Ping は axum が自動 Pong。Pong 等は無視。
            }
        }
    };

    // 4) どちらか一方が終われば全体を畳む(select! 終了で両 future が drop = input drop)。
    //    最大セッション timeout で半開き接続の sh 生存を防ぐ。
    let session = async {
        tokio::select! {
            _ = to_client => {}
            _ = to_container => {}
        }
    };
    let _ = tokio::time::timeout(TERMINAL_MAX_SESSION, session).await;
}

/// 升级後の失敗を、開いた socket に人間可読バイト列(xterm がそのまま表示)+ Close で伝える。
/// Text ではなく Binary:前端は inbound を端末へ食わせるだけで Text 制御の受信を持たないため。
async fn terminal_fail(mut socket: WebSocket, note: &str) {
    let body = format!("\r\n[tsubomi] {note}\r\n");
    let _ = socket.send(Message::Binary(body.into_bytes().into())).await;
    let _ = socket.send(Message::Close(None)).await;
}

/// `{"type":"resize","cols":N,"rows":M}` を `(cols, rows)` に。型不一致 / 別種は None。
/// 暴走値は上限でクランプ(daemon に変な PTY サイズを渡さない)。
fn parse_resize(json: &str) -> Option<(u16, u16)> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    if v.get("type")?.as_str()? != "resize" {
        return None;
    }
    let cols = v.get("cols")?.as_u64()?.clamp(1, 1000) as u16;
    let rows = v.get("rows")?.as_u64()?.clamp(1, 1000) as u16;
    Some((cols, rows))
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
    let (cpu_pct, mem_bytes) = sample_stats(state, &name).await?;
    Some(ServiceStat {
        cpu_pct,
        mem_bytes: mem_bytes as i64,
    })
}

/// 1 コンテナ(名前 or id)を 1 サンプル stats して `(cpu_pct, mem_bytes)` を返す。
/// `stream=false` で daemon の 2 サンプルから precpu を埋めるので CPU% が出る(~1 秒)。
/// service stats と platform stats が共有する。取得失敗は None(best-effort)。
async fn sample_stats(state: &AppState, name_or_id: &str) -> Option<(Option<f64>, u64)> {
    let opts = StatsOptionsBuilder::default().stream(false).build();
    let sample = state.docker.stats(name_or_id, Some(opts)).next().await?.ok()?;
    let mem_bytes = sample
        .memory_stats
        .as_ref()
        .and_then(|m| m.usage)
        .unwrap_or(0);
    Some((compute_cpu_pct(&sample), mem_bytes))
}

/// 平台自身の 1 コンテナの監視指標(リソース概要「プラットフォーム自身」。各コンテナ別に出す)。
#[derive(Clone, serde::Serialize)]
pub struct ContainerStat {
    /// 表示名(先頭の `tsubomi-` を剥がした短名。例 server / pg-platform / valkey)。
    pub name: String,
    /// CPU 使用率(%)。算出不能は None。
    pub cpu_pct: Option<f64>,
    /// 内存使用量(bytes)。
    pub mem_bytes: u64,
}

/// 平台自身(server + infra)の各コンテナの 1 サンプル stats を返す(リソース概要の
/// 「プラットフォーム自身」)。対象 = 名前が `tsubomi-` で始まり、かつ用户 app の
/// `tsubomi.managed` ラベルを**持たない** running コンテナ(= infra + server。用户 service
/// コンテナは managed ラベルで除外)。各コンテナを並行に 1 サンプルする。best-effort:
/// 列挙 / 各 stats の失敗は黙って飛ばす。閲覧者がいる時だけ 5s 毎に呼ばれる(metrics サンプラ)。
pub async fn platform_stats(state: &AppState) -> Vec<ContainerStat> {
    let opts = ListContainersOptionsBuilder::default().all(false).build(); // running のみ
    let Ok(list) = state.docker.list_containers(Some(opts)).await else {
        return Vec::new();
    };
    let targets: Vec<(String, String)> = list
        .into_iter()
        .filter_map(|c| {
            let id = c.id?;
            let raw = c.names.as_ref()?.first()?.trim_start_matches('/').to_string();
            // 平台容器だけ:tsubomi- 名前 かつ managed ラベル無し(用户 app を除外)。
            let managed = c
                .labels
                .as_ref()
                .is_some_and(|l| l.contains_key(LABEL_MANAGED));
            (raw.starts_with("tsubomi-") && !managed).then_some((id, raw))
        })
        .collect();

    let futs = targets.into_iter().map(|(id, raw)| async move {
        let (cpu_pct, mem_bytes) = sample_stats(state, &id).await?;
        let name = raw.strip_prefix("tsubomi-").unwrap_or(&raw).to_string();
        Some(ContainerStat {
            name,
            cpu_pct,
            mem_bytes,
        })
    });
    let mut stats: Vec<ContainerStat> = futures_util::future::join_all(futs)
        .await
        .into_iter()
        .flatten()
        .collect();
    // server を先頭、残りは名前順(安定表示)。
    stats.sort_by_key(|c| (c.name != "server", c.name.clone()));
    stats
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
