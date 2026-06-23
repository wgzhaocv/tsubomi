# tsubomi セキュリティ強化バックログ(後相で着手)

codex の監査(2026-06-17)で挙がった指摘のうち、**「直すべきだが改修が大きい / インフラ層」**で
今回見送ったものをここに集約する。小さく高価値なものは既に修正済み(下記「対応済み」)。
着手する人は各項の **修復方向** と **規模** を見て、独立したスライスとして進めること。

## 対応済み(参考)

- 卷 symlink/TOCTOU を fd 相対操作へ(safe_path 全面書き換え)。
- session(cookie)CSRF/CSWSH:不安全メソッドで Origin を管制面に固定(`auth/middleware.rs`)。
- 容器加固(温和集):pids_limit / 日志轮转 / no-new-privileges / cap_drop NET_RAW(`services/docker.rs`)。
- 死列 `service_details.public` を削除。
- 危険操作の確認コード:mail 未設定 + dev フラグ未指定なら fail-fast、log 出力は dev フラグ門控(`admin/actions.rs`)。
- 公開 DB の IP 許可リストを **fail-closed**(空リストで TCP 入口を書かない。`ipblock.rs`)。
- deploy hook:ソフト削除 service を認証前に弾く + nonce 長さ/文字種制限(`services/deploy.rs`)。
- `GET /services/:id/logs` にバイト上限(`services/docker.rs`)。
- route ドリフト収束(deploy 末尾の route::write 失敗で公開 URL が旧版を指したまま漂流する穴。`services/reconcile.rs`)。

---

## 1.(High)registry の repo/service 級認可が無い — 跨租户 registry 汚染・ストレージ DoS

- **現状**:`registry.<domain>` は Traefik の per-user BasicAuth だけ。後端 registry 自体は無認証
  (`compose.prod.yml` の registry / `services/registry.rs`)。Traefik は「正当な registry ユーザか」しか見ず、
  URL path の `<service_id>` repo をそのユーザが所有するかは判定しない。
- **影響**:任意テナントが自分の registry 資格で `registry.<domain>/<任意の service UUID>`(や任意 repo 名)へ
  manifests/blobs を push できる。**deploy hook は受害者の deploy key + digest ピン留めを要求するので直接の
  乗っ取り(他人の service を勝手にデプロイ)はできない**が、(a)他人の repo namespace を汚染、(b)registry の
  ディスク圧迫(DoS)、(c)GC/purge の意味論を複雑化できる。
- **現状の緩和**:digest 内容アドレス + per-service deploy key で「デプロイ経路」は守られている。漏れているのは
  「push 先 repo の所有権」だけ。
- **修復方向**:Docker registry の **token 認証(authz service)** を挟み、`username → 所有 service_id 群` で
  repository scope を発行する。あるいは **per-service の registry 資格**に切り替える。併せて registry の repo を
  定期列挙し、どの生存 service にも紐づかない / owner 不一致の repo を GC する。
- **規模**:大(認可サービス新設 or 資格モデル変更 + GC バッチ)。

## 2.(High)容器がディスクを枯渇させられる経路が残る — writable layer / volume / backup

- **現状**:HostConfig には memory / pids_limit / 日志轮转 を入れたが、**readonly rootfs / writable layer の
  quota / volume quota / backup の上限が無い**(`services/docker.rs` / `services/inject.rs` の bind / `config.rs`
  の volumes_dir / `gc.rs` の日次 rsync)。ディスク水位告警は主に volumes_dir 基準で、`/var/lib/docker`・registry・
  backup が別 FS だと取りこぼし得る。
- **影響**:暴走 / 悪意 app が容器 rootfs に大量書き込み、または mount volume へ無限書き込み。日次 `rsync -a` が
  volumes を backup へ複製して増幅。宿主機ディスク満杯 → Docker / registry / Postgres dump / Traefik 動的設定書込が
  巻き添えで全平台障害。
- **現状の緩和**:メモリ硬上限・pids_limit・日志轮转で「メモリ / PID / ログ」面の枯渇は塞いだ。残るは「ディスク本体」。
- **修復方向**:
  - 容器を既定 **read-only rootfs + 必要 dir(/tmp 等)に tmpfs**(破壊面があるので段階導入 / per-service オプト可)。
  - volume / writable layer に **project quota(xfs/btrfs/zfs)** を被せる(per-volume サイズ上限。`max_upload_bytes`
    は 1 リクエスト上限であって総量 quota ではない)。
  - Docker root / registry / backup の **各 FS を個別監視**し、高水位で熔断(新規 deploy / アップロード拒否)。
  - backup を増分 / 圧縮 + 保持容量上限。
- **規模**:大(ホスト FS 構成 + quota 機構 + 監視拡張。一部は OS / FS 依存)。

## 3.(Medium)テナント容器に egress(出站)策が無い — 私網隔離 ≠ 滥用隔離

- **現状**:per-service bridge 私網で東西向(他テナント / infra 内部網)は遮断したが
  (`services/network.rs` / `services/docker.rs`)、**出站方向の策が無い**。bridge は既定 non-internal で、
  DOCKER-USER の egress deny/allow も egress proxy も無い。
- **影響**:悪意 app が公網へスキャン / 滥発、Docker bridge gateway や宿主機の 0.0.0.0 バインドサービスを探る。
  対外滥用は IP reputation も毀損する。
- **現状の緩和**:跨租户 east-west は per-service 私網で下げてある。欠けているのは egress 層。
- **修復方向**:egress 策を明文化 —— 既定で宿主 / プライベート網段を遮断し、DNS / HTTP(S) か proxy 経由だけ許す。
  公網能力が要る service は明示的に開放。`DOCKER-USER` チェインで回帰テストを持つ。
- **規模**:中〜大(iptables/nftables の DOCKER-USER 運用 + テスト。OrbStack dev では検証しづらく prod Linux 前提)。

---

## 小さめの follow-up(余力で)

- **(Medium)env key / volume mount path の健全化**(`services/mod.rs` の `validate_env_key` /
  `validate_mount_path`、`services/inject.rs`):env key は今 空/`=`/NUL のみ拒否、mount path は絶対 + `:`/NUL のみ。
  `LD_PRELOAD`/`NODE_OPTIONS`/`PATH` 等や `/etc`・`/usr` への mount を許す。**主にテナント自身の容器を壊す自傷**で
  跨租户ではないため優先度低だが、PaaS では 502 多発 + サポート負荷になる。修復:env key を `[A-Za-z_][A-Za-z0-9_]{0,127}` 等に、
  mount path に予約名 / システムパス denylist + 正規化。(初回監査でも挙がり、ユーザ判断で見送った項)。
- **(Low)deploy 失敗ログ尾の secret 混入**(`services/deploy.rs` の `start_container` → `deploys.error`、
  `services/mod.rs`):起動失敗 app が stderr に `DATABASE_URL` 等を吐くと、その尾 1500 文字が `deploys.error`(DB +
  バックアップ)に残る。同 owner にしか見えないが secret の寿命を延ばす。修復:URL/password/token パターンの redaction、
  あるいはログ参照だけ保存し全文は docker logs から権限付きで即時取得。**ログ尾は排障に有用なので redaction の質と要相談**。
- **(Low/既出)viewer login のレート制限**(`admin/viewer.rs`):CLAUDE.md M4 §で既に「否決(後相)」と明記済み。
  現状 bcrypt + 最小長 8 のみ。修復:user/session/IP で滑动窗口 + 失敗退避、失敗を audit、または owner 招待式 viewer grant。
