//! メール送信(Resend HTTP API)。M4 ガバナンスの通知路:磁盘水位告警(S2)、
//! owner の危険操作の検証コード(S3)。新 crate は足さない — 既存 state.http(reqwest)で叩く。
//!
//! 退路:`RESEND_API_KEY` 未設定なら**送らず log だけ**残して Ok(dev / Resend 未契約時)。
//! 本番で初めて実送する。宛先は基本 owner(config.owner_emails が owner の定義 = 真実源)。

use crate::state::AppState;

const RESEND_ENDPOINT: &str = "https://api.resend.com/emails";

/// テキストメールを送る。失敗しても呼び出し側(告警 / 検証コード)は best-effort で扱える。
/// 宛先が空なら何もしない。key 未設定は「送らず log」= Ok(運用上の正常系)。
pub async fn send(
    state: &AppState,
    to: &[String],
    subject: &str,
    body_text: &str,
) -> anyhow::Result<()> {
    if to.is_empty() {
        tracing::warn!(subject, "メール宛先が空 — 送信せず(owner_emails 未設定?)");
        return Ok(());
    }
    let cfg = &state.config;
    let (Some(key), Some(from)) = (&cfg.resend_api_key, &cfg.mail_from) else {
        // dev / 未契約:実送しない。**本文は log に出さない**(検証コード等の秘密が漏れる)。
        // subject + 宛先数だけ残す(設計 §4.1)。dev で本文が要る場合は呼び出し側が別途出す。
        tracing::info!(
            subject,
            recipients = to.len(),
            "[mail:dropped] RESEND_API_KEY 未設定のため送信しない"
        );
        return Ok(());
    };

    let resp = state
        .http
        .post(RESEND_ENDPOINT)
        .bearer_auth(key)
        .json(&serde_json::json!({
            "from": from,
            "to": to,
            "subject": subject,
            "text": body_text,
        }))
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Resend が {status} を返した: {body}");
    }
    tracing::info!(subject, to = ?to, "メール送信 OK");
    Ok(())
}
