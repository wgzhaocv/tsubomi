//! service アクセス統計(doc/paas-service-stats-design.md)。
//!
//! 背骨:traefik が access log を JSON で stdout に吐く(輪転は compose の json-file が持つ)→
//! ここが bollard logs(follow + timestamps)で追尾 → `request_events` へ batch INSERT →
//! 集計はクエリ時(事前集計しない — 社内規模では GROUP BY で足りる §0-C)。
//! リクエスト経路には何も足さない(access log は応答後の非同期書き)= app の遅延影響ゼロ。
//!
//! - IP は保存しない:visitor_hash = sha256(UTC日付 || ip || ua) 先頭 16 バイト(§0-D)。
//! - 実 IP の取り方は **設定分岐のみ**(`StatsIpSource`)。「ヘッダがあれば使う」自動判定は
//!   偽装口になるのでしない(§2.1)。
//! - offset(docker 行タイムスタンプ)は platform_config に永続化し、INSERT 成功後に前進する
//!   (= クラッシュ時は欠落より重複に倒す。境界重複は行タイムスタンプ比較でほぼ消える §1-9)。
//! - 保留(既定 30 日)の DELETE は gc の housekeeping が呼ぶ([`sweep`])。
//!   service の物理 purge は FK ON DELETE CASCADE で自動連鎖(掃除コード不要)。

use crate::config::StatsIpSource;
use crate::error::AppResult;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, Query, State};
use bollard::query_parameters::LogsOptionsBuilder;
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};
use tsubomi_shared::{ServiceStatsDto, StatsPointDto, StatsSliceDto, StatsTotalsDto};
use uuid::Uuid;

/// offset を持つ platform_config のキー(値 = `{"since": "<RFC3339>"}`)。
const SINCE_KEY: &str = "stats_tail_since";
/// batch INSERT の間隔と最大行数。どちらか先に達した方で flush する。
const FLUSH_INTERVAL: Duration = Duration::from_secs(5);
const FLUSH_MAX_ROWS: usize = 500;
/// path の保存上限(バイト)。攻撃スキャン等の異常長 URL で行を肥大させない。
const PATH_MAX_BYTES: usize = 512;

pub fn spawn(state: AppState) {
    tokio::spawn(async move { run(state).await });
}

