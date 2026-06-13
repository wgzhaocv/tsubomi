#!/usr/bin/env bash
# つぼみのサーバイメージ(rust サーバ + ビルド済み SPA を同梱)を multi-arch で
# ビルドして Docker レジストリへ push する。ビルドは開発機 / CI で行い、VPS 側は
# 出来上がったイメージを pull して起動するだけ(infra/compose.prod.yml)。
#
# 事前準備: docker login <レジストリ>
#
# 使い方:
#   REGISTRY=ghcr.io/USER ./scripts/release-image.sh
#   REGISTRY=docker.io/USER IMAGE=tsubomi-server TAG=v1 ./scripts/release-image.sh
#   PLATFORMS=linux/arm64 REGISTRY=... ./scripts/release-image.sh   # 香橙派だけなら高速
#
# 既定は amd64 + arm64 の両対応(CLAUDE.md「初日から両アーキ」)。他アーキは QEMU
# エミュレーションでビルドするため時間がかかる。単一アーキは PLATFORMS で絞れる。
set -euo pipefail

REGISTRY="${REGISTRY:?REGISTRY を指定してください(例: ghcr.io/USER, docker.io/USER, your.registry:5000)}"
IMAGE="${IMAGE:-tsubomi-server}"
TAG="${TAG:-latest}"
PLATFORMS="${PLATFORMS:-linux/amd64,linux/arm64}"
REF="${REGISTRY%/}/${IMAGE}:${TAG}"

cd "$(dirname "$0")/.."

# 他アーキを QEMU でエミュレートするための binfmt 登録(冪等)。
if [ "$PLATFORMS" != "linux/$(uname -m | sed 's/x86_64/amd64/;s/aarch64/arm64/')" ]; then
  docker run --privileged --rm tonistiigi/binfmt --install all >/dev/null 2>&1 || true
fi

# multi-arch 対応の buildx ビルダーを用意(冪等)。
if ! docker buildx inspect tsubomi-builder >/dev/null 2>&1; then
  docker buildx create --name tsubomi-builder --driver docker-container >/dev/null
fi
docker buildx use tsubomi-builder

echo "▶ building ${REF}  [${PLATFORMS}] ..."
docker buildx build --platform "${PLATFORMS}" -t "${REF}" --push .

echo "✅ pushed ${REF}"
echo ""
echo "VPS 側(Docker さえあれば OS 不問・justfile 不要):"
echo "  1) docker login ${REGISTRY%%/*}"
echo "  2) その機の .env.production を用意し、TSUBOMI_IMAGE=${REF} を追記"
echo "  3) docker compose --env-file .env.production -f compose.prod.yml up -d"
