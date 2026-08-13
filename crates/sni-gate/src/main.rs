//! tsubomi-sni-gate — 公開データベース(frp 中継)の VPS エッジ SNI 受け入れゲート。
//!
//! 背景:公開ポートに裸の Postgres(pgbouncer)を晒すと、大量スキャンが frps の
//! work-connection 池を**TLS / 認証より前に**食い潰し、隧道が全員に対して死ぬ
//! (DoS 型事故、`doc/incident-frp-pg-public-2026-06-22.md`)。
//!
//! このゲートを frps の**前**に置き、frp のリソースを消費する前に SNI で准入する:
//!   client → [ゲート :443] → (localhost) frps → 隧道 → frpc → pgbouncer
//!
//! 効きどころ:
//!   - **TLS は終端しない**。平文の ClientHello から SNI を覗くだけ → エンドツーエンドで
//!     `verify-full` も証明書も不変。
//!   - **プリアンブルをゲートがローカル終結**(自分で `S` を返す)。SNI を検証して**初めて**バックエンドへ
//!     繋ぎ、プリアンブルを replay する → スキャン器はバックエンド(frp)接続を一切起こさない。
//!   - SNI ≠ 許可ドメイン / 非 Postgres プリアンブル / プリアンブルタイムアウト = 即切断(fail-closed)。
//!   - **クライアント側の拒否は既定で無言**(大量スキャンで journald を溢れさせない)。記録に値する
//!     のはバックエンド=自インフラの不調だけ。理由を見たいときは `--log-rejects`。

mod sni;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::Parser;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, copy_bidirectional};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;

/// Postgres SSLRequest: length=8, code=80877103 (0x04d2162f)。
const SSL_REQUEST: [u8; 8] = [0x00, 0x00, 0x00, 0x08, 0x04, 0xd2, 0x16, 0x2f];
/// Postgres GSSENCRequest: length=8, code=80877104 (0x04d21630)。
/// Debian/Ubuntu の libpq は GSSAPI 込みで既定 `gssencmode=prefer` = これを**先**に送る。
const GSS_REQUEST: [u8; 8] = [0x00, 0x00, 0x00, 0x08, 0x04, 0xd2, 0x16, 0x30];
/// TLS record の最大サイズ(RFC 上限 16384)+ ヘッダ 5。
const MAX_TLS_RECORD: usize = 16384 + 5;
/// TLS record の content_type = handshake(0x16)。接続の先頭がこれなら redis(rediss = TLS-on-connect、
/// プリアンブルなし)、そうでなければ Postgres の SSLRequest/GSSENCRequest プリアンブル(0x00 始まり)とみなす。
const TLS_HANDSHAKE: u8 = 0x16;

#[derive(Parser)]
#[command(
    name = "tsubomi-sni-gate",
    about = "Postgres-over-TLS の SNI 受け入れゲート(frps 前置・TLS 非終端)"
)]
struct Args {
    /// 待ち受けアドレス(インターネット)。
    #[arg(long, env = "SNI_GATE_LISTEN", default_value = "0.0.0.0:443")]
    listen: SocketAddr,

    /// **Postgres** パスのバックエンド(localhost の frps pg proxy ポート)。先頭が 8B SSLRequest/GSSENCRequest
    /// の接続をここへ流す(プリアンブルをローカル終結し replay する)。
    #[arg(long, env = "SNI_GATE_BACKEND", default_value = "127.0.0.1:6432")]
    backend: SocketAddr,

    /// **Postgres** パスで許可する SNI(カンマ区切り複数可)。これ以外は即切断。省略可(redis 専用 gate
    /// なら不要 — 起動時に pg/redis いずれかの route が必須かを検査する)。
    #[arg(long = "sni", env = "SNI_GATE_SNI", value_delimiter = ',')]
    allowed_sni: Vec<String>,

    /// **redis**(rediss = TLS-on-connect)パスのバックエンド(frps cache proxy ポート)。**未指定なら redis パス
    /// (先頭 0x16)は拒否**(fail-closed = pg のみ配備で HTTPS スキャンを弾く)。
    #[arg(long, env = "SNI_GATE_REDIS_BACKEND")]
    redis_backend: Option<SocketAddr>,

