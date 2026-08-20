# tsubomi 蕾 — 社内 PaaS プラットフォーム

セルフホストの「基礎版 Vercel + Neon」:社内の非エンジニアが AI(CLI)経由で
app をデプロイし、データベース / ボリュームを作る。
単機運用、プラットフォームのプロセスはホスト直走り(docker.sock を保持)。
ホストは今は香橙派(**ARM64**)、後で **x86_64** 機にも移す/増やす ⇒
イメージ・配布物は初日から両アーキテクチャ対応。

## 必読ドキュメント(アーキテクチャを変える前に読む)

設計・調査・障害記録の md は全部 **`doc/`** にある(`CLAUDE.md`・`README.md` だけ根に残す)。
以下のパスはその前提。

**根に `NEXT.md` があれば最初にそれを読む** — 前のセッションからの引き継ぎ(未完の検証 /
先送りした判断 / 検証レシピ)。中身が片付いたら**削除する**一時ファイルなので、無ければ
「引き継ぎ事項なし」の意味。

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
**AI 複測(同日)→ v47 / tbm 1.0.26 で 2 バグ追修**:(bug①)exit code が実際は 3/3 欠落 —
restart policy 下では inspect の State が再起動でリセットされ、速い crash-loop は常に crash-loop
分岐へ。**docker events(die/oom)を第一源**に変更(最初の die が「その退出」の exitCode を保持。
crash-loop 実機で exit=101 捕獲を実証)、inspect は再起動回数と fallback に降格。(bug②)純基底
イメージ(`FROM debian` + CMD のみ)が**再 push でも恒久 manifest unknown** — 假 201 の真因は
registry の `blobdescriptor: inmemory` cache が**別プロセスの blob 掃除を知らない**こと
(distribution 既知欠陥)。純基底の子 manifest は digest が全 build 同一 = 毒 cache に必中。
恒久修正 = **GC 成功後に registry を自動再起動**(`registry::restart_registry`、深夜帯の数秒断は
受容)。教訓:**registry の DELETE を伴う操作は、serving プロセスの cache 失効までがワンセット**。

**第 3 のデプロイ経路(deploy-source)(2026-07-08、server v49 / tbm 1.0.28)**。「ビルド環境が
無いと現成イメージすら部署できない」という §4 闸門の意味論的な穴を塞ぐ:**サーバ側で**既成
イメージを pull(`tbm deploy --image <ref>`)、またはコンテキスト無し Dockerfile(FROM/RUN 等のみ、
**COPY/ADD 不可**)を build(`tbm deploy --dockerfile <path>`)して内部 registry へ push し、既存
`run_digest` パイプラインで起こす — GitHub もユーザ機の docker も不要。app のコードは入れられない
(無 context の契約)ので経路 1/2 の領分は不変で、自帯 DB・valkey・meilisearch 等の**インフラ容器の
最短路**が主用途(§3.2 が `--image` で三条に短縮)。配方(image ref / Dockerfile 全文)は
`service_details.source_kind/source_spec`(migration `20260709000001`)に provenance として残す
(ファイル真源は増やさない。平台私有 DSL ではなく世界共通の Dockerfile 形式 = AI が学習コスト無しで
書ける)。実装:新 `services/source.rs`(検証 = 命令白名単 + COPY/ADD/VOLUME/multi-stage 拒否 +
`# escape=` 非既定拒否 + 内部 registry/loopback 参照拒否 = 越境読み取り防止、合成 git_sha = 純 hex で
CLI の sha 待機と互換、in-memory tar)、`docker.rs`(pull_external / push_to_internal / build_dockerfile =
classic builder・メモリ 1GiB + swap 無効 + CPU 2 コア上限で宿主機保護)、`registry.rs`
(pushed_manifest_digest = push 後に Docker-Content-Digest を取る。bollard 0.21 の push 応答に digest が
無いため)、依存に `tar`。端点 `POST /services/{id}/deploy-source` は **202 即返し**(取得は分単位 =
CF tunnel 100s 対策で非同期 spawn)+ 取得を **1 service 同時 1 デプロイ**に制限(409。Pi 飽和 + 同 tag
競合防止)。取得中の phase='deploying' 起点で起動時 recover_interrupted が中断を回収。品質検証 =
4 simplify agents + codex 深審で真バグ 6 件を出荷前に回収:①digest UPDATE の握り潰しで succeeded なのに
digest='pending' 固着(→ 必錯必閉)②`# escape=` 継続文字改変で COPY すり抜け(→ 拒否 + logical_lines が
コメント行を畳まない)③取得失敗の phase='failed' が並行成功を stomp(→ `WHERE phase='deploying'` 条件化)
④INSERT+配方 UPDATE 非原子(→ 1 tx)⑤内部 registry 参照で跨租户読み取り(→ ホスト拒否)⑥慢 acquire 後に
stop/新デプロイを run_digest が巻き戻す(→ phase スナップショット門)+ 並行同 tag push(→ registry tag を
deploy_id で一意化)。dev e2e 済み(whoami --watch / pgvector stateful 再デプロイでデータ健在 /
alpine --dockerfile / 失敗路径 / 中断恢复 / 409 / private --watch は完走待ち)。

