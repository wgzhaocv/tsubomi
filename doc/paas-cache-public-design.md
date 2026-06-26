# 公開 cache(外部 `rediss://` 接続文字列)— 実装設計

マイルストーン外の追加(M5 cache への後付け)。目標は **upstash のように、ローカルに valkey が
無くても「開発用の接続文字列を一本」もらって `new Redis(URL)` で直に繋げる**こと(原生プロトコル・
ioredis 既定のまま、**会社ネットワークからも通る**)。公開 DB(`db.tsubomi-app.com`)の双子。

> **落地状態(2026-06-26):LIVE・公網 e2e 済み**。cache は **専用ポート `cache.tsubomi-app.com:8080`**
> (VPS の専用 **cache-gate**【`deploy/cache-gate/`・`--redis-allow-no-sni` = SNI 無しも許可】→ **独立 frpc-cache 池**
> → valkey TLS)。**ioredis / go-redis / redis-py / redis-rs / Bun.RedisClient のいずれも裸
> `new Redis("rediss://…:8080")` で接続可**(SNI を送らない client も通す)。非 TLS / 畸形 / 他ドメイン SNI は弾く。
> pg は別の :443 sni-gate(SNI 必須・pg 専用)で frp 池も独立。server v33 / tbm CLI 1.0.11。
>
> **⚠ 本文との差分**:当初設計は「:443 で pg/cache を SNI 多重化」だったが、**SNI を送らない client(Bun 原生)を
> 裸串で通す**ため cache を専用ポート **:8080**(no-SNI 許可の cache-gate + 独立 frp 池)へ分離した。
> 以下本文中の「cache を :443 で SNI 振り分け」「`TSUBOMI_CACHE_PUBLIC_PORT=443`」等の記述は **この追補で上書き**
> (:8080・no-SNI・独立 frpc-cache が正)。実装は `crates/sni-gate`(`--redis-allow-no-sni`)/ `deploy/cache-gate/`。

> ## 2026-06-26 改訂(本書の前提が変わった — 必読)
>
> **初版は「公開 DB の原始設計」(直 VPS + Traefik TCP 入口 + 会社 IP 白名単)を写した双子だった。
> だが公開 DB は実際にはそう実装されていない。** 2026-06-22 の DoS 事故
> (`incident-frp-pg-public-2026-06-22.md`)を経て、公開 DB の as-built は:
>
> ```
> 客户端 →[VPS :443  自前 SNI 闸门 tsubomi-sni-gate(TLS 非終端・SNI だけ覗く)]
>        →(localhost)frps → frp 隧道 → frpc@Pi(docker)→ pgbouncer@Pi(端到端 TLS 終端・LE 証書)→ pg-tenant
> ```
>
> よって本書は as-built に合わせて全面改訂した。**初版の「6380 番ポート」と「Traefik TCP + ipblock
> の S2」は破棄**(理由は下記「確定した事実」)。これは**ライブの活文書**で、討論しながら更新する。
> 各項に **【確定】**(仓库内で確認済)/ **【提案】**(未実装の設計)/ **【未確認】**(要上機)/
> **【判断ポイント】**(ユーザのリスク/UX 判断が要る)を付ける。

---

## 確定した事実(2026-06-26 調査・仓库内で確認済)

1. **公網ポートは 443、6432/6380 ではない**【確定】。会社出口防火墙は**ポート白名単**で、実測
   `21/22/53/80/443/8000/8080` のみ放行、`5432/6432/7000` および redis の `6379/6380` は**全封**
   (`incident-frp-pg-public-2026-06-22.md` §4.5)。初版の 6380 は**会社ネットワークから出られない**。
   公開 DB は事故後に公網口を **6432→443** に移し、frps は root で 443 を bind。これが「どこからでも繋がる」
   ための要石(Neon が 443 を使うのと同じ理由)。

