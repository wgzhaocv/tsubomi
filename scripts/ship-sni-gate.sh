#!/usr/bin/env bash
# 公網数据库 辺縁 SNI 闸门(crates/sni-gate)を VPS へ build & deploy する。
#   使い方: scripts/ship-sni-gate.sh [proxy]
#
# crates/sni-gate を x86_64-linux-gnu に交叉編譯 → 二進制 + systemd unit を scp →
# 入替 & 再起動。VPS は x86_64 Debian(glibc)なので gnu ターゲット。
#
# 前提(release-cli.sh と同じ):
#   - rustup 1.95 toolchain に x86_64-unknown-linux-gnu の std
#   - cargo-zigbuild + zig(クロスリンク)
#   - Homebrew の rust が PATH 先頭にいるため ~/.cargo/bin を前置する
#   - 配備先(既定 proxy)は root SSH 可
set -euo pipefail
cd "$(dirname "$0")/.."

HOST="${1:-${SNI_GATE_HOST:-proxy}}"
export PATH="$HOME/.cargo/bin:$PATH"
TARGET=x86_64-unknown-linux-gnu

echo "=== build tsubomi-sni-gate ($TARGET) ==="
cargo zigbuild --release -p tsubomi-sni-gate --target "$TARGET"
BIN="target/$TARGET/release/tsubomi-sni-gate"

echo "=== ship to $HOST ==="
# 二進制は一時名で scp → atomic mv(走行中バイナリの差し替えは mv が安全)。
# 1 つのバイナリを 2 インスタンスで使う:tsubomi-sni-gate(:443 pg)+ tsubomi-cache-gate(:8080 cache・
# 独立 frp 池・SNI 無し許可)。両 unit を配って両方再起動する。
scp -q "$BIN" "$HOST:/usr/local/bin/tsubomi-sni-gate.new"
scp -q deploy/sni-gate/tsubomi-sni-gate.service "$HOST:/etc/systemd/system/tsubomi-sni-gate.service"
scp -q deploy/cache-gate/tsubomi-cache-gate.service "$HOST:/etc/systemd/system/tsubomi-cache-gate.service"
ssh "$HOST" 'set -e
  mv /usr/local/bin/tsubomi-sni-gate.new /usr/local/bin/tsubomi-sni-gate
  chmod 755 /usr/local/bin/tsubomi-sni-gate
  systemctl daemon-reload
  systemctl enable tsubomi-sni-gate tsubomi-cache-gate
  systemctl restart tsubomi-sni-gate tsubomi-cache-gate
  sleep 0.6
  systemctl --no-pager --full status tsubomi-sni-gate tsubomi-cache-gate | head -30'

echo ""
echo "=== done。確認: ssh $HOST '\''ss -tlnp | grep -E \":443|:8080\"'\'' / journalctl -u tsubomi-cache-gate -f ==="
