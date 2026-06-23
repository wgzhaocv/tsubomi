# 事故调查报告 & 解决方案：公网数据库连接（frp/pg-public）中断

- **日期**：2026-06-22
- **影响服务**：tsubomi PaaS 的数据库公网连接（`db.tsubomi-app.com:6432`）
- **现象**：客户端使用 tbm 提供的公网连接串连不上数据库（连接挂起 / 超时）
- **当前状态**：✅ 已恢复（重启 frpc 后端到端实测通过）；根因为架构脆弱性，需加固防复发

> **加固落地（2026-06-22，方案 A+D）**：已实施——公网口 **6432 → 443**；VPS 边缘自写 **SNI 闸门**
> （`tsubomi-sni-gate`，systemd，TLS 不终端，只放行 SNI=db.tsubomi-app.com，扫描在边缘秒拒不耗 frp 池）；
> frps `proxyBindAddr=127.0.0.1`（proxy 口转 loopback，闸门独占公网 443）；frpc `transport.poolCount=64`
> + 容器 `--ulimit nofile=65535`；frps `LimitNOFILE=65535`；nft 放行 443 / 撤 6432（443 **不**按源 IP
> 限速，避免误伤公司 NAT 共享出口）。连接串生成切 `:443`（`.env.production` TSUBOMI_DB_PUBLIC_PORT=443）。
> VPS 本机经 `openssl -starttls postgres` 全链路实测通过（出真 LE 证书；错 SNI 被闸门断开）。
> 实现见 `crates/sni-gate/`、`deploy/sni-gate/`、`scripts/ship-sni-gate.sh`。
>
> **provider 防火墙（解决）**：调查中发现 VPS 自身可达 443、但外部到 VPS:443 被 ConoHa 网络层封
> （历史只放行 22/6432/7000，与公司出口放行的 80/443/53/8000/8080 无交集）。已在 **ConoHa 安全组把
> 6432 规则改为 443**（22/7000 保留）。改后**外部公网客户端**实测全链路通过：好 SNI → TLSv1.3 +
> 真 LE 证书（`db.tsubomi-app.com`）；错 SNI 被闸门断开；外部 6432 已关。
> 残（用户最终验收）：公司 WSL 跑 `psql …:443…sslmode=verify-full` 确认公司防火墙穿透。

---

## 0. 速览（TL;DR，细节见下文，届时重查）

**两个独立问题：**
1. **隧道挂掉**：公网 6432 被扫描器轰炸 → 占满 frp work-conn 池（`pool is full`）→ 隧道对所有人失效。**重启 frpc 临时恢复**，但会复发。
2. **客户端连不上（公司网络）**：公司出口防火墙**按端口白名单**，只放行 `80/443/53/8000/8080`，封掉 `5432/6432/7000`。换串没用。Neon 能连是因为它走 **443（WebSocket）**，不是 5432。

**修复方向：**
- **改端口**：frp 公网口 6432 → **443**（`frpc.toml` `remotePort=443` + `frps.toml` `allowPorts` 加 443；frps 是 root 能绑 443）。连接串显式写 `:443`（Postgres 默认 5432，sslmode 不改端口）。
- **挡扫描**：调大 frpc `transport.poolCount`（现默认偏小）+ VPS 边缘 **SNI 闸门**（监听 443，SNI=db.tsubomi-app.com 才放行）。
- **凭据**：轮换本次明文外泄的库密码（`tbm db rotate`）。

**架构选择（A/B/C 取舍，详见 §4.5）**：要"任意客户端零 lib + 443" → 留 VPS、frp 走 443；要"砍 VPS 搭 CF" → 得 Neon 式 Postgres-over-WebSocket（客户端需 lib）。

**接入备忘**：`ssh proxy`(VPS,root)、`ssh opi`(香橙派)；frps=systemd `/etc/frp/frps.toml` 日志 `/var/log/frps.log`；frpc=opi docker `tsubomi-frpc`。

---

## 1. 架构概览

