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
| caller は再デプロイまで旧値で断線し得る | 注入値は「起動の瞬間に解決」の背骨どおり(rotate と同じ作法)。未反映バッジが案内する。 |
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
