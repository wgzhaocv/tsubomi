//! valkey(cache)インスタンスとのやり取りを 1 箇所に集約する:per-cache の ACL ユーザの
//! 発行 / 削除 / 起動時 + 周期の収束(doc/paas-m5-design.md §6 / §7.3)。管制面 DB(pg-platform)が
//! 真実源で、valkey の per-cache ACL は揮発(`ACL SETUSER` はメモリのみ)なので平台が収束させる。
//!
//! 隔離は ACL(値アクセス `~<ns>:*` + チャンネル `&<ns>:*` + コマンド白名単
//! `+@all -@admin -@dangerous`)。acl_user / namespace は平台が `c_<shortid>` で生成する
//! (英小文字始まりの安全な識別子)。redis の `.arg()` は各引数を独立した RESP バルク文字列で
//! 送る(SQL のような文字列連結ではない)ので、コマンドインジェクションは原理的に起きない。

use crate::error::AppResult;
use crate::state::AppState;
use crate::tenant;
use redis::aio::MultiplexedConnection;

/// SCAN を 1 バッチ進める(cursor + `MATCH <pattern>` + COUNT)。count_keys / purge_namespace が
/// 共有する(コマンド組み立ての重複を避ける)。返り値 = (次 cursor, このバッチの key 群)。
async fn scan_batch(
    conn: &mut MultiplexedConnection,
    cursor: &str,
    pattern: &str,
) -> AppResult<(String, Vec<String>)> {
    Ok(redis::cmd("SCAN")
        .arg(cursor)
        .arg("MATCH")
        .arg(pattern)
        .arg("COUNT")
        .arg(256)
        .query_async(conn)
        .await?)
}

/// cache の acl_user(= namespace)を生成する:`c_<shortid>`。同値で両方を兼ねる(§2)。
pub fn gen_name() -> String {
    format!("c_{}", tenant::short_id())
}

/// per-cache の ACL ユーザを 1 接続上で作る / 上書きする(冪等)。reconcile が 1 接続で多数を
/// 貼り直せるよう接続を引数に取る。`reset` で初期化してから組み立てる。
async fn set_user_on(
    conn: &mut MultiplexedConnection,
    acl_user: &str,
    namespace: &str,
    password: &str,
) -> AppResult<()> {
    let _: () = redis::cmd("ACL")
        .arg("SETUSER")
        .arg(acl_user)
        .arg("reset") // 既存設定を全消去してから冪等に組み立てる
        .arg("on")
        .arg(format!(">{password}")) // パスワード追加は単一の `>`
        .arg(format!("~{namespace}:*")) // この前缀の key だけ値を読み書き可
        .arg("resetchannels")
        .arg(format!("&{namespace}:*")) // pub/sub もこの前缀のチャンネルだけ
        .arg("+@all")
        .arg("-@admin") // CONFIG / CLIENT KILL / ACL / REPLICAOF 等の管理系を禁止
        .arg("-@dangerous") // FLUSHALL / FLUSHDB / KEYS / SHUTDOWN / DEBUG / SWAPDB 等を禁止
        // SCRIPT / FUNCTION の **容器コマンド**は key 前缀で名前空間化されない**グローバル**状態を
        // 触る(SCRIPT FLUSH = 共有スクリプトキャッシュ全消し / FUNCTION FLUSH = 他テナントの
        // 関数ライブラリ破壊)ので個別に禁止する。EVAL / EVALSHA / FCALL は残る — スクリプト内の
        // key/channel アクセスは ACL パターンで検査される(§6。cross-ns は NOPERM)。
        .arg("-function")
        .arg("-script")
        .query_async(conn)
        .await?;
    Ok(())
}

/// per-cache の ACL ユーザを作る / 上書きする(冪等)。create と単発の収束で使う。
pub async fn set_user(
    client: &redis::Client,
    acl_user: &str,
    namespace: &str,
    password: &str,
) -> AppResult<()> {
    let mut conn = client.get_multiplexed_async_connection().await?;
    set_user_on(&mut conn, acl_user, namespace, password).await
}

