#!/usr/bin/env sh
# acme.sh の reloadcmd から呼ぶ:db.tsubomi-app.com の LE 証書を pgbouncer の TLS ボリュームへ入れ、
# pgbouncer に **SIGHUP で再読込**させ、**実際に出ている証書が入れた物と一致するまで確認する**。
#
# **これは仕組み上の要**(2026-07-26):service へ注入する内部接続文字列のホストは pgbouncer 証書の
# 名前(= この公開名)なので、**この証書が切れると全テナント app の DB 接続が落ちる**。更新の
# 自動化はオプションではない(理由は doc/paas-db-public-design.md)。だからこのスクリプトは
# **閉環しない限り非零で終わる** — acme.sh のログに失敗が残り、黙って古い証書のまま進まない。
#
# 配備:Pi の ~/tsubomi-deploy(compose プロジェクト名 = tsubomi-deploy)で動かす前提。
#   acme.sh の発行/更新コマンドに `--reloadcmd "/home/zwg/reload-pgb-cert.sh"` を渡す
#   (cache 側の reload-valkey-cert.sh と同型)。証書更新は 60 日毎程度なので頻度は低い。
#   ※本番ホストには既に配備済み。このファイルは**正本の留め置き**(ホストが飛んだら DR で使う)。
#
# 上書き可能な env:
#   LE_DOMAIN   証書の名前(既定 db.tsubomi-app.com)。LE_DIR / 鍵ファイル名の単一真源。
#   LE_DIR      acme.sh の証書ディレクトリ(既定 ~/.acme.sh/<LE_DOMAIN>_ecc)
#   PGB_TLS_VOL pgbouncer TLS ボリューム名(既定 tsubomi-deploy_pgb_tls = compose の pgb_tls)
#   PGB_NAME    pgbouncer のコンテナ名(既定 tsubomi-pgbouncer)
set -eu

LE_DOMAIN="${LE_DOMAIN:-db.tsubomi-app.com}"
LE_DIR="${LE_DIR:-$HOME/.acme.sh/${LE_DOMAIN}_ecc}"
PGB_TLS_VOL="${PGB_TLS_VOL:-tsubomi-deploy_pgb_tls}"
PGB_NAME="${PGB_NAME:-tsubomi-pgbouncer}"

# **ボリューム名の取り違え防止**:誤った名前を渡すと docker は新しい空ボリュームを作って「成功」してしまい、
# 実 pgbouncer は古い証書のまま = 最も危険な偽成功。実際に pgbouncer が使っているボリュームか確かめる。
if ! docker inspect "$PGB_NAME" \
  --format '{{range .Mounts}}{{.Name}}{{"\n"}}{{end}}' 2>/dev/null |
  grep -qx "$PGB_TLS_VOL"; then
  echo "✗ ボリューム $PGB_TLS_VOL は $PGB_NAME にマウントされていません(PGB_TLS_VOL を確認)" >&2
  exit 1
fi

