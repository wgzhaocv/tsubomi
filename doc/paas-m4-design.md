# tsubomi PaaS — M4 ガバナンス 実装設計(第 5 層)

> `paas-tech-design.md`(第 4 層)の §7 ガバナンス / §6 admin API 面 / §8 reconcile を、
> **そのまま書き起こせる粒度**まで落とす:migration・admin API 契約・匿名化クエリ・
> 指標採集・Resend メール基盤・磁盘水位告警・危険操作の二段確認・audit 閲覧。
>
> **第 4 層と矛盾させない。**§0 の 6 決定は不変。本書が新たに「確定」するのは
> 第 4 層 §7 が方針だけ示してコードに落ちていない穴(viewer の機構・検証コードの
> 形態・指標の採り方・閾値の去重)だけ。それらは §10 に一覧し、各々**否決可**
> (第 4 層 §0 の作法を踏襲)。
>
> 背骨を一言で:**ガバナンス = 可視性(見える)+ 兜底(処置できる)の 2 枚。
> 「見る」は緩く広く(設計 v2 §7:共有パスワードで誰でも只読)、「動かす」は厳しく
> (owner 身分 + 後端毎回検証 + Resend 検証コード)。前端の表示制御はただの UX。**
>
> 完了判定(第 4 層 §9 の M4 行):**owner が見える・対処できる。**

---

## 0. スコープ

M4 が出すもの:

- **管制面の可視化**:跨ユーザの匿名化された資源一覧(overview / ranking)。
  真名 + 匿名番号(`service1`/`db2`…)+ 監視指標(CPU / 内存 / 存储 / 占用)。
  資源の**名前・内容は見せない**(設計 v2 §7 の誠実な境界)。
- **Resend メール基盤**(`mail.rs`、既存 reqwest を再利用 — 新 crate なし)。
- **磁盘水位告警**:gc の周期で `df` を見て閾値超え → owner にメール(去重付き)。
- **「最後の砦」の危険操作**:owner が他人の service/database/volume を stop / delete。
  後端毎回 owner 検証 + **Resend 検証コードの二段確認**。削除は既存のソフト削除
  (対象ユーザのゴミ箱へ)を再利用 = 復元可。
- **audit_log の補完**:owner 代理操作で `target_user` を埋める + **閲覧 API / 画面**
  (今まで書くだけで読む口が無かった)。
- **共有パスワードの只读 viewer(S5、実装済み)**:ログイン済み社内ユーザが共有パスワードを
  入れると、overview / ranking を**只读**で見られる(session 単位 8h grant)。owner が随時リセット
  (旧 grant は全失効)。**audit は対象外**(真名 + 操作流水の明文 = §7 匿名化の範囲外)。

M4 が**出さない**もの:

- owner 管理 UI(2 人目の owner を web で追加 / 削除)。当面は env 種子
  (`TSUBOMI_OWNER_EMAILS`)で足り、設計 v2 §7 の「互いに削除 + 被削除者へ通知」は
  後相に送る(本書 §10-H で否決可として記録)。
- cache(valkey)の指標 / 操作 = M5(cache そのものが M5)。
- CLI 面。**owner ガバナンスは web 専用**(CLI は AI 駆動のユーザ資源操作専用 —
  プロジェクト規約)。admin ハンドラは session のみ(Bearer cli_token を受けない)。

---

## 1. 着工順序(5 スライス、各々単体で検証可能)

| # | スライス | 範囲 | 依存 | 検証 |
|---|---|---|---|---|
| **S1 可視化** | migration(`platform_config` / `admin_action_codes`)+ `GET /api/admin/overview\|ranking`(跨ユーザ匿名化 + 指標)+ `docker.rs::stats` + web 2 画面 + 侧栏(owner 限定) | — | owner が全ユーザの資源と使用量を見られる;非 owner は 403 |
| **S2 メール基盤 + 磁盘告警** | config(Resend / 閾値)+ `mail.rs`(reqwest)+ gc の `df` 検査 → 閾値超えで owner にメール(去重) | — | 閾値を下げる → 告警メールが 1 通来る(dev は key 無しで log) |
| **S3 最後の砦** | 検証コード二段 + owner 跨ユーザ stop/delete(既存ソフト削除を再利用、`target_user` を audit)+ web 操作ボタン / コード入力 | S2(メール) | owner が他人の service をメール検証コード入力後に停止 / 削除でき、ゴミ箱から復元できる |
| **S4 audit 閲覧** | `GET /api/admin/audit`(分頁 + actor/target 真名 join)+ web 監査ログ画面(owner) | S1–S3(データが溜まる) | owner が S1–S3 の操作履歴を画面で辿れる |
| **S5(実装済み)共有 viewer** | `sessions.viewer_until` + `platform_config['viewer_password']`(bcrypt)+ `POST viewer/login`(204)/ `GET\|POST viewer/password`(owner)+ **overview/ranking** の読み口を「owner OR viewer grant」に緩める(audit は除く)+ web の RequireViewer 解錠フォーム + AdminSettings | S1 | 非 owner が共有パスワードを入れて overview/ranking を只读で見られ、owner が設定 / リセット(旧 grant 失効)できる |