```
客户端 (libpq, verify-full)
   │  postgres://db_<id>_<role>:<pwd>@db.tsubomi-app.com:6432/db_<id>
   ▼
[VPS / proxy  133.88.123.119]  ConoHa, Debian 13
   frps  (systemd, :7000 控制, :6432 公开)   ← 裸 TCP 透传, 公网直接暴露
   │  frp tunnel (内部 TLS)
   ▼
[Orange Pi  zwgopi  192.168.0.106 / 公网出口 112.139.95.42]  RK3588
   tsubomi-frpc  (docker, alpine)            ← proxy name: pg-public, 6432→6432
   │
   tsubomi-pgbouncer (docker, :6432)         ← 终止客户端 TLS, 出示 LE 证书; 按 用户名/库名 路由
   │
   tsubomi-pg-tenant (docker, postgres:18)   ← 实际租户库
```

- 客户端 TLS（`sslmode=verify-full`, `sslrootcert=system`）**端到端终止在 Orange Pi 的 pgbouncer**，证书为 Let's Encrypt 签发的 `CN=db.tsubomi-app.com`。
- frp 仅做裸 TCP 透传，VPS 上**除 frps 外无任何代理**（无 nginx/traefik/haproxy，无 docker）。

---

## 2. 时间线（注意时区：frps 日志 JST=UTC+9，frpc 容器 UTC）

| 时间 | 事件 |
|---|---|
| 06-20 01:52 (UTC) | frpc 启动，反复 `dial tcp 133.88.123.119:7000: i/o timeout`（连不上 frps）|
| 06-20 01:55 (UTC) | frpc `login to server success`，`pg-public` 隧道建立（run id `bf28…`）|
| 06-20 06:12 (UTC) | 大量 `StartWorkConn contains error: work connection pool is full, discarding`，随后**日志静默约 2 天** |
| 06-22（调查当天）| 公网连接串不可用；frps 仍在监听 6432，控制连接 ESTABLISHED，但客户端连接挂起 |
| 06-22 06:17 (UTC) | **重启 tsubomi-frpc 容器**，干净重连（新 run id `e129…`，`new proxy pg-public success`）|
| 06-22 调查后段 | 从 Orange Pi 经 `proxy:6432` 走隧道端到端实测：Postgres SSLRequest 返回 `S` → **隧道恢复** |

> 注：frpc 容器重启发生在「先确认、不要动服务」指示**之前**；该重启即为恢复动作。其后所有操作均为只读。

---

## 3. 根因分析

### 直接原因
frpc 的 **work connection pool 被耗尽**（`work connection pool is full, discarding`）。frp 的工作模型是：frps 在公开端口 **每 accept 一个 TCP 连接**，就立刻向 frpc 索取一个 work connection 配对——**发生在任何 TLS / Postgres 数据之前**。一旦 work conn 池满，新连接（包括合法 DB 连接）全部被丢弃，隧道对所有人失效，且未自愈，直到重启 frpc。

### 为什么池子会满：公网扫描洪流
公开端口 6432 一眼可辨为数据库，被互联网自动化扫描持续轰炸。证据：
- VPS 防火墙 `flood6432` 集合中累积了几十个扫描源 IP，**高度集中在少数 /24 网段**：`87.236.176.0/24`、`185.247.137.0/24`、`195.96.139.0/24`，以及 `193.124.20.x` / `193.163.125.x` / `69.5.169.x` 等。即少数几家扫描机构/僵尸网络各用整段 IP 轮扫。
- 租户 Postgres 日志可见明确的**凭据爆破/探测**：
  ```
  FATAL: role "postgres" does not exist          (试默认超级用户)
  FATAL: database "tsubomi_admin" does not exist  (猜管理库名)
  FATAL: permission denied for database "db_jihlrq4s9mwy"  (试他库 id)
  ```
  本库因随机用户名 + 高熵密码 + 强制 `verify-full` 而未被攻破，但扫描连接持续占用 frp 槽位。

### 防御为何没扛住
VPS 上现有的 nftables 防护是**按单 IP 限速**：
```
tcp dport 6432 ct state new add @flood6432 { ip saddr limit rate over 60/minute burst 30 packets } drop
```
它能压制单 IP 高频，但挡不住「**多 IP 小流量**」的聚合洪流；且 frp 的 work conn 池默认值很小（frpc.toml 未设 `transport.poolCount`），聚合后的扫描量仍足以将其打满。

### 结论
这是**可用性事故（DoS 型），非数据泄露/入侵**。本质是「把裸 TCP 端口直接暴露公网 + frp 池子太小」的架构没扛住互联网背景噪音。

---

## 4. 解决方案

