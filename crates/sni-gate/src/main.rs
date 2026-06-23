//! tsubomi-sni-gate — 公網数据库(frp 中継)の VPS 辺縁 SNI 准入闸门。
//!
//! 背景:公網ポートに裸の Postgres(pgbouncer)を晒すと、扫描洪流が frps の
//! work-connection 池を**TLS / 認証より前に**食い潰し、隧道が全員に対して死ぬ
//! (DoS 型事故、`doc/incident-frp-pg-public-2026-06-22.md`)。
//!
//! この闸门を frps の**前**に置き、frp の資源を消費する前に SNI で准入する:
//!   client → [闸门 :443] → (localhost) frps → 隧道 → frpc → pgbouncer
//!
//! 効きどころ:
//!   - **TLS は終端しない**。平文の ClientHello から SNI を覗くだけ → 端到端
//!     `verify-full` も証明書も不変。
//!   - **前導を闸门が本地終結**(自分で `S` を返す)。SNI を検証して**初めて**後端へ
//!     繋ぎ、前導を replay する → 扫描器は後端(frp)接続を一切起こさない。
//!   - SNI ≠ 許可ドメイン / 非 Postgres 前導 / 前導タイムアウト = 即切断(fail-closed)。
//!   - **客户端側の拒否は既定で無言**(扫描洪流で journald を溢れさせない)。記録に値する
//!     のは後端=自インフラの不調だけ。理由を見たいときは `--log-rejects`。

mod sni;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::Parser;
use tokio::io::{AsyncReadExt, AsyncWriteExt, copy_bidirectional};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;

/// Postgres SSLRequest: length=8, code=80877103 (0x04d2162f)。
const SSL_REQUEST: [u8; 8] = [0x00, 0x00, 0x00, 0x08, 0x04, 0xd2, 0x16, 0x2f];
/// Postgres GSSENCRequest: length=8, code=80877104 (0x04d21630)。
/// Debian/Ubuntu の libpq は GSSAPI 込みで既定 `gssencmode=prefer` = これを**先**に送る。
const GSS_REQUEST: [u8; 8] = [0x00, 0x00, 0x00, 0x08, 0x04, 0xd2, 0x16, 0x30];
/// TLS record の最大サイズ(RFC 上限 16384)+ ヘッダ 5。
const MAX_TLS_RECORD: usize = 16384 + 5;

#[derive(Parser)]
#[command(
    name = "tsubomi-sni-gate",
    about = "Postgres-over-TLS の SNI 准入闸门(frps 前置・TLS 非終端)"
)]
struct Args {
    /// 監聽地址(公網)。
    #[arg(long, env = "SNI_GATE_LISTEN", default_value = "0.0.0.0:443")]
    listen: SocketAddr,

    /// 後端(localhost の frps proxy ポート)。
    #[arg(long, env = "SNI_GATE_BACKEND", default_value = "127.0.0.1:6432")]
    backend: SocketAddr,

    /// 許可する SNI(カンマ区切り複数可)。これ以外は即切断。
    #[arg(
        long = "sni",
        env = "SNI_GATE_SNI",
        value_delimiter = ',',
        required = true
    )]
    allowed_sni: Vec<String>,

    /// 前導(GSS/SSLRequest + ClientHello)読み取りと後端接続のタイムアウト秒。slowloris 対策。
    #[arg(long, env = "SNI_GATE_TIMEOUT", default_value_t = 10)]
    timeout: u64,

    /// 検証フェーズ(splice 前)の同時接続上限。ハンドシェイク濫用の絞り。
    #[arg(long, env = "SNI_GATE_MAX_PENDING", default_value_t = 8192)]
    max_pending: usize,

    /// 成立済みセッション(splice 中)の同時上限。許可 SNI を知る相手が握手後に
    /// 接続を抱えて闸门の fd を食い潰すのを防ぐ backstop(codex review [中] 指摘)。
    /// idle セッションの根治は pgbouncer の idle 超時 + 租户別 conn_limit(capacity 次回)。
    #[arg(long, env = "SNI_GATE_MAX_ACTIVE", default_value_t = 4096)]
    max_active: usize,

    /// 客户端側の拒否理由も記録する(既定 off = 扫描洪流で journald を溢れさせない)。
    #[arg(long, env = "SNI_GATE_LOG_REJECTS", default_value_t = false)]
    log_rejects: bool,
}

