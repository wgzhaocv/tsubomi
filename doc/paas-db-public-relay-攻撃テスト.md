# 公開DB暴露面の攻撃テスト —— 結果と発見(2026-06-20)

frp 中継で公開した Postgres(`db.tsubomi-app.com:6432`)に対し、**自インフラへの認可済み攻撃テスト**を一通り実施した記録。背景は [[paas-db-public-relay-結論]]、関連 [[tsubomi-db-public-relay]]。

> 対象:ConoHa VPS(133.88.123.119)+ 香橙派の frp/pgbouncer/pg-tenant。テスト用テナント DB を 2 つ作って実施後に削除。サーバは **v25**(sslmode 内外分離 + 外部 verify-full 既定)で稼働中。

---

## 1. テスト結果マトリクス

| # | テスト | 期待 | 実測 | 判定 |
|---|---|---|---|---|
| 港 | 港スキャン(VPS 外側) | 22/6432/7000 のみ | 21/25/53/80/443/3306/**5432**/8080/9090 全 closed・22/6432/7000 のみ open | ✅ |
| SSH | パスワード login 強制 | 拒否 | サーバは `publickey` のみ提示 → `Permission denied (publickey)`。`sshd -T`:passwordauth=**no**, kbdinteractive=**no**, emptypw=no | ✅ |
| A1 | `sslmode=disable` | 拒否 | `FATAL: SSL required`(pgbouncer `client_tls_sslmode=require`) | ✅ |
| A2 | `verify-full` で IP 直指(SAN 不一致) | 拒否 | `server certificate for "db.tsubomi-app.com" does not match host name "133.88.123.119"` | ✅ MITM 防御の核 |
| A3 | `verify-full` で正規ホスト | 接続+読書 | `E2E OK / PostgreSQL 18.4`、CREATE/INSERT/SELECT/DROP 成功(CA 配布不要・system 信頼ストア) | ✅ |
| B1 | frp 制御港 7000 へ非 frp/無 mTLS | 拒否/無応答 | 非 frp バイトは静黙切断。frps は `transport.tls.force` + mTLS(`trustedCaFile`)+ token | ✅ |
| C1 | 正規ロール + 誤パスワード | 認証失敗 | `FATAL: SASL authentication failed`(SCRAM) | ✅ |
| C2 | 存在しないロール | 拒否 | `FATAL: no such user` | ✅ |
| C3 | 誤パスワード連打 ×10 | (限流の有無) | 全て SASL failed・封禁なし・3s(≈0.3s/回)= **DB 層に fail2ban/限流なし** | ⚠ 後述 |
| **D1** | **跨租户**:A の資格で B の DB | 拒否 | SCRAM は通るが `FATAL: permission denied for database "db_…"` = **CONNECT 権限層で隔離** | ✅✅ |
| E1 | ロール権限属性 | 非特権 | superuser/createdb/createrole/bypassrls **全 f** | ✅ |
| E2 | `COPY ... TO PROGRAM`(RCE) | 拒否 | `permission denied`(要 `pg_execute_server_program`) | ✅ |
| E3 | `pg_read_file('/etc/passwd')` | 拒否 | `permission denied for function pg_read_file` | ✅ |
| E4 | `pg_authid`(全ロールのハッシュ) | 拒否 | `permission denied for table pg_authid` | ✅ |
| E5 | `CREATE DATABASE` / `CREATE ROLE` | 拒否 | 両方 `permission denied` | ✅ |
| E6 | pgbouncer auth_query 関数で他ハッシュ吸出 | 拒否 | `function public.pgbouncer_get_auth does not exist`(テナント DB に露出せず) | ✅ |
| E7 | DB 名の列挙(`pg_database`) | (情報露出面) | count/名前は見える(world-readable)が**接続は D1 で不可** | ⚠ 受容済 |
| E8 | `SET ROLE tsubomi_admin`(提権) | 拒否 | `permission denied to set role` | ✅ |
| 密 | パスワード強度 | 高エントロピー | 32 文字 URL-safe base64 ≈ **192 bit** | ✅ |

---

## 2. 発見:主干防御は効いている

- **暗号は必須かつ端到端**:非 TLS は `SSL required` で拒否、verify-full の**ホスト名検証**が IP 直指/別ホストを弾く = 能動 MITM はクライアントが verify-full を使う限り防げる(その verify-full が **v25 から既定の接続文字列**になった = Neon との差が縮まった)。
- **テナント隔離は権限層で硬い**:正規資格でも他テナント DB は CONNECT 拒否。SCRAM 認証を通過した後に DB-ACL で落ちる = 二段で正しい。
- **ロールは完全に閉じ込め**:非 superuser・RCE(COPY TO PROGRAM)不可・サーバファイル読取不可・ハッシュ読取不可・DB/ロール作成不可・提権不可。テナントは自 DB の中だけ。
- **攻撃面が最小**:VPS 外側は 22/6432/7000 のみ。5432(既定 pg)も web(80/443)も無し。SSH はパスワード login 完全無効。

## 3. 残課題(いずれも**侵入ではなく纵深防御**の話)

| 優先 | 課題 | 影響 | 推奨 |
|---|---|---|---|
| 中 | **DB 港(6432)に限流/fail2ban なし**(C3) | 暴力破解の試行が打ち放題 | エッジ(VPS nftables)で送信元 IP 毎の**接続レート制限**。pgbouncer の認証失敗で fail2ban は本トポロジでは難(VPS は frp トンネルしか見えず実 client IP を持たない・PROXY protocol は pgbouncer 非対応)= **エッジの接続レート制限が現実解** |
| 中 | **`max_client_conn=1000` + エッジ無制限** | 接続枯渇 DoS の面 | エッジで送信元毎の**同時接続数上限**(nftables `ct count`) |
| 低 | `PermitRootLogin yes` | password 全無効なので実質 key 専用 | 明示的に `prohibit-password` |
| 低 | `pg_database` で DB 名列挙可(E7) | 名前のみ露出(接続不可) | 受容(名前は乱数 shortid)。気になれば pg_database への列挙制限は副作用大なので非推奨 |
| 既知 | verify-full を使わず `require` 止まりのクライアント | 能動 MITM 残留(pgbouncer は channel binding 非対応 → [[paas-db-public-relay-pgbouncer-cb]]) | 既定を verify-full にした(v25)。`require` への意図的降格は利用者責任 |

## 4. 総合判定

公開 DB 暴露面は**中核防御(TLS 強制・SCRAM+192bit パスワード・厳格なテナント隔離・閉じ込めた非特権ロール・RCE/ファイル読取/提権の経路なし・最小攻撃面)が揃っている**。実弾テストで侵入経路は見つからなかった。残る指摘は**限流/DoS の纵深防御**であって、データ侵害ではない。最優先の現実的改善は **VPS エッジ(nftables)での接続レート制限 + 同時接続数上限**。
