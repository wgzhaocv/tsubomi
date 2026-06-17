# tsubomi egress(出站隔離)設計

テナント容器の**出站方向**を仕組みで縛る。背骨「隔離は仕組みで守る、規律に頼らない」の
最後の欠落ピース。per-service 私網は東西向(他テナント/infra 内部網)を既に下げたが、
容器が宿主機や私網へ自由に出ていける穴が残っている(`paas-security-backlog.md` 項 3)。

> **検証は prod Linux のみ**:OrbStack/macOS dev には iptables が無い。dev では egress
> module は完全に no-op(後述 §3.4)。本書の規則・閾値は香橙派(192.168.0.106)での
> recon(2026-06-17)に基づく。

## §0 recon 確定事項(2026-06-17・香橙派)

黙って覆さない前提。変わったら本節を更新してから設計を動かす。

- **Docker 29.2.1 / FirewallBackend = iptables**(nft ではない)。規則は iptables で書く。
- **DOCKER-USER は空**(`-N DOCKER-USER` のみ)。FORWARD policy = **DROP**。
  FORWARD の順序:`ts-forward`(tailscale)→ `DOCKER-USER` → `DOCKER-FORWARD`(Docker 本体)。
- **server は root の host プロセス**(systemd unit 無し)。→ **server 自身が iptables を打てる**
  (sudo/helper 不要)。dev / 非 root では no-op。
- **tsubomi 管制面は全て loopback**:server 9090 / registry 5000 / pg-platform 5434 /
  pg-tenant 5435 / valkey-admin 6433 / traefik 80 は `127.0.0.1` のみ → 容器から到達不可。✓
- **共有機に 0.0.0.0 サービス多数**(容器が網関 IP 経由で全部到達できる):
  sshd:22・rpcbind:111・**裸 postgres:5432**・**redis:6379**・**pgbouncer:6432**・
  各種パネル 3000-3333 / 8080-8097 / 5555 等。**これが現に開いている門**。
- **私網面**:LAN `192.168.0.0/24`(宿主 192.168.0.106)・**tailscale `100.106.83.107`**(tailnet)・
  他 docker 栈(gotify / rag-deploy / infra、172.x 系)。
- **租户桥は docker 自動割当**:現状 `192.168.16.0/20`(172.x が他栈で混み 192.168/16 に溢れた)。
  → 自動割当は LAN に近く、源 CIDR でのマッチングに使えない。**専属 CIDR を明示割当する**(§3.1)。

## §1 脅威 / 非脅威(放行)

**遮断する(脅威)**
1. 容器 → **宿主機の 0.0.0.0 サービス**(sshd / 裸 PG / redis / pgbouncer / パネル)。INPUT 経路。
2. 容器 → **他テナント**容器(横移動)。FORWARD 経路。
3. 容器 → **LAN / tailnet / 他 docker 栈**(スキャン・横移)。FORWARD 経路。

**放行する(非脅威 — 壊してはいけない)**
- **同桥東西向**:app → 同じ私網に attach された infra(pgbouncer / valkey / traefik)。
- **入站 + established**:浏览器 → traefik → app の WS / HTTP 応答(app の websocket 提供は不影響)。
- **出公網は全 TCP**(ユーザ決定 2026-06-17)。app → 外部 API / 外部 PG / SMTP / WS、ポート無制限。
  既定で公網可なので per-service の「公網開放トグル」は作らない。

> websocket の確認:① app が WS サーバ = 入站 + established → egress は触らない。
> ② app が WS クライアント = 出公網 TCP → 全放行。どちらも不影響。

## §2 ポリシー(確定)

> **目標アドレスだけで判定する。ポート/プロトコルでは縛らない。**

- 既定:**宿主機 + 全私網を遮断、公網は全 TCP 放行**。
- 私網の定義:`10.0.0.0/8` + `172.16.0.0/12` + `192.168.0.0/16` + `100.64.0.0/10`(tailscale CGNAT)
  + `169.254.0.0/16`(link-local / クラウドメタデータ)。
- 同桥(= 自テナント subnet)宛は遮断の例外(東西向の infra 到達)。
- DROP を使う(REJECT ではない):スキャンを遅くし、存在情報を返さない。

## §3 機構

### §3.1 租户桥の専属 CIDR(前提工事)

租户トラフィックを源 CIDR で一意に識別するため、各 service 私網に **subnet を明示割当**する。

- **`TENANT_POOL`**(env `TSUBOMI_TENANT_POOL`、既定 `10.231.0.0/16`)。10/8 は本ホストで未使用。
  LAN(192.168)・docker 自動(172.17 / 192.168.16)・tailnet(100.x)と重ならない。
