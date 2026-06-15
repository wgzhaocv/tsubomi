# tsubomi 蕾 — 社内 PaaS プラットフォーム

セルフホストの「基礎版 Vercel + Neon」:社内の非エンジニアが AI(CLI)経由で
app をデプロイし、データベース / ボリュームを作る。
単機運用、プラットフォームのプロセスはホスト直走り(docker.sock を保持)。
ホストは今は香橙派(**ARM64**)、後で **x86_64** 機にも移す/増やす ⇒
イメージ・配布物は初日から両アーキテクチャ対応。

## 必読ドキュメント(アーキテクチャを変える前に読む)

- `paas-design-v2.md` — 設計意図:4 種のリソース(service/database/cache/volume)+
  動詞は「注入」ひとつ;境界と引き受けたコスト。
- `paas-tech-design.md` — 技術設計:**§0 の 6 つの確定事項を黙って覆さない**。
  DDL・デプロイ経路・API 面・マイルストーンは全部ここ。

背骨を一言で:**管制面 Postgres が「期望状態」を持ち、現実(コンテナ/ユーザ DB/
ディスク)をそこへ収束させる**。注入はバインディングだけを保存し、値はコンテナ
起動の瞬間に解決する(だから rotate 後は再デプロイして初めて効く — これは仕様)。

## フェーズ(現在地:M5 cache(valkey)完了 — 設計フェーズの 4 リソース出揃い)

M0 基盤(ログイン/CLI token)→ **M1 database(完了)** → **M2 volume(完了)** →
**M3 service(完了)** → **M4 ガバナンス(完了)** → **M5 cache/valkey(完了)**。
各フェーズ単体で使える状態にする。マイグレーションはフェーズ毎に追加。

M5 で入ったもの(S1–S3。dev e2e 済み):infra に **valkey**(`valkey/valkey:8-alpine`、edge 参加、
default off + `tsubomi-admin` を compose の `--user` で静的定義、ホスト側 6433 で loopback 公開)+
migration `cache_details`(`acl_user=namespace=c_<shortid>`/`password_enc`/`rotated_at`)。**cache リソース一式**:
`crates/server/src/{caches.rs,valkey.rs}`(create/list/get/rename/url/rotate/delete)。隔離は **valkey ACL**
(値 `~<ns>:*` + チャンネル `&<ns>:*` + コマンド白名単 `+@all -@admin -@dangerous -function -script` = 越境 /
FLUSHALL / KEYS / SCRIPT・FUNCTION FLUSH は NOPERM。値は隔離・key/channel **名**は SCAN/PUBSUB で列挙され得る =
受容済み §11-I)。per-cache ACL は揮発なので**起動時 + 30s 周期で収束**(`valkey::reconcile_acls`、毎 tick fresh に
生存 cache を読む = RACE-1)。**注入**:cache → `REDIS_URL`(内部入口 `tsubomi-valkey:6379`)+ `REDIS_KEY_PREFIX`
(`<ns>:`。値は起動の瞬間に解決 — rotate は再デプロイで効く)。rotate は **DB 先 → valkey**(背骨どおり前向き収束)。
ゴミ箱:delete=`ACL DELUSER`(key 温存)/ restore=ACL 再作成 + 生存 key 数報告(best-effort)/ purge=`SCAN+UNLINK`。
owner 最後の砦に cache delete、admin overview/ranking に cache(指標=key 数)。web 詳細(`CacheDetail.tsx`)+
CLI `tbm cache`。**最終 e2e 済み**:cache を使う service(Node ioredis カウンタ)をデプロイし、公開 URL で
`<ns>:visits` を INCR して跨リクエスト永続・隔離内を実機確認。実装級は **`paas-m5-design.md`**。

M3 は prod-infra 込みで完了し **`tsubomi-app.com` で本番稼働・端到端検証済み**(両デプロイ経路:
`git push`→GitHub Actions と `tbm deploy --local` の両方で `https://<sub>.tsubomi-app.com` が開くことを実機確認)。
本番トポロジ:香橙派(arm64、共有ホスト)+ **Cloudflare Tunnel**(上流 TLS 終端 → `TSUBOMI_TLS` 未設定 =
traefik は HTTP :80)。専用ドメイン `tsubomi-app.com`(CF zone)でサービスは一級子域 `*.tsubomi-app.com` =
免費 Universal SSL 覆盖(ACM 不要)。デプロイは `just ship`(`docker save|ssh load`、`~/tsubomi-deploy`)。
詳細・2 モード(上流TLS / 直VPS+LE)は `paas-m3-design.md` §13。

