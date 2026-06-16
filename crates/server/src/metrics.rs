//! ホスト(サーバ本体 = 香橙派)の CPU / メモリ / ディスク使用量を WebSocket で配信する。
//! リソース概要(admin overview)に「サーバー」区块として出す。
//!
//! 設計の要(ユーザ制約):**① コードに性能影響を与えない ② 頻度は高くなくてよい
//! ③ 誰も見ていない時は監視器を起動しない ④ WebSocket**。→ `tokio::sync::broadcast` の
//! **共有サンプラ**で実現する:最初の閲覧者が WS で繋いだ時だけ採样 task を起こし、
//! 5s 周期で 1 回採って全閲覧者へ扇出、最後の閲覧者が切れたら(送信先ゼロ)自動停止する。
//! → 閲覧者ゼロなら採样 task は存在しない(要件③)。
//!
//! 採取は新 crate を足さず(設計 §10-D「sysinfo を足さない」)、Linux なら /proc、
//! ディスクは `df`(macOS/Linux 両対応)。dev(macOS native)は /proc が無いので CPU/メモリは
//! None(UI は「—」)、ディスクのみ実値。prod(Linux コンテナ、host network)は /proc が host
//! 値を返すので全部実値。
//!
//! 鉴权:`/api/admin/metrics` は `require_auth` middleware の内側 = WS 升级(cookie 付き GET)も
//! AuthCtx を持つ。そこで `require_viewer_web`(owner または共有パスワード viewer・session のみ。
//! Bearer 不可)で守る — リソース概要自体が viewer 可なので一貫(host 指標は非機密の platform 値)。

use crate::admin::require_viewer_web;
use crate::auth::AuthCtx;
use crate::error::AppResult;
use crate::state::AppState;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use serde::Serialize;
use std::path::Path;
use std::time::Duration;
use tokio::sync::broadcast;

/// 採样間隔。頻度は高くなくてよい(要件②)— 5s。
const SAMPLE_INTERVAL: Duration = Duration::from_secs(5);
/// CPU% を初回フレームから出すための暖機間隔(差分窓)。CPU は 2 サンプルの差分なので、
/// これが無いと初回は prev 無しで None になり「CPU だけ数秒遅れて出る」。起動時に 1 度
/// /proc/stat を読み、この間隔を置いてから初回フレームを出す。短すぎると差分のノイズが
/// 大きいので ~1s(以降は 5s 窓で平準化される)。
const CPU_WARMUP: Duration = Duration::from_millis(1000);

/// ホスト指標のスナップショット。WS で JSON テキストとして送る。各値は best-effort:
/// 取得不能(dev の macOS で /proc 無し、df 失敗 等)は None で、前端は「—」を出す。
#[derive(Clone, Serialize)]
pub struct HostMetrics {
    /// CPU 使用率(%、0–100)。前回サンプルとの差分で算出。
    pub cpu_pct: Option<f64>,
    /// 使用中メモリ(bytes)。MemTotal − MemAvailable。
    pub mem_used: Option<u64>,
    /// 総メモリ(bytes)。
    pub mem_total: Option<u64>,
    /// 使用中ディスク(bytes)。
    pub disk_used: Option<u64>,
    /// 総ディスク(bytes)。
    pub disk_total: Option<u64>,
    /// ディスク使用率(%)。
    pub disk_pct: Option<u8>,
    /// 平台自身(server + infra)の**各コンテナ**の CPU/メモリ(加総せず個別に出す)。
    /// 用户 app は含めない(managed ラベルで除外)。dev は server が容器でないので出ない。
    pub platform: Vec<crate::services::docker::ContainerStat>,
}

/// `df -Pk` の 1 行を解析したディスク容量。`gc` のディスク警告(使用率)とも共有する。
#[derive(Clone, Copy)]
pub struct DiskBytes {
    pub total: u64,
    pub used: u64,
    pub pct: u8,
}

/// `GET /api/admin/metrics`(WebSocket)。owner または viewer(web セッション)のみ。
/// 升级後は共有サンプラへ subscribe し、5s 毎のスナップショットを socket へ転送する。
pub async fn metrics_ws(
    auth: AuthCtx,
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
) -> AppResult<impl IntoResponse> {
    require_viewer_web(&auth)?;
    Ok(ws.on_upgrade(move |socket| handle_socket(socket, state)))
}

