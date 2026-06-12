use std::io::{self, BufRead, Write};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::Utc;
use gethostname::gethostname;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tsubomi_shared::pkce_challenge;

use crate::api::fetch_me;
use crate::commands::resolve_server_from;
use crate::config;
use crate::oauth::{
    build_authorize_url, exchange_code, generate_state, generate_verifier, loopback_redirect_uri,
    manual_redirect_uri,
};

/// loopback リスナーでブラウザのリダイレクトを待つ上限。
const LOOPBACK_TIMEOUT: Duration = Duration::from_secs(300);

pub async fn run(server_override: Option<String>, manual: bool) -> Result<()> {
    // 読み込みは一度だけ:既存設定にアイデンティティをマージするのと、
    // override が無いときのサーバ URL 解決の両方に使う。
    let mut cfg = config::load()?.unwrap_or_default();
    let server_url = resolve_server_from(server_override.as_deref(), Some(&cfg));
    let server_url = server_url.as_str();

    // PKCE:verifier はこのプロセスから出ない。URL を通るのは challenge
    // (sha256 ハッシュ)だけなので、傍受されても無力。
    let verifier = generate_verifier();
    let challenge = pkce_challenge(&verifier);
    let state = generate_state();
    let hint = build_hint();

    let token = if manual {
        // manual:ブラウザと CLI が別マシン(SSH 先など)でも使える。
        // サーバのページがコードを表示し、ユーザが貼り戻す。
        let redirect = manual_redirect_uri(server_url);
        let url = build_authorize_url(server_url, &redirect, &challenge, &state, &hint)?;
        open_in_browser(&url);
        let code = read_code_from_stdin()?;
        exchange_code(server_url, &redirect, &code, &verifier, &state).await?
    } else {
        // loopback(デフォルト、RFC 8252):一回限りのローカルリスナーが
        // リダイレクトを直接受け取る。コピペ不要。
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .context("failed to bind loopback listener")?;
        let port = listener.local_addr()?.port();
        let redirect = loopback_redirect_uri(port);
        let url = build_authorize_url(server_url, &redirect, &challenge, &state, &hint)?;
        open_in_browser(&url);
        println!("ブラウザで「許可する」を押してください…(--manual でコピペ方式)");

        let (code, returned_state) =
            tokio::time::timeout(LOOPBACK_TIMEOUT, wait_for_code(listener))
                .await
                .context("timed out waiting for the browser (try: tbm login --manual)")??;
        // CSRF:自分が発行した state が返ってきたことを確認。
        if returned_state != state {
            bail!("state mismatch in loopback callback");
        }
        exchange_code(server_url, &redirect, &code, &verifier, &state).await?
    };

    // 保存の前に検証する:壊れたトークンは次のコマンドで静かに失敗する
    // のではなく、ここで大きな音を立てる。whoami 用のアイデンティティの
    // キャッシュも兼ねる。
    let me = fetch_me(server_url, &token).await?;

    cfg.server_url = server_url.to_owned();
    cfg.token = Some(token);
    cfg.email = Some(me.email.clone());
    cfg.user_id = Some(me.user_id.clone());
    config::save(&cfg)?;

    println!("ok {}", me.email);
    Ok(())
}

fn open_in_browser(url: &str) {
    println!("opening: {url}");
    if webbrowser::open(url).is_err() {
        eprintln!("(browser failed to open; copy the URL above into a browser manually)");
    }
}

fn read_code_from_stdin() -> Result<String> {
    print!("paste code: ");
    io::stdout().flush().ok();
    let mut code = String::new();
    io::stdin()
        .lock()
        .read_line(&mut code)
        .context("failed to read code from stdin")?;
    let code = code.trim().to_owned();
    if code.is_empty() {
        bail!("no code entered");
    }
    Ok(code)
}

/// `GET /callback?code=…&state=…` が来るまで受け付け、code と state を返す。
/// favicon 等の無関係なリクエストには 404 を返して待ち続ける。
async fn wait_for_code(listener: tokio::net::TcpListener) -> Result<(String, String)> {
    loop {
        let (mut stream, _) = listener.accept().await.context("loopback accept failed")?;

        // リクエストヘッダだけ読めば十分(GET にボディは無い)。
        let mut buf = vec![0u8; 4096];
        let n = stream.read(&mut buf).await.unwrap_or(0);
        let head = String::from_utf8_lossy(&buf[..n]);
        let path = head
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .unwrap_or("");

        if !path.starts_with("/callback") {
            let _ = stream
                .write_all(b"HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n")
                .await;
            continue;
        }

        let parsed = url::Url::parse(&format!("http://127.0.0.1{path}"))
            .context("failed to parse callback URL")?;
        let get = |k: &str| {
            parsed
                .query_pairs()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.into_owned())
        };
        let (Some(code), Some(state)) = (get("code"), get("state")) else {
            let _ = stream
                .write_all(b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\n")
                .await;
            bail!("callback missing code/state");
        };

        let body = "<!doctype html><meta charset=\"utf-8\"><title>tbm</title>\
            <body style=\"font-family:system-ui;display:grid;place-items:center;height:100vh;margin:0\">\
            <div style=\"text-align:center\"><h2>✓ tbm にログインしました</h2>\
            <p>このタブを閉じて、ターミナルに戻ってください。</p></div>";
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = stream.write_all(resp.as_bytes()).await;
        let _ = stream.shutdown().await;
        return Ok((code, state));
    }
}

fn build_hint() -> String {
    let host = gethostname().to_string_lossy().into_owned();
    let host = if host.is_empty() {
        "cli".to_owned()
    } else {
        host
    };
    format!("{host}-{}", Utc::now().format("%Y-%m-%d"))
}
