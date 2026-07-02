# registry 直連入口(CF 100MB 上限の回避)— 実装級設計

## 背景(なぜ要るか)

registry の push 入口(`registry.tsubomi-app.com`)は Cloudflare Tunnel 経由で、CF proxy は
**request body ≈100MB(Free/Pro)** の上限を持つ。`docker push` は**層ごとに 1 つの HTTP
リクエスト**で blob を送る(クライアントは層を分割 push できない・設定も無い)ため、
**圧縮後 >100MB の層はどの経路の工夫でも 413 で必ず割れる**(実例:pgroonga イメージ。
2026-07 の実利用フィードバック #3)。上限は CF の計画依存で、Tunnel でも同じ・上げるには
Business/Enterprise しかない(公式 docs + community で確認済み)。

そこで **CF を通らない push 専用の直連入口**を足す。pull / 小さい層の push は従来どおり
CF 経由と**共存**(二枚看板)。

## 経路

```
GHA runner / tbm deploy --local
  --HTTPS--> registry-direct.tsubomi-app.com(DNS: 灰云 A → VPS 133.88.123.119)
    → VPS tsubomi-sni-gate :443(--https-sni 完全一致で准入。TLS 非終端 passthrough)
    → frps 127.0.0.1:6443(allowPorts に追加)
    → frpc(香橙派、/home/zwg/frp/frpc.toml に proxy 追加)
    → traefik :8443(127.0.0.1 bind。**ここで TLS 終端** — LE DNS-01 証明書)
    → basicAuth(registry.yml、CF 入口と同じ middleware)→ tsubomi-registry:5000
```

db-public(sni-gate + frp)と同じ骨格。違いは 2 点:

1. **TLS 終端が本機 traefik**:pgbouncer と違い registry は自前で TLS を張れないので、
   traefik の `registrydirect` entrypoint(:8443)が LE 証明書で終端する。証明書は
   **DNS-01**(cloudflare provider、`CF_DNS_API_TOKEN`)— この部署は公網 :80 が無いので
   HTTP-01 は不成立。DNS-01 なら灰云のままでよい。
2. **IP 許可リストを適用しない**:push 元は GitHub Actions runner(Azure IP、不定)。
   守りは TLS + registry basicAuth + sni-gate の SNI 完全一致(扫描を frp より前で遮断)。

## 実装(コード側。全て条件付き = 未設定なら従来と完全同一)

- `crates/sni-gate`:`--https-backend` / `--https-sni` の第 3 route(TLS-on-connect、
  redis と同じ passthrough 手順。SNI 完全一致必須・SNI 無しは常に拒否)。
- `crates/server/src/config.rs`:`TSUBOMI_REGISTRY_DIRECT`(host[:port]、任意)。
  `registry_ci_host()` = CI へ配る push 先(直連があれば優先)。
- `crates/server/src/services/registry.rs::render`:direct_host があれば第 2 router
  (`registrydirect` entrypoint + `certResolver: ledns`)を registry.yml に追記。
  `load()` の `RegistryCreds.host` が `registry_ci_host()` に。
- `compose.prod.registry-direct.yml`:traefik に :8443 entrypoint + `ledns`
  (DNS-01 cloudflare)+ acme 永続 volume。command 全置換の地雷は db-public と同じ。
- `deploy/sni-gate/tsubomi-sni-gate.service`:ExecStart に `--https-*` を追加。

## 落地手順(運用)

1. **CF DNS**:`registry-direct` A レコード → `133.88.123.119`、**灰云(DNS only)**。
2. **VPS**(`ssh proxy`):
   - `/etc/frp/frps.toml` の `allowPorts` に `{ single = 6443 }` を追加 → `systemctl restart frps`
   - `just ship-sni-gate`(新バイナリ + unit。--https route 付きで再起動)
3. **香橙派**(`ssh zwg@192.168.0.106`):
   - `/home/zwg/frp/frpc.toml` に proxy 追加 → `docker restart tsubomi-frpc`:
     ```toml
     [[proxies]]
     name = "registry-direct"
     type = "tcp"
     localIP = "127.0.0.1"
     localPort = 8443
     remotePort = 6443
     ```
   - `.env.production` に `TSUBOMI_REGISTRY_DIRECT` / `CF_DNS_API_TOKEN` / `TSUBOMI_ACME_EMAIL`
   - `docker compose --env-file .env.production -f compose.prod.yml -f compose.prod.registry-direct.yml up -d traefik`
   - server を新イメージへ(`just ship`)→ 起動時 `sync_traefik` が registry.yml に直連 router を書く
4. **検証**:
   - `docker login registry-direct.tsubomi-app.com`(既存アカウント)が通る
   - >100MB の単層を持つテストイメージの push が成功する(CF 経由では 413 になるもの)
   - 既存 CF 経由の pull(`registry.tsubomi-app.com`)が不変
5. **既存 service の切替**:`gh variable set TSUBOMI_REGISTRY -R <repo> --body registry-direct.tsubomi-app.com`
   (新規 service は create 時から直連が配られる)。

## 受容した性質

- 直連入口は SNI さえ知れば TLS 握手まで到達できる(その先は basicAuth)。sni-gate の
  stats(accepted/no_sni/rejected)で扫描の観測は可能。
- frp 隧道は pg / cache と**別ポートだが同じ frps プロセス**:registry 洪流が pg 池を
  巻き込まないのは sni-gate の SNI 准入が前段で効くため(池自体は tsubomi-frpc = pg と共有。
  cache だけが独立池)。さらに :443 gate 自体の max_pending / max_active も pg と共有 —
  正しい SNI で TLS を張ったまま居座るクライアントは basicAuth 前に gate セッションと frp
  接続を消費できる(codex 監査 [中])。push は稀な CI イベントで handshake timeout も効くので
  受容。**問題になったら**:①frpc-registry を分離(cache 方式)②gate に route 別
  active 上限を足す、の順で対処する。
- acme-dns.json 喪失 = 再発行。named volume で永続化し、LE レート制限(同一 FQDN 週 5 枚)に注意。
