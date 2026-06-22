# 并发容量与"有效连接打满"风险分析（tsubomi 公网数据库）

- **日期**：2026-06-22
- **范围**：`db.tsubomi-app.com:6432` 这条 frp 隧道 + pgbouncer 的**并发承载能力**
- **与扫描 DoS 的关系**：**不同的风险，不同的解法**。扫描洪流靠"边缘 SNI 闸门"挡；本文讲的是**合法连接本身**把容量占满的风险（含噪声邻居）。

---

## 1. 先纠一个误解：frp 的 poolCount 不是并发上限

`transport.poolCount` 是 frpc **预热的空闲工作连接缓冲**，用来降低建连延迟，**不是"同时最多多少条连接"的硬封顶**。frp 默认对单个 proxy 的并发连接数**不设硬上限**——每多一条并发连接，就多一条 work connection，两端各占一个 socket / 文件描述符（fd）。

所以"有效连接能不能打满"的真正天花板，在 **fd 限制**、**pgbouncer 限额**、**Postgres max_connections** 这几层，而不是 poolCount。

---

## 2. 实测到的各层限额（2026-06-22）

| 层 | 限额 | 说明 |
|---|---|---|
| **frpc 文件描述符（fd）** | **1024**（soft）| 每条隧道连接 ≈ 2 个 fd（到 frps 一个 + 到 pgbouncer 一个）→ **约 500 条并发即耗尽 frpc 的 fd** |
| pgbouncer `max_client_conn` | 1000 | 最多接受 1000 个客户端连接 |
| pgbouncer `default_pool_size` | 20 | 每个 (用户, 库) 到真实 PG 仅 20 条，transaction 模式复用 |
| pgbouncer `pool_mode` | transaction | 每事务结束即归还服务端连接，"以少扛多"效率高 |
| Postgres `max_connections` | ~100（默认）| 被 pgbouncer 池化保护，一般够用 |
| 调查时实际连接数 | 0 | 当时空载，余量充足 |

### 瓶颈排序
**frpc 的 1024 fd（≈500 并发）< pgbouncer 1000 < (池化后的) PG**。

也就是说：**第一个会崩的是 frpc 的 fd**。真有约 500 条有效连接同时挂着，frpc 因 fd 耗尽而无法再建 work connection → 隧道对所有人卡死。症状和"扫描打满"一模一样，但来源是合法流量。

---

## 3. 为什么"合法连接"也会打满

`pool_mode = transaction` 让 pgbouncer 很会复用后端连接，但有个关键点：

> **只要客户端的 TCP 连接还连着（哪怕完全 idle），就实打实占着：1 对 frpc fd + 1 个 pgbouncer client 槽位。**

常见把容量吃满的合法场景：
- **连接泄漏**：app 开了连接不关（缺连接池 / 忘记 release）。
- **idle in transaction**：开了事务挂着不提交，长期占用。
- **每请求开新连接**：高并发下瞬间拉起大量连接。
- **客户端没设上限**：ORM / 连接池配置的 max 过大。

这些都不是攻击，是正常但没管好的用法，足以把 frpc 的 ~500 fd 或 pgbouncer 的 1000 槽位占满。

---

## 4. 架构隐患：噪声邻居 / 共担命运

**所有租户共用同一条 frp 隧道 + 同一个 pgbouncer**，且**当前没有按租户的连接数配额**。后果：

> 任何**单个租户**的 app 连接泄漏、或一口气开几百条连接，就能耗尽共享的 frpc fd / pgbouncer 槽位，**把全部租户一起拖垮**。

这是多租户 PaaS 必须解决的隔离问题——一颗老鼠屎坏一锅汤。

---

## 5. 解决方案

### 立即（低风险、高收益）
1. **调大 frpc 的 fd 上限**：容器层设 `LimitNOFILE`（systemd unit）或 docker `--ulimit nofile=65535:65535`。**1024 对一个公共数据库网关太低，这是最该先做的一项。**
2. **设 idle 回收超时**（pgbouncer）：
   - `server_idle_timeout`（回收空闲的服务端连接）
   - `client_idle_timeout` / `idle_transaction_timeout`（踢掉挂着不干活、idle-in-transaction 的客户端）
   让泄漏 / 闲置连接自动释放，不长期占槽。

### 中期（隔离）
3. **按租户配额**：给每个 tenant 角色设 `max_user_connections`（或 pgbouncer 的 per-user pool 限制），保证单租户打不垮共享层。
4. **app 侧连接池纪律**：每个应用持有少量长连接而非每请求新建；设合理的 pool max。

### 容量规划
5. 明确单条隧道的设计并发上限（受 frpc fd 约束），超过则考虑：多隧道 / 多 frpc 实例分摊，或每租户独立 proxy。

---

## 6. 与"扫描 DoS"的区别（别混淆）

| | 扫描洪流（DoS） | 有效连接打满 |
|---|---|---|
| 来源 | 公网扫描器 | 自己的 app / 用户 |
| 现象 | `work connection pool is full, discarding` | fd / 槽位耗尽，新连接挂起 |
| 主解法 | **边缘 SNI 闸门**（拒在分配资源前）| **抬高 fd 上限 + idle 超时 + 租户配额 + app 连接池** |
| 关系 | 两者都会"打满隧道"，但**互相独立，需分别治理** | |

---

## 7. 关键参数速查（调优时照着改）

- frpc fd：`LimitNOFILE=65535`（systemd）或 `--ulimit nofile=65535`（docker）。当前 **1024**。
- frpc `transport.poolCount`：当前**未设（默认偏小）**，建议显式调大以抗突发。
- pgbouncer：`max_client_conn=1000`、`default_pool_size=20`、`pool_mode=transaction`（现状）；补 `server_idle_timeout` / `client_idle_timeout` / `idle_transaction_timeout`。
- 租户隔离：`ALTER ROLE <tenant> CONNECTION LIMIT n;` 或 pgbouncer per-user 限制。

> 注：本文数字为 2026-06-22 实测（frpc fd=1024、pgbouncer 1000/20/transaction、当时 0 活动连接），调参后请复核。
