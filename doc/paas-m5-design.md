# tsubomi PaaS — M5 cache(valkey)実装設計(第 5 層)

> `paas-tech-design.md`(第 4 層)の §2 cache_details / §5 cache 注入 / §7 ACL 隔離 / §10 M5 を、
> **そのまま書き起こせる粒度**まで落とす:infra 追加、migration、ACL の SETUSER/DELUSER、
> REDIS_URL 注入、key 前缀 + コマンド白名単での隔離、rotate / ゴミ箱 / 起動時 ACL 収束。
>
> **第 4 層と矛盾させない。**§0 の 6 決定は不変。本書が新たに「確定」するのは第 4 層が方針だけ
> 示してコードに落ちていない穴(key 隔離の機構・内部入口・コマンド白名単・ゴミ箱の意味論・
> 依存・備份)。それらは §11 に一覧し、各々**否決可**(第 4 層 §0 の作法)。
>
> 背骨を一言で:**cache は「簡略版 database」** — 共有 valkey インスタンスに per-cache の ACL
> ユーザを作り、接続文字列を service に注入する。隔離は **valkey ACL(key 前缀 `~ns:*` +
> コマンド白名単)**= 越境は NOPERM(平台は数据路径に居ない・直連モデル)。
>
> 完了判定(第 4 層 §9 / §10 の M5 行):**作った cache に app が `REDIS_URL` で繋がり、
> 自分の namespace 外の key / 危険コマンドは NOPERM で弾かれる。**

---

## 0. スコープ

M5 が出すもの:

- infra に **valkey**(`tsubomi-edge` 参加。コンテナが docker DNS `tsubomi-valkey:6379` で直連)。
- `cache` リソース一式:create / list / status / delete / rotate(`tbm cache …`)。
- **ACL 隔離**:per-cache に `ACL SETUSER`(key 前缀 `~<ns>:*` + チャンネル `&<ns>:*` +
  コマンド白名単)。越境 / 危険コマンドは NOPERM。
- **注入**:cache → `REDIS_URL`(内部入口の ACL ユーザ文字列)+ `REDIS_KEY_PREFIX`(= `<ns>:`)。
- **ゴミ箱**:delete → ゴミ箱(凭据壳 + key は内存に温存)→ restore で「消さなかったかのよう」。
- **起動時 ACL 収束**(reconcile 哲学):cache_details が期望状態、valkey の ACL を起動時に収束。
- web:cache 一覧 / 詳細(接続文字列・REDIS_KEY_PREFIX・使用内存)。

M5 が**出さない**もの:cache の外部(公網 / 会社 CIDR)入口(v1 は内部注入のみ。§11-B)/ cache データの
備份(易失なので取らない。§11-F)/ namespace 跨ぎの共有 cache(1 cache = 1 namespace = 1 ACL ユーザ)。

---

## 1. 着工順序(3 スライス、各々単体で検証可能)

| # | スライス | 範囲 | 検証 |
|---|---|---|---|
| **S1 基盤** | infra に valkey(edge)+ migration(`cache_details`)+ redis 依存 + `tbm cache create/list/delete`(ACL SETUSER/DELUSER、namespace/acl_user/password 暗号化保存)+ 起動時 ACL 収束 + web 一覧 | `tbm cache create x` → valkey に ACL ユーザができる;list/delete が効く;server 再起動で ACL が復活 |
| **S2 注入 + 隔離** | inject.rs の cache 分支:`REDIS_URL` + `REDIS_KEY_PREFIX` を内部入口で注入 + ACL の key 前缀/コマンド白名単 + **e2e 越境 NOPERM** | service に inject → 再デプロイ → コンテナ内で `redis-cli -u $REDIS_URL` が自分の `ns:*` だけ読み書きでき、他 key / FLUSHALL は NOPERM |
| **S3 rotate + ゴミ箱 + web 詳細** | rotate(ACL resetpass)+ delete→ゴミ箱→restore(凭据壳・key 温存)+ web 詳細 + `tbm cache` 補全 | rotate で旧文字列即死(再デプロイで新効);delete→restore でデータ含め復活 |

S1 が地基(valkey 接続 + ACL 操作 + 収束)。S2 が M5 の核(隔離の e2e = 完了判定)。

---

## 2. データモデル(migration:`20260619000001_cache.sql`)

第 4 層 §2 の `cache_details` を**そのまま**写し起こす。