2. **準入は自前 SNI 闸门(`crates/sni-gate/`)、Traefik でも IP 白名単でもない**【確定】。
   - VPS 辺縁に `tsubomi-sni-gate`(Rust・systemd・x86_64)を置き、公網 :443 を独占。frps は
     `proxyBindAddr=127.0.0.1` で proxy 口を loopback に隠す。
   - 闸门は **TLS を終端しない**。平文の ClientHello から **SNI を覗くだけ**で許可ドメイン以外を即切断
     (fail-closed)→ 扫描洪流が frp の work-conn 池を**認証より前に**食い潰す DoS を辺縁で弾く(事故根因)。
   - 端到端 TLS・証書・接続文字列は不変(passthrough)。
   - 実装:`crates/sni-gate/src/{main.rs,sni.rs}`、systemd unit `deploy/sni-gate/tsubomi-sni-gate.service`、
     配備 `scripts/ship-sni-gate.sh`(zigbuild x86_64 → scp 二進制 + unit → restart)。

3. **現在の sni-gate は Postgres 専用・単一後端・単一 SNI**【確定】。`main.rs`:
   - `--backend` は**1 つ**、`--sni` は単一許可表。**多重化(SNI で振り分け)していない**。
   - 前導が **Postgres 固有**:先頭 8 バイトの `SSLRequest`(必要なら `GSSENCRequest` を先に `N` で断る)を
     受け、闸门が自ら `S` を返し、**その後**に TLS ClientHello を読む。後端へは `SSLRequest→S` を**replay**。
   - ⇒ **redis の `rediss://`(TLS-on-connect・前導なし・先頭 `0x16`)は現状の闸门を通れない**
     (先頭 8 バイト比較で `SSLRequest` に一致せず `bail!` 拒否)= **新規コードが要る**(後述「山场」)。
   - ただし `sni.rs::parse_sni` は **TLS ClientHello を解くだけで協議非依存** = **redis でもそのまま再利用可**
     (単体テスト充実)。

4. **frp 配置は仓库に無い = ops 改動**【確定】。frps は VPS の systemd(`/etc/frp/frps.toml`)、frpc は Pi の
   docker コンテナ `tsubomi-frpc`(`/frp/frpc.toml`)。「第二 proxy を足す」は**機器側の操作**であってコード差分ではない。

5. **コード側の写し元は揃っている**【確定】(`crates/server/src/config.rs`):
   `db_internal_sslmode`/`db_public_sslmode` 分離・`db_public_*`・`cache_internal_host/port` は実在。
   `cache_public_*` は**無い**(本件で足す)。`databases.rs::{build_url,require_db_public}` が S1 の写し元。

6. **valkey は loopback のみ・TLS 口も証書も無い**【確定】(`compose.prod.yml`):`allkeys-lru` +
   `127.0.0.1:6433:6379`。TLS 終端は本件で足す。`caches.rs::{url,rotate}` は今 `build_url`(内部 `redis://`)を
   返し**闸は無い**。

7. **証書基盤は acme.sh が既にある**【確定】。公開 DB は acme.sh の DNS-01(CF_Token)で
   `db.tsubomi-app.com` の LE 証書を取得・90 日自動更新し、pgbouncer に差し替え済み
   (`paas-db-public-relay-結論.md` / 関連メモ)。**lego は使っていない**(初版の S3 記述は acme.sh に訂正)。

---

## なぜ・確定した前提

- **CF Tunnel(香橙派の HTTP 入口)では redis 裸 TCP を出せない**【確定】。よって公開 cache も DB と同じく
  **公網 IP の VPS(ConoHa `proxy`)を frp 中継として経由**する。直 VPS で Traefik を立てる話ではない。
- **公開は必ず TLS(`rediss://`)**【確定の方針】。redis `AUTH` は**パスワードを平文で送る**。裸 TCP =
  資格情報とデータがワイヤで丸見え。だから外部串は無条件で `rediss://`(選べない)。
