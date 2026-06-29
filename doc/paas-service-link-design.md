# tsubomi PaaS — service↔service 内部リンク 実装設計

> M6 網隔離(per-service 私網 + egress)の**後続**。マイルストーン外の追加機能で、
> cache のあとの「コンテナ内アクセス(terminal / exec)」と同じ立ち位置(背骨は変えない・新表なし)。
>
> 解く問題:今 app A が app B を呼ぶには B の公開 URL `https://b.<domain>` しか無く、
> 同一ホスト上なのに **Cloudflare を往復する公網ラウンドトリップ**(数十〜百 ms)になる。
> A に **机器内部の接続文字列**を渡し、B へ**直連**させて公網を省く。
>
> 背骨を一言で:**service は「URL を注入できるリソース」になる** — A に B の注入を保存すると、
> A は `B_URL=http://<B-subdomain>:<B-port>` を受け取り、**B の稼働コンテナが A の私網へ
> 別名 attach** されて docker DNS で解決する。値はコンテナ起動の瞬間に解決(rotate と同じ作法 =
> 再デプロイで効く)。
>
> 完了判定:**cache を使う e2e と同型** — caller を開くと callee の応答が返り、かつ
> リンクしていない第三の app には届かない(M6 が塌れていない)。**活体検証は香橙派(prod Linux)**
> でのみ意味を持つ(dev=OrbStack は網隔離も egress も強制しない)。

---

## 0. スコープと確定事項

出すもの:

- **注入の新 kind `service`**:`tbm inject B --into A` で A→B のバインディングを保存し、
  A に `B_URL`(既定名)= `http://<B-subdomain>:<B-container_port>` を注入する。
- **網リンク**:B の稼働コンテナを A の私網 `tsubomi-svc-<A>` へ docker 網別名 = B の subdomain で
  attach し、docker DNS で `<subdomain>` を引けるようにする(直連 = traefik を経由しない)。
- **収束**:caller 側は `ensure_service_network`、callee 側は deploy 後の `attach_as_callee`、
  漏れは reconcile が拾う。eject / 削除は detach / 拆網で掃く。
- 入口:CLI `tbm inject <service>`(横断検索に service を追加)+ web「環境変数」ページの注入下拉。

出さないもの:

- **跨租户リンク**(別ユーザの service へは繋がない。M6 の真の境界 = 租户なので、ここを開けない)。
- **traefik 経由の「HTTP のみ公開」モード**(裸コンテナを露出しない代替案 A)。同一 owner 前提で
  直連を採るので不要(§10-A に否決理由)。
- service の**外部**入口の追加(B の公開は従来どおり `https://b.<domain>` のまま。本書は内部のみ)。

確定する細部(各々**否決可** — 第 4 層 §0 の作法)。コードに落ちていない穴を埋める:

- **§0-A トポロジ方向** = callee を caller の私網へ。caller A は自網のまま、B を A の網に客人として
  入れる。granularity は per-link(A が注入した相手だけ A の網に来る)= 「接続は明示バインディングに従う」
  という tsubomi 全体の作法と一致。
- **§0-B 別名** = B の subdomain(全局 UNIQUE・DNS 安全・予約語で infra 名と衝突しない)。
- **§0-C 内部 URL** = `http://<subdomain>:<container_port>`(http 固定。理由は §9)。
- **§0-D 同一 owner 限定**は**自動で担保**される(`create_injection` の源クエリが既に `user_id=$2`)。
  追加で**自注入禁止**だけ足す。
- **§0-E egress は不変**(§6)。
- **§0-F 新表・新 migration 無し**(`injections.resource_id` は kind 制約の無い裸 FK)。

---

## 1. 着工順序(2 スライス + 文書)

| # | スライス | 範囲 | 検証 |
|---|---|---|---|
| **S0 文書** | 本書 + CLAUDE.md に M6 後続の一段 | — |
| **S1 後端** | `inject.rs` の service 分支 + `create_injection` の白名単/既定名/自注入禁止 + `network.rs` の別名 attach / `attach_as_callee` / 拆網全断 / reconcile の陳腐 callee GC + `docker::run` のフック + 純関数単体 | API で注入作成 → caller 再デプロイ → コンテナ内から `http://<sub>:<port>` 直連が通る |
| **S2 入口** | CLI `resolve_resource` に service 追加 + web「環境変数」下拉に service | `tbm inject b --into a` / web の下拉で service を選べる |

