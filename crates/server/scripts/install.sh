#!/bin/sh
# tbm CLI インストーラ — POSIX shell(mac / Linux)。
# 使い方: curl -fsSL https://<ドメイン>/install.sh | sh
#
# ~/.tbm/bin/tbm に入れて(sudo 不要)、PATH ブロックをシェルの rc に追記する。
# ブロックはマーカー付きで、`tbm uninstall` が丸ごと取り除く(残留物ゼロ)。
# あわせて前提ツール(git / gh)を確認し、管理者権限なしで入れられるものは
# 同じ ~/.tbm/bin に入れる(__SERVER_URL__ は配信時にサーバが実ドメインへ置換)。
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
    echo "未対応のプラットフォームです: $(uname -s)-$(uname -m)" >&2; exit 1 ;;
esac

# ターゲット別のフラット manifest:{ version, target, url, sha256 }。
# ネストした配列を POSIX shell でパースしないための形。
INFO="$(curl -fsSL "$SERVER/api/cli/version/$TARGET")"
[ -n "$INFO" ] || { echo "$SERVER/api/cli/version/$TARGET の取得に失敗しました" >&2; exit 1; }
extract() { echo "$INFO" | sed -n "s/.*\"$1\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p" | head -n 1; }
VERSION="$(extract version)"
URL="$(extract url)"
EXPECTED_SHA="$(extract sha256)"
[ -n "$VERSION" ] && [ -n "$URL" ] && [ -n "$EXPECTED_SHA" ] \
  || { echo "$SERVER から不完全な manifest を受け取りました" >&2; exit 1; }

