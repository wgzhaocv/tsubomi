# ---- web:bun で SPA バンドルをビルド ----
# web/dist は静的成果物(アーキ非依存)なので **ビルドホストの native アーキで 1 回だけ** ビルドする
# (`--platform=$BUILDPLATFORM`)。これが無いと multi-arch ビルドで amd64 側の `bun run build` が
# QEMU エミュレーション下でハングする(実測 2026-06-26:amd64 の bun が起動行も出さず停止)。
# 両ターゲットの stage-2 は同じ /web/dist を COPY するので、native 1 回で十分。
FROM --platform=$BUILDPLATFORM oven/bun:1 AS web-builder
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
# **ビルドホストの native アーキで動かし、rust の交差編譯で目標アーキを出す**
# (`--platform=$BUILDPLATFORM` + `--target`)。web stage と同じ理由 + もう一つ:
# ここを目標アーキで動かすと amd64 側の `cargo build` が **QEMU エミュレーション下で
# Rust をフルコンパイル**することになり、multi-arch ビルドが桁違いに遅くなる
# (2026-08-19:これが Hub への multi-arch push を諦めていた実際の理由)。加えて
# 目標アーキの `rust:1.95`(1.14GB)を Hub から取る必要も消える = 回線が細い開発機で
# 詰まる箇所が 1 つ減る。
#
# jemalloc-sys は jemalloc を C からコンパイルするので、目標アーキ用の C クロス
# ツールチェーンが要る(Debian の gcc-<arch>-linux-gnu。zig は使わない — macOS 宿主の
# cargo-zigbuild は sqlx の proc-macro dylib を壊す実測あり)。両アーキ分を入れておくと
# stage がターゲット間で共有され、キャッシュも効く。
# `--bin tsubomi-server` はサーバの依存グラフだけをコンパイルし、CLI 側をスキップする。
FROM --platform=$BUILDPLATFORM rust:1.95-slim-trixie AS rust-builder
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential gcc-x86-64-linux-gnu gcc-aarch64-linux-gnu \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY migrations ./migrations
# TARGETARCH(amd64/arm64)→ rust の triple。リンカと `cc` crate 用の CC を目標アーキの
# クロス gcc に向ける(前者が rustc のリンク、後者が jemalloc の C コンパイル)。成果物は
# 最終 stage が triple を知らなくて済むよう `/out` の固定パスへ置く。
# ※ `\` 継続の中に `#` コメント行を挟まない — パーサ次第で後続がシェルのコメントに
#   飲まれ得るので、説明はこの位置に書く。
ARG TARGETARCH
RUN set -eux; \
    case "$TARGETARCH" in \
      amd64) triple=x86_64-unknown-linux-gnu; cc=x86_64-linux-gnu-gcc ;; \
      arm64) triple=aarch64-unknown-linux-gnu; cc=aarch64-linux-gnu-gcc ;; \
      *) echo "未対応の TARGETARCH: $TARGETARCH" >&2; exit 1 ;; \
    esac; \
    rustup target add "$triple"; \
    linker_var="CARGO_TARGET_$(echo "$triple" | tr 'a-z-' 'A-Z_')_LINKER"; \
    export "$linker_var=$cc"; \
    export "CC_$(echo "$triple" | tr '-' '_')=$cc"; \
    cargo build --release --target "$triple" --bin tsubomi-server; \
    install -D "target/$triple/release/tsubomi-server" /out/tsubomi-server

# ---- ランタイム ----
# debian-slim に PGDG の postgresql-client-18 だけを足す。M1 のバックアップ /
# ゴミ箱が使う pg_dump / psql は **サーバ(pg-tenant / pg-platform = 18)と同じ
# メジャー版**でないと動かない(古い pg_dump は新しいサーバを dump 不可)。
# postgres:18 を丸ごと背負う(≈469MB)より小さく(≈180MB)、能力は同一
# (pg_dump/psql 18 + libpq + ca-certificates)。arm64/amd64 両対応。
# rsync は volumes の日次バックアップ(gc.rs の rsync スナップショット)に要る — 不在だと
# spawn が `No such file or directory` で失敗し volumes だけバックアップされない。
FROM debian:trixie-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && install -d /usr/share/postgresql-common/pgdg \
    && curl -fsSL https://www.postgresql.org/media/keys/ACCC4CF8.asc \
         -o /usr/share/postgresql-common/pgdg/apt.postgresql.org.asc \
    && echo "deb [signed-by=/usr/share/postgresql-common/pgdg/apt.postgresql.org.asc] https://apt.postgresql.org/pub/repos/apt trixie-pgdg main" \
         > /etc/apt/sources.list.d/pgdg.list \
    && apt-get update \
    && apt-get install -y --no-install-recommends postgresql-client-18 iptables rsync \
    && apt-get purge -y --auto-remove curl \
    && rm -rf /var/lib/apt/lists/*
# M6 egress:server は host netns で `iptables` を打ってテナントアウトバウンドを遮断する(services/egress.rs)。
# debian trixie の iptables は既定で **nft バックエンド** = host(v1.8.7 nf_tables)と一致するので
# 同じテーブルを操作できる(legacy だと別テーブルで無効化する)。compose 側で cap_add: NET_ADMIN が要る。
WORKDIR /app
COPY --from=rust-builder /out/tsubomi-server /usr/local/bin/tsubomi-server
COPY --from=web-builder /web/dist /app/web/dist
EXPOSE 9090
# サーバは web/dist から SPA を配信し(TSUBOMI_WEB_DIR デフォルト、/app 相対)、
# /api を 0.0.0.0:9090 で受ける(8080 は amber が使う)。
CMD ["tsubomi-server"]