/// 追尾の外側ループ:切断・容器不在は backoff 付きで永久に再接続する(dev で traefik が
/// 居なくても server は動く)。十分長く生きた接続の後は backoff をリセット。
async fn run(state: AppState) {
    let mut backoff = 2u64;
    loop {
        let started = Instant::now();
        match tail_once(&state).await {
            Ok(()) => tracing::info!("stats: traefik ログ流が終了(再接続します)"),
            Err(e) => tracing::warn!(error = %e, "stats: traefik ログ追尾が失敗(再接続します)"),
        }
        if started.elapsed() > Duration::from_secs(60) {
            backoff = 2;
        }
        tokio::time::sleep(Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(60);
    }
}

/// 1 本の logs 接続を張り、流が切れるまで処理する。
async fn tail_once(state: &AppState) -> anyhow::Result<()> {
    let since = load_since(state).await?;
    let opts = LogsOptionsBuilder::default()
        .stdout(true)
        // traefik のアプリログが stderr 側に出る構成でも巻き込まない(選別は JSON 欄でも
        // 二重に掛かるが、読む量自体を減らす)。
        .stderr(false)
        .follow(true)
        // docker が行頭に付ける RFC3339Nano を offset に使う(StartUTC はイベント時刻で、
        // 書き込み順と単調でないため offset には不適)。
        .timestamps(true)
        // bollard の since は i32 秒。秒粒度の取りこぼし側に倒し(floor)、重複は下の
        // 行タイムスタンプ比較で消す。since 未保存(初回)は「今」から = 過去は遡らない(§1-8)。
        .since(since.unwrap_or_else(Utc::now).timestamp().clamp(0, i32::MAX as i64) as i32)
        .build();

    let mut stream = state.docker.logs(&state.config.traefik_container, Some(opts));
    let parser = woothee::parser::Parser::new();
    let mut buf: Vec<Event> = Vec::new();
    // 最後に処理した docker 行タイムスタンプ。flush 成功時にこの値を永続化する。
    let mut last_ts: Option<DateTime<Utc>> = since;
    // 16KiB 超の 1 行は docker が複数エントリに割る(partial message)。完結行(\n 終端)まで
    // 継ぎ足してから parse する — 先頭片の ts を行の ts とする(codex 審査 2026-08-20)。
    let mut pending: Option<(DateTime<Utc>, String)> = None;
    // 再接続の境界探索中だけ ts < saved を捨てる。docker の行 ts は厳密単調でない(Moby 自身が
    // そう扱う)ので、境界を跨いだら比較をやめる = 同時刻の未処理行を「処理済み」と誤断しない。
    // saved と同時刻の処理済み行は再処理される(欠落より重複 §1-9)。
    let mut boundary_passed = false;
    let mut cf_header_warned = false;
    let mut malformed_warned = false;
    let mut tick = tokio::time::interval(FLUSH_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            item = stream.next() => match item {
                Some(Ok(frame)) => {
                    let raw = frame.into_bytes();
                    let chunk = String::from_utf8_lossy(&raw);
                    // "2026-08-19T12:34:56.123456789Z {json…}" — 行頭は docker のタイムスタンプ
                    // (partial の各片にも付く)。
                    let Some((ts_str, payload)) = chunk.split_once(' ') else { continue };
                    let Ok(piece_ts) = DateTime::parse_from_rfc3339(ts_str) else { continue };
                    let piece_ts = piece_ts.with_timezone(&Utc);
                    if !boundary_passed {
                        if since.is_some_and(|s| piece_ts < s) {
                            continue;
                        }
                        boundary_passed = true;
                    }
                    // 完結行(\n 終端)まで継ぎ足す。異常長は捨てる(攻撃的な巨大行で無界に
                    // 溜めない — 上限は PATH や UA の正当ケースより十分大きい 256 KiB)。
                    let complete = payload.ends_with('\n');
                    let (line_ts, line) = match pending.take() {
                        Some((first_ts, mut acc)) => {
                            acc.push_str(payload);
                            if !complete {
                                if acc.len() <= 256 * 1024 {
                                    pending = Some((first_ts, acc));
                                }
                                continue;
                            }
                            (first_ts, std::borrow::Cow::Owned(acc))
                        }
                        None if !complete => {
                            pending = Some((piece_ts, payload.to_string()));
                            continue;
                        }
                        None => (piece_ts, std::borrow::Cow::Borrowed(payload)),
                    };
                    match parse_event(line.trim(), state.config.stats_ip_source, &parser) {
                        ParseOutcome::Event(ev) => buf.push(ev),
                        ParseOutcome::MissingCfHeader(ev) => {
                            // cf モードなのに Cf-Connecting-Ip が無い = CF を外した部署で設定を
                            // 変え忘れた可能性(dev のローカル直叩きでも出る)。一度だけ報せる。
                            if !cf_header_warned {
                                cf_header_warned = true;
                                tracing::warn!(
                                    "stats: TSUBOMI_STATS_IP_SOURCE=cf ですが Cf-Connecting-Ip ヘッダが\
                                     ありません(peer アドレスへ回退)。CF 配下でない部署は peer に設定"
                                );
                            }
                            buf.push(ev);
                        }
                        ParseOutcome::Skip => {}
                        ParseOutcome::Malformed => {
                            // follow 接続は健康なら何日でも生きる = 終端時レポートでは形式変化に
                            // 気付けない。接続毎に一度だけ、その場で報せる(数える機構は持たない)。
                            if !malformed_warned {
                                malformed_warned = true;
                                tracing::warn!(
                                    "stats: parse できない access log 行(traefik の形式変化の可能性)"
                                );
                            }
                        }
                    }
                    last_ts = Some(line_ts);
                    if buf.len() >= FLUSH_MAX_ROWS && !flush(state, &mut buf, last_ts).await {
                        // 満杯でも書けない = DB 長期障害。溜め続けると無界(OOM)なので接続を
                        // 畳んで backoff — offset は進んでいないので復旧後に docker ログから
                        // 再読する(欠落より重複。codex 審査 2026-08-20)。
                        anyhow::bail!("request_events への書き込みが満杯フラッシュでも失敗");
                    }
                }
                Some(Err(e)) => {
                    flush(state, &mut buf, last_ts).await;
                    return Err(e.into());
                }
                None => {
                    flush(state, &mut buf, last_ts).await;
                    return Ok(());
                }
            },
            _ = tick.tick() => {
                if !buf.is_empty() {
                    flush(state, &mut buf, last_ts).await;
                }
            }
        }
    }
}