M1 で入ったもの:`resources` スーパーテーブル + `database_details`/`database_roles`
+ `audit_log`;pg-tenant(ユーザ DB)+ pgbouncer(外部入口、auth_query、client TLS);DB 作成/
一覧/接続文字列/rotate/web SQL/ソフト削除→ゴミ箱→復元/日次バックアップ;at-rest
暗号化(crypto.rs、XChaCha20-Poly1305);`tbm db` サブコマンド。**双 role**:app
(内部、M3 で service に注入)+ human(外部、rotate 可)— 詳細は §2/§5。

M2 で入ったもの:`volume_details`;**volume は顶层リソース**(service 所有ではない)。
各 volume は独立した假根サンドボックス `volumes/<user>/<id>`。**唯一のハード境界 =
パストラバーサル防御**(`volumes/safe_path.rs`:Linux=openat2 `RESOLVE_BENEATH|NO_SYMLINKS`、
dev macOS=canonicalize フォールバック、`..`/絶対/NUL/symlink 越えを全拒否)。ファイル API
(列挙/ダウンロード/アップロード=一時ファイル+atomic rename/削除/mkdir/move)+ web
ファイルブラウザ(**パスは URL の splat に持つ** `/volumes/:id/files/<path>`)+ `tbm volume`
フル + ゴミ箱(trash へ mv / 復元 / 完全削除)の web/CLI 入口 + volumes の日次 rsync。
**注入(service への mount + `STORAGE_PATH`)は M3** — 動詞「注入」の相手は service。

M3 で入ったもの(S1–S8):`service_details`/`deploys`/`injections`/`service_env`/
`deploy_nonces`;**service リソース一式** — create + GitHub オーケストレーション(CLI が
ユーザ自身の `gh` で repo/secret/workflow を設定。平台は GitHub に触れない)、deploy hook
(HMAC=権限・nonce・digest ピン留め)+ 非同期パイプライン(bollard、**start-first swap** =
新コンテナを起こし存活確認 → route 切替 → 旧削除。失敗時は旧版を温存 §6.4)、**注入**
(database→app role の内部接続文字列 / volume→bind mount + `STORAGE_PATH` / 静的 env。値は
**コンテナ起動の瞬間に解決** = rotate 後は再デプロイで効く)、lifecycle(start/stop/logs/
delete→ゴミ箱/rollback)、web 詳細ページ(概要/デプロイ/注入/環境変数/ログ)、**reconcile**
(`services/reconcile.rs`、起動時フル + 30s:存在収束 + 孤児掃除。`restart=unless-stopped` が
第一の保険、これが第二)。ルーティングは **traefik file provider**(`svc-<id>.yml`。docker
provider は Docker Engine 29 で壊れるため不使用)。**残り = prod-infra**:GH Actions buildx
双架(arm64+amd64 manifest list)+ 本番 traefik(:443 + LE + 会社 IP 許可リスト)/ pgbouncer /
registry 入口の落とし込み。

M4 で入ったもの(S1–S4。**owner ガバナンスは web 専用** — admin ハンドラは owner 身分 **かつ**
session 由来を毎回検証 `admin::require_owner_web`、Bearer cli_token は拒否):`platform_config` +
`admin_action_codes`。**`crates/server/src/admin/`** に集約 — (S1)**可視化** overview/ranking:
跨ユーザの**匿名化**一覧(真名 + 匿名番号 service1 等、display_name/中身は出さない)+ 指標
(service=bollard stats CPU/内存、database=`pg_database_size`、volume=`volumes::dir_usage` 再利用)。
(S2)**Resend メール基盤** `mail.rs`(既存 reqwest、`RESEND_API_KEY` 未設定=log のみ・本文は出さない)
+ **ディスク水位警告**(gc の 1h tick で `df -Pk`、`platform_config['disk_alert_state']` で去重 =
level 上昇 or 24h、送信成功時のみ notified_at 前進)。(S3)**最後の砦**:owner が他人の
service/database/volume を停止/削除(`POST /api/admin/resources/:id/{stop,delete}`、code 無し=
6 桁コードを owner にメール / code 有り=単回消費で検証 → 実行 → **誤コードは焚码で総当たり封じ**)。
既存ソフト削除を `soft_delete(state,id)`(所有権・audit 抜きの素の操作)に切り出しユーザ口と共有、
owner の delete も**対象ユーザのゴミ箱**へ(復元可)。`audit_with_target`(target_user も記録)。
(S4)**audit 閲覧** `GET /api/admin/audit`(keyset 分頁 + action 前方一致、actor/target_user 真名 join、
target_resource は UUID のまま)。web は侧栏 owner 限定 + `RequireOwner` ルート守衛に集約。
**否決(後相)**:共有パスワードの只读 viewer(設計 §7 の「見るは共有密码」。owner-gated で完了判定を
満たすため、別認証路は当面入れない — `paas-m4-design.md` §10-A)/ owner 管理 UI(2 人目追加削除は
env 種子のまま §10-H)。実装級は **`paas-m4-design.md`**。

