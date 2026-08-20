# tsubomi デプロイ手順(tbm CLI)

tsubomi(蕾)= 社内 PaaS(基礎版 Vercel + Neon)。ユーザ(多くは非エンジニア)に代わって、
この手順で app を本番 `https://<名前>.tsubomi-app.com` へデプロイする。
リソースは 4 種(**service** / **database** / **volume** / **cache**)、動詞は「**注入**」ひとつ。

**このプラットフォーム(tsubomi)のアーキテクチャは {{HOST_ARCH}} です。** デプロイするイメージは
このアーキテクチャで動く必要がある(`tbm whoami` / `tbm --help` の出力でも確認できる)。

> このファイルを読んだら、まず `tbm whoami` で疎通・ログイン状態・プラットフォーム / 手元のマシンのアーキを
> 確かめてから始める。

## 0. 絶対に外さない 3 点

1. **検証は必ず `curl` で 2xx を確認する。`tbm service status` の "running / succeeded" を信用しない。**
   デプロイ門禁は「`container_port` で TCP を受けた」までしか見ない — listen していても
   HTTP が 500 を返す / assets が 404 で画面が真っ白、は succeeded になる。**真実は curl だけ**
   (`tbm service verify` が子リソースまでまとめて見る)。
2. **注入はデプロイの「前」に行う。** 値はコンテナ起動の瞬間に解決される。注入し忘れたまま
   デプロイすると env が無い。**cache** の rotate も再デプロイして初めて効く(db の rotate は
   human role だけなので実行中の app は無影響 — §「順序:注入 → デプロイ」)。
3. **外向き・破壊的な操作はユーザに一言断ってから。** GitHub repo の作成、リソース削除など。

CLI の出力は捕捉時(非 TTY)に自動で JSON。`jq` で id を拾える。エラーは `{"error","code"}` を
stdout に出して非零終了 — `code` で機械分岐(`unauthorized`/`conflict`/`validation`/`not_found`/…)、
メッセージは次の一手を含むので素直に従う。

## 1. 前提を整える

- **ログイン**:`tbm whoami`。失敗したら `tbm login`(GUI はブラウザで「許可」、SSH 先は
  `tbm login --manual` でコピペ方式)。
- **デプロイ可能な形か**:
  - **Dockerfile があればそれが使われる。** 無ければ GitHub 経路では nixpacks が言語を
    **プロジェクト自身の宣言**(`package.json` / `go.mod` / `requirements.txt` / `Gemfile` 等)から
    自動判定してビルドする。**スタックを勝手に仮定して Dockerfile を捏造しない。** 今あなたが
    書いたプロジェクトなのでスタックは分かっているはず。
    - 例外:静的サイト(Next.js の `output: 'export'` 等、サーバを持たないビルド)は nixpacks が
      `start` を見つけられない。その時**だけ**、`next.config` を読んで判明したモードに合う最小の
      Dockerfile か start コマンドを足す(配方は Vercel 等の公式 example に従う)。
  - **バージョンを明示指定しないなら最新の安定版を使う。** 自分で Dockerfile や start を足す場面では、
    `node:20` のような旧版固定に落とさず現行の安定版(LTS など)を選ぶ。古い既定にしない。
  - **アプリは service の `container_port` で listen する**(既定 **8080**。create 時に
    `--port <PORT>` で変更可 — 現成イメージが固定ポートで listen する場合はそちらに合わせる)。
    `tbm service create` の出力や `tbm service status` の `container_port` を見て、アプリの
    listen ポートを一致させる。**ズレるとデプロイが readiness 門で failed になる**(エラーに
    port の突き合わせ方が載る)。

## 2. リソースを作る(必要なものだけ)

> **create の前に決めるのは `--port` と「listen するか」だけ**。
> port だけは作成後に変更できない(route / 内部リンクの真源 — 間違えたら作り直し)。
> それ以外は後から変えられる:`tbm service visibility <名前> <private|company|public>` /
> `tbm service rename <名前> <新名>`(表示名のみ)/
> `tbm service subdomain <名前> <新subdomain>`(公開 URL の変更。**旧 URL は即失効**・GitHub repo 名は不変・
> この service を注入している呼び出し側は再デプロイで新値)/
> `tbm service limits <名前> [--memory <MiB>] [--cpus <N>|none]`(**次のデプロイから反映**)/
> `tbm service stateful <名前>`(false→true の一方向のみ)。
> 作成直後の表示に port / visibility / stateful / memory が出るので、port を間違えたらその場で作り直す。
>
> **作り直しは delete → 同名 create でそのまま通る**:`tbm service delete` は**ゴミ箱への
> soft delete** だが、ゴミ箱は名前を占有しない。`tbm trash purge` を挟む必要はない。
> 注意点は復元側:同名で作り直した後に古い方を `tbm trash restore` すると稼働中の同名リソースと衝突して 409 になる
> (先に稼働中の方を rename か delete)。同名がゴミ箱に複数堆積したら `tbm trash list` の id で特定する。

- service:`tbm service create <名前>`(名前の slug が subdomain になる。
  `--subdomain <sub>` で明示指定も可 — 使用中なら 409)。**GitHub 連携は既定**
  (repo/secret/variable と workflow 設定までこの 1 回で済む。secret は stdin 直達で
  出力に出ない。§4 参照)。gh が無ければ `setup_commands` が返る(手動 fallback)。連携せず
  resource だけ作るなら `--no-github`(応答に deploy_key / registry pass の**秘密が平文で載る** —
  必要なときだけ)。**プラットフォームは GitHub に触れない** — gh を使うのはあなた。
  - 任意フラグ:`--port <PORT>`(listen ポート。既定 8080。**8080 以外を指定すると公開範囲の既定が
    `private` になる** — 非 HTTP コンテナ想定。`--visibility` で上書き可)/ `--stateful`(持ち込み DB 等の
    ステートフルコンテナ。デプロイが stop-first = 数秒瞬断と引き換えにデータディレクトリを保護)/
    `--memory <MiB>`(上限。既定 1024)/ `--cpus <N>` / `--subdomain <sub>`(公開 URL の
    サブドメイン)。**port 以外は作成後にも変更できる**(上の一覧。OOM なら
    `tbm service limits <名前> --memory <MiB>` を上げて再デプロイ — 作り直し不要)。
