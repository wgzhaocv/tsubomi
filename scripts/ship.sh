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

# TAG / DIR はそのまま ssh 越しの遠隔シェルに展開される(下の docker compose 行)。空白や
# シェルメタ文字が混じると解析崩れ / コマンド注入になりうるので、安全な文字集合に縛る。
case "$TAG" in *[!A-Za-z0-9._-]*) echo "TAG に使えない文字が含まれています: $TAG" >&2; exit 1;; esac
case "$DIR" in *[!A-Za-z0-9._/-]*) echo "DIR に使えない文字が含まれています: $DIR" >&2; exit 1;; esac

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
#
# overlay(compose.prod.*.yml)は **対象ホストに置いてあるものがそのホストの拓撲宣言**
# (例:Pi = cache-public + registry-direct、VPS = db-public。初回に手で置く)。ship は
#   1) ホストに在る overlay のうち repo に同名があるものを配布(静默ドリフト防止)、
#   2) 在るもの全部を -f に連ねる。
# 全ファイルで up しないと compose がファイル集合の差を「孤児コンテナ」と誤認するし、
# もし valkey / traefik が再作成される事態(定義変更 + no-recreate の外)では overlay 抜きの
# 構成で再建されて TLS 口・registry 直連入口が静かに消える(2026-08-03 の孤児警告の真因)。
echo "▶ ${HOST} へ compose 定義を配布..."
# ホストの overlay を枚挙。検査は **遠隔の glob 展開点** でやる — ls の出力を検査するのでは
# 遅い(改行入り名は「期待形の複数行」に化け、空ディレクトリの誤マッチは無音で base-only に
# 退化する)。glob は基底に当たらない(compose.prod.*.yml は * の前後に一段ずつ要る)ので
# 「未展開の字面 glob = overlay 無し」は base-only ホストでも初回でも正常な 0 行。
# 「cd はできるが列挙できない」(権限)は [ -r . ] で響いて止める。外側を || true で包んでは
# いけない — ssh 断まで「overlay 無し」に静默退化し、overlay 抜きの構成で up してしまう
# (この改修が防ぎたい事故そのもの)。LC_ALL=C は遠隔 locale による glob の並び順の揺れを
# 殺す(-f の順序 = command 後勝ちの勝敗に直結。zz- 接頭辞の私有ファイル必末尾も C 前提)。
overlays=$(ssh "$HOST" "cd ${DIR} && [ -r . ] && LC_ALL=C sh -c '
  for f in compose.prod.*.yml; do
    [ \"\$f\" = \"compose.prod.*.yml\" ] && exit 0
    case \"\$f\" in *[!A-Za-z0-9._-]*) echo \"overlay 名に使えない文字: \$f\" >&2; exit 1;; esac
    [ -f \"\$f\" ] || { echo \"通常ファイルでない overlay: \$f\" >&2; exit 1; }
    printf \"%s\\n\" \"\$f\"
  done
'")
compose_files="-f compose.prod.yml"
to_ship=(compose.prod.yml)
while IFS= read -r f; do
  case "$f" in
    "") continue ;;
    # 二重防御(正検査は遠隔側)。基底名が混じるのも汚染(glob は基底に当たらない)なので
    # 静かに読み飛ばさず、期待形以外は必ず止める — 断片名(例 .env.production)を
    # 「repo に在る」と誤認して配布しないため。
    *[!A-Za-z0-9._-]*) echo "overlay 名に使えない文字: $f" >&2; exit 1 ;;
    compose.prod.*.yml) ;; # 期待形
    *) echo "overlay の期待形(compose.prod.*.yml)でない行: $f — 枚挙汚染の疑い" >&2; exit 1 ;;
  esac
  if [ -f "$f" ]; then
    to_ship+=("$f")
  else
    echo "⚠ ${HOST}:${DIR}/$f は repo に無い(配布スキップ、-f には含める)。" \
      "意図した host 私有 overlay なら無視可 / 廃止済みなら host 側で削除を" >&2
  fi
  compose_files="${compose_files} -f $f"