**AI 審査(2026-07-08)→ server v48 / tbm 1.0.27 で 7 項採用**。ソース精読 + 実測部署ベースの
外部審査(R1-R11)から性価比の高い 7 つを実装:(R1)**デプロイ門禁に TCP readiness 探測** —
`docker::wait_tcp_ready` が commit_success 前に container_port の listen を確認(既定 60s、
`TSUBOMI_READY_TIMEOUT_SECS`、0=無効)。company/public のみ(private は listen しない worker を
許容)。「succeeded なのに静默 502」(監听錯 port / 起動数秒後クラッシュ)がここで failed になり
error に次の一手が載る。決定 E の deferred readiness の最小形 = 素の TCP なので stateful も同門・
migration 不要。dev macOS はホスト→bridge IP 不達のため探測スキップ(egress と同じ prod-only 型)。
(R9)**egress を A/B 二重バッファ + 原子切替に** — 旧 flush→refill は途中失敗で DROP 欠けの
まま 30s fail-open。外殻(`TSUBOMI-EGRESS` 等)は内実 `…-A/-B` への jump 1 本だけ持ち、組み立て
失敗 = 切替前 = fail-closed(`egress.rs::swap_refill`、旧レイアウト移行も無窓)。(R3)**traefik
YAML 書き込み点の白名単検証** `route::ensure_yaml_embeddable`(route + registry。上游校験が緩んでも
YAML 注入にしない縦深防御)。(R4)**CPU 硬上限(任意)** — migration `cpu_limit_millis`(NULL=
従来)、`--cpus 0.5` → NanoCpus。web UI は未対応(必要になったら)。(R2)**認証入口レート制限**
(`ratelimit.rs` 固定窓・CF-Connecting-IP 鍵):login/callback/token=30/分、viewer login=10/分
(bcrypt 保護 — M4「後相」の回収)。一般 API には掛けない(AI の正当バーストを殺さない)。
(R7)`tbm service delete --with-repo`(gh で連带削除。省略時は text で残留ヒント)。(R11)
**DR 恢复 runbook** `doc/paas-dr-restore-runbook.md`(管制面 / テナント DB / volume / フル DR の
4 型 + master key を別置きで控える前提 + 年 1 演練表。「復元したことのないバックアップは
バックアップではない」)。**見送り**:R5(GC 中 registry read-only — 日次固定時刻 + 48h 下限 +
再起動で残余は極小)/ R8(ログ歴史基盤 — 独立設計が要る規模、順手では作らない)。
出荷前の品質検証 = 4 simplify agents + codex 深審で真バグ 6 件を回収:①stateful の失敗回滚 /
中断復旧が**走行中の**新コンテナを 10s SIGKILL(→ 30s 猶予 `remove_one`/`remove_others_grace`)
②reconcile 復活が探測で failed 化 → 健全 app の永久静默停止(→ 復活は探測しない。ただし codex
指摘で **M6 callee の private は探測対象**に)③`recover_interrupted` が stateful stop-first の
「旧=停止・新=唯一走行」を stateless 前提で処理(→ keep の走行確認 + 旧再 start)④egress 移行の
残骸掃除が前から削除で東西向丢包窓(→ 末尾逆順 + iptables `-w 5`)⑤`--with-repo` が同名無関係
repo を誤削除し得る(→ `TSUBOMI_SERVICE_ID` variable 照合)⑥限流が IPv6 轮换洪水で無界 / 非 CF
部署で全員単一バケツ / CLI が 429 を server_error 誤判(→ 満杯 fail-closed + XFF 末尾退避 +
`rate_limited` code)。

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

