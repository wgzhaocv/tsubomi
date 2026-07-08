# tsubomi 蕾 — 社内 PaaS プラットフォーム

セルフホストの「基礎版 Vercel + Neon」:社内の非エンジニアが AI(CLI)経由で
app をデプロイし、データベース / ボリュームを作る。
単機運用、プラットフォームのプロセスはホスト直走り(docker.sock を保持)。
ホストは今は香橙派(**ARM64**)、後で **x86_64** 機にも移す/増やす ⇒
イメージ・配布物は初日から両アーキテクチャ対応。

## 必読ドキュメント(アーキテクチャを変える前に読む)

設計・調査・障害記録の md は全部 **`doc/`** にある(`CLAUDE.md`・`README.md` だけ根に残す)。
以下のパスはその前提。

- `doc/paas-design-v2.md` — 設計意図:4 種のリソース(service/database/cache/volume)+
  動詞は「注入」ひとつ;境界と引き受けたコスト。
- `doc/paas-tech-design.md` — 技術設計:**§0 の 6 つの確定事項を黙って覆さない**。
  DDL・デプロイ経路・API 面・マイルストーンは全部ここ。

背骨を一言で:**管制面 Postgres が「期望状態」を持ち、現実(コンテナ/ユーザ DB/
ディスク)をそこへ収束させる**。注入はバインディングだけを保存し、値はコンテナ
起動の瞬間に解決する(だから rotate 後は再デプロイして初めて効く — これは仕様)。

## フェーズ(現在地:M5 cache(valkey)完了 — 設計フェーズの 4 リソース出揃い)

M0 基盤(ログイン/CLI token)→ **M1 database(完了)** → **M2 volume(完了)** →
**M3 service(完了)** → **M4 ガバナンス(完了)** → **M5 cache/valkey(完了)**。
各フェーズ単体で使える状態にする。マイグレーションはフェーズ毎に追加。

M5 で入ったもの(S1–S3。dev e2e 済み):infra に **valkey**(`valkey/valkey:8-alpine`、edge 参加、
default off + `tsubomi-admin` を compose の `--user` で静的定義、ホスト側 6433 で loopback 公開)+
migration `cache_details`(`acl_user=namespace=c_<shortid>`/`password_enc`/`rotated_at`)。**cache リソース一式**:
`crates/server/src/{caches.rs,valkey.rs}`(create/list/get/rename/url/rotate/delete)。隔離は **valkey ACL**
(値 `~<ns>:*` + チャンネル `&<ns>:*` + コマンド白名単 `+@all -@admin -@dangerous -@scripting` = 越境 /
FLUSHALL / KEYS / EVAL 系・SCRIPT・FUNCTION は NOPERM。スクリプティング全禁は単一スレッド共有 valkey の
イベントループ DoS 対策 — codex 監査 2026-06-26。値は隔離・key/channel **名**は SCAN/PUBSUB で列挙され得る =
受容済み §11-I)。per-cache ACL は揮発なので**起動時 + 30s 周期で収束**(`valkey::reconcile_acls`、毎 tick fresh に
生存 cache を読む = RACE-1)。**注入**:cache → `REDIS_URL`(内部入口 `tsubomi-valkey:6379`)+ `REDIS_KEY_PREFIX`
(`<ns>:`。値は起動の瞬間に解決 — rotate は再デプロイで効く)。rotate は **DB 先 → valkey**(背骨どおり前向き収束)。
ゴミ箱:delete=`ACL DELUSER`(key 温存)/ restore=ACL 再作成 + 生存 key 数報告(best-effort)/ purge=`SCAN+UNLINK`。
owner 最後の砦に cache delete、admin overview/ranking に cache(指標=key 数)。web 詳細(`CacheDetail.tsx`)+
CLI `tbm cache`。**最終 e2e 済み**:cache を使う service(Node ioredis カウンタ)をデプロイし、公開 URL で
`<ns>:visits` を INCR して跨リクエスト永続・隔離内を実機確認。**prod-infra 込み**:`compose.prod.yml` に valkey
(loopback 6433・edge・外部 ingress なし・admin pass 必須)、`just ship` が M5 イメージ build + compose 配布 +
`up -d --no-recreate`(不足 infra=valkey 等を起こす)+ `up -d server`(server だけ入替=全 app 無瞬断)で展開
(前提:Pi の `.env.production` に `TSUBOMI_VALKEY_ADMIN_PASS`/`_URL`)。実装級は **`doc/paas-m5-design.md`**。