- **内部注入は明文 `redis://` のまま不変**【確定】。app は docker 網内で `redis://tsubomi-valkey:6379` を使う
  (`services/inject.rs` が `cache_internal_host/port` から直接組む。`caches::build_url` は通らない)。ここは触らない。
- **cache は単一 ACL ユーザ `c_<shortid>` を内外兼用**【確定】。DB の app/human 双 role と違い、同じ `c_xxx` が
  内部明文口(6379)でも公網 TLS 口でも認証される(同一 valkey 実例の同一 ACL ユーザ)。よって cache に
  「外部専用 role」は要らない — DB との最大の簡略化。
- **IP 白名単は持たない**【確定の方針】。公開 DB が事故後に IP 白名単を採らなかった(443 を源 IP で限速すると
  会社 NAT の共有出口を巻き添えにする)のと同じ。「どこからでも繋がる」= upstash 体験の前提でもある。
  境界は **TLS + ランダム長パスワード + クライアント側証書検証 + valkey ACL(`~c_xxx:*`)**、可用性(扫描遮断)は
  **sni-gate**が持つ。

---

## アーキ:公開 DB と同じ 443 を共有(sni-gate 多重化 + frp 中継)

```
 クライアント (任意の ioredis/redis-cli)
   │  rediss://c_xxx:pw@cache.tsubomi-app.com:443        ← TLS は valkey と端到端(闸门・frp は素通し)
   ▼
 VPS proxy  :443  tsubomi-sni-gate(TLS 非終端・SNI で振り分け)
   │   SNI=cache.tsubomi-app.com → (localhost) frps の cache proxy 口
   │   SNI=db.tsubomi-app.com    → (localhost) frps の pg proxy 口(既存)
   │   それ以外 / 非 TLS / timeout → 即切断(fail-closed)
   ▼   frp 隧道(裸 TCP 透過・内層 mTLS)
 香橙派  frpc(tsubomi-frpc)→ 127.0.0.1:<valkey TLS 口>
   ▼
 valkey  --tls-port <ローカル>(SAN=cache.tsubomi-app.com の LE 証書で TLS 終端)+ --port 6379(内部明文・不変)
   ▼  ACL: ~c_xxx:* / &c_xxx:* / +@all -@admin -@dangerous -function -script
```

- **公網に晒すのは VPS :443 の sni-gate ただ一つ**。pg も cache も**同じ 443**を共有し、闸门が **SNI で振り分ける**
  (`cache.tsubomi-app.com` と `db.tsubomi-app.com` は別 SNI なので区別できる = cache は自分のホスト名が要る)。
- **端到端 TLS は valkey が終端**(pgbouncer が DB の TLS を終端するのと同型)。frp も闸门も平文に戻さない。
- valkey の内部明文口 6379 は不変(注入はそのまま)。**TLS 口は追加**で、frp の先(Pi のローカル)にだけ立てる。
  公網に出るのは 443 だけ。

---

## 山场(初版 S2 を置換):sni-gate を redis 対応に拡張する 【提案・本件の核心】

初版の「Traefik TCP 入口 + ipblock(IP 白名単)」は as-built に存在しないので**丸ごと破棄**。代わりに
**443 上の既存 sni-gate に redis を相乗りさせる**のが本件の山场。redis と Postgres の**前導の違い**が肝:

| | 先頭バイト | 前導 | 闸门の振る舞い |
|---|---|---|---|
| Postgres | `0x00`(`SSLRequest` の長さ高位) | あり:`SSLRequest`→闸门が `S` | 既存:`S` を返し ClientHello を読む。後端へ前導 replay |
| redis `rediss` | `0x16`(TLS handshake record) | **なし**(TLS-on-connect) | **新規**:前導なしで即 ClientHello を読む。後端へ replay しない |

拡張の要点【提案】:

- **前門で先頭 1 バイトを嗅ぐ**(消費せず分岐)。`0x16` → redis 路径 / `0x00` → 既存 Postgres 路径。
  ※ 現 `read_preamble_and_hello` は `read_exact(8)` してから比較するので、redis では**その 8 バイトが
  ClientHello の一部**= 失わない形(peek / バッファ後分岐)へ小改修が要る。
- redis 路径:`S` を返さない・後端に `SSLRequest` を replay しない。`parse_sni` で SNI を取り(**そのまま再利用**)、
  許可なら **valkey TLS 口(frps の cache proxy 口)へ繋ぎ、読んだ ClientHello を流し込んで splice**。
- **後端の振り分け**:`--backend`/`--sni` を **SNI→(後端アドレス, 協議)の対応**に拡張
  (例 `--route cache.tsubomi-app.com=127.0.0.1:<cache口>:redis` / `--route db.tsubomi-app.com=...:pg`)。
  `connect_backend` に **redis 変種(前導 replay 無し)**を足す。
- `max_pending`/`max_active` セマフォと 60s 統計はそのまま両協議に効く。

> **設計の一線**:redis 分岐も Postgres 分岐と同じ fail-closed(SNI 不一致・非 TLS・timeout = 無言で切断)。
> `sni.rs` の「宣言長を厳密消費・畸形は拒否」を弱めない。

---

## S1 — config + build_url + AuthInfo + web(平台コード)【提案】

`databases.rs` の公開 DB(Phase1/2)を写す。ただし **port=443・host=`cache.tsubomi-app.com`**。

- **config**(`config.rs`、`db_public_*` の隣):
  - `cache_public_enabled: bool`(`TSUBOMI_CACHE_PUBLIC_ENABLED`、既定 false)
  - `cache_public_host: String`(`TSUBOMI_CACHE_PUBLIC_HOST`、例 `cache.tsubomi-app.com`)
  - `cache_public_port: u16`(`TSUBOMI_CACHE_PUBLIC_PORT`、**既定 443**)
  - 外部 sslmode の旋钮は無い(外部は恒に `rediss://`)。
- **`build_url`**(`caches.rs`):**分支**を足す(下記【判断ポイント】の理由で、DB のような硬 403 闸ではなく分支)。
  ```rust
  fn build_url(state: &AppState, acl_user: &str, password: &str) -> String {
      let cfg = &state.config;
      if cfg.cache_public_enabled {
          // 外部:TLS 固定。valkey が SAN=cache_public_host の LE 証書を出すので ioredis は
          // 既定検証で通る(rediss は Node 同梱 CA で既定検証 = sslrootcert 等価物は不要)。
          format!("rediss://{acl_user}:{password}@{}:{}", cfg.cache_public_host, cfg.cache_public_port)
      } else {
          // 内部:従来どおり明文(これは url/rotate 表示用 = 注入された REDIS_URL の控え)。
          format!("redis://{acl_user}:{password}@{}:{}", cfg.cache_internal_host, cfg.cache_internal_port)
      }
  }
  ```
- **AuthInfo**(`crates/shared/src/lib.rs`、`#[serde(default)]`):`cache_public_enabled` を載せる
  (`db_public_enabled` と並べる)。部署事実なので `/auth/info`(ログイン前から読める)に置く。
- **web**(`web/src/routes/CacheDetail.tsx`):`useAuthInfoQuery()` で判定。有効→公開接続文字列カード
  (DB の `ConnectionStringSection` 同型:reveal/rotate を内包)。**namespace と keyPrefix の注意書きを併記**
  (下記クライアント注意)。無効→従来どおり内部 URL のまま(防御は不要 — 内部串は無害)。

---

## S2(改め)— sni-gate 拡張(redis 多重化)【提案・コード】

