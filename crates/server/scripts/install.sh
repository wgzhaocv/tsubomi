#!/bin/sh
# tbm CLI インストーラ — POSIX shell(mac / Linux)。
# 使い方: curl -fsSL https://<ドメイン>/install.sh | sh
#
# ~/.tbm/bin/tbm に入れて(sudo 不要)、PATH ブロックをシェルの rc に追記する。
# ブロックはマーカー付きで、`tbm uninstall` が丸ごと取り除く(残留物ゼロ)。
# __SERVER_URL__ は配信時にサーバが実ドメインへ置換する。
set -eu

SERVER="${TSUBOMI_SERVER_URL:-__SERVER_URL__}"
# インストール先と PATH マーカーは `tbm uninstall`(crates/cli/src/commands/
# uninstall.rs)と同期契約:マーカー文字列の正本は tsubomi-shared の
# PATH_MARKER_BEGIN/END。どちらかを変えるときは両方揃えること。
INSTALL_ROOT="${HOME}/.tbm"
INSTALL_DIR="${INSTALL_ROOT}/bin"
MARKER_BEGIN="# >>> tbm cli >>>"
MARKER_END="# <<< tbm cli <<<"

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64)          TARGET="aarch64-apple-darwin" ;;
  Linux-x86_64)          TARGET="x86_64-unknown-linux-gnu" ;;
  Linux-aarch64|Linux-arm64) TARGET="aarch64-unknown-linux-gnu" ;;
  Darwin-x86_64)
    echo "tbm は Intel Mac には未対応です(Apple Silicon のみ)。" >&2; exit 1 ;;
  *)
    echo "unsupported platform: $(uname -s)-$(uname -m)" >&2; exit 1 ;;
esac

# ターゲット別のフラット manifest:{ version, target, url, sha256 }。
# ネストした配列を POSIX shell でパースしないための形。
INFO="$(curl -fsSL "$SERVER/api/cli/version/$TARGET")"
[ -n "$INFO" ] || { echo "failed to fetch $SERVER/api/cli/version/$TARGET" >&2; exit 1; }
extract() { echo "$INFO" | sed -n "s/.*\"$1\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p" | head -n 1; }
VERSION="$(extract version)"
URL="$(extract url)"
EXPECTED_SHA="$(extract sha256)"
[ -n "$VERSION" ] && [ -n "$URL" ] && [ -n "$EXPECTED_SHA" ] \
  || { echo "incomplete manifest from $SERVER" >&2; exit 1; }

# manifest の url は相対パス(ドメイン非依存)。絶対ならそのまま。
case "$URL" in
  /*) URL="$SERVER$URL" ;;
esac

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
ARCHIVE="$TMP/tbm.tar.gz"

echo "downloading tbm $VERSION ($TARGET)"
curl -fsSL "$URL" -o "$ARCHIVE"

# manifest の sha256 と照合。改竄・途中で切れた配信・キャッシュ汚染の
# どれであっても、PATH にバイナリを置く前に止める。
if command -v sha256sum >/dev/null 2>&1; then
  ACTUAL_SHA="$(sha256sum "$ARCHIVE" | awk '{print $1}')"
else
  ACTUAL_SHA="$(shasum -a 256 "$ARCHIVE" | awk '{print $1}')"
fi
if [ "$ACTUAL_SHA" != "$EXPECTED_SHA" ]; then
  echo "checksum mismatch for $URL" >&2
  echo "  expected: $EXPECTED_SHA" >&2
  echo "  actual:   $ACTUAL_SHA" >&2
  exit 1
fi

mkdir -p "$INSTALL_DIR"
tar -xz -C "$TMP" -f "$ARCHIVE"
chmod +x "$TMP/tbm"
mv "$TMP/tbm" "$INSTALL_DIR/tbm"

echo ""
echo "tbm $VERSION installed to $INSTALL_DIR/tbm"

# PATH 追記:マーカー付きブロックを、存在する rc ファイルに入れる。
# 既にブロックがあるファイルはスキップ(再実行しても増殖しない)。
# `tbm uninstall` はこのマーカーを目印にブロックごと取り除く。
PATH_LINE="export PATH=\"\$HOME/.tbm/bin:\$PATH\""
add_block() {
  rc="$1"
  [ -f "$rc" ] || return 0
  grep -qF "$MARKER_BEGIN" "$rc" && return 0
  printf '\n%s\n%s\n%s\n' "$MARKER_BEGIN" "$PATH_LINE" "$MARKER_END" >> "$rc"
  echo "added PATH block to $rc"
}

ADDED=""
for rc in "$HOME/.zshrc" "$HOME/.bashrc" "$HOME/.bash_profile" "$HOME/.profile"; do
  if [ -f "$rc" ]; then
    add_block "$rc"
    ADDED=1
  fi
done
# fish は構文が違うので別建て。
FISH_RC="$HOME/.config/fish/config.fish"
if [ -f "$FISH_RC" ] && ! grep -qF "$MARKER_BEGIN" "$FISH_RC"; then
  printf '\n%s\nfish_add_path -g "%s"\n%s\n' "$MARKER_BEGIN" "$INSTALL_DIR" "$MARKER_END" >> "$FISH_RC"
  echo "added PATH block to $FISH_RC"
  ADDED=1
fi
# rc が一つも無い環境(素の sh など)では ~/.profile を作る。
if [ -z "$ADDED" ]; then
  printf '%s\n%s\n%s\n' "$MARKER_BEGIN" "$PATH_LINE" "$MARKER_END" >> "$HOME/.profile"
  echo "created ~/.profile with PATH block"
fi

# 初期設定:server_url を書いておく(インストーラは自分のドメインを知っている)。
# これが無いと `tbm login` が dev デフォルト(localhost)に向かってしまう。
# 既存の設定(トークン入りかもしれない)は壊さない。
# パスは Rust 側 ProjectDirs::from("jp","flegrowth","tsubomi")(crates/cli/src/
# config.rs)の解決結果をミラーしている。そちらを変えたらここも揃えること。
case "$(uname -s)" in
  Darwin) CFG_DIR="$HOME/Library/Application Support/jp.flegrowth.tsubomi" ;;
  *)      CFG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/tsubomi" ;;
esac
if [ ! -f "$CFG_DIR/config.toml" ]; then
  mkdir -p "$CFG_DIR"
  printf 'server_url = "%s"\n' "$SERVER" > "$CFG_DIR/config.toml"
  chmod 600 "$CFG_DIR/config.toml"
  echo "configured server: $SERVER"
fi

# curl | sh は子プロセスなので、親シェルの PATH はここからは触れない
# (Unix の原則:環境は親→子にしか流れない)。rc には書いたので新しい
# シェルでは効く。今のシェルで即使うには exec $SHELL(推奨コマンドに同梱)。
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    echo ""
    echo "今のシェルですぐ使うには: exec \$SHELL"
    ;;
esac

echo ""
echo "next: tbm login"