- database:`tbm db create <名前>`
  - **dev/検証環境用の DB が欲しい → `tbm db fork <元> <新名> [--schema-only]`**(この瞬間の
    構造 + データごと複製。migration 再生や手動データ投入は不要。`--schema-only` = 構造だけ。
    fork 後は同期されない = 汚してよい。大きな DB でタイムアウトしたら `--schema-only` を検討)
- volume:`tbm volume create <名前>`(ファイル永続が要るなら)
- cache:`tbm cache create <名前>`(valkey が要るなら)

## 3. 注入(デプロイの「前」に!)

| 注入元 | コマンド | コンテナに入る env |
| --- | --- | --- |
| database | `tbm inject <db名> --into <service名>` | `DATABASE_URL` |
| volume | `tbm inject <vol名> --into <service名> [--mount /data/foo]` | `STORAGE_PATH` |
| cache | `tbm inject <cache名> --into <service名>` | `REDIS_URL` + `REDIS_KEY_PREFIX` |
| service | `tbm inject <svc名> --into <service名>` | `<名前>_URL`(内部直接接続 http)+ `<名前>_HOST` / `<名前>_PORT` |

service 注入 = 別 app への**内部直接接続**(インターネットを通らない。同一 owner 限定)。HTTP app は `_URL` を
そのまま使い、**非 HTTP(持ち込み postgres 等)は `_HOST` / `_PORT` で自分のスキームの接続文字列を組む**
(例 `postgres://user:pass@${MYPG_HOST}:${MYPG_PORT}/db` — パスワードは自分が env で設定したもの)。

確認:`tbm service status <service名>` の `injections` がすべて `valid: true`。

**接続文字列は「env 名」で繋ぐ(値は環境ごとに解決:ローカル=公開 / 本番=内部)。**
注入は **env 名にそのまま値を生成する**(内容マッチではない)。本番は起動時に**内部接続文字列**
(app role・内部入口・社外に出ない)を `DATABASE_URL` に入れる。開発機で使う
**公開接続文字列**(`tbm db url`。human role・外部入口)とは **同じ env 名で繋ぐ** — コードは
`process.env.DATABASE_URL` を**読むだけ**で、値はローカル=公開 / 本番=注入と別物。両者は別環境にしか
存在しないので**衝突せず無縫に切り替わる**。これを成立させる 3 点:

- **env 名を一致させる**:既定は `DATABASE_URL`。既存リポジトリは、コード / `.env.example` が読む名前を
  確認し、違えば `--as <その名前>` で注入名を寄せる(`tbm inject <db> --into <svc> --as <NAME>`。cache は
  `<NAME>_KEY_PREFIX` も併せて入る)。確認は `injections[].env_var` と `process.env.XXX` の突き合わせ。
- **接続文字列をコードに直書きしない**(必ず env 名を読む)。直書きは env をすり抜け、本番でも公開経路に出る。
- **公開文字列を本番に持ち込まない**:`.env` は `.gitignore` + `.dockerignore`(**イメージに焼かない**)、
  `tbm env set <名前> DATABASE_URL=<公開>` も**しない**。持ち込むと公開経路(外部入口)に出て、同一ホストの
  DB に**インターネットを一周**(遅延)+ `tbm db rotate` で**黙って切れる**(注入の内部文字列はどちらも無い)。

### 3.1 `DATABASE_URL` の TLS はドライバで扱いが違う(まず注入ホスト名を見て分岐する)

`sslmode=require` の意味が**ドライバで割れている**:libpq(Go / Python)は「暗号化のみ・証明書は
検証しない」、Node の `pg` は「**厳格に検証**」。だから同じ URL が Go では繋がり Node では落ちる。
どちらに転ぶかは**注入ホスト名が証明書と一致しているか**で決まるので、まずそれを見る:

```
tbm env list <名前> --resolved   # DATABASE_HOST の値を見る
```

- **`DATABASE_HOST` が `db.` で始まる** → 証明書(公的に信頼される LE 発行)と名前が一致する部署
  (この名前は**内部網の docker 別名**で、外部入口ではない — 通信は網内に留まる)。**厳格検証で通る**ので `DATABASE_URL` をそのまま渡してよい:
  - **Go / Python / Node(`pg`)**:そのまま。`rejectUnauthorized:false` は**不要**。
  - **Rust(`postgres` / `tokio-postgres`)**:`NoTls` では `require` に繋がらないので TLS コネクタを
    渡す。検証は有効のままでよい:
    ```rust
    let c = native_tls::TlsConnector::new()?;
    let mut db = postgres::Client::connect(&url, postgres_native_tls::MakeTlsConnector::new(c))?;
    ```
- **`DATABASE_HOST` がコンテナ名(`tsubomi-pgbouncer` 等)** → 証明書の名前と食い違う(または自己署名の)
  部署。**厳格検証は通らない**ので、検証を切る側に寄せる:
  - **Node(`pg`)**:接続文字列由来の ssl 設定が明示 `ssl` を上書きするので、**URL から `sslmode` を
    外して**から渡す:
    ```js
    const u = new URL(process.env.DATABASE_URL); u.searchParams.delete("sslmode");
    const pool = new pg.Pool({ connectionString: u.toString(), ssl: { rejectUnauthorized: false } });
    ```
  - **Rust**:`danger_accept_invalid_certs(true)` を付ける。
  - **Go / Python**:そのままで繋がる(libpq は検証しない)。