**M5 後の追加(マイルストーン外):コンテナ内アクセス**。service 詳細から動いているコンテナの中を
確認する 2 入口を `bollard exec` ひとつを土台に足した(新テーブル・migration なし)。**A. web 対話
ターミナル**(`GET /services/{id}/terminal` WS + PTY、`@wterm/react`、`services/docker.rs::handle_terminal`、
web `ServiceTerminal.tsx`)+ **B. CLI 一発 exec**(`POST /services/{id}/exec`、`tbm service exec <name> --
<cmd…>`、`{stdout,stderr,exit_code,truncated,timed_out}`)。役割分担:**対話 PTY は CLI の AI フレンドリ
JSON 契約に合わないので web 専用**、一発 exec は捕獲出力 = AI 駆動可なので CLI に乗せる。どちらも
`ensure_owned`(**所有者の自資源のみ**・owner→他人は不可)で守り、暴露は **web SQL と同一ティア**
(env 注入値が見える等は受容済み)。監査は exec=argv 記録 / terminal=open イベントのみ(PTY 打鍵は
記録不可)。terminal は **session 由来必須**(Bearer 拒否 = web 専用)+ **WS 升级で Origin を管制面
オリジンに固定**(`auth::require_ws_origin` / `Config.control_origins` 既定 server_url +
`TSUBOMI_CONTROL_ORIGIN`。CSWSH 対策 — テナント app は same-site なので SameSite=Lax だけでは
不足。既存 metrics WS にも同適用)。地雷(tty 一致・WS split で 2 方向・input drop が唯一の回収・
最大セッション timeout・出力 cap 厳守)は **`doc/paas-terminal-design.md`** に集約。

**M6 後の追加(マイルストーン外):service↔service 内部リンク**。app A が app B を呼ぶのに公開 URL
(Cloudflare 往復 = インターネット経由)しか無かったのを、**注入で内部直連**できるようにした(新表・migration なし)。
`tbm inject B --into A` で A に `B_URL=http://<B-subdomain>:<B-port>` を注入し、**B の serving コンテナを
A の per-service 私網へ docker 網別名 = B の subdomain で客人 attach** → A は docker DNS で B へ直連(インターネット
不経由)。M6 の真の境界=租户なので **同一 owner 限定**(注入作成時に自動担保 + 自注入禁止)。egress は
同 subnet RETURN で素通り=不変。実装:`inject.rs` の service 分支(値解決)+ `network.rs`(別名 connect /
`attach_callees`=caller 側が callee の route 後端を attach / `attach_as_callee`=callee の deploy 直後 / 
`detach_callee`=eject 即時 / `remove_service_network` は全 endpoint 剥がし / reconcile に陳腐客人 GC)+ 
**attach は deploy の route 切替点で呼ぶ**(公開カットオーバーと内部可達性を揃える — codex 監査)。CLI/web
は注入入口に service を足すだけ(`resolve_resource` / `ServiceEnv.tsx` の下拉)。正直な差異(内部串は http・
Host は `b:<port>`・IP 白名単/中間件なし)は受容済み。**本番 e2e 済み**:fg-arch の私網に hanadayori を
リンクし、診断コンテナから `http://hanadayori:8080` が実体を返す一方、未リンクの sagi-ad-demo は `bad address`
(隔離維持)を香橙派で実機確認。地雷・確定事項は **`doc/paas-service-link-design.md`**。

**内部リンク後の追加(マイルストーン外):service 公開範囲(visibility)三態**。全 service に必ず公開 URL が
生える前提を崩し、`service_details.visibility`(migration 1 本、`private`/`company`=既定/`public`)で
**route ファイル(`svc-<id>.yml`)の生成を分岐**する:private=書かない(subdomain 温存・外部からは catch-all →
302 /noservice。監視系 worker 用)/ company=現状(ipallow middleware)/ public=middleware を掛けない
(一般公開 — 当初 M3 で drop した `public` 列の意図の再来。本人裁量 + audit)。**切替は即時**
(`POST /services/{id}/visibility` が deploy_lock 内で DB 先行 → route ファイル再生成/削除。env と違い
再デプロイ不要)。reconcile の drift 判定は `(backend, ipallow)` の組に拡張(public→company の書込失敗が
fail-open で残る穴を塞ぐ)。付随修理:`attach_callees` の callee 解決を route ファイル依存から
`serving_container`(DB の直近成功 deploy + 実走確認)へ = **private callee への M6 リンクが主用途**。
入口:`tbm service visibility`(status 表示 / verify は private 短絡)+ web 概要の Radio 3 択(URL バナーは
灰化・温存)。**本番 e2e 済み**(2026-07-03、server v39 / tbm 1.0.18):private=どの IP からも 302
/noservice(社外 VPS からも確認)・yml の ipallow 行が public で消え company で戻る・切替は traefik file
watch で数秒反映・**private callee への M6 内部リンクが caller コンテナから実体を返し**、未リンクは
`bad address`(隔離維持)。会社 IP 許可リストは現状**空 = fail-open**(company≒public。owner が
entries を入れた時に差が立ち上がる)。実装級は **`doc/paas-service-visibility-design.md`**。

