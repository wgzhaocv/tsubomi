# db fork — database 複製(設計メモ)

> 実装:`crates/server/src/databases.rs::fork / fork_inner / provision_database`、
> `tenant.rs::fork_database`(pg_dump|psql パイプ)、CLI `tbm db fork`、
> web `DatabaseOverview.tsx::ForkSection`。migration なし。

## 動機と境界

「基礎版 Neon」の看板能力 = 複製。dev/prod のような複数環境を作るとき、空庫 +
migration 再生 + 手動データ投入しかなかったのを、**「この瞬間の完全な複写」を一動詞**で
提供する。CREATEDB は tenant-admin 権限であり、ユーザ容器では代替不能 = 平台がやるべき
仕事(「只有平台能做的,才值得平台做」の筛子を通過)。

**同期はやらない(恒久)**:fork の意味論は「分岐した瞬間から別々の道」。
- データ向下(prod→dev の刷新)= **再 fork**(消して fork し直す)。
- 構造向上(dev→prod)= **app 自身の migration**(ユーザ自留地。平台は触らない)。
持続同期は dev の意義(汚してよい)を壊すので機能として存在しない。

## 確定事項

| # | 決定 | 理由 |
|---|---|---|
| 1 | 同期 201・migration ゼロ | database_details に phase 列が無く、202 化は状態の器 + 起動時回収 + ポーリング GET が芋づる。soft_delete が同期 dump している前例あり |
| 2 | **`pg_dump \| psql` パイプ直結**(TEMPLATE 不使用・中間ファイル無し) | `CREATE DATABASE … TEMPLATE` はテンプレート元に接続 1 本でもあると失敗 = pgbouncer 常時接続の本番で実質不成立。pg_dump は MVCC の一致スナップショット = 元 DB 無停止。パイプは磁盤 I/O ゼロ + dump/restore 並走で墙鐘約半分(落盤中転は simplify レビューで却下 — 「restore 失敗時に dump を残して排障」という理由は、無条件削除するなら成立しない) |
| 3 | 新 DB は完全な新規資源(新 wire 名 / 新 role 3 本 / 新パスワード 2 本) | 元と資格情報を共有しない(資格情報 4 種の相互流用禁止と同じ精神)。開通は `provision_database`(create と共有の骨格)に一本化 |
| 4 | `--schema-only`(既定はデータ込み) | pg_dump 原生フラグの透传でほぼ無料。機微データを撒かない / 大庫の高速化 / CI 用。第三档(採样・脱敏)は作らない |
| 5 | ハンドラ本体は `tokio::spawn` + JoinHandle await | クライアント切断(CF Tunnel ~100s)でハンドラ future が cancel されても**作業は完走**し、ロールバック規律が壊れない。応答を受け取れなくても新 DB は一覧に現れる |
| 6 | タイムアウト `TSUBOMI_FORK_TIMEOUT_SECS`(既定 300)は**流し込み段だけ**に掛ける | commit(insert_rows)まで包むと「期限が commit 直後に切れる → platform 行は在るのに tenant DB を掃除」という不変式破りが起き得る(altitude レビューで捕獲)。DDL/insert は有界なので包まない。`kill_on_drop` で期限切れは子プロセスも止まる(「T 秒で報告」ではなく「T 秒で止める」) |
| 7 | **流し込み(psql)は admin ではなく新 DB の app role で接続する** | codex 審査の主指摘:dump の中身はユーザ制御(CHECK 制約から呼ばれる関数等に任意 SQL)で、admin セッション + `SET ROLE` は `RESET ROLE` 一発で superuser に戻れる = 跨租户。app role なら session_user 自体が無特権。`ALTER ROLE app SET ROLE owner` 済みなので所有権は従来どおり owner。**trash 復元(`tenant::restore_database`)も同穴なので同時に修正**(既存の穴 — fork が炙り出した) |

## 処理の順序(ロールバック規律)

```
所有権 + 存在チェック(元。ゴミ箱内は 404 に収束)
→ 新名の EXISTS(ensure_db_name_free、409。UNIQUE の最終ガードは insert_rows)
→ provision_database(create と共有):
   ① create_database(新 DB + role 3 本)
   ② fork_database = pg_dump 元 | psql 新(timeout はここだけ。
      PGOPTIONS role=owner で復元オブジェクトを owner 所有に。
      **exit code は両方検査** — pg_dump 途中死でも psql は正常終了し得る)
   ③ encrypt + insert_rows(platform 行、1 tx)
→ audit "db.fork"(detail に source_id / source_display_name / schema_only)
```

①以降のどこで失敗しても掃除は `drop_database_and_roles`(IF EXISTS)1 本。
元 DB にはどの経路でも一切書かない。

## 受容した限界

- **CF Tunnel の ~100s 応答上限**:大庫のフルデータ fork は応答が先に切れ得る。作業は
  spawn で完走するので `tbm db list` で確認できる。痛くなったら 202 + phase 化(今回はやらない)。
- **server プロセスが fork 中に死ぬと孤児 tenant DB が残る**(窓は分単位 — `just ship` の
  server 入替が着地し得る)。自動回収はしない(DR 直後の誤爆が怖い)代わりに、
  **起動時に platform 行の無い `db_*` を warn で可視化**する(`log_orphan_tenant_dbs`)。
- fork 中の元 DB への書き込みはスナップショットに入らない(MVCC の一致点はそれ以前)— 仕様。
- 並行の同名 fork は片方が終盤の UNIQUE で 409(事前 EXISTS で窓は小さい)— 受容。
- conn_limit は既定(100)で引き継がない(元がカスタムでも新 DB は既定。owner 調整は後相のまま)。

codex 深審(2026-08-03)で指摘され**受容**したもの(いずれも発生条件が極端 or 既存 create と同型):

- **RLS policy が元の wire role 名(`db_<src>_app` 等)を直接参照している場合**、fork 先では policy が
  新 role に合わず(0 行 / 書込拒否)、元の物理削除も跨庫依存で阻まれ得る。dump 時の policy 書換は
  別量級の工事なので受容(社内で wire 名直書きの RLS はまず出ない。踏んだら fork 先で policy を張り直す)。
- **commit 確認パケット喪失の歧義**(commit 成功なのにエラー扱い → ロールバックが tenant DB を落とし
  platform 行だけ残る)— 管制面 Postgres は同一ホスト loopback で窓は実質ゼロ。create も M1 から同型。
- **platform SQL / DDL は無期限**(advisory lock を長く握る他事務がいると fork task が待ち続ける)—
  create と同型。期限を足すと上の commit 歧義を自分で作るため、現状維持。
- **pg_dump をローカル kill しても元側 backend のロック待ちは即死しない** — `--lock-wait-timeout` で
  待ち自体を有界にした。残余(granted 後の書込で EPIPE 死)は受容。
- **孤児検査は単向**(DB 有り・行無しのみ。role だけの残骸や restore 中断の状態錯配は見ない)—
  最小可視化という位置づけどおり。双方向対账が要る規模になったら独立設計。
- 旧 v43 未満の server に新 CLI の fork を打つと SPA fallback の 200+HTML で JSON parse エラーになる —
  該当部署はもう存在しない(v43 で /api 404 化済み)。