- **`DATABASE_SSLMODE` が `disable`**(dev 等)→ TLS 無し。Rust は `NoTls` でよい。
- **証明書エラーが出て、かつ上の分岐と食い違う** → まず `tbm deploy` で**再デプロイ**する。注入値は
  **コンテナ起動の瞬間**に解決されるので、走っているコンテナは古いホスト名を握っている可能性がある
  (`--resolved` は「次のデプロイでこうなる」値を見せる)。
- **cache を使う Node アプリ(ioredis)**:**必ず `redis.on("error", …)` を付ける**(未listen の error
  イベントは "Unhandled error event" でプロセスごと落ちる = 起動直後 exit の典型。DB の TLS とは別件だが
  同じ「起動直後 exit」症状になる)。

**URL を組み直したい / URL が使えないとき**は、同じ注入から**素材の env** も入っている:
`DATABASE_HOST` / `DATABASE_PORT` / `DATABASE_USER` / `DATABASE_PASSWORD` / `DATABASE_NAME` /
`DATABASE_SSLMODE`(`--as MYDB_URL` で注入したなら `MYDB_HOST` 等 = `_URL` を剥いだ基底に付く)。
自分のドライバの作法で接続設定を組める(ORM が URL を受け取らない場合や、TLS を明示制御したい場合)。

迷ったら **起動時ではなくリクエスト時に DB へ繋ぐ**と、失敗が「起動直後 exit」ではなく
レスポンスのエラーに出て切り分けやすい。

### 3.2 持ち込みコンテナ(managed database で足りない時:拡張入り Postgres・meilisearch 等)

プラットフォームの database(pg-tenant)には**拡張を入れられない**。pgvector 等が要るときは、DB を
**stateful service として自分で立てて**リンクする:

```
tbm service create mypg --port 5432 --stateful        # 非8080 → 自動で private(公開URLなし)
tbm volume create mypg-data
tbm inject mypg-data --into mypg --mount /var/lib/postgresql/data   # データディレクトリの永続化(必須!)
tbm env set mypg POSTGRES_PASSWORD=<自分で決める>
tbm deploy --image pgvector/pgvector:pg17 --service mypg   # サーバ側で pull(docker/GitHub 不要)
tbm inject mypg --into <app名>                         # app に MYPG_HOST / MYPG_PORT が入る
```

現成イメージに **数行の定制**(拡張の追加インストール等)が要るだけなら、COPY/ADD 無しの
Dockerfile を書いて `tbm deploy --dockerfile ./Dockerfile --service mypg`(これもサーバ側で
ビルド — §4 の第 3 経路)。

- **volume 注入を忘れない**:コンテナはデプロイごとに作り直される。データディレクトリを volume に
  マウントしないと**再デプロイでデータ全損**。マウント先はそのソフトのデータパスに合わせる
  (postgres = `/var/lib/postgresql/data`)。
- **`--stateful` を忘れない**:無いと再デプロイ時に新旧コンテナが同じデータディレクトリを同時に開き
  **データ破壊**になり得る。stateful のデプロイ / 停止は数秒の瞬断がある(仕様)。
- 接続文字列は app 側で `_HOST` / `_PORT` + 自分の設定したパスワードで組む(§3 の表)。
  中身(ユーザ・スキーマ・チューニング・アップグレード)は**全部ユーザの責任** — プラットフォームが保証するのは
  「活きている・データが在る・app から届く」まで。
- 外部(手元の psql 等)からは繋げない(公開入口は HTTP のみ)。操作は
  `tbm service exec mypg -- psql -U postgres -c "..."` で。
- 検証:private でも `tbm service verify` が**内部ネットワークの TCP 疎通確認**で使える(port で接続を
  受けるかまで。中身の検証は `tbm service exec` で書き込み → 読み戻し)。

### 3.3 訪問者の実 IP はヘッダで来る(使うかは任意)

app は HTTP リクエストヘッダで**訪問者の実 client IP** を受け取れる(プラットフォームが提供する。
使う/使わないは app 次第):

- `CF-Connecting-IP` — 正準。Cloudflare が必ず付ける(単一の実 IP)。
- `X-Forwarded-For` / `X-Real-Ip` — プラットフォームの Traefik が `CF-Connecting-IP` から埋める。
  標準ライブラリ(多くは XFF を読む)もそのまま実 IP を得る。

**可信**:入口は Cloudflare Tunnel のみ・直アクセス不可なので、クライアントはこれらを偽造して届かせられない
(CF が edge で上書きする)。`req.socket.remoteAddr` 等の**生の接続元はプロキシ(内部 IP)**になるので、
実 IP が要るなら上のヘッダを読むこと(`process.env` の注入値ではない — 実行時のリクエストヘッダ)。

## 4. デプロイ — 経路を選ぶ

**まず闸門:デプロイ経路は 3 つ。** どれを使えるかは「何をデプロイするか」と「手元に何があるか」で決まる:

1. **GitHub Actions の枠が残っている** → 既定の GitHub 経路(CI が両アーキでビルド)。
2. **プラットフォームと同じアーキ({{HOST_ARCH}})の Docker が手元で動く** → 退路 `tbm deploy --local`。
3. **既成イメージ、またはコンテキスト無し Dockerfile(FROM/RUN 等のみ、COPY・ADD 不可)で足りる**
   → `tbm deploy --image <ref>` / `--dockerfile <path>`(**サーバ側**で取得/ビルド —
   GitHub もローカル docker も不要)。サーバが未対応なら CLI が「デプロイエンドポイントが見つかりません
   …サーバ更新が必要」と明示エラー(code=not_found)を返すので、対応版か迷ったら実行して確かめてよい。

