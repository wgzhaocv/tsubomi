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
  backend = `db_internal_host:db_internal_port`(= `tsubomi-pgbouncer:6432`、既存 config 再利用)。
  空 cidr = fail-open(HTTP 版と同じ約束)。入口名 `postgres` は const `POSTGRES_ENTRYPOINT`(compose と一致契約)。
- **TLS は Traefik で終端しない**:pgbouncer が client TLS を終端する(`client_tls_sslmode=require` + 自署証明書
  `pgbouncer-certgen`、scram-sha-256)。よって Traefik は `HostSNI(*)` の**素の TCP passthrough**で pgbouncer へ
  バイト転送し、client の `sslmode=require` は pgbouncer と**端到端**で TLS を張る(Traefik に証明書不要)。
- **compose**(`compose.prod.db-public.yml`、`compose.prod.tls.yml` を手本にした override):
  `-f compose.prod.yml -f compose.prod.db-public.yml`。traefik の `command` を全置換で base の `web`(:80)を
  再掲 + `--entrypoints.postgres.address=:6432` を追加、`ports` に `:6432` をマージ。pgbouncer の host publish は
  `ports: !reset []` で落とす(公開 6432 は Traefik が単独で持つ=二重 bind 回避。pgbouncer へは Traefik も注入
  app も docker DNS `tsubomi-pgbouncer:6432` で内部到達)。
- **接続文字列**(`build_url`)は S1 のまま不変:host=`TSUBOMI_DB_PUBLIC_HOST`(VPS 公開名)、port=`6432`、
  `sslmode=require`。

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
- **VPS(落地後)**:`-f compose.prod.yml -f compose.prod.db-public.yml up -d` →
  許可 IP から `psql "postgres://…@<vps>:6432/…?sslmode=require"` が通り、**非許可 IP は拒否**されること。
  web で `ip-allowlist` を足し引きして TCP 側が即収束するか確認。

## Out of scope

- nftables 等ホスト FW での IP 制御(Traefik 層に寄せる)。
- TLS+DB-public の command 完全自動合成(初版は常用形 + 上記注意)。
- 公開 DB の接続数制限 / fail2ban / `sslmode=verify-full` 化(将来)。
