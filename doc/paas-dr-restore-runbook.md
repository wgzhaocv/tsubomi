# 災害復旧(DR)リストア runbook

AI 審査 R11 への回答:日次バックアップ(`gc.rs::run_backup`)は「書くだけ」で、恢复の
コード経路・手順書が存在しなかった。本書が**唯一の恢复手順**。年に一度は §7 の演練を実施する。

対象事故:管制面 DB の損壊 / テナント DB の損壊・誤削除(ゴミ箱 3 日窓を過ぎたもの)/
ディスク・ホスト全損。ゴミ箱内の復元は本書の対象外(web / `tbm trash` の既存機能)。

---

## 0. 前提 — これが無いとバックアップは開けない

| もの | 置き場所 | 注意 |
|---|---|---|
| **TSUBOMI_MASTER_KEY** | Pi の `~/tsubomi-deploy/.env.production` | **最重要**。DB 内の全暗号列(deploy_key_enc / password_enc 等)はこの鍵で封緘。鍵を失うと平文バックアップ以外は全て廃紙。**バックアップとは別の場所(パスワードマネージャ等)に必ず控える** |
| 日次バックアップ | `/srv/tsubomi/backups/YYYY-MM-DD/` | サーバプロセスが毎日生成、**7 日で自動削除**(`BACKUP_RETAIN_DAYS`) |
| compose 定義 | `~/tsubomi-deploy/compose.prod.yml` | `just ship` が毎回配布(git にもある) |
| .env.production | Pi のみ(git に無い) | master key / valkey admin pass / owner 種など。**これ自体も控えを取る** |

バックアップディレクトリの中身(1 日分):

```
/srv/tsubomi/backups/2026-07-08/
├── platform.sql        # 管制面 pg-platform の全量 pg_dump(期望状態の正本)
├── db_ab12cd34.sql     # テナント DB ごとの pg_dump(--no-owner --no-privileges)
├── db_…​.sql
└── volumes/            # /srv/tsubomi/volumes の rsync -a スナップショット
```

**含まれないもの**:registry のイメージ(恢复後に再デプロイで再 push すれば戻る)、
valkey のキャッシュ値(定義上 cache = 消えてよい。ACL は reconcile が管制面から再生成)、
pg-platform / pg-tenant の**クラスタレベル資産**(role は dump に入らない — §3 参照)。

⚠️ **既知の弱点**:バックアップは**同じディスク**にある。ディスク全損には
`rsync -a pi:/srv/tsubomi/backups/ <別マシン>/` の外部同期を cron 等で別途仕込むこと
(平台の機能としては未実装 — 受容済み)。

---

## 1. 事故の型を判定する

| 症状 | 型 | 進む先 |
|---|---|---|
| 管制面が起動しない / platform DB が壊れた | A: 管制面のみ | §2 |
| 特定テナント DB が壊れた / 3 日窓を過ぎた誤削除 | B: テナント DB 単体 | §3 |
| volume のファイルを過去時点に戻したい | C: volume | §4 |
| ホスト / ディスク全損(新機に再構築) | D: フル DR | §5 |

---

## 2. A: 管制面(pg-platform)の復元

```bash
# 1. server を止める(管制面へ書く者を無くす)
cd ~/tsubomi-deploy && docker compose -f compose.prod.yml stop server

# 2. 壊れた DB を退避リネームし、空の DB を作る(接続情報は .env.production の DATABASE_URL)
docker exec -it tsubomi-pg-platform psql -U tsubomi -d postgres -c \
  "ALTER DATABASE tsubomi RENAME TO tsubomi_broken_$(date +%s);"
docker exec -it tsubomi-pg-platform psql -U tsubomi -d postgres -c \
  "CREATE DATABASE tsubomi OWNER tsubomi;"

# 3. 最新バックアップを流し込む(dump は全量 = スキーマ + _sqlx_migrations も入っている)
docker exec -i tsubomi-pg-platform psql -U tsubomi -d tsubomi -v ON_ERROR_STOP=1 -q \
  < /srv/tsubomi/backups/<最新日付>/platform.sql

# 4. server を起こす(起動時 migration 検証は dump 内の _sqlx_migrations と一致するはず)
docker compose -f compose.prod.yml up -d server
```

検証:`tbm service list` が出る / web の overview が出る / `docker logs tsubomi-server` に
migration エラーが無い。**注意**:バックアップ時点以降の作成・削除・rotate は失われる
(コンテナ実体と DB がずれる)— reconcile が「DB に無い管理コンテナ = 孤児」として掃除する
方向に収束するので、**復元直後に利用者へ「昨日以降に作った資源は作り直し」と告知**する。

---

## 3. B: テナント DB 単体の復元