- 各 service 桥は pool から `/24` を取る(`10.231.<n>.0/24`、最大 256 service)。
  `network.rs::ensure_service_network` の `NetworkCreateRequest` に IPAM config(subnet + gateway)を載せる。
- 割当方式:`ensure_service_network` は **inspect で存在確認**し、無い時だけ採番して作る(既存網は
  subnet 据え置き = 冪等。reconcile の毎 tick 既存網パスでは重い list_networks を走らせない)。採番は
  現存する**全 docker 網**の subnet を集め、pool 内で重ならない**最小空き /24** を選ぶ。**採番〜create は
  プロセス内ロック**(`NET_ALLOC_LOCK`)で直列化 — 別 service の同時 deploy が同じ空き /24 を掴む TOCTOU を
  防ぐ(これが無いと 2 つ目の create が subnet 重複で虚假失敗 / 最悪 同一 CIDR 共有)。空き枯渇は **Err**
  (黙って docker 自動割当に倒さない — pool 外 subnet は egress が識別できず不変条件が壊れる)。DB 追加列は
  持たない(subnet は docker network inspect で再導出でき、reconcile が同じものを読む)。
- pool は起動時に `Ipv4Net` へ parse + `/24` 以上を検証(`config.rs`、domain / master_key と同じ fail-fast)。
- **既存桥の移行**:現状 192.168.16/20 の桥は次 deploy / reconcile で新 pool へ作り直す。test env
  かつ service 僅少なので redeploy で吸収(無瞬断の必要なし。落地時に 1 回 `tbm deploy` で十分)。

### §3.2 iptables 規則(Docker 保全の入口 + 自前チェイン)

2 つの自前チェインに中身を閉じ込め、入口だけ固定する(冪等・order 配慮):

- **FORWARD 側** = `TSUBOMI-EGRESS`。入口は **DOCKER-USER から jump**:
  `-A DOCKER-USER -j TSUBOMI-EGRESS`(DOCKER-USER は Docker が再起動でも flush せず、
  `-A FORWARD -j DOCKER-USER` を必ず再付与する = 入口が生き続ける)。
- **INPUT 側** = `TSUBOMI-INGRESS-HOST`。入口は **INPUT 先頭へ insert**:
  `-I INPUT 1 -j TSUBOMI-INGRESS-HOST`(INPUT は docker が再建しないので安定。reconcile が再断言)。
- 入口 jump は「存在しなければ追加」で冪等。チェイン中身は reconcile 毎に **flush → refill**。

`TSUBOMI-EGRESS`(FORWARD = 容器 → 他網):
```
-A TSUBOMI-EGRESS -m conntrack --ctstate ESTABLISHED,RELATED -j RETURN
# 各生存テナント subnet S(infra も S 内にいる)= 同桥東西向を許可
-A TSUBOMI-EGRESS -s 10.231.<n>.0/24 -d 10.231.<n>.0/24 -j RETURN
# … 生存 service の数だけ …
# 私網 + tailnet + link-local を遮断(他テナント = 別 /24 もここで落ちる)
-A TSUBOMI-EGRESS -s 10.231.0.0/16 -d 10.0.0.0/8     -j DROP
-A TSUBOMI-EGRESS -s 10.231.0.0/16 -d 172.16.0.0/12  -j DROP
-A TSUBOMI-EGRESS -s 10.231.0.0/16 -d 192.168.0.0/16 -j DROP
-A TSUBOMI-EGRESS -s 10.231.0.0/16 -d 100.64.0.0/10  -j DROP
-A TSUBOMI-EGRESS -s 10.231.0.0/16 -d 169.254.0.0/16 -j DROP
-A TSUBOMI-EGRESS -j RETURN   # 公網はここを抜けて DOCKER-FORWARD へ → 放行
```

`TSUBOMI-INGRESS-HOST`(INPUT = 容器 → 宿主機の任意 IP):
```
-A TSUBOMI-INGRESS-HOST -m conntrack --ctstate ESTABLISHED,RELATED -j RETURN
-A TSUBOMI-INGRESS-HOST -s 10.231.0.0/16 -j DROP   # sshd / 裸PG / redis / panel… 全宿主サービス
```

> tailscale 注意:`ts-forward` は DOCKER-USER より**前**に走る。tenant → tailnet を ts-forward が
> 先に ACCEPT してしまうと我々の DROP が効かない可能性がある。§4 の probe で実機確認し、効かなければ
> INGRESS と同様に FORWARD 先頭へ jump を挿す(`-I FORWARD 1 -j TSUBOMI-EGRESS`、reconcile 再断言)に
> 切り替える。設計の既定は DOCKER-USER 入口、fallback が FORWARD 先頭。