S1 が**地基**:跨ユーザ匿名化クエリ + 指標採集はここで一度作れば S3/S4 が乗る。
S2 → S3 の順は不変(メール基盤を磁盘告警という低リスク場面で先に貫通させてから、
危険操作の検証コードに繋ぐ)。

---

## 2. データモデル(migration:`20260618000001_governance.sql`)

第 4 層 §2 の `platform_config`(M0 の DDL に載っていたが migration には未投入)を
今ここで作る。加えて検証コードの単回消費テーブルを足す。`audit_log` は既存
(M1 の `20260613000002_resources.sql`)で、本書は **`target_user` を活用** +
閲覧用の index を足すだけ。

```sql
-- ============ 平台設定(key→jsonb)============
-- 第 4 層 §2 の定義そのまま。M4 では:
--   'disk_alert_state' = { level: 'ok'|'warn'|'critical', notified_at: <ts> }  -- 告警去重(§4)
--   'viewer_password'  = { hash: '<bcrypt>', updated_by: '<uuid>' }            -- S5 実装済み(更新時刻は updated_at 列)
-- ※ viewer grant 自体は sessions.viewer_until 列(migration 20260621000001)。
create table platform_config (
  key        text primary key,
  value      jsonb not null,
  updated_at timestamptz not null default now()
);

-- ============ 危険操作の検証コード(単回消費。authcodes と同じ Postgres 流儀)============
-- owner が他人の資源を stop/delete する二段確認の 1 段目で 1 行 INSERT、
-- 2 段目で DELETE..RETURNING(単回消費)。期限切れは gc が掃除。
-- code は平文を保存せず sha256(他の token と同じ規律)。
create table admin_action_codes (
  code_hash   text        primary key,             -- sha256(6 桁コード)hex
  actor_id    uuid        not null references users(id) on delete cascade,
  resource_id uuid        not null,                 -- 対象資源(resources.id)。soft 削除済みでも参照したいので FK なし
  action      text        not null check (action in ('stop','delete')),
  expires_at  timestamptz not null,                 -- now() + 10min
  created_at  timestamptz not null default now()
);
create index on admin_action_codes (expires_at);

-- ============ audit_log の閲覧用 index(表自体は M1 既存)============
-- 既存:created_at DESC / target_resource。閲覧フィルタ用に 2 本足す。
create index on audit_log (actor_id);
create index on audit_log (target_user);
```

確定する細部:

- **`admin_action_codes` のキー = `code_hash`(PK)**:同じ owner が同じ資源に対し連続で
  コードを請求したら、古い行は期限で自然消滅(gc)/ 衝突しても新しい hash で別行。検証は
  `DELETE FROM admin_action_codes WHERE code_hash=$1 AND actor_id=$2 AND resource_id=$3
  AND action=$4 AND expires_at > now() RETURNING 1`(単回消費 + 期限 + 文脈一致を 1 文で)。
- **`target_user`**:既存 `audit()` は actor/action/target_resource/detail しか埋めない。
  owner 代理操作では「誰の資源を触ったか」が要るので、§5 で `audit_with_target()`
  (target_user も埋める版)を足す。既存呼び出しは触らない(後方互換)。
- **viewer 密码は bcrypt**(registry htpasswd と同じ。`bcrypt 0.19` 既存)。session/token
  系の sha256 とは別 — これは「人が入力する低エントロピー秘密」なので一方向の遅い hash。
  → S5 実装済み(§7 / §10-A)。bcrypt は数百 ms の同期 CPU なので `spawn_blocking` に逃がす。

---

## 3. 可視化:overview / ranking(S1)

### 3.1 匿名化の規律(設計 v2 §7)

owner(/ S5 の viewer)が見るのは:

- **ユーザの真名**(`users.name`、null なら `email`)。誰が使っているかは見える。
- **匿名番号** `<kind><anon_seq>`(`service1` / `database2` / `volume1`)。資源の
  `display_name` は**見せない**(資源の意味は本人にしか分からない)。