/// 1 リクエスト分の保存行(request_events の 1 行)。
struct Event {
    service_id: Uuid,
    ts: DateTime<Utc>,
    method: String,
    path: String,
    status: i16,
    duration_ms: i32,
    visitor_hash: [u8; 16],
    device: String,
    browser: Option<String>,
    os: Option<String>,
    country: Option<String>,
    referer_host: Option<String>,
}

enum ParseOutcome {
    Event(Event),
    /// cf モードでヘッダ欠落(peer 回退で作った Event 付き — 警告は呼び出し側で一度だけ)。
    MissingCfHeader(Event),
    /// 正常な非対象行(アプリログ / 非 svc router)。
    Skip,
    /// JSON だが期待欄が壊れている等。多発したら形式変化を疑う。
    Malformed,
}

/// traefik access log(JSON)の使う欄だけの写像。compose の keep リストと対
/// (defaultmode=drop なのでこれ以外は来ない)。
#[derive(Deserialize)]
struct AccessLine {
    #[serde(rename = "RouterName")]
    router_name: Option<String>,
    #[serde(rename = "ClientAddr")]
    client_addr: Option<String>,
    #[serde(rename = "RequestPath")]
    request_path: Option<String>,
    #[serde(rename = "RequestMethod")]
    request_method: Option<String>,
    #[serde(rename = "DownstreamStatus")]
    downstream_status: Option<i64>,
    /// ナノ秒(traefik の JSON access log の単位)。
    #[serde(rename = "Duration")]
    duration_ns: Option<i64>,
    #[serde(rename = "StartUTC")]
    start_utc: Option<String>,
    #[serde(rename = "request_User-Agent")]
    user_agent: Option<String>,
    #[serde(rename = "request_Referer")]
    referer: Option<String>,
    #[serde(rename = "request_Cf-Connecting-Ip")]
    cf_connecting_ip: Option<String>,
    #[serde(rename = "request_Cf-Ipcountry")]
    cf_ipcountry: Option<String>,
}

/// 1 行 → Event。純関数(I/O なし)= テスト対象の本体。
fn parse_event(payload: &str, ip_source: StatsIpSource, parser: &woothee::parser::Parser) -> ParseOutcome {
    // stdout には traefik のアプリログも混ざり得る:JSON でない行は対象外(Skip)。
    let Ok(line) = serde_json::from_str::<AccessLine>(payload) else {
        return ParseOutcome::Skip;
    };
    // access log 行の指紋:StartUTC + DownstreamStatus(アプリログの JSON 形式化にも耐える)。
    let (Some(start_utc), Some(status)) = (line.start_utc.as_deref(), line.downstream_status) else {
        return ParseOutcome::Skip;
    };
    let Some(service_id) = line
        .router_name
        .as_deref()
        .and_then(crate::services::route::parse_router_name)
    else {
        return ParseOutcome::Skip; // apex / catch-all / registry / 内部 router は対象外。
    };
    let Ok(ts) = DateTime::parse_from_rfc3339(start_utc) else {
        return ParseOutcome::Malformed;
    };
    let ts = ts.with_timezone(&Utc);
    if !(100..=599).contains(&status) {
        return ParseOutcome::Malformed;
    }

    let peer_host = line.client_addr.as_deref().map(host_of_addr).unwrap_or_default();
    // §2.1:分岐は設定のみ。peer モードはヘッダ(国も含む)を一切見ない。
    let (ip, country, cf_missing) = match ip_source {
        StatsIpSource::Cf => match line.cf_connecting_ip.as_deref() {
            Some(h) => (h.trim().to_string(), line.cf_ipcountry.clone(), false),
            None => (peer_host.to_string(), None, true),
        },
        StatsIpSource::Peer => (peer_host.to_string(), None, false),
    };

    let ua = line.user_agent.as_deref().unwrap_or("");
    let (device, browser, os) = classify_ua(parser, ua);
    let ev = Event {
        service_id,
        ts,
        method: line.request_method.unwrap_or_else(|| "-".to_string()),
        path: normalize_path(line.request_path.as_deref().unwrap_or("/")),
        status: status as i16,
        // ns → ms(§6-3 の地雷)。異常値は飽和。
        duration_ms: (line.duration_ns.unwrap_or(0) / 1_000_000).clamp(0, i32::MAX as i64) as i32,
        visitor_hash: visitor_hash(ts, &ip, ua),
        device,
        browser,
        os,
        country: country.map(|c| c.trim().to_uppercase()).filter(|c| !c.is_empty()),
        referer_host: line.referer.as_deref().and_then(referer_host),
    };
    if cf_missing {
        ParseOutcome::MissingCfHeader(ev)
    } else {
        ParseOutcome::Event(ev)
    }
}

