#!/usr/bin/env bash
# tbm CLI のリリースを作って香橙派へ公開する。
#   使い方: scripts/release-cli.sh [zwg@192.168.0.106]
#
# 4 ターゲットをビルド → tar.gz/zip に梱包 → sha256 → manifest.json →
# Pi の ~/tsubomi/releases/ へ scp。サーバは TSUBOMI_RELEASE_DIR から
# /api/cli/version と /api/cli/dl/ を配信する。
#
# 前提:
#   - rustup の 1.95 toolchain に各 target の std(rustup target add 済み)
#   - cargo-zigbuild + zig(linux / windows-gnu のクロスリンク)
#   - Homebrew の rust が PATH 先頭にいるため ~/.cargo/bin を前置する
set -euo pipefail
cd "$(dirname "$0")/.."

# 公開先:引数 > TSUBOMI_RELEASE_PI > デフォルト(現行の香橙派)。
PI="${1:-${TSUBOMI_RELEASE_PI:-zwg@192.168.0.106}}"
export PATH="$HOME/.cargo/bin:$PATH"

VERSION="$(grep -m1 '^version' crates/cli/Cargo.toml | sed 's/.*"\(.*\)"/\1/')"
OUT="target/cli-release"
rm -rf "$OUT" && mkdir -p "$OUT/dl" "$OUT/latest"

# リリースは不可変:同じバージョンを別の内容で再発行すると、CDN(Cloudflare は
# .gz/.zip をエッジでキャッシュする)に古いアーカイブが残り、manifest の sha256 と
# 食い違って checksum mismatch になる。内容を変えたら必ず version を上げること。
if ssh "$PI" "test -f ~/tsubomi/releases/dl/tbm-$VERSION-aarch64-apple-darwin.tar.gz" 2>/dev/null; then
  echo "error: tbm $VERSION は既に公開済み。crates/cli/Cargo.toml の version を上げてから再実行。" >&2
  exit 1
fi

echo "=== building tbm $VERSION for 4 targets ==="
# mac は native、それ以外は zigbuild(zig がリンカ)。
cargo build --release -p tsubomi-cli                                      # aarch64-apple-darwin
cargo zigbuild --release -p tsubomi-cli --target aarch64-unknown-linux-gnu
cargo zigbuild --release -p tsubomi-cli --target x86_64-unknown-linux-gnu
cargo zigbuild --release -p tsubomi-cli --target x86_64-pc-windows-gnu

package_unix() { # $1=target $2=binary-path
  local name="tbm-$VERSION-$1.tar.gz"
  tar -czf "$OUT/dl/$name" -C "$(dirname "$2")" tbm
  echo "$name"
}

echo "=== packaging ==="
A_MAC="$(package_unix aarch64-apple-darwin target/release/tbm)"
A_LARM="$(package_unix aarch64-unknown-linux-gnu target/aarch64-unknown-linux-gnu/release/tbm)"
A_LX64="$(package_unix x86_64-unknown-linux-gnu target/x86_64-unknown-linux-gnu/release/tbm)"
A_WIN="tbm-$VERSION-x86_64-pc-windows-gnu.zip"
(cd target/x86_64-pc-windows-gnu/release && zip -q "$OLDPWD/$OUT/dl/$A_WIN" tbm.exe)

sha() { shasum -a 256 "$OUT/dl/$1" | awk '{print $1}'; }

# manifest の url は相対パス:デプロイ先のドメインに依存しない。
cat > "$OUT/latest/manifest.json" <<EOF
{
  "version": "$VERSION",
  "targets": [
    { "target": "aarch64-apple-darwin",      "url": "/api/cli/dl/$A_MAC",  "sha256": "$(sha "$A_MAC")" },
    { "target": "aarch64-unknown-linux-gnu", "url": "/api/cli/dl/$A_LARM", "sha256": "$(sha "$A_LARM")" },
    { "target": "x86_64-unknown-linux-gnu",  "url": "/api/cli/dl/$A_LX64", "sha256": "$(sha "$A_LX64")" },
    { "target": "x86_64-pc-windows-gnu",     "url": "/api/cli/dl/$A_WIN",  "sha256": "$(sha "$A_WIN")" }
  ]
}
EOF

echo "=== publishing to $PI ==="
ssh "$PI" 'mkdir -p ~/tsubomi/releases/dl ~/tsubomi/releases/latest'
scp -q "$OUT"/dl/* "$PI":~/tsubomi/releases/dl/
scp -q "$OUT/latest/manifest.json" "$PI":~/tsubomi/releases/latest/manifest.json

echo ""
echo "released tbm $VERSION:"
ls -lh "$OUT/dl"
echo ""
echo "確認: curl -s \$SERVER/api/cli/version | head"
