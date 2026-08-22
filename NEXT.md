# NEXT.md — 作業の引き継ぎ(2026-08-22 時点)

> **これは一時ファイル**。ここの TODO が片付いたら**削除する**
> (根に残すのは `CLAUDE.md` / `README.md` だけ、が本来の約束 — 設計・調査・障害記録は `doc/`)。
> 恒久的な内容はすべて `doc/paas-service-subdomain-design.md` §6・§7 と `CLAUDE.md` の該当段落に
> 既に書いてある。このファイルは**まだ終わっていないこと**だけを持つ。

---

## 1. 現在地(全部 push 済み・本番稼働中)

**出荷済み(2026-08-22)**:

- **server `tsubomi:v63`**(本番 = 香橙派)。`72d6c60`(codex 修正 7 件)+ web の 2 件を含む。
  展開は `HOST=opi TAG=v63 just ship`(infra `--no-recreate` + server 単換 = 無瞬断。
  ship 前後で既存 4 app の応答が同一 200/200/200/403 — 403 は会社 IP 許可リストで無関係)。
- **tbm 1.1.5** を Pi へ公開済み(`just release-cli-publish`)+ ローカルも `tbm update` 済み。
  1.1.4 のままだと `72d6c60` の CLI 修正(`--redeploy-callers` の client-side 同値判定の撤回)が
  誰にも届かなかった。
- migration は v62 時点から増えていない(`20260820000001_deploys_trigger.sql` が最後)。

直近の commit(古い順):`72d6c60`(codex 7 件)→ `d68481b`(NEXT.md)→ `1528d29`(subdomain の
Enter)→ `c372576`(CLI 1.1.5)→ `b82ec8f`(一覧行の truncate)。

**このセッションで足した web の 2 件**:

| commit | 内容 |
|---|---|
| `1528d29` | **subdomain の Enter を「同値 / 不正値なら何も起きない」に固める**。`lib/subdomain.ts` に形式検証の純関数(サーバ `validate_subdomain` の写し・**権威はサーバのまま**)を切り出し改名 modal と作成フォームで共有。送信可否を 1 か所で導出し、**隠し submit を `disabled`**(HTML の暗黙送信は default button が disabled なら発火しない = Enter で action すら呼ばれない)。`onSubmit`+`preventDefault` → **React 19 の `<form action>`**。`useActionState` は不採用(pending / error は 2 つの mutation が持っており、**半完成**を state に写すと真源が二重になる) |
| `b82ec8f` | **長い件名で「ボタンが次の行へ落ちる」を truncate に**。flex の行分割は左列の **max-content 幅**で決まるので `min-w-0` だけでは足りず `flex-1`(basis 0)が要る。デプロイ履歴 + 環境変数タブの 2 行(同型)を修正。ヘッダ系の `flex-wrap` は意図的なので不変 |

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

### (B) web UI の**ブラウザ目視は未実施**(ユーザ判断で今回は見送り)

`tsc -b` / `vp lint` / `vp build` は通り、`1528d29` の入力検証はサーバ規則との**逐条一致を
機械照合済み**(24 ケース:予約語 7 語 + `tsubomi-` 前綴 / 大文字・記号 / 数字始まり / `-` 終わり /
51 字 / 境界の 50 字 / 空文字 — うち 8 ケースは本番 API の実応答と突き合わせた)。
残るのは**ブラウザでしか見えない分**だけ:

1. 概要の常設「呼び出し側」セクション(0 件でセクションごと消えるか)
2. 変更 modal の 4 状態(取得中 / 取得失敗 / 0 件 / N 件)
3. 既定チェック済み checkbox(対象 0 件で出ない・名単未知でも出る)
4. **半完成**(改名は成功・relink 失敗)で modal が閉じず専用文案 + 再試行
5. modal を開き直して前回の失敗バナーが残らないこと(`relink.reset()`)
6. `CallerItem` のバッジ色が「未反映(要デプロイ)」の琥珀と衝突しないこと
7. Deploys タブの `[自動:注入元の改名に追従]` ラベル
8. `1528d29`:不正値を打つと入力欄の下に赤字が出て **Enter で何も起きない** /
   同値では淡色で「現在のサブドメインと同じです」/ `b82ec8f`:長い件名で
   **ボタンが右端に残り件名が `…` で切れる**

`just db-up` + `just dev` → :5173。caller/callee ペアの作り方は §3 のレシピ。

### (C) 本番 e2e ~~未実施~~ → **完了(2026-08-22、server v63 / tbm 1.1.5)**

`--port 80` = private(公開 URL を持たない)のテスト service 2 本で、dev と同じ筋を本番の
香橙派で通した。**14 項目の確認内容と実測値は `doc/paas-service-subdomain-design.md` §7.8**
(恒久記録)。要点だけ:改名で**実際に断線する**ところ(凍結 env は旧名 / 網別名は新名 /
旧名は `bad address`)を実測してから連帯再デプロイで追従を確認、**同値再実行でも relink が
飛ぶ**(72d6c60 の撤回)/ **停止中・failed の caller は叩き起こさない** / 並行 2 発は同期 409。
終了後に delete → purge し、コンテナ・網・route ファイルの残留ゼロと既存 4 app の無変化を確認。

**残りの本番未検証**:`deploy --watch`(GitHub Actions 経路)は今回触っていない。

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
- **本番でも「壊してから直す」順で測る**(2026-08-22 の本番 e2e)。改名 → **断線を実測**
  (旧名が `bad address`)→ relink → 追従、の順にすると「元から繋がっていただけ」を
  追従と誤読しない。テスト service は **`--port 80`(= visibility 推導が private)**にすれば
  公開 URL が生えないので本番でも外部影響なく作れる。
- **flex で truncate させたいなら `min-w-0` だけでは足りない**(`b82ec8f`)。行分割の判定は
  左列の **max-content 幅**なので、`flex-1`(basis 0)にしないと縮む前に折り返す
  = 「ボタンが次の行の先頭へ落ちる」。`flex-wrap` を親に付けたままだと truncate は一生効かない。
- **HTML の暗黙送信は default button が disabled なら発火しない**(`1528d29`)。form の隠し
  submit を `disabled` にするのが「Enter で何も起きない」の最短路(関数側のガードは二重の防壁
  として残す — footer のボタンの disabled は form submit の防壁にはならない)。

---

## 5. 参照

- **実装級**:`doc/paas-service-subdomain-design.md` **§6**(影響名単)/ **§7**(連帯再デプロイ。
  4 述語の表 / 入場制限 / provenance / 受容した差異 / 品質検証の記録)
- `CLAUDE.md` の該当 2 段落(subdomain の段落の直後)
- skill(AI 面):`crates/cli/skill/tsubomi-deploy.md` の §「注入元の subdomain を変えたとき」
- 設計時の対抗審査で出た P0 4 件は doc §6.5 に記録(2 件は既存バグで、うち 1 件は `8d2ad8a` で修正済み)