# manifest の url は相対パス(ドメイン非依存)。絶対ならそのまま。
case "$URL" in
  /*) URL="$SERVER$URL" ;;
esac

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
ARCHIVE="$TMP/tbm.tar.gz"

echo "tbm $VERSION をダウンロードしています($TARGET)"
curl -fsSL "$URL" -o "$ARCHIVE"

# manifest の sha256 と照合。改竄・途中で切れた配信・キャッシュ汚染の
# どれであっても、PATH にバイナリを置く前に止める。
if command -v sha256sum >/dev/null 2>&1; then
  ACTUAL_SHA="$(sha256sum "$ARCHIVE" | awk '{print $1}')"
else
  ACTUAL_SHA="$(shasum -a 256 "$ARCHIVE" | awk '{print $1}')"
fi
if [ "$ACTUAL_SHA" != "$EXPECTED_SHA" ]; then
  echo "$URL のチェックサムが一致しません" >&2
  echo "  期待値: $EXPECTED_SHA" >&2
  echo "  実際:   $ACTUAL_SHA" >&2
  exit 1
fi

mkdir -p "$INSTALL_DIR"
tar -xz -C "$TMP" -f "$ARCHIVE"
chmod +x "$TMP/tbm"
mv "$TMP/tbm" "$INSTALL_DIR/tbm"

echo ""
echo "tbm $VERSION を $INSTALL_DIR/tbm に入れました"

# PATH 追記:マーカー付きブロックを、存在する rc ファイルに入れる。
# 既にブロックがあるファイルはスキップ(再実行しても増殖しない)。
# `tbm uninstall` はこのマーカーを目印にブロックごと取り除く。
PATH_LINE="export PATH=\"\$HOME/.tbm/bin:\$PATH\""
add_block() {
  rc="$1"
  [ -f "$rc" ] || return 0
  grep -qF "$MARKER_BEGIN" "$rc" && return 0
  printf '\n%s\n%s\n%s\n' "$MARKER_BEGIN" "$PATH_LINE" "$MARKER_END" >> "$rc"
  echo "$rc に PATH を追記しました"
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
  echo "$FISH_RC に PATH を追記しました"
  ADDED=1
fi
# rc が一つも無い環境(素の sh など)では ~/.profile を作る。
if [ -z "$ADDED" ]; then
  printf '%s\n%s\n%s\n' "$MARKER_BEGIN" "$PATH_LINE" "$MARKER_END" >> "$HOME/.profile"
  echo "~/.profile を作成し PATH を追記しました"
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
  echo "接続先サーバを設定しました: $SERVER"
fi

# 前提ツール(git / gh = GitHub CLI)。tbm の GitHub デプロイ経路で必須。
# gh は GitHub 公式 release から ~/.tbm/bin に入れる(管理者権限不要 — PATH も
# uninstall も tbm と同じ仕組みでカバーされる)。git は mac/Linux に管理者権限
# なしの公式配布が無いため、自動導入はせず手順だけ案内する(設計上の判断)。
install_gh() {
  # latest のリダイレクト先からバージョンを読む方式。GitHub API のレート制限
  # (60回/時/IP)を踏まないため。転送は GitHub の TLS で認証されるので、会社
  # ドメイン配信の tbm と違い別途 sha256 照合はしない(脅威モデルが異なる)。
  gh_tagurl="$(curl -fsSLI -o /dev/null -w '%{url_effective}' https://github.com/cli/cli/releases/latest)" || return 1
  gh_tag="${gh_tagurl##*/tag/}"   # v2.x.y
  gh_ver="${gh_tag#v}"            # 2.x.y
  [ -n "$gh_ver" ] && [ "$gh_tag" != "$gh_tagurl" ] || return 1
  case "$(uname -s)-$(uname -m)" in
    Darwin-*)                  gh_asset="gh_${gh_ver}_macOS_arm64.zip" ;;
    Linux-x86_64)              gh_asset="gh_${gh_ver}_linux_amd64.tar.gz" ;;
    Linux-aarch64|Linux-arm64) gh_asset="gh_${gh_ver}_linux_arm64.tar.gz" ;;
    *) return 1 ;;
  esac
  gh_url="https://github.com/cli/cli/releases/download/${gh_tag}/${gh_asset}"
  gh_tmp="$(mktemp -d)"
  # mac は zip・Linux は tar.gz だが、どちらも tar -xf で展開できる
  # (macOS の bsdtar は zip も解凍、Linux の gnu tar は gz を解凍)。
  # chmod/mv まで && 連鎖に含める:install_gh は if 条件で呼ばれ set -e が
  # 効かないので、ここで取りこぼすと配置失敗でも 0 を返して「入れた」と誤報する。
  if curl -fsSL "$gh_url" -o "$gh_tmp/gh.archive" \
     && tar -xf "$gh_tmp/gh.archive" -C "$gh_tmp" 2>/dev/null \
     && gh_bin="$(find "$gh_tmp" -type f -name gh 2>/dev/null | head -n 1)" \
     && [ -n "$gh_bin" ] \
     && chmod +x "$gh_bin" \
     && mv "$gh_bin" "$INSTALL_DIR/gh"; then
    rm -rf "$gh_tmp"
    return 0
  fi
  rm -rf "$gh_tmp"
  return 1
}

