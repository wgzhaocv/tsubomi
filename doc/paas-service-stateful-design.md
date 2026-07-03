# tsubomi PaaS — service 任意ポート + stateful(自帯コンテナ)実装設計

> visibility 三態(`doc/paas-service-visibility-design.md`)に続く**マイルストーン外の追加機能**
> (背骨は変えない・新表なし・列 1 本 + パラメータ解放)。
>
> 解く問題:managed database(pg-tenant)は拡張(pgvector 等)を入れられない。ユーザが
> 「拡張入り Postgres」「meilisearch」「Grafana」のような**自帯コンテナ**を必要としたとき、
> 平台は全部を先回りで用意できない(できるべきでもない)。service は既に「任意の Dockerfile を
> 走らせる箱」だが、3 つの焊死がこの用途を塞いでいる:
> ① `container_port` が create 時に 8080 固定(`mod.rs::insert_attempt` の DTO と DDL DEFAULT のみ —
> **下游の route / PORT env / M6 リンク URL は既に全部 DB 由来**で、焊死は入口 1 箇所)。
> ② deploy が start-first swap = 新旧容器が**同一データ目録を同時に開く**。postgres の
> postmaster.pid 防双開は `kill(pid,0)` 探活で、**跨 PID namespace では信頼できない**
> (旧 postmaster の PID が新容器内に存在しない → 錠を陳腐と誤判して双開 → データ破壊)。
> ③ M6 リンクの注入値が `http://<sub>:<port>` テンプレート固定で、非 HTTP ソフトには
> scheme が廃紙(HOST/PORT の素材が無い)。
>
> 設計判断の経緯(2026-07 の検討):第 5 のリソース「自定義容器」や compose_spec 多容器も
> 検討したが、**4 リソース + 動詞「注入」の分類学を動かさず service を 3 箇所で撑開する**
> 本案に収斂した。compose のユーザ願望は「N 個の service + 注入で連線」に分解される。
> 多容器(compose_spec、tech-design M6)は別需要(同生死 sidecar/worker)として後相に残る。
>
> 完了判定(prod・香橙派):pgvector 入り Postgres を
> `tbm service create mypg --port 5432 --stateful` → 自動 private → volume 注入(PGDATA)→
> `tbm deploy --local` → web app に M6 リンク + `_HOST`/`_PORT` で接続文字列を組んで実接続 →
> **redeploy してデータ健在**(stop-first の実証)→ 故意に壊した image で deploy 失敗 →
> **旧版が自動復旧して serving**(§4 の退路)。

---

## 0. スコープと確定事項

出すもの:

- **`container_port` を create 時に指定可能**(既定 8080。1–65535 検証)。
- **visibility の既定推導**:create 時に明示指定が無ければ、port==8080 → `company`(現状)、
  port!=8080 → `private`。推導は server の create handler(単一真源)。
- **`service_details.stateful`(新列・migration 1 本)**:true の service は deploy /
  rollback / reconcile 復活が **stop-first**(旧停止 → 新起動)。既定 false = 挙動不変。
- **M6 リンク注入に `<BASE>_HOST` / `<BASE>_PORT` を追加**(`_URL` は温存)。
- **`memory_mb` を create 時に指定可能**(既定 **1024** = migration 20260620 が引き上げた
  DDL DEFAULT と一致させる[512 と書くと是正の逆行 — simplify review 2026-07-03 で検出]。128–4096 検証)。
- CLI(`tbm service create --port/--stateful/--visibility/--memory`)+ web create フォーム。

出さないもの(いずれも本設計を塞がない):

- **公網裸 TCP 入口**:ingress は HTTP のみ。外部 psql 直連は不可(必要になれば pg:443 中継 /
  cache-gate の作法で別設計 §10-A)。本機能は「平台内 app が使う依存」の語義。
- **`--image` 直拉**(repo / 1 行 Dockerfile 無しで公共 image を走らせる糖衣):後相 §10-B。
  当面は `FROM pgvector/pgvector:pg17` の 1 行 Dockerfile + `tbm deploy --local` で足りる。
