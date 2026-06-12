default:
    @just --list

# バックエンド + フロントエンドを同時起動。Ctrl-C で両方止まる
dev:
    #!/usr/bin/env bash
    set -euo pipefail
    trap 'kill 0' EXIT
    cargo run -p tsubomi-server &
    (cd web && bun run dev) &
    wait

# axum サーバを http://localhost:9090 で起動
dev-server:
    cargo run -p tsubomi-server

# web dev サーバを起動(http://localhost:5173、/api → :9090 にプロキシ)
dev-web:
    cd web && bun run dev

# tbm CLI を実行。例:`just cli health` / `just cli login`
cli *args:
    cargo run -p tsubomi-cli -- {{args}}

# 管制面 postgres を起動(インフラ層。香橙派でも同じファイル)
db-up:
    docker compose -f infra/docker-compose.yml up -d

# 管制面 postgres を停止(データは volume に残る)
db-down:
    docker compose -f infra/docker-compose.yml down

# 管制面 DB に psql で入る
db-psql:
    docker exec -it tsubomi-pg-platform psql -U tsubomi -d tsubomi_platform

# web の依存をインストール
web-install:
    cd web && bun install

# リリースビルド:server + cli バイナリ + 本番 web バンドル
build:
    cargo build --release
    cd web && bun run build

# Rust のテストを全部実行
test:
    cargo test --workspace

# Rust + web のフォーマット
fmt:
    cargo fmt --all
    cd web && bun run fmt

# 型チェック + clippy + web lint
check:
    cargo check --workspace
    cargo clippy --workspace -- -D warnings
    cd web && bun run lint

# オールインワンイメージをビルド:rust サーバ + ビルド済み SPA を 1 ポートで配信
docker-build:
    docker build -t tsubomi-server:latest .

# アプリ全体を docker でビルド + 起動(detached、http://localhost:9090)
up:
    docker compose up --build -d

# docker アプリを停止 + 削除
down:
    docker compose down

# サーバログを追う
logs:
    docker compose logs -f server

# リリース CLI バイナリをビルド -> target/release/tbm
release-cli:
    cargo build --release -p tsubomi-cli

# CLI を 4 ターゲットでビルドして香橙派へリリース公開
release-cli-publish:
    chmod +x scripts/release-cli.sh && scripts/release-cli.sh
