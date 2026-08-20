# tsubomi PaaS — service subdomain の作成時指定 + 作成後変更 実装設計

> visibility / stateful / limits に続く「作成後変更可」系の追加(マイルストーン外)。
> 背骨は変えない:**DB が期望状態、現実(route ファイル / docker 網別名)をそこへ収束させる**。
>
> 解く問題:subdomain は display_name の slugify で自動採番され、気に入らなくても
> 作り直すしかなかった。①作成時に任意指定(`--subdomain` / web 入力)②作成後に変更
> (`tbm service subdomain` / web 概要の編集)の両方を開ける。
>
> 対象:server v59 / tbm 1.1.1。migration 1 本(`20260819000001`)。

---

## 0. スコープと確定事項

- **作成時指定は任意**:`CreateServiceReq.subdomain: Option<String>`。省略 = 従来の
  slugify 自動採番(一行不変)。指定時は **1 回だけ** insert を試し、使用中なら 409
  (明示指定した名前を乱数サフィックスで別名に化けさせない)。
- **変更端点** `POST /api/services/{id}/subdomain`:`set_visibility` と同じ
  「deploy_lock 内で DB 先行 → 現実収束」。同値は冪等(UPDATE・audit・時刻を動かさない)
  だが**収束段は再実行する** — 前回の route/別名の反映失敗を同じコマンドの再実行で回収できる
  (「再実行も可能」を嘘にしない。simplify/codex 審査の共通指摘)。応答は更新後 ServiceDto。
- **検証規則**(`validate_subdomain`、create / 変更で共用):小文字英数と `-`・英字始まり・
  `-` 終わり禁止・50 字以内・予約語拒否。**自動採番の出力も同じ規則を通る**(性質テストで
  機械封じ):乱数サフィックス形は `suffixed_candidate` が base を詰めて **suffix 込み 50 字**を
  守る(素朴な `base(50)-xxxx` = 55 字は「自動で付いた subdomain を変更端点で再指定できない」
  round-trip 不全 — codex 審査で顕在化)。409 文言はゴミ箱占有のヒント付き(`subdomain_taken_msg`
  が create / 変更の単一真源。subdomain の UNIQUE は display_name と違いゴミ箱内も占有する)。
- **予約語の拡張**:固定語(paas/registry/traefik/www/api + **db/cache** = 公開 DB / cache
  入口名)に **`tsubomi-` 前綴**を追加。
  subdomain は M6 リンクで per-service 私網の docker 網別名になるため、私網に同居する
  infra / app コンテナ名(`tsubomi-pgbouncer` / `tsubomi-valkey` / `tsubomi-<uuid>`)と
  docker DNS で衝突し得る。自動採番ループの skip にも同条件を追加(既存の暗穴:
  表示名「tsubomi valkey」→ slug `tsubomi-valkey` が従来は通っていた)。`tsubomi-` で始まる
  base はサフィックスを付けても前綴が残る = 全試行 skip で必ず失敗するので、**前綴を剥がして
  救済**(剥がした残りが英字始まりでなければ "app")。既存行の残余は起動時 warn
  (`warn_reserved_subdomains` — 自動改名はしない、判断は人間)。
- **旧 subdomain は凍結しない**(ユーザ決定):解放即再利用可。外部書签が別 app に
  当たり得るのは全 PaaS 共通の受容。改名頻度制限もしない(平台とユーザの境界)。

## 1. 変更の現実収束(端点内・deploy_lock 下)

serving コンテナが在るときだけ:

1. **route**:private → 何もしない(ファイル無しが期望状態)。company/public →
   新 host で `svc-<id>.yml` を原子上書き(ファイル名は id 基準なので削除→新規は不要)。
   旧 URL は catch-all → 302 /noservice に自然落ち。
2. **M6 別名換血** `network::realias_as_callee`:この service を注入している全 caller
   私網で `reattach_with_alias`(disconnect → 新別名 connect → `endpoint_has_alias` 閉環確認 —
   `migrate_pgbouncer_aliases` の実証済みレシピ。docker の網別名は初回 connect でしか確定せず、
   既接続 403 は冪等吞み = 付け直すしかない)。**既に正しい別名の網は触らない**(同値再実行 =
   収束の再試行で健全リンクを無駄に瞬断しない)。best-effort・per-item warn。

収束失敗は `UnavailableMsg`(「保存済み。reconcile が 30 秒以内に収束」)— DB は
更新済みなので reconcile(下)が直す。

## 2. reconcile の収束拡張(取りこぼしの保険)