S1 が核(注入解決 + 網収束 = 完了判定)。S2 は人の入口(API は S1 で完結 = curl で端到端テスト可)。

---

## 2. トポロジ(§0-A/B)

```
  tsubomi-svc-<A>  (caller A の私網, /24)
    ├─ A の app コンテナ (network_mode = この網)
    ├─ tsubomi-traefik / -pgbouncer / -valkey  (infra, 既存)
    └─ B の app コンテナ  ← attach(客人。別名 "b" = B の subdomain)
```

- A は自分の私網に居るまま。B は **A の網に追加 endpoint** を持つ(B 自身の網 `tsubomi-svc-<B>` も保持)。
- A は `http://b:<port>` を引く → docker DNS が別名 `b` を B の A 網内 IP に解決 → 直連。
- B が A の網内 IP を持つ ⇒ A の subnet 内 ⇒ egress の `-s subnet -d subnet RETURN` で素通り(§6)。
- 同じ A に複数 callee を入れると、それらは A の subnet を共有し**相互可達**になる(同 owner なので受容)。
- A→B と B→A の相互リンクも可(両方の網に互いが客人として入る。attach は冪等)。

---

## 3. 注入解決(`crates/server/src/services/inject.rs`)

`resolve()` の match に `"service"` 分支を足す(cache 分支と同型):

```rust
"service" => {
    // 内部直連 URL。失効(B 削除済み)→ None でスキップ(env に出さない)。
    if let Some((subdomain, port)) = fetch_service_endpoint(state, resource_id).await? {
        env.push((env_var, format!("http://{subdomain}:{port}")));
    }
}
```

helper(`fetch_cache_creds` と同型):

```sql
SELECT d.subdomain, d.container_port
  FROM resources r JOIN service_details d ON d.resource_id = r.id
 WHERE r.id = $1 AND r.kind = 'service' AND r.deleted_at IS NULL
```

失効の意味論は db/cache/volume と同一:B が削除済みなら env に出さず、A は普通に起動する(`B_URL`
が無いだけ)。B を復元すれば次の deploy で生き返る。**値の解決は注入解決のみ** — 実際に届くかは §4 の
網リンクが担保する(両者は別関心事。env 文字列があっても網が無ければ繋がらない)。

---

## 4. ネットワーク収束(`crates/server/src/services/network.rs`)

### 4.1 別名付き connect

`connect()` を別名対応にする(infra は別名なしで呼ぶ):

```rust
async fn connect(state, network, container, aliases: &[String]) -> AppResult<()> {
    let endpoint_config = (!aliases.is_empty()).then(|| EndpointSettings {
        aliases: Some(aliases.to_vec()),
        ..Default::default()
    });
    let req = NetworkConnectRequest { container: container.into(), endpoint_config };
    // 既接続 403 は冪等に握り潰す(現行どおり)
}
```

### 4.2 caller 側 — `ensure_service_network(A)`(create 前に呼ばれる既存関数)

infra を attach したあとに続けて:**A が注入する service(callee)を列挙** → 各 callee の
**route 後端コンテナ**(`route::backend_container` = 直近成功 deploy の serving 容器)を取り →
A の網へ別名 = callee.subdomain で connect。**走行中の任意コンテナ(`running_container_name`)では
なく route 後端を使う**理由:callee の in-flight な swap 中でも「公開中の版」だけを指し、別名の
取り違え(古い/新しいどちらを掴むか不定)を避けるため(codex 監査)。route 無し(未デプロイ /
停止 = route 撤去済み)なら skip(URL も §3 で空 or 解決不能 = 同じ縮退)。

```sql
-- A の callee 一覧(subdomain も取る = 別名に使う)
SELECT r.id, d.subdomain
  FROM injections i
  JOIN resources r        ON r.id = i.resource_id
  JOIN service_details d  ON d.resource_id = r.id
 WHERE i.service_id = $1 AND r.kind = 'service' AND r.deleted_at IS NULL
```

これで A の deploy 直前(コンテナ起動の前)に callee が網に居る = A 起動時に DNS が引ける。

### 4.3 callee 側 — `attach_as_callee(B, container_name)`(**deploy 成功の route 切替点**で呼ぶ)

B 自身の deploy で新コンテナ名が決まる。**B を注入している A 群**(生存している caller だけ)を
逆引きし、各 A の網へ `container_name` を別名 = B.subdomain で connect:

