#!/usr/bin/env bash
# レジストリを介さず、ビルド機 → 対象ホストへ Docker イメージを直接転送してデプロイ。
# LAN 内の香橙派などに、Hub への push/pull を待たず速く配れる(`docker save | ssh
# docker load`)。対象ホストのアーキを検出し、同アーキなら native ビルド(高速)。
#
# 前提:対象ホストに compose.prod.yml と .env.production を置いておく(既定 ~/tsubomi-deploy)。
# 使い方:
#   HOST=zwg@192.168.0.106 ./scripts/ship.sh
#   HOST=user@ip TAG=v2 DIR=tsubomi-deploy ./scripts/ship.sh
set -euo pipefail

HOST="${HOST:?HOST を指定してください(例 HOST=zwg@192.168.0.106)}"
TAG="${TAG:-local}"
DIR="${DIR:-tsubomi-deploy}" # 対象ホストの home 相対。compose.prod.yml / .env.production の場所
IMAGE="tsubomi:${TAG}"

cd "$(dirname "$0")/.."

# 対象ホストのアーキを検出 → ビルドする platform を決める
remote_arch=$(ssh "$HOST" 'uname -m')
local_arch=$(uname -m)
case "$remote_arch" in
  aarch64 | arm64) platform=linux/arm64 ;;
  x86_64 | amd64) platform=linux/amd64 ;;
  *)
    echo "未知の対象アーキ: $remote_arch"
    exit 1
    ;;
esac
echo "▶ build (${platform};  対象=${remote_arch} / ビルド機=${local_arch}) ..."
# 同アーキは native で高速。別アーキは buildx+QEMU で遅い(その場合は registry 経由が無難)。
docker buildx build --platform "$platform" -t "$IMAGE" --load .

echo "▶ ${HOST} へ直接転送(docker save | ssh docker load)..."
docker save "$IMAGE" | ssh "$HOST" 'docker load'

# compose 定義だけ配ればよい(pg-tenant 初期化 / pgbouncer 設定 / userlist は
# compose.prod.yml に inline 埋め込み済み = 別ファイル不要)。.env.production は秘密
# なので同期しない(対象ホスト側で管理)。
echo "▶ ${HOST} へ compose.prod.yml を配布..."
scp -q compose.prod.yml "$HOST:${DIR}/compose.prod.yml"

echo "▶ ${HOST} で起動(${DIR}/compose.prod.yml)..."
ssh "$HOST" "cd ${DIR} && TSUBOMI_IMAGE=${IMAGE} docker compose --env-file .env.production -f compose.prod.yml up -d"

# 後始末:同じ tag を再ビルド/再 load する度、前の版が <none>(dangling)で残って
# 溜まる。両側で dangling のみ掃除(-f = タグ付きには触れない ⇒ ロールバック用の
# 旧版は安全)。失敗してもデプロイ自体は成功扱い(|| true)。
echo "▶ dangling イメージを掃除(ビルド機 + ${HOST})..."
docker image prune -f >/dev/null 2>&1 || true
ssh "$HOST" 'docker image prune -f' >/dev/null 2>&1 || true

echo "✅ ${HOST} に直接デプロイ完了 (image=${IMAGE})"