- **監視指標**(下表)。資源の**内容**(DB の中身 / ファイル / env 明文)は**見せない**。
  誠実な境界:pg admin 凭证を持つ者は技術的には読める = これは UI 級の保証(§7 の注記)。

| 種別 | 指標 | 採り方 |
|---|---|---|
| service | CPU% + 内存(bytes) | bollard 一発 stats(§3.3)。停止中は 0 / null |
| database | 存储(bytes) | `pg_database_size(pg_dbname)`(tenant_admin_url 経由) |
| volume | 占用(bytes) | host_path のディレクトリ走査 or `du -sb`(best-effort、§3.4) |
| cache | 已用内存 | **M5**(cache 自体が M5)。本書は出さない |

### 3.2 クエリ(跨ユーザ、`list_resources` の owner 版)

```sql
SELECT r.id, r.user_id, u.name, u.email, r.kind, r.anon_seq, r.deleted_at,
       d.pg_dbname,                 -- database のみ(指標採集用、UI には出さない)
       v.host_path                  -- volume のみ(同上)
  FROM resources r
  JOIN users u ON u.id = r.user_id
  LEFT JOIN database_details d ON d.resource_id = r.id
  LEFT JOIN volume_details   v ON v.resource_id = r.id
 WHERE r.deleted_at IS NULL
 ORDER BY u.name NULLS LAST, r.kind, r.anon_seq;
```

- `pg_dbname` / `host_path` は**指標採集にだけ使い、DTO には載せない**(wire 名 = 内容に
  近いので匿名化の趣旨に反する)。DTO は `{ owner_name, kind, anon_no, metric }` だけ。
- **overview** = この結果を種別ごとに集計(総数 + 総使用量)。
- **ranking** = 指標で降順ソートして上位 N(`?kind=&limit=`)。

### 3.3 `docker.rs::stats`(新規、bollard 一発)

```
pub async fn stats(state, service_id) -> Option<ContainerStat>
  // bollard stats(stream=false)を 1 サンプル。CPU% は cpu_delta/system_delta*ncpu、
  // 内存は memory_stats.usage。コンテナ不在 / 停止中は None(UI は「-」表示)。
```

reconcile が引く現行コンテナ名の解決は既存 `docker.rs` の流儀を踏襲(`tsubomi.service_id`
ラベル or 現行名)。**best-effort**:stats が取れなくても overview は出す(指標欄が空)。

### 3.4 指標採集のコスト(積み残し・否決可 §10-C)

- service stats / pg_database_size は速い(ms 級)。**volume の `du` は大きいディレクトリで
  遅い**。v1 は**オンデマンド + 軟タイムアウト**(例:各 volume 200ms で諦め「計測中」)。
- 後相:gc の周期で計測して `platform_config` か新列にキャッシュ(m3 の N+1 注記と同じ
  「単機・少数では無視可、増えたらキャッシュ」)。本書はオンデマンドで確定、キャッシュは deferred。

### 3.5 web

`/admin`(overview)+ `/admin/ranking`。owner 限定で侧栏に出す
(`dashboard-layout.tsx` の `me?.role === "owner"` ブロックに追加、IpAllowlist と同居)。
画面側でも `me.role !== "owner"` を弾く(IpAllowlist.tsx と同じ — UX だけ、後端が 403 で守る)。
TanStack Query + 共用 Card/Title(frontend 規約)。

### 3.6 ホスト(サーバ本体)指標の WS 配信(リソース概要に追加)

上の overview は**各ユーザ資源**の集計。これとは別に、**宿主機(香橙派 = ARM64)本体の
CPU/メモリ/ディスク使用量**を「リソース概要」上部の「サーバー」カードに出す。ユーザ制約:
**①性能影響なし ②低頻度 ③誰も見ていない時は起動しない ④WebSocket**。

- **共有サンプラ(`crates/server/src/metrics.rs`)**:`AppState` に
  `metrics_tx: broadcast::Sender<HostMetrics>` + `metrics_running: Arc<tokio::sync::Mutex<bool>>`。
  WS 接続(`handle_socket`)で **先に `subscribe()`(受信者+1)→ ロック → `!running` なら
  `running=true` にして採样 task を spawn**。サンプラは 5s 毎に採取(ロック外)→ ロック →
  `send()` が Err(受信者ゼロ)なら `running=false` で停止。**「subscribe+起動判定」と
  「send+停止判定」を同ロックで直列化**して無人放置 / 二重起動を排除(= 閲覧者ゼロなら
  採样は走らない、要件③)。初期受信者は `channel(8)` 直後に drop(閲覧者ゼロ=受信者ゼロ)。
