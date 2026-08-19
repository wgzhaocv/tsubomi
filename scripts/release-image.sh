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

# ビルダーの選択。**`docker buildx use` は使わない** — あれは全体の既定ビルダーを永続的に
# 書き換えるので、その後の `scripts/ship.sh`(ビルダーを指定しない)まで docker-container
# ドライバに引きずり込む。container ドライバはデーモンとは別のイメージキャッシュを持つため
# 基底イメージを自分で Hub から取り直し、Hub が細い回線では無音で数十分止まる
# (2026-08-19:ship がここで固まっていた実害。`--builder` で 1 回のビルドにだけ効かせる)。
#
# 優先順:① BUILDX_BUILDER が指定されていればそれ(buildx が自前で解釈するので何も渡さない)
#   ② デーモンが containerd イメージストアなら docker ドライバのまま multi-arch を出せる
#      = デーモンのキャッシュをそのまま使えて速い ③ どちらでもなければ container ドライバの
#      専用ビルダーを用意(従来の道)。
builder_args=()
if [ -n "${BUILDX_BUILDER:-}" ]; then
  echo "▶ ビルダー: \$BUILDX_BUILDER=${BUILDX_BUILDER}"
elif docker info -f '{{.DriverStatus}}' 2>/dev/null | grep -q 'containerd.snapshotter'; then
  echo "▶ ビルダー: 既定(docker ドライバ + containerd イメージストア = multi-arch 可)"
else
  if ! docker buildx inspect tsubomi-builder >/dev/null 2>&1; then
    docker buildx create --name tsubomi-builder --driver docker-container >/dev/null
  fi
  builder_args=(--builder tsubomi-builder)
  echo "▶ ビルダー: tsubomi-builder(docker-container ドライバ)"
fi

# 他アーキのエミュレーション(binfmt)。**既にビルダーが対応を宣言しているなら何もしない** —
# OrbStack / Docker Desktop は最初から複数アーキを出せる。以前は無条件に
# `docker run --privileged --rm tonistiigi/binfmt --install all >/dev/null 2>&1` を走らせて
# いたが、これは (a) 不要な場合まで Hub から image を取りに行き、(b) 出力を捨てているので
# 回線が細いと**無音で数十分止まる**(2026-08-19 の実害:2 度これで固まった)。必要なときだけ、
# 出力を見せて走らせる。
missing=""
# inspect も同じビルダーに向ける(空配列を "" として渡さないよう、名前があるときだけ足す)。
inspect_args=()
[ "${#builder_args[@]}" -gt 0 ] && inspect_args=("${builder_args[1]}")
have_platforms="$(docker buildx inspect --bootstrap "${inspect_args[@]}" 2>/dev/null | sed -n 's/^Platforms:[[:space:]]*//p' | tr -d ' ')"
for p in ${PLATFORMS//,/ }; do
  case ",${have_platforms}," in *",${p},"*) ;; *) missing="${missing}${p} " ;; esac
done
if [ -n "$missing" ]; then
  echo "▶ binfmt 登録(未対応: ${missing}) — Hub から tonistiigi/binfmt を取得します..."
  docker run --privileged --rm tonistiigi/binfmt --install all || {
    echo "⚠ binfmt 登録に失敗。PLATFORMS を native だけに絞るか、回線を確認してください" >&2
    exit 1
  }
fi

echo "▶ building ${REF}  [${PLATFORMS}] ..."
docker buildx build "${builder_args[@]}" --platform "${PLATFORMS}" -t "${REF}" --push .

echo "✅ pushed ${REF}"
echo ""
echo "VPS 側(Docker さえあれば OS 不問・justfile 不要):"
echo "  1) docker login ${REGISTRY%%/*}"
echo "  2) その機の .env.production を用意し、TSUBOMI_IMAGE=${REF} を追記"
echo "  3) docker compose --env-file .env.production -f compose.prod.yml up -d"
echo "     (overlay(compose.prod.*.yml)を置いた機では在るもの全部を -f に連ねる)"