- **compose_spec 多容器**(同生死 sidecar):tech-design M6 のまま後相。
- **port / memory の create 後変更**:否決 §10-C(理由は §0-D)。

確定する細部(各々**否決可**):

- **§0-A port は「宣言」であって配線ではない**:route ファイルの backend URL・PORT env・
  M6 リンク URL は全部 deploy 時に DB の `container_port` から烙印される既存管線。入口
  (create)にパラメータを 1 個足すだけで、下游は 1 行も変えない。
- **§0-B 推導は初期値のみ**:明示 `--visibility` が常に優先。create 以降、port と visibility は
  独立フィールド(port を理由に visibility を後から自動変更しない)。web create も同じ server
  推導に乗る(前端で二重実装しない)。
- **§0-C stateful は deploy 語義の宣言**:false = start-first swap(無瞬断・無状態前提)/
  true = stop-first(数秒瞬断・データ目録の単独占有を保証)。**片方向変更のみ**
  (false→true は既存 workaround 済み service の救済に許可、true→false は拒否 —
  swap がデータ目録双開を起こす方向なので入口で塞ぐ)。変更入口は S2 では作らず、
  必要になったら visibility と同じ専用 POST を足す(§10-D)。
- **§0-D port は create 時のみ = 不変**:後変更を許すと「DB の port ≠ serving 容器が listen する
  port」の窓が生まれ、visibility 切替の route 再生成(DB から読む)が**動いている容器と不整合な
  route を書く**。port 不変なら「DB = 全 deploy = serving 実態」が恒等で成立し、この穴が
  存在しない。間違えたら作り直し(AI 駆動なので setup_commands 再実行のコストは低い)。
  後相で必要になったら「deploys 行に port を焼いて serving 実態を真源にする」改修とセットで解禁(§10-C)。
- **§0-E stateful の失敗退路 = 旧容器の温存再起動**:stop-first は「旧を stop(**remove しない**)→
  新を起動 → is_live → commit → route/attach → 旧を remove」。新が起動に失敗したら
  新を掃除して**旧 stopped 容器を `start_container` で再起動** = 自動復旧(route / attach は
  旧容器名のまま無傷)。「旧版温存」の精神(§6.4)を stateful でも最大限保つ。
- **§0-F 瞬断は stateful の契約**:正常時 = stop→start の数秒。失敗時 = 旧再起動までの十数秒。
  受容(データ整合の対価。無瞬断が欲しいものは stateful ではない)。
- **§0-G graceful stop の猶予 30s**:DB は SIGTERM 後の flush に時間がかかる。docker stop の
  t=10s 既定では SIGKILL に倒れて WAL 回復頼みになるため、stateful の stop は t=30 を明示。
  (stateless の stop_remove は現状のまま。)
- **§0-H `_HOST`/`_PORT` の名前は `_URL` の作法を踏襲**:注入 env_var の末尾 `_URL` を剥いだ
  BASE に `_HOST` / `_PORT` を付ける(cache の `key_prefix_env` と同型)。値:HOST = callee の
  subdomain(docker 網別名)、PORT = callee の container_port。**`_URL` は温存**(HTTP app の互換)。
  派生名が別注入の env_var と衝突したら dedup_env_last の後勝ち(cache `_KEY_PREFIX` と同じ
  既知ギャップ、受容)。
- **§0-I PORT env は従来どおり注入**:非 HTTP ソフト(postgres 等)は無視するだけ = 無害。
  分岐を増やさない。

---

## 1. DDL(migration 1 本)

```sql
-- service の deploy 語義:false = start-first swap(無瞬断・無状態)/ true = stop-first
-- (数秒瞬断・データ目録の単独占有)。既存行は DEFAULT false = 挙動不変。
ALTER TABLE service_details
  ADD COLUMN stateful BOOLEAN NOT NULL DEFAULT false;
```

`container_port` / `memory_mb` / `visibility` は既存列(焊死は入口だけ)。migration は触らない。

---

## 2. API 面(S1)

`CreateServiceReq` に任意フィールドを足す(既存クライアントは省略 = 全部既定 = 挙動不変):