### 设计原则
昂贵资源（frp work conn）在 **frps accept 的瞬间**就被消耗，**早于** TLS 终止与 Postgres 认证。因此**准入校验必须前移到 VPS 边缘、frps 之前**，且此时唯一可见的应用层信息是 **TLS ClientHello 里的 SNI**。

> 类比：Neon 用 SNI/endpoint 在边缘代理处路由+准入，非法连接当场丢弃、不消耗后端；AWS RDS IAM 则把签名令牌放在 password 字段做认证。二者解决的是**不同**问题。

### 方案 A（推荐，治本）：VPS 边缘 SNI 准入闸门
在 VPS 上放一个**懂 Postgres SSLRequest 前导**的轻量闸门，把 frps 收到 localhost，公网 6432 由闸门接管：

```
客户端 → [VPS :6432 SNI 闸门] → (localhost) frps → frp tunnel → pgbouncer
                   │
                   └─ SNI ≠ db.tsubomi-app.com  → 立即断开（不进隧道）
```

- 当前所有合法客户端的 SNI 都是固定的 `db.tsubomi-app.com`；扫描器按 IP 扫、ClientHello 不带此 SNI → **绝大多数扫描在边缘被秒拒，不再消耗 frp 池**。
- 用 **TLS passthrough**（只读明文 SNI，不终止 TLS）→ **无需改证书、无需改连接串**，端到端 `verify-full` 不变。
- Postgres 协议是先发 8 字节 SSLRequest、服务器回 `S`、再 TLS 握手带 SNI；闸门需处理这个前导。

**实现选型（VPS 现在是裸的，需新增一个组件）**：
1. **Traefik**（单静态二进制）：TCP router + `HostSNI(\`db.tsubomi-app.com\`)` + Postgres 入口 + `tls.passthrough`。配置量小、可观测性好。
2. **轻量自写代理**（~50 行 Go）：读 SSLRequest→回 `S`→peek ClientHello SNI→校验→splice 到 localhost frps。依赖最少、最可控。
3. （可选）sniproxy/nginx stream：注意原生不处理 Postgres STARTTLS 前导，需确认或打补丁，一般不如上面两个直接。

### 方案 B（可选升级，Neon 级）：把租户标识签名进子域名
将连接串主机名从固定的 `db.tsubomi-app.com` 改为 `<紧凑签名token>.db.tsubomi-app.com`：
- 边缘闸门**验签** SNI，伪造/过期的当场断开 → 连「哪些租户存在」都枚举不出来。
- 需要：`*.db.tsubomi-app.com` 通配证书（Let's Encrypt DNS-01）、改 tbm 连接串生成逻辑、通配 DNS 解析到 VPS。
- 紧凑令牌而非 JWT（域名标签 ≤63 字符）：HMAC 短码 / branca / paseto 之类。

### 方案 C（认证加固，正交，治"拖库"非"扫描"）：password 字段签名令牌
将静态密码换成**时效性签名令牌**（AWS RDS IAM 风格），pgbouncer/PG 侧验签：
- 优点：密码不再长期有效、泄露窗口小。
- 注意：发生在 frp 之后，**不能**缓解池子耗尽；与方案 A 叠加使用。

### 方案 D（缓解，立即可做，不治本）
- 调大 frpc `transport.poolCount`（现用默认值，偏小），提升抗突发能力。
- 收紧 `flood6432` 阈值 / 给集合元素加 `timeout` 自动清理。
- 若客户端 IP 固定，可临时把 6432 改为 IP 白名单准入。

---

## 4.5 客户端连不上 ≠ 隧道故障：公司出口防火墙封了 6432

> 调查中发现的**独立问题**：即便隧道健康、换了新连接串/新密码，从某些网络（如公司网络）连接仍 `timeout`。

### 现象
- 重做连接串后仍 `timeout`。注意：**timeout = 网络层不通；密码错误会被立即拒绝（快速 reject），不会 timeout**——所以 timeout 与字符串/密码无关。

### 实测证据（客户端 = 公司网络的 WSL，公网出口 IP 221.255.179.142）

**第一步：排除 VPS 限速、确认隧道正常**
| 测试 | 结果 | 含义 |
|---|---|---|
| 该 IP 是否在 VPS `flood6432` 黑名单 | ❌ 不在 | 6432 超时**不是**被 VPS 限速 |
| WSL → `proxy:7000`（VPS 无限速、对全网 accept）| ❌ 超时 | 包要出得去就能连上 → 出站被挡 |
| WSL → 你 VPS:22（SSH）| ✅ 通 | 同一目的地、低端口可达 → 非"封整台 VPS"|
| 香橙派 → `proxy:6432` 端到端 | ✅ `recv=b'S'` | 隧道本身正常 |

