# tsubomi PaaS — M3 service 実装設計(第 5 層)

> `paas-tech-design.md`(第 4 層)の §4 デプロイ経路 / §5 注入 / §3 reconcile を、
> **そのまま書き起こせる粒度**まで落とす。migration・compose 追記・API 契約・
> workflow テンプレ・bollard のコンテナパラメータ・traefik label・reconcile の
> ループまで。
>
> **第 4 層と矛盾させない。**§0 の 6 決定は不変。本書が新たに「確定」するのは
> 第 4 層が「M3 で確定」と書いた穴(注入の内部入口など)だけ。それらは §11 に
> 一覧し、各々**否決可**(第 4 層 §0 の作法を踏襲)。
>
> 完了判定(第 4 層 §9 より):**push から 30 秒で `https://<service>.<ドメイン>`
> が開く;`tbm inject` で app が DB / ボリュームに繋がる;ホスト再起動から自己回復。**

---

## 0. スコープ

M3 が出すもの:

- registry(infra に追加)+ traefik(infra に追加)+ `tsubomi-edge` ネットワーク
- `service` リソース一式:create / list / status / logs / start / stop / delete
- GitHub オーケストレーション(CLI が**ユーザ自身の `gh`** で repo + secret + workflow)
- deploy hook(HMAC / nonce / digest)+ 非同期デプロイパイプライン(bollard)
- 注入機構:database / volume → service、静的 env(`tbm env`)、`tbm inject` / `tbm eject`
- reconcile v1(起動時フル + 周期:存在収束 + 孤児掃除 + nonce 掃除)
- web:service 一覧 / 詳細(phase・logs・env・注入)/ 作成導線

M3 が**出さない**もの(後相に送る):compose 複数コンテナ(M6)/ ブルーグリーン
(§6.5 参照、v1 は瞬断許容)/ valkey 注入(M5)/ 管理画面の owner 操作(M4)。

---

## 1. 着工順序(8 スライス、各々単体で検証可能)

| # | スライス | 検証 |
|---|---|---|
| S1 | **migration**:service_details / service_env / injections / deploys / deploy_nonces | `just db-up` でマイグレーション通過、`\d` で 5 表 |
| S2 | **infra**:`tsubomi-edge` 網 + registry + traefik + pgbouncer を edge に参加 | `docker compose up` で 4 サービス健全、`docker push 127.0.0.1:5000/...` 通る |
| S3 | **曳光弾**:手 push したイメージ → `curl` で HMAC hook → digest pull → bollard でコンテナ起動 + **file provider でルーティング**(svc-<id>.yml) → subdomain で開く(create/GH 連携抜き、注入抜き) | ブラウザで `<sub>.localhost`(dev)/ `<sub>.<ドメイン>`(本番)が開く |
| S4 | **service create + GH オーケストレーション**:API(行挿入・鍵生成)+ CLI が `gh` で repo/secret/workflow | `tbm service create x` → `git push` → 自動デプロイ |
| S5 | **deploy パイプライン完成**:phase 状態機械・deploys 履歴・swap・`tbm deploy --local` | `tbm service status` が deploying→running、履歴が残る |
| S6 | **注入**:内部入口確定(§11-A)・env 解決・volume mount・`tbm inject`/`eject`/`env` | コンテナ内で `echo $DATABASE_URL` が app role の内部文字列 |
| S7 | **lifecycle + web**:start/stop/logs/delete + web 画面 | web から phase が見え、停止/再開できる |
| S8 | **reconcile v1**:起動時フル + 周期 tick | host 再起動 → desired=running が自動復活、孤児消える |

S1→S2→S3 が**未知リスクの本体**。ここを貫通させれば残りは「順序の問題」だけになる
(第 4 層 §9 の曳光弾の意図)。

---

## 2. データモデル(migration:`20260615000001_service.sql`)

第 4 層 §2 の DDL を**そのまま**写し起こす(service_details / service_env /
injections / deploys / deploy_nonces)。`resources` スーパーテーブルは M0 で既存
(`anon_seq` / `purge_after` / `trash_meta` も既にある)。本 migration が触るのは
service 系 5 表のみ。

確定する細部(第 4 層 DDL に対する補足):

- **`service_details.container_port int not null default 8080`(本書が足す唯一の列)**:
  app が容器内で listen する port。traefik はここへ転送(§6.3)。対外は常に traefik の
  :80/:443、これは**容器内**の port(80 は非 root が bind 不可 + 框架の既定が高 port —
  §11-B)。既定 8080 + `PORT` 注入で大半は無設定で通り、port を写し固定した app だけ変更。
- `health_path`(就緒探针 URL)は**足さない**:v1 の swap は「旧停止→新起動」で健康
  ゲートを使わない(決定 E)。零瞬断 swap / 誠実な readiness をやる時に足す — 小さな
  migration 1 本で済む(deferred)。
