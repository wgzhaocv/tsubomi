//! TLS ClientHello から SNI(server_name 拡張)を取り出す最小パーサ。
//!
//! 闸门は TLS を**終端しない** — ClientHello は平文なので、その中の SNI を
//! 覗くだけ(TLS 本体は下流の pgbouncer が終端する)。
//!
//! 設計の一線:**fail-closed**。途中で切れた / 壊れた / 想定外の入力では panic せず
//! `None` を返し、呼び出し側はそれを「拒否」と解釈する。境界チェックを全分岐に掛け、
//! 宣言長は**厳密に**消費する(尾部にゴミがある畸形 ClientHello も拒否 = 闸门を第一の
//! フィルタとして弱めない。codex review [中] 指摘)。

/// バイト列を前から読むカーソル。範囲外アクセスは黙って `None`(panic しない)。
struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn u8(&mut self) -> Option<u8> {
        let b = *self.buf.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }

    fn u16(&mut self) -> Option<usize> {
        let hi = self.u8()? as usize;
        let lo = self.u8()? as usize;
        Some((hi << 8) | lo)
    }

    fn u24(&mut self) -> Option<usize> {
        let a = self.u8()? as usize;
        let b = self.u8()? as usize;
        let c = self.u8()? as usize;
        Some((a << 16) | (b << 8) | c)
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let s = self.buf.get(self.pos..end)?;
        self.pos = end;
        Some(s)
    }

    fn skip(&mut self, n: usize) -> Option<()> {
        self.take(n).map(|_| ())
    }

    fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }
}

/// ClientHello ハンドシェイクメッセージ(TLS record の**中身**。先頭は handshake type、
/// record ヘッダ 5B は含まない)から最初の host_name SNI を取り出す。
///
/// 見つからない / 壊れている / SNI 拡張が無い / 宣言長と実体が食い違う場合は `None`
/// (= 呼び出し側で拒否)。全拡張を走査して構造を検証してから SNI を返す。
///
/// 注:ClientHello が複数 record に断片化している場合は body が途中で尽きて `None` に倒れる
/// (= 拒否)。実在の libpq / openssl クライアントは ClientHello を 1 record で送るので実害なし。
pub fn parse_sni(handshake: &[u8]) -> Option<String> {
    let mut r = Reader::new(handshake);
    // handshake header: type(1) == 0x01 (ClientHello), length(3)
    if r.u8()? != 0x01 {
        return None;
    }
    let hs_len = r.u24()?;
    let body = r.take(hs_len)?;

    let mut r = Reader::new(body);
    r.skip(2)?; // client_version
    r.skip(32)?; // random
    let sid_len = r.u8()? as usize; // legacy_session_id
    r.skip(sid_len)?;
    let cs_len = r.u16()?; // cipher_suites
    r.skip(cs_len)?;
    let cm_len = r.u8()? as usize; // legacy_compression_methods
    r.skip(cm_len)?;
    // extensions は ClientHello の最後のフィールド。これより後に body が残っていたら畸形。
    let ext_total = r.u16()?;
    let exts = r.take(ext_total)?;
    if r.remaining() != 0 {
        return None;
    }

    // 全拡張を走査し構造を検証(途中に壊れた拡張があれば拒否)。SNI は最初の 1 つを採る。
    let mut er = Reader::new(exts);
    let mut sni = None;
    while er.remaining() > 0 {
        let ext_type = er.u16()?;
        let ext_len = er.u16()?;
        let ext_data = er.take(ext_len)?;
        if ext_type == 0x0000 && sni.is_none() {
            // server_name 拡張(RFC 6066)。構造が壊れていれば None 伝播 = 拒否。
            sni = Some(parse_server_name(ext_data)?);
        }
    }
    sni
}

/// server_name 拡張の中身から最初の host_name(name_type == 0)を取り出す。
/// list 全体を走査して構造を検証し、宣言長と食い違えば `None`。
fn parse_server_name(data: &[u8]) -> Option<String> {
    let mut r = Reader::new(data);
    let list_len = r.u16()?;
    let list = r.take(list_len)?;
    if r.remaining() != 0 {
        return None; // server_name_list の後にゴミ = 畸形
    }

    let mut lr = Reader::new(list);
    let mut host = None;
    while lr.remaining() > 0 {
        let name_type = lr.u8()?;
        let name_len = lr.u16()?;
        let name = lr.take(name_len)?;
        if name_type == 0x00 && host.is_none() {
            // host_name。SNI は A-label(ASCII)前提。非 UTF-8 は拒否扱い(None)。
            host = Some(std::str::from_utf8(name).ok()?.to_string());
        }
    }
    host
}

#[cfg(test)]
mod tests {
    use super::*;

    /// server_name 拡張のバイト列(type+len+data)を組む。
    fn server_name_ext(sni: &str) -> Vec<u8> {
        let host = sni.as_bytes();
        // server_name_list の 1 エントリ: name_type(0) + name_len(2) + name
        let mut entry = vec![0x00u8];
        entry.extend_from_slice(&(host.len() as u16).to_be_bytes());
        entry.extend_from_slice(host);
        // 拡張 data: list_len(2) + list
        let mut sn = Vec::new();
        sn.extend_from_slice(&(entry.len() as u16).to_be_bytes());
        sn.extend_from_slice(&entry);
        // 拡張: type(2)=0x0000 + len(2) + data
        let mut ext = Vec::new();
        ext.extend_from_slice(&0x0000u16.to_be_bytes());
        ext.extend_from_slice(&(sn.len() as u16).to_be_bytes());
        ext.extend_from_slice(&sn);
        ext
    }