    /// **redis** パスで許可する SNI(カンマ区切り複数可)。`redis_backend` とセットで指定。
    #[arg(long = "redis-sni", env = "SNI_GATE_REDIS_SNI", value_delimiter = ',')]
    redis_sni: Vec<String>,

    /// **redis 専用ポート用の緩和**:SNI 無しの ClientHello も許可する(`Bun.RedisClient` 等、SNI を
    /// 送らない client 向け)。SNI が**有る**場合は依然 `--redis-sni` と一致必須(他ドメイン狙いのスキャンは
    /// 拒否)。443 共用 gate では **off**(SNI 無し = pg と区別不能なので拒否)。専用ポート(例 8080)用。
    /// なお非 TLS / 畸形 ClientHello は SNI の有無に関係なく弾かれる(基本的な拒否は維持)。
    #[arg(long, env = "SNI_GATE_REDIS_ALLOW_NO_SNI", default_value_t = false)]
    redis_allow_no_sni: bool,

    /// **HTTPS**(TLS-on-connect)パスのバックエンド(frps の registry proxy ポート等)。redis と同じ
    /// passthrough(TLS 非終端・ClientHello replay)だが、**SNI 完全一致が必須**の別 route として
    /// 持つ:registry push を CF 100MB 上限を通らずに受ける直接接続入口用(paas-registry-direct-design)。
    /// 未指定なら HTTPS パスは無し(既存配備は不変)。
    #[arg(long, env = "SNI_GATE_HTTPS_BACKEND")]
    https_backend: Option<SocketAddr>,

    /// **HTTPS** パスで許可する SNI(カンマ区切り複数可)。`https_backend` とセットで指定。
    /// SNI 無しは常に拒否(素性の知れない TLS スキャンを frp より前で弾く)。
    #[arg(long = "https-sni", env = "SNI_GATE_HTTPS_SNI", value_delimiter = ',')]
    https_sni: Vec<String>,

    /// プリアンブル(GSS/SSLRequest + ClientHello)読み取りとバックエンド接続のタイムアウト秒。slowloris 対策。
    #[arg(long, env = "SNI_GATE_TIMEOUT", default_value_t = 10)]
    timeout: u64,

    /// 検証フェーズ(splice 前)の同時接続上限。ハンドシェイク濫用の絞り。
    #[arg(long, env = "SNI_GATE_MAX_PENDING", default_value_t = 8192)]
    max_pending: usize,

    /// 成立済みセッション(splice 中)の同時上限。許可 SNI を知る相手が握手後に
    /// 接続を抱えてゲートの fd を食い潰すのを防ぐ backstop(codex review [中] 指摘)。
    /// idle セッションの根治は pgbouncer の idle 超時 + 租户別 conn_limit(capacity 次回)。
    #[arg(long, env = "SNI_GATE_MAX_ACTIVE", default_value_t = 4096)]
    max_active: usize,

    /// クライアント側の拒否理由も記録する(既定 off = 大量スキャンで journald を溢れさせない)。
    #[arg(long, env = "SNI_GATE_LOG_REJECTS", default_value_t = false)]
    log_rejects: bool,
}

#[derive(Default)]
struct Stats {
    accepted: AtomicU64,
    /// accepted のうち SNI 無しで通過した数(`--redis-allow-no-sni` のパス)。
    /// no-SNI 許可ポートでは「TLS を喋るスキャン器」も accepted に入るため、この内訳が無いと
    /// 正規接続とスキャンの区別がつかない(実運用フィードバック起因)。記録は計数のみ(軽量)。
    no_sni: AtomicU64,
    rejected: AtomicU64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Arc::new(Args::parse());
    // route が 1 つも無いと無意味(全拒否)。pg(--sni)/ redis / https のいずれかは要る。
    if args.allowed_sni.is_empty() && args.redis_backend.is_none() && args.https_backend.is_none()
    {
        bail!(
            "route が未設定:--sni(pg)/ --redis-backend(redis)/ --https-backend(https)のいずれかを指定してください"
        );
    }
    // backend だけ指定して対の SNI が空だと、そのパスは実行時に全拒否になる。
    // 起動時に誤配置として弾く(オペレータへの早期フィードバック)。
    if args.redis_backend.is_some() && args.redis_sni.is_empty() {
        bail!("--redis-backend を指定したら --redis-sni も必須です(空だと redis パスが全拒否になる)");
    }
    if args.https_backend.is_some() && args.https_sni.is_empty() {
        bail!("--https-backend を指定したら --https-sni も必須です(空だと https パスが全拒否になる)");
    }
    let listener = TcpListener::bind(args.listen)
        .await
        .with_context(|| format!("bind {}", args.listen))?;
    eprintln!(
        "tsubomi-sni-gate: listening {} | pg {} sni {:?} | redis {:?} sni {:?} | https {:?} sni {:?}",
        args.listen,
        args.backend,
        args.allowed_sni,
        args.redis_backend,
        args.redis_sni,
        args.https_backend,
        args.https_sni
    );