- **route host drift**:drift 判定を `(backend, ipallow)` → **`(host, backend, ipallow)`**
  の三組に拡張(`route::current` に `parse_host` を追加 — `build_service_doc` の rule 行の
  「write の逆」でテスト密結合)。動機は visibility が ipallow を組に足したのと同型:
  変更の書込だけが失敗すると「DB は新 subdomain・現実は旧 host」が黙って残り、新 URL が
  永久 404 になる。lock 取得後の fresh 再確認も `fresh_visibility` → `fresh_route_inputs`
  (visibility + subdomain)に拡張 — lock 待ち中の変更で旧 host を書き戻さない。
- **別名 drift**:`connect` が「既接続だったか」を返すようにし、`attach_callees` は既接続時
  のみ別名を検査 → 陳腐なら付け替え。検査は**三値**(`endpoint_alias_state`):inspect 失敗 =
  「判定不能 = 触らない」— 不明を陳腐扱いすると健全リンクを毎 tick force-disconnect で瞬断する
  (審査②④の共通指摘)。付け替えの直前に subdomain を **fresh 再読**(`fresh_subdomain`)—
  tick 冒頭の読みが realias と交錯した旧値のとき、付いたばかりの新別名を剥がす巻き戻りを防ぐ
  (codex 審査)。定常コスト = 30s tick あたり既接続 callee 数ぶんの inspect_container 1 回。

## 3. 未反映の可視化(caller 側)

migration `20260819000001`:`service_details.subdomain_changed_at TIMESTAMPTZ NOT NULL
DEFAULT 'epoch'` + 有限値 CHECK。回填値 = 新規行 DEFAULT と同値(epoch は
`DateTime<Utc>` で読み戻せる — 2026-07-26 の -infinity 事故の約束事)。

`list_injections` の「注入値が今のコンテナと違う」時刻に、注入元 service の
`subdomain_changed_at` を追加(cache の `rotated_at` と完全同型の GREATEST 参加)。
caller の `_URL`/`_HOST` は起動時解決の旧値のままなので、caller の CLI
`[未反映:要デプロイ]` / web バッジが**零改修**で点く。同値変更で時刻を動かさないのは
偽の未反映を出さないため。

## 4. 受容した差異(直さないと決めたもの)

| 差異 | 理由 |
|---|---|
| GitHub repo 名は旧 subdomain のまま | 平台は GitHub に触れない(rename と同型)。 |
| `tbm service delete --with-repo` は改名後、現 subdomain 名で repo を探すため見つからず**エラーで正直に失敗**(旧名 repo は `gh repo delete` で手動掃除。エラー文に次の一手を併記) | `TSUBOMI_SERVICE_ID` 照合が誤削除は防ぐ = 安全側。repo 名の追跡は GitHub 操作になるので平台の領分外。 |
| caller は再デプロイまで旧値で断線し得る | 注入値は「起動の瞬間に解決」の背骨どおり(rotate と同じ作法)。**§6 で影響名単を出すようにした**(誰が切れるかを改名の前後で言う)が、再デプロイを押すのは依然ユーザ。未反映バッジも案内する。 |
| 既存注入の env 名(`API_BACKEND_URL` 等)は旧 subdomain 由来のまま | 名前を変えると利用側 app のコード修正を強制する。値だけ新しくなる。 |
| 変更 409 で他人の subdomain の存在が分かる | 作成時採番と同じ性質。全 PaaS 共通。 |
| DB 先行と route 改写の間 + 改写失敗時、旧 host の router が残る短窓(解放された旧名を他 service が即取得すると Host 二重の短窓)| 背骨(DB=期望状態)の順序どおり。reconcile の host drift 判定が ≤30s で是正 — visibility の fail-open 受容と同型。 |
| caller の deploy / reconcile と realias の交錯で旧別名が一時復活し得る(lock 非共有) | 陳腐書き手は全員ワンショット・reconcile は毎 tick fresh 読みなので次 tick(≤30s)で必ず新別名へ収束(フリップフロップしない)。attach_callees は動かす直前に fresh 再読 + 三値検査(inspect 失敗 = 触らない)で窓を ms 級に縮小済み。 |
| needs_redeploy の ms 級見逃し窓(UPDATE の now() と commit の間に caller が deploy を完走した場合) | 単文 UPDATE で now()≈commit。実害シナリオは行ロック長期保持が要る = 理論上のみ。 |
| serving コンテナが「present だが走っていない」間は旧 host の route が残る | backend が死んでいるので fail-closed(誤配なし)。復活・次 deploy・start で解消。 |

## 5. 検証

- 単体:validate 真理値表 / slugify→validate 性質 / parse_host round-trip。
- dev e2e:作成時指定(反映・409・400)/ 変更(route の Host 即切替・private 不変・
  同値冪等)/ M6 別名換血 + 未反映バッジ + caller 再デプロイで復旧 / reconcile の
  host・別名 drift 是正(手で改竄 → ≤30s)。
- **migration 既存行読み戻し**:既存 service + injections 行がある DB で適用 →
  注入一覧が落ちず needs_redeploy=false(「既存データだけが踏む穴は新規作成の検証では
  見えない」)。