/// `host:port` / `[v6]:port` → host。peer アドレス(ClientAddr)用。
fn host_of_addr(addr: &str) -> &str {
    let addr = addr.trim();
    if let Some(rest) = addr.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(addr);
    }
    // IPv4 / ホスト名:最後の ':' より前(v6 裸表記はここに来ない — docker は必ず括弧を付ける)。
    addr.rsplit_once(':').map(|(h, _)| h).unwrap_or(addr)
}

/// クエリ文字列を落とし(トークン等の混入防止)、UTF-8 境界を守って 512 バイトに切る。
fn normalize_path(path: &str) -> String {
    let p = path.split(['?', '#']).next().unwrap_or(path);
    if p.len() <= PATH_MAX_BYTES {
        return p.to_string();
    }
    let mut end = PATH_MAX_BYTES;
    while end > 0 && !p.is_char_boundary(end) {
        end -= 1;
    }
    p[..end].to_string()
}

/// 匿名 visitor id(§0-D):UTC **日付** + ip + ua の hash 先頭 16 バイト。日付で自然に
/// ローテーションする(Vercel と同じ口径)。日付は必ず UTC(§6-5 — TZ 依存にしない)。
fn visitor_hash(ts: DateTime<Utc>, ip: &str, ua: &str) -> [u8; 16] {
    let mut h = Sha256::new();
    // 日付は数値(1970 起点の日数)で混ぜる — 文字列化の割り当てを毎行払わない。
    h.update((ts.timestamp().div_euclid(86_400)).to_le_bytes());
    h.update([0]);
    h.update(ip);
    h.update([0]);
    h.update(ua);
    h.finalize()[..16].try_into().expect("sha256 は 16 バイト以上")
}

/// UA → (device, browser, os)。woothee の category を 4 値に畳む。
fn classify_ua(parser: &woothee::parser::Parser, ua: &str) -> (String, Option<String>, Option<String>) {
    let Some(r) = parser.parse(ua) else {
        // 空 UA / 未知:bot 断定はしない(訪客集計から誤って落とすよりは other)。
        return ("other".to_string(), None, None);
    };
    let device = match r.category {
        "pc" => "desktop",
        "smartphone" | "mobilephone" => "mobile",
        "crawler" => "bot",
        _ => "other",
    };
    let clean = |s: &str| {
        let s = s.trim();
        (!s.is_empty() && s != "UNKNOWN").then(|| s.to_string())
    };
    (device.to_string(), clean(r.name), clean(r.os))
}

/// Referer のホスト部だけ(フル URL は保存しない)。
fn referer_host(referer: &str) -> Option<String> {
    let u = url::Url::parse(referer.trim()).ok()?;
    u.host_str().map(|h| h.to_string())
}

