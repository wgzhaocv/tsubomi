# tsubomi PaaS — 技術設計(第 4 層)

> `paas-design-v2.md`(設計意図層)を受けて、本書はそれを実装可能な形に落とす:
> ホスト構成、データモデル(DDL)、状態機械、デプロイ経路、reconcile、API 面、
> セキュリティの柵、構築順序。
> 背骨を一言で:**管制面 Postgres が「期望状態」を持ち、現実(コンテナ/ユーザ DB/
> ディスク)が「実際状態」。プラットフォームの仕事のすべて = 実際を期望へ収束させること。**

---

## 0. 本書で確定した決定(個別に否決可)

| # | 決定 | 一言の理由 |
|---|---|---|
| 1 | 管制面とユーザ DB は**別々の Postgres インスタンス**(どちらも psql) | ユーザがテナント側インスタンスを潰しても、管制面(最後の砦のパネル)は生きていること |
| 2 | 4 種のリソースは**`resources` スーパーテーブル 1 枚 + 種別毎の detail テーブル 4 枚** | 「4 種は対称」を字義通りに:注入の FK が綺麗、管理画面はクエリ 1 本、ゴミ箱はロジック 1 組 |
| 3 | deploy hook は **image digest を運ぶ**(tag ではなく)。プラットフォームは digest で pull | digest は内容アドレス ⇒ 他人が tag を上書きしても毒は入らない。registry の per-repo ACL が不要になる |
| 4 | registry と deploy hook は**会社 IP 許可リストから除外**(公開 + 各自の認証) | GitHub-hosted runner の IP は予測不能で、公網到達が必須;認証は htpasswd / HMAC |
| 5 | reconcile = **起動時フル + 周期ライト**;**env のドリフトは追わない** | env は起動の瞬間にだけ解決(rotate 後に旧コンテナが旧値を持つのは仕様)。reconcile は「走るべきものが走っている」ことだけを見る |
| 6 | ホストは 2 層:**infra compose(プラットフォーム不干渉)**と**プラットフォーム管理のユーザコンテナ** | 鶏と卵:プラットフォーム自身の Postgres をプラットフォームが起動することはできない |

---

## 1. ホスト構成

```
単機(Linux。今は香橙派 arm64、後で x86_64 機にも)
├─ systemd: tsubomi-server          ← プラットフォーム本体。ホスト直走り、docker.sock 保持(bollard)
├─ infra compose(IaC 管理、プラットフォームは触らない) /srv/tsubomi/infra/docker-compose.yml
│   ├─ traefik        :80/:443      入口 + Let's Encrypt + ipAllowList
│   ├─ registry       127.0.0.1:5000(ローカル pull 用)。push は traefik 経由 registry.<ドメイン>
│   ├─ pg-platform    127.0.0.1:5434  管制面メタデータ(loopback のみ、コンテナから到達不能)
│   ├─ pg-tenant      infra 内部網     ユーザ DB インスタンス(直接は公開しない)
│   ├─ pgbouncer      host:6432       プール + 外部接続の入口(ファイアウォールで CIDR 制限)
│   └─ valkey         infra 内部網     キャッシュ(M5)
├─ プラットフォーム管理のユーザコンテナ   docker network: tsubomi-edge
└─ /srv/tsubomi/
    ├─ volumes/<user_id>/<volume_id>/   ボリューム実体
    ├─ trash/                            ゴミ箱実体(移動されたボリューム / dump)
    └─ backups/                          日次バックアップ
```

**ネットワーク隔離の要点**
- ユーザコンテナは `tsubomi-edge` のみ(traefik に接続)。infra 内部網にユーザ
  コンテナは繋がない ⇒ pg-platform / registry の内部面に触れない。
- pg-platform は `127.0.0.1:5434` バインド:bridge のコンテナはホストの loopback
  に到達できず、天然に隔離。