    /// 与えた拡張ブロックを内包する最小 ClientHello(record ヘッダ抜き)を組む。
    fn client_hello_with_exts(exts: &[u8]) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&[0x03, 0x03]); // client_version
        body.extend_from_slice(&[0u8; 32]); // random
        body.push(0x00); // session_id len = 0
        body.extend_from_slice(&2u16.to_be_bytes()); // cipher_suites len
        body.extend_from_slice(&[0x00, 0x2f]); // 1 suite
        body.push(0x01); // compression_methods len
        body.push(0x00); // null
        body.extend_from_slice(&(exts.len() as u16).to_be_bytes()); // extensions len
        body.extend_from_slice(exts);
        // handshake: type(1)=0x01 + len(3) + body
        let mut hs = vec![0x01u8];
        let l = body.len();
        hs.push((l >> 16) as u8);
        hs.push((l >> 8) as u8);
        hs.push(l as u8);
        hs.extend_from_slice(&body);
        hs
    }

    fn client_hello(sni: &str) -> Vec<u8> {
        client_hello_with_exts(&server_name_ext(sni))
    }

    #[test]
    fn extracts_sni() {
        let hs = client_hello("db.tsubomi-app.com");
        assert_eq!(parse_sni(&hs).as_deref(), Some("db.tsubomi-app.com"));
    }

    #[test]
    fn extracts_short_and_long_sni() {
        assert_eq!(parse_sni(&client_hello("a")).as_deref(), Some("a"));
        let long = "x".repeat(253);
        assert_eq!(parse_sni(&client_hello(&long)).as_deref(), Some(long.as_str()));
    }

    #[test]
    fn truncation_never_panics_and_is_rejected() {
        // 完全な ClientHello のあらゆる前置切片で panic しない。最後まで揃う前は None。
        let hs = client_hello("db.tsubomi-app.com");
        for i in 0..hs.len() {
            assert_eq!(parse_sni(&hs[..i]), None, "prefix len {i} should not parse");
        }
        assert!(parse_sni(&hs).is_some());
    }

    #[test]
    fn wrong_handshake_type_rejected() {
        let mut hs = client_hello("db.tsubomi-app.com");
        hs[0] = 0x02; // ServerHello 等
        assert_eq!(parse_sni(&hs), None);
    }

    #[test]
    fn empty_input_rejected() {
        assert_eq!(parse_sni(&[]), None);
        assert_eq!(parse_sni(&[0x01]), None);
        assert_eq!(parse_sni(&[0x16, 0x03, 0x03]), None);
    }

    #[test]
    fn client_hello_without_sni_rejected() {
        // 拡張ブロックを空にした ClientHello → SNI 無し → None。
        assert_eq!(parse_sni(&client_hello_with_exts(&[])), None);
    }

    #[test]
    fn declared_length_overrun_rejected() {
        // handshake length が実バイト数より大きい → take が None → 拒否。
        let mut hs = client_hello("db.tsubomi-app.com");
        let real = ((hs[1] as usize) << 16) | ((hs[2] as usize) << 8) | hs[3] as usize;
        let lie = real + 1;
        hs[1] = (lie >> 16) as u8;
        hs[2] = (lie >> 8) as u8;
        hs[3] = lie as u8;
        assert_eq!(parse_sni(&hs), None);
    }

    #[test]
    fn trailing_garbage_after_extensions_rejected() {
        // body 末尾(extensions の後)にゴミを 1 バイト足し、hs_len を +1 → 厳密消費で拒否。
        let mut hs = client_hello("db.tsubomi-app.com");
        hs.push(0xff);
        let real = ((hs[1] as usize) << 16) | ((hs[2] as usize) << 8) | hs[3] as usize;
        let lie = real + 1;
        hs[1] = (lie >> 16) as u8;
        hs[2] = (lie >> 8) as u8;
        hs[3] = lie as u8;
        assert_eq!(parse_sni(&hs), None);
    }

    #[test]
    fn malformed_trailing_extension_rejected() {
        // 有効な SNI 拡張のあとに、宣言長が溢れる壊れた拡張を置く → 全走査で拒否。
        // (旧実装は SNI を見つけた時点で早期 return し、これを取りこぼしていた)
        let mut exts = server_name_ext("db.tsubomi-app.com");
        exts.extend_from_slice(&0x0017u16.to_be_bytes()); // 適当な ext type
        exts.extend_from_slice(&0xffffu16.to_be_bytes()); // len=65535 だが data 無し
        let hs = client_hello_with_exts(&exts);
        assert_eq!(parse_sni(&hs), None);
    }

    #[test]
    fn server_name_without_host_type_rejected() {
        // server_name list はあるが host_name(type 0)エントリが無い → SNI 無し扱いで拒否。
        let mut entry = vec![0x01u8]; // name_type = 1(未定義)
        entry.extend_from_slice(&3u16.to_be_bytes());
        entry.extend_from_slice(b"abc");
        let mut sn = Vec::new();
        sn.extend_from_slice(&(entry.len() as u16).to_be_bytes());
        sn.extend_from_slice(&entry);
        let mut ext = Vec::new();
        ext.extend_from_slice(&0x0000u16.to_be_bytes());
        ext.extend_from_slice(&(sn.len() as u16).to_be_bytes());
        ext.extend_from_slice(&sn);
        assert_eq!(parse_sni(&client_hello_with_exts(&ext)), None);
    }
}
