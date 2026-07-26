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
| **acme.sh のアカウント + DNS API トークン** | Pi の `~/.acme.sh/`(`CF_Token` 等は同ディレクトリの account.conf) | **新たな単点**(2026-07-26)。`db.<域名>` の証書を再発行できないと、注入ホスト名が証書と食い違い**厳格検証する駆動系の app が全滅**する(§E)。バックアップに入っていない — 別途控える |
| 証書更新 hook | 正本 = git の `deploy/db-public/reload-pgb-cert.sh`(cache 側は `deploy/cache-public/`) | ホストへは**手で配置**して acme.sh の `--reloadcmd` に登録する(compose は参照しない)。§5 で再配置が必要 |

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
| 特定の端点だけ 5xx / 無応答。`docker logs tsubomi-server` に `panicked at` が在り、server が繰り返し起動している | **F: コード/データ不整合(バックアップは無傷)** | §5.95 |

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
5.5. **pgbouncer の証書を戻す**(server より先。§E の予防):acme.sh を入れ直し、控えた
   アカウント / DNS トークンで `db.<域名>` を発行 → `deploy/db-public/reload-pgb-cert.sh` を
   ホストへ配置して `--reloadcmd` に登録。**pgbouncer を起こしてから**手で 1 度実行する
   (未起動だと「次回起動で反映」と言って正常終了するだけで、閉環確認まで進まない)。
   pgbouncer 稼働中なら、スクリプトは serving 証書の指紋が入れた物と一致するまで確認し、
   **一致しなければ非零で終わる** — subject / notAfter が出れば成功。
   **acme.sh を通せないなら**、その間は `.env.production` の `TSUBOMI_DB_INTERNAL_HOST` を
   **容器名(`tsubomi-pgbouncer`)へ戻しておく** — 種の自己署名では厳格検証する駆動系が繋がらないので、
   検証しない駆動系だけでも動く状態に倒す(後で証書を入れたら戻して server 再起動 + 再デプロイ)。
6. 残りの infra + server を起こす:`docker compose -f compose.prod.yml up -d`。
7. **全 service を再デプロイ**(registry イメージはバックアップ外。CI 再実行か
   `tbm deploy --local`。rotate した DB の新パスワードもこの再デプロイで注入される)。
8. DNS / CF Tunnel を新機へ向け直す。

---

## 5.9. E: pgbouncer 証書の失効・期限切れ(全テナント app の DB が落ちる)

**症状の見え方が厄介**:注入ホスト名は pgbouncer 証書の公開名に揃えてあるので(m3 設計 §11 決定 A')、
証書が切れる / 名前が食い違うと **`sslmode=require` を厳格検証する駆動系(Node の `pg` 等)だけ**が
TLS エラーで落ちる。libpq 系(Go / Python)は検証しないので**平然と動き続ける** ⇒ 「一部の app だけ壊れた」
ように見えて、共通原因(証書)に辿り着きにくい。**複数 app が同時に DB エラーを出したら最初にここを見る**。

```bash
# 1. 今出ている証書を確認(notAfter が過去 / 名前が db.<域名> でない = これ)
ssh <host> 'openssl s_client -connect 127.0.0.1:6432 -starttls postgres </dev/null 2>/dev/null |
  openssl x509 -noout -subject -dates'
# 2. 更新して反映(hook が卷へ入れて SIGHUP、末尾で反映後の証書を印字する)
ssh <host> '~/.acme.sh/acme.sh --renew -d db.<域名> --ecc --force && ~/reload-pgb-cert.sh'
```

**5 分の退路**(証書をすぐ直せない時):`.env.production` の `TSUBOMI_DB_INTERNAL_HOST` を
`tsubomi-pgbouncer` に戻し、server を入れ替え(`just ship` の `up -d server` で足りる)、影響 service を
**再デプロイ**する(注入値は起動の瞬間に解決されるため)。これで検証しない駆動系は復活する。
Node 側は §3.1 の「容器名のとき」の書き方(`rejectUnauthorized:false`)に倒す。

**予防**:acme.sh の日次 cron + `--reloadcmd` が入っていること、`~/.acme.sh/acme.sh --list` の
`Renew` 列が未来日であることを演練時に確認する(§7)。更新は 60 日毎なので**沈黙する窓が長い**。

---

## 5.95. F: コード/データ不整合(復元ではない — 前進修復のみ)

**バックアップもホストも無傷なのに読めない**型。2026-07-26 に実際に起きた:migration が既存行を
`'-infinity'` で回填したが、Postgres の infinity は Rust の `DateTime<Utc>` に読み込めず sqlx が panic。
当時この平台は **`panic = "abort"`** だったので、**1 リクエストの panic で server プロセスが落ちた** —
`restart: unless-stopped` がすぐ拾い、起動時 reconcile が丁寧に収束させるので、外からは「その端点だけ
壊れている」ように見えた。実際の波及:

- **進行中の deploy が `failed`(`error='server がデプロイ中に再起動しました'`)になる** — 他人の
  デプロイが、誰かが web の env タブを開くたびに死ぬ。
- `logs --follow` / web terminal(WS)/ `tbm deploy --local` のアップロードが全切断。

**この波及は 2026-07-26 に塞いだ**(`panic = "abort"` を外し router に `CatchPanicLayer`)。
以後の panic は**そのリクエストが 500** になるだけで、プロセスは生き続ける(panic はログに残る)。
つまり今この型を踏んだら **「特定の端点だけ 500」+ ログに `panicked at`** という、より素直な形で出る
— 再起動が増えないので `RestartCount` は手掛かりにならない。診断は下の 1) をログ中心に読む。

