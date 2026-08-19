# service アクセス統計(stats)設計 — Vercel 風の per-service 統計タブ

status: **確定(2026-08-19 ユーザ合意)**。実装時の微修正 2 点:
①FK は張る(`REFERENCES resources(id) ON DELETE CASCADE` — 全 service 子表と同型。soft delete は
resources 行を消さないので保留と両立し、物理 purge で自動連鎖 = 掃除コード不要。在途 INSERT は
INSERT..SELECT + WHERE EXISTS で無害化)②traefik 容器名の env は既存 `TSUBOMI_TRAEFIK_CONTAINER` を
流用(新設しない)。
要望:「URL analytics のような per-service 統計(訪問数・デバイス・ブラウザ・route・IP 等)。
**平台自身の機能**(平台は将来ホスト移転する — CF に依存しない)。計上対象は**平台外部からの
リクエスト**。web は service 詳細に**新タブ**。データ保留は **30 日**」。

## 0. 確定事項

- **A. データ源は traefik access log 一本 — CF 非依存・ユーザ app 無改変**。「traefik が唯一の公開入口」
  という平台自身の不変式に載る(file provider、router 名 = `svc-<id>`)。どのホストへ移転しても、前段が
  CF Tunnel でも直 VPS でも成立する。CF 由来のヘッダ(実 IP / 国)は**任意の増強材料**であって依存ではない。
  Vercel(客端スクリプト + beacon)方式は不採用:平台はユーザコードに触れない原則。
- **B. ログはファイルでなく stdout — 輪転は docker に任せる**。access log を traefik の stdout に JSON で
  出し(traefik の既定出力先)、compose の json-file logging(max-size/max-file)が輪転を持つ。server は
  **bollard logs(follow + timestamps)**で追尾 — W1 流式ログと同じ既存パターン。rename + SIGUSR1 +
  inode 追跡の自前輪転は**作らない**(Vercel の「ログファイルを持たない」の平台版)。
- **C. 生イベント保存 + クエリ時集計、保留 30 日**(`TSUBOMI_STATS_RETENTION_DAYS` 既定 30)。社内規模では
  事前集計バケットは過剰複雑。掃除は既存 gc housekeeping(1h tick)に DELETE 1 本。
- **D. IP は平文で保存しない**。独立訪客 = `sha256(day || client_ip || user_agent)` 先頭 16 バイト
  (Vercel と同じ口径:日単位リセットの匿名 visitor id、cookie 無し)。
- **E. 実 IP の取得は部署トポロジで明示分岐(`TSUBOMI_STATS_IP_SOURCE`)、「ヘッダがあれば使う」自動判定は
  しない**(§2.1 — 偽装対策)。
- **F. 口径は「リクエスト」**(静的資産・API 込み。Vercel の pageview ではない — UI 文案で明示)。
  ただし**訪客系の指標は bot(UA 分類)を除外**して数える(bot 込みの訪客数は無意味)。
- **G. UA 解析は woothee**(純査表・外部データファイル無し)で browser / os / device の 3 分列を出す
  (Vercel 同等の見え方。「簡易 4 分類」では不足 — ユーザ要望に browser 明示)。

## 1. 計上境界(「平台外部」の定義)と受容事項

**計上する** = traefik の `svc-<id>` router を通ったもの。平台の探活は TCP 直連、M6 内部リンクは
docker 私網直連、reconcile / デプロイも traefik 不経由 — **平台内部の営みは構造的に混入しない**。

| # | 事象 | 受容理由 |
|---|------|----------|
| 1 | 租户 app が**公開 URL 経由**で他 app を呼ぶと外部訪問として計上 | 流量は実際に平台を出て戻る = 入口で不可区別。正経の互調は M6 内链に誘導済み |
| 2 | M6 内部リンクは計上されない | 本機能は「公開入口の統計」 |
| 3 | private service はデータ無し | route ファイル無し = traefik に流量が来ない。仕様どおり |
| 4 | 前段(CF 等)がエッジで返した分は計上されない | 源站に届かない。自建統計の一般限界 |
| 5 | bot 判定は UA 自己申告ベース | 偽装 UA は見抜けない |
| 6 | route pattern(`/blog/[slug]` 折叠)は出せない — 実 path の Top N のみ | 框架内部の路由表は代理層に無い(Vercel は框架統合で取る)。app 協力が要る = 境界外 |
| 7 | 国は前段が教える時だけ(CF `Cf-Ipcountry`)。CF を外すと空 | 自前 GeoIP(GeoLite2)は license + 更新の独立運維。列は nullable、後付け可 |
| 8 | 有効化前の過去は遡れない | access log は有効化時点から |
| 9 | server 再起動の境界で数行の重複計上があり得る | 「欠落より重複」(offset は INSERT 成功後に前進)。境界秒の数行のみ |