#[derive(Default)]
struct Stats {
    accepted: AtomicU64,
    rejected: AtomicU64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Arc::new(Args::parse());
    let listener = TcpListener::bind(args.listen)
        .await
        .with_context(|| format!("bind {}", args.listen))?;
    eprintln!(
        "tsubomi-sni-gate: listening {} -> {} (allowed sni: {:?})",
        args.listen, args.backend, args.allowed_sni
    );

    let sem = Arc::new(Semaphore::new(args.max_pending));
    let active_sem = Arc::new(Semaphore::new(args.max_active));
    let stats = Arc::new(Stats::default());

    // 60s 毎に accept/reject 累計を出す(journald で「どれだけ扫描を弾いたか」が見える)。
    {
        let stats = stats.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(60));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                eprintln!(
                    "tsubomi-sni-gate: stats accepted={} rejected={}",
                    stats.accepted.load(Ordering::Relaxed),
                    stats.rejected.load(Ordering::Relaxed),
                );
            }
        });
    }

    loop {
        let (client, peer) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                eprintln!("accept error: {e}");
                continue;
            }
        };
        let args = args.clone();
        let sem = sem.clone();
        let active_sem = active_sem.clone();
        let stats = stats.clone();
        tokio::spawn(async move {
            // pending permit は**検証フェーズだけ**保持(splice 前に解放)= 安く濫用できる
            // ハンドシェイク段だけ絞る。長命セッションは別の active permit で数える。
            let permit = match sem.try_acquire_owned() {
                Ok(p) => p,
                Err(_) => {
                    stats.rejected.fetch_add(1, Ordering::Relaxed);
                    eprintln!("reject {peer}: pending limit reached");
                    return;
                }
            };
            let mut client = client;
            match handshake(&mut client, &args).await {
                Ok(Some(backend)) => {
                    drop(permit);
                    // 成立済みセッションの上限。超過は新規を拒否(自身の fd 枯渇を防ぐ)。
                    let active_permit = match active_sem.try_acquire_owned() {
                        Ok(p) => p,
                        Err(_) => {
                            stats.rejected.fetch_add(1, Ordering::Relaxed);
                            eprintln!("reject {peer}: active session limit reached");
                            return;
                        }
                    };
                    stats.accepted.fetch_add(1, Ordering::Relaxed);
                    splice(client, backend).await;
                    drop(active_permit);
                }
                Ok(None) => {
                    // 客户端側の拒否(SNI 不一致 / 非 Postgres / 断連 等)。既定で無言。
                    stats.rejected.fetch_add(1, Ordering::Relaxed);
                }
                Err(e) => {
                    // 後端=自インフラの不調。常に記録(frps 落ち等の早期発見)。
                    stats.rejected.fetch_add(1, Ordering::Relaxed);
                    eprintln!("backend error serving {peer}: {e:#}");
                }
            }
        });
    }
}

/// 前導処理 + SNI 検証。許可なら後端へ繋いで前導を replay し、その stream を返す。
///
/// 戻り値の住み分け:
///   - `Ok(Some(backend))` = 許可(splice へ)
///   - `Ok(None)`          = 客户端側の拒否(無言。`--log-rejects` 時のみ理由を出す)
///   - `Err(_)`            = 後端=自インフラの不調(常に記録)
async fn handshake(client: &mut TcpStream, args: &Args) -> Result<Option<TcpStream>> {
    let dur = Duration::from_secs(args.timeout);

    // --- 前導 + ClientHello 読み取り(全体にタイムアウト)。失敗は客户端側の拒否扱い。---
    let record = match tokio::time::timeout(dur, read_preamble_and_hello(client)).await {
        Ok(Ok(rec)) => rec,
        Ok(Err(e)) => return Ok(reject(args, || format!("{e}"))),
        Err(_) => return Ok(reject(args, || "preamble timeout".into())),
    };

    // --- SNI 検証(record の中身 = ヘッダ 5B を除いた handshake)---
    let sni = match sni::parse_sni(&record[5..]) {
        Some(s) => s,
        None => return Ok(reject(args, || "no SNI in ClientHello".into())),
    };
    if !args.allowed_sni.iter().any(|a| a.eq_ignore_ascii_case(&sni)) {
        return Ok(reject(args, || format!("SNI {sni:?} not allowed")));
    }

    // --- 許可。後端へ繋いで前導を replay(後端接続にもタイムアウト)---
    let backend = match tokio::time::timeout(dur, connect_backend(args, &record)).await {
        Ok(Ok(b)) => b,
        Ok(Err(e)) => return Err(e),
        Err(_) => bail!("backend setup timeout ({})", args.backend),
    };
    Ok(Some(backend))
}

