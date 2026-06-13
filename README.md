# tsubomi 蕾

社内 PaaS プラットフォーム(セルフホストの「基礎版 Vercel + Neon」)。設計ドキュメント:
[paas-design-v2.md](paas-design-v2.md)(意図)/ [paas-tech-design.md](paas-tech-design.md)(技術設計)。
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

単機運用・ホスト直走り。サーバは **host ネットワーク**で `127.0.0.1:9090`(本番は
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

新しい VPS に必要なのは **Docker だけ**(ソース・just・sh は不要)。

1. **Docker を入れる**:`curl -fsSL https://get.docker.com | sh`
2. **2 ファイルだけ**を任意のディレクトリ(例 `~/tsubomi`)に置く:
   - `compose.prod.yml` — リポジトリからコピー(pg-tenant 初期化 / pgbouncer 設定 /
     userlist は全部この中に inline 埋め込み済み = 別ファイル不要)
   - `.env.production` — 同じ場所に新規作成(`.env.example` がひな形)
3. **`.env.production` を本番値で埋める**(主なキー。全量は `.env.example` 参照):

   ```env
   PG_PLATFORM_PASSWORD=<強いパスワード>
   DATABASE_URL=postgres://tsubomi:<同じパスワード>@127.0.0.1:5434/tsubomi_platform
   GOOGLE_CLIENT_ID=...
   GOOGLE_CLIENT_SECRET=...
   GOOGLE_REDIRECT_URI=https://<ドメイン>/api/auth/google/callback
   TSUBOMI_SERVER_URL=https://<ドメイン>
   TSUBOMI_ALLOWED_HD=<会社ドメイン>      # 複数はカンマ区切り
   TSUBOMI_OWNER_EMAILS=<owner のメール>   # 複数はカンマ区切り
   TSUBOMI_BIND_ADDR=127.0.0.1:9090
   TSUBOMI_COOKIE_SECURE=true             # HTTPS 必須
   # ── M1 database ──
   PG_TENANT_PASSWORD=<強いパスワード>     # pg-tenant(ユーザ DB)admin
   TENANT_ADMIN_URL=postgres://tsubomi_admin:<同じパスワード>@127.0.0.1:5435/postgres
   TSUBOMI_MASTER_KEY=<base64 32 bytes>   # head -c 32 /dev/urandom | base64
   TSUBOMI_DB_PUBLIC_HOST=<DB の到達先ホスト/IP>  # 接続文字列に出す。会社網から届く宛先
   PGBOUNCER_BIND_ADDR=0.0.0.0            # 外部接続を受ける(送信元制限は↓の柵で)
   ```

   `PG_PLATFORM_PASSWORD` と `DATABASE_URL`、`PG_TENANT_PASSWORD` と `TENANT_ADMIN_URL`
   のパスワードはそれぞれ**必ず一致**させる(compose が新規 pg をこの値で初期化する)。
   バックアップ / ゴミ箱の dump は host の `/srv/tsubomi` に出る(compose がマウント)。
4. **Google OAuth** に本番の redirect URI を追加:
   `https://<ドメイン>/api/auth/google/callback`
5. **起動**(公開イメージを pull し、管制面 pg + server をまとめて立てる):

   ```bash
   docker compose --env-file .env.production -f compose.prod.yml up -d
   ```
6. **TLS リバースプロキシ**を前段に置き、`<ドメイン>` → `127.0.0.1:9090` へ転送する。
   例(Caddy):`<ドメイン> { reverse_proxy 127.0.0.1:9090 }`
   - **M1 の DB 入口(pgbouncer :6432)を必ず会社 CIDR に絞る**(iptables の
     `DOCKER-USER` チェーン。ufw だけでは Docker を素通りする — design v2 §1)。
     `PGBOUNCER_BIND_ADDR=0.0.0.0` は「受ける」だけで、送信元制限はこの柵が担う。
   - DB ワイヤ自体は **client TLS(自己署名、`sslmode=require`)で暗号化済み** ——
     pgbouncer が起動時に証明書を生成し、平文接続(`sslmode=disable`)は拒否する。
     CIDR 制限と合わせて二重(LAN 盗聴も塞ぐ)。
7. **確認 / ログ**:

   ```bash
   curl -fsS http://127.0.0.1:9090/api/health
   docker compose -f compose.prod.yml logs -f server
   ```
8. **更新**:新しい `compose.prod.yml`(既定タグが上がっている)を取得して
   `docker compose --env-file .env.production -f compose.prod.yml up -d`
   (別タグなら `TSUBOMI_IMAGE=...:vN` を前置して実行)。停止は
   `docker compose -f compose.prod.yml down`。

### メンテナ向け:配布・更新

イメージを更新・配布するのは**メンテナだけ**。配り先で 2 通り。

**A. レジストリへ publish(別マシン / 不特定の VPS 用。各 VPS は `docker pull`)**

```bash
docker login docker.io
REGISTRY=docker.io/wgzhaofumi IMAGE=tsubomi TAG=v2 just release-image  # multi-arch push
# just 無し:  REGISTRY=docker.io/wgzhaofumi IMAGE=tsubomi TAG=v2 bash scripts/release-image.sh
```

publish 後、VPS 側は新タグで起こし直すだけ:
`TSUBOMI_IMAGE=docker.io/wgzhaofumi/tsubomi:v2 docker compose --env-file .env.production -f compose.prod.yml up -d`
(または `compose.prod.yml` の既定タグを上げて取得 → `up -d`)。

**B. LAN 内ホストへ直送(香橙派など。Hub を介さず速い)**

ビルド機 → 対象ホストへ `docker save | ssh docker load` で直接渡し、その場で起こす。
対象のアーキを検出して native ビルドするので同アーキ(Mac arm64 → 香橙派 arm64)は高速:

```bash
HOST=zwg@192.168.0.106 just ship          # 既定タグ tsubomi:local で直送 + 起動
# HOST=user@ip TAG=v2 just ship
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
tbm db create <名前>     # DB 作成(平台が wire 名・role・パスワードを生成)
tbm db list
tbm db url <名前>        # 外部接続文字列(= パスワード。共有しない)
tbm db connect <名前>    # 無密码で psql 接続(PGPASSWORD、履歴に残さない)
tbm db rotate <名前>     # パスワード再生成(古い接続文字列は即失効)
tbm db delete <名前>     # ゴミ箱へ(3 日間は復元可能)
```

CLI のサーバ URL の解決順:`--server` / `TSUBOMI_SERVER` → 保存済み設定
(インストーラが server_url を書いておく)→ `http://localhost:5173`(dev)。

リリース公開は `just release-cli-publish`(4 ターゲットをビルドして Pi へ。
内容を変えたら必ず version を上げる — 同名再発行は CDN キャッシュと衝突する)。

## API(M0–M1)

web と CLI は同一ハンドラの 2 入口。分岐は認証 extractor(session cookie / Bearer)だけ。

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