# 1) LE 証書を pgb_tls ボリュームへ(一時 alpine コンテナで cp)。pgbouncer は読み取り専用 mount なので
#    ボリュームに直接書く必要がある(ホストからボリュームの中身は直接見えないため docker run 経由)。
#    compose の pgbouncer-certgen が置く自己署名の種を、ここで公開名の LE 証書に差し替える。
#
#    **cert と key は「対」として切り替える**:2 つの mv の間で電源が落ちると新 cert + 旧 key が
#    残り、次の起動で TLS が壊れる。そこで版付きディレクトリへ両方置いて**symlink 1 本を張り替える**
#    (rename は同一ボリュームで原子的 = 対の切替も 1 手で済む)。pgbouncer.ini が指す
#    /etc/pgbouncer/tls/server.{crt,key} は、この symlink 経由で常に整合した対を指す。
#    併せて **cert と key が対か**(公開鍵の一致)を切替前に検証する。
docker run --rm \
  -v "$PGB_TLS_VOL":/tls \
  -v "$LE_DIR":/le:ro \
  -e LE_DOMAIN="$LE_DOMAIN" \
  alpine:3 sh -c '
    set -eu
    apk add --no-cache openssl >/dev/null 2>&1
    test -f /le/fullchain.cer
    test -f "/le/${LE_DOMAIN}.key"
    # cert と key が対応しているか(公開鍵が一致するか)を先に検証する。
    c=$(openssl x509 -in /le/fullchain.cer -noout -pubkey)
    k=$(openssl pkey -in "/le/${LE_DOMAIN}.key" -pubout)
    [ "$c" = "$k" ] || { echo "✗ cert と key が対応していません" >&2; exit 1; }
    rel=versions/$(openssl x509 -in /le/fullchain.cer -noout -serial | cut -d= -f2)
    mkdir -p "/tls/$rel"
    cp /le/fullchain.cer "/tls/$rel/server.crt"
    cp "/le/${LE_DOMAIN}.key" "/tls/$rel/server.key"
    # pgbouncer(非 root で走る)が読める必要がある。ボリュームは docker 内部にしか露出しないので、
    # 実行 uid を仮定して締めるより **読めることを確実にする**方を採る(読めないと全テナントの
    # DB が落ちる = 締めすぎの代償が大きすぎる)。
    chmod 644 "/tls/$rel/server.crt" "/tls/$rel/server.key"
    # symlink を原子的に張り替える(ln -sf は既存を上書きするだけで原子的でないので tmp 経由)。
    # **相対**リンクにするのが要点:このボリュームは helper では /tls、pgbouncer では
    # /etc/pgbouncer/tls にマウントされるので、絶対パスだと pgbouncer 側で解決できない。
    ln -sf "$rel/server.crt" /tls/.server.crt.lnk && mv -T /tls/.server.crt.lnk /tls/server.crt
    ln -sf "$rel/server.key" /tls/.server.key.lnk && mv -T /tls/.server.key.lnk /tls/server.key
    # 古い版を掃除(直近 3 版だけ残す。ロールバック用の余地 + ボリュームの肥大防止)。
    ls -1dt /tls/versions/*/ 2>/dev/null | tail -n +4 | xargs -r rm -rf
    # 入れた証書の指紋を、この後の閉環確認のために書き出す。
    openssl x509 -in /le/fullchain.cer -noout -fingerprint -sha256 | cut -d= -f2 > /tls/expected.fp'

want=$(docker run --rm -v "$PGB_TLS_VOL":/tls:ro alpine:3 cat /tls/expected.fp)

# 2) pgbouncer に再読込させる。SIGHUP は設定と証書を読み直すだけで、張られている接続は切らない。
#    未起動(初回ブートストラップ等)なら証書は既にボリュームへ配置済みなので次の起動で読まれる = 正常終了。
if ! docker kill -s HUP "$PGB_NAME" >/dev/null 2>&1; then
  echo "$PGB_NAME 未起動 — 証書はボリュームに配置済み。次回起動で反映されます。"
  exit 0
fi

# 3) **閉環確認**:HUP の戻り値は「信号を送れた」だけなので、serving している leaf の指紋が
#    入れた物と一致するまで待つ。一致しなければ**非零で終わる**(古い証書のまま黙って成功しない)。
i=0
while [ "$i" -lt 10 ]; do
  got=$(docker run --rm --network "container:$PGB_NAME" alpine:3 sh -c \
    'apk add --no-cache openssl >/dev/null 2>&1 &&
     echo | openssl s_client -connect 127.0.0.1:6432 -starttls postgres 2>/dev/null |
       openssl x509 -noout -fingerprint -sha256 2>/dev/null | cut -d= -f2' || true)
  if [ "$got" = "$want" ]; then
    docker run --rm --network "container:$PGB_NAME" alpine:3 sh -c \
      'apk add --no-cache openssl >/dev/null 2>&1 &&
       echo | openssl s_client -connect 127.0.0.1:6432 -starttls postgres 2>/dev/null |
         openssl x509 -noout -subject -dates'
    echo "✓ pgbouncer が新しい証書を出しています ($LE_DOMAIN)"
    exit 0
  fi
  i=$((i + 1))
  sleep 2
done

echo "✗ SIGHUP 後も serving 証書が入れた物と一致しません(期待 $want / 実際 ${got:-取得不能})。" >&2
echo "  手で $PGB_NAME を再起動して確認してください。放置すると厳格検証する app の DB が落ちます。" >&2
exit 1
