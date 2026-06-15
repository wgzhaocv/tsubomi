-- tsubomi M5:cache(valkey)の detail テーブル。resources スーパーテーブルに
-- kind='cache' の行がぶら下がり、ここに valkey 固有の期望状態を持つ。
-- 背骨:cache_details が真実源 = valkey の per-cache ACL ユーザは平台が起動時 +
-- 周期で収束させる(揮発。paas-m5-design.md §7.3 / §2)。
--
-- acl_user = namespace = c_<shortid>(同値で足りる。DDL の 2 列は将来の分離余地)。
-- key 隔離は valkey ACL(~<namespace>:* / &<namespace>:*)+ コマンド白名単。
-- password_enc は rotate / restore で原文が要るので復元可能な暗号化(crypto.rs、
-- database パスワードと同じ XChaCha20-Poly1305)。
CREATE TABLE cache_details (
    resource_id  UUID PRIMARY KEY,
    kind         TEXT        NOT NULL DEFAULT 'cache' CHECK (kind = 'cache'),
    acl_user     TEXT        UNIQUE NOT NULL,            -- valkey ACL のログイン名。c_<shortid>
    namespace    TEXT        UNIQUE NOT NULL,            -- key 前缀。~<namespace>:* / &<namespace>:*
    password_enc BYTEA       NOT NULL,                   -- ACL パスワード(復元可能な暗号化)
    rotated_at   TIMESTAMPTZ,                            -- 最後の rotate 時刻(UI の「失効済み」ソフト提示)
    -- kind 付き複合 FK:resources(id, kind) の UNIQUE を参照し、種別の取り違えを型で防ぐ。
    FOREIGN KEY (resource_id, kind) REFERENCES resources(id, kind) ON DELETE CASCADE
);
