# tsubomi PaaS — コンテナ内アクセス(ターミナル + exec)実装設計

> M5 完了後の**追加機能**(マイルストーン外)。service 詳細にコンテナの中を確認する手段を
> 足す。背骨は変えない:管制面 Postgres が期望状態を持つ枠組みはそのまま、これは「現実の
> コンテナを覗く / 触る」読み取り・運用系の窓口を増やすだけ(新テーブル・新 migration なし)。
>
> 背骨を一言で:**bollard exec ひとつを土台に、用途で 2 入口に分ける** — 人間の対話探索は
> web の PTY ターミナル、AI / スクリプト / 線上診断は CLI の一発 exec。どちらも所有者が
> **自分の**稼働中コンテナにだけ届く。
>
> 完了判定:自分の running service に web ターミナルから `/bin/sh` で入れ、`tbm service exec
> <name> -- <cmd>` で stdout/stderr/exit_code が返る。他人の service は両方 404。

---

## 0. なぜ・スコープ

service 詳細には 概要/デプロイ/注入/環境変数/ログ があるが、**ログは標準出力の tail を
pull する一方向の窓**でしかなく、動いているコンテナの中(プロセス・ファイル・`env` の注入値
検証・`curl` の内部疎通)を確認できない。線上で不具合が出たとき所有者(や代理 AI)が現場を
確認できない、という穴を塞ぐ。

出すもの(2 能力):

- **A. web 対話ターミナル**(WS・PTY):ブラウザから自分の running コンテナへ `docker exec -it
  … /bin/sh` する。`@wterm/react` で描画、後端は `services/docker.rs::handle_terminal`。
- **B. CLI 一発 exec**(HTTP・非対話):`tbm service exec <name> -- <cmd…>`。1 コマンドを実行し
  `{stdout, stderr, exit_code, truncated, timed_out}` を返す。`docker exec`(`-it` なし)相当。

出さないもの:owner→他人コンテナ(M4 匿名化・監査と衝突)/ 対話 PTY の打鍵監査(裸ストリーム
で不可)/ CLI 対話シェル(`tbm service shell`。今回は範囲外)/ 新テーブル・migration。

---

## 1. 役割分担の原則(なぜ対話は web だけ・exec は CLI に乗るか)

CLI の I/O 規約は「AI フレンドリ JSON」(`CLAUDE.md`)。**対話 PTY はこの契約に本質的に合わない**
(AI は対話シェルを駆動しない)ので web 専用にする — `db connect` を json モードで起動せず
接続先だけ返すのと同じ理屈。一方 **一発 exec は捕獲出力 = AI が駆動できる**ので CLI に乗せる
(CLI 面は「AI 駆動のユーザ資源操作専用」という既定方針と一致)。両者とも同一 axum ハンドラの
2 入口で、web/CLI の分岐は認証 extractor だけ。

---

## 2. 権限・暴露境界(破ってはいけない線をどう守るか)

- **所有者の自資源のみ。** A も B も既存の `ensure_owned(state, user_id, id)` を通る
  (他人 / 不在 / 削除済みは 404)。owner ガバナンスとは無関係 = owner が他人のコンテナに
  入る経路は**作らない**(M4 の匿名化境界・真名+明文の監査契約と正面衝突するため)。
- **A(terminal)は session 由来必須(web 専用)。** `auth.is_session()` を要求し Bearer cli_token は
  拒否する(`require_owner_web` と同じ作法)。対話 PTY は CLI の JSON 契約に合わないので入口を
  web セッションに限る。B(exec)は CLI 主用途なので Bearer も session も受ける。
- **暴露レベルは既存の web SQL と同一ティア。** shell / exec から `env` で注入値(DB app role
  接続文字列・`REDIS_URL`・`STORAGE_PATH`)が平文で見える。所有者は元々その binding を持ち
  rotate もできる = 受容済み(web SQL が自分の DB に任意 SQL を打てるのと同列)。DB app role が
  「内部・非公開」設計であることへの軽微な侵蝕は、ここに意識的に記録しておく。
- **隔離は破れない。** exec / shell は自分のコンテナ内に閉じる:メモリ硬上限そのまま、
  ネットワークは `tsubomi-edge` のみ、docker.sock には触れない(平台プロセスだけが保持)。
  コンテナ内 root であってもホスト root ではない。

---

## 3. 監査の範囲(記録できるもの・できないもの)

- **B(exec)**:`audit` に `service.exec` + **argv** を残す(コマンドは追える)。出力は秘密を
  含み得るので記録しない。
- **A(terminal)**:`audit` に `service.terminal.open` の **起動イベントのみ**。対話 PTY は
  裸のバイトストリームなので**打鍵内容は記録不可** — これは仕様上の限界として受容する。

---

## 4. ワイヤープロトコル(A の WS)

`@wterm/react` は「自前トランスポート持ち込み」設計(`onData(string)` / `onResize(cols,rows)` /
`write(bytes)`)。後端の素直な 2 フレーム規約に乗せる:

| 向き | フレーム | 意味 |
|---|---|---|
| client → server | `Binary` | 生 stdin バイト(`input.write_all`) |
| client → server | `Text`(JSON) | 制御 `{"type":"resize","cols":N,"rows":M}` → `resize_exec` |
| server → client | `Binary` | exec 出力(tty 下の生 PTY バイト。失敗通知も人間可読 Binary)|

前端:`onData → ws.send(TextEncoder.encode(d))`、`onResize → ws.send(JSON)`、
`onmessage(ArrayBuffer) → term.write(Uint8Array)`、`ws.binaryType="arraybuffer"`。

---

## 5. 後端の地雷(handle_terminal の正しさ)

`bollard 0.21` の exec を `tokio` で双方向に流す。落とし穴(コード内コメントの正本):

