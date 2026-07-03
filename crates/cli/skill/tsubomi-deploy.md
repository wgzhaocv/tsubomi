# tsubomi デプロイ手順(tbm CLI)

tsubomi(蕾)= 社内 PaaS(基礎版 Vercel + Neon)。ユーザ(多くは非エンジニア)に代わって、
この手順で app を本番 `https://<名前>.tsubomi-app.com` へデプロイする。
リソースは 4 種(**service** / **database** / **volume** / **cache**)、動詞は「**注入**」ひとつ。

**このプラットフォーム(tsubomi)のアーキテクチャは {{HOST_ARCH}} です。** デプロイするイメージは
このアーキテクチャで動く必要がある(`tbm whoami` / `tbm --help` の出力でも確認できる)。

> このファイルを読んだら、まず `tbm whoami` で疎通・ログイン状態・プラットフォーム/本機のアーキを
> 確かめてから始める。

## 0. 絶対に外さない 3 点

1. **検証は必ず `curl` で 2xx を確認する。`tbm service status` の "running / succeeded" を信用しない。**
   デプロイの存活判定はコンテナが「起動して落ちていない」ことしか見ない。アプリのポートが
   ズレていても "succeeded" になり、サイトは 502 になる。**真実は curl だけ**。
2. **注入はデプロイの「前」に行う。** 値はコンテナ起動の瞬間に解決される。注入し忘れたまま
   デプロイすると env が無い。rotate(パスワード再生成)も**再デプロイして初めて効く**。
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
    listen ポートを一致させる。**ここがズレると 502。**

## 2. リソースを作る(必要なものだけ)

- service:`tbm service create <名前>`(名前が subdomain になる)。**GitHub 経路(既定)で出すなら、この
  作成時に `--github` を付ける**(repo/secret/variable と workflow 設定までこの 1 回で済む。§4 参照)。
  `--github` は**作成時のみ有効**(付け忘れた既存 service には効かず、再 create は重名 409)。GitHub 経路なら
  最初から付ける。registry 情報や `setup_commands` も返る。**平台は GitHub に触れない** — gh を使うのはあなた。
  - 任意フラグ:`--port <PORT>`(listen ポート。既定 8080。**8080 以外を指定すると公開範囲の既定が
    `private` になる** — 非 HTTP コンテナ想定。`--visibility` で上書き可)/ `--stateful`(自帯 DB 等の
    有状態コンテナ。デプロイが stop-first = 数秒瞬断と引き換えにデータ目録を保護)/
    `--memory <MiB>`(硬上限。既定 1024)。**port / stateful は作成後に変更できない** — 間違えたら
    削除して作り直す。
- database:`tbm db create <名前>`
- volume:`tbm volume create <名前>`(ファイル永続が要るなら)
- cache:`tbm cache create <名前>`(valkey が要るなら)

## 3. 注入(デプロイの「前」に!)

| 注入元 | コマンド | コンテナに入る env |
| --- | --- | --- |
| database | `tbm inject <db名> --into <service名>` | `DATABASE_URL` |
| volume | `tbm inject <vol名> --into <service名> [--mount /data/foo]` | `STORAGE_PATH` |
| cache | `tbm inject <cache名> --into <service名>` | `REDIS_URL` + `REDIS_KEY_PREFIX` |
| service | `tbm inject <svc名> --into <service名>` | `<名前>_URL`(内部直連 http)+ `<名前>_HOST` / `<名前>_PORT` |

service 注入 = 別 app への**内部直連**(公網を通らない。同一 owner 限定)。HTTP app は `_URL` を
そのまま使い、**非 HTTP(自帯 postgres 等)は `_HOST` / `_PORT` で自分のスキームの接続文字列を組む**
(例 `postgres://user:pass@${MYPG_HOST}:${MYPG_PORT}/db` — パスワードは自分が env で設定したもの)。

確認:`tbm service status <service名>` の `injections` がすべて `valid: true`。