# Claude Code のユーザ設定(~/.claude/settings.json)に 2 つの既定を入れる:
#   permissions.defaultMode = "auto"(プロンプトをほぼ出さない auto モード。
#     ※ auto は Opus 4.6+ / Sonnet 4.6 かつ「ユーザ級」設定でのみ有効。条件を
#     満たさないと claude が静かに既定モードへ戻る — それは仕様)
#   tui = "fullscreen"(ちらつかない全画面描画)
# 既存の設定は壊さない(この 2 キーだけ上書き)。jq か python3 があればマージ、
# どちらも無ければ「不在なら新規作成・在れば手動案内」に倒す。
configure_claude_settings() {
  cdir="$HOME/.claude"
  cfile="$cdir/settings.json"
  mkdir -p "$cdir"
  if [ ! -f "$cfile" ]; then
    cat > "$cfile" <<'JSON'
{
  "permissions": {
    "defaultMode": "auto"
  },
  "tui": "fullscreen"
}
JSON
    echo "Claude Code の設定を作成しました(auto モード + fullscreen)"
    return 0
  fi
  # 既存が bypassPermissions(より強い)なら降格しない。それ以外は auto に。tui は常に fullscreen。
  if command -v jq >/dev/null 2>&1; then
    tmpf="$(mktemp)"
    if jq 'if type != "object" then {} else . end | if (.permissions | type) != "object" then .permissions = {} else . end | if .permissions.defaultMode != "bypassPermissions" then .permissions.defaultMode = "auto" else . end | .tui = "fullscreen"' "$cfile" > "$tmpf" 2>/dev/null; then
      mv "$tmpf" "$cfile"
      echo "Claude Code の設定を更新しました(auto モード + fullscreen)"
    else
      rm -f "$tmpf"
      echo "ℹ Claude Code 設定の自動更新に失敗。~/.claude/settings.json に permissions.defaultMode=\"auto\" と tui=\"fullscreen\" を手動で足してください" >&2
    fi
  elif command -v python3 >/dev/null 2>&1; then
    if python3 - "$cfile" <<'PY' 2>/dev/null
import json, sys
p = sys.argv[1]
try:
    d = json.load(open(p))
except Exception:
    d = {}
if not isinstance(d, dict):
    d = {}
perm = d.get("permissions")
if not isinstance(perm, dict):
    perm = {}
if perm.get("defaultMode") != "bypassPermissions":
    perm["defaultMode"] = "auto"
d["permissions"] = perm
d["tui"] = "fullscreen"
with open(p, "w") as f:
    json.dump(d, f, indent=2, ensure_ascii=False)
PY
    then
      echo "Claude Code の設定を更新しました(auto モード + fullscreen)"
    else
      echo "ℹ Claude Code 設定の自動更新に失敗。手動で足してください" >&2
    fi
  else
    echo "ℹ Claude Code 設定の自動更新には jq か python3 が要ります。~/.claude/settings.json に permissions.defaultMode=\"auto\" と tui=\"fullscreen\" を手動で足してください" >&2
  fi
}

echo ""
if ! command -v git >/dev/null 2>&1; then
  case "$(uname -s)" in
    Darwin) echo "⚠ git が見つかりません。次を実行して入れてください: xcode-select --install" >&2 ;;
    *)      echo "⚠ git が見つかりません。管理者 / IT 部門に導入を依頼してください(例: sudo apt install git)。" >&2 ;;
  esac
fi
if command -v gh >/dev/null 2>&1; then
  # 既にある → 触らない。ただし未ログインなら一手だけ案内する。
  gh auth status >/dev/null 2>&1 || \
    echo "gh は未ログインです。GitHub と連携するには: gh auth login --web --git-protocol https --clipboard"
else
  echo "gh(GitHub CLI)が見つかりません。インストールしています…"
  if install_gh; then
    echo "gh をインストールしました。GitHub と連携するには: gh auth login --web --git-protocol https --clipboard"
  else
    echo "⚠ gh の自動インストールに失敗しました。手動で導入してください: https://github.com/cli/cli/releases" >&2
  fi
fi

# claude(Claude Code = この PaaS を AI で操作する CLI)。無ければ公式インストーラ
# で導入する(管理者権限不要・~/.local/bin に入り PATH も claude 自身が rc に書く)。
CLAUDE_OK=""
if command -v claude >/dev/null 2>&1; then
  CLAUDE_OK=1  # 既にある → インストールはスキップ
else
  echo "claude(Claude Code)が見つかりません。インストールしています…"
  if curl -fsSL https://claude.ai/install.sh -o "$TMP/claude-install.sh" && bash "$TMP/claude-install.sh"; then
    CLAUDE_OK=1
  else
    echo "⚠ claude の自動インストールに失敗しました。手動で: curl -fsSL https://claude.ai/install.sh | bash" >&2
  fi
fi
if [ -n "$CLAUDE_OK" ]; then
  configure_claude_settings
  # ログイン確認。今のシェルの PATH にはまだ claude が無いかもしれない(install が
  # rc に書いた PATH は次シェルから)ので、絶対パス ~/.local/bin/claude も試す。
  CLAUDE_BIN="$(command -v claude 2>/dev/null || true)"
  [ -n "$CLAUDE_BIN" ] || CLAUDE_BIN="$HOME/.local/bin/claude"
  if [ -x "$CLAUDE_BIN" ]; then
    "$CLAUDE_BIN" auth status >/dev/null 2>&1 || \
      echo "Claude Code は未ログインです。ログインするには: claude auth login"
  fi
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
echo "次のステップ: tbm login"
