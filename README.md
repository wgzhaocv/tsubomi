# tsubomi 蕾

社内 PaaS プラットフォーム(セルフホストの「基礎版 Vercel + Neon」)。設計ドキュメント:
[doc/paas-design-v2.md](doc/paas-design-v2.md)(意図)/ [doc/paas-tech-design.md](doc/paas-tech-design.md)(技術設計)。
開発の約束事は [CLAUDE.md](CLAUDE.md) を参照。

```
tsubomi/
├── Cargo.toml              # workspace(resolver 3、release プロファイル)
├── crates/
│   ├── shared/             # tsubomi-shared — server と cli が共有する serde 型
│   ├── server/             # tsubomi-server — axum 管制面(bin)
│   └── cli/                # tsubomi-cli — clap クライアント(bin 名:`tbm`)
├── infra/                  # インフラ層の docker compose(管制面 postgres など)
├── migrations/             # sqlx マイグレーション。サーバ起動時に埋め込みで実行
├── web/                    # Vite(vite-plus / `vp`)+ React + TS + Tailwind v4 + shadcn
└── justfile
```

## 前提

- Rust(`rust-toolchain.toml` でピン)
- フロントエンド用の [bun](https://bun.sh)
- [just](https://github.com/casey/just) + Docker

## 開発

```bash
just web-install         # 初回のみ — web の依存をインストール
just db-up               # infra:pg-platform(:5434)+ pg-tenant(:5435)+ pgbouncer(:6432)
cp .env.example .env     # GOOGLE_CLIENT_ID / GOOGLE_CLIENT_SECRET + M1 の TSUBOMI_MASTER_KEY を埋める
just dev                 # server :9090 + web :5173 を同時起動。Ctrl-C で両方停止
```

Google OAuth クライアント:Google Cloud Console で作成(種別:Web application、
同意画面は **Internal**)。Authorized redirect URI は
`http://localhost:5173/api/auth/google/callback`。ログインは
`TSUBOMI_ALLOWED_HD` の Workspace ドメインに制限される(サーバ側 `hd` 検証)。

## デプロイ

単一ホスト運用・ホスト直走り。サーバは **host ネットワーク**で `127.0.0.1:9090`(本番は
`TSUBOMI_BIND_ADDR`)に待ち受け、前段の TLS リバースプロキシ越しに公開する。設定は
ホスト毎の **`.env.production`**(git 管理外)。host ネットなので
`DATABASE_URL=127.0.0.1:5434` が dev / 本番で共通のまま通る。`just` / ソース / sh が
無いマシン(Windows 等)でも `docker compose` だけで完結する。

配布は公開イメージ **`docker.io/wgzhaofumi/tsubomi`**(multi-arch: arm64 = 香橙派 /
amd64 = x86_64 VPS)。**運用側はこれを pull するだけ — 自前ビルドは不要**。使う
イメージは `compose.prod.yml` の既定値に固定済みなので `.env.production` には書かない
(別タグを試すときだけ環境変数 `TSUBOMI_IMAGE` で上書き)。`.env.production` は
**サーバ設定だけ**を持つ。

### 自分の VPS で動かす(本番セットアップ)

新しい VPS に必要なのは **Docker だけ**(ソース・just・sh は不要)。以下は
**直 VPS(グローバル IP + traefik 自身が :443 を終端 = モード B)**の最短路。上流が TLS を
終端する構成(Cloudflare Tunnel 等 = モード A。香橙派の実態)との差分は 6 に、
両モードの詳細は `doc/paas-m3-design.md` §13 に。

1. **Docker を入れる**:`curl -fsSL https://get.docker.com | sh`
2. **土台を先に作る**。`tsubomi-edge` は compose が `external` 参照するので**無いと
   `up` が失敗する**。`/srv/tsubomi` を先に掘るのは、docker に作らせると root 所有になり
   プラットフォームが書けないため:

   ```bash
   docker network create tsubomi-edge
   sudo mkdir -p /srv/tsubomi/{traefik-dynamic,traefik-plugins,backups,trash,volumes,releases}
   mkdir -p ~/tsubomi-deploy && cd ~/tsubomi-deploy
   ```
3. **compose 定義と設定を置く**:
   - `compose.prod.yml` — リポジトリからコピー(pg-tenant 初期化 / pgbouncer 設定 /
     userlist は全部この中に inline 埋め込み済み = 別ファイル不要)
   - `.env.production` — 同じ場所に新規作成(`.env.example` がひな形)
   - overlay(TLS / 公開 DB / 公開 cache)は 7 で足す
4. **`.env.production` を本番値で埋める**(全量は `.env.example` 参照)。
   **★印は省略すると起動しない / 静かに壊れる**:

   ```env
   # ── 管制面 / テナント DB ──
   PG_PLATFORM_PASSWORD=<強いパスワード。英数字のみ>
   DATABASE_URL=postgres://tsubomi:<同じパスワード>@127.0.0.1:5434/tsubomi_platform
   PG_TENANT_PASSWORD=<強いパスワード。英数字のみ>
   TENANT_ADMIN_URL=postgres://tsubomi_admin:<同じパスワード>@127.0.0.1:5435/postgres
   PGBOUNCER_AUTH_PASSWORD=<強いパスワード。英数字のみ>   # ★compose が :? で必須化
   TSUBOMI_MASTER_KEY=<base64 32 bytes>                  # head -c 32 /dev/urandom | base64
   # ── cache(valkey)──
   TSUBOMI_VALKEY_ADMIN_PASS=<強いパスワード。英数字のみ> # ★compose が :? で必須化
   TSUBOMI_VALKEY_ADMIN_URL=redis://tsubomi-admin:<同じパスワード>@127.0.0.1:6433
                                                         # ★未設定だと dev の既定値を掴み cache が全滅
   # ── 認証 / 身元 ──
   GOOGLE_CLIENT_ID=...
   GOOGLE_CLIENT_SECRET=...
   GOOGLE_REDIRECT_URI=https://<ドメイン>/api/auth/google/callback
   TSUBOMI_SERVER_URL=https://<ドメイン>
   TSUBOMI_ALLOWED_HD=<会社ドメイン>      # 複数はカンマ区切り
   TSUBOMI_OWNER_EMAILS=<owner のメール>   # 複数はカンマ区切り
   TSUBOMI_COOKIE_SECURE=true             # HTTPS 必須
   # ── service(デプロイ経路 + ルーティング)──
   TSUBOMI_DOMAIN=<ドメイン>                       # service は <subdomain>.<ドメイン> で公開
   TSUBOMI_PLATFORMS=linux/amd64                   # ★ホストの arch(既定 linux/arm64)。
                                                   #   x86 VPS で直さないとユーザ app が動かない
   TSUBOMI_REGISTRY_PUSH=registry.<ドメイン>       # CI が push する公開 registry(pull と別 = 認証入口を張る)
   TSUBOMI_BIND_ADDR=0.0.0.0:9090                  # 直VPS:apex は traefik(コンテナ)→ host-gateway 経由で
                                                   #   届くので loopback 不可。:9090 は FW で塞ぐ
   TSUBOMI_TLS=true                                # traefik 自身が :443 + LE(モード B)
   TSUBOMI_ACME_EMAIL=<LE 通知メール>
   TRAEFIK_BIND_ADDR=0.0.0.0
   # ── DB 注入(service が受け取る内部接続文字列)──
   TSUBOMI_DB_INTERNAL_HOST=db.<ドメイン>  # ★pgbouncer の証明書と**同じ公開名**にする(下記 7)
   TSUBOMI_DB_INTERNAL_PORT=6432
   TSUBOMI_DB_SSLMODE=require              # ★既定は disable。pgbouncer は平文を拒否するので
                                           #   そのままだと注入された app が全部 DB に繋げない
   ```

   `PG_PLATFORM_PASSWORD` と `DATABASE_URL`、`PG_TENANT_PASSWORD` と `TENANT_ADMIN_URL`、
   `TSUBOMI_VALKEY_ADMIN_PASS` と `TSUBOMI_VALKEY_ADMIN_URL` のパスワードはそれぞれ
   **必ず一致**させる(compose が新規 pg / valkey をこの値で初期化する)。
   バックアップ / ゴミ箱の dump は host の `/srv/tsubomi` に出る(compose がマウント)。
5. **Google OAuth** に本番の redirect URI を追加:
   `https://<ドメイン>/api/auth/google/callback`
6. **前段ルーティングと DNS**。M3 で compose に traefik(file provider)+ registry が入った。
   2 モード(詳細は `doc/paas-m3-design.md` §13):
   - **(B) traefik 自身が :443 + Let's Encrypt**(直 VPS、`TSUBOMI_TLS=true`)— 前段プロキシ不要。
     `<ドメイン>` / `*.<ドメイン>` / `registry.<ドメイン>` の A を VPS のグローバル IP へ。
     LE は **tlsChallenge(:443)**なので **Cloudflare の proxy(オレンジ雲)は使えない** —
     DNS-only にする(ついでに CF proxy の body ≈100MB 制限も消えるので `docker push` が楽)。
     起動は `-f compose.prod.yml -f compose.prod.tls.yml`(overlay を複数使うなら下記の統合を先に)。
   - **(A) 上流が TLS 終端**(Cloudflare Tunnel / CF proxy / Caddy / nginx。`TSUBOMI_TLS` 未設定)—
     前段から 3 系統を転送:`<ドメイン>` → `127.0.0.1:9090`(apex / server。この場合 `TSUBOMI_BIND_ADDR`
     は `127.0.0.1:9090` でよい)、`*.<ドメイン>` → `127.0.0.1:80`(traefik → service コンテナ)、
     `registry.<ドメイン>` → `127.0.0.1:80`(traefik → registry、basicAuth)。
   - **overlay を 2 枚以上重ねるときは 1 枚に統合する**。`compose.prod.*.yml` はどれも traefik /
     valkey の `command`(リスト)を**後勝ちで全置換**するので、素直に並べると先のものが消える。
     統合先は **repo に無い名前**(例 `compose.prod.zz-merged.yml`)にすること — repo と同名の
     ファイルは `just ship` が毎回上書きして統合結果が消える。`zz-` 接頭辞は `-f` の末尾に
     並ぶので command が必ず勝つ。
7. **公開 DB / 公開 cache(任意。グローバル IP のある VPS でだけ成立)**。
   どちらも**平台側のコード変更は不要** — overlay + env + 証明書だけで開く。
   - **公開 DB**:`compose.prod.db-public.yml`(traefik に TCP 入口 `postgres` を生やし、
     pgbouncer の host publish を落とす)+ `.env.production` に
     `TSUBOMI_DB_PUBLIC_ENABLED=true` / `TSUBOMI_DB_PUBLIC_HOST=db.<ドメイン>` /
     `TSUBOMI_DB_PUBLIC_PORT=6432` / `TSUBOMI_DB_SSLMODE_EXTERNAL=verify-full`。
     プラットフォームが `db-tcp.yml`(TCP router + ipAllowList + backend = pgbouncer)を書く。
     **公開 DB は fail-closed** — 会社 IP 許可リストが**空だと入口を書かない**
     (`ipblock.rs`。空 = fail-open の HTTP 側とは別ポリシー:Postgres を黙ってインターネットへ
     晒さないため)。誰でも繋げてよいなら `0.0.0.0/0` を**明示的に**登録する。
   - **公開 cache**:`compose.prod.cache-public.yml`(valkey に TLS 口 `:6380` を生やす)+
     `TSUBOMI_CACHE_PUBLIC_ENABLED=true` / `_HOST=cache.<ドメイン>` / `_PORT=<公開ポート>`。
     単一 VPS なら TLS 口をそのまま公開ポートへ publish すればよい(香橙派は公網 IP が無いので
     frp + VPS の cache-gate を挟んでいる — VPS では丸ごと不要)。**cache 側に IP 許可リストは無い**
     (TLS + per-cache ACL パスワードのみ)。絞るならホストの iptables `DOCKER-USER` で。
   - **pgbouncer / valkey の LE 証明書は運用上の必須項**。compose が起動時に置くのは自己署名の
     **種**で、厳格に検証する駆動系(node-postgres 等)は繋げない。`db.<ドメイン>` と
     `cache.<ドメイン>` を acme.sh(:80 は traefik が使うので **DNS-01**)で取り、reloadcmd に
     `deploy/db-public/reload-pgb-cert.sh` / `deploy/cache-public/reload-valkey-cert.sh` を渡す
     (両スクリプトは「実際に出ている証明書が入れた物と一致するまで」確認して、駄目なら非零終了する)。
     **注入ホスト名 = この証明書の公開名**という不変式が全テナント app の DB 接続の生命線
     (`doc/paas-db-public-design.md`「証書名は仕組みの一部」/ DR は `doc/paas-dr-restore-runbook.md` §E)。
   - ホストのファイアウォールは **iptables の `DOCKER-USER` チェーン**で書く(ufw だけでは
     Docker が publish したポートを素通りする — design v2 §1)。
8. **起動**(公開イメージを pull し、infra + server をまとめて立てる):

   ```bash
   docker compose --env-file .env.production -f compose.prod.yml -f <overlay…> up -d
   ```

   server が起動時に migration を流し、`apex.yml` / `ipallow.yml` / `registry.yml` を
   動的設定ディレクトリへ書く(traefik が file watch で拾う)。
9. **確認 / ログ**:

   ```bash
   curl -fsS http://127.0.0.1:9090/api/health
   docker compose -f compose.prod.yml logs -f server
   ```
10. **更新(server だけ・ユーザ app 無瞬断)**:新しい `compose.prod.yml` を取得して
    `TSUBOMI_IMAGE=...:vN docker compose --env-file .env.production -f compose.prod.yml up -d server`
    (overlay を置いた機では在るもの全部を `-f` に連ねる — `just ship` は自動で全載せ)。
    **`up -d`(全 service)ではなく `up -d server` に絞る**のが要点 — traefik / pg / valkey などデータ面・
    入口を巻き込んで再生成せず、全 app の同時瞬断を避ける(infra は compose 内で digest 固定済みなので
    勝手に動かない)。停止は `docker compose -f compose.prod.yml stop`(`down` は使わない — コンテナを
    消して次の `up` で全部作り直す = 不要な全 app 瞬断になる。external 網も外す)。

### メンテナ向け:配布・更新

イメージを更新・配布するのは**メンテナだけ**。配り先で 2 通り。

**A. レジストリへ publish(別マシン / 不特定の VPS 用。各 VPS は `docker pull`)**

```bash
docker login docker.io
REGISTRY=docker.io/wgzhaofumi IMAGE=tsubomi TAG=v5 just release-image  # multi-arch push
# just 無し:  REGISTRY=docker.io/wgzhaofumi IMAGE=tsubomi TAG=v5 bash scripts/release-image.sh
```

publish 後、VPS 側は新タグで server だけ起こし直す:
`TSUBOMI_IMAGE=docker.io/wgzhaofumi/tsubomi:v5 docker compose --env-file .env.production -f compose.prod.yml up -d server`
(または `compose.prod.yml` の既定タグを上げて取得 → `up -d server`。`up -d server` に絞るのは上記 10 と同じ理由 —
infra を巻き込まずユーザ app を瞬断させない。初回構築だけは infra 一式が要るので全 `up -d`)。

**B. LAN 内ホストへ直送(香橙派など。Hub を介さず速い)**

ビルド機 → 対象ホストへ `docker save | ssh docker load` で直接渡し、その場で起こす。
対象のアーキを検出して native ビルドするので同アーキ(Mac arm64 → 香橙派 arm64)は高速:

```bash
HOST=zwg@192.168.0.106 just ship          # 既定タグ tsubomi:local で直送 + 起動
# HOST=user@ip TAG=v5 just ship
# just 無し:  HOST=zwg@192.168.0.106 bash scripts/ship.sh
```

事前に対象ホストへ `compose.prod.yml` と `.env.production` を置いておく(既定 `~/tsubomi-deploy`)。

## tbm CLI

インストール(配布物はサーバが配信。ドメインは自動注入される):

```bash
# macOS / Linux
curl -fsSL https://<ドメイン>/install.sh | sh && exec $SHELL
# Windows PowerShell:  irm https://<ドメイン>/install.ps1 | iex
# Windows cmd:         curl -fsSL https://<ドメイン>/install.bat -o %TEMP%\tbm-install.bat && %TEMP%\tbm-install.bat
```

```bash
tbm login                # 自動判定。ローカルはブラウザで「許可する」を押すだけ
                         # (RFC 8252 loopback)、SSH 先・ヘッドレスは自動でコピペ方式
tbm login --manual       # コピペ方式を強制(自動判定が漏れたとき。sudo / mosh 等)
tbm login --web          # ブラウザ方式を強制(VS Code Remote 等で上書きしたいとき)
tbm whoami
tbm update               # 手動セルフアップデート(バージョンチェックは通知のみ)
tbm uninstall            # 設定・PATH・本体まで残留物ゼロで削除

# M1 database
tbm db create <名前>     # DB 作成(プラットフォームが wire 名・role・パスワードを生成)
tbm db list
tbm db url <名前>        # 外部接続文字列(= パスワード。共有しない)
tbm db connect <名前>    # 無密码で psql 接続(PGPASSWORD、履歴に残さない)
tbm db query <名前> <SQL> # 任意 SQL を実行(psql 不要。web の SQL エディタと同じ経路。`-` で stdin)
tbm db fork <元> <新名>  # 構造 + データごと複製(--schema-only で構造だけ)。同期はしない
tbm db rotate <名前>     # パスワード再生成(古い接続文字列は即失効)
tbm db delete <名前>     # ゴミ箱へ(3 日間は復元可能)

# M2–M5 リソース(すべて delete → ゴミ箱 → `tbm trash` で 3 日以内は復元可)
tbm volume create|list|ls|put|get|rm <名前> …    # ボリューム(ファイル置き場)
tbm cache create|list|url|rotate <名前>          # cache(valkey。REDIS_URL を注入)
tbm service create <名前> [--github] [--port N] [--stateful] [--memory MB]
tbm deploy --local --service <名前>              # ローカルビルドでデプロイ(GitHub 不要の退路)
tbm deploy --image <ref> | --dockerfile <path>   # サーバ側で pull / build(GitHub も docker も不要)
tbm deploy --watch                               # push → Actions 追跡 → デプロイ完走 → 検証を一括
tbm service status|logs|exec|metrics|deploys|verify|rollback <名前>
tbm service visibility <名前> <private|company|public>  # 公開範囲(即時反映・再デプロイ不要)
tbm service rename|subdomain|limits|stateful <名前> …  # 作成後に変えられないのは port だけ
tbm inject <リソース名> --into <service名>           # 注入(database/volume/cache/service → service)
```

CLI のサーバ URL の解決順:`--server` / `TSUBOMI_SERVER` → 保存済み設定
(インストーラが server_url を書いておく)→ `http://localhost:5173`(dev)。

リリース公開は `just release-cli-publish`(4 ターゲットをビルドして配信ホストの
`~/tsubomi/releases/` へ。公開先は `TSUBOMI_RELEASE_PI=<user>@<host>` か引数で指定 —
既定は現行の香橙派。サーバ側の `TSUBOMI_RELEASE_DIR` / `TSUBOMI_RELEASES_HOST` が
このディレクトリを指していること。プラットフォームのアーキは公開先の `uname -m` から
CLI に焼き込まれる)。内容を変えたら必ず version を上げる — 同名再発行は CDN キャッシュと衝突する。

## API(M0–M1 の断面)

web と CLI は同一ハンドラの 2 入口。分岐は認証 extractor(session cookie / Bearer)だけ。
M2 以降の面(volume / service / cache / 注入 / visibility / admin など)は
`doc/paas-tech-design.md` と各フェーズの design doc、現況は `CLAUDE.md` を参照。

| Method | Path | 認証 |
| --- | --- | --- |
| GET | `/api/health` | — |
| GET | `/api/auth/google/start` → `/callback` | — |
| GET/POST | `/api/auth/me`、`/api/auth/logout` | session/token |
| POST | `/api/oauth/authorize` | session のみ |
| POST | `/api/oauth/token` | PKCE |
| GET/POST/DELETE | `/api/tokens[/{id}]` | session/token |
| GET | `/api/cli/version[/{target}]` | — |
| GET | `/api/resources` | session/token |
| GET/POST | `/api/databases` | session/token |
| GET/DELETE | `/api/databases/{id}` | session/token |
| GET | `/api/databases/{id}/url` | session/token |
| POST | `/api/databases/{id}/rotate` | session/token |
| POST | `/api/databases/{id}/query` | session/token(その DB 自身の資格情報で実行) |
| GET | `/api/trash`;POST `/api/trash/{id}/restore`;DELETE `/api/trash/{id}` | session/token |

## 依存の追加

Rust の依存は `cargo add` 経由のみ(`[dependencies]` を手書きしない):

```bash
cargo add -p tsubomi-server <crate>
```

shadcn コンポーネント:`cd web && bunx shadcn@latest add button`
