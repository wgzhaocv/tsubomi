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

**出荷済み**:

- **server `tsubomi:v62`**(本番 = 香橙派)。`HOST=opi TAG=v62 just ship` で展開。
  **v62 のサーバコード = 現 HEAD**(ship 後の 3 commit は skill / CLAUDE.md だけ)。
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

### (A) codex 第 2 巡の深審が**未完** ← 最優先

Task 2(`90e9f24`)+ Task 1 のうち審査後に直した部分(`800bec0` の web / CLI)を対象に
codex ultra を走らせたが、**最終報告を書く前に額度切れで停止**した。

- 途中で 1 件だけ吐いた指摘(**`deploy_source` が `deploy_lock` の外で `phase='deploying'` を
  書くので、`CallerRelink` の phase 補償が自分の書いていない marker を消す**)は**修正済み**
  (`deploy.rs` の補償 UPDATE に「自分以外の非 terminal な deploy 行が無いこと」条件を追加)。
- 残りの報告は失われた。**もう一度通すこと**。

やり方:

```bash
brew upgrade codex            # 実行前に必ず最新化
# 審査範囲:git show 90e9f24 と、800bec0 のうち
#   crates/cli/src/commands/service.rs / web/src/routes/ServiceOverview.tsx / web/src/lib/services.ts
codex exec --model gpt-5.6-sol -c model_reasoning_effort=ultra --sandbox read-only \
  "$(cat <プロンプト>)" < /dev/null > /tmp/codex-out.md 2>&1
```

**`< /dev/null` を忘れないこと** — 付けないと codex がプロンプトを引数で受け取ったうえで
stdin も読もうとして無反応のまま止まる(この日 10 分無駄にした)。
額度切れの扱いは memory `codex-banked-reset` を見る(**`/reset` はもう存在しない** —
Codex が 5 時間枠を廃止したので、エラー本文の復帰時刻まで待つのが唯一の手)。

疑ってほしい点(前回のプロンプトの要点):`DeployTrigger` の 4 述語が既存の全デプロイ経路
(hook / start / rollback / reconcile / deploy-source)の意味を変えていないか / phase 補償の
穴 / no-downgrade 門が正当な再リンクまで弾く経路(未デプロイ = NULL、deploy-source の
`'pending'`)/ 入場枠 guard の漏れとデッドロック / `relink_callers` の再判定が中途で
callee 停止した場合 / CLI の json 契約 / web の入れ子 onSuccess と unmount 後の解決。

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

`just db-up` + `just dev` → :5173。検証用の caller/callee ペアの作り方は §3 のレシピ。

### (C) 本番 e2e は**未実施**(意図的)

dev では端到端で確認済み(断線の実測 → 追従 → 実到達 / 護欄の回帰)。本番では
**読み取りのみ**確認した(新端点が 200 で `[]`、migration の読み戻し、全 app 無変化)。

本番で書き込み e2e をやっていない理由:**本番に生きた M6 リンクが 1 本も無い**
(`injections` の service 注入は 1 行あるが、caller・callee ともゴミ箱の中 = 過去の e2e の残骸)。
やるなら本番にテスト service を 2 つ作る判断が必要 → **ユーザに確認してから**。
実際の service↔service リンクが生まれたときに自然に検証されるので、急がなくてよい。

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
