# NEXT.md — 作業の引き継ぎ(2026-08-20 時点)

> **これは一時ファイル**。ここの TODO が片付いたら**削除する**
> (根に残すのは `CLAUDE.md` / `README.md` だけ、が本来の約束 — 設計・調査・障害記録は `doc/`)。
> 恒久的な内容はすべて `doc/paas-service-subdomain-design.md` §6・§7 と `CLAUDE.md` の該当段落に
> 既に書いてある。このファイルは**まだ終わっていないこと**だけを持つ。

---

## 1. 現在地(全部 push 済み・本番稼働中)

直近 6 commit(古い順):

| commit | 内容 |
|---|---|
| `800bec0` | **改名の影響名単** `GET /services/{id}/callers`(+ `tbm service callers` / web 概要の常設セクション / modal の案内を条件化)。併せて `DatabaseOverview` の誤った rotate 文案と、その出所の doc 2 箇所を修正 |
| `8d2ad8a` | **既存バグ修正**:孤児の進行中 `deploys` 行で `deploy-source` が永久 409 になる穴(起動時に非 terminal 行を閉じる) |
| `90e9f24` | **連帯再デプロイ** `POST /services/{id}/redeploy-callers`(+ `DeployTrigger::CallerRelink` / 実行枠の入場制限 / `deploys.trigger` migration / CLI 双入口 / web の checkbox と半完成エラー) |
| `eb4aee5` `8389586` `3abf4ca` | skill(AI 面)の追記と、外部審査で出た**事実誤り・矛盾**の修正 |
| `b4323ef` | この NEXT.md + CLAUDE.md からのポインタ |
| `72d6c60` | **codex 深審の真バグ 7 件を修正(未 ship)** — 詳細は (A) |

**出荷済み**:

- **server `tsubomi:v62`**(本番 = 香橙派)。`HOST=opi TAG=v62 just ship` で展開。
  **⚠ v62 は `90e9f24` 時点のサーバコード**。`72d6c60`(codex 修正 7 件)は**まだ本番に無い** —
  次は `HOST=opi TAG=v63 just ship`。CLI 側の変更も含むので `just release-cli-publish` の前に
  `crates/cli/Cargo.toml` の version を **1.1.5** へ上げること(リリースは不可変)。
- migration `20260820000001_deploys_trigger.sql` は本番適用済み。既存 32 行が `'user'` に回填され、
  **既存行を API 経由で読み戻すところまで確認済み**(新規作成の e2e では見えない穴の約束)。
- **tbm 1.1.4** を Pi へ公開済み(`just release-cli-publish`)+ ローカルも `tbm update` 済み。
  skill の投影(`~/.claude/skills/` と共有庫 `~/.agents/skills/`)も更新済み。
- ship 前後で本番の全テナント app の公開 URL が**同一の応答**(403/200/200/200)。403 は会社 IP
  許可リストによるもので、この変更とは無関係。

**旧 CLI(1.1.1)は v62 に対して壊れない**ことを確認済み(serde が未知の `trigger` を無視)。
強制アップグレードの窓は無い。

---

## 2. まだ終わっていないこと(優先順)

### (A) ~~codex 第 2 巡の深審~~ → **完了(2026-08-20 夕)**。残った指摘が下記 (E)

再走して真バグ 15 件(High 4 / Medium 9 / Low 2)。**7 件を修正して `72d6c60` で出荷準備済み
(未 ship)**、残り 8 件は判断のうえ (E) へ送った。修正した分の要点は commit message と
doc §7 に。特に **2 つは自分の設計判断の撤回**なので、次に触る人は逆戻ししないこと:

- `CallerRelink` は **readiness を探測する**(当初は探測しない設計だった)。再リンクは注入 env が
  変わった状態で起こし直すので「同じ image」は「今回の env でも ready」を保証しない。
  v48 の懸念は `damages_phase_on_failure=false` で既に無効化されている。
- phase 復元に「自分以外の非 terminal な deploy 行が無い」条件を**足してはいけない**
  (足したら `deploying` 永久固着を産んだ)。理由は `deploy.rs` の当該コメントに全部書いた。

### (B) web UI を**目視で確認していない** ← 実装は型検査だけ

