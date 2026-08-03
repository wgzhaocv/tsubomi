# 公開 DB(外部接続文字列)の開閉 + ipblock — 実装設計

マイルストーン外の追加(M1 database への後付け)。**部署のトポロジで外部 DB 接続を開閉し、開く時は
会社 IP 許可リストで絞る**。2 スライス:S1 = 能力開閉トグル、S2 = Traefik TCP 入口 + IP 許可リスト流用。

## なぜ(背景)

香橙派(CF Tunnel 部署)では DB の human 接続文字列が `192.168.0.106:6432`(届かない LAN IP)を
表示していた。原因:**CF Tunnel は HTTP/HTTPS しか中継せず、Postgres の裸 TCP を公網へ出せない**
(Spectrum は有償)。Pi に公網 IP も無い。よって CF 部署では外部 DB 接続は**そもそも提供できない**。
一方、公網 IP を持つ VPS なら提供できる。さらに「誰でも繋げる」のは不可で、既存の会社 IP 許可リスト
(Traefik 層、HTTP 用)を DB にも効かせたい。

背骨どおり:管制面(config + `ip_allow_entries`)が期望状態を持ち、現実(web 表示 / Traefik 動的設定)を
そこへ収束させる。

## S1 — 能力開閉トグル `TSUBOMI_DB_PUBLIC_ENABLED`(既定 false)

- **config**(`crates/server/src/config.rs`):`db_public_enabled: bool`。`cookie_secure`/`tls` と同じ env-bool 解析。
  `tls`(誰が TLS 終端するか)とは**独立**の関心事なので別フラグ(結合しない)。
- **後端 gate**(`crates/server/src/databases.rs`):`require_db_public(&state)` を `url`/`rotate` の先頭で呼ぶ。
  無効なら **403 `AppError::ForbiddenMsg`**(理由付き 403)を返す。**400 にしない理由**:CLI 契約で 400→
  `validation`(AI が入力ミスと誤解し無駄に再試行)、403→`forbidden`(端末扱い=再試行しない)。文案は次の
  一手(web SQL タブ)を含める。`ForbiddenMsg` は `error.rs` に追加した「理由を載せられる 403」(固定文言の
  `Forbidden` と並ぶ、`BadRequest`/`Conflict` と同じ string 持ち 4xx の 403 版)。
- **能力の前端伝達**:`AuthInfo`(`/auth/info`、公開・ログイン前から読める)に `db_public_enabled` を載せる
  (`crates/shared/src/lib.rs`、`#[serde(default)]`)。`Me`(ユーザ属性)ではなく `AuthInfo`(部署事実)に置く。
- **web**(`web/src/routes/DatabaseOverview.tsx`):`useAuthInfoQuery()` で判定。有効→接続文字列カード
  (`ConnectionStringSection` に抽出した自包含子組件:reveal/rotate/rotate モーダルを内包、hooks 自取で
  prop 配らない)。無効→「SQL/テーブルタブを使え」の案内のみ。読込中は描画なし(`enabled ? … : authInfo ? 案内 : null`)。
- **web SQL タブと human role 自体はこのフラグと無関係で常に動く**:web SQL は `tenant_admin_url`(内部)で
  human として接続し、公開ホストを使わないため。だから外部接続を畳んでもデータ確認・編集は web から可能。

**効果**:CF Pi は env 未設定=false なので、再デプロイで接続文字列カードが消え `/url`・`/rotate` は 403。
誤誘導の LAN IP を出さなくなる(= 元の不具合の解消)。

## S2 — Traefik TCP 入口 + IP 許可リスト流用(VPS 用。dev で描画+単体テスト、活体は VPS 落地後)

Postgres は pgbouncer:6432 へ**直結**で Traefik を通らないため、HTTP の ipAllowList が効かない。
公開 DB を **Traefik の TCP 入口経由**にし、**同じ `ip_allow_entries`** を TCP の ipAllowList として流用する。

