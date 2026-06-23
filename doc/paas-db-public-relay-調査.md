# paas db-public 公開接続 — Neon 調査ノート(frp 中継方式の設計入力)

> ⚠ **【重要訂正 2026-06-20】** 本書 §4・§11 は `channel_binding=require` を「IP 白名単を捨てる要石」とした。
> **その後の実測+一次出典で、tsubomi の PgBouncer(1.25.2)は channel binding を客户端に提供しないと判明**(→ `paas-db-public-relay-pgbouncer-cb.md`)。
> **要石は成立しない**。修正後の結論・脅威モデル・実装方針は **`paas-db-public-relay-結論.md`** を正とする。本書は Neon の仕組み理解の資料として残すが、channel binding 依存の記述はそのまま採用しないこと。

**一行要旨**:Neon は公開 Postgres を **IP 許可リストではなく**「共有 Proxy + SNI ルーティング + 全接続 TLS 必須 + ランダムなロール別パスワード + channel binding」の多層で守っている。tsubomi が frp 中継で同じことをやるなら、**pgbouncer の client TLS を唯一の終端に保ち、配る接続文字列に `sslmode=verify-full` + `channel_binding=require` を必須化し、ランダム長パスワードだけを発行する**のが直輸入できる肝。Neon の SNI ルーティング/Proxy 機構は「多テナントを 1 つの共有 proxy に集約して捌く」ためのもので、**pgbouncer 1 台を指す frp 中継には不要**。