## 重要な約束事

- **ドキュメント(md)もコードコメントも日本語で書く**(設計議論の中国語
  ドキュメント paas-design-v2.md は例外)。
- Rust の依存は `cargo add -p <crate>` のみ。`[dependencies]` は手書きしない。
- フロントの `vp` = vite-plus(bun + React + TS + Tailwind v4 + shadcn)。vite の
  typo ではない。
- **auth は `~/Desktop/projects/amber` からの移植**(users+credentials の 2 表、
  PKCE CLI ログイン、token は sha256 保存)。tsubomi の差分:session /
  oauth-state / authcode は **Redis ではなく Postgres**(単回消費は
  `DELETE..RETURNING`);Google ログインに **hd ドメイン制限**
  (`TSUBOMI_ALLOWED_HD`、カンマ区切り複数可、サーバ側検証);owner ロール
  (env で種付け、ログイン時昇格);apps の概念は無し。
- Google OAuth は oauth2 crate を使わず手書き(認可 URL + token 交換だけ)。
  理由:oauth2 5.0 が reqwest 0.12 に縛られ、最新 reqwest に上げられないため。
- `time` crate は `=0.3.47` にピン:0.3.48 が cookie 0.18 の blanket impl と
  衝突する(E0119)。cookie 側の対応版が出たら外す。
- CLI バイナリは **`tbm`**(crate 名は tsubomi-cli のまま)。token プレフィックス
  `tbm_`、authcode `tbmc_`、client_id `tbm-cli`。
- CLI の更新は**通知制**:version check はコマンド後に stderr で一言出すだけ。
  更新は常にユーザの手動 `tbm update`(自動更新はしない)。
- `tbm login` は**自動判定**:ローカル GUI は **RFC 8252 loopback**(127.0.0.1 の
  一回限りリスナー)でブラウザの「許可する」だけ、SSH 先・ヘッドレスは自動で
  コピペ方式に倒す(SSH では loopback が原理的に不成立 — リダイレクト先の
  127.0.0.1 は手元マシンを指しリスナーのいる遠隔機ではない)。検出は
  `SSH_CONNECTION`/`SSH_TTY` + Linux の DISPLAY 無し。完全でない(sudo は env を
  消す / mosh)ので `--manual`(強制コピペ)/ `--web`(強制 loopback)で上書き可。
  判定ロジックは `choose_manual()` に切り出し真理値表テスト済み。サーバ側の
  redirect_uri 許可は 2 形のみ(完全一致の本番コールバック / loopback 任意ポート)。
- CLI の配布:`just release-cli-publish`(scripts/release-cli.sh)が 4 ターゲット
  (mac-arm / linux-arm64 / linux-x64 / windows-x64-gnu)をビルドして Pi の
  `~/tsubomi/releases/` へ公開。インストーラは `/install.sh|.ps1|.bat`(配信時に
  サーバがドメインを注入し、初期 config に server_url も書く)、manifest の url は
  相対パス — どちらもドメイン非依存。
  **リリースは不可変**:内容を変えたら必ず CLI の version を上げる(同名再発行は
  Cloudflare が .gz/.zip をキャッシュするため checksum mismatch になる。スクリプトに
  ガードあり)。
  install.sh は rc に PATH マーカーブロックを書き、`tbm uninstall` がそれを目印に
  残留物ゼロで消す。マーカーの正本は tsubomi-shared の `PATH_MARKER_BEGIN/END`
  (install.sh にはインライン展開 — 変えるときは両方揃える)。