`tsc -b` と `vp lint` は通っているが、**ブラウザで一度も見ていない**。確認すべきもの:

1. 概要の常設「呼び出し側」セクション(0 件でセクションごと消えるか)
2. subdomain 変更 modal の **4 状態**:取得中「確認しています…」/ 取得失敗「確認できません
   でした」+ 一般注意 / 0 件(何も出ない)/ N 件(名単 + 影響文)
3. 既定チェック済みの checkbox(対象 0 件で出ないこと・全 skip で disabled)
4. **半完成**:改名は成功したが relink が失敗した場合、modal が閉じず専用文案 + 再試行ボタン
5. modal を開き直したとき前回の失敗バナーが残らないこと(`relink.reset()`)
6. `CallerItem` のバッジ(停止中 = 灰 / 直近デプロイ失敗 = 赤)が
   「未反映(要デプロイ)」の琥珀と**別の色**であること(色の意味が衝突していた既存問題)
7. Deploys タブに `[自動:注入元の改名に追従]` のラベルが出ること
8. **`72d6c60` で挙動が変わった分**:①入力欄で **Enter**(同値のまま)を押しても何も起きない
   ②名単の取得を失敗させた状態(server を止める等)でも checkbox が出て、勾選のまま送ると
   relink POST が飛ぶ ③改名の応答前に画面を離れても relink が送られる(Network タブで 2 本目を確認)

`just db-up` + `just dev` → :5173。検証用の caller/callee ペアの作り方は §3 のレシピ。

### (C) 本番 e2e は**未実施**(意図的)

dev では端到端で確認済み(断線の実測 → 追従 → 実到達 / 護欄の回帰)。本番では
**読み取りのみ**確認した(新端点が 200 で `[]`、migration の読み戻し、全 app 無変化)。

本番で書き込み e2e をやっていない理由:**本番に生きた M6 リンクが 1 本も無い**
(`injections` の service 注入は 1 行あるが、caller・callee ともゴミ箱の中 = 過去の e2e の残骸)。
やるなら本番にテスト service を 2 つ作る判断が必要 → **ユーザに確認してから**。
実際の service↔service リンクが生まれたときに自然に検証されるので、急がなくてよい。

### (E) codex 第 2 巡で**採用しなかった 8 件**(理由付き。次スライスの材料)

いずれも「実害はあるが、直すには設計 or migration が要る」もの。**軽い順ではなく、重い順**に
書く(次に着手するなら上から):

1. **phase の所有者を列で持つ**(High 1 の原理的な解)。`service_details.phase_owner_deploy_id`
   を足し、phase を書く全経路(`run_digest` / `deploy_source` / `stop_containers` /
   `commit_success` / `mark_failed` / `recover_interrupted`)が owner を同時に書き、復元は
   owner 一致でのみ行う。今は「消し得るが実害は小さい方」を選んでいるだけ。
2. **`commit_success` 後・cutover 前の ship で新旧コンテナが永久併存**(codex 指摘・**対象差分外の
   既存バグ**)。DB は既に running/succeeded なので `recover_interrupted` の対象外、service は
   生きているので孤児掃除も無視する ⇒ HTTP は二重、worker は二重実行。reconcile が
   「直近成功 deploy のコンテナ以外」を deploy_lock 下で掃除する必要がある。**これが一番重い**。
3. **`deploy_source` と relink の入場が原子的でない**(Medium 5)。両者が別々に「in-flight 無し」を
   読んで行を作れる ⇒ relink が旧 digest で成功 commit → source が完了時に `running` を見て
   自分を failed にする。`service_details.image_digest` は旧値なので no-downgrade 門では
   検出できない。**check・行作成・marker を同じ advisory lock か DB 行で直列化**するのが解。
4. **relink 中に callee が再改名されると、成功した caller が即座に旧名を持つ**(Medium 7)。
   世代門が無い(current-digest 門は caller の digest しか見ない)。callee の
   `subdomain_changed_at` をバッチに持ち、commit 前に照合して skip / retry する。
