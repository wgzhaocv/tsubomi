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
# debian-slim に PGDG の postgresql-client-18 だけを足す。M1 のバックアップ /
# ゴミ箱が使う pg_dump / psql は **サーバ(pg-tenant / pg-platform = 18)と同じ
# メジャー版**でないと動かない(古い pg_dump は新しいサーバを dump 不可)。
# postgres:18 を丸ごと背負う(≈469MB)より小さく(≈180MB)、能力は同一
# (pg_dump/psql 18 + libpq + ca-certificates)。arm64/amd64 両対応。
FROM debian:trixie-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && install -d /usr/share/postgresql-common/pgdg \
    && curl -fsSL https://www.postgresql.org/media/keys/ACCC4CF8.asc \
         -o /usr/share/postgresql-common/pgdg/apt.postgresql.org.asc \
    && echo "deb [signed-by=/usr/share/postgresql-common/pgdg/apt.postgresql.org.asc] https://apt.postgresql.org/pub/repos/apt trixie-pgdg main" \
         > /etc/apt/sources.list.d/pgdg.list \
    && apt-get update \
    && apt-get install -y --no-install-recommends postgresql-client-18 iptables \
    && apt-get purge -y --auto-remove curl \
    && rm -rf /var/lib/apt/lists/*
# M6 egress:server は host netns で `iptables` を打ってテナント出站を遮断する(services/egress.rs)。
# debian trixie の iptables は既定で **nft バックエンド** = host(v1.8.7 nf_tables)と一致するので
# 同じテーブルを操作できる(legacy だと別テーブルで無効化する)。compose 側で cap_add: NET_ADMIN が要る。
WORKDIR /app
COPY --from=rust-builder /build/target/release/tsubomi-server /usr/local/bin/tsubomi-server
COPY --from=web-builder /web/dist /app/web/dist
EXPOSE 9090
# サーバは web/dist から SPA を配信し(TSUBOMI_WEB_DIR デフォルト、/app 相対)、
# /api を 0.0.0.0:9090 で受ける(8080 は amber が使う)。
CMD ["tsubomi-server"]
