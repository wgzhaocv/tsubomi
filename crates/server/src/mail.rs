//! メール送信(Resend SDK)。M4 ガバナンスの通知路:owner 解除 / 磁盘水位告警 / owner
//! 危険操作の検証コード。**resend-rs(rustls-tls)** で送る(以前は手書き reqwest POST)。
//! 本文は React Email から焼いた静的 HTML(`mail/templates/*.html`)を {{var}} 置換して
//! html に、現行の素文面を text fallback に。
//!
//! 退路:`RESEND_API_KEY` / `TSUBOMI_MAIL_FROM` 未設定なら**送らず log だけ**残して Ok
//! (dev / Resend 未契約時)。失敗は best-effort(呼び出し側は告警 / コードで握る)。
//! 宛先は基本 owner(owner_roster が真実源 — owners.rs)。

use crate::state::AppState;
use resend_rs::Resend;
use resend_rs::types::CreateEmailBaseOptions;

/// React Email(`web/emails/`)から `just emails` で焼いた静的 HTML。{{var}} は `render` で置換。
/// 生成物だが include_str! のため commit する(変更時は `just emails` で再生成)。
pub const TPL_OWNER_REMOVE: &str = include_str!("mail/templates/owner_remove.html");
pub const TPL_DISK_ALERT: &str = include_str!("mail/templates/disk_alert.html");
pub const TPL_ACTION_CODE: &str = include_str!("mail/templates/action_code.html");

/// HTML + テキスト fallback でメールを送る。宛先が空 / key 未設定なら送らず Ok(退路)。
pub async fn send(
    state: &AppState,
    to: &[String],
    subject: &str,
    html: &str,
    text: &str,
) -> anyhow::Result<()> {
    if to.is_empty() {
        tracing::warn!(subject, "メール宛先が空 — 送信せず(owner 未設定?)");
        return Ok(());
    }
    let cfg = &state.config;
    let (Some(key), Some(from)) = (&cfg.resend_api_key, &cfg.mail_from) else {
        // dev / 未契約:実送しない。**本文は log に出さない**(検証コード等の秘密が漏れる)。
        tracing::info!(
            subject,
            recipients = to.len(),
            "[mail:dropped] RESEND_API_KEY 未設定のため送信しない"
        );
        return Ok(());
    };

    let opts = CreateEmailBaseOptions::new(from.as_str(), to.to_vec(), subject)
        .with_html(html)
        .with_text(text);
    // 既存の state.http(redirect なし + 10s タイムアウト)を resend に渡す。毎回 client を
    // 建て直さず、Resend が無反応でも 10s で諦める(告警 tick / コード請求を吊らせない)。
    Resend::with_client(key, state.http.clone())
        .emails
        .send(opts)
        .await
        .map_err(|e| anyhow::anyhow!("Resend 送信に失敗: {e}"))?;
    tracing::info!(subject, to = ?to, "メール送信 OK");
    Ok(())
}

/// HTML テンプレの `{{key}}` を **HTML エスケープした値**で置換する(新 crate 無し)。
/// テンプレは React Email で焼いた静的 HTML、動的箇所だけ {{key}} で空けてある。
pub fn render(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = template.to_string();
    for (key, value) in vars {
        out = out.replace(&format!("{{{{{key}}}}}"), &escape_html(value));
    }
    out
}

/// HTML 文脈に値を差し込む前の最小エスケープ(属性値・テキスト両用)。
fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 各テンプレを「想定する全変数」で render し、`{{ }}` が残らないことを保証する。
    /// テンプレに新しい占位符が増えて呼び出し点 / ここの変数表が漏れると即落ちる
    /// (= 未充填の {{x}} がメールに出る回帰を機械で検知。`just emails` 後もこれで担保)。
    #[test]
    fn templates_have_no_unfilled_placeholders() {
        let cases: &[(&str, &[(&str, &str)])] = &[
            (TPL_OWNER_REMOVE, &[]),
            (
                TPL_DISK_ALERT,
                &[
                    ("accent", "#f5c31c"),
                    ("pct", "80"),
                    ("level", "warn"),
                    ("warn", "80"),
                    ("critical", "95"),
                    ("path", "/srv/volumes"),
                ],
            ),
            (
                TPL_ACTION_CODE,
                &[
                    ("code", "123456"),
                    ("kind", "service"),
                    ("action", "delete"),
                    ("ttl", "10"),
                ],
            ),
        ];
        for (tpl, vars) in cases {
            let out = render(tpl, vars);
            assert!(!out.contains("{{"), "未充填の {{ がテンプレに残っている");
            assert!(!out.contains("}}"), "未充填の }} がテンプレに残っている");
        }
    }
}