## 2. データ源:traefik access log → stdout(compose 変更)

`compose.prod.yml` と `infra/docker-compose.yml`(dev)の traefik に追加:

```yaml
command:
  - --accesslog=true
  - --accesslog.format=json
  - --accesslog.fields.defaultmode=drop        # 使う欄だけ keep
  - --accesslog.fields.names.RouterName=keep
  - --accesslog.fields.names.ClientAddr=keep   # peer トポロジではこれが実 client IP
  - --accesslog.fields.names.RequestPath=keep
  - --accesslog.fields.names.RequestMethod=keep
  - --accesslog.fields.names.DownstreamStatus=keep
  - --accesslog.fields.names.Duration=keep
  - --accesslog.fields.names.StartUTC=keep
  - --accesslog.fields.headers.defaultmode=drop
  - --accesslog.fields.headers.names.User-Agent=keep
  - --accesslog.fields.headers.names.Referer=keep
  - --accesslog.fields.headers.names.Cf-Connecting-Ip=keep   # cf トポロジでのみ信用(§2.1)
  - --accesslog.fields.headers.names.Cf-Ipcountry=keep
logging:
  driver: json-file
  options: { max-size: "50m", max-file: "3" }   # 輪転は docker が持つ(§0-B)
```

- 出力先未指定 = stdout(traefik 既定)。traefik 自身のアプリログと同じ流に混ざるが、tailer は
  **JSON parse + `StartUTC`/`DownstreamStatus` 欄の存在**で access log 行だけを選別する(§4)。
- filepath / 挂载 / USR1 は不要。docker logs API は json-file の輪転済みファイルも跨いで読める。

### 2.1 実 client IP — トポロジ明示分岐(偽装対策)

**「ヘッダが在れば使う」は禁止**:直連 VPS では任意クライアントが `Cf-Connecting-Ip` / XFF を自分で
付けられる = 訪客数を投毒できる。CF でこのヘッダが信用できるのは **CF が必ず覆写する**からであって、
ヘッダの存在自体に信用は無い(Vercel にこの問題が無いのは採集点が自社エッジ = 大門口だから。
我々の採集点は CF モードでは大門口ではない)。

- `TSUBOMI_STATS_IP_SOURCE`(既定 `cf`):
  - `cf` — `Cf-Connecting-Ip` ヘッダを採用(CF Tunnel / CF proxy 配下。現本番)。
  - `peer` — `ClientAddr` のみ採用、**一切のヘッダを無視**(直 VPS で traefik が入口)。
- 設定と実データの矛盾(cf なのにヘッダ皆無)は tailer が一度だけ warn(移転後の設定忘れ検知)。

## 3. DDL(migration 1 本、`20260819000002_request_events.sql`)

```sql
CREATE TABLE request_events (
    id            bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    service_id    uuid        NOT NULL REFERENCES resources(id) ON DELETE CASCADE,
    ts            timestamptz NOT NULL CHECK (isfinite(ts)),
    method        text        NOT NULL,
    path          text        NOT NULL,   -- クエリ文字列は tailer が落とす(トークン混入防止)+ 512 字切り
    status        smallint    NOT NULL,
    duration_ms   integer     NOT NULL,
    visitor_hash  bytea       NOT NULL,   -- sha256(day || ip || ua)[..16]
    device        text        NOT NULL,   -- 'desktop' | 'mobile' | 'bot' | 'other'(woothee category)
    browser       text,                   -- woothee name(不明は NULL)
    os            text,                   -- woothee os(不明は NULL)
    country       text,                   -- CF-IPCountry(2 字)。無ければ NULL(§1-7)
    referer_host  text                    -- Referer のホスト部だけ(フル URL は保存しない)
);
CREATE INDEX request_events_service_ts ON request_events (service_id, ts);
```

## 4. tailer(新 `crates/server/src/stats.rs`)

- 起動時 spawn。bollard `logs`(container = `TSUBOMI_TRAEFIK_CONTAINER` 既定 `tsubomi-traefik`、
  follow + timestamps + since)で追尾。容器不在 / 切断は backoff 付き再接続(dev で traefik 未起動でも
  server は動く — warn のみ)。
- offset = **docker が行に付けるタイムスタンプ**を platform_config `stats_tail_since` に永続化
  (batch INSERT 成功後に前進 = 「欠落より重複」§1-9)。