- **平台側描画**(`crates/server/src/ipblock.rs`):`render_db_tcp_yaml(cidrs, backend)` が `tcp:` 動的設定
  (router `tsubomi-postgres` entryPoints=`postgres` rule=`HostSNI(*)` / middleware `tsubomi-pg-ipallow`
  ipAllowList / service=backend)を組み立てる。`sync_traefik_inner` が cidr を読んだ後、
  `db_public_enabled` なら `db-tcp.yml` を原子書き込み、無効なら削除。**既存の `ipblock::sync_traefik` の
  3 呼出(起動時 main / ip-allowlist の create / delete)で HTTP・TCP 両方が同時収束**(新呼出点なし)。
  backend は **pgbouncer の容器名**(`pgbouncer_container:db_internal_port`)で引く — 注入用の
  `db_internal_host`(= 証書の公開名)ではない。後者を後端に書くと、その名前が traefik 視点で引けない
  瞬間に公網 DNS へ落ちて **traefik が自分自身へ転送する自環**になる(下の §「証書名は仕組みの一部」)。
  **空 cidr は fail-closed**(呼び出し側が本関数を呼ばない = DB を 0.0.0.0/0 に晒さない。HTTP service の
  空=fail-open とは逆)。入口名 `postgres` は const `POSTGRES_ENTRYPOINT`(compose と一致契約)。
- **TLS は Traefik で終端しない**:pgbouncer が client TLS を終端する(`client_tls_sslmode=require`、
  scram-sha-256)。証書は compose の `pgbouncer-certgen` が**種として自署**を置き、本番は acme.sh が
  公開名(`db.<域名>`)の LE 証書で**上書き**して運用する(下の §「証書名は仕組みの一部」)。よって
  Traefik は `HostSNI(*)` の**素の TCP passthrough**で pgbouncer へバイト転送し、client の
  `sslmode=require` は pgbouncer と**端到端**で TLS を張る(Traefik に証書不要)。
- **compose**(`compose.prod.db-public.yml`、`compose.prod.tls.yml` を手本にした override):
  `-f compose.prod.yml -f compose.prod.db-public.yml`。traefik の `command` を全置換で base の `web`(:80)を
  再掲 + `--entrypoints.postgres.address=:6432` を追加、`ports` に `:6432` をマージ。pgbouncer の host publish は
  `ports: !reset []` で落とす(公開 6432 は Traefik が単独で持つ=二重 bind 回避。pgbouncer へは Traefik も注入
  app も docker DNS で内部到達 — 名前は下の §「証書名は仕組みの一部」参照)。
- **接続文字列**(`build_url`)は S1 のまま不変:host=`TSUBOMI_DB_PUBLIC_HOST`(VPS 公開名)、port=`6432`、
  `sslmode=require`。

## 証書名は仕組みの一部(`sslmode` の駆動系差 — 2026-07-26)

**ここが本書の正本**。コード・compose・`.env.example`・skill は 1〜2 行でここを指す。

**問題**:`sslmode=require` の意味が**駆動系で割れている**。libpq(Go の lib/pq・Python の psycopg)は
「暗号化するが証書は検証しない」、Node の `pg` は「**厳格に検証する**」。だから注入する 1 本の
`DATABASE_URL` が、Go では繋がり Node では落ちる。しかも `pg` は接続文字列の ssl 指定が明示 `ssl` 引数を
上書きするので、利用側は「URL から `sslmode` を削ってから `ssl` を渡す」という**文書を読まないと辿り着けない**
回避を強いられていた(AI 利用フィードバック 2026-07-26 で実際に踏まれた)。

**採った解**:**注入する接続文字列の host を pgbouncer の証書の名前に揃える**。pgbouncer は公開名
(`db.<域名>`)の LE 証書を出す(acme.sh が種の自署を上書き)ので、注入ホストも `db.<域名>` にすれば
**厳格に検証しても通る** = 同じ 1 本の URL で両系統が動く。`TSUBOMI_DB_INTERNAL_HOST` がその値。