```sql
create table cache_details (
  resource_id  uuid primary key,
  kind         text not null default 'cache' check (kind = 'cache'),
  foreign key (resource_id, kind) references resources(id, kind) on delete cascade,
  acl_user     text unique not null,            -- valkey ACL のログイン名。c_<shortid>
  namespace    text unique not null,            -- key 前缀。~<namespace>:* / &<namespace>:*
  password_enc bytea not null,                  -- ACL パスワード。rotate 可 ⇒ 復元可能な暗号化(crypto.rs)
  rotated_at   timestamptz                      -- 最後の rotate 時刻(UI の「失効済み」ソフト提示)
);
```

確定する細部:

- **`acl_user` = `namespace` = `c_<shortid>`**(同じ shortid で両方を生成。別々の列だが同値で足りる
  — DDL の 2 列は将来の分離余地)。`<shortid>` は DNS/valkey 安全な小文字英数(database の
  `gen_dbname` を踏襲)。key 前缀は `<namespace>:`。
- `password_enc`:32 byte 乱数を crypto.rs で封緘(database パスワードと同じ XChaCha20-Poly1305)。
- ゴミ箱は M1 の `resources.deleted_at / purge_after / trash_meta` をそのまま使う(cache 専用列は不要)。
  trash_meta は `{acl_user, namespace}`(復元はこの 2 つ + detail の password_enc で ACL を再作成)。
- **⚠ 実装メモ**:`sqlx::migrate!("../../migrations")` は**コンパイル時にマイグレーションを埋め込む**。
  この新ファイルを足したら **server を再ビルドしないと適用されない**(CI / `just release-image` は
  クリーンビルドなので本番は問題なし。dev で `cargo build` が新ファイルを検知しない時は state.rs を touch して
  再ビルドを強制 — M4 S4 で踏んだ点)。

---

## 3. infra の追加(`infra/docker-compose.yml`)

**admin の定義方式(確定)**:aclfile は `${VAR}` を展開せず、また aclfile と inline `user` 指令は
**排他**(両立すると valkey が起動拒否)= aclfile を使うと実行時 `ACL SETUSER` も使えない。よって
**aclfile は使わず、admin を compose の `command --user` で静的定義**する(`${TSUBOMI_VALKEY_ADMIN_PASS}`
は compose が插值。per-cache ユーザは実行時 `ACL SETUSER` で足し、§7.3 が収束)。YAML 上 `~ & + > *` は
行頭で特殊なので各トークンを引用する。

```yaml
valkey:
  image: valkey/valkey:8-alpine      # 公式・multi-arch(arm64+amd64。香橙派 OK)
  container_name: tsubomi-valkey
  restart: unless-stopped
  # default は off、平台専用 tsubomi-admin だけが管理権(§11-J)。--user は username 以降の
  # 各トークンを規則として読む。per-cache ユーザは平台が実行時に ACL SETUSER で足す(揮発、§7.3 で収束)。
  command:
    - valkey-server
    - --maxmemory
    - ${TSUBOMI_VALKEY_MAXMEMORY:-256mb}
    - --maxmemory-policy
    - allkeys-lru
    - --user
    - default
    - "off"
    - --user
    - tsubomi-admin
    - "on"
    - ">${TSUBOMI_VALKEY_ADMIN_PASS:-tsubomi_valkey_dev}"
    - "~*"
    - "&*"
    - "+@all"
  ports:
    # ホスト側 6433(6379 はローカル redis に取られがち = pg の 5434/5435 と同じ衝突回避)。
    # コンテナ内は 6379 のまま = 注入の docker DNS(tsubomi-valkey:6379)は不変。外部入口なし(§11-B)。
    - "127.0.0.1:6433:6379"            # 平台(host 直走り)の admin 接続用 loopback
  volumes:
    - valkey_data:/data               # RDB(キー本体。per-cache ACL は平台が収束で貼り直す)
  networks: [default, tsubomi-edge]   # edge 参加 = コンテナが tsubomi-valkey:6379 で直連
```

- **edge 参加**(M3 §3.4 の pgbouncer と同型):ユーザコンテナは `tsubomi-valkey:6379` を docker DNS で
  引く。infra default 網にも居るので平台(host 直走り)からも届く。pg-platform / pg-tenant の隔離は不変。
- **平台の admin 接続入口**:`ports: 127.0.0.1:6433`(loopback のみ。pg-tenant と同型)。平台はホスト直走り
  なので docker DNS は引けず、loopback 公開で admin 接続する。公網 / 会社 CIDR には晒さない(§11-B)。
