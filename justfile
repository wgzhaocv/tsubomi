default:
    @just --list

# run backend + frontend together; Ctrl-C stops both
dev:
    #!/usr/bin/env bash
    set -euo pipefail
    trap 'kill 0' EXIT
    cargo run -p tsubomi-server &
    (cd web && bun run dev) &
    wait

# run the axum server on http://localhost:8080
dev-server:
    cargo run -p tsubomi-server

# run the web dev server (http://localhost:5173, proxies /api -> :8080)
dev-web:
    cd web && bun run dev

# run the CLI, e.g. `just cli hello` or `just cli health`
cli *args:
    cargo run -p tsubomi-cli -- {{args}}

# install web dependencies
web-install:
    cd web && bun install

# release build: server + cli binaries, plus the production web bundle
build:
    cargo build --release
    cd web && bun run build

# run all rust tests
test:
    cargo test --workspace

# format rust + web
fmt:
    cargo fmt --all
    cd web && bun run fmt

# typecheck rust + clippy + web lint
check:
    cargo check --workspace
    cargo clippy --workspace -- -D warnings
    cd web && bun run lint

# build the all-in-one image: rust server + built SPA, served on one port
docker-build:
    docker build -t tsubomi-server:latest .

# build + run the whole app in docker, detached, on http://localhost:8080
up:
    docker compose up --build -d

# stop + remove the docker app
down:
    docker compose down

# follow server logs
logs:
    docker compose logs -f server

# build the release CLI binary -> target/release/tsubomi
release-cli:
    cargo build --release -p tsubomi-cli