**visibility 後の追加(マイルストーン外):service 任意ポート + stateful(自帯コンテナ)**。managed database に
拡張(pgvector 等)を入れられない需要への回答 — 第 5 のリソースも compose も作らず、**service を 3 箇所で
撑開**して「自帯 postgres / meilisearch / Grafana」を成立させた(migration 1 本 = `service_details.stateful`)。
(S1)create パラメータ解放:`container_port`(1–65535。**8080 焊死は入口 1 箇所だけで、route / PORT env /
M6 リンク URL の下游は元々 DB 由来**)+ `memory_mb`(既定 1024)+ `stateful`、CLI `--port/--stateful/
--visibility/--memory` + web 詳細設定折疊。**visibility 省略時は port から推導**(8080→company / 他→private。
単一真源は server の create handler、CLI/web は None 素通し)。CLI は作成回显を検証し旧サーバの静默無視を
エラー化。port / stateful は**作成後不変**(変更許可は deploys に port を焼く改修とセット — 設計 §10-C)。
(S2)**stateful = stop-first deploy**:swap は新旧が同一データ目録を同時に開く(postgres の postmaster.pid
防双開は跨 PID namespace で信頼できない = 双開→破壊)ため、`docker::stop_running`(SIGTERM 猶予 30s・
**remove しない**)→ 新起動 → 失敗なら温存した旧を再 start = **旧版自動復旧**。猶予は共有停止路径
`stop_remove` が**自分で stateful を読んで**決める(stop / delete / purge も 30s)。route 切替失敗時、
stateful は内部カットオーバーを進める(旧は停止済みで温存の意味が無い)。分岐は `run_digest_inner` 一箇所 =
hook / start / rollback / reconcile 復活の全経路をカバー。(S3)M6 リンク注入に **`_HOST` / `_PORT`** を併注
(`_URL` の http テンプレは非 HTTP ソフトに廃紙 — 利用側が自分のスキームで接続文字列を組む素材。
`inject.rs::host_port_base`、resolved env の由来判定は `derived_env_source` に一般化)。dev e2e 済み:
postgres(--port 5432 --stateful + volume=PGDATA)の redeploy でデータ健在・坏 image で旧版自動復旧・
graceful stop。**副産物の発見 → 同日修正(v42)**:registry GC の `--delete-untagged` が(a)tag 再利用で
失参照になった**現役 digest** を回収し start/rollback が pull 404(既存バグ・dev 実証)、(b)**tag 付き
index の子 manifest まで食う**(distribution 既知欠陥 — 本番 index の子欠損で実証。keep 保護 tag 方式は
子欠損 index に PUT 400 で不成立 = 方式転換)。最終形 = **`--delete-untagged` 廃止**、manifest 削除の
判断は平台だけ:`registry::protect_and_expire_manifests`(日次 GC 前段)が keep 窓(現役 ∪ 直近 5
distinct 成功版)外の terminal 旧版を「index → 子」の順に明示 DELETE(子は keep/in-flight index が
共有する分を除外 — buildx キャッシュの同一子共有を dev で実証)。rollback 実効窓 = 5 版に確定。
実装級・受容・残余は **`doc/paas-service-stateful-design.md` §10-E**。

**1.0.20 後の追加(マイルストーン外):部署闭环 + 可観測性 + db query 強化(server v43 / tbm 1.0.21)**。
AI 重度利用フィードバック第二弾。今回は **server も動かした**(v43 = Docker Hub push + `just ship`。
無瞬断:infra `--no-recreate` + server 単換)。**server 側(W1-W3)**:(W1)**流式ログ**
`GET /services/{id}/logs/stream`(bollard follow → `Body::from_stream`、30 分 backstop を docker.rs で
強制、Bearer/session 両対応で CSWSH 無縁 = read-only 自資源)+ `/api` 未マッチを 404 に確定
(旧サーバは SPA fallback で 200+HTML を返し新 CLI が未対応端点を機械判別できなかった穴を塞ぐ)。
(W2)**単発 metrics** `GET /services/{id}/metrics`(inspect + running 時のみ stats、CPU/メモリ上限比/
再起動/uptime/OOM。停止も 200 running:false)。(W3)**db query パラメータ化**(`QueryReq.params`、bind
経路、`col_to_string` に binary format 分岐 + NUMERIC を bigdecimal で直解、human role・timeout・
1000 行上限は不変)。**CLI 側(1.0.21)**:(C1)`verify --for-sha <sha|HEAD>`(CI ビルド窓もカバーする
端到端待機 + serving 報告)/(C2)**`deploy --watch`**(push→Actions 追跡→デプロイ完走→検証を一括。
gh はユーザ自身)/(C3)`logs --follow/--since`・`service metrics/deploys/open`・`db query --csv/--param`/
(C4)**deploy preflight**(.env 混入 / COPY 元不在 / EXPOSE 不一致を build 前に警告・阻止しない)。
本番 e2e 済み(2026-07-03、server v43 / tbm 1.0.21):metrics 実値・logs --follow 実時流式・
db --param/--csv、無瞬断展開(全 app 200 継続)。実装は各切片の commit と本段落。