> メタ:2026-06-20 deep-research(6 角度・23 出典・110 主張抽出 → 上位 25 を 3 票対抗検証 → 22 確証/3 証伪 → 合成 15)。
> 信頼度凡例:**【確証 3-0】**=3 検証者全員支持/**【2-1】**=多数支持・割れあり/**【推論・中】**=一次出典で裏取りできず合成。
> 背景は [[tsubomi-db-public-relay]]([memory])・設計は `paas-db-public-design.md`。本書は **調査と示唆**であって確定設計ではない。

---

## 0. なぜ調べたか

tsubomi 本番は香橙派が Cloudflare Tunnel 配下 = **裸 TCP の公網入口が無い**ので、NeonDB 式の「接続文字列を任意の psql に貼れば繋がる」公開 DB ができない。解として **公網 IP の小 VPS(ConoHa)を frp で TCP 中継**にして、内網 pgbouncer:6432 を外へ出す。その際に合意した方針が「**IP 白名単を捨て、0.0.0.0/0 + 受限 pg 役割 + pgbouncer TLS**」(= NeonDB 式)。

ここで唯一の不安は「**IP 白名単を捨てて本当に安全か**」。tsubomi の設計思想は「隔離は仕組みで守る・規律に頼らない」で、IP 許可リスト(traefik 層)は一線として扱ってきた。だから「Neon は IP 白名単無しの公開でどう守っているか」を調べ、その手当てを tsubomi に移植できるか確かめるのがこの調査の目的。

---

## 1. 接続ルーティング:Proxy + SNI + endpoint ID 【確証 3-0】

- Neon は **endpoint ID をホスト名の先頭に埋め込み**(例 `ep-cool-darkness-123456.region.aws.neon.tech`)、これを **TLS の SNI 拡張**で運ぶ。理由は明快:**Postgres ワイヤプロトコルはドメイン名を一切運ばない**ので、共有 proxy はどのテナントの compute に繋ぐべきか他に知る術がない。
- 共有 **Neon Proxy** が TLS を終端し、SNI を読み、compute を引いて転送する。
- → **これは「多テナント fan-in」を捌くための機構**。tsubomi の frp は **1 つの pgbouncer** を指すだけなので、この SNI ルーティング/endpoint ID/自前 proxy は **作らなくてよい**(後述 §10)。

## 2. SNI 非対応クライアントの罠と回避策 【確証 3-0】

- libpq が SNI を送るようになったのは **Postgres 14(2021-09 リリース)**から。それ以前の libpq 同梱クライアントは SNI を送れず、Neon ではルーティング不能で `The endpoint ID is not specified` エラーになる。
- 回避策(段階的):
  1. **推奨**:接続文字列に `options=endpoint=<id>`(URL 形式では `options=endpoint%3D<id>`、`%3D` は `=`)。
  2. **最終手段**:パスワード欄に埋める `endpoint=<id>;<password>`(`;` を許さないクライアント=AWS DMS 等は `endpoint=<id>$<password>`)。
- **罠**【2-1】:パスワード欄方式は認証を **SCRAM-SHA-256 → 平文 password 認証へ降格**させる(平文をワイヤに流す。ただし TLS 内)。Neon は「`sslmode=verify-full` か channel binding を使う限り HTTPS 相当」と書くが、**この文言は自己矛盾**(channel binding は SCRAM-SHA-256-PLUS が前提で、平文降格と両立しない)。→ **Neon の主張であって独立した保証ではない**。tsubomi では §10 の通り、**パスワード埋め込み方式は採らない**ので無関係。

## 3. TLS 強制と sslmode / 証明書 【確証 3-0】

- Neon は **全接続で TLS 必須**、非 TLS は即拒否(`hostssl` レコードのみの pg_hba と同じ挙動)。`sslmode=disable` は通らない。
- `sslmode` は `require` / `verify-ca` / `verify-full` をサポート、**`verify-full` を最推奨**(暗号化 + サーバ証明書のホスト名検証)。
- 重要な落とし穴(PostgreSQL 公式 libpq-ssl Table 32.1 でも裏取り):
  - psql 既定の `prefer` は暗号化も認証も保証せず **MITM 脆弱**。
  - `require` は**暗号化はするがホスト名/署名 CA を検証しない** → 単体では MITM を防げない。
  - 真の MITM 防御 = **`verify-full` + ルート CA(`sslrootcert`)**。
  - (libpq は CRL を明示設定しない限り失効確認はしない。Neon の "revocation checks" 表現はやや緩い。)

## 4. channel binding —— 公開暴露の要石 【確証 3-0 / 一部 2-1】

- `channel_binding=require` は **SCRAM-SHA-256-PLUS** でクライアント/サーバを相互認証する。**サーバ証明書を SCRAM 交換にハッシュ混入**するので、証明書をコピーしただけの MITM は**対応する秘密鍵を持たず**認証が失敗する。
- 効能:**`sslmode=require` 単体(CA/証明書検証なし)でも MITM を防げる**。channel binding 型は `tls-server-end-point`、**TLS 接続でのみ利用可**=まさに公開暴露の状況に効く。
- `channel_binding=require` は -PLUS が提供されないと**失敗する**ので、**沈黙の降格を塞ぐ**。
- → **これが「IP 白名単無しで 0.0.0.0/0 に晒す」を正当化できる最大の単一コントロール**。クライアント側が CA を持たずとも(verify-full が難しい相手でも)MITM 耐性が立つ。

## 5. 凭据モデルとローテーション 【確証 3-0】

- **ロール別**で、作成時に**サーバ生成のランダムパスワード**(`npg_` 接頭辞)。ランダム=辞書攻撃に耐性。SQL の `CREATE ROLE ... PASSWORD` で作る場合は**60bit エントロピー下限**。
- ローテーション 2 系統:
  - (a) 専用エンドポイント `POST .../roles/{role}/reset_password`:**新パスワードを生成して返す**(ユーザ指定値は受けない)。**旧パスワードは最後の非同期オペが終わるまで有効**(切替は非アトミック・compute 接続は切れる)。
  - (b) ユーザ指定の `ALTER USER ... WITH PASSWORD`(SQL)。
- **tsubomi との一致**:tsubomi の「rotate は DB 先 → 収束、再デプロイで初めて効く」という背骨は、Neon の**非アトミックな移行窓**とまったく同じ形。設計は妥当と裏付けられた。

## 6. 接続プール(PgBouncer)【確証 3-0】

- Neon は **PgBouncer transaction モード**。`pool_mode=transaction` / `max_client_conn=10000` / `default_pool_size=0.9*max_connections` / `max_prepared_statements=1000` / `query_wait_timeout=120`。**ユーザ変更不可**。
- `max_client_conn=10000` は**受け付けるクライアント接続の上限であって同時実行トランザクション数ではない**(1 CU で同時実行は ~377)。
- pooled / direct は**ホスト名だけで切替**:`-pooler` 接尾辞付き → PgBouncer 経由(transaction)、素のホスト名 → 直結(`pg_dump`/マイグレーション推奨)。

## 7. serverless driver(参考。tsubomi は当面不要)【確証 3-0】

- `@neondatabase/serverless` は **wss://(セキュア WebSocket)**経由。認証は **SCRAM → 平文 password に降格**(TLS で保護)。許容理由は (a) ランダムパスワードのみ発行=辞書攻撃免疫、(b) SCRAM は意図的に ~100ms CPU を食う設計で **serverless の CPU 予算(Cloudflare Workers の 10ms/50ms)を超える**から。
- **証伪済み(0-3)= 繰り返さないこと**:「serverless は TCP/プールを保持できないから WebSocket」は**誤り**。本当の理由は **CPU コスト + ラウンドトリップ削減**。
- tsubomi は普通の TCP(pgbouncer)を出すので、この HTTP/WS 層は不要。将来エッジ実行から繋ぎたくなった時の参考まで。

## 8. 攻撃面と緩和 【推論・中】

- IP 白名単無しの公開 Postgres は **接続フラッド**と**資格情報スタッフィング/総当たり**スキャンに晒される(公開 5432/6432 はボット常連)。
- tsubomi の手当て(推奨):
  - pgbouncer の `max_client_conn` / per-user プール上限を**低めに**。
  - `query_wait_timeout` と idle タイムアウトを設定。
  - **ランダムなロール別パスワード + channel binding** が総当たり/MITM を実質無効化する(ここは確証ベース)。
- ⚠ **信頼度=中の理由**:**Neon の proxy 層レート制限・scale-to-zero/suspend が暴露面に与える影響・autoscaling の DoS 挙動・IP Allowlist の実装・Protected Branches・Private Networking/PrivateLink は、今回どれも一次出典で確認できなかった**(§12 の未解決へ)。この節は推論。

---

## 9. tsubomi への示唆 —— 採用すべき 【合成】

1. **pgbouncer の client TLS を、frp が中継する唯一の終端にする**。VPS 上では **素の TCP パススルー**にとどめ、**VPS に二つ目の TLS 終端 proxy を置かない**。tsubomi は既に pgbouncer で client TLS を終端しており、これは Neon と同じ正しい形。
2. **非 TLS を pgbouncer で拒否**(`client_tls_sslmode = require`/`verify-*`)。Neon の `hostssl` のみ拒否に倣う。
3. **配る接続文字列に `sslmode=verify-full` + `channel_binding=require` を必須化**し、**サーバ CA 証明書を同梱**して verify-full を実際に可能にする。**これが IP 白名単を安全に外せる根拠**。
4. **ランダムな長いロール別パスワードのみ発行**(tsubomi の at-rest XChaCha20-Poly1305 + app/human 双 role 設計は既に整合)。

## 10. tsubomi への示唆 —— 真似しない / 注意 【合成】

1. **自前 SNI ルータ/endpoint ID/Neon Proxy は作らない**。あれは「多テナントを共有 proxy に集約して demux する」ためだけの機構。tsubomi の frp は **1 つの pgbouncer** を指すので丸ごと不要。
2. **PG14 未満/非 SNI の回避策**と**パスワード欄埋め込み**は Neon 固有の痛み(demux + 旧クライアントの帰結)。tsubomi は単一ホストで demux 不要なので**最初から踏まない**。
3. **channel binding は SCRAM-SHA-256-PLUS が前提=平文パスワード経路と非両立**。→ **channel binding を採り、パスワード欄埋め込み方式は絶対に採らない**。
4. **frp 越しではクライアントの送信元 IP が VPS になり、実 IP は消える**(TCP 中継の宿命)。将来 per-IP のレート制限/ログを入れるなら要対策(frp の PROXY protocol で実 IP 透過、等)。

---

## 11. 結論:IP 白名単を捨てる判断は是か?

**条件付きで是**。「IP 白名単無し公開」を支えるのは Neon でも IP ではなく **`verify-full` + `channel_binding=require` + ランダムパスワード**の三本柱。これらが成立するなら 0.0.0.0/0 暴露は防御可能。tsubomi は (3)(4) を満たせる位置にいる。

ただし**実装前に必ず潰すべき前提が一つある**(§12-④):**pgbouncer の client 側が SCRAM-SHA-256-PLUS(channel binding)を喋れるか**、そして**それが frp パススルー越しに端到端で成立するか**。

- frp が **素の TCP 中継**である限り、TLS セッションはクライアント↔pgbouncer の端到端なので、`tls-server-end-point` バインディング(pgbouncer が提示する葉証明書をハッシュ)は**理屈上は生き残る**。frps↔frpc 間に frp 独自 TLS を張っても、それは外側トンネルで内側の pg TLS は端到端のまま。→ **壊れない見込みだが、実機で要検証**。
- もし pgbouncer が channel binding を喋れないなら、要石が抜け、防御は「verify-full + ランダムパスワード + TLS」に落ちる(それでも相応に堅いが、verify-full はクライアント側の `sslrootcert` 設定に依存し、徹底させにくい)。**その場合は IP 白名単の放棄を再考する余地が出る**。

---

## 12. 未解決の問い(実装前に潰す)

今回の検証で**一次出典が取れず残った**もの。実装着手前にここを埋める価値が高い:

1. **Neon Proxy は接続レート制限/総当たりスロットルを proxy 層(Postgres 認証に到達する前)で掛けているか?パラメータは?** —— no-allowlist 計画にとって**最も関連する未知**。
2. **scale-to-zero/suspend と暴露面の相互作用**:未認証の接続フラッドで compute を強制 wake できる(コスト/DoS ベクタ)か?Neon は wake を認証成功にゲートしているか?
3. **Neon の IP Allowlist(有料)/Protected Branches/Private Networking(PrivateLink)の中身** —— TLS+channel-binding+ランダムパスワード以外に tsubomi が足すべきコントロールを示唆しないか。
4. **【最重要】frp 中継越しに pgbouncer の channel binding が端到端で機能するか**(§11)。加えて、frp の送信元 IP 書き換え下で**実クライアント IP をログ用に回収する**手段。

---

## 13. 出典と調査の限界

**出典の質**:確証主張のほぼ全ては Neon 自身の一次ドキュメント/API リファレンス + PostgreSQL 公式ドキュメント(libpq-ssl Table 32.1、sasl-authentication)。技術機構としては高品質だが**一部はベンダ自己記述**。暗号/MITM 系の主張は PostgreSQL コアドキュメントで独立裏取り済み=最も堅い。

主要一次出典:
- Neon: `connect/connectivity-issues`, `connect/connection-errors`, `connect/connect-securely`, `connect/connection-pooling`, `connect/choose-connection`, `manage/roles`, `api-docs reset_password`, `security/security-overview`, `introduction/ip-allow`, `guides/neon-private-networking`, blog `quicker-serverless-postgres` / `avoid-mitm-attacks-with-psql-postgres-16`。
- PostgreSQL: `docs/current/sasl-authentication`, `docs/current/libpq-ssl`。

**割れた票(2-1)**:① パスワード欄降格が "HTTPS 相当" という主張は Neon の立場で内部矛盾あり=独立保証ではない。② channel binding が `sslmode=require` 単体でも MITM を防ぐ点は多数支持だが残留異論あり(PostgreSQL ドキュメントで多数説を裏取り済み)。

**時効性**:libpq-SNI/PG14/2021-09 は確定史実。PgBouncer 設定値(`max_client_conn=10000` 等)は Neon が変えうる現行値=利用前に再確認。

**証伪済み(引用禁止)**:(1)「serverless は TCP を保持できないから WebSocket」(真因は CPU コスト+RTT)、(2) PostgreSQL の SASL は SCRAM-SHA-256/-PLUS/OAUTHBEARER の 3 つだけ、という言い切り、(3) channel-binding MITM 機構の特定の言い回し。

**カバレッジ欠落**:IP Allowlist 機構・Protected Branches・Private Networking/PrivateLink・proxy 層レート制限・autoscaling・scale-to-zero の暴露面影響は**今回未確認**。§8・§12 はそれゆえ推論寄り。

**tsubomi 固有の注記**:frp の送信元 IP NAT(実 IP が VPS に化ける)は TCP 中継の運用特性であって Neon 由来の事実ではない。