**接続文字列は「env 名」で繋ぐ(値は環境ごとに解決:ローカル=公開 / 本番=内部)。**
注入は **env 名にそのまま値を生成する**(内容マッチではない)。本番は起動時に**内部接続文字列**
(app role・内部入口 `tsubomi-pgbouncer`・社外に出ない)を `DATABASE_URL` に入れる。開発機で使う
**公開接続文字列**(`tbm db url`。human role・外部入口)とは **同じ env 名で繋ぐ** — コードは
`process.env.DATABASE_URL` を**読むだけ**で、値はローカル=公開 / 本番=注入と別物。両者は別環境にしか
存在しないので**衝突せず無縫に切り替わる**。これを成立させる 3 点:

- **env 名を一致させる**:既定は `DATABASE_URL`。既存リポジトリは、コード / `.env.example` が読む名前を
  確認し、違えば `--as <その名前>` で注入名を寄せる(`tbm inject <db> --into <svc> --as <NAME>`。cache は
  `<NAME>_KEY_PREFIX` も併せて入る)。確認は `injections[].env_var` と `process.env.XXX` の突き合わせ。
- **接続文字列をコードに直書きしない**(必ず env 名を読む)。直書きは env をすり抜け、本番でも公開経路に出る。
- **公開文字列を本番に持ち込まない**:`.env` は `.gitignore` + `.dockerignore`(**イメージに焼かない**)、
  `tbm service env set DATABASE_URL=<公開>` も**しない**。持ち込むと公開経路(外部入口)に出て、同一ホストの
  DB に**インターネットを一周**(遅延)+ `tbm db rotate` で**黙って切れる**(注入の内部文字列はどちらも無い)。

### 3.1 `DATABASE_URL` の TLS は言語で扱いが違う(つまずきやすい)

注入される `DATABASE_URL` は **`sslmode=require`**(libpq の意味 = 暗号化はするが**証明書は検証しない**。
内部の自己署名証明書のため検証は通らない)。この `require` の解釈がドライバで割れる:

- **Go(lib/pq)/ Python(psycopg)**:`require` を「暗号化のみ・検証なし」と解釈 → **そのまま繋がる**。
- **Node.js(`pg`)/ Next.js**:`pg` は `require` でも証明書を**厳密検証**する(libpq の
  「require=検証なし」互換ではない)ため内部の自己署名証明書で失敗する。しかも接続文字列由来の
  ssl 設定が明示 `ssl` を上書きするので、**URL から `sslmode` を外して** `ssl` を明示する:
  ```js
  const u = new URL(process.env.DATABASE_URL); u.searchParams.delete("sslmode");
  const pool = new pg.Pool({ connectionString: u.toString(), ssl: { rejectUnauthorized: false } });
  ```
- **cache を使う Node アプリ(ioredis)**:**必ず `redis.on("error", …)` を付ける**(未listen の error
  イベントは "Unhandled error event" でプロセスごと落ちる = 起動直後 exit の典型。DB の TLS とは別件だが
  同じ「起動直後 exit」症状になる)。
- **Rust(`postgres` / `tokio-postgres`)**:`NoTls` では `sslmode=require` に繋がらない。TLS
  コネクタを渡す(検証なし = `require` の意味に合わせる):
  ```rust
  let c = native_tls::TlsConnector::builder().danger_accept_invalid_certs(true).build()?;
  let mut db = postgres::Client::connect(&url, postgres_native_tls::MakeTlsConnector::new(c))?;
  ```

迷ったら **起動時ではなくリクエスト時に DB へ繋ぐ**と、失敗が「起動直後 exit」ではなく
レスポンスのエラーに出て切り分けやすい。

### 3.2 自帯コンテナ(managed database で足りない時:拡張入り Postgres・meilisearch 等)

平台の database(pg-tenant)には**拡張を入れられない**。pgvector 等が要るときは、DB を
**stateful service として自分で立てて**リンクする:

```
tbm service create mypg --port 5432 --stateful        # 非8080 → 自動で private(公開URLなし)
tbm volume create mypg-data
tbm inject mypg-data --into mypg --mount /var/lib/postgresql/data   # データ目録の永続化(必須!)
tbm env set mypg POSTGRES_PASSWORD=<自分で決める>
printf 'FROM pgvector/pgvector:pg17\n' > Dockerfile   # 現成イメージなら 1 行でよい
tbm deploy --local --service mypg --context .
tbm inject mypg --into <app名>                         # app に MYPG_HOST / MYPG_PORT が入る
```

- **volume 注入を忘れない**:コンテナはデプロイごとに作り直される。データ目録を volume に
  マウントしないと**再デプロイでデータ全損**。マウント先はそのソフトのデータパスに合わせる
  (postgres = `/var/lib/postgresql/data`)。
- **`--stateful` を忘れない**:無いと再デプロイ時に新旧コンテナが同じデータ目録を同時に開き
  **データ破壊**になり得る。stateful のデプロイ / 停止は数秒の瞬断がある(仕様)。
- 接続文字列は app 側で `_HOST` / `_PORT` + 自分の設定したパスワードで組む(§3 の表)。
  中身(ユーザ・スキーマ・チューニング・升級)は**全部ユーザの責任** — 平台が保証するのは
  「活きている・データが在る・app から届く」まで。
- 外部(手元の psql 等)からは繋げない(公網入口は HTTP のみ)。操作は
  `tbm service exec mypg -- psql -U postgres -c "..."` で。
- 検証:`tbm service verify` は private では使えない。`tbm service exec` で書き込み → 読み戻し。

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

**まず闸門:デプロイには「ビルド環境」が要る。** イメージをビルドして push できるのは次の
**2 つのどちらか**で、**最低 1 つを満たさなければデプロイできない**:

1. **GitHub Actions の枠が残っている** → 既定の GitHub 経路(CI が両アーキでビルド)。
2. **プラットフォームと同じアーキ({{HOST_ARCH}})の Docker が手元で動く** → 退路 `tbm deploy --local`。

**どちらも満たせないとき**(Actions 枠切れ **かつ** 手元に同アーキの動く Docker が無い = 跨アーキしか
ない / Docker が無い)**は、この環境ではデプロイできない — それが正しい結論。** 手を止めてユーザに
そう伝える。**サーバ側ビルドや別経路を勝手に発明しない。** プラットフォームは**ユーザ機でビルドする
設計**であって、ビルド環境を用意するかは**ユーザ側の判断**(同アーキ機を使う / Docker を入れる /
GitHub の枠を空ける)。「いま部署できるビルド環境が無い」と伝え、どれで進めるかをユーザに選ばせる。

### 既定:GitHub 経路(`gh` を使う。CI が build/push)

service を **§2 で `--github` 付きで作成済み**なら GitHub 連携は完了している:平台が `gh` 経由で
repo 作成・secret / variable 設定・`.github/workflows/tsubomi-deploy.yml` の書き出しまで実施済み
(秘密は stdin 渡しで `ps` に出ない。**Windows / mac / Linux どの shell でも動く**。create 出力 JSON の
`github.configured` が true なら完了)。あとは `git add/commit/push` → GitHub Actions が自動でビルド &
デプロイ。

- **まだ `--github` を付けていない場合**:`--github` は作成時のみ有効なので、最初の `tbm service create
  <名前> --github` で付けるのが要点(既存 service への再 create は重名 409)。create 応答(JSON)には
  `setup_commands`(`gh repo create` / `gh secret set` / `gh variable set`。**POSIX shell 前提**)も
  返るので、bash / zsh なら手で順に実行してもよい。ただし Windows(PowerShell)では `printf` / `$(…)` が
  動かないため、最初から `--github` で作るのが確実。