### §3.3 収束(reconcile)— 新 module `services/egress.rs`

ipblock と同型「期望状態 → 現実へ収束」:

- **起動時**(main、root のとき)+ **network reconcile tick(30s)**で `egress::reconcile` を呼ぶ。
- 毎回 **fresh** に生存 service の subnet を docker network inspect で読む(`valkey::reconcile_acls` /
  `reconcile_networks` と同じ作法 — race 回避)→ 2 チェインを flush → refill。
- **deploy 経路でも同期**:`ensure_service_network` の直後・容器 start の**前**に `egress::reconcile` を
  呼ぶ。新桥の同桥 RETURN が入る前に容器が起きて app→pgbouncer が一瞬 DROP される穴を塞ぐ(背骨「現実は
  容器起動の瞬間に正しく」)。
- **直列化(`EGRESS_LOCK`)**:reconcile tick と deploy の `docker::run` から並行に呼ばれ得る。flush→refill が
  割り込むと「チェイン空の一瞬=遮断が外れる」窓 + jump 二重挿入が起きる。プロセス内 Mutex で直列化する。
  **try_lock で飛ばさず必ず待つ** — deploy 経路は新 subnet を反映して収束させる義務があるため(skip すると
  新桥の同桥 RETURN が入らず app→infra が落ちる)。
- **best-effort + 安全側**:書き込み失敗はログのみ(次 tick / 次 deploy で収束)。ただし FW は
  fail-**closed** が原則 — 入口 jump が立たない限り素通しなので、起動時に入口断言が失敗したら error ログを
  上げる(無言で素通しさせない)。
- **権限ガード**:`cfg!(target_os = "linux")` かつ uid==0 のときだけ iptables を打つ。それ以外
  (dev macOS / 非 root)は no-op + 一度だけ info ログ。

### §3.4 dev との差

OrbStack / macOS には iptables が無い。`egress` module は Linux+root 以外で完全 no-op。
dev e2e は素通しのまま(網隔離の検証は元々 prod 前提 — memory `orbstack-dev-networking`)。

## §4 検証(backlog 要求の「DOCKER-USER 回帰テスト」)

on-box スクリプト `scripts/egress-check.sh`(prod Linux で実行):使い捨て容器を生存租户桥に attach し、

- ✓ 公網到達:`curl -sS -m5 https://example.com` 成功
- ✓ 同桥 infra 到達:`tsubomi-pgbouncer:6432` / `tsubomi-valkey:6379` に TCP connect 成功。
  併せて **`getent hosts tsubomi-pgbouncer` が同 /24 の IP を返す**ことを確認(Docker DNS は網スコープ
  なので同 /24 が返るはず。万一 infra 網の 172.x が返ると `-d 172.16/12 DROP` に巻かれるため要確認)
- ✗ 宿主到達不可:host:22 / host:5432 / host:6379 / host:6432 へ connect タイムアウト
- ✗ tailnet 到達不可:`100.106.83.107` へ connect タイムアウト
- ✗ 横移不可:他テナント subnet の IP へ connect タイムアウト
- ✓ websocket:WS を提供する实测 service に外から接続できる(入站不影響の確認)

実機 e2e は M5 cache のときと同じく「実際に使う service をデプロイして公開 URL で確認」も併走。

## §5 スライス

- **E1(前提工事):租户桥 専属 CIDR 化**(§3.1)。`network.rs` に IPAM 明示割当 + 空き /24 採番。
  既存 service を 1 回 redeploy で新 pool へ移す。**フィルタはまだ入れない**(トラフィック識別の土台のみ)。
  回帰:既存 service が新 subnet で起動 + app→pgbouncer/valkey が従来どおり動く。
- **E2(本体):egress module + iptables 収束**(§3.2 / §3.3)。INPUT(宿主遮断)+ DOCKER-USER
  入口(私網/横移遮断)+ 同桥 RETURN。起動時 + reconcile + deploy 同期。dev no-op。
- **E3(検証 + 微调):** `scripts/egress-check.sh` + 実機回帰(公網/同桥 OK・宿主/tailnet/横移 NG・
  WS OK)。ts-forward の順序問題が出たら §3.2 の fallback(FORWARD 先頭 jump)へ。

各スライスは独立に simplify + codex review → commit → merge → push(working-style どおり)。