**第二步（严谨验证）：用与 VPS 无关、在所有端口都监听的第三方 `portquiz.net`，隔离掉"目的地"变量**
| 端口 | 结果 |
|---|---|
| 80 / 443 / 53 / 8080 | ✅ 秒连（放行）|
| **5432** | ❌ 超时（**也被封！**）|
| **6432** | ❌ 超时 |
| **7000** | ❌ 超时 |

排除链：① 目的地是无关第三方 → 排除"VPS 的问题"；② 同一台主机有的端口通、有的不通 → 按**端口**过滤而非按目的地封锁；③ 同在你 VPS 上 22 通、6432 不通 → 再次印证按端口。**结论：是这条网络（公司/IT 管控的出口）的按端口出站白名单所致，与隧道 / 字符串 / VPS 都无关。**

### 根因
公司出口是一份**指定端口白名单**——实测放行 **21 / 22 / 53 / 80 / 443 / 8000 / 8080**（web + 基础服务），封掉**所有数据库口（5432 / 6432 / 3306 …）、邮件口（587 / 465 …）、7000、8443 等**。注意 8000/8080 开但 8443 关 → 不是"放行所有 web 端口"，而是指定了具体端口。（并发扫描会因本地 conntrack 压力产生假阴性，端口判定以单独复测为准。）
- **tsubomi 连不上** → 走 **6432**（被封）。换串无用，端口没变就还是被挡在内网。
- **NeonDB 能用 ≠ 因为 5432**：实测 **5432 在本网络同样被封**。Neon 之所以能连，是因为它走 **443**。
- ⚠️ **修正**：早前"改用 5432 即可"的判断在本网络**不成立**——**正确解是 443**（见下及 §5）。

**实测验证 Neon 走 443（强制 IPv4，目标 = Neon 自己的 IP `13.228.184.177`，从公司 WSL）：**
| 端口 | 结果 |
|---|---|
| Neon:5432 | ❌ 超时（公司连 Neon 的 IP 上 5432 都封 → 按端口封、与目的地无关）|
| Neon:443 | ✅ 75ms 连通 |

→ 用户的 Neon 连接串无显式端口（libpq 默认 5432，在公司会超时）；能过是因为用了 **Neon serverless 驱动**（Postgres-over-WebSocket，走 443）。纯 psql 直连同串会卡 5432。

**对 tsubomi 的启示（比 Neon 更省事）**：Neon 的 443 是 WebSocket，需专用 serverless 驱动；而 tsubomi 的 frp 是**裸 TCP 透传**，把 Postgres 协议直接放 443，**任何标准 libpq 客户端写 `:443` 即可直连**，无需特殊驱动 → 兼容性比 Neon 还好。

### "6432 是不是标准端口"——两种含义别混淆
| "标准"含义 | 例 | 6432 |
|---|---|---|
| 某软件的约定默认端口 | 5432=PG、**6432=PgBouncer**、6379=Redis | ✅ 是（PgBouncer 默认）|
| 企业防火墙默认放行的知名端口 | 80/443/53/8080（本网络实测；**5432 也被封**）| ❌ 不是 |

6432 是 **PgBouncer 的标准口**，不是**防火墙白名单的标准口**；本网络连 5432 都不放行，只有 web 类端口（尤其 **443**）可靠。

### 为什么落在 6432：对外端口照搬了对内端口
`frpc.toml` 把 pgbouncer 的内部端口**原样透传**成公网端口：
```
localPort  = 6432   # pgbouncer 监听口（其默认）
remotePort = 6432   # 公网暴露口 ← 直接照搬了 pgbouncer 默认
```

### 设计教训 + 改法（对外端口 ≠ 对内端口）
**对内** 用 6432（PgBouncer 约定）没问题；**对外** 应该用防火墙放行的端口。本网络实测 **5432 也被封**，所以**对外应用 443**（企业绝不封；VPS 当前 443 空闲）。