5. **docker / DB の一過性失敗を「callee 停止」に潰している**(Medium 8)。`serving_container` が
   `Option` なので、確認不能と確認済み停止が同じ `None` になり、**全 caller が対象外の 202** に
   なる(成功したように見えて何もしない)。fallible な lookup を作り、確認不能は 503 にする。
6. **`run_digest` 冒頭の DB error が INSERT 済み deploy 行を閉じない**(Medium 10)。
   `.await?` で抜けるので `received` 行がプロセス中ずっと残り、以降の relink は in-flight 扱い・
   deploy-source は 409(`8d2ad8a` が閉じるのは**次の起動時**)。行作成後の全 Err を 1 つの
   lifecycle guard で terminal 化する。
7. **202 バッチの真の結果が観測できない / ship で残りが消える**(Medium 12)。recheck・digest
   解決・eject で落ちた分は audit にしか残らず、`GET /callers` は前の status を返す。batch id と
   per-target outcome を永続化し、status 端点で terminal まで poll / resume するのが解。
   CLI の「開始しました」も「要求時点の候補を受理」に直す。
8. **fan-out が O(N²)**(Low 14)。target ごとに全 caller を再取得 + docker presence。
   caller id 指定の単行 fresh verdict にする。今の規模(N=1〜5)では実害なし。

**この 8 件は「今の実装が嘘をつく」類ではなく「並行や中断で壊れる」類**。日常運用では
まず踏まないが、踏んだときは静かなので、次に service のデプロイ経路を触るときにまとめて
片付けるのが効率的(特に 1・2・3 は同じ「phase / 入場の所有権」という一つの問題の別の顔)。

### (D) 意図的に先送りしたもの(忘れないための記録。すぐやる必要はない)

- **未反映バッジの反転**(既存バグ):caller が停止していると `serving_since` が `None` に
  なり `needs_redeploy=false` = 「反映済み」に見える。直すには `InjectionDto` の三態化が要る。
  今回の機能では**発火しない**(停止中は判定で skip される)。詳細 doc §7.7。
- **`Reconcile` は失敗で `phase='failed'` を書き続ける**:`probes_readiness` を免除した理由
  (対象は元々健全 / start-first で旧は無傷 / failed は自愈網からの除名)は Reconcile にも
  効くはずだが、既存挙動の変更は今回の射程外。`impl DeployTrigger` の表に**格子として
  明示**してあるので、次に触る人が判断できる。
- **skill の章順**:§3.1(TLS の駆動系差)と §3.3(実 IP ヘッダ)は排障時の参考資料なのに
  「資源を作る」と「デプロイ」の間に 55 行居座っている。末尾の参考章へ移すと主線が
  前提 → 資源 → 注入 → デプロイ → 検証 → 排障 → 参考になる。**削減ではなく再構成**なので
  今回は見送った(44KB の AI 向け文書を並べ替えると相互参照が崩れるリスク)。
- `tbm` の 3 箇所で `catch_unwind` + spawn の定型が 3 通りに、web で「body を返す
  手書き `useMutation`」が 5 箇所に増えた。どちらも「そろそろ抽出時」の閾値超えだが、
  今回のスライスの責任ではない(既存分を巻き込む改修になる)。

---

## 3. 検証のレシピ(dev。ここで詰まった分を残す)

```bash
just db-up && just dev        # infra + server :9090 + web :5173
```

**dev 用 token の作り方**(web のログインフローを通さずに CLI を使う。**検証後は必ず失効**):

```bash
TOK="tbm_devverify_$(openssl rand -hex 12)"
HASH=$(printf '%s' "$TOK" | shasum -a 256 | cut -d' ' -f1)
docker exec -i tsubomi-pg-platform psql -U tsubomi -d tsubomi_platform -q -c \
  "INSERT INTO cli_tokens (user_id, name, token_hash) VALUES ('<dev user id>','tmp-verify','$HASH');"
# 使い終わったら:UPDATE cli_tokens SET revoked_at=now() WHERE name='tmp-verify';
```

**caller/callee ペア**(`traefik/whoami` は tiny で port 80 を listen する = 最適):

