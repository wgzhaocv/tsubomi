# tsubomi PaaS — service 公開範囲(visibility)実装設計

> service↔service 内部リンク(`doc/paas-service-link-design.md`)に続く**マイルストーン外の追加機能**
> (背骨は変えない・新表なし・列を 1 本足すだけ)。
>
> 解く問題:今はどの service にも必ず公開 URL `https://<sub>.<domain>` が生える(会社 IP 許可リスト付き)。
> しかし監視・通知系の worker は **HTTP 入口そのものが不要**で、URL が在ること自体が余計な露出になる。
> 逆に、当初 M3 の DDL に居て一度も配線されず drop した `service_details.public` 列
> (migration 20260622。意図 =「ipAllowList を app 単位で豁免する」)が示すとおり、
> **インターネット全体に公開したい**需要も設計当時から在った。この 2 つは同じ軸の両端なので、三態 1 列に畳む。
>
> 背骨を一言で:**`svc-<id>.yml`(traefik route ファイル)は DB の期望状態から生成される** — だから
> 「公開範囲」は列を 1 本足して「書く/書かない/middleware を掛けない」を分岐するだけで実現でき、
> **切替は即時**(ファイル再生成のみ。env 注入と違いコンテナ起動時に固化される値ではない = 再デプロイ不要)。
>
> 完了判定:香橙派(prod)で public=社外 IP から開く / company=社内のみ(回帰)/ private=どの IP でも
> 302 `/noservice`、かつ **private callee への M6 内部リンクが生きている**(本機能の主用途)。
> dev は catchall も IP 許可も無いので、活体判定は prod でのみ意味を持つ(§11)。

---

## 0. スコープと確定事項

出すもの:

- **`service_details.visibility` 三態**:`private`(route ファイル無し。subdomain は DB に温存、
  外部からのアクセスは既存 catch-all → 302 `/noservice`)/ `company`(現状 = 既定。route + `tsubomi-ipallow@file`)/
  `public`(route はあるが ipallow middleware を掛けない = どこからでも到達可能)。
- **切替入口**:`POST /api/services/{id}/visibility`(web / CLI 同一ハンドラ)+ `tbm service visibility`
  + web 概要ページの Radio 3 択。即時反映・audit 記録。
- **収束**:deploy 切替点と reconcile が visibility を尊重。reconcile の drift 判定は
  `(backend 容器, ipallow 有無)` の組に拡張(§6)。
- **M6 リンクの解耦**:callee の serving 容器解決を route ファイル依存から DB 由来へ(§5。
  private callee へのリンクを成立させるための前提修理)。

出さないもの:

- **部分公開**(per-path 公開、Basic 認証、トークン付き URL 等)。三態で足りない要件が出たら別設計(§10-A)。
- **admin(owner ガバナンス)面への表示・操作**。visibility はユーザ自身の資源操作(§10-B)。
- **リリース作業**(skill 文書は CLI に焼き込まれるため、反映は次回 version bump に同乗。本書のスコープ外)。

確定する細部(各々**否決可** — 第 4 層 §0 の作法)。コードに落ちていない穴を埋める:

- **§0-A 三態 1 列**:`visibility TEXT NOT NULL DEFAULT 'company' CHECK (IN ('private','company','public'))`。
  新表なし。既存行は DEFAULT で全部 `company` = 挙動不変。boolean 2 本(ingress 有無 × ipallow 豁免)は
  「無 ingress × 豁免」という無意味な組合せを生むので採らない。
- **§0-B private の意味論 = 「route ファイルが存在しない」**。subdomain は解放しない(URL 文字列は
  DTO に残り続ける — 再公開したとき同じ URL で復活するのが要点)。外部からは catch-all(priority 1)に
  落ちて 302 `/noservice` = 「未デプロイの子域」と区別が付かない見え方(意図どおりの不可視)。