    let sem = Arc::new(Semaphore::new(args.max_pending));
    let active_sem = Arc::new(Semaphore::new(args.max_active));
    let stats = Arc::new(Stats::default());

    // 60s 毎に accept/reject 累計を出す(journald で「どれだけスキャンを弾いたか」が見える)。
    {
        let stats = stats.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(60));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                eprintln!(
                    "tsubomi-sni-gate: stats accepted={} (no_sni={}) rejected={}",
                    stats.accepted.load(Ordering::Relaxed),
                    stats.no_sni.load(Ordering::Relaxed),
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
                Ok(Some((backend, no_sni))) => {
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
                    if no_sni {
                        stats.no_sni.fetch_add(1, Ordering::Relaxed);
                    }
                    splice(client, backend).await;
                    drop(active_permit);
                }
                Ok(None) => {
                    // クライアント側の拒否(SNI 不一致 / 非 Postgres / 断連 等)。既定で無言。
                    stats.rejected.fetch_add(1, Ordering::Relaxed);
                }
                Err(e) => {
                    // バックエンド=自インフラの不調。常に記録(frps 落ち等の早期発見)。
                    stats.rejected.fetch_add(1, Ordering::Relaxed);
                    eprintln!("backend error serving {peer}: {e:#}");
                }
            }
        });
    }
}

/// ゲートが振り分けるバックエンドの種別。プリアンブルの有無 = バックエンドへの接続手順が違う(下記 connect_*)。
enum Proto {
    /// Postgres:SSLRequest プリアンブルをローカル終結 → バックエンドへ replay してから ClientHello を流す。
    Pg,
    /// redis(rediss = TLS-on-connect):プリアンブルなし。ClientHello をそのまま流す。
    Redis,
}

/// 読み取り段の結果:振り分け先(バックエンド種別 + アドレス + 読んだ ClientHello record)か、クライアント側の拒否。
enum Routed {
    To {
        proto: Proto,
        addr: SocketAddr,
        record: Vec<u8>,
        /// SNI 無しで通過したか(`--redis-allow-no-sni` のパスのみ true。統計の内訳用)。
        no_sni: bool,
    },
    Reject(String),
}