/// 1 接続の取り回し:subscribe → (必要なら)サンプラ起動 → recv ループで転送。
async fn handle_socket(mut socket: WebSocket, state: AppState) {
    // **先に subscribe**(受信者数 +1)してから running を判定・起動する。この 2 つを
    // metrics_running ロック内で行い、サンプラ側の「送信 + running 反転」と直列化する
    // ことで、最後の閲覧者が切れた瞬間の取りこぼし(起こしたのに誰も採らない)や
    // 二重起動を防ぐ。詳細は spawn_sampler のコメント。
    let mut rx = state.metrics_tx.subscribe();
    {
        let mut running = state.metrics_running.lock().await;
        if !*running {
            *running = true;
            spawn_sampler(state.clone());
        }
    }

    loop {
        tokio::select! {
            recv = rx.recv() => match recv {
                Ok(snap) => {
                    let Ok(text) = serde_json::to_string(&snap) else { continue };
                    if socket.send(Message::Text(text.into())).await.is_err() {
                        break; // client 切断
                    }
                }
                // 取りこぼし(遅い client)は最新へ追従するだけ。
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break, // 通常起きない
            },
            // client からの close / エラーを即検知して抜ける(一方向ストリームなので
            // 受け取ったフレーム自体は無視。axum が Ping に自動 Pong する)。
            msg = socket.recv() => match msg {
                None | Some(Err(_)) => break,
                Some(Ok(_)) => continue,
            },
        }
    }
    // ここを抜ける = rx が drop され受信者数 −1。最後の閲覧者ならサンプラが
    // 次 tick の送信で受信者ゼロを検知し、自動停止する(要件③)。
}

/// 共有サンプラを起こす。5s 毎に 1 回採取して broadcast へ送る。送信先(WS 受信者)が
/// ゼロになったら自動停止する — `Sender::send` は受信者ゼロのとき Err を返すので、それを
/// 停止の合図に使う。**送信と running 反転を metrics_running ロック内で**行い、handle_socket の
/// 「subscribe + 起動判定」と直列化する(これで最後の 1 人が切れた瞬間に新規接続が来ても、
/// 古いサンプラが残るか・新サンプラが起きるかのどちらかになり、無人 or 二重を防ぐ)。
fn spawn_sampler(state: AppState) {
    tokio::spawn(async move {
        tracing::debug!("host metrics サンプラ起動(閲覧者あり)");
        // CPU% は 2 サンプルの差分。**ループ前に 1 度読んで短い暖機間隔を置く**ことで、
        // 初回フレームから CPU% を出す(これが無いと初回は prev 無し=None で「CPU だけ
        // 数秒遅れて出る」。mem/disk は瞬時値なので元々すぐ出る)。dev(/proc 無し)は
        // read が None なので CPU は「—」のまま(挙動は変わらない)。
        let mut prev_cpu = read_cpu_times().await;
        tokio::time::sleep(CPU_WARMUP).await;
        let mut tick = tokio::time::interval(SAMPLE_INTERVAL);
        // 1 回の採取(docker stats ~1-2s)が間隔を超えても**バースト追い上げしない** —
        // 取りこぼした tick は捨てて次の周期へ。負荷を一定に保つ(性能影響を出さない要件)。
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await; // 0th tick は即発火 → 初回フレーム(暖機済みで CPU も値あり)
            // 採取前に受信者ゼロを判定して停止する(要件③)。これが無いと、最後の閲覧者が
            // 切れても**次 tick で docker stats を 1 バッチ余計に走らせて**から send Err で
            // 気づく。先に弾けば「誰も見ていない時は重い採取を一切しない」が完全になる。
            // handle_socket の「subscribe → lock → !running で起動」と同ロックで直列化される
            // ので、判定と新規接続の race は無い(subscribe 済みなら count>0 を見る)。
            {
                let mut running = state.metrics_running.lock().await;
                if state.metrics_tx.receiver_count() == 0 {
                    *running = false;
                    tracing::debug!("host metrics サンプラ停止(閲覧者ゼロ)");
                    break;
                }
            }
            // 採取はロックの外(数 ms の I/O + docker stats。ロックは送信判定だけ短く保つ)。
            let cur_cpu = read_cpu_times().await;
            let cpu_pct = prev_cpu.zip(cur_cpu).and_then(|(p, c)| cpu_delta_pct(p, c));
            prev_cpu = cur_cpu;
            let mem = read_mem().await;
            let disk = disk_metrics(&state.config.volumes_dir).await;
            // 平台自身の各コンテナ(server + infra)。docker stats を並行に取る(~1-2s)。
            // 採取はロックの外なので host 指標の鮮度を妨げない。
            let platform = crate::services::docker::platform_stats(&state).await;
            let snap = HostMetrics {
                cpu_pct,
                mem_used: mem.map(|(used, _)| used),
                mem_total: mem.map(|(_, total)| total),
                disk_used: disk.map(|d| d.used),
                disk_total: disk.map(|d| d.total),
                disk_pct: disk.map(|d| d.pct),
                platform,
            };

            let mut running = state.metrics_running.lock().await;
            if state.metrics_tx.send(snap).is_err() {
                // backstop:採取中(~1-2s)に最後の 1 人が切れた場合はここで気づく。
                *running = false;
                tracing::debug!("host metrics サンプラ停止(閲覧者ゼロ)");
                break;
            }
        }
    });
}