**CPU の見せ方(2026-08-19、server v58 / tbm 1.1.0)**:docker のコンテナ CPU% は **100% = 1 コア**
(`compute_cpu_pct` が online_cpus を掛ける)なので、8 コアの本番機で 4 コア使う app は「400%」と
出て「使いすぎ」と誤読される。**分母は面ごとに違うのが仕様**で、片方に統一はできない —
admin 概要 / ランキングは跨ユーザ比較なので**ホスト全体比**(サーバで正規化 = `cpu_pct_host`)、
service 詳細 / `tbm service metrics` は「天井にどれだけ近いか」が問いなので**その service の
上限**(無ければホスト全体)。後者はサーバが**素材**(docker 生値 `cpu_pct` + `host_cores` +
**適用済み** `cpu_limit_millis`)を渡し天井の選択を客側に委ねる。命名で区別する(`_host` 接尾 =
ホスト全体比 / 接尾なし = docker 生値、`ServiceMetricsDto` だけ = shipped CLI が消費するので改名しない)。
正規化は `cpu_delta/system_delta` にコア数を**掛けないだけ**なので**サンプル内在** = 起動時に
キャッシュしたコア数に依存しない。分母に**DB の設定値を使ってはいけない**:上限変更は次のデプロイ
から効くので、変更直後は「実際は適用済み上限の 100% なのに『上限の 50%』」と嘘になる(未反映は
`mem_limit_pending`/`cpu_limit_pending` が別に言う。判定もサーバ側 — CPU は下記の頭打ちが絡むため)。
**CPU 上限の上界はホストのコア数**(固定 16 を廃止。daemon はコア数超えの NanoCPUs でコンテナ作成を
拒否し、上限は次のデプロイから効くので「設定から遠く離れた場所でデプロイ失敗」になっていた)。
入口検証(`check_cpu_limit_millis`)だけでは旧値・機体移動を塞げないので**施加点でも頭打ち**
(`docker::effective_cpu_millis` = 規則の唯一の家)。ここで弾くと健全な app が全復活経路で永久停止する
(v48 の穴と同型)ため、頭打ち + warn を選んだ(物理最大へ寄せる方向しかないので要求より少ない CPU に
なることは原理上ない)。メモリの上限範囲は**方針値**なので固定のまま(docker は物理超えを拒否しない)。
実装級は **`doc/paas-m4-design.md` §3.3** の分母表。

**subdomain の作成時指定 + 作成後変更(2026-08-19、server v59 / tbm 1.1.1)**:slugify 自動採番
しか無かった subdomain を、①create で任意指定(`--subdomain` / web 詳細設定。指定時は 1 回だけ
insert・使用中 409 = 乱数サフィックスで化けさせない)②作成後変更(`tbm service subdomain` /
web 概要の編集 modal、`POST /services/{id}/subdomain`)の両方に開けた(migration 1 本 =
`subdomain_changed_at`)。検証 `validate_subdomain` は slugify の出力形と一致(性質テストで機械封じ)
+ 予約語に **`tsubomi-` 前綴 + db/cache**を追加(M6 別名が私網の infra/app コンテナ名と docker DNS
衝突する既存の暗穴も同時に塞ぐ — 自動採番ループの skip も同条件・`tsubomi-` base は前綴剥がしで救済・
既存行は起動時 warn。suffix 込み 50 字上限 = 自動採番の出力も validate を通る round-trip)。変更は set_visibility と同じ
「deploy_lock 内 DB 先行 → 現実収束」:route は新 host で `svc-<id>.yml` 原子上書き(旧 URL は
catch-all → 302。**凍結しない** = 解放即再利用可は受容)、M6 別名は `realias_as_callee`
(disconnect → 新別名 connect → `endpoint_has_alias` 閉環確認 = migrate_pgbouncer_aliases のレシピ)。
取りこぼしは reconcile が直す:drift 判定を **(host, backend, ipallow) 三組**に拡張(host を見ないと
変更の書込失敗で新 URL が永久 404)+ `attach_callees` が既接続時に別名検査(**三値** — inspect 失敗 =
触らない、付け替え直前に fresh 再読 = realias との交錯で新別名を剥がす巻き戻り防止)→ 付け替え。
同値変更は冪等(時刻不動)だが**収束段は再実行**(「再実行も可能」を嘘にしない)。caller の
`_URL`/`_HOST` は起動時解決の旧値のまま = **caller 再デプロイまで断線**し得るのは背骨どおりで、
`list_injections` の GREATEST に `subdomain_changed_at` を足し(cache rotate と同型)未反映バッジが
零改修で点く(同値変更は時刻を動かさない = 偽未反映なし)。受容:GitHub repo 名は旧名のまま
(`--with-repo` は現 subdomain 名で探すため当たらない。**ただし repo 削除は best-effort** =
service の削除は成功し `tbm` は 0 で終わる。`TSUBOMI_SERVICE_ID` 照合で誤削除はしない)・既存注入の
env 名は不変(値だけ新しくなる)。実装級・受容表は **`doc/paas-service-subdomain-design.md`**。

