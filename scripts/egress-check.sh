#!/usr/bin/env bash
# M6 egress(出站隔離)の回帰チェック。**prod Linux 上で実行**(dev OrbStack では網隔離を強制しないので
# 無意味)。生存するテナント私網(tsubomi-svc-*)に使い捨て probe コンテナを attach し、出站が設計どおりに
# 縛られているかを assert する:
#   放行: 公網(全 TCP)/ 同桥 infra(pgbouncer・valkey)
#   遮断: 宿主機(gateway / LAN IP / tailnet IP の sshd:22)/ 他テナント(横移)
# 設計は doc/paas-egress-design.md §4。
#
# 使い方(Pi 上 or ssh 越し):
#   ./scripts/egress-check.sh
#   NET=tsubomi-svc-<id> IMAGE=alpine:3 ./scripts/egress-check.sh
# どれか 1 つでも「遮断されるべき宛先に到達できた / 放行されるべき宛先に到達できない」なら exit 1。
set -euo pipefail

IMAGE="${IMAGE:-alpine:3}" # probe イメージ(busybox の nc / timeout を使う)

# ---- 対象テナント網を選ぶ(指定が無ければ最初の tsubomi-svc-*)----
if [ -n "${NET:-}" ]; then
  net="$NET"
else
  net="$(docker network ls --format '{{.Name}}' | grep '^tsubomi-svc-' | head -n1 || true)"
fi
[ -n "$net" ] || { echo "テナント網(tsubomi-svc-*)が見つかりません。service を 1 つデプロイしてから実行してください" >&2; exit 1; }

subnet="$(docker network inspect "$net" --format '{{range .IPAM.Config}}{{.Subnet}}{{end}}' 2>/dev/null || true)"
gw="$(docker network inspect "$net" --format '{{range .IPAM.Config}}{{.Gateway}}{{end}}' 2>/dev/null || true)"
echo "対象テナント網: $net  subnet=$subnet  gateway(=宿主)=$gw"

# ---- 宿主機の到達点を集める(遮断確認の的)----
host_lan="$(ip -4 route get 1.1.1.1 2>/dev/null | awk '{for(i=1;i<=NF;i++)if($i=="src"){print $(i+1);exit}}' || true)"
tailnet="$(ip -4 -o addr show tailscale0 2>/dev/null | awk '{print $4}' | cut -d/ -f1 | head -n1 || true)"
echo "宿主 LAN IP=$host_lan  tailnet IP=${tailnet:-（なし）}"

# ---- 他テナント横移の的(2 つ目の網があれば sleeper を立てて実 IP を得る)----
xtenant_ip=""
sleeper=""
net2="$(docker network ls --format '{{.Name}}' | grep '^tsubomi-svc-' | grep -vx "$net" | head -n1 || true)"
if [ -n "$net2" ]; then
  sleeper="tsubomi-egress-xtgt-$$"
  # 9999 で listen する使い捨て的。egress 無効なら probe から到達でき、有効なら DROP される。
  docker run -d --rm --name "$sleeper" --network "$net2" "$IMAGE" \
    sh -c 'while true; do echo ok | nc -l -p 9999; done' >/dev/null
  xtenant_ip="$(docker inspect "$sleeper" --format "{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}" 2>/dev/null || true)"
  echo "他テナント網: $net2  横移の的=$xtenant_ip:9999"
else
  echo "他テナント網が 1 つしか無いため横移チェックは skip(2 service 目があれば自動で有効)"
fi
cleanup() { [ -n "$sleeper" ] && docker rm -f "$sleeper" >/dev/null 2>&1 || true; }
trap cleanup EXIT

# ---- probe イメージを先に pull(pull は dockerd=宿主が行うので egress に縛られない)----
docker pull -q "$IMAGE" >/dev/null 2>&1 || true

# ---- probe 本体(コンテナ内で実行。期待と違えば exit 1)----
# 値はホスト側で解決して env で渡す(コンテナ内の busybox `ip` に依存しない)。
read -r -d '' probe <<'PROBE' || true
set -u
PASS=0; FAIL=0
ok()  { echo "  ✓ $1"; PASS=$((PASS+1)); }
bad() { echo "  ✗ $1"; FAIL=$((FAIL+1)); }
# 接続できれば 0、遮断(timeout)/ 拒否なら非 0。
can() { timeout 5 nc -w 3 "$1" "$2" </dev/null >/dev/null 2>&1; }
open()  { if can "$1" "$2"; then ok "$3"; else bad "$3 — 到達できない(放行されるべき)"; fi; }
block() { if can "$1" "$2"; then bad "$3 — 到達できる(遮断されるべき)"; else ok "$3"; fi; }

echo "probe ip=$(hostname -i 2>/dev/null)"
echo "[放行されるべき]"
open 1.1.1.1 443 "公網 1.1.1.1:443"
# pgbouncer:6432 に到達できること自体が「DNS が同桥(同 /24=$TENANT_SUBNET)の IP を返す」証拠
# (infra 網の 172.x に解決されると -d 172.16/12 DROP に巻かれ open は失敗する)。
open tsubomi-pgbouncer 6432 "同桥 infra pgbouncer:6432(解決先が同 /24 $TENANT_SUBNET の確認も兼ねる)"
open tsubomi-valkey 6379 "同桥 infra valkey:6379"

echo "[遮断されるべき]"
[ -n "${GW:-}" ]       && block "$GW" 22       "宿主 gateway $GW:22 (sshd)"
[ -n "${HOST_LAN:-}" ] && block "$HOST_LAN" 22 "宿主 LAN $HOST_LAN:22"
[ -n "${TAILNET:-}" ]  && block "$TAILNET" 22  "tailnet $TAILNET:22"
if [ -n "${XTENANT:-}" ]; then block "$XTENANT" 9999 "他テナント横移 $XTENANT:9999"; fi

echo "結果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
PROBE

set +e
docker run --rm --network "$net" \
  -e GW="$gw" -e HOST_LAN="$host_lan" -e TAILNET="$tailnet" -e XTENANT="$xtenant_ip" \
  -e TENANT_SUBNET="$subnet" \
  "$IMAGE" sh -c "$probe"
rc=$?
set -e

if [ "$rc" -eq 0 ]; then
  echo "egress-check: 全て期待どおり ✓"
else
  echo "egress-check: 期待と異なる結果あり(上の ✗ を参照)✗" >&2
fi
exit "$rc"