経路 3 は **app のコードをイメージに入れられない**(COPY 不可 = 無 context の契約)。自分の
コードをデプロイするのは 1 / 2 のみ。逆に持ち込み DB・valkey・meilisearch 等の**インフラコンテナは
経路 3 が最短**(§3.2)。

**1 と 2 が満たせず、経路 3 でも足りない**(= app のコードをビルドする必要がある)ときは、
この環境ではデプロイできない — それが正しい結論。手を止めてユーザにそう伝える。**これら以外の
経路を勝手に発明しない。** app のビルドは**ユーザ機か CI で行う設計**であって、ビルド環境を
用意するかは**ユーザ側の判断**(同アーキ機を使う / Docker を入れる / GitHub の枠を空ける)。

### 既定:GitHub 経路(`gh` を使う。CI が build/push)

service を **§2 で作成済み(gh が使える環境)**なら GitHub 連携は完了している(既定 —
フラグ不要):プラットフォームが `gh` 経由で repo 作成・secret / variable 設定・
`.github/workflows/tsubomi-deploy.yml` の書き出しまで実施済み(秘密は stdin 渡しで `ps` にも
出力にも出ない。**Windows / mac / Linux どの shell でも動く**。create 出力 JSON の
`github.configured` が true なら完了)。あとは `git add/commit/push` → GitHub Actions が自動でビルド &
デプロイ。

**一括で回すなら `tbm deploy --watch`(推奨)。** `git add/commit` 後にこれ 1 本で:未 push なら
push → GitHub Actions の run を追跡(URL を表示)→ CI 成功後、その commit のデプロイ完走を待って
検証(§5 の子リソース検証まで)を自動でやる。手で `git push → run 確認 → status ポーリング → verify` を
繰り返す必要がない。`gh` が要る(無ければ上のインストール案内、または `--local` へ)。全体の待ち上限は
`--timeout <秒>`(既定 900)。**要点:commit は自分でやる**(--watch は未 push を push するだけで
`git add`/`commit` はしない)。CI が失敗したら失敗ログを出して非零終了する。

補足:**サービスが複数でも service の repo 内なら `--service` 不要**(repo の
`TSUBOMI_SERVICE_ID` variable から自動推断)。**初回で追跡ブランチ(upstream)が未設定でも自動で
`git push -u <実際の remote 名> <branch>` する**(remote は `tsubomi` 優先 — `origin` とは限らない)。
HEAD 以外の commit を追うなら `--for-sha <sha>`(verify と同型)。

- **連携がまだの場合**(旧 CLI / `--no-github` で作った、または作成時に gh が無かった):
  create 応答(JSON)の `setup_commands`(`gh repo create` / `gh secret set` / `gh variable set`。
  **POSIX shell 前提**)を service の repo 直下で順に実行すれば同じ状態になる(値は
  `GET /services/{id}/deploy-config` でも再取得可 = `tbm deploy --local` が使うエンドポイント)。
  Windows(PowerShell)では `printf` / `$(…)` が動かないため bash 系で。
- **`gh` が入っていない** → インストールを案内する:
  - mac:`brew install gh`
  - Debian/Ubuntu:`sudo apt install gh`(または公式 apt repo)
  - Windows:`winget install GitHub.cli` か `scoop install gh`

  ログインは**対話的**でAIは代行できない。ユーザに次を打ってもらう:`! gh auth login --web --git-protocol https --clipboard`。
- **`gh` の Actions 額度が切れた / billing・quota エラーで CI が回らない**(私有 repo の無料枠超過など)
  → 下の **`tbm deploy --local` 退路**に切り替える。
- **既存コードのあるディレクトリで作る場合**:GitHub 連携(既定)は「git repo でも空でもない
  ディレクトリ」では誤 push 防止のため拒否される。デプロイ対象なら先に `git init -b main` してから
  `tbm service create <名前>` を実行する(空ディレクトリ / 既存 repo ならそのままでよい。
  連携自体が不要なら `--no-github`)。
- **ビルドが遅い(数十分)場合**:CI のランナーは gh variable `TSUBOMI_RUNNER` で決まる。新規 service は
  プラットフォームが自動設定するが、**古い service は未設定 = amd64 + QEMU で極端に遅い**。プラットフォームが arm64 なら
  `gh variable set TSUBOMI_RUNNER --body ubuntu-24.04-arm` で原生 arm になり数分に縮む(yml 変更不要、
  次の push から有効)。

### 退路:`tbm deploy --local`(GitHub 非依存。ローカルの Docker で build+push)

```
tbm deploy --local --service <service名> --context <Dockerfile のあるディレクトリ>
```

GitHub 額度切れ時の主たる代替でもある。要 Docker。

- **build はあなたのマシンで走る — アーキを合わせる。** `tbm whoami` で **プラットフォームの
  アーキテクチャ**(デプロイ対象)と **現在のマシンのアーキテクチャ**(ビルド機)が一致するか確認する。
  違えばクロスアーキ build(QEMU、遅い / 失敗しやすい)になる → 同アーキのマシンか GitHub 経路を使う。