**1.0.21 後の追加(マイルストーン外):デプロイ可観測性 + --watch QoL(server v45 / tbm 1.0.24)**。
AI 重度利用フィードバック第三弾。発端の「ログが stdout しか出ない」は**実証の結果誤診**
(logs は当初から stdout+stderr 両取り — 同構成コンテナ + 同版 bollard で実測。真因は空バイナリが
exit 0 で無出力 + 失敗コンテナ掃除後の logs は旧コンテナを指す)で、本丸は「秒退時に退出コードが
見えない」方。**server**:`docker::crash_summary`(失敗 deploy のエラーに exit code / OOMKilled /
再起動回数 + exit code 別ヒントを併載。restart 済みは exit=0 リセットを誤診しないよう crash-loop
文言に切替、OOM は true のときだけ添える)。**CLI**:`deploy --watch` の ① upstream 未設定は実
remote で自動 `push -u`(選好:`@{push}` → pushRemote/pushDefault → tsubomi → origin → 唯一。
origin 固定案内は tsubomi remote で失敗していた)② 複数サービスでも repo の `TSUBOMI_SERVICE_ID`
variable から自動推断 ③ `--for-sha`(verify と同型。`^{commit}` 実在検証・過去 sha 追跡時は HEAD の
WIP を巻き込み push しない)④ gh 呼び出しに `-R` 貫通(複数 remote の既定解決エラー回避)。
**skill**:「起動直後にクラッシュする」playbook(exit code 速查 → 観察モード → exec 調査。2>&1
不要を明記)。品質検証は 4 simplify agents + codex 二輪(計 21 findings、真バグ 6 件を出荷前後に
回収 — clap の `--local --for-sha` 静默受理 / 偽 sha の timeout 空費 / crash-loop の exit=0 誤報 等)。
本番展開済み(2026-07-08、Docker Hub v44/v45 双架 + Pi 無瞬断 + CLI 4 平台)。
**同日の本番事故 → 恒久修正(server v46 / tbm 1.0.25)**:registry GC が**起動直後 tick** で
走る設計のため、ship のたびに任意時刻で manifest DELETE + blob 掃除(Pi で 10 分超)が発火。
掃除中に同一 digest を再 push すると dedup が掃除前 blob を見て書き込みを省略 → **PUT 201 なのに
GET 404**(CI は push 成功、deploy は manifest unknown。利用 AI は「registry 双入口の分裂」と
誤診したが、実体は同一 registry での假成功 — DELETE→再 push→GET 200 を窓外で実証し切り分け)。
修正:①expendable に **48h 年齢下限**(直近 push は消さない — 再 push 競合の餌 + 失敗イメージは
再試行/診断に要る)②GC を**毎日 19:05 UTC(04:05 JST)固定**・起動 tick 廃止(gc.rs
`until_next_utc`)③pull の manifest unknown エラーに「再デプロイで再 push」の次の一手 + skill
早見表に一行。復旧は再デプロイのみ(毒された digest は再 push で実体が落ちる)。

