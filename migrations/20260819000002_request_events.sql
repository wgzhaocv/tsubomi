-- service アクセス統計(doc/paas-service-stats-design.md)。
-- traefik access log(stdout JSON)を server の tailer が追尾し、ここへ batch INSERT する
-- 生イベント表。事前集計はしない(社内規模ではクエリ時 GROUP BY で足りる — 設計 §0-C)。
-- 保留は既定 30 日(gc の housekeeping が DELETE)。
--
-- FK は ON DELETE CASCADE:soft delete(deleted_at)では resources 行が残るので統計も残り、
-- ゴミ箱からの物理 purge(resources 行 DELETE)で自動連鎖する = 掃除コード不要。
-- 在途 INSERT との競合は書き込み側が INSERT..SELECT + WHERE EXISTS で無害化する。
-- ts の CHECK は 20260727000001 と同方針(-infinity 回填事故 2026-07-26 の再発防止)。
-- IP は保存しない:visitor_hash = sha256(UTC日付 || client_ip || user_agent) 先頭 16 バイト
-- (日単位でリセットされる匿名 visitor id — 統計であって追跡ではない。設計 §0-D)。
CREATE TABLE request_events (
    id            bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    service_id    uuid        NOT NULL REFERENCES resources(id) ON DELETE CASCADE,
    ts            timestamptz NOT NULL CHECK (isfinite(ts)),
    method        text        NOT NULL,
    -- クエリ文字列は tailer が保存前に落とす(トークン等の混入防止)+ 512 字切り。
    path          text        NOT NULL,
    status        smallint    NOT NULL,
    duration_ms   integer     NOT NULL,
    visitor_hash  bytea       NOT NULL,
    -- woothee の category から:'desktop' | 'mobile' | 'bot' | 'other'。
    device        text        NOT NULL,
    browser       text,
    os            text,
    -- 前段(CF)が教える時だけ(Cf-Ipcountry、2 字)。CF を外した部署では NULL(設計 §1-7)。
    country       text,
    -- Referer のホスト部だけ(フル URL は保存しない)。
    referer_host  text
);

CREATE INDEX request_events_service_ts ON request_events (service_id, ts);

-- 保留期掃除(gc 1h tick の `DELETE .. WHERE ts < …`)用。時系列追記のみの表なので
-- BRIN が最適(書き込みコストほぼ零・範囲述語に強い)。無いと毎時フルスキャンになる。
CREATE INDEX request_events_ts_brin ON request_events USING brin (ts);