- 本番 e2e(ship 後):公開 URL 切替 / 旧 URL 302 / M6 実機(香橙派)。

---

## 6. 影響名単(caller 側の可視化)

**解く問題**:改名の案内が弱かった。web の変更 modal と CLI の回显は **caller が 1 件も無くても**
「このサービスを注入している他のサービスは再デプロイで新しい値が入ります」を無条件に出していた
= 大半の service では無関係な脅し文。逆に caller が居るときも**誰なのかは言わない**ので、
ユーザは service 一覧を総当たりで探すことになる。しかもこの断線は「新機能がまだ繋がっていない」
ではなく「**動いていたものが今壊れた**」クラス(注入作成時の未反映は追加のみで何も壊れない)。

`GET /api/services/{id}/callers` → `Vec<ServiceCallerDto>`(読み取りのみ・migration なし)。

### 6.1 逆引きの述語は 1 本(`inject::service_caller_rows`)

家は **`services/inject.rs`** — `injections` 関係の**正向**の解決(`resolve` / `derived_env_keys`)が
既にあるので、逆向はその対偶。`network::service_callers`(網操作。id だけ要る)はこの薄い投影に
した。**別々の SQL にすると「名単に出た集合」と「`realias_as_callee` が実際に触る集合」が
ドリフトし、プレビューが嘘になる**。

形は **`GROUP BY` を使わない**:`resources` を主表に `EXISTS` で絞り、env 名は
`ARRAY(SELECT … ORDER BY …)` の相関副問い合わせで集める。理由は 2 つ。

- **「1 行 1 caller」が構造的に自明**になる。同一 caller が同じ callee を複数の env 名で注入して
  いるのは普通に起こり、行が増えると網操作が 2 回走り、連帯再デプロイ(§7 予定)は同じ service を
  2 度デプロイする。`GROUP BY` + `array_agg` だと 1 行性が「GROUP BY の網羅性」に依存し、
  読者が毎回検算しないと保証できない(simplify 審査)。
- **`array_agg` は NULL を返し得るが `ARRAY(…)` は空配列を返す**。`Vec<String>` の受け側で
  decode panic の経路が原理的に消える(2026-07-26 の `-infinity` 事故と同型のリスク)。

**caller の所有者では絞らない**。`ensure_owned` が callee の所有を既に証明しており、同一 owner は
**注入作成時**に担保されている(M6 の境界は租户)。ここで絞ると、万一跨 owner の注入が生まれた
ときに**この端点だけが realias が触る集合より少なく見せる** = 影響範囲の提示が嘘になる。
黙って隠すより担保を作成時に置く方が正しい層(simplify 審査で方針転換)。

**同じ述語の 3 つめの写しが `deploy::is_linked_callee`** にある(EXISTS 版。readiness 門禁と
probe が使う)。あちらは真偽 1 個で足り、readiness 門禁を重くしたくないので軽い問い合わせのまま
残した — **「生存 caller」の定義を変えるときは両方直す**(コメントに相互参照あり)。

### 6.2 DTO は「改名前の事前確認」に絞る

`ServiceCallerDto` = id / display_name / env_vars / desired_state / last_deploy_status /
last_deploy_error。`last_deploy_*` を載せるのは「**この呼び出し側はそもそも既に壊れている**」を
リンクを切る前に知るため(事前確認)。※ これは*直近の* deploy であって「自分の操作が起こした
deploy」ではないので、**連帯再デプロイの結果表示には使えない** — そちらは `deploys` に
provenance 列を足すのが正しい層(§7 の宿題)。

### 6.3 入口

- **web**:概要に**常設の「呼び出し側」セクション**(0 件ならセクションごと出さない)+ 変更 modal
  の案内を 2 つに割った — 無条件の 2 文(旧 URL 即失効 / GitHub repo 名は不変)と、
  **`callers.length > 0` のときだけ**の名単。caller 名は詳細ページへの `<Link>`(セクションの
  用途は「ここへ行って再デプロイする」なので死んだテキストにしない)。
- **CLI**:`tbm service callers <名前>`(**改名する前に**影響範囲を引けるのが主用途 = AI 向け)+
  `service subdomain` の text 回显を名単ベースに。**json 出力は `ServiceDto` のまま**(shipped 契約)。
  改名は既に成功しているので、名単の取得失敗でコマンドを失敗させない(注記のみ)。
- 状態の文言は `desiredLabel` / `deployStatusLabel`(lib の単一真源)を引く。バッジの色語彙は
  `components/phase-badge.tsx` の `Badge`(tone)に集約した — 直書きしていた琥珀は
  **「未反映(要デプロイ)」と全く同じ色**で、停止中の caller が未反映に見えていた(simplify 審査)。

### 6.4 併せて直した:database rotate の誤った文案