**stateful 後の追加(マイルストーン外):CLI の AI フレンドリ改善(tbm 1.0.20)**。AI 利用の
フィードバック起点の CLI 純粋な磨き込み(server はほぼ不変)。(1)**`tbm db query --tsv`**:
JSON の `results[0].rows[0][0]` を毎回 jq/node で剥く手間を無くす行だけのタブ区切り出力
(tuples-only・NULL は空・`\`/タブ/改行はエスケープで「1 行=1 レコード」保証。`count=$(tbm db
query db "select count(*)…" --tsv)` を一発に)。(2)**`tbm service verify --wait [--timeout]`**:
`tbm deploy` は送信即戻り・切替は非同期(数秒の滾動遅延)なので、これまで status を手で輪詢
していたのを、最新デプロイ(created_at DESC)を 2s 輪詢して succeeded まで待ってから検証する
(failed は error + 次の一手で非零終了 / succeeded 直後は traefik file-watch 反映を 15s 窓で吸収)。
既知の限界:GitHub 経路で CI ビルド中(hook 未達)は最新=旧版のため待たずに検証。(3)`tbm
--help` トップ概要を実サブコマンド面に同期(db に query/info 等が欠けていた — AI の第一発見面)+
`parent_about_lists_all_subcommands` テストでドリフト機械封じ。(4)`tbm whoami` の JSON から
avatar_url(長大 URL = AI 捕捉の雑音)除去(`WhoamiOut` を明示ビュー化。shared 契約は不変)。

M3 は prod-infra 込みで完了し **`tsubomi-app.com` で本番稼働・端到端検証済み**(両デプロイ経路:
`git push`→GitHub Actions と `tbm deploy --local` の両方で `https://<sub>.tsubomi-app.com` が開くことを実機確認)。
本番トポロジ:香橙派(arm64、共有ホスト)+ **Cloudflare Tunnel**(上流 TLS 終端 → `TSUBOMI_TLS` 未設定 =
traefik は HTTP :80)。専用ドメイン `tsubomi-app.com`(CF zone)でサービスは一級子域 `*.tsubomi-app.com` =
免費 Universal SSL 覆盖(ACM 不要)。デプロイは `just ship`(`docker save|ssh load`、`~/tsubomi-deploy`)。
詳細・2 モード(上流TLS / 直VPS+LE)は `doc/paas-m3-design.md` §13。

M1 で入ったもの:`resources` スーパーテーブル + `database_details`/`database_roles`
+ `audit_log`;pg-tenant(ユーザ DB)+ pgbouncer(外部入口、auth_query、client TLS);DB 作成/
一覧/接続文字列/rotate/web SQL/ソフト削除→ゴミ箱→復元/日次バックアップ;at-rest
暗号化(crypto.rs、XChaCha20-Poly1305);`tbm db` サブコマンド。**双 role**:app
(内部、M3 で service に注入)+ human(外部、rotate 可)— 詳細は §2/§5。
**外部接続文字列は部署トポロジで開閉**(`TSUBOMI_DB_PUBLIC_ENABLED`、既定 false):CF Tunnel など
公開 TCP 入口を持たない部署では web が接続文字列カードを隠し `/url`・`/rotate` も後端で拒否
(`require_db_public`。届かない LAN IP の誤誘導を断つ)。グローバル IP の VPS でのみ true。web SQL タブと
human role 自体はこのフラグと無関係で常に動く(web SQL は tenant_admin_url 経由 = 公開ホスト不使用)。
AuthInfo(`/auth/info`)に `db_public_enabled` を載せ前端が判定。**公開 DB の ipblock も実装済み**:
有効時 `ipblock::sync_traefik` が `db-tcp.yml`(Traefik **TCP** router + ipAllowList + service=
内部 pgbouncer)を書き、**会社 IP 許可リスト(`ip_allow_entries`)を TCP にも流用**(無効なら削除)。
pgbouncer が client TLS を終端するので Traefik は素の TCP passthrough。VPS は `compose.prod.db-public.yml`
を重ねて Traefik に `postgres`(:6432)入口を生やす(`db_public_enabled=true` と override はセット)。
描画 + 単体テストは dev で検証済み・**活体検証は VPS 落地後**。実装級は **`doc/paas-db-public-design.md`**。

M2 で入ったもの:`volume_details`;**volume は顶层リソース**(service 所有ではない)。
各 volume は独立した假根サンドボックス `volumes/<user>/<id>`。**唯一のハード境界 =
パストラバーサル防御**(`volumes/safe_path.rs`:Linux=openat2 `RESOLVE_BENEATH|NO_SYMLINKS`、
dev macOS=canonicalize フォールバック、`..`/絶対/NUL/symlink 越えを全拒否)。ファイル API
(列挙/ダウンロード/アップロード=一時ファイル+atomic rename/削除/mkdir/move)+ web
ファイルブラウザ(**パスは URL の splat に持つ** `/volumes/:id/files/<path>`)+ `tbm volume`
フル + ゴミ箱(trash へ mv / 復元 / 完全削除)の web/CLI 入口 + volumes の日次 rsync。
**注入(service への mount + `STORAGE_PATH`)は M3** — 動詞「注入」の相手は service。

M3 で入ったもの(S1–S8):`service_details`/`deploys`/`injections`/`service_env`/
`deploy_nonces`;**service リソース一式** — create + GitHub オーケストレーション(CLI が
ユーザ自身の `gh` で repo/secret/workflow を設定。平台は GitHub に触れない)、deploy hook
(HMAC=権限・nonce・digest ピン留め)+ 非同期パイプライン(bollard、**start-first swap** =
新コンテナを起こし存活確認 → route 切替 → 旧削除。失敗時は旧版を温存 §6.4)、**注入**
(database→app role の内部接続文字列 / volume→bind mount + `STORAGE_PATH` / 静的 env。値は
**コンテナ起動の瞬間に解決** = rotate 後は再デプロイで効く)、lifecycle(start/stop/logs/
delete→ゴミ箱/rollback)、web 詳細ページ(概要/デプロイ/注入/環境変数/ログ)、**reconcile**
(`services/reconcile.rs`、起動時フル + 30s:存在収束 + 孤児掃除。**起動時のみ中断デプロイ収束**=
デプロイ中に server が落ち `phase=deploying` で残った service を deploy_lock 内で desired へ寄せる
[旧版維持で無瞬断 / 孤児新コンテナ掃除]。`restart=unless-stopped` が第一の保険、これが第二)。ルーティングは **traefik file provider**(`svc-<id>.yml`。docker
provider は Docker Engine 29 で壊れるため不使用)。**残り = prod-infra**:GH Actions buildx
双架(arm64+amd64 manifest list)+ 本番 traefik(:443 + LE + 会社 IP 許可リスト)/ pgbouncer /
registry 入口の落とし込み。

M4 で入ったもの(S1–S5。**owner ガバナンスは web 専用** — admin ハンドラは owner 身分 **かつ**
session 由来を毎回検証 `admin::require_owner_web`、Bearer cli_token は拒否):`platform_config` +
`admin_action_codes`。**`crates/server/src/admin/`** に集約 — (S1)**可視化** overview/ranking:
跨ユーザの**匿名化**一覧(真名 + 匿名番号 service1 等、display_name/中身は出さない)+ 指標
(service=bollard stats CPU/内存、database=`pg_database_size`、volume=`volumes::dir_usage` 再利用)。
**ホスト指標**(`metrics.rs`):リソース概要に宿主機の CPU/メモリ/ディスク使用量を出す。
**WS + `tokio::sync::broadcast` の共有サンプラ** — 最初の閲覧者が `/api/admin/metrics` に繋いだ時だけ
採样 task を起こし 5s 毎に全閲覧者へ扇出、最後が切れたら自動停止(誰も見てなければ走らない)。
「subscribe+起動判定」と「send+停止判定」を `metrics_running` ロックで直列化(無人/二重を排除)。
採取は新 crate なし:CPU=`/proc/stat` 差分・メモリ=`/proc/meminfo`・ディスク=`df`(gc と共有)。
dev(macOS)は /proc 無しで CPU/メモリ「—」、prod(Linux)は実値。鉉权は `require_viewer_web`。
**最下部に「プラットフォーム自身」**:同 snapshot に平台容器(server + infra。用户 app 除外)の
**各コンテナ別** CPU/メモリ(`docker::platform_stats`、`join_all` 並行 stats)。性能対策:閲覧中のみ・
`MissedTickBehavior::Skip` + 採取前に `receiver_count()==0` で停止(無人で docker stats を 1 度も走らせない)。実装級は §3.6。
(S2)**Resend メール基盤** `mail.rs`(既存 reqwest、`RESEND_API_KEY` 未設定=log のみ・本文は出さない)
+ **ディスク水位警告**(gc の 1h tick で `df -Pk`、`platform_config['disk_alert_state']` で去重 =
level 上昇 or 24h、送信成功時のみ notified_at 前進)。(S3)**最後の砦**:owner が他人の
service/database/volume を停止/削除(`POST /api/admin/resources/:id/{stop,delete}`、code 無し=
6 桁コードを owner にメール / code 有り=単回消費で検証 → 実行 → **誤コードは焚码で総当たり封じ**)。
既存ソフト削除を `soft_delete(state,id)`(所有権・audit 抜きの素の操作)に切り出しユーザ口と共有、
owner の delete も**対象ユーザのゴミ箱**へ(復元可)。`audit_with_target`(target_user も記録)。
(S4)**audit 閲覧** `GET /api/admin/audit`(keyset 分頁 + action 前方一致、actor/target_user 真名 join、
target_resource は UUID のまま)。web は侧栏 owner 限定 + `RequireOwner` ルート守衛に集約。
(S5)**共有パスワード viewer**(設計 §7「見るは共有密码」= 看/操作の二層分離の「看」):ログイン済み社内
ユーザが共有パスワードを入れると **overview/ranking** を只读で見られる(`sessions.viewer_until` の 8h grant、
密码は `platform_config['viewer_password']`=bcrypt)。`AuthCtx.is_viewer`(`session::get` が同じ行で算出)+
`require_viewer_web`(owner OR viewer)で**読み口(overview/ranking)だけ**緩め、**audit / 危険操作 / パスワード設定 /
ipblock は owner のまま**(audit は真名+明文流水 = 匿名化の範囲外)。owner が設定/リセット(`POST /api/admin/viewer/
password`、リセットで旧 grant 全失効)。bcrypt は `spawn_blocking`、パスワードは 8 文字以上。web は `RequireViewer`
解錠フォーム + owner の `AdminSettings`、危険ボタンは owner のみ表示。dev e2e で鉴权フロー検証済み。
**否決(後相)**:owner 管理 UI(2 人目追加削除は env 種子のまま §10-H)/ viewer login の失敗レート制限
(今は bcrypt + 最小長 8 のみ)。実装級は **`doc/paas-m4-design.md`**。

## 重要な約束事

- **ドキュメント(md)もコードコメントも日本語で書く**(設計議論の中国語
  ドキュメント doc/paas-design-v2.md は例外)。
- Rust の依存は `cargo add -p <crate>` のみ。`[dependencies]` は手書きしない。
- フロントの `vp` = vite-plus(bun + React + TS + Tailwind v4 + shadcn)。vite の
  typo ではない。
- **auth は `~/Desktop/projects/amber` からの移植**(users+credentials の 2 表、
  PKCE CLI ログイン、token は sha256 保存)。tsubomi の差分:session /
  oauth-state / authcode は **Redis ではなく Postgres**(単回消費は
  `DELETE..RETURNING`);Google ログインに **hd ドメイン制限**
  (`TSUBOMI_ALLOWED_HD`、カンマ区切り複数可、サーバ側検証);owner ロール
  (env で種付け、ログイン時昇格);apps の概念は無し。
- Google OAuth は oauth2 crate を使わず手書き(認可 URL + token 交換だけ)。
  理由:oauth2 5.0 が reqwest 0.12 に縛られ、最新 reqwest に上げられないため。
- `time` crate は `=0.3.47` にピン:0.3.48 が cookie 0.18 の blanket impl と
  衝突する(E0119)。cookie 側の対応版が出たら外す。
- CLI バイナリは **`tbm`**(crate 名は tsubomi-cli のまま)。token プレフィックス
  `tbm_`、authcode `tbmc_`、client_id `tbm-cli`。
- CLI の更新は**通知制**:version check はコマンド後に stderr で一言出すだけ。
  更新は常にユーザの手動 `tbm update`(自動更新はしない)。
- `tbm login` は**自動判定**:ローカル GUI は **RFC 8252 loopback**(127.0.0.1 の
  一回限りリスナー)でブラウザの「許可する」だけ、SSH 先・ヘッドレスは自動で
  コピペ方式に倒す(SSH では loopback が原理的に不成立 — リダイレクト先の
  127.0.0.1 は手元マシンを指しリスナーのいる遠隔機ではない)。検出は
  `SSH_CONNECTION`/`SSH_TTY` + Linux の DISPLAY 無し。完全でない(sudo は env を
  消す / mosh)ので `--manual`(強制コピペ)/ `--web`(強制 loopback)で上書き可。
  判定ロジックは `choose_manual()` に切り出し真理値表テスト済み。サーバ側の
  redirect_uri 許可は 2 形のみ(完全一致の本番コールバック / loopback 任意ポート)。
- CLI の配布:`just release-cli-publish`(scripts/release-cli.sh)が 4 ターゲット
  (mac-arm / linux-arm64 / linux-x64 / windows-x64-gnu)をビルドして Pi の
  `~/tsubomi/releases/` へ公開。インストーラは `/install.sh|.ps1|.bat`(配信時に
  サーバがドメインを注入し、初期 config に server_url も書く)、manifest の url は
  相対パス — どちらもドメイン非依存。
  **リリースは不可変**:内容を変えたら必ず CLI の version を上げる(同名再発行は
  Cloudflare が .gz/.zip をキャッシュするため checksum mismatch になる。スクリプトに
  ガードあり)。
  install.sh は rc に PATH マーカーブロックを書き、`tbm uninstall` がそれを目印に
  残留物ゼロで消す。マーカーの正本は tsubomi-shared の `PATH_MARKER_BEGIN/END`
  (install.sh にはインライン展開 — 変えるときは両方揃える)。
- **プラットフォームのアーキは CLI に焼き込む(arm を仮定しない)**:release-cli.sh が公開先ホストの
  `uname -m` を検出して `TSUBOMI_HOST_ARCH`(明示で上書き可)に入れ、`crate::platform::host_arch`
  (`option_env!`)が `tbm --help` / `tbm whoami` / skill 冒頭の `{{HOST_ARCH}}` を埋める。`tbm --help` は
  オフライン生成なのでコンパイル時に焼くのが要点。どのマシンに tsubomi をデプロイしても、その時の
  ホストのアーキが入る(将来 x86_64 へ移しても同じ仕組み)。これは CI のマルチアーキ集合
  `TSUBOMI_PLATFORMS`(buildx の build 対象、§6.6)とは別概念。
- クロスビルドの注意:Homebrew の rust が PATH 先頭にいるので、ビルドスクリプトは
  `PATH="$HOME/.cargo/bin:$PATH"` を前置して rustup の 1.95 を使う(リンクは zig)。
  CLI の TLS は rustls-no-provider + **ring**(aws-lc は windows-gnu / linux への
  クロスコンパイルが通らない)。main() で provider を install_default() している。
- web と CLI は同一 axum ハンドラの 2 入口。分岐は認証 extractor(session
  cookie / Bearer)だけ。新機能を API ハンドラとして書けば CLI から自動的に
  使える。
- **CLI は AI 駆動が主用途 — I/O は「AI フレンドリ」を既定にする**(新コマンドも
  必ずこの型に従う。実装は `crates/cli/src/{main.rs,api.rs,commands/}`):
  - **出力形式はグローバル `-o/--output`(env `TBM_OUTPUT`)、既定 `auto`**。auto は
    **stdout が端末なら text・パイプ/捕捉なら json**(`commands::OutputFormat::resolve`)。
    AI は出力を捕捉する=非 TTY なので、`-o` を付けなくても自動で JSON になるのが要点。
  - **成功出力(json)は shared の DTO をそのまま serde_json**(裸の array/object・
    フィールド安定・jq 可能)。新フィールドは足してよいが既存を壊さない。
  - **エラー(json)は `{"error","code"}` を stdout に出して非零終了**。成功も業務
    エラーも同じ stdout 流で parse できる(`main` のエラー信封)。
  - **`code` は機械分岐用の安定列挙、`error` は人間可読の文案**。code を文字列照合
    させない。列挙:`unauthorized / forbidden / not_found / conflict / validation /
    server_error`(+ ローカル解決の `not_found`、その他 `error`)。HTTP ステータスから
    派生(`api::code_for`)。API 由来は `api::ApiError` に載せて `main` で downcast。
  - **サーバはユーザエラーを 4xx で返す(500 に潰さない)**。重複は 409 Conflict、検証
    失敗は 400。DB の UNIQUE 違反(23505)は `databases::map_unique` で Conflict に。
    これを怠ると AI が「サーバ障害」と誤判し無駄リトライする(過去の実害)。
  - **エラーメッセージは次の一手を含める**(`tbm login を実行` / `tbm db list で確認`)。
    AI が自己修正できる。
  - **秘密は警告を stderr・値を stdout**(json では値だけ。例 `{url}`)。対話的操作
    (`db connect` の psql)は json では起動せず接続先だけ返す。version 通知などの雑音は
    json モードで抑止する。
  - **引数にも help を必ず書く**(`<NAME>` 等)。AI に意味を推測させない。
  - 既知の許容ギャップ:clap の使用法エラー(引数不足等)は text/stderr/**exit 2**
    (実行時エラーの exit 1 と区別できるので可)。display_name 等の表示名は自由文字列
    (識別子は別生成の wire 名)。

## 開発

```bash
just db-up        # infra 起動:pg-platform(:5434)+ pg-tenant(:5435)+ pgbouncer(:6432)
                  #   (マイグレーションはサーバ起動時に自動)
just dev          # server :9090 + web :5173(/api プロキシ。8080 は amber)
just cli login    # dev では CLI のデフォルトサーバは :5173(ログインフローが SPA ルートを使う)
just check        # cargo check + clippy -D warnings + web lint
```

`.env` 必須:`GOOGLE_CLIENT_ID/SECRET`(Google Cloud Console で OAuth client を
作成、redirect URI に `http://localhost:5173/api/auth/google/callback` を追加)。
ドメイン制限と owner の種も `.env` にある。

## 破ってはいけない一線

- 隔離は仕組みで守る、規律に頼らない:IP 許可リスト(Traefik 層)、制限付き pg
  資格情報、volume のパストラバーサル防御(openat2)、コンテナのメモリ硬上限。
- 資格情報 4 種(接続文字列 / deploy key / session / CLI token)は相互流用禁止。
  ハッシュか復元可能かは「プラットフォームが原文を必要とするか」で決まる。
- owner 操作はバックエンドで毎回検証。フロントの表示制御はただの UX。
- マルチアーキテクチャ:イメージ / バイナリは両ターゲット(aarch64 = 香橙派、
  x86_64 = 将来のホスト)。M3 の GH Actions は buildx で両アーキテクチャを出す。
- **適用済みマイグレーションは不変**(`migrations/*.sql`)。sqlx はファイル全体の
  checksum を取るので、**コメント 1 文字でも変えると本番 DB の記録と不一致**になり、
  server が起動時のマイグレーション検証で落ちて 502 になる(2026-06-24 の本番障害=
  doc 集約の一括置換が適用済みマイグレーションの doc パス注釈を書き換えた)。doc 整理 /
  一括 sed / リネーム sweep は **`migrations/` を必ず除外**する。修正が要るなら既存を編集せず
  **新しいマイグレーションを足す**(やむなく差し戻すときは元の内容へ戻して checksum を一致させる)。
