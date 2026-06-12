# ---- web: build the SPA bundle with bun ----
FROM oven/bun:1 AS web-builder
WORKDIR /web
# vite-plus (`vp`) builds an HTTPS client at startup; without system CA certs
# it panics ("No CA certificates were loaded"). The slim bun image ships none.
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY web/package.json web/bun.lock ./
RUN bun install --frozen-lockfile
COPY web/ ./
RUN bun run build

# ---- rust: build the server binary ----
# tsubomi-server is pure Rust (axum/tokio/serde, rustls — no C deps), so the
# slim image needs no build-essential/pkg-config. `--bin tsubomi-server` only
# compiles the server's dependency graph, skipping the CLI's reqwest/aws-lc.
FROM rust:1.95-slim-trixie AS rust-builder
WORKDIR /build
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
RUN cargo build --release --bin tsubomi-server

# ---- runtime ----
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=rust-builder /build/target/release/tsubomi-server /usr/local/bin/tsubomi-server
COPY --from=web-builder /web/dist /app/web/dist
EXPOSE 8080
# Server serves the SPA from web/dist (TSUBOMI_WEB_DIR default, relative to /app)
# and the API under /api on 0.0.0.0:8080.
CMD ["tsubomi-server"]