/// `df -Pk <path>` で path を含む filesystem の容量(bytes)+ 使用率を取る。POSIX `-P` で
/// 固定 6 列(Filesystem 1024-blocks Used Available Capacity Mounted-on)・`-k` で 1024 ブロック。
/// macOS/Linux 両対応。解析失敗は None(best-effort)。`gc` のディスク警告とも共有する。
pub async fn disk_metrics(path: &Path) -> Option<DiskBytes> {
    let out = tokio::process::Command::new("df")
        .arg("-Pk")
        .arg(path)
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut fields = text.lines().nth(1)?.split_whitespace(); // ヘッダの次の行
    let total_k: u64 = fields.nth(1)?.parse().ok()?; // 列 1:1024-blocks
    let used_k: u64 = fields.next()?.parse().ok()?; // 列 2:Used
    let pct: u8 = fields.nth(1)?.trim_end_matches('%').parse().ok()?; // 列 4:Capacity
    Some(DiskBytes {
        total: total_k * 1024,
        used: used_k * 1024,
        pct,
    })
}

/// `/proc/meminfo` から (使用中, 総量) を bytes で。MemTotal − MemAvailable = 使用中。
/// macOS には /proc が無いので None(dev)。
async fn read_mem() -> Option<(u64, u64)> {
    let text = tokio::fs::read_to_string("/proc/meminfo").await.ok()?;
    let mut total = None;
    let mut avail = None;
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("MemTotal:") {
            total = parse_meminfo_kb(v);
        } else if let Some(v) = line.strip_prefix("MemAvailable:") {
            avail = parse_meminfo_kb(v);
        }
    }
    let total = total?;
    Some((total.saturating_sub(avail?), total))
}

/// `/proc/meminfo` の値("  16384000 kB")を bytes に。
fn parse_meminfo_kb(s: &str) -> Option<u64> {
    s.split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()
        .map(|kb| kb * 1024)
}

/// `/proc/stat` の集計 cpu 行から (total, idle) jiffies。差分で CPU% を出す元データ。
#[derive(Clone, Copy)]
struct CpuTimes {
    total: u64,
    idle: u64,
}

/// `/proc/stat` の先頭 "cpu …" 行を読む。macOS には無いので None(dev)。
async fn read_cpu_times() -> Option<CpuTimes> {
    let text = tokio::fs::read_to_string("/proc/stat").await.ok()?;
    let mut it = text.lines().next()?.split_whitespace();
    if it.next()? != "cpu" {
        return None;
    }
    // user nice system idle iowait irq softirq steal …(idle は 4 列目 + iowait 5 列目)。
    let vals: Vec<u64> = it.filter_map(|x| x.parse().ok()).collect();
    if vals.len() < 4 {
        return None;
    }
    let idle = vals[3] + vals.get(4).copied().unwrap_or(0);
    Some(CpuTimes {
        total: vals.iter().sum(),
        idle,
    })
}

/// 2 サンプルの差分から CPU 使用率(%)。間隔ゼロ / カウンタ巻き戻りは None。
fn cpu_delta_pct(prev: CpuTimes, cur: CpuTimes) -> Option<f64> {
    let dt = cur.total.checked_sub(prev.total)?;
    let di = cur.idle.checked_sub(prev.idle)?;
    if dt == 0 {
        return None;
    }
    Some((dt - di) as f64 / dt as f64 * 100.0)
}