- **本番(`compose.prod.yml`)**:同じ valkey サービスを本番 compose にも足す(`tsubomi-edge` 参加、
  `127.0.0.1:6433` loopback 公開、`valkey_data` 永続、admin pass は `${TSUBOMI_VALKEY_ADMIN_PASS:?...}` で
  **必須**)。server コンテナは host ネットなので 127.0.0.1:6433 で admin 接続(pg と同じ)。外部 ingress は
  無いので `compose.prod.tls.yml`(traefik :443)には変更不要。`just ship` が M5 入りの server イメージを
  build → compose.prod.yml を配布 → `compose up -d` で valkey も作成・収束。デプロイ前提は Pi の
  `.env.production` に `TSUBOMI_VALKEY_ADMIN_PASS` + `TSUBOMI_VALKEY_ADMIN_URL`(= `redis://tsubomi-admin:<同pass>@127.0.0.1:6433`)。
  cache データは備份しない(§11-F)ので valkey_data は日次バックアップ対象外(RDB は再起動跨ぎのみ)。
- **admin 認証**:`default` は **off**。平台専用 `tsubomi-admin`(強乱数パスワード、compose の `--user`)だけが
  管理権を持つ。平台は `redis://tsubomi-admin:<pass>@…:6379` で `ACL SETUSER` を発行。per-cache ユーザは
  `-@admin` 済みなので ACL/CONFIG を打てない。edge 上の不可信コンテナに admin 入口を晒さない(§11-J)。
- **maxmemory + allkeys-lru**:cache は**本質的に易失**。共有実例が満ちたら LRU で**任意の key を**
  溢れさせる(整機を守る)。⇒ **delete→restore の「データ復活」は best-effort**(温存中の key も圧力下で
  evict されうる)。また 1 テナントの大量書き込みが**他テナントの key を evict** しうる(noisy neighbor、
  cache としては許容範囲だが §11-D に明記)。非 TTL データを守りたいなら `volatile-lru`(TTL 付きだけ
  evict、ただし非 TTL 書き込みで OOM 余地)に切替可 — §11-D で否決可。
- **ACL の永続化はしない(per-cache 分)**:`ACL SETUSER` は揮発。**永続化は平台の収束に委ねる**
  (§7.3)= cache_details が真実源(背骨)。RDB はキー本体だけ戻し、per-cache ACL は平台が貼り直す。
  admin だけは compose の `--user` で静的に在る(収束の前提)。

---

## 4. create / list / delete(S1)

database(`databases.rs` + `tenant.rs`)を範に、cache 版を `caches.rs` + `valkey.rs`(ACL 操作)に。

### 4.1 `POST /api/caches`(create)
入力 `{ name }`。やること(database create と同型):
1. `display_name` 検証 + 重複チェック(`resources` UNIQUE)。
2. `shortid` 生成 → `acl_user = namespace = c_<shortid>`。`password = gen_password()`。
3. **valkey に ACL 作成**:`valkey::set_user(state, acl_user, namespace, &password)`(§6 のコマンド)。
   失敗したら中止(平台行は入れない)。
4. `password_enc = crypto.encrypt(password)`。
5. `resources`(kind=cache、anon_seq)+ `cache_details` 挿入。失敗したら valkey の ACL を掃除(DELUSER)。
6. DTO 返却(秘密はここでは出さない — 接続文字列は `/url` 専用、database と同じ規律)。

### 4.2 list / status
`GET /api/caches`(resources と join)/ `GET /api/caches/:id`(namespace / rotated_at / 使用内存)。
使用内存 = valkey に `MEMORY USAGE` を namespace の代表 key で…は不正確 → v1 は **`DBSIZE` ではなく
namespace の概算**(`SCAN MATCH ns:* COUNT` で key 数、または省略)。owner 可視化(M4 ranking の cache)も
この値を使う。**正確な per-namespace メモリは valkey に無い**ので「key 数」を出す(§11-F の妥協)。

### 4.3 delete
ソフト削除(database と同型、§7.2):**`ACL DELUSER` だけ**(key は内存に温存)→ `deleted_at` +
`trash_meta={acl_user,namespace}`。実体(key)は purge(3d)で消す。

---

## 5. 注入(S2、起動の瞬間 — 決定 #5)

`inject.rs` の `resolve` に cache 分支を足す(現状 `// cache(REDIS_URL)は M5` のコメント箇所)。

```
cache → env_var(既定 REDIS_URL)= redis://<acl_user>:<password>@<内部入口>:6379
      + <REDIS_KEY_PREFIX 既定 env>= <namespace>:
```

- **内部入口**(§11-A)= docker DNS `tsubomi-valkey:6379`(config `TSUBOMI_CACHE_INTERNAL_HOST/PORT`)。
  pgbouncer と同型でコンテナは社外に出ない。