done <<<"$overlays"
# 同一目的地なので 1 回の scp でまとめて配る(N+1 回の握手を 1 回に)。
scp -q "${to_ship[@]}" "$HOST:${DIR}/"

# Traefik ローカルプラグイン(vendor:traefik-plugins/)+ 静的 dynamic 設定(cloudflared 実 IP
# middleware)を配布。CF Tunnel 越しに実 client IP を Cf-Connecting-Ip → X-Forwarded-For へ写し、
# 会社 IP 許可リストを実 IP で効かせる(traefik-plugins/README.md)。源は静的(per-deploy で変わらない)
# が冪等なので毎回配り、fresh host も自動セットアップする。/srv/tsubomi は root 所有なので docker 経由
# で置く(zwg は sudo 無し)。配置先既定:プラグイン=/srv/tsubomi/traefik-plugins、middleware 定義=
# /srv/tsubomi/traefik-dynamic(compose の TSUBOMI_TRAEFIK_PLUGINS_DIR / TSUBOMI_TRAEFIK_DYNAMIC_DIR)。
# 注:既存 traefik の再作成は ship では行わない(no-recreate)= プラグイン配線の反映は別途 `up -d traefik`。
echo "▶ ${HOST} へ Traefik プラグイン + dynamic 設定を配布..."
ssh "$HOST" "rm -rf ${DIR}/.ship-traefik && mkdir -p ${DIR}/.ship-traefik"
scp -rq traefik-plugins "$HOST:${DIR}/.ship-traefik/traefik-plugins"
scp -q traefik-dynamic/cloudflare-realip.yml "$HOST:${DIR}/.ship-traefik/cloudflare-realip.yml"
ssh "$HOST" "docker run --rm -v /srv/tsubomi:/dest -v \$HOME/${DIR}/.ship-traefik:/src:ro alpine sh -c '
  mkdir -p /dest/traefik-plugins /dest/traefik-dynamic &&
  cp -r /src/traefik-plugins/. /dest/traefik-plugins/ &&
  cp /src/cloudflare-realip.yml /dest/traefik-dynamic/cloudflare-realip.yml' \
  && rm -rf ${DIR}/.ship-traefik"

echo "▶ ${HOST} で起動(${DIR}: ${compose_files})..."
# **平台更新はユーザ app への影響を最小化する** — ship は「server だけ」を入れ替える:
#   1) up -d --no-recreate:足りないものだけ起こす(初回デプロイで infra 一式を立ち上げる)。
#      既存コンテナは **絶対に作り直さない** ので、traefik / pgbouncer / valkey / pg-tenant
#      といったデータ面・入口を巻き込んで再生成しない(= 全 app の同時瞬断を防ぐ)。
#   2) up -d server:server だけを新イメージへ作り直す。server は host ネットでユーザ
#      リクエスト経路に居ないので、この入れ替えで走行中の app トラフィックは切れない。
# (infra(traefik/pg/valkey 等)の意図的な更新は別操作 — それらは digest ピンで固定してある。)
compose="TSUBOMI_IMAGE=${IMAGE} docker compose --env-file .env.production ${compose_files}"
ssh "$HOST" "cd ${DIR} && ${compose} up -d --no-recreate && ${compose} up -d server"

# 後始末:同じ tag を再ビルド/再 load する度、前の版が <none>(dangling)で残って
# 溜まる。両側で dangling のみ掃除(-f = タグ付きには触れない ⇒ ロールバック用の
# 旧版は安全)。失敗してもデプロイ自体は成功扱い(|| true)。
echo "▶ dangling イメージを掃除(ビルド機 + ${HOST})..."
docker image prune -f >/dev/null 2>&1 || true
ssh "$HOST" 'docker image prune -f' >/dev/null 2>&1 || true

echo "✅ ${HOST} に直接デプロイ完了 (image=${IMAGE})"
