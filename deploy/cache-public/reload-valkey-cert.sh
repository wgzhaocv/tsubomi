#!/usr/bin/env sh
# acme.sh の reloadcmd から呼ぶ:cache.tsubomi-app.com の LE 証書を valkey の TLS 卷へ入れ、
# valkey に **無停止リロード**させる(CONFIG SET tls-cert-file / tls-key-file)。
#
# 配備:Pi の ~/tsubomi-deploy(compose プロジェクト名 = tsubomi-deploy)で動かす前提。
#   acme.sh の発行/更新コマンドに `--reloadcmd "/home/zwg/reload-valkey-cert.sh"` を渡す
#   (pgbouncer 側の reload-pgb-cert.sh と同型)。証書更新は 60 日毎程度なので頻度は低い。
#
# 上書き可能な env:
#   LE_DIR        acme.sh の証書ディレクトリ(既定 ~/.acme.sh/cache.tsubomi-app.com_ecc)
#   CACHE_TLS_VOL valkey TLS 卷名(既定 tsubomi-deploy_cache_tls = compose の cache_tls)
#   ENV_FILE      admin パスを読む .env(既定 ~/tsubomi-deploy/.env.production)
set -eu

LE_DIR="${LE_DIR:-$HOME/.acme.sh/cache.tsubomi-app.com_ecc}"
CACHE_TLS_VOL="${CACHE_TLS_VOL:-tsubomi-deploy_cache_tls}"
ENV_FILE="${ENV_FILE:-$HOME/tsubomi-deploy/.env.production}"

# 1) LE 証書を cache_tls 卷へ(一時 alpine コンテナで cp)。valkey は読み取り専用 mount なので
#    卷に直接書く必要がある(ホストから卷の中身は直接見えないため docker run 経由)。
docker run --rm \
  -v "$CACHE_TLS_VOL":/tls \
  -v "$LE_DIR":/le:ro \
  alpine:3 sh -c '
    cp /le/fullchain.cer /tls/server.crt &&
    cp /le/cache.tsubomi-app.com.key /tls/server.key &&
    chmod 644 /tls/server.crt /tls/server.key'

# 2) valkey に無停止リロード。admin パス(default は off、tsubomi-admin だけ管理権)を .env から取る。
#    パスは「英数字のみ」(compose 制約)なので末尾の空白 / CR を除去すれば十分。
PASS="$(grep -E '^TSUBOMI_VALKEY_ADMIN_PASS=' "$ENV_FILE" | head -n1 | cut -d= -f2- | tr -d '[:space:]')"

# valkey が未起動(初回ブートストラップ等)なら CONFIG SET は通らないが、証書は既に卷へ配置済みなので
# 次の起動時に読まれる。そのため reload は best-effort 扱い(set -e で全体を失敗させない)。
if docker exec tsubomi-valkey valkey-cli --user tsubomi-admin -a "$PASS" --no-auth-warning PING >/dev/null 2>&1; then
  docker exec tsubomi-valkey valkey-cli --user tsubomi-admin -a "$PASS" --no-auth-warning \
    CONFIG SET tls-cert-file /tls/server.crt
  docker exec tsubomi-valkey valkey-cli --user tsubomi-admin -a "$PASS" --no-auth-warning \
    CONFIG SET tls-key-file /tls/server.key
  echo "valkey TLS cert reloaded (cache.tsubomi-app.com)"
else
  echo "valkey 未起動 — 証書は卷に配置済み。次回起動で反映されます。"
fi