/// プリアンブル処理 + SNI 検証 + 振り分け。許可ならバックエンドへ繋いでその stream を返す。
///
/// **timeout は 2 段**:
///   (A) 読み取り段(先頭バイト + プリアンブル + ClientHello + SNI 検証 = `read_phase`)を **1 つ**の
///       timeout で囲む。段ごとに分けると slowloris のソケット保持時間が延びるため、読み取りは
///       接続毎まとめて 1×。
///   (B) バックエンド(frps)接続を別 timeout で。(A) の失敗 = クライアント側(無言拒否)、(B) の失敗 = 自インフラ
///       (Err として記録)、という住み分けを保つ。
///
/// 戻り値:`Ok(Some((stream, no_sni)))` = 許可(splice へ。no_sni は統計の内訳用)/
/// `Ok(None)` = クライアント側の拒否 / `Err(_)` = 自インフラ不調。
async fn handshake(client: &mut TcpStream, args: &Args) -> Result<Option<(TcpStream, bool)>> {
    let dur = Duration::from_secs(args.timeout);

    // (A) 読み取り + 協議判定 + SNI 検証(まとめて 1 つの timeout)。
    let routed = match tokio::time::timeout(dur, read_phase(client, args)).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => return Ok(reject(args, || format!("{e}"))), // 読み取り中の I/O エラー
        Err(_) => return Ok(reject(args, || "handshake read timeout".into())),
    };
    let (proto, addr, record, no_sni) = match routed {
        Routed::To {
            proto,
            addr,
            record,
            no_sni,
        } => (proto, addr, record, no_sni),
        Routed::Reject(why) => return Ok(reject(args, move || why)),
    };

    // (B) バックエンド接続(別 timeout = 自インフラの不調は Err として記録)。
    let connected = match proto {
        Proto::Pg => tokio::time::timeout(dur, connect_pg_backend(addr, &record)).await,
        Proto::Redis => tokio::time::timeout(dur, connect_redis_backend(addr, &record)).await,
    };
    match connected {
        Ok(Ok(b)) => Ok(Some((b, no_sni))),
        Ok(Err(e)) => Err(e),
        Err(_) => bail!("backend setup timeout ({addr})"),
    }
}

/// 先頭 1 バイトで協議を判定し、プリアンブル / ClientHello を読んで SNI を検証する(I/O をここに集約 = 呼び側が
/// 1 つの timeout で囲める)。先頭 `0x16` = redis(TLS-on-connect)、それ以外 = Postgres(SSLRequest
/// プリアンブルは `0x00` 始まり)。クライアント側の拒否は `Ok(Routed::Reject)`、読み取り I/O エラーは `Err`。
async fn read_phase(client: &mut TcpStream, args: &Args) -> Result<Routed> {
    let mut first = [0u8; 1];
    client.read_exact(&mut first).await?;

    if first[0] == TLS_HANDSHAKE {
        // TLS-on-connect(redis / https)。どちらの route も無ければ ClientHello を読む前に拒否
        // (pg のみ部署で HTTPS スキャンを安く弾く)。
        let redis_addr = args.redis_backend.filter(|_| !args.redis_sni.is_empty());
        let https_addr = args.https_backend.filter(|_| !args.https_sni.is_empty());
        if redis_addr.is_none() && https_addr.is_none() {
            return Ok(Routed::Reject("tls route not configured".into()));
        }
        // 先頭 `0x16` は読み済み = それを content_type に TLS record を組む(非 TLS / 畸形は read_tls_record が拒否)。
        let record = read_tls_record(client, TLS_HANDSHAKE).await?;
        let parsed = sni::parse_sni(&record[5..]);
        // 1) HTTPS route(registry 直接接続):SNI 完全一致のみ。redis より先に判定する
        //    (SNI が違えば次の redis 判定へ落ちる = 共存可能)。
        if let (Some(addr), Some(sni)) = (https_addr, parsed.as_deref())
            && sni_allowed(&args.https_sni, sni)
        {
            return Ok(Routed::To {
                proto: Proto::Redis, // TLS passthrough は redis と同一手順(プリアンブルなし・replay のみ)
                addr,
                record,
                no_sni: false,
            });
        }
        // 2) redis route。未配備なら拒否。
        let Some(addr) = redis_addr else {
            return Ok(Routed::Reject("redis route not configured".into()));
        };
        // SNI 検証:有 → `--redis-sni` 一致必須(他ドメイン狙いのスキャンを拒否)。
        // 無 → `--redis-allow-no-sni` on なら許可(専用 :8080 = Bun 原生など SNI 非送出 client 向け)、
        //      off なら拒否(pg と同居の :443 では SNI 無し = pg と区別不能のため)。
        // ※ 非 TLS / record framing 不正は read_tls_record が既に弾く(allow-no-sni でも基本拒否は維持)。
        let no_sni = match parsed {
            Some(sni) if sni_allowed(&args.redis_sni, &sni) => false,
            Some(sni) => return Ok(Routed::Reject(format!("redis SNI {sni:?} not allowed"))),
            None if args.redis_allow_no_sni => true,
            None => return Ok(Routed::Reject("no SNI in ClientHello (--redis-allow-no-sni off)".into())),
        };
        Ok(Routed::To {
            proto: Proto::Redis,
            addr,
            record,
            no_sni,
        })
    } else {
        // Postgres。先頭バイトを渡しプリアンブル(GSS/SSLRequest)をローカル終結 → ClientHello を読む。
        let record = read_pg_preamble_and_hello(client, first[0]).await?;
        let sni = match sni::parse_sni(&record[5..]) {
            Some(s) => s,
            None => return Ok(Routed::Reject("no SNI in ClientHello".into())),
        };
        if !sni_allowed(&args.allowed_sni, &sni) {
            return Ok(Routed::Reject(format!("pg SNI {sni:?} not allowed")));
        }
        Ok(Routed::To {
            proto: Proto::Pg,
            addr: args.backend,
            record,
            no_sni: false,
        })
    }
}