**subdomain 後の追加(マイルストーン外):改名の影響名単(2026-08-20、tbm 1.1.4。server は次版 — **まだ ship していない**)**。
改名の案内が「caller が 1 件も無くても無条件に脅し文を出し、居るときも誰なのか言わない」形だった
のを、実際の呼び出し側を出すようにした(読み取りのみ・migration なし)。`GET /services/{id}/callers`。
**逆引きの述語の家は `inject.rs`**(`injections` の正向解決の対偶)で、`network::service_callers`
(網操作。id だけ要る)はその薄い投影 — 別々の SQL にすると「名単に出た集合」と
「`realias_as_callee` が実際に触る集合」がドリフトし**プレビューが嘘になる**。SQL は
`GROUP BY` を使わず `resources` 主表 + `EXISTS` + `ARRAY(SELECT …)` にした:「1 行 1 caller」が
構造的に自明になる(行が増えると網操作が 2 回走り、連帯再デプロイは同じ service を 2 度デプロイ)+
`array_agg` の NULL(= `Vec<String>` の decode panic 経路)が原理的に消える。**caller の所有者では
絞らない** — 絞ると跨 owner の注入が生まれた日に**この端点だけが realias より少なく見せる** =
影響範囲が嘘になるので、担保は注入作成時に置く(`is_linked_callee` が同じ述語の 3 つめの写しで、
readiness 門禁を重くしないため軽い EXISTS のまま = 定義を変えるときは両方直す)。入口:web 概要の
常設「呼び出し側」セクション(0 件ならセクションごと出さない)+ 変更 modal の案内を
「無条件の 2 文」と「caller が居るときだけの名単」に分割 + `tbm service callers`(**改名する前に**
影響範囲を引ける = AI 向け)。バッジの色語彙は `phase-badge.tsx` の `Badge`(tone)に集約
(直書きの琥珀が「未反映(要デプロイ)」と同色で、停止中の caller が未反映に見えていた)。
**併せて修正**:`DatabaseOverview` の rotate 文案「注入済みのサービスは再デプロイするまで古い
文字列のまま」は**嘘**(db rotate は human role だけを回し、注入されるのは app role。だから
`list_injections` の GREATEST に `database_details.rotated_at` を入れていない)。出所が設計 doc
だったので `paas-tech-design.md` / `paas-m5-design.md` の根も直した — 直さないと次に rotate 文案を
書く人が再生産する。品質検証 = 設計時に Plan agent 対抗審査で P0 4 件(うち 2 件は既存バグ。
いずれも状態を変える次スライス側の穴)+ 実装後に 4 simplify agents(最大の指摘 =「判定一族に
このスライスの消費者が居ない」→ 次スライスへ移送)+ codex ultra で**真バグ 4 件**を出荷前回収。
**その 4 件の教訓が一般形として効く**:①**無条件の警告を条件付きにすると、条件が「未知」の
ときに旧実装より警告が減る** — `callers === undefined`(取得前 / 500 / 再取得中)を 0 件扱いに
すると実リンクがあるのに警告なしで改名が通る(取得中は待たせ、失敗は「確認できませんでした」と
言い、modal を開いた瞬間に refetch)②**注入の作成 / 削除は注入元(callee)の逆引き名単も変える**
ので `serviceKeys.all` を落とす(狭いキーだけでは callee 側が古い `[]` を staleTime 分使う)
③**「注入関係」を「今切れているリンク」と断定しない** — 停止中 / 未デプロイの caller には凍結
env も生きたリンクも無く、そこで再デプロイを促すと `commit_success` が `desired_state='running'`
を書いて**ユーザが止めた service を起こす**(断定は稼働中の相手だけ。同値の再実行では
`resolve_service_row` で改名前の値を持って何も言わない)④`env_vars` は `injections.env_var` の
**保存名だけ**で派生 `_HOST`/`_PORT` は含まない(文言は「注入名」)。実装級は
**`doc/paas-service-subdomain-design.md` §6**。

