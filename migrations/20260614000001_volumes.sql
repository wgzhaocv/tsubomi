-- tsubomi M2:volume(ファイルシステム)の detail テーブル。
-- 背骨(tech-design §2):4 種のリソースは resources スーパーテーブル 1 枚 +
-- 種別毎の detail テーブル。M2 では volume の detail を足す。
--
-- volume は顶层リソース(service/database/cache と平級)。各 volume は
-- 独立した假根サンドボックス /srv/tsubomi/volumes/<user_id>/<volume_id>/。
-- 注入(service への mount)は M3 — ここではファイル置き場の実体だけを持つ。

-- ===========================================================================
-- volume_details(resources と 1:1)
--   host_path は volume の假根(物理パス)。display_name(resources)とは別 —
--   リネームは host_path に触れない(M1 database の pg_dbname と同じ規律)。
-- ===========================================================================

CREATE TABLE volume_details (
    resource_id UUID PRIMARY KEY,
    kind        TEXT NOT NULL DEFAULT 'volume' CHECK (kind = 'volume'),
    host_path   TEXT NOT NULL UNIQUE,                 -- /srv/tsubomi/volumes/<user>/<id>
    FOREIGN KEY (resource_id, kind) REFERENCES resources(id, kind) ON DELETE CASCADE
);
