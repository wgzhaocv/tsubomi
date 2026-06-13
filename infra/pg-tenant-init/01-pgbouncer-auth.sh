#!/bin/bash
# pgbouncer の auth_query 用ロールと、pg_shadow を引く SECURITY DEFINER 関数を作る。
# pgbouncer はクライアント接続毎にこの関数で SCRAM 検証子を取得する ⇒ DB / ロールを
# 動的に作っても pgbouncer の再読込が要らない(rotate も即時に新接続へ反映)。
#
# 初回起動時(空データディレクトリ)のみ実行される。IaC 層 — 平台は触らない(決定 #6)。
# auth_pw は pg-tenant サービスの環境変数。pgbouncer/userlist.txt と一致させること。
set -e

psql -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname postgres \
  --set=auth_pw="${PGBOUNCER_AUTH_PASSWORD:-tsubomi_pgb_dev}" <<-'EOSQL'
  CREATE ROLE pgbouncer_auth LOGIN PASSWORD :'auth_pw';

  -- pgbouncer の auth_user は pg_shadow を直接読めない。SECURITY DEFINER 関数で
  -- 関数所有者(= admin)の権限で 1 ユーザ分のハッシュだけ返す。PUBLIC からは剥奪。
  CREATE OR REPLACE FUNCTION public.pgbouncer_get_auth(p_usename text)
    RETURNS TABLE(usename text, passwd text)
    LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS
  $func$
    SELECT usename::text, passwd::text FROM pg_shadow WHERE usename = p_usename;
  $func$;

  REVOKE ALL ON FUNCTION public.pgbouncer_get_auth(text) FROM PUBLIC;
  GRANT EXECUTE ON FUNCTION public.pgbouncer_get_auth(text) TO pgbouncer_auth;

  -- クロス DB の情報漏洩を塞ぐ:テナントのロールが管理 DB(postgres/template1)に
  -- 連がって他の DB 名 / ロール名を列挙できないようにする。各テナント DB 自体の
  -- 隔離は平台が DB 作成時に REVOKE CONNECT … FROM PUBLIC で行う(§7)。
  -- admin は superuser なので CONNECT 検査を素通りする。auth_query 用に
  -- pgbouncer_auth にだけ postgres への CONNECT を戻す。
  REVOKE CONNECT ON DATABASE postgres FROM PUBLIC;
  REVOKE CONNECT ON DATABASE template1 FROM PUBLIC;
  GRANT CONNECT ON DATABASE postgres TO pgbouncer_auth;
EOSQL