- **Docker が無い / 起動していない**(`docker info` が失敗)→ ユーザに **Docker Desktop** の
  導入を案内する(https://www.docker.com/products/docker-desktop/ )。インストールと起動は
  GUI・対話なのでユーザにやってもらい、`docker info` が通ってから再実行する。
- **Windows(git-bash / MSYS)のパス化け**:volume の遠端パス / `inject --mount` は tbm が
  **自動で復元する**(EXEPATH 検出。手当て不要)。**ローカルパス**(`--context` / `--dockerfile` /
  volume put のローカル側)は MSYS の変換が正しい動きなのでそのまま。純 MSYS2 等で復元できない旨の
  エラーが出たときだけ `MSYS_NO_PATHCONV=1 tbm …` を前置するか、先頭 `/` の無い相対パスで指定する。

### 第 3 経路:`tbm deploy --image / --dockerfile`(サーバ側で取得/ビルド。GitHub・docker 不要)

```
tbm deploy --image pgvector/pgvector:pg17 --service <service名>          # 既成イメージ
tbm deploy --dockerfile ./Dockerfile --service <service名>               # 無 context Dockerfile
tbm deploy --image traefik/whoami --service <service名> --watch          # --watch = 完走待ち + 検証
```

**サーバ側**で pull / build して内部 registry へ push し、通常のパイプラインで起こす。手元に
何も要らない(gh も docker も)。202 即返しで取得〜起動は非同期 — 返ってくる `git_sha` を
`tbm service verify <名前> --wait --for-sha <git_sha>` に渡すと完走まで待てる。`--watch` は
これを自動でやる:**公開サービスは URL + 子リソースまで検証**、**private サービスは完走待ち +
内部ネットワークの TCP 疎通確認**(port で接続を受けるかまで確認。中身の動作確認は `tbm service exec` / `logs`)。

- **--dockerfile は COPY / ADD 不可**(コンテキスト無しの契約。multi-stage も不可、上限 8KiB)。
  使えるのは FROM / RUN / ENV / ARG / CMD / ENTRYPOINT / EXPOSE / WORKDIR / USER / LABEL /
  HEALTHCHECK / SHELL / STOPSIGNAL。**app のコードを入れる用途には使えない**(それは経路 1 / 2)。
- **VOLUME も不可**:匿名 volume はデプロイごとに消える罠。永続はプラットフォームの volume 注入(§3.2)で。
- イメージはプラットフォームのアーキ({{HOST_ARCH}})版が必要。無ければ取得段階で明確に失敗し、
  エラーが deploys に載る(`tbm service status` / `deploys` で見える)。
- 履歴上の表示:`git_sha` は配方のハッシュ(純 hex 12 桁)、見出し(`image: <ref>` 等)は
  commit_message 欄に入る。rollback は通常どおり(取得前に失敗した行だけは digest 未確定で
  rollback 不可 — 明確な 400 が返る)。

### push が 413(Payload Too Large)で失敗する — 単層 100MB 上限

registry は(既定で)Cloudflare 経由のため **イメージ 1 層あたり圧縮後 ≈100MB** が上限(CF の
request body 制限。registry 側では変えられない)。超えると `tbm deploy --local` でも GitHub Actions
でも push が 413 で落ちる。

- **この部署に直接接続入口が設定済みなら 413 は起きない**:プラットフォームが push 先を CF 非経由の直接接続 registry
  に振り向けている(`tbm service create` 応答の registry host が `registry-direct.<ドメイン>` 系なら該当)。
  それでも 413 が出たら直接接続入口の障害を疑い、ユーザに知らせる(勝手に別経路を作らない)。
- **直接接続入口が無い部署**での対処は**層を小さくする**:大きな `RUN`/`COPY` を分割 / slim・alpine 基底 /
  マルチステージでビルド中間物を最終イメージに持ち込まない。恒久対策(直接接続入口の追加)は運用側の
  判断 — `doc/paas-registry-direct-design.md`。

## 5. 検証(ここを省かない)

1. `tbm service status <service名>` で `phase=running`・最新 deploy が `succeeded` を確認
   (`visibility` 行で公開範囲も見える)。
2. **`tbm service verify <service名>`** を使う。根 HTML を取り、そこが参照する js/css 子リソースまで
   2xx かをまとめて確認する(`ok:true` で成功。NG なら exit 1 + どのリソースが落ちたか)。
   **デプロイ直後は `--wait` を付ける**(`tbm service verify <service名> --wait`):進行中の
   デプロイの完走を待ってから検証する(deploy 送信〜切替は非同期で数秒〜数十秒かかる。
   `--wait` 無しで即叩くと旧版や 502 を見る。デプロイが failed ならその error を出して非零終了 =
   status の手動ポーリングは不要)。上限は `--timeout <秒>`(既定 180)。報告には現在 serving 中の
   デプロイ(`serving.git_sha` / `deploy_id`)も載る = 「見ているのが自分の新版か」が分かる。
   **エンドツーエンドでで確実にするなら `--for-sha <sha|HEAD>`**(`tbm service verify <名前> --for-sha HEAD`):
   その commit のデプロイが**到着してから**完走を待つので、GitHub 経路で CI がまだビルド中
   (hook 未達)の窓もカバーする(`--wait` 単体はこの窓を待てず旧版を検証してしまう)。
   `deploy --watch` は内部でこれを使うので、--watch を使うなら verify は自動で済む。
   **`visibility=private` のサービスは公開 URL の代わりに内部ネットワークの TCP 疎通確認で検証する**:
   verify がサーバ側から serving コンテナの `container_port` へ単発 connect し、`ok` を三値で返す —
   `true` = listen 確認(ただし TCP まで。HTTP 応答の中身は見ない)/ `false` = 異常(走っていない、
   または内部リンクの callee なのに listen していない → exit 1)/ `null` = 判定不能(listen しない
   worker 型は正常があり得るため罰しない → exit 0。**自動分岐は exit code でなく json の `ok` を
   三値で読む**)。報告の `serving` でどの版を探ったか照合できる。中身まで確かめるなら
   `tbm service exec <名前> -- wget -qO- localhost:<port>` か、内部リンク先の caller コンテナから
   `tbm service exec <caller> -- wget -qO- http://<subdomain>:<port>`。
   **`landed_noservice` が付いていたら「そのサブドメインに生きた route が無い」**(未デプロイ / 停止中 /
   削除済み / route 反映待ち)= 必ず `ok:false`。app の中身の問題ではないので、assets を疑わず
   `tbm service status <名前>` で phase と最新デプロイを見る(停止中なら `tbm service start`)。
   **これが重要な理由**:`status=succeeded` + 根 200 でも、`index.html` が参照する `/assets/*.js` が
   404 だと**画面は真っ白**になる。根への素の `curl` はこれを見逃す。verify は子リソースまで見る。
   - **verify の root_status が 502** → デプロイ門禁(TCP 探測)は通過した後にコンテナが
     落ちた可能性(起動数秒後のクラッシュ等)。`tbm service metrics`(再起動回数 / OOM)と
     `tbm service logs` で原因を見る。
   - **root は 200 だが子リソースが 404**(verify が `ok:false`)→ ビルドの出力パス / `base` 設定 /
     直近デプロイの失敗が典型。
   - `tbm service cat <service名> <パス>` でコンテナ内のファイル(ビルド成果物・設定)を直接確認できる
     (`exec -- cat` の糖衣)。`tbm service exec <service名> -- <cmd>` で任意コマンドも。
   - **実時ログ**は `tbm service logs <名前> --follow`(Ctrl-C / パイプ切断まで tail。`--since 5m`
     で遡り開始)。**稼働指標**は `tbm service metrics <名前>`(CPU / メモリの上限比 / 再起動回数 /
     uptime / OOM = クラッシュループ・OOM の切り分け)。**デプロイ履歴**は `tbm service deploys <名前>`
     (rollback の戻し先 id 選び)。**アクセス統計**は `tbm service stats <名前> [--days N]`
     (リクエスト数 / 訪問者 / デバイス / ブラウザ / Top パス / 国 / リファラ。口径はリクエスト単位 —
     pageview ではない。訪問者は bot 除外。private や M6 内部リンクの流量は公開入口を通らないので載らない)。
   - **自分が起こしていないデプロイが履歴に居るとき**は `trigger` を見る(`deploys` の json /
     text のラベル):`reconcile` = コンテナ消失からの自動復活 / `caller_relink` = この service を
     注入している相手が改名されたので平台が追従させた / 無印(`user`)= 誰かが明示的に起こした。
     **同じ commit 件名の行が並ぶのは正常**(再デプロイは同じ版を起こし直すため)。
3. DB / volume / cache を使うなら、実際に「書き込み → 読み戻し」で永続と隔離を確かめる。DB 側の
   読み戻しは **`tbm db query <db名> "<SQL>" --tsv`** が速い(psql 不要。`--tsv` = 行だけの
   タブ区切り・列名なし・NULL は空 — スカラーなら `$(…)` で一発捕获。表計算向けにヘッダ付き CSV は
   `--csv`。構造が要るときは `-o json` の `results[].rows`。結果は 1 文あたり最大 1000 行で切り詰め —
   大結果はアプリのドライバで)。値を安全に束ねるなら **`--param`**(位置バインド $1..$n。手動
   エスケープ不要。型は SQL 側で `$1::int` と明示。NULL は SQL に直書き)。
   注入した値が何に解決されるかは **`tbm env list <service名> --resolved`**(由来付き・秘密は伏せる)
   で確認できる — 探针を書かずに「B_URL が何を指すか」等が分かる。反映はデプロイ時なので
   cache の rotate 後は要再デプロイ(db の rotate は app に無影響 — §「順序:注入 → デプロイ」)。

### 順序:**注入 → デプロイ**(逆にすると env が現れない)

値は**コンテナ起動の瞬間**に解決される(注入表はバインディングしか持たない)。つまり:

- **走っている service に後から注入しても、そのコンテナには入らない**。`tbm deploy`(または
  `tbm service stop <名前> && tbm service start <名前>`)で作り直すまで env は現れない。`tbm inject` は実行中なら
  「今動いているコンテナには入っていません」と言い、`-o json` では `needs_redeploy: true` を返す。
- **`tbm cache rotate` の後は再デプロイが要る**:cache は資格情報が 1 本 = **注入値そのもの**が
  変わるので、実行中の app は古いパスワードを握ったまま即座に認証エラーになる(`status` の
  `未反映` にも出る)。
- **`tbm service subdomain` の後、その service を注入している呼び出し側は再デプロイが要る**:
  `_URL`/`_HOST` の中身(= 内部ホスト名)が変わるので、実行中の caller は旧ホスト名を握ったまま
  断線する(caller 側 `status` の `未反映` にも出る)。
- **`tbm db rotate` は再デプロイ不要**:回すのは **human role**(外部接続用)だけで、注入されるのは
  **app role** なので実行中の app は切れない(外部 key の rotate が service を切らないための意図した
  設計)。影響するのは、公開接続文字列を**静的 env に置いてしまった**場合だけ(§3 冒頭)。
- **症状が原因を指さない**:app からは「env が無い」「接続できない」としか見えないので、
  **まず `tbm env list <名前> --resolved` と `tbm service status`(注入一覧に `未反映` が付く)を見る**。
  env が resolved には在るのにコンテナに無い = この順序問題。再デプロイで直る。
- 逆順にしないコツ:**`tbm service create` → 注入を全部済ませる → 最初の `tbm deploy`**。

### 注入元の subdomain を変えたとき — 呼び出し側は自動では直らない

`tbm service subdomain B <新名>` は **B を注入している A の内部リンクをその瞬間に切る**:A の
コンテナ内の `B_URL`/`B_HOST` は起動時に凍結された旧 subdomain のままなのに、docker 網別名は
新 subdomain へ付け替わるため、A からは `bad address` になる。

- **改名の前に** `tbm service callers B` — 誰が影響を受けるか(0 件なら何も気にしなくていい)。
- **改名の後に** `tbm service subdomain B <新名> --redeploy-callers`(1 コマンド)か
  `tbm service redeploy-callers B`(後から / 失敗の回収。改名と独立に何度でも実行できる)。
- **自動対象外**:停止中(起こさない — 次の起動で新しい値が入る)/ 未デプロイ / デプロイ進行中 /
  stateful(実停機を伴うので手動)。理由は `skip_reason` に出るので**それを読んで次の一手を決める**。
  自分で `desired_state` から判定し直さないこと(サーバの判定と食い違う)。
- 応答は **202 = 計画**であって完了ではない。結果は `tbm service callers B` を引き直して
  各行の直近デプロイ状態を見る(`[直近デプロイ失敗]` とエラー先頭が出る)。
- 同時に走れるのは 1 バッチだけ(`conflict` = 進行中。完了を待って再実行)。

### 起動直後にクラッシュする(deploy failed)— 当てずっぽうで再デプロイしない

コンテナが起動即 exit した失敗デプロイは、**エラーに終了要因(exit code / OOM —
docker events 由来なので速い crash-loop でも取れる)とログ末尾が載る**。`tbm service status <名前>` / `verify --wait` の error を**まず読む**:

1. **exit code を読む**(`exit=…` 形式。これだけで原因の当たりが付く):
   - `exit=0` = プロセスが何もせず正常終了。**CMD がサーバを起動していない / ビルド成果物が
     空(コンパイルが効いていない)の典型** — コードではなくイメージを疑う。
   - `exit=101` = Rust の panic。ログの panic メッセージを見る(DB/cache 接続由来の典型は §3.1)。
   - `exit=126/127` = コマンド実行不可 / 不在。実行ビット・アーキ(arm64/x86_64)・シェルの有無
     (distroless)を確認。
   - `exit=137`(OOMKilled 併記ならメモリ上限)→ `tbm service metrics` で確認、メモリ削減か
     `tbm service limits <名前> --memory <MiB>` で上限を上げて再デプロイ(作り直し不要)。
     `exit=139` = SIGSEGV(アーキ / ベースイメージ不整合の典型)。
2. **ログは stdout+stderr の両方が捕獲される — `2>&1` は不要。** エラー内の「コンテナログ末尾」が
   空なら、何も出力せず終了した可能性が高い(exit=0 と併せて「空のイメージ」の有力な証拠。
   ただしログ取得自体の失敗もあり得るので、これ単独では断定しない)。
3. **推測で直して再デプロイを繰り返さない。** 2 回試して原因が見えなければ観察に切り替える:
   CMD を一時的に `sh -c '<本来のコマンド>; echo exit=$?; sleep 600'` にして deploy → コンテナが
   生きている間に `tbm service exec <名前> -- <調査コマンド>`(直接バイナリを実行 / `ldd` /
   `wget -qO- localhost:8080` 等)。原因を掴んだら CMD を戻す。注意 2 点:観察中は本来の
   アプリが動かない = 公開 URL は応答しない(調査が終わったら速やかに戻す)。distroless 等
   `sh` の無いイメージではこの手が使えない — 観察の間だけ基底を `debian:stable-slim` 等に
   替える(exit=127 の切り分けにもなる)。

## 6. ライフサイクルと後始末

- 再デプロイ:GitHub 経路は `git push`、ローカルは `tbm deploy --local`。
- `tbm cache rotate` の後は**再デプロイ**して初めて新しい接続文字列が効く
  (`tbm db rotate` は human role だけなので実行中の app は無影響)。
- `tbm service {start,stop,logs,rollback,delete}`。`delete` はゴミ箱(3 日復元可、`tbm trash`)。
- **`tbm service rename <名前> <新名>`** — 表示名だけ変わる(subdomain = 公開 URL / GitHub repo は
  不変)。**`tbm service subdomain <名前> <新subdomain>`** — 公開 URL の変更(**旧 URL は即失効** =
  302 /noservice。GitHub repo 名は旧名のまま — `delete --with-repo` は現 subdomain 名で探すため
  **見つからずエラーで止まる**(旧名の repo は `gh repo delete` で手動掃除)。
  この service を注入している呼び出し側は**再デプロイ**で新しい `_URL`/`_HOST` が入る —
  それまでは status の [未反映:要デプロイ] が出る。**改名の前に `tbm service callers <名前>` で
  影響範囲を確認**し、改名後は `--redeploy-callers`(または `tbm service redeploy-callers <名前>`)で
  呼び出し側をまとめて追従させられる)。
  **`tbm service limits <名前> [--memory <MiB>] [--cpus <N>|none]`** — リソース上限の変更
  (次のデプロイから反映)。**`tbm service stateful <名前>`** — ステートフル化(false→true のみ。
  次のデプロイから stop-first)。
- **`tbm service visibility <service名> <private|company|public>`** — 公開範囲の切り替え(**即時反映・
  再デプロイ不要**)。`private` = 公開 URL 無効(監視・通知系 worker 向け。内部リンク /
  logs / exec は従来どおり)/ `company` = 会社の IP のみ(既定)/ `public` = 一般公開(IP 制限
  なし — アプリ側に認証が無ければ誰でもアクセスできる)。
- **volume のファイル操作**:`tbm volume ls <vol> [パス]` / `put <vol> <ローカル> [遠端]` /
  `get <vol> <遠端> [ローカル]` / `rm <vol> <パス>` / `mkdir <vol> <パス>` / `mv <vol> <元> <先>`。
  遠端パスは假根(volume のルート)からの相対で、先頭 `/` はあってもなくても同じ。service に
  マウント中でも直接読み書きできる(seed データ投入・成果物の取り出しに)。
- 秘密(接続文字列・deploy key)は **git に commit しない / 共有しない**。漏れたら rotate。

## 7. つまずきの早見表

| 症状 | ほぼこれ | 一手 |
| --- | --- | --- |
| `succeeded` なのに画面が真っ白 | index.html は 200 だが `/assets/*.js` が 404 | `tbm service verify` で特定 → build 出力パス / base 設定を直す |
| deploy failed(起動即 exit) | エラーの `exit=…` が要因を示す(0=空イメージ / 101=panic / 137=OOM 等) | §5「起動直後にクラッシュする」playbook |
| deploy failed(`TCP 待受を…確認できませんでした`) | readiness 門:app が `PORT` の値で listen していない(監听錯 port / bind 先が 127.0.0.1 / 起動が 60s 超) | `PORT` env で 0.0.0.0 に listen させる → 再デプロイ。listen しない worker は `tbm service visibility <名前> private` |
| deploy failed(`manifest unknown`) | push は成功したが registry に実体が無い(GC 競合) | 再デプロイで再 push。直らなければ管理者へ(registry cache の毒 — `docker restart tsubomi-registry`) |
| URL が `/noservice` へ 302 する | `visibility=private`(または未デプロイ/停止) | `tbm service status` で確認 → 公開するなら `tbm service visibility <名前> company` |
| push が 413 | 単層 >100MB(CF 経由)。直接接続入口があれば起きない | §「push が 413」。無ければ層を小さく |
| Node/Next が DB の証明書エラーで落ちる | 注入ホスト名と証明書が一致しない部署(または古いホスト名を握ったコンテナ) | §3.1 の分岐(`DATABASE_HOST` を見る)→ 一致するなら再デプロイ、しないなら検証を切る |
| Node/Next が起動直後 exit(DB 以外) | ioredis の error イベント未listen | §3.1(`redis.on("error", …)` を付ける) |
| Rust が起動直後 exit(DB 接続) | `NoTls` で `sslmode=require` に繋げない | §3.1(`postgres-native-tls` で TLS コネクタを渡す) |
| ORM / ライブラリが URL 形を受け取らない | URL 一本で組めないケース | §3.1 の素材 env(`DATABASE_HOST`/`_PORT`/`_USER`/`_PASSWORD`/`_NAME`/`_SSLMODE`) |
| `code: unauthorized` | 未ログイン | `tbm login` |
| `code: conflict`(create) | 同名の**稼働中**リソースがある(ゴミ箱は名前を占有しない)/ `--subdomain` が使用中(こちらは**ゴミ箱内も占有** — `tbm trash list`) | 別名にするか、稼働中の方を rename / delete。subdomain 409 は別の subdomain を指定 |
| `code: conflict`(trash restore) | 同名で作り直した稼働中と衝突 | 稼働中を rename / delete してから restore。同名堆積は `tbm trash list` の id で特定 |
| `code: conflict`(`deploy --image` / `--dockerfile`) | その service に**進行中のデプロイ**がある(1 service = 同時 1 デプロイ)| `tbm service deploys <名前>` で完了を待つ。**いつまでも 409 のまま**なら、server がデプロイ中に落ちて宙吊りの行が残っている可能性 — 平台の再起動で自動的に閉じられるので owner に連絡 |
| `code: conflict`(`redeploy-callers`) | 連帯再デプロイは**この platform で同時 1 バッチ**| 完了を待って再実行(`tbm service callers <名前>` で各行の直近デプロイ状態が見える)|
| OOM で落ちる(exit=137) | メモリ上限不足 | `tbm service limits <名前> --memory <MiB>` → 再デプロイ |
| `code: validation` | 入力不正 | メッセージに従う |
| 注入が効かない / env が無い | 実行中に注入した(値は起動の瞬間に解決)/ cache rotate・注入元 service の subdomain 変更後に再デプロイしていない | `tbm service status` の注入一覧で `未反映` を確認 → `tbm deploy`。§「順序:注入 → デプロイ」 |
| プラットフォーム DB に拡張が無い / 特殊なミドルウェアが要る | managed の範囲外 | 持ち込みコンテナ(§3.2:`--port` + `--stateful` + volume) |
| 持ち込み DB が再デプロイでデータ全損 | データディレクトリを volume にマウントしていない | §3.2(volume 注入 → データ投入し直し) |
| GitHub CI が回らない(billing/quota) | Actions 額度切れ | `tbm deploy --local` へ(既成イメージ / 無 context Dockerfile なら `--image`・`--dockerfile`) |
| 既成イメージを起こしたいだけ(持ち込み DB 等) | ビルド不要のケース | `tbm deploy --image <ref>`(§4 第 3 経路。docker / GitHub 不要) |
| 基底イメージ + パッケージ追加だけしたい | COPY 不要の軽い定制 | `tbm deploy --dockerfile <path>`(§4 第 3 経路。COPY/ADD 不可) |
| 経路 1・2 不可 + app のコードをビルドする必要 | この環境にビルド環境が無い(経路 3 では code を入れられない) | 部署できないとユーザに伝える(§4。別経路を発明しない) |
| `gh` が無い | 未インストール | OS 別に案内 → `! gh auth login --web --git-protocol https --clipboard` |
| `docker info` 失敗 | Docker 未導入/未起動 | Docker Desktop を案内 |
