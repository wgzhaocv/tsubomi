# PgBouncer は channel binding を喋らない —— 決定的事実(実測 + 一次出典)

**一行**:tsubomi の pgbouncer(`edoburu/pgbouncer:v1.25.2-p0`)は **SCRAM-SHA-256-PLUS / channel binding を客户端に提供しない**。これは隔離実測 + PgBouncer 公式 changelog/NEWS/GitHub issue の**三重確証**。→ 第1ラウンド調査([[paas-db-public-relay-調査]] = `paas-db-public-relay-調査.md`)が「IP 白名単を捨てる根拠」とした **channel binding は使えない**。結論は `paas-db-public-relay-結論.md`。

> 信頼度:**最高**(実機実測 + 一次ソース複数が一致、反証票も整合)。2026-06-20。

---

## 1. 何が確定したか

クライアントが `channel_binding=require` で **pgbouncer に**接続すると **必ず失敗する**。pgbouncer は SCRAM-SHA-256(無印)しか提供せず、`-PLUS` を提供しないため。

⚠ **混同注意**:これは **「pgbouncer の client 側 channel binding」**の話。**Postgres 本体は** channel binding(SCRAM-SHA-256-PLUS / `tls-server-end-point`)を**サポートする**。両者は別物。tsubomi の公開経路は pgbouncer 終端なので、効くのは「pgbouncer の有無」のほう = **無い**。

## 2. 実測(隔離テスト・本番無関係)

同じ pgbouncer イメージ(`edoburu/pgbouncer:v1.25.2-p0`)で、自己署名証明書 + 静的 userlist(`cbuser`)+ `client_tls_sslmode=require` の使い捨てコンテナを立て、psql(libpq 18)で接続:

```
channel_binding=require → psql: error: ... channel binding is required,
   but server did not offer an authentication method that supports channel binding
channel_binding=disable → 認証通過(偽バックエンドに繋ぎに行き timeout。認証エラーではない)
```

`require` は **channel binding 交渉の段階で失敗**、`disable` は**素の SCRAM 認証を通過**。差は channel binding そのもの → **pgbouncer は -PLUS を出していない**。本番データ/役割には一切触れず、テスト後即破棄。

> 補足:本番 pgbouncer に偽ユーザで `channel_binding=require` を投げると `FATAL: no such user` が返り、これは **ユーザ照合で短路**して channel binding 交渉まで到達しないため**判定に使えない**(だから隔離テストで真ユーザを用意した)。

## 3. 一次出典(全て 3-0 確証)

- **PgBouncer changelog / NEWS.md**(1.12.0、2019-10-17):
  > "Accept SCRAM channel binding enabled clients. Previously, a client supporting channel binding (PostgreSQL 11+) would get a connection failure ... **(PgBouncer does not support channel binding. This change just fixes support for clients that offer it.)**"
  → 1.12.0 は「cb 対応クライアントを**受け入れる**(蹴らない)」だけ。**pgbouncer 自身は cb を行わない**。
- **changelog**:1.13.0〜**1.25.2(2026-05-08)まで channel binding 追加は無し**。SCRAM 関連の後続変更は 1.25.0 の性能改善のみ。
- **GitHub issue #522**:1.14 で `channel_binding=require` → `channel binding is required, but server did not offer ...`(= 実測と同一の文言)。
- **反証で逆に補強(0-3 で棄却)**:「SSL ビルドのサーバなら -PLUS を出す」という一般論は **pgbouncer には当てはまらない**。pgbouncer は OpenSSL 3.5.6 込みでビルドされている(`pgbouncer -V` で確認)のに -PLUS を出さない = **SSL の有無ではなく、実装していないだけ**。
- **PostgreSQL SASL ドキュメント**:channel binding = SCRAM-SHA-256-PLUS / `tls-server-end-point`、サーバ証明書を SCRAM 交換に混入して MITM/中継攻撃を防ぐ仕組み(これ自体は正しい。ただし**それを行う主体が pgbouncer には無い**)。

## 4. tsubomi への影響(詳細は結論ファイル)

- 配る接続文字列に **`channel_binding=require` を入れてはいけない**(接続が必ず失敗する)。
- 「IP 白名単を捨てても channel binding が MITM を防ぐ」という第1ラウンドの主柱は**成立しない**。残る MITM 防御は **クライアント側の `sslmode=verify-full` 依存**(サーバ側で強制不能)。
- → これが「IP 白名単を本当に捨てるか」の再考につながる。`paas-db-public-relay-結論.md` を参照。

## 5. 変通(もし channel binding をどうしても使いたい場合)

- pgbouncer を経由せず **Postgres 本体**に直結すれば cb は効く(が、プール無し + tenant DB 直曝で却下)。
- pgbouncer の前段に **channel binding 対応の代理**を置く(Neon は自前 Rust proxy)。OSS では PgCat / Supavisor が候補だが、**client 側 cb 対応かは本調査で未確認**(残課題)。単一ホストの小規模 PaaS には過剰の公算大。