```rust
pub struct CreateServiceReq {
    pub name: String,
    #[serde(default)] pub container_port: Option<i32>,  // 既定 8080。1–65535
    #[serde(default)] pub visibility: Option<String>,   // 既定 = §0-B の推導
    #[serde(default)] pub stateful: Option<bool>,       // 既定 false
    #[serde(default)] pub memory_mb: Option<i32>,       // 既定 1024。128–4096
}
```

- 検証は server 側:port 範囲、memory 範囲、visibility は既存 `Visibility` パース。
- `insert_attempt` が列を明示 INSERT(現在は DDL DEFAULT 任せ)。
- 推導ロジックは create handler に 1 箇所
  (`visibility.unwrap_or(if port == 8080 { Company } else { Private })`)+ 単体テスト。
- audit(`service.create`)の detail に port / stateful / visibility を足す。

CLI:`tbm service create <NAME> --port <PORT> --stateful --visibility <V> --memory <MB>`
(全部任意。引数 help 必須 — AI フレンドリ規約)。web は create ダイアログに「詳細設定」折疊。

---

## 3. deploy 管線の分岐(S2・本体)

`run_digest_inner` の起動部を stateful で分岐する。共通部(pull / inject 解決 / RunSpec 組立 /
commit_success / route / attach / audit)は不変。

```
stateless(現状のまま)                stateful(新)
────────────────────────              ────────────────────────
1. pull                               1. pull
2. inject 解決                        2. inject 解決
3. 新容器起動 + is_live               3. 旧容器を stop(t=30、remove しない)
   └ 失敗 → 新掃除、旧無傷           4. 新容器起動 + is_live
4. commit_success                        └ 失敗 → 新掃除 → 旧を再 start = 自動復旧
5. route 切替 → attach → 旧 remove    5. commit_success
                                         └ 失敗 → 新掃除 → 旧を再 start
                                      6. route(visibility 尊重)→ attach → 旧 remove
```

- **旧の特定**:`list_by_service` の走行容器(通常 1 つ)。無ければ stop を飛ばして起動のみ
  (初回 deploy / クラッシュ後の reconcile 復活は自然にこの経路)。
- **旧の再 start**:bollard `start_container`(stopped 容器の名前・網 endpoint・binds は温存
  される)。route / attach は切替前なので無傷 = 復旧に再配線不要。再 start 自体が失敗したら
  それ以上は救えない — phase=failed + error に両方の失敗を記録(退路は `tbm service rollback`)。
- **deploy_lock は既に取っている**(run_digest 先頭)。stop→start の窓に reconcile が割り込む
  競合は、reconcile も同 lock + trigger 再確認で守られる(既存機構、追加なし)。
- rollback / reconcile 復活 / 中断 deploy 収束は全部 `run_digest` 経由なので、**分岐 1 箇所で
  全経路が stateful 語義になる**。

---

## 4. 注入の増分(S3)

`inject.rs` の `"service"` 分支(1 箇所)に 2 行足す:

```
env.push((env_var, format!("http://{subdomain}:{port}")));      // 既存
env.push((format!("{base}_HOST"), subdomain));                   // 追加
env.push((format!("{base}_PORT"), port.to_string()));            // 追加
```

`base` は `env_var.strip_suffix("_URL")`(無ければ env_var 全体)— `key_prefix_env` と同型の
純関数 + テスト。ユーザは `postgres://user:pass@$MYPG_HOST:$MYPG_PORT/db` を自分の密碼で組む
(平台は自帯コンテナの中の資格情報に関与しない — 管理境界 = ユーザ)。

---

## 5. 触らないもの(確認済みの既存到達点)

- **内部到達性**:M6 リンクの attach は docker 網別名 = **port 制限なし**(宣言 port 以外も
  内部からは届く。MinIO の 9000/9001 のような複数 port ソフトも成立)。
- **visibility=private**:route ファイル無し・公開 URL 無し(v38 で e2e 済み)。非 8080 の
  既定がこれになるだけ。