1. **create と start の `tty` を一致させる。** 片方だけ tty=true だと daemon の 8 バイト多重化
   ヘッダの有無が decoder とずれ、出力が壊れる(xterm にゴミ)。
2. **WS を `split()` して 2 方向を独立に進める。** `input` と `output` は同一ハイジャック TCP の
   両半分。1 つの `select!` で直列化すると遅い write が出力を塞ぐ(HOL ブロック)。
3. **`input` の drop が唯一の後始末。** `delete_exec` は無い。stdin EOF → `sh` 終了 → daemon が
   exec を回収。client→container 方向の future が確実に drop されないとゾンビ exec + docker.sock
   接続をリークする。`select!` 終了で両 future が drop される。
4. **`resize_exec` は `start_exec` の後でのみ有効。** PTY 既定は 80x24、最初の resize で合わせる。
5. **背圧は `ws_tx.send().await` に任せる。** output と sink の間に無制限バッファを挟まない
   (暴走プロセスでメモリ無制限になる)。
6. **最大セッション timeout で包む(既定 60 分)。** CF Tunnel など逆プロキシ越しの半開き接続で
   `recv` も `output` も EOF せず `sh` が生き残るのを防ぐ backstop。
7. **升级後の失敗は HTTP で返せない。** 開いた socket に人間可読 `Binary` + `Close` で伝える。

非対話 exec(B)は `tty なし` = daemon が stdout/stderr を多重化分離するので別々に蓄積、出力上限
(1MiB)で `truncated`、サーバ側 60s timeout で `timed_out`、`inspect_exec` で `exit_code`。

---

## 6. CLI の I/O(`tbm service exec`)

- **json モード**:`ExecResult` をそのまま出す。`exit_code` は**データ**(tbm 自身は 0 終了 =
  リクエスト成功 ≠ 業務エラー。AI はこの値で分岐)。
- **text モード**:stdout は stdout・stderr は stderr へ素通しし、**コマンドの exit_code を tbm の
  終了コードへ伝播**(ssh / docker exec 風。シェルで `&&` 連結可)。`truncated`/`timed_out` は
  stderr に一言。
- argv はそのまま渡す(shell 解釈なし)。pipe/glob は `-- sh -c "ps | grep node"`。

---

## 7. 触るファイル(実装の地図)

| 層 | ファイル | 追加 |
|---|---|---|
| shared | `crates/shared/src/lib.rs` | `ExecReq` / `ExecResult` |
| 後端 | `crates/server/src/services/docker.rs` | `running_container_name` / `exec_capture` / `handle_terminal` / `terminal_fail` / `parse_resize` |
| 後端 | `crates/server/src/services/mod.rs` | `POST /services/{id}/exec`(`exec`)/ `GET /services/{id}/terminal`(`terminal`)+ 監査 + argv 検証 |
| CLI | `crates/cli/src/api.rs` | `service_exec` |
| CLI | `crates/cli/src/commands/service.rs` | `ServiceCmd::Exec` + ハンドラ |
| web | `web/src/routes/ServiceTerminal.tsx` | ターミナル画面(`@wterm/react`)|
| web | `ServiceLayout.tsx` / `router.tsx` | サブナビ + 子ルート |
| web | `web/src/wterm.d.ts` | `@wterm/react/css` の副作用 import 宣言 |

ライフサイクル:WS close 駆動 + 最大セッション timeout。再デプロイで容器が入れ替わると exec は
容器ごと死ぬ(WS が閉じる)= 想定挙動。

## 8. セキュリティ(codex 深度レビュー反映)

- **CSWSH(Cross-Site WebSocket Hijacking)— 対応済み。** session cookie は `SameSite=Lax` で、
  テナント app は `<sub>.<domain>` = 管制面と**同一登録ドメイン = same-site**。よって SameSite は
  テナント app ↔ 管制面の間を守らない(悪意あるテナント app が被害者の cookie 付きで管制 WS を
  開け得る)。**対策 = WS 升级で `Origin` を管制面オリジンに固定**(`auth::require_ws_origin`)。
  allowlist は `Config.control_origins` = 既定 `server_url`(ブラウザが到達するオリジン。dev は
  `http://localhost:5173`、prod は OAuth redirect と同じ管制面 URL)+ `TSUBOMI_CONTROL_ORIGIN`
  (カンマ区切りで追加)。`terminal` と既存の `/api/admin/metrics` WS の**両方**に適用。ブラウザは
  WS で必ず `Origin` を送るので **Origin 欠落も拒否**(対話 WS は web 専用)。加えて `terminal` は
  `is_session()` 必須(Bearer 経路を塞ぐ。web 専用の徹底)。
- **同時セッション数の上限 — 未実装(後続候補)。** 認証済みテナントが terminal / exec を多数並行に
  開くと、コンテナ cgroup の外(server task・docker daemon 状態・FD)は宿主資源。per-user /
  per-service の小さな semaphore + `429` が推奨だが、当面は据え置き(脅威=社内ユーザ・影響限定)。
- **timeout 後の orphan exec — 受容。** exec_capture が 60s で打ち切った後もコンテナ内プロセスは
  自然終了まで走る(docker exec の仕様)。cgroup で抑えられるので受容(長時間は web ターミナルへ誘導)。
- **出力 cap は厳密。** 1 フレームで予算超過しても残余だけ取り込み `truncated=true`(1MiB を厳守)。
- **WS Origin 検証は今はハンドラ明示(後続候補)。** terminal / metrics が `require_ws_origin` を
  各々呼ぶ。WS ルートが 3 本以上に増えたら、升级前に走る middleware / extractor へ寄せて
  「足し忘れ = セキュリティ穴」を構造的に防ぐ(現状 2 本なので明示で十分・むしろ可視)。
