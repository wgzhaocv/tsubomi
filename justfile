default:
    @just --list

# バックエンド + フロントエンドを同時起動。Ctrl-C で両方止まる。
# サーバは cargo watch で自動再ビルド+再起動(Rust を触ったら勝手に反映される)。
# web は vite が HMR するので watch 不要。
dev:
    #!/usr/bin/env bash
    set -euo pipefail
    trap 'kill 0' EXIT
    cargo watch -w crates -x 'run -p tsubomi-server' &
    (cd web && bun run dev) &
    wait

# tbm CLI を実行。例:`just cli health` / `just cli db list`
cli *args:
    cargo run -p tsubomi-cli -- {{args}}

# React Email テンプレ(web/emails/)を静的 HTML に焼き、crates/server/src/mail/templates/
# へ書き出す。テンプレ(.tsx)を変えたら実行して生成 HTML をコミットする(include_str! で埋め込む)。
emails:
    cd web && bun run scripts/render-emails.tsx

# infra 起動: pg-platform:5434 / pg-tenant:5435 / pgbouncer:6432 / registry:5000 /
#   traefik:8088(MIG は起動時に自動)。tsubomi-edge 網を先に作る(compose は external 参照)。
db-up:
    #!/usr/bin/env bash
    set -euo pipefail
    docker network create tsubomi-edge 2>/dev/null || true
    # traefik の動的設定ディレクトリを先に作る(無いと docker が root 所有で作り、平台が書けない)。
    # パスは .env の TSUBOMI_TRAEFIK_DYNAMIC_DIR(dev は /tmp 配下、Mac 可書)。compose も同じ
    # .env を --env-file で読み、server の書き込み先と mount を揃える。
    dir=$(grep -E '^TSUBOMI_TRAEFIK_DYNAMIC_DIR=' .env 2>/dev/null | cut -d= -f2- | tr -d '"' || true)
    mkdir -p "${dir:-/srv/tsubomi/traefik-dynamic}"
    docker compose --env-file .env -f infra/docker-compose.yml up -d

# infra を停止(データは volume に残る)
db-down:
    docker compose -f infra/docker-compose.yml down

# 管制面 DB(pg-platform)に psql で入る
db-psql:
    docker exec -it tsubomi-pg-platform psql -U tsubomi -d tsubomi_platform

# テナント DB インスタンス(pg-tenant、ユーザ DB 群)に admin で入る
db-psql-tenant:
    docker exec -it tsubomi-pg-tenant psql -U tsubomi_admin -d postgres

# web の依存をインストール(初回のみ)
web-install:
    cd web && bun install

# Rust のテストを全部実行
test:
    cargo test --workspace

# Rust + web のフォーマット
fmt:
    cargo fmt --all
    cd web && bun run fmt

# 型チェック + clippy + web lint(コミット前の門番)
check:
    cargo check --workspace
    cargo clippy --workspace -- -D warnings
    cd web && bun run lint

# LAN 内ホスト(香橙派など)へ直接転送してデプロイ。レジストリ不要・速い:
# arch を検出 → native ビルド → docker save | ssh docker load → 起動。
# 例: HOST=zwg@192.168.0.106 just ship   /   HOST=... TAG=v2 just ship
ship:
    chmod +x scripts/ship.sh && scripts/ship.sh

# サーバイメージを multi-arch(amd64+arm64)でビルドしてレジストリへ push。
# リモート VPS 用(各 VPS は compose.prod.yml で docker pull 起動)。
# 例: REGISTRY=docker.io/USER IMAGE=tsubomi TAG=v2 just release-image
release-image:
    chmod +x scripts/release-image.sh && scripts/release-image.sh

# tbm CLI を 4 ターゲットでビルドして香橙派へリリース公開。
# 内容を変えたら crates/cli/Cargo.toml の version を上げてから実行(不可変リリース)。
release-cli-publish:
    chmod +x scripts/release-cli.sh && scripts/release-cli.sh

# 公網DB 辺縁 SNI 闸门(crates/sni-gate)を x86_64-linux に交叉編譯(配備せず確認だけ)。
build-sni-gate:
    PATH="$HOME/.cargo/bin:$PATH" cargo zigbuild --release -p tsubomi-sni-gate --target x86_64-unknown-linux-gnu

# SNI 闸门を VPS(既定 proxy)へ build & deploy(二進制 + systemd unit、入替 & 再起動)。
# 例: just ship-sni-gate  /  SNI_GATE_HOST=proxy just ship-sni-gate
ship-sni-gate host="proxy":
    chmod +x scripts/ship-sni-gate.sh && scripts/ship-sni-gate.sh {{host}}
