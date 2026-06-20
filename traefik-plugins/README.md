# traefik-plugins —— Traefik ローカルプラグイン(vendor)

CF Tunnel 配下では cloudflared が Traefik の直接 peer(docker 網 172.x)になり、かつ
**`X-Forwarded-For` を送らず `Cf-Connecting-Ip` だけ送る**。Traefik の `ipAllowList` は
XFF / RemoteAddr しか読めない(CF-Connecting-IP は読めない)ため、会社 IP 許可リストが
実 client IP で判定できない。これを埋めるためのローカルプラグイン。

## 中身

- `src/github.com/kubitodev/traefik-cloudflared-source-ip/` — [kubitodev/traefik-cloudflared-source-ip](https://github.com/kubitodev/traefik-cloudflared-source-ip) v1.0.9(MIT)を vendor。
  `Cf-Connecting-Ip` を `X-Forwarded-For` / `X-Real-Ip` に写す(純 header 操作・依存なし・Yaegi 互換)。
  リモート取得(`--experimental.plugins`)だと Traefik 起動時に联网必須でビルド機/カタログ障害で起動不能になりうるため、
  **ローカル vendor**(`--experimental.localPlugins`)にして起動の联网依存を断つ。

## Traefik への配線(`compose.prod.yml`)

```
command:
  - --experimental.localPlugins.cloudflared-source-ip.moduleName=github.com/kubitodev/traefik-cloudflared-source-ip
  - --entrypoints.web.http.middlewares=cloudflared-source-ip@file   # web 入口に全体適用(各 service の ipallow より先)
volumes:
  - <plugins-dir>:/plugins-local:ro    # Traefik は /plugins-local/src/<moduleName>/ を探す
```

middleware の定義は `traefik-dynamic/cloudflare-realip.yml`(静的 dynamic 設定。`excludednets`=内部網)。

## Pi への配置(`just ship` が自動配布)

`scripts/ship.sh` が compose と一緒に、この `traefik-plugins/` と `traefik-dynamic/cloudflare-realip.yml`
を `/srv/tsubomi/{traefik-plugins,traefik-dynamic}` へ **docker 経由**で配る(`/srv/tsubomi` は root 所有 +
zwg は sudo 無しのため)。冪等で fresh host も自動セットアップ。既定パスは compose の
`TSUBOMI_TRAEFIK_PLUGINS_DIR` / `TSUBOMI_TRAEFIK_DYNAMIC_DIR`。

`just ship` を使わない経路(DockerHub pull で更新する VPS 等)では手動で同じ配置を:

```sh
# リポジトリの traefik-plugins/ と traefik-dynamic/cloudflare-realip.yml を Pi の home へ scp した後:
docker run --rm -v /srv/tsubomi:/dest -v $HOME/staging:/src:ro alpine sh -c '
  mkdir -p /dest/traefik-plugins /dest/traefik-dynamic &&
  cp -r /src/traefik-plugins/. /dest/traefik-plugins/ &&
  cp /src/cloudflare-realip.yml /dest/traefik-dynamic/cloudflare-realip.yml'
```

注:プラグイン配線(localPlugins / entrypoint middleware)の反映には traefik の再作成が要る
(`up -d traefik`)。ship は no-recreate なので既存 traefik は作り直さない。

## 信頼前提

Traefik は loopback(`127.0.0.1:80`)のみ listen = 入口は cloudflared だけ。client は Traefik に直接届かず
`Cf-Connecting-Ip` を偽装できない(CF が edge で必ず上書きする)。観測上 client 由来の XFF は Traefik に届かない
(cloudflared が転送しない)ので、プラグインは `Cf-Connecting-Ip` に回退して実 IP を得る。`TSUBOMI_BIND_ADDR` や
入口を公網に開く構成にするとこの前提が崩れる。