- クロスビルドの注意:Homebrew の rust が PATH 先頭にいるので、ビルドスクリプトは
  `PATH="$HOME/.cargo/bin:$PATH"` を前置して rustup の 1.95 を使う(リンクは zig)。
  CLI の TLS は rustls-no-provider + **ring**(aws-lc は windows-gnu / linux への
  クロスコンパイルが通らない)。main() で provider を install_default() している。
- web と CLI は同一 axum ハンドラの 2 入口。分岐は認証 extractor(session
  cookie / Bearer)だけ。新機能を API ハンドラとして書けば CLI から自動的に
  使える。
- **CLI は AI 駆動が主用途 — I/O は「AI フレンドリ」を既定にする**(新コマンドも
  必ずこの型に従う。実装は `crates/cli/src/{main.rs,api.rs,commands/}`):
  - **出力形式はグローバル `-o/--output`(env `TBM_OUTPUT`)、既定 `auto`**。auto は
    **stdout が端末なら text・パイプ/捕捉なら json**(`commands::OutputFormat::resolve`)。
    AI は出力を捕捉する=非 TTY なので、`-o` を付けなくても自動で JSON になるのが要点。
  - **成功出力(json)は shared の DTO をそのまま serde_json**(裸の array/object・
    フィールド安定・jq 可能)。新フィールドは足してよいが既存を壊さない。
  - **エラー(json)は `{"error","code"}` を stdout に出して非零終了**。成功も業務
    エラーも同じ stdout 流で parse できる(`main` のエラー信封)。
  - **`code` は機械分岐用の安定列挙、`error` は人間可読の文案**。code を文字列照合
    させない。列挙:`unauthorized / forbidden / not_found / conflict / validation /
    server_error`(+ ローカル解決の `not_found`、その他 `error`)。HTTP ステータスから
    派生(`api::code_for`)。API 由来は `api::ApiError` に載せて `main` で downcast。
  - **サーバはユーザエラーを 4xx で返す(500 に潰さない)**。重複は 409 Conflict、検証
    失敗は 400。DB の UNIQUE 違反(23505)は `databases::map_unique` で Conflict に。
    これを怠ると AI が「サーバ障害」と誤判し無駄リトライする(過去の実害)。
  - **エラーメッセージは次の一手を含める**(`tbm login を実行` / `tbm db list で確認`)。
    AI が自己修正できる。
  - **秘密は警告を stderr・値を stdout**(json では値だけ。例 `{url}`)。対話的操作
    (`db connect` の psql)は json では起動せず接続先だけ返す。version 通知などの雑音は
    json モードで抑止する。
  - **引数にも help を必ず書く**(`<NAME>` 等)。AI に意味を推測させない。
  - 既知の許容ギャップ:clap の使用法エラー(引数不足等)は text/stderr/**exit 2**
    (実行時エラーの exit 1 と区別できるので可)。display_name 等の表示名は自由文字列
    (識別子は別生成の wire 名)。

## 開発

```bash
just db-up        # infra 起動:pg-platform(:5434)+ pg-tenant(:5435)+ pgbouncer(:6432)
                  #   (マイグレーションはサーバ起動時に自動)
just dev          # server :9090 + web :5173(/api プロキシ。8080 は amber)
just cli login    # dev では CLI のデフォルトサーバは :5173(ログインフローが SPA ルートを使う)
just check        # cargo check + clippy -D warnings + web lint
```

`.env` 必須:`GOOGLE_CLIENT_ID/SECRET`(Google Cloud Console で OAuth client を
作成、redirect URI に `http://localhost:5173/api/auth/google/callback` を追加)。
ドメイン制限と owner の種も `.env` にある。

## 破ってはいけない一線

- 隔離は仕組みで守る、規律に頼らない:IP 許可リスト(Traefik 層)、制限付き pg
  資格情報、volume のパストラバーサル防御(openat2)、コンテナのメモリ硬上限。
- 資格情報 4 種(接続文字列 / deploy key / session / CLI token)は相互流用禁止。
  ハッシュか復元可能かは「プラットフォームが原文を必要とするか」で決まる。
- owner 操作はバックエンドで毎回検証。フロントの表示制御はただの UX。
- マルチアーキテクチャ:イメージ / バイナリは両ターゲット(aarch64 = 香橙派、
  x86_64 = 将来のホスト)。M3 の GH Actions は buildx で両アーキテクチャを出す。