改动很小（pgbouncer 不用动）。**因为 VPS 上 frps 以 root 运行（实测确认），可直接 bind 特权端口 443，无需额外的端口转发层**：
```toml
# frpc.toml（香橙派）
localPort  = 6432    # 不动，pgbouncer 还在 6432
remotePort = 443     # 公网口改 443

# frps.toml（VPS）—— 必须改！
allowPorts = [{ single = 443 }]   # 现状只允许 6432；不加 443 会被 frps 拒绝
```
- frps 自己 bind 443 → 流量直接进隧道。**不需要 nft DNAT / socat 转发**（那只有 frps 非 root、不能绑 443 时才需要绕）。
- **连接串必须显式写 `:443`**：`postgresql://user:pass@db.tsubomi-app.com:443/db?sslmode=verify-full&sslrootcert=system`。
  > ⚠️ Postgres 默认端口**永远是 5432**，`sslmode` 只决定要不要 TLS、**不改端口**（"SSL 默认 443"是 HTTP 的规矩，不适用 Postgres）。不写 `:443` 客户端会去敲 5432。
- 5432 在本网络也被封，**不要选 5432**；443 是唯一稳妥选择。
- 公网 443 仍是裸开放端口、仍会被扫（HTTPS 扫描到 pgbouncer 会因协议不符被拒，但仍占 frp 槽位）→ 配合 §4 方案 D（调大 poolCount）+ 方案 A（边缘 SNI 闸门，监听 443，SNI=db.tsubomi-app.com 才放行）。
- VPS 443 当前空闲（Web 走 Pi 上的 cloudflared 隧道，不经 VPS）；若日后 VPS 也要跑 HTTPS，可用 SNI 在 443 上区分 DB vs web。

### 与"砍 VPS 走 CF（Neon 模式）"的三角取舍
三个愿望最多同时满足两个：**A** 客户端零 lib（裸 Postgres）／**B** 免自管 VPS（搭 CF）／**C** 走 443 穿防火墙。
| 组合 | 可行 | 代价 |
|---|---|---|
| **A+C**（零 lib + 443）| ✅ frp 走 443（本方案）| 留 VPS + 需 SNI 闸门挡扫描 |
| **B+C**（免 VPS + 443）| ✅ Postgres-over-WebSocket 走 CF（Neon 模式，复用现有 cloudflared）| 客户端要 lib（`@neondatabase/serverless`）或本地桥；psql/TablePlus 不能直连 |
| A+B | ❌ 裸 5432 直连，Pi 在 NAT 后且过不了防火墙 | 不可行 |
> Neon 之所以"到处能连"，是**两条都给**：5432 裸口（零 lib，普通网络）+ serverless lib（443/WS，受限网络）。实测：纯 psql 从不受限网络连 Neon 5432 成功；从公司连 5432 超时、443 通。

---

## 5. 建议执行顺序

1. **立即**：轮换本次明文外泄的库凭据（`tbm db rotate`）。
2. **可达性**：把公网口从 6432 改到 **443**（见 §4.5；本网络实测 5432 也被封，唯 443 可靠），否则受限网络（公司）永远连不上。改 `frpc.toml` `remotePort=443`，更新连接串为 `:443`。
3. **短期缓解**：调大 frpc `transport.poolCount` + 收紧 nft 限速（方案 D）。
4. **治本**：VPS 边缘 SNI 准入闸门（方案 A，推荐 Traefik 或自写小代理），监听在 443。
5. **加固升级**（按需）：签名子域名（方案 B）+ 令牌认证（方案 C）。

---

## 6. 附录：关键命令 / 凭据线索（调查用）

- 接入：`ssh proxy`（VPS, root, 已装本机公钥）、`ssh opi`（Orange Pi, zwg）。
- frps：systemd 服务 `frps`，配置 `/etc/frp/frps.toml`，日志 `/var/log/frps.log`。
- frpc：Orange Pi 上 docker 容器 `tsubomi-frpc`，配置 `/frp/frpc.toml`（容器内），`docker logs tsubomi-frpc`。
- 防火墙：VPS `nft list ruleset`（`table inet filter`，`flood6432` 集合）。
- 端到端探测：从 Orange Pi `python3 - 133.88.123.119 6432`（发 Postgres SSLRequest，回 `S` 即隧道通）。
- 凭据线索：`docker logs tsubomi-pg-tenant | grep FATAL`（看扫描爆破）。

> 连接串中的库密码本报告**已脱敏**；该凭据已建议轮换。