/// batch INSERT + offset 前進。**INSERT 成功後にのみ** since を進める(欠落より重複)。
/// INSERT..SELECT + WHERE EXISTS で、purge 済み service や陳腐 route の行を静かに落とす
/// (FK CASCADE との在途競合をエラーにしない)。失敗は warn して buf を持ち越す
/// (次の flush で再試行。それでも失敗し続けたら行は流の切断まで溜まる — 上限は
/// FLUSH_MAX_ROWS 到達毎の再試行なので無界には伸びない)。
async fn flush(state: &AppState, buf: &mut Vec<Event>, last_ts: Option<DateTime<Utc>>) -> bool {
    if !buf.is_empty() {
        let n = buf.len();
        // 列は全部**借用**で組む(clone しない)— 成功時はどうせ捨てる buf を、失敗時は
        // そのまま持ち越して次の flush で再試行する(所有権は buf に残したまま)。
        let service_ids: Vec<Uuid> = buf.iter().map(|e| e.service_id).collect();
        let tss: Vec<DateTime<Utc>> = buf.iter().map(|e| e.ts).collect();
        let methods: Vec<&str> = buf.iter().map(|e| e.method.as_str()).collect();
        let paths: Vec<&str> = buf.iter().map(|e| e.path.as_str()).collect();
        let statuses: Vec<i16> = buf.iter().map(|e| e.status).collect();
        let durations: Vec<i32> = buf.iter().map(|e| e.duration_ms).collect();
        let hashes: Vec<&[u8]> = buf.iter().map(|e| e.visitor_hash.as_slice()).collect();
        let devices: Vec<&str> = buf.iter().map(|e| e.device.as_str()).collect();
        let browsers: Vec<Option<&str>> = buf.iter().map(|e| e.browser.as_deref()).collect();
        let oses: Vec<Option<&str>> = buf.iter().map(|e| e.os.as_deref()).collect();
        let countries: Vec<Option<&str>> = buf.iter().map(|e| e.country.as_deref()).collect();
        let referers: Vec<Option<&str>> = buf.iter().map(|e| e.referer_host.as_deref()).collect();
        let res = sqlx::query(
            "INSERT INTO request_events
               (service_id, ts, method, path, status, duration_ms, visitor_hash,
                device, browser, os, country, referer_host)
             SELECT * FROM UNNEST($1::uuid[], $2::timestamptz[], $3::text[], $4::text[],
                                  $5::smallint[], $6::int[], $7::bytea[], $8::text[],
                                  $9::text[], $10::text[], $11::text[], $12::text[])
               AS t(service_id, ts, method, path, status, duration_ms, visitor_hash,
                    device, browser, os, country, referer_host)
             WHERE EXISTS (SELECT 1 FROM resources r WHERE r.id = t.service_id)",
        )
        .bind(&service_ids)
        .bind(&tss)
        .bind(&methods)
        .bind(&paths)
        .bind(&statuses)
        .bind(&durations)
        .bind(&hashes)
        .bind(&devices)
        .bind(&browsers)
        .bind(&oses)
        .bind(&countries)
        .bind(&referers)
        .execute(&state.db)
        .await;
        match res {
            Ok(_) => buf.clear(),
            Err(e) => {
                tracing::warn!(error = %e, rows = n, "stats: request_events への書き込みに失敗(次回 flush で再試行)");
                return false; // offset を進めない(この buf の行を失わない)。
            }
        }
    }
    if let Some(ts) = last_ts {
        save_since(state, ts).await;
    }
    true
}

/// 保存済み offset を読む。**行が無いことだけ**が None(初回)— DB エラーや壊れた保存値を
/// 「cursor なし = 今から」と混同すると、その窓の行を飛ばした上で次の flush が新 cursor を
/// 保存して欠落が永久化する(codex 審査 2026-08-20)。エラーは呼び出し側が backoff。
async fn load_since(state: &AppState) -> anyhow::Result<Option<DateTime<Utc>>> {
    let v: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT value FROM platform_config WHERE key = $1")
            .bind(SINCE_KEY)
            .fetch_optional(&state.db)
            .await?;
    let Some(v) = v else { return Ok(None) };
    let s = v
        .get("since")
        .and_then(|s| s.as_str())
        .ok_or_else(|| anyhow::anyhow!("stats_tail_since の保存値が不正: {v}"))?;
    Ok(Some(DateTime::parse_from_rfc3339(s)?.with_timezone(&Utc)))
}

async fn save_since(state: &AppState, ts: DateTime<Utc>) {
    let v = serde_json::json!({ "since": ts.to_rfc3339() });
    if let Err(e) = sqlx::query(
        "INSERT INTO platform_config (key, value, updated_at) VALUES ($1, $2, now())
         ON CONFLICT (key) DO UPDATE SET value = $2, updated_at = now()",
    )
    .bind(SINCE_KEY)
    .bind(&v)
    .execute(&state.db)
    .await
    {
        tracing::warn!(error = %e, "stats: offset の保存に失敗(次回 flush で再試行)");
    }
}

/// 保留期(既定 30 日)を過ぎた行の掃除。gc の housekeeping(1h tick)から呼ばれる。
pub async fn sweep(state: &AppState) {
    match sqlx::query("DELETE FROM request_events WHERE ts < now() - make_interval(days => $1)")
        .bind(state.config.stats_retention_days as i32)
        .execute(&state.db)
        .await
    {
        Ok(r) if r.rows_affected() > 0 => {
            tracing::info!(rows = r.rows_affected(), "stats: 保留期切れの request_events を掃除");
        }
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "stats: request_events の掃除に失敗"),
    }
}

// ===== API(GET /api/services/:id/stats)=====

#[derive(Deserialize)]
pub struct StatsQuery {
    /// 集計日数(1〜30、既定 7)。
    pub days: Option<u32>,
}