/// per-cache の ACL ユーザを削除する(= 即座にその資格でログイン不可)。key は温存される。
/// DELUSER は存在しないユーザでもエラーにならない(削除数を返すだけ)= 冪等。
pub async fn del_user(client: &redis::Client, acl_user: &str) -> AppResult<()> {
    let mut conn = client.get_multiplexed_async_connection().await?;
    let _: i64 = redis::cmd("ACL")
        .arg("DELUSER")
        .arg(acl_user)
        .query_async(&mut conn)
        .await?;
    Ok(())
}

/// namespace 配下の key 数を概算する(SCAN で数える。admin 接続)。§4.2:per-namespace の正確な
/// メモリは valkey に無いので key 数を代用(詳細表示 / restore 報告 / owner ranking が使う)。
/// 失敗 / valkey 不通は None(best-effort)。admin は `+@all` なので SCAN 可。
pub async fn count_keys(client: &redis::Client, namespace: &str) -> Option<i64> {
    let mut conn = client.get_multiplexed_async_connection().await.ok()?;
    let pattern = format!("{namespace}:*");
    let mut cursor = "0".to_string();
    let mut total: i64 = 0;
    loop {
        let (next, keys) = scan_batch(&mut conn, &cursor, &pattern).await.ok()?;
        total += keys.len() as i64;
        cursor = next;
        if cursor == "0" {
            break;
        }
    }
    Some(total)
}

/// namespace 配下の全 key を削除する(SCAN + UNLINK。purge で確実にメモリ解放。§7.2)。
/// admin 接続。空でも冪等。
pub async fn purge_namespace(client: &redis::Client, namespace: &str) -> AppResult<()> {
    let mut conn = client.get_multiplexed_async_connection().await?;
    let pattern = format!("{namespace}:*");
    let mut cursor = "0".to_string();
    loop {
        let (next, keys) = scan_batch(&mut conn, &cursor, &pattern).await?;
        if !keys.is_empty() {
            // UNLINK(非同期解放)。key 群を 1 コマンドに束ねる。
            let mut unlink = redis::cmd("UNLINK");
            for k in &keys {
                unlink.arg(k);
            }
            let _: i64 = unlink.query_async(&mut conn).await?;
        }
        cursor = next;
        if cursor == "0" {
            break;
        }
    }
    Ok(())
}

/// ACL 収束(reconcile 哲学。起動時 + 周期で呼ぶ。§7.3)。cache_details(真実源)の
/// **生存 cache を毎回 fresh に SELECT** して ACL を貼り直す(RACE-1:古いスナップショットを
/// 使うと delete↔tick の競態で削除直後ユーザを一瞬復活させ得る。delete 済みは選ばれない)。
/// best-effort = エラーは log に握り潰し、背景処理は決して落とさない。1 接続を使い回す。
pub async fn reconcile_acls(state: &AppState) {
    let rows: Vec<(String, String, Vec<u8>)> = match sqlx::query_as(
        "SELECT cd.acl_user, cd.namespace, cd.password_enc
           FROM cache_details cd
           JOIN resources r ON r.id = cd.resource_id
          WHERE r.kind = 'cache' AND r.deleted_at IS NULL",
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(error = ?e, "valkey reconcile: cache 一覧の取得に失敗");
            return;
        }
    };
    if rows.is_empty() {
        return;
    }

    let mut conn = match state.valkey.get_multiplexed_async_connection().await {
        Ok(c) => c,
        Err(e) => {
            // valkey が落ちている → 次の tick で再挑戦(自己回復)。
            tracing::warn!(error = ?e, "valkey reconcile: 接続に失敗(次の tick で再試行)");
            return;
        }
    };

    let mut ok = 0usize;
    for (acl_user, namespace, password_enc) in &rows {
        let password = match state.crypto.decrypt(password_enc) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = ?e, acl_user, "valkey reconcile: パスワード復号に失敗");
                continue;
            }
        };
        match set_user_on(&mut conn, acl_user, namespace, &password).await {
            Ok(()) => ok += 1,
            Err(e) => {
                tracing::warn!(error = ?e, acl_user, "valkey reconcile: ACL SETUSER に失敗")
            }
        }
    }
    tracing::debug!(total = rows.len(), ok, "valkey reconcile: ACL 収束");
}