`DatabaseOverview` の rotate modal は「注入済みのサービスは再デプロイするまで古い文字列のまま」と
言っていたが、**db rotate は `role_kind='human'` だけを回す**(`databases::rotate`)。service に
注入されるのは **app role** なので影響ゼロ — サーバの `list_injections` が
`database_details.rotated_at` を未反映判定の `GREATEST` に**意図的に入れていない**のと同じ事実。
放置すると「rotate したら再デプロイ」を教え続ける(**cache では正しく database では嘘**)。

**出所は設計 doc** だったので根も直した:`paas-tech-design.md`(human role の「再デプロイで反映」)と
`paas-m5-design.md`(cache rotate を「database rotate と同じ意味論」と書いていた)。
直さないと次に rotate 文案を書く人が再生産する(altitude 審査)。`migrations/` に同文が無いことは
確認済み(適用済みマイグレーションは不可変)。

### 6.5 「未知」を「0 件」と同一視しない(codex 審査の最重要指摘)

無条件の警告を条件付きに変えると、**取得できていないときに旧実装より警告が減る**という
方向性の事故が起きる。改名 modal で `callers === undefined`(取得前 / 500 / 再取得中)を
空配列扱いすると、実リンクがあるのに**警告なしで改名が通る**。対処:

- **取得中**は「確認しています…」+ 送信ボタンを止める(短命なので待たせて良い)。
- **取得失敗**は「確認できませんでした」+ 一般的な注意を出す。**ボタンは止めない** —
  補助的な読みの不調で主操作(改名)を塞ぐのは行き過ぎ。
- **modal を開いた瞬間に `refetch()`**:別タブ / CLI で注入された分を取りこぼした古い名単の
  まま改名させない。
- **inject / eject は `serviceKeys.all` を落とす**。注入の作成 / 削除は自分の injections
  だけでなく**注入元(callee)の逆引き名単**も変えるので、狭いキーだけ落とすと callee の概要が
  古い `[]` を staleTime 分そのまま使う。

### 6.6 「注入関係」を「今切れているリンク」と断定しない

端点が返すのは**注入関係**であって、実コンテナの有無ではない。それを「今この瞬間切れています」と
書くと 2 通りで嘘になる(codex 審査):

- **停止中 / 未デプロイの caller** には凍結された env も生きたリンクも無い。それでも再デプロイを
  促すと、`commit_success` が `desired_state='running'` を書くので**ユーザが止めていた service を
  起こす**。⇒ 断定は**稼働中の相手だけ**に絞り、停止中には「次に起動したときに新しい値が入る」と言う。
- **同値の再実行**では、サーバは時刻も動かさず別名も剥がさない。⇒ CLI は `resolve_service_row` で
  改名**前**の subdomain を持ち(id 解決と同じ 1 リクエスト)、変化が無いときは影響を言わない。

`env_vars` の契約も直した:返すのは `injections.env_var` の**保存名(バインディング名)**だけで、
派生する `_HOST` / `_PORT` は含まない(展開するなら静的 env に譲った分の考慮も要る = 別の関心事)。
DTO コメントと UI/CLI の文言を「注入名」に揃えた。

### 6.7 品質検証

- **設計時**:Plan agent の対抗審査で **P0 4 件**(うち 2 件は既存バグ)を実装前に回収 —
  停止済み caller の叩き起こし / 健康な caller への readiness 門禁と自愈網からの除名 /
  僵屍 `received` 行が `deploy-source` の 409 門を永久に毒する / digest スナップショットによる
  静默ロールバック。いずれも**状態を変える §7 側**の穴なので、そちらの前提として持ち越し。
- **実装後**:4 simplify agents。最大の指摘は「**判定一族(`will_redeploy` / `skip_reason` /
  純関数)に Task 1 の消費者が居ない**」= §7 へ移送(DTO の追加は加算方向なので後出しが安全)。
  連鎖して 2 本の `EXISTS`・毎リクエストの docker 往復・真理値表テスト 2 本が消えた。
  SQL の再構成 / 所有者フィルタの撤去 / 述語の家の移動 / web の色衝突と文言ドリフトも同審査由来。
- **codex ultra**:真バグ 4 件を出荷前に回収(§6.5 / §6.6)。server の SQL・認可・
  `service_callers` の集合等価性は「到達可能な状態でバグなし」と確認された
  (`service_details` の存在は逆向き FK では強制されないが、`resources` と同一 tx で作られ
  単独削除経路も無いので、差が出るのは手 SQL で孤児を作った破損状態だけ)。
- dev 実機:端点 200 / `[]` / 401 / 404、**同一 caller の 2 env 名が 1 行に集約**、CLI text/json、
  改名回显が 0 件で消えて 1 件で名単を出すこと。検証用データは全て元の状態へ戻した
  (soft-delete の時刻は audit_log から復元)。