```bash
# 1) 診断(この 3 つで型が確定する)
docker logs tsubomi-server 2>&1 | grep -A5 'panicked at'
docker inspect -f '{{.RestartCount}}' tsubomi-server
docker exec tsubomi-pg-platform psql -U tsubomi -d tsubomi_platform   -c "SELECT count(*) FROM deploys WHERE error LIKE '%再起動%'"   # 被害を受けた deploy
```

手順(**この順序が要点**):

1. **即応 = データを直す**(イメージ更新なしでその場で効く)。例:
   `UPDATE injections SET created_at='epoch' WHERE created_at='-infinity';`
2. 被害を受けた deploy の利用者に**再デプロイを案内**する。
3. 恒久修正(読み側の丸め / CHECK 制約 / migration)は落ち着いてから前へ進める。
4. **旧イメージへ戻すのは不可** — 下記 §5.96。

## 5.96. migration を含む版へ上げたら、イメージは戻せない(片道切符)

`state.rs` の `sqlx::migrate!(...).run()` は既定で `ignore_missing = false`。**DB に適用済みで手元の
バイナリに無い version があると `VersionMissing` で起動を拒否**する。つまり:

> **migration を 1 本足した版を ship した瞬間から、前の版へのロールバックは使えない。**
> 管制面が丸ごと 502 になる(テナント app は traefik のファイル route で生き残る)。**前進修復のみ。**

緊急にどうしても戻すなら `_sqlx_migrations` から該当 version の行を消してから旧版を起こす
(スキーマは新しいまま = データ側の後始末は手動)。**通常は選ばない** — 前へ直す方が速く安全。

## 6. やってはいけないこと

- **platform.sql を「動いている」管制面に重ね掛けしない**(必ず空 DB に流す。重複行で半端に死ぬ)。
- **master key を変えたまま復元しない**(暗号列が全部開けなくなる。復元は必ず同じ鍵で)。
- 復元中に server を走らせない(reconcile が中途半端な DB を正として孤児掃除を始める)。
- **`pgb_tls` 卷を消さない / certgen を再実行させない**:`if [ ! -f ]` なので卷が空だと**自己署名の種が
  静かに再生成**され、注入ホスト名と食い違って §E の状態になる(LE 証書の再配置が必要)。

## 7. 演練(年 1 回)

dev か予備機で:①最新バックアップを取り寄せ ②§2 を実施 ③テナント 1 本を §3 で復元
④`tbm db query` で実データを確認 ⑤**証書更新の演練**(`acme.sh --list` の Renew 列が未来日か +
`--renew --force` → `reload-pgb-cert.sh` の後で app が切れないか)⑥所要時間を本書末尾に記録。
**「復元したことのないバックアップはバックアップではない」**。

| 演練日 | 実施者 | 所要 | メモ |
|---|---|---|---|
| (未実施) | | | |