- **`REDIS_KEY_PREFIX` も注入する**(§11-C の核):app は redis クライアントの keyPrefix にこれを
  設定する(ioredis/node-redis/go-redis 等が対応、1 行)。ACL が `~<ns>:*` で兜底するので、前缀を
  付けないアクセスは NOPERM = fail-safe。
  - **prefix env の命名(確定)**:既定注入は `REDIS_URL` + `REDIS_KEY_PREFIX` の 2 本。`--as <ENV>` で
    URL の env_var を変えた場合、prefix は `<ENV>_KEY_PREFIX`(例 `--as CACHE_URL` → `CACHE_KEY_PREFIX`。
    末尾 `_URL` を `_KEY_PREFIX` に置換、無ければ付加)。値は常に `<namespace>:`。
    - 派生 prefix 名は**予約 / 去重しない**:静的 env や他注入と同名になれば deploy 時に後勝ち(利用者の
      設定ミス。ACL `~ns:*` が安全境界なので機能影響のみ・情報漏れではない)。通常の `REDIS_URL` 既定では衝突しない。
- 失効(注入元が soft 削除済み)→ 空に解決(database / volume と同じ。第 4 層 §5)。
- 値は**コンテナ起動の瞬間に解決**(rotate / inject 後は再デプロイで効く — 決定 #5)。

---

## 6. ACL 隔離(S2、完了判定の核)

`valkey::set_user(acl_user, namespace, password)` が発行する ACL:

```
ACL SETUSER <acl_user> reset
ACL SETUSER <acl_user> on >password         # ★ パスワード追加は単一の > (>> は誤り)
  ~<namespace>:*                 # この前缀の key だけ読み書き可
  resetchannels &<namespace>:*   # pub/sub もこの前缀のチャンネルだけ
  +@all -@admin -@dangerous      # 危険(FLUSHALL/FLUSHDB/KEYS/SHUTDOWN/DEBUG/CONFIG/SWAPDB…)を除く全コマンド
  -@scripting                    # EVAL/EVALSHA/FCALL/SCRIPT/FUNCTION = スクリプティング全面禁止 — 下記
```
(valkey の `ACL SETUSER` は同一ユーザへの複数呼びが累積。上は 1 回で書くなら一行に連ねる。
`reset` で初期化してから組み立てると冪等。`>password` は**単一の `>`**。)

- `~<ns>:*`:他人の namespace の key を**値として読み書き**すると **NOPERM**。
- **`-@scripting`(S1 で `-function -script`、その後 codex 監査 2026-06-26 で全面禁止に拡大)**:
  `@scripting` = `EVAL` / `EVALSHA` / `EVAL_RO` / `FCALL` / `SCRIPT` / `FUNCTION` を**まとめて禁止**する。
  **理由は越境防止ではない**(スクリプト内で触る key/channel も ACL パターンで検査され cross-ns は NOPERM
  だった)。**単一スレッドの共有 valkey でのイベントループ DoS を断つため** — 重い / 無限ループの Lua を
  投げられると全テナントを巻き込んで valkey が固まる(`lua-time-limit` は自動 kill せず、テナントは
  `SCRIPT KILL` も持たない)。**管理系の容器コマンドも同カテゴリで一網に塞がる**:`SCRIPT FLUSH`
  (共有スクリプトキャッシュ全消し)と `FUNCTION FLUSH`/`LOAD`/`DELETE`(他テナントの関数ライブラリを
  破壊・上書き)は **key 前缀で名前空間化されないグローバル状態**を触るが、これらも `@scripting` に含まれる
  (旧 `-function -script` を包含。`@dangerous` には入らないので個別禁止が必要だった点は不変)。
- `-@dangerous`:FLUSHALL / FLUSHDB(共有実例を消す)/ KEYS(全 key 走査)/ SHUTDOWN / DEBUG 等を禁止。
- `-@admin`:CONFIG / CLIENT KILL / ACL / REPLICAOF 等の管理系を禁止。
- `&<ns>:*` + resetchannels:pub/sub も namespace 内のみ。
- **Lua / Functions = 全面禁止(`-@scripting`)**:当初は「EVAL を残し、スクリプト内の key も ACL の
  key パターンで検査(valkey/redis ≥ 7.0 の挙動。`otherns:*` への `redis.call` は NOPERM)」で隔離は
  成立していた。**が、可用性(DoS)面で残った穴** — 重い / 無限ループ Lua が単一スレッドの共有 valkey を
  固める — を codex 監査(2026-06-26)が指摘し、**EVAL 系も含めスクリプティングを全て禁止**にした(上項)。
  テナントが Lua を使いたい用途は今は無く、共有実例の安定を優先する。
- **⚠ 既知の隔離ギャップ(§11-I):key の「**値**」アクセスは ~ns:* で隔離されるが、`SCAN` / `RANDOMKEY` /
  `DBSIZE` は **key 引数を取らない**ため ACL の key パターンで**出力がフィルタされない**(redis/valkey の
  既知挙動)。つまり他テナントの **key 名 / 総数が列挙され得る**(値は依然 NOPERM)。完了判定の
  「越境 NOPERM」は**値アクセスには成立、key 名の列挙には成立しない**。内部ツール・key 名は機密扱い
  しない前提で**受容し明記**(§11-I)。SCAN を塞ぐと自分の key も SCAN できず実用性を損なうので塞がない。
- **⚠ pub/sub introspection も同類のギャップ(§11-I・S2 で実測確認)**:`&<ns>:*` は **channel への
  publish / subscribe(値・メッセージ)**を隔離する(`SUBSCRIBE otherns:*` → **NOPERM** 確認済み)が、
  `PUBSUB CHANNELS *` / `PUBSUB NUMSUB` は channel パターンを引数に取らない introspection なので
  **他 ns の活性 channel 名が列挙され得る**(valkey 8 で実測確認 — SCAN の key 名列挙と同類)。値/メッセージは
  依然 NOPERM なので受容(§11-I)。`-PUBSUB` で塞ぐと自分の channel も列挙できず実用性を損なうので塞がない。
- **e2e 完了判定**:inject した service のコンテナで `redis-cli -u $REDIS_URL`:
  - `SET <prefix>foo 1` → OK / `GET <prefix>foo` → 1
  - `GET otherns:bar`(**値**)→ **NOPERM** / `FLUSHALL` → **NOPERM** / `KEYS *` → **NOPERM**
  - (`SCAN` は許可され key 名は見える = §11-I の受容済みギャップ)
  - `PUBSUB CHANNELS *`(他 ns の channel **名**は見える = §11-I 受容済み)/ `SUBSCRIBE otherns:*` → **NOPERM**(ACL-1 確認済み)

---

## 7. rotate / ゴミ箱 / ACL 収束(起動時 + 周期)(S3 + S1)

### 7.1 rotate
`POST /api/caches/:id/rotate`:新 password 生成 → **`password_enc` / `rotated_at` を先に更新(DB=真実源)**
→ `valkey::set_user`(reset → 新パスで再構築。旧パス即死、key 規則は維持)。
**再デプロイで新文字列が効く**(database rotate と同じ意味論)。
**順序(DB 先 → valkey)が肝**:背骨どおり DB が期望状態で valkey はそこへ収束する。set_user が落ちても
周期収束が DB の新パスへ**前向きに**貼り直す(旧パスは復活しない)。逆順だと DB 更新失敗時に収束が旧パスへ
revert し、rotate 済みの旧資格が蘇る(S3 codex review で是正)。

### 7.2 ゴミ箱(delete → restore → purge)
- **delete**(§4.3):`ACL DELUSER <user>`(= 即座にその資格でログイン不可)。**key は内存に温存(試みる)**。
  `deleted_at` + `trash_meta`。→ 注入は宙吊りで失効(復元で生き返る、第 4 層 §2)。
- **restore**:`ACL SETUSER` を **同じ user / namespace / password(detail の password_enc を復号)** で
  再作成 → 残っていた key はそのまま見える。
  - **生存 key 数を報告(TRASH-1)**:restore 時に `SCAN MATCH <namespace>:* COUNT` で**生き残った key 数を数えて返す/
    ログする**。`allkeys-lru` で温存中の key が evict され「空で復元」した場合に、利用者へ best-effort であることを
    可視化する(0 件なら UI/CLI で「データは evict 済み・凭据のみ復元」と示せる)。コストは低い(復元は稀)。
- **⚠ 復元時のデータは best-effort**(§11-D):valkey は `allkeys-lru`。delete から restore までの間に
  **メモリ圧力で温存中の key が evict され得る**。よって「データ含め消さなかったかのよう」は**保証では
  なく best-effort**(凭据壳 + 生き残った key)。cache は本質的に易失なので、これは許容する設計
  (db/volume の「データ完全復元」とは意図的に区別 — §11-D)。
- **purge**(reconcile/gc が 3d 到来で):`ACL DELUSER`(冪等)+ `namespace:*` の key を削除(`SCAN`+`UNLINK`)
  + 行を物理削除。← ここで確実にメモリ解放。

### 7.3 ACL 収束(reconcile 哲学。**起動時 + 周期**)
`ACL SETUSER` は揮発。cache_details(真実源)へ valkey の ACL を収束させる:
- **起動時に 1 回フル**(main の起動シーケンス、`ipblock::sync_traefik` と同列)。
- **加えて周期(reconcile の tick に相乗り、既定 30s)**でも収束する。← **valkey が単独で再起動した場合**
  (= ACL 全消失。平台 server は再起動していない)に、起動時収束だけでは**永遠に貼り直されない**穴を塞ぐ。
  周期収束があれば valkey 再起動から最大 1 tick で自己回復する。実装は安価(全 cache を ACL SETUSER し直す
  だけ。差分検出は不要、冪等)。
- **再接続窓**:valkey 再起動直後〜次の収束まで、user app の再接続は AUTH 失敗し得る(redis クライアントの
  自動リトライが救う)。周期収束で窓を tick 幅に抑える。
- delete 済みは SETUSER しない(既に DELUSER 済み)。収束は「生きた cache の ACL を在らしめる」だけ。
- **⚠ 競態回避(RACE-1・実装制約)**:周期収束は**毎 tick で fresh に「生存(`deleted_at IS NULL`)cache」を
  SELECT してから SETUSER** する(古いスナップショットを使い回さない)。さもないと delete の `ACL DELUSER` と
  tick が交错したとき、古い一覧を基にした SETUSER が**削除直後のユーザを一瞬復活**させ得る。実装は
  「fresh に読む → 生存分だけ SETUSER」の素直な順序で足り、ロックは不要(DB が真実源、収束は冪等)。

---

## 8. API 面 / CLI

```
cache POST   /api/caches                作成（§4.1）
      GET    /api/caches                一覧
      GET    /api/caches/:id            詳細（namespace / rotated_at / key 数）
      GET    /api/caches/:id/url        REDIS_URL + REDIS_KEY_PREFIX（警告付き。秘密は stdout 値）
      POST   /api/caches/:id/rotate     パスワード変更（旧文字列即死）
      DELETE /api/caches/:id            ソフト削除（DELUSER、key 温存）
inject  既存 /api/services/:id/injections に cache を足す（kind=cache を許可）
```

CLI(database を範に、AI フレンドリ規約):
```
tbm cache create|list|status|url|rotate|delete
tbm inject <cache> --into <svc> [--as REDIS_URL]   # 既存 inject に cache 種別を通すだけ
```

---

## 9. 新規依存 / コード配置

- **依存**:`cargo add -p tsubomi-server redis`(tokio 対応。ACL 発行 + e2e 検証用)。
  valkey イメージは公式 multi-arch(crate 追加不要)。
- **server**:
  - `crates/server/src/caches.rs`(`databases.rs` を範に CRUD + rotate + url）。
  - `crates/server/src/valkey.rs`(`tenant.rs` を範に:admin 接続 + `set_user` / `del_user` /
    `purge_namespace` / `reconcile_acls`）。
  - `crates/server/src/services/inject.rs` に cache 分支（§5）。
  - `crates/server/src/config.rs`:`TSUBOMI_VALKEY_ADMIN_URL`（`tsubomi-admin` ユーザで接続。default off）/
    `TSUBOMI_VALKEY_ADMIN_PASS`（compose の users.acl と共有）/ `TSUBOMI_CACHE_INTERNAL_HOST` `_PORT`（注入入口）。
  - `crates/server/src/trash.rs` の restore / purge に cache 分支（§7.2）。
  - `crates/server/src/main.rs` 起動シーケンスに `valkey::reconcile_acls`（起動時フル）+
    `crates/server/src/services/reconcile.rs` の周期 tick にも相乗り（valkey 単独再起動からの自己回復、§7.3）。
  - `crates/server/src/routes.rs` に cache ルート。`state.rs` に valkey admin 接続（or 都度接続）。
- **shared**:cache の DTO（CacheDto / CacheUrlResp。database の DTO を範に serde 安定）。
- **web**:`routes/Cache*.tsx` + `lib/caches.ts`（database の web を範に）+ RESOURCES に cache を有効化。
- **CLI**:`crates/cli/src/commands/cache.rs`（`db.rs` を範に）。

---

## 10. 横切り(M5 で守る一線)

- **隔離は機構**:valkey ACL（key 前缀 + コマンド白名単）が硬い境界。平台は ACL を貼るだけで
  データ路径には居ない（直連）。app が前缀を付けない / 越境 → NOPERM（fail-safe）。
- **資格情報の分立**：cache の REDIS_URL は他の凭据（接続文字列 / deploy key / session / CLI token）と
  独立。password_enc は復元可能な暗号化（rotate / restore に原文が要る）。
- **マルチアーキ**：valkey 公式イメージは arm64+amd64。
- **共有実例の故障域**：1 valkey を全 cache で共有（DB の単一実例と同じ引き受け）。maxmemory + LRU で
  整機を守る。per-cache の正確なメモリ計上は valkey に無い（§11-F）。

---

## 11. 本書が確定した決定(各々**否決可** — 第 4 層 §0 の作法)

| # | 決定 | 理由 | 否決した場合 |
|---|---|---|---|
| **A** | **内部入口 = valkey を `tsubomi-edge` に参加させ docker DNS `tsubomi-valkey:6379` で直連**（M3 §11-A の pgbouncer と同型） | 既存の隔離(edge)を再利用、コンテナを infra 網に入れず公開ホスト名のヘアピンも不要 | 専用入口 valkey をもう 1 個 / 外部 `cache.<域名>:6379`(CIDR、ヘアピン依存) |
| **B** | **cache は内部注入のみ(外部入口なし)** | app から使うのが主用途。db の human 外部串のような外部デバッグ需要は cache では薄い | 外部 `cache.<域名>:6379`(会社 CIDR)を足す（valkey を host 公開 + DOCKER-USER CIDR） |
| **C** | **key 隔離 = ACL `~<ns>:*` + `REDIS_URL` と `REDIS_KEY_PREFIX` の 2 本注入**。app はクライアントの keyPrefix に設定 | 隔離を機構(ACL)で硬く保ちつつ、app 側の負担は 1 行。前缀無しは NOPERM = fail-safe | 透過プロキシで前缀を平台が付与(平台が数据路径に入る — 設計が拒否)/ valkey DB index で分離(ACL は DB を分離しない = 安全でない)/ cache 毎に valkey 実例(重い) |
| **D** | **delete = `ACL DELUSER` のみ（key は内存温存を試みる）、restore = ACL 再作成、purge(3d) で key 削除。データ復元は best-effort** | dump 無しで凭据壳 + 生き残った key を復元。`allkeys-lru` なので**温存中の key は圧力下で evict され得る** ⇒ db/volume の完全復元とは意図的に区別(cache は本質的に易失)。**noisy neighbor**:1 テナントの大量書き込みが他テナントの key を evict しうる(cache として許容、明記) | delete 時に key も即削除(メモリ即解放だが復元でデータ喪失)/ dump してファイル trash(重い)/ `volatile-lru`(TTL 付きだけ evict、非 TTL を守るが OOM 余地) |
| **E** | **コマンド白名単 = `+@all -@admin -@dangerous -@scripting`**（key は `~<ns>:*`、channel は `&<ns>:*`） | `@dangerous` で FLUSHALL/FLUSHDB/KEYS/SWAPDB/SHUTDOWN/DEBUG/CONFIG を一網で禁止(valkey 8 で実測確認)しつつ string/hash/list/set/zset/stream/pubsub/SCAN は使える。**`@scripting`(EVAL/EVALSHA/FCALL/SCRIPT/FUNCTION)は全面禁止** — 単一スレッドの共有 valkey で重い/無限ループ Lua がイベントループを固める DoS を断つため(codex 監査 2026-06-26)。`SCRIPT FLUSH`/`FUNCTION FLUSH` の共有状態破壊も同カテゴリで一網に塞がる(§6) | 個別 `+GET +SET …` の許可リスト（網羅が大変・取りこぼし）/ `+@all`(危険) / **EVAL を残す**(当初案。隔離は ACL key パターンで成立するが DoS 面が残る — 監査で却下) |
| **F** | **cache データは備份しない / per-namespace メモリは「key 数」で代用** | cache は易失（消えても再生成される前提）。valkey に per-namespace の正確メモリ API は無い | valkey BGSAVE の RDB を日次備份に / `MEMORY USAGE` をキー走査で集計(O(n)・高コスト) |
| **G** | **per-cache ACL 永続化は平台の収束に委ねる(`ACL SETUSER` は揮発)。収束は起動時 **+ 周期(30s)**の両方。周期収束は毎 tick で fresh に生存 cache を読んでから SETUSER(RACE-1)** | cache_details が真実源 = 背骨。**周期収束は必須**:valkey 単独再起動(ACL 全消失・平台は無再起動)を起動時収束だけでは直せない穴を塞ぐ。再接続窓は tick 幅+クライアント再試行で吸収。**fresh スナップショット**で delete↔tick の競態(削除直後ユーザの一瞬復活)を防ぐ(§7.3) | 起動時のみ(valkey 単独再起動で ACL が永遠に欠落)/ aclfile + `ACL SAVE`(per-cache も valkey 側に二重真実源)/ 古いスナップショットで収束(delete と競態) |
| **H** | **依存 = `redis` crate** | 成熟・tokio 対応・ACL 発行と e2e 検証に十分 | `fred`(高機能だが M5 には過剰) |
| **I** | **key の「値」隔離は ACL `~ns:*` で硬いが、`SCAN`/`RANDOMKEY`/`DBSIZE` + `PUBSUB CHANNELS/NUMSUB` による他テナントの key 名/channel 名/総数の列挙は防げない — これを受容し明記** | これらは key/channel パターンを引数に取らず ACL のパターンで出力がフィルタされない(redis/valkey の既知挙動)。値・メッセージは依然 NOPERM。内部ツールで key/channel 名は機密扱いしない。**PUBSUB の channel 名列挙は valkey 8 で実測確認(SUBSCRIBE 値路径は NOPERM)**(ACL-1・§6) | `-SCAN -RANDOMKEY -DBSIZE`(+ 必要なら `-PUBSUB`)も禁止(自分の key も SCAN 不可で実用性を損なう)/ cache 毎に別 valkey 実例(重い) |
| **J** | **valkey の `default` ユーザは off、平台専用 `tsubomi-admin`(強乱数・compose の `--user` で静的定義)だけが管理権** | valkey は edge 上で不可信コンテナから到達可能。`default` を残すと admin 入口を晒す。専用 admin + default off で攻撃面を絞る(per-cache は `-@admin` 済み)。aclfile は `${VAR}` 非展開 + 実行時 SETUSER と排他なので使わない(§3) | `default` に requirepass のまま(admin 入口が edge に晒される)/ valkey を edge に載せない(注入の内部入口が成立しない) |

---

## 12. 完了判定(第 4 層 §9 / §10 の M5 行)

- [x] `tbm cache create x` → valkey に ACL ユーザができ、`tbm cache list` に出る(S1)— dev e2e 済み
- [x] **valkey 単独再起動**(`docker restart tsubomi-valkey`)→ 周期収束で ACL が ~7s で復活(S1・§7.3)— dev e2e 済み
- [x] `tbm inject x --into <svc>` + デプロイ → コンテナ内 `$REDIS_URL` で自分の `<prefix>key` を読み書き(S2/最終 e2e)
- [x] **越境(値)/ 危険コマンドが NOPERM**:`GET otherns:*` / `FLUSHALL` / `KEYS *` / `CONFIG` / `SWAPDB` /
      `SCRIPT FLUSH` / `FUNCTION FLUSH` が弾かれる(S2・完了判定の核)。`SCAN` の key 名列挙は受容済み(§11-I)— dev e2e 済み
- [x] `PUBSUB CHANNELS *` 実測済み(ACL-1):他 ns の channel 名は列挙され得る(§11-I 受容)、SUBSCRIBE 値路径は NOPERM
- [x] `tbm cache rotate x` で旧文字列が即死(WRONGPASS)、再デプロイで新文字列が効く(S3/最終 e2e)— dev e2e 済み
- [x] `tbm cache delete x` → ゴミ箱 → restore で凭据 + 生き残った key 復活(best-effort・**生存 key 数を audit 報告** TRASH-1)、
      purge で ACL + key + 行を実体削除(S3・§7.2)— dev e2e 済み
- [x] web で cache 一覧 / 詳細(接続文字列 reveal + REDIS_KEY_PREFIX + key 数 + rotate + 削除)— endpoint は CLI で e2e 済み、
      web は vp build + lint クリーン(database 詳細 UI の同型)。cache の CRUD は **web 画面 + CLI のみ**(公開入口なし = §11-B)
- [x] owner 最後の砦で cache を delete(session 二段確認 + `owner.delete_cache` audit)— dev e2e 済み
- [x] **総合 e2e(最終)**:cache を作り、**実際に cache を使う service**(Node ioredis カウンタ)を立て注入 →
      `tbm deploy`/hook でデプロイ → 公開 URL(traefik)で `REDIS_URL` + `REDIS_KEY_PREFIX` を使い `<ns>:visits` を
      INCR して跨リクエストで永続・隔離内に収まることを確認。rotate → 再デプロイで新コンテナが新パスで接続(決定 #5)— dev e2e 済み
```
