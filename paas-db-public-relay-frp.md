# frp 中継の挙動と安全加固 —— tsubomi 公開DB用

**一行**:frp の TCP 代理は**裸バイト透過**で、クライアント↔pgbouncer の TLS は**端到端で保たれる**(frp の `transport.tls` は frpc↔frps の**外層トンネル**で、運ぶ payload を書き換えない)。実 client IP は **PROXY protocol** で後端へ渡せるが、**pgbouncer はそれを解さない**ので使い所に注意。関連:[[paas-db-public-relay-結論]] / [[paas-db-public-relay-pgbouncer-cb]]。

> 信頼度:frp の各挙動はいずれも **3-0 確証**(gofrp 公式ドキュメント + frp の `*_full_example.toml`)。CVE 項のみ部分確認。2026-06-20。

---

## 1. TLS は端到端で生き残る 【確証 3-0】

- frp の `transport.tls`(`frpc`↔`frps`)は **トンネルの外層暗号**。有効時「traffic between frpc and frps will be globally encrypted」。**proxied application payload を書き換えない**。
- → TCP 代理を通る pgbouncer の **client TLS ハンドシェイクはクライアントと pgbouncer の間で端到端**に成立する。frp は中の TLS に触れない。
- **含意**:
  - tsubomi が pgbouncer の client TLS をそのまま外へ出すのは正しい(VPS に二つ目の TLS 終端を置く必要なし。Neon と同形)。
  - 将来 pgbouncer の前段に **channel binding 対応の代理**を置いた場合でも、その代理の証明書に対する `tls-server-end-point` バインディングは **frp 透過越しでも成立する**(frp は内側 TLS を素通しするので、cb が壊れる心配は frp 由来では無い)。

## 2. 実 client IP の透過(PROXY protocol)【確証 3-0、ただし後端要件あり】

- frp は `transport.proxyProtocolVersion`(v1/v2)で**実クライアント IP を後端サービスへ** PROXY protocol で渡せる。
- ⚠ **落とし穴**:PROXY protocol は**後端が解せる必要がある**。**pgbouncer は inbound の PROXY protocol を解さない**ので、frpc が PROXY ヘッダを前置すると pgbouncer はそれをプロトコル異常として弾く。
- → tsubomi で実 client IP を使いたい(fail2ban / per-IP 制限 / 監査)なら現実的には:
  1. **VPS 側で見る**(`frps` の接続や iptables/nftables は**実クライアント IP を生で持つ**)。IP ベースの防御は VPS エッジに置くのが素直。← 推奨
  2. もしくは pgbouncer の前に **PROXY 対応 shim(haproxy 等)**を挟んで実 IP を解す(複雑化)。

## 3. frps 公網加固 【確証 3-0】

- **`auth.token` 必須**:frpc↔frps の登録を共有トークンで縛る(無いと誰でもトンネルを生やせる)。
- **`transport.tls.enable = true`**:トンネル層を TLS で暗号化(`certFile`/`keyFile`/`trustedCaFile`/`serverName`、`frps` 側は `transport.tls.force` で素の接続を拒否可)。
- **`allowPorts`**(範囲/単一ポート)+ **`maxPortsPerClient`**:frps が公開を許すポートを**6432 のみ**等に絞る。乱立防止。
- **bindAddr / 管理ポート**:frps の `webServer`(ダッシュボード)や `bindPort` を公網に晒さない/別途 firewall で絞る。
- ConoHa 安全組:公開する **TCP 6432**(と frps 制御 `bindPort`)だけを許可。制御ポートは Pi の egress IP に絞れればなお良い(Pi は外向き接続なので)。

## 4. 既知脆弱性 【部分確認】

- **CVE-2026-40910 / GHSA-pq96-pwvg-vrr9**:frp の**認証バイパス**。報告では **HTTP vhost の `routeByHTTPUser` 機能**に固有で、**TCP 代理・通常 HTTP 代理は非該当**、**0.68.1 で修正**とされる(本ラウンドは session limit で部分確認に留まる)。
  - tsubomi は **TCP 代理のみ**なので該当しない見込みだが、**新しめの frp(>= 0.68.1)を使う**こと。`routeByHTTPUser` は使わない。

## 5. tsubomi 向けまとめ

- frp は計画どおり **裸 TCP 透過**で使う(pgbouncer の TLS を端到端で出す)。✅
- 加固は **token + transport.tls + allowPorts(6432 限定)+ 新しめ版**。
- IP ベースの防御・監査は **VPS エッジ**側で(PROXY protocol を pgbouncer に向けない)。
- frp 由来で channel binding が壊れることは無い(が、そもそも pgbouncer が cb 非対応 = [[paas-db-public-relay-pgbouncer-cb]])。