- **採取(新 crate なし、§10-D)**:CPU=`/proc/stat` の cpu 行差分(idle=idle+iowait)、
  メモリ=`/proc/meminfo`(MemTotal−MemAvailable)、ディスク=`df -Pk`(`metrics::disk_metrics`、
  gc の磁盘水位告警と**共有**)。dev(macOS)は /proc 無しで CPU/メモリ None(UI「—」)、
  prod(Linux コンテナ、host network)は全て実値。初回スナップショットは CPU だけ None
  (前回サンプル無し)、次 tick から実値。
- **鉉权**:`GET /api/admin/metrics`(admin routes 配下 = `require_auth` の内側 → WS 升级も
  AuthCtx を持つ)。handler 冒頭で `require_viewer_web`(owner または共有パスワード viewer・
  **web session のみ**、Bearer 拒否)。指標は platform レベルで非機密(資源の内容ではない)。
- **前端**:`web/src/lib/host-metrics.ts` の `useHostMetrics()`(ページ表示中だけ WS 接続・
  unmount で close = 要件③の前端側)+ `AdminOverview.tsx` の「サーバー」カード(用量バーは
  `VolumeFileBrowser` の意匠を踏襲、`formatBytes` 再利用)。dev proxy は `vite.config.ts` の
  `/api` に `ws:true`。**端到端済み**:dev(macOS、disk 実値・cpu/mem—)+ prod(Pi、CPU/メモリ/
  ディスク全部実値、wss が Cloudflare Tunnel を通過)で確認。
- **プラットフォーム自身(各コンテナ別)**:同じ snapshot に `platform: Vec<ContainerStat>` を
  載せ、最下部の「プラットフォーム自身」カードで **server + infra の各コンテナ**(用户 app は
  除外)の CPU/メモリを**加総せず一覧**(どの基礎設施が重いか分かる)。採取 = `docker::platform_stats`
  (running から「`tsubomi-` 名前 かつ `tsubomi.managed` ラベル無し」を絞り、`join_all` で並行に
  `docker stats(stream=false)` を 1 サンプル。`stats` と `sample_stats` を共有)。**性能**:閲覧中
  のみ 5s 毎に ~6-7 コンテナ並行 stats(各 ~1s)。負荷一定化のため interval は
  `MissedTickBehavior::Skip`(バースト追い上げ無し)+ **採取前に `receiver_count()==0` を判定して
  停止**(最後の閲覧者が切れたら docker stats を 1 バッチも余計に走らせない)。dev は server が
  容器でないので並ばない(infra のみ)。codex 性能レビュー済み(無人で完全停止・FD リーク無し)。
- **CPU 初回表示の暖機**:CPU% は 2 サンプルの差分なので、素直に作ると初回フレームは prev 無し=
  None で「CPU だけ 5s 遅れて出る」(mem/disk は瞬時値で即出る)。採样ループ前に 1 度 /proc/stat を
  読み `CPU_WARMUP`(~1s)置くことで初回フレームから CPU% を出す(prod 実測:初回 ~3s で CPU 値あり)。

**使用量ランキング(§3 の補足)**:overview/ranking は WS ではなく HTTP(TanStack Query)。
ランキングは採集が重い(資源ごとに docker stats/`pg_database_size`/du)+ 使用量は緩やかに変わるため、
**60s の定期 refetch**(`refetchInterval`、WS ではない)で「ゆっくり活きる」程度に留める。「使用量」は
種別で意味が違う(service=メモリ / database=ストレージ / volume=ディスク / cache=キー数)ので、
1 列に混ぜて降順にする表では**行ごとに指標名を併記**して誤読を防ぐ(`AdminRanking.tsx` の `USAGE_METRIC`)。

---

## 4. メール基盤(Resend)+ 磁盘水位告警(S2)

### 4.1 `mail.rs`(Resend HTTP、新 crate なし)

```
config(env):
  RESEND_API_KEY      省略可。未設定 = 送らず log のみ(dev / Resend 未契約時の退路)
  TSUBOMI_MAIL_FROM   送信元(例:"tsubomi <noreply@tsubomi-app.com>")。RESEND_API_KEY が
                      在れば必須

mail::send(state, to: &[String], subject, body_text) -> anyhow::Result<()>
  key 無し → tracing::info!("[mail:dropped] to={to} subject={subject}") して Ok(())
  key 有り → POST https://api.resend.com/emails
            Authorization: Bearer <key>
            { from, to, subject, text }
            非 2xx は Err(本文を log)
```

- 既存 `reqwest`(rustls-tls + json)をそのまま使う。`state` に reqwest クライアントが
  あれば再利用、無ければ `mail.rs` で 1 個持つ。