/// 客户端側の拒否:既定で無言、`--log-rejects` 時のみ理由を出す。常に `None` を返す。
fn reject(args: &Args, reason: impl FnOnce() -> String) -> Option<TcpStream> {
    if args.log_rejects {
        eprintln!("reject(client): {}", reason());
    }
    None
}

/// 前導(GSS は `N` で断り SSLRequest を待つ)→ 闸门が `S` を返す → 客户端の
/// TLS ClientHello record を 1 本読んで返す(ヘッダ 5B + 本体)。
async fn read_preamble_and_hello(client: &mut TcpStream) -> Result<Vec<u8>> {
    let mut req = [0u8; 8];
    client.read_exact(&mut req).await?;
    // GSSEncRequest が先に来たら本地で `N`(GSS 非対応)を返し、次の前導を待つ。
    if req == GSS_REQUEST {
        client.write_all(b"N").await?;
        client.read_exact(&mut req).await?;
    }
    if req != SSL_REQUEST {
        bail!("not a postgres SSLRequest preamble");
    }
    // 闸门が「TLS で良い」= `S` を本地で返す。客户端は直後に ClientHello を送る。
    client.write_all(b"S").await?;

    // TLS record ヘッダ(5B): content_type(1) + version(2) + length(2)。
    let mut hdr = [0u8; 5];
    client.read_exact(&mut hdr).await?;
    if hdr[0] != 0x16 {
        bail!(
            "expected TLS handshake record, got content type 0x{:02x}",
            hdr[0]
        );
    }
    let rec_len = ((hdr[3] as usize) << 8) | hdr[4] as usize;
    if rec_len == 0 || rec_len + 5 > MAX_TLS_RECORD {
        bail!("bad TLS record length {rec_len}");
    }
    let mut record = Vec::with_capacity(5 + rec_len);
    record.extend_from_slice(&hdr);
    record.resize(5 + rec_len, 0);
    client.read_exact(&mut record[5..]).await?;
    Ok(record)
}

/// 後端(localhost frps)へ繋ぎ、前導(SSLRequest→`S` 受領)を replay してから
/// 客户端の ClientHello record を流し込む。以降は素通し可能な stream を返す。
async fn connect_backend(args: &Args, client_hello_record: &[u8]) -> Result<TcpStream> {
    let mut backend = TcpStream::connect(args.backend)
        .await
        .with_context(|| format!("connect backend {}", args.backend))?;
    backend.write_all(&SSL_REQUEST).await?;
    // 不変量:後端(pgbouncer)は ClientHello を受け取る前、SSLRequest への応答として
    // **1 バイト `S` だけ**を返す。余分な先行バイトは想定しない(正常な pgbouncer は出さない。
    // 出すなら誤配置 = ここで気付くべき。codex review [低] 指摘)。
    let mut s = [0u8; 1];
    backend.read_exact(&mut s).await?;
    if s[0] != b'S' {
        bail!("backend declined SSL (replied 0x{:02x})", s[0]);
    }
    backend.write_all(client_hello_record).await?;
    Ok(backend)
}

/// 双方向素通し。どちらかが閉じたら終わる。
async fn splice(mut client: TcpStream, mut backend: TcpStream) {
    // エラー(片側 RST 等)は通常のセッション終了なので握り潰す。
    let _ = copy_bidirectional(&mut client, &mut backend).await;
}