```sql
SELECT i.service_id                              -- = A(B を注入している caller)
  FROM injections i
  JOIN resources caller ON caller.id = i.service_id
  JOIN resources src    ON src.id = i.resource_id   -- src = B
 WHERE i.resource_id = $1
   AND src.kind = 'service'
   AND caller.deleted_at IS NULL                 -- caller 生存(soft-delete 済みの孤児網に入れ直さない)
```

**呼ぶ位置が肝心**:`docker::run` の start 直後ではなく、**deploy.rs の `route::write` 成功(=公開
カットオーバー)直後・旧コンテナ撤去(`remove_others`)の前**で呼ぶ(codex 監査 Finding 1/2)。理由:

- start 直後だと `commit_success` 前に新版が内部公開され、deploy が後で失敗・巻き戻ると **caller が
  未コミットの版に繋がっていた**ことになる(start-first の安全境界を破る)。route 切替点なら公開と
  内部のカットオーバーが揃う。
- 「新を付けてから旧を消す」順序:旧 endpoint は `remove_others` の旧コンテナ削除で自然消滅 = 別名は
  新へ収束。新を付ける前に旧を消すと一瞬 A→B が切れる。新旧が一瞬同居する間は別名が両者に round-robin
  し得るが、**双方とも健全**(start-first は新の存活確認後に切替)・`remove_others` で即収束 = 公開
  route の「旧が一瞬残る」窓と同等。`remove_others` 失敗時も reconcile の §4.4(3)が掃く。

これが無いと、B の swap で旧コンテナ消滅時に A 網上の endpoint も消え、次の reconcile(最大 30s)まで
A→B が切れる。`attach_as_callee` がこの窓を塞ぐ。best-effort(失敗は log のみ、reconcile が後で拾う)。

### 4.4 reconcile(`reconcile_networks`、30s + 起動時)

- **(1) caller 側収束**:生存 service 各々に `ensure_service_network` を呼ぶ ⇒ §4.2 の `attach_callees`
  が毎 tick callee の route 後端を attach し直す(B が redeploy しても次 tick で拾う)。
- **(2) 孤児私網 GC**:既存。生存 service を持たない管理網を撤去。
- **(3) 陳腐な客人 GC**(追加):生存 caller の私網に居残る「現リンクに無い別 service の app 容器」を
  剥がす。`docker::list_managed`(`(容器id, service_id)`)で容器→service を引き、各 caller 網を inspect、
  **caller 自身でも現リンク先(desired)でもない managed app 容器**を force-disconnect。eject の即時
  `detach_callee`(§7)が取りこぼした客人をここで収束させる(背骨「DB の期望状態へ現実を寄せる」と
  整合 — codex 監査 Finding 4)。infra は `managed` ラベルを持たず `list_managed` に出ないので対象外 = 安全。

### 4.5 拆網 — `remove_service_network(A)`(A 削除時)

現行は infra 3 つだけ disconnect → remove。callee が客人として残ると remove が "active endpoints"
で失敗するので、**網上の全コンテナを inspect で列挙 → 全 force-disconnect → remove** に一般化する
(infra も callee も区別なく剥がす)。

---

## 5. `create_injection` ハンドラ(`crates/server/src/services/mod.rs`)

源リソースの取得クエリを service にも開く + ガード 2 つ:

- kind 白名単 `IN ('database','volume','cache')` → **`'service'` 追加**。
- 取得後 **自注入禁止**:`kind == "service" && req.resource_id == id` なら validation エラー
  (「自分自身は注入できません」)。
- 既定値 match に `"service" =>` 追加:`env_var` 既定を **B の subdomain から導く**
  (大文字化・`-`→`_`・`_URL` 付加。例 `api-backend` → `API_BACKEND_URL`。`validate_env_key` を必ず通る形)、
  `mount_path = None`。subdomain はこの分支で `service_details` から引く(源クエリに service 用の
  subdomain を載せてもよい)。
- **同一 owner は自動**:源クエリは元々 `user_id = $2`(= auth.user_id)で縛っているので、別ユーザの
  service は `NotFound` になる。追加コードは不要。

`InjectionDto.resource_kind` は自然に `"service"` になり、CLI / web は generic に表示する(§8)。

---

## 6. egress は不変(§0-E)

`egress.rs` の FORWARD チェインは「ESTABLISHED 放行 → **各租户 subnet `-s s -d s` RETURN(同 subnet
東西向)** → `pool→私網 DROP` → 末尾 RETURN」。B を A の網へ入れると B は A の subnet 内 IP を得るので、
A↔B は `-s s -d s` RETURN に当たり**素通り**。新しい穴は開かない:

- B が A の subnet に居て届くのは A の subnet 内(= A 自身 + infra + A の他 callee)だけ。別租户の
  subnet へは依然 DROP。
- 別 owner の service へは §0-D で**そもそもリンクを作れない**ので、跨租户の到達性は生まれない。

⇒ `egress.rs` は**触らない**。

---

## 7. 失効と清掃(他注入と同型)

- **eject(リンクだけ外す、B は稼働継続)**:injection 行削除 + `detach_callee` で即 B を A の網から
  外す(`delete_injection` 内、best-effort)。reconcile は外したものを足し直さない(§4.4(1)は現リンクだけ
  attach)。A 再デプロイで `B_URL` も消える。即 detach が失敗しても、**reconcile の陳腐客人 GC(§4.4(3))が
  次 tick で掃く**(背骨どおりの収束。即 detach は UX 即応の eager 前線、reconcile が backstop)。
- **B 软删(ゴミ箱)**:B のコンテナ停止/削除 → A 網上の endpoint 自然消滅 + §3 が注入を空解決。
  A は `B_URL` 欠落で普通に稼働。B 復元 → 次 deploy で生き返る。
- **A 软删**:§4.5 で A の網を全断 → 撤去。B は自網のまま無傷。

---

## 8. 入口(CLI / web)

- **CLI**(`crates/cli/src/commands/inject.rs::resolve_resource`):`tokio::join!` に
  `api::service_list`(既存)を足し、`display_name` 一致で service も候補に入れる。複数種別ヒットの
  曖昧エラーは現行どおり。`tbm service status` の注入表示は `resource_kind` を generic に出すので無改修。
  使用例:`tbm inject api-backend --into web-frontend --as BACKEND_URL`。
- **web**(`web/src/routes/ServiceEnv.tsx`):`useServices` を hooks に追加 → 注入下拉の options に
  `${display_name}(service)` を spread。service 選択時は mount 入力を隠す(`isService`)。
  注入リストの遷移先 `resourceHref` に service(`/services/<id>`)を追加。説明文は service を
  「の内部 URL」と表示。

---

## 9. http / Host の正直な差異(受容済み・bug ではない)

公開串 `https://b.<domain>` と内部串 `http://b:<port>` は**同じ B プロセス**に当たる(traefik は
HTTP 意味論を透過転送するだけ)。機能は同一だが、内部串は edge 層を通らないため次が異なる:

- **http(TLS 無し)**:内部網なので不要だが、B が http→https 強制リダイレクトや Secure cookie の
  みを発行する設計だと内部 http で不整合になり得る。純粋な API 呼び出しは問題なし。
- **Host ヘッダが `b:<port>`**(`b.<domain>` ではない):vhost ルーティングや Host から絶対 URL を
  組む app は差が出る。大半の service 間 API では無関係。
- **IP 許可リスト / middleware を通らない**(内部なので意図どおり)。

これらは設計上の受容点。`doc/paas-cache-public-design.md` の「直連 = 値は隔離されるが…」と同じ性質の
正直な開示。

---

## 10. 否決(後相)

- **§10-A traefik 経由の「HTTP のみ公開」モード**(代替案 A):A が B の裸コンテナを摸れない代わりに
  traefik 一跳を挟む。同一 owner 前提なら裸コンテナ到達は自分の爆発半径内 = 受容できるので、より速く
  部署時の零件も少ない直連を採用。HTTP のみに限定したい要件が将来出たら再考。
- **§10-B 跨租户リンク**:M6 の真の境界を破るので出さない。どうしても要るなら共有 viewer 的な別設計。
- **§10-C per-owner メッシュ網**(owner の全 app を一網に)。reconcile は単純化するが「接続は明示
  バインディングに従う」原則を崩し、未リンクの app まで繋がる驚きがある。per-link を採る。

---

## 11. 完了判定(香橙派 e2e)

`callee`(`GET /ping`→`pong-from-callee`)と `caller`(`GET /` で `fetch($CALLEE_URL+"/ping")`)を
本番に部署 → `tbm inject callee --into caller --as CALLEE_URL` → caller 再デプロイ →
`https://caller.<domain>/` が `pong-from-callee` を返す(= 内部直連が公網を通らず成立)。かつ
**リンクしていない第三の app には caller から届かない**(M6 未塌)。dev では網隔離が強制されないため、
この判定は **prod Linux でのみ**有効。