- 宛先 = **owner**。真実源は `config.owner_emails`(設計 v2 §7 の env 種子 = owner の定義)。

### 4.2 磁盘水位告警(gc の housekeeping tick に同居)

第 4 層 §3 reconcile §4 の「ディスク水位チェック(閾値超え → Resend で owner 通知)」を
**gc.rs の housekeeping(1h tick)**に置く(reconcile はコンテナ/route 収束に純化済み —
m3 §8。磁盘検査は DB ハウスキーピング側、nonce 掃除と同居の判断と一致)。

```
config(env、既定):
  TSUBOMI_DISK_WARN_PCT      = 80   使用率がこれを超えたら warn
  TSUBOMI_DISK_CRITICAL_PCT  = 90   critical
  (監視対象 = volumes_dir / backup_dir を含む filesystem。実装は volumes_dir の在る fs)

検査(1h ごと):
  used% = df(対象パス)            # `df -P -k <path>` を shell 実行して解析(sysinfo crate 不要、
                                   #   rsync/pg_dump と同じく外部コマンド)
  level = used% >= critical ? 'critical' : used% >= warn ? 'warn' : 'ok'
  prev  = platform_config['disk_alert_state']   (level + notified_at)
  通知条件(去重):
    - level が上がった(ok→warn, warn→critical)                    → 送る
    - 同 level に留まっていても notified_at が 24h 超古い(再喚起)  → 送る
    - level が下がった / ok                                         → 送らない(状態だけ更新)
  送ったら platform_config['disk_alert_state'] を更新 + audit('disk.alert', detail=用量)
```

- **去重が肝**:1h tick で閾値超えのたびに送ると owner の受信箱が溢れる。`platform_config` に
  最後の level + 時刻を持ち、「level 上昇」か「24h 経過」でしか送らない。
- `df` は best-effort:解析失敗は log だけ(告警は安全側に倒し、止めない)。

---

## 5. 最後の砦:危険操作 + Resend 検証コード(S3)

### 5.1 二段確認のフロー(設計 v2 §7「owner が他人の資源を動かす → Gmail 検証コード」)

owner 専用、跨ユーザ。**2 段**:

```
1 段目(コード請求):POST /api/admin/resources/:id/{stop|delete}   body 無し / {code:null}
  - require_owner
  - 資源を引く(deleted_at IS NULL、kind で操作の妥当性確認)
  - 6 桁コード生成 → sha256 を admin_action_codes に INSERT(actor=owner, resource, action,
    expires=now()+10min)
  - mail::send(owner 自身へ)「<対象ユーザ> の <kind><anon_no> を <stop|delete> するコード: NNNNNN」
  - 200 { "code_required": true }     ← AI/UI は「コードを入れて再送」と分かる

2 段目(確定):POST /api/admin/resources/:id/{stop|delete}   { code: "NNNNNN" }
  - require_owner
  - DELETE FROM admin_action_codes WHERE code_hash=sha256(code) AND actor_id=owner
    AND resource_id=:id AND action=$ AND expires_at>now() RETURNING 1   (単回消費)
    無 → 400(検証エラー。コードが無効/期限切れは「認証(401)」ではなく不正な入力)。
         加えて総当たり防止に同一 (actor,resource,action) の未使用コードを焼く(再請求=再メールが必要)。
  - 実行(§5.2)→ audit_with_target(actor=owner, action='owner.stop_service' 等,
    target_resource=:id, target_user=対象ユーザ, detail)
  - 200 結果 DTO
```

- コードは owner**自身**のメールに届く(他人の資源を触る owner の本人確認)。設計 v2 §7 の
  「Gmail 検証コード」をそのまま。`RESEND_API_KEY` 未設定の dev では log に出るのでそれを使う。
- **owner が自分の資源を消す**のはこの口ではない(設計 v2 §7:自分のは名前入力確認、メール
  無し = 既存のユーザ削除フロー)。admin の口は跨ユーザ専用 = 常にコードを要求する。

### 5.2 実行 = 既存のソフト削除 / 停止を再利用(owner 認可版)

第 4 層 §11 / v2 §11 の意味論を壊さない:**owner の delete も普通のソフト削除**
(対象ユーザのゴミ箱へ、3 日猶予、復元可)。stop も普通の `desired_state=stopped`。
違いは「所有者チェックを owner 権限で代替」する点だけ。