**影響名単の次(同日):連帯再デプロイ(tbm 1.1.4、migration 1 本。server は次版 — **まだ ship していない**)**。名単を出す
だけでなく、**その相手を今の版のまま再デプロイして注入値を追従させる** opt-in の一発
(`POST /services/{id}/redeploy-callers` + `tbm service redeploy-callers` + `service subdomain
--redeploy-callers` + web modal の既定チェック済み checkbox)。背骨は不変(値は起動の瞬間に解決)、
変えるのは「その再デプロイを誰が押すか」だけ。**静默の自動連鎖にはしない**。
**前提として既存バグを 1 本潰した**:`redeploy` は deploys 行を `received` で作ってから
deploy_lock を待つので、その窓でプロセスが落ちた行は永久に残り(phase はまだ 'deploying' でないので
`recover_interrupted` の候補集に入らず、gc は terminal 行しか消さない)**`deploy_source` の入場門が
永久 409** = `tbm deploy --image` が使えなくなっていた ⇒ 起動時に非 terminal 行を全部閉じる
(起動直後の非 terminal は定義上すべて孤児)。**新契機 `DeployTrigger::CallerRelink`** は
4 つの次元を `impl DeployTrigger` の**具名述語**に集めた(呼び出し点に `trigger ==` を散らすと、
契機を足した日に「なぜ Reconcile は phase を落とすのか」が**答えではなく遺漏**として残る):
再確認する(停止済み caller を叩き起こさない — `commit_success` が desired を running に戻すので
ここが唯一の防壁)/ 探測しない・失敗で phase を落とさない(対象は元々健全な service。failed に
すると `converge_running` の候補集から外れ**自愈網から除名** = v48 と同型)/ 現役 digest 必須
(ロック待ち中に caller が新版を出していたら静默ロールバックしない)。**「failed にしない」だけでは
足りない**のが実測の発見 — `run_digest` が開始時に書いた `'deploying'` で**固着**して同じ害になる
ので、失敗時は**門で読んだ phase の値**へ戻す(リテラルを書かない)。しかもその UPDATE は
`phase='deploying'` だけでは**自分が書いていない marker を消す**(`deploy_source` は取得開始時に
**deploy_lock の外で** phase を立てる)⇒ 自分以外の非 terminal 行が無いことを条件に足す。
**入場制限は実行枠そのもの**を handler で `try_lock_owned`(取れなければ 409、guard は spawn へ
move して Drop 解放)— 当初の「per-callee 集合 + spawn 内で枠 acquire」は**枠待ちのバッチに
202「開始しました」を返す = 応答が嘘**になっていた(審査 3 本の共通指摘)。対象ゼロなら spawn
しない(幽霊 409 と空 audit を作らない)。`deploy_lock` の流用は却下(fan-out は分単位で、その間
stop/delete/visibility/改名が固まる)。**旧債の返済**:`deploys.trigger`(migration。回填値 =
新規行の DEFAULT `'user'`、既存行の読み戻しまで検証)— `redeploy` は再生する版の commit_message を
そのまま書くので、平台が自動で起こした行がユーザ自身の再デプロイと**見分けが付かなかった**
(同 digest の行が全部「稼働中」に見えた 2026-07-26 の web 事故と同じ根)。CLI 履歴と web が
`reconcile` / `caller_relink` にだけラベルを出す。**教訓の一般形**:①**無条件の警告を条件付きに
すると、条件が「未知」のときに旧実装より弱くなる** ②**「失敗しても壊さない」は「状態を変えない」
ではなく「入口の状態へ戻す」** ③**lock の外で書かれる状態は、lock 内からでも所有権を持たない**
④判定はサーバの純関数を単一真源にし**クライアントで再導出しない**。品質検証 = 設計時の対抗審査
P0 4 件 + 実装後 4 simplify + codex ultra(codex は額度切れで最終報告前に停止 —
**再走が未完なので次に触るときに一度通す**)。実装級は **§7**。

**AI フィードバック第 4 弾(2026-07-26、server v53 / tbm 1.0.32):`sslmode` の駆動系差を仕組みで消す**。
発端は「注入された `DATABASE_URL` が Go では繋がるのに Node で落ちる」。**`sslmode=require` の意味が
駆動系で割れている**(libpq = 暗号化するが証書を検証しない / node-postgres = **厳格に検証**)ため、
利用側は「URL から sslmode を削って `ssl:{rejectUnauthorized:false}` を渡す」という文書必読の回避を
強いられていた。**真因は自己署名ではなく hostname mismatch**(本番 pgbouncer は acme.sh が置いた LE 証書
`CN=db.tsubomi-app.com` を出すのに、注入ホストは容器名 `tsubomi-pgbouncer`)— 実機の
`openssl s_client` で確定。
**不変式「注入ホスト名 = pgbouncer 証書の公開名」**を立て、その名前を **per-service 私網の docker 網別名**
として pgbouncer に付ける(`network.rs::pgbouncer_aliases`。公網 DNS は docker 内蔵 DNS が遮蔽 = 通信は
網内のまま)。**別名を付ける場所が要点** — テナント容器は M6 網隔離で私網にしか居らず `tsubomi-edge` は
残骸なので、compose で edge に付けても**見えない**(最初これで踏んだ。codex 相当の審査で捕獲)。別名は初回
connect でしか付かないため、既存私網には起動時に `migrate_pgbouncer_aliases` が後付けする(disconnect →
別名付き reconnect)。`verify-full` へ上げる案は却下(libpq に `sslrootcert=system` が要り、それが今度は
Node で壊れる = 非互換を移すだけ)。**引き受けたコスト:LE 証書が全テナント app の生命線**になった
(以前は検証されないので切れても内部注入は動いた)⇒ 更新 hook の正本を `deploy/db-public/reload-pgb-cert.sh`
に留め置き、DR runbook に **§E(証書失効)**と前提・演練を追加。**分離**:`db_internal_host` は「証書の身元」
であって配管先ではないので、公開 DB の traefik 後端(`ipblock.rs`)は**容器名**で引く(公開名を書くと引けない
瞬間に traefik が自分へ転送する自環)。値は**ホスト名のみ**を起動時検証(port 混入で全テナントの URL が壊れる)。
併せて **database 注入に素材 env** を追加(`_HOST`/`_PORT`/`_USER`/`_PASSWORD`/`_NAME`/`_SSLMODE` = M6 の
`_HOST`/`_PORT` の一般化。URL を受け取らない ORM 用)。派生名の単一真源は `inject::derived_env_keys`(衝突検査と
web の由来ラベルが同じ関数を引く = 手書きの対応表を廃止)。後缀が 6 本に増え `DATABASE_HOST` 等の**ありふれた
名前**に当たるので、**注入同士の衝突は create 時に 400**(基底が同じ `X`/`X_URL` は「URL は A・パスワードは B」の
静かな取り違えになる。検査と INSERT は同一 tx + 行ロックで TOCTOU を塞ぐ)/ **静的 env とは静的が勝つ**
(派生は便利品なので後勝ちを止めた — 既存 app を次の deploy で黙って別 DB へ繋ぎ替えない。譲った名前は
警告で列挙し、web も応答 body を捨てずに表示する)。裸 `_PASSWORD` が
`env/resolved` に平文で出る穴も同時に塞いだ(旧マスクは URL 形しか見ていなかった)。skill §3.1 は
**`DATABASE_HOST` を見て分岐する**形に書き換え(dev / 旧部署は容器名のままなので無条件断言にしない)。
実装級・却下理由・受容は **`doc/paas-db-public-design.md`「証書名は仕組みの一部」**(正本)+ m3 設計 §11 決定 A'。
**続き(同日):注入の未反映を可視化 + CLI QoL(server 同版 / tbm 1.0.32)**。坑 2「注入は部署の前」への
回答:migration `injections.created_at` を足し、**今 serving している deploy 行の作成時刻より後に
作られた注入**を `InjectionDto.needs_redeploy` で出す(走行中フラグの流用ではないので**部署前から在る
注入は「反映済み」のまま**)。基準に `finished_at` を使ってはいけない — commit_success は readiness 探測
(既定 60s)の後なので、「**デプロイ中に注入した**」ケースで未反映が反映済みに**反転する**(この機能が
最も要る場面で裏返る = 見逃し。simplify/codex review で発見)。行の作成時刻は env 解決より必ず前なので
過剰警告側に倒れる。**cache rotate も未反映として拾う**(注入値そのものが変わるため)が、**db rotate は
human role だけ**を回し app role は不変なので対象にしない。入口 = `tbm inject` の強い文案 / `service status` の
`[未反映:要デプロイ]` / web のバッジ / json。skill に §「順序:注入 → デプロイ」。
CLI 側:create の text 回显に port/visibility/stateful/memory + **推導が起きた時だけ**その理由
(`--port` が visibility を動かす「隔空作用」を黙って起こさない)/ deploy の skill 案内に **24h
クールダウン**(毎回出すと読んだ後は雑音で本当の警告が埋もれる)/ 利用者向け英文文案を日本語へ統一。
**作成後に変えられるのは visibility だけ**(memory/cpus の変更端点は無い — skill に嘘を書いていたので訂正)。

