-- conn_limit の既定を 100 に引き上げる(利用者少数 = 偶の重利用を縛らない方針)。
-- 新規 DB は databases.rs の DEFAULT_CONN_LIMIT(=100)を CREATE ROLE / INSERT で書く。
-- ここでは列の DEFAULT を 100 に揃え(従来 20 で散在していたのを解消)、既存行も 100 へ
-- 底上げする(下げはしない = 将来 owner が個別に上げた値は温存)。
-- 実際の Postgres ロール上限(pg-tenant の CONNECTION LIMIT)はデプロイ時に ALTER ROLE で
-- 揃える(`rolconnlimit=20` の既存テナント役割 = app/human を 100 へ。owner は NOLOGIN で対象外)。
ALTER TABLE database_roles ALTER COLUMN conn_limit SET DEFAULT 100;
UPDATE database_roles SET conn_limit = 100 WHERE conn_limit < 100;