/// 期間内の統計を 1 応答で返す(時系列 + totals + 内訳 Top10 × 7)。秘密なし = 素の Json。
pub async fn stats(
    auth: crate::auth::AuthCtx,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<StatsQuery>,
) -> AppResult<Json<ServiceStatsDto>> {
    crate::services::ensure_owned(&state, auth.user_id, id).await?;
    // 上限は保留日数から導き(30 の再エンコードをしない)、超過要求は 400 でなく**実効値へ
    // 丸める** — web の期間ボタンは固定(24h/7日/30日)なので、保留を短くした部署で統計タブが
    // 開けなくなる 400 を作らない。応答の days/from/to が実際の窓を言う(codex 審査 2026-08-20)。
    let max_days = state.config.stats_retention_days.max(1);
    let days = q.days.unwrap_or(7).clamp(1, max_days);
    // 刻みは自動:短期間は hour(点が足りないと曲線にならない)、長期間は day(hour だと 720 点)。
    let interval = if days <= 2 { "hour" } else { "day" };
    // 窓は interval 境界に揃える(rolling window だと最古の部分バケットが UI の 0 埋め範囲から
    // こぼれ、totals と series が食い違う)。切り下げは UTC 固定 — SQL 側の date_trunc も
    // 'UTC' を明示する(session TimeZone に依存させない)。
    let now = Utc::now();
    let to = trunc_utc(now, interval);
    let from = trunc_utc(now - chrono::Duration::days(days as i64), interval);

    // 全集計を単一の REPEATABLE READ READ ONLY tx で読む — 語句ごとに snapshot が進むと
    // 5s flush の commit を跨いで「series の合計 ≠ totals」の自己矛盾応答になる。
    let mut tx = state.db.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *tx)
        .await?;

    let series: Vec<(DateTime<Utc>, i64, i64)> = sqlx::query_as(
        "SELECT date_trunc($3, ts, 'UTC') AS t,
                count(*) AS requests,
                count(DISTINCT visitor_hash) FILTER (WHERE device <> 'bot') AS visitors
           FROM request_events
          WHERE service_id = $1 AND ts >= $2
          GROUP BY 1 ORDER BY 1",
    )
    .bind(id)
    .bind(from)
    .bind(interval)
    .fetch_all(&mut *tx)
    .await?;

    let totals: (i64, i64, i64, Option<f64>) = sqlx::query_as(
        "SELECT count(*),
                count(DISTINCT visitor_hash) FILTER (WHERE device <> 'bot'),
                count(*) FILTER (WHERE device = 'bot'),
                avg(duration_ms)::float8
           FROM request_events
          WHERE service_id = $1 AND ts >= $2",
    )
    .bind(id)
    .bind(from)
    .fetch_one(&mut *tx)
    .await?;

    // 内訳の SQL は各々静的リテラル(sqlx の SqlSafeStr 制約 = 動的組み立て禁止に従う)。
    // NULL の扱いは列毎:browser/os は unknown に畳む(欠落も情報)、country/referer は
    // 行ごと除外(無い方が普通)。逐次で撃つ(並列 join は機構代の割に、索引済みの
    // ミリ秒クエリ 7 本では体感差が無い — simplify 審査 2026-08-20)。
    macro_rules! slice_sql {
        ($key_expr:literal) => {
            concat!(
                "SELECT ", $key_expr, " AS key, count(*) AS requests
                   FROM request_events
                  WHERE service_id = $1 AND ts >= $2
                  GROUP BY 1 ORDER BY 2 DESC, 1 LIMIT 10"
            )
        };
        ($key_expr:literal, not_null) => {
            concat!(
                "SELECT ", $key_expr, " AS key, count(*) AS requests
                   FROM request_events
                  WHERE service_id = $1 AND ts >= $2
                    AND ", $key_expr, " IS NOT NULL
                  GROUP BY 1 ORDER BY 2 DESC, 1 LIMIT 10"
            )
        };
    }
    async fn slice(
        tx: &mut sqlx::PgConnection,
        sql: &'static str,
        id: Uuid,
        from: DateTime<Utc>,
    ) -> Result<Vec<StatsSliceDto>, sqlx::Error> {
        let rows: Vec<(String, i64)> =
            sqlx::query_as(sql).bind(id).bind(from).fetch_all(tx).await?;
        Ok(rows
            .into_iter()
            .map(|(key, requests)| StatsSliceDto { key, requests })
            .collect())
    }
    let top_paths = slice(&mut tx, slice_sql!("path"), id, from).await?;
    let statuses = slice(&mut tx, slice_sql!("((status / 100)::text || 'xx')"), id, from).await?;
    let devices = slice(&mut tx, slice_sql!("device"), id, from).await?;
    let browsers = slice(&mut tx, slice_sql!("COALESCE(browser, 'unknown')"), id, from).await?;
    let oses = slice(&mut tx, slice_sql!("COALESCE(os, 'unknown')"), id, from).await?;
    let countries = slice(&mut tx, slice_sql!("country", not_null), id, from).await?;
    let referers = slice(&mut tx, slice_sql!("referer_host", not_null), id, from).await?;
    tx.commit().await?;

    Ok(Json(ServiceStatsDto {
        days,
        interval: interval.to_string(),
        from,
        to,
        series: series
            .into_iter()
            .map(|(t, requests, visitors)| StatsPointDto { t, requests, visitors })
            .collect(),
        totals: StatsTotalsDto {
            requests: totals.0,
            visitors: totals.1,
            bot_requests: totals.2,
            avg_duration_ms: totals.3,
        },
        top_paths,
        statuses,
        devices,
        browsers,
        oses,
        countries,
        referers,
    }))
}