**同日の本番事故 → 恒久修正**:上記 migration が既存行を **`'-infinity'`** で回填したが、Postgres の
infinity は `DateTime<Utc>` に読み込めず sqlx が「`NaiveDateTime + TimeDelta` overflowed」で **panic**、
本番の `GET /services/:id/injections`(= `tbm service status` / web の env タブ)が全部落ちた。
**dev で露出しなかった理由**:dev の injections 表は空で新規行は `now()`、`-infinity` を**読み戻す経路が
一度も走らなかった** — **migration の回填値は「新規行での動作確認」では検証できない**(既存行を作って
読み直すまでが検証)。修正は 2 段:新 migration で epoch へ寄せる(適用済みファイルは不変)+ SQL 側で
`GREATEST(created_at,'epoch')` に丸めて**二度と端点を落とさない**。
教訓の一般形:**既存データだけが踏む穴は、新規作成の e2e では見えない**。
併せて web:同じイメージを 2 回デプロイすると同 digest の行が複数でき**その全部が「稼働中」に見えて**
いたので、「digest が一致する最初の成功行」1 件に絞り、バッジを左のステータス側へ移した
(右に置くとバッジの幅の分だけボタンが行ごとにずれて履歴が不揃いに見える — ユーザ報告)。

**db fork(2026-08-03、tbm 1.0.34):database 複製 — 「基礎版 Neon」の看板能力**。dev/検証環境用に
「この瞬間の構造 + データごと」の複製を一動詞で:`tbm db fork <元> <新名> [--schema-only]` + web 概要の
「複製」セクション。**同期は作らない(恒久)** — fork 後は分道揚镳が仕様(データ向下 = 再 fork /
構造向上 = app 自身の migration というユーザ自留地)。採用筛子は「**只有平台能做的,才值得平台做**」
(CREATEDB は tenant-admin 権限 = ユーザ容器で代替不能。cron 案はこの筛子で却下 — crond 容器で足りる)。
実装:同期 201・migration ゼロ、`pg_dump 元 | psql 新` の**パイプ直結**(TEMPLATE はテンプレート元に
接続 1 本で失敗 = pgbouncer 常時接続の本番で不成立。パイプは中間ファイル無し + dump/restore 並走)、
新 DB は完全な新規資源(新 wire 名 + 新 role 3 本 + 新パスワード — 資格情報を元と共有しない)。開通は
`databases.rs::provision_database`(create と fork の共有骨格:tenant DDL →[流し込み]→ platform 行、
失敗はどこでも `drop_database_and_roles` 1 手)。**タイムアウト(`TSUBOMI_FORK_TIMEOUT_SECS` 既定 300)は
流し込み段だけに掛ける** — commit まで包むと「期限が commit 直後に切れ platform 行だけ残る」不変式破りが
起き得る(4 simplify agents の審査で捕獲。併せて `kill_on_drop` + pg_dump `--lock-wait-timeout` で
期限切れは実際に止まる、ハンドラは spawn 包裹 = CF Tunnel ~100s 切断でも完走)。**codex 深審の主捕獲:
流し込み psql を admin + SET ROLE でなく新 DB の app role 接続に**(dump 内容はユーザ制御 —
CHECK 制約の関数等から `RESET ROLE` で superuser に戻れた = 跨租户。app なら session_user が無特権。
**trash 復元も同穴だったので同時修正** — fork が既存の穴を炙り出した)。残骸可視化:起動時に
platform 行の無い `db_*` を warn(`log_orphan_tenant_dbs`、spawn 化で起動非阻塞・自動削除はしない)。
実装級・受容 6 項は **`doc/paas-db-fork-design.md`**。

