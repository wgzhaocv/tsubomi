-- service_details.public を削除する。
--
-- 当初(20260615000001_service.sql)は「public = true で ipAllowList を除外する」意図で
-- 足した列だが、実装が一度も読まなかった(route.rs は常に ipallow middleware を付ける)し、
-- 値を立てる API も無い = 完全な死に列。「public service の能力がある」という誤解だけを生むので
-- 削除する(設計に public service の需要が出たら、その時に列 + API + 監査 + route 分岐を揃えて入れる)。
ALTER TABLE service_details DROP COLUMN IF EXISTS public;