役割の前提:テナント dump は `--no-owner --no-privileges`。**role(o_… / u_… / h_…)は
クラスタ資産で dump に入らない**。管制面が生きていれば role は既存なので手順は短い。

```bash
# 1. 対象の実 DB 名を管制面から引く(display_name → pg_dbname / owner role)
docker exec -it tsubomi-pg-platform psql -U tsubomi -d tsubomi -c \
  "SELECT d.pg_dbname FROM database_details d JOIN resources r ON r.id=d.resource_id
    WHERE r.display_name='<表示名>';"

# 2. 壊れた DB を退避リネーム → owner 付きで再作成(owner role 名 = o_<shortid>、
#    pg_dbname の db_ を o_ に読み替えたもの。TENANT_ADMIN_URL は .env.production 参照)
docker exec -it tsubomi-pg-tenant psql -U admin -d postgres -c \
  "ALTER DATABASE db_ab12cd34 RENAME TO db_ab12cd34_broken;"
docker exec -it tsubomi-pg-tenant psql -U admin -d postgres -c \
  "CREATE DATABASE db_ab12cd34 OWNER o_ab12cd34;"

# 3. dump を流し込む(作成物を owner 所有にするため role を切替えて流す —
#    tenant.rs::restore_database と同じ流儀)
docker exec -i -e PGOPTIONS='-c role=o_ab12cd34' tsubomi-pg-tenant \
  psql -U admin -d db_ab12cd34 -v ON_ERROR_STOP=1 -q \
  < /srv/tsubomi/backups/<日付>/db_ab12cd34.sql
```

検証:web の SQL タブで `SELECT count(*)`、app は**再デプロイ不要**(接続文字列は不変)。
**pg-tenant クラスタごと失った場合**(role も無い):先に §2 で管制面を戻し、
`CREATE ROLE o_… NOLOGIN` + `CREATE ROLE u_…/h_… LOGIN IN ROLE o_…` を作ってから上記 2-3 を
実行し、**パスワードは web / CLI の rotate で振り直す**(平文パスワードはどこにも無い —
rotate が管制面と実 role を同時に更新する正規経路)。rotate 後は注入先 app の再デプロイ。

---

## 4. C: volume の復元

バックアップは素の rsync ミラーなので、逆向きに rsync するだけ。**全上書きではなく
`--ignore-existing` や対象パス指定で被害範囲だけ戻す**のが安全。

```bash
# 例:volume 全体を過去時点へ(消えたファイルの救出は --ignore-existing が安全)
rsync -a --ignore-existing \
  /srv/tsubomi/backups/<日付>/volumes/<user>/<volume_id>/ \
  /srv/tsubomi/volumes/<user>/<volume_id>/
```

app が bind mount で見ているのはホスト側の実ディレクトリなので反映は即時。

---

## 5. D: フル DR(新ホスト再構築)

順序が本体。**「compose → 管制面 → テナント → volumes → 検証」**:

1. 新機に docker / justfile 前提を用意し、`~/tsubomi-deploy/` に `compose.prod.yml` と
   **控えておいた `.env.production`**(master key 含む)を置く。
2. 外部に同期してあったバックアップを `/srv/tsubomi/backups/<日付>/` へ戻す。
3. `docker compose -f compose.prod.yml up -d pg-platform pg-tenant` だけ起こし、
   §2 で管制面を復元(server はまだ起こさない)。
4. §3 の「クラスタごと失った場合」の手順で各テナント DB を復元(role 再作成 + rotate)。
5. `volumes/` を `/srv/tsubomi/volumes/` へ rsync(§4、こちらは全量で良い)。
6. 残りの infra + server を起こす:`docker compose -f compose.prod.yml up -d`。
7. **全 service を再デプロイ**(registry イメージはバックアップ外。CI 再実行か
   `tbm deploy --local`。rotate した DB の新パスワードもこの再デプロイで注入される)。
8. DNS / CF Tunnel を新機へ向け直す。

---

## 6. やってはいけないこと

- **platform.sql を「動いている」管制面に重ね掛けしない**(必ず空 DB に流す。重複行で半端に死ぬ)。
- **master key を変えたまま復元しない**(暗号列が全部開けなくなる。復元は必ず同じ鍵で)。
- 復元中に server を走らせない(reconcile が中途半端な DB を正として孤児掃除を始める)。

## 7. 演練(年 1 回)

dev か予備機で:①最新バックアップを取り寄せ ②§2 を実施 ③テナント 1 本を §3 で復元
④`tbm db query` で実データを確認 ⑤所要時間を本書末尾に記録。
**「復元したことのないバックアップはバックアップではない」**。

| 演練日 | 実施者 | 所要 | メモ |
|---|---|---|---|
| (未実施) | | | |