- **volume 注入**:データ目録の永続化はこれで足りる(bind mount + 日次 rsync 同乗)。
- **egress / 容器加固 / reconcile 保活 / logs / exec / terminal / ゴミ箱**:全部そのまま効く。

---

## 6. 地雷(実装時に踏みやすい順)

1. **stateful 分岐を「旧 remove → 新起動」と書かない**こと。remove すると §0-E の自動復旧が
   消える。stop(温存)→ 新失敗時に再 start、が本設計の核心。
2. **commit_success 失敗時も旧再起動**(表 §3 の 5)。start-first では「新掃除だけ」で旧が
   serving 継続だが、stateful では旧が stopped なので**能動的に**戻す必要がある。
3. **graceful stop t=30**(§0-G)。既定 10s のまま SIGKILL に倒すと、次回起動が WAL 回復で
   遅くなり is_live 窓(900ms×3)と誤衝突しうる。
4. **is_live は DB の遅発クラッシュを検出しない**(起動 1 秒後の設定エラー死等)。既存 app と
   同じ受容 — phase は reconcile が failed に寄せ、logs で診断(既存機構)。
5. **非 8080 + company/public の組は乱码/502**(traefik HTTP → 非 HTTP 後端)。穴ではなく
   噪音(traefik は HTTP しか話さない)。既定 private が防ぎ、明示で開けた人の自己裁量(§0-B)。
6. **`_HOST`/`_PORT` の衝突は後勝ち**(§0-H)。UNIQUE(service_id, env_var) は `_URL` にしか
   効かない。cache `_KEY_PREFIX` と同じ受容。
7. **推導を CLI / web で二重実装しない**(§0-B)。server が唯一の真源。CLI は None を送るだけ。
8. **旧容器再 start 後のデータ目録**:失敗した新容器がデータを触った後(例:pg 大版本升級の
   途中死)の再 start は救えないことがある。自管 DB の升級リスクとして受容(managed database
   との差 = 管理境界。ユーザに見える文言は deploys.error に載せる)。

---

## 7. 受容するコスト(正直な差異)

- stateful の deploy は**必ず瞬断**(数秒〜失敗時十数秒)。
- 自帯コンテナの中身(資格情報・升級・チューニング・スキーマ)は**全部ユーザの責任**。
  managed database/cache(web SQL / rotate / 托管備份)との住み分けはここ — 平台が保証するのは
  「活きている・データ目録が在る・app から届く」まで。
- 公網から自帯コンテナへの直連(外部 psql 等)は不可(§10-A まで)。
- 非 HTTP ソフトの deploy 済み検証は平台からは浅い(is_live のみ)。接続レベルの疎通は
  ユーザ/AI が `tbm service exec` 等で確認する。

---

## 8. 切片(各切片で simplify + codex review → commit)

| # | 内容 | 量級 |
|---|---|---|
| S1 | create パラメータ解放(port / visibility 推導 / memory)+ 検証 + CLI/web 入口 + テスト | 小 |
| S2 | `stateful` 列(migration)+ stop-first 分岐(§3)+ 失敗復旧 + dev e2e | 中・本体 |
| S3 | `_HOST`/`_PORT` 注入 + テスト | 小 |
| S4 | prod e2e(完了判定の pgvector 流程)+ skill 文書増補(次回 version bump に同乗) | 小 |

## 10. 後相(否決済み・保留)

- **§10-A 公網裸 TCP**:per-service TCP 入口。pg:443 frp 池 / cache-gate の作法を一般化する
  独立設計。需要が立ってから。
- **§10-B `--image` 直拉**:公共 image を repo 無しで走らせる糖衣。外部 registry pull
  (Docker Hub 限流・双架確認・tag→digest 固定)が新規面なので独立切片。
- **§10-C port の create 後変更**:deploys 行に port を焼き「serving 実態」を真源にする改修と
  セットでのみ解禁(§0-D の不整合を塞いでから)。
- **§10-D stateful の後から変更入口**(false→true 片方向、visibility 同型の専用 POST):
  既存 workaround service(DB を stateless で走らせている人)が現れたら。
- **compose_spec 多容器**(tech-design M6):同生死 sidecar/worker の需要が立ってから。
