# 公開DB(frp 中継)—— 結論と実装方針

**この一枚で足りる要約**。詳細根拠は同プレフィックスの 3 ファイル:
- [[paas-db-public-relay-調査]] — Neon の公開暴露の仕組み(第1ラウンド。**channel binding を要石とした部分は本書で訂正**)
- [[paas-db-public-relay-pgbouncer-cb]] — pgbouncer は channel binding を喋らない(実測+一次出典)
- [[paas-db-public-relay-frp]] — frp の透過挙動と加固

> 2026-06-20。背景 [[tsubomi-db-public-relay]]。本書は方針提案であり最終確定設計ではない(下の「判断ポイント」はユーザのリスク判断が要る)。

---

## 1. 前提の訂正(ここが今回の肝)

第1ラウンドは「**`channel_binding=require` が MITM を防ぐから IP 白名単を捨ててよい**」を結論の柱にしていた。
**この柱は崩れた**:tsubomi の pgbouncer(1.25.2)は **channel binding を客户端に提供しない**(実測 + 公式 changelog/issue で三重確証 → [[paas-db-public-relay-pgbouncer-cb]])。

従って公開時の防御は実質 **次の 3 つだけ**:
1. **全接続 TLS 必須**(`client_tls_sslmode=require`、既に設定済み)
2. **ランダムな長いロール別パスワード**(辞書攻撃免疫。tsubomi の at-rest 暗号 + app/human 双 role と整合)
3. **クライアント側の `sslmode=verify-full`**(証明書チェーン+ホスト名検証で MITM を防ぐ)

問題は **3 はサーバ側で強制できない**こと。pgbouncer は「TLS を要求」できても「クライアントに証明書検証を要求」できない。`channel_binding=require` という保険(検証をサボるクライアントも守る)が **pgbouncer では使えない**。Neon はここを channel binding で埋めているが、**tsubomi は埋められない** = Neon よりこの一点だけ弱い。

## 2. 脅威モデル(IP 白名単を捨てた場合)

| 脅威 | 防御 | 状態 |
|---|---|---|
| 受動盗聴(回線傍受) | TLS 必須 | ✅ 防げる(verify-full 不要) |
| ブルートフォース/credential stuffing | ランダム長パスワード(+ 連接上限 + fail2ban) | ✅ ほぼ防げる |
| **能動 MITM(on-path 攻撃者)** | **`verify-full` 依存**。channel binding の保険なし | ⚠ **verify-full を使わないクライアントは脆弱** |

能動 MITM は「クライアント↔VPS の経路上に攻撃者がいる」前提(汚染 WiFi・BGP ハイジャック・悪性 ISP 等)= **ハードルは高いが実在**する脅威。`verify-full` + CA を使うクライアントは防げる。`require` 止まりのクライアントは防げない。

## 3. 選択肢と推奨

- **A. シンプル公開(verify-full 必須化 + 残留リスク受容)** ← 推奨の土台
  配る接続文字列を **`?sslmode=verify-full`** にし、**CA 証明書を web UI から配布**。ランダムパスワード。`channel_binding=require` は**入れない**(pgbouncer で失敗するため)。Neon の推奨と同等(channel binding の保険だけ無い)。
- **D. 運用加固(A に足す)** ← 推奨に同梱
  VPS エッジで **連接レート制限(iptables/nftables)**・**低い `max_client_conn`**・**fail2ban**(実 client IP は VPS エッジで生に見える → [[paas-db-public-relay-frp]] §2)。frp は token + transport.tls + allowPorts(6432 限定)+ 新しめ版。
- **B. IP 白名単を残す(纵深防御)** ← 保守的退路
  tsubomi の既存 `ip_allow_entries` 機構を活かし、**per-DB で opt-in**。既定は「公開 + verify-full」、owner が必要なら既知 IP に絞れる。能動 MITM 残留リスクが**受容できないなら**これ。NeonDB 式「どこからでも」UX とはトレードオフ。
- **C. cb 対応代理を前段に置く** ← 非推奨
  PgCat/Supavisor の client 側 cb 対応は**未確認**、単一ホスト小規模には過剰。今回は採らない。