**AI フィードバック第 5 弾(2026-08-13、server v54 / tbm 1.0.35):「機制優先」の刚性緩和 4 点**。
外部 AI の試用フィードバック 4 条への回答。方針 =「skill で AI を引導するのではなく CLI/server に
固定する」(引導は速度・精度・コストで負ける — ユーザ定調)。(S1)**ゴミ箱は名前を占有しない**:
表級 `UNIQUE(user_id,kind,display_name)` → 活体のみの部分ユニーク index(migration `20260813000001`)。
delete → 同名 create が purge 不要で通る。restore は活体衝突を物理復元の**前**に 409 + TOCTOU は
map_unique + **物理復元の巻き戻し**(undo_restore — 旧 DB 露出 / volume 孤児化を防ぐ)。restore/purge/gc
は per-id ロック(deploy_lock 流用)で直列化 — GC が読んだ候補の復元後破壊を封殺。db restore は
drop-first で再試行安全に。CLI trash は同名堆積を id(完全 UUID = id 語義 / 8 桁以上 hex 前方一致 /
両方該当は曖昧エラー)で消歧。anon_seq の UNIQUE は**据え置き**(採番が全行を見る前提)。
(S2)`tbm service rename`(PATCH /services/{id}。subdomain/URL/GitHub repo 不変 — db rename と同型)。
(S3)`tbm service limits`(memory/cpus。**次のデプロイから反映** — run_digest が毎回 DB 直読)+
`tbm service stateful`(false→true 単方向 §10-D。true→false は双開方向なので入口なし)。作成後不変は
**port だけ**に縮小(§0-D は不変)。web 概要に上限表示 + 変更 UI(lost update は seed 快照 diff で防止)。
(S4)**private の verify = 内網 TCP 探活**:`GET /services/{id}/probe`(単発 connect、全 attach 網、
非 Linux は「探せない」と正直に返す)+ is_callee(**生きた** caller の注入だけ数える)。CLI の判定は
三値(`ok: true/false/null` — listen しない worker は罰しない。自動分岐は json の ok を読む)+ serving
照合材料付き。(S5)**MSYS パス化けの確定的復元**:volume 遠端パス / `--mount` は EXEPATH 前綴一致で
CLI が自動復元(遠端パスはドライブレターを持ち得ない = 無歧義。ローカルパスは MSYS 変換の恩恵側なので
不変)。(S6)**create の json 既定 = GitHub 編排**(秘密 stdin 直達 = 転録に残らない。旧摊平 DTO は
`--no-github` に退避 — 明示 opt-in でのみ秘密が stdout に出る)。付随修理(codex 3 輪 + simplify 5 輪、
真バグ 10+ 件):stop-first が RESTARTING を停止対象に(crash-loop 旧容器の双開)/ stateful 退路は
「直近成功容器 1 つ + 新の不走行確認」の二重門 / deploy --watch は解決済み UUID を持ち回り rename に
耐性(名前再解決を廃止。resolve への UUID 直通は「B の名前 = A の id」誤配送になるので不採用)/
web は Outlet を id で key(同 route 遷移で modal・秘密・編集状態が別リソースへ持ち越される事故を
一括封殺)。教訓:**「作成後変更可」を足したら、その不可能性を前提にした文案・skill・退路コードを
全部掃く**(3 箇所の「visibility だけ」文案が審査 3 本全部に刺された)。**同日追補**:web 引導頁の
Windows 既定を cmd に(社給 PC の PowerShell 制限)+ install.bat を日本語化 — リポジトリ実体は
UTF-8、配信時に CP932 へ転碼(cli_release.rs。cmd は OEM コードページでバッチを解釈するため)。
エンコーディング契約は install.bat 頭注釈が正本:日本語は echo/REM のみ・**括弧は必ず全角**
(半角 ) は if ブロックを早期閉鎖し後続が無条件実行される — 実機で「横幅だけ出して静默終了」を
起こした)・CP932 非対応文字禁止(encoding_rs が `&#N;` に置換し `&` が命令区切りになる)。
配信側は unmappable を tracing::error で見張る。旧版互換は全撤去(ユーザ定調:利用者が付くまで
互換は負債)。