```bash
T() { ./target/debug/tbm --server http://127.0.0.1:9090 --token "$TOK" "$@"; }
T service create t-callee --port 80 --no-github
T service create t-caller --port 80 --no-github
T inject t-callee --into t-caller
T deploy --image traefik/whoami:latest --service t-callee   # サーバ側 pull = docker 不要
T deploy --image traefik/whoami:latest --service t-caller   # 注入後に再デプロイして env を凍結
```

**踏んだ落とし穴**:

- **`--server` / `--token` は `--` より前に置く**。`exec -- <cmd>` の後ろに置くと全部コンテナ内
  コマンドの引数として渡り、CLI は保存済み設定(= **本番**)を見に行く。
- **`traefik/whoami` は scratch イメージ**なので `sh` / `printenv` / `wget` が無い
  → `service exec` で中を見られない。**凍結された env は `docker inspect` で読む**:
  `docker inspect <容器> --format '{{range .Config.Env}}{{println .}}{{end}}' | grep T_CALLEE`
- **網別名**は callee 側の容器を見る:
  `docker inspect <callee容器> --format '{{json (index .NetworkSettings.Networks "tsubomi-svc-<caller id>").Aliases}}'`
  改名すると「凍結 env = 旧名 / 別名 = 新名」になる = これが断線の実体。
- **到達性**は traefik から測る(caller の私網に居るので docker DNS が引ける):
  `docker exec tsubomi-traefik wget -qO- --timeout=4 http://<新subdomain>:80/`
- **`docker ps | head -1` で容器を選ぶな**。start-first swap の最中は新旧が並ぶので、
  撤去途中の旧容器を掴んで「env が追従していない」と誤診する(この日 1 度誤診した)。
  容器名は明示するか `deploys` の直近成功 id から `container_name` を組む。
- **失敗路径の作り方**:`docker stop tsubomi-registry` → relink すると pull が失敗する。
  終わったら `docker start tsubomi-registry`。
- **後片付け**:`T service delete <名前>` → `T trash purge <id>`。網とコンテナの残留は
  `docker network ls | grep tsubomi-svc` / `docker ps -a --filter label=tsubomi.service_id=<id>`
  で確認(reconcile の孤児 GC が 30s で網を消すのも同時に確認できる)。

---

## 4. 触るときの注意(この 2 スライスで学んだこと)

- **無条件の警告を条件付きに変えると、条件が「未知」のときに旧実装より弱くなる**。
  `callers === undefined`(取得前 / 500 / 再取得中)を 0 件扱いにすると、実リンクがあるのに
  警告なしで改名が通った。取得中は待たせ、失敗は「確認できませんでした」と言う。
- **「失敗しても壊さない」は「状態を変えない」ではなく「入口の状態へ戻す」**。
  `mark_failed` をやめただけでは `run_digest` が開始時に書いた `phase='deploying'` で固着し、
  結局 `converge_running` の候補集から外れて同じ害になる。
- **lock の外で書かれる状態は、lock 内からでも所有権を持たない**。`deploy_source` は
  取得開始時に `deploy_lock` の外で phase を立てるので、`WHERE phase='deploying'` だけの
  条件付き UPDATE は他人の marker を消す。
- **判定はサーバの純関数を単一真源にし、クライアントで再導出しない**。
  `will_redeploy` / `skip_reason` をそのまま読ませる(`desired_state` から独自判定すると
  実行側とずれる)。
- **migration の回填値は既存行を読み戻すまで検証が終わらない**(新規作成の e2e では見えない)。
- **dev の実測でしか出ない穴がある**:phase 固着も「registry を止めて pull を失敗させる」
  という**失敗路径の実測**で初めて出た。門禁・正常路径だけ試して終わりにしない。

---

## 5. 参照

- **実装級**:`doc/paas-service-subdomain-design.md` **§6**(影響名単)/ **§7**(連帯再デプロイ。
  4 述語の表 / 入場制限 / provenance / 受容した差異 / 品質検証の記録)
- `CLAUDE.md` の該当 2 段落(subdomain の段落の直後)
- skill(AI 面):`crates/cli/skill/tsubomi-deploy.md` の §「注入元の subdomain を変えたとき」
- 設計時の対抗審査で出た P0 4 件は doc §6.5 に記録(2 件は既存バグで、うち 1 件は `8d2ad8a` で修正済み)