- 行選別:JSON parse 成功 + `StartUTC`・`DownstreamStatus` 存在 + `RouterName` が `svc-<uuid>@file` 形
  (逆関数はテスト付き、接尾辞をハードコードしない)。catch-all / registry / apex / アプリログは捨てる。
  parse 失敗はカウントだけして黙って捨てる(ログ形式変化で全体を止めない)。
- 5s または 500 行でまとめて batch INSERT(UNNEST 一括)。
- 掃除:gc housekeeping(1h)で `DELETE FROM request_events WHERE ts < now() - interval '30 days'`
  (`TSUBOMI_STATS_RETENTION_DAYS`)。service purge 時は trash 経路で service_id の行も DELETE。

## 5. API / 入口

- `GET /api/services/{id}/stats?days=7`(1–30、既定 7。`ensure_owned`、Bearer/session 両対応)。
  **1 端点 1 応答**(AI が 1 コールで全体像):
  時系列(hour/day 自動:≤2 日は hour)+ totals(requests / visitors / bot_requests / avg_duration)+
  Top10 × {path, status(類別), device, browser, os, country, referer_host}。
  visitors は bot 除外(§0-F)、DTO は shared crate(CLI と共用)。
- web:service 詳細に**新タブ「統計」**(概要/デプロイ/環境変数/ログ/ターミナル の並びに追加)。
  推移チャート + 内訳リスト。
- CLI:`tbm service stats <name> [--days N]` — json は DTO そのまま(AI フレンドリ規約)。

## 5.5 本番反映の 2 つの罠(codex 審査 2026-08-20)

1. **overlay の command は全置換**:`compose.prod.{tls,registry-direct,db-public}.yml` は traefik の
   `command:` を丸ごと再定義するので、base にだけ accesslog を書くと **overlay 部署では消えて統計が
   静かにゼロ**になる。3 枚全部に再掲済み — 以後 accesslog 設定を変えるときは base + 3 overlay +
   dev infra の **5 箇所**を揃えること。
2. **`just ship` は traefik を再作成しない**(`up -d --no-recreate` + server 単換 = 無瞬断の代償)。
   **既存機での初回上線時だけ**、明示的に traefik を上げ直す(**自分の部署が使っている overlay の組**を
   全部 `-f` に載せること — 組が欠けると別の設定に巻き戻る):
   `docker compose --env-file .env.production -f compose.prod.yml -f <使用中の overlay…> up -d traefik`
   (数秒の全 app 瞬断が出るので深夜帯推奨。以後の ship では不要。**新規 VPS への部署では不要** —
   最初から accesslog 入りの compose で作成されるため)。

## 6. 地雷

1. **IP ソースの自動判定をしたくなる誘惑**(§2.1):ヘッダ存在で分岐すると偽装口が開く。分岐は設定のみ。
2. **RouterName の接尾辞**:`@file` を仮定でベタ書きしない(provider 名は理論上変わり得る —
   `svc-<uuid>@` までで判定し uuid を取る)。
3. **Duration の単位**:traefik JSON の Duration は**ナノ秒**。ms への換算を間違えると全指標が 10^6 倍。
4. **path の暴発**:攻撃スキャンで distinct 爆発 — Top N は SQL GROUP BY で問題ないが、web 表示は
   エスケープ表示(見た目破壊対策)。クエリ文字列は保存前に落とす。
5. **visitor_hash の day 境界**:day は **UTC 日付**で固定(JST にすると CF/Vercel と口径がずれる上、
   サーバの TZ 依存になる)。
6. **docker logs の since 境界**:境界秒の重複は受容(§1-9)。境界判定は「探索中のみ ts < saved を
   捨てる」— docker の行 ts は厳密単調でないため、境界通過後は比較しない(同時刻の未処理行を
   誤って捨てない。codex 審査)。
7. **16KiB 超の 1 行**は docker が partial 分割する — tailer は \n 終端まで継ぎ足してから parse
   (上限 256KiB)。
8. **保留日数は起動時に 1〜3650 を強制**(0 = 全削除・巨大値 = i32 で負化けの事故防止)。days の
   上限超過は 400 でなく実効値へ丸め、応答の days/from/to が真実を言う。

## 7. 見送り

- 事前集計(小時バケット):量が要求するまで。
- 自前 GeoIP(GeoLite2)/ route pattern / pageview 口径(opt-in beacon スクリプト + 摂取端点 = 阶段 2 候補)。
- CF GraphQL Analytics API:CF 依存 + zone 級のみ。平台移転方針と正面衝突。
- owner 跨ユーザ統計:必要になったら同じ表から作れる。