- **§0-C public = ipallow middleware を掛けない、それだけ**。entrypoint / tls / certResolver は不変。
  設定は**本人裁量 + audit 兜底**(owner 限定にしない):平台哲学「能力は平台が提供し、使うかは
  ユーザ裁量」どおり。自分の app をインターネット全体に出すのは自分の爆発半径内。監査は `service.visibility` で残る。
- **§0-D 切替は即時**:route ファイルは DB から再生成できるので、toggle ハンドラが lock 内で
  書換え/削除する(再デプロイ不要)。rotate(値がコンテナに固化される)と作法が違うことを明示する。
- **§0-E serving 容器の真源は DB**:`container_name(id, latest_succeeded_deploy_id)` + 実走確認
  (reconcile が既にこの規約)。`attach_callees` もこれに揃え、route ファイル(private では不存在)を
  読む経路を廃す。
- **§0-F middleware ドリフトは受容しない**:`public→company` の切替でファイル書込だけ失敗すると
  「DB は社内限定・現実は一般公開」が黙って残る(fail-open のセキュリティドリフト)。reconcile の
  drift 判定を `(backend, ipallow)` の組にして ≤30s で収束させる。受容するのは「≤30s の窓」だけ(§9)。

---

## 1. 着工順序(3 スライス + 文書)

| # | スライス | 範囲 | 検証 |
|---|---|---|---|
| **S1 文書** | 本書 + CLAUDE.md に一段 | — |
| **S2 後端の芯** | migration + DTO + `route.rs`(ipallow 引数 / pure builder / parser)+ `deploy.rs` 分岐 + `serving_container` 昇格 + `attach_callees` 切替 + `reconcile.rs` 3 値収束 + 単体 | 既存行は全部 company 既定・値を変える API が無い = **外部挙動完全不変**の安全な中間着地。`just check` + 単体緑 |
| **S3 切替入口** | shared Req + `set_visibility` ハンドラ + audit + CLI(visibility / status / verify 短絡) | dev で toggle → `traefik-dynamic/` のファイル生滅と middleware 行を grep で確認 |
| **S4 入口(web)+ skill** | 概要ページ Radio + バナー灰化 + skill 文書 | web 目視 + dev e2e 一式(§11) |

---

## 2. migration(`migrations/20260702000001_service_visibility.sql`)

```sql
ALTER TABLE service_details
  ADD COLUMN visibility TEXT NOT NULL DEFAULT 'company'
  CHECK (visibility IN ('private','company','public'));
```

- 旧 `public` 列は 20260622 で drop 済み — 追加のみ。
- コメントは自己完結にし **doc パスを書かない**(適用済み migration 不変の一線。doc リネーム sweep が
  触れない形にしておく — 2026-06-24 障害の教訓)。

---

## 3. route.rs — 「言われた通りに書く/消す純粋な書き手」を保つ

`route::ensure(desired)` 型の汎用収束関数は**作らない**:3 つの呼び出し点(deploy / reconcile / toggle)は
backend の解決方法も失敗時の扱い(旧温存 / log / 5xx)も違い、単一関数に畳むと分岐が中へ移るだけ。
共有すべきは「serving 容器の解決」であり、それは §4 のヘルパが担う。

- `write(state, id, subdomain, container, port, ipallow: bool)`:`ipallow=false` のとき
  `middlewares: ["tsubomi-ipallow@file"]` 行を丸ごと出さない(company と public の差はこの 1 行だけ。
  空許可リストは既に fail-open なので ipblock 側は無改修)。
- 内部を pure `build_service_doc(name, host, backend, ipallow, tls) -> String` に抽出
  (`build_catchall_doc` と同型 = 単体テスト可能)。
- `parse_ipallow(content) -> bool` + `has_ipallow(state, id) -> Option<bool>`(ファイル無し = None)を追加
  (`parse_backend_container` と対。write のフォーマットに密結合 — ズレたらテストが落ちる構造も同じ)。

---

## 4. serving 容器ヘルパの昇格(mod.rs)

`reconcile::expected_running_container`(直近成功 deploy の容器名が実走中の時だけ Some)を
`services/mod.rs` へ移設し、docker 照会込みの糖衣を足す:

```rust
pub(crate) async fn expected_running_container(state, id, running_names: &[String]) -> Option<String>
pub(crate) async fn serving_container(state, id) -> Option<String>   // presence を引いてから判定
```

reconcile は presence を既に手に持つので移設版を直接呼ぶ(docker 照会を二重にしない)。
`attach_callees` と toggle ハンドラは糖衣を使う。あわせて `Visibility` enum を mod.rs に置く
(`parse`(API 400 検証)+ `ipallow()`。DB の CHECK と対を成す単一真源):

```rust
pub(crate) enum Visibility { Private, Company, Public }
```

---

## 5. attach_callees の解耦(network.rs — M6 リンク × private の前提修理)

現行は `route::backend_container(callee_id)`(= route ファイルの解析)で callee の serving 容器を
解決している。private callee はファイルが無いので **skip されリンクが繋がらない** — 監視系の内部 API
という本機能の主用途がまさに壊れる。変更は 1 点:

```rust
route::backend_container(state, callee_id)  →  serving_container(state, callee_id).await
```

- 切替点が「route 書込時」から「commit_success(DB)時」へ僅かに前倒しになるが、commit_success 時点で
  新容器は is_live 確認済み = 窓中に attach しても生きた新版を掴む。connect は加算的で旧 endpoint は
  旧容器削除で自然消滅 → A→B 断は生じない(§9 受容)。
- 系として、company/public の route 書込が**失敗**した窓(旧 route 温存・reconcile が ≤30s で新へ収束)では
  **内部リンクが公開より先に新版を指し得る**。双方 commit 済み・存活確認済みの健全版で収束方向も同一なので
  受容する(§9。visibility 別に解決経路を二重化する代替は route ファイル依存の復活 = 本末転倒なので採らない
  — codex 監査 2026-07-02)。
- callee 側の即時切替は従来どおり `attach_as_callee`(deploy 切替点)。private でも呼ぶ(§6)。
- `detach_callee` は `running_container_name` のままで正しい(無改修)。

---

## 6. deploy / reconcile の 3 値収束

**deploy.rs(run_digest_inner)**:spec の SELECT に `visibility` を足し、route 切替点を分岐:

- `private`:`route::write` の代わりに `route::remove`(冪等。旧 visibility の残骸掃き)。
  **`attach_as_callee` + `remove_others` は必ず進める**(内部の切替点は commit_success)。
  `remove` 失敗時**も**旧掃除を進めるのは意図した **fail-closed**:陳腐ファイルは消えた backend を指し
  外部は最悪 502(= 内容不可達)で、旧掃除を止めて旧版が外部に**公開され続ける**方より安全側。error で記録し、
  reconcile の private 分岐が ≤30s でファイルを回収して /noservice に収束する(codex 監査 2026-07-02)。
- `company` / `public`:従来どおり `route::write(.., ipallow)`。Ok → attach + 旧掃除 / Err → 旧温存。

**reconcile.rs(converge_running)**:候補 SELECT に `visibility` を追加し、

- `private`:期望状態 = 「ファイル無し」。ファイルが在れば deploy_lock → fresh 再確認(visibility と
  ファイル存在)→ `route::remove` + audit(`route_drift`)。**回収はコンテナ状態と無関係に、存在収束
  (redeploy)より先に行う** — 後回しにすると「コンテナ消失 → 復活 redeploy 失敗 → phase=failed で
  候補外」の経路で陳腐ファイルが永久残留する(converge は phase=running だけを見るため — codex 監査
  2026-07-02 第 2 回)。容器消失→復活パス(redeploy)自体は deploy.rs の分岐が visibility を読むので
  無改修で正しい。
- `company` / `public`:drift 条件 = backend 不一致 **OR** `has_ipallow(..) != Some(期望 ipallow)`。
  修正は lock + fresh 再確認 → `route::write(.., ipallow)`。これで §0-F(fail-open ドリフト排除)が成る。
  **lock 取得後の fresh 再確認は 4 点セット**(visibility / 期望 backend / ファイル存在 / ipallow 有無)—
  取得待ちの間に toggle や deploy が走った可能性があるため、どれか一つでも古い値で書くと陳腐な
  flavor の route を書き戻す(codex 監査 2026-07-02)。