- `service_details.subdomain`:DNS ラベル安全な小文字 slug。生成は §5.3。
- `service_details.deploy_key_enc`:32 byte 乱数を crypto.rs で封緘(DB パスワードと同じ XChaCha20-Poly1305)。HMAC の鍵そのもの。
- `service_details.image_digest`:`sha256:…`(arch ごとに解決される digest、§6.6)。
- `service_details.memory_mb=512 / cpu_shares=1024`:既定値を migration の default で踏襲。
- registry 資格情報は**専用カラムを足さない**:per-user の htpasswd アカウント
  1 つ(§5.2)を `platform_config` か users 派生で持つ。service 毎には作らない
  (digest ピン留めで per-repo ACL 不要 — 決定 #3)。→ §11-D で否決可。

`deploys.status` の遷移は §6.4。`deploy_nonces` は §6.2 でリプレイ防御に使い、
reconcile が 1h 超を掃除(§8)。

---

## 3. infra の追加(`infra/docker-compose.yml`)

### 3.1 ネットワーク

```
networks:
  default:        # 既存。pg-platform / pg-tenant / pgbouncer / registry / traefik
  tsubomi-edge:   # 新規。external: true(平台が bollard で作る/参照する)
    external: true
    name: tsubomi-edge
```

- **`tsubomi-edge` は external**:平台(bollard)がユーザコンテナを attach する網。
  compose は参照するだけ。初回は `docker network create tsubomi-edge`(justfile /
  起動スクリプトに入れる)。
- 接続マトリクス:
  - traefik → default + tsubomi-edge(registry へ default、ユーザコンテナへ edge)
  - registry → default(traefik から到達。ホスト loopback にも publish)
  - **pgbouncer → default + tsubomi-edge**(←§11-A の内部入口。ユーザコンテナが
    `tsubomi-pgbouncer:6432` を docker DNS で引ける)
  - pg-platform / pg-tenant → default のみ(ユーザコンテナから物理的に不可達 — §1 不変)
  - ユーザコンテナ → **tsubomi-edge のみ**(平台が attach。infra default には繋がない)

§1 の隔離は保たれる:コンテナは edge 上の traefik と pgbouncer にしか会えず、
pgbouncer 越しにしか pg-tenant に届かない(pg-tenant の admin 面・pg-platform には
一切触れない)。

### 3.2 registry

```yaml
registry:
  image: registry:2
  container_name: tsubomi-registry
  restart: unless-stopped
  environment:
    REGISTRY_AUTH: htpasswd
    REGISTRY_AUTH_HTPASSWD_REALM: tsubomi
    REGISTRY_AUTH_HTPASSWD_PATH: /auth/htpasswd
    REGISTRY_STORAGE_DELETE_ENABLED: "true"   # GC のため
  volumes:
    - registry_data:/var/lib/registry
    - ./registry/htpasswd:/auth/htpasswd:ro
  ports:
    - "127.0.0.1:5000:5000"   # ローカル pull 用(平台が digest pull)
  labels:                     # 本番のみ:traefik 経由 push 入口
    - traefik.enable=true
    - traefik.http.routers.registry.rule=Host(`registry.${TSUBOMI_DOMAIN}`)
    - traefik.http.routers.registry.entrypoints=websecure
    - traefik.http.routers.registry.tls.certresolver=le
    - traefik.http.services.registry.loadbalancer.server.port=5000
    # registry + hook は ipAllowList を強制除外(決定 #4)= middleware を付けない
```

- **push 入口** = `registry.<ドメイン>`(公開 + basic auth、TLS は traefik が終端)。
  GH Action はここへ push。
- **pull 入口** = `127.0.0.1:5000`(平台が digest 指定 pull)。localhost は docker が
  insecure registry として許すので証明書不要。
- dev:push も pull も `127.0.0.1:5000`。htpasswd は dev 用の固定アカウント。

### 3.3 traefik

```yaml
traefik:
  image: traefik:v3.5
  container_name: tsubomi-traefik
  restart: unless-stopped
  command:
    # file provider のみ(docker provider ではない — §11-H)。平台が動的設定を書き watch で反映。
    - --providers.file.directory=/etc/traefik/dynamic
    - --providers.file.watch=true
    - --entrypoints.web.address=:80
    - --entrypoints.websecure.address=:443       # 本番
    # 本番のみ:LE(会社 CIDR ipAllowList は file provider の middleware = ipblock)
    - --certificatesresolvers.le.acme.tlschallenge=true
    - --certificatesresolvers.le.acme.email=${TSUBOMI_ACME_EMAIL}
    - --certificatesresolvers.le.acme.storage=/acme/acme.json
  ports:
    - "80:80"        # dev は ${TRAEFIK_BIND_ADDR:-127.0.0.1}:8088:80
    - "443:443"      # 本番
  volumes:
    # 平台が書き出す動的設定(svc-<id>.yml + ipallow.yml)。docker.sock は不要。
    - ${TSUBOMI_TRAEFIK_DYNAMIC_DIR}:/etc/traefik/dynamic:ro
    - traefik_acme:/acme   # 本番 LE
  networks: [default, tsubomi-edge]
```

- **file provider(docker provider ではない — §11-H)**:平台が動的設定ファイルを
  `traefik_dynamic_dir`(server の env `TSUBOMI_TRAEFIK_DYNAMIC_DIR`、compose も同じパスを
  read-only マウント)に書き、traefik が watch してホットリロードする。**docker API を一切
  触らない**。後端へはコンテナ名 `tsubomi-<id>` を **edge 網の docker DNS** で解決して到達する
  (名前解決は provider とは別レイヤ)。
  - 各 service:平台が `svc-<id>.yml` を書く(router = Host ルール + service + ipblock
    middleware 参照、service = 後端 `http://tsubomi-<id>:<container_port>`)。実装 `services/route.rs`。
  - **★ 動的設定は YAML(.yml)で書く**:traefik の directory file provider は実測で **.json を
    静かに無視する**(監視には追加されるが設定にマージされない)。YAML は読む。
- **会社 CIDR の ipAllowList = ipblock(file provider の middleware)**:DB の `ip_allow_entries`
  が真実源、`ipblock::sync_traefik` が `ipallow.yml`(middleware `tsubomi-ipallow`)を書く。
  **空リスト = fail-open(全 IP 許可)**、1 件以上でその CIDR だけ許可。各 service router が
  `tsubomi-ipallow@file` を参照する(`docker.rs` ではなく `route.rs` が付与)。registry / hook は
  この middleware を付けないことで除外(決定 #4)。owner だけが CIDR を足す/消せる。
- **TLS = 既定で LE TLS-ALPN-01(按需・子域ごと)。DNS 厂商に依存しない**:必要なのは
  `*.<ドメイン>` の A レコード(どの DNS でも可、API 不要)+ 開いている :80/:443 だけ。
  根拠:§1(第 4 層)で :80/:443 は公網開放で、service の会社 CIDR 制限は traefik の L7
  ipAllowList(防火墙ではない)。ACME チャレンジは entrypoint 層で応答され ipAllowList の
  **前**に処理されるので、LE は公網から任意の子域を検証できる(検証後の実アクセスだけ 403)。
  - **DNS-01 通配证(`*.<ドメイン>`)は配置級のオプション升级**(コード不変、traefik の
    command + provider token を足すだけ — §11-G)。動機は 2 つだけ:LE 速率限界に当たる
    (子域ごと 1 枚 → 頻繁な作成削除)/ CT 透明ログへの子域名漏れを避けたい。当面は不要。
- **dev**:LE 無し、web エントリポイント(:80)だけ。ホスト名は `*.localhost`
  (Chrome/Firefox は `*.localhost` を 127.0.0.1 に解決)。traefik を dev compose で
  `8088:80` に publish し、`http://<sub>.localhost:8088` で開く。本番は :443 + LE。

### 3.4 pgbouncer の edge 参加

既存 pgbouncer に `networks: [default, tsubomi-edge]` を足すだけ。pgbouncer.ini /
userlist.txt / auth_query は M1 のまま不変。ユーザコンテナは
`tsubomi-pgbouncer:6432`(docker DNS)で内部入口に届く。詳細 §7.2。

---

## 4. 状態機械(第 4 層 §3 の確定)

```
desired_state ∈ {running, stopped}      ← ユーザ/owner の操作だけが変える(期望)
phase ∈ {created, deploying, running, stopped, failed}   ← 平台が観測/遷移(実際)

created ──deploy──▶ deploying ──成功──▶ running ◀──start── stopped
   ▲                    │                  │              ▲
   │                  失敗                 stop            │
   │                    ▼                  ▼               │
   └──────────────── failed            stopped ───────────┘
```

- `tbm service create` → `phase=created, desired_state=stopped`(まだ何も走らない)。
- 初回 deploy hook 受信 → `phase=deploying`、成功で `phase=running,
  desired_state=running`、失敗で `phase=failed`(旧コンテナがあれば触らない)。
- `tbm service stop` → `desired_state=stopped` + コンテナ停止 → `phase=stopped`。
- `tbm service start` → `desired_state=running` + 最新 digest でコンテナ起動 →
  `phase=running`(digest が無ければ「まだ deploy していない」エラー)。
- reconcile は **desired と phase の乖離だけ**を直す(§8)。env のドリフトは追わない
  (決定 #5)。

---

## 5. service create + GitHub オーケストレーション(S4)

### 5.1 `POST /api/services` の責務(平台、GitHub 資格情報ゼロ)

入力 `{ display_name, memory_mb?, cpu_shares?, public? }`。やること:

1. `resources` 挿入(kind=service、anon_seq は user+kind 内連番、display_name 一意)。
2. `service_details` 挿入:
   - `deploy_key`(32B 乱数)生成 → `deploy_key_enc` に封緘
   - `subdomain` 生成(§5.3)
   - 既定 limits / `desired_state=stopped, phase=created`
3. **registry 資格情報**:per-user htpasswd アカウントが無ければ作る(§5.2)。
4. レスポンス DTO(CLI が GH 操作に使う):

```json
{
  "id": "…", "display_name": "myapp", "subdomain": "myapp-q3x",
  "deploy_key": "…(平文。発行時の 1 回だけ返す)…",
  "registry": { "host": "registry.<ドメイン>", "user": "<user>", "pass": "…" },
  "hook_url": "https://paas.<ドメイン>/api/hook/deploy",
  "platforms": "linux/arm64",
  "workflow_yaml": "…(§5.4 のテンプレを subdomain 等で展開済み)…"
}
```

`deploy_key` / `registry.pass` は**この応答でしか平文を出さない**(以後は API で
deploy_key を取れるが、それは `tbm deploy --local` 用の自分の service への読み取り権、
第 4 層 §4b)。

### 5.2 registry アカウント

- per-**user** に htpasswd 1 行(`<gh-user-or-uuid>` : bcrypt(乱数パスワード))。
  service 毎には作らない(digest ピン留めで per-repo ACL 不要 — 決定 #3 / §11-D)。
- htpasswd ファイルは registry コンテナがマウントしている `./registry/htpasswd`。
  平台が**ホスト上のこのファイルを追記**して `docker kill -s HUP tsubomi-registry`
  で再読込(registry は HUP で htpasswd を読み直す)。
  - dev は固定 1 アカウント、追記処理はスキップ。

### 5.3 subdomain 生成

`slug(display_name)`(小文字英数 + ハイフン、先頭末尾ハイフン禁止、63 文字以内)。
衝突したら `-<4 文字 base32 乱数>` を付けて再試行(`subdomain` UNIQUE)。
予約語(`paas` / `registry` / `www`)は弾く。

### 5.4 CLI 側オーケストレーション(`tbm service create`)

CLI が**ユーザ自身の `gh`**(ローカルにログイン済み前提)で:

```
1. tbm API POST /api/services  → 上の DTO を受ける
2. gh repo create <user>/<display_name> --private --source=. --remote=tsubomi
   (既存 repo なら --source 無しで作成 or 既存をそのまま使う。冪等)
3. gh secret set TSUBOMI_DEPLOY_KEY  --body <deploy_key>
   gh secret set TSUBOMI_REGISTRY_USER --body <registry.user>
   gh secret set TSUBOMI_REGISTRY_PASS --body <registry.pass>
   gh variable set TSUBOMI_SERVICE_ID  --body <id>
   gh variable set TSUBOMI_REGISTRY    --body <registry.host>
   gh variable set TSUBOMI_HOOK_URL    --body <hook_url>
   gh variable set TSUBOMI_PLATFORMS   --body <platforms>   # 平台が公布する build 対象 arch
4. .github/workflows/tsubomi-deploy.yml を workflow_yaml で書き込み（無ければ）
5. ユーザに「git add/commit/push すれば自動デプロイ」と案内
```

- **平台は GitHub に一切触れない**。`gh` 操作は全部 CLI（ユーザの権限）。
- `gh` が無い / 未ログインなら、手順 2-4 を**ユーザが手で実行する用のコマンド列**を
  出力(AI フレンドリ:json モードでは `{repo, secrets, variables, workflow_yaml}` を
  返すだけで実行しない、text モードでは実行 + 進捗)。
- secret = HMAC 鍵 / registry 資格情報(GitHub Secrets、ログに出ない)。
  variable = 非機密(service_id / registry host / hook url)。

### 5.5 workflow テンプレート(`tsubomi-deploy.yml`)

```yaml
name: tsubomi deploy
on: { push: { branches: [main] } }
jobs:
  deploy:
    # 既定は amd64 ランナー + QEMU。arm64 を原生で速くしたいなら ubuntu-24.04-arm に
    # 変えるだけ(私有 repo の arm ランナーは有料档 — §11-C)。
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: docker/setup-buildx-action@v3
      - uses: docker/login-action@v3
        with:
          registry: ${{ vars.TSUBOMI_REGISTRY }}
          username: ${{ secrets.TSUBOMI_REGISTRY_USER }}
          password: ${{ secrets.TSUBOMI_REGISTRY_PASS }}
      # build:Dockerfile があればそれ、無ければ nixpacks（§11-C）。
      # --platform は平台が公布する arch だけ(既定 linux/arm64)= 使わぬ arch を焼かない
      # (GH Action 分の節約 + QEMU 模拟の最小化)。GHA 層キャッシュで再 build は数十秒。
      - id: build
        run: |
          IMAGE=${{ vars.TSUBOMI_REGISTRY }}/${{ vars.TSUBOMI_SERVICE_ID }}:${{ github.sha }}
          if [ -f Dockerfile ]; then
            docker buildx build --platform "${{ vars.TSUBOMI_PLATFORMS }}" \
              --cache-from type=gha --cache-to type=gha,mode=max \
              --push -t "$IMAGE" --metadata-file meta.json .
            DIGEST=$(jq -r '."containerimage.digest"' meta.json)
          else
            npx -y @railway/nixpacks build . --name "$IMAGE" \
              --platform "${{ vars.TSUBOMI_PLATFORMS }}" --push
            DIGEST=$(docker buildx imagetools inspect "$IMAGE" --format '{{json .Manifest.Digest}}' | tr -d '"')
          fi
          echo "digest=$DIGEST" >> "$GITHUB_OUTPUT"
      - name: notify tsubomi
        run: |
          BODY=$(jq -nc --arg s "${{ vars.TSUBOMI_SERVICE_ID }}" \
            --arg sha "${{ github.sha }}" --arg d "${{ steps.build.outputs.digest }}" \
            --argjson ts "$(date +%s)" --arg n "$(openssl rand -hex 16)" \
            '{service_id:$s, git_sha:$sha, image_digest:$d, ts:$ts, nonce:$n}')
          SIG=$(printf '%s' "$BODY" | openssl dgst -sha256 -hmac "${{ secrets.TSUBOMI_DEPLOY_KEY }}" -hex | sed 's/^.* //')
          curl -fsS -X POST "${{ vars.TSUBOMI_HOOK_URL }}" \
            -H "content-type: application/json" -H "x-tsubomi-signature: $SIG" -d "$BODY"
```

- **buildx で arm64 + amd64 の manifest list を push**(マルチアーキ — 決定/§10)。
  返る digest は manifest list の digest(§6.6)。
- HMAC は**送る生バイト列そのもの**に対して計算(hook 側も生バイトで検証 — §6.2)。

---

## 6. deploy hook + パイプライン(S3 / S5)

### 6.1 エンドポイント

`POST /api/hook/deploy`(**session 不要、IP 除外**:決定 #4)。body は
`{ service_id, git_sha, image_digest, ts, nonce }`、header `X-Tsubomi-Signature: <hex>`。

axum ハンドラは **`Bytes` で生 body を取る**(serde で受けると再シリアライズで
バイトが変わり HMAC が割れる)。検証後に `serde_json::from_slice`。

### 6.2 検証(全部通って初めて受理)

1. body から service_id だけ先に引く(JSON を一度パース。署名前なので
   「存在する service か」だけ確認、まだ信用しない)。
2. `deploy_key`(復号)で `hmac_sha256(key, raw_body)` を計算、`X-Tsubomi-Signature` と
   **定数時間比較**(`subtle` / 既存の比較ユーティリティ)。
3. `|now - ts| ≤ 300s`。
4. `INSERT INTO deploy_nonces(service_id, nonce)` — UNIQUE 違反(23505)なら
   リプレイとして 409。
5. 全部 OK → `deploys(status='received')` 挿入 → 非同期パイプラインを spawn →
   202 を即返す(GH Action を待たせない)。

不正は 401(署名不一致)/ 400(ts 範囲外・body 不正)/ 409(nonce 重複)。AI/ログが
区別できる code を返す(CLAUDE.md のエラー規約)。

### 6.3 コンテナ作成パラメータ(bollard)

イメージ = `127.0.0.1:5000/<service_id>@<image_digest>`(digest ピン留め)。
`create_container` の主な指定:

- **name**:`tsubomi-<service_id>`(**安定名**。file provider の後端 URL を固定にするため。
  swap は旧停止→新起動なので同名衝突しない。当初案の `-<sha12>` 一意名は廃止)。
- **Env**:§7 で解決した最終 env(静的 + 注入 + `PORT`)。
- **HostConfig**:
  - `NetworkMode = tsubomi-edge`(edge 網のみ。infra 内部網には繋がない — 隔離の一線)
  - `Memory = memory_mb * 1024 * 1024`(硬上限。OOM は単一コンテナだけ殺す)
  - `CpuShares = cpu_shares`(ソフト)
  - `RestartPolicy = unless-stopped`(reconcile の第一の保険 — 第 4 層 §3)
  - `Binds`:volume 注入のマウント(§7.3)
- **Labels(tsubomi 管理用のみ)**:`tsubomi.service_id=<id>`、`tsubomi.git_sha=<sha>`、
  `tsubomi.managed=true`(reconcile / 孤児検出が使う — §8)。**traefik label は付けない**
  (ルーティングは docker provider ではなく file provider — §11-H)。

**ルーティング = file provider**(traefik label ではない)。コンテナ起動成功後、平台が
`services/route.rs` で `<traefik_dynamic_dir>/svc-<id>.yml` を書く:

```yaml
http:
  routers:
    svc-<id>:
      rule: "Host(`<subdomain>.<ドメイン>`)"
      entryPoints: ["web"]                   # 本番は websecure(+ tls)
      service: "svc-<id>"
      middlewares: ["tsubomi-ipallow@file"]  # 会社 IP 許可リスト(ipblock)。registry/hook は付けない
  services:
    svc-<id>:
      loadBalancer:
        servers:
          - url: "http://tsubomi-<id>:<container_port>"   # edge 網の docker DNS で解決
```

- **router/service 名 = `svc-<id>`**(安定)。後端は安定コンテナ名 `tsubomi-<id>` を edge 網の
  docker DNS で解決(provider と名前解決は別レイヤ)。**.yml で書く**(traefik directory file
  provider は .json を無視する — §11-H)。

### 6.4 パイプライン(`deploys.status` を進めながら)

```
received
  → pulling   : 127.0.0.1:5000 から digest pull（bollard create_image）
  → starting  : env 解決（§7）→ 新コンテナ create + edge connect + start
  → （swap、§6.5）
  → succeeded : service_details.image_digest / last_deploy_at / phase=running /
                desired_state=running 更新。deploys.finished_at。
失敗（どの段でも）:
  → failed    : deploys.status=failed + error 文。phase=failed。
                **旧コンテナは触らない**（旧バージョンが走り続ける — 第 4 層 §4b）。
```

パイプラインは tokio タスク(server は既に `tokio = full`)。bollard は async。
失敗は anyhow で集約し `deploys.error` に人間可読で残す。

### 6.5 swap セマンティクス(v1)

第 4 層 §10「ブルーグリーンではない / 秒単位の瞬断は許容」を採る。v1 は:

```
1. 旧コンテナ（tsubomi.service_id ラベルで列挙、今 running の同 service）を stop + remove
2. 新コンテナを create + start
```

= **旧停止 → 新起動**(瞬断 = 新コンテナ起動の数秒)。new-first(健全性ゲート付き
ゼロ瞬断)は health check 基盤が要るので後相(M6 候補)。§11-E で否決可。

> 注:第 4 層 §4b は「新を起動 → 旧を停止」と書くが、それだと瞬断が出ない
> (§10 の「瞬断」と矛盾)。**本書は §10 を正**とし旧→新に統一。§4b の文言は
> 揃えるか、第 4 層に「§4b は M3 で旧→新に確定」と注記する(§11-E)。

### 6.6 アーキテクチャ = 平台が公布する(無条件多 arch をやめる)

build は**平台が公布する arch だけ**を出す(`TSUBOMI_PLATFORMS`、今日は `linux/arm64`
の 1 つ)。host は今 1 台(香橙派 arm64)で amd64 を焼いても誰も pull しないうえ、跨 arch
build は GH ランナー上で QEMU 模拟になり遅い → GH Action 分の浪費。だから既定で host arch
だけを焼く。

- digest 機制は不変:`--platform` が 1 つでも複数でも push は manifest(list)を作り、
  `containerimage.digest`(buildx)/ `imagetools inspect`(nixpacks)で digest を取る。
  平台がこの digest で pull すると docker が host arch のサブマニフェストを選ぶ。
- **将来 x86_64 host を足したら**、平台が公布する arch 集合を `linux/arm64,linux/amd64` に
  変えるだけ。`TSUBOMI_PLATFORMS` を更新 → 次の deploy から自動で多 arch(manifest list が
  両 arch を持ち、各 host が自分のを引く)。**データ駆動、ハードコードしない** ⇒
  「dual-arch from day one」原則の本意(arch に縛られない)は保ちつつ、今のコストは払わない。
- CLI バイナリの多ターゲット配布は別物(本当に多ターゲットが要る)— 影響なし。

### 6.7 `tbm deploy --local`(GitHub 非依存の退路)

CLI がローカルで `docker buildx build --push`(同じ registry)→ digest 取得 →
deploy_key(自分の service を API から取得)で hook を自分で叩く。Action と同じ
body / 署名。CI が無い環境・緊急時の経路(第 4 層 §4b)。

### 6.8 build と run の分離 ─ `run_digest` 単一操作

**build(イメージを作る)と run(イメージを起こす)は別部分**(決定 #3 の核)。平台は
**build しない**(香橙派に nixpacks/buildx を置かない);build は CI か `--local`。平台の
run 半分は 1 つの内部操作に集約する:

```
run_digest(service_id, image_digest, git_sha):
  pull(127.0.0.1:5000 から digest 指定)
  → env 解決(§7)
  → 新コンテナ create + edge connect + start
  → swap(§6.5)
  → service_details.image_digest / phase=running / deploys 更新
```

呼び出し側(全部 run_digest を共有):

| 呼び出し側 | digest の出所 |
|---|---|
| deploy hook(HMAC 検証後) | GH Action が今 build した |
| `tbm deploy --local` | ローカルで build+push した |
| `tbm service start` | service の現 `image_digest`(再 build しない) |
| **`tbm service rollback <deploy-id>`** | `deploys` 履歴から旧 digest を選ぶ |
| reconcile(§8) | service の現 digest |

**rollback はタダで出る**:履歴の各 deploy が digest を持つので、旧 digest を run_digest に
渡すだけ。新規 build も GH Action 分も要らない。

---

## 7. 注入の解決(S6、起動の瞬間 — 決定 #5)

`POST /api/services/:id/injections` は **バインディングだけ**保存(`injections` 表)。
値はコンテナ create の直前に解決する。最終 env =

```
service_env（復号した静的値）
  ∪ injections を 1 件ずつ解決:
      database → DATABASE_URL（既定。env_var で別名可）= 内部入口の app role 文字列（§7.2）
      volume   → 指定 env_var = mount_path、host_path を mount_path に bind（§7.3）
      cache    → REDIS_URL（M5）
  ∪ PORT = container_port（既定 8080。app が $PORT を読む流儀向け。§11-B）
```

### 7.1 失効の意味論

注入先がソフト削除済み(`resources.deleted_at` 非 NULL)→ その 1 件は**空に解決**し、
UI/CLI で「失効」表示。service は普通に起動する(第 4 層 §5、特例ではない)。
復元すれば自動で生き返る(`injections` はソフト削除に触れない — 第 4 層 §2)。

### 7.2 database 注入の内部入口(§11-A の確定)

- 解決するのは **app role**(human ではない)→ 「外部 key の rotate が走る service を
  切らない」が成立(第 4 層 §2)。
- 文字列 = `postgres://<app_role>:<app_pass>@tsubomi-pgbouncer:6432/<pg_dbname>?sslmode=require`
  - host = **docker DNS の `tsubomi-pgbouncer`**(pgbouncer が edge に参加 — §3.4)。
    コンテナは社外に出ず、公開ホスト名のヘアピンも不要。
  - `sslmode=require`:pgbouncer は平文を拒否(M1 の client TLS)。自己署名なので
    CA 検証はしない(`require` は検証しない)。
  - human が手にする外部文字列(`tbm db connect` / `/url`)は従来どおり
    `db.<ドメイン>:6432`(会社 CIDR)。**別 role の別文字列**。ユーザに見えるのは外部
    1 本だけ、内部は平台が注入する不可視の配管(第 4 層 §5)。

### 7.3 volume 注入

- `Binds`:`<host_path>:<mount_path>`(`host_path` = `volume_details.host_path` =
  `/srv/tsubomi/volumes/<user>/<id>`)。`mount_path` 既定 `/data/<volume display_name>`。
- env_var(既定 `STORAGE_PATH`)= `mount_path`。
- `mount_path` の親が無ければ create 前に mkdir(第 4 層 §5)。
- バインドマウントなので safe_path(openat2)は**通らない**:volume の host_path は
  平台が管理する固定パスで、ユーザ入力ではない(トラバーサルの面が無い)。

### 7.4 `tbm env` / `tbm inject` / `tbm eject`

```
tbm env set <svc> KEY=VAL…        # service_env を upsert（value_enc 封緘）
tbm env unset <svc> KEY…
tbm env list <svc>                # 値は伏せる（json は key だけ。秘密は出さない）
tbm inject <resource> --into <svc> [--as ENV] [--mount /path]   # injections 挿入
tbm eject <injection-id>          # injections 削除
```

いずれも **再デプロイ(or start)して初めて効く**(値は起動の瞬間に解決 — 決定 #5)。
CLI/UI はこれを明示(「反映には再デプロイ」)。

---

## 8. reconcile v1(S8)

起動時に 1 回フル、その後 `tokio::time::interval`(既定 30s)で周期ライト。

```
フル / 周期 共通:
  desired = SELECT service_details WHERE desired_state='running' AND resources.deleted_at IS NULL
  actual  = bollard list_containers（label tsubomi.managed=true）

1. 存在の収束:
   - desired にあって actual に無い（or 停止中）→ image_digest があれば起動（パイプライン start 段）
     image_digest が無い（未 deploy）→ 何もしない（created のまま）
   - restart=unless-stopped が第一の保険、これは第二（第 4 層 §3）
2. 孤児の掃除:
   - tsubomi.service_id ラベルを持つが DB に生きた行が無い（or deleted）コンテナ → stop + remove
3. nonce 掃除:DELETE FROM deploy_nonces WHERE seen_at < now() - 1h
4. purge:resources.purge_after <= now() の行を物理削除（trash 実体も。第 4 層 §8）
   ※ M3 では service の purge（コンテナ/イメージ）だけ担保。db/volume の purge は既存。
```

**やらないこと(決定 #5)**:env / 注入のドリフトは追わない。reconcile は「走るべきが
走っている」だけを見る。

---

## 9. API 面 / CLI(第 4 層 §6 の M3 分を確定)

```
service POST   /api/services                  作成（§5.1）
        GET    /api/services                  一覧（resources と join）
        GET    /api/services/:id              詳細（phase / digest / limits / injections / env keys）
        DELETE /api/services/:id              ソフト削除（コンテナ stop+remove → deleted_at）
        POST   /api/services/:id/start|stop   desired_state 変更 + 反映
        GET    /api/services/:id/logs?tail=   docker logs 素通し（bollard、stream）
        GET    /api/services/:id/deploys      デプロイ履歴
        POST   /api/services/:id/rollback     { deploy_id } 旧 digest を run_digest で再起動（§6.8）
        PUT    /api/services/:id/env          静的 env 一括置換
        POST   /api/services/:id/injections   { resource_id, env_var, mount_path? }
        DELETE /api/injections/:id
hook    POST   /api/hook/deploy               HMAC、session なし（§6）
```

CLI 1:1(CLAUDE.md の AI フレンドリ I/O 規約に従う — DTO そのまま serde、エラー封筒、
秘密は stdout 値・警告 stderr):

```
tbm service create|list|status|logs|start|stop|delete|rollback
tbm deploy [--local]
tbm inject <resource> --into <svc> [--as ENV] [--mount /path]
tbm eject <injection>
tbm env set|unset|list <svc>
```

web:`/services` 一覧、`/services/:id` 詳細(phase バッジ・logs ストリーム・env
編集・注入の付け外し・deploy 履歴)。既存の TanStack Query + Zustand + 共用
Button/Dialog を踏襲(frontend 規約)。

---

## 10. 新規依存 / コード配置

- **依存**:`cargo add -p tsubomi-server bollard`(docker.sock の async クライアント)。
  定数時間比較は既存ユーティリティ or `cargo add -p tsubomi-server subtle`。
- **server**:
  - `crates/server/src/services/`(`databases.rs` / `volumes/` に倣う):
    `mod.rs`(CRUD)、`deploy.rs`(hook + パイプライン)、`inject.rs`(注入解決)、
    `docker.rs`(bollard ラッパ:create/start/stop/remove/logs/list)、
    `reconcile.rs`(§8)。
  - `routes.rs` に service/hook ルート追加。`state.rs` に bollard クライアント +
    reconcile ハンドル。
- **CLI**:`crates/cli/src/commands/service.rs`(+ `env.rs` / `inject.rs` or service に同居)。
  `gh` 実行は `std::process::Command`(ローカルの gh を呼ぶだけ)。
- **shared**:service / deploy / injection の DTO(web と CLI で共用、serde 安定)。

---

## 11. 本書が確定した決定(各々**否決可** — 第 4 層 §0 の作法)

| # | 決定 | 理由 | 否決した場合 |
|---|---|---|---|
| **A** | **DB 注入の内部入口 = pgbouncer を `tsubomi-edge` に参加させ、コンテナは `tsubomi-pgbouncer:6432` を docker DNS で引く**(app role + `sslmode=require`) | 第 4 層 §5 の第一候補そのもの。既存 pgbouncer(TLS / auth_query)を再利用、コンテナを infra 網に入れずに済み §1 隔離不変、公開ホスト名のヘアピン不要 | 代替:内部入口専用の 2 個目 pgbouncer を edge に立てる(部品増)/ 注入文字列も外部 `db.<ドメイン>:6432` にする(§1 の DOCKER-USER bridge 許可が前提、ヘアピン依存) |
| **B** | **`container_port`(既定 8080)を service の列に持つ + `PORT` も注入**。traefik はその port へ転送 | 対外は 80/443(traefik)だが容器内 port は別:80 は非 root が bind 不可 + 框架の既定が高 port。`PORT` 注入だけだと port を写し固定した app が 502 → 明示列で確実 | 注入規約だけ($PORT 依存、脆い)/ EXPOSE を読む(検出脆) |
| **C** | **build = Dockerfile 優先・無ければ nixpacks;対象 arch は平台が公布(`TSUBOMI_PLATFORMS`、今 arm64 のみ);GHA 層キャッシュ**。runner 既定 amd64+QEMU(原生 arm は `runs-on` 一行で) | 第 4 層 §4b。無条件多 arch は使わぬ arch を焼き GH Action 分浪費 + QEMU で遅い(host は今 1 台)。キャッシュで再 build は数十秒 | nixpacks 固定 / Dockerfile 必須 / 無条件 arm64+amd64 / 原生 arm runner 既定(私有 repo 有料) |
| **D** | **registry 資格情報は per-user 1 アカウント**(per-service ではない) | digest ピン留めで per-repo ACL 不要(決定 #3) | per-service アカウント(htpasswd 肥大、利点薄い) |
| **E** | **swap = 旧停止 → 新起動(瞬断許容)**。§4b の「新→旧」は §10 に合わせ旧→新へ確定 | v1 は health check 基盤が無い。第 4 層 §10「ブルーグリーンではない」 | health ゲート付き new-first(ゼロ瞬断、基盤要・後相) |
| **F** | **dev のルーティング = traefik + `*.localhost`(:8088, HTTP)**、本番 = :443 + LE + ipAllowList | `*.localhost` は主要ブラウザが 127.0.0.1 に解決、証明書不要 | nip.io / lvh.me(外部 DNS 依存) |
| **G** | **TLS = 既定 LE TLS-ALPN-01(按需・子域ごと)、DNS 厂商非依存**。DNS-01 通配证は配置級のオプション升级 | §1 で :80/:443 公開・ipAllowList は L7 ⇒ LE が公網から子域を検証可。`*.<ドメイン>` A レコード(API 不要)だけで足る | DNS-01 通配を既定(provider API 要)/ 内部 CA(ブラウザ警告) |
| **H** | **traefik は file provider のみ(docker provider 不使用)**。平台が `svc-<id>.yml`(ルート、route.rs)を、ipblock が `ipallow.yml`(middleware)を `traefik_dynamic_dir` に書く。**形式は .yml** | **実測で確定**:Docker Engine 29 が最小 API を 1.40 に上げ、traefik の docker クライアントは 1.24 に落ちて弾かれ provider が全コンテナを見失う(404)。file provider は docker API 不要・docker.sock マウントも不要。traefik directory provider は **.json を無視**(実測)するので YAML 必須 | docker provider(Docker 29 で壊れる)/ socket proxy で API バージョン書換(部品増・未確認) |

---

## 12. 完了判定(第 4 層 §9 の M3 行を満たす)

- [ ] `tbm service create x` → `git push` → **30 秒以内**に `<sub>.<ドメイン>` が開く
- [ ] `tbm inject <db> --into x` + 再デプロイ → コンテナ内 `$DATABASE_URL` が app role の内部文字列で、実際に接続できる
- [ ] `tbm inject <volume> --into x --mount /data` → コンテナ内 `/data` に volume が見える
- [ ] `tbm service stop/start/logs/status` が期待通り
- [ ] ホスト再起動(or `docker rm` で全コンテナ消す)→ reconcile が desired=running を自動復活
- [ ] 孤児コンテナ(DB に行が無い `tsubomi.managed` コンテナ)が reconcile で消える
- [ ] deploy hook:署名不一致 401 / ts 範囲外 400 / nonce 重複 409 / 正常 202
- [ ] `tbm deploy --local` が GitHub 非依存で同じ結果を出す
- [ ] `tbm service rollback <deploy-id>` が旧 digest を再起動(再 build なし)
```
