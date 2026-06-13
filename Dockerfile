# ---- web:bun で SPA バンドルをビルド ----
FROM oven/bun:1 AS web-builder
WORKDIR /web
# vite-plus(`vp`)は起動時に HTTPS クライアントを作る。システムの CA 証明書が
# 無いと panic する("No CA certificates were loaded")。slim の bun イメージには
# 入っていないので追加する。
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY web/package.json web/bun.lock ./
RUN bun install --frozen-lockfile
COPY web/ ./
RUN bun run build

# ---- rust:サーババイナリをビルド ----
# jemalloc-sys は jemalloc を C からコンパイルするので、ビルダーには C
# ツールチェーン(build-essential = gcc + make)が要る。`--bin tsubomi-server`
# はサーバの依存グラフだけをコンパイルし、CLI 側の依存をスキップする。
FROM rust:1.95-slim-trixie AS rust-builder
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY migrations ./migrations
RUN cargo build --release --bin tsubomi-server

# ---- ランタイム ----
# ベースを postgres:18 にする理由:M1 のバックアップ / ゴミ箱が pg_dump / psql を
# 使う。これらは **サーバ(pg-tenant / pg-platform = 18)と同じメジャー版**でないと
# 動かない(古い pg_dump は新しいサーバを dump 不可)。postgres:18 イメージは
# pg_dump/psql 18 + libpq + ca-certificates を最初から備え、arm64/amd64 両対応。
# 自前サーバを動かすので postgres の entrypoint は無効化する。
FROM postgres:18
WORKDIR /app
COPY --from=rust-builder /build/target/release/tsubomi-server /usr/local/bin/tsubomi-server
COPY --from=web-builder /web/dist /app/web/dist
EXPOSE 9090
# サーバは web/dist から SPA を配信し(TSUBOMI_WEB_DIR デフォルト、/app 相対)、
# /api を 0.0.0.0:9090 で受ける(8080 は amber が使う)。
ENTRYPOINT []
CMD ["tsubomi-server"]