**recover_interrupted(起動時の中断デプロイ収束)に 1 分岐追加**:`desired='running'` の維持パスでも
`visibility='private'` なら `route::remove` を呼ぶ。private の deploy が commit 前後どちらで中断しても
陳腐 route ファイルが残り得るが、これを周期 converge の実行順序に依存させず起動時に確実に掃く
(codex 監査 2026-07-02)。stop / restore / trash / purge は無改修(stop は元々ファイル削除、restore は
route を書かず次の start/deploy が適用、purge は remove 済み)。

---

## 7. 切替ハンドラ(`POST /api/services/{id}/visibility`)

stop と同型の小さなハンドラ。順序が肝心:

1. `ensure_owned`(404 ゲート。lock 外・安価)
2. `Visibility::parse`(不正値は 400 — 500 に潰さない)
3. `state.deploy_lock(id)` 取得(deploy / start / stop / soft_delete と同一 lock で直列化。
   in-flight deploy との交錯はどちらの順でも最終状態が DB と一致する)
4. **DB 先行 UPDATE**(背骨:DB=期望状態)。`resources.deleted_at IS NULL` を課し
   `RETURNING subdomain, container_port`、rows=0 → 404(lock 待ち中の削除完走を弾く)
5. `audit("service.visibility", {"visibility": v})`(恒久的状態変化の直後 = 後段が失敗しても監査は DB と一致)
6. 現実収束(lock 内):
   - `private` → `route::remove`(冪等)
   - `company`/`public` → `serving_container` が Some の時だけ `route::write(.., ipallow)`。
     None(停止 / 未デプロイ / failed で serving 無し)は何も書かない —
     「停止 service に route ファイル無し」の不変条件を維持。次の start / deploy が適用する。
7. 収束失敗は 5xx(文案に「reconcile が 30 秒以内に収束・再実行も可」= AI が自己修正できる)。204 で返す。

shared は `SetServiceVisibilityReq { visibility: String }` + `ServiceDto` に
`#[serde(default)] visibility: String`(空 = 旧サーバ → client は company 扱い)。

---

## 8. 入口(CLI / web)

- **CLI**:`tbm service visibility <NAME> <private|company|public>`(値は clap ValueEnum = 推測不要、
  help は日本語で 3 値の意味と「即時反映・再デプロイ不要」を書く)。json 出力は安定フィールド
  `{"visibility":"<value>"}`、text は値ごとの日本語確認文。`tbm service status` に公開範囲行を追加
  (json は DTO 直出)。**`tbm service verify` は private を短絡**:公開 URL を探測する前に
  「非公開のため検証スキップ。公開するには `tbm service visibility <name> company`」を返して非零終了
  (接続失敗の紛らわしい報告で AI が「サーバ障害」と誤判し無駄リトライする既知の実害パターンを断つ)。
  旧サーバ(visibility 空)は company 扱いで従来動作。
- **web**:概要ページに「公開範囲」section(既存 `Radio`、横並び 3 択、mutate 中 disabled)。
  private のときは公開 URL バナーを**消さずに灰色化**(「非公開」ラベル、URL 文字列とコピーは温存 =
  subdomain 保存の要件、「開く」は無効化 — 302 /noservice にしか行かないため)。

---

## 9. 正直な差異・受容済み(bug ではない)

- **≤30s の収束窓**:toggle の書込失敗(5xx 返却後)や deploy の route 書込/削除失敗時、現実が DB に
  追い付くまで最大 1 reconcile tick。恒久ドリフトは §0-F で排除済み、受容するのは窓だけ。private の
  deploy で `route::remove` だけ失敗した窓は外部が 502(fail-closed。§6)。
- **attach の切替点前倒し**(§5):route 書込時 → commit_success 時。双方とも健全な版を指すため実害なし。
  route 書込失敗窓では内部リンクが公開より先に新版を指し得る(§5。健全版どうし・収束方向同一で受容)。