上の「山场」を `crates/sni-gate/` に実装し、`scripts/ship-sni-gate.sh` で VPS へ再配備。
**単体テスト**:redis 路径(`0x16` 始まり・前導なし)で正しく SNI を取り後端へ振り分けること、
非許可 SNI / 非 TLS / 前導 timeout は拒否、既存 Postgres 路径が無回帰なこと。

---

## S3 — valkey TLS 終端 + 証書(acme.sh 復用)+ frp 第二 proxy(ops/infra)【提案・要上機】

S1/S2 だけでは valkey が TLS を喋らない = `rediss://` が落ちる。これを埋める ops。

- **証書**【提案】:既存 acme.sh の DNS-01 に **`cache.tsubomi-app.com` を SAN 追加**(一枚二 SAN が最省。
  もしくは別証書)。verify の前提は「valkey が出す証書の SAN = 接続ホスト名」。
- **valkey TLS 口**【提案】:`compose.prod.cache-public.yml`(override)で `--tls-port <ローカル>` +
  `--tls-cert-file`/`--tls-key-file` を生やす(証書は共用 volume)。内部 6379 明文は不変。
- **frp**【ops・仓库外】:frpc@Pi に cache proxy を 1 本足す(`localPort=<valkey TLS 口>`、frps の loopback proxy 口へ)。
  frps@VPS の `allowPorts` にその loopback 口を追加。sni-gate の `--route` で `cache.tsubomi-app.com` をその口に向ける。
- **DNS**【ops】:`cache.tsubomi-app.com` を **DNS-only(灰云)A → VPS 公網 IP**(CF 代理は HTTP のみ=裸 TCP 不可)。
- **更新リロード**【提案・要上機確認】:acme.sh の reloadcmd で valkey に `CONFIG SET tls-cert-file`/`tls-key-file`
  を打って無停止リロード(pgbouncer の reload と同型)。**この熱加载が実機で効くかは未確認**(下記)。

> **依存順**:S3(valkey TLS + 証書 + frp)を入れてから `cache_public_enabled=true`。順序を逆にすると
> 串は配れるが `rediss://` が落ちる。

---

## 接続文字列の最終形 + クライアント側の注意

```js
// url/rotate が返す外部串(cache_public_enabled 時)
const redis = new Redis("rediss://c_xxx:pw@cache.tsubomi-app.com:443", { keyPrefix: "c_xxx:" })
```

- **keyPrefix が最大の落とし穴**【確定の語義】:ACL は `~c_xxx:*` しか許さないので、裸の `redis.set("foo",1)` は
  **NOPERM**。`keyPrefix: "c_xxx:"`(= 注入時の `REDIS_KEY_PREFIX` と同じ)を付けるか key 自身に前缀を付ける。
  **`url`/`get` のレスポンスに namespace(=acl_user)を必ず載せ**、web カードにも明記する。
- **生 redis-cli / valkey-cli は `--sni` が必須**【確定・実機 2026-06-26】:辺縁の sni-gate は ClientHello の
  SNI で振り分けるので、SNI を送らない素の `redis-cli -u rediss://…` は握手段で切られる(`unexpected eof`)。
  さらに `-u` モードでは `REDISCLI_AUTH`/`VALKEYCLI_AUTH` env が拾われず WRONGPASS になる。正しい形は
  `redis-cli --tls --sni cache.<域名> -h cache.<域名> -p 443 --user c_xxx`(パスワードは env)。
  **`tbm cache connect` がこの形を内部で組む**ので CLI 利用者は意識不要。**ioredis(Node)は TLS で SNI を
  既定送出**するので `new Redis("rediss://…",{keyPrefix})` のまま通る(主用途。`new Redis` 一行で繋がる)。
- **rotate の生効**:外部串は人が直接使う = **rotate 後すぐ新串が有効**(`rotate` は DB 先 → valkey、即更新)。
  「再デプロイで効く」は**注入された app 容器**の話(値は起動時解決)で、人が手に持つ外部串とは別。混同しない。