| 種別 | stop | delete |
|---|---|---|
| service | `services::stop` 相当(desired_state=stopped + コンテナ停止) | `services::delete` 相当(コンテナ stop+remove → deleted_at、ゴミ箱) |
| database | — (DB に「停止」は無意味。本書は出さない、§10-G) | `databases::delete` 相当(pg_dump → trash → DROP) |
| volume | — | `volumes::delete` 相当(trash へ mv) |

- 既存ハンドラは `WHERE user_id = $auth` で所有者を縛っている。owner 代理用に**内部関数を
  切り出し**(所有者 user_id を引数で受ける)、ユーザ口は `auth.user_id`、admin 口は
  資源の `user_id` を渡す。重複ロジックを増やさない(リファクタ範囲は各モジュール内)。
- delete は対象ユーザの**ゴミ箱**に入る(その人が復元できる)。owner が完全削除するなら
  既存のゴミ箱 purge を別途(本書では追わない、ゴミ箱は本人/期限)。

### 5.3 web

overview / ranking の各行に owner 操作(stop / delete)。押下 → 1 段目 POST →
「owner のメールに届いたコードを入力」モーダル → 2 段目 POST。IpAllowlist の削除確認
モーダルの流儀 + コード入力欄。**前端は UX、後端が毎回 owner + コードを検証**。

---

## 6. audit_log 閲覧(S4)

「監査 = ガバナンス可視性のもう半分」(第 4 層 §7)。今まで書く一方だったので読む口を足す。

```
GET /api/admin/audit?cursor=<id>&limit=50&action=&actor=&target_user=
  - require_owner
  - SELECT a.*, actor.name/email, tu.name/email
      FROM audit_log a
      LEFT JOIN users actor ON actor.id = a.actor_id
      LEFT JOIN users tu    ON tu.id    = a.target_user
     WHERE (a.id < cursor)  -- キーセット分頁(id DESC)
       [AND a.action=… AND a.actor_id=… AND a.target_user=…]
     ORDER BY a.id DESC LIMIT limit+1
  - DTO: { id, created_at, action, actor_name, target_user_name, target_resource, detail }
```

- **キーセット分頁**(`id < cursor`、id は BIGINT IDENTITY = 単調)。OFFSET は使わない。
- `target_resource` は UUID をそのまま(資源名は出さない = 匿名化の趣旨)。`detail` の jsonb は
  そのまま出す(既存の audit は cidr / kind 等の非機密のみ。秘密は元から入れていない)。
- web:`/admin/audit`(owner)。action / actor / target でフィルタ + 「もっと読む」。

---

## 7. 共有パスワード viewer(S5、実装済み)

設計 v2 §7:**「看」靠共享密码** — ログイン済み社内ユーザが共有パスワードを入れると
overview / ranking を**只读**で見られる(owner でなくても)。owner が随時リセット。
S5 は §10-A で「否決可・後相」に置いていたが、本スライスで実装した(as-built は以下)。

```
migration 20260621000001:sessions に viewer_until TIMESTAMPTZ を足す(grant の絶対期限)。
共有パスワードは platform_config['viewer_password'] = { hash(bcrypt), updated_by }
  (更新時刻は platform_config.updated_at 列を流用 = 二重持ちしない)。

is_viewer は session::get が同じ行で算出する:(viewer_until IS NOT NULL AND viewer_until > now())。
AuthCtx.is_viewer に載り(Bearer 経路は常に false)、require_viewer_web =
  is_session() && (is_owner() || is_viewer)。

POST /api/admin/viewer/login    { password }                         [session]
  - is_session 必須(Bearer は 403 — viewer は web 専用)。
  - 未設定 → 400(「owner に設定を依頼」)。bcrypt::verify は spawn_blocking に逃がす。
  - OK → session::grant_viewer(viewer_until = now()+8h)→ 204。NG → 400。

POST /api/admin/viewer/password { password }                         [owner]
  - require_owner_web。trim 後 8 文字以上(MIN_VIEWER_PASSWORD_LEN。bcrypt と併せ総当たり下限)。
  - bcrypt::hash(spawn_blocking)→ upsert + 旧 grant 全失効を 1 トランザクションで
    (UPDATE sessions SET viewer_until = NULL WHERE viewer_until IS NOT NULL = §7「リセット=旧失効」)。
  - 設定後の状態(ViewerStatusResp)を返す。
GET  /api/admin/viewer/password                                      [owner]
  - 設定済みか + updated_at + 設定者の真名(本体 / hash は返さない)。設定ページ表示用。
```

**緩める読み口は overview / ranking のみ。audit は owner-only のまま**(audit は actor/対象の
**真名** + detail の明文を持ち、§7 の匿名化只读の範囲を超えるため。設計 v2 §7 の viewer 範囲も
overview/ranking で、audit は M4 で後付けした S4)。危険操作(§5)/ viewer 設定 / ipblock も owner のみ。

