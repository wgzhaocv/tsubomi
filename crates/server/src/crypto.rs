//! at-rest 暗号化:XChaCha20-Poly1305。deploy key / DB パスワード / env 値のように
//! 「プラットフォームが原文を必要とする」秘密を**復元可能**に保存する(ハッシュに
//! できる session / token とは別 — tech-design §7。M1 では DB パスワードが最初の客)。
//!
//! 正直な境界:master key は同一ホスト上にある(/etc/tsubomi/master.key か env)。
//! 守れるのはバックアップ / dump の漏洩であって、ホスト陥落ではない。

use anyhow::{Context, Result, bail};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand::Rng;

/// XNonce の長さ(XChaCha20 の拡張 nonce)。保存形式は `nonce ‖ ciphertext+tag`。
const NONCE_LEN: usize = 24;

pub struct Cipher {
    inner: XChaCha20Poly1305,
}

impl Cipher {
    pub fn new(key: &[u8; 32]) -> Self {
        // key は常に 32 bytes(config が保証)なので new_from_slice は成功する。
        let inner = XChaCha20Poly1305::new_from_slice(key).expect("master key must be 32 bytes");
        Self { inner }
    }

    /// 平文を暗号化し `nonce(24) ‖ ciphertext+tag` を返す。nonce は毎回乱数。
    pub fn encrypt(&self, plaintext: &str) -> Result<Vec<u8>> {
        let mut nonce = [0u8; NONCE_LEN];
        rand::rng().fill_bytes(&mut nonce);
        let ct = self
            .inner
            .encrypt(&XNonce::from(nonce), plaintext.as_bytes())
            // aead::Error は中身を晒さない設計なので Display を包むだけ。
            .map_err(|e| anyhow::anyhow!("encrypt failed: {e}"))?;
        let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ct);
        Ok(out)
    }

    /// `encrypt` の逆。改竄 / 鍵違いは Poly1305 タグ検証で失敗する。
    pub fn decrypt(&self, blob: &[u8]) -> Result<String> {
        if blob.len() <= NONCE_LEN {
            bail!("ciphertext too short");
        }
        let (nonce, ct) = blob.split_at(NONCE_LEN);
        let nonce: [u8; NONCE_LEN] = nonce.try_into().expect("split_at guarantees length");
        let pt = self
            .inner
            .decrypt(&XNonce::from(nonce), ct)
            .map_err(|e| anyhow::anyhow!("decrypt failed (tampered or wrong key): {e}"))?;
        String::from_utf8(pt).context("decrypted bytes are not utf-8")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_cipher() -> Cipher {
        Cipher::new(&[7u8; 32])
    }

    #[test]
    fn roundtrip() {
        let c = test_cipher();
        let secret = "postgres://u:p@host:6432/db_abc?sslmode=disable";
        let blob = c.encrypt(secret).unwrap();
        assert_eq!(c.decrypt(&blob).unwrap(), secret);
        // nonce が乱数なので同じ平文でも毎回違う暗号文。
        assert_ne!(c.encrypt(secret).unwrap(), c.encrypt(secret).unwrap());
    }

    #[test]
    fn tamper_is_rejected() {
        let c = test_cipher();
        let mut blob = c.encrypt("seikret").unwrap();
        let last = blob.len() - 1;
        blob[last] ^= 0x01; // タグ or 本体を 1 ビット壊す
        assert!(c.decrypt(&blob).is_err());
    }

    #[test]
    fn wrong_key_is_rejected() {
        let blob = test_cipher().encrypt("seikret").unwrap();
        let other = Cipher::new(&[9u8; 32]);
        assert!(other.decrypt(&blob).is_err());
    }

    #[test]
    fn too_short_is_rejected() {
        assert!(test_cipher().decrypt(&[0u8; 10]).is_err());
    }
}