- **値はキャッシュ = 揮発**:valkey は `allkeys-lru`。内存逼迫で key は淘汰され得る。cache の語義。
- **MITM 残留リスクは DB より軽い**【確定】:DB は pgbouncer が channel binding を喋れず「verify をサボる
  クライアント」が能動 MITM に脆弱(`paas-db-public-relay-結論.md`)。一方 **ioredis の `rediss://` は既定で CA 検証**
  するので、既定クライアントはこの穴が無い(psql の verify-full 明示より安全側)。

---

## デプロイ契約・地雷(VPS/Pi で守る)

1. **公網口は 443 一択**【確定】。6379/6380/6432 は会社防火墙で封。連接串は明示で `:443`。
2. **443 は sni-gate が独占、frps proxy 口は loopback**【確定】。cache を足すとき frps の新 proxy 口も loopback、
   公網に新ポートを開けない(全部 443 + SNI 振り分け)。
3. **`cache_public_enabled=true` と override・frp・DNS はセット**【提案】。フラグだけ true で valkey TLS 口 /
   frpc proxy / sni-gate route / DNS のどれかが欠けると `rediss://` が繋がらない。
4. **証書 SAN に `cache.tsubomi-app.com` を必ず含める**【提案】(既定検証の前提)。acme.sh の `--domains` に入れる。
5. **IP 白名単は使わない**【確定の方針】。443 を源 IP で限速すると会社 NAT 共有出口を巻き添えにする(DB の地雷と同文)。
6. **真の client IP**【確定】:frp は裸 TCP 透過、実 client IP は VPS 辺縁(sni-gate / nftables)でのみ生に見える。
   レート制限 / fail2ban を入れるなら辺縁で(valkey 側は frp の IP に潰れる)。
7. **frp work-conn 池 / fd / valkey maxclients が天井**【確定】(`capacity-valid-connections-2026-06-22.md`)。
   poolCount は予熱バッファで上限ではない。公開前に valkey `maxclients` と Pi/VPS の `nofile` を見直す。

---

## 検証

- **dev**:`just check`(cargo check + clippy -D warnings + web lint)+ `cargo test -p tsubomi-sni-gate`
  (redis 路径の SNI 振り分け / 非 TLS・非許可 SNI 拒否 / Postgres 無回帰)。
- **VPS/Pi(落地後)**【要上機】:許可外ネットワークも含め `new Redis("rediss://c_xxx:pw@cache.tsubomi-app.com:443",
  {keyPrefix:"c_xxx:"})` が**追加 TLS オプション無しで**通り、`SET/GET/INCR` が namespace 内で効くこと。
  別 namespace 越境が NOPERM。`redis-cli --tls -u 'rediss://…:443'` でも確認。**会社ネットワークから 443 で通ること**
  (6379/6380 は封なので必ず 443 実測)。SNI を偽ると sni-gate が即切断。
- **S3 単体**【要上機】:`openssl s_client -connect cache.tsubomi-app.com:443 -servername cache.tsubomi-app.com` で
  **公開 CA チェーン**(自己署名でない)+ SAN=cache ホスト名が返ること。

---

## ライブ実機で確認済(2026-06-26・read-only)

`ssh proxy`(VPS root, 133.88.123.119)/ `ssh pi`(香橙派 zwg, LAN 192.168.0.106。**別名は `pi`、`opi` ではない**)で確認:

- **VPS sni-gate**【確定・実機】:`active`、`0.0.0.0:443` を listen。ExecStart =
  `--listen 0.0.0.0:443 --backend 127.0.0.1:6432 --sni db.tsubomi-app.com`(**単一後端・単一 SNI** = 多重化なしを実機確認)。
