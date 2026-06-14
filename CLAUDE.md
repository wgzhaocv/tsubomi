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

## フェーズ(現在地:M2 完了)

M0 基盤(ログイン/CLI token)→ **M1 database(完了)** → **M2 volume(完了)** → M3 service
(デプロイ経路+注入)→ M4 ガバナンス → M5 valkey。各フェーズ単体で使える状態にする。
マイグレーションはフェーズ毎に追加。

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