- **viewer は web/session のみ**(CLI 無し)。owner ガバナンス web 専用の規約に従う。
- **積み残し(後相)**:login の**失敗レート制限**。今は bcrypt cost 12(≈数百 ms / 試行)+
  最小長 8 でオンライン総当たりを不経済にする一次防御のみ。本格的な失敗計数 / lockout は別スライス。

---

## 8. API 面 / web ルート / CLI

```
admin(web/session のみ。Bearer cli_token は受けない = owner ガバナンス web 専用)
  GET  /api/admin/overview                  種別ごと総数 + 総使用量(匿名)        [owner]
  GET  /api/admin/ranking?kind=&limit=      指標降順の匿名ランキング              [owner]
  GET  /api/admin/audit?cursor=&limit=&…    監査ログ閲覧(actor/target 真名 join) [owner]
  POST /api/admin/resources/:id/stop        二段(コード請求 → 確定)             [owner]
  POST /api/admin/resources/:id/delete      二段(同上、ソフト削除→対象ゴミ箱)   [owner]
  -- S5(実装済み)
  POST /api/admin/viewer/login              共有パスワード → viewer grant(204)   [session]
  GET  /api/admin/viewer/password           設定状態 + メタ(設定ページ用)        [owner]
  POST /api/admin/viewer/password           共有パスワードの設定/リセット         [owner]
```

> overview / ranking は **[owner OR viewer]**(`require_viewer_web`)に緩めた。audit / 危険操作 /
> viewer パスワード設定は **[owner]** のまま(`require_owner_web`)。

- ルートは `crates/server/src/admin/`(新 module)に集約、`routes.rs` で `/api/admin` 配下に
  マウント。`require_owner` は ipblock.rs のものを `auth` か admin 共通へ移して共有。
- **CLI:無し**。CLAUDE.md / メモリ `owner-features-web-only` の通り、admin 系はコマンド面を
  足さない。

web ルート(`router.tsx`、DashboardLayout 配下、owner 限定で侧栏):
```
/admin            → AdminOverview   ┐ <RequireViewer>(未解錠なら解錠フォームを内蔵描画。
/admin/ranking    → AdminRanking    ┘  owner || is_viewer で <Outlet>。危険ボタンは owner のみ)
/admin/audit      → AdminAudit      ┐ <RequireOwner>(audit は匿名化外なので viewer 不可)
/admin/settings   → AdminSettings   ┘  共有パスワードの設定 / リセット(owner)
```
- 侧栏:管制面 / 使用量ランキングは全ログインユーザに出す(未解錠は解錠フォームへ)。
  監査ログ / 共有パスワード設定 / IP 許可リストは owner 限定。
- viewer の解錠は専用ページではなく `RequireViewer` がその場でフォームを出す(login 成功 →
  `me` 無効化 → `is_viewer` 翻転 → `<Outlet>` に切替)。

---

## 9. 新規依存 / コード配置

- **依存**:無し(reqwest / bcrypt / sha2 / rand すべて既存)。`df` は外部コマンド shell。
- **server**:
  - `crates/server/src/admin/`(`mod.rs` ルート + `require_owner_web` / `require_viewer_web`、
    `overview.rs` 可視化 + 指標、`actions.rs` 危険操作 + 検証コード、`audit_view.rs` 閲覧、
    `viewer.rs` 共有パスワード S5)。`auth/session.rs` に `grant_viewer`、`auth.rs` の AuthCtx に `is_viewer`。
  - `crates/server/src/mail.rs`(Resend、§4.1)。
  - `crates/server/src/services/docker.rs` に `stats`(§3.3)を追加。
  - `crates/server/src/gc.rs` の housekeeping に磁盘検査(§4.2)を追加。
  - `crates/server/src/databases.rs` の `audit` の隣に `audit_with_target`(§2)。
  - `crates/server/src/config.rs` に Resend / 閾値の env。
  - 各リソースモジュール(services/databases/volumes)に owner 代理用の所有者引数版を切り出し。
- **shared**:admin の DTO(AdminOverviewDto / AdminRankingRowDto / AuditEntryDto /
  AdminActionResp など。serde 安定、匿名フィールドのみ)。
- **web**:`routes/Admin*.tsx`(S5 で `AdminSettings.tsx` 追加)、`components/require-viewer.tsx`
  (解錠フォーム)、`lib/admin.ts`(TanStack Query フック)、`lib/auth.ts`(Me.is_viewer)、侧栏 + router。

---