- **frps**【確定・実機】:`bindPort=7000`(制御)、proxy は **`127.0.0.1:6432`(loopback)**、`allowPorts=[{single=6432}]`、
  mTLS force + token、`proxyBindAddr=127.0.0.1`。⇒ **公網 443 は sni-gate が独占し、frps proxy は loopback 6432 のまま**
  (事故報告の「remotePort=443」案ではなく、**gate が 443 を持ち frps は loopback に留める**形で落ちた = 本書アーキ図どおり)。
- **nftables**【確定・実機】:`tcp dport 443 accept` + `tcp dport 7000 accept` のみ。6432 は公網に開いていない(loopback)。
- **frpc@Pi**【確定・実機】:`tsubomi-frpc`、`poolCount=64`、mTLS、proxy は **`pg-public` 1 本のみ**(localPort 6432→remotePort 6432)。
  ⇒ cache を足す = **`[[proxies]]` を 1 本追加**(localPort=valkey TLS 口、remotePort=新 loopback 口)+ frps `allowPorts` にその口を追加。
- **valkey**【確定・実機】:**v8.1.8**(`allkeys-lru`/maxmemory 256mb/admin は ACL `--user` で静的定義)。**TLS 口は無し**(本件で足す)。
- **証書**【確定・実機】:acme.sh は `db.tsubomi-app.com_ecc` のみ、**SAN = `db.tsubomi-app.com` 単一**。
  ⇒ cache 用に **`cache.tsubomi-app.com` を SAN 追加**(or 別証書)が必須。

### まだ未確認(dev で検証する)
- **valkey 8.1.8 の `CONFIG SET tls-cert-file`/`tls-key-file` 熱加载が無停止で効くか**(更新リロードの可否)。
  本番では書き込み検証をしない方針なので dev で確認する。

---

## 判断ポイント(ユーザの判断が要る)

1. **build_url は分支か硬 403 闸か**【推奨:分支】。DB は `require_db_public` で url/rotate を 403 にする
   (human role の外部串は外部でしか意味が無く、LAN IP を見せると誤誘導だから)。だが **cache の内部串は
   「注入される REDIS_URL の控え」として意味がある**ので、public off でも内部 `redis://` を返してよい(現挙動の温存・
   無回帰)。⇒ **闸は付けず build_url 分支**を推す。DB と非対称なのは意図的。

   > **現挙動の混乱(2026-06-26、ユーザ報告)**:`CacheDetail.tsx` は今、reveal で **内部串**
   > `redis://c_xxx:pw@tsubomi-valkey:6379` を出す(`cache_internal_host` 既定 `tsubomi-valkey` = **docker DNS 名**)。
   > これは**注入された service コンテナの中でしか名前解決できない** = 手元マシンからは繋がらず、ユーザは
   > 「公開文字列なのに使えない・意味が分からない」と感じた。カードに警告文(「内部入口・注入したサービスの
   > コンテナからのみ」)はあるが弱い。**この feature(公開串)が解決する当の問題そのもの**。
   > public **off** の間は、カードの文言を「これは社内サービスに注入される値で、手元からは繋がらない」と
   > もっと明確にする(or reveal を控える)ことを併せて検討する。
2. **証書は db と一枚共用(二 SAN)か別証書か**【推奨:一枚二 SAN】。acme.sh `--domains db... --domains cache...`。
3. **cache 専用ホスト名**は `cache.tsubomi-app.com` 固定でよいか(sni-gate は SNI で振り分けるので DB と別 SNI が必須)。

---

## Out of scope

- 署名子域名(Neon 方案 B 相当・`<token>.cache.tsubomi-app.com`)による「存在するテナント名の枚挙」遮断(将来)。
- password 字段の時効署名トークン(方案 C 相当)。
- 公開 cache の接続数制限 / fail2ban / mutual-TLS(`--tls-auth-clients yes`)化(将来)。
- 証書更新の background 収束化(初版は acme.sh reloadcmd。背骨どおりの「期望状態→収束」化は将来)。
- nftables 等ホスト FW での IP 制御(辺縁=sni-gate / nft に寄せる)。