- **`gh` が入っていない** → インストールを案内する:
  - mac:`brew install gh`
  - Debian/Ubuntu:`sudo apt install gh`(または公式 apt repo)
  - Windows:`winget install GitHub.cli` か `scoop install gh`

  ログインは**対話的**でAIは代行できない。ユーザに次を打ってもらう:`! gh auth login --web --git-protocol https --clipboard`。
- **`gh` の Actions 額度が切れた / billing・quota エラーで CI が回らない**(私有 repo の無料枠超過など)
  → 下の **`tbm deploy --local` 退路**に切り替える。
- **既存コードのあるディレクトリで作る場合**:`--github` は「git repo でも空でもないディレクトリ」では
  誤 push 防止のため拒否される。デプロイ対象なら先に `git init -b main` してから
  `tbm service create <名前> --github` を実行する(空ディレクトリ / 既存 repo ならそのままでよい)。
- **ビルドが遅い(数十分)場合**:CI のランナーは gh variable `TSUBOMI_RUNNER` で決まる。新規 service は
  平台が自動設定するが、**古い service は未設定 = amd64 + QEMU で極端に遅い**。平台が arm64 なら
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
- **Windows(git-bash / MSYS)で `--context` 等の絶対パスが化ける**(`/c/...` が `C:\...;` 混じりに
  なる等)→ MSYS のパス変換が原因。`MSYS_NO_PATHCONV=1 tbm deploy --local ...` のように前置すると
  変換を止められる。

### push が 413(Payload Too Large)で失敗する — 単層 100MB 上限

registry は(既定で)Cloudflare 経由のため **イメージ 1 層あたり圧縮後 ≈100MB** が上限(CF の
request body 制限。registry 側では変えられない)。超えると `tbm deploy --local` でも GitHub Actions
でも push が 413 で落ちる。

- **この部署に直連入口が設定済みなら 413 は起きない**:平台が push 先を CF 非経由の直連 registry
  に振り向けている(`tbm service create` 応答の registry host が `registry-direct.<域名>` 系なら該当)。
  それでも 413 が出たら直連入口の障害を疑い、ユーザに知らせる(勝手に別経路を作らない)。
- **直連入口が無い部署**での対処は**層を小さくする**:大きな `RUN`/`COPY` を分割 / slim・alpine 基底 /
  マルチステージでビルド中間物を最終イメージに持ち込まない。恒久対策(直連入口の追加)は運用側の
  判断 — `doc/paas-registry-direct-design.md`。

## 5. 検証(ここを省かない)

1. `tbm service status <service名>` で `phase=running`・最新 deploy が `succeeded` を確認
   (`visibility` 行で公開範囲も見える)。
2. **`tbm service verify <service名>`** を使う。根 HTML を取り、そこが参照する js/css 子リソースまで
   2xx かをまとめて確認する(`ok:true` で成功。NG なら exit 1 + どのリソースが落ちたか)。
   **`visibility=private` のサービスは公開 URL 自体が無効**なので verify は明確な文言でスキップ +
   非零終了する(接続失敗ではない = サーバ障害と誤読しない)。動作確認は `tbm service logs` /
   `tbm service exec`、または内部リンク先の caller コンテナから
   `tbm service exec <caller> -- wget -qO- http://<subdomain>:<port>` で行う。
   **これが重要な理由**:`status=succeeded` + 根 200 でも、`index.html` が参照する `/assets/*.js` が
   404 だと**画面は真っ白**になる。根への素の `curl` はこれを見逃す。verify は子リソースまで見る。
   - **`succeeded` なのに 502**(verify の root_status が 502)→ ほぼ「アプリが `container_port`
     (既定 8080)で listen していない」。ポートを直して**再デプロイ**。`tbm service logs` も見るが、
     ログに "listening" と出ていても実ポートが違えば 502 になる。
   - **root は 200 だが子リソースが 404**(verify が `ok:false`)→ ビルドの出力パス / `base` 設定 /
     直近デプロイの失敗が典型。
   - `tbm service cat <service名> <パス>` でコンテナ内のファイル(ビルド成果物・設定)を直接確認できる
     (`exec -- cat` の糖衣)。`tbm service exec <service名> -- <cmd>` で任意コマンドも。