/// UTC 固定の interval 切り下げ(Postgres の `date_trunc(interval, ts, 'UTC')` と同値。
/// hour/day は暦の複雑さが無いので epoch 秒の整除で足りる)。
fn trunc_utc(t: DateTime<Utc>, interval: &str) -> DateTime<Utc> {
    let secs: i64 = if interval == "hour" { 3600 } else { 86_400 };
    DateTime::from_timestamp(t.timestamp().div_euclid(secs) * secs, 0)
        .expect("epoch 整除は常に有限")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trunc_utc_matches_postgres_date_trunc() {
        let t: DateTime<Utc> = "2026-08-19T13:47:31.5Z".parse().unwrap();
        assert_eq!(trunc_utc(t, "hour").to_rfc3339(), "2026-08-19T13:00:00+00:00");
        assert_eq!(trunc_utc(t, "day").to_rfc3339(), "2026-08-19T00:00:00+00:00");
        // epoch 前でも div_euclid で正しく床方向(万一の時計異常でも panic しない)。
        let old: DateTime<Utc> = "1969-12-31T23:30:00Z".parse().unwrap();
        assert_eq!(trunc_utc(old, "day").to_rfc3339(), "1969-12-31T00:00:00+00:00");
    }

    #[test]
    fn host_of_addr_strips_port() {
        assert_eq!(host_of_addr("1.2.3.4:5678"), "1.2.3.4");
        assert_eq!(host_of_addr("[2001:db8::1]:443"), "2001:db8::1");
        assert_eq!(host_of_addr("1.2.3.4"), "1.2.3.4");
    }

    #[test]
    fn normalize_path_drops_query_and_truncates() {
        assert_eq!(normalize_path("/a/b?token=secret"), "/a/b");
        assert_eq!(normalize_path("/a#frag"), "/a");
        let long = format!("/{}", "x".repeat(600));
        assert_eq!(normalize_path(&long).len(), PATH_MAX_BYTES);
        // UTF-8 境界:マルチバイト途中で切らない(panic しない)。
        let jp = format!("/{}", "あ".repeat(300));
        let cut = normalize_path(&jp);
        assert!(cut.len() <= PATH_MAX_BYTES);
        assert!(jp.starts_with(&cut));
    }

    #[test]
    fn visitor_hash_rotates_daily() {
        let d1: DateTime<Utc> = "2026-08-19T23:59:59Z".parse().unwrap();
        let d2: DateTime<Utc> = "2026-08-20T00:00:01Z".parse().unwrap();
        let same_day: DateTime<Utc> = "2026-08-19T00:00:01Z".parse().unwrap();
        assert_eq!(visitor_hash(d1, "1.2.3.4", "ua"), visitor_hash(same_day, "1.2.3.4", "ua"));
        assert_ne!(visitor_hash(d1, "1.2.3.4", "ua"), visitor_hash(d2, "1.2.3.4", "ua"));
        assert_ne!(visitor_hash(d1, "1.2.3.4", "ua"), visitor_hash(d1, "5.6.7.8", "ua"));
        assert_eq!(visitor_hash(d1, "1.2.3.4", "ua").len(), 16);
    }

    fn sample_line(id: Uuid) -> String {
        format!(
            r#"{{"RouterName":"svc-{id}@file","ClientAddr":"10.0.0.9:1234",
              "RequestPath":"/hello?x=1","RequestMethod":"GET","DownstreamStatus":200,
              "Duration":2500000,"StartUTC":"2026-08-19T01:02:03.5Z",
              "request_User-Agent":"Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36",
              "request_Referer":"https://example.com/from?q=1",
              "request_Cf-Connecting-Ip":"203.0.113.7","request_Cf-Ipcountry":"JP"}}"#
        )
    }

    #[test]
    fn parse_event_cf_mode_uses_header_ip_and_country() {
        let parser = woothee::parser::Parser::new();
        let id = Uuid::new_v4();
        let ParseOutcome::Event(ev) = parse_event(&sample_line(id), StatsIpSource::Cf, &parser)
        else {
            panic!("Event になるはず");
        };
        assert_eq!(ev.service_id, id);
        assert_eq!(ev.path, "/hello");
        assert_eq!(ev.status, 200);
        assert_eq!(ev.duration_ms, 2); // 2_500_000 ns → 2 ms
        assert_eq!(ev.country.as_deref(), Some("JP"));
        assert_eq!(ev.device, "desktop");
        assert_eq!(ev.referer_host.as_deref(), Some("example.com"));
        // ヘッダ IP(203.0.113.7)で hash される = peer(10.0.0.9)の hash と異なる。
        assert_ne!(
            ev.visitor_hash,
            visitor_hash(ev.ts, "10.0.0.9", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36")
        );
    }

    #[test]
    fn parse_event_peer_mode_ignores_all_headers() {
        let parser = woothee::parser::Parser::new();
        let id = Uuid::new_v4();
        let ParseOutcome::Event(ev) = parse_event(&sample_line(id), StatsIpSource::Peer, &parser)
        else {
            panic!("Event になるはず");
        };
        // 偽装可能な Cf-* は国も含めて一切見ない(§2.1)。
        assert_eq!(ev.country, None);
        assert_eq!(
            ev.visitor_hash,
            visitor_hash(ev.ts, "10.0.0.9", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36")
        );
    }

    #[test]
    fn parse_event_cf_mode_without_header_falls_back_to_peer() {
        let parser = woothee::parser::Parser::new();
        let id = Uuid::new_v4();
        let line = format!(
            r#"{{"RouterName":"svc-{id}@file","ClientAddr":"192.168.1.2:9","RequestPath":"/",
              "RequestMethod":"GET","DownstreamStatus":301,"Duration":1000000,
              "StartUTC":"2026-08-19T01:02:03Z"}}"#
        );
        match parse_event(&line, StatsIpSource::Cf, &parser) {
            ParseOutcome::MissingCfHeader(ev) => {
                assert_eq!(ev.status, 301);
                assert_eq!(ev.device, "other"); // UA なし。
            }
            _ => panic!("MissingCfHeader になるはず"),
        }
    }

    #[test]
    fn parse_event_skips_non_target_lines() {
        let parser = woothee::parser::Parser::new();
        // traefik アプリログ(非 JSON)。
        assert!(matches!(
            parse_event("time=... level=info msg=hello", StatsIpSource::Cf, &parser),
            ParseOutcome::Skip
        ));
        // JSON だが access log でない。
        assert!(matches!(
            parse_event(r#"{"level":"info","msg":"x"}"#, StatsIpSource::Cf, &parser),
            ParseOutcome::Skip
        ));
        // access log だが svc router でない(catch-all)。
        assert!(matches!(
            parse_event(
                r#"{"RouterName":"catchall@file","DownstreamStatus":302,"StartUTC":"2026-08-19T00:00:00Z"}"#,
                StatsIpSource::Cf,
                &parser
            ),
            ParseOutcome::Skip
        ));
    }

    #[test]
    fn classify_ua_maps_categories() {
        let parser = woothee::parser::Parser::new();
        let (d, b, _) = classify_ua(&parser, "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1");
        assert_eq!(d, "mobile");
        assert!(b.is_some());
        let (d, _, _) = classify_ua(&parser, "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)");
        assert_eq!(d, "bot");
        let (d, b, o) = classify_ua(&parser, "");
        assert_eq!((d.as_str(), b, o), ("other", None, None));
    }
}
