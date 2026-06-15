# tsubomi デプロイ手順(tbm CLI)

tsubomi(蕾)= 社内 PaaS(基礎版 Vercel + Neon)。ユーザ(多くは非エンジニア)に代わって、
この手順で app を本番 `https://<名前>.tsubomi-app.com` へデプロイする。
リソースは 4 種(**service** / **database** / **volume** / **cache**)、動詞は「**注入**」ひとつ。

> このファイルを読んだら、まず `tbm whoami` で疎通とログイン状態を確かめてから始める。

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
  - **アプリは service の `container_port` で listen する**(既定 **8080**)。`tbm service create` の
    出力や `tbm service status` の `container_port` を見て、アプリの listen ポートを一致させる。
    **ここがズレると 502。**

## 2. リソースを作る(必要なものだけ)

- service:`tbm service create <名前>`(名前が subdomain になる。registry 情報や GitHub 連携用の
  `setup_commands` も返る。**平台は GitHub に触れない** — gh を使うのはあなた)。
- database:`tbm db create <名前>`
- volume:`tbm volume create <名前>`(ファイル永続が要るなら)
- cache:`tbm cache create <名前>`(valkey が要るなら)

## 3. 注入(デプロイの「前」に!)

| 注入元 | コマンド | コンテナに入る env |
| --- | --- | --- |
| database | `tbm inject <db名> --into <service名>` | `DATABASE_URL` |
| volume | `tbm inject <vol名> --into <service名> [--mount /data/foo]` | `STORAGE_PATH` |
| cache | `tbm inject <cache名> --into <service名>` | `REDIS_URL` + `REDIS_KEY_PREFIX` |

確認:`tbm service status <service名>` の `injections` がすべて `valid: true`。
（env 名を変えたいときは `--as <NAME>`。cache は `<NAME>_KEY_PREFIX` も併せて入る。）

## 4. デプロイ — 経路を選ぶ

### 既定:GitHub 経路(`gh` を使う。CI が build/push)

`tbm service create` の出力 `setup_commands`(`gh repo create` / `gh secret set` / `gh variable set`)を
順に実行し、`workflow_yaml` の内容を `.github/workflows/tsubomi-deploy.yml` に置いて
`git add/commit/push` → GitHub Actions が自動でビルド & デプロイ。

- **`gh` が入っていない** → インストールを案内する:
  - mac:`brew install gh`
  - Debian/Ubuntu:`sudo apt install gh`(または公式 apt repo)
  - Windows:`winget install GitHub.cli` か `scoop install gh`

  ログインは**対話的**でAIは代行できない。ユーザに次を打ってもらう:`! gh auth login`。
- **`gh` の Actions 額度が切れた / billing・quota エラーで CI が回らない**(私有 repo の無料枠超過など)
  → 下の **`tbm deploy --local` 退路**に切り替える。

### 退路:`tbm deploy --local`(GitHub 非依存。ローカルの Docker で build+push)

```
tbm deploy --local --service <service名> --context <Dockerfile のあるディレクトリ>
```

GitHub 額度切れ時の主たる代替でもある。要 Docker。

- **Docker が無い / 起動していない**(`docker info` が失敗)→ ユーザに **Docker Desktop** の
  導入を案内する(https://www.docker.com/products/docker-desktop/ )。インストールと起動は
  GUI・対話なのでユーザにやってもらい、`docker info` が通ってから再実行する。

## 5. 検証(ここを省かない)

1. `tbm service status <service名>` で `phase=running`・最新 deploy が `succeeded` を確認。
2. **必ず** `curl -fsS https://<subdomain>.tsubomi-app.com/`(数回リトライ)。**2xx が返って初めて成功**。
   - **`succeeded` なのに 502** → ほぼ「アプリが `container_port`(既定 8080)で listen していない」。
     ポートを直して**再デプロイ**。`tbm service logs <service名>` も見るが、ログに "listening" と
     出ていても実ポートが違えば 502 になる。**ground truth は curl。**
3. DB / volume / cache を使うなら、実際に「書き込み → 読み戻し」で永続と隔離を確かめる。

## 6. ライフサイクルと後始末

- 再デプロイ:GitHub 経路は `git push`、ローカルは `tbm deploy --local`。
- `tbm db rotate` / `tbm cache rotate` の後は**再デプロイ**して初めて新しい接続文字列が効く。
- `tbm service {start,stop,logs,rollback,delete}`。`delete` はゴミ箱(3 日復元可、`tbm trash`)。
- 秘密(接続文字列・deploy key)は **git に commit しない / 共有しない**。漏れたら rotate。

## 7. つまずきの早見表

| 症状 | ほぼこれ | 一手 |
| --- | --- | --- |
| `succeeded` なのに 502 | アプリが 8080 で listen していない | ポート修正 → 再デプロイ |
| `code: unauthorized` | 未ログイン | `tbm login` |
| `code: conflict` | 名前が既出 | 別名にする |
| `code: validation` | 入力不正 | メッセージに従う |
| 注入が効かない | デプロイ前に注入していない / rotate 後に再デプロイしていない | 注入を確認 → 再デプロイ |
| GitHub CI が回らない(billing/quota) | Actions 額度切れ | `tbm deploy --local` へ |
| `gh` が無い | 未インストール | OS 別に案内 → `! gh auth login` |
| `docker info` 失敗 | Docker 未導入/未起動 | Docker Desktop を案内 |