/// クライアント側の拒否:既定で無言、`--log-rejects` 時のみ理由を出す。常に `None` を返す。
fn reject<T>(args: &Args, reason: impl FnOnce() -> String) -> Option<T> {
    if args.log_rejects {
        eprintln!("reject(client): {}", reason());
    }
    None
}

/// SNI が許可リストにあるか(大文字小文字を無視)。pg / redis 両パスで共有。
fn sni_allowed(allowed: &[String], sni: &str) -> bool {
    allowed.iter().any(|a| a.eq_ignore_ascii_case(sni))
}

/// Postgres プリアンブル(GSS は `N` で断り SSLRequest を待つ)→ ゲートが `S` を返す → クライアントの
/// TLS ClientHello record を 1 本読んで返す。`first` は `handshake` が先読みした先頭バイト。
async fn read_pg_preamble_and_hello(client: &mut TcpStream, first: u8) -> Result<Vec<u8>> {
    let mut req = [0u8; 8];
    req[0] = first;
    client.read_exact(&mut req[1..]).await?;
    // GSSEncRequest が先に来たらローカルで `N`(GSS 非対応)を返し、次のプリアンブル(新規 8B)を待つ。
    if req == GSS_REQUEST {
        client.write_all(b"N").await?;
        client.read_exact(&mut req).await?;
    }
    if req != SSL_REQUEST {
        bail!("not a postgres SSLRequest preamble");
    }
    // ゲートが「TLS で良い」= `S` をローカルで返す。クライアントは直後に ClientHello を送る。
    client.write_all(b"S").await?;
    // ClientHello は新規の TLS record。content_type(1B)を読んでから残りを read_tls_record で。
    let mut ct = [0u8; 1];
    client.read_exact(&mut ct).await?;
    read_tls_record(client, ct[0]).await
}

/// TLS record を 1 本読んで返す(ヘッダ 5B + 本体)。`content_type` は呼び側が先に読んだ 1 バイト
/// (pg は `S` 応答後に新規読み、redis は接続の先頭バイトを流用)。handshake(0x16)以外は拒否。
async fn read_tls_record<R: AsyncRead + Unpin>(stream: &mut R, content_type: u8) -> Result<Vec<u8>> {
    if content_type != TLS_HANDSHAKE {
        bail!("expected TLS handshake record, got content type 0x{content_type:02x}");
    }
    // ヘッダ残り 4B: version(2) + length(2)。
    let mut rest = [0u8; 4];
    stream.read_exact(&mut rest).await?;
    let rec_len = ((rest[2] as usize) << 8) | rest[3] as usize;
    if rec_len == 0 || rec_len + 5 > MAX_TLS_RECORD {
        bail!("bad TLS record length {rec_len}");
    }
    let mut record = Vec::with_capacity(5 + rec_len);
    record.push(content_type);
    record.extend_from_slice(&rest);
    record.resize(5 + rec_len, 0);
    stream.read_exact(&mut record[5..]).await?;
    Ok(record)
}

