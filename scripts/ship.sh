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

# compose 定義 + infra の config(pg-tenant 初期化 / pgbouncer)を同梱転送する。
# これらはデプロイ成果物の一部(イメージと一緒に動く)。.env.production は秘密なので
# 同期しない(対象ホスト側で管理 — 初回 M1 化のときだけ手動で配る)。
echo "▶ ${HOST} へ compose / infra config を配布..."
ssh "$HOST" "mkdir -p ${DIR}/pg-tenant-init ${DIR}/pgbouncer"
scp -q compose.prod.yml "$HOST:${DIR}/compose.prod.yml"
scp -q infra/pg-tenant-init/* "$HOST:${DIR}/pg-tenant-init/"
scp -q infra/pgbouncer/pgbouncer.ini "$HOST:${DIR}/pgbouncer/pgbouncer.ini"
# pgbouncer の userlist は秘密(auth_user パスワード)。git の dev 既定値ではなく
# .env.production の PGBOUNCER_AUTH_PASSWORD から生成して配る(pg-tenant 側の初期化と一致)。
pgb_pw=$(grep -E '^PGBOUNCER_AUTH_PASSWORD=' .env.production 2>/dev/null | head -1 | cut -d= -f2-)
pgb_pw="${pgb_pw:-tsubomi_pgb_dev}"
printf '"pgbouncer_auth" "%s"\n' "$pgb_pw" | ssh "$HOST" "cat > ${DIR}/pgbouncer/userlist.txt"

echo "▶ ${HOST} で起動(${DIR}/compose.prod.yml)..."
ssh "$HOST" "cd ${DIR} && TSUBOMI_IMAGE=${IMAGE} docker compose --env-file .env.production -f compose.prod.yml up -d"
echo "✅ ${HOST} に直接デプロイ完了 (image=${IMAGE})"