**公網に出ない仕掛け**:その名前を **docker 網別名**として pgbouncer に生やす(平台が per-service 私網へ
infra を attach する時に付ける = `services/network.rs::pgbouncer_aliases`)。docker 内蔵 DNS が公網 DNS より
先に別名を返すので、解決先はコンテナの網内 IP。**別名を付ける場所を間違えると静かに壊れる** —
テナント容器は M6 網隔離で per-service 私網にしか居ないので、compose で `tsubomi-edge` に付けても見えない
(edge は M6 以降 infra が居るだけの残骸)。別名が無いと公網 DNS に落ち、通信が**網外へ出る**(中継 VPS 経由)か
egress の私網遮断で**届かない**。

**却下した代替**:内部も `verify-full` に上げる案 — libpq 側に `sslrootcert=system` が必要になり、それが
今度は Node で壊れる(接続文字列のパスとして読まれる)= 非互換を別の駆動系へ移すだけ。
`sslmode=no-verify` は libpq が受け付けない(pg 独自)。`disable` は pgbouncer が
`client_tls_sslmode=require` で拒否する。

**引き受けたコスト**:**LE 証書が全テナント app の生命線になった**。以前は検証されないので切れても内部注入は
動いていた。よって更新の自動化(acme.sh `--reloadcmd` = `deploy/db-public/reload-pgb-cert.sh`)と期限監視・
DR 手順が必須(`doc/paas-dr-restore-runbook.md` §E)。**種のままでは繋がらない**(自署 = CA 不信頼)ので、
新規部署・DR では acme.sh を通すまで `TSUBOMI_DB_INTERNAL_HOST` を容器名に戻しておくのが退路。

**分離しておくこと**:`db_internal_host` は「**証書の身元**」であって「配管先」ではない。traefik の公開 DB
後端(`ipblock.rs`)は**容器名**で引く — 公開名を後端に書くと、引けない瞬間に traefik が自分自身へ転送する
自環になる。

## デプロイ契約・地雷(VPS で守る)

1. **真の client IP**:Traefik が**直接** client の TCP を受ける構成でのみ ipAllowList が正しい IP を見る。
   前段に L4 proxy/LB を挟むなら traefik に `--entrypoints.postgres.proxyProtocol.trustedIPs=<上流>` を足し、
   上流で PROXY protocol を有効化する(でないと全 client が上流 IP に潰れ許可リストが無意味)。
2. **`db_public_enabled=true` と override はセット**:フラグだけ true で override を重ねないと、`db-tcp.yml` が
   未定義の `postgres` 入口を参照し router が不活性(Traefik は警告のみ・無害だが繋がらない)。
3. **`compose.prod.tls.yml` と同居**(直 VPS で traefik が :443 も終端)する場合、両者とも `command` を全置換
   するので **web+websecure+postgres を一つの command に統合**すること(本 override 単体は上流 TLS = HTTP :80
   のみの常用形を想定)。
4. **既存 CF Pi は無影響**:base compose を変えず override も重ねないため。pgbouncer の `0.0.0.0:6432` 公開を
   閉じたいなら CF 側 `.env` で `PGBOUNCER_BIND_ADDR=127.0.0.1`(任意・別件)。

## 検証

- **dev(済)**:`just check`(cargo check + clippy -D warnings + web lint)+ `cargo test -p tsubomi-server ipblock`
  (`render_db_tcp_yaml` の passthrough/fail-open/cidr 制限テスト)。
- **VPS(落地後)**:`-f compose.prod.yml -f compose.prod.db-public.yml up -d`(他の overlay も
  在れば全部 `-f` に — ship は自動)→
  許可 IP から `psql "postgres://…@<vps>:6432/…?sslmode=require"` が通り、**非許可 IP は拒否**されること。
  web で `ip-allowlist` を足し引きして TCP 側が即収束するか確認。

## Out of scope

- nftables 等ホスト FW での IP 制御(Traefik 層に寄せる)。
- TLS+DB-public の command 完全自動合成(初版は常用形 + 上記注意)。
- 公開 DB の接続数制限 / fail2ban / `sslmode=verify-full` 化(将来)。