**subdomain 後の追加(マイルストーン外):service アクセス統計(2026-08-20、server v60 / tbm 1.1.2)**。
Vercel 風の per-service 統計を**平台自身の機能**として(CF 非依存・ユーザ app 無改変・請求路径零遅延)。
背骨:traefik access log(stdout JSON。compose は **base + 3 overlay + dev infra の 5 箇所** —
overlay の command は**全置換**なので再掲必須、欠けると統計が静かにゼロ)→ `stats.rs` の tailer
(bollard logs follow + timestamps、offset は platform_config `stats_tail_since`・INSERT 成功後にのみ前進 =
欠落より重複、16KiB partial frame は \n まで再構成、満杯 flush 失敗は bail → backoff 再読、境界 dedup は
「探索中のみ ts < saved」)→ `request_events`(migration `20260819000002`、FK CASCADE = purge 自動連鎖、
BRIN(ts)、保留 `TSUBOMI_STATS_RETENTION_DAYS` 既定 30・起動時 1〜3650 強制)→ **クエリ時集計**
(事前集計なし。単一 REPEATABLE READ RO tx、`date_trunc(…,'UTC')`、窓は interval 境界揃えで DTO が
from/to を返し web の 0 埋めが従う、days 超過は 400 でなく実効値へ丸め)。**IP は保存しない**
(visitor = sha256(UTC日付+IP+UA) 先頭 16B の日次ローテ匿名 hash、bot は UA 分類で訪客から除外)。
**実 IP は `TSUBOMI_STATS_IP_SOURCE` で明示分岐**(cf=Cf-Connecting-Ip / peer=ClientAddr のみ。
「ヘッダがあれば使う」自動判定は偽装口なので無い — 直 VPS 部署は peer、.env.example / m3 §13.B)。
UA 解析 = woothee(device/browser/os)。入口:web 詳細「統計」タブ(visx = 初のチャート、期間
24h/7日/30日)+ `GET /services/{id}/stats`(1 端点 1 応答)+ `tbm service stats`。口径はリクエスト
(pageview ではない)、M6 内链・private・探活は構造的に不計上。router 名の書式と逆関数は
`route.rs::{router_name,parse_router_name}` に同居。既存機の初回上線のみ traefik 明示再作成が要る
(`just ship` は traefik を再作成しない)。品質検証 = 4 simplify + codex ultra で真バグ 10 件を
出荷前回収。実装級・受容・地雷は **`doc/paas-service-stats-design.md`**。

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
- **マイグレーションの回填値は「新規行の DEFAULT と同じ値」にする**。違う値(センチネル)を使うときは、
  その値が **Rust の受け側の型で読み戻せることを既存行で確認する**まで完了ではない(2026-07-26 の事故:
  `-infinity` を回填 → sqlx が `DateTime<Utc>` に読めず panic。dev は既存行が無いので**新規作成の e2e では
  一度も踏まなかった**)。一般形:**既存データだけが踏む穴は、新規作成の検証では見えない**。
  時刻列は `20260727000001` の CHECK(有限値)で書き込み側も塞いである。
- **panic はそのリクエストに閉じ込める**:`panic = "abort"` は**外した**(2026-07-26)+ router に
  `CatchPanicLayer`。以前は abort だったので、ハンドラ 1 つの panic が**プロセスごと**落とし、
  進行中の他人のデプロイが失敗扱いになり WS も切れていた(実際に sqlx の decode panic で発生)。
  今は 500 に閉じ込まる(panic はログに残る)。代償は unwind テーブル分のバイナリ増
  (配布する tbm も同じ profile:実測 1.80MB → 2.14MB / +19%)— 管制面は他人の app を預かる側
  なので、この交換を選んだ。それでもリクエスト経路で panic し得るコードは書かない(500 は 500)。
- **適用済みマイグレーションは不変**(`migrations/*.sql`)。sqlx はファイル全体の
  checksum を取るので、**コメント 1 文字でも変えると本番 DB の記録と不一致**になり、
  server が起動時のマイグレーション検証で落ちて 502 になる(2026-06-24 の本番障害=
  doc 集約の一括置換が適用済みマイグレーションの doc パス注釈を書き換えた)。doc 整理 /
  一括 sed / リネーム sweep は **`migrations/` を必ず除外**する。修正が要るなら既存を編集せず
  **新しいマイグレーションを足す**(やむなく差し戻すときは元の内容へ戻して checksum を一致させる)。