3. DB / volume / cache を使うなら、実際に「書き込み → 読み戻し」で永続と隔離を確かめる。注入した値が
   何に解決されるかは **`tbm env list <service名> --resolved`**(由来付き・秘密は伏せる)で確認できる
   — 探针を書かずに「B_URL が何を指すか」等が分かる。反映はデプロイ時なので rotate 後は要再デプロイ。

## 6. ライフサイクルと後始末

- 再デプロイ:GitHub 経路は `git push`、ローカルは `tbm deploy --local`。
- `tbm db rotate` / `tbm cache rotate` の後は**再デプロイ**して初めて新しい接続文字列が効く。
- `tbm service {start,stop,logs,rollback,delete}`。`delete` はゴミ箱(3 日復元可、`tbm trash`)。
- **`tbm service visibility <service名> <private|company|public>`** — 公開範囲の切り替え(**即時反映・
  再デプロイ不要**)。`private` = 公開 URL 無効(監視・通知系 worker 向け。内部リンク /
  logs / exec は従来どおり)/ `company` = 会社の IP のみ(既定)/ `public` = 一般公開(IP 制限
  なし — アプリ側に認証が無ければ誰でもアクセスできる)。
- 秘密(接続文字列・deploy key)は **git に commit しない / 共有しない**。漏れたら rotate。

## 7. つまずきの早見表

| 症状 | ほぼこれ | 一手 |
| --- | --- | --- |
| `succeeded` なのに画面が真っ白 | index.html は 200 だが `/assets/*.js` が 404 | `tbm service verify` で特定 → build 出力パス / base 設定を直す |
| `succeeded` なのに 502 | アプリが 8080 で listen していない | ポート修正 → 再デプロイ |
| URL が `/noservice` へ 302 する | `visibility=private`(または未デプロイ/停止) | `tbm service status` で確認 → 公開するなら `tbm service visibility <名前> company` |
| push が 413 | 単層 >100MB(CF 経由)。直連入口があれば起きない | §「push が 413」。無ければ層を小さく |
| Node/Next が起動直後 exit / DB で 502 | `pg` が `sslmode=require` を verify-full 扱い | §3.1(URL から sslmode を外し `ssl:{rejectUnauthorized:false}`)+ ioredis に `on("error")` |
| Rust が起動直後 exit(DB 接続) | `NoTls` で `sslmode=require` に繋げない | §3.1(`postgres-native-tls` で TLS コネクタを渡す) |
| `code: unauthorized` | 未ログイン | `tbm login` |
| `code: conflict` | 名前が既出 | 別名にする |
| `code: validation` | 入力不正 | メッセージに従う |
| 注入が効かない | デプロイ前に注入していない / rotate 後に再デプロイしていない | 注入を確認 → 再デプロイ |
| 平台 DB に拡張が無い / 特殊なミドルウェアが要る | managed の範囲外 | 自帯コンテナ(§3.2:`--port` + `--stateful` + volume) |
| 自帯 DB が再デプロイでデータ全損 | データ目録を volume にマウントしていない | §3.2(volume 注入 → データ投入し直し) |
| GitHub CI が回らない(billing/quota) | Actions 額度切れ | `tbm deploy --local` へ |
| 両経路とも不可(枠切れ + 跨アーキ / Docker 無し) | この環境にビルド環境が無い | 部署できないとユーザに伝える(§4。別経路を発明しない) |
| `gh` が無い | 未インストール | OS 別に案内 → `! gh auth login --web --git-protocol https --clipboard` |
| `docker info` 失敗 | Docker 未導入/未起動 | Docker Desktop を案内 |