### 推奨:A + D を既定とし、B(per-DB opt-in 白名単)を残す
理由:tsubomi は小規模社内 PaaS で、現実的脅威(verify-full 推奨エンドポイントに対する能動 MITM)は低い。verify-full + ランダムパスワード + 運用加固で実用上十分。ただし「検証サボりクライアントの MITM 保険が無い」を**正直に受け入れる**判断が要るので、不安なら per-DB の IP 白名単 opt-in を残しておけば後から締められる。

## 4. 推奨アーキテクチャ

```
 クライアント (任意の psql/app)
   │  postgres://c_xxx:pw@db.tsubomi-app.com:6432/db?sslmode=verify-full  (+ CA 同梱)
   │  ← TLS は pgbouncer と端到端(frp は素通し)
   ▼
 ConoHa VPS  frps  (公開 :6432 / 制御 :bindPort)
   │  token + transport.tls、allowPorts=6432、エッジで連接レート制限 + fail2ban
   ▼  frp トンネル(裸 TCP 透過)
 香橙派  frpc → 127.0.0.1:6432 (pgbouncer, client_tls_sslmode=require)
   ▼
 pgbouncer(client TLS 終端・SCRAM-SHA-256)→ pg-tenant
```

## 5. 実装チェックリスト(着手時)

- [ ] **証明書**:pgbouncer の `server.crt` の CN/SAN に **`db.tsubomi-app.com`** を含める(verify-full の前提)。署名 CA を決め、**その CA/cert を web UI からダウンロード可能に**。
- [ ] **DNS**:CF に `db.tsubomi-app.com` の **DNS-only(灰云)A 記録 → 133.88.123.119**(橙云不可=CF 代理は HTTP のみ)。
- [ ] **frp**:VPS に frps(>=0.68.1、`auth.token`、`transport.tls.enable`/`force`、`allowPorts=[{single=6432}]`)。Pi に frpc(`127.0.0.1:6432`、`remotePort=6432`)。
- [ ] **VPS 安全組/firewall**:TCP 6432 を 0.0.0.0/0(A 採用時)。frps 制御ポートは Pi egress に絞る。連接レート制限(nftables)。
- [ ] **fail2ban**:VPS エッジで失敗認証/連接過多を見て ban(実 IP はエッジで生)。
- [ ] **pgbouncer**:`max_client_conn` を公開前提で見直し、idle/`query_wait_timeout` 確認。`client_tls_sslmode` は `require` 維持(可能なら検討:より厳格な設定)。
- [ ] **tsubomi 本体**:`TSUBOMI_DB_PUBLIC_HOST=db.tsubomi-app.com`、`TSUBOMI_DB_PUBLIC_PORT=6432`、`TSUBOMI_DB_PUBLIC_ENABLED=true`。接続文字列に `?sslmode=verify-full` を付与。**`channel_binding=require` は付けない**。値は容器起動時解決 = **server 再デプロイで初めて効く**。
- [ ] (B 採用時)`ip_allow_entries` を per-DB opt-in として UI/後端に通す。

## 6. 残課題(本ラウンド未確認 = 偽ではない。session limit で検証中断、4:50 JST 後に再検証可)

1. **Neon proxy 層の認証前レート制限/ブルートフォース節流**の有無とパラメータ(一次確認できず)。tsubomi のエッジ加固の参考になるはず。
2. **scale-to-zero/suspend × 暴露面**:未認証フラッドで wake を強制できるか(tsubomi は scale-to-zero 無しなので影響小、だが Neon の設計は参考)。
3. **Neon IP Allowlist(Scale プラン有料)/ Protected Branches / Private Networking(AWS PrivateLink)**の正確な機構(出典はあるが未検証)。B を選ぶ際の参考。
4. **Supabase の pgbouncer 向け fail2ban フィルタ**(`supabase/postgres` に実在の模様、出典あり・未検証)。D の fail2ban 実装の雛形になる。pgbouncer の失敗ログ文字列(`password authentication failed` / `no such user`)が ban のキー。
5. **PgCat / Supavisor の client 側 channel binding 対応**(C を将来検討するなら要確認)。
6. **frp CVE-2026-40910** の正確な影響版範囲(HTTP vhost 限定・TCP 非該当の見込みだが部分確認)。