- **per-callee の docker 照会コスト増**(attach_callees がファイル読みから presence 照会へ)。単機規模で許容。
- **private は「認証」ではない**:HTTP 入口を消すだけ。M6 リンク経由・`tbm service exec`・web terminal
  からは従来どおり届く(いずれも所有者鉴权済みの経路)。
- **public は文字通りの一般公開**:ipallow を外すので、アプリ側に認証が無ければ誰でも触れる。
  本人裁量 + audit(§0-C)。
- **dev(domain=localhost)には catchall が無い**:private は単に直アクセス不可。活体挙動(302)の
  確認は prod のみ(§11)。
- **dev(OrbStack)では traefik の file watch が発火しない**:virtiofs 越しの bind mount に fsnotify が
  届かず、切替の実効確認には traefik 再起動が要る(2026-07-02 の dev e2e で実測:再起動後は company=
  実体応答 / private=404 と正しく反映)。prod Linux(原生 bind mount)は watch が効き即時 — M3 以来の
  deploy 切替(route ファイル書換)が本番実証済みの同一経路。

---

## 10. 否決(後相)

- **§10-A 部分公開**(per-path / Basic 認証 / 署名付き URL):三態で満たせない要件が実際に出たら別設計。
  traefik middleware の積み増しで実現可能な下地(route ファイル生成の分岐)は本設計で既に在る。
- **§10-B admin(owner)面への visibility 表示・代理切替**:owner ガバナンスは「見る=匿名化 /
  最後の砦=停止・削除」の現行二層で足りる。visibility はユーザの自資源操作(CLI/web の通常入口)。
- **§10-C `route::ensure` 汎化**(§3 冒頭に理由)。
- **§10-D visibility の第 4 値**(例:リンク先からのみ許可の ACL 的なもの):M6 の網隔離が既に
  同等の境界を仕組みで担保している。

---

## 11. 完了判定(dev + 香橙派 e2e)

**dev(`just dev`。観測点 = `traefik-dynamic/` のファイル実体)**:

1. create + `tbm deploy --local` → `svc-<id>.yml` に middleware 行あり(company 既定)。
2. `visibility public` → 即時再生成・middleware 行なし。`status`(text/json)に公開範囲。
3. `visibility private` → ファイル消滅。`verify` が明確な短絡文言 + 非零終了。web で Radio + バナー灰化。
4. 停止状態で toggle → ファイルが生えない。start 後に DB の値どおり。
5. drift 3 種(ファイル手動削除 / middleware 行手動改変 / private なのに手動でファイル設置)→
   30s 以内に各々収束 + `route_drift` audit。
6. `just check` 緑。

**prod(香橙派)活体** — **全項済み(2026-07-03、server v38)**:

1. ✅ private = どの IP からも 302 `/noservice`(ローカル + 社外 VPS 133.88.123.119 の両方で確認)。
   public = 社外からも実体応答・yml に ipallow 行なし。company = 実体応答・ipallow 行あり。
   ※「company = 社外 NG」は**現状検証不能**:本番の会社 IP 許可リストが空 = fail-open
   (`ipallow.yml` が 0.0.0.0/0)で company≒public。これは既存の平台状態(owner が entries を
   入れた時に差が立ち上がる)であり本機能の回帰ではない。
2. ✅ toggle は再デプロイ無しで ~4 秒で反映(prod Linux の traefik file watch は正常)。
3. ✅ **M6 × private(主用途)**:private callee(vis-e2e)への注入を持つ caller コンテナから
   `wget http://vis-e2e:8080` が実体を返し、未リンクの service へは `bad address`(隔離回帰)。
4. ✅ `just ship`(v38):migration 20260702000001 自動適用、既存 7 service(全 company)無瞬断。

その後 **server v39 / tbm 1.0.18**(2026-07-03)でユーザ可視ラベルの文言修正のみ
(公網/全網公開 → 外部/一般公開 等。機能変更なし)。