/// Postgres バックエンド(frps pg proxy)へ繋ぎ、プリアンブル(SSLRequest→`S` 受領)を replay してから
/// クライアントの ClientHello record を流し込む。以降は素通し可能な stream を返す。
async fn connect_pg_backend(addr: SocketAddr, client_hello_record: &[u8]) -> Result<TcpStream> {
    let mut backend = TcpStream::connect(addr)
        .await
        .with_context(|| format!("connect pg backend {addr}"))?;
    backend.write_all(&SSL_REQUEST).await?;
    // 不変量:バックエンド(pgbouncer)は ClientHello を受け取る前、SSLRequest への応答として
    // **1 バイト `S` だけ**を返す。余分な先行バイトは想定しない(正常な pgbouncer は出さない)。
    let mut s = [0u8; 1];
    backend.read_exact(&mut s).await?;
    if s[0] != b'S' {
        bail!("pg backend declined SSL (replied 0x{:02x})", s[0]);
    }
    backend.write_all(client_hello_record).await?;
    Ok(backend)
}

/// redis バックエンド(frps cache proxy、valkey が TLS を終端)へ繋ぎ、読んだ ClientHello record を
/// そのまま流す。**プリアンブルは無い**(rediss = TLS-on-connect)ので `S` 応答も replay もしない。
async fn connect_redis_backend(addr: SocketAddr, client_hello_record: &[u8]) -> Result<TcpStream> {
    let mut backend = TcpStream::connect(addr)
        .await
        .with_context(|| format!("connect redis backend {addr}"))?;
    backend.write_all(client_hello_record).await?;
    Ok(backend)
}

/// 双方向素通し。どちらかが閉じたら終わる。
async fn splice(mut client: TcpStream, mut backend: TcpStream) {
    // エラー(片側 RST 等)は通常のセッション終了なので握り潰す。
    let _ = copy_bidirectional(&mut client, &mut backend).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// content_type(0x16) + version(2) + length(2) + body の TLS record を組む。
    fn tls_record(body: &[u8]) -> Vec<u8> {
        let mut rec = vec![TLS_HANDSHAKE, 0x03, 0x03];
        rec.extend_from_slice(&(body.len() as u16).to_be_bytes());
        rec.extend_from_slice(body);
        rec
    }

    // content_type は呼び側が先読みする契約なので、テストも先頭バイトを別渡し・残りを stream に置く。
    // `&[u8]` は tokio の AsyncRead を実装するので read_tls_record にそのまま渡せる。

    #[tokio::test]
    async fn read_tls_record_roundtrips() {
        let rec = tls_record(b"clienthello-body-bytes");
        let mut rest: &[u8] = &rec[1..];
        let out = read_tls_record(&mut rest, rec[0]).await.unwrap();
        assert_eq!(out, rec);
    }

    #[tokio::test]
    async fn read_tls_record_rejects_non_handshake_content_type() {
        // 0x17 = application_data 等。handshake(0x16)以外は拒否。
        let mut rest: &[u8] = &[0x03, 0x03, 0x00, 0x01, 0xff];
        assert!(read_tls_record(&mut rest, 0x17).await.is_err());
    }

    #[tokio::test]
    async fn read_tls_record_rejects_zero_length() {
        let mut rest: &[u8] = &[0x03, 0x03, 0x00, 0x00];
        assert!(read_tls_record(&mut rest, TLS_HANDSHAKE).await.is_err());
    }

    #[tokio::test]
    async fn read_tls_record_rejects_truncated_body() {
        // 宣言長より本体が 1B 短い → read_exact が EOF で失敗(畸形を素通ししない)。
        let rec = tls_record(b"0123456789");
        let truncated = &rec[1..rec.len() - 1];
        let mut rest: &[u8] = truncated;
        assert!(read_tls_record(&mut rest, rec[0]).await.is_err());
    }

    #[test]
    fn sni_allowed_is_case_insensitive_and_exact() {
        let allowed = vec!["cache.tsubomi-app.com".to_string()];
        assert!(sni_allowed(&allowed, "cache.tsubomi-app.com"));
        assert!(sni_allowed(&allowed, "CACHE.Tsubomi-App.com")); // 大文字小文字を無視
        assert!(!sni_allowed(&allowed, "db.tsubomi-app.com")); // 別ホストは不可
        assert!(!sni_allowed(&allowed, "evil.cache.tsubomi-app.com")); // 部分一致は不可
        assert!(!sni_allowed(&[], "anything")); // 空リスト = 全拒否(redis 未設定相当)
    }
}