- pgbouncer は `host:6432` で公開し、**iptables の DOCKER-USER チェーン**で送信元を
  制限:会社 CIDR + docker bridge 網段(172.16.0.0/12)。
  - 後者は「接続文字列 1 本がどこでも使える」ため:コンテナ内 app も社内デバッグも
    **同じ** `db.<ドメイン>:6432` の文字列。
  - Docker が ufw を素通りするのは有名な穴。CIDR 制限は DOCKER-USER に置くこと。
    ufw のルールだけでは効かない。
  - **ワイヤは client TLS(自己署名、`sslmode=require`)で暗号化済み**(M1 で実装):
    pgbouncer が起動時に証明書を生成し、平文接続(`sslmode=disable`)を拒否する。
    接続文字列も `sslmode=require` で出す。CIDR 制限と二重で LAN 上の受動的盗聴も塞ぐ。
- ファイアウォール:80/443(traefik)、6432(CIDR 制限)、22。他は全部閉じる。
  tsubomi-server は `127.0.0.1:9090` で待ち(8080 は同居の amber)、traefik 経由 `paas.<ドメイン>`
  (会社 CIDR)で公開。

**ドメイン**
- プラットフォーム UI/API:`paas.<ドメイン>`(会社 CIDR)
- registry:`registry.<ドメイン>`(公開 + basic auth;決定 #4)
- deploy hook:`paas.<ドメイン>/api/hook/deploy`(公開 + HMAC;決定 #4)
- ユーザ service:`<service[-ランダム語]>.<ドメイン>`(会社 CIDR、`public=true` で除外)

---

## 2. データモデル(管制面 DDL)

```sql
-- ============ アイデンティティ(実装済み:migrations/20260613000001_auth.sql、M0)============
-- 構造は amber からの移植:users + credentials の 2 表(credential_type:
-- google/passkey。v2 §7 の passkey 用に予約)。tsubomi の差分:role カラム、
-- email NOT NULL、session / oauth-state / PKCE authcode を Postgres に置く
-- (amber は Redis;単回消費は DELETE..RETURNING で等価に)。
-- テーブル:users(role: user/owner)/ credentials / sessions(sha256)/
--   cli_tokens(tbm_ プレフィックス、sha256)/ oauth_states / authcodes(PKCE)。
-- owner ≤ 2 はアプリ層で検証;owner は env TSUBOMI_OWNER_EMAILS で種付けし
-- ログイン時に昇格(first-login-wins はしない);Google の hd ドメイン制限
-- TSUBOMI_ALLOWED_HD(カンマ区切り複数可)はサーバ側で検証。
-- 完全な DDL はマイグレーションファイル参照。ここでは重複させない。

-- ============ リソースのスーパーテーブル(決定 #2)============
create table resources (
  id           uuid primary key default gen_random_uuid(),
  user_id      uuid not null references users(id),
  kind         text not null check (kind in ('service','database','cache','volume')),
  display_name text not null,
  anon_seq     int  not null,        -- 管理画面の匿名番号(user+kind 内で連番):service1/2…
  created_at   timestamptz not null default now(),
  deleted_at   timestamptz,          -- 非 NULL = ゴミ箱の中
  purge_after  timestamptz,          -- = deleted_at + 3d。reconcile が期限到来で物理削除
  trash_meta   jsonb,                -- 復元に必要なもの:dump パス / trash パスなど
  unique (user_id, kind, display_name),
  unique (user_id, kind, anon_seq),
  unique (id, kind)                  -- ↓ detail / injections の「kind 付き複合 FK」用。
);                                   --   「service にしか注入できない」を DB 制約にする

-- ============ detail 4 枚(スーパーテーブルと 1:1)============
create table service_details (
  resource_id    uuid primary key,
  kind           text not null default 'service' check (kind = 'service'),
  foreign key (resource_id, kind) references resources(id, kind) on delete cascade,
  repo           text,                          -- "owner/name"。ユーザ自身の gh で作成
  subdomain      text unique not null,
  deploy_key_enc bytea not null,                -- HMAC の原文。at-rest 暗号化(§7)
  image_digest   text,                          -- 現在走るべきイメージ(sha256:…、決定 #3)
  desired_state  text not null default 'stopped' check (desired_state in ('running','stopped')),
  phase          text not null default 'created'
                 check (phase in ('created','deploying','running','stopped','failed')),
  phase_detail   text,                          -- 失敗理由など。UI/CLI 向け
  memory_mb      int  not null default 512,     -- --memory 硬上限。初日から
  cpu_shares     int  not null default 1024,    -- --cpu-shares ソフト制限
  public         bool not null default false,   -- true = ipAllowList から除外
  compose_spec   jsonb,                         -- null=単一コンテナ;{entry,main,services:[…]}(M6)
  last_deploy_at timestamptz
);

create table database_details (
  resource_id  uuid primary key,
  kind         text not null default 'database' check (kind = 'database'),
  foreign key (resource_id, kind) references resources(id, kind) on delete cascade,
  pg_dbname    text unique not null,            -- db_<shortid>。pg-tenant インスタンス内。
                                                --   display_name(resources)とは別 — 単一実例で
                                                --   グローバル一意な wire 名が要るため(改名は接続文字列に触れない)
  rotated_at   timestamptz                      -- human role の最後の rotate 時刻(UI の「失効済み」ソフト提示)
);

-- database の登録資格情報は **2 つの role**(M0 設計の単一 pg_role/password から改訂)。
-- 同じ DB に対しどちらも全権だが、用途・rotate 方針・到達経路を分ける:
--   app   = 内部。デプロイ済み service に注入(M3)、内部路径、既定では rotate しない
--           → 「外部 key の rotate が走っている service を切らない」を成立させる。
--   human = 外部。ローカル開発 / DBeaver / `tbm db connect` / web SQL が使う。pgbouncer の
--           外部入口(会社 CIDR)経由、rotate 可(rotate = 旧文字列即死、再デプロイで反映)。
-- 隔離しているのは「漏洩の被害面 + rotate が service を切らないこと」であって権限ではない
-- (漏れた human key は rotate するまで当該 DB に全権 — だから rotate は human を既定にする)。
create table database_roles (
  resource_id  uuid not null references resources(id) on delete cascade,
  role_kind    text not null check (role_kind in ('app','human')),
  pg_role      text unique not null,
  password_enc bytea not null,                  -- 復元は「同じパスワードで再作成」⇒ 復元可能な保存が必須(v2 §11)
  conn_limit   int  not null default 20,        -- CREATE ROLE … CONNECTION LIMIT
  primary key (resource_id, role_kind)
);

create table cache_details (
  resource_id  uuid primary key,
  kind         text not null default 'cache' check (kind = 'cache'),
  foreign key (resource_id, kind) references resources(id, kind) on delete cascade,
  acl_user     text unique not null,
  namespace    text unique not null,            -- valkey ACL ~namespace:* プレフィックス
  password_enc bytea not null
);

create table volume_details (
  resource_id uuid primary key,
  kind        text not null default 'volume' check (kind = 'volume'),
  foreign key (resource_id, kind) references resources(id, kind) on delete cascade,
  host_path   text unique not null              -- /srv/tsubomi/volumes/<user>/<id>
);

-- ============ env と注入 ============
-- 静的 env:人 / AI が入れたリテラル値
create table service_env (
  service_id uuid not null,
  skind      text not null default 'service' check (skind = 'service'),
  foreign key (service_id, skind) references resources(id, kind) on delete cascade,
  key        text not null,
  value_enc  bytea not null,
  primary key (service_id, key)
);

-- 注入:「バインディング」だけを保存。値はコンテナ起動の瞬間に都度解決(決定 #5 の根拠)
create table injections (
  id          uuid primary key default gen_random_uuid(),
  service_id  uuid not null,
  skind       text not null default 'service' check (skind = 'service'),
  resource_id uuid not null references resources(id) on delete cascade,
  env_var     text not null,        -- DATABASE_URL / REDIS_URL / STORAGE_PATH …
  mount_path  text,                 -- volume のみ:コンテナ内マウント先。デフォルト /data/<name>
  foreign key (service_id, skind) references resources(id, kind) on delete cascade,
  unique (service_id, env_var)
);
-- ソフト削除(deleted_at)はこの表に触れない ⇒ 注入は宙吊りで失効、復元すれば
-- 自動的に生き返る — v2 §11 の意味論がタダで成立。
-- 物理削除(purge)はカスケードでバインディングを掃除。

-- ============ デプロイ ============
create table deploys (
  id           uuid primary key default gen_random_uuid(),
  service_id   uuid not null references resources(id) on delete cascade,
  git_sha      text not null,
  image_digest text not null,
  status       text not null check (status in ('received','pulling','starting','succeeded','failed')),
  error        text,
  created_at   timestamptz not null default now(),
  finished_at  timestamptz
);

create table deploy_nonces (
  service_id uuid not null references resources(id) on delete cascade,
  nonce      text not null,
  seen_at    timestamptz not null default now(),
  primary key (service_id, nonce)               -- reconcile が 1 時間超の行を周期的に掃除
);

-- ============ ガバナンス ============
create table audit_log (
  id              bigint generated always as identity primary key,
  actor_id        uuid references users(id),
  action          text not null,    -- 'owner.stop_service' / 'db.rotate' / 'trash.purge' …
  target_resource uuid,
  target_user     uuid,
  detail          jsonb,
  created_at      timestamptz not null default now()
);

create table platform_config (
  key        text primary key,      -- 'viewer_password_hash' / 'quota_defaults' / …
  value      jsonb not null,
  updated_at timestamptz not null default now()
);
```

---

## 3. 状態機械と reconcile

### service の二重状態(期望 vs 観測)
- `desired_state ∈ {running, stopped}`:ユーザが**望む**状態。ユーザ操作
  (start/stop/deploy/削除)はこれだけを変える。
- `phase ∈ {created, deploying, running, stopped, failed}`:**実際**どこにいるか。
  プラットフォームが観測して書き戻す。ユーザからは読み取り専用。

```
created ──deploy──▶ deploying ──成功──▶ running ◀──start── stopped
                        │                  │
                        └─失敗──▶ failed   └──stop / crash──▶ stopped / failed
```

### reconcile(決定 #5)
**起動時に一度フル + 60 秒毎にライト**。職務リスト(意図的に短い):

1. **存在の収束**:`desired_state=running` かつ未ソフト削除の service について、
   スペック(image_digest + 解決済み env + マウント + limits + labels)通りの
   コンテナが存在して走っていることを確認。欠落 / 異常 → スペック通りに再作成。
   コンテナ自身の `restart=unless-stopped` が第一の保険、reconcile は第二。
2. **孤児の掃除**:`tsubomi.service_id` ラベルを持つが DB に生きた行が無い
   コンテナ → 停止 + 削除。
3. **ゴミ箱の期限到来の物理削除**:`purge_after < now()` → 残骸を DROP / trash
   実体を削除 / 行を物理削除(カスケードでバインディング掃除)。
4. **雑務**:deploy_nonces の古い行掃除、session の期限切れ掃除、ディスク水位
   チェック(閾値超え → Resend で owner に通知)、registry GC(週次、未参照
   blob の削除)。

**意図的にやらないこと**:env / 注入のドリフトは追わない。env は起動の瞬間に
だけ解決され、env 変更 / rotate / リソース削除のどれも自動再起動を**引き起こさ
ない** — 「再デプロイして初めて効く」は v2 §11 が決めた仕様であり、reconcile が
勝手に再起動するのはむしろ仕様違反。

---

## 4. デプロイ経路

### 4a. service 作成(CLI がオーケストレーション。プラットフォームは GitHub 資格情報ゼロ)

```
tbm service create myapp
  1. プラットフォーム API:resources + service_details を挿入。deploy_key、
     subdomain、registry 資格情報を生成
  2. CLI が「ユーザ自身の gh」で:
     gh repo create <user>/myapp --private
     gh secret set TSUBOMI_DEPLOY_KEY / REGISTRY_USER / REGISTRY_PASS
     .github/workflows/deploy.yml を書き込む(テンプレートはプラットフォーム API が提供)
  3. git push ⇒ 経路 4b が自動で走る
```

### 4b. push → 公開

```
GH Action(ユーザのリポジトリ)
  1. build:nixpacks / リポジトリ内の Dockerfile
  2. docker push registry.<ドメイン>/<service_id>:<git_sha>
  3. push が返した image digest(sha256:…)を捕まえる
  4. POST paas.<ドメイン>/api/hook/deploy
       body: { service_id, git_sha, image_digest, ts, nonce }
       X-Tsubomi-Signature: hex(hmac_sha256(deploy_key, raw_body))

プラットフォーム(hook 受信):
  1. service を引く → deploy_key で HMAC を再計算、定数時間比較
  2. |now − ts| ≤ 300s;(service_id, nonce) が未見(INSERT が一意制約に当たれば拒否)
  3. deploys(received) を挿入 → 非同期パイプライン:
     pull(digest 指定、127.0.0.1:5000 から)
     → env 解決(§5)
     → コンテナ create:limits + ボリュームマウント + tsubomi labels + traefik labels
       (Host ルーティング / ipAllowList)
     → 新を起動 → 旧を停止 → 旧を削除(秒単位の瞬断。社内ツールとして許容済み)
     → phase=running、deploys.status=succeeded、image_digest を書き込み
  どこかで失敗:deploys.status=failed + error、phase=failed。旧コンテナは触らない
  (旧バージョンが走り続ける)
```

- **HMAC = 権限そのもの**:service X の deploy key を持つこと = X をデプロイする
  権限。ユーザ認証を重ねない。
- **digest ピン留め = 毒入れ防御**(決定 #3):プラットフォームは digest でしか
  pull しないので、registry 上の tag を誰が上書きしても無意味 ⇒ registry は
  グローバル htpasswd(ユーザ毎アカウント)で足り、per-repo ACL は不要。
- **ローカルビルドの退路**(v2 §8 の 2 本目):`tbm deploy --local` = ローカルで
  docker build → push → 自分で hook を叩く。deploy key は CLI がプラットフォーム
  API から取得(ユーザは自分の service への読み取り権がある)。GitHub Secrets に
  依存しない。

### 4c. デプロイ時系列での env 注入
create(env 注入済み)→ start の順 — コンテナは最初の命令から全 env が見える
(v2 §3「先に注入、後に起動」)。

---

## 5. 注入の解決(起動の瞬間)

```
コンテナ env = service_env(静的、復号)
            ∪ injections を 1 件ずつ解決:
                database → DATABASE_URL = postgres://<app_role>:<pass>@<内部入口>:6432/<dbname>
                cache    → REDIS_URL    = redis://:<pass>@cache.<ドメイン>:6379  (M5)
                volume   → host_path を mount_path にマウント;env_var = mount_path;
                           ディレクトリが無ければ先に mkdir
```

- database 注入は **app role(内部)**を解決する(human role ではない)。これにより
  **外部 key の rotate が走っている service を切らない**(§2 database_roles)。
- 注入される接続文字列は**内部入口**を指す(コンテナは社外に出ず内部路径)。**内部入口の
  実体は M3 で確定**(候補:pgbouncer を edge+infra 両網に跨らせ docker 内部名で到達。
  ユーザコンテナを infra 網に繋がない §1 の隔離は不変)。一方 human が手にする外部文字列
  (`tbm db connect` / `/url` / DBeaver)は `db.<ドメイン>:6432`(会社 CIDR)を指す。
  両者は **別 role の別文字列**だが、ユーザが見るのは外部 1 本だけ(内部は平台が注入する
  不可視の配管)。
- 注入先のリソースがソフト削除済み → その 1 件は空に解決され UI/CLI で「失効」
  表示。service は普通に起動する(v2 §11:特例ではない)。

---

## 6. API 面(web と CLI が唯二のフロントエンド。ハンドラは同一)

認証:`session cookie`(web)と `Bearer cli_token`(CLI)の 2 つの extractor →
同じ `AuthCtx { user_id, role }` に解決。下流に分岐なし。

```
auth      GET  /api/auth/google/start → callback → session;POST /api/auth/logout
          POST /api/cli-tokens(web のみ発行)/ GET / DELETE(失効)

resources GET /api/resources                       4 種をフラットに(dashboard はクエリ 1 本)
service   POST/GET/DELETE /api/services[/:id]
          POST /api/services/:id/start|stop        desired_state を変える
          GET  /api/services/:id/logs?tail=        docker logs の素通し
          GET  /api/services/:id/deploys           デプロイ履歴
          PUT  /api/services/:id/env               静的 env の一括置換
database  POST/GET/DELETE /api/databases[/:id]
          POST /api/databases/:id/rotate           パスワード変更 + 旧文字列は即死
          GET  /api/databases/:id/url              接続文字列(表示箇所に警告文)
          POST /api/databases/:id/query            web SQL クライアント(その DB 自身の制限付き資格情報で接続!)
volume    POST/GET/DELETE /api/volumes[/:id]
          GET/PUT/DELETE /api/volumes/:id/files?path=   ファイル管理(擬似ルート、§7 トラバーサル防御)
inject    POST   /api/services/:id/injections      { resource_id, env_var, mount_path? }
          DELETE /api/injections/:id
trash     GET /api/trash;POST /api/trash/:id/restore;DELETE /api/trash/:id(永久)
hook      POST /api/hook/deploy                    HMAC、session なし(IP 除外)
admin     POST /api/admin/view-login               共有パスワード → 読み取り専用ビュー(匿名番号+使用量)
          GET  /api/admin/overview|ranking
          POST /api/admin/resources/:id/stop|delete  owner 本人確認 + Resend 検証コード
```

CLI のコマンド面は 1:1 対応(各コマンド = API 呼び出し 1 本。AI が理解すべき
概念は「リソース」と「注入」の 2 つだけ):

```
tbm login                                        # ブラウザで CLI token を発行
tbm service create|list|status|logs|start|stop|delete
tbm deploy [--local]
tbm db create|list|url|rotate|delete
tbm db connect <db>                              # 無密码:CLI 認証済み → human 外部文字列 → psql を exec(PGPASSWORD、履歴に残さない)
tbm volume create|list|delete|rename
tbm volume ls|put|get|rm|mkdir|mv <vol> …        # 假根内のファイル操作(全て safe_path を通る)
tbm cache create|…                               # M5
tbm inject <resource> --into <service> [--as ENV] [--mount /path]
tbm eject <injection>
tbm env set|unset|list
tbm trash list|restore|purge
```

---

## 7. セキュリティの柵一覧(どの柵がどこに住むか)

| 柵 | 置き場所 | 備考 |
|---|---|---|
| 会社 IP 許可リスト | traefik ipAllowList middleware(全 service にデフォルト適用) | `public=true` で除外;**registry + hook は強制除外**(決定 #4) |
| メモリ / CPU クォータ | docker create のパラメータ(memory_mb / cpu_shares) | 初日から;OOM は単一コンテナだけを殺す |
| パストラバーサル防御 | volume ファイル API:Linux `openat2(RESOLVE_BENEATH \| RESOLVE_NO_SYMLINKS)` | カーネルレベルで「ユーザのルート内」を断言。手書き resolve より硬い。唯一のハードな安全境界 |
| ユーザ DB の隔離 | pg-tenant:DB 毎に `CREATE DATABASE` + `CREATE ROLE … NOSUPERUSER NOCREATEDB CONNECTION LIMIT n` | web SQL クライアントは**必ずその DB 自身の資格情報で接続**。admin 資格情報での代理クエリは絶対にしない |
| 管制面の隔離 | pg-platform は loopback のみ + 別の admin アイデンティティ(決定 #1) | ユーザコンテナは物理的に触れない |
| 資格情報の分立 | session / cli_token はハッシュ保存;deploy_key / DB パスワード / env 値は暗号化して復元可能 | 「ハッシュにできるか」は「プラットフォームが原文を必要とするか」で決まる。好みではない |
| at-rest 暗号化 | XChaCha20-Poly1305。master key は `/etc/tsubomi/master.key`(root のみ) | 正直な境界:同一ホストに鍵があるので、守れるのはバックアップ / dump の漏洩であってホスト陥落ではない |
| リプレイ防御 | hook の ts ± 300s + nonce 一意制約 | |
| owner の高リスク操作 | バックエンドが操作毎に role=owner を検証 + Resend 検証コード;ユーザが自分のを消すときは名前入力確認 | フロントの表示制御はただの UX |
| 監査 | audit_log:owner の代理操作、rotate、削除、復元を全部記録 | ガバナンス可視性のもう半分 |

---

## 8. バックアップとゴミ箱(仕組みは初日から、数値は後で調整)

- **日次**:pg-platform 全量 dump + pg-tenant の DB 毎 `pg_dump` + volumes の
  rsync スナップショット → `/srv/tsubomi/backups/`。保持 7 日(後で調整)。
- **削除フロー**(すべてソフト削除、猶予 3 日):
  - service:コンテナ停止・削除 → 行に `deleted_at`(ボリュームは連動しない)
  - database:`pg_dump`(無圧縮)→ dump パスを `trash_meta` に → 再作成メタ
    (role / パスワード / DB 名)は detail 行に元からある → DROP DATABASE
  - volume / cache:実体を `/srv/tsubomi/trash/` に `mv`(ほぼゼロコスト)、
    新パスを `trash_meta` に
- **復元**:database = 同名 role + 同じパスワードで再作成 + dump を流し込む;
  volume = `mv` で戻す + 注入は自動的に生き返る。消さなかったかのように。
- **物理削除**:reconcile が期限到来で実行、またはユーザ / owner がゴミ箱で
  「永久に削除」。

---

## 9. 構築順序(プラットフォーム → database → ファイルシステム → service → valkey)

並べ方の原則:**先に「純ソフトウェア」のリソース(db/volume。外部統合に触らない)、
かつ各フェーズが単独で完結した価値を持つ** — database が出来た時点で社内 Neon
(pooler が会社から到達可能なので、ローカル開発は接続文字列だけで使える。
デプロイゼロでもユーザが付く);volume が出来た時点で社内ファイル置き場
(web ファイルブラウザ)。動詞「注入」は service フェーズに送る(注入の相手は
そもそも service)。
マイグレーションはフェーズ毎に追加:M0 でアイデンティティ + スーパーテーブル +
audit。以後の各フェーズが自分の detail テーブルを足す。

| M | 内容 | 完了判定 |
|---|---|---|
| **M0 基盤** ✅ | infra compose(管制面 pg。**registry は M3 に先送り**)+ マイグレーション(アイデンティティ)+ Google ログイン(hd 検証)+ session + owner 種付け + CLI token(PKCE、loopback ログイン)+ dashboard 骨格 + **tbm 配布系**(4 ターゲットビルド / インストーラ 3 種 / 不可変リリース / 残留物ゼロ uninstall) | ブラウザでログインできる;`tbm login` はブラウザの「許可する」だけで完了;本番(香橙派)で稼働中 |
| **M1 database** ✅ | DB 作成(CREATE DATABASE + 制限付き role)/ rotate / 接続文字列ページ(警告付き)+ web SQL クライアント(その DB 自身の資格情報で接続)+ conn limit + 日次バックアップ開始 + ゴミ箱(ソフト削除 → pg_dump を trash に → 同じパスワードで再作成して復元) | 社内ネットから `psql <接続文字列>` が通る;rotate で旧文字列は即死;削除 → 復元が「消さなかったかのよう」 |
| **M2 volume(ファイルシステム)** ✅ | ファイル API(openat2 トラバーサル防御、ハード境界。dev は canonicalize フォールバック)+ web ファイルブラウザ(擬似ルート、パスは URL の splat に持つ)+ `tbm volume` フル(create/list/rename/delete + ls/put/get/rm/mkdir/mv)+ ゴミ箱(trash へ mv / 復元)の web/CLI 入口 + volumes の日次 rsync バックアップ | トラバーサルのテストケース全拒否;ファイル置き場として日常に使える |
| **M3 service** | registry 開始 + create(ユーザ gh オーケストレーション + workflow テンプレート)→ Action → hook(HMAC/nonce/digest)→ コンテナ + traefik ルーティング + limits + logs/start/stop + **注入機構**(db/volume → service;静的 env)+ reconcile v1(存在 + 孤児) | push から 30 秒で `https://myapp.<ドメイン>` が開く;`tbm inject` で app が DB / ボリュームに繋がる;ホスト再起動から自己回復 |
| **M4 ガバナンス** | 管理画面(共有パスワードの読み取り専用ビュー / ランキング / 最後の砦の操作 + Resend 検証コード)+ ディスク警報 + audit_log 補完 | owner が見える・対処できる |
| **M5 cache(valkey)** | ACL:namespace プレフィックス + コマンド許可リスト;REDIS_URL 注入 | 越境は NOPERM |
| **M6** | compose 複数コンテナ service(cgroup parent slice で合算クォータ)+ passkey + クォータ数値の調整 | |

**引き受けた並べ方のコスト**:リスク最大のデプロイ経路(Action→registry→hook→
コンテナ→ルーティング)に触るのが M3 になる。緩和:M1 の間に半日で**曳光弾**を
1 発撃つ — 適当なリポジトリの Action でイメージを仮 registry に push + `curl` で
hook を模擬 + 手打ち `docker run` に traefik label。仮説の検証だけしてプラット
フォームのコードは書かない。検証済みなら、後回しは「順序の問題」だけになり
未知のリスクは残らない。

---

## 10. 新たに引き受けたコスト / 未決

- **registry + hook が公網到達可能**(決定 #4):認証は htpasswd / HMAC だけで
  IP 層は無し。引き受け済み(digest ピン留め + HMAC で被害面は「ノイズ」級まで
  圧縮される)。
- **デプロイ切替に秒単位の瞬断**(旧停止 → 新起動。ブルーグリーンではない)。
  社内ツールとして引き受け済み。
- **pgbouncer が docker 網段を許可**(「1 本の接続文字列がどこでも使える」ため)
  ⇒ どのユーザコンテナも文字列さえ持てばどの DB にも繋がれる — これは v2 §4
  「会社 IP 内で文字列を持つ者は繋がれる」という引き受け済み境界のコンテナ側への
  投影であり、新しい信頼は増えていない。
- **ホストのマルチアーキテクチャ**:今は香橙派(ARM64/aarch64)、後で x86_64 機に
  移す/増やす ⇒ 成果物は**初日から両アーキテクチャ**:infra イメージは multi-arch
  (postgres/traefik/registry の公式イメージは対応済み);M3 の GitHub Actions は
  buildx で **arm64 + amd64 両方のイメージ**を出す(デプロイ時にホストが自動選択);
  tbm CLI の配布ターゲットは aarch64-apple-darwin / aarch64-unknown-linux-gnu /
  x86_64-unknown-linux-gnu。プラットフォーム自身のバイナリ(tsubomi-server)は
  ターゲット機上でビルドするかクロスコンパイルで両対応。
- 未決(いずれも M0–M2 を塞がない):compose_spec のスキーマ
  詳細化(M6 前);バックアップの遠隔保管;registry GC のパラメータ。
