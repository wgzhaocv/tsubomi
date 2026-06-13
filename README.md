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
just db-up               # 管制面 postgres(127.0.0.1:5434)
cp .env.example .env     # GOOGLE_CLIENT_ID / GOOGLE_CLIENT_SECRET を埋める
just dev                 # server :9090 + web :5173 を同時起動。Ctrl-C で両方停止
```

Google OAuth クライアント:Google Cloud Console で作成(種別:Web application、
同意画面は **Internal**)。Authorized redirect URI は
`http://localhost:5173/api/auth/google/callback`。ログインは
`TSUBOMI_ALLOWED_HD` の Workspace ドメインに制限される(サーバ側 `hd` 検証)。

## デプロイ

単機運用・ホスト直走り。サーバは **host ネットワーク**で動き `:9090` をホストに出す
(前段に reverse proxy を置く想定)。設定はホスト毎の **`.env.production`**(git 管理外。
`TSUBOMI_IMAGE` と `PG_PLATFORM_PASSWORD` もここに書く)。host ネットなので
`DATABASE_URL=127.0.0.1:5434` が dev / 本番で共通のまま通る。`just` が無いマシン
(Windows 等)でも下段の生コマンドだけで完結する。

イメージは初日から **両アーキ対応**(arm64 = 香橙派 / amd64 = x86_64 VPS)。

### A. レジストリ経由(リモート VPS 推奨・ビルド不要・OS 非依存)

ビルド機で multi-arch イメージを作ってレジストリへ push:

```bash
docker login docker.io
REGISTRY=docker.io/<USER> IMAGE=tsubomi TAG=v1 just release-image
# just 無し: REGISTRY=docker.io/<USER> IMAGE=tsubomi TAG=v1 bash scripts/release-image.sh
# 単一アーキで高速化: PLATFORMS=linux/arm64 REGISTRY=... bash scripts/release-image.sh
```

VPS 側は `compose.prod.yml` と `.env.production` を置いて起動するだけ:

```bash
docker login docker.io   # 公開イメージなら不要
docker compose --env-file .env.production -f compose.prod.yml up -d
```

`compose.prod.yml` が管制面 pg(loopback バインドで隔離)+ server を一括起動する。
停止は `docker compose -f compose.prod.yml down`。

### B. ローカルビルド(ソースのあるホスト。例:香橙派 arm64)

```bash
just deploy   # 管制面 pg 起動 → サーバを build + 起動(.env.production を使用)
# just 無し:
#   docker compose --env-file .env.production -f infra/docker-compose.yml up -d
#   docker compose --env-file .env.production up --build -d
```

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
```

CLI のサーバ URL の解決順:`--server` / `TSUBOMI_SERVER` → 保存済み設定
(インストーラが server_url を書いておく)→ `http://localhost:5173`(dev)。

リリース公開は `just release-cli-publish`(4 ターゲットをビルドして Pi へ。
内容を変えたら必ず version を上げる — 同名再発行は CDN キャッシュと衝突する)。

## API(M0)

| Method | Path | 認証 |
| --- | --- | --- |
| GET | `/api/health` | — |
| GET | `/api/auth/google/start` → `/callback` | — |
| GET/POST | `/api/auth/me`、`/api/auth/logout` | session/token |
| POST | `/api/oauth/authorize` | session のみ |
| POST | `/api/oauth/token` | PKCE |
| GET/POST/DELETE | `/api/tokens[/{id}]` | session/token |
| GET | `/api/cli/version[/{target}]` | — |

## 依存の追加

Rust の依存は `cargo add` 経由のみ(`[dependencies]` を手書きしない):

```bash
cargo add -p tsubomi-server <crate>
```

shadcn コンポーネント:`cd web && bunx shadcn@latest add button`