## 10. 本書が確定した決定(各々**否決可** — 第 4 層 §0 の作法)

| # | 決定 | 理由 | 否決した場合 |
|---|---|---|---|
| **A**(後に実装) | **共有 viewer(S5)は当初否決可・最後**としていたが、設計 v2 §7「看/操作の二層分離」の「看」が欠けるため **後で実装した**(§7 as-built)。`sessions.viewer_until` 追加・overview/ranking のみ緩める・audit は owner のまま・リセットで旧 grant 全失効 | §7 の「誰でも只读」が平台の核心価値(可視性)の半分。owner-gated だけだと「全社で围观」が成立しない | (実装済み)残りは login の失敗レート制限のみ後相 |
| **B** | **危険操作 = 2 段(コード請求 → 確定)、同一エンドポイントに `code` 有無で分岐**。コードは owner 自身へメール、`admin_action_codes` で単回消費 | 状態を持たない 1 往復で済み、AI/UI も `code_required` で次の一手が分かる(CLAUDE.md のエラー規約) | コードを別エンドポイントで請求(口が増える)/ TOTP(共有秘密の配布が要る) |
| **C** | **指標採集はオンデマンド + 軟タイムアウト**(volume の du が遅いので諦める)。キャッシュは後相 | 単機・少数では十分。m3 の N+1 注記と同じ判断(増えたらキャッシュ) | gc 周期で計測してキャッシュ(列 or platform_config)を先に作る |
| **D** | **磁盘空闲 = `df` を shell**(sysinfo 等の crate を足さない) | rsync/pg_dump と同じ「外部コマンドを叩く」流儀。依存ゼロ | `sysinfo`/`fs2` crate(移植性は上がるが依存増) |
| **E** | **owner の delete も普通のソフト削除(対象ユーザのゴミ箱へ、復元可)** | v2 §11 の意味論を壊さない。owner は「処置」するが「抹消」はしない(ゴミ箱は本人/期限) | owner は即時完全削除も可(危険、誤操作が不可逆) |
| **F** | **admin は web/session 専用、CLI 無し** | プロジェクト規約(owner ガバナンス web 専用、CLI は AI 駆動のユーザ資源操作) | admin にも CLI を足す(規約違反、AI に owner 権を握らせない方針に反する) |
| **G** | **database / volume の「停止」は出さない**(delete のみ。service だけ stop/delete) | DB / volume に「停止」は無意味(コンテナではない)。設計 v2 §7 の表の「停止」は実質 service 向け | database 接続を一時遮断する「凍結」を別途定義(M4 範囲外) |
| **H** | **owner 管理 UI(2 人目追加/削除)は M4 範囲外**。env 種子のまま | 単機・owner 2 名固定で足りる。web 管理は複雑さに見合わない | 設計 v2 §7 の「互いに削除 + 被削除者へメール通知」を web に実装 |
| **I** | **真名 = `users.name`(null なら `email`)** | hd 制限 Google は email 必須・name は任意。両方で確実に「誰か」を出す | email 固定(name があっても出さない)/ name 必須化 |

---

## 11. 完了判定(第 4 層 §9 の M4 行「owner が見える・対処できる」)

> **完了(S1–S5、dev e2e 済み)**。S5(共有パスワード viewer)も §10-A の否決を覆して実装し、
> 設計 v2 §7「看(誰でも只读)/ 操作(owner)の二層分離」が揃った。残りは login の失敗レート制限のみ後相。

- [x] owner で `/admin` を開くと、全ユーザの資源が **真名 + 匿名番号 + 使用量**で見える
      (資源名・内容は出ない)。非 owner は 403 / 画面で弾かれる(S1)
- [x] ランキングが指標降順で出る(service=CPU/内存、database=存储、volume=占用)(S1)
- [x] ディスク使用率を閾値より上げる(or 閾値を下げる)と owner に警告メールが届き、
      連続 tick で重複しない(dev は log で確認)(S2)
- [x] owner が他人の service を **メール検証コード入力後に** stop / delete でき、
      コード無し / 誤コードは拒否される(誤コードは焚码で総当たり封じ)(S3)
- [x] owner の delete が対象ユーザの**ゴミ箱**に入り、本人が復元できる(S3)
- [x] `/admin/audit` で S1–S3 の操作(`owner.stop_service` 等)が **actor / 対象ユーザ真名付き**で
      辿れる(S4)
- [x] S5 共有パスワード viewer:非 owner が共有パスワードで overview / ranking を只读で見られ
      (audit は不可)、owner が設定 / リセット(旧 grant 全失効)できる。dev e2e で鉴权フロー検証済み
```
